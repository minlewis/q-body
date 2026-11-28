use super::types::*;
use super::Journal;

#[test]
fn golden_signal_vocab_hint_bytes() {
    // 词表文案字节级断言：改词表必先改这条测试
    assert_eq!(
        EvolutionSignal::vocab_hint(),
        "refactor|dedup|test|perf|bump"
    );
}

#[test]
fn golden_signal_vocab_closed_set_roundtrip() {
    // 闭集内逐词 round-trip：as_str ↔ from_vocab 一一对应
    for (i, sig) in EVOLUTION_SIGNALS.iter().enumerate() {
        let word = sig.as_str();
        let back = EvolutionSignal::from_vocab(word)
            .unwrap_or_else(|| panic!("词表词 {word:?} 解析失败"));
        assert_eq!(&back, sig, "词表第 {i} 词 round-trip 失败");
    }
}

#[test]
fn test_signal_vocab_rejects_out_of_band() {
    // 闭集外拒绝（含大小写不敏感 + prose 误燃不通过）
    assert_eq!(
        EvolutionSignal::from_vocab("REFACTOR"),
        Some(EvolutionSignal::Refactor)
    );
    assert_eq!(EvolutionSignal::from_vocab(""), None);
    assert_eq!(EvolutionSignal::from_vocab("unknown"), None);
    assert_eq!(EvolutionSignal::from_vocab("refactor|dedup"), None);
    assert_eq!(EvolutionSignal::from_vocab("signal must be refactor"), None);
}

#[test]
fn test_signal_vocab_pins_both_spelling_layers() {
    // 两层拼写分别钉死（golden 钉现状，usage.rs 判例）：
    // ① API 词表层（handler 入参 / 错误文案）= 小写 as_str()
    // ② JSONL serde 层 = PascalCase 变体名（既有 journal 兼容，不得擅改）
    // 若未来统一两层拼写，必须同步迁移存量 journal 数据并改本测试。
    assert_eq!(
        serde_json::to_string(&EvolutionSignal::Refactor).unwrap(),
        "\"Refactor\""
    );
    assert_eq!(
        serde_json::to_string(&EvolutionSignal::Dedup).unwrap(),
        "\"Dedup\""
    );
    assert_eq!(
        serde_json::to_string(&EvolutionSignal::Test).unwrap(),
        "\"Test\""
    );
    assert_eq!(
        serde_json::to_string(&EvolutionSignal::Perf).unwrap(),
        "\"Perf\""
    );
    assert_eq!(
        serde_json::to_string(&EvolutionSignal::Bump).unwrap(),
        "\"Bump\""
    );
    assert_eq!(EvolutionSignal::Refactor.as_str(), "refactor");
    assert_eq!(
        EvolutionSignal::vocab_hint(),
        "refactor|dedup|test|perf|bump"
    );
}

#[test]
fn test_journal_tracks_evolution_signals() {
    let mut journal = Journal::new();

    journal.record(
        EvolutionSignal::Refactor,
        "c197a87c2805".into(),
        "extract handler error logic".into(),
    );
    journal.record(
        EvolutionSignal::Dedup,
        "c197a87c2805".into(),
        "merge duplicate task parsing".into(),
    );
    journal.record(
        EvolutionSignal::Test,
        "c197a87c2805".into(),
        "add agent-card smoke test".into(),
    );
    journal.record(
        EvolutionSignal::Perf,
        "c197a87c2805".into(),
        "use RwLock instead of Mutex".into(),
    );
    journal.record(
        EvolutionSignal::Bump,
        "c197a87c2805".into(),
        "bump tokio to 1.42".into(),
    );

    // 验证每种信号都能被统计到 Journal
    assert_eq!(journal.count_by_signal(&EvolutionSignal::Refactor), 1);
    assert_eq!(journal.count_by_signal(&EvolutionSignal::Dedup), 1);
    assert_eq!(journal.count_by_signal(&EvolutionSignal::Test), 1);
    assert_eq!(journal.count_by_signal(&EvolutionSignal::Perf), 1);
    assert_eq!(journal.count_by_signal(&EvolutionSignal::Bump), 1);
    assert_eq!(journal.total_events(), 5);
}

#[test]
fn test_evolution_loop_four_stages() {
    let mut journal = Journal::new();

    // 一条完整的四阶段闭环事件
    journal.record_loop(
        EvolutionSignal::Refactor,
        "session 2026-06-14".into(),
        "add evolution event record type".into(),
        "added EvolutionStage enum + record_loop/count_by_stage".into(),
        "cargo test passed".into(),
    );
    // 一条仅 Source/Suggestion 的事件（闭环未走完）
    journal.record(
        EvolutionSignal::Test,
        "session 2026-06-14".into(),
        "add smoke test".into(),
    );

    assert_eq!(journal.total_events(), 2);

    // 两条事件都到达 Source / Suggestion 阶段
    assert_eq!(journal.count_by_stage(EvolutionStage::Source), 2);
    assert_eq!(journal.count_by_stage(EvolutionStage::Suggestion), 2);
    // 只有第一条完整闭环事件到达 Action / Verification
    assert_eq!(journal.count_by_stage(EvolutionStage::Action), 1);
    assert_eq!(journal.count_by_stage(EvolutionStage::Verification), 1);

    // stage_text 取值
    let complete = journal
        .events
        .iter()
        .find(|e| e.reached(EvolutionStage::Verification))
        .unwrap();
    assert_eq!(
        complete.stage_text(EvolutionStage::Action),
        Some("added EvolutionStage enum + record_loop/count_by_stage")
    );
}

#[test]
fn test_dedup_candidate_threshold() {
    let mut journal = Journal::new();

    // 同类信号刚出现 1 次 → 不算候选
    journal.record(
        EvolutionSignal::Refactor,
        "session 2026-06-15".into(),
        "extract handler error logic".into(),
    );
    assert!(!journal.is_dedup_candidate(&EvolutionSignal::Refactor));
    assert!(journal.dedup_candidates().is_empty());

    // 同类信号累计到 2 次 → 标记为 dedup/refactor 候选
    journal.record(
        EvolutionSignal::Refactor,
        "session 2026-06-15".into(),
        "extract task parsing".into(),
    );
    assert!(journal.is_dedup_candidate(&EvolutionSignal::Refactor));

    // 另一类信号刚出现 1 次 → 仍不算候选
    journal.record(
        EvolutionSignal::Test,
        "session 2026-06-15".into(),
        "add agent-card smoke test".into(),
    );
    assert!(!journal.is_dedup_candidate(&EvolutionSignal::Test));

    // dedup_candidates 只列出已达阈值的类型
    let candidates = journal.dedup_candidates();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0], EvolutionSignal::Refactor);

    // 第二类也累计到 2 次
    journal.record(
        EvolutionSignal::Test,
        "session 2026-06-15".into(),
        "add health smoke test".into(),
    );
    let candidates = journal.dedup_candidates();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.contains(&EvolutionSignal::Refactor));
    assert!(candidates.contains(&EvolutionSignal::Test));
}

#[test]
fn test_prediction_record_and_validate() {
    let mut journal = Journal::new();

    // 写入两条未校验的预测（≈ /risk snapshot）
    let idx_a = journal.record_prediction("handler.rs 下次会被 refactor".into());
    let idx_b = journal.record_prediction("a2a/types.rs 下次会引入 dedup 候选".into());

    assert_eq!(journal.total_predictions(), 2);
    assert_eq!(journal.pending_predictions().len(), 2);
    assert_eq!(journal.validated_predictions().len(), 0);

    let pending_a = &journal.pending_predictions()[0];
    assert!(pending_a.validated_at.is_none());
    assert!(pending_a.actual.is_none());
    assert!(pending_a.delta.is_none());
    assert!(!pending_a.is_validated());

    // 校验第一条（≈ /risk validate） — 回填 actual / delta
    let ok = journal.validate_prediction(
        idx_a,
        "handler.rs 被 dispatch 重构（hits）".into(),
        "预测：refactor；实际：refactor → 命中".into(),
    );
    assert!(ok);

    assert_eq!(journal.pending_predictions().len(), 1);
    assert_eq!(journal.validated_predictions().len(), 1);

    let validated = &journal.validated_predictions()[0];
    assert!(validated.is_validated());
    assert_eq!(
        validated.actual.as_deref(),
        Some("handler.rs 被 dispatch 重构（hits）")
    );
    assert_eq!(
        validated.delta.as_deref(),
        Some("预测：refactor；实际：refactor → 命中")
    );
    assert!(validated.validated_at.is_some());

    // 已校验的条目不允许被二次覆盖
    let twice = journal.validate_prediction(idx_a, "覆盖".into(), "覆盖".into());
    assert!(!twice, "重复校验同一条预测应返回 false，避免覆盖对账记录");

    // 越界索引应返回 false，且不影响存储
    let out_of_range = journal.validate_prediction(999, "x".into(), "x".into());
    assert!(!out_of_range);
    assert_eq!(journal.total_predictions(), 2);

    // 第二条仍处于 pending
    let still_pending = journal
        .pending_predictions()
        .iter()
        .any(|p| p.prediction == "a2a/types.rs 下次会引入 dedup 候选");
    assert!(still_pending);
    assert_eq!(idx_b, 1);
}

#[test]
fn test_cycle_reset_and_seen_state() {
    let mut journal = Journal::new();

    let cycle_0 = journal.cycle_id();

    // 初始：seen_state 为空，任何事件 id 都未被见过
    assert_eq!(journal.seen_count(), 0);
    assert!(!journal.was_seen_in_cycle("source-a"));

    // 标记两条事件 id 为本 cycle 已见
    journal.mark_seen("source-a");
    journal.mark_seen("source-b");
    assert_eq!(journal.seen_count(), 2);
    assert!(journal.was_seen_in_cycle("source-a"));
    assert!(journal.was_seen_in_cycle("source-b"));
    assert!(!journal.was_seen_in_cycle("source-c"));

    // 重复 mark 同一 id 不会增加去重表大小
    journal.mark_seen("source-a");
    assert_eq!(journal.seen_count(), 2);

    // 落几条事件 / 一条预测，确认它们不会在 reset_cycle 时被清空
    journal.record(
        EvolutionSignal::Refactor,
        "session 2026-06-21".into(),
        "extract dispatch helper".into(),
    );
    let pred_idx = journal.record_prediction("下次会出 dedup 候选".into());

    // 等一小段时间确保 cycle_id 时间戳前进
    std::thread::sleep(std::time::Duration::from_millis(2));

    // 进入下一个 cycle
    journal.reset_cycle();

    // cycle_id 应向前推进
    assert!(
        journal.cycle_id() > cycle_0,
        "reset_cycle 后 cycle_id 应严格大于上一轮 cycle_id"
    );
    // seen_state 已清空
    assert_eq!(journal.seen_count(), 0);
    assert!(!journal.was_seen_in_cycle("source-a"));
    assert!(!journal.was_seen_in_cycle("source-b"));

    // append-only 的历史不被清空
    assert_eq!(journal.total_events(), 1);
    assert_eq!(journal.total_predictions(), 1);
    assert_eq!(pred_idx, 0);

    // 新 cycle 可以再次标记同名事件，且不与上一 cycle 状态混淆
    journal.mark_seen("source-a");
    assert!(journal.was_seen_in_cycle("source-a"));
    assert_eq!(journal.seen_count(), 1);
}

#[test]
fn test_event_consumed_state() {
    let mut journal = Journal::new();

    // 落三条事件
    journal.record(
        EvolutionSignal::Refactor,
        "session 2026-06-24".into(),
        "extract dispatch helper".into(),
    );
    journal.record(
        EvolutionSignal::Test,
        "session 2026-06-24".into(),
        "add consumed-state test".into(),
    );
    journal.record_loop(
        EvolutionSignal::Dedup,
        "session 2026-06-24".into(),
        "merge duplicate parsing".into(),
        "merged two parse paths".into(),
        "cargo test passed".into(),
    );

    // 初始：全部未消费
    assert_eq!(journal.unconsumed_events().len(), 3);
    assert!(!journal.events[0].is_consumed());

    // 消费第一条
    let ok = journal.mark_consumed(0);
    assert!(ok);
    assert!(journal.events[0].is_consumed());
    assert!(journal.events[0].consumed_at.is_some());
    assert_eq!(journal.unconsumed_events().len(), 2);

    // 重复消费同一条 → false
    let twice = journal.mark_consumed(0);
    assert!(!twice);

    // 消费第二条
    assert!(journal.mark_consumed(1));
    assert_eq!(journal.unconsumed_events().len(), 1);

    // 越界索引 → false
    assert!(!journal.mark_consumed(999));

    // 第三条仍未消费
    let remaining = journal.unconsumed_events();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].signal, EvolutionSignal::Dedup);

    // consumed_at 不被 reset_cycle 清空（与 append-only 历史一致）
    journal.reset_cycle();
    assert!(journal.events[0].is_consumed());
    assert_eq!(journal.unconsumed_events().len(), 1);
}
