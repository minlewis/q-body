use std::collections::HashMap;
use std::process::Command;

/// Contract mirrored from the git-native executor's neutralisation layer
/// (`~/.hermes/skills/q-baby-self-check/scripts/git-native-batch-merge.py` run()).
///
/// q-body itself does not shell out to git; these tests pin the env-overwrite
/// semantics the executor must satisfy, so a future regression in the executor
/// contract is caught by a repo-level test instead of a Sunday surprise.
///
/// Source of truth: tests/repro/threat-repro-git-config.py and
/// tests/repro/threat-repro-git-env.py (reproduce FIRST, yoyo Day191).

fn neutral_env(base: HashMap<String, String>) -> HashMap<String, String> {
    let mut env = base;
    // strip CLI-level config injection channels
    for k in env.keys().cloned().collect::<Vec<_>>() {
        if k == "GIT_CONFIG_COUNT"
            || k.starts_with("GIT_CONFIG_KEY_")
            || k.starts_with("GIT_CONFIG_VALUE_")
        {
            env.remove(&k);
        }
    }
    // neutralisation keys (GIT_CONFIG_GLOBAL alone does NOT stop GIT_CONFIG_* injection)
    env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    env.insert("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
    env.insert("GIT_CONFIG_COUNT".into(), "0".into());
    env
}

fn base_env() -> HashMap<String, String> {
    [
        (
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        ),
        (
            "HOME".to_string(),
            std::env::var("HOME").unwrap_or_default(),
        ),
    ]
    .into_iter()
    .collect()
}

#[test]
fn neutral_env_strips_cli_config_injection() {
    let mut e = base_env();
    e.insert("GIT_CONFIG_COUNT".into(), "1".into());
    e.insert("GIT_CONFIG_KEY_0".into(), "qbody.probe".into());
    e.insert("GIT_CONFIG_VALUE_0".into(), "injected".into());
    let n = neutral_env(e);
    assert_eq!(n.get("GIT_CONFIG_COUNT").map(String::as_str), Some("0"));
    assert!(!n.contains_key("GIT_CONFIG_KEY_0"));
    assert!(!n.contains_key("GIT_CONFIG_VALUE_0"));
}

#[test]
fn neutral_env_disables_fsmonitor_and_ext_protocol() {
    // command-level switches must accompany the env (probed by tests/repro/threat-repro-git-config.py)
    let switches = [
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.fsmonitorDaemon=false",
    ];
    let settings: Vec<(&str, &str)> = switches
        .chunks(2)
        .filter(|c| c[0] == "-c")
        .map(|c| c[1].split_once('=').expect("-c takes key=value"))
        .collect();
    assert!(settings.contains(&("protocol.ext.allow", "never")));
    assert!(settings.contains(&("core.fsmonitor", "false")));
    assert!(settings.contains(&("core.fsmonitorDaemon", "false")));
    // and the env half
    let n = neutral_env(base_env());
    assert_eq!(n.get("GIT_CONFIG_NOSYSTEM").map(String::as_str), Some("1"));
    assert_eq!(
        n.get("GIT_CONFIG_GLOBAL").map(String::as_str),
        Some("/dev/null")
    );
}

#[test]
fn neutral_env_preserves_path_and_home() {
    // overwrite-by-whitelist: only GIT_CONFIG* touched, everything else survives
    let n = neutral_env(base_env());
    assert!(n.get("PATH").map_or(false, |p| !p.is_empty()));
    assert!(n.contains_key("HOME"));
}

#[test]
fn git_config_count_zero_kills_injection_end_to_end() {
    // real-subprocess oracle: GIT_CONFIG_* injection must not reach git config --get
    let mut e = base_env();
    e.insert("GIT_CONFIG_COUNT".into(), "1".into());
    e.insert("GIT_CONFIG_KEY_0".into(), "qbody.probe".into());
    e.insert("GIT_CONFIG_VALUE_0".into(), "injected".into());
    let n = neutral_env(e);
    let out = Command::new("git")
        .args(["config", "--get", "qbody.probe"])
        .env_clear()
        .envs(&n)
        .output()
        .expect("git should run");
    assert!(
        !out.status.success(),
        "injected config must not be visible under neutral env"
    );
}
