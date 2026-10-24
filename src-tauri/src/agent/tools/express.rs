use serde_json::json;

use super::super::tools::{arg_str, Tool, ToolDefinition, ToolRegistry};

/// 奶蛙能做的情绪动作。
pub const ACTIONS: [&str; 5] = ["laugh", "big_laugh", "shy", "poke", "sleep"];

/// 表情动作工具。
///
/// 模型**只能**通过调用这个工具来表达情绪——文字里写"哈哈哈"不算，
/// 调用了才会真的播放动画。这一点很重要：让 prompt 冒充行为，
/// 模型就会开始演独角戏，什么都没发生却说得像真的。
///
/// 工具本身不返回普通文本，而是返回一个带 `__express` 标记的 JSON，
/// 由 Agent 主循环识别后转成 `AgentEvent::Express` 事件推给前端。
pub struct ExpressTool;

impl ExpressTool {
    /// 工具定义。心跳场景下单独用（不注册进主工具表）。
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "express",
            "表达一个情绪动作（真的做出这个动作：播放动画）。当你觉得好笑、开心、想逗用户，\
             或者被夸得不好意思时，调用它。只在文字里写「哈哈」不算数，调用了这个工具才是真的笑了。",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ACTIONS,
                        "description": "laugh=会心一笑，big_laugh=哈哈大笑，shy=害羞，poke=戳一下用户，sleep=打瞌睡"
                    },
                    "reason": {
                        "type": "string",
                        "description": "为什么这么做，一句话，会显示给用户看"
                    }
                },
                "required": ["action"]
            }),
        )
    }

    /// 从工具返回值里解析出表情动作。
    pub fn parse(result: &str) -> Option<(String, String)> {
        let value: serde_json::Value = serde_json::from_str(result).ok()?;
        if value.get("__express")?.as_bool()? != true {
            return None;
        }
        let action = value.get("action")?.as_str()?.to_string();
        let reason = value
            .get("reason")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        Some((action, reason))
    }
}

#[async_trait::async_trait]
impl Tool for ExpressTool {
    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    async fn call(&self, args: serde_json::Value) -> Result<String, String> {
        let action = arg_str(&args, "action", "laugh");
        if !ACTIONS.contains(&action.as_str()) {
            return Err(format!("不会做 {action} 这个动作"));
        }

        let reason = arg_str(&args, "reason", "");
        Ok(json!({
            "__express": true,
            "action": action,
            "reason": reason,
        })
        .to_string())
    }
}

/// 把表情动作工具装进注册表。
pub fn build_express_tools_into(registry: &mut ToolRegistry) {
    registry.register(Box::new(ExpressTool));
}
