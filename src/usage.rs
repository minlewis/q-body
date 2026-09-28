//! CLI usage 文案 golden tests（standalone 纯函数模块，learnings / clamp / darkroom 同先例）。
//!
//! 借鉴：yologdev/yoyo-evolve — Day 212 Task 2：help text 用测试钉住 `-p` 与
//! `--print` 的 stdout 语义差异，文档与行为一起 gate。任何文案改动跑测试防漂移。
//!
//! q-body 实际 CLI 面很小：`--port[=N]`（src/main.rs 参数循环）+ 未来 skills
//! 子命令扩展。本模块把「输入 → 解析结果 + 面向用户的文案」做成词表驱动，
//! 文案字符串集中单一事实源，golden tests 断言语义，宿主接线一行。

/// 单条 usage 语义表项：识别词 + 解析语义 + 用户可见文案。
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEntry {
    /// 识别词（如 "--port"），前缀匹配形式（"--port=" 派生自同一词根）
    pub token: &'static str,
    /// 用户可见语义说明（golden 断言对象）
    pub doc: &'static str,
}

/// 内置词表：q-body 当前 CLI 面的单一事实源。
pub fn builtin_usage() -> Vec<UsageEntry> {
    vec![UsageEntry {
        token: "--port",
        doc: "监听端口；--port=N 或 --port N，默认 41242",
    }]
}

/// 端口解析结果：语义三态，杜绝「解析失败静默用默认值」的自 concealing。
#[derive(Debug, Clone, PartialEq)]
pub enum PortOutcome {
    /// 成功解析到端口值
    Parsed(u16),
    /// 识别了词但值非法（如 --port=abc）— 回退默认但必须可观测
    InvalidValue(String),
    /// 未知参数 — 回退默认但必须可观测
    Unknown(String),
}

pub const DEFAULT_PORT: u16 = 41242;

/// 解析单个参数（对应 main.rs 参数循环的单次迭代语义）。
pub fn parse_arg(arg: &str) -> PortOutcome {
    if let Some(v) = arg.strip_prefix("--port=") {
        return match v.parse::<u16>() {
            Ok(n) => PortOutcome::Parsed(n),
            Err(_) => PortOutcome::InvalidValue(v.to_string()),
        };
    }
    if arg == "--port" {
        // main.rs 当前不解析分离值对；语义上视为「识别词、待值」，
        // 与生产行为一致：保持默认端口（golden 钉住这个现状）
        return PortOutcome::Parsed(DEFAULT_PORT);
    }
    PortOutcome::Unknown(arg.to_string())
}

/// 解析整组参数的最终端口（对应 main.rs 循环收敛语义：后者覆盖前者，
/// 非法/未知项不中断循环，最终落默认或最后一个合法值）。
pub fn resolve_port(args: &[String]) -> (u16, Vec<PortOutcome>) {
    let mut port = DEFAULT_PORT;
    let mut outcomes = Vec::new();
    for a in args {
        let o = parse_arg(a);
        if let PortOutcome::Parsed(n) = o.clone() {
            // 分离形式 "--port" 钉在默认值（生产现状），不覆盖
            if a != "--port" {
                port = n;
            }
        }
        outcomes.push(o);
    }
    (port, outcomes)
}

/// 面向用户的 usage 文案（单一事实源，golden tests 逐行断言）。
pub fn usage_text() -> String {
    let mut lines = vec![
        "q-body A2A Server".to_string(),
        "用法: q-body [--port=N]".to_string(),
    ];
    for e in builtin_usage() {
        lines.push(format!("  {}  —  {}", e.token, e.doc));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- golden：词表语义逐条钉死（yoyo Day 212：文档即断言）----

    #[test]
    fn golden_usage_text_lines() {
        let t = usage_text();
        assert!(t.contains("q-body A2A Server"));
        assert!(t.contains("用法: q-body [--port=N]"));
        assert!(t.contains("--port  —  监听端口；--port=N 或 --port N，默认 41242"));
    }

    #[test]
    fn golden_builtin_vocab() {
        let v = builtin_usage();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].token, "--port");
        assert!(v[0].doc.contains("默认 41242"));
    }

    // ---- 解析语义三态 ----

    #[test]
    fn test_parse_port_eq_form() {
        assert_eq!(parse_arg("--port=5000"), PortOutcome::Parsed(5000));
    }

    #[test]
    fn test_parse_port_bare_form_pinned_to_default() {
        // 生产现状：分离值对不解析，golden 钉住「不静默假装支持」
        assert_eq!(parse_arg("--port"), PortOutcome::Parsed(DEFAULT_PORT));
    }

    #[test]
    fn test_parse_invalid_value_observable() {
        assert_eq!(
            parse_arg("--port=abc"),
            PortOutcome::InvalidValue("abc".to_string())
        );
    }

    #[test]
    fn test_parse_unknown_observable() {
        assert_eq!(
            parse_arg("--verbose"),
            PortOutcome::Unknown("--verbose".into())
        );
    }

    // ---- 循环收敛语义 ----

    #[test]
    fn test_resolve_last_valid_wins() {
        let args: Vec<String> = ["--port=5000", "--port=6000"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (port, outcomes) = resolve_port(&args);
        assert_eq!(port, 6000);
        assert_eq!(outcomes.len(), 2);
    }

    #[test]
    fn test_resolve_invalid_does_not_block_later_valid() {
        let args: Vec<String> = ["--port=abc", "--port=7777"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (port, _) = resolve_port(&args);
        assert_eq!(port, 7777);
    }

    #[test]
    fn test_resolve_default_on_empty() {
        let (port, outcomes) = resolve_port(&[]);
        assert_eq!(port, DEFAULT_PORT);
        assert!(outcomes.is_empty());
    }
}
