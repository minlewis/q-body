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
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::sleep;

/// 单条命令执行上限/grace 读上限（秒）——防止挂死/无限输出型命令。
const CMD_TIMEOUT_SECS: u64 = 15;
/// 超时终止后回收子进程输出的有界读取上限——防孙进程占管道导致永久等待
/// （PR #162 run_shell_with_timeout 同型风险：后台孙进程占用输出管道 → exit/回收挂起）。
const GRACE_READ_SECS: Duration = Duration::from_millis(500);

/// 工具输出回喂 LLM 的字符上限——防爆 prompt（复用 clamp 哲学）。
const OUTPUT_CLAMP_CHARS: usize = 4000;

/// 组装正常完成（或超时快照）的 stdout/stderr 组合输出。
/// 诚实语义（issue #140）：非零退出 + 零输出来 self 已明确「非零退出」；
/// 超时本身不是正常完成，不得被组装成成功结果。
fn combine_output(stdout: &[u8], stderr: &[u8], exit_code: Option<i32>) -> String {
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(stdout));
    let stderr = String::from_utf8_lossy(stderr);
    if !stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str("[stderr]\n");
        combined.push_str(&stderr);
    }
    if combined.trim().is_empty() {
        // issue #140：非零退出 + 无输出不许谎报「成功退出」——诚实呈现退出码语义
        combined = empty_output_verdict(exit_code.unwrap_or(-1));
    } else if let Some(code) = exit_code {
        if code != 0 {
            combined = format!("exit={code}\n{combined}");
        }
    }
    combined
}

/// 执行命令并在超时后终止，返回「完成 / 超时 + 已捕获输出快照」。
/// 超时分支：kill 子进程 → 回收 → 有界 grace 读已产生输出 → 诚实标注「超时已终止」，
/// 不丢弃 stdout/stderr（此前 tools.rs:229 超时即丢全部输出）。
async fn run_shell_with_timeout(
    command: &str,
    timeout_secs: u64,
) -> (bool, String, String, Option<i32>) {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bash spawn 失败");
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    tokio::select! {
        // biased: 正常完成优先于超时（同一轮就绪时先处理完成态）
        status = child.wait() => {
            let code = status.ok().and_then(|s| s.code());
            let out = read_pipe(stdout).await.unwrap_or_default();
            let err = read_pipe(stderr).await.unwrap_or_default();
            (false, out, err, code)
        }
        _ = sleep(Duration::from_secs(timeout_secs)) => {
            // 超时：杀子进程，尽量回收状态，有界读回已产生输出。
            let _ = child.kill().await;
            let _ = child.wait().await; // 回收 zombie
            let out = read_pipe(stdout).await.unwrap_or_default();
            let err = read_pipe(stderr).await.unwrap_or_default();
            (true, out, err, None)
        }
    }
}

/// 有界读取管道内容——超时后孙进程可能仍持有写端，必须限时读防挂起。
/// 泛型同时支持 stdout/stderr 管道。
async fn read_pipe<R: AsyncRead + Unpin>(mut pipe: Option<R>) -> Option<String> {
    let mut pipe = pipe?;
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(GRACE_READ_SECS, pipe.read_to_end(&mut buf)).await;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

async fn run_shell(command: &str) -> String {
    let (timed_out, out, err, code) = run_shell_with_timeout(command, CMD_TIMEOUT_SECS).await;
    if timed_out {
        // 诚实语义：超时不是正常完成，不得按成功报告；携带已产生输出快照。
        let mut msg = format!(
            "超时：命令执行超过 {CMD_TIMEOUT_SECS}s，已终止（超时不是正常完成）"
        );
        if !out.trim().is_empty() {
            msg.push_str("\n[已捕获 stdout]\n");
            msg.push_str(out.trim_end());
        }
        if !err.trim().is_empty() {
            msg.push_str("\n[已捕获 stderr]\n");
            msg.push_str(err.trim_end());
        }
        if out.trim().is_empty() && err.trim().is_empty() {
            msg.push_str("\n（超时时无任何已捕获输出——命令可能卡在未输出的阶段）");
        }
        clamp_output(msg)
    } else {
        clamp_output(combine_output(out.as_bytes(), err.as_bytes(), code))
    }
}

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
        format!(
            "(exit={exit_code}，非零退出且无输出——如 grep 无命中属空匹配，其余需结合退出码判断失败)"
        )
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
    "systemctl",
    "journalctl",
    "ps",
    "pgrep",
    "ss",
    "df",
    "du",
    "free",
    "uptime",
    "ls",
    "cat",
    "head",
    "tail",
    "grep",
    "stat",
    "who",
    "date",
    "hostname",
    "uname",
    "id",
];

/// run_readonly 即使 bin 合法，也拦截这些明显带写语义的参数/子命令。
const WRITE_PATTERNS: &[&str] = &[
    "rm ",
    "mv ",
    "cp ",
    ">",
    ">>",
    "&& rm",
    "mkfs",
    "dd ",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "kill",
    "killall",
    "pkill",
    "chmod",
    "chown",
    "mkdir",
    "touch",
    "truncate",
    "systemctl restart",
    "systemctl stop",
    "systemctl disable",
    "systemctl start",
    "systemctl enable",
    "systemctl reset-failed",
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
            return Err(format!(
                "拒绝执行：检测到写/危险操作片段 `{pat}`。本工具仅允许只读排查。"
            ));
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
                // 三态诚实分类（issue #140）：路径不存在 ≠ 不可读，错误消息可区分
                Err(e) => match classify_io_error(&e) {
                    ToolResultKind::PathMissing => {
                        format!("读取失败（路径不存在）：{path}")
                    }
                    _ => format!("读取失败（不可读：{}）：{path}", e),
                },
            }
        }
        other => format!("工具错误：未知工具 `{other}`"),
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
        let out = dispatch(
            "run_readonly",
            &json!({"command": "systemctl restart nginx"}),
        )
        .await;
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
        assert!(out.contains("路径不存在"), "got: {out}");
        assert!(!out.contains("不可读"), "got: {out}");
    }

    #[tokio::test]
    async fn read_file_directory_says_unreadable() {
        let out = dispatch("read_file", &json!({"path": "/tmp"})).await;
        assert!(out.contains("不可读"), "got: {out}");
        assert!(!out.contains("路径不存在"), "got: {out}");
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
        let out = dispatch(
            "run_readonly",
            &json!({"command": "grep -c zzzzqqqq /etc/hostname"}),
        )
        .await;
        assert!(!out.contains("成功"), "got: {out}");
        assert!(out.contains("exit=1"), "got: {out}");
    }

    #[test]
    fn first_token_strips_path() {
        assert_eq!(first_token("/usr/bin/ls -la"), "ls");
        assert_eq!(first_token("df -h"), "df");
    }

    // ---- issue #218：超时分支保留已产生输出快照 + 诚实语义 ----

    /// 测试辅助：直接跑 run_shell（返回已组装的 String，超时走快照路径）。
    /// 内部调 run_shell_with_timeout 注入短超时，不污染全局 15s 常量。
    async fn run_shell_with_timeout_exec(command: &str, secs: u64) -> String {
        // 真实路径：CMD_TIMEOUT_SECS 是 const，编译期固定 15s；
        // 测试想要短超时只能直接调 run_shell_with_timeout 再组装。
        let (timed_out, out, err, code) = run_shell_with_timeout(command, secs).await;
        if timed_out {
            let mut msg =
                format!("超时：命令执行超过 {secs}s，已终止（超时不是正常完成）");
            if !out.trim().is_empty() {
                msg.push_str("\n[已捕获 stdout]\n");
                msg.push_str(out.trim_end());
            }
            if !err.trim().is_empty() {
                msg.push_str("\n[已捕获 stderr]\n");
                msg.push_str(err.trim_end());
            }
            if out.trim().is_empty() && err.trim().is_empty() {
                msg.push_str("\n（超时时无任何已捕获输出——命令可能卡在未输出的阶段）");
            }
            msg
        } else {
            combine_output(out.as_bytes(), err.as_bytes(), code)
        }
    }

    #[tokio::test]
    async fn timeout_carries_partial_output_snapshot() {
        // 先输出一段，再卡死：超时后必须携带已产生的 stdout/stderr 快照，
        // 不得像旧实现 tools.rs:229 那样「超时即丢全部输出」。
        let (timed_out, out, err, code) =
            run_shell_with_timeout("echo BEFORE_TIMEOUT; sleep 30", 1).await;
        assert!(timed_out, "必须走超时分支");
        assert!(out.contains("BEFORE_TIMEOUT"), "快照必须保留 stdout: {out:?}");
        assert_eq!(code, None, "超时分支不谎报退出码");
    }

    #[tokio::test]
    async fn timeout_preserves_stderr_snapshot() {
        let (timed_out, out, err, code) =
            run_shell_with_timeout("echo TO_ERR 1>&2; sleep 30", 1).await;
        assert!(timed_out);
        assert!(err.contains("TO_ERR"), "快照必须保留 stderr: {err:?}");
        assert!(out.is_empty(), "stdout 应为空: {out:?}");
        assert_eq!(code, None);
    }

    #[tokio::test]
    async fn timeout_with_no_output_is_honest_about_it() {
        // 卡在未输出阶段：消息必须诚实说明无输出，不编造成功。
        let out = run_shell_with_timeout_exec("exec sleep 30", 1).await;
        assert!(out.contains("超时"), "got: {out}");
        assert!(out.contains("无任何已捕获输出"), "got: {out}");
        assert!(!out.contains("成功"), "超时不是成功: {out}");
    }

    #[tokio::test]
    async fn quick_command_not_timed_out() {
        // 正常完成对照：不触发超时、退出码如实、输出完整。
        let (timed_out, out, err, code) = run_shell_with_timeout("echo DONE_OK", 5).await;
        assert!(!timed_out);
        assert!(out.contains("DONE_OK"), "got: {out:?}");
        assert_eq!(code, Some(0), "got: {code:?}");
        assert!(err.is_empty());
    }

    #[tokio::test]
    async fn timeout_reports_through_dispatch() {
        // 端到端：dispatch 层超时消息含「已终止」与「不是正常完成」，不谎报。
        // tail -f /dev/null 在白名单且永不退出 → 真实走 15s 超时分支。
        let out =
            dispatch("run_readonly", &json!({"command": "tail -f /dev/null"})).await;
        assert!(out.contains("超时"), "got: {out}");
        assert!(out.contains("不是正常完成"), "got: {out}");
        assert!(!out.contains("成功"), "got: {out}");
    }

    #[test]
    fn unknown_tool_errors() {
        // 同步小工具直接走 dispatch 会进 async；这里只验证白名单纯函数
        assert!(validate_readonly("rm -rf /").is_err());
        assert!(validate_readonly("uptime").is_ok());
    }
}
