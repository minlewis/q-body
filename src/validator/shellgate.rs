//! shellgate — 只读命令的 shell 词法级门禁（issue #152）
//!
//! TAO 三问：
//! 1. 接线了吗？→ 是，`tools::validate_readonly` 每夜真实调用（agent 每次 run_readonly 必经）。
//! 2. 能进 SOUL.md 吗？→ 不能，词法切分与白名单判定是机械约束，LLM 无法自行可靠执行。
//! 3. 损掉了什么？→ 砍掉「字面 contains 匹配 + 多段命令/管道/命令替换」这一整类能力，
//!    换成「壳词法切分 + 逐段白名单 + 拒绝不可静态判定语法」更强也更严的判定。
//!
//! 为什么需要（2026-10-14 本机实跑探针实录，`~/.hermes/cron/output/probe_20261014/`）：
//! 旧实现 `command.contains("systemctl restart")` 只挡得住单一空格写法，
//! `systemctl  restart` / `systemctl\tstop` / `systemctl 'restart'` / `systemctl --no-block restart`
//! 等 7 种 bash 词法等价写法全部放行、bash 真拿到 restart/stop argv——
//! 只读巡检的安全声明不成立。
//!
//! 设计取向（借鉴 yoyo Day 225/228「先实测护栏是否真被绕过，再补归一化」）：
//! **只读工具的解析器应当保守到宁可误拒**——凡静态无法判定的语法一律拒绝执行，
//! 而不是猜它的语义。

use std::fmt;

/// 一条简单命令（被 `;` / `&&` / `||` / `|` / `&` / 换行 分隔的段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 已完成引号解引用的词序列（`'foo'\t'bar'` → `["foo", "bar"]`）。
    pub words: Vec<String>,
}

/// 只读门禁拒绝原因。每个变体都要能回答「哪个词/哪个字符触发」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellGateError {
    /// 空命令。
    Empty,
    /// 检测到多段/管道/后台命令：一次只执行单条只读命令。
    MultiSegment { count: usize },
    /// shell 元字符：静态无法判定其展开结果。
    UnsupportedSyntax { token: String },
    /// 引号未闭合。
    UnterminatedQuote { quote: char },
    /// 转义符后无字符。
    UnterminatedEscape,
    /// 首词不在只读白名单内。
    BinNotAllowed { bin: String, allowed: Vec<String> },
    /// 该 bin 有只读子命令闭集，但未找到子命令。
    MissingSubcommand { bin: String, allowed: Vec<String> },
    /// 子命令不在只读闭集内（默认拒绝，写子命令因此无处藏身）。
    SubcommandNotAllowed {
        bin: String,
        sub: String,
        allowed: Vec<String>,
    },
}

impl fmt::Display for ShellGateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "空命令"),
            Self::MultiSegment { count } => write!(
                f,
                "检测到 {count} 段命令（`;`/`&&`/`||`/`|`/`&`/换行 分隔）。\
                 本工具一次只执行单条只读命令，不执行多段/管道/后台命令。"
            ),
            Self::UnsupportedSyntax { token } => write!(
                f,
                "检测到 shell 元字符 `{token}`：其展开结果无法静态判定，\
                 只读工具一律拒绝（变量展开、命令替换、重定向、子 shell 均不在允许范围）。"
            ),
            Self::UnterminatedQuote { quote } => write!(f, "引号 `{quote}` 未闭合"),
            Self::UnterminatedEscape => write!(f, "转义符 `\\` 后缺字符"),
            Self::BinNotAllowed { bin, allowed } => {
                write!(f, "`{bin}` 不在只读白名单内。允许: {}", allowed.join(", "))
            }
            Self::MissingSubcommand { bin, allowed } => write!(
                f,
                "`{bin}` 只放行只读子命令，但命令未给出子命令。允许: {}",
                allowed.join(", ")
            ),
            Self::SubcommandNotAllowed { bin, sub, allowed } => write!(
                f,
                "`{bin} {sub}` 不是只读子命令（默认拒绝）。允许: {}",
                allowed.join(", ")
            ),
        }
    }
}

impl std::error::Error for ShellGateError {}

/// 有「只读子命令闭集」的 bin。**未列出的 bin 不豁免**——它们的 bin 级白名单
/// 已在 ALLOWED_BINS 把关。
const SUBCOMMAND_ALLOWLIST: &[(&str, &[&str])] = &[(
    "systemctl",
    &[
        "status",
        "is-active",
        "is-enabled",
        "is-failed",
        "is-system-running",
        "show",
        "cat",
        "list-units",
        "list-unit-files",
        "list-timers",
        "list-sockets",
        "list-dependencies",
        "get-default",
    ],
)];

/// 取值的短/长选项：其后一个词是它的值而非子命令。
/// 只为让 `systemctl -H host status x` 这类写法不误判，不影响安全判定——
/// 写子命令无论出现在哪个位置都命中不了只读闭集。
const VALUE_TAKING_OPTIONS: &[&str] = &[
    "-H",
    "--host",
    "-M",
    "--machine",
    "-p",
    "--property",
    "-t",
    "--type",
    "--state",
    "--job-mode",
    "--signal",
    "-n",
    "--lines",
    "--root",
    "--kill-who",
    "--what",
];

/// 把命令切成段（引号解引用、空白归并、拒绝不可静态判定的语法）。
///
/// 返回多段 >1 时调用方按 `MultiSegment` 拒绝——本函数只负责如实切分。
pub fn split_segments(cmd: &str) -> Result<Vec<Segment>, ShellGateError> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut has_cur = false;
    let mut chars = cmd.chars().peekable();

    macro_rules! push_word {
        () => {
            if has_cur {
                words.push(std::mem::take(&mut cur));
                has_cur = false;
            }
        };
    }
    macro_rules! push_seg {
        () => {
            push_word!();
            if !words.is_empty() {
                segs.push(Segment {
                    words: std::mem::take(&mut words),
                });
            }
        };
    }

    while let Some(c) = chars.next() {
        match c {
            // 单引号：字面量，无转义
            '\'' => {
                has_cur = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err(ShellGateError::UnterminatedQuote { quote: '\'' }),
                    }
                }
            }
            // 双引号：`\"` / `\\` / `\$` 转义，其余字面量
            '"' => {
                has_cur = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch) => cur.push(ch),
                            None => return Err(ShellGateError::UnterminatedQuote { quote: '"' }),
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err(ShellGateError::UnterminatedQuote { quote: '"' }),
                    }
                }
            }
            '\\' => {
                has_cur = true;
                match chars.next() {
                    Some(ch) => cur.push(ch),
                    None => return Err(ShellGateError::UnterminatedEscape),
                }
            }
            // 不可静态判定的语法：变量/命令替换、重定向、子 shell、brace 展开
            '$' | '`' | '>' | '<' | '(' | ')' | '{' | '}' => {
                return Err(ShellGateError::UnsupportedSyntax {
                    token: c.to_string(),
                });
            }
            // 段分隔符：`;` `|` `||` `&` `&&` 换行
            ';' | '|' | '&' | '\n' => {
                // `||` / `&&` 两字符算子不影响语义：都是段分隔
                if (c == '|' && chars.peek() == Some(&'|'))
                    || (c == '&' && chars.peek() == Some(&'&'))
                {
                    chars.next();
                }
                push_seg!();
            }
            ' ' | '\t' | '\r' => push_word!(),
            _ => {
                has_cur = true;
                cur.push(c);
            }
        }
    }
    push_seg!();
    Ok(segs)
}

/// 取首个词并剥掉目录（`/usr/bin/ls` → `ls`）。
fn bin_of(seg: &Segment) -> String {
    seg.words
        .first()
        .map(|w| w.rsplit('/').next().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// 找首个「非选项」词，跳过取值选项的值。
fn subcommand_of(seg: &Segment) -> Option<String> {
    let mut i = 1;
    while i < seg.words.len() {
        let w = &seg.words[i];
        if w.starts_with('-') {
            let eats_value = !w.contains('=') && VALUE_TAKING_OPTIONS.contains(&w.as_str());
            i += if eats_value { 2 } else { 1 };
            continue;
        }
        return Some(w.clone());
    }
    None
}

/// 只读门禁：词法切分 → 单段校验 → bin 白名单 → （有闭集的 bin）子命令白名单。
///
/// 判定顺序刻意「先词法后白名单」：`systemctl  restart` 这类绕过在词法阶段
/// 就被归一成与 `systemctl restart` 完全相同的词流，写子命令再也无法靠空白/引号藏身。
pub fn check_readonly(cmd: &str, allowed_bins: &[&str]) -> Result<Segment, ShellGateError> {
    let segs = split_segments(cmd)?;
    if segs.is_empty() {
        return Err(ShellGateError::Empty);
    }
    if segs.len() > 1 {
        return Err(ShellGateError::MultiSegment { count: segs.len() });
    }
    let seg = segs.into_iter().next().expect("len==1");
    let bin = bin_of(&seg);
    if !allowed_bins.contains(&bin.as_str()) {
        return Err(ShellGateError::BinNotAllowed {
            bin,
            allowed: allowed_bins.iter().map(|s| s.to_string()).collect(),
        });
    }
    if let Some((_, allowed)) = SUBCOMMAND_ALLOWLIST.iter().find(|(b, _)| *b == bin) {
        let allowed_vec: Vec<String> = allowed.iter().map(|s| s.to_string()).collect();
        match subcommand_of(&seg) {
            None => {
                return Err(ShellGateError::MissingSubcommand {
                    bin,
                    allowed: allowed_vec,
                });
            }
            Some(sub) if !allowed.contains(&sub.as_str()) => {
                return Err(ShellGateError::SubcommandNotAllowed {
                    bin,
                    sub,
                    allowed: allowed_vec,
                });
            }
            Some(_) => {}
        }
    }
    Ok(seg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINS: &[&str] = &[
        "systemctl",
        "journalctl",
        "ps",
        "df",
        "cat",
        "grep",
        "ls",
        "ss",
        "uptime",
    ];

    fn gate(cmd: &str) -> Result<Segment, ShellGateError> {
        check_readonly(cmd, BINS)
    }

    // ── RED 复刻：10-14 探针实录的绕过写法表，全部必须被拒 ──
    // 每条都来自实录（`~/.hermes/cron/output/probe_20261014/README.md`），
    // 旧 `contains` 实现对其中 7 种 GATE=PASS、bash 真拿到 restart/stop argv。

    #[test]
    fn probe_bypass_table_is_all_rejected() {
        for cmd in [
            "systemctl restart nginx",                     // 基线：单空格
            "systemctl  restart nginx",                    // 双空格
            "systemctl\tstop nginx",                       // TAB
            "systemctl 'restart' nginx",                   // 子命令单引号
            "systemctl \"stop\" nginx",                    // 子命令双引号
            "systemctl ''restart'' nginx",                 // 空引号拼接
            "systemctl re'st'art nginx",                   // 引号内拼接
            "systemctl --no-block restart nginx",          // 选项插在子命令前
            "systemctl --quiet stop nginx",                // 选项变体 stop
            "/usr/bin/systemctl  restart nginx",           // 绝对路径 + 双空格
            "cat /etc/hostname; systemctl  stop nginx",    // 分号串联
            "uptime && systemctl restart nginx",           // &&
            "cat /etc/hostname | systemctl restart nginx", // 管道
            "systemctl restart nginx &",                   // 后台
            "uptime\nsystemctl stop nginx",                // 换行
        ] {
            assert!(gate(cmd).is_err(), "绕过写法未被拒绝: {cmd}");
        }
    }

    // ── 不可静态判定语法：一律拒绝（保守到宁可误拒）──

    #[test]
    fn rejects_unresolvable_syntax() {
        for cmd in [
            "cat $FILE",
            "cat ${FILE}",
            "cat `hostname`",
            "cat $(hostname)",
            "ls > /tmp/x",
            "ls >> /tmp/x",
            "cat < /etc/hostname",
            "cat (ls)",
            "X=rm; $X -rf /",
            "BASH_ARGV0=rm; $BASH_ARGV0 -rf /",
        ] {
            assert!(gate(cmd).is_err(), "应当拒绝: {cmd}");
        }
    }

    #[test]
    fn rejects_write_subcommands_not_in_closed_set() {
        // 写子命令全表（含会诱导写模式闸的变体）→ 闭集外一律拒绝
        const WRITE_SUBS: &[&str] = &[
            "start",
            "stop",
            "restart",
            "try-restart",
            "reload",
            "reload-or-restart",
            "enable",
            "disable",
            "mask",
            "unmask",
            "isolate",
            "kill",
            "daemon-reload",
            "daemon-reexec",
            "set-property",
            "edit",
            "link",
            "revert",
            "set-default",
            "freeze",
            "thaw",
            "suspend",
            "hibernate",
            "switch-root",
            "halt",
            "poweroff",
            "reboot",
            "rescue",
            "emergency",
        ];
        for sub in WRITE_SUBS {
            let cmd = format!("systemctl {sub} nginx");
            assert!(gate(&cmd).is_err(), "写子命令放行: {cmd}");
        }
    }

    #[test]
    fn missing_subcommand_is_rejected() {
        assert!(matches!(
            gate("systemctl"),
            Err(ShellGateError::MissingSubcommand { .. })
        ));
    }

    #[test]
    fn unquote_and_path_are_normalized() {
        let seg = gate("/usr/bin/systemctl --no-pager status nginx").unwrap();
        assert_eq!(bin_of(&seg), "systemctl");
        assert_eq!(subcommand_of(&seg).as_deref(), Some("status"));
    }

    #[test]
    fn value_taking_option_does_not_shadow_subcommand() {
        // `-H host` 的 host 是选项值，不是子命令 → 仍能正确找到 status
        let seg = gate("systemctl -H myhost status nginx").unwrap();
        assert_eq!(subcommand_of(&seg).as_deref(), Some("status"));
    }

    // ── 良性只读命令不得误伤 ──

    #[test]
    fn benign_readonly_commands_pass() {
        for cmd in [
            "df -h /",
            "uptime",
            "ls -la /var/log",
            "cat /etc/hostname",
            "ps aux",
            "ss -tlnp",
            "systemctl status nginx",
            "systemctl --no-pager is-active q-body",
            "systemctl list-units --type=service",
            "systemctl --type=service list-units",
            "journalctl -u q-body --since '1 hour ago'",
            "journalctl -u q-body --since \"1 hour ago\" -n 50",
            "grep -c zzzz /etc/hostname",
            "systemctl -H myhost status nginx",
        ] {
            assert!(
                gate(cmd).is_ok(),
                "良性只读命令被误伤: {cmd} → {:?}",
                gate(cmd)
            );
        }
    }

    #[test]
    fn quoted_path_with_space_is_one_word() {
        let seg = gate("cat '/var/log/my app.log'").unwrap();
        assert_eq!(seg.words, vec!["cat", "/var/log/my app.log"]);
    }

    #[test]
    fn non_whitelisted_bin_is_rejected() {
        assert!(matches!(
            gate("python3 -c 'print(1)'"),
            Err(ShellGateError::BinNotAllowed { .. })
        ));
        assert!(matches!(
            gate("rm -rf /tmp/x"),
            Err(ShellGateError::BinNotAllowed { .. })
        ));
    }

    #[test]
    fn empty_and_unterminated_are_rejected() {
        assert!(matches!(gate(""), Err(ShellGateError::Empty)));
        assert!(matches!(gate("   "), Err(ShellGateError::Empty)));
        assert!(matches!(
            gate("cat 'unclosed"),
            Err(ShellGateError::UnterminatedQuote { .. })
        ));
        assert!(matches!(
            gate("cat \\"),
            Err(ShellGateError::UnterminatedEscape)
        ));
    }

    #[test]
    fn error_messages_name_the_offending_token() {
        let msg = gate("cat $FILE").unwrap_err().to_string();
        assert!(msg.contains('$'), "got: {msg}");
        let msg = gate("systemctl restart nginx").unwrap_err().to_string();
        assert!(msg.contains("restart"), "got: {msg}");
        let msg = gate("cat /etc/hostname; ls").unwrap_err().to_string();
        assert!(msg.contains("2 段"), "got: {msg}");
    }
}
