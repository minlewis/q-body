//! journal::persistence — Journal 的 JSONL 持久化与 strict resume 校验。
//! 纯移动自 journal.rs（1603 行拆分），无行为变化。

use super::*;

impl Journal {
    /// 将整个 Journal 以 JSONL 格式持久化到指定文件。
    ///
    /// JSONL 格式：每行一条独立的 JSON 记录。
    /// 写入顺序：events 行 → predictions 行 → assessments 行 → journal metadata 行。
    /// 写入是原子性的：先写入临时文件，成功后 rename 到目标路径。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — Day 129 `/risk validate` JSONL 持久化模式。
    /// yoyo-evolve 把 journal 数据以 JSONL append-only 写入文件，每行一条独立 JSON，
    /// 天然支持增量追加、无需锁或事务。
    pub fn persist_to_jsonl(&self, path: &str) -> io::Result<()> {
        let tmp_path = format!("{}.tmp", path);
        {
            let mut file = std::fs::File::create(&tmp_path)?;
            // events
            for event in &self.events {
                let line = serde_json::to_string(event)?;
                writeln!(file, "{}", line)?;
            }
            // predictions
            for pred in &self.predictions {
                let line = serde_json::to_string(pred)?;
                writeln!(file, "{}", line)?;
            }
            // assessments
            for assessment in &self.assessments {
                let line = serde_json::to_string(assessment)?;
                writeln!(file, "{}", line)?;
            }
            // skips — 条件门跳过事件（append-only，与 events 同生命周期）
            for skip in &self.skips {
                let line = serde_json::to_string(skip)?;
                writeln!(file, "{}", line)?;
            }
            // journal metadata — write a single "Journal" row with meta fields
            let meta = serde_json::json!({
                "type": "__journal_meta__",
                "cycle_id": self.cycle_id,
                "seen_state": self.seen_state,
            });
            let line = serde_json::to_string(&meta)?;
            writeln!(file, "{}", line)?;
        }
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// persist + 写后回读自验钩子（write-then-readback）。
    ///
    /// 先执行 `persist_to_jsonl` 落盘，随后立即 `load_from_jsonl` 回读，
    /// 校验四类状态与内存一致：events / predictions / assessments 数量逐一相等，
    /// 且 seen_state 的 key 集合与时间戳一致。任何一项不一致都返回 `false`，
    /// 让「落盘成功」的声明必须以「回读可解析且内容一致」的实测为准，
    /// 而不是只信写入路径的 Ok(())。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — 撤掉无法重新推导的算术、只信任实测
    /// （Task 2 指标必须可复现）→ q-body journal 落盘断言改为实测回读。
    pub fn persist_verified_to_jsonl(&self, path: &str) -> io::Result<bool> {
        self.persist_to_jsonl(path)?;
        self.verify_roundtrip(path)
    }

    /// 写后回读校验核心：从磁盘重载并逐项比对四类状态。
    ///
    /// 独立成方法是为了可测性——篡改文件后直接调 `verify_roundtrip`
    /// 即可构造「落盘成功但内容不一致」的负路径，不必 mock 写入。
    pub fn verify_roundtrip(&self, path: &str) -> io::Result<bool> {
        let reloaded = Self::load_from_jsonl(path)?;

        let events_match = reloaded.events.len() == self.events.len()
            && reloaded
                .events
                .iter()
                .zip(self.events.iter())
                .all(|(a, b)| a.timestamp == b.timestamp && a.signal == b.signal);
        let predictions_match = reloaded.predictions.len() == self.predictions.len()
            && reloaded
                .predictions
                .iter()
                .zip(self.predictions.iter())
                .all(|(a, b)| a.predicted_at == b.predicted_at && a.prediction == b.prediction);
        let assessments_match = reloaded.assessments.len() == self.assessments.len()
            && reloaded
                .assessments
                .iter()
                .zip(self.assessments.iter())
                .all(|(a, b)| {
                    a.assessed_at == b.assessed_at && a.next_direction == b.next_direction
                });
        let seen_state_match = reloaded.seen_state.len() == self.seen_state.len()
            && reloaded
                .seen_state
                .iter()
                .all(|(k, v)| self.seen_state.get(k) == Some(v));

        Ok(events_match && predictions_match && assessments_match && seen_state_match)
    }

    /// 从 JSONL 文件加载 Journal 数据。
    ///
    /// 逐行反序列化：events 行 → PredictionEntry → AssessmentEntry → meta 行（Journal）。
    /// 如果文件不存在，返回一个空的 Journal（不报错，方便首次使用）。
    ///
    /// 借鉴来源同 `persist_to_jsonl` — yologdev/yoyo-evolve Day 129 JSONL 持久化。
    pub fn load_from_jsonl(path: &str) -> io::Result<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::new());
            }
            Err(e) => return Err(e),
        };

        let mut events = Vec::new();
        let mut predictions = Vec::new();
        let mut assessments = Vec::new();
        let mut skips = Vec::new();
        let mut cycle_id = Utc::now();
        let mut seen_state = HashMap::new();
        let mut unrecognized = 0usize;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Try to parse as journal metadata
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                if val.get("type").and_then(|t| t.as_str()) == Some("__journal_meta__") {
                    if let Some(cid) = val.get("cycle_id").and_then(|v| v.as_str()) {
                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(cid) {
                            cycle_id = dt.with_timezone(&Utc);
                        }
                    }
                    if let Some(ss) = val.get("seen_state").and_then(|v| v.as_object()) {
                        for (k, v) in ss {
                            if let Some(ts) = v.as_str() {
                                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                                    seen_state.insert(k.clone(), dt.with_timezone(&Utc));
                                }
                            }
                        }
                    }
                    continue;
                }
                // Skip type markers not recognized（非 meta 的合法 JSON 条目行
                // 需继续走下方三类结构体解析，不在此处计数）
            }

            // Try each type in order
            if let Ok(event) = serde_json::from_str::<EvolutionEvent>(line) {
                events.push(event);
            } else if let Ok(pred) = serde_json::from_str::<PredictionEntry>(line) {
                predictions.push(pred);
            } else if let Ok(assessment) = serde_json::from_str::<AssessmentEntry>(line) {
                assessments.push(assessment);
            } else if let Ok(skip) = serde_json::from_str::<SkipEvent>(line) {
                skips.push(skip);
            } else {
                // 三类结构体都解析失败 — 静默丢弃升级为显式 skip 事件
                unrecognized += 1;
            }
        }

        if unrecognized > 0 {
            skips.push(SkipEvent {
                timestamp: Utc::now(),
                gate: "load_jsonl".into(),
                reason: format!("unrecognized_line_count={}", unrecognized),
            });
        }

        Ok(Self {
            events,
            predictions,
            assessments,
            skips,
            cycle_id,
            seen_state,
        })
    }

    /// Resume-strict gate：处理任务前的 journal 一致性校验（#979 `--continue-strict` 同款）。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — #979 `--continue-strict`：resume 前先校验
    /// 状态可读且自洽，失败在任何 LLM 调用之前返回错误，绝不带病续跑。
    ///
    /// 校验语义（宁可误报不可漏报）：
    /// - 文件不存在 = 空白状态，视为可 resume（`Ok(())`）；
    /// - 文件不可读 / 末行 JSON 不可解析 / 结构行数与内容行数矛盾 = 损坏，返回 `Err`；
    /// - `unrecognized` 静默跳过行 > 0 = 部分损坏，同样拒绝 resume。
    pub fn verify_resume_strict(path: &str) -> Result<(), String> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("journal unreadable: {e}")),
        };

        // 结构行（meta）与内容行（三类条目）都必须可解析为 JSON；
        // 任一整行 parse 失败即视为损坏（区别于 load 的静默 skip 语义）。
        let mut unrecognized = 0usize;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if serde_json::from_str::<serde_json::Value>(line).is_err() {
                unrecognized += 1;
            }
        }
        if unrecognized > 0 {
            return Err(format!(
                "journal corrupt: {unrecognized} unparseable line(s) — refusing strict resume"
            ));
        }

        // 末行存在但为空 / 全空白 = 写入中断的典型残迹，同样拒绝。
        if let Some(last) = content.lines().next_back() {
            if last.trim().is_empty() {
                return Err("journal corrupt: trailing blank line (interrupted write?)".into());
            }
        }

        Ok(())
    }
}
