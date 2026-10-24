use serde::{Deserialize, Serialize};

/// 模型请求的工具调用（OpenAI 兼容协议）。
///
/// 注意 `arguments` 是 **JSON 字符串**而不是对象，这是 OpenAI 工具调用协议
/// 的要求：参数在流式响应里是分片到达的字符串，流结束后才能整体解析。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// 一条对话消息。序列化成 OpenAI `messages` 数组的元素。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        /// 纯文本回复。有工具调用时可能为空。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Message::User {
            content: content.into(),
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Message::Tool {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
        }
    }
}
