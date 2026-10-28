//! 暴露状态的持久性分类（Durable / SessionScoped / Unspecified）
//!
//! 借鉴 yologdev/yoyo-evolve #1088 — "stops promising a session setting
//! that dies with the process"：session 内存态与持久态分离，外部一律读持久态。
//! q-body 对应：为每个暴露给外部的状态标注持久性，堵住「内存态当持久态承诺」
//! 的自 concealing 路径（客户端读到 task 状态，进程重启后全部消失，接口却不声明）。

/// 状态暴露层的持久性分类
///
/// - `Durable`：落盘持久态，进程重启后可恢复，外部可放心读取。
/// - `SessionScoped`：仅进程内存态，随进程死亡即失，外部读取必须结合该事实理解，
///   不能承诺「这个状态还在」。
/// - `Unspecified`：未声明持久性的暴露状态——审计标记点，默认不得被当作持久态承诺。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceClass {
    /// 落盘持久态（journal / state 文件），重启可恢复
    Durable,
    /// 仅进程内存态（TaskStore 之类），随进程死亡即失
    SessionScoped,
    /// 未声明持久性，不得默认当作持久态承诺
    Unspecified,
}

/// 单一事实源词表：持久性分类的展示名
impl PersistenceClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            PersistenceClass::Durable => "durable",
            PersistenceClass::SessionScoped => "session-scoped",
            PersistenceClass::Unspecified => "unspecified",
        }
    }
}

/// 暴露状态条目：一个对外暴露的状态 + 其持久性分类
///
/// 用结构性判据取代易漂移的 prose：每个暴露态必须显式归类，
/// 未归类（Unspecified）即触发审计告警，杜绝「内存态静默当持久态承诺」。
#[derive(Debug, Clone)]
pub struct ExposedState {
    /// 状态名（如 "task"）
    pub name: &'static str,
    /// 持久性分类
    pub class: PersistenceClass,
}

/// 审计结果：该暴露状态是否被「当作持久态」安全承诺
///
/// - `Ok` 仅当分类为 Durable。
/// - SessionScoped / Unspecified 一律返回 Err（含一句可读提示），
///   呼应 yoyo #1012 语义：不可当持久态承诺的状态，接口必须把该事实说出口。
pub fn can_promise_persistent(state: &ExposedState) -> Result<(), String> {
    match state.class {
        PersistenceClass::Durable => Ok(()),
        PersistenceClass::SessionScoped => Err(format!(
            "state '{}' is session-scoped: dies with the process, not a durable promise",
            state.name
        )),
        PersistenceClass::Unspecified => Err(format!(
            "state '{}' has unspecified persistence: cannot be promised as durable",
            state.name
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_is_promisable() {
        let s = ExposedState {
            name: "journal",
            class: PersistenceClass::Durable,
        };
        assert!(can_promise_persistent(&s).is_ok());
    }

    #[test]
    fn session_scoped_is_not_promisable() {
        let s = ExposedState {
            name: "task-store",
            class: PersistenceClass::SessionScoped,
        };
        let err = can_promise_persistent(&s).unwrap_err();
        assert!(err.contains("session-scoped"));
        assert!(err.contains("dies with the process"));
    }

    #[test]
    fn unspecified_is_not_promisable() {
        let s = ExposedState {
            name: "future-metric",
            class: PersistenceClass::Unspecified,
        };
        let err = can_promise_persistent(&s).unwrap_err();
        assert!(err.contains("unspecified persistence"));
    }

    #[test]
    fn as_str_matches_vocabulary() {
        assert_eq!(PersistenceClass::Durable.as_str(), "durable");
        assert_eq!(PersistenceClass::SessionScoped.as_str(), "session-scoped");
        assert_eq!(PersistenceClass::Unspecified.as_str(), "unspecified");
    }

    /// 审计锚点：当前唯一已知的「随进程死亡仍被暴露」状态是 TaskStore。
    /// 此测试钉住该事实——若 future 把 task 持久化，改名并升级为 Durable，
    /// 这条测试随即转红逼出该决定（§22 出勤钩子：不缺席检查）。
    #[test]
    fn task_store_must_never_be_promised_as_durable_until_persisted() {
        let s = ExposedState {
            name: "task",
            class: PersistenceClass::SessionScoped,
        };
        assert!(can_promise_persistent(&s).is_err());
    }
}