use std::collections::HashMap;

use futures_util::StreamExt;

use serde::Deserialize;

use super::message::{Message, ToolCall};
use super::sse::SseParser;
use super::tools::ToolDefinition;

/// 一次请求的结果：这一轮模型返回的文本 + 工具调用请求。
#[derive(Debug, Default, Clone)]
pub struct TurnOutput {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

pub struct MiniMaxClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl Default for MiniMaxClient {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: crate::config::DEFAULT_BASE_URL.to_string(),
            api_key: String::new(),
            model: crate::config::DEFAULT_MODEL.to_string(),
        }
    }
}

impl MiniMaxClient {
    /// 运行时更新端点配置（用户在设置窗口改完立即生效）。
    pub fn configure(&mut self, base_url: &str, api_key: &str, model: &str) {
        if !base_url.trim().is_empty() {
            self.base_url = base_url.trim().to_string();
        }
        self.api_key = api_key.trim().to_string();
        if !model.trim().is_empty() {
            self.model = model.trim().to_string();
        }
    }

    /// 发一轮请求，把流式响应累积成完整的文本和工具调用。
    pub async fn chat_turn(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<TurnOutput, String> {
        if self.api_key.is_empty() {
            return Err("还没配置 API Key。在托盘菜单「设置」里填一下就能用了。".to_string());
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let payload = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "stream": true,
            "temperature": 0.7,
            "top_p": 0.95,
            "max_completion_tokens": 4096,
            // 桌宠要的是秒级回复，不需要长思考。开着 adaptive 会让模型
            // 先输出一段 <think> 思考，延迟高还很费 token。
            "thinking": { "type": "disabled" },
        });

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|err| format!("连接模型服务失败：{err}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("模型服务返回 {status}：{body}"));
        }

        let mut parser = SseParser::new();
        let mut accumulator = ToolCallAccumulator::default();
        let mut content = String::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| format!("读取流失败：{err}"))?;
            let text = String::from_utf8_lossy(&chunk).to_string();

            for payload in parser.push(&text) {
                let Ok(chunk) = serde_json::from_str::<StreamChunk>(&payload) else {
                    // 单帧解析失败不影响整体，跳过。
                    continue;
                };

                for choice in chunk.choices {
                    if let Some(delta) = choice.delta.content {
                        content.push_str(&delta);
                    }
                    for call in choice.delta.tool_calls {
                        accumulator.push(call);
                    }
                }
            }
        }

        Ok(TurnOutput {
            content: strip_think_tags(&content),
            tool_calls: accumulator.finish(),
        })
    }
}

/// 去掉回复里的 `<think>...</think>` 思考块。
///
/// 即便请求里已经关了 thinking，模型偶尔还是会吐思考标签，漏到正文里
/// 会让奶蛙突然冒出一段莫名其妙的内心戏。
fn strip_think_tags(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;

    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/* ------------------------------------------------------------------ *
 * 流式响应解析
 * ------------------------------------------------------------------ */

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: ChunkDelta,
}

#[derive(Debug, Default, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// 按 `index` 累积分片到达的工具调用。
///
/// 流式响应里一个工具调用的 name / arguments 是被拆成多片发过来的，
/// 必须按 index 归并到一起，最后再把 arguments 整体解析成 JSON。
#[derive(Debug, Default)]
struct ToolCallAccumulator {
    slots: HashMap<u32, PartialToolCall>,
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    fn push(&mut self, delta: ToolCallDelta) {
        let index = delta.index.unwrap_or(0);
        let slot = self.slots.entry(index).or_default();

        if let Some(id) = delta.id {
            if !id.is_empty() {
                slot.id = id;
            }
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                if !name.is_empty() {
                    slot.name = name;
                }
            }
            if let Some(arguments) = function.arguments {
                slot.arguments.push_str(&arguments);
            }
        }
    }

    fn finish(self) -> Vec<ToolCall> {
        let mut entries: Vec<(u32, PartialToolCall)> = self.slots.into_iter().collect();
        entries.sort_by_key(|(index, _)| *index);

        entries
            .into_iter()
            .filter(|(_, call)| !call.name.is_empty())
            .map(|(_, call)| ToolCall {
                id: call.id,
                kind: "function".to_string(),
                function: super::message::ToolCallFunction {
                    name: call.name,
                    arguments: call.arguments,
                },
            })
            .collect()
    }
}
