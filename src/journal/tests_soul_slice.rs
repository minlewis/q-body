use super::types::*;
use super::Journal;
use chrono::Utc;


#[test]
fn test_soul_slice_empty_journal_returns_empty() {
    let j = Journal::new();
    assert_eq!(j.soul_context_slice(2000), "");
}

#[test]
fn test_soul_slice_zero_budget_returns_empty() {
    let mut j = Journal::new();
    j.record(EvolutionSignal::Test, "src".into(), "add test".into());
    assert_eq!(j.soul_context_slice(0), "");
}

#[test]
fn test_soul_slice_respects_budget_keeps_newest() {
    let mut j = Journal::new();
    // 3 条 suggestion 各 50 字符
    for i in 0..3 {
        j.record(EvolutionSignal::Test, format!("src-{}", i), "s".repeat(50));
    }
    // 预算只够 1 条（50+5+32=87）
    let slice = j.soul_context_slice(100);
    assert!(slice.contains("s".repeat(50).as_str()));
    assert!(slice.contains("src-2"), "最新一条必须被保留");
    assert!(!slice.contains("src-0"), "最老条目应被预算挤出");
    assert!(slice.len() <= 100 + 32, "拼接后不得显著超预算");
}

#[test]
fn test_soul_slice_preserves_chronological_order() {
    let mut j = Journal::new();
    j.record(EvolutionSignal::Dedup, "a".into(), "first".into());
    j.record(EvolutionSignal::Perf, "b".into(), "second".into());
    let slice = j.soul_context_slice(2000);
    let first = slice.find("first").expect("first present");
    let second = slice.find("second").expect("second present");
    assert!(first < second, "切片按时间升序（oldest first）");
}

// ---- 序列级 emission-point 测试（issue #110）----
// 借鉴：yologdev/yoyo-evolve — #804「测 SEQUENCE 不是单行」：
// budget-slice 这类 emission-point 必须在「连续写 N 条」的序列行为上验证，
// 单条边界测试不足以覆盖切片语义。

/// 连续写 N 条后按预算取尾：最新 N-k 条保留、最老 k 条被挤出。
#[test]
fn test_soul_slice_sequence_keeps_newest_window() {
    let mut j = Journal::new();
    // 每条 cost = 50(suggestion) + 8(source) + 32(padding) = 90
    for i in 0..10 {
        j.record(EvolutionSignal::Test, format!("src-{}", i), "s".repeat(50));
    }
    // 预算恰好够 3 条：270
    let slice = j.soul_context_slice(270);
    assert!(
        slice.contains("src-9") && slice.contains("src-8") && slice.contains("src-7"),
        "最新 3 条必须全保留"
    );
    assert!(!slice.contains("src-6"), "预算外紧邻条目必须被挤出");
    assert!(!slice.contains("src-0"), "最老条目必须被挤出");
    // 序列内仍按时间升序
    let s7 = slice.find("src-7").expect("src-7 present");
    let s9 = slice.find("src-9").expect("src-9 present");
    assert!(s7 < s9, "窗口内保持 oldest-first");
}

/// 切片边界：预算掐在两段之间——多一条就超、少一条就有富余。
#[test]
fn test_soul_slice_sequence_budget_boundary_between_items() {
    let mut j = Journal::new();
    // 每条 cost = 10(suggestion) + 3(source "s-N") + 32(padding) = 45
    for i in 0..4 {
        j.record(EvolutionSignal::Test, format!("s-{}", i), "x".repeat(10));
    }
    // 预算 = 3×45 − 1 = 134：只容 2 条（第 3 条需累计 135 > 134）
    let slice = j.soul_context_slice(134);
    assert!(
        slice.contains("s-3") && slice.contains("s-2"),
        "最新 2 条全保留"
    );
    assert!(
        !slice.contains("s-1") && !slice.contains("s-0"),
        "其余全部挤出"
    );
    // 预算 +1 = 135：恰好容下第 3 条
    let slice3 = j.soul_context_slice(135);
    assert!(slice3.contains("s-1"), "预算恰好多 1 字节即多容 1 条");
    assert!(!slice3.contains("s-0"), "第 4 条仍被挤出");
}

/// 回读一致性：persist → load 往返后，同一预算的切片逐字节一致。
#[test]
fn test_soul_slice_sequence_roundtrip_consistency() {
    let mut j = Journal::new();
    for i in 0..6 {
        j.record(
            EvolutionSignal::Test,
            format!("rt-{}", i),
            format!("payload-{}-{}", i, "d".repeat(20)),
        );
    }
    let before = j.soul_context_slice(300);
    assert!(!before.is_empty());

    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_soul_slice_seq_{}.jsonl", timestamp);
    j.persist_to_jsonl(&path).expect("persist should succeed");
    let loaded = Journal::load_from_jsonl(&path).expect("load should succeed");
    let after = loaded.soul_context_slice(300);

    assert_eq!(before, after, "往返后同一预算切片必须逐字节一致");
    std::fs::remove_file(&path).ok();
}

/// 序列极值：单条超预算（cost > max_chars）→ 整条被拒，切片为空。
#[test]
fn test_soul_slice_sequence_single_item_over_budget_rejected() {
    let mut j = Journal::new();
    j.record(EvolutionSignal::Test, "src".into(), "h".repeat(500));
    // 预算远小于单条 cost
    assert_eq!(
        j.soul_context_slice(100),
        "",
        "单条超预算必须整条拒收，返回空串"
    );
    // 多条中只有超预算条：同样为空
    j.record(EvolutionSignal::Perf, "small".into(), "ok".into());
    let slice = j.soul_context_slice(100);
    // 遍历从最新开始：small 可容、huge 被拒
    assert!(slice.contains("ok"), "可容条目正常纳入");
    assert!(!slice.contains("hhh"), "超预算条目不得部分泄漏");
}
