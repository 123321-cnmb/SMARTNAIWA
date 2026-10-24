use tauri::ipc::Channel;

use super::events::AgentEvent;
use super::llm::MiniMaxClient;
use super::message::Message;
use super::tools::ToolRegistry;

/// 单轮对话内最多允许模型调用多少轮工具。
const MAX_TOOL_ROUNDS: usize = 8;

/// 单次工具结果的字数上限，防止把上下文撑爆。
const MAX_TOOL_RESULT_CHARS: usize = 8000;

/// 跑完一轮 Agent 循环。
///
/// 流程：把消息和工具清单发给模型 → 模型要么给最终答复，要么要求调工具 →
/// 执行工具、把结果回喂给模型 → 重复，直到模型给出最终答复或轮数耗尽。
///
/// 返回最终答复文本。所有中间过程通过 `channel` 推给前端。
///
/// `on_express` 用来把「奶蛙要做一个动作」转成全局广播。
/// **不能只靠前端某个窗口转发**——聊天窗口和桌宠窗口是两个 webview，
/// 在聊天窗口里发起的对话也要能触发动画，所以这一步放在 Rust 侧做。
pub async fn run_agent(
    client: &MiniMaxClient,
    messages: &mut Vec<Message>,
    registry: &ToolRegistry,
    channel: &Channel<AgentEvent>,
    on_express: &(dyn Fn(String, String) + Send + Sync),
) -> Result<String, String> {
    let definitions = registry.definitions();
    let mut final_text = String::new();
    // 正文有没有真的推给前端。
    //
    // 模型把「答案」和「调工具」放在同一轮的时候，正文会被当成"调工具前的废话"
    // 压下去（见下）。可要是下一轮它又没再说话，用户就什么都收不到了——
    // 表现就是"奶蛙笑了一下"，然后没了。所以这里记一笔，
    // 循环结束时如果正文一次都没发出去，补发最后一次的内容。
    let mut assistant_text_sent = false;

    for round in 0..MAX_TOOL_ROUNDS {
        let turn = client.chat_turn(messages, &definitions).await?;

        if !turn.content.is_empty() {
            final_text = turn.content.clone();

            // 模型在调用工具前说的那半句（"我先查一下…"）不显示：
            // 它对用户没价值，而且模型最容易在这里跑英文。
            // 「正在查」这件事交给工具卡片表达就够了。
            if turn.tool_calls.is_empty() {
                let _ = channel.send(AgentEvent::Assistant {
                    content: turn.content.clone(),
                });
                assistant_text_sent = true;
            }
        }

        // 没有工具调用 = 模型给出了最终答复。
        if turn.tool_calls.is_empty() {
            if !assistant_text_sent && !final_text.is_empty() {
                let _ = channel.send(AgentEvent::Assistant {
                    content: final_text.clone(),
                });
            }
            let _ = channel.send(AgentEvent::Done {
                final_text: final_text.clone(),
            });
            return Ok(final_text);
        }

        // 模型要求调工具：先把这一轮的 assistant 消息（含 tool_calls）写回上下文。
        messages.push(Message::Assistant {
            content: if turn.content.is_empty() {
                None
            } else {
                Some(turn.content.clone())
            },
            tool_calls: turn.tool_calls.clone(),
        });

        for call in &turn.tool_calls {
            let args: serde_json::Value =
                serde_json::from_str(&call.function.arguments).unwrap_or(serde_json::json!({}));

            let _ = channel.send(AgentEvent::ToolCall {
                name: call.function.name.clone(),
                args: args.clone(),
            });

            let outcome = registry.call(&call.function.name, args).await;
            let content = match outcome {
                Ok(result) => {
                    // 表情动作工具不是普通工具：它转成一个事件，模型这边
                    // 只收到「你已经真的做了」的确认，不参与后续推理。
                    if let Some((action, reason)) =
                        super::tools::express::ExpressTool::parse(&result)
                    {
                        let _ = channel.send(AgentEvent::Express {
                            action: action.clone(),
                            reason: reason.clone(),
                        });
                        on_express(action, reason);
                        // 也要发 ToolResult：不然前端那张"正在做…"的卡片
                        // 会一直转下去，直到用户以为卡死了。
                        let _ = channel.send(AgentEvent::ToolResult {
                            name: call.function.name.clone(),
                            result: "（动作已播放）".to_string(),
                        });
                        "（你已经真的做出了这个动作，用户看得见）".to_string()
                    } else {
                        let _ = channel.send(AgentEvent::ToolResult {
                            name: call.function.name.clone(),
                            result: result.clone(),
                        });
                        result
                    }
                }
                Err(error) => {
                    let _ = channel.send(AgentEvent::ToolError {
                        name: call.function.name.clone(),
                        error: error.clone(),
                    });
                    // 失败也要回喂给模型，否则模型会一直等这个工具的结果。
                    format!("工具执行失败：{error}")
                }
            };

            let truncated = truncate(&content, MAX_TOOL_RESULT_CHARS);
            messages.push(Message::tool(call.id.clone(), truncated));
        }

        if round == MAX_TOOL_ROUNDS - 1 {
            if !assistant_text_sent && !final_text.is_empty() {
                let _ = channel.send(AgentEvent::Assistant {
                    content: final_text.clone(),
                });
            }
            let _ = channel.send(AgentEvent::Error {
                message: "工具调用轮数用尽".to_string(),
            });
        }
    }

    Ok(final_text)
}

/// 按字符截断（中文按字算）。
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push_str("\n…（内容过长已截断）");
    out
}
