//! Evolution gate — cost-estimate 前置闸门
//!
//! 借鉴 Q00/ouroboros 的 staged evaluation budgeted loop 思路：
//! 评估循环分阶段推进，每个提案进入评估前必须自带成本预估，
//! gate 先按预算闸门筛掉不划算的提案——别把评估预算烧在注定被否的提案上。
//!
//! 为什么需要这个：LLM 演化提案如果不在入口做成本申报校验，
//! 评估循环会把 token 预算平均烧在所有提案上，包括那些
//! 申报缺失、数值非法、超预算、或预期价值低于成本的提案。
//! 前置 gate 让不划算的提案在进入评估循环之前就被拒之门外。
//!
//! 对应 backlog 2026-08-29 — P0（issue #96）。
//! 类型层准备；handler.rs 运行时接线按既定先例推迟。

use std::fmt;

/// 成本申报字段缺失或非法时使用的统一字段名常量
pub const FIELD_EST_TOKENS: &str = "est_tokens";
pub const FIELD_EST_VALUE: &str = "est_value";

/// 一次 LLM 演化提案的成本申报
///
/// 从提案 JSON 中提取：`est_tokens`（预估 token 成本，正整数）、
/// `est_value`（预估价值，非负浮点数，量纲与 value_floor 一致）。
#[derive(Debug, Clone, PartialEq)]
pub struct CostEstimate {
    /// 预估 token 成本（必须 > 0）
    pub est_tokens: u64,
    /// 预估价值（必须 >= 0；与 `EvolutionGate::value_floor` 同量纲）
    pub est_value: f64,
}

/// 前置闸门的经济学判定
#[derive(Debug, Clone, PartialEq)]
pub enum GateDecision {
    /// 申报齐全且划算 — 放行进入评估循环
    Admit,
    /// 申报齐全但 token 成本超过 gate 预算 — 拒绝
    OverBudget { est_tokens: u64, budget: u64 },
    /// 申报齐全但预期价值低于成本 — 拒绝
    Uneconomical { est_value: f64, est_tokens: u64 },
}

/// gate 拒绝原因
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    /// 输入不是合法 JSON
    MalformedJson { detail: String },
    /// 缺少成本申报字段
    MissingField { field: String },
    /// 字段存在但值非法（0 token / 负数价值 / 非数值类型）
    InvalidValue { field: String, detail: String },
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateError::MalformedJson { detail } => {
                write!(f, "提案不是合法 JSON，前置 gate 拒绝：{detail}")
            }
            GateError::MissingField { field } => {
                write!(
                    f,
                    "提案缺少成本申报字段 `{field}`，前置 gate 拒绝。\
                     申报必须含 `{FIELD_EST_TOKENS}`（正整数）与 `{FIELD_EST_VALUE}`（非负数）。"
                )
            }
            GateError::InvalidValue { field, detail } => {
                write!(f, "提案字段 `{field}` 值非法：{detail}")
            }
        }
    }
}

impl std::error::Error for GateError {}

/// cost-estimate 前置闸门
///
/// `token_budget`：单提案 token 成本上限；`value_floor`：预期价值下限
/// （与 est_value 同量纲，例如"每 token 的价值单价"或绝对价值分）。
#[derive(Debug, Clone)]
pub struct EvolutionGate {
    pub token_budget: u64,
    pub value_floor: f64,
}

impl EvolutionGate {
    pub fn new(token_budget: u64, value_floor: f64) -> Self {
        Self {
            token_budget,
            value_floor,
        }
    }

    /// 主入口：从提案 JSON 字符串提取申报并做前置判定。
    ///
    /// `MalformedJson` / `MissingField` / `InvalidValue` 三类直接拒绝；
    /// 申报齐全时进入经济学判定（`evaluate`）。
    pub fn evaluate_json(&self, proposal_json: &str) -> Result<GateDecision, GateError> {
        let value: serde_json::Value =
            serde_json::from_str(proposal_json).map_err(|e| GateError::MalformedJson {
                detail: e.to_string(),
            })?;
        let estimate = Self::parse_estimate(&value)?;
        Ok(self.evaluate(estimate))
    }

    /// 经济学判定：先预算后价值。
    ///
    /// est_tokens > token_budget → OverBudget；
    /// est_value < value_floor → Uneconomical；
    /// 两者都过 → Admit。
    pub fn evaluate(&self, estimate: CostEstimate) -> GateDecision {
        if estimate.est_tokens > self.token_budget {
            return GateDecision::OverBudget {
                est_tokens: estimate.est_tokens,
                budget: self.token_budget,
            };
        }
        if estimate.est_value < self.value_floor {
            return GateDecision::Uneconomical {
                est_value: estimate.est_value,
                est_tokens: estimate.est_tokens,
            };
        }
        GateDecision::Admit
    }

    /// 从提案 JSON 提取成本申报，缺字段/非法值精确报错
    pub fn parse_estimate(value: &serde_json::Value) -> Result<CostEstimate, GateError> {
        let est_tokens = Self::require_u64_field(value, FIELD_EST_TOKENS)?;
        let est_value = Self::require_f64_field(value, FIELD_EST_VALUE)?;
        Ok(CostEstimate {
            est_tokens,
            est_value,
        })
    }

    fn require_u64_field(value: &serde_json::Value, field: &str) -> Result<u64, GateError> {
        let v = value.get(field).ok_or_else(|| GateError::MissingField {
            field: field.to_string(),
        })?;
        v.as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| GateError::InvalidValue {
                field: field.to_string(),
                detail: format!("期望正整数，实际为 {v}"),
            })
    }

    fn require_f64_field(value: &serde_json::Value, field: &str) -> Result<f64, GateError> {
        let v = value.get(field).ok_or_else(|| GateError::MissingField {
            field: field.to_string(),
        })?;
        let n = v.as_f64().ok_or_else(|| GateError::InvalidValue {
            field: field.to_string(),
            detail: format!("期望数值，实际为 {v}"),
        })?;
        if !n.is_finite() || n < 0.0 {
            return Err(GateError::InvalidValue {
                field: field.to_string(),
                detail: format!("期望非负有限数值，实际为 {n}"),
            });
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATE: EvolutionGate = EvolutionGate {
        token_budget: 10_000,
        value_floor: 0.5,
    };

    fn proposal(tokens: &str, value: &str) -> String {
        format!(
            r#"{{"action":"refactor","{FIELD_EST_TOKENS}":{tokens},"{FIELD_EST_VALUE}":{value}}}"#
        )
    }

    #[test]
    fn test_admit_healthy_proposal() {
        let d = GATE
            .evaluate_json(&proposal("4000", "2.0"))
            .expect("申报齐全应解析成功");
        assert_eq!(d, GateDecision::Admit);
    }

    #[test]
    fn test_missing_tokens_field() {
        let json = r#"{"action":"refactor"}"#;
        let err = GATE.evaluate_json(json).unwrap_err();
        assert_eq!(
            err,
            GateError::MissingField {
                field: FIELD_EST_TOKENS.to_string()
            }
        );
    }

    #[test]
    fn test_missing_value_field() {
        let json = r#"{"est_tokens":100}"#;
        let err = GATE.evaluate_json(json).unwrap_err();
        assert_eq!(
            err,
            GateError::MissingField {
                field: FIELD_EST_VALUE.to_string()
            }
        );
    }

    #[test]
    fn test_invalid_zero_tokens() {
        let err = GATE.evaluate_json(&proposal("0", "1.0")).unwrap_err();
        assert!(matches!(err, GateError::InvalidValue { .. }));
    }

    #[test]
    fn test_invalid_negative_value() {
        let err = GATE.evaluate_json(&proposal("100", "-0.5")).unwrap_err();
        assert!(matches!(err, GateError::InvalidValue { .. }));
    }

    #[test]
    fn test_invalid_non_numeric_value() {
        let err = GATE
            .evaluate_json(&proposal("100", "\"high\""))
            .unwrap_err();
        assert!(matches!(err, GateError::InvalidValue { .. }));
    }

    #[test]
    fn test_malformed_json() {
        let err = GATE.evaluate_json("not json at all").unwrap_err();
        assert!(matches!(err, GateError::MalformedJson { .. }));
    }

    #[test]
    fn test_over_budget_rejected() {
        let d = GATE
            .evaluate_json(&proposal("20000", "9.0"))
            .expect("申报齐全应解析成功");
        assert_eq!(
            d,
            GateDecision::OverBudget {
                est_tokens: 20_000,
                budget: 10_000
            }
        );
    }

    #[test]
    fn test_uneconomical_rejected() {
        let d = GATE
            .evaluate_json(&proposal("100", "0.1"))
            .expect("申报齐全应解析成功");
        assert_eq!(
            d,
            GateDecision::Uneconomical {
                est_value: 0.1,
                est_tokens: 100
            }
        );
    }

    #[test]
    fn test_boundary_exact_budget_admitted() {
        // est_tokens == budget、est_value == floor：都踩线 → 仍放行
        let d = GATE
            .evaluate_json(&proposal("10000", "0.5"))
            .expect("申报齐全应解析成功");
        assert_eq!(d, GateDecision::Admit);
    }

    #[test]
    fn test_error_message_mentions_fields() {
        let err = GATE.evaluate_json(r#"{}"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(FIELD_EST_TOKENS));
        assert!(msg.contains(FIELD_EST_VALUE));
    }

    #[test]
    fn test_display_malformed_json() {
        let err = GATE.evaluate_json("{broken").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("前置 gate 拒绝"));
    }
}
