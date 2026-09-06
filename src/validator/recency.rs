//! Recency guard — 事件率下降时自动 fallback 到 date-range scan
//!
//! 借鉴 yologdev/yoyo-evolve Day 175 learning：
//! 「a fixed-count recency query becomes an all-time archive as the event rate falls」
//!
//! 问题：当事件率下降到 N/d 以下，`LIMIT 100` 的 recency query 会返回全部历史
//! （因为最近 100 条就是全部）。这导致"看到最近 100 条"实际上等于"看到全部"，
//! 判重逻辑失效，重复条目再次进入。
//!
//! 改法：根据当前事件率选择查询策略：
//! - 事件率 > 阈值 → FixedLimit（固定 LIMIT，高效）
//! - 事件率 ≤ 阈值 → DateRange（时间范围扫描，准确）

use chrono::{DateTime, Utc};

/// Recency guard 配置
#[derive(Debug, Clone, Copy)]
pub struct RecencyGuard {
    /// 事件率阈值（events/day），低于此值时 fallback 到 date-range scan
    pub events_per_day_threshold: f64,
}

impl Default for RecencyGuard {
    fn default() -> Self {
        Self {
            events_per_day_threshold: 5.0,
        }
    }
}

impl RecencyGuard {
    /// 使用默认配置创建 guard
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用自定义阈值创建 guard
    pub fn with_threshold(threshold: f64) -> Self {
        Self {
            events_per_day_threshold: threshold,
        }
    }

    /// 判断当前事件率是否需要 fallback 到 date-range scan
    ///
    /// 当 `current_rate`（events/day）< 阈值时返回 `true`，表示应使用 DateRange 策略。
    /// 等于阈值不触发（backlog 语义：「下降到 N/d **以下**」才 fallback）。
    pub fn should_fallback_to_date_scan(&self, current_rate: f64) -> bool {
        current_rate < self.events_per_day_threshold
    }

    /// 根据当前事件率选择查询策略
    pub fn select_query_strategy(&self, current_rate: f64) -> RecencyQuery {
        if self.should_fallback_to_date_scan(current_rate) {
            // 事件率低 → 用 date-range scan 确保准确
            RecencyQuery::DateRange {
                since: Utc::now() - chrono::Duration::days(7),
            }
        } else {
            // 事件率高 → 固定 LIMIT 高效
            RecencyQuery::FixedLimit { limit: 100 }
        }
    }
}

/// 查询策略
#[derive(Debug, Clone, PartialEq)]
pub enum RecencyQuery {
    /// 固定 LIMIT 查询 — 高效，适合事件率高的场景
    FixedLimit {
        /// 最大返回条数
        limit: usize,
    },
    /// 时间范围扫描 — 准确，适合事件率低的场景
    DateRange {
        /// 扫描起始时间（UTC）
        since: DateTime<Utc>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_high_rate_uses_fixed_limit() {
        let guard = RecencyGuard::default();
        let strategy = guard.select_query_strategy(100.0);
        assert!(matches!(strategy, RecencyQuery::FixedLimit { limit: 100 }));
    }

    #[test]
    fn test_low_rate_fallbacks_to_date_range() {
        let guard = RecencyGuard::default();
        let strategy = guard.select_query_strategy(1.0);
        assert!(matches!(strategy, RecencyQuery::DateRange { .. }));
    }

    #[test]
    fn test_zero_rate_also_fallbacks() {
        let guard = RecencyGuard::default();
        let strategy = guard.select_query_strategy(0.0);
        assert!(matches!(strategy, RecencyQuery::DateRange { .. }));
    }

    #[test]
    fn test_at_threshold_uses_fixed_limit() {
        // 恰好等于阈值 → 不 fallback（< 阈值才 fallback，等于不触发）
        let guard = RecencyGuard::default();
        let strategy = guard.select_query_strategy(5.0);
        assert!(matches!(strategy, RecencyQuery::FixedLimit { limit: 100 }));
    }

    #[test]
    fn test_recency_guard_default_config() {
        let guard = RecencyGuard::default();
        assert!((guard.events_per_day_threshold - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_custom_threshold() {
        let guard = RecencyGuard::with_threshold(10.0);
        assert!((guard.events_per_day_threshold - 10.0).abs() < f64::EPSILON);

        // 低于阈值 → fallback
        assert!(guard.should_fallback_to_date_scan(5.0));
        // 高于阈值 → 不 fallback
        assert!(!guard.should_fallback_to_date_scan(15.0));
    }

    #[test]
    fn test_should_fallback_at_boundary() {
        let guard = RecencyGuard::with_threshold(3.0);
        // 低于阈值 → fallback
        assert!(guard.should_fallback_to_date_scan(2.999));
        // 等于阈值 → 不 fallback
        assert!(!guard.should_fallback_to_date_scan(3.0));
        // 高于阈值 → 不 fallback
        assert!(!guard.should_fallback_to_date_scan(3.001));
    }
}
