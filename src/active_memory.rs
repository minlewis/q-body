//! Active Memory — q-body 的记忆合成模块
//!
//! 借鉴：yologdev/yoyo-evolve — `synthesize: regenerate active memory context`
//!
//! yoyo-evolve 在每个 task 入口前从持久化记忆中读取最近活跃记录，
//! 合成 "active memory context" 注入 LLM system prompt。
//! q-body 对应：MemoryEntry / ActiveMemory struct + synthesize() 方法，
//! 从 `~/.hermes/q-body-memory/` 读取 JSONL 记忆文件。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 单条记忆记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// 记忆唯一标识
    pub id: String,
    /// 记忆内容
    pub content: String,
    /// 记录时间戳
    pub timestamp: DateTime<Utc>,
    /// 来源（如 "task", "journal", "manual"）
    pub source: String,
    /// 可选标签
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 活跃记忆管理器
#[derive(Debug, Clone)]
pub struct ActiveMemory {
    /// 记忆条目列表（按时间戳降序，最新的在前）
    entries: Vec<MemoryEntry>,
    /// 记忆存储目录
    storage_dir: PathBuf,
    /// 合成时返回的最大条目数
    max_synthesize: usize,
}

impl Default for ActiveMemory {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            storage_dir: PathBuf::from(
                std::env::var("HOME")
                    .unwrap_or_else(|_| "/tmp".into()),
            )
            .join(".hermes")
            .join("q-body-memory"),
            max_synthesize: 20,
        }
    }
}

impl ActiveMemory {
    /// 创建一个新的 ActiveMemory 实例
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置存储目录
    pub fn with_storage_dir(mut self, dir: PathBuf) -> Self {
        self.storage_dir = dir;
        self
    }

    /// 设置最大合成条目数
    pub fn with_max_synthesize(mut self, max: usize) -> Self {
        self.max_synthesize = max;
        self
    }

    /// 添加一条新记忆（自动保持时间降序）
    pub fn add_entry(&mut self, entry: MemoryEntry) {
        let ts = entry.timestamp;
        // 找到插入位置（保持降序，最新的在前）
        let pos = self
            .entries
            .iter()
            .position(|e| e.timestamp < ts)
            .unwrap_or(self.entries.len());
        self.entries.insert(pos, entry);
    }

    /// 从 JSONL 文件加载记忆
    pub fn load_from_jsonl(&mut self) -> Result<usize, String> {
        let path = self.storage_dir.join("memories.jsonl");
        if !path.exists() {
            return Ok(0);
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        let mut count = 0;
        self.entries.clear();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<MemoryEntry>(trimmed) {
                Ok(entry) => {
                    // 保持降序插入
                    let pos = self
                        .entries
                        .iter()
                        .position(|e| e.timestamp < entry.timestamp)
                        .unwrap_or(self.entries.len());
                    self.entries.insert(pos, entry);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!("Skipping malformed JSONL line: {}", e);
                }
            }
        }

        Ok(count)
    }

    /// 保存记忆到 JSONL 文件（append-only）
    pub fn save_to_jsonl(&self, entry: &MemoryEntry) -> Result<(), String> {
        // 确保目录存在
        std::fs::create_dir_all(&self.storage_dir)
            .map_err(|e| format!("Failed to create storage dir: {}", e))?;

        let path = self.storage_dir.join("memories.jsonl");
        let line = serde_json::to_string(entry)
            .map_err(|e| format!("Failed to serialize entry: {}", e))?;

        // append-only 写入
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))
            .and_then(|mut file| {
                use std::io::Write;
                writeln!(file, "{}", line)
                    .map_err(|e| format!("Failed to write: {}", e))
            })?;

        Ok(())
    }

    /// 合成活跃记忆上下文摘要
    ///
    /// 返回格式化的文本摘要，包含最近 N 条记忆的关键信息。
    /// 适用于注入 LLM system prompt 作为上下文。
    pub fn synthesize(&self) -> String {
        if self.entries.is_empty() {
            return "No active memory available.".to_string();
        }

        let count = self.entries.len().min(self.max_synthesize);
        let mut parts = Vec::new();

        parts.push(format!(
            "[Active Memory — {} recent entries]",
            count
        ));

        for entry in self.entries.iter().take(count) {
            let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC");
            let tags = if entry.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", entry.tags.join(", "))
            };
            parts.push(format!(
                "  • [{}] from {}: {}{}",
                ts, entry.source, entry.content, tags
            ));
        }

        parts.push(format!(
            "[End of active memory — showing {} of {} entries]",
            count,
            self.entries.len()
        ));

        parts.join("\n")
    }

    /// 返回当前条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        id: &str,
        content: &str,
        timestamp: DateTime<Utc>,
        source: &str,
        tags: Vec<&str>,
    ) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            content: content.to_string(),
            timestamp,
            source: source.to_string(),
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_empty_memory_synthesize() {
        let mem = ActiveMemory::new();
        let result = mem.synthesize();
        assert_eq!(result, "No active memory available.");
    }

    #[test]
    fn test_add_entry_and_synthesize() {
        let mut mem = ActiveMemory::new();
        let now = Utc::now();

        mem.add_entry(make_entry(
            "1",
            "Hello world",
            now,
            "test",
            vec!["greeting"],
        ));

        let result = mem.synthesize();
        assert!(result.contains("Hello world"));
        assert!(result.contains("from test"));
        assert!(result.contains("[greeting]"));
        assert!(result.contains("[Active Memory"));
        assert!(result.contains("[End of active memory"));
        assert_eq!(mem.len(), 1);
    }

    #[test]
    fn test_synthesize_respects_max() {
        let mut mem = ActiveMemory::new().with_max_synthesize(2);
        let now = Utc::now();

        for i in 0..5 {
            mem.add_entry(make_entry(
                &format!("id-{}", i),
                &format!("Entry {}", i),
                now - chrono::Duration::seconds(i),
                "test",
                vec![],
            ));
        }

        let result = mem.synthesize();
        // 应该只包含 2 条
        assert!(result.contains("2 recent entries"));
        assert!(result.contains("Entry 0"));
        assert!(result.contains("Entry 1"));
        assert!(!result.contains("Entry 2"));
        assert_eq!(mem.len(), 5); // 5 entries stored, but only 2 in synthesis
    }

    #[test]
    fn test_insertion_order_descending() {
        let mut mem = ActiveMemory::new();
        let now = Utc::now();

        // 插入顺序：旧 → 新
        mem.add_entry(make_entry(
            "old",
            "Oldest",
            now - chrono::Duration::hours(2),
            "test",
            vec![],
        ));
        mem.add_entry(make_entry(
            "mid",
            "Middle",
            now - chrono::Duration::hours(1),
            "test",
            vec![],
        ));
        mem.add_entry(make_entry(
            "new",
            "Newest",
            now,
            "test",
            vec![],
        ));

        let result = mem.synthesize();
        // 最新的在前
        let new_pos = result.find("Newest").unwrap();
        let mid_pos = result.find("Middle").unwrap();
        let old_pos = result.find("Oldest").unwrap();
        assert!(new_pos < mid_pos);
        assert!(mid_pos < old_pos);
    }

    #[test]
    fn test_jsonl_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();

        let mut mem = ActiveMemory::new().with_storage_dir(dir_path.clone());
        let now = Utc::now();

        let entry = make_entry(
            "persist-1",
            "Persisted memory",
            now,
            "test",
            vec!["persist"],
        );
        mem.add_entry(entry.clone());
        mem.save_to_jsonl(&entry).unwrap();

        // 新建一个实例并加载
        let mut loaded = ActiveMemory::new().with_storage_dir(dir_path);
        let count = loaded.load_from_jsonl().unwrap();
        assert_eq!(count, 1);
        assert_eq!(loaded.len(), 1);

        let result = loaded.synthesize();
        assert!(result.contains("Persisted memory"));
    }

    #[test]
    fn test_entries_empty_after_clear() {
        let mut mem = ActiveMemory::new();
        let now = Utc::now();

        mem.add_entry(make_entry("1", "Temp", now, "test", vec![]));
        assert_eq!(mem.len(), 1);
        assert!(!mem.is_empty());

        // 通过加载空目录来清空
        mem.entries.clear();
        assert_eq!(mem.len(), 0);
        assert!(mem.is_empty());

        let result = mem.synthesize();
        assert_eq!(result, "No active memory available.");
    }
}