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
use std::collections::HashMap;

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

/// 预测/校验闭环条目 — 把「当时的判断」与「事后实际结果」绑在同一条记录里。
///
/// 借鉴来源：yologdev/yoyo-evolve — Day 112 `/risk validate`。yoyo-evolve 用
/// `/risk snapshot` 落盘预测、用 `/risk validate` 事后回 git 对账（哪些进了
/// revert、哪些进了 fix），把「我以为会坏的」和「实际坏的」放在同一条对账记录
/// 上展现 hits / misses。q-body 对应改法：单一结构 + `Option<>` 字段就地从
/// 未校验切到已校验，避免双表/双查询，保证生命周期单调推进。
///
/// 本期只落最小数据结构 + 单测；A2A `journal_validate_prediction` skill 接线
/// 属架构级改动，按 06-14 / 06-15 既定先例推迟到后续 PR。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionEntry {
    pub predicted_at: DateTime<Utc>,
    pub prediction: String,
    pub validated_at: Option<DateTime<Utc>>,
    pub actual: Option<String>,
    pub delta: Option<String>,
}

impl PredictionEntry {
    /// 是否已经走完校验阶段（即 `actual` 已填）。
    pub fn is_validated(&self) -> bool {
        self.validated_at.is_some() && self.actual.is_some()
    }
}

/// Journal — 进化事件 + 预测/校验闭环存储
///
/// 06-21 扩展：增加 `cycle_id` + `seen_state`，提供 reset-cycle 语义防止
/// 每日养料回灌把同一份 source 反复落到 Journal。
///
/// 借鉴来源：yologdev/yoyo-evolve — Day 112-113 的
/// `social session (learnings + seen-state)` + `skill-evolve: reset counter (cycle ...)`
/// 模式。yoyo 在每个进化周期边界处把 counter / seen-state 一起 reset，
/// 配合一份 `seen-state` map 记录当前 cycle 内已处理过的事件，下一轮判重
/// 直接查 map 而不是回扫整个 journal。
#[derive(Debug, Clone)]
pub struct Journal {
    events: Vec<EvolutionEvent>,
    predictions: Vec<PredictionEntry>,
    /// 当前 cycle 起始时间戳（每次 `reset_cycle` 刷新到 now）。
    cycle_id: DateTime<Utc>,
    /// 当前 cycle 内已见过的事件 id → 最近一次 mark_seen 时间。
    /// 每次 `reset_cycle` 整体清空。
    seen_state: HashMap<String, DateTime<Utc>>,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

impl Journal {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            predictions: Vec::new(),
            cycle_id: Utc::now(),
            seen_state: HashMap::new(),
        }
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

    /// 写入一条未校验的预测（≈ yoyo-evolve `/risk snapshot`）。
    ///
    /// 返回该预测在内部存储中的索引，后续可用 `validate_prediction` 回填校验结果。
    pub fn record_prediction(&mut self, prediction: String) -> usize {
        self.predictions.push(PredictionEntry {
            predicted_at: Utc::now(),
            prediction,
            validated_at: None,
            actual: None,
            delta: None,
        });
        self.predictions.len() - 1
    }

    /// 事后回填预测的实际结果与 delta（≈ yoyo-evolve `/risk validate`）。
    ///
    /// 索引越界或该条已校验过时返回 `false`，避免覆盖既有对账记录。
    pub fn validate_prediction(&mut self, idx: usize, actual: String, delta: String) -> bool {
        match self.predictions.get_mut(idx) {
            Some(entry) if entry.validated_at.is_none() => {
                entry.validated_at = Some(Utc::now());
                entry.actual = Some(actual);
                entry.delta = Some(delta);
                true
            }
            _ => false,
        }
    }

    /// 还未校验的预测（snapshot 已落但 validate 还没回填）。
    pub fn pending_predictions(&self) -> Vec<&PredictionEntry> {
        self.predictions
            .iter()
            .filter(|p| !p.is_validated())
            .collect()
    }

    /// 已校验完成的预测（用于后续 precision-at-N 等聚合分析的数据底座）。
    pub fn validated_predictions(&self) -> Vec<&PredictionEntry> {
        self.predictions
            .iter()
            .filter(|p| p.is_validated())
            .collect()
    }

    /// 预测总数（pending + validated）。
    pub fn total_predictions(&self) -> usize {
        self.predictions.len()
    }

    /// 当前 cycle 起始时间戳。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — `skill-evolve: reset counter (cycle ...)`
    /// 每个进化周期有显式起点，cycle_id 起观测与对账作用。
    pub fn cycle_id(&self) -> DateTime<Utc> {
        self.cycle_id
    }

    /// 把一个事件 id 标记为本 cycle 已见。重复 mark 会刷新 last_seen_at。
    pub fn mark_seen(&mut self, event_id: impl Into<String>) {
        self.seen_state.insert(event_id.into(), Utc::now());
    }

    /// 该事件 id 是否在当前 cycle 内已被 mark_seen 过。
    ///
    /// 配合每日养料回灌：cron 跑到一份 source 前先查这个，命中即跳过，
    /// 避免在同一 cycle 内反复回灌相同养料。
    pub fn was_seen_in_cycle(&self, event_id: &str) -> bool {
        self.seen_state.contains_key(event_id)
    }

    /// 当前 cycle 内已见事件 id 的数量。
    pub fn seen_count(&self) -> usize {
        self.seen_state.len()
    }

    /// 进入下一个 cycle：刷新 `cycle_id` 到 now，并清空 `seen_state`。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — `skill-evolve: reset counter (cycle ...)`
    /// 在 cycle 边界处把 counter / seen-state 一起 reset，让 cycle 之间相互独立。
    /// 注意：进化事件 (`events`) 和预测条目 (`predictions`) 是 append-only 历史，
    /// 不在 cycle 内被清空，只重置 cycle 边界 + 当前 cycle 的去重状态。
    pub fn reset_cycle(&mut self) {
        self.cycle_id = Utc::now();
        self.seen_state.clear();
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

    #[test]
    fn test_prediction_record_and_validate() {
        let mut journal = Journal::new();

        // 写入两条未校验的预测（≈ /risk snapshot）
        let idx_a = journal.record_prediction("handler.rs 下次会被 refactor".into());
        let idx_b = journal.record_prediction("a2a/types.rs 下次会引入 dedup 候选".into());

        assert_eq!(journal.total_predictions(), 2);
        assert_eq!(journal.pending_predictions().len(), 2);
        assert_eq!(journal.validated_predictions().len(), 0);

        let pending_a = &journal.pending_predictions()[0];
        assert!(pending_a.validated_at.is_none());
        assert!(pending_a.actual.is_none());
        assert!(pending_a.delta.is_none());
        assert!(!pending_a.is_validated());

        // 校验第一条（≈ /risk validate） — 回填 actual / delta
        let ok = journal.validate_prediction(
            idx_a,
            "handler.rs 被 dispatch 重构（hits）".into(),
            "预测：refactor；实际：refactor → 命中".into(),
        );
        assert!(ok);

        assert_eq!(journal.pending_predictions().len(), 1);
        assert_eq!(journal.validated_predictions().len(), 1);

        let validated = &journal.validated_predictions()[0];
        assert!(validated.is_validated());
        assert_eq!(
            validated.actual.as_deref(),
            Some("handler.rs 被 dispatch 重构（hits）")
        );
        assert_eq!(
            validated.delta.as_deref(),
            Some("预测：refactor；实际：refactor → 命中")
        );
        assert!(validated.validated_at.is_some());

        // 已校验的条目不允许被二次覆盖
        let twice = journal.validate_prediction(idx_a, "覆盖".into(), "覆盖".into());
        assert!(!twice, "重复校验同一条预测应返回 false，避免覆盖对账记录");

        // 越界索引应返回 false，且不影响存储
        let out_of_range = journal.validate_prediction(999, "x".into(), "x".into());
        assert!(!out_of_range);
        assert_eq!(journal.total_predictions(), 2);

        // 第二条仍处于 pending
        let still_pending = journal
            .pending_predictions()
            .iter()
            .any(|p| p.prediction == "a2a/types.rs 下次会引入 dedup 候选");
        assert!(still_pending);
        assert_eq!(idx_b, 1);
    }

    #[test]
    fn test_cycle_reset_and_seen_state() {
        let mut journal = Journal::new();

        let cycle_0 = journal.cycle_id();

        // 初始：seen_state 为空，任何事件 id 都未被见过
        assert_eq!(journal.seen_count(), 0);
        assert!(!journal.was_seen_in_cycle("source-a"));

        // 标记两条事件 id 为本 cycle 已见
        journal.mark_seen("source-a");
        journal.mark_seen("source-b");
        assert_eq!(journal.seen_count(), 2);
        assert!(journal.was_seen_in_cycle("source-a"));
        assert!(journal.was_seen_in_cycle("source-b"));
        assert!(!journal.was_seen_in_cycle("source-c"));

        // 重复 mark 同一 id 不会增加去重表大小
        journal.mark_seen("source-a");
        assert_eq!(journal.seen_count(), 2);

        // 落几条事件 / 一条预测，确认它们不会在 reset_cycle 时被清空
        journal.record(
            EvolutionSignal::Refactor,
            "session 2026-06-21".into(),
            "extract dispatch helper".into(),
        );
        let pred_idx = journal.record_prediction("下次会出 dedup 候选".into());

        // 等一小段时间确保 cycle_id 时间戳前进
        std::thread::sleep(std::time::Duration::from_millis(2));

        // 进入下一个 cycle
        journal.reset_cycle();

        // cycle_id 应向前推进
        assert!(
            journal.cycle_id() > cycle_0,
            "reset_cycle 后 cycle_id 应严格大于上一轮 cycle_id"
        );
        // seen_state 已清空
        assert_eq!(journal.seen_count(), 0);
        assert!(!journal.was_seen_in_cycle("source-a"));
        assert!(!journal.was_seen_in_cycle("source-b"));

        // append-only 的历史不被清空
        assert_eq!(journal.total_events(), 1);
        assert_eq!(journal.total_predictions(), 1);
        assert_eq!(pred_idx, 0);

        // 新 cycle 可以再次标记同名事件，且不与上一 cycle 状态混淆
        journal.mark_seen("source-a");
        assert!(journal.was_seen_in_cycle("source-a"));
        assert_eq!(journal.seen_count(), 1);
    }
}
