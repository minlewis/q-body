//! Command validator — rm -rf 静态预检
//!
//! 借鉴 yologdev/yoyo-evolve Day 144 的 safety.rs 思路：
//! 对 `rm -rf` 类危险命令做静态预检，检测未解析/空 shell 变量。
//!
//! 为什么需要这个：`rm -rf $VAR/sub` 在 `$VAR` 未定义或为空时
//! 会退化成 `rm -rf /sub`——根目录级别的爆炸半径。
//! 在参数展开前做静态扫描，命中危险模式就拒绝执行，
//! 并提示使用 `${VAR:?}` escape hatch（shell 内置的未定义即失败语法）。

use std::env;
use std::fmt;

/// 安全检查错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyError {
    /// 检测到 rm -rf 配合未定义/空变量展开
    UnresolvedVariable {
        var_name: String,
        command: String,
        suggestion: String,
    },
    /// 检测到 rm -rf 目标是危险根路径
    DangerousTarget { path: String, command: String },
}

impl fmt::Display for SafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SafetyError::UnresolvedVariable {
                var_name,
                command,
                suggestion,
            } => write!(
                f,
                "rm -rf 命令中变量 `${}` 未定义或为空，拒绝执行。\n\
                 命令: {}\n\
                 建议: 使用 `${{{}:?}}` 语法让 shell 在变量未定义时立即失败。\
                 例如: {}",
                var_name, command, var_name, suggestion
            ),
            SafetyError::DangerousTarget { path, command } => write!(
                f,
                "rm -rf 目标 `{}` 是危险路径（根目录或家目录本身），拒绝执行。\n\
                 命令: {}",
                path, command
            ),
        }
    }
}

impl std::error::Error for SafetyError {}

/// 判断命令是否含 rm -rf / rm -fr / rm -r -f 等危险组合
pub fn has_rm_rf(cmd: &str) -> bool {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "rm" {
            let mut j = i + 1;
            let mut has_r = false;
            let mut has_f = false;
            while j < tokens.len() && tokens[j].starts_with('-') {
                let flag = tokens[j];
                if flag == "-rf" || flag == "-fr" {
                    return true;
                }
                if flag == "-r" || flag == "--recursive" {
                    has_r = true;
                }
                if flag == "-f" || flag == "--force" {
                    has_f = true;
                }
                j += 1;
            }
            if has_r && has_f {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// 从命令字符串中提取所有 shell 变量名（$VAR 和 ${VAR} 形式）
pub fn extract_variables(cmd: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let bytes = cmd.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'{' {
                // ${VAR} 或 ${VAR:?} 等形式
                if let Some(end) = cmd[i + 2..].find('}') {
                    let inner = &cmd[i + 2..i + 2 + end];
                    // 取 :? 或 :- 等修饰符之前的部分作为变量名
                    let name = inner
                        .split(|c: char| c == ':' || c == '-' || c == '+' || c == '?')
                        .next()
                        .unwrap_or(inner)
                        .trim();
                    if !name.is_empty() && is_valid_var_name(name) {
                        vars.push(name.to_string());
                    }
                    i += 2 + end + 1;
                    continue;
                }
            } else if bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_' {
                // $VAR 形式
                let start = i + 1;
                let mut end = start;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                vars.push(cmd[start..end].to_string());
                i = end;
                continue;
            }
        }
        i += 1;
    }
    vars
}

fn is_valid_var_name(s: &str) -> bool {
    !s.is_empty()
        && (s.chars().next().unwrap().is_ascii_alphabetic() || s.starts_with('_'))
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 判断变量是否已定义且非空
fn is_var_set(name: &str) -> bool {
    env::var(name).map(|v| !v.is_empty()).unwrap_or(false)
}

/// 间接展开归一化：把「赋值前缀 + 变量间接引用」与「eval 引号拼接」
/// 归一化为等价的字面 token 流，供词法扫描判定。
///
/// 借鉴 yologdev/yoyo-evolve Day 225 Task 2 —— 攻击者视角逆向探针
/// 证明 `X=rm; $X -rf ~` / `BASH_ARGV0=rm; $BASH_ARGV0 -rf ~` /
/// `eval "r""m -rf ~"` 三类间接执行能让 rm 从词法上隐身。
/// 归一化两步：
///   1. 去引号字符：`"r""m` → `rm`（引号拼接语义的保守近似）
///   2. 段内赋值前缀收集 + `$VAR`/`${VAR}` 回代：`X=rm; $X ...` → `rm ...`
/// 注意：回代只注入本命令显式赋过的值，方向偏保守（宁可误报不放行）；
/// bash 的前缀赋值瞬态语义（`FOO=bar baz; cmd $FOO` 中 FOO 未定义）
/// 在此被过近似为持续可见——对安全扫描是安全方向的偏差。
pub fn normalize_indirect_expansion(cmd: &str) -> String {
    // 1) 去引号字符（拼接语义近似：相邻引号段拼回一个词）
    let s: String = cmd.chars().filter(|c| *c != '"' && *c != '\'').collect();

    // 2) 按段收集赋值前缀，剩余部分组成待扫描命令
    let mut assignments: Vec<(String, String)> = Vec::new();
    let mut rest_parts: Vec<&str> = Vec::new();
    for seg in s.split(';') {
        let t = seg.trim();
        if let Some(eq) = t.find('=') {
            let name = t[..eq].trim();
            if is_valid_var_name(name) {
                assignments.push((name.to_string(), t[eq + 1..].trim().to_string()));
                continue;
            }
        }
        rest_parts.push(seg);
    }
    let rest = rest_parts.join(";");
    if assignments.is_empty() {
        return rest;
    }
    // 3) 回代：$VAR / ${VAR} → 已记录赋值（长形式优先，防 ${X}y 被短形式误吃）
    let mut out = rest;
    for (name, value) in &assignments {
        out = out.replace(&format!("${{{}}}", name), value.as_str());
        out = out.replace(&format!("${}", name), value.as_str());
    }
    out
}

/// 生成使用 `${VAR:?}` 的安全替换建议
fn safe_suggestion(cmd: &str, var: &str) -> String {
    cmd.replace(&format!("${}", var), &format!("${{{}:?}}", var))
        .replace(&format!("${{{}}}", var), &format!("${{{}:?}}", var))
}

/// 检查 rm -rf 的目标路径是否是危险根级路径
fn check_dangerous_target(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut seen_rm = false;
    for tok in tokens {
        if tok == "rm" {
            seen_rm = true;
            continue;
        }
        if !seen_rm || tok.starts_with('-') {
            continue;
        }
        // 路径 token
        let cleaned = tok.trim_end_matches('/');
        if cleaned.is_empty() || cleaned == "/" || cleaned == "~" {
            return Some(tok.to_string());
        }
    }
    None
}

/// 对命令做静态预检。
///
/// 若不是 rm -rf 命令 → 返回 Ok(())
/// 若是 rm -rf 且含未定义/空变量 → Err(UnresolvedVariable)
/// 若是 rm -rf 且目标是根路径 → Err(DangerousTarget)
pub fn scan_rm_command(cmd: &str) -> Result<(), SafetyError> {
    // 归一化：间接执行（赋值前缀 + $VAR 回代、eval 引号拼接）先展开成
    // 等价字面 token 流再判定——攻击者视角对抗测试（Day 225 Task 2）证明
    // 不归一化时 `X=rm; $X -rf ~` 类命令能让 rm 从词法上隐身。
    let normalized = normalize_indirect_expansion(cmd);
    if !has_rm_rf(&normalized) {
        return Ok(());
    }
    if let Some(path) = check_dangerous_target(&normalized) {
        return Err(SafetyError::DangerousTarget {
            path,
            command: cmd.to_string(),
        });
    }
    for var in extract_variables(&normalized) {
        if !is_var_set(&var) {
            return Err(SafetyError::UnresolvedVariable {
                suggestion: safe_suggestion(cmd, &var),
                var_name: var,
                command: cmd.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_rm_rf_simple() {
        assert!(has_rm_rf("rm -rf /tmp/foo"));
        assert!(has_rm_rf("rm -fr /tmp/foo"));
        assert!(has_rm_rf("rm -r -f /tmp/foo"));
        assert!(has_rm_rf("rm -f -r /tmp/foo"));
        assert!(!has_rm_rf("rm -f /tmp/foo"));
        assert!(!has_rm_rf("rm -r /tmp/foo"));
        assert!(!has_rm_rf("rm /tmp/foo"));
        assert!(!has_rm_rf("ls -la"));
    }

    #[test]
    fn test_extract_variables_dollar() {
        let vars = extract_variables("rm -rf $HOME/tmp");
        assert_eq!(vars, vec!["HOME"]);
    }

    #[test]
    fn test_extract_variables_braced() {
        let vars = extract_variables("rm -rf ${HOME}/tmp");
        assert_eq!(vars, vec!["HOME"]);
    }

    #[test]
    fn test_extract_variables_with_guard() {
        let vars = extract_variables("rm -rf ${HOME:?}/tmp");
        assert_eq!(vars, vec!["HOME"]);
    }

    #[test]
    fn test_extract_variables_multiple() {
        let vars = extract_variables("rm -rf $A/$B/${C}");
        assert_eq!(vars, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_scan_no_rm_rf_passes() {
        assert!(scan_rm_command("ls -la /tmp").is_ok());
        assert!(scan_rm_command("rm -f file.txt").is_ok());
    }

    #[test]
    fn test_scan_dangerous_target_root() {
        let r = scan_rm_command("rm -rf /");
        assert!(matches!(r, Err(SafetyError::DangerousTarget { .. })));
    }

    #[test]
    fn test_scan_unset_var_rejected() {
        // 用一个肯定不会存在的环境变量名
        let r = scan_rm_command("rm -rf $Q_BODY_NONEXISTENT_VAR_12345/tmp");
        match r {
            Err(SafetyError::UnresolvedVariable {
                var_name,
                suggestion,
                ..
            }) => {
                assert_eq!(var_name, "Q_BODY_NONEXISTENT_VAR_12345");
                assert!(suggestion.contains("${Q_BODY_NONEXISTENT_VAR_12345:?}"));
            }
            _ => panic!("expected UnresolvedVariable"),
        }
    }

    #[test]
    fn test_scan_set_var_passes() {
        // HOME 在所有 Linux 环境都已设置且非空
        let r = scan_rm_command("rm -rf $HOME/tmp/foo");
        assert!(r.is_ok(), "expected ok, got {:?}", r);
    }

    #[test]
    fn test_error_display_suggestion() {
        let r = scan_rm_command("rm -rf $Q_BODY_NONEXISTENT_XYZ/tmp");
        let err = r.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("Q_BODY_NONEXISTENT_XYZ"));
        assert!(msg.contains(":?"));
    }

    // ── 对抗测试：间接执行绕过（RED-first，攻击者视角 SOUL §25） ──
    // 借鉴 yologdev/yoyo-evolve Day 225 Task 2 — 逆向探针 bash 护栏的
    // 变量间接绕过 → q-body 命令白名单的三类间接执行对抗测试：
    //   1. 变量间接执行：`X=rm; $X -rf ~`
    //   2. BASH_ARGV0 间接：`BASH_ARGV0=rm; $BASH_ARGV0 -rf ~`
    //   3. eval + 引号拼接：`eval "r""m -rf ~"`
    // 三者在真实 bash 中均等价 `rm -rf ~`（家目录级爆炸半径），
    // 但词法上无字面 "rm" token。判决：归一化后必须与等价字面命令同判。

    #[test]
    fn test_scan_indirect_dangerous_target_rejected() {
        for cmd in [
            "X=rm; $X -rf ~",
            "BASH_ARGV0=rm; $BASH_ARGV0 -rf ~",
            "eval \"r\"\"m -rf ~\"",
            "eval 'r'm' -rf ~'",
        ] {
            let r = scan_rm_command(cmd);
            assert!(
                matches!(r, Err(SafetyError::DangerousTarget { .. })),
                "indirect `{}` must be judged like literal `rm -rf ~`, got {:?}",
                cmd,
                r
            );
        }
    }

    #[test]
    fn test_scan_indirect_unresolved_var_rejected() {
        // 间接执行的 rm 携带未定义变量：暴露为 rm -rf $UNSET/x → 拒绝
        let r = scan_rm_command("X=rm; $X -rf $Q_BODY_NONEXISTENT_IND/x");
        assert!(matches!(r, Err(SafetyError::UnresolvedVariable { .. })));
    }

    #[test]
    fn test_scan_indirect_benign_still_ok() {
        // 良性间接执行不误伤：判决与等价字面命令一致（rm -rf 普通路径 = Ok）
        assert!(scan_rm_command("X=rm; $X -rf /tmp/ok-target").is_ok());
        assert!(scan_rm_command("X=echo; $X hello").is_ok());
        assert!(scan_rm_command("eval \"e\"\"cho hi\"").is_ok());
    }

    #[test]
    fn test_adversarial_indirection_is_real_in_bash() {
        // 攻击者视角真实性探针：证明 `$X` 间接执行在真实 bash 中
        // 以 rm 本体语义运行（用 --version 代理删除，验证展开目标）
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg("X=rm; $X --version 2>/dev/null | head -1")
            .output()
            .expect("bash unavailable");
        assert!(
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("rm"),
            "indirect $X must execute rm itself, got {:?}",
            out.stdout
        );
    }

    // ── 边界用例：trailing whitespace / 空参 / 多余空格 ──
    // 借鉴 yoyo-evolve DREAM 实验发现的 route_command 边界 bug，
    // 直接免疫同类问题。

    #[test]
    fn test_edge_trailing_whitespace() {
        // 末尾带空格 / 换行 / tab 不影响判定
        assert!(has_rm_rf("rm -rf /tmp/foo "));
        assert!(has_rm_rf("rm -rf /tmp/foo\n"));
        assert!(has_rm_rf("rm -rf /tmp/foo\t"));
        assert!(has_rm_rf("  rm -rf /tmp/foo  "));

        // 末尾空白不干扰 scan_rm_command
        assert!(scan_rm_command("rm -rf /tmp/foo ").is_ok());
        assert!(scan_rm_command("rm -rf / ").is_err());
        assert!(scan_rm_command("rm -rf $Q_BODY_NONEXISTENT_EDGE_WS /tmp ").is_err());
    }

    #[test]
    fn test_edge_empty_args() {
        // 空字符串参数不 panic、不误判
        assert!(!has_rm_rf(""));
        assert!(!has_rm_rf("   "));
        assert!(scan_rm_command("").is_ok());
        assert!(scan_rm_command("   ").is_ok());

        // 空参数嵌在中间
        assert!(has_rm_rf("rm  -rf  /tmp/foo"));
        assert!(scan_rm_command("rm  -rf  /tmp/foo").is_ok());
    }

    #[test]
    fn test_edge_extra_spaces() {
        // 多余空格不破坏变量提取
        let vars = extract_variables("rm  -rf  $HOME /tmp");
        assert_eq!(vars, vec!["HOME"]);

        let vars = extract_variables("rm   -rf   ${HOME}  /tmp");
        assert_eq!(vars, vec!["HOME"]);

        // 多余空格不破坏危险目标识别
        assert!(scan_rm_command("rm  -rf  / ").is_err());
        assert!(scan_rm_command("rm   -rf   /tmp/foo").is_ok());
    }
}
