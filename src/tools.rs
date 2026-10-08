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

/// 单次命令执行上限（秒）——防止挂死/无限输出型命令。
const CMD_TIMEOUT_SECS: u64 = 15;
/// 工具输出回喂 LLM 的字符上限——防爆 prompt（复用 clamp 哲学）。
const OUTPUT_CLAMP_CHARS: usize = 4000;

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

/// 分发并执行一次工具调用，返回回喂给 LLM 的结果字符串。
pub async fn dispatch(name: &str, args: &serde_json::Value) -> String {
    match name {
        "run_readonly" => {
            let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
                return "工具错误：缺少 command 参数".to_string();
            };
            if let Err(rej) = validate_readonly(command) {
                return format!("⛔ 安全拦截（未执行）：{rej}");
            }
            run_shell(command).await
        }
        "read_file" => {
            let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                return "工具错误：缺少 path 参数".to_string();
            };
            match tokio::fs::read_to_string(path).await {
                Ok(content) => clamp_output(content),
                Err(e) => format!("读取失败：{e}"),
            }
        }
        other => format!("工具错误：未知工具 `{other}`"),
    }
}

async fn run_shell(command: &str) -> String {
    let cmd = command.to_string();
    let result = timeout(Duration::from_secs(CMD_TIMEOUT_SECS), async move {
        tokio::task::spawn_blocking(move || {
            Command::new("bash").arg("-c").arg(&cmd).output()
        })
        .await
    })
    .await;

    match result {
        Err(_) => format!("超时：命令执行超过 {CMD_TIMEOUT_SECS}s，已终止"),
        Ok(Err(join_err)) => format!("执行失败：{join_err}"),
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
                combined = format!("(命令成功退出，exit={}，无输出)", out.status.code().unwrap_or(-1));
            } else if !out.status.success() {
                combined = format!("exit={}\n{combined}", out.status.code().unwrap_or(-1));
            }
            clamp_output(combined)
        }
        Ok(Ok(Err(e))) => format!("执行失败：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readonly_runs_df() {
        let out = dispatch("run_readonly", &json!({"command": "df -h /"})).await;
        assert!(out.contains("/") || out.contains("Filesystem"));
    }

    #[tokio::test]
    async fn rejects_non_whitelisted_bin() {
        let out = dispatch("run_readonly", &json!({"command": "python3 -c 'print(1)'"})).await;
        assert!(out.contains("安全拦截"));
    }

    #[tokio::test]
    async fn rejects_write_even_with_allowed_bin() {
        let out = dispatch("run_readonly", &json!({"command": "systemctl restart nginx"})).await;
        assert!(out.contains("安全拦截"));
    }

    #[tokio::test]
    async fn rejects_redirect() {
        let out = dispatch("run_readonly", &json!({"command": "ls > /tmp/x"})).await;
        assert!(out.contains("安全拦截"));
    }

    #[tokio::test]
    async fn read_file_works() {
        let out = dispatch("read_file", &json!({"path": "/etc/hostname"})).await;
        assert!(!out.trim().is_empty());
    }

    #[tokio::test]
    async fn read_file_missing_reports_error() {
        let out = dispatch("read_file", &json!({"path": "/nonexistent-xyz-123"})).await;
        assert!(out.contains("读取失败"));
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
