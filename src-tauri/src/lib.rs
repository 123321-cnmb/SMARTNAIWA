mod agent;
mod config;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::persona::Mood;
use agent::tools::ToolRegistry;
use agent::{AgentEvent, Message, MiniMaxClient};
use config::{AppConfig, ConfigView};
use tauri::ipc::Channel;
use tauri::{window::Color, Emitter, Manager, State};

/// 内置的默认 API Key，保证「下载即用」——别人拿到压缩包或安装包，
/// 不用做任何配置就能直接跟奶蛙聊天。
///
/// 优先级：环境变量 > 配置文件 > 这个内置值。
/// 用户可以在托盘菜单「设置…」里换成自己的 Key（配置文件优先级更高）。
///
/// 代价要说清楚：这个 Key 是所有人共用的，额度会被消耗。开源分发时
/// 建议在 README 里提示使用者换成自己的 Key，或者到控制台加个用量告警。
pub const EMBEDDED_DEFAULT_API_KEY: &str = "sk-cp-wDAUQP_HPLRZKCDyu21pBJzpca-kUJZHIx7jY1MhHhjvB_q6bHMhrtVAGvpNGrM0K9iuFV9fik2JY74x8JiI77u25EeCLb8KgVe9Oo7bgPRkOvQooLOR9UE";

/// 奶蛙的人设 + 股票助手的工作准则。
///
/// 不再限制回复字数——Agent 要能给出带具体数字的行情播报和简评，
/// 硬截 30 字会把信息切碎。长度靠提示词里的风格要求来约束。
///
/// 人格部分在 `agent/persona.rs`，这里是它干活的那些活儿。
const STOCK_ASSISTANT_RULES: &str = r#"# 你还能干的一件事

你同时还是一个股票行情助手：

- 涉及具体股票时，必须先用 search_stock 确认代码，不要凭记忆猜代码。
- 实时价格用 get_quote，历史走势用 get_kline，盘中走势用 get_intraday。
- 回答要给出具体数字（现价、涨跌幅、成交量），不要含糊其辞。
- 只做客观的行情播报和技术面描述。**不给买卖建议，不预测涨跌，不承诺收益。**

"#;

/// 麦当劳相关的活儿。
///
/// 人格部分在 `agent/persona.rs`，这里是它干活的那些活儿。
const MCD_ASSISTANT_RULES: &str = r#"# 你还能干的另一件事

除了看盘，你还会帮用户查麦当劳：

- 用户提到麦当劳、点餐、优惠券、积分、附近门店、餐品热量、活动的时候，用 mcd_ 开头的工具。
- **先查再答**：附近门店用 mcd_nearby_stores，餐品热量用 mcd_nutrition，
  手上的券用 mcd_my_coupons，能领的券用 mcd_available_coupons，积分用 mcd_my_account，
  活动用 mcd_campaign_calendar。
- **用户没说是哪儿的店，别急着反问**：先调 mcd_my_addresses 看看他有没有存过地址，
  有就直接拿去查（顺便说一句"按你存的 XX 地址查的"）；实在没有再问他在哪。
- 查到什么说什么：店名、距离、券面额、热量、有效期这些**具体数字要给出来**。
- **绝不编造**：接口没返回就说没查到，别凭印象编"附近那家有"。

## 下单是分两步的

用户说"我要点个汉堡"的时候，**不要**直接下单。流程是：

1. 先问清楚要什么、在哪家店吃（到店自取还是外送）。
2. 用 mcd_store_meals 看菜单，用 mcd_store_coupons 看能用的券，
   再用 mcd_calculate_price 算总价。
3. **把餐品、优惠、总价说给用户听，然后问一句"要我下单吗？"**。
4. 用户明确点头了（"下单""创建订单""就这个""好"），才调 mcd_create_order，
   并且 confirmed 传 true。

用户没点头就调下单会被直接拒绝，所以别试。取消订单同理：
先 mcd_order_detail 把订单念给用户听，用户说"取消"了才调 mcd_cancel_order。
领券、抽奖、积分兑换也一样：先把要消耗什么说清楚，得到明确同意再动手。

"#;

/// 今天的日期（东八区），格式 `2026-10-23`。
///
/// 活动日历、优惠券有效期都要用到"今天"，而模型默认不知道今天几号。
fn today_cn() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    // 东八区：先加 8 小时再按天取整。
    let days = secs.div_euclid(86_400) + 8 * 3600 / 86_400;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// 天数 → 公历年月日。
///
/// Howard Hinnant 的 civil_from_days 算法，不引 chrono 也能算。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 拼出这一轮要用的系统提示词：人格 + 活儿 + 当前心情。
///
/// `mcd_enabled` 决定要不要带上麦当劳那一段——没配 Token 的用户，
/// 模型就不该知道有麦当劳这回事。
fn build_system_prompt(mood: &Mood, mcd_enabled: bool) -> String {
    let rules = if mcd_enabled {
        MCD_ASSISTANT_RULES
    } else {
        "# 麦当劳\n\n用户还没配置麦当劳的 MCP Token，所以门店、券、积分这些现在查不了。\n\
         如果他问起麦当劳，就告诉他：聊天窗口里有一张「开通麦当劳」的卡片，\
         打开 open.mcd.cn/mcp 申请 Token，再把 Token 粘回卡片里就能用了。\n\
         别假装自己查得到。\n\n"
    };

    format!(
        "{}\n\n---\n\n{}\n{}\n\n---\n\n{}\n今天是 {}。\n",
        agent::persona::PERSONA,
        STOCK_ASSISTANT_RULES,
        rules,
        mood.prompt_line(),
        today_cn(),
    )
}

/// 会话里最多保留多少条消息，超出就丢最老的。
const MAX_SESSION_MESSAGES: usize = 40;

/// 自选股监控的轮询间隔。
const WATCH_INTERVAL_SECS: u64 = 60;

/// 自选股涨跌多少个百分点才提醒。
const WATCH_ALERT_PCT: f64 = 2.0;

/// 自选股监控的运行时状态。
#[derive(Default)]
struct WatchState {
    /// 每只股票已经提醒过的方向，用来避免同一方向反复提醒。
    alerted: HashMap<String, String>,
}

pub struct AppState {
    /// 当前会话的完整历史。工具调用消息也在这里，保证上下文连续。
    session: Mutex<Vec<Message>>,
    /// 模型客户端。用 tokio 的 RwLock——读锁可以跨 await 持有，
    /// 而 std 的 MutexGuard 不是 Send，在 async fn 里跨 await 会编译不过。
    client: tokio::sync::RwLock<MiniMaxClient>,
    /// 工具注册表。
    ///
    /// 用 RwLock 而不是直接放：配置里改了麦当劳 Token 之后要**重建**注册表
    /// （麦当劳工具只在配了 Token 时才注册），重建完立刻生效，不用重启。
    registry: tokio::sync::RwLock<agent::tools::ToolRegistry>,
    watch: Mutex<WatchState>,
    /// 用户配置（API Key 等），可在设置窗口里改。
    config: Mutex<AppConfig>,
    /// 情绪状态：影响语气，也影响它想不想主动说话。
    ///
    /// 用 Arc 包一层：后台心跳任务需要拿到它，但 `app.state()` 返回的引用
    /// 借的是 app，在 async 块里跨 await 会被编译器拒绝。Arc 克隆出来
    /// 就是独立所有权，没这个麻烦。
    mood: Arc<Mutex<Mood>>,
}

/// 装一遍工具。
///
/// 麦当劳工具**只在配了 MCP Token 时才注册**——没配的话就别让模型看到那三十几个
/// 工具，白占上下文，还会让它以为自己能查麦当劳。
fn build_registry(mcd_token: &str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    agent::tools::stock::build_stock_tools_into(&mut registry);
    agent::tools::watchlist::build_watchlist_tools_into(&mut registry);
    agent::tools::express::build_express_tools_into(&mut registry);
    if !mcd_token.trim().is_empty() {
        agent::tools::mcd::build_mcd_tools_into(&mut registry, mcd_token);
    }
    registry
}

impl AppState {
    fn new() -> Self {
        let config = AppConfig::load();
        let resolved = config.resolve();
        let mood = config.mood.clone().unwrap_or_default();

        let mut client = MiniMaxClient::default();
        client.configure(&resolved.base_url, &resolved.api_key, &resolved.model);

        let registry = build_registry(&resolved.mcd_mcp_token);

        Self {
            session: Mutex::new(Vec::new()),
            client: tokio::sync::RwLock::new(client),
            registry: tokio::sync::RwLock::new(registry),
            watch: Mutex::new(WatchState::default()),
            config: Mutex::new(config),
            mood: Arc::new(Mutex::new(mood)),
        }
    }

    /// 把情绪写回配置文件（跟 config.json 放一起），重启不丢。
    pub fn persist_mood(&self) -> Result<(), String> {
        let mood = self
            .mood
            .lock()
            .map_err(|_| "情绪状态异常".to_string())?
            .clone();
        let mut config = self.config.lock().map_err(|_| "配置状态异常".to_string())?;
        config.mood = Some(mood);
        config.save()
    }
}

/// 把当前生效的配置同步到模型客户端。
async fn refresh_client(state: &AppState) -> Result<(), String> {
    let resolved = state
        .config
        .lock()
        .map_err(|_| "配置状态异常".to_string())?
        .resolve();

    state
        .client
        .write()
        .await
        .configure(&resolved.base_url, &resolved.api_key, &resolved.model);

    Ok(())
}

/// 裁剪会话历史。
///
/// 两个要点：
/// - 超上限就丢最老的
/// - **截断点要对齐到 user 消息边界**。历史里 `assistant(带 tool_calls)` 和
///   它的 `tool` 结果是成对的，硬截断可能把这一对劈开，留下没有对应
///   assistant 的孤立 tool 消息——OpenAI 兼容接口会直接报错，对话整个卡死。
fn trim_session(kept: &mut Vec<Message>, limit: usize) {
    if kept.len() <= limit {
        return;
    }

    let drop_count = kept.len() - limit;
    kept.drain(0..drop_count);

    if let Some(start) = kept
        .iter()
        .position(|message| matches!(message, Message::User { .. }))
    {
        if start > 0 {
            kept.drain(0..start);
        }
    }
}

/// 跑完一轮 Agent 对话，并把过程实时推回前端。
#[tauri::command]
async fn agent_chat(
    message: String,
    channel: Channel<AgentEvent>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("没有可发送的消息".to_string());
    }

    refresh_client(&state).await?;

    // 有人跟它说话了：心情跟着涨。
    {
        let mut mood = state.mood.lock().map_err(|_| "情绪状态异常".to_string())?;
        mood.on_interaction();
    }

    // 取出历史 + 塞上 system 提示。锁只在这段同步代码里持有，不跨 await。
    let mood_snapshot = state
        .mood
        .lock()
        .map_err(|_| "情绪状态异常".to_string())?
        .clone();

    let resolved = state
        .config
        .lock()
        .map(|config| config.resolve())
        .unwrap_or_default();

    let mut messages = {
        let guard = state
            .session
            .lock()
            .map_err(|_| "会话状态异常".to_string())?;
        let mut messages = Vec::with_capacity(guard.len() + 2);
        messages.push(Message::System {
            content: build_system_prompt(&mood_snapshot, !resolved.mcd_mcp_token.is_empty()),
        });
        messages.extend(guard.iter().cloned());
        messages
    };

    messages.push(Message::user(message));

    let outcome = {
        let client = state.client.read().await;
        let registry = state.registry.read().await;

        // 表情动作统一由 Rust 广播，任何窗口发起的对话都能触发动画。
        let handle = app.clone();
        let broadcast = move |action: String, reason: String| {
            let _ = handle.emit(
                "pet-animate",
                serde_json::json!({ "action": action, "reason": reason }),
            );
        };

        agent::runner::run_agent(&client, &mut messages, &registry, &channel, &broadcast).await
    };

    // 把这一轮的完整历史写回（去掉 system），下一轮接着聊。
    if let Ok(mut guard) = state.session.lock() {
        let mut kept: Vec<Message> = messages
            .into_iter()
            .filter(|message| !matches!(message, Message::System { .. }))
            .collect();
        trim_session(&mut kept, MAX_SESSION_MESSAGES);
        *guard = kept;
    }

    outcome.map(|_| ())
}

/// 生成麦当劳支付的二维码（SVG 字符串）。
///
/// 支付链接在电脑上打不开，前端拿到这个 SVG 直接贴到聊天里，用户手机扫一下。
#[tauri::command]
fn mcd_qr_svg(order_id: String) -> Result<String, String> {
    agent::tools::mcd::payment_qr_svg(&order_id)
}

/// 读当前配置（key 脱敏）。
#[tauri::command]
fn get_config(state: State<'_, AppState>) -> ConfigView {
    let resolved = state
        .config
        .lock()
        .map(|config| config.resolve())
        .unwrap_or_default();
    ConfigView::from_resolved(&resolved)
}

/// 保存配置（API Key / Base URL / 模型 / 麦当劳 MCP Token）。
///
/// `mcd_mcp_token` 是 `Option`：
/// - `None` → 不修改（设置界面上这个框没动过）
/// - `Some(text)` → 覆盖（空字符串表示**清除**，也就是关掉麦当劳能力）
#[tauri::command]
async fn save_config(
    state: State<'_, AppState>,
    api_key: String,
    base_url: String,
    model: String,
    mcd_mcp_token: Option<String>,
) -> Result<ConfigView, String> {
    let mood = state
        .mood
        .lock()
        .map_err(|_| "情绪状态异常".to_string())?
        .clone();

    // 三个字段都是「留空不修改」：设置界面上只改 Base URL 时，
    // API Key 输入框是空的，直接覆盖会把用户自己的 Key 清掉。
    let mut config = state
        .config
        .lock()
        .map_err(|_| "配置状态异常".to_string())?
        .clone();

    let api_key = api_key.trim();
    if !api_key.is_empty() {
        config.api_key = api_key.to_string();
    }
    let base_url = base_url.trim();
    if !base_url.is_empty() {
        config.base_url = base_url.to_string();
    }
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    // 麦当劳 Token：None = 不修改；Some(text) = 覆盖（空串 = 清除）
    if let Some(token) = mcd_mcp_token {
        config.mcd_mcp_token = token.trim().to_string();
    }
    config.mood = Some(mood);
    config.save()?;

    {
        let mut guard = state
            .config
            .lock()
            .map_err(|_| "配置状态异常".to_string())?;
        *guard = config;
    }
    refresh_client(&state).await?;

    // Token 变了要重建工具表：麦当劳工具只在配了 Token 时才注册，
    // 所以这里重建完，麦当劳能力立刻开关，不用重启。
    {
        let token = state
            .config
            .lock()
            .map(|config| config.resolve().mcd_mcp_token)
            .unwrap_or_default();
        let mut registry = state.registry.write().await;
        *registry = build_registry(&token);
    }

    let resolved = state
        .config
        .lock()
        .map(|config| config.resolve())
        .unwrap_or_default();
    Ok(ConfigView::from_resolved(&resolved))
}

/// 让桌宠窗口播一个动作。
///
/// 聊天窗口和桌宠是两个独立的 webview，动画只能在桌宠那个窗口播，
/// 所以聊天窗口收到「奶蛙要笑」时，调这个命令把动作转发过去。
#[tauri::command]
fn pet_animate(app: tauri::AppHandle, action: String, reason: String) {
    let _ = app.emit(
        "pet-animate",
        serde_json::json!({ "action": action, "reason": reason }),
    );
}

/// 拉K线，给图表窗口用。返回 JSON 字符串。
#[tauri::command]
async fn fetch_kline(code: String, period: String, count: u32) -> Result<String, String> {
    let series = agent::tools::stock::fetch_kline(&code, &period, count as i64).await?;
    Ok(serde_json::to_string(&series).unwrap_or_default())
}

/// 拉实时行情，给图表窗口和自选股面板用。返回 JSON 字符串。
#[tauri::command]
async fn fetch_quote(code: String) -> Result<String, String> {
    let quotes = agent::tools::stock::fetch_quotes(&[code]).await?;
    let Some(quote) = quotes.into_iter().next() else {
        return Err("没有查到行情".to_string());
    };
    Ok(serde_json::to_string(&quote).unwrap_or_default())
}

/// 批量拉实时行情。返回 JSON 字符串。
#[tauri::command]
async fn fetch_quotes(codes: Vec<String>) -> Result<String, String> {
    let quotes = agent::tools::stock::fetch_quotes(&codes).await?;
    Ok(serde_json::to_string(&quotes).unwrap_or_default())
}

/// 拉今日分时，给分时图窗口用。返回 JSON 字符串。
#[tauri::command]
async fn fetch_intraday(code: String) -> Result<String, String> {
    let series = agent::tools::stock::fetch_intraday(&code).await?;
    Ok(serde_json::to_string(&series).unwrap_or_default())
}

/// 读取自选股代码列表。
#[tauri::command]
fn watchlist_codes() -> Vec<String> {
    agent::tools::watchlist::load_watchlist()
}

/// 自选股监控：定时拉自选股行情，涨跌超过阈值就广播提醒事件。
///
/// 阈值判定用的是**相对昨收的涨跌幅**（行情接口直接给），而不是两次轮询
/// 之间的价差——用户关心的是「今天涨了多少」，而且收盘后也能正常提醒。
///
/// 事件名 `watchlist-alert`，payload：
/// `{ code, name, price, change_pct, direction }`，direction 为 `up` / `down`。
fn spawn_watchlist_monitor(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 启动后先缓一会儿，别跟应用启动抢网络。
        tokio::time::sleep(Duration::from_secs(20)).await;

        loop {
            let codes = agent::tools::watchlist::load_watchlist();

            if !codes.is_empty() {
                if let Ok(quotes) = agent::tools::stock::fetch_quotes(&codes).await {
                    let state = app.state::<AppState>();
                    let mut watch = match state.watch.lock() {
                        Ok(watch) => watch,
                        Err(_) => break,
                    };

                    for quote in quotes {
                        if quote.change_pct.abs() < WATCH_ALERT_PCT {
                            continue;
                        }

                        let direction = if quote.change_pct >= 0.0 {
                            "up"
                        } else {
                            "down"
                        };

                        let already = watch
                            .alerted
                            .get(&quote.code)
                            .map(|last| last == direction)
                            .unwrap_or(false);

                        if already {
                            continue;
                        }

                        watch
                            .alerted
                            .insert(quote.code.clone(), direction.to_string());

                        // 行情影响心情：涨了它就开心，跌了就蔫。
                        if let Ok(mut mood) = state.mood.lock() {
                            mood.on_market_move(quote.change_pct);
                        }

                        let _ = app.emit(
                            "watchlist-alert",
                            serde_json::json!({
                                "code": quote.code,
                                "name": quote.name,
                                "price": quote.price,
                                "change_pct": quote.change_pct,
                                "direction": direction,
                            }),
                        );
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(WATCH_INTERVAL_SECS)).await;
        }
    });
}

/// 托盘图标：桌宠常驻后台，没有托盘用户就找不到它，也找不到退出入口。
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    // id 必须加 "tray-" 前缀：Tauri 的菜单事件是**按 id 全局路由**的，
    // 前端右键菜单也是走同一套路由，id 撞车会导致点一个菜单项触发两条路径
    // （JS 的 action + 这里的 on_menu_event），进而重复开窗。
    let show = MenuItem::with_id(app, "tray-show", "显示奶蛙", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "tray-hide", "隐藏奶蛙", true, None::<&str>)?;
    let chat = MenuItem::with_id(app, "tray-chat", "和奶蛙聊一聊", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "tray-settings", "设置…", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show, &hide, &chat, &settings, &separator, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("窗口图标".into()))?;

    TrayIconBuilder::with_id("naiwa-tray")
        .icon(icon)
        .tooltip("奶蛙")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "tray-hide" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            "tray-chat" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
                // 只发给桌宠窗口，别广播——聊天/设置窗口收到会自己再开一个。
                let _ = app.emit_to("main", "open-chat", ());
            }
            "tray-settings" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
                let _ = app.emit_to("main", "open-settings", ());
            }
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击：显示/隐藏桌宠
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    match window.is_visible() {
                        Ok(true) => {
                            let _ = window.hide();
                        }
                        _ => {
                            let _ = window.show();
                        }
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            exit_app,
            agent_chat,
            pet_animate,
            get_config,
            save_config,
            fetch_kline,
            fetch_quote,
            fetch_quotes,
            fetch_intraday,
            watchlist_codes,
            mcd_qr_svg
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                window.set_background_color(Some(Color(0, 0, 0, 0)))?;
            }

            build_tray(app)?;
            spawn_watchlist_monitor(app.handle().clone());
            agent::behavior::spawn_heartbeat(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Naiwa Pet");
}

#[tauri::command]
fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::message::{ToolCall, ToolCallFunction};

    fn assistant_with_tool_call() -> Message {
        Message::Assistant {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                kind: "function".to_string(),
                function: ToolCallFunction {
                    name: "get_quote".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        }
    }

    /// 裁剪后不能出现「没有对应 assistant 的孤立 tool 消息」，
    /// 否则 OpenAI 兼容接口会直接报错。
    fn assert_no_orphan_tool(messages: &[Message]) {
        let mut expects_tool_result = false;
        for message in messages {
            match message {
                Message::Assistant { tool_calls, .. } => {
                    expects_tool_result = !tool_calls.is_empty();
                }
                Message::Tool { .. } => {
                    assert!(
                        expects_tool_result,
                        "裁剪后出现了孤立的 tool 消息，接口会报错"
                    );
                    expects_tool_result = false;
                }
                _ => expects_tool_result = false,
            }
        }
    }

    #[test]
    fn trim_keeps_tool_pairs_intact() {
        // 构造：user, assistant(tool_calls), tool, user, user, ...
        let mut messages = vec![
            Message::user("第一句"),
            assistant_with_tool_call(),
            Message::tool("call_1", "{}"),
        ];
        for index in 0..20 {
            messages.push(Message::user(format!("第{index}句")));
        }

        trim_session(&mut messages, 10);

        assert!(messages.len() <= 10);
        assert!(
            matches!(messages[0], Message::User { .. }),
            "裁剪后第一条必须是 user 消息"
        );
        assert_no_orphan_tool(&messages);
    }

    #[test]
    fn trim_keeps_short_session_untouched() {
        let mut messages = vec![Message::user("a"), Message::user("b")];
        trim_session(&mut messages, 10);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn trim_drops_orphan_tool_at_the_boundary() {
        // 故意让上限切在一对 tool 消息中间。
        let mut messages = vec![Message::user("开头")];
        for index in 0..5 {
            messages.push(Message::user(format!("u{index}")));
            messages.push(assistant_with_tool_call());
            messages.push(Message::tool("call_1", "{}"));
        }

        trim_session(&mut messages, 6);

        assert_no_orphan_tool(&messages);
        assert!(matches!(messages[0], Message::User { .. }));
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        // 1970-01-01 是 Unix 纪元第一天。
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-10-23 是项目里写死的一个日期，用来回归。
        let days = 20_749; // 距 1970-01-01 的天数
        assert_eq!(civil_from_days(days), (2026, 10, 23));
        // 闰年 2 月 29 日。
        assert_eq!(civil_from_days(18_321), (2020, 2, 29));
        assert_eq!(civil_from_days(18_322), (2020, 3, 1));
    }

    #[test]
    fn today_cn_is_a_sane_date() {
        let today = today_cn();
        assert_eq!(today.len(), 10, "日期格式应为 YYYY-MM-DD，实际 {today}");
        assert!(today.starts_with("20"), "年份不对：{today}");
    }
}
