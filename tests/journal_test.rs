//! q-body Journal 模块单元测试
//!
//! 覆盖范围（v0.1.4 P0 范围）：
//! - JournalEntry::auto / manual 构造
//! - JSON 序列化往返
//! - JournalStore CRUD（save / get / list / add_learning）
//!
//! 不测试：文件持久化（依赖 tmp 路径，下个 PR 加）

use q_body::journal::{JournalEntry, JournalStore};

#[test]
fn auto_entry_has_expected_fields() {
    let e = JournalEntry::auto("task-1", "用户问 hello".into());
    assert_eq!(e.task_id, "task-1");
    assert_eq!(e.summary, "用户问 hello");
    assert_eq!(e.source, "auto");
    assert!(e.learnings.is_empty(), "auto 应无 learning");
    assert!(!e.created_at.is_empty(), "created_at 应自动填");
}

#[test]
fn manual_entry_has_learnings() {
    let e = JournalEntry::manual(
        "task-2",
        "用户咨询架构".into(),
        vec!["老板偏好 Rust".into(), "A2A 协议用 jsonrpc".into()],
    );
    assert_eq!(e.source, "manual");
    assert_eq!(e.learnings.len(), 2);
    assert_eq!(e.learnings[0], "老板偏好 Rust");
}

#[test]
fn entry_json_roundtrip() {
    let original = JournalEntry::manual(
        "task-3",
        "test".into(),
        vec!["l1".into(), "l2".into()],
    );
    let json = serde_json::to_string(&original).expect("序列化应成功");
    let restored: JournalEntry = serde_json::from_str(&json).expect("反序列化应成功");
    assert_eq!(restored.task_id, original.task_id);
    assert_eq!(restored.summary, original.summary);
    assert_eq!(restored.learnings, original.learnings);
    assert_eq!(restored.source, original.source);
    assert_eq!(restored.created_at, original.created_at);
}

#[tokio::test]
async fn store_save_and_get() {
    let store = JournalStore::new();
    let entry = JournalEntry::auto("task-A", "first".into());
    store.save(entry).await;

    let got = store.get("task-A").await;
    assert!(got.is_some(), "刚 save 的应能 get 到");
    assert_eq!(got.unwrap().summary, "first");
}

#[tokio::test]
async fn store_get_nonexistent_returns_none() {
    let store = JournalStore::new();
    let got = store.get("nonexistent").await;
    assert!(got.is_none(), "不存在的 task_id 应返回 None");
}

#[tokio::test]
async fn store_list_returns_all_ids() {
    // list() 实际签名: Vec<String> (返回 task_ids)
    let store = JournalStore::new();
    for i in 0..5 {
        let e = JournalEntry::auto(&format!("task-{}", i), format!("entry {}", i));
        store.save(e).await;
    }
    let ids = store.list().await;
    assert_eq!(ids.len(), 5, "list 应返回 5 个 task_id");
    assert!(ids.contains(&"task-0".to_string()));
    assert!(ids.contains(&"task-4".to_string()));
}

#[tokio::test]
async fn store_save_upserts() {
    // save 同 task_id 应覆盖（upsert 语义）
    let store = JournalStore::new();
    let e1 = JournalEntry::auto("task-X", "first version".into());
    let e2 = JournalEntry::auto("task-X", "second version".into());
    store.save(e1).await;
    store.save(e2).await;

    let ids = store.list().await;
    assert_eq!(ids.len(), 1, "upsert 后应只有 1 条");
    assert_eq!(store.get("task-X").await.unwrap().summary, "second version");
}

#[tokio::test]
async fn add_learning_appends() {
    let store = JournalStore::new();
    let e = JournalEntry::auto("task-L", "summary".into());
    store.save(e).await;

    let ok = store.add_learning("task-L", "老板偏好简洁".into()).await;
    assert!(ok, "add_learning 应返回 true");

    let got = store.get("task-L").await.unwrap();
    assert_eq!(got.learnings.len(), 1);
    assert_eq!(got.learnings[0], "老板偏好简洁");

    // 再加一条
    store.add_learning("task-L", "微信限流归因存疑".into()).await;
    let got = store.get("task-L").await.unwrap();
    assert_eq!(got.learnings.len(), 2);
}

#[tokio::test]
async fn add_learning_on_nonexistent_returns_false() {
    let store = JournalStore::new();
    let ok = store.add_learning("ghost", "x".into()).await;
    assert!(!ok, "不存在 task 的 add_learning 应返回 false");
}