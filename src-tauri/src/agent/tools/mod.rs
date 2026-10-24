pub mod express;
pub mod mcd;
pub mod stock;
pub mod watchlist;

use std::collections::HashMap;

use serde::Serialize;

/// 工具定义，序列化成 OpenAI `tools` 数组的元素。
///
/// schema 全部手写。这里刻意**不**做「从 docstring 自动生成」那套——
///
/// 那套会把 `list[X]` 这类复杂类型降级成 `"string"`，模型按 schema 传字符串，
/// 实现却按列表遍历，工具必炸，而且不报错。
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunctionSchema,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn new(name: &str, description: &str, parameters: serde_json::Value) -> Self {
        Self {
            kind: "function".to_string(),
            function: ToolFunctionSchema {
                name: name.to_string(),
                description: description.to_string(),
                parameters,
            },
        }
    }
}

/// 一个可被模型调用的工具。
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    /// 执行工具。返回的字符串会作为 `role: "tool"` 消息回喂给模型。
    async fn call(&self, args: serde_json::Value) -> Result<String, String>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.definition().function.name, tool);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions: Vec<ToolDefinition> =
            self.tools.values().map(|tool| tool.definition()).collect();
        // 顺序稳定，方便对照调试。
        definitions.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        definitions
    }

    pub async fn call(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
        let Some(tool) = self.tools.get(name) else {
            return Err(format!("未知工具：{name}"));
        };
        tool.call(args).await
    }
}

/// 从 JSON 对象里取字符串参数，缺失或类型不符时返回默认值。
pub fn arg_str(args: &serde_json::Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or(default)
        .to_string()
}

/// 从 JSON 对象里取整数参数。
pub fn arg_i64(args: &serde_json::Value, key: &str, default: i64) -> i64 {
    args.get(key)
        .and_then(|value| value.as_i64())
        .or_else(|| {
            args.get(key)
                .and_then(|value| value.as_f64())
                .map(|value| value as i64)
        })
        .unwrap_or(default)
}
