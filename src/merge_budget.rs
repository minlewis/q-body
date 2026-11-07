//! 功能合批预算闸 —— 单周预算上限 N 条，超限冻结等下周期。
//!
//! 借鉴：Q00/ouroboros — budgeted evolution loop（预算制进化循环：节奏固定，池子有上限）。
//! 借鉴锚点：backlog 2026-11-07 P0「q-body 合批门禁」——把 TAO「克制扩展」从三问质询
//! 升级为可量化的执行闸，命中 §28 击穿即冻结语义。
//!
//! 设计语义：
//! - 每条功能合批 PR 是一次「扩张」。克制扩展 = 每周合批数量受限。
//! - `MergeBudget` 记一个乘数 `max_per_week` 与已记账的条目；查超限时返回冻结状态。
//! - 冻结是 gate 语义：超限即「等待下一周期」，不自动放行、不静默吞。
//! - 预算账本可序列化往返，供 cron/对镜在周界重置点消费（接线点落地后一行接）。
//!
//! 依赖零新增；纯函数，无 IO，可测试。

/// 默认每周合批预算上限：N 条功能性 PR。
pub const DEFAULT_MAX_MERGES_PER_WEEK: u32 = 5;

/// 预算闸判定结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BudgetVerdict {
    /// 在预算内，可合批。
    WithinBudget { used: u32, budget: u32 },
    /// 已超限，冻结当前周期。
    Frozen { used: u32, budget: u32 },
}

impl BudgetVerdict {
    /// 是否放行（在预算内才放行）。
    pub fn is_approved(&self) -> bool {
        matches!(self, BudgetVerdict::WithinBudget { .. })
    }
}

/// 单周功能合批预算账本（不可变视图，账本累计由接线方写回）。
///
/// 等于上限不算超限（09-06 recency `<` 判例：`<` 才是超限触发）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeBudget {
    /// 本周已合批计数。
    pub merges_this_week: u32,
    /// 每周预算上限。
    pub max_per_week: u32,
}

impl MergeBudget {
    /// 构造账本；`max_per_week == 0` 视为「未配置」→ 保守按 0 冻结（fail-closed，对齐 yoyo #982）。
    pub fn new(merges_this_week: u32, max_per_week: u32) -> Self {
        Self {
            merges_this_week,
            max_per_week,
        }
    }

    /// 判定当前状态。等于上限不算超限（`<` 判例）。
    pub fn verdict(&self) -> BudgetVerdict {
        if self.merges_this_week < self.max_per_week {
            BudgetVerdict::WithinBudget {
                used: self.merges_this_week,
                budget: self.max_per_week,
            }
        } else {
            BudgetVerdict::Frozen {
                used: self.merges_this_week,
                budget: self.max_per_week,
            }
        }
    }

    /// 合批后记账（+1），返回新账本。是否放行由调用方先查 `verdict()`。
    pub fn record_merge(&self) -> Self {
        Self::new(self.merges_this_week + 1, self.max_per_week)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认预算为 5。
    #[test]
    fn default_budget_is_five() {
        assert_eq!(DEFAULT_MAX_MERGES_PER_WEEK, 5);
    }

    /// 初始零合批在预算内。
    #[test]
    fn fresh_budget_within() {
        let b = MergeBudget::new(0, 5);
        assert!(b.verdict().is_approved());
        assert_eq!(
            b.verdict(),
            BudgetVerdict::WithinBudget { used: 0, budget: 5 }
        );
    }

    /// 恰好用满预算：已到上限，下一次合批被冻结（gate 在合批前判定）。
    /// 这是 fail-closed：若 used==max 仍放行，第 N+1 条扩张就会悄悄超线（§28 击穿）。
    #[test]
    fn exact_budget_freezes_next() {
        let b = MergeBudget::new(5, 5);
        assert!(!b.verdict().is_approved());
        assert_eq!(b.verdict(), BudgetVerdict::Frozen { used: 5, budget: 5 });
    }

    /// 超过上限即冻结。
    #[test]
    fn over_budget_freezes() {
        let b = MergeBudget::new(6, 5);
        assert!(!b.verdict().is_approved());
        assert_eq!(b.verdict(), BudgetVerdict::Frozen { used: 6, budget: 5 });
    }

    /// 未配置（0 上限）保守按冻结，fail-closed。
    #[test]
    fn unconfigured_budget_freezes() {
        let b = MergeBudget::new(1, 0);
        assert!(!b.verdict().is_approved());
        assert_eq!(b.verdict(), BudgetVerdict::Frozen { used: 1, budget: 0 });
    }

    /// 记账 +1 推进账本。
    #[test]
    fn record_merge_increments() {
        let b = MergeBudget::new(4, 5);
        let next = b.record_merge();
        assert_eq!(next.merges_this_week, 5);
        // 记到上限后即冻结：下一次合批被 gate 拦（fail-closed，§28）。
        assert!(!next.verdict().is_approved());
    }

    /// 序列化往返保持状态（供 cron 账本持久化）。
    #[test]
    fn serde_roundtrip() {
        let b = MergeBudget::new(3, 5);
        let json = serde_json::to_string(&b).unwrap();
        let back: MergeBudget = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }
}