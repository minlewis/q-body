//! q-body library crate — 暴露模块供 integration tests 与复用。
//!
//! binary (`main.rs`) 与 lib 共享同一份模块代码，避免双实现漂移。

pub mod a2a;
pub mod handler;
pub mod state;
