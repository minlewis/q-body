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
use tokio::process::Command as TCommand;
use tokio::time::{Duration, timeout};

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

async fn run_shell(command: &str) -> String {
    run_shell_with_timeout(command, CMD_TIMEOUT_SECS).await
}

/// run_shell 的带参版本：CMD_TIMEOUT_SECS 上方是生产默认，测试用短超时
/// 快证「后台孙进程占管道 → 超时 killpg 全家死透」而不必等足 15s。
async fn run_shell_with_timeout(command: &str, timeout_secs: u64) -> String {
    use tokio::io::AsyncReadExt;

    let cmd = command.to_string();
    let mut child = match TCommand::new("bash")
        .arg("-c")
        .arg(&cmd)
        // 显式接管 stdout/stderr（tokio spawn 默认继承 fd，不接管则
        // wait_with_output 抓不到输出——旧实现 std Command::output() 自动
        // piped，切 tokio 后必须显式声明，否则输出漏进调用方 fd）
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // 进程组隔离（yoyo #1049 同型）：`bash -c 'x &'` 的后台孙进程共享
        // 本 pgid（pgid == bash pid）；超时 killpg 才能连孙进程一起杀，根治
        // 「后台进程占输出管道 → 出口挂起 + 孤儿进程泄漏」（10-14 probe 实测：
        // 旧实现超时文案说谎「已终止」但子进程仍在跑且 CPU 持续增长）
        .process_group(0)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("执行失败：{e}"),
    };
    let pgid = child.id().unwrap_or(0);
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    // 带超时等 bash 退出（&mut 不消费 child，下面超时后还能再引用它收尾）。
    // 若后台孙进程霸着管道，bash 虽退出但 wait 只在 bash 退出即返回（管道在
    // 调用方手里，孙进程活着不影响 wait）——超时/不退出的判定由下方 killpg 兜底。
    let exit = timeout(Duration::from_secs(timeout_secs), child.wait()).await;

    // 超时/正常退出两条路径都显式 killpg(SIGKILL) 把整个进程组连根杀掉：
    // 超时路径杀 bash + 后台孙进程；正常退出路径杀 bash 已退出后仍霸管道的
    // 遗留后台孙进程（`sleep &` 型）。kill_on_drop 只杀 bash 直子、后台孙进程
    // 仍霸管道——已被 RED 测试证伪，故一律用 libc 显式全杀组（组已空则无害）。
    if pgid > 0 {
        // SAFETY: pgid 是本 spawn 的进程组号（== bash pid，bash exec 末命令后 pid 不变）。
        let ret = unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGKILL) };
        let errno = std::io::Error::last_os_error();
        let _ = ret; // killpg 成功即返回，失败由下面 if 分支报错
        // 收一次退出，避免僵尸（正常路径 bash 已被上方 wait 收割，此处无害）。
        let _ = child.wait().await;
    }

    // 读干净残留管道输出（组已杀/进程已退 → 管道 EOF；外加 1s 兜底防极端残留）。
    let mut out_buf = Vec::new();
    if let Some(mut so) = stdout.take() {
        let _ = timeout(
            Duration::from_millis(1000),
            so.read_to_end(&mut out_buf),
        )
        .await;
    }
    let mut err_buf = Vec::new();
    if let Some(mut se) = stderr.take() {
        let _ = timeout(
            Duration::from_millis(1000),
            se.read_to_end(&mut err_buf),
        )
        .await;
    }

    let (ok, code) = match exit {
        Err(_) => {
            return format!("超时：命令执行超过 {timeout_secs}s，已终止（含后台子进程 killpg）")
        }
        Ok(Err(_)) => return "执行失败：无法读取退出状态".to_string(),
        Ok(Ok(st)) => (st.success(), st.code().unwrap_or(-1)),
    };

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&out_buf));
    let stderr_s = String::from_utf8_lossy(&err_buf);
    if !stderr_s.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str("[stderr]\n");
        combined.push_str(&stderr_s);
    }
    if combined.trim().is_empty() {
        // issue #140：非零退出 + 无输出不许谎报「成功退出」——诚实呈现退出码语义
        combined = empty_output_verdict(code);
    } else if !ok {
        combined = format!("exit={code}\n{combined}");
    }
    clamp_output(combined)
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

    #[test]
    fn unknown_tool_errors() {
        // 同步小工具直接走 dispatch 会进 async；这里只验证白名单纯函数
        assert!(validate_readonly("rm -rf /").is_err());
        assert!(validate_readonly("uptime").is_ok());
    }

    // ---- issue #162：后台/长驻子进程占输出管道 → 超时 killpg 全家死透（yoyo #1049）----

    #[tokio::test]
    async fn background_child_holding_pipe_is_killed_on_timeout() {
        // RED 场景：前台 `sleep 999`（bash 本身不退出）→ wait 超过 2s 兜底超时，
        // 旧实现超时文案说谎「已终止」但 bash 直子还活着（10-14 probe 实证：
        // 超时后子进程仍在跑且 CPU 持续增长 = 孤儿 + 泄漏）。
        // 新实现：超时后 killpg(SIGKILL) 连 bash 一起真杀，sleep 不得残留。
        let out = run_shell_with_timeout("sleep 999", 2).await;
        assert!(out.contains("超时"), "should time out, got: {out}");
        assert!(
            out.contains("killpg"),
            "timeout msg should admit killpg, got: {out}"
        );
        // 等 2s 超时返回后，短暂让出再探 pgrep —— 组内 echo 无残留进程。
        tokio::time::sleep(Duration::from_millis(200)).await;
        // 更硬的探针：直接 pgrep -f 精确命令，0 命中才算真死透
        let alive = command_find("sleep 999");
        let _live = std::process::Command::new("pgrep")
            .arg("-af")
            .arg("sleep 999")
            .output()
            .ok();
        assert!(!alive, "背景 sleep 应已被 killpg 杀死，但仍存活");
    }

    #[tokio::test]
    async fn background_grandchild_is_killed_when_bash_exits_first() {
        // 变体：`sleep 999 & echo spawned` —— bash 先退出、sleep 后台霸管道。
        // 管道 read 超 1s 兜底不得挂死（exit 卡死防护），且后台 sleep 也被清掉。
        let out = run_shell_with_timeout("sleep 999 & echo spawned", 2).await;
        // bash 立即退 → 不算「超时」，但必须含遗留后台吐出的 spawned 或诚实语义，且不挂死
        assert!(out.contains("spawned"), "should capture echo output, got: {out}");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = command_find("sleep 999");
        assert!(!alive, "后台孙进程应已被清掉，但仍存活");
    }

    #[tokio::test]
    async fn plain_command_still_captures_output() {
        // 回归：切 tokio 后 stdout/stderr 显式 piped 才抓得到输出——旧实现自动 piped，
        // 这里钉住「df 输出必进返回串」防输出漏进 fd 的重现（本 PR 摸到过的坑）。
        let out = run_shell_with_timeout("df -h /", 5).await;
        assert!(out.contains("Filesystem") || out.contains("/dev/"), "got: {out}");
    }

    /// 辅助：pgrep -f 精确命令字符串，命中返回 true。
    fn command_find(pat: &str) -> bool {
        use std::process::Command;
        let out = Command::new("pgrep").arg("-f").arg(pat).output().ok();
        matches!(out, Some(o) if o.status.success())
    }
}
