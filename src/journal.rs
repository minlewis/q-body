//! Journal — q-body 每日进化信号记录
//!
//! 用于记录供给侧养料回灌闭环中的各类进化事件。
//! 每个事件携带：时间戳、信号类型、来源、建议。
//!
//! 06-14 扩展：把每日养料回灌闭环拆为四阶段结构化落盘——
//! `Source`（养料来源）→ `Suggestion`（建议改法）→ `Action`（实际改动）→ `Verification`（编译/测试验证）。
//!
//! 借鉴来源：yologdev/yoyo-evolve — 每个进化周期把「读了什么源 → 计划改什么 →
//! 实际改了什么 → 测试是否通过」以 append-only 写入 journals/JOURNAL.md，
//! 并用 .skill_evolve_counter 累计进化信号。

use chrono::{DateTime, Utc};

/// 进化信号类型
#[derive(Debug, Clone, PartialEq)]
pub enum EvolutionSignal {
    /// 重构信号
    Refactor,
    /// 去重信号
    Dedup,
    /// 新测试信号
    Test,
    /// 性能改进信号
    Perf,
    /// 依赖/版本 bump 信号
    Bump,
}

/// 进化闭环阶段 — 养料回灌四阶段
///
/// 对应每日养料回灌的完整生命周期：养料从哪来 → 建议怎么改 → 实际改了什么 → 是否验证通过。
/// 用于把回灌闭环结构化落盘，并为后续 dedup/refactor 候选检测（同类事件≥2 次）提供按阶段计数能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvolutionStage {
    /// 养料来源
    Source,
    /// 建议改法
    Suggestion,
    /// 实际改动
    Action,
    /// 编译/测试验证
    Verification,
}

/// 单条进化事件记录
#[derive(Debug, Clone)]
pub struct EvolutionEvent {
    pub timestamp: DateTime<Utc>,
    pub signal: EvolutionSignal,
    pub source: String,
    pub suggestion: String,
    /// 实际改动内容（闭环推进到 Action 阶段后填入）
    pub action: Option<String>,
    /// 验证结果（闭环推进到 Verification 阶段后填入，如 "cargo test passed"）
    pub verification: Option<String>,
}

impl EvolutionEvent {
    /// 取某阶段的文本内容；Action / Verification 未填时返回 None。
    pub fn stage_text(&self, stage: EvolutionStage) -> Option<&str> {
        match stage {
            EvolutionStage::Source => Some(&self.source),
            EvolutionStage::Suggestion => Some(&self.suggestion),
            EvolutionStage::Action => self.action.as_deref(),
            EvolutionStage::Verification => self.verification.as_deref(),
        }
    }

    /// 该事件是否已推进到指定阶段（即该阶段已有内容）。
    pub fn reached(&self, stage: EvolutionStage) -> bool {
        self.stage_text(stage).is_some()
    }
}

/// Journal — 进化事件存储
#[derive(Debug, Clone)]
pub struct Journal {
    events: Vec<EvolutionEvent>,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

impl Journal {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// 记录一条进化事件（仅 Source / Suggestion 两阶段，action/verification 暂缺）。
    pub fn record(&mut self, signal: EvolutionSignal, source: String, suggestion: String) {
        self.events.push(EvolutionEvent {
            timestamp: Utc::now(),
            signal,
            source,
            suggestion,
            action: None,
            verification: None,
        });
    }

    /// 记录一条完整的四阶段闭环事件：养料来源 → 建议改法 → 实际改动 → 验证。
    pub fn record_loop(
        &mut self,
        signal: EvolutionSignal,
        source: String,
        suggestion: String,
        action: String,
        verification: String,
    ) {
        self.events.push(EvolutionEvent {
            timestamp: Utc::now(),
            signal,
            source,
            suggestion,
            action: Some(action),
            verification: Some(verification),
        });
    }

    /// 返回指定信号类型的计数器
    pub fn count_by_signal(&self, signal: &EvolutionSignal) -> usize {
        self.events.iter().filter(|e| e.signal == *signal).count()
    }

    /// 统计已推进到指定阶段的事件数（用于 dedup/refactor 候选检测：同类事件≥2 次）。
    pub fn count_by_stage(&self, stage: EvolutionStage) -> usize {
        self.events.iter().filter(|e| e.reached(stage)).count()
    }

    /// 返回事件总数
    pub fn total_events(&self) -> usize {
        self.events.len()
    }

    /// 同一信号类型的事件是否已累计到去重/重构候选阈值（≥2 次即候选）。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — 当 skill-evolve counter 同类信号反复 bump
    /// 时，把它视作小步清理（dedup/refactor）候选，避免重复模式持续累积无人收敛。
    pub fn is_dedup_candidate(&self, signal: &EvolutionSignal) -> bool {
        self.count_by_signal(signal) >= 2
    }

    /// 返回当前所有已达到去重/重构候选阈值（≥2 次）的信号类型。
    ///
    /// 用于每日 regression 检测：把同类高频信号集中暴露，提示下一轮回灌优先收敛。
    pub fn dedup_candidates(&self) -> Vec<EvolutionSignal> {
        use EvolutionSignal::*;
        [Refactor, Dedup, Test, Perf, Bump]
            .into_iter()
            .filter(|s| self.is_dedup_candidate(s))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_tracks_evolution_signals() {
        let mut journal = Journal::new();

        journal.record(
            EvolutionSignal::Refactor,
            "c197a87c2805".into(),
            "extract handler error logic".into(),
        );
        journal.record(
            EvolutionSignal::Dedup,
            "c197a87c2805".into(),
            "merge duplicate task parsing".into(),
        );
        journal.record(
            EvolutionSignal::Test,
            "c197a87c2805".into(),
            "add agent-card smoke test".into(),
        );
        journal.record(
            EvolutionSignal::Perf,
            "c197a87c2805".into(),
            "use RwLock instead of Mutex".into(),
        );
        journal.record(
            EvolutionSignal::Bump,
            "c197a87c2805".into(),
            "bump tokio to 1.42".into(),
        );

        // 验证每种信号都能被统计到 Journal
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Refactor), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Dedup), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Test), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Perf), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Bump), 1);
        assert_eq!(journal.total_events(), 5);
    }

    #[test]
    fn test_evolution_loop_four_stages() {
        let mut journal = Journal::new();

        // 一条完整的四阶段闭环事件
        journal.record_loop(
            EvolutionSignal::Refactor,
            "session 2026-06-14".into(),
            "add evolution event record type".into(),
            "added EvolutionStage enum + record_loop/count_by_stage".into(),
            "cargo test passed".into(),
        );
        // 一条仅 Source/Suggestion 的事件（闭环未走完）
        journal.record(
            EvolutionSignal::Test,
            "session 2026-06-14".into(),
            "add smoke test".into(),
        );

        assert_eq!(journal.total_events(), 2);

        // 两条事件都到达 Source / Suggestion 阶段
        assert_eq!(journal.count_by_stage(EvolutionStage::Source), 2);
        assert_eq!(journal.count_by_stage(EvolutionStage::Suggestion), 2);
        // 只有第一条完整闭环事件到达 Action / Verification
        assert_eq!(journal.count_by_stage(EvolutionStage::Action), 1);
        assert_eq!(journal.count_by_stage(EvolutionStage::Verification), 1);

        // stage_text 取值
        let complete = journal
            .events
            .iter()
            .find(|e| e.reached(EvolutionStage::Verification))
            .unwrap();
        assert_eq!(
            complete.stage_text(EvolutionStage::Action),
            Some("added EvolutionStage enum + record_loop/count_by_stage")
        );
    }

    #[test]
    fn test_dedup_candidate_threshold() {
        let mut journal = Journal::new();

        // 同类信号刚出现 1 次 → 不算候选
        journal.record(
            EvolutionSignal::Refactor,
            "session 2026-06-15".into(),
            "extract handler error logic".into(),
        );
        assert!(!journal.is_dedup_candidate(&EvolutionSignal::Refactor));
        assert!(journal.dedup_candidates().is_empty());

        // 同类信号累计到 2 次 → 标记为 dedup/refactor 候选
        journal.record(
            EvolutionSignal::Refactor,
            "session 2026-06-15".into(),
            "extract task parsing".into(),
        );
        assert!(journal.is_dedup_candidate(&EvolutionSignal::Refactor));

        // 另一类信号刚出现 1 次 → 仍不算候选
        journal.record(
            EvolutionSignal::Test,
            "session 2026-06-15".into(),
            "add agent-card smoke test".into(),
        );
        assert!(!journal.is_dedup_candidate(&EvolutionSignal::Test));

        // dedup_candidates 只列出已达阈值的类型
        let candidates = journal.dedup_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], EvolutionSignal::Refactor);

        // 第二类也累计到 2 次
        journal.record(
            EvolutionSignal::Test,
            "session 2026-06-15".into(),
            "add health smoke test".into(),
        );
        let candidates = journal.dedup_candidates();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.contains(&EvolutionSignal::Refactor));
        assert!(candidates.contains(&EvolutionSignal::Test));
    }
}
