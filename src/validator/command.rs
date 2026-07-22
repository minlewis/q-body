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
    if !has_rm_rf(cmd) {
        return Ok(());
    }
    if let Some(path) = check_dangerous_target(cmd) {
        return Err(SafetyError::DangerousTarget {
            path,
            command: cmd.to_string(),
        });
    }
    for var in extract_variables(cmd) {
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
            Err(SafetyError::UnresolvedVariable { var_name, suggestion, .. }) => {
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
}
