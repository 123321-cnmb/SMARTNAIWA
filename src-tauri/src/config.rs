use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 配置文件里的可选项。放在 `%APPDATA%\naiwa-pet\config.json`。
///
/// 开源版本不再内置任何 API Key——每个人自己配自己的 key，
/// 否则发布出去的二进制会被所有人共用同一个 key，额度很快就被刷爆。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    /// OpenAI 兼容端点的 API Key（MiniMax / DeepSeek / 其它兼容网关都行）
    #[serde(default)]
    pub api_key: String,
    /// 自定义 Base URL，留空用官方地址
    #[serde(default)]
    pub base_url: String,
    /// 模型名，留空用默认值
    #[serde(default)]
    pub model: String,
    /// 麦当劳 MCP 的 Token。
    ///
    /// 这个 Token 绑定的是**个人麦当劳账户**（手机号登录后申请），能查券、下单、
    /// 查订单和收货地址。所以**绝不内置**，由使用者自己在设置窗口里填。
    /// 留空 = 不启用麦当劳能力（工具根本不注册，模型也看不到这些工具）。
    #[serde(default)]
    pub mcd_mcp_token: String,
    /// 情绪状态，跟配置存在一起，重启不丢
    #[serde(default)]
    pub mood: Option<crate::agent::persona::Mood>,
}

/// 最终生效的配置：环境变量优先于配置文件。
#[derive(Debug, Clone, Default)]
pub struct ResolvedConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// 用的是内置默认 Key（用户没有自己配）
    pub using_builtin: bool,
    /// 麦当劳 MCP Token，空 = 不启用
    pub mcd_mcp_token: String,
}

impl ResolvedConfig {
    pub fn has_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

pub const DEFAULT_BASE_URL: &str = "https://api.minimaxi.com/v1";
pub const DEFAULT_MODEL: &str = "MiniMax-M3";

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join("naiwa-pet").join("config.json")
    }

    /// 读配置文件。读不到或坏了就返回默认值，不让程序起不来。
    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(Self::config_path()) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("创建配置目录失败：{err}"))?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|err| format!("序列化配置失败：{err}"))?;
        std::fs::write(&path, text).map_err(|err| format!("写入配置失败：{err}"))
    }
    /// 环境变量 > 配置文件 > 默认值。
    pub fn resolve(&self) -> ResolvedConfig {
        let env_key = std::env::var("MINIMAX_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let env_base = std::env::var("MINIMAX_BASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let env_model = std::env::var("MINIMAX_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let file_key = self.api_key.trim().to_string();
        let file_base = self.base_url.trim().to_string();
        let file_model = self.model.trim().to_string();
        let file_mcd = self.mcd_mcp_token.trim().to_string();

        let env_mcd = std::env::var("MCD_MCP_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        // 环境变量 > 配置文件 > 内置默认（保证下载即用）
        let (api_key, using_builtin) = match env_key {
            Some(key) => (key, false),
            None => match file_key {
                key if !key.is_empty() => (key, false),
                _ => (crate::EMBEDDED_DEFAULT_API_KEY.to_string(), true),
            },
        };

        ResolvedConfig {
            api_key,
            using_builtin,
            base_url: env_base
                .or_else(|| Some(file_base))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: env_model
                .or_else(|| Some(file_model))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            // 麦当劳 Token 只有环境变量和配置文件两个来源，没有内置值。
            mcd_mcp_token: env_mcd.unwrap_or(file_mcd),
        }
    }
}

/// 给前端看的配置（key 脱敏，不外泄）。
#[derive(Debug, Clone, Serialize)]
pub struct ConfigView {
    pub has_key: bool,
    /// 只回显末 4 位，方便用户确认配对了没有
    pub key_hint: String,
    /// 当前用的是不是内置的默认 Key
    pub using_builtin: bool,
    pub base_url: String,
    pub model: String,
    pub config_path: String,
    /// 有没有配麦当劳 MCP Token（只给"有没有"，不给内容）
    pub mcd_token_set: bool,
    /// 是不是**第一次启动**（配置文件还不存在）。
    ///
    /// 用来在前端显示"首次设置"引导：第一次让人先把 Key 和 MCP Token 填了，
    /// 之后就不再打扰。
    pub first_run: bool,
}

impl ConfigView {
    pub fn from_resolved(resolved: &ResolvedConfig) -> Self {
        let key = resolved.api_key.trim();
        let key_hint = if key.len() > 4 {
            format!("…{}", &key[key.len() - 4..])
        } else if key.is_empty() {
            String::new()
        } else {
            key.to_string()
        };

        Self {
            has_key: resolved.has_key(),
            key_hint,
            using_builtin: resolved.using_builtin,
            base_url: resolved.base_url.clone(),
            model: resolved.model.clone(),
            config_path: AppConfig::config_path().to_string_lossy().to_string(),
            mcd_token_set: !resolved.mcd_mcp_token.trim().is_empty(),
            first_run: !AppConfig::config_path().exists(),
        }
    }
}
