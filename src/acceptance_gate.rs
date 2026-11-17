//! 行为级验收门 — 借鉴 Q00/ouroboros「interview-gated + 预算化进化」
//!
//! backlog 2026-11-17 — P0「evolution 验收门」：
//! 除 cron D 现有「P0 锚点 → issue → cargo check」外，新行为落地前还须过一条
//! 「损掉了什么」行为自证（对齐 TAO 三问第三问），并给单周期自进化的改动量
//! 设预算上限，防扩张跑赢宪法追认（对应 SOUL §28 击穿即冻结）。
//!
//! 为什么需要这个：操作类 cron（Cron D/B）每晚跑 cargo check 证明「编译通过」，
//! 但「编译通过」只证明能 build，不证明扩张是克制的——v0.2.0 的 +374 行、
//! handler.rs 808→873 越过单文件硬线，都是在 cargo check 全绿的机械节奏里发生的。
//! 本 gate 把「三问第三问（损掉了什么）」从 prose 承诺升级为机检谓词：
//! 每条新行为落地前必须声明「这次损掉了什么」+ 预估改动行数，
//! 两者缺一或超预算即被 gate 退回，扩张必须与宪法追认同速。
//!
//! 借鉴：Q00/ouroboros — interview-gated, staged evaluation, budgeted evolution loop
//! → q-body activation 加 budgeted gate（同 awakening_gate 同源思路）。

use std::fmt;

/// 「这次损掉了什么」字段——TAO 三问第三问的机检载体
pub const FIELD_LOST_WHAT: &str = "lost_what";

/// 单周期改动量预算默认值（行为自证所需的最小微回灌改动量）
pub const DEFAULT_CHANGE_BUDGET: usize = 50;

/// 一次新行为的验收申报
///
/// 从行为描述中提取：`lost_what`（「这次损掉了什么」自证，非空）、
/// `change_lines`（单周期预估改动行数，通常来自 `git diff --stat` 净改动）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceProof {
    /// 三问第三问自证——这次行为损掉了什么（兼容性/可维护性/预算/边界）
    pub lost_what: String,
    /// 单周期预估改动行数（净行数）
    pub change_lines: usize,
}

/// 验收闸门判定
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceVerdict {
    /// 自证齐全且改动量在预算内 — 放行落地
    Admit,
    /// 「损掉了什么」缺失或为空 — 退回（三问第三问不答即不放行）
    NoLostWhat,
    /// 自证齐全但改动量超单周期预算 — 冻结
    OverBudget { change_lines: usize, budget: usize },
    /// 申报缺失（未给 lost_what 与 change_lines）— fail-closed 保守退回
    MissingProof,
}

/// 申报缺失/非法时返回的错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofError {
    /// lost_what 字段缺失
    MissingLostWhat,
    /// lost_what 字段存在但为空/纯空白
    EmptyLostWhat,
    /// change_lines 字段缺失
    MissingChangeLines,
    /// change_lines 解析失败（非非负整数）
    InvalidChangeLines { detail: String },
}

impl fmt::Display for ProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProofError::MissingLostWhat => {
                write!(f, "缺少 `{FIELD_LOST_WHAT}` 自证字段，验收 gate 退回")
            }
            ProofError::EmptyLostWhat => {
                write!(f, "`{FIELD_LOST_WHAT}` 为空，三问第三问不答即不放行")
            }
            ProofError::MissingChangeLines => {
                write!(f, "缺少 `change_lines` 字段，单周期改动量须申报")
            }
            ProofError::InvalidChangeLines { detail } => {
                write!(f, "`change_lines` 值非法：{detail}")
            }
        }
    }
}

/// 行为级验收闸门
///
/// `change_budget`：单周期改动行数上限（默认 `DEFAULT_CHANGE_BUDGET`；
/// 沙漏对齐 backlog slot rule「<50 行」）。
#[derive(Debug, Clone)]
pub struct AcceptanceGate {
    pub change_budget: usize,
}

impl Default for AcceptanceGate {
    fn default() -> Self {
        Self {
            change_budget: DEFAULT_CHANGE_BUDGET,
        }
    }
}

impl AcceptanceGate {
    pub fn new(change_budget: usize) -> Self {
        Self { change_budget }
    }

    /// 主入口：从行为描述 JSON 提取申报并做验收判定。
    ///
    /// `MissingProof` / `NoLostWhat` 直接拒绝；申报齐全时进预算判定（`evaluate`）。
    pub fn evaluate_json(&self, proof_json: &str) -> Result<AcceptanceVerdict, ProofError> {
        let value: serde_json::Value = serde_json::from_str(proof_json).map_err(|e| {
            ProofError::InvalidChangeLines {
                detail: format!("行为描述不是合法 JSON：{e}"),
            }
        })?;
        let proof = Self::parse_proof(&value)?;
        Ok(self.evaluate(proof))
    }

    /// 验收判定：先自证后预算。
    ///
    /// lost_what 为空 → NoLostWhat；change_lines > budget → OverBudget；
    /// 两者都过 → Admit。
    ///
    /// budget 语义沿用 09-06 recency `<` 判例：`<=` 放行，超过才退回
    /// （预支 vs 已用满：此处「未超预算」即放行，等于预算不算超限）。
    pub fn evaluate(&self, proof: AcceptanceProof) -> AcceptanceVerdict {
        if proof.lost_what.trim().is_empty() {
            return AcceptanceVerdict::NoLostWhat;
        }
        if proof.change_lines > self.change_budget {
            return AcceptanceVerdict::OverBudget {
                change_lines: proof.change_lines,
                budget: self.change_budget,
            };
        }
        AcceptanceVerdict::Admit
    }

    /// 从行为描述 JSON 提取验收申报，缺字段/非法值精确报错
    pub fn parse_proof(value: &serde_json::Value) -> Result<AcceptanceProof, ProofError> {
        let lost_what = value
            .get(FIELD_LOST_WHAT)
            .ok_or(ProofError::MissingLostWhat)?
            .as_str()
            .ok_or_else(|| ProofError::EmptyLostWhat)?
            .to_string();
        if lost_what.trim().is_empty() {
            return Err(ProofError::EmptyLostWhat);
        }
        let change_lines = value
            .get("change_lines")
            .ok_or(ProofError::MissingChangeLines)?
            .as_u64()
            .ok_or_else(|| ProofError::InvalidChangeLines {
                detail: "期望非负整数".to_string(),
            })?;
        Ok(AcceptanceProof {
            lost_what,
            change_lines: change_lines as usize,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATE: AcceptanceGate = AcceptanceGate {
        change_budget: DEFAULT_CHANGE_BUDGET,
    };

    fn proof(lost: &str, lines: &str) -> String {
        format!(
            r#"{{"action":"refactor","{FIELD_LOST_WHAT}":"{lost}","change_lines":{lines}}}"#
        )
    }

    #[test]
    fn test_admit_healthy_behavior() {
        let d = GATE
            .evaluate_json(&proof("handler.rs 增加 2 行 import，无行为变化", "5"))
            .expect("自证齐全应解析成功");
        assert_eq!(d, AcceptanceVerdict::Admit);
    }

    #[test]
    fn test_missing_lost_what() {
        let err = GATE
            .evaluate_json(r#"{"action":"refactor","change_lines":10}"#)
            .unwrap_err();
        assert_eq!(err, ProofError::MissingLostWhat);
    }

    #[test]
    fn test_empty_lost_what_rejected() {
        let err = GATE
            .evaluate_json(&proof("", "10"))
            .unwrap_err();
        assert_eq!(err, ProofError::EmptyLostWhat);
        // 纯空白同样拒绝
        let err2 = GATE
            .evaluate_json(&proof("   ", "10"))
            .unwrap_err();
        assert_eq!(err2, ProofError::EmptyLostWhat);
    }

    #[test]
    fn test_missing_change_lines() {
        let err = GATE
            .evaluate_json(&format!(r#"{{"{FIELD_LOST_WHAT}":"损了一点"}}"#))
            .unwrap_err();
        assert_eq!(err, ProofError::MissingChangeLines);
    }

    #[test]
    fn test_over_budget_frozen() {
        let d = GATE
            .evaluate_json(&proof("拆 journal.rs +60", "1600"))
            .expect("自证齐全应解析成功");
        assert_eq!(
            d,
            AcceptanceVerdict::OverBudget {
                change_lines: 1600,
                budget: DEFAULT_CHANGE_BUDGET
            }
        );
    }

    #[test]
    fn test_boundary_exact_budget_admitted() {
        // change_lines == budget：踩线 → 仍放行（recency `<` 判例）
        let d = GATE
            .evaluate_json(&proof("边界微调", "50"))
            .expect("自证齐全应解析成功");
        assert_eq!(d, AcceptanceVerdict::Admit);
    }

    #[test]
    fn test_missing_proof_json_invalid() {
        let err = GATE.evaluate_json("not json").unwrap_err();
        assert!(err.to_string().contains("不是合法 JSON"));
    }

    #[test]
    fn test_zero_change_lines_admitted_with_proof() {
        // 零改动但自证非空 → 放行（0 不算超预算）
        let d = GATE
            .evaluate_json(&proof("纯文档注释，无代码行为变化", "0"))
            .expect("自证齐全应解析成功");
        assert_eq!(d, AcceptanceVerdict::Admit);
    }

    #[test]
    fn test_custom_budget_respected() {
        let gate = AcceptanceGate::new(200);
        let d = gate
            .evaluate(AcceptanceProof {
                lost_what: "精简日志冗余".to_string(),
                change_lines: 150,
            });
        assert_eq!(d, AcceptanceVerdict::Admit);
        let d2 = gate
            .evaluate(AcceptanceProof {
                lost_what: "大改 handler 消息组装".to_string(),
                change_lines: 250,
            });
        assert_eq!(
            d2,
            AcceptanceVerdict::OverBudget {
                change_lines: 250,
                budget: 200
            }
        );
    }

    #[test]
    fn test_fail_closed_missing_proof() {
        // 未提供自证（构造空 JSON）→ 拒绝而非放行（yoyo #982 fail-closed 同款）
        let d = GATE
            .evaluate(AcceptanceProof {
                lost_what: String::new(),
                change_lines: 0,
            });
        assert_eq!(d, AcceptanceVerdict::NoLostWhat);
    }
}