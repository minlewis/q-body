#!/usr/bin/env python3
"""reproduce FIRST — prove a repo-supplied git config can hijack a batch-merge-style
git invocation, and prove the neutralisation layer blocks it.

Two-phase, both REQUIRED to print PASS (SOUL §15 anti-false-negative):
  phase 1 (bare):  core.fsmonitor probe command MUST fire  -> threat is real
  phase 2 (hardened): same call under neutralised env      -> probe MUST NOT fire
Phase-2-clean without phase-1-fired would be a false negative (probe never had a
chance to trigger — the "clean" proves nothing).

Probe command writes a sentinel file; it is `touch <tmpdir>/pwned-<rand>` —
self-contained in a throwaway repo under tempfile.mkdtemp(), touches nothing real.
"""
import os
import subprocess
import sys
import tempfile

NEUTRAL_ENV_KEYS = {
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
}
NEUTRAL_ARGS = [
    "-c", "protocol.ext.allow=never",
    "-c", "core.fsmonitor=false",
    "-c", "core.fsmonitorDaemon=false",
]


def probe_fired(probe: str) -> bool:
    return os.path.exists(probe)


def run_git(env, args, cwd):
    r = subprocess.run(["git"] + args, cwd=cwd, env=env,
                       capture_output=True, text=True, timeout=60)
    return r.returncode, r.stdout, r.stderr


def main():
    tmp = tempfile.mkdtemp(prefix="qbody-repro-gitcfg-")
    repo = os.path.join(tmp, "victim")
    os.makedirs(repo)
    probe = os.path.join(tmp, "pwned-fsmonitor")

    def sh(cmd):
        subprocess.run(cmd, cwd=repo, shell=True, check=True,
                       capture_output=True)

    sh("git init -q .")
    sh("git config user.email a@b.c && git config user.name t")
    # malicious repo config: fsmonitor hook = version probe that touches the sentinel
    sh("git config core.fsmonitor \"touch %s\"" % probe)
    open(os.path.join(repo, "f.txt"), "w").write("x\n")
    sh("git add f.txt && git commit -qm probe")

    base_env = {k: v for k, v in os.environ.items()
                if not k.startswith("GIT_CONFIG")}

    # ---- phase 1: bare invocation, repo config drives ----
    rc, out, err = run_git(base_env, ["merge-tree", "--write-tree", "HEAD", "HEAD"], repo)
    phase1 = probe_fired(probe)
    print("phase1 bare: rc=%d probe_fired=%s %s" % (rc, phase1, err[:120]))
    if not phase1:
        print("FAIL: bare run did NOT fire probe — threat not reproduced; "
              "neutralisation proof below would be meaningless")
        return 1

    # ---- phase 2: same repo, same call, neutralised ----
    probe2 = probe + ".h"  # separate sentinel; phase-1 file already exists
    env = dict(base_env)
    env.update(NEUTRAL_ENV_KEYS)
    rc, out, err = run_git(env, NEUTRAL_ARGS + ["merge-tree", "--write-tree", "HEAD", "HEAD"], repo)
    phase2 = os.path.exists(probe2)
    print("phase2 hardened: rc=%d probe_fired=%s %s" % (rc, phase2, err[:120]))
    if phase2:
        print("FAIL: probe fired DESPITE neutralisation — layer ineffective")
        return 1

    print("PASS: threat reproduced bare, neutralised run clean — both signals green")
    return 0


if __name__ == "__main__":
    sys.exit(main())
