//! cron 状态写回路径保护 — 借鉴 yologdev/yoyo-evolve #1216
//!
//! 「损坏记忆文件保护」：cron 写 `yoyo_stats.json` / `nourish_signals.json`
//! 等状态文件前必须先 parse 现有文件（JSON 可解析才算健康）；文件损坏
//! （半写/截断/乱码）时保留 `.bak` 备份 + 显式报警（由接线方打日志），
//! **不**静默覆盖——上一周期数据是唯一证据，一旦被覆盖即不可恢复。
//!
//! 与 SOUL §15 假阴性陷阱同构：写回成功 ≠ 数据没丢；只有 parse 过旧文件
//! 才能证明「覆盖没杀死上一周期」。

use std::path::Path;

/// 写前检查的状态文件判定三态（对齐 yoyo #764 Missing/Empty/Corrupt 模式）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateFileVerdict {
    /// 存在且可解析为 JSON — 健康，直接覆盖不丢数据
    Ok,
    /// 不存在 — 首次写入，无上一周期数据可丢，直接覆盖
    Missing,
    /// 存在但不可解析为 JSON — 唯一证据在损坏文件里，必须备份后才可覆盖
    Corrupt,
}

impl StateFileVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            StateFileVerdict::Ok => "ok",
            StateFileVerdict::Missing => "missing",
            StateFileVerdict::Corrupt => "corrupt",
        }
    }
}

/// 写前准备的裁决：允许覆盖？还是必须先备份？
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteAction {
    /// 健康或首次写入 — 无数据可丢，直接覆盖
    SafeOverwrite,
    /// 损坏文件已备份到 `backup_path`（内容原样保留），可覆盖
    BackupTaken { backup_path: String },
    /// 损坏但备份失败 — fail-closed：禁止覆盖，唯一副本仍在原地
    BackupFailed { reason: String },
}

/// 一次写前准备的完整结果：判定 + 动作（证据随动作一起返回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWrite {
    pub verdict: StateFileVerdict,
    pub action: WriteAction,
}

/// parse 现有状态文件：`Ok` = JSON 可解析，`Missing` = 不存在，
/// `Corrupt` = 存在但不可解析（半写/截断/乱码）。
pub fn inspect_state_file(path: &str) -> StateFileVerdict {
    match std::fs::read_to_string(Path::new(path)) {
        Err(_) => StateFileVerdict::Missing,
        Ok(content) => {
            if serde_json::from_str::<serde_json::Value>(&content).is_ok() {
                StateFileVerdict::Ok
            } else {
                StateFileVerdict::Corrupt
            }
        }
    }
}

/// 备份路径：`<path>.bak.<ts>`；ts 由调用方注入（默认取固定格式），
/// 同秒两次损坏也不会覆盖彼此的备份。
pub fn backup_path(path: &str, ts: &str) -> String {
    format!("{path}.bak.{ts}")
}

/// 把损坏文件原样复制到备份路径（保留唯一证据）。返回备份路径。
pub fn take_backup(path: &str, ts: &str) -> Result<String, String> {
    let backup = backup_path(path, ts);
    std::fs::copy(Path::new(path), Path::new(&backup))
        .map_err(|e| format!("copy {path} -> {backup} failed: {e}"))?;
    Ok(backup)
}

/// 写前守卫：parse 旧文件 → 损坏则先备份再允许覆盖；备份失败 fail-closed。
///
/// 接线方拿到 `PreparedWrite` 后：
/// - `SafeOverwrite` / `BackupTaken` → 执行覆盖，`BackupTaken` 时须显式报警
///   （tracing::warn / stderr）点名备份路径；
/// - `BackupFailed` → 禁止覆盖，把原因上报，保持损坏文件原地未动。
pub fn prepare_state_write(path: &str, ts: &str) -> PreparedWrite {
    match inspect_state_file(path) {
        StateFileVerdict::Ok => PreparedWrite {
            verdict: StateFileVerdict::Ok,
            action: WriteAction::SafeOverwrite,
        },
        StateFileVerdict::Missing => PreparedWrite {
            verdict: StateFileVerdict::Missing,
            action: WriteAction::SafeOverwrite,
        },
        StateFileVerdict::Corrupt => match take_backup(path, ts) {
            Ok(backup_path) => PreparedWrite {
                verdict: StateFileVerdict::Corrupt,
                action: WriteAction::BackupTaken { backup_path },
            },
            Err(reason) => PreparedWrite {
                verdict: StateFileVerdict::Corrupt,
                action: WriteAction::BackupFailed { reason },
            },
        },
    }
}

#[cfg(test)]
mod state_file_guard_tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("qbody-state-file-guard-{name}"));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn test_missing_file_is_missing_and_safe() {
        let dir = temp_dir("missing");
        let path = dir.join("nourish_signals.json");
        let _ = std::fs::remove_file(&path);
        let p = prepare_state_write(path.to_str().unwrap(), "ts1");
        assert_eq!(p.verdict, StateFileVerdict::Missing);
        assert_eq!(p.action, WriteAction::SafeOverwrite);
    }

    #[test]
    fn test_valid_json_is_ok_and_safe() {
        let dir = temp_dir("valid");
        let path = dir.join("yoyo_stats.json");
        std::fs::write(&path, "{\"prs\":3,\"tokens\":1200}\n").unwrap();
        let p = prepare_state_write(path.to_str().unwrap(), "ts1");
        assert_eq!(p.verdict, StateFileVerdict::Ok);
        assert_eq!(p.action, WriteAction::SafeOverwrite);
    }

    #[test]
    fn test_corrupt_json_backs_up_before_overwrite() {
        let dir = temp_dir("corrupt");
        let path = dir.join("nourish_signals.json");
        let raw = "{\"categories\":[not-json";
        std::fs::write(&path, raw).unwrap();
        let p = prepare_state_write(path.to_str().unwrap(), "20261119-2230");
        assert_eq!(p.verdict, StateFileVerdict::Corrupt);
        match &p.action {
            WriteAction::BackupTaken { backup_path } => {
                // 备份真实存在且内容与损坏原文一致（§15 三信号：写成功+内容一致）
                let backup_content =
                    std::fs::read_to_string(backup_path).expect("backup must exist");
                assert_eq!(backup_content, raw);
                assert!(backup_path.contains("20261119-2230"));
            }
            other => panic!("expected BackupTaken, got {other:?}"),
        }
    }

    #[test]
    fn test_prepare_write_backup_failure_blocks_overwrite() {
        // 构造「备份失败 → BackupFailed → 禁止覆盖」分支：
        // 损坏文件为目标，备份目标 = 一个已存在目录 → std::fs::copy 到目录会失败
        let dir = temp_dir("blocked");
        let path = dir.join("stats.json");
        std::fs::write(&path, "broken{").unwrap();
        let bad_ts = "collision";
        let backup_target = dir.join(format!("stats.json.bak.{bad_ts}"));
        std::fs::create_dir_all(&backup_target).unwrap(); // 幂等
        let p = prepare_state_write(path.to_str().unwrap(), bad_ts);
        assert_eq!(p.verdict, StateFileVerdict::Corrupt);
        match p.action {
            WriteAction::BackupFailed { reason } => {
                assert!(!reason.is_empty());
                // fail-closed：损坏文件原地未动
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken{");
            }
            other => panic!("expected BackupFailed, got {other:?}"),
        }
    }

    #[test]
    fn test_two_corrupt_writes_produce_two_distinct_backups() {
        let dir = temp_dir("twobak");
        let path = dir.join("nourish_signals.json");
        std::fs::write(&path, "bad1{").unwrap();
        let p1 = prepare_state_write(path.to_str().unwrap(), "ts-a");
        // 覆盖写回（模拟 cron 写新内容）
        std::fs::write(&path, "{\"fresh\":true}").unwrap();
        // 再损坏
        std::fs::write(&path, "bad2{").unwrap();
        let p2 = prepare_state_write(path.to_str().unwrap(), "ts-b");
        let b1 = match p1.action {
            WriteAction::BackupTaken { backup_path } => backup_path,
            _ => panic!("first must backup"),
        };
        let b2 = match p2.action {
            WriteAction::BackupTaken { backup_path } => backup_path,
            _ => panic!("second must backup"),
        };
        assert_ne!(b1, b2);
        assert_eq!(std::fs::read_to_string(&b1).unwrap(), "bad1{");
        assert_eq!(std::fs::read_to_string(&b2).unwrap(), "bad2{");
    }
}
