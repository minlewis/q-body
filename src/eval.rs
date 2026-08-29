//! 三态 validation ledger
//!
//! 借鉴 yologdev/yoyo-evolve #764 的三态区分取代二值判别模式。
//!
//! 为什么需要这个：二值判别（pass/fail）在 state.db 损坏、evolution_log 为空、
//! target/release/q-body 缺失等场景下会给出误导性 "pass" 结果——
//! 条件触发型脚本静默通过，错误被隐藏到下次条件满足才爆发。
//! 三态 ledger 强制每个判别点返回精确状态，从源头上杜绝假阴性毕业。
//!
//! 与 07-07 的 journal JSONL 持久化互补：前者是事件持久化，
//! 后者是状态完整性断言。

use std::fmt;
use std::path::Path;

/// 三态 validation 状态
///
/// 取代传统的 pass/fail 二值，每个判别点精确返回：
/// - `Missing`: 文件/资源不存在（未创建或被删除）
/// - `Empty`: 存在但无内容（零字节或空行）
/// - `Corrupt`: 存在但内容异常（格式错误、不完整、损坏）
/// - `Valid`: 存在且内容正常
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationState {
    Missing,
    Empty,
    Corrupt,
    Valid,
}

impl ValidationState {
    /// 是否通过了完整性检查（Valid 才算通过）
    pub fn is_valid(&self) -> bool {
        matches!(self, ValidationState::Valid)
    }

    /// 是否需要人工介入（Missing / Corrupt）
    pub fn needs_attention(&self) -> bool {
        matches!(self, ValidationState::Missing | ValidationState::Corrupt)
    }

    /// 严重程度排序：Missing > Corrupt > Empty > Valid
    pub fn severity(&self) -> u8 {
        match self {
            ValidationState::Missing => 0,
            ValidationState::Corrupt => 1,
            ValidationState::Empty => 2,
            ValidationState::Valid => 3,
        }
    }
}

impl fmt::Display for ValidationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationState::Missing => write!(f, "MISSING"),
            ValidationState::Empty => write!(f, "EMPTY"),
            ValidationState::Corrupt => write!(f, "CORRUPT"),
            ValidationState::Valid => write!(f, "VALID"),
        }
    }
}

/// 单条 validation 记录
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    /// 检查点标识（如 "state.db.integrity"、"evolution_log.load"）
    key: String,
    /// 检查路径
    path: String,
    /// 检查结果状态
    state: ValidationState,
    /// 额外信息（如预期大小、实际大小、错误描述）
    detail: Option<String>,
}

impl LedgerEntry {
    pub fn new(key: &str, path: &str, state: ValidationState, detail: Option<&str>) -> Self {
        Self {
            key: key.to_string(),
            path: path.to_string(),
            state,
            detail: detail.map(|s| s.to_string()),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn state(&self) -> &ValidationState {
        &self.state
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for LedgerEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} @ {}: {}", self.state, self.key, self.path, self.detail.as_deref().unwrap_or("no detail"))
    }
}

/// 三态 validation ledger
///
/// 聚合多个判别点的检查结果，提供整体状态查询。
#[derive(Debug, Clone)]
pub struct Ledger {
    entries: Vec<LedgerEntry>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 添加一条 entry
    pub fn add(&mut self, entry: LedgerEntry) {
        self.entries.push(entry);
    }

    /// 获取所有 entry
    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    /// 按 key 查找 entry
    pub fn get(&self, key: &str) -> Option<&LedgerEntry> {
        self.entries.iter().find(|e| e.key() == key)
    }

    /// 所有检查是否全部 Valid
    pub fn all_valid(&self) -> bool {
        self.entries.iter().all(|e| e.state().is_valid())
    }

    /// 需要人工介入的条目
    pub fn attention_items(&self) -> Vec<&LedgerEntry> {
        self.entries.iter().filter(|e| e.state().needs_attention()).collect()
    }

    /// 最严重状态（缺失 > 损坏 > 空 > 有效）
    pub fn worst_state(&self) -> ValidationState {
        self.entries
            .iter()
            .map(|e| e.state().clone())
            .min_by_key(|s| s.severity())
            .unwrap_or(ValidationState::Valid)
    }

    // ============================================================
    // 关键判别点方法
    // ============================================================

    /// 检查文件/目录是否存在且非空
    ///
    /// - 不存在 → Missing
    /// - 存在且零字节 → Empty
    /// - 存在且非零 → Valid
    pub fn check_state(&mut self, key: &str, path: &str) -> ValidationState {
        let p = Path::new(path);
        let state = if !p.exists() {
            ValidationState::Missing
        } else if p.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
            ValidationState::Empty
        } else {
            ValidationState::Valid
        };

        self.add(LedgerEntry::new(
            key,
            path,
            state.clone(),
            Some(&format!("size={}", p.metadata().map(|m| m.len() as i64).unwrap_or(-1))),
        ));
        state
    }

    /// 完整性检查：文件大小是否在预期范围内
    ///
    /// - 不存在 → Missing
    /// - 存在但大小 < min_bytes → Corrupt
    /// - 存在且大小在范围内 → Valid
    pub fn integrity_check(&mut self, key: &str, path: &str, min_bytes: u64) -> ValidationState {
        let p = Path::new(path);
        let state = if !p.exists() {
            ValidationState::Missing
        } else {
            match p.metadata() {
                Ok(meta) if meta.len() >= min_bytes => ValidationState::Valid,
                Ok(_meta) => ValidationState::Corrupt,
                Err(_) => ValidationState::Corrupt,
            }
        };

        let detail = match p.metadata() {
            Ok(meta) => format!("expected>=min={}, actual={}", min_bytes, meta.len()),
            Err(e) => format!("failed to read metadata: {}", e),
        };

        self.add(LedgerEntry::new(key, path, state.clone(), Some(&detail)));
        state
    }

    /// 检查 evolution_log 文件内容有效性
    ///
    /// - 不存在 → Missing
    /// - 存在但空文件 → Empty
    /// - 存在但有内容但非 JSONL → Corrupt（每行非 JSON）
    /// - 存在且至少有一条有效 JSONL → Valid
    pub fn evolution_log_check(&mut self, key: &str, path: &str) -> ValidationState {
        let p = Path::new(path);
        let state = if !p.exists() {
            ValidationState::Missing
        } else {
            match std::fs::read_to_string(p) {
                Ok(content) if content.trim().is_empty() => ValidationState::Empty,
                Ok(content) => {
                    // 至少有一行是有效 JSON
                    let has_valid_json = content
                        .lines()
                        .any(|line| serde_json::from_str::<serde_json::Value>(line.trim()).is_ok());
                    if has_valid_json {
                        ValidationState::Valid
                    } else {
                        ValidationState::Corrupt
                    }
                }
                Err(_) => ValidationState::Corrupt,
            }
        };

        self.add(LedgerEntry::new(key, path, state.clone(), None));
        state
    }

    /// 检查编译产物是否存在
    ///
    /// - 不存在 → Missing
    /// - 存在但零字节 → Corrupt（空二进制不可执行）
    /// - 存在且非零 → Valid
    pub fn binary_check(&mut self, key: &str, path: &str) -> ValidationState {
        let p = Path::new(path);
        let state = if !p.exists() {
            ValidationState::Missing
        } else {
            match p.metadata() {
                Ok(meta) if meta.len() > 0 => ValidationState::Valid,
                Ok(_) => ValidationState::Corrupt,
                Err(_) => ValidationState::Corrupt,
            }
        };

        let detail = match p.metadata() {
            Ok(meta) => format!("size={}", meta.len()),
            Err(e) => format!("failed to read metadata: {}", e),
        };

        self.add(LedgerEntry::new(key, path, state.clone(), Some(&detail)));
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // ValidationState 单元测试
    // ============================================================

    #[test]
    fn test_state_is_valid() {
        assert!(ValidationState::Valid.is_valid());
        assert!(!ValidationState::Missing.is_valid());
        assert!(!ValidationState::Empty.is_valid());
        assert!(!ValidationState::Corrupt.is_valid());
    }

    #[test]
    fn test_state_needs_attention() {
        assert!(ValidationState::Missing.needs_attention());
        assert!(ValidationState::Corrupt.needs_attention());
        assert!(!ValidationState::Empty.needs_attention());
        assert!(!ValidationState::Valid.needs_attention());
    }

    #[test]
    fn test_state_severity_ordering() {
        assert!(ValidationState::Missing.severity() < ValidationState::Corrupt.severity());
        assert!(ValidationState::Corrupt.severity() < ValidationState::Empty.severity());
        assert!(ValidationState::Empty.severity() < ValidationState::Valid.severity());
    }

    #[test]
    fn test_state_display() {
        assert_eq!(ValidationState::Missing.to_string(), "MISSING");
        assert_eq!(ValidationState::Empty.to_string(), "EMPTY");
        assert_eq!(ValidationState::Corrupt.to_string(), "CORRUPT");
        assert_eq!(ValidationState::Valid.to_string(), "VALID");
    }

    // ============================================================
    // LedgerEntry 单元测试
    // ============================================================

    #[test]
    fn test_ledger_entry_creation() {
        let entry = LedgerEntry::new(
            "state.db.integrity",
            "/tmp/test.db",
            ValidationState::Valid,
            Some("size=4096"),
        );
        assert_eq!(entry.key(), "state.db.integrity");
        assert_eq!(entry.path(), "/tmp/test.db");
        assert_eq!(entry.state(), &ValidationState::Valid);
        assert_eq!(entry.detail(), Some("size=4096"));
    }

    #[test]
    fn test_ledger_entry_no_detail() {
        let entry = LedgerEntry::new(
            "state.db.integrity",
            "/tmp/test.db",
            ValidationState::Missing,
            None,
        );
        assert_eq!(entry.detail(), None);
    }

    #[test]
    fn test_ledger_entry_display() {
        let entry = LedgerEntry::new(
            "state.db.integrity",
            "/tmp/test.db",
            ValidationState::Missing,
            Some("file not found"),
        );
        let display = entry.to_string();
        assert!(display.contains("MISSING"));
        assert!(display.contains("state.db.integrity"));
        assert!(display.contains("file not found"));
    }

    // ============================================================
    // Ledger 功能测试
    // ============================================================

    #[test]
    fn test_empty_ledger_is_valid() {
        let ledger = Ledger::new();
        assert!(ledger.all_valid());
        assert!(ledger.entries().is_empty());
        assert_eq!(ledger.worst_state(), ValidationState::Valid);
    }

    #[test]
    fn test_ledger_add_and_get() {
        let mut ledger = Ledger::new();
        let entry = LedgerEntry::new("test.key", "/tmp/test", ValidationState::Valid, None);
        ledger.add(entry);
        assert_eq!(ledger.entries().len(), 1);
        assert!(ledger.get("test.key").is_some());
        assert!(ledger.get("nonexistent").is_none());
    }

    #[test]
    fn test_ledger_all_valid_with_mixed() {
        let mut ledger = Ledger::new();
        ledger.add(LedgerEntry::new("a", "/tmp/a", ValidationState::Valid, None));
        ledger.add(LedgerEntry::new("b", "/tmp/b", ValidationState::Empty, None));
        assert!(!ledger.all_valid());
    }

    #[test]
    fn test_ledger_attention_items() {
        let mut ledger = Ledger::new();
        ledger.add(LedgerEntry::new("a", "/tmp/a", ValidationState::Missing, None));
        ledger.add(LedgerEntry::new("b", "/tmp/b", ValidationState::Valid, None));
        ledger.add(LedgerEntry::new("c", "/tmp/c", ValidationState::Corrupt, None));
        ledger.add(LedgerEntry::new("d", "/tmp/d", ValidationState::Empty, None));
        assert_eq!(ledger.attention_items().len(), 2);
    }

    #[test]
    fn test_ledger_worst_state_ordering() {
        let mut ledger = Ledger::new();
        ledger.add(LedgerEntry::new("a", "/tmp/a", ValidationState::Valid, None));
        ledger.add(LedgerEntry::new("b", "/tmp/b", ValidationState::Empty, None));
        ledger.add(LedgerEntry::new("c", "/tmp/c", ValidationState::Missing, None));
        // Missing 比 Empty 严重
        assert_eq!(ledger.worst_state(), ValidationState::Missing);
    }

    // ============================================================
    // 关键判别点集成测试
    // ============================================================

    #[test]
    fn test_check_state_missing() {
        let mut ledger = Ledger::new();
        let state = ledger.check_state("missing.file", "/tmp/__nonexistent_eval_test_file__");
        assert_eq!(state, ValidationState::Missing);
    }

    #[test]
    fn test_check_state_valid() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_test_valid");
        std::fs::write(&tmp, b"hello")?;

        let mut ledger = Ledger::new();
        let state = ledger.check_state("valid.file", tmp.to_str().unwrap());
        assert_eq!(state, ValidationState::Valid);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_check_state_empty() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_test_empty");
        std::fs::write(&tmp, b"")?;

        let mut ledger = Ledger::new();
        let state = ledger.check_state("empty.file", tmp.to_str().unwrap());
        assert_eq!(state, ValidationState::Empty);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_integrity_check_missing() {
        let mut ledger = Ledger::new();
        let state = ledger.integrity_check(
            "state.db.integrity",
            "/tmp/__nonexistent_state_db__",
            1024,
        );
        assert_eq!(state, ValidationState::Missing);
    }

    #[test]
    fn test_integrity_check_below_min() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_integrity_small");
        std::fs::write(&tmp, b"tiny")?;

        let mut ledger = Ledger::new();
        let state = ledger.integrity_check("small.file", tmp.to_str().unwrap(), 1024);
        assert_eq!(state, ValidationState::Corrupt);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_integrity_check_valid() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_integrity_ok");
        let content = vec![b'a'; 2048];
        std::fs::write(&tmp, &content)?;

        let mut ledger = Ledger::new();
        let state = ledger.integrity_check("ok.file", tmp.to_str().unwrap(), 1024);
        assert_eq!(state, ValidationState::Valid);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_evolution_log_check_missing() {
        let mut ledger = Ledger::new();
        let state = ledger.evolution_log_check(
            "evolution_log.load",
            "/tmp/__nonexistent_evolution_log__",
        );
        assert_eq!(state, ValidationState::Missing);
    }

    #[test]
    fn test_evolution_log_check_empty() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_evolog_empty");
        std::fs::write(&tmp, b"")?;

        let mut ledger = Ledger::new();
        let state = ledger.evolution_log_check("evolog.empty", tmp.to_str().unwrap());
        assert_eq!(state, ValidationState::Empty);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_evolution_log_check_valid_jsonl() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_evolog_valid");
        std::fs::write(&tmp, b"{\"event\":\"test\"}\n{\"event\":\"another\"}\n")?;

        let mut ledger = Ledger::new();
        let state = ledger.evolution_log_check("evolog.valid", tmp.to_str().unwrap());
        assert_eq!(state, ValidationState::Valid);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_evolution_log_check_corrupt_format() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_evolog_corrupt");
        std::fs::write(&tmp, b"not json\nstill not json\n")?;

        let mut ledger = Ledger::new();
        let state = ledger.evolution_log_check("evolog.corrupt", tmp.to_str().unwrap());
        assert_eq!(state, ValidationState::Corrupt);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_binary_check_missing() {
        let mut ledger = Ledger::new();
        let state = ledger.binary_check(
            "target.release.q_body",
            "/tmp/__nonexistent_binary__",
        );
        assert_eq!(state, ValidationState::Missing);
    }

    #[test]
    fn test_binary_check_zerobyte() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_binary_empty");
        std::fs::write(&tmp, b"")?;

        let mut ledger = Ledger::new();
        let state = ledger.binary_check("binary.empty", tmp.to_str().unwrap());
        assert_eq!(state, ValidationState::Corrupt);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }

    #[test]
    fn test_binary_check_valid() -> std::io::Result<()> {
        let tmp = std::env::temp_dir().join("__eval_binary_valid");
        std::fs::write(&tmp, b"ELF...")?;

        let mut ledger = Ledger::new();
        let state = ledger.binary_check("binary.valid", tmp.to_str().unwrap());
        assert_eq!(state, ValidationState::Valid);

        std::fs::remove_file(&tmp)?;
        Ok(())
    }
}