use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{Emitter, Manager};

use super::message::Message;
use super::tools::ToolDefinition;
use crate::AppState;

/// 奶蛙主动冒泡时用的提示词。
///
/// 关键点：
/// 1. **明确告诉它「可以闭嘴」，但沉默不是默认选项**——不给这个出口，
///    模型每次心跳都会硬挤出一句话，很快就被用户关掉了。
/// 2. **给它具体可干的事**（看看自选股），不然它面对空场景只会选择沉默。
const SPONTANEOUS_PROMPT: &str = r#"你刚刚自己想主动跟用户说句话，不是被问的。

你可以先看看自己的自选股今天怎么样了，有想说的就说一句。
一句话，**不超过 25 个字**，中文，口语。可以：
- 说说你自选股里的某只今天怎么样了
- 问用户一个轻松的小问题
- 抱怨一下你等太久了
- 就单纯表达一下此刻的心情

沉默不是默认选项。只有在你确实没话说、而且说了也是废话的时候，
才调用 stay_quiet。不要说「有什么可以帮您」这种客服话，不要说「在的，请说」。
"#;

/// 「保持安静」工具。只有这一个选项时，模型要么说点什么，要么闭嘴。
pub fn stay_quiet_definition() -> ToolDefinition {
    ToolDefinition::new(
        "stay_quiet",
        "只有在真的完全没想法、说了也是废话的时候才调用它。默认应该说点什么。",
        json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    )
}

/// 调整一下心情。
///
/// 抽成同步函数是因为：在 async 块里直接 `app.state::<AppState>()` 拿借用，
/// 编译器会认为它跨了 await 点而拒绝。同步函数里没有这个问题。
fn tweak_mood(app: &tauri::AppHandle, change: impl FnOnce(&mut super::persona::Mood)) {
    let mood = app.state::<AppState>().mood.clone();
    let mut guard = match mood.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    change(&mut guard);
}

/// 调整心情并落盘。
fn tweak_and_persist_mood(app: &tauri::AppHandle, change: impl FnOnce(&mut super::persona::Mood)) {
    tweak_mood(app, change);
    let _ = app.state::<AppState>().persist_mood();
}

/// 心跳：让奶蛙每隔一阵自己醒过来，决定要不要说句话。
///
/// 频率刻意压得低（默认 3~9 分钟一轮，且只有心情和冷却都允许时才真的开口），
/// 桌宠主动找人说话这件事，**太勤快就变成骚扰了**。
///
/// 三个环境变量可以覆盖节奏，方便调试：
/// `NAIWA_HEARTBEAT_STARTUP_SECS` / `NAIWA_HEARTBEAT_SECS` / `NAIWA_HEARTBEAT_COOLDOWN_SECS`
pub fn spawn_heartbeat(app: tauri::AppHandle) {
    let startup_delay = env_secs("NAIWA_HEARTBEAT_STARTUP_SECS", 90);
    let interval = env_secs("NAIWA_HEARTBEAT_SECS", 180);
    let cooldown = env_secs("NAIWA_HEARTBEAT_COOLDOWN_SECS", 20 * 60);

    tauri::async_runtime::spawn(async move {
        // 启动后先安静一会儿，别刚开机就聒噪。
        tokio::time::sleep(Duration::from_secs(startup_delay)).await;

        let mut last_spoke_at = Instant::now();
        let mut idle_ticks: u32 = 0;

        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            idle_ticks += 1;

            // 情绪随时间流逝缓慢变化。
            tweak_and_persist_mood(&app, |mood| {
                mood.decay(3.0);
                mood.apply_time_of_day(current_hour());
            });

            // 冷却：一段时间内不重复主动开口。
            if last_spoke_at.elapsed() < Duration::from_secs(cooldown) {
                eprintln!(
                    "[heartbeat] 冷却中，剩 {}s",
                    cooldown - last_spoke_at.elapsed().as_secs()
                );
                continue;
            }

            // 至少闲置两轮再说，太快冒泡会烦人。
            if idle_ticks < 2 {
                eprintln!("[heartbeat] 才闲置 {idle_ticks} 轮，再等等");
                continue;
            }

            // 困了就不折腾了。
            let energy = {
                let mood = app.state::<AppState>().mood.clone();
                let guard = match mood.lock() {
                    Ok(guard) => guard,
                    Err(_) => continue,
                };
                guard.energy
            };
            if energy <= 0.25 {
                eprintln!("[heartbeat] 太困了（energy={energy:.2}），不打扰");
                continue;
            }

            // 概率收敛：越久没人理，越可能冒泡（黏人属性）。
            let chance = if idle_ticks > 8 { 0.75 } else { 0.4 };
            eprintln!(
                "[heartbeat] tick={idle_ticks} energy={energy:.2} chance={chance:.2} elapsed={}s",
                last_spoke_at.elapsed().as_secs()
            );
            if pseudo_random() > chance {
                eprintln!("[heartbeat] 这轮没抽中，跳过");
                continue;
            }

            let Some(text) = request_spontaneous(&app).await else {
                continue;
            };

            last_spoke_at = Instant::now();
            idle_ticks = 0;

            // 聊到高兴了，好奇心满足一下。
            tweak_mood(&app, |mood| {
                mood.on_spontaneous();
            });
            let _ = app.emit("pet-spontaneous", text);
        }
    });
}

/// 让模型自己想一句话出来。返回 None 表示它选择安静。
///
/// 允许它先查一下自选股再说话——给一件具体可干的事，
/// 否则面对空场景它大概率只会选沉默。
async fn request_spontaneous(app: &tauri::AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let client = state.client.read().await;

    let mut messages = vec![
        Message::System {
            content: SPONTANEOUS_PROMPT.to_string(),
        },
        Message::User {
            content: "（心跳）现在你想跟用户说点什么吗？".to_string(),
        },
    ];

    let tools = vec![
        stay_quiet_definition(),
        super::tools::express::ExpressTool::definition(),
        super::tools::watchlist::list_watchlist_definition(),
        super::tools::stock::quote_definition(),
    ];

    // 最多三轮：模型可能先查自选股、再想动作、最后才开口，
    // 轮数太少会把「还没说完」截断成「没说话」。
    for round in 0..3 {
        let turn = client.chat_turn(&messages, &tools).await.ok()?;

        eprintln!(
            "[heartbeat] 第{round}轮 工具={:?} 内容={:?}",
            turn.tool_calls
                .iter()
                .map(|call| call.function.name.clone())
                .collect::<Vec<_>>(),
            turn.content.chars().take(50).collect::<String>()
        );

        if turn.tool_calls.is_empty() {
            let text = turn.content.trim().to_string();
            if text.is_empty() {
                eprintln!("[heartbeat] 这轮没吐字，算它沉默");
                return None;
            }
            eprintln!("[heartbeat] 它说：{text}");
            return Some(text);
        }

        messages.push(Message::Assistant {
            content: if turn.content.is_empty() {
                None
            } else {
                Some(turn.content.clone())
            },
            tool_calls: turn.tool_calls.clone(),
        });

        for call in &turn.tool_calls {
            match call.function.name.as_str() {
                "stay_quiet" => {
                    eprintln!("[heartbeat] 模型选择保持安静");
                    return None;
                }
                "express" => {
                    // 它想边说边做个表情，把动作转发给桌宠窗口去播。
                    let args: serde_json::Value =
                        serde_json::from_str(&call.function.arguments).unwrap_or(json!({}));
                    let action = args
                        .get("action")
                        .and_then(|value| value.as_str())
                        .unwrap_or("laugh")
                        .to_string();
                    let reason = args
                        .get("reason")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                    let _ = app.emit("pet-animate", json!({ "action": action, "reason": reason }));
                }
                _ => {
                    // 白名单：模型可能会幻觉出没在 tools 列表里的工具名，
                    // 而主 registry 里注册了 add_watchlist / remove_watchlist，
                    // 不能让它在心跳里改用户自选股。
                    const HEARTBEAT_ALLOWED: [&str; 2] = ["list_watchlist", "get_quote"];
                    if !HEARTBEAT_ALLOWED.contains(&call.function.name.as_str()) {
                        eprintln!("[heartbeat] 拒绝调用未授权工具 {}", call.function.name);
                        continue;
                    }

                    // 查自选股 / 查行情，拿到结果喂回去。
                    let args: serde_json::Value =
                        serde_json::from_str(&call.function.arguments).unwrap_or(json!({}));
                    let result = state
                        .registry
                        .read()
                        .await
                        .call(&call.function.name, args)
                        .await
                        .unwrap_or_else(|err| format!("（查询失败：{err}）"));
                    messages.push(Message::tool(call.id.clone(), result));
                }
            }
        }
    }

    None
}

/// 读一个秒数配置，没设就用默认值。
fn env_secs(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// 当前小时（本地时间）。
///
/// `NAIWA_FAKE_HOUR` 可以强制指定，方便在深夜调试白天的行为。
fn current_hour() -> u32 {
    if let Ok(fake) = std::env::var("NAIWA_FAKE_HOUR") {
        if let Ok(hour) = fake.trim().parse::<u32>() {
            if hour < 24 {
                return hour;
            }
        }
    }

    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| {
            // UTC 小时 + 东八区偏移，粗略够用。
            let hours = d.as_secs() / 3600 + 8;
            (hours % 24) as u32
        })
        .unwrap_or(12)
}

/// 一个不需要额外依赖的伪随机数（0.0~1.0）。
///
/// 乘法必须用 `wrapping_mul`：这里乘的是 6.4e18 的常数，
/// 纳秒时间戳乘上去远超 u64 范围，普通 `*` 在 debug 模式下会 panic，
/// 直接把整个心跳任务干掉（release 模式才只是静默回绕，更难查）。
fn pseudo_random() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let mixed = nanos
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    // 取高 31 位，映射到 0.0~1.0
    ((mixed >> 33) as f64) / ((1u64 << 31) as f64)
}
