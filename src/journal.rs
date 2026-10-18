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

mod persistence;
mod soul_context;
mod types;

pub use types::{
    AssessmentEntry, EvolutionEvent, EvolutionSignal, EvolutionStage, PredictionEntry, SkipEvent,
    EVOLUTION_SIGNALS,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};

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
    /// 条件门显式跳过记录（append-only，不随 reset_cycle 清空）。
    skips: Vec<SkipEvent>,
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
            skips: Vec::new(),
            cycle_id: Utc::now(),
            seen_state: HashMap::new(),
        }
    }

    /// 记录一条条件门跳过事件（gate + reason + 时间戳）。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — validation event 把 "unhittable" 说出口。
    /// 所有条件门的静默跳过路径都应改调本方法，让"跳过了什么"可审计。
    pub fn record_skip(&mut self, gate: impl Into<String>, reason: impl Into<String>) {
        self.skips.push(SkipEvent {
            timestamp: Utc::now(),
            gate: gate.into(),
            reason: reason.into(),
        });
    }

    /// 返回所有 skip 事件（按落盘顺序）。
    pub fn skips(&self) -> &[SkipEvent] {
        &self.skips
    }

    /// skip 事件总数。
    pub fn total_skips(&self) -> usize {
        self.skips.len()
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
            Some(event) if event.consumed_at.is_some() => {
                self.record_skip("mark_consumed", "already_consumed");
                false
            }
            _ => {
                self.record_skip("mark_consumed", "index_out_of_range");
                false
            }
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
            Some(entry) if entry.validated_at.is_some() => {
                self.record_skip("validate_prediction", "already_validated");
                false
            }
            _ => {
                self.record_skip("validate_prediction", "index_out_of_range");
                false
            }
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
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_persistence;
#[cfg(test)]
mod tests_soul_slice;
