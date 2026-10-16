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

/// 路径 provenance 判定（issue #152 同源谓词，两条工具臂共用）。
///
/// 借鉴 yoyo #1024（同一谓词挡每一扇门）+ #1023（fail closed 按 provenance
/// 判定，不按 flag/片段黑名单）：返回 canonicalize 后的绝对路径，仅当
/// canonical 结果落在 ALLOW_ROOTS 且不触碰本地凭据/私钥/令牌存储时放行。
/// deny-by-default — 未知路径、凭据目录一律拒绝，绝不因「白名单 bin 合法」
/// 或「绝对路径不含写片段」放行凭据读取。
pub fn path_provenance(path: &str) -> Result<std::path::PathBuf, String> {
    let raw = std::path::Path::new(path);
    // canonicalize 解析相对路径/符号链接 — 判定永远基于真实落点。
    let canon = raw
        .canonicalize()
        .map_err(|e| format!("路径不可解析（{e}）：{path}"))?;

    // 凭据/私钥/令牌存储 → 无条件拒绝（fail closed 优先）
    for d in DENY_SUBPATHS {
        if canon.starts_with(d) {
            return Err(format!(
                "拒绝读取凭据/私钥/令牌存储路径：{path}（本地敏感区，不进入模型上下文）"
            ));
        }
    }

    // 允许前缀：canonical 绝对路径必须落在任一系统只读/工作区根内
    for root in ALLOW_ROOTS {
        if canon.starts_with(root) {
            return Ok(canon);
        }
    }

    Err(format!(
        "拒绝读取：{path}（不在允许只读路径内；deny-by-default）"
    ))
}

/// 只读读取的允许根（canonicalize 后判断）。系统常规 + q-body 工作区。
const ALLOW_ROOTS: &[&str] = &[
    "/etc",
    "/var",
    "/usr",
    "/proc",
    "/sys",
    "/tmp",
    "/home/ubuntu/q-body",
    "/home/ubuntu/.hermes/skills",
];

/// 即使路径在允许根内，也无条件拒绝的本地敏感子路径（凭据/私钥/令牌）。
const DENY_SUBPATHS: &[&str] = &[
    "/home/ubuntu/.git-credentials",
    "/home/ubuntu/.ssh",
    "/home/ubuntu/.config/gh",
    "/home/ubuntu/.config/tokencost",
    "/home/ubuntu/.hermes/cron",
    "/home/ubuntu/.hermes/secrets",
];

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

/// run_readonly 读侧命令的 provenance 闸（#152）。
///
/// 对命令做朴素 token 化，取出每个「绝对路径形态」token（以 `/` 开头、
/// 非 option/非内联赋值），逐个过 `path_provenance`（fail closed：
/// 任一传递路径落在敏感区/允许域外即整条拒绝，不执行）。
/// 与 read_file 臂共用同一谓词（yoyo #1024 same predicate 挡每一扇门）。
pub fn path_provenance_command(command: &str) -> Result<(), String> {
    for tok in command.split_whitespace() {
        let tok = tok.trim_matches(|c| c == '\'' || c == '"' || c == '`');
        if !tok.starts_with('/') || tok.starts_with("--") {
            continue;
        }
        // 内联赋值（VAR=/path）与双 token 间无空格等形态不在此朴素覆盖内；
        // 只处理最直接的「绝对路径作为参数」这条被探针坐实的逃逸线。
        if let Err(prov) = path_provenance(tok) {
            // 特殊：path_provenance 的「不可解析」在命令级视为非路由路径，跳过
            if prov.starts_with("路径不可解析") {
                continue;
            }
            return Err(prov);
        }
    }
    Ok(())
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
            // provenance 闸：白名单 bin 合法不够，读侧路径还要过敏感区/允许域校验
            if let Err(prov) = path_provenance_command(command) {
                return format!("⛔ 安全拦截（未执行）：{prov}");
            }
            run_shell(command).await
        }
        "read_file" => {
            let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                return "工具错误：缺少 path 参数".to_string();
            };
            // provenance 闸：read_file 臂与 run_readonly 同一谓词（#1024 同源）
            // 例外：路径不可解析（不存在）→ 不是凭据路径（凭据必存在），
            // 放行到正常读，由 #140 三态诚实报告「路径不存在」而非「安全拦截」。
            if let Err(prov) = path_provenance(path) {
                if !prov.starts_with("路径不可解析") {
                    return format!("⛔ 安全拦截（未执行）：{prov}");
                }
            }
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
    let cmd = command.to_string();
    let result = timeout(Duration::from_secs(CMD_TIMEOUT_SECS), async move {
        tokio::task::spawn_blocking(move || Command::new("bash").arg("-c").arg(&cmd).output()).await
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

    // ---- issue #152：provenance 闸（fail closed by provenance，非 flag/片段黑名单）----

    #[test]
    fn provenance_denies_credentials_file() {
        // 探针坐实：cat ~/.git-credentials 旧实现 GATE=PASS 读 133B 含 ghp_
        let e = path_provenance("/home/ubuntu/.git-credentials");
        assert!(e.is_err(), "凭据文件必须被拒: {e:?}");
        assert!(e.unwrap_err().contains("凭据/私钥/令牌"));
    }

    #[test]
    fn provenance_denies_ssh_dir() {
        let e = path_provenance("/home/ubuntu/.ssh/q-body_deploy");
        assert!(e.is_err(), "SSH 私钥必须被拒");
        assert!(e.unwrap_err().contains("凭据/私钥/令牌"));
    }

    #[test]
    fn provenance_denies_gh_config() {
        // 读侧路径不触碰 ~/.config/gh（含 auth token，deny-by-default）
        assert!(path_provenance("/home/ubuntu/.config/gh/hosts.yml").is_err());
    }

    #[test]
    fn provenance_allows_system_root() {
        // 合法只读：/etc/hostname canonical 落 /etc 允许根内
        let p = path_provenance("/etc/hostname").unwrap();
        assert_eq!(p, std::path::Path::new("/etc/hostname"));
    }

    #[test]
    fn provenance_allows_workspace() {
        // q-body 工作区允许根（含遍历 src/、soul/ 等）
        let p = path_provenance("/home/ubuntu/q-body/src/tools.rs").unwrap();
        assert!(p.starts_with("/home/ubuntu/q-body"));
    }

    #[test]
    fn provenance_unresolvable_reports_denied_not_lie() {
        // 不存在的路径 canonicalize 失败 → path_provenance 报错（不是放行）
        assert!(path_provenance("/nonexistent-xyz-123/file").is_err());
        // 但 read_file 臂对它放行到 #140 三态（路径不存在），不是安全拦截：
        // 不存在的路径读不到任何凭据，不属于泄漏面（见 read_file_missing 语义）
    }

    #[tokio::test]
    async fn read_file_missing_still_reports_path_missing_not_security() {
        // 回归守住 #140 三态诚实：不存在的路径报「路径不存在」，不得被误判为安全拦截
        let out = dispatch("read_file", &json!({"path": "/nonexistent-xyz-123"})).await;
        assert!(out.contains("路径不存在"), "got: {out}");
        assert!(!out.contains("安全拦截"), "读了不存在的路径不提安全: {out}");
    }

    #[tokio::test]
    async fn read_file_denies_credentials_via_provenance() {
        // 探针坐实 PATH_GATE=NONE 的逃逸线：read_file 臂读凭据 133B 含 ghp_
        let out = dispatch("read_file", &json!({"path": "/home/ubuntu/.git-credentials"})).await;
        assert!(out.contains("安全拦截"), "read_file 必须拦凭据: {out}");
        assert!(out.contains("凭据"), "got: {out}");
        assert!(!out.contains("ghp_"), "凭据内容不得泄露进回复: {out}");
    }

    #[tokio::test]
    async fn run_readonly_denies_credential_cat() {
        // 探针坐实：cat ~/.git-credentials 旧实现 GATE=PASS + bash 真拿 133B
        let out = dispatch(
            "run_readonly",
            &json!({"command": "cat /home/ubuntu/.git-credentials"}),
        )
        .await;
        assert!(out.contains("安全拦截"), "run_readonly 必须拦凭据 cat: {out}");
        assert!(out.contains("凭据"), "got: {out}");
        assert!(!out.contains("ghp_"), "凭据不得进回复: {out}");
    }

    #[tokio::test]
    async fn run_readonly_denies_ssh_head() {
        // 探针坐实：head -5 ~/.ssh/q-body_deploy 旧实现 GATE=PASS + 私钥 411B
        let out = dispatch(
            "run_readonly",
            &json!({"command": "head -5 /home/ubuntu/.ssh/q-body_deploy"}),
        )
        .await;
        assert!(out.contains("安全拦截"), "必须拦私钥 head: {out}");
        assert!(out.contains("凭据"), "got: {out}");
        assert!(!out.contains("PRIVATE KEY"), "私钥不得进回复: {out}");
    }

    #[tokio::test]
    async fn run_readonly_allows_system_cat() {
        // 合法只读不受影响：cat /etc/hostname 仍在允许根内
        let out = dispatch(
            "run_readonly",
            &json!({"command": "cat /etc/hostname"}),
        )
        .await;
        assert!(out.contains("安全拦截") == false, "合法只读被误拦: {out}");
        assert!(!out.trim().is_empty(), "got: {out}");
    }
}
