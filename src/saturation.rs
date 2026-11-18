//! src/saturation.rs — 信号饱和检测纯函数模块
//!
//! 借鉴：yologdev/yoyo-evolve — skill-evolve saturation NO-OP + evt-0057 cooldown prediction。
//! yoyo 在信号类别高度重复时产出 NO-OP / cooldown，而不是硬凑新方向；q-body 的
//! 每日养料（Cron A）与之同构——每天必须产出 P0 会退化成仪式感（SOUL §18/§20），
//! 用「与昨日 key_signals 类别重合率」做数据驱动的停手。
//!
//! 用法：Cron A prompt 增补「信号饱和检测」段，运行本模块判定；判定饱和时
//! 产出占位 P0（`（今日无 P0 — 信号饱和第 N 天）`）而非硬凑方向。
//!
//! standalone 先例（08-05 learnings.rs / 09-02 clamp.rs / 09-04 darkroom.rs）：
//! 纯函数 + 单测先落 standalone 模块；运行时接线（读昨日 signal 快照、写今日
//! 快照的路径）合入 main 后一行接线。

/// 默认重合率阈值：与昨日信号类别重合 ≥ 80% 判定为饱和
pub const DEFAULT_SATURATION_RATIO: f64 = 0.8;

/// 饱和判定结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saturation {
    /// 有新增量信号，正常产出
    Fresh,
    /// 与昨日高度重合，判定饱和（cooldown）
    Saturated,
    /// 昨日无信号基准可比较（首次运行），保守按不饱和处理
    NoBaseline,
}

/// 信号类别集合：用排序去重后的类别名序列表示（可哈希比较）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalSet {
    pub categories: Vec<String>,
}

impl SignalSet {
    /// 从类别名迭代器构造，自动排序去重
    pub fn from_iter<I, S>(iter: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut v: Vec<String> = iter.into_iter().map(Into::into).collect();
        v.sort();
        v.dedup();
        SignalSet { categories: v }
    }

    pub fn is_empty(&self) -> bool {
        self.categories.is_empty()
    }

    /// 类别数
    pub fn len(&self) -> usize {
        self.categories.len()
    }
}

/// 计算重合类别数（交集大小）
pub fn overlap_count(a: &SignalSet, b: &SignalSet) -> usize {
    // 两者均已排序去重，可双指针求交集
    let (mut i, mut j, mut count) = (0, 0, 0);
    while i < a.categories.len() && j < b.categories.len() {
        match a.categories[i].cmp(&b.categories[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                count += 1;
                i += 1;
                j += 1;
            }
        }
    }
    count
}

/// 重合率 = 交集大小 / 今日类别数（分母是今日，衡量「今日有多少是重复昨日」）。
/// 今日为空集合 → 无信号，按 Fresh（无事发生，不硬造饱和）。
pub fn overlap_ratio(today: &SignalSet, yesterday: &SignalSet) -> f64 {
    if today.is_empty() {
        return 0.0;
    }
    overlap_count(today, yesterday) as f64 / today.len() as f64
}

/// 核心判定：饱和检测
///
/// - 昨日无基准（空集合 / 无快照）→ NoBaseline，不饱和
/// - 今日空集合 → Fresh（0 信号 = 无事发生，不硬凑饱和）
/// - 重合率 ≥ threshold → Saturated，否则 Fresh
///
/// threshold 语义沿用 09-06 recency `<` 判例：等于阈值即饱和（≥ 而非 >）。
pub fn detect_saturation(today: &SignalSet, yesterday: &SignalSet, threshold: f64) -> Saturation {
    if yesterday.is_empty() {
        return Saturation::NoBaseline;
    }
    let ratio = overlap_ratio(today, yesterday);
    if ratio >= threshold {
        Saturation::Saturated
    } else {
        Saturation::Fresh
    }
}

/// 生成降级后的占位 P0 行（数据驱动停手，非硬凑方向）
///
/// `n` = 连续饱和第 N 天，从 1 起计。
pub fn saturated_p0_line(n: u32) -> String {
    format!("- P0: （今日无 P0 — 信号饱和第 {n} 天，重合率≥80% 自动降级占位）")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set<const N: usize>(cats: [&str; N]) -> SignalSet {
        SignalSet::from_iter(cats)
    }

    #[test]
    fn test_from_iter_sorts_and_dedups() {
        let s = SignalSet::from_iter(["b", "a", "b", "c"]);
        assert_eq!(s.categories, vec!["a", "b", "c"]);
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn test_overlap_count_basic() {
        let a = set(["refactor", "test", "perf"]);
        let b = set(["refactor", "test", "cleanup"]);
        assert_eq!(overlap_count(&a, &b), 2);
    }

    #[test]
    fn test_overlap_count_no_overlap() {
        let a = set(["refactor"]);
        let b = set(["bugfix"]);
        assert_eq!(overlap_count(&a, &b), 0);
    }

    #[test]
    fn test_ratio_saturated_at_80_percent() {
        // 今天 5 类，4 类与昨日重合 = 0.8
        let today = set(["refactor", "test", "perf", "cleanup", "new1"]);
        let yesterday = set(["refactor", "test", "perf", "cleanup", "old1"]);
        assert!((overlap_ratio(&today, &yesterday) - 0.8).abs() < 1e-9);
        assert_eq!(
            detect_saturation(&today, &yesterday, DEFAULT_SATURATION_RATIO),
            Saturation::Saturated
        );
    }

    #[test]
    fn test_ratio_below_threshold_fresh() {
        // 今天 5 类，3 类重合 = 0.6 < 0.8
        let today = set(["refactor", "new2", "new3", "new4", "new5"]);
        let yesterday = set(["refactor", "test", "perf", "cleanup", "old1"]);
        assert_eq!(
            detect_saturation(&today, &yesterday, DEFAULT_SATURATION_RATIO),
            Saturation::Fresh
        );
    }

    #[test]
    fn test_exact_threshold_is_saturated() {
        // 边界：2/2 = 1.0 ≥ 0.8 饱和；1/1 = 1.0 饱和（同一类重复）
        let today = set(["refactor"]);
        let yesterday = set(["refactor"]);
        assert_eq!(
            detect_saturation(&today, &yesterday, DEFAULT_SATURATION_RATIO),
            Saturation::Saturated
        );
    }

    #[test]
    fn test_no_baseline_fresh() {
        // 昨日无基准 → NoBaseline（首次运行不误判饱和）
        let today = set(["refactor", "test"]);
        let empty = SignalSet::from_iter(Vec::<String>::new());
        assert_eq!(
            detect_saturation(&today, &empty, DEFAULT_SATURATION_RATIO),
            Saturation::NoBaseline
        );
    }

    #[test]
    fn test_empty_today_is_not_saturated() {
        // 今日无信号 → 不饱和（无事发生，不硬造饱和）
        let empty = SignalSet::from_iter(Vec::<String>::new());
        let yesterday = set(["refactor", "test"]);
        assert_eq!(
            detect_saturation(&empty, &yesterday, DEFAULT_SATURATION_RATIO),
            Saturation::Fresh
        );
    }

    #[test]
    fn test_custom_threshold() {
        // 自定义阈值 0.5：2/3 ≈ 0.667 ≥ 0.5 饱和；用 0.9 则 Fresh
        let today = set(["refactor", "test", "new1"]);
        let yesterday = set(["refactor", "test", "old1"]);
        assert_eq!(
            detect_saturation(&today, &yesterday, 0.5),
            Saturation::Saturated
        );
        assert_eq!(
            detect_saturation(&today, &yesterday, 0.9),
            Saturation::Fresh
        );
    }

    #[test]
    fn test_saturated_p0_line_format() {
        let line = saturated_p0_line(3);
        assert!(line.starts_with("- P0: "), "必须符合下游格式契约");
        assert!(line.contains("信号饱和第 3 天"));
        // 占位行仍以 `- P0: ` 开头，nourish-to-backlog.sh 正则可继续匹配
        let line1 = saturated_p0_line(1);
        assert!(line1.contains("信号饱和第 1 天"));
    }
}