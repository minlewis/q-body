//! learnings — 双文件学习记录（learnings.md + seen_state.json）
//!
//! 独立于 journal.rs 的 standalone 模块，实现：
//! - learnings.md（append-only，增量追加）
//! - seen_state.json（signal hash set，去重判定）
//!
//! 相同 signal hash 命中已 seen 集合时跳过写入。
//! 借鉴：yologdev/yoyo-evolve — social session 双文件结构
//!   (learnings.md + seen_state.json，signal hash 去重)

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 一条学习记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedEntry {
    /// 信号哈希（去重 key）
    pub signal_hash: String,
    /// 学习内容
    pub content: String,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后一次被 seen 的时间
    pub seen_at: Option<DateTime<Utc>>,
}

/// 双文件学习存储
#[derive(Debug)]
pub struct LearningsStore {
    /// learnings.md 文件路径
    learnings_path: PathBuf,
    /// seen_state.json 文件路径
    seen_state_path: PathBuf,
    /// 已 seen 的 signal hash 集合
    seen: HashSet<String>,
    /// 内存中缓存的条目
    entries: Vec<LearnedEntry>,
}

impl LearningsStore {
    /// 创建新的 LearningsStore，加载已有持久化数据
    pub fn new(base_dir: &Path) -> Result<Self, String> {
        let learnings_path = base_dir.join("learnings.md");
        let seen_state_path = base_dir.join("seen_state.json");

        // 确保目录存在
        fs::create_dir_all(base_dir)
            .map_err(|e| format!("创建 learnings 目录失败: {}", e))?;

        // 加载已 seen 的 signal hash 集合
        let seen = if seen_state_path.exists() {
            let content = fs::read_to_string(&seen_state_path)
                .map_err(|e| format!("读取 seen_state.json 失败: {}", e))?;
            serde_json::from_str(&content)
                .unwrap_or_default()
        } else {
            HashSet::new()
        };

        // 加载已有学习记录（仅用于 status 查询，不全部缓存在内存）
        let entries = if learnings_path.exists() {
            Self::parse_learnings_file(&learnings_path)
        } else {
            Vec::new()
        };

        Ok(Self {
            learnings_path,
            seen_state_path,
            seen,
            entries,
        })
    }

    /// 解析 learnings.md 文件（逐行，每行一个 JSON）
    fn parse_learnings_file(path: &Path) -> Vec<LearnedEntry> {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                serde_json::from_str::<LearnedEntry>(line).ok()
            })
            .collect()
    }

    /// 添加一条学习记录（先查 seen_state 去重，命中则跳过）
    /// 返回 true 表示新写入，false 表示已 seen 跳过
    pub fn add_entry(&mut self, signal_hash: &str, content: &str) -> Result<bool, String> {
        // 去重检查
        if self.seen.contains(signal_hash) {
            return Ok(false);
        }

        let entry = LearnedEntry {
            signal_hash: signal_hash.to_string(),
            content: content.to_string(),
            created_at: Utc::now(),
            seen_at: None,
        };

        // append 到 learnings.md（JSONL 格式）
        let line = serde_json::to_string(&entry)
            .map_err(|e| format!("序列化学习条目失败: {}", e))?;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.learnings_path)
            .map_err(|e| format!("打开 learnings.md 失败: {}", e))?;

        writeln!(file, "{}", line)
            .map_err(|e| format!("写入 learnings.md 失败: {}", e))?;

        // 标记 seen
        self.seen.insert(signal_hash.to_string());
        self.entries.push(entry);

        // 持久化 seen_state
        self.save_seen_state()?;

        Ok(true)
    }

    /// 标记一条记录为 seen（更新 seen_at）
    pub fn mark_seen(&mut self, signal_hash: &str) -> Result<bool, String> {
        if self.seen.contains(signal_hash) {
            return Ok(false); // 已 seen
        }

        self.seen.insert(signal_hash.to_string());
        self.save_seen_state()?;
        Ok(true)
    }

    /// 检查 signal_hash 是否已被 seen
    pub fn is_seen(&self, signal_hash: &str) -> bool {
        self.seen.contains(signal_hash)
    }

    /// 获取所有学习记录
    pub fn entries(&self) -> &[LearnedEntry] {
        &self.entries
    }

    /// 获取已 seen 的 signal hash 数量
    pub fn seen_count(&self) -> usize {
        self.seen.len()
    }

    /// 获取未 seen 的记录（无 seen_at 的条目）
    pub fn unseen_entries(&self) -> Vec<&LearnedEntry> {
        self.entries
            .iter()
            .filter(|e| e.seen_at.is_none())
            .collect()
    }

    /// 持久化 seen_state 到 JSON 文件
    fn save_seen_state(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.seen)
            .map_err(|e| format!("序列化 seen_state 失败: {}", e))?;

        // 原子写入：先写临时文件，再 rename
        let tmp_path = self.seen_state_path.with_extension("json.tmp");
        fs::write(&tmp_path, &json)
            .map_err(|e| format!("写入 seen_state 临时文件失败: {}", e))?;
        fs::rename(&tmp_path, &self.seen_state_path)
            .map_err(|e| format!("替换 seen_state.json 失败: {}", e))?;

        Ok(())
    }

    /// 重新加载 seen_state（从磁盘）
    pub fn reload_seen_state(&mut self) -> Result<(), String> {
        if self.seen_state_path.exists() {
            let content = fs::read_to_string(&self.seen_state_path)
                .map_err(|e| format!("读取 seen_state.json 失败: {}", e))?;
            self.seen = serde_json::from_str(&content).unwrap_or_default();
        }
        Ok(())
    }

    /// 重新加载 learnings（从磁盘）
    pub fn reload_learnings(&mut self) -> Result<(), String> {
        if self.learnings_path.exists() {
            self.entries = Self::parse_learnings_file(&self.learnings_path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_temp_store() -> (LearningsStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = LearningsStore::new(dir.path()).unwrap();
        (store, dir)
    }

    #[test]
    fn test_new_store_is_empty() {
        let (store, _dir) = setup_temp_store();
        assert_eq!(store.seen_count(), 0);
        assert!(store.entries().is_empty());
    }

    #[test]
    fn test_add_new_entry() {
        let (mut store, _dir) = setup_temp_store();
        let added = store.add_entry("hash1", "content1").unwrap();
        assert!(added, "新条目应被写入");
        assert_eq!(store.seen_count(), 1);
        assert_eq!(store.entries().len(), 1);
    }

    #[test]
    fn test_duplicate_entry_skipped() {
        let (mut store, _dir) = setup_temp_store();
        let added1 = store.add_entry("hash1", "content1").unwrap();
        assert!(added1);

        let added2 = store.add_entry("hash1", "content1 again").unwrap();
        assert!(!added2, "重复 signal_hash 应跳过");
        assert_eq!(store.seen_count(), 1, "seen 集合不应增长");
        assert_eq!(store.entries().len(), 1, "不应新增条目");
    }

    #[test]
    fn test_mark_seen_new_hash() {
        let (mut store, _dir) = setup_temp_store();
        let marked = store.mark_seen("hash-new").unwrap();
        assert!(marked, "新 hash 应被标记");
        assert!(store.is_seen("hash-new"));
    }

    #[test]
    fn test_mark_seen_existing_hash() {
        let (mut store, _dir) = setup_temp_store();
        store.add_entry("hash1", "content1").unwrap();
        let marked = store.mark_seen("hash1").unwrap();
        assert!(!marked, "已 seen 的 hash 标记应返回 false");
    }

    #[test]
    fn test_is_seen() {
        let (mut store, _dir) = setup_temp_store();
        assert!(!store.is_seen("unknown"));
        store.add_entry("h1", "c1").unwrap();
        assert!(store.is_seen("h1"));
    }

    #[test]
    fn test_persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();

        // 写入
        {
            let mut store = LearningsStore::new(dir.path()).unwrap();
            store.add_entry("hash1", "学习内容1").unwrap();
            store.add_entry("hash2", "学习内容2").unwrap();
        }

        // 重新加载
        {
            let store = LearningsStore::new(dir.path()).unwrap();
            assert_eq!(store.seen_count(), 2, "persist 后 seen 应恢复");
            assert!(store.is_seen("hash1"));
            assert!(store.is_seen("hash2"));
        }
    }

    #[test]
    fn test_learnings_file_created_on_write() {
        let (mut store, dir) = setup_temp_store();
        let path = dir.path().join("learnings.md");
        assert!(!path.exists(), "未写入时 learnings.md 不应存在");
        store.add_entry("h1", "c1").unwrap();
        assert!(path.exists(), "写入后 learnings.md 应存在");
    }

    #[test]
    fn test_seen_state_file_exists_after_write() {
        let (mut store, dir) = setup_temp_store();
        store.add_entry("h1", "c1").unwrap();
        let path = dir.path().join("seen_state.json");
        assert!(path.exists(), "写入后 seen_state.json 应存在");

        // 验证内容
        let content = fs::read_to_string(&path).unwrap();
        let parsed: HashSet<String> = serde_json::from_str(&content).unwrap();
        assert!(parsed.contains("h1"));
    }
}