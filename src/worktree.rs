//! 派生前脏工作树探测（backlog 2026-10-20；借鉴 yoyo-evolve #982 spawn 护栏）
//!
//! 背景：q-body 未来在 `delegate_task` 派生子代理前，必须先探测目标工作树
//! 是否脏（有未提交改动）。若脏仍派发，子代理可能在不知道存量改动的情况下
//! 覆盖/强删/丢活——yoyo #982 的 spawn 护栏语义：探测到未提交工作即中止派发
//! 并落账。本模块是纯函数探测层（可测、无 IO 副作用），未来接线点在
//! `delegate_task` 派生前调用 `probe_worktree_dirty()`，脏则中止 + 落 journal。
//!
//! 判定（对齐 health.rs EvidenceSource 的 Measured/Unconfigured/Unavailable，
//! 以及 09-16 §22「修复不是资产，检查才是」：探测不能静默降级为假干净）：
//!   - Clean             = 干净工作树（Measured：git status 真实跑过，零未提交）
//!   - Dirty{ n }        = 有 n 个未提交条目（Measured：真实计数）
//!   - GitUnavailable    = git 二进制缺失 / git 命令失败 / 非 git 仓库（Unavailable：
//!                         无法测量，按保守语义中止派发——宁可误报不可在未知态放行脏树）
//!
//! `should_block_delegation()` 是对外唯一判定口：仅 Dirty 与 GitUnavailable 中止
//! 派发（Clean 放行）。测试从「输入为 clean/dirty 必须被尊重」两个方向钉死
//! （负路径不写假绿，参照 10-19 env_keys 变异验证纪律）。

use std::path::Path;
use std::process::Command;

/// 脏工作树探测判定
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyVerdict {
    /// 干净工作树：git status --porcelain 空输出
    Clean,
    /// 有未提交条目（含 modified/untracked/staged/deleted），n = 条目数
    Dirty { n: usize },
    /// 探测失败（git 缺失 / git 报错 / 非 git 仓库）：无法测量，保守按脏处置
    GitUnavailable,
}

impl DirtyVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            DirtyVerdict::Clean => "clean",
            DirtyVerdict::Dirty { .. } => "dirty",
            DirtyVerdict::GitUnavailable => "git_unavailable",
        }
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self, DirtyVerdict::Dirty { .. })
    }
}

/// 判定是否应中止派发（对外唯一判定口）。
///
/// 保守语义：只有明确测到 Clean 才放行；Dirty 与 GitUnavailable 都中止——
/// GitUnavailable 是「未能测量」（Unavailable），在能证明干净之前不放行子代理
/// 触碰工作树（对齐 yoyo #982 fail-closed 护栏）。
pub fn should_block_delegation(v: &DirtyVerdict) -> bool {
    match v {
        DirtyVerdict::Clean => false,
        DirtyVerdict::Dirty { .. } | DirtyVerdict::GitUnavailable => true,
    }
}

/// 判断目录是否是一个 git 工作树（存在 .git 目录或 .git 文件，submodule 情形）
pub fn is_git_repo(dir: &Path) -> bool {
    let git_marker = dir.join(".git");
    git_marker.is_dir() || git_marker.is_file()
}

/// 探测工作树脏状态（纯函数：内部用 std::process::Command 跑只读 git status）
///
/// 非 git 仓库目录 → GitUnavailable（保守按脏处置）。依赖外层 PATH 的 git 二进制。
pub fn probe_worktree_dirty(dir: &Path) -> DirtyVerdict {
    if !is_git_repo(dir) {
        return DirtyVerdict::GitUnavailable;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let n = String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();
            if n == 0 {
                DirtyVerdict::Clean
            } else {
                DirtyVerdict::Dirty { n }
            }
        }
        // git 二进制缺失 / git 报错 → Unavailable，保守按脏
        _ => DirtyVerdict::GitUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TMP_SEQ: AtomicU32 = AtomicU32::new(0);

    /// 自建临时 git 仓库（不引 tempfile，规避 TAO 依赖硬线）
    /// TempDir 封装：Drop 时清理，测完不留脏目录。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("qbody-wt-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).expect("create tempdir");
            TempDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tmp_git_repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        let ok = Command::new("git")
            .arg("init")
            .arg("-q")
            .arg("-b")
            .arg("main")
            .arg(dir.path())
            .status()
            .expect("git init");
        assert!(ok.success(), "git init failed for {name}");
        dir
    }

    #[test]
    fn clean_repo_reports_clean() {
        let dir = tmp_git_repo("clean");
        let v = probe_worktree_dirty(dir.path());
        assert_eq!(v, DirtyVerdict::Clean);
        assert!(!should_block_delegation(&v));
    }

    #[test]
    fn dirty_repo_reports_count_and_blocks() {
        let dir = tmp_git_repo("dirty");
        fs::write(dir.path().join("untracked.txt"), "x").unwrap();
        let v = probe_worktree_dirty(dir.path());
        assert!(v.is_dirty());
        assert!(should_block_delegation(&v));
        match v {
            DirtyVerdict::Dirty { n } => assert_eq!(n, 1),
            other => panic!("expected Dirty{{n:1}}, got {other:?}"),
        }
    }

    #[test]
    fn staged_modified_and_untracked_all_counted() {
        let dir = tmp_git_repo("mixed");
        // 三个来源都算脏：staged 新增 + 已跟踪文件修改 + untracked
        fs::write(dir.path().join("a.txt"), "v1").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "a.txt"])
            .status()
            .unwrap();
        fs::write(dir.path().join("a.txt"), "v2").unwrap(); // 修改已 staged 文件
        fs::write(dir.path().join("b.txt"), "untracked").unwrap(); // untracked
        let v = probe_worktree_dirty(dir.path());
        match v {
            DirtyVerdict::Dirty { n } => assert!(n >= 2, "expected >=2, got {n}"),
            other => panic!("expected Dirty, got {other:?}"),
        }
        assert!(should_block_delegation(&v));
    }

    #[test]
    fn non_repo_dir_blocks_conservatively() {
        let dir = TempDir::new("plain");
        let v = probe_worktree_dirty(dir.path());
        // 非 git 仓库：is_git_repo 前置失败 → GitUnavailable（保守按脏阻止，
        // 在能证明是干净 git 工作树前不放行子代理触碰）
        assert_eq!(v, DirtyVerdict::GitUnavailable);
        assert!(should_block_delegation(&v));
    }

    #[test]
    fn missing_dir_does_not_false_green() {
        // §22 纪律：证明 dirty 判定不是假绿——目录不存在时 git 会失败，
        // 但必须报 Unavailable（阻止）而非假 Clean
        let dir = tmp_git_repo("adan");
        let missing = dir.path().join("does-not-exist");
        let v = probe_worktree_dirty(&missing);
        assert_eq!(v, DirtyVerdict::GitUnavailable);
        assert!(should_block_delegation(&v));
    }

    #[test]
    fn is_git_repo_true_after_init() {
        let dir = tmp_git_repo("margin");
        assert!(is_git_repo(dir.path()));
        let plain = TempDir::new("plain2");
        assert!(!is_git_repo(plain.path()));
    }
}