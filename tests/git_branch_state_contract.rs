//! Contract tests for scripts/git-branch-state.py (10-23 backlog: shared branch reader).
//!
//! These are **real-process** tests: they spawn the actual script against a
//! scratch git repo and assert byte-level semantics, so added modes (unborn /
//! detached) are proven reachable — not a copy-paste of the implementation
//! (SOUL §15 three-signal: current-clean + failure-path-walked + source==state).
//! Derives from 2026-09-07 threat-repro git config precedent (reproduce FIRST,
//! real deviation, not mocks).

use std::fs;
use std::path::Path;
use std::process::Command;

fn scratch_repo(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "git-branch-state-contract-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .expect("git spawn");
    assert!(out.status.success(), "git {:?} failed: {:?}", args, out);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run_script(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/git-branch-state.py");
    let out = Command::new("python3")
        .arg(&script)
        .arg("--repo")
        .arg(dir)
        .args(args)
        .output()
        .expect("python3 spawn");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

#[test]
fn attached_branch_reports_name() {
    let dir = scratch_repo("attached");
    git(&dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "c1"]);
    let (ok, out, _) = run_script(&dir, &[]);
    assert!(ok && out == "main", "got ok={} out={:?}", ok, out);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn unborn_repo_reports_unborn() {
    let dir = scratch_repo("unborn");
    git(&dir, &["init", "-q", "-b", "main"]); // HEAD → refs/heads/main, no commits
    let (ok, out, _) = run_script(&dir, &[]);
    assert!(ok && out == "unborn", "got ok={} out={:?}", ok, out);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn detached_head_reports_sha() {
    let dir = scratch_repo("detached");
    git(&dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "c1"]);
    git(&dir, &["checkout", "--detach", "HEAD"]); // move HEAD to the commit itself
    let (ok, out, _) = run_script(&dir, &[]);
    assert!(ok, "detached should be readable");
    assert!(out.starts_with("detached:"), "got {:?}", out);
    assert_eq!(out.len(), "detached:".len() + 12, "short sha, got {:?}", out);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn non_repo_is_not_a_false_clean() {
    let dir = scratch_repo("nonrepo").join("inner"); // NOT a git repo
    fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = run_script(&dir, &[]);
    assert!(!ok, "should fail closed outside a repo");
    assert!(out.is_empty(), "no stdout on error, got {:?}", out);
    assert!(err.contains("not-a-git-repo"), "got {:?}", err);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn assert_passes_only_on_expected_branch() {
    let dir = scratch_repo("assert");
    git(&dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "c1"]);
    // match
    let (ok, out, _) = run_script(&dir, &["--assert", "main"]);
    assert!(ok && out.starts_with("ok:"), "got {:?}", out);
    // mismatch → non-zero
    let (ok, _, err) = run_script(&dir, &["--assert", "feat/nope"]);
    assert!(!ok, "assert must fail on wrong branch");
    assert!(err.contains("expected HEAD on branch 'feat/nope'"), "got {:?}", err);
    fs::remove_dir_all(&dir).ok();
}