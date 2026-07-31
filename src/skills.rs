//! Skills registry — hot-reloadable skill management for q-body
//!
//! Inspired by evalstate/fast-agent's Skills-as-first-class model
//! and IBM/mcp-context-forge's plugin registry discovery pattern.
//!
//! SkillsRegistry 提供注册、查询、热重载 API：
//! - `register`: 注册一个新 skill（覆盖已存在的同名 skill）
//! - `get` / `list`: 查询 skill 信息
//! - `reload`: 热重载已注册的 skill（不存在则返回 false）
//! - `len` / `is_empty` / `methods`: 辅助查询

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 技能清单：描述一个 skill 的元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    /// 技能 JSON schema（可选，用于 LLM 工具调用参数校验）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// 技能入口点（如文件路径、端点 URL、函数名）
    pub entrypoint: String,
}

/// 技能注册表：支持注册、查询、热重载
#[derive(Debug, Clone)]
pub struct SkillsRegistry {
    skills: HashMap<String, SkillManifest>,
}

impl SkillsRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// 注册一个 skill。如果同名 skill 已存在则覆盖（返回 true 表示覆盖了旧值）
    pub fn register(&mut self, manifest: SkillManifest) -> bool {
        let name = manifest.name.clone();
        self.skills.insert(name, manifest).is_some()
    }

    /// 按名称查询 skill
    pub fn get(&self, name: &str) -> Option<&SkillManifest> {
        self.skills.get(name)
    }

    /// 列出所有已注册 skill（按名称排序）
    pub fn list(&self) -> Vec<&SkillManifest> {
        let mut v: Vec<_> = self.skills.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// 热重载一个已注册的 skill。如果 skill 不存在则返回 false
    pub fn reload(&mut self, manifest: SkillManifest) -> bool {
        if !self.skills.contains_key(&manifest.name) {
            return false;
        }
        self.skills.insert(manifest.name.clone(), manifest);
        true
    }

    /// 已注册的 skill 数量
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// 注册表是否为空
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// 返回所有已注册 skill 的名称列表
    pub fn methods(&self) -> Vec<String> {
        let mut m: Vec<String> = self.skills.keys().cloned().collect();
        m.sort();
        m
    }
}

impl Default for SkillsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest(name: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: format!("Test skill {}", name),
            schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                }
            })),
            entrypoint: format!("skills/{}.json", name),
        }
    }

    #[test]
    fn test_new_registry_is_empty() {
        let reg = SkillsRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn test_register_and_get() {
        let mut reg = SkillsRegistry::new();
        let manifest = sample_manifest("search");
        assert!(!reg.register(manifest.clone()));
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());

        let got = reg.get("search").unwrap();
        assert_eq!(got.name, "search");
        assert_eq!(got.version, "1.0.0");
    }

    #[test]
    fn test_register_overwrites() {
        let mut reg = SkillsRegistry::new();
        let m1 = sample_manifest("search");
        reg.register(m1);
        let m2 = SkillManifest {
            version: "2.0.0".to_string(),
            ..sample_manifest("search")
        };
        assert!(reg.register(m2)); // returned true means overwrote
        assert_eq!(reg.get("search").unwrap().version, "2.0.0");
    }

    #[test]
    fn test_reload_existing_skill() {
        let mut reg = SkillsRegistry::new();
        reg.register(sample_manifest("search"));
        let updated = SkillManifest {
            version: "1.1.0".to_string(),
            ..sample_manifest("search")
        };
        assert!(reg.reload(updated));
        assert_eq!(reg.get("search").unwrap().version, "1.1.0");
    }

    #[test]
    fn test_reload_nonexistent_skill() {
        let mut reg = SkillsRegistry::new();
        assert!(!reg.reload(sample_manifest("ghost")));
    }

    #[test]
    fn test_list_sorted() {
        let mut reg = SkillsRegistry::new();
        reg.register(sample_manifest("z-final"));
        reg.register(sample_manifest("a-first"));
        reg.register(sample_manifest("m-mid"));
        let list = reg.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "a-first");
        assert_eq!(list[1].name, "m-mid");
        assert_eq!(list[2].name, "z-final");
    }

    #[test]
    fn test_methods_list() {
        let mut reg = SkillsRegistry::new();
        reg.register(sample_manifest("search"));
        reg.register(sample_manifest("compute"));
        let methods = reg.methods();
        assert_eq!(methods, vec!["compute", "search"]);
    }

    #[test]
    fn test_skill_manifest_serialize_roundtrip() {
        let manifest = sample_manifest("test-skill");
        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: SkillManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test-skill");
        assert_eq!(deserialized.version, "1.0.0");
        assert!(deserialized.schema.is_some());
    }

    #[test]
    fn test_skill_manifest_serialize_no_schema() {
        let manifest = SkillManifest {
            name: "simple".into(),
            version: "0.1.0".into(),
            description: "no schema skill".into(),
            schema: None,
            entrypoint: "skills/simple.json".into(),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        // schema 字段应为 null 或不存在（取决于 serde skip_serializing_if 行为）
        // 关键是：反序列化回来时 schema 应为 None
        let deserialized: SkillManifest = serde_json::from_str(&json).unwrap();
        assert!(deserialized.schema.is_none());
    }
}