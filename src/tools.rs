//! tools.rs — 真实工具执行层（agent 对环境采取行动的「手」）
//!
//! TAO 三问：
//! 1. 接线了吗？→ 是，dispatch_tool 被 agent loop 调用，真跑 shell/读文件。
//! 2. 能进 SOUL.md 吗？→ 不能，进程执行/超时/截断是代码逻辑，LLM 无法保证隔离。
//! 3. 损掉了什么？→ 命令执行白名单 + 超时 + 输出截断 + 复用 validator 安全网，
//!    砍掉了「任意 shell 直通」这个最危险能力，换成显式安全工具。
//!
//! 第一版（运维手）只给最小工具，范围驱动，不做通用 shell。

use crate::validator::command as safety;
use serde_json::json;
use std::process::Command;
use tokio::time::{Duration, timeout};

/// 工具结果二态（issue #146 / backlog 2026-10-10）：
/// 动作实际未发生（被拦截/缺参/未知工具/超时/执行失败/路径错误）时禁止伪装成
/// 成功形状的纯文本回喂——必须显式 Failed（借鉴 yolo #982 commit no-op fail-loud）。
/// 空匹配（exit=0 无输出 / grep 无命中）仍是合法成功（#140 语义不回退）。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ToolStatus {
    /// 动作已发生且成功（含合法空结果）。
    Ok,
    /// 动作未发生或失败——fail-loud，不得当成功消费。
    Failed,
}

/// dispatch 的结构化返回：status 供调用方/日志判定，content 仍回喂 LLM。
#[derive(Debug, PartialEq, Eq)]
pub struct ToolOutcome {
    pub status: ToolStatus,
    pub content: String,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        ToolOutcome { status: ToolStatus::Ok, content: content.into() }
    }
    pub fn failed(content: impl Into<String>) -> Self {
        ToolOutcome { status: ToolStatus::Failed, content: content.into() }
    }
}

/// 单次命令执行上限（秒）——防止挂死/无限输出型命令。
const CMD_TIMEOUT_SECS: u64 = 15;
/// 工具输出回喂 LLM 的字符上限——防爆 prompt（复用 clamp 哲学）。
const OUTPUT_CLAMP_CHARS: usize = 4000;

/// 工具结果三态分类（issue #140）。
/// 空匹配是合法成功（grep 无命中 / 空文件），路径缺失与不可读是错误——
/// 三者不可混为一个「空结果 exit 0」（借鉴 yoyo Day 221 grep 语义诚实化）。
#[derive(Debug, PartialEq, Eq)]
pub enum ToolResultKind {
    /// 合法成功但零结果。
    EmptyMatch,
    /// 路径不存在 → 错误。
    PathMissing,
    /// 存在但读不了（权限/目录/编码等）→ 错误。
    Unreadable,
}

/// io::Error → 三态：NotFound 归路径缺失，其余（PermissionDenied /
/// IsADirectory / InvalidData…）归不可读。失败路径必须可区分，不许全折叠。
pub fn classify_io_error(e: &std::io::Error) -> ToolResultKind {
    match e.kind() {
        std::io::ErrorKind::NotFound => ToolResultKind::PathMissing,
        _ => ToolResultKind::Unreadable,
    }
}

/// 非零退出 + 零输出的诚实语义：退出码如实呈现，不许称「成功」。
/// exit=0 无输出是合法空结果；exit!=0 无输出必须显式标为非零退出。
pub fn empty_output_verdict(exit_code: i32) -> String {
    if exit_code == 0 {
        "(命令完成，exit=0，无输出——空结果是合法结果)".to_string()
    } else {
        format!("(exit={exit_code}，非零退出且无输出——如 grep 无命中属空匹配，其余需结合退出码判断失败)")
    }
}

/// 声明给 LLM 的工具 schema（OpenAI function-calling 格式）。
pub fn tool_specs() -> serde_json::Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "run_readonly",
                "description": "在服务器上执行一条只读排查命令（运维巡检用）。允许: systemctl status/is-active, journalctl, ps, pgrep, ss, df, du, free, uptime, ls, cat, head, tail, grep, curl(GET), stat, who, date。禁止任何修改/删除/重启写操作。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "要执行的完整 shell 命令"}
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "读取服务器上某个文本文件的内容（最多前 4000 字符）。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "文件绝对路径"}
                    },
                    "required": ["path"]
                }
            }
        }
    ])
}

/// run_readonly 允许的命令前缀（首个可执行词）。
/// 显式白名单：只放行只读排查工具，写操作一律拒绝。
const ALLOWED_BINS: &[&str] = &[
    "systemctl", "journalctl", "ps", "pgrep", "ss", "df", "du", "free", "uptime", "ls", "cat",
    "head", "tail", "grep", "stat", "who", "date", "hostname", "uname", "id",
];

/// run_readonly 即使 bin 合法，也拦截这些明显带写语义的参数/子命令。
const WRITE_PATTERNS: &[&str] = &[
    "rm ", "mv ", "cp ", ">", ">>", "&& rm", "mkfs", "dd ", "shutdown", "reboot", "halt",
    "poweroff", "kill", "killall", "pkill", "chmod", "chown", "mkdir", "touch", "truncate",
    "systemctl restart", "systemctl stop", "systemctl disable", "systemctl start",
    "systemctl enable", "systemctl reset-failed",
];

fn first_token(command: &str) -> String {
    command
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// 白名单 + 写模式 + rm-rf 安全网三重闸。任一不过即拒绝（不执行）。
fn validate_readonly(command: &str) -> Result<(), String> {
    let bin = first_token(command);
    if !ALLOWED_BINS.contains(&bin.as_str()) {
        return Err(format!(
            "拒绝执行：`{bin}` 不在只读白名单内。允许: {}",
            ALLOWED_BINS.join(", ")
        ));
    }
    // systemctl 单独再查子命令（ALLOWED_BINS 有 systemctl，但只准只读子命令）
    for pat in WRITE_PATTERNS {
        if command.contains(pat) {
            return Err(format!("拒绝执行：检测到写/危险操作片段 `{pat}`。本工具仅允许只读排查。"));
        }
    }
    // 复用既有 rm-rf 空变量/危险根检测（纵深防御）
    if let Err(e) = safety::scan_rm_command(command) {
        return Err(e.to_string());
    }
    Ok(())
}

fn clamp_output(s: String) -> String {
    if s.chars().count() <= OUTPUT_CLAMP_CHARS {
        s
    } else {
        let head: String = s.chars().take(OUTPUT_CLAMP_CHARS).collect();
        format!("{head}\n...[输出已截断，超过 {OUTPUT_CLAMP_CHARS} 字符]")
    }
}

/// 分发并执行一次工具调用，返回结构化结果。
/// Failed 状态 = 动作未发生（拦截/缺参/未知工具/超时/IO 失败），调用方据此 fail-loud。
pub async fn dispatch(name: &str, args: &serde_json::Value) -> ToolOutcome {
    match name {
        "run_readonly" => {
            let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
                return ToolOutcome::failed("工具错误：缺少 command 参数");
            };
            if let Err(rej) = validate_readonly(command) {
                return ToolOutcome::failed(format!("⛔ 安全拦截（未执行）：{rej}"));
            }
            run_shell(command).await
        }
        "read_file" => {
            let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                return ToolOutcome::failed("工具错误：缺少 path 参数");
            };
            match tokio::fs::read_to_string(path).await {
                Ok(content) => ToolOutcome::ok(clamp_output(content)),
                // 三态诚实分类（issue #140）：路径不存在 ≠ 不可读，错误消息可区分
                Err(e) => match classify_io_error(&e) {
                    ToolResultKind::PathMissing => ToolOutcome::failed(format!(
                        "读取失败（路径不存在）：{path}"
                    )),
                    _ => ToolOutcome::failed(format!("读取失败（不可读：{}）：{path}", e)),
                },
            }
        }
        other => ToolOutcome::failed(format!("工具错误：未知工具 `{other}`")),
    }
}

async fn run_shell(command: &str) -> ToolOutcome {
    let cmd = command.to_string();
    let result = timeout(Duration::from_secs(CMD_TIMEOUT_SECS), async move {
        tokio::task::spawn_blocking(move || {
            Command::new("bash").arg("-c").arg(&cmd).output()
        })
        .await
    })
    .await;

    match result {
        Err(_) => ToolOutcome::failed(format!("超时：命令执行超过 {CMD_TIMEOUT_SECS}s，已终止")),
        Ok(Err(join_err)) => ToolOutcome::failed(format!("执行失败：{join_err}")),
        Ok(Ok(Ok(out))) => {
            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&out.stdout));
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.trim().is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str("[stderr]\n");
                combined.push_str(&stderr);
            }
            if combined.trim().is_empty() {
                // issue #140：非零退出 + 无输出不许谎报「成功退出」——诚实呈现退出码语义
                let code = out.status.code().unwrap_or(-1);
                combined = empty_output_verdict(code);
                // exit=0 无输出 = 合法空结果（grep 无命中）；非零退出 = 动作未按预期发生
                if code == 0 {
                    ToolOutcome::ok(combined)
                } else {
                    ToolOutcome::failed(combined)
                }
            } else if !out.status.success() {
                let code = out.status.code().unwrap_or(-1);
                ToolOutcome::failed(format!("exit={code}\n{combined}"))
            } else {
                ToolOutcome::ok(clamp_output(combined))
            }
        }
        Ok(Ok(Err(e))) => ToolOutcome::failed(format!("执行失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readonly_runs_df() {
        let out = dispatch("run_readonly", &json!({"command": "df -h /"})).await;
        assert_eq!(out.status, ToolStatus::Ok, "got: {out:?}");
        assert!(out.content.contains("/") || out.content.contains("Filesystem"));
    }

    #[tokio::test]
    async fn rejects_non_whitelisted_bin() {
        let out = dispatch("run_readonly", &json!({"command": "python3 -c 'print(1)'"})).await;
        assert_eq!(out.status, ToolStatus::Failed);
        assert!(out.content.contains("安全拦截"));
    }

    #[tokio::test]
    async fn rejects_write_even_with_allowed_bin() {
        let out = dispatch("run_readonly", &json!({"command": "systemctl restart nginx"})).await;
        assert_eq!(out.status, ToolStatus::Failed);
        assert!(out.content.contains("安全拦截"));
    }

    #[tokio::test]
    async fn rejects_redirect() {
        let out = dispatch("run_readonly", &json!({"command": "ls > /tmp/x"})).await;
        assert_eq!(out.status, ToolStatus::Failed);
        assert!(out.content.contains("安全拦截"));
    }

    #[tokio::test]
    async fn read_file_works() {
        let out = dispatch("read_file", &json!({"path": "/etc/hostname"})).await;
        assert_eq!(out.status, ToolStatus::Ok);
        assert!(!out.content.trim().is_empty());
    }

    #[tokio::test]
    async fn read_file_missing_reports_error() {
        let out = dispatch("read_file", &json!({"path": "/nonexistent-xyz-123"})).await;
        assert_eq!(out.status, ToolStatus::Failed);
        assert!(out.content.contains("读取失败"));
    }

    // ---- issue #140：三态诚实分类，失败路径测试钉死 ----

    #[test]
    fn io_error_not_found_is_path_missing() {
        let e = std::fs::read_to_string("/nonexistent-xyz-123").unwrap_err();
        assert_eq!(classify_io_error(&e), ToolResultKind::PathMissing);
    }

    #[test]
    fn io_error_directory_is_unreadable_not_missing() {
        // 目录存在但 read_to_string 读不了 → Unreadable，不得折叠进 PathMissing
        let e = std::fs::read_to_string("/tmp").unwrap_err();
        assert_eq!(classify_io_error(&e), ToolResultKind::Unreadable);
    }

    #[tokio::test]
    async fn read_file_missing_says_path_missing() {
        let out = dispatch("read_file", &json!({"path": "/nonexistent-xyz-123"})).await;
        assert_eq!(out.status, ToolStatus::Failed);
        assert!(out.content.contains("路径不存在"), "got: {:?}", out);
        assert!(!out.content.contains("不可读"), "got: {:?}", out);
    }

    #[tokio::test]
    async fn read_file_directory_says_unreadable() {
        let out = dispatch("read_file", &json!({"path": "/tmp"})).await;
        assert_eq!(out.status, ToolStatus::Failed);
        assert!(out.content.contains("不可读"), "got: {:?}", out);
        assert!(!out.content.contains("路径不存在"), "got: {:?}", out);
    }

    #[test]
    fn zero_exit_no_output_is_empty_match_not_success_lie() {
        let v = empty_output_verdict(0);
        assert!(v.contains("exit=0") && v.contains("合法"), "got: {v}");
        assert!(!v.contains("成功退出"), "不许谎报成功: {v}");
    }

    #[test]
    fn nonzero_exit_no_output_is_not_reported_as_success() {
        // grep 无命中 exit=1 + 零输出的旧实现会打出「命令成功退出，exit=1」——语义自欺
        let v = empty_output_verdict(1);
        assert!(v.contains("exit=1") && v.contains("非零退出"), "got: {v}");
        assert!(!v.contains("成功"), "不许谎报成功: {v}");
    }

    #[tokio::test]
    async fn grep_no_match_reports_nonzero_exit_honestly() {
        // 端到端失败路径：真实 shell 探针，exit=1 零输出不得含「成功」
        let out = dispatch("run_readonly", &json!({"command": "grep -c zzzzqqqq /etc/hostname"})).await;
        assert_eq!(out.status, ToolStatus::Failed, "非零退出不得标 Ok: {out:?}");
        assert!(!out.content.contains("成功"), "got: {:?}", out);
        assert!(out.content.contains("exit=1"), "got: {:?}", out);
    }

    // ---- issue #146 / backlog 2026-10-10：no-op fail-loud 二态 ----

    #[test]
    fn empty_output_zero_exit_is_ok_not_failed() {
        // grep exit=0 无输出 = 合法空结果，仍是 Ok（#140 语义不回退）
        let v = empty_output_verdict(0);
        assert!(v.contains("exit=0") && v.contains("合法"));
    }

    #[tokio::test]
    async fn noop_paths_all_fail_loud() {
        // 拦截（动作未发生）
        let blocked = dispatch("run_readonly", &json!({"command": "rm -rf /tmp/zzz"})).await;
        assert_eq!(blocked.status, ToolStatus::Failed);
        // 缺参（动作未发生）
        let missing = dispatch("run_readonly", &json!({})).await;
        assert_eq!(missing.status, ToolStatus::Failed);
        let missing_path = dispatch("read_file", &json!({})).await;
        assert_eq!(missing_path.status, ToolStatus::Failed);
        // 未知工具（动作未发生）
        let unknown = dispatch("no_such_tool", &json!({})).await;
        assert_eq!(unknown.status, ToolStatus::Failed);
        // 路径缺失（动作未发生）
        let bad_path = dispatch("read_file", &json!({"path": "/nonexistent-xyz-123"})).await;
        assert_eq!(bad_path.status, ToolStatus::Failed);
        // 非零退出且无输出（动作未按预期发生）
        let nonzero = dispatch("run_readonly", &json!({"command": "grep -c zzzzqqqq /etc/hostname"})).await;
        assert_eq!(nonzero.status, ToolStatus::Failed);
        // 真实成功（对照：动作已发生）
        let good = dispatch("run_readonly", &json!({"command": "date"})).await;
        assert_eq!(good.status, ToolStatus::Ok);
    }

    #[test]
    fn first_token_strips_path() {
        assert_eq!(first_token("/usr/bin/ls -la"), "ls");
        assert_eq!(first_token("df -h"), "df");
    }

    #[test]
    fn unknown_tool_errors() {
        // 同步小工具直接走 dispatch 会进 async；这里只验证白名单纯函数
        assert!(validate_readonly("rm -rf /").is_err());
        assert!(validate_readonly("uptime").is_ok());
    }
}
