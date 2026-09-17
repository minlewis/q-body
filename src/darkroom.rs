//! Dark room ranking — 按"暗度"给 src/ 模块排序，回灌优先攻最暗模块。
//!
//! 借鉴来源：yologdev/yoyo-evolve — Blind round + dark room 排名。
//! yoyo 用 stale snapshots 给模块打暗度分（0.9 = #1 dark room），进化循环
//! 优先攻最暗模块。q-body 对应：Cron D 每日微回灌目前按 backlog 顺序线性
//! 挑任务，无模块级暗度信号；本模块让"下一步攻哪"由可量化暗度决定。
//!
//! Standalone 纯函数模块（先例：learnings.rs / clamp.rs / eval.rs）：
//! 不接线 handler，等依赖模块（覆盖率/盲测数据源）合入 main 后一行接线。
//!
//! 暗度 = 三维加权（归一化到 0.0-1.0，越高越暗）：
//! - coverage       测试覆盖：越低越暗（无覆盖 = 1.0，全覆盖 = 0.0）
//! - change_freq    改动频率：越高越暗（归一化到 max 频率）
//! - days_since_probe 距上次盲测天数：越久越暗（cap 到 30 天封顶）

/// 单个模块的暗度输入
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDarkness {
    pub module: String,
    /// 测试覆盖率 0.0-1.0（未知覆盖 = 0.0，即最暗假设）
    pub coverage: f64,
    /// 近 30 天改动次数（commits touching the module）
    pub change_freq: u32,
    /// 距上次盲测（blind probe / 显式测试）天数；从未盲测 = 30（封顶）
    pub days_since_probe: u32,
}

/// 权重（与 issue #102 提案一致，可按 evo 结论再调）
pub const WEIGHT_COVERAGE: f64 = 0.5;
pub const WEIGHT_CHANGE_FREQ: f64 = 0.3;
pub const WEIGHT_PROBE_STALENESS: f64 = 0.2;

/// 改动频率归一化上限：30 天内 10 次改动即视为最高频
pub const CHANGE_FREQ_CAP: u32 = 10;
/// 盲测陈旧度封顶天数
pub const PROBE_STALENESS_CAP: u32 = 30;

impl ModuleDarkness {
    /// 归一化暗度分 0.0-1.0（越高越暗）。
    ///
    /// score = 0.5 * (1 - coverage)
    ///       + 0.3 * min(change_freq / 10, 1)
    ///       + 0.2 * min(days_since_probe / 30, 1)
    pub fn darkness_score(&self) -> f64 {
        let coverage_dark = (1.0 - self.coverage.clamp(0.0, 1.0)).max(0.0);
        let freq_dark = (self.change_freq as f64 / CHANGE_FREQ_CAP as f64).min(1.0);
        let stale_dark = (self.days_since_probe as f64 / PROBE_STALENESS_CAP as f64).min(1.0);

        WEIGHT_COVERAGE * coverage_dark
            + WEIGHT_CHANGE_FREQ * freq_dark
            + WEIGHT_PROBE_STALENESS * stale_dark
    }
}

/// Dark room 排行：按暗度分降序（#1 = 最暗模块）。
///
/// 并列时按模块名字典序稳定排序，保证同一输入产出同一排名。
pub fn rank(modules: &[ModuleDarkness]) -> Vec<(usize, &ModuleDarkness, f64)> {
    let mut indexed: Vec<(usize, &ModuleDarkness, f64)> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| (i, m, m.darkness_score()))
        .collect();
    // 降序按分数；并列时按 (原索引, 模块名) 稳定排序
    indexed.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    indexed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(module: &str, coverage: f64, change_freq: u32, days_since_probe: u32) -> ModuleDarkness {
        ModuleDarkness {
            module: module.to_string(),
            coverage,
            change_freq,
            days_since_probe,
        }
    }

    #[test]
    fn test_score_fully_covered_fresh_module_is_bright() {
        // 全覆盖 + 低频改动 + 刚盲测过 → 最亮（接近 0）
        let m = m("validator", 1.0, 0, 0);
        assert!(m.darkness_score() < 0.05, "score = {}", m.darkness_score());
    }

    #[test]
    fn test_score_uncovered_high_churn_never_probed_is_darkest() {
        // 零覆盖 + 高频改动 + 从未盲测 → 最暗（= 1.0）
        let m = m("handler", 0.0, 10, 30);
        assert!((m.darkness_score() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_score_weights_match_issue_102() {
        // 手工验算：0.5*(1-0.2) + 0.3*(5/10) + 0.2*(10/30)
        let m = m("a2a", 0.2, 5, 10);
        let expected = 0.5 * 0.8 + 0.3 * 0.5 + 0.2 * (10.0 / 30.0);
        assert!((m.darkness_score() - expected).abs() < 1e-9);
    }

    #[test]
    fn test_score_clamps_out_of_range_coverage() {
        // 覆盖率超界应被 clamp，不 panic、不越界
        assert!((m("x", 1.5, 0, 0).darkness_score() - 0.0).abs() < 1e-9);
        assert!((m("y", -0.5, 0, 0).darkness_score() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_score_caps_churn_and_staleness() {
        // 超过 cap 的改动频率/陈旧度封顶，不无限加分
        let capped = m("x", 0.0, 100, 300).darkness_score();
        let at_cap = m("x", 0.0, 10, 30).darkness_score();
        assert!((capped - at_cap).abs() < 1e-9);
        assert!((capped - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_rank_orders_descending_and_stable() {
        let modules = vec![
            m("bright", 1.0, 0, 0),    // score ~0.0
            m("darkest", 0.0, 10, 30), // score 1.0
            m("mid", 0.5, 5, 15),      // score 0.5*0.5+0.3*0.5+0.2*0.5 = 0.5
        ];
        let ranked = rank(&modules);
        assert_eq!(ranked[0].1.module, "darkest");
        assert_eq!(ranked[1].1.module, "mid");
        assert_eq!(ranked[2].1.module, "bright");
        assert_eq!(ranked[0].0, 1); // 原索引保留
    }

    #[test]
    fn test_rank_tie_breaks_deterministically_by_original_order() {
        // 同分模块按原索引序稳定排名，同一输入同一输出
        let modules = vec![m("first", 0.0, 0, 0), m("second", 0.0, 0, 0)];
        let ranked = rank(&modules);
        assert_eq!(ranked[0].1.module, "first");
        assert_eq!(ranked[1].1.module, "second");
        let ranked2 = rank(&modules);
        assert_eq!(ranked[0].1.module, ranked2[0].1.module);
    }

    #[test]
    fn test_current_darkroom_is_handler_shape() {
        // 形状测试：handler.rs（改动最频繁、无单测）应压过 validator（有单测）
        let modules = vec![
            m("validator/command.rs", 0.8, 3, 5),
            m("handler.rs", 0.1, 8, 20),
        ];
        let ranked = rank(&modules);
        assert_eq!(ranked[0].1.module, "handler.rs");
    }
}
