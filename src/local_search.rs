//! Local-first MCP 搜索模块 — 参考 modelcontextprotocol/specification 的 Resources 模型
//!
//! MCP 的 Resources 定义了一种 local-first 的资源发现模式：
//! server 声明可用资源列表（URI + name + description），
//! client 通过 resources/list 和 resources/read 访问。
//!
//! 借鉴：modelcontextprotocol/specification — MCP Resources 模型
//!
//! 类型层准备，handler.rs 运行时接线按既定先例推迟。

use std::path::Path;

/// 搜索条目
#[derive(Debug, Clone, PartialEq)]
pub struct SearchEntry {
    /// 资源 URI（如 file:///home/ubuntu/q-body/src/main.rs）
    pub uri: String,
    /// 资源名称（文件名）
    pub name: String,
    /// 资源描述
    pub description: String,
    /// 文件路径
    pub path: String,
}

/// 搜索索引结果
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub entries: Vec<SearchEntry>,
    pub total: usize,
}

/// 本地文件搜索索引 — 以 MCP Resource 语义暴露本地文件搜索能力
#[derive(Debug, Clone)]
pub struct LocalSearchIndex {
    entries: Vec<SearchEntry>,
}

impl Default for LocalSearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalSearchIndex {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 索引一个文件，以 MCP Resource 语义记录
    pub fn index_file(&mut self, path: &str) -> Result<(), String> {
        let p = Path::new(path);
        if !p.exists() {
            return Err(format!("file not found: {}", path));
        }
        if !p.is_file() {
            return Err(format!("not a file: {}", path));
        }

        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());

        let uri = format!("file://{}", p.canonicalize().map_err(|e| e.to_string())?.display());

        // 跳过已存在的同名条目
        if self.entries.iter().any(|e| e.uri == uri) {
            return Ok(());
        }

        self.entries.push(SearchEntry {
            uri,
            name,
            description: format!("Local file: {}", path),
            path: path.to_string(),
        });

        Ok(())
    }

    /// 按关键词搜索本地文件
    pub fn search(&self, keyword: &str) -> SearchResult {
        let kw = keyword.to_lowercase();
        let matched: Vec<SearchEntry> = self
            .entries
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&kw)
                    || e.path.to_lowercase().contains(&kw)
                    || e.description.to_lowercase().contains(&kw)
            })
            .cloned()
            .collect();
        let total = matched.len();
        SearchResult {
            entries: matched,
            total,
        }
    }

    /// 列出所有已索引资源（对应 MCP resources/list）
    pub fn list_resources(&self) -> Vec<&SearchEntry> {
        self.entries.iter().collect()
    }

    /// 索引数量
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

    #[test]
    fn test_new_index_is_empty() {
        let idx = LocalSearchIndex::new();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_index_file_not_found() {
        let mut idx = LocalSearchIndex::new();
        let result = idx.index_file("/nonexistent/path.rs");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_index_file_and_list() {
        // Use a real file that exists
        let mut idx = LocalSearchIndex::new();
        let result = idx.index_file("src/main.rs");
        assert!(result.is_ok(), "indexing main.rs should succeed: {:?}", result);
        assert_eq!(idx.len(), 1);

        let resources = idx.list_resources();
        assert_eq!(resources.len(), 1);
        assert!(resources[0].name.contains("main.rs"));
        assert!(resources[0].uri.starts_with("file://"));
    }

    #[test]
    fn test_search_by_name() {
        let mut idx = LocalSearchIndex::new();
        let _ = idx.index_file("src/main.rs");
        let _ = idx.index_file("src/handler.rs");

        let result = idx.search("handler");
        assert_eq!(result.total, 1);
        assert!(result.entries[0].name.contains("handler"));
    }

    #[test]
    fn test_search_no_match() {
        let idx = LocalSearchIndex::new();
        let result = idx.search("nonexistent_keyword");
        assert_eq!(result.total, 0);
        assert!(result.entries.is_empty());
    }

    #[test]
    fn test_duplicate_index_skipped() {
        let mut idx = LocalSearchIndex::new();
        let _ = idx.index_file("src/main.rs");
        let _ = idx.index_file("src/main.rs");
        assert_eq!(idx.len(), 1);
    }
}