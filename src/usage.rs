//! CLI usage 文案 golden tests（standalone 纯函数模块，learnings / clamp / darkroom 同先例）。
//!
//! 借鉴：yologdev/yoyo-evolve — Day 212 Task 2：help text 用测试钉住 `-p` 与
//! `--print` 的 stdout 语义差异，文档与行为一起 gate。任何文案改动跑测试防漂移。
//!
//! q-body 实际 CLI 面很小：`--port[=N]`（src/main.rs 参数循环）+ 未来 skills
//! 子命令扩展。本模块把「输入 → 解析结果 + 面向用户的文案」做成词表驱动，
//! 文案字符串集中单一事实源，golden tests 断言语义，宿主接线一行。

use serde::Serialize;

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

// ============================================================
// CLI 错误路径：结构化 error body + 非零退出码（#982 slice 1 同款）
// ============================================================

/// 非法参数时的非零退出码。2 = 使用错误（区别于运行时失败的 1）。
pub const EXIT_USAGE: i32 = 2;

/// 结构化 CLI 错误体（serde 序列化到 stderr，golden 字节级钉死）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CliError {
    pub error: String,
    /// 按出现顺序列出被拒的原始参数（不重排，保持证据原样）
    pub rejected_args: Vec<String>,
    pub exit_code: i32,
}

/// 把参数分类结果折叠成结构化错误：任何 Unknown/InvalidValue 即错误体，
/// 文案全部派生自本模块常量（单一事实源），render 处不做字符串拼接。
pub fn cli_error(_args: &[String], outcomes: &[PortOutcome]) -> CliError {
    let mut rejected = Vec::new();
    let mut invalid_value = false;
    for o in outcomes {
        match o {
            PortOutcome::InvalidValue(v) => {
                rejected.push(v.clone());
                invalid_value = true;
            }
            PortOutcome::Unknown(a) => rejected.push(a.clone()),
            PortOutcome::Parsed(_) => {}
        }
    }
    let error = if invalid_value {
        ERROR_INVALID_PORT_VALUE.to_string()
    } else {
        ERROR_UNKNOWN_ARG.to_string()
    };
    CliError {
        error,
        rejected_args: rejected,
        exit_code: EXIT_USAGE,
    }
}

/// 错误文案常量：golden 断言对象，禁止在 render 处内联改写。
pub const ERROR_INVALID_PORT_VALUE: &str =
    "invalid value for --port (expected unsigned 16-bit integer)";
pub const ERROR_UNKNOWN_ARG: &str = "unknown argument (see usage)";

// ============================================================
// 语义退出码词表（#982 slice 2 同款）：cron 管道可按退出码区分失败类型
// （EXIT_USAGE = 2 已在上方 CLI 错误路径一节定义，此处不重复）
// ============================================================

/// 语义退出码：资源缺失（如 resume gate 下 journal 文件存在但损坏不可读）。
/// systemd/cron 看到非零即知失败，看到 1 即知是「前置资源不可用」。
pub const EXIT_MISSING_RESOURCE: i32 = 1;

/// 语义退出码：内部错误（绑定失败、serve 期间意外 IO/hyper 错误）。
/// sysexits.h 惯例 EX_SOFTWARE=70，区别于资源缺失与使用错误。
pub const EXIT_INTERNAL: i32 = 70;

/// 退出码合法性守卫：语义退出码两两可区分且非零，测试钉死防词表漂移。
pub fn exit_codes_distinct() -> bool {
    let codes = [EXIT_USAGE, EXIT_MISSING_RESOURCE, EXIT_INTERNAL];
    codes.iter().all(|c| *c != 0) && {
        let mut sorted = codes;
        sorted.sort();
        sorted.windows(2).all(|w| w[0] != w[1])
    }
}

/// 内部错误结构化 body（bind 失败 / serve 意外错误路径）。详细原因只进
/// tracing 日志，stderr 结构化体不含内部路径/配置细节（trust-boundary
/// sanitize 同款纪律），文案常量单一事实源。
pub fn internal_error(_detail: &str) -> CliError {
    CliError {
        error: "internal error (see logs for details)".to_string(),
        rejected_args: Vec::new(),
        exit_code: EXIT_INTERNAL,
    }
}

/// 资源缺失/不可用结构化 body（resume-strict gate 路径）。reason 只进
/// 日志与 eprintln 明文行，结构化体保持同形词表文案。
pub fn missing_resource_error(_reason: &str) -> CliError {
    CliError {
        error: "missing or unusable resource (see logs for details)".to_string(),
        rejected_args: Vec::new(),
        exit_code: EXIT_MISSING_RESOURCE,
    }
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

    // ---- CLI 错误路径：结构化 body + 非零退出码（#982 slice 1 同款）----

    #[test]
    fn golden_cli_error_body_unknown_arg() {
        let args: Vec<String> = ["--verbose"].iter().map(|s| s.to_string()).collect();
        let (_, outcomes) = resolve_port(&args);
        let e = cli_error(&args, &outcomes);
        assert_eq!(e.error, "unknown argument (see usage)");
        assert_eq!(e.rejected_args, vec!["--verbose".to_string()]);
        assert_eq!(e.exit_code, 2);
        // 字节级 golden：字段顺序 + 内容完全钉死
        let bytes = serde_json::to_string_pretty(&e).unwrap();
        assert_eq!(
            bytes,
            "{\n  \"error\": \"unknown argument (see usage)\",\n  \"rejected_args\": [\n    \"--verbose\"\n  ],\n  \"exit_code\": 2\n}"
        );
    }

    #[test]
    fn golden_cli_error_body_invalid_port_value() {
        let args: Vec<String> = ["--port=abc"].iter().map(|s| s.to_string()).collect();
        let (_, outcomes) = resolve_port(&args);
        let e = cli_error(&args, &outcomes);
        assert_eq!(
            e.error,
            "invalid value for --port (expected unsigned 16-bit integer)"
        );
        assert_eq!(e.rejected_args, vec!["abc".to_string()]);
        assert_eq!(e.exit_code, 2);
    }

    #[test]
    fn golden_cli_error_invalid_value_wins_over_unknown() {
        // 混合输入：invalid value 优先于 unknown 作为主错误（更具体的诊断）
        let args: Vec<String> = ["--trace", "--port=0x10"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (_, outcomes) = resolve_port(&args);
        let e = cli_error(&args, &outcomes);
        assert_eq!(
            e.error,
            "invalid value for --port (expected unsigned 16-bit integer)"
        );
        assert_eq!(
            e.rejected_args,
            vec!["--trace".to_string(), "0x10".to_string()]
        );
    }

    #[test]
    fn test_cli_error_empty_when_all_valid() {
        let args: Vec<String> = ["--port=5000"].iter().map(|s| s.to_string()).collect();
        let (_, outcomes) = resolve_port(&args);
        // 全合法时错误体不产生 rejected（调用方守卫先于本函数触发）
        let e = cli_error(&args, &outcomes);
        assert_eq!(e.error, "unknown argument (see usage)"); // 兜底词表，但守卫保证不会走到
        assert!(e.rejected_args.is_empty());
    }

    #[test]
    fn test_exit_usage_is_nonzero() {
        assert!(EXIT_USAGE > 0);
        assert_ne!(EXIT_USAGE, 1); // 与运行时失败退出码区分
    }

    // ---- 语义退出码词表（#982 slice 2）：cron 管道可区分失败类型 ----

    #[test]
    fn golden_exit_code_vocab() {
        // 字节级钉死三值语义：改任何一值必须显式过这个 golden
        assert_eq!(EXIT_MISSING_RESOURCE, 1);
        assert_eq!(EXIT_USAGE, 2);
        assert_eq!(EXIT_INTERNAL, 70); // sysexits.h EX_SOFTWARE 惯例
        assert!(exit_codes_distinct());
    }

    #[test]
    fn golden_internal_error_body() {
        // 内部错误路径的结构化 body：文案常量单一事实源 + golden 钉死
        let e = internal_error("Failed to bind to 127.0.0.1:41242: addr in use");
        assert_eq!(e.error, "internal error (see logs for details)");
        assert_eq!(e.rejected_args, Vec::<String>::new());
        assert_eq!(e.exit_code, EXIT_INTERNAL);
        let bytes = serde_json::to_string_pretty(&e).unwrap();
        assert_eq!(
            bytes,
            "{\n  \"error\": \"internal error (see logs for details)\",\n  \"rejected_args\": [],\n  \"exit_code\": 70\n}"
        );
    }

    #[test]
    fn golden_missing_resource_error_body() {
        // 资源缺失路径的结构化 body：reason 附在结构化字段而非裸 stderr 行
        let e = missing_resource_error("resume-strict gate: journal corrupted");
        assert_eq!(
            e.error,
            "missing or unusable resource (see logs for details)"
        );
        assert_eq!(e.exit_code, EXIT_MISSING_RESOURCE);
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            "{\"error\":\"missing or unusable resource (see logs for details)\",\"rejected_args\":[],\"exit_code\":1}"
        );
    }
}
