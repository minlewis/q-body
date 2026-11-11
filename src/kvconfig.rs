//! kvconfig — 键值配置行解析的单一解析函数。
//!
//! 借鉴：yologdev/yoyo-evolve #1177 — quote-aware strip + tomllib 差分。
//! yoyo 曾因「朴素剥注释/剥引号」把引号内含 `#` 或 `=` 的值截断，造成配置语义漂移。
//! 本模块提供唯一的 `parse_kv_line` 解析入口：行尾注释剥离与值引号剥离均为
//! quote-aware（感知引号状态），并用「朴素剥法 vs quote-aware 剥法」的差分测试
//! 锚定语义，防未来手写解析器重蹈漂移覆辙。
//!
//! ## 语义约束（差分锚定）
//! 1. 行尾 `#` 注释：仅当 `#` 不在引号对内部时才算注释开始。
//! 2. 值引号：`"value"` / `'value'` 去外层成对引号；值内含的引号字符保留原义。
//! 3. 键值分隔 `=`：取第一个未在引号内的 `=`。
//! 4. 空行 / 纯注释行 / 无 `=` 的行 → None（不是错误，交回调方按缺省处理）。

/// 引号状态：当前是否在一对引号内部（' 或 "）。
#[derive(Clone, Copy, PartialEq, Debug)]
enum QuoteState {
    None,
    Single,
    Double,
}

impl QuoteState {
    fn toggle(self, c: char) -> QuoteState {
        match (self, c) {
            (QuoteState::None, '\'') => QuoteState::Single,
            (QuoteState::None, '"') => QuoteState::Double,
            (QuoteState::Single, '\'') => QuoteState::None,
            (QuoteState::Double, '"') => QuoteState::None,
            _ => self,
        }
    }
}

/// 剥离行尾注释（quote-aware）：`#` 在引号对内部时不算注释开始。
/// 返回 None 表示整个 `#` 起都是注释（或行本无内容）。
pub fn strip_comment(line: &str) -> Option<&str> {
    let mut state = QuoteState::None;
    let mut prev_escaped = false;
    for (i, c) in line.char_indices() {
        if prev_escaped {
            prev_escaped = false;
            continue; // 转义后的字符不参与引号状态切换（`\#` 不是注释）
        }
        if c == '\\' {
            prev_escaped = true;
            continue;
        }
        if c == '#' && state == QuoteState::None {
            let before = &line[..i];
            if before.trim().is_empty() {
                return None; // 整行是注释
            }
            return Some(before);
        }
        state = state.toggle(c);
    }
    if line.trim().is_empty() {
        None
    } else {
        Some(line)
    }
}

/// 剥离值外层成对引号（quote-aware）：仅去成对的首尾引号，内含引号保留。
pub fn strip_quotes(value: &str) -> &str {
    let v = value.trim();
    if v.len() >= 2 {
        let chars: Vec<char> = v.chars().collect();
        let (first, last) = (chars[0], chars[v.len() - 1]);
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return v[1..v.len() - 1].trim();
        }
    }
    v
}

/// 解析一条 `KEY = value` 配置行。
/// 返回 (key, value)，key/value 均已 trim；无 `=` 或空/注释行返回 None。
pub fn parse_kv_line(line: &str) -> Option<(&str, &str)> {
    let content = strip_comment(line)?;
    let mut state = QuoteState::None;
    let mut prev_escaped = false;
    for (i, c) in content.char_indices() {
        if prev_escaped {
            prev_escaped = false;
            continue; // 转义后的字符不参与引号状态（`\=` 不是分隔符）
        }
        if c == '\\' {
            prev_escaped = true;
            continue;
        }
        if c == '=' && state == QuoteState::None {
            let (k, v) = content.split_at(i);
            let key = k.trim();
            let value = strip_quotes(&content[i + 1..]);
            if key.is_empty() {
                return None;
            }
            return Some((key, value));
        }
        state = state.toggle(c);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 差分锚定：朴素剥注释（见到 `#` 即切）把引号内 `#` 截断，
    /// quote-aware 剥法保留之——这条正是 yoyo #1177 抓到的漂移形状。
    #[test]
    fn differential_naive_vs_quote_aware_comment() {
        let line = "GREETING = \"hello # world\"";
        // 朴素剥法：char_indices 见到第一个 '#' 即切 → "GREETING = \"hello "
        let naive = line.split('#').next().unwrap_or("");
        // quote-aware 剥法：# 在引号内，保留
        let stripped = strip_comment(line).unwrap_or("");
        assert!(naive != stripped, "两者的差异正是 #1177 要锚定的漂移");
        assert!(stripped.contains('#'));
        assert!(!naive.contains('#'));
    }

    /// 差分锚定：行尾真实注释应被剥掉（# 在引号外）。
    #[test]
    fn strip_trailing_comment_outside_quotes() {
        let line = "KEY = value  # real comment";
        let (k, v) = parse_kv_line(line).unwrap();
        assert_eq!(k, "KEY");
        assert_eq!(v, "value");
    }

    /// 值内含引号 + `=`：值保留原义，`=` 不被误当分隔符。
    #[test]
    fn quoted_value_keeps_inner_symbols() {
        let (k, v) = parse_kv_line("MSG = 'a=b # c'").unwrap();
        assert_eq!(k, "MSG");
        assert_eq!(v, "a=b # c");
    }

    /// 引号对不完全闭合时，按无引号语义处理（不 panic）。
    #[test]
    fn unbalanced_quote_is_tolerated() {
        let (_, v) = parse_kv_line("GREETING = \"hello # world").unwrap();
        // 引号未闭合 → 状态停在引号内 → `#` 不切注释，值保留原样（含尾引号）
        assert!(v.contains('#'));
    }

    /// 空行 / 纯注释 / 无 `=` → None。
    #[test]
    fn empty_or_comment_or_no_equal_is_none() {
        assert!(strip_comment("").is_none());
        assert!(strip_comment("   ").is_none());
        assert!(strip_comment("# just a comment").is_none());
        assert!(parse_kv_line("").is_none());
        assert!(parse_kv_line("  # comment only").is_none());
        assert!(parse_kv_line("NO_EQUAL_SIGN").is_none());
        assert!(parse_kv_line("= missing key").is_none());
    }

    /// 值有外层引号但不含引号内符号，也被正确剥离。
    #[test]
    fn simple_quoted_value_stripped() {
        let (k, v) = parse_kv_line("NAME = \"q-body\"").unwrap();
        assert_eq!(k, "NAME");
        assert_eq!(v, "q-body");
    }
}
