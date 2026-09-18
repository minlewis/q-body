//! gate 谓词漂移审计 — 借鉴 yolo-evolve Day 198
//!
//! backlog 2026-09-14 — P0：grep 全部 gate/判断函数，核对每个判定谓词与
//! 它声称回答的问题是否同一事实（yoyo 式漂移 = gate 键 A、问题键 B），
//! 并为每个 gate 补一条「no 输入必须被尊重」的负路径单测。
//!
//! 本模块不复制生产代码——直接 `use` 各 gate 的真实实现来跑审计用例，
//! 审计对象换了实现（谓词漂移）这些测试就会红，这正是审计的本意。
//!
//! 审计结论（逐 gate 核对，谓词体实读）：
//!
//! 1. `EvolutionGate::evaluate`（src/evolution_gate.rs）
//!    声称回答：提案是否划算（Admit/OverBudget/Uneconomical）。
//!    谓词事实：`est_tokens > token_budget` / `est_value < value_floor`，
//!    与文档语义逐字对齐——**无漂移**。负路径钉死见 `audit_no_token_zero_estimate_is_rejected`。
//!
//! 2. `RecencyGuard::should_fallback_to_date_scan`（src/validator/recency.rs）
//!    声称回答：事件率是否低到固定 LIMIT 会退化成全量归档。
//!    谓词事实：`current_rate < threshold`（09-06 修复后为严格小于），
//!    文档注释同一表述——**无漂移**。负路径钉死见
//!    `audit_no_healthy_rate_must_not_fallback`：声称回答"何时放弃
//!    recency query"的 gate，在事件率健康时必须说 no，fallback 不得发生。
//!
//! 3. `scan_rm_command` / `is_var_set`（src/validator/command.rs）
//!    声称回答：rm -rf 的变量是否已定义且非空（yoyo D144 `rm -rf $VAR`
//!    空变量退化事故）。
//!    谓词事实：`env::var(name).map(|v| !v.is_empty())` 同时核对
//!    「已定义」与「非空」两个事实，语义正确——**无漂移**。负路径钉死见
//!    `audit_no_unset_variable_must_be_respected`：变量不存在时 gate 说 no，
//!    扫描必须拒绝，不得当作已解析放行。
//!
//! 4. `trust_input`（`src/trust_input.rs`，196 行，**已在 main**）
//!    重点排查对象（本条目原文点名）。逐谓词核对：
//!    - `audit_injection` 谓词 `new_hash == prev_hash → None`：声称回答
//!      "内容是否变化"，核对的是同一内容的同一 FNV 指纹——键一致，**无漂移**。
//!    - `within_budget` 谓词 `lines <= TAO_MAX_LINES`：声称回答"是否在
//!      TAO ≤3000 行预算内"，边界语义与 TAO 宪法"≤"一致——**无漂移**。
//!    - 风险点（非漂移，记录在案）：`budget_ok=false` 只落账不拦截是
//!      有意设计（P0=journal 记忆优先，同 cost_warn 哲学），不是
//!      "ignores a no"——no 被记录为事实而非被吞掉。
//!
//!    订正（2026-09-18）：本条目原文称该分支「未合 main」，与事实不符——
//!    `src/trust_input.rs` 已在 main 上（196 行），但全仓零引用，
//!    见其 `DEBT(unwired)` 台账。上述人工核对结论不变；
//!    补对应用例待该模块真正接线后进行。

#[cfg(test)]
mod audit_tests {
    use crate::evolution_gate::{CostEstimate, EvolutionGate, GateDecision};
    use crate::validator::command::scan_rm_command;
    use crate::validator::recency::RecencyGuard;

    /// 审计 1 — evolution_gate：零 token 申报（gate 明确说 no 的输入）必须被拒绝。
    /// 若谓词漂移成 `>=` 或漏掉 `*n > 0` 校验，此测即红。
    #[test]
    fn audit_no_zero_token_estimate_is_rejected() {
        let gate = EvolutionGate::new(10_000, 0.5);
        let d = gate.evaluate(CostEstimate {
            est_tokens: 0,
            est_value: 999.0,
        });
        assert_ne!(d, GateDecision::Admit, "0 token 申报必须被 gate 拒绝");
    }

    /// 审计 1b — evolution_gate 负路径：价值低于下限（no）必须被尊重为 Uneconomical。
    #[test]
    fn audit_no_uneconomical_estimate_is_rejected() {
        let gate = EvolutionGate::new(10_000, 0.5);
        let d = gate.evaluate(CostEstimate {
            est_tokens: 100,
            est_value: 0.4,
        });
        assert_ne!(d, GateDecision::Admit, "低于 value_floor 的提案必须被拒绝");
    }

    /// 审计 2 — recency gate 负路径：事件率健康（rate >= threshold）时
    /// gate 必须说 no（不 fallback）。这是本 gate 存在的全部意义：
    /// 低事件率才放弃 recency query，健康时放弃即谓词漂移。
    #[test]
    fn audit_no_healthy_rate_must_not_fallback() {
        let guard = RecencyGuard::with_threshold(5.0);
        assert!(
            !guard.should_fallback_to_date_scan(5.0),
            "rate == threshold：严格 < 谓词必须说不"
        );
        assert!(
            !guard.should_fallback_to_date_scan(50.0),
            "rate 远高于 threshold：gate 必须说不，不得 fallback"
        );
        assert_eq!(
            guard.select_query_strategy(50.0),
            crate::validator::recency::RecencyQuery::FixedLimit { limit: 100 },
            "健康事件率必须走 FixedLimit recency query"
        );
    }

    /// 审计 3 — command validator 负路径：`rm -rf $UNSET_VAR` 中变量不存在
    /// （env 无此键）时 gate 说 no，扫描必须以 UnresolvedVariable 拒绝。
    /// 若谓词漂移成"找不到变量就当已定义"或漏判空串，此测即红。
    #[test]
    fn audit_no_unset_variable_must_be_respected() {
        // 用一个不可能在环境中存在的变量名（含非法字符，env 键不允许）
        let var = "Q_BODY_AUDIT_UNSET_9F3E";
        // 确保确实未设置（测试自证前提，不依赖环境巧合）
        // edition 2024 起 env 变异为 unsafe；测试进程单线程使用这些键，安全
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(var);
        }
        let err = scan_rm_command(&format!("rm -rf ${var}/data"));
        match err {
            Err(crate::validator::command::SafetyError::UnresolvedVariable {
                var_name, ..
            }) => {
                assert_eq!(var_name, var, "拒绝时报告的变量名必须与实际未设置变量一致");
            }
            other => panic!("未设置变量必须被拒绝（UnresolvedVariable），实际：{other:?}"),
        }
    }

    /// 审计 3b — 空字符串变量等同未设置（is_var_set 的「非空」半边谓词）。
    #[test]
    fn audit_no_empty_variable_must_be_respected() {
        let var = "Q_BODY_AUDIT_EMPTY_9F3E";
        // 保存并恢复环境，避免污染并行/后续测试
        let saved = std::env::var(var).ok();
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(var, "");
        }
        let err = scan_rm_command(&format!("rm -rf ${{{var}}}/data"));
        if let Err(e) = &err {
            // 期望路径：空值按未解析拒绝
            assert!(matches!(
                e,
                crate::validator::command::SafetyError::UnresolvedVariable { .. }
            ));
        } else {
            panic!("空变量必须按未解析拒绝，实际 Ok(())");
        }
        // 恢复
        #[allow(unused_unsafe)]
        unsafe {
            match saved {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
    }
}
