#!/usr/bin/env python3
"""reproduce FIRST — prove GIT_CONFIG_COUNT/KEY/VALUE env can inject arbitrary git
config into a batch-merge-style invocation, and prove the neutralisation layer
strips it (same two-phase contract as threat-repro-git-config.py).

Uses `git config --get <key>` as the oracle: injected alias/config must be visible
bare, invisible under the neutral env. Self-contained temp repo.
"""
import os
import subprocess
import sys
import tempfile

NEUTRAL_ENV_KEYS = {
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    # GIT_CONFIG_GLOBAL does NOT stop CLI-level env injection — must zero the count
    # (learned from this repro's own first run: injection survived /dev/null alone)
    "GIT_CONFIG_COUNT": "0",
}


def run_git(env, args, cwd):
    r = subprocess.run(["git"] + args, cwd=cwd, env=env,
                       capture_output=True, text=True, timeout=60)
    return r.returncode, r.stdout.strip(), r.stderr.strip()


def main():
    tmp = tempfile.mkdtemp(prefix="qbody-repro-gitenv-")
    repo = os.path.join(tmp, "victim")
    os.makedirs(repo)
    subprocess.run(["git", "init", "-q", "."], cwd=repo, check=True)

    base_env = {k: v for k, v in os.environ.items()
                if not k.startswith("GIT_CONFIG")}

    # malicious caller env: CLI-level config injection
    poisoned = dict(base_env)
    poisoned.update({
        "GIT_CONFIG_COUNT": "1",
        "GIT_CONFIG_KEY_0": "qbody.probe",
        "GIT_CONFIG_VALUE_0": "injected",
    })

    # ---- phase 1: bare-ish (only poisoned env), injection visible ----
    rc, out, err = run_git(poisoned, ["config", "--get", "qbody.probe"], repo)
    phase1 = (rc == 0 and out == "injected")
    print("phase1 poisoned env: rc=%d value=%r" % (rc, out))
    if not phase1:
        print("FAIL: GIT_CONFIG_* injection did not take effect — threat not reproduced")
        return 1

    # ---- phase 2: poisoned env + neutralisation ----
    env = dict(poisoned)
    env.update(NEUTRAL_ENV_KEYS)
    rc, out, err = run_git(env, ["config", "--get", "qbody.probe"], repo)
    phase2 = (rc == 0)
    print("phase2 neutralised: rc=%d value=%r" % (rc, out))
    if phase2:
        print("FAIL: injected config survived neutralisation")
        return 1

    print("PASS: env injection reproduced, neutralised clean — both signals green")
    return 0


if __name__ == "__main__":
    sys.exit(main())
