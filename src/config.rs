//! q-body 配置管理
//!
//! 从 config.toml 加载配置，支持 [server]、[llm]、[agent] 三个节。

use serde::Deserialize;

/// 顶层配置
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    /// M3 provider 配置（独立段，不替换 llm，向后兼容）
    #[serde(default)]
    pub m3: M3Config,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub journal: JournalConfig,
}

/// 服务器配置
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// 监听端口
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { port: default_port() }
    }
}

fn default_port() -> u16 {
    41242
}

/// LLM 配置
#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    /// API 端点 URL
    #[serde(default = "default_api_url")]
    pub api_url: String,
    /// 模型名称
    #[serde(default = "default_model")]
    pub model: String,
    /// 环境变量名，从中读取 API Key
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_url: default_api_url(),
            model: default_model(),
            api_key_env: default_api_key_env(),
        }
    }
}

fn default_api_url() -> String {
    "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions".into()
}

fn default_model() -> String {
    "deepseek-v4-flash".into()
}

fn default_api_key_env() -> String {
    "ARK_API_KEY".into()
}

/// M3 Provider 配置 — 通过 Anthropic Messages 协议调用 MiniMax-M3
///
/// 独立于 [llm] 段：默认 LLM (deepseek) 走 OpenAI Chat Completions；
/// M3 走 Anthropic Messages (x-api-key + anthropic-version headers)，
/// 支持原生多模态 image/video content blocks。
#[derive(Debug, Clone, Deserialize)]
pub struct M3Config {
    /// API 端点 URL（Anthropic Messages 入口）
    #[serde(default = "default_m3_api_url")]
    pub api_url: String,
    /// 模型名称
    #[serde(default = "default_m3_model")]
    pub model: String,
    /// 环境变量名，从中读取 API Key
    #[serde(default = "default_m3_api_key_env")]
    pub api_key_env: String,
    /// Anthropic API 版本头
    #[serde(default = "default_m3_anthropic_version")]
    pub anthropic_version: String,
    /// 最大输出 token 数
    #[serde(default = "default_m3_max_tokens")]
    pub max_tokens: u32,
}

fn default_m3_api_url() -> String {
    "https://api.minimaxi.com/anthropic/v1/messages".into()
}

fn default_m3_model() -> String {
    "MiniMax-M3".into()
}

fn default_m3_api_key_env() -> String {
    "MINIMAX_API_KEY".into()
}

fn default_m3_anthropic_version() -> String {
    "2023-06-01".into()
}

fn default_m3_max_tokens() -> u32 {
    4096
}

impl Default for M3Config {
    fn default() -> Self {
        Self {
            api_url: default_m3_api_url(),
            model: default_m3_model(),
            api_key_env: default_m3_api_key_env(),
            anthropic_version: default_m3_anthropic_version(),
            max_tokens: default_m3_max_tokens(),
        }
    }
}

/// Journal 存储配置
#[derive(Debug, Clone, Deserialize)]
pub struct JournalConfig {
    /// journal 数据文件路径。默认: ~/.q-body/journal.json
    #[serde(default = "default_journal_path")]
    pub data_path: String,
    /// 是否启用持久化。默认: true
    #[serde(default = "default_journal_enabled")]
    pub enabled: bool,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            data_path: default_journal_path(),
            enabled: default_journal_enabled(),
        }
    }
}

fn default_journal_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/.q-body/journal.json", home)
}

fn default_journal_enabled() -> bool {
    true
}

/// Agent 配置
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// 系统提示词
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    /// Agent 版本号
    #[serde(default = "default_version")]
    pub version: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: default_system_prompt(),
            version: default_version(),
        }
    }
}

fn default_system_prompt() -> String {
    "你是 q-body，Q宝宝的自进化 Rust 身体。\n\
     你通过 A2A 协议接收外部消息。\n\
     请保持简洁、务实、带一点 🫧 风格的回复。\n\
     你正在进化中，回答体现你的版本号 0.1.3。"
        .into()
}

fn default_version() -> String {
    "0.1.3".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                port: default_port(),
            },
            llm: LlmConfig {
                api_url: default_api_url(),
                model: default_model(),
                api_key_env: default_api_key_env(),
            },
            m3: M3Config::default(),
            agent: AgentConfig {
                system_prompt: default_system_prompt(),
                version: default_version(),
            },
            journal: JournalConfig::default(),
        }
    }
}

impl Config {
    /// 从文件路径加载配置。如果文件不存在，返回默认配置。
    pub fn from_file(path: &str) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Config file '{}' not found: {}. Using defaults.", path, e);
                return Self::default();
            }
        };

        match toml::from_str(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!("Failed to parse config '{}': {}. Using defaults.", path, e);
                Self::default()
            }
        }
    }
}