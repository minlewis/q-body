//! Task Execution Sandbox — 类型层准备
//!
//! 借鉴：TencentCloud/CubeSandbox — Rust 沙箱设计
//! CubeSandbox 是腾讯开源的 AI Agent 沙箱（9771★），提供即时、并发、安全、轻量的
//! task 执行隔离层。q-body 的 A2A task handler 需要执行隔离层，防止恶意 task 或 bug
//! 影响宿主进程。
//!
//! 当前层是类型准备：定义 SandboxConfig / SandboxResult / Sandbox 类型，
//! 运行时 pid namespace / cgroup / temp fs 实际隔离接线按既定先例推迟。

use std::time::Duration;

/// 沙箱执行配置
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// 执行超时（超过则 TimedOut）
    pub timeout: Duration,
    /// 临时工作目录（None = 系统默认临时目录）
    pub work_dir: Option<String>,
    /// 最大内存限制（字节，None = 不限制）
    pub max_memory_bytes: Option<u64>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            work_dir: None,
            max_memory_bytes: None,
        }
    }
}

/// 沙箱执行结果
#[derive(Debug, Clone, PartialEq)]
pub enum SandboxResult {
    /// 正常完成，含 stdout
    Completed { stdout: String, exit_code: i32 },
    /// 超时
    TimedOut,
    /// 被外部终止
    Killed,
    /// 执行出错（命令不存在、权限不足等）
    Error { message: String },
}

impl SandboxResult {
    /// 是否成功完成
    pub fn is_ok(&self) -> bool {
        matches!(self, SandboxResult::Completed { exit_code: 0, .. })
    }
}

/// 沙箱执行器
#[derive(Debug, Clone)]
pub struct Sandbox {
    config: SandboxConfig,
}

impl Sandbox {
    /// 创建新沙箱
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// 返回当前配置
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// 执行命令（当前为同步模拟，实际隔离接线推迟）
    ///
    /// 此方法在当前版本中直接调用 std::process::Command 执行；
    /// pid namespace / cgroup / temp fs 等实际隔离将在后续架构级实现中补充。
    #[cfg(not(tarpaulin_include))]
    pub fn execute(&self, cmd: &str, args: &[&str]) -> SandboxResult {
        let timeout = self.config.timeout;
        let mut child = match std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return SandboxResult::Error {
                    message: format!("failed to spawn '{}': {}", cmd, e),
                };
            }
        };

        // 等待子进程，带超时
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() >= timeout {
                        let _ = child.kill();
                        return SandboxResult::TimedOut;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return SandboxResult::Error {
                        message: format!("wait error: {}", e),
                    };
                }
            }
        };

        let output = child.wait_with_output().ok();
        let stdout = output
            .as_ref()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        SandboxResult::Completed {
            stdout,
            exit_code: status.code().unwrap_or(-1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_default_config() {
        let config = SandboxConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert!(config.work_dir.is_none());
        assert!(config.max_memory_bytes.is_none());
    }

    #[test]
    fn test_sandbox_custom_config() {
        let config = SandboxConfig {
            timeout: Duration::from_secs(60),
            work_dir: Some("/tmp/sandbox".to_string()),
            max_memory_bytes: Some(512 * 1024 * 1024),
        };
        let sandbox = Sandbox::new(config);
        assert_eq!(sandbox.config().timeout, Duration::from_secs(60));
        assert_eq!(
            sandbox.config().work_dir.as_deref(),
            Some("/tmp/sandbox")
        );
        assert_eq!(sandbox.config().max_memory_bytes, Some(512 * 1024 * 1024));
    }

    #[test]
    fn test_sandbox_result_is_ok() {
        assert!(SandboxResult::Completed {
            stdout: "ok".into(),
            exit_code: 0
        }
        .is_ok());

        assert!(!SandboxResult::Completed {
            stdout: "".into(),
            exit_code: 1
        }
        .is_ok());

        assert!(!SandboxResult::TimedOut.is_ok());
        assert!(!SandboxResult::Killed.is_ok());
        assert!(!SandboxResult::Error {
            message: "fail".into()
        }
        .is_ok());
    }

    #[test]
    fn test_sandbox_execute_success() {
        let sandbox = Sandbox::new(SandboxConfig::default());
        let result = sandbox.execute("echo", &["hello sandbox"]);
        assert!(result.is_ok());
        if let SandboxResult::Completed { stdout, exit_code } = &result {
            assert_eq!(*exit_code, 0);
            assert!(stdout.contains("hello sandbox"));
        }
    }

    #[test]
    fn test_sandbox_execute_not_found() {
        let sandbox = Sandbox::new(SandboxConfig::default());
        let result = sandbox.execute("nonexistent_cmd_xyz", &[]);
        match &result {
            SandboxResult::Error { message } => {
                assert!(message.contains("nonexistent_cmd_xyz"));
            }
            _ => panic!("expected Error variant"),
        }
    }
}