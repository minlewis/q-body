//! Journal — q-body 的会话记录与学习提取系统
//!
//! 每次 SendMessage 完成后自动创建一个 Journal 条目，
//! 记录：会话摘要、学习点、任务状态。
//! 支持手动追加和查询。
//!
//! 这是 q-body 自进化循环的第一步（yoyo 式 session wrap-up + journal + learnings）
//!
//! v0.1.3 新增：文件持久化 — JournalStore 自动保存到 ~/.q-body/journal.json

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// 一条 Journal 记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// 关联的 Task ID
    pub task_id: String,
    /// 创建时间 (ISO 8601)
    pub created_at: String,
    /// 会话摘要（用户说了什么、agent 应答了什么）
    pub summary: String,
    /// 学习点列表
    pub learnings: Vec<String>,
    /// 这条 Journal 的来源（auto = SendMessage 自动生成, manual = 手动追加）
    pub source: String,
}

impl JournalEntry {
    pub fn auto(task_id: &str, summary: String) -> Self {
        Self {
            task_id: task_id.to_string(),
            created_at: Utc::now().to_rfc3339(),
            summary,
            learnings: Vec::new(),
            source: "auto".into(),
        }
    }

    pub fn manual(task_id: &str, summary: String, learnings: Vec<String>) -> Self {
        Self {
            task_id: task_id.to_string(),
            created_at: Utc::now().to_rfc3339(),
            summary,
            learnings,
            source: "manual".into(),
        }
    }
}

/// Journal 存储（内存 + 文件持久化）
#[derive(Debug, Clone)]
pub struct JournalStore {
    entries: Arc<RwLock<HashMap<String, JournalEntry>>>,
    /// 持久化文件路径。如果为 None，则为纯内存模式（不写入磁盘）
    data_path: Option<Arc<PathBuf>>,
}

impl JournalStore {
    /// 创建纯内存 JournalStore
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            data_path: None,
        }
    }

    /// 创建带文件持久化的 JournalStore
    ///
    /// 自动从 `data_path` 加载已有条目，
    /// 每次 save/add_learning 后自动持久化到磁盘。
    pub fn new_persistent(data_path: PathBuf) -> Self {
        let store = Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            data_path: Some(Arc::new(data_path)),
        };
        store.load();
        store
    }

    /// 从持久化文件加载条目
    fn load(&self) {
        if let Some(ref path) = self.data_path {
            match std::fs::read_to_string(path.as_ref()) {
                Ok(content) if !content.trim().is_empty() => {
                    match serde_json::from_str::<HashMap<String, JournalEntry>>(&content) {
                        Ok(entries) => {
                            // Use try_write to avoid blocking in non-async context
                            // If write lock is contended, skip the load silently
                            if let Ok(mut store) = self.entries.try_write() {
                                let count = entries.len();
                                *store = entries;
                                tracing::info!(
                                    "JournalStore: loaded {} entries from {}",
                                    count,
                                    path.as_ref().display()
                                );
                            } else {
                                tracing::warn!(
                                    "JournalStore: RwLock busy, deferred loading from {}",
                                    path.as_ref().display()
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "JournalStore: failed to parse '{}': {}. Starting fresh.",
                                path.as_ref().display(),
                                e
                            );
                        }
                    }
                }
                Ok(_) => {
                    tracing::debug!(
                        "JournalStore: '{}' is empty. Starting fresh.",
                        path.as_ref().display()
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tracing::info!(
                        "JournalStore: '{}' not found. Will create on first save.",
                        path.as_ref().display()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "JournalStore: failed to read '{}': {}. Starting fresh.",
                        path.as_ref().display(),
                        e
                    );
                }
            }
        }
    }

    /// 持久化当前所有条目到文件
    fn persist(&self) {
        if let Some(ref path) = self.data_path {
            if let Some(parent) = path.as_ref().parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(store) = self.entries.try_read() {
                match serde_json::to_string_pretty(&*store) {
                    Ok(json) => match std::fs::write(path.as_ref(), &json) {
                        Ok(_) => {
                            tracing::debug!(
                                "JournalStore: persisted {} entries to {}",
                                store.len(),
                                path.as_ref().display()
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "JournalStore: failed to write '{}': {}",
                                path.as_ref().display(),
                                e
                            );
                        }
                    },
                    Err(e) => {
                        tracing::error!("JournalStore: failed to serialize: {}", e);
                    }
                }
            } else {
                tracing::warn!("JournalStore: RwLock busy, skipped persist");
            }
        }
    }

    /// 保存（或覆盖）一个 Journal 条目
    pub async fn save(&self, entry: JournalEntry) {
        {
            let mut store = self.entries.write().await;
            store.insert(entry.task_id.clone(), entry);
        }
        self.persist();
    }

    /// 获取指定 Task 的 Journal
    pub async fn get(&self, task_id: &str) -> Option<JournalEntry> {
        let store = self.entries.read().await;
        store.get(task_id).cloned()
    }

    /// 列出所有 Journal 条目 ID
    pub async fn list(&self) -> Vec<String> {
        let store = self.entries.read().await;
        let mut keys: Vec<String> = store.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// 追加学习点到已有 Journal
    pub async fn add_learning(&self, task_id: &str, learning: String) -> bool {
        let result = {
            let mut store = self.entries.write().await;
            if let Some(entry) = store.get_mut(task_id) {
                entry.learnings.push(learning);
                true
            } else {
                false
            }
        };
        self.persist();
        result
    }
}

impl Default for JournalStore {
    fn default() -> Self {
        Self::new()
    }
}