//! q-body Config 模块单元测试
//!
//! 覆盖范围（v0.1.4 P0 范围）：
//! - 默认值加载
//! - 自定义 TOML 反序列化
//! - 缺段时使用 Default
//!
//! 不测试：网络/文件 IO（handler 层面，v0.2+ 再加）

use q_body::config::{Config, LlmConfig, M3Config, ServerConfig};

#[test]
fn default_config_has_expected_port() {
    let cfg = Config::default();
    assert_eq!(cfg.server.port, 41242, "默认端口应为 41242");
}

#[test]
fn default_llm_uses_deepseek_v4_flash() {
    let cfg = Config::default();
    assert_eq!(cfg.llm.model, "deepseek-v4-flash");
    assert!(cfg.llm.api_url.contains("volces.com"), "默认应指向火山方舟");
    assert_eq!(cfg.llm.api_key_env, "ARK_API_KEY");
}

#[test]
fn default_m3_provider() {
    let cfg = Config::default();
    assert!(cfg.m3.api_url.contains("minimaxi"), "M3 默认走 MiniMax 提供的 Anthropic 兼容入口");
    assert_eq!(cfg.m3.model, "MiniMax-M3");
    assert_eq!(cfg.m3.anthropic_version, "2023-06-01");
    assert!(cfg.m3.max_tokens > 0, "M3 max_tokens 应有合理值");
}

#[test]
fn parse_minimal_toml() {
    let toml_str = r#"
[server]
port = 5000
"#;
    let cfg: Config = toml::from_str(toml_str).expect("最小 TOML 应能解析");
    assert_eq!(cfg.server.port, 5000);
    // 其余段走默认
    assert_eq!(cfg.llm.model, "deepseek-v4-flash");
}

#[test]
fn parse_full_toml() {
    let toml_str = r#"
[server]
port = 8080

[llm]
api_url = "https://api.openai.com/v1/chat/completions"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"

[m3]
api_url = "https://api.anthropic.com/v1/messages"
model = "claude-sonnet-4"
api_key_env = "ANTHROPIC_API_KEY"
anthropic_version = "2023-06-01"
max_tokens = 8192

[agent]
system_prompt = "You are a test agent."
version = "0.1.4"

[journal]
data_path = "/tmp/q-body-test/journal.json"
enabled = false
"#;
    let cfg: Config = toml::from_str(toml_str).expect("完整 TOML 应能解析");
    assert_eq!(cfg.server.port, 8080);
    assert_eq!(cfg.llm.model, "gpt-4o");
    assert_eq!(cfg.llm.api_key_env, "OPENAI_API_KEY");
    assert_eq!(cfg.m3.model, "claude-sonnet-4");
    assert_eq!(cfg.m3.max_tokens, 8192);
    assert_eq!(cfg.agent.version, "0.1.4");
    assert!(!cfg.journal.enabled);
    assert_eq!(cfg.journal.data_path, "/tmp/q-body-test/journal.json");
}

#[test]
fn missing_section_uses_default() {
    // 只给 [server]，其余段应自动 Default
    let toml_str = r#"
[server]
port = 9999
"#;
    let cfg: Config = toml::from_str(toml_str).expect("缺段应能解析");
    assert_eq!(cfg.server.port, 9999);
    // 默认值验证
    assert_eq!(cfg.llm.api_key_env, "ARK_API_KEY");
    assert_eq!(cfg.m3.anthropic_version, "2023-06-01");
    assert_eq!(cfg.agent.version, "0.1.3"); // 配置默认版本
}

#[test]
fn invalid_port_rejected() {
    // port 是 u16，越界值应被 TOML 解析拒绝
    let toml_str = r#"
[server]
port = 99999
"#;
    let result: Result<Config, _> = toml::from_str(toml_str);
    assert!(result.is_err(), "port 越界应被拒绝");
}

#[test]
fn server_config_default_trait() {
    // ServerConfig 没有 derive(Default)，所以这里走 Config::default() 路径
    // （用户真实使用方式 — Config::default() 显式构造子结构）
    let cfg = Config::default();
    assert_eq!(cfg.server.port, 41242);
}

#[test]
fn llm_config_default_trait() {
    let cfg = Config::default();
    assert!(!cfg.llm.api_url.is_empty());
    assert!(!cfg.llm.model.is_empty());
    assert!(!cfg.llm.api_key_env.is_empty());
}

#[test]
fn m3_config_default_trait() {
    let cfg = Config::default();
    assert_eq!(cfg.m3.anthropic_version, "2023-06-01");
    assert!(cfg.m3.max_tokens > 0);
}