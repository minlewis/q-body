//! journal::types — Journal 数据类型定义（纯移动自 journal.rs，issue: 1603 行拆分）
//!
//! 本模块只含类型与类型自身的小 impl；Journal 存储/持久化逻辑留在父模块。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

/// signal 闭集词表 — 单一事实源（issue #128）。
///
/// JournalRecord 处理器自有闭集：handler 解析分支与用户可见错误文案
/// 都从这两个常量派生，新增信号只改枚举 + 此处词表，golden 测试钉住。
pub const EVOLUTION_SIGNALS: [EvolutionSignal; 5] = [
    EvolutionSignal::Refactor,
    EvolutionSignal::Dedup,
    EvolutionSignal::Test,
    EvolutionSignal::Perf,
    EvolutionSignal::Bump,
];

impl EvolutionSignal {
    /// 词表内的规范小写拼写（与 JSONL 序列化 / API 词一致）
    pub fn as_str(&self) -> &'static str {
        match self {
            EvolutionSignal::Refactor => "refactor",
            EvolutionSignal::Dedup => "dedup",
            EvolutionSignal::Test => "test",
            EvolutionSignal::Perf => "perf",
            EvolutionSignal::Bump => "bump",
        }
    }

    /// 用户可见词表文案（handler invalid_params 错误消息单一来源）
    pub fn vocab_hint() -> String {
        EVOLUTION_SIGNALS
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }

    /// 按词表解析入站 signal 字符串（大小写不敏感，同 handler 现行为）
    pub fn from_vocab(raw: &str) -> Option<EvolutionSignal> {
        EVOLUTION_SIGNALS
            .iter()
            .find(|s| s.as_str() == raw.to_lowercase())
            .cloned()
    }
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

/// 单条 skip 事件 — 条件门的显式跳过记录。
///
/// 借鉴来源：yologdev/yoyo-evolve — validation event 把 "unhittable" 说出口。
/// 条件触发型路径被静默跳过时，"没报错 = 没发生" 是自 concealing 的盲区
/// （SOUL §15 假阴性陷阱的同构问题）。每个 gate 的跳过都应落一条带 reason
/// 与时间戳的显式记录，让"跳过了什么、为什么跳"可审计可回放。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkipEvent {
    pub timestamp: DateTime<Utc>,
    /// 哪个条件门跳过（如 "mark_consumed" / "validate_prediction" / "load_jsonl"）
    pub gate: String,
    /// 跳过原因（如 "already_consumed" / "index_out_of_range"）
    pub reason: String,
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
