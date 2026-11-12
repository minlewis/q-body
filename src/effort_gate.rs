//! Effort gate: 动工前成本评估门 (2026-11-12 backlog P0)
//!
//! 借鉴 Q00/ouroboros — budgeted evolution loop（先评估再执行）
//! → q-body 回灌闭环补执行前预算门：动工前先估改动规模/复杂度，
//!    预估超阈值即退回 backlog 不动手，杜绝「小目标也能码越改越大」
//!    （scope creep，同 yoyo #982 fail-closed 语义）。
//!
//! 语义（对齐 09-06 recency `<` 判例：等于上限不算超限）：
//!   est_lines <= max_lines          → WithinBudget（放行动工）
//!   est_lines >  max_lines          → OverBudget（退回，估错规模）
//!   未给预估 (None) / max=0         → fail-closed 按超限（未测量即不得宣称可做）

/// 默认单条改动行数上限，对齐 backlog「挑 <50 行改动的条目」slot rule。
pub const DEFAULT_MAX_LINES: usize = 50;

/// 未给预估时的保守标记（fail-closed：没评估就当超限）。
pub const UNSPECIFIED_OVER: u64 = u64::MAX;

/// 判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortVerdict {
    /// 预估在预算内，可以动工。
    WithinBudget,
    /// 预估超预算，退回 backlog 不动手。
    OverBudget { est_lines: u64, max_lines: usize },
}

/// 预算门（不可变配置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffortGate {
    /// 行数上限；0 = 保守全冻（fail-closed）。
    pub max_lines: usize,
}

impl EffortGate {
    /// 新建门。`max_lines == 0` 视为「全冻」（未配置 = fail-closed 保守按冻）。
    pub fn new(max_lines: usize) -> Self {
        Self { max_lines }
    }

    /// 默认门（上限 = DEFAULT_MAX_LINES）。
    pub fn default_gate() -> Self {
        Self::new(DEFAULT_MAX_LINES)
    }

    /// 判定决策：预估改动行数是否在预算内。
    ///
    /// - `est_lines = Some(n)`：`n <= max_lines` 放行，否则退回。
    /// - `est_lines = None`：未给出预估 → 保守按超限退回（fail-closed）。
    /// - `max_lines == 0`：未配置上限 → 全冻（fail-closed）。
    pub fn should_proceed(&self, est_lines: Option<u64>) -> EffortVerdict {
        if self.max_lines == 0 {
            // 未配置 = 没评估过许可范围，保守冻结（同 merge_budget max=0 按冻判例）。
            return EffortVerdict::OverBudget {
                est_lines: est_lines.unwrap_or(UNSPECIFIED_OVER),
                max_lines: 0,
            };
        }
        let est = est_lines.unwrap_or(UNSPECIFIED_OVER);
        if est <= self.max_lines as u64 {
            EffortVerdict::WithinBudget
        } else {
            EffortVerdict::OverBudget {
                est_lines: est,
                max_lines: self.max_lines,
            }
        }
    }

    /// 便捷谓词：是否可以动工。
    pub fn is_approved(&self, est_lines: Option<u64>) -> bool {
        matches!(self.should_proceed(est_lines), EffortVerdict::WithinBudget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_estimate_within_budget() {
        let gate = EffortGate::default_gate();
        assert_eq!(gate.should_proceed(Some(20)), EffortVerdict::WithinBudget);
        assert!(gate.is_approved(Some(20)));
    }

    #[test]
    fn over_budget_returns_overbudget() {
        let gate = EffortGate::default_gate();
        assert_eq!(
            gate.should_proceed(Some(300)),
            EffortVerdict::OverBudget {
                est_lines: 300,
                max_lines: 50
            }
        );
        assert!(!gate.is_approved(Some(300)));
    }

    #[test]
    fn boundary_equal_to_max_is_within() {
        // 对齐 09-06 recency `<` 判例：等于上限不算超限（"超过 N 行"才退回）。
        let gate = EffortGate::new(50);
        assert_eq!(gate.should_proceed(Some(50)), EffortVerdict::WithinBudget);
    }

    #[test]
    fn unspecified_estimate_fails_closed() {
        // 未给预估 = 没评估过 → 保守按超限退回（yoyo #982 fail-closed）。
        let gate = EffortGate::default_gate();
        assert_eq!(
            gate.should_proceed(None),
            EffortVerdict::OverBudget {
                est_lines: UNSPECIFIED_OVER,
                max_lines: 50
            }
        );
        assert!(!gate.is_approved(None));
    }

    #[test]
    fn unconfigured_gate_fails_closed_all_frozen() {
        // max=0 = 未配置 → 全冻（fail-closed）。
        let gate = EffortGate::new(0);
        assert_eq!(
            gate.should_proceed(Some(10)),
            EffortVerdict::OverBudget {
                est_lines: 10,
                max_lines: 0
            }
        );
        assert!(!gate.is_approved(Some(10)));
    }

    #[test]
    fn explicit_custom_limit_respected() {
        let gate = EffortGate::new(120);
        assert!(gate.is_approved(Some(120)));
        assert!(!gate.is_approved(Some(121)));
    }
}
