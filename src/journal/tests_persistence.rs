use super::types::*;
use super::Journal;
use chrono::Utc;


#[test]
fn test_persist_verified_readback_roundtrip() {
    let mut journal = Journal::new();
    journal.record(
        EvolutionSignal::Refactor,
        "session 2026-09-17".into(),
        "add readback hook".into(),
    );
    journal.record_loop(
        EvolutionSignal::Test,
        "session 2026-09-17".into(),
        "verified persist".into(),
        "added persist_verified_to_jsonl".into(),
        "cargo test passed".into(),
    );
    let pred_idx = journal.record_prediction("readback will pass".into());
    journal.validate_prediction(pred_idx, "passed".into(), "matched".into());
    journal.record_assessment(1, 0, Some(1.0), "keep hooking write paths".into());
    journal.mark_seen("source-readback");

    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_verified_journal_{}.jsonl", timestamp);

    let ok = journal
        .persist_verified_to_jsonl(&path)
        .expect("persist_verified should succeed");
    assert!(ok, "写后回读四类状态应全部一致");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_verify_roundtrip_detects_tampered_file() {
    let mut journal = Journal::new();
    journal.record(
        EvolutionSignal::Dedup,
        "session 2026-09-17".into(),
        "tamper detection".into(),
    );
    journal.mark_seen("source-tamper");

    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_tampered_journal_{}.jsonl", timestamp);

    journal
        .persist_to_jsonl(&path)
        .expect("persist should succeed");

    // 负路径 1：文件被截断（事件行丢失）→ 回读不一致必须判 false
    std::fs::write(&path, "").expect("truncate should succeed");
    let ok = journal
        .verify_roundtrip(&path)
        .expect("load of empty file should not error");
    assert!(!ok, "截断文件回读事件数不一致，必须判 false");

    // 负路径 2：写入无法解析的垃圾行 → 回读丢事件，同样判 false
    std::fs::write(&path, "not-valid-json at all\n").expect("write should succeed");
    let ok = journal
        .verify_roundtrip(&path)
        .expect("load of garbage lines should not error");
    assert!(!ok, "垃圾行回读丢事件，必须判 false");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_persist_verified_missing_dir_is_io_error() {
    let journal = Journal::new();
    let result = journal.persist_verified_to_jsonl("/nonexistent_dir_xyz/j.jsonl");
    assert!(result.is_err(), "目标目录不存在应返回 IO 错误而非 false");
}

#[test]
fn test_assessment_record() {
    let mut journal = Journal::new();

    // 第一周期：2 有效 / 1 无效，prediction 命中率 0.5，方向：收敛 dedup
    journal.record_assessment(2, 1, Some(0.5), "收敛 dedup 候选".into());
    assert_eq!(journal.total_assessments(), 1);

    let a = &journal.assessments()[0];
    assert_eq!(a.effective_suggestions, 2);
    assert_eq!(a.ineffective_suggestions, 1);
    assert_eq!(a.prediction_hit_rate, Some(0.5));
    assert_eq!(a.next_direction, "收敛 dedup 候选");
    assert!(a.assessed_at <= Utc::now());

    // 第二周期：尚无已校验 prediction → hit_rate = None
    journal.record_assessment(0, 0, None, "积攒更多 prediction 样本".into());
    assert_eq!(journal.total_assessments(), 2);
    assert_eq!(journal.assessments()[1].prediction_hit_rate, None);

    // reset_cycle 不清空 append-only 的评估记录（与 events / predictions 一致）
    journal.reset_cycle();
    assert_eq!(journal.total_assessments(), 2);
}

#[test]
fn test_persist_and_load_jsonl_roundtrip() {
    let mut journal = Journal::new();

    // 落几条事件、两条预测、一条评估
    journal.record(
        EvolutionSignal::Refactor,
        "session 2026-07-07".into(),
        "add JSONL persistence".into(),
    );
    journal.record_loop(
        EvolutionSignal::Test,
        "session 2026-07-07".into(),
        "add roundtrip test".into(),
        "added test_persist_and_load_jsonl_roundtrip".into(),
        "cargo test passed".into(),
    );
    let pred_idx = journal.record_prediction("下次会加 data-driven 测试".into());
    journal.validate_prediction(
        pred_idx,
        "roundtrip test added".into(),
        "预测：data-driven；实际：roundtrip → 接近".into(),
    );
    journal.record_assessment(2, 0, Some(0.8), "JSONL 持久化+更完善 data-driven".into());

    // 标记 seen_state
    journal.mark_seen("source-a");
    journal.mark_seen("source-b");

    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_journal_{}.jsonl", timestamp);

    // persist
    journal
        .persist_to_jsonl(&path)
        .expect("persist should succeed");

    // 验证文件存在且非空
    let content = std::fs::read_to_string(&path).expect("should read file");
    assert!(!content.is_empty(), "JSONL file should not be empty");
    let line_count = content.lines().count();
    assert_eq!(
        line_count, 5,
        "2 events + 1 prediction + 1 assessment + 1 meta = 5 lines"
    );

    // load
    let loaded = Journal::load_from_jsonl(&path).expect("load should succeed");

    // 验证内容一致性
    assert_eq!(loaded.total_events(), 2);
    assert_eq!(loaded.total_predictions(), 1);
    assert_eq!(loaded.total_assessments(), 1);

    // 事件内容
    let loaded_refactor = loaded
        .events
        .iter()
        .find(|e| e.signal == EvolutionSignal::Refactor);
    assert!(loaded_refactor.is_some());
    assert_eq!(loaded_refactor.unwrap().source, "session 2026-07-07");

    // 预测
    let loaded_pred = &loaded.predictions[0];
    assert!(loaded_pred.is_validated());
    assert_eq!(loaded_pred.actual.as_deref(), Some("roundtrip test added"));

    // 评估
    assert_eq!(loaded.assessments[0].effective_suggestions, 2);
    assert_eq!(loaded.assessments[0].prediction_hit_rate, Some(0.8));

    // seen_state 持久化
    assert!(loaded.was_seen_in_cycle("source-a"));
    assert!(loaded.was_seen_in_cycle("source-b"));
    assert!(!loaded.was_seen_in_cycle("source-c"));
    assert_eq!(loaded.seen_count(), 2);

    // 清理
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_load_jsonl_file_not_found_returns_empty() {
    let journal = Journal::load_from_jsonl("/tmp/nonexistent_journal.jsonl")
        .expect("missing file should return empty journal");
    assert_eq!(journal.total_events(), 0);
    assert_eq!(journal.total_predictions(), 0);
    assert_eq!(journal.total_assessments(), 0);
    assert_eq!(journal.total_skips(), 0);
    assert_eq!(journal.seen_count(), 0);
}

#[test]
fn test_skip_events_on_gate_rejections() {
    // 借鉴：yoyo-evolve validation event — 条件门的每次跳过必须显式落账
    let mut journal = Journal::new();
    assert_eq!(journal.total_skips(), 0);

    // mark_consumed：越界 → skip
    assert!(!journal.mark_consumed(999));
    // mark_consumed：重复消费 → skip
    journal.record(
        EvolutionSignal::Test,
        "session 2026-09-23".into(),
        "skip event audit".into(),
    );
    assert!(journal.mark_consumed(0));
    assert!(!journal.mark_consumed(0));
    // validate_prediction：越界 → skip；重复校验 → skip
    let idx = journal.record_prediction("p".into());
    assert!(journal.validate_prediction(idx, "a".into(), "d".into()));
    assert!(!journal.validate_prediction(idx, "a".into(), "d".into()));
    assert!(!journal.validate_prediction(999, "a".into(), "d".into()));

    let skips = journal.skips();
    assert_eq!(skips.len(), 4, "4 次静默跳过应全部落显式 skip 事件");
    assert_eq!(skips[0].gate, "mark_consumed");
    assert_eq!(skips[0].reason, "index_out_of_range");
    assert_eq!(skips[1].gate, "mark_consumed");
    assert_eq!(skips[1].reason, "already_consumed");
    assert_eq!(skips[2].gate, "validate_prediction");
    assert_eq!(skips[2].reason, "already_validated");
    assert_eq!(skips[3].gate, "validate_prediction");
    assert_eq!(skips[3].reason, "index_out_of_range");
    // 每条 skip 事件都带时间戳
    assert!(skips.iter().all(|s| s.timestamp <= Utc::now()));
}

#[test]
fn test_skip_events_persist_and_load_roundtrip() {
    let mut journal = Journal::new();
    journal.record_skip("hash_gate", "condition_not_met");
    journal.record_skip("probe", "already_running");

    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_journal_skip_{}.jsonl", timestamp);
    journal
        .persist_to_jsonl(&path)
        .expect("persist should succeed");

    let loaded = Journal::load_from_jsonl(&path).expect("load should succeed");
    assert_eq!(loaded.total_skips(), 2);
    assert_eq!(loaded.skips()[0].gate, "hash_gate");
    assert_eq!(loaded.skips()[0].reason, "condition_not_met");
    assert_eq!(loaded.skips()[1].gate, "probe");
    assert_eq!(loaded.skips()[1].reason, "already_running");
    assert_eq!(loaded.skips()[0].timestamp, journal.skips()[0].timestamp);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_load_jsonl_unrecognized_lines_become_skip_event() {
    // 静默丢弃未识别行 → 升格为一条显式 skip 事件
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_journal_garbage_{}.jsonl", timestamp);
    std::fs::write(&path, "not json at all\n{\"also\":\"not an entry\"}\n")
        .expect("write garbage file");

    let loaded = Journal::load_from_jsonl(&path).expect("load should succeed");
    assert_eq!(loaded.total_events(), 0);
    assert_eq!(
        loaded.total_skips(),
        1,
        "未识别行应聚合为一条 load_jsonl skip 事件"
    );
    assert_eq!(loaded.skips()[0].gate, "load_jsonl");
    assert_eq!(loaded.skips()[0].reason, "unrecognized_line_count=2");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_persist_and_load_empty_journal() {
    let journal = Journal::new();
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_empty_journal_{}.jsonl", timestamp);

    journal
        .persist_to_jsonl(&path)
        .expect("persist empty journal should succeed");

    let loaded = Journal::load_from_jsonl(&path).expect("load empty journal should succeed");
    assert_eq!(loaded.total_events(), 0);
    assert_eq!(loaded.total_predictions(), 0);
    assert_eq!(loaded.total_assessments(), 0);
    assert_eq!(loaded.seen_count(), 0);

    let _ = std::fs::remove_file(&path);
}

// ---- resume-strict gate（#979 `--continue-strict` 同款）----

#[test]
fn test_resume_strict_missing_file_is_ok() {
    let path = "/tmp/test_resume_strict_missing_does_not_exist.jsonl";
    let _ = std::fs::remove_file(path);
    assert!(
        Journal::verify_resume_strict(path).is_ok(),
        "文件不存在 = 空白状态，应放行"
    );
}

#[test]
fn test_resume_strict_clean_journal_passes() {
    let mut journal = Journal::new();
    journal.record(EvolutionSignal::Test, "src".into(), "suggestion".into());
    journal.record_skip("probe", "already_running");
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_resume_strict_clean_{}.jsonl", timestamp);
    journal
        .persist_to_jsonl(&path)
        .expect("persist should succeed");
    assert!(
        Journal::verify_resume_strict(&path).is_ok(),
        "完整落盘的 journal 应通过 strict gate"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_resume_strict_rejects_garbage_lines() {
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_resume_strict_garbage_{}.jsonl", timestamp);
    std::fs::write(&path, "{\"ok\":true}\nnot json at all\n").expect("write garbage file");
    let err =
        Journal::verify_resume_strict(&path).expect_err("不可解析行必须拒绝 strict resume");
    assert!(err.contains("unparseable"), "错误信息应点明损坏原因：{err}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_resume_strict_rejects_truncated_trailing_line() {
    // 模拟写入中断：最后一行 JSON 被截断（非完整 JSON = unparseable）
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_resume_strict_trunc_{}.jsonl", timestamp);
    std::fs::write(&path, "{\"type\":\"__journal_meta__\"}\n{\"signal\":\"Te")
        .expect("write truncated file");
    assert!(
        Journal::verify_resume_strict(&path).is_err(),
        "截断的末行（写入中断残迹）必须拒绝 strict resume"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_resume_strict_rejects_trailing_blank_line() {
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let path = format!("/tmp/test_resume_strict_blank_{}.jsonl", timestamp);
    std::fs::write(&path, "{\"ok\":true}\n   \n").expect("write blank-tail file");
    let err = Journal::verify_resume_strict(&path)
        .expect_err("末行空白（写入中断残迹）必须拒绝 strict resume");
    assert!(
        err.contains("trailing blank"),
        "错误信息应点明损坏原因：{err}"
    );
    let _ = std::fs::remove_file(&path);
}
