//! executed-artifact 回查 — 借鉴 yologdev/yoyo-evolve Day 214
//!
//! 「exit 0 不等于记录存在」：任何落盘动作完成后立即回读产物文件本身，
//! 验证它真实存在、非空、且末行可解析，堵住「写入路径报成功但记录缺失」类假成功。
//! 与 journal 的 `verify_roundtrip`（内存态一致回读）互补：本模块只验磁盘上
//! 的产物本体，不依赖内存态——产物被外部篡改/截断/删除时它仍会说真话。

use std::path::Path;

/// 产物回查判定三态（对齐 yoyo #764 Missing/Empty/Corrupt 模式）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactVerdict {
    /// 文件存在、非空、末行可解析为 JSON、行数达下限
    Present,
    /// 文件不存在（读不到 = 假成功铁证）
    Missing,
    /// 文件存在但为空 / 末行不可解析 / 行数不足下限（假成功嫌疑）
    Corrupt,
}

impl ArtifactVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactVerdict::Present => "present",
            ArtifactVerdict::Missing => "missing",
            ArtifactVerdict::Corrupt => "corrupt",
        }
    }
}

/// 一次产物回查的结果：判定 + 实测行数 + 产物路径（证据随判定一起返回）。
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactCheck {
    pub path: String,
    pub verdict: ArtifactVerdict,
    /// 非空行实测行数（文件 Missing 时为 0）
    pub line_count: usize,
}

/// 落盘后回查：文件存在吗？非空吗？末行是合法 JSON 吗？行数达下限吗？
///
/// 空行 / 纯空白行不计入行数；末行取最后一条非空行做 JSON 解析。
pub fn check_artifact(path: &str, min_lines: usize) -> ArtifactCheck {
    let (verdict, line_count) = match std::fs::read_to_string(Path::new(path)) {
        Err(_) => (ArtifactVerdict::Missing, 0),
        Ok(content) => {
            let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
            let n = lines.len();
            let last_parses =
                n > 0 && serde_json::from_str::<serde_json::Value>(lines[n - 1]).is_ok();
            if last_parses && n >= min_lines {
                (ArtifactVerdict::Present, n)
            } else {
                (ArtifactVerdict::Corrupt, n)
            }
        }
    };
    ArtifactCheck {
        path: path.to_string(),
        verdict,
        line_count,
    }
}

#[cfg(test)]
mod artifact_tests {
    use super::*;

    #[test]
    fn test_missing_file_is_missing() {
        let c = check_artifact("/nonexistent/artifact-probe.jsonl", 1);
        assert_eq!(c.verdict, ArtifactVerdict::Missing);
        assert_eq!(c.line_count, 0);
    }

    #[test]
    fn test_empty_file_is_corrupt() {
        let dir = std::env::temp_dir().join("qbody-artifact-empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        let c = check_artifact(path.to_str().unwrap(), 1);
        assert_eq!(c.verdict, ArtifactVerdict::Corrupt);
        assert_eq!(c.line_count, 0);
    }

    #[test]
    fn test_valid_jsonl_is_present_with_count() {
        let dir = std::env::temp_dir().join("qbody-artifact-valid");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("valid.jsonl");
        std::fs::write(&path, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n").unwrap();
        let c = check_artifact(path.to_str().unwrap(), 3);
        assert_eq!(c.verdict, ArtifactVerdict::Present);
        assert_eq!(c.line_count, 3);
    }

    #[test]
    fn test_corrupt_last_line_is_corrupt() {
        let dir = std::env::temp_dir().join("qbody-artifact-corrupt");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("corrupt.jsonl");
        std::fs::write(&path, "{\"a\":1}\nnot-json-at-all\n").unwrap();
        let c = check_artifact(path.to_str().unwrap(), 1);
        assert_eq!(c.verdict, ArtifactVerdict::Corrupt);
        assert_eq!(c.line_count, 2);
    }

    #[test]
    fn test_blank_lines_not_counted() {
        let dir = std::env::temp_dir().join("qbody-artifact-blank");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("blank.jsonl");
        std::fs::write(&path, "\n  \n{\"a\":1}\n\n\t\n").unwrap();
        let c = check_artifact(path.to_str().unwrap(), 1);
        assert_eq!(c.verdict, ArtifactVerdict::Present);
        assert_eq!(c.line_count, 1);
    }

    #[test]
    fn test_min_lines_not_met_is_corrupt() {
        let dir = std::env::temp_dir().join("qbody-artifact-minlines");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("short.jsonl");
        std::fs::write(&path, "{\"a\":1}\n").unwrap();
        let c = check_artifact(path.to_str().unwrap(), 2);
        assert_eq!(c.verdict, ArtifactVerdict::Corrupt);
        assert_eq!(c.line_count, 1);
    }
}
