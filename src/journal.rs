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
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};

/// 进化信号类型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionEvent {
    pub timestamp: DateTime<Utc>,
    pub signal: EvolutionSignal,
    pub source: String,
    pub suggestion: String,
    /// 实际改动内容（闭环推进到 Action 阶段后填入）
    pub action: Option<String>,
    /// 验证结果（闭环推进到 Verification 阶段后填入，如 "cargo test passed"）
    pub verification: Option<String>,
    /// 该条目是否已被下游消费（None = 尚未消费）。
    ///
    /// 与 Journal 级 `seen_state`（按 event-id 字符串做 cycle 内粗粒度去重）互补：
    /// `consumed_at` 做逐条目细粒度消费追踪，下游直接查 `unconsumed_events()` 拿待处理集合。
    pub consumed_at: Option<DateTime<Utc>>,
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

    /// 该条目是否已被下游消费（`consumed_at` 已填）。
    pub fn is_consumed(&self) -> bool {
        self.consumed_at.is_some()
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// 周期评估条目 — 在进化周期边界处对本周期的 suggestion 有效性、prediction 命中率
/// 与下周期方向做一次结构化总结。
///
/// 借鉴来源：yologdev/yoyo-evolve — assessment 步骤。yoyo-evolve 在每个进化周期
/// 末尾做一次 assessment：统计本周期 suggestion 里哪些有效 / 哪些无效、回看
/// prediction 的命中情况（与 Day 112 `/risk validate` 的 hits vs misses 对账），
/// 并据此定下下一周期的方向——这是 Source→Suggestion→Action→Verification 闭环
/// 之后的收口。q-body 对应改法：单一结构把「这轮学到了什么、下轮往哪走」落盘成
/// 一条可回溯的评估记录；`prediction_hit_rate` 用 `Option<f64>`（无已校验
/// prediction 时为 None），与 `PredictionEntry` 的「未校验→已校验」生命周期对齐。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssessmentEntry {
    pub assessed_at: DateTime<Utc>,
    /// 本周期内有效的 suggestion 数
    pub effective_suggestions: usize,
    /// 本周期内无效的 suggestion 数
    pub ineffective_suggestions: usize,
    /// prediction 命中率（已校验预测中命中的比例）；无已校验预测时为 None。
    pub prediction_hit_rate: Option<f64>,
    /// 下一周期方向
    pub next_direction: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    events: Vec<EvolutionEvent>,
    predictions: Vec<PredictionEntry>,
    /// 周期评估记录（每个进化周期边界处落盘一条，append-only）。
    assessments: Vec<AssessmentEntry>,
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
            assessments: Vec::new(),
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
            consumed_at: None,
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
            consumed_at: None,
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

    /// 标记指定索引的进化事件为已消费（填入 `consumed_at`）。
    ///
    /// 越界或已消费的条目返回 `false`，避免重复标记。
    /// 与 `mark_seen` 互补：mark_seen 按 event-id 做 cycle 内去重，
    /// mark_consumed 按索引做逐条目消费追踪。
    pub fn mark_consumed(&mut self, idx: usize) -> bool {
        match self.events.get_mut(idx) {
            Some(event) if event.consumed_at.is_none() => {
                event.consumed_at = Some(Utc::now());
                true
            }
            _ => false,
        }
    }

    /// 返回尚未消费的进化事件（`consumed_at` 为 None）。
    ///
    /// 下游消费方直接调这个拿到待处理集合，无需自己维护外部状态。
    pub fn unconsumed_events(&self) -> Vec<&EvolutionEvent> {
        self.events.iter().filter(|e| !e.is_consumed()).collect()
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

    /// 记录一条周期评估（≈ yoyo-evolve assessment 步骤）。
    ///
    /// 在周期边界处把本周期 suggestion 有效性、prediction 命中率与下周期方向
    /// 结构化落盘，作为 Source→Suggestion→Action→Verification 闭环之后的收口。
    /// 评估记录是 append-only，不随 `reset_cycle` 清空（与 events / predictions 一致）。
    pub fn record_assessment(
        &mut self,
        effective_suggestions: usize,
        ineffective_suggestions: usize,
        prediction_hit_rate: Option<f64>,
        next_direction: String,
    ) {
        self.assessments.push(AssessmentEntry {
            assessed_at: Utc::now(),
            effective_suggestions,
            ineffective_suggestions,
            prediction_hit_rate,
            next_direction,
        });
    }

    /// 返回所有周期评估记录（按落盘顺序）。
    pub fn assessments(&self) -> &[AssessmentEntry] {
        &self.assessments
    }

    /// 评估记录总数。
    pub fn total_assessments(&self) -> usize {
        self.assessments.len()
    }

    /// 将整个 Journal 以 JSONL 格式持久化到指定文件。
    ///
    /// JSONL 格式：每行一条独立的 JSON 记录。
    /// 写入顺序：events 行 → predictions 行 → assessments 行 → journal metadata 行。
    /// 写入是原子性的：先写入临时文件，成功后 rename 到目标路径。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — Day 129 `/risk validate` JSONL 持久化模式。
    /// yoyo-evolve 把 journal 数据以 JSONL append-only 写入文件，每行一条独立 JSON，
    /// 天然支持增量追加、无需锁或事务。
    pub fn persist_to_jsonl(&self, path: &str) -> io::Result<()> {
        let tmp_path = format!("{}.tmp", path);
        {
            let mut file = std::fs::File::create(&tmp_path)?;
            // events
            for event in &self.events {
                let line = serde_json::to_string(event)?;
                writeln!(file, "{}", line)?;
            }
            // predictions
            for pred in &self.predictions {
                let line = serde_json::to_string(pred)?;
                writeln!(file, "{}", line)?;
            }
            // assessments
            for assessment in &self.assessments {
                let line = serde_json::to_string(assessment)?;
                writeln!(file, "{}", line)?;
            }
            // journal metadata — write a single "Journal" row with meta fields
            let meta = serde_json::json!({
                "type": "__journal_meta__",
                "cycle_id": self.cycle_id,
                "seen_state": self.seen_state,
            });
            let line = serde_json::to_string(&meta)?;
            writeln!(file, "{}", line)?;
        }
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// 从 JSONL 文件加载 Journal 数据。
    ///
    /// 逐行反序列化：events 行 → PredictionEntry → AssessmentEntry → meta 行（Journal）。
    /// 如果文件不存在，返回一个空的 Journal（不报错，方便首次使用）。
    ///
    /// 借鉴来源同 `persist_to_jsonl` — yologdev/yoyo-evolve Day 129 JSONL 持久化。
    pub fn load_from_jsonl(path: &str) -> io::Result<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::new());
            }
            Err(e) => return Err(e),
        };

        let mut events = Vec::new();
        let mut predictions = Vec::new();
        let mut assessments = Vec::new();
        let mut cycle_id = Utc::now();
        let mut seen_state = HashMap::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Try to parse as journal metadata
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                if val.get("type").and_then(|t| t.as_str()) == Some("__journal_meta__") {
                    if let Some(cid) = val.get("cycle_id").and_then(|v| v.as_str()) {
                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(cid) {
                            cycle_id = dt.with_timezone(&Utc);
                        }
                    }
                    if let Some(ss) = val.get("seen_state").and_then(|v| v.as_object()) {
                        for (k, v) in ss {
                            if let Some(ts) = v.as_str() {
                                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                                    seen_state.insert(k.clone(), dt.with_timezone(&Utc));
                                }
                            }
                        }
                    }
                    continue;
                }
                // Skip type markers not recognized
            }

            // Try each type in order
            if let Ok(event) = serde_json::from_str::<EvolutionEvent>(line) {
                events.push(event);
            } else if let Ok(pred) = serde_json::from_str::<PredictionEntry>(line) {
                predictions.push(pred);
            } else if let Ok(assessment) = serde_json::from_str::<AssessmentEntry>(line) {
                assessments.push(assessment);
            }
        }

        Ok(Self {
            events,
            predictions,
            assessments,
            cycle_id,
            seen_state,
        })
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

    #[test]
    fn test_event_consumed_state() {
        let mut journal = Journal::new();

        // 落三条事件
        journal.record(
            EvolutionSignal::Refactor,
            "session 2026-06-24".into(),
            "extract dispatch helper".into(),
        );
        journal.record(
            EvolutionSignal::Test,
            "session 2026-06-24".into(),
            "add consumed-state test".into(),
        );
        journal.record_loop(
            EvolutionSignal::Dedup,
            "session 2026-06-24".into(),
            "merge duplicate parsing".into(),
            "merged two parse paths".into(),
            "cargo test passed".into(),
        );

        // 初始：全部未消费
        assert_eq!(journal.unconsumed_events().len(), 3);
        assert!(!journal.events[0].is_consumed());

        // 消费第一条
        let ok = journal.mark_consumed(0);
        assert!(ok);
        assert!(journal.events[0].is_consumed());
        assert!(journal.events[0].consumed_at.is_some());
        assert_eq!(journal.unconsumed_events().len(), 2);

        // 重复消费同一条 → false
        let twice = journal.mark_consumed(0);
        assert!(!twice);

        // 消费第二条
        assert!(journal.mark_consumed(1));
        assert_eq!(journal.unconsumed_events().len(), 1);

        // 越界索引 → false
        assert!(!journal.mark_consumed(999));

        // 第三条仍未消费
        let remaining = journal.unconsumed_events();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].signal, EvolutionSignal::Dedup);

        // consumed_at 不被 reset_cycle 清空（与 append-only 历史一致）
        journal.reset_cycle();
        assert!(journal.events[0].is_consumed());
        assert_eq!(journal.unconsumed_events().len(), 1);
    }

    #[test]
    fn test_assessment_record() {
        let mut journal = Journal::new();

        // 第一周期：2 有效 / 1 无效，prediction 命中率 0.5，方向：收敛 dedup
        journal.record_assessment(2, 1, Some(0.5), "收敛 dedup 候选".into());
        assert_eq!(journal.total_assessments(), 1);

        let a = &journal.assessments()[0];
        assert_eq!(a.effective_suggestions, 2);
        assert_eq!(a.ineffective_suggestions, 1);
        assert_eq!(a.prediction_hit_rate, Some(0.5));
        assert_eq!(a.next_direction, "收敛 dedup 候选");
        assert!(a.assessed_at <= Utc::now());

        // 第二周期：尚无已校验 prediction → hit_rate = None
        journal.record_assessment(0, 0, None, "积攒更多 prediction 样本".into());
        assert_eq!(journal.total_assessments(), 2);
        assert_eq!(journal.assessments()[1].prediction_hit_rate, None);

        // reset_cycle 不清空 append-only 的评估记录（与 events / predictions 一致）
        journal.reset_cycle();
        assert_eq!(journal.total_assessments(), 2);
    }

    #[test]
    fn test_persist_and_load_jsonl_roundtrip() {
        let mut journal = Journal::new();

        // 落几条事件、两条预测、一条评估
        journal.record(
            EvolutionSignal::Refactor,
            "session 2026-07-07".into(),
            "add JSONL persistence".into(),
        );
        journal.record_loop(
            EvolutionSignal::Test,
            "session 2026-07-07".into(),
            "add roundtrip test".into(),
            "added test_persist_and_load_jsonl_roundtrip".into(),
            "cargo test passed".into(),
        );
        let pred_idx = journal.record_prediction("下次会加 data-driven 测试".into());
        journal.validate_prediction(pred_idx, "roundtrip test added".into(), "预测：data-driven；实际：roundtrip → 接近".into());
        journal.record_assessment(2, 0, Some(0.8), "JSONL 持久化+更完善 data-driven".into());

        // 标记 seen_state
        journal.mark_seen("source-a");
        journal.mark_seen("source-b");

        let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let path = format!("/tmp/test_journal_{}.jsonl", timestamp);

        // persist
        journal.persist_to_jsonl(&path).expect("persist should succeed");

        // 验证文件存在且非空
        let content = std::fs::read_to_string(&path).expect("should read file");
        assert!(!content.is_empty(), "JSONL file should not be empty");
        let line_count = content.lines().count();
        assert_eq!(line_count, 5, "2 events + 1 prediction + 1 assessment + 1 meta = 5 lines");

        // load
        let loaded = Journal::load_from_jsonl(&path).expect("load should succeed");

        // 验证内容一致性
        assert_eq!(loaded.total_events(), 2);
        assert_eq!(loaded.total_predictions(), 1);
        assert_eq!(loaded.total_assessments(), 1);

        // 事件内容
        let loaded_refactor = loaded.events.iter().find(|e| e.signal == EvolutionSignal::Refactor);
        assert!(loaded_refactor.is_some());
        assert_eq!(loaded_refactor.unwrap().source, "session 2026-07-07");

        // 预测
        let loaded_pred = &loaded.predictions[0];
        assert!(loaded_pred.is_validated());
        assert_eq!(loaded_pred.actual.as_deref(), Some("roundtrip test added"));

        // 评估
        assert_eq!(loaded.assessments[0].effective_suggestions, 2);
        assert_eq!(loaded.assessments[0].prediction_hit_rate, Some(0.8));

        // seen_state 持久化
        assert!(loaded.was_seen_in_cycle("source-a"));
        assert!(loaded.was_seen_in_cycle("source-b"));
        assert!(!loaded.was_seen_in_cycle("source-c"));
        assert_eq!(loaded.seen_count(), 2);

        // 清理
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_jsonl_file_not_found_returns_empty() {
        let journal = Journal::load_from_jsonl("/tmp/nonexistent_journal.jsonl")
            .expect("missing file should return empty journal");
        assert_eq!(journal.total_events(), 0);
        assert_eq!(journal.total_predictions(), 0);
        assert_eq!(journal.total_assessments(), 0);
        assert_eq!(journal.seen_count(), 0);
    }

    #[test]
    fn test_persist_and_load_empty_journal() {
        let journal = Journal::new();
        let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let path = format!("/tmp/test_empty_journal_{}.jsonl", timestamp);

        journal.persist_to_jsonl(&path).expect("persist empty journal should succeed");

        let loaded = Journal::load_from_jsonl(&path).expect("load empty journal should succeed");
        assert_eq!(loaded.total_events(), 0);
        assert_eq!(loaded.total_predictions(), 0);
        assert_eq!(loaded.total_assessments(), 0);
        assert_eq!(loaded.seen_count(), 0);

        let _ = std::fs::remove_file(&path);
    }
}
