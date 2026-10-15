//! tools.rs — 真实工具执行层（agent 对环境采取行动的「手」）
//!
//! TAO 三问：
//! 1. 接线了吗？→ 是，dispatch_tool 被 agent loop 调用，真跑 shell/读文件。
//! 2. 能进 SOUL.md 吗？→ 不能，进程执行/超时/截断是代码逻辑，LLM 无法保证隔离。
//! 3. 损掉了什么？→ 命令执行白名单 + 超时 + 输出截断 + 复用 validator 安全网，
//!    砍掉了「任意 shell 直通」这个最危险能力，换成显式安全工具。
//!    （issue #152 收紧：再砍掉「字面 contains 匹配 / 多段命令 / 管道 / 变量展开 /
//!    命令替换」一整类能力，换成壳词法切分 + 只读子命令闭集——写子命令再也无法
//!    靠空白/引号等价写法规避，代价是读路径上更保守、宁可误拒。）
//!
//! 第一版（运维手）只给最小工具，范围驱动，不做通用 shell。

use crate::validator::command as safety;
use crate::validator::shellgate;
use serde_json::json;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
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

/// run_readonly 写模式黑名单——**仍在**，作为子命令闭集之外的纵深防御：
/// 拦掉跨 bin 的写语义参数（重定向 `>` 已由词法层拒绝，这里保留兜底）。
const WRITE_PATTERNS: &[&str] = &[
    "rm", "mv", "cp", "mkfs", "dd", "shutdown", "reboot", "halt", "poweroff", "kill", "killall",
    "pkill", "chmod", "chown", "mkdir", "touch", "truncate",
];

/// 白名单 + 词法门禁 + rm-rf 安全网三重闸。任一不过即拒绝（不执行）。
///
/// issue #152：旧实现用 `command.contains("systemctl restart")` 字面匹配，实测被
/// `systemctl  restart` / `systemctl\tstop` / `systemctl 'restart'` / `systemctl --no-block restart`
/// 等 7 种 bash 词法等价写法绕过。现改为「壳词法切分 → 单段校验 → bin 白名单 →
/// 只读子命令闭集（默认拒绝）」，写子命令再也无法靠等价写法藏身。
fn validate_readonly(command: &str) -> Result<(), String> {
    let seg = shellgate::check_readonly(command, ALLOWED_BINS).map_err(|e| e.to_string())?;

    // 纵深防御：跨 bin 的写语义词（词级精确比对，不再用 `contains` 子串匹配——
    // 子串匹配会把 `--lines 5` 这类参数误判成 `ls`，也会把 `ls` 误判成 `kill`）。
    for w in &seg.words {
        let stem = w.trim_matches(|c: char| c == '-' || c == '/' || c.is_ascii_digit());
        if WRITE_PATTERNS.contains(&stem) {
            return Err(format!("检测到写/危险操作词 `{w}`。本工具仅允许只读排查。"));
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

/// 对独立进程组发 SIGKILL（负 pgid + `--`，且**不经 shell**）。
///
/// 实测坑（2026-10-14）：`bash -c "kill -KILL -- -PGID"` 会被 shell 当选项解析，
/// 报 `Illegal number: -` 而静默失败——必须直接 exec `kill` 并让它拿到 `-PGID` 参数。
/// 返回是否确认已发出（`ESRCH` 也算成功：目标已自行退出）。
fn kill_process_group(pgid: u32) -> bool {
    if pgid == 0 {
        return false;
    }
    Command::new("kill")
        .arg("-KILL")
        .arg("--")
        .arg(format!("-{pgid}"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn run_shell(command: &str) -> String {
    let cmd = command.to_string();
    // 子进程自成一进程组（process_group(0) → pgid == pid），
    // 这样超时时可以对整组发信号，而不是只放弃 await。
    let pid_slot = Arc::new(AtomicI32::new(0));
    let slot = Arc::clone(&pid_slot);

    let handle = tokio::task::spawn_blocking(move || {
        let mut c = Command::new("bash");
        c.arg("-c")
            .arg(&cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c.process_group(0);
        match c.spawn() {
            Err(e) => Err(e.to_string()),
            Ok(child) => {
                slot.store(child.id() as i32, Ordering::SeqCst);
                child.wait_with_output().map_err(|e| e.to_string())
            }
        }
    });

    match timeout(Duration::from_secs(CMD_TIMEOUT_SECS), handle).await {
        // 超时：真杀进程组，并让文案与实测同源（issue #152）。
        // 旧实现只放弃 await，子进程继续跑却回「已终止」——文案说谎 + 漏孤儿。
        Err(_) => {
            let pid = pid_slot.load(Ordering::SeqCst) as u32;
            // 边界：spawn 尚未完成就被超时（pid 仍为 0）→ 无法定向 kill，如实说明。
            if pid == 0 {
                return format!(
                    "超时：命令执行超过 {CMD_TIMEOUT_SECS}s，但子进程尚未取得 pid，进程组未能定向终止（可能泄漏）"
                );
            }
            let killed = kill_process_group(pid);
            // 竞态：发信号前后子进程可能刚好自行结束——用 pid 是否仍在来区分三态，
            // 不把「已自行退出」谎报成「已终止」，也不把「杀失败」说成成功。
            let still_alive = Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            let t = CMD_TIMEOUT_SECS;
            if !still_alive {
                format!("超时：命令执行超过 {t}s，子进程已终止（SIGKILL pgid={pid}；或已自行退出）")
            } else if killed {
                format!("超时：命令执行超过 {t}s，已向进程组发出 SIGKILL（pgid={pid}）")
            } else {
                format!(
                    "超时：命令执行超过 {t}s，进程组终止失败（pgid={pid} 可能泄漏，kill 返非零）"
                )
            }
        }
        Ok(Err(join_err)) => format!("执行失败：{join_err}"),
        Ok(Ok(Err(e))) => format!("执行失败：{e}"),
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
                combined = empty_output_verdict(out.status.code().unwrap_or(-1));
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
        for cmd in ["systemctl restart nginx", "ls > /tmp/x"] {
            let out = dispatch("run_readonly", &json!({"command": cmd})).await;
            assert!(out.contains("安全拦截"), "未拦截 {cmd} → {out}");
        }
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
    fn io_error_kinds_are_not_folded_together() {
        // NotFound → PathMissing；目录读不了 → Unreadable，不得折叠进 PathMissing
        let missing = std::fs::read_to_string("/nonexistent-xyz-123").unwrap_err();
        assert_eq!(classify_io_error(&missing), ToolResultKind::PathMissing);
        let dir = std::fs::read_to_string("/tmp").unwrap_err();
        assert_eq!(classify_io_error(&dir), ToolResultKind::Unreadable);
    }

    #[tokio::test]
    async fn read_file_errors_are_distinguishable() {
        let missing = dispatch("read_file", &json!({"path": "/nonexistent-xyz-123"})).await;
        assert!(missing.contains("路径不存在"), "got: {missing}");
        assert!(!missing.contains("不可读"), "got: {missing}");
        let dir = dispatch("read_file", &json!({"path": "/tmp"})).await;
        assert!(dir.contains("不可读"), "got: {dir}");
        assert!(!dir.contains("路径不存在"), "got: {dir}");
    }

    #[test]
    fn exit_codes_are_reported_honestly() {
        // exit=0 无输出是合法空结果；exit!=0 无输出必须显式标非零退出，都不许说「成功」
        let zero = empty_output_verdict(0);
        assert!(
            zero.contains("exit=0") && zero.contains("合法"),
            "got: {zero}"
        );
        assert!(!zero.contains("成功退出"), "不许谎报成功: {zero}");
        let one = empty_output_verdict(1);
        assert!(
            one.contains("exit=1") && one.contains("非零退出"),
            "got: {one}"
        );
        assert!(!one.contains("成功"), "不许谎报成功: {one}");
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
        // 原 first_token 已升级为 shellgate 的 bin 归一化（issue #152）；
        // 覆盖不变：绝对路径 / 目录前缀仍剥到裸 bin 名。
        let seg = shellgate::split_segments("/usr/bin/ls -la").unwrap();
        assert_eq!(seg[0].words[0].rsplit('/').next().unwrap(), "ls");
        assert!(shellgate::check_readonly("df -h", ALLOWED_BINS).is_ok());
    }

    #[test]
    fn unknown_tool_errors() {
        // 同步小工具直接走 dispatch 会进 async；这里只验证白名单纯函数
        assert!(validate_readonly("rm -rf /").is_err());
        assert!(validate_readonly("uptime").is_ok());
    }

    // ---- issue #152：门禁绕过变体（复刻 10-14 探针实录，全部必须拒绝）----

    #[test]
    fn gate_rejects_all_probe_bypass_variants() {
        for cmd in [
            "systemctl restart nginx",                  // 基线：单空格
            "systemctl  restart nginx",                 // 双空格
            "systemctl\tstop nginx",                    // TAB
            "systemctl 'restart' nginx",                // 子命令单引号
            "systemctl \"stop\" nginx",                 // 子命令双引号
            "systemctl ''restart'' nginx",              // 空引号拼接
            "systemctl --no-block restart nginx",       // 选项插在子命令前
            "systemctl --quiet stop nginx",             // 选项变体 stop
            "/usr/bin/systemctl  restart nginx",        // 绝对路径 + 双空格
            "cat /etc/hostname; systemctl  stop nginx", // 分号串联
        ] {
            let out = validate_readonly(cmd);
            assert!(out.is_err(), "绕过写法未被拒绝: {cmd}");
        }
    }

    #[tokio::test]
    async fn gate_rejects_bypass_variants_through_dispatch() {
        // 端到端：绕过写法必须回「安全拦截」，绝不能真执行
        for cmd in [
            "systemctl  restart nginx",
            "systemctl 'restart' nginx",
            "systemctl --no-block restart nginx",
            "cat /etc/hostname; systemctl  stop nginx",
            "cat $FILE",
            "cat `hostname`",
        ] {
            let out = dispatch("run_readonly", &json!({"command": cmd})).await;
            assert!(out.contains("安全拦截"), "未拦截 {cmd} → {out}");
        }
    }

    #[test]
    fn gate_does_not_injure_benign_readonly() {
        for cmd in [
            "df -h /",
            "uptime",
            "ls -la /var/log",
            "cat /etc/hostname",
            "ps aux",
            "systemctl status nginx",
            "systemctl --no-pager is-active q-body",
            "journalctl -u q-body --since '1 hour ago' -n 50",
            "grep -c zzzz /etc/hostname",
        ] {
            assert!(
                validate_readonly(cmd).is_ok(),
                "良性只读命令被误伤: {cmd} → {:?}",
                validate_readonly(cmd)
            );
        }
    }

    // ---- issue #152：超时路径真杀进程组 + 文案与事实同源 ----

    #[tokio::test]
    async fn timeout_reaps_process_group_and_says_so() {
        let marker = format!("QBODY_TIMEOUT_REAP_PROBE_{}", std::process::id());
        // 直接调 run_shell 用一条超时命令（绕过 run_readonly 白名单）
        let cmd = format!("sleep 30; echo {marker}");
        let t0 = std::time::Instant::now();
        let out = timeout(Duration::from_secs(CMD_TIMEOUT_SECS + 10), run_shell(&cmd))
            .await
            .expect("run_shell 自身应在超时后尽快返回");
        let elapsed = t0.elapsed();

        // ① 文案必须如实提到「已终止」且带 pgid —— 不许只说「已终止」而进程还活着
        assert!(out.contains("超时"), "got: {out}");
        assert!(
            out.contains("已终止") || out.contains("已向进程组发出 SIGKILL"),
            "文案未如实说明终止动作: {out}"
        );
        assert!(out.contains("pgid="), "文案缺 pgid 证据: {out}");
        // ② 返回不得等到 sleep 自然结束（否则等于没杀）
        assert!(
            elapsed < Duration::from_secs(20),
            "超时返回过慢，疑似未真杀: {elapsed:?}"
        );

        // ③ 真杀证据：标记进程不得存活（给 1s 让 SIGKILL 生效）
        tokio::time::sleep(Duration::from_secs(1)).await;
        let alive = Command::new("pgrep")
            .arg("-f")
            .arg(&marker)
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);
        // 兜底清理，避免测试自身留下孤儿
        if alive {
            let _ = Command::new("pkill")
                .arg("-KILL")
                .arg("-f")
                .arg(&marker)
                .status();
        }
        assert!(!alive, "超时后子进程仍存活 —— 孤儿未 reap（文案在说谎）");
    }

    #[test]
    fn kill_process_group_rejects_zero_pgid() {
        // pgid=0 会误伤调用方自身进程组 —— 必须拒绝
        assert!(!kill_process_group(0));
    }
}
