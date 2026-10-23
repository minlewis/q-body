#!/usr/bin/env python3
"""Unified git branch-state reader for q-body cron scripts.

Consolidates scattered ad-hoc branch assertions (`git branch --show-current` /
`git rev-parse --abbrev-ref HEAD` parsed differently in self-check / nourish /
backlog paths) into ONE helper with explicit unborn / detached / <branch> /
error semantics — yoyo #1072 single shared branch reader.

Exit 0 + stdout prints exactly ONE of:
    <branch>            attached to a named branch with commits
    unborn              attached to refs/heads/<name> but no commits yet
    detached:<sha12>    detached HEAD

Non-zero exit + stderr (nothing on stdout) when the state is NOT readable:
    not-a-git-repo      cwd is not inside a git work tree
    unreadable          git failed unexpectedly

Also supports `--assert <branch>` gate mode: exit 0 iff attached to <branch>,
non-zero + stderr otherwise — the Cron D "branch 归位 gate" that used to live
only as prose in SKILL.md and was never machine-enforced.

Derived from 2026-09-07 feat/issue-106 (git chokepoint hardening) + 2026-10-23
backlog (shared branch reader): one predicate, every door (same-predicate-
every-door, yoyo #1024/#1023).
"""
import subprocess
import sys


def read_branch_state(repo="."):
    """Return (state, error). state is None when error is set."""
    def git(*a):
        return subprocess.run(
            ["git", *a], capture_output=True, text=True, cwd=repo,
            timeout=30)

    r = git("rev-parse", "--is-inside-work-tree")
    if r.returncode != 0 or r.stdout.strip() != "true":
        return None, "not-a-git-repo"

    # Attached branch OR unborn branch: symbolic-ref HEAD resolves to refs/heads/<name>.
    r = git("symbolic-ref", "-q", "HEAD")
    if r.returncode == 0:
        name = r.stdout.strip().replace("refs/heads/", "")
        # unborn when refs/heads/<name> exists (HEAD points at it) but has no commits
        if git("rev-parse", "HEAD").returncode != 0:
            return "unborn", None
        return name, None

    # Detached HEAD: symbolic-ref fails but rev-parse HEAD succeeds.
    r = git("rev-parse", "HEAD")
    if r.returncode == 0:
        return "detached:" + r.stdout.strip()[:12], None

    return None, "unreadable"


def main():
    argv = sys.argv[1:]
    repo = "."
    if argv and argv[0] == "--repo":
        repo = argv[1]
        argv = argv[2:]
    if not argv:
        state, err = read_branch_state(repo)
        if err:
            print(err, file=sys.stderr)
            return 1
        print(state)
        return 0
    if argv[0] == "--assert":
        want = argv[1] if len(argv) > 1 else ""
        state, err = read_branch_state(repo)
        if err:
            print("ABORT: %s" % err, file=sys.stderr)
            return 1
        if state != want:
            print("ABORT: expected HEAD on branch '%s', got '%s'"
                  % (want, state), file=sys.stderr)
            return 1
        print("ok: HEAD on '%s'" % state)
        return 0
    print("usage: git-branch-state.py [--repo PATH] [--assert BRANCH]",
          file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())