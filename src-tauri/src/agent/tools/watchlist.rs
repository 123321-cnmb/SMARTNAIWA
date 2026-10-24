use std::path::PathBuf;

use serde_json::json;

use super::super::tools::{arg_str, Tool, ToolDefinition, ToolRegistry};

/// 自选股存放在用户配置目录下，随应用走。
fn watchlist_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("naiwa-pet").join("watchlist.json")
}

/// 读取自选股列表。
pub fn load_watchlist() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(watchlist_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text).unwrap_or_default()
}

/// 写回自选股列表。
pub fn save_watchlist(codes: &[String]) -> Result<(), String> {
    let path = watchlist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建目录失败：{err}"))?;
    }
    let text = serde_json::to_string_pretty(codes).map_err(|err| format!("序列化失败：{err}"))?;
    std::fs::write(&path, text).map_err(|err| format!("写入自选股失败：{err}"))
}

pub struct WatchlistTool {
    action: String,
}

impl WatchlistTool {
    fn new(action: &str) -> Self {
        Self {
            action: action.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for WatchlistTool {
    fn definition(&self) -> ToolDefinition {
        match self.action.as_str() {
            "add" => ToolDefinition::new(
                "add_watchlist",
                "把一只股票加入用户的自选股列表。",
                json!({
                    "type": "object",
                    "properties": {
                        "code": {
                            "type": "string",
                            "description": "带市场前缀的股票代码，例如 sh600000"
                        }
                    },
                    "required": ["code"]
                }),
            ),
            "remove" => ToolDefinition::new(
                "remove_watchlist",
                "把一只股票从用户的自选股列表里移除。",
                json!({
                    "type": "object",
                    "properties": {
                        "code": {
                            "type": "string",
                            "description": "带市场前缀的股票代码，例如 sh600000"
                        }
                    },
                    "required": ["code"]
                }),
            ),
            _ => ToolDefinition::new(
                "list_watchlist",
                "列出用户自选股列表里的所有股票代码。",
                json!({
                    "type": "object",
                    "properties": {}
                }),
            ),
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<String, String> {
        let mut codes = load_watchlist();

        match self.action.as_str() {
            "add" => {
                let code = arg_str(&args, "code", "").trim().to_lowercase();
                if code.is_empty() {
                    return Err("股票代码不能为空".to_string());
                }
                if !codes.contains(&code) {
                    codes.push(code.clone());
                    save_watchlist(&codes)?;
                }
                Ok(json!({ "action": "add", "code": code, "count": codes.len() }).to_string())
            }
            "remove" => {
                let code = arg_str(&args, "code", "").trim().to_lowercase();
                let before = codes.len();
                codes.retain(|item| item != &code);
                if codes.len() != before {
                    save_watchlist(&codes)?;
                }
                Ok(json!({ "action": "remove", "code": code, "count": codes.len() }).to_string())
            }
            _ => Ok(json!({ "codes": codes }).to_string()),
        }
    }
}

/// 心跳场景单独用的自选股工具定义（只列，不增删）。
pub fn list_watchlist_definition() -> ToolDefinition {
    ToolDefinition::new(
        "list_watchlist",
        "列出用户自选股里的股票代码。",
        json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    )
}

pub fn build_watchlist_tools_into(registry: &mut ToolRegistry) {
    registry.register(Box::new(WatchlistTool::new("add")));
    registry.register(Box::new(WatchlistTool::new("remove")));
    registry.register(Box::new(WatchlistTool::new("list")));
}
