//! Reflect — 任务反思 + 独立评分（Phase 1 评估闭环，2026-09-12 老板五阶段拍板）
//!
//! 设计判据（TAO 三问）：
//! 1. 接线：handler.rs `Reflect` A2A 方法——外部送入反思文本，返回评分与固化判定
//! 2. 能进 SOUL.md 吗：不能——评分是确定性代码逻辑，LLM 做不对（会刷分）
//! 3. 损掉了什么：什么都没损——170 行纯函数，评分规则显式可测试
//!
//! 老板铁律：**评分函数跟 agent 解耦，别让 agent 自己给自己打分**。
//! 本模块全部为确定性规则评分（关键词覆盖/长度/结构完备性），
//! 不调用 LLM，不存在自我刷分路径。

/// 单项评分（0.0 ~ 1.0）
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreItem {
    pub name: &'static str,
    pub score: f64,
    pub weight: f64,
}

/// 评分结果
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreReport {
    pub items: Vec<ScoreItem>,
    pub total: f64,
}

/// 固化判定（TAO: 分数够高→固化为策略；不够→待观察）
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// 总分 ≥ threshold → 固化进 SOUL.md「已验证策略」区
    Solidify,
    /// 低于阈值 → 标记待观察，不固化
    Observe,
}

pub const DEFAULT_THRESHOLD: f64 = 0.7;

/// 结构完备性评分：反思必须回答三问（做了什么/结果如何/哪里改进）
/// 每个维度由一组锚点关键词覆盖，命中任一即得分。
fn structure_score(text: &str) -> f64 {
    let dims: [(&str, [&str; 4]); 3] = [
        ("做了什么", ["did", "done", "completed", "执行"]),
        ("结果如何", ["result", "output", "passed", "结果"]),
        ("哪里改进", ["improve", "lesson", "next", "改进"]),
    ];
    let lower = text.to_lowercase();
    let hits = dims
        .iter()
        .filter(|(_, anchors)| anchors.iter().any(|a| lower.contains(a)))
        .count();
    hits as f64 / 3.0
}

/// 具体性评分：反思不能是空话。有数字/路径/commit 的反思更可信。
fn specificity_score(text: &str) -> f64 {
    let has_digit = text.chars().any(|c| c.is_ascii_digit());
    let has_path = text.contains('/') || text.contains(".rs") || text.contains(".md");
    let has_marker = text.contains('#') || text.contains("commit");
    let signals = [has_digit, has_path, has_marker];
    let n = signals.iter().filter(|&&s| s).count();
    match n {
        3 => 1.0,
        2 => 0.8,
        1 => 0.5,
        _ => 0.2,
    }
}

/// 长度评分：太短（<50 字符）没有反思密度；太长（>5000）是垃圾倾倒
fn length_score(len: usize) -> f64 {
    match len {
        0..=49 => 0.2,
        50..=99 => 0.5,
        100..=5000 => 1.0,
        _ => 0.6,
    }
}

/// 对一段反思文本做确定性独立评分（不调 LLM）
pub fn score_reflection(text: &str) -> ScoreReport {
    let items = vec![
        ScoreItem {
            name: "structure",
            score: structure_score(text),
            weight: 0.4,
        },
        ScoreItem {
            name: "specificity",
            score: specificity_score(text),
            weight: 0.4,
        },
        ScoreItem {
            name: "length",
            score: length_score(text.chars().count()),
            weight: 0.2,
        },
    ];
    let total = items.iter().map(|i| i.score * i.weight).sum();
    ScoreReport { items, total }
}

/// 按阈值给固化判定
pub fn verdict(report: &ScoreReport, threshold: f64) -> Verdict {
    if report.total >= threshold {
        Verdict::Solidify
    } else {
        Verdict::Observe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "本次执行 completed：修改 handler.rs 重试路径，46 tests passed，结果 result 见 commit #a1b2c3。改进 improve：下次先量化再收窄关键词表。";

    #[test]
    fn test_good_reflection_solidifies() {
        let r = score_reflection(GOOD);
        assert!(r.total >= DEFAULT_THRESHOLD, "total={}", r.total);
        assert_eq!(verdict(&r, DEFAULT_THRESHOLD), Verdict::Solidify);
    }

    #[test]
    fn test_empty_text_observes() {
        let r = score_reflection("");
        assert_eq!(verdict(&r, DEFAULT_THRESHOLD), Verdict::Observe);
    }

    #[test]
    fn test_vague_text_observes() {
        // 三个维度都命中但零具体信号 → specificity 0.2 拖总分
        let r = score_reflection("did things, got result, will improve next time");
        assert_eq!(verdict(&r, DEFAULT_THRESHOLD), Verdict::Observe);
    }

    #[test]
    fn test_garbage_dump_penalized() {
        let long_junk = format!("{}{}", "执行执行改进改进".repeat(800), " done");
        let r = score_reflection(&long_junk);
        let len_item = r.items.iter().find(|i| i.name == "length").unwrap();
        assert!(len_item.score < 1.0, "超长应降分");
    }

    #[test]
    fn test_dimension_scores_independent() {
        let r = score_reflection("改进 improve：路径 src/handler.rs #98");
        let s = r.items.iter().find(|i| i.name == "structure").unwrap();
        assert!((s.score - 1.0 / 3.0).abs() < 1e-9, "只命中1/3维度");
    }

    #[test]
    fn test_verdict_threshold_exact() {
        let r = ScoreReport {
            items: vec![],
            total: 0.7,
        };
        assert_eq!(verdict(&r, DEFAULT_THRESHOLD), Verdict::Solidify);
    }
}
