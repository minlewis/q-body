//! Trust 状态持久化 — workspace/project 信任状态的持久化存储
//!
//! 借鉴：yologdev/yoyo-evolve — `--trust-project-always` 持久化 + `is_trust_project()`
//! yoyo 将项目信任状态写入 user-level store（~/.config/yoyo/trust.db），
//! 后续启动直接读取免确认，避免每轮 systemd restart 后信任状态丢失。
//!
//! q-body 对应：TrustEntry / TrustStore 结构体，JSON 持久化到 ~/.config/q-body/trust.db

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 信任条目：记录一个项目路径的信任状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    /// 项目路径（绝对路径）
    pub project_path: String,
    /// 信任创建时间戳（Unix 秒）
    pub trusted_at: u64,
    /// 过期时间戳（Unix 秒），None 表示永不过期
    pub expires_at: Option<u64>,
}

impl TrustEntry {
    pub fn new(project_path: &str, expires_in: Option<Duration>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            project_path: project_path.to_string(),
            trusted_at: now,
            expires_at: expires_in.map(|d| now + d.as_secs()),
        }
    }

    /// 该条目是否已过期
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                now >= exp
            }
            None => false, // 永不过期
        }
    }
}

/// 信任存储：管理所有已信任项目的持久化 HashMap
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustStore {
    /// key: 项目路径，value: 信任条目
    entries: HashMap<String, TrustEntry>,
    /// 持久化文件路径
    #[serde(skip)]
    db_path: PathBuf,
}

impl TrustStore {
    /// 创建 TrustStore，从默认路径加载
    pub fn new() -> Self {
        let db_path = Self::default_db_path();
        let mut store = Self {
            entries: HashMap::new(),
            db_path: db_path.clone(),
        };
        store.load();
        store
    }

    /// 指定路径创建 TrustStore
    pub fn with_path(db_path: PathBuf) -> Self {
        let mut store = Self {
            entries: HashMap::new(),
            db_path,
        };
        store.load();
        store
    }

    /// 默认持久化路径：~/.config/q-body/trust.db
    fn default_db_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home)
            .join(".config")
            .join("q-body")
            .join("trust.db")
    }

    /// 从文件加载信任状态
    fn load(&mut self) {
        if self.db_path.exists() {
            match fs::read_to_string(&self.db_path) {
                Ok(content) => {
                    if let Ok(parsed) =
                        serde_json::from_str::<HashMap<String, TrustEntry>>(&content)
                    {
                        self.entries = parsed;
                        // 清理过期条目
                        self.entries.retain(|_, entry| !entry.is_expired());
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load trust.db: {e}");
                }
            }
        }
    }

    /// 持久化信任状态到文件
    fn save(&self) -> Result<(), String> {
        // 确保目录存在
        if let Some(parent) = self.db_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create trust db dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| format!("failed to serialize trust entries: {e}"))?;
        fs::write(&self.db_path, &json).map_err(|e| format!("failed to write trust.db: {e}"))?;
        Ok(())
    }

    /// 检查项目路径是否已被信任
    pub fn is_trusted(&self, path: &str) -> bool {
        match self.entries.get(path) {
            Some(entry) => !entry.is_expired(),
            None => false,
        }
    }

    /// 标记项目路径为信任
    /// `expires_in` 为 None 表示永不过期
    pub fn mark_trusted(&mut self, path: &str, expires_in: Option<Duration>) -> Result<(), String> {
        let entry = TrustEntry::new(path, expires_in);
        self.entries.insert(path.to_string(), entry);
        self.save()?;
        Ok(())
    }

    /// 撤销项目路径的信任
    pub fn revoke(&mut self, path: &str) -> Result<(), String> {
        self.entries.remove(path);
        self.save()?;
        Ok(())
    }

    /// 列出所有信任的项目路径
    pub fn list_trusted(&self) -> Vec<&TrustEntry> {
        self.entries.values().filter(|e| !e.is_expired()).collect()
    }

    /// 清理过期条目
    pub fn purge_expired(&mut self) -> Result<(), String> {
        let before = self.entries.len();
        self.entries.retain(|_, entry| !entry.is_expired());
        if self.entries.len() < before {
            self.save()?;
        }
        Ok(())
    }

    /// 获取内部条目数（用于测试）
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for TrustStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 使用临时目录创建 TrustStore，避免污染真实文件
    fn temp_store() -> (TempDir, TrustStore) {
        let dir = TempDir::new();
        let db_path = dir.path().join("trust.db");
        let store = TrustStore::with_path(db_path);
        (dir, store)
    }

    struct TempDir {
        path: PathBuf,
        _id: u64,
    }

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("q-body-trust-test-{id}"));
            let _ = fs::create_dir_all(&dir);
            Self { path: dir, _id: id }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_new_store_is_empty() {
        let (_dir, store) = temp_store();
        assert_eq!(store.len(), 0);
        assert!(!store.is_trusted("/some/path"));
    }

    #[test]
    fn test_mark_and_check_trusted() {
        let (_dir, mut store) = temp_store();
        store.mark_trusted("/home/user/project", None).unwrap();
        assert!(store.is_trusted("/home/user/project"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_revoke_trust() {
        let (_dir, mut store) = temp_store();
        store.mark_trusted("/home/user/project", None).unwrap();
        assert!(store.is_trusted("/home/user/project"));
        store.revoke("/home/user/project").unwrap();
        assert!(!store.is_trusted("/home/user/project"));
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_expired_trust_is_not_trusted() {
        let (_dir, mut store) = temp_store();
        // 1 秒过期
        store
            .mark_trusted("/tmp/ephemeral", Some(Duration::from_secs(1)))
            .unwrap();
        assert!(store.is_trusted("/tmp/ephemeral"));
        // 让时间过去
        std::thread::sleep(Duration::from_secs(2));
        assert!(!store.is_trusted("/tmp/ephemeral"));
    }

    #[test]
    fn test_list_trusted_excludes_expired() {
        let (_dir, mut store) = temp_store();
        store.mark_trusted("/perm", None).unwrap();
        store
            .mark_trusted("/ephemeral", Some(Duration::from_secs(1)))
            .unwrap();
        std::thread::sleep(Duration::from_secs(2));
        let list = store.list_trusted();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].project_path, "/perm");
    }

    #[test]
    fn test_purge_expired() {
        let (_dir, mut store) = temp_store();
        store.mark_trusted("/perm", None).unwrap();
        store
            .mark_trusted("/ephemeral", Some(Duration::from_secs(1)))
            .unwrap();
        std::thread::sleep(Duration::from_secs(2));
        store.purge_expired().unwrap();
        assert_eq!(store.len(), 1);
        assert!(store.is_trusted("/perm"));
    }

    #[test]
    fn test_persistence_roundtrip() {
        let dir = TempDir::new();
        let db_path = dir.path().join("trust.db");
        {
            let mut store = TrustStore::with_path(db_path.clone());
            store.mark_trusted("/persist", None).unwrap();
        } // drop
        {
            let store = TrustStore::with_path(db_path);
            assert!(store.is_trusted("/persist"));
            assert_eq!(store.len(), 1);
        }
    }
}