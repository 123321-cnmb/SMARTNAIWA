use serde::Serialize;

/// Agent 运行时推给前端的事件。
///
/// 前端按 `type` 分流渲染。字段用 snake_case，和 Rust 侧一致。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// 模型回复的文本分片，逐字到达。
    Assistant { content: String },
    /// 模型请求调用工具。
    ToolCall { name: String, args: serde_json::Value },
    /// 工具执行成功。
    ToolResult { name: String, result: String },
    /// 工具执行失败。
    ToolError { name: String, error: String },
    /// 奶蛙做了一个情绪动作（真的播了动画，不是文字描述）。
    Express { action: String, reason: String },
    /// 整轮对话结束。
    Done { final_text: String },
    /// 出错。
    Error { message: String },
}
