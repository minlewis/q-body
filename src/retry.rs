//! 重试计数器 — 追踪 LLM 调用重试频率
//!
//! 借鉴：yologdev/yoyo-evolve — Task 1+eval-fix 1 节奏
//! yoyo 在每个任务的第一步执行后做 eval-fix 循环，失败时自动重试并记数。
//! → q-body 对应：LLM 调用失败时自动重试（最多 2 次），每次重试 bump 计数器，
//!    `retry_counts()` 暴露各信号累计重试次数供 evo 审计。
//!
//! 信号类型：
//! - `llm_http_error`：HTTP 请求失败（网络不可达、超时等）
//! - `llm_api_error`：LLM API 返回非 2xx 状态码
//! - `llm_parse_error`：响应 JSON 解析失败

use std::collections::HashMap;

/// 重试计数器 — 追踪 LLM 调用重试频率
#[derive(Debug, Clone)]
pub struct RetryCounter {
    /// 当前周期编号（单调递增）
    cycle: u64,
    /// 当前周期内的重试计数：signal_name → count
    counts: HashMap<String, u32>,
    /// 历史归档：cycle → signal_name → count
    archive: HashMap<u64, HashMap<String, u32>>,
}

impl RetryCounter {
    /// 创建一个新的空计数器，cycle 从 1 开始
    pub fn new() -> Self {
        Self {
            cycle: 1,
            counts: HashMap::new(),
            archive: HashMap::new(),
        }
    }

    /// 为指定信号 bump 一次重试计数，返回更新后的值
    pub fn bump(&mut self, signal: &str) -> u32 {
        let count = self.counts.entry(signal.to_string()).or_insert(0);
        *count += 1;
        *count
    }

    /// 重置当前周期：将当前 counts 归档后清空，cycle 递增
    pub fn reset(&mut self) {
        if !self.counts.is_empty() {
            self.archive.insert(self.cycle, self.counts.clone());
        }
        self.counts.clear();
        self.cycle += 1;
    }

    /// 获取当前周期的重试计数（按信号名排序）
    pub fn counts(&self) -> Vec<(String, u32)> {
        let mut pairs: Vec<(String, u32)> = self.counts.clone().into_iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }

    /// 获取当前周期编号
    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    /// 获取指定信号在当前周期的重试次数（不存在时返回 0）
    pub fn count_for(&self, signal: &str) -> u32 {
        self.counts.get(signal).copied().unwrap_or(0)
    }

    /// 获取完整历史归档：cycle → signal → count
    pub fn archive(&self) -> &HashMap<u64, HashMap<String, u32>> {
        &self.archive
    }

    /// 总重试次数（当前周期 + 历史归档）
    pub fn total_retries(&self) -> u64 {
        let current: u64 = self.counts.values().map(|v| *v as u64).sum();
        let historical: u64 = self
            .archive
            .values()
            .flat_map(|m| m.values())
            .map(|v| *v as u64)
            .sum();
        current + historical
    }
}

impl Default for RetryCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_counter_empty() {
        let c = RetryCounter::new();
        assert_eq!(c.cycle(), 1);
        assert!(c.counts().is_empty());
        assert_eq!(c.total_retries(), 0);
    }

    #[test]
    fn test_bump_increments_counter() {
        let mut c = RetryCounter::new();
        assert_eq!(c.bump("llm_http_error"), 1);
        assert_eq!(c.bump("llm_http_error"), 2);
        assert_eq!(c.count_for("llm_http_error"), 2);
    }

    #[test]
    fn test_bump_multiple_signals() {
        let mut c = RetryCounter::new();
        c.bump("llm_http_error");
        c.bump("llm_api_error");
        c.bump("llm_http_error");
        assert_eq!(c.count_for("llm_http_error"), 2);
        assert_eq!(c.count_for("llm_api_error"), 1);
        assert_eq!(c.count_for("llm_parse_error"), 0);
    }

    #[test]
    fn test_reset_archives_and_clears() {
        let mut c = RetryCounter::new();
        c.bump("llm_http_error");
        c.bump("llm_http_error");
        c.bump("llm_api_error");
        assert_eq!(c.cycle(), 1);

        c.reset();
        assert_eq!(c.cycle(), 2);
        assert!(c.counts().is_empty());
        assert_eq!(c.count_for("llm_http_error"), 0);

        // 归档保留
        let archived = c.archive();
        assert_eq!(archived.len(), 1);
        let cycle1 = &archived[&1];
        assert_eq!(cycle1.get("llm_http_error"), Some(&2));
        assert_eq!(cycle1.get("llm_api_error"), Some(&1));
    }

    #[test]
    fn test_unknown_signal_returns_zero() {
        let c = RetryCounter::new();
        assert_eq!(c.count_for("nonexistent"), 0);
    }

    #[test]
    fn test_total_retries() {
        let mut c = RetryCounter::new();
        c.bump("llm_http_error");
        c.bump("llm_http_error");
        c.bump("llm_api_error");
        assert_eq!(c.total_retries(), 3);

        c.reset();
        c.bump("llm_parse_error");
        assert_eq!(c.total_retries(), 4); // 3 archived + 1 current
    }

    #[test]
    fn test_empty_reset_advances_cycle() {
        let mut c = RetryCounter::new();
        c.reset(); // empty — no archive entry
        assert_eq!(c.cycle(), 2);
        assert!(c.archive().is_empty());
    }

    #[test]
    fn test_default_is_empty() {
        let c: RetryCounter = Default::default();
        assert_eq!(c.cycle(), 1);
        assert!(c.counts().is_empty());
    }
}