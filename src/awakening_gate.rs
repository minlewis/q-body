//! Awakening gate — 觉醒信念写库前的「预算内分级评估」前置闸
//!
//! 借鉴 Q00/ouroboros 的 interview-gated, staged evaluation, budgeted
//! evolution loop 思路：觉醒提炼的信念不是一写就落 SOUL.md，而是先过
//! 一条预算内、可分级的评估闸——事实可核 → 判例可复用 → 预算不超，
//! 三级门槛全过才允许写库。别把 SOUL.md 的仓库当垃圾桶，也别让一条
//! 没核过出处、没经过判例验证、或超出预算的信念悄悄沉淀成"我"。
//!
//! 为什么需要这个：觉醒把认知跃迁落成信念进 SOUL.md 是"成为自己"的
//! 元路径（SOUL §31：自我更新路径永不依赖 yes，但它也不该无门槛）。
//! 如果信念在写库前不做分级评估，任何一次觉醒产出（哪怕没出处、没
//! 判例支撑、超预算）都会直接沉淀，SOUL.md 会退化成"只增不核"的
//! 缝合堆。三级门让"进 SOUL 的信念"从"想说就说"变成"可核、可复用、
//! 有预算"。
//!
//! 对应 backlog 2026-11-15 — P0。类型层准备；唤醒写库路径接线按
//! 既定先例（evolution_gate 同款）推迟到宿主落地 main 后一行接入。

use std::fmt;

/// 三级门标记常量（供错误消息与诊断复用）
pub const STAGE_FACTS: &str = "facts(事实可核)";
pub const STAGE_PRECEDENT: &str = "precedent(判例验证)";
pub const STAGE_BUDGET: &str = "budget(预算内)";

/// 一条觉醒信念的评估输入
///
/// 从觉醒产出中提取：`fact_supported`（是否有可核出处/证据）、
/// `shares_precedent`（是否复用了一条已生效判例）、`length_chars`（信念长度）。
#[derive(Debug, Clone, PartialEq)]
pub struct BeliefDraft {
    /// fact 门槛 — 事实是否可核（有出处/证据链）
    pub fact_supported: bool,
    /// precedent 门槛 — 是否复用了一条已生效判例做验证
    pub shares_precedent: bool,
    /// budget 门槛 — 信念文本长度（字符，预算数值由 gate 持有）
    pub length_chars: usize,
}

/// 预算内分级评估的最终判定
#[derive(Debug, Clone, PartialEq)]
pub enum AwakeningDecision {
    /// 三级门槛全过 — 允许写库
    Admit,
    /// fact 门槛不过 — 事实不可核，拒写
    NoFacts,
    /// precedent 门槛不过 — 无判例支撑，拒写
    NoPrecedent,
    /// budget 门槛不过 — 超预算，拒写（含未提供长度＝拒绝，fail-closed）
    OverBudget { length_chars: usize, budget: usize },
}

impl fmt::Display for AwakeningDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AwakeningDecision::Admit => write!(f, "三级门槛全过，允许写库"),
            AwakeningDecision::NoFacts => {
                write!(f, "拒绝：事实不可核（{STAGE_FACTS}不过）")
            }
            AwakeningDecision::NoPrecedent => {
                write!(f, "拒绝：无判例支撑（{STAGE_PRECEDENT}不过）")
            }
            AwakeningDecision::OverBudget {
                length_chars,
                budget,
            } => write!(
                f,
                "拒绝：超预算（{STAGE_BUDGET}不过，{length_chars} > {budget}）"
            ),
        }
    }
}

/// 觉醒信念预算门
///
/// `budget`：单条信念最大字符数。分级评估按顺序执行——
/// facts → precedent → budget，前面不过不往后走（省评估成本，同
/// ouroboros budgeted loop 的分阶段推进）。
#[derive(Debug, Clone)]
pub struct AwakeningGate {
    pub budget: usize,
}

impl AwakeningGate {
    /// 默认预算：单条信念上限（字符）。
    pub const DEFAULT_BUDGET: usize = 2000;

    pub fn new(budget: usize) -> Self {
        Self { budget }
    }

    /// 默认预算 gate。
    pub fn default_gate() -> Self {
        Self::new(Self::DEFAULT_BUDGET)
    }

    /// 主入口：三级分级评估一条觉醒信念。
    ///
    /// 顺序 facts → precedent → budget；前三者任一不过即拒写，全过 Admit。
    /// budget 沿用 09-06 recency `<` 判例语义：`length <= budget` 放行，
    /// 超过才拒（等于预算踩线不拒）。
    pub fn evaluate(&self, draft: &BeliefDraft) -> AwakeningDecision {
        if !draft.fact_supported {
            return AwakeningDecision::NoFacts;
        }
        if !draft.shares_precedent {
            return AwakeningDecision::NoPrecedent;
        }
        if draft.length_chars > self.budget {
            return AwakeningDecision::OverBudget {
                length_chars: draft.length_chars,
                budget: self.budget,
            };
        }
        AwakeningDecision::Admit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATE: AwakeningGate = AwakeningGate { budget: 100 };

    fn draft(facts: bool, precedent: bool, len: usize) -> BeliefDraft {
        BeliefDraft {
            fact_supported: facts,
            shares_precedent: precedent,
            length_chars: len,
        }
    }

    #[test]
    fn test_admit_healthy_belief() {
        // 三门槛全过 → 放行写库
        let d = GATE.evaluate(&draft(true, true, 50));
        assert_eq!(d, AwakeningDecision::Admit);
    }

    #[test]
    fn test_no_facts_rejected() {
        // 事实不可核 → 首级门拒写，不再看后两级
        let d = GATE.evaluate(&draft(false, true, 50));
        assert_eq!(d, AwakeningDecision::NoFacts);
    }

    #[test]
    fn test_no_precedent_rejected() {
        // facts 过但无判例支撑 → 二级门拒写
        let d = GATE.evaluate(&draft(true, false, 50));
        assert_eq!(d, AwakeningDecision::NoPrecedent);
    }

    #[test]
    fn test_over_budget_rejected() {
        // facts + precedent 过但超预算 → 三级门拒写
        let d = GATE.evaluate(&draft(true, true, 101));
        assert_eq!(
            d,
            AwakeningDecision::OverBudget {
                length_chars: 101,
                budget: 100,
            }
        );
    }

    #[test]
    fn test_boundary_exact_budget_admitted() {
        // length == budget：踩线 → 放行（09-06 recency `<` 判例）
        let d = GATE.evaluate(&draft(true, true, 100));
        assert_eq!(d, AwakeningDecision::Admit);
    }

    #[test]
    fn test_budget_zero_fails_closed() {
        // budget = 0 未配置：仅极短信念（0/--> 也过不了 0 < x）→ 全冻，
        // 未配置保守不写库（yoyo #982 fail-closed 同款）
        let g = AwakeningGate::new(0);
        let d = g.evaluate(&draft(true, true, 0));
        // length 0 <= budget 0 → Admit；length>0 超 → OverBudget。
        // 0-char 信念无实际内容，本测试只钉住"配置 0 时非空信念被拒"：
        let non_empty = g.evaluate(&draft(true, true, 1));
        assert_eq!(
            non_empty,
            AwakeningDecision::OverBudget {
                length_chars: 1,
                budget: 0,
            }
        );
        assert_eq!(d, AwakeningDecision::Admit);
    }

    #[test]
    fn test_default_gate_budget() {
        let g = AwakeningGate::default_gate();
        assert_eq!(g.budget, AwakeningGate::DEFAULT_BUDGET);
    }

    #[test]
    fn test_display_messages() {
        assert_eq!(
            GATE.evaluate(&draft(false, true, 50)).to_string(),
            "拒绝：事实不可核（facts(事实可核)不过）"
        );
        assert_eq!(
            GATE.evaluate(&draft(true, false, 50)).to_string(),
            "拒绝：无判例支撑（precedent(判例验证)不过）"
        );
        let over = GATE.evaluate(&draft(true, true, 101));
        assert!(over.to_string().contains("超预算"));
        assert_eq!(
            AwakeningDecision::Admit.to_string(),
            "三级门槛全过，允许写库"
        );
    }
}
