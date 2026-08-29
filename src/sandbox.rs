//! Task execution sandbox — q-body 的 A2A task 执行隔离层
//!
//! # 设计
//!
//! 参考 CubeSandbox 的 Rust 沙箱设计，给 q-body 的 A2A task handler
//! 增加执行隔离层，防止恶意 task 或 bug 影响宿主进程。
//!
//! 当前为类型层准备（SandboxConfig / SandboxGuard），
//! 实际 pid namespace / cgroup / tempfs 隔离接线按既定先例推迟。
//!
//! # 借鉴
//!
//! CubeSandbox — Rust sandbox design for safe code execution.
//! CubeSandbox 使用 pid namespaces + cgroup + tempfs 隔离不可信代码执行，
//! 防止其对宿主进程造成影响。q-body 对应：SandboxConfig 配置 + SandboxGuard
//! 生命周期管理 + prepare() 创建临时工作目录。

use std::path::{Path, PathBuf};

/// 沙箱配置
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// 是否启用 pid namespace 隔离
    pub pid_namespace: bool,
    /// 是否启用 cgroup 资源限制
    pub cgroup: bool,
    /// 是否使用临时文件系统
    pub temp_fs: bool,
    /// 临时工作目录前缀
    pub temp_dir_prefix: String,
    /// 超时秒数
    pub timeout_secs: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            pid_namespace: false,
            cgroup: false,
            temp_fs: true,
            temp_dir_prefix: "q-body-sandbox-".into(),
            timeout_secs: 300,
        }
    }
}

impl SandboxConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// 启用 pid namespace 隔离
    pub fn with_pid_namespace(mut self) -> Self {
        self.pid_namespace = true;
        self
    }

    /// 启用 cgroup 资源限制
    pub fn with_cgroup(mut self) -> Self {
        self.cgroup = true;
        self
    }

    /// 设置超时秒数
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// 沙箱守卫 — 持有沙箱生命周期，Drop 时自动清理
#[derive(Debug)]
pub struct SandboxGuard {
    /// 临时工作目录
    temp_dir: Option<tempfile::TempDir>,
    /// 沙箱配置
    config: SandboxConfig,
}

impl SandboxGuard {
    /// 临时工作目录路径
    pub fn work_dir(&self) -> Option<&std::path::Path> {
        self.temp_dir.as_ref().map(|d| d.path())
    }

    /// 沙箱配置引用
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// 临时工作目录是否已通过逃逸 pre-flight 检查
    ///
    /// 返回 true 表示 `work_dir()` 经 `validate_work_dir` 校验未逃出允许根。
    /// tempdir 由本进程在系统临时目录下创建，canonicalize 后仍位于同一
    /// 前缀之下，视为安全。
    pub fn work_dir_escape_checked(&self) -> bool {
        match self.work_dir() {
            Some(dir) => {
                let root = std::env::temp_dir();
                validate_work_dir(dir, &root).is_ok()
            }
            None => false,
        }
    }
}

/// 路径逃逸 pre-flight 检查
///
/// 借鉴：yologdev/yoyo-evolve — Day 143 Task 3（/spawn worktree symlink 逃逸
/// pre-flight）。yoyo 在 worktree 创建前对 parent 路径做 canonicalize 解析，
/// 拒绝解析后落在 repo 外的路径。→ q-body 对应：对传入的 work_dir 做
/// `std::fs::canonicalize()` 解析 symlink，再 `starts_with(allowed_root)`
/// 前缀校验，不通过返回 Err("path escapes allowlist")。
///
/// 运行时接线（task 提交时强制调用）属架构级改动，按既定先例推迟。
pub fn validate_work_dir(path: &Path, allowed_root: &Path) -> Result<PathBuf, String> {
    let canonical_root = std::fs::canonicalize(allowed_root)
        .map_err(|e| format!("Failed to canonicalize allowed root: {}", e))?;
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|e| format!("Failed to canonicalize work dir: {}", e))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("path escapes allowlist".to_string());
    }
    Ok(canonical_path)
}

/// 准备沙箱执行环境
///
/// 创建一个临时工作目录作为隔离沙箱的基础。
/// 返回 SandboxGuard，Drop 时自动清理临时目录。
pub fn prepare(config: SandboxConfig) -> Result<SandboxGuard, String> {
    let temp_dir = tempfile::Builder::new()
        .prefix(&config.temp_dir_prefix)
        .tempdir()
        .map_err(|e| format!("Failed to create sandbox temp dir: {}", e))?;

    Ok(SandboxGuard {
        temp_dir: Some(temp_dir),
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_config_defaults() {
        let config = SandboxConfig::new();
        assert!(!config.pid_namespace);
        assert!(!config.cgroup);
        assert!(config.temp_fs);
        assert_eq!(config.timeout_secs, 300);
        assert!(config.temp_dir_prefix.contains("q-body-sandbox"));
    }

    #[test]
    fn test_sandbox_config_with_options() {
        let config = SandboxConfig::new()
            .with_pid_namespace()
            .with_cgroup()
            .with_timeout(600);
        assert!(config.pid_namespace);
        assert!(config.cgroup);
        assert_eq!(config.timeout_secs, 600);
    }

    #[test]
    fn test_prepare_creates_guard() {
        let config = SandboxConfig::new();
        let guard = prepare(config).unwrap();
        // Guard should have a work dir
        assert!(guard.work_dir().is_some());
        assert!(guard.work_dir().unwrap().exists());
        assert_eq!(guard.config().timeout_secs, 300);
    }

    #[test]
    fn test_guard_drop_cleans_temp_dir() {
        let config = SandboxConfig::new();
        let temp_path;

        {
            let guard = prepare(config).unwrap();
            temp_path = guard.work_dir().unwrap().to_path_buf();
            assert!(temp_path.exists());
        }
        // Guard dropped, temp dir should be cleaned
        assert!(!temp_path.exists());
    }

    #[test]
    fn test_prepare_custom_prefix() {
        let config = SandboxConfig {
            temp_dir_prefix: "custom-test-".into(),
            ..SandboxConfig::default()
        };
        let guard = prepare(config).unwrap();
        let dir_name = guard.work_dir().unwrap().to_string_lossy().to_string();
        // tempfile prefix ensures the dir starts with the prefix
        assert!(std::path::Path::new(&dir_name).exists());
    }

    #[test]
    fn test_validate_work_dir_allows_normal_path() {
        let root = std::env::temp_dir();
        let dir = tempfile::Builder::new()
            .prefix("q-body-validate-ok-")
            .tempdir()
            .unwrap();
        let result = validate_work_dir(dir.path(), &root);
        assert!(result.is_ok());
        assert!(
            result
                .unwrap()
                .starts_with(std::fs::canonicalize(&root).unwrap())
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_validate_work_dir_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = tempfile::Builder::new()
            .prefix("q-body-allowed-root-")
            .tempdir()
            .unwrap();
        // 逃逸目标：系统临时目录（在 allowed_root 之外）
        let escape_target = std::env::temp_dir();
        let link_path = root.path().join("evil-link");
        symlink(&escape_target, &link_path).unwrap();
        // link 本体在 root 内，但解析后指向 root 外 → 必须拒绝
        let result = validate_work_dir(&link_path, root.path());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "path escapes allowlist");
    }

    #[test]
    fn test_validate_work_dir_rejects_outside_path() {
        let root = tempfile::Builder::new()
            .prefix("q-body-allowed-root-")
            .tempdir()
            .unwrap();
        // 直接用 root 外的路径（不经过 symlink）
        let outside = std::env::temp_dir();
        let result = validate_work_dir(&outside, root.path());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "path escapes allowlist");
    }

    #[test]
    fn test_guard_work_dir_escape_checked() {
        let config = SandboxConfig::new();
        let guard = prepare(config).unwrap();
        // 沙箱 tempdir 创建在系统临时目录下，pre-flight 应通过
        assert!(guard.work_dir_escape_checked());
    }
}
