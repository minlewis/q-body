//! Per-line clamp — 内容组装进 LLM prompt 前的单行截断保护。
//!
//! 借鉴：yologdev/yoyo-evolve #877 clamp per LINE。
//! 问题：skill/journal 内容里一条超长 minified 行（如压缩 JSON）进入
//! prompt 会吃满 context 预算。按总量 clamp 会破坏行结构；按行 clamp
//! 只截断超长行，其余行原样保留。
//!
//! main 上尚无 `src/skills.rs`（在 feat/issue-81-skills-hot-reload 分支），
//! 按 08-05 learnings.rs 先例先落 standalone 纯函数模块，
//! skills.rs 合入 main 后在组装点一行接线：
//! `let content = clamp::clamp_content(&content, clamp::DEFAULT_MAX_LINE_BYTES);`

/// 默认单行字节上限：8 KB
pub const DEFAULT_MAX_LINE_BYTES: usize = 8 * 1024;

/// 截断标记（附加在截断后的行尾）
pub const TRUNCATED_MARK: &str = "[truncated]";

/// 对单行做 clamp：超过 `max_bytes` 时按 UTF-8 char boundary 截断并附标记。
///
/// 返回的行总字节数（含标记）保证 `<= max_bytes`。
/// 若 `max_bytes` 连标记都容不下（<= 标记长度），降级为纯截断不附标记。
/// 恰好等于 `max_bytes` 的行原样返回，不附标记。
pub fn clamp_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }

    // 按 char boundary 收缩，避免切在多字节字符中间产生非法 UTF-8
    let boundary_safe_truncate = |mut end: usize| -> &str {
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        &line[..end]
    };

    // 预算容不下标记：降级为纯截断
    if max_bytes <= TRUNCATED_MARK.len() {
        return boundary_safe_truncate(max_bytes).to_string();
    }

    let mut out = String::with_capacity(max_bytes);
    out.push_str(boundary_safe_truncate(max_bytes - TRUNCATED_MARK.len()));
    out.push_str(TRUNCATED_MARK);
    out
}

/// 对多行内容逐行应用 `clamp_line`，保留行数与行序。
pub fn clamp_content(content: &str, max_bytes: usize) -> String {
    content
        .lines()
        .map(|line| clamp_line(line, max_bytes))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_line_untouched() {
        assert_eq!(clamp_line("hello", 100), "hello");
    }

    #[test]
    fn test_exact_limit_untouched() {
        // 恰好等于阈值的行原样返回，不附标记
        let line = "a".repeat(64);
        assert_eq!(clamp_line(&line, 64), line);
    }

    #[test]
    fn test_ascii_truncation_appends_mark() {
        let line = "a".repeat(100);
        let clamped = clamp_line(&line, 64);
        assert!(clamped.ends_with(TRUNCATED_MARK));
        assert!(clamped.starts_with("aaa"));
        assert!(clamped.len() <= 64);
    }

    #[test]
    fn test_result_never_exceeds_limit() {
        let line = "x".repeat(10_000);
        for max in [8usize, 16, 100, DEFAULT_MAX_LINE_BYTES] {
            assert!(clamp_line(&line, max).len() <= max);
        }
    }

    #[test]
    fn test_cjk_char_boundary_safe() {
        // 每个 CJK 字符 3 字节；截断点必须落在 char boundary 上
        let line = "中".repeat(1000); // 3000 bytes
        let clamped = clamp_line(&line, 64);
        assert!(clamped.len() <= 64);
        // 结果必须是合法 UTF-8（String 构造本身已保证），且不含半个字
        assert!(clamped.ends_with(TRUNCATED_MARK));
        let body = clamped.trim_end_matches(TRUNCATED_MARK);
        assert!(body.chars().all(|c| c == '中'));
        assert_eq!(body.len() % 3, 0);
    }

    #[test]
    fn test_content_preserves_line_count() {
        let long = "z".repeat(200);
        let content = format!("first\n{long}\nlast");
        let clamped = clamp_content(&content, 64);
        let lines: Vec<&str> = clamped.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "first");
        assert_eq!(lines[2], "last");
        assert!(lines[1].ends_with(TRUNCATED_MARK));
    }

    #[test]
    fn test_empty_and_single_line() {
        assert_eq!(clamp_content("", 64), "");
        assert_eq!(clamp_content("ok", 64), "ok");
        assert_eq!(clamp_content(&"q".repeat(999), 16).len() <= 16, true);
    }

    #[test]
    fn test_default_threshold_is_8k() {
        assert_eq!(DEFAULT_MAX_LINE_BYTES, 8192);
    }
}
