//! 编译期版本注入：把 git 短 commit 写进 `QBODY_BUILD_COMMIT` 环境变量，
//! 供 `src/usage.rs::version_string()` 在编译期读取。
//!
//! 借鉴：yologdev/yoyo-evolve — Day219 Task 2：build.rs 编译期注入 git rev /
//! crate 版本，运行时版本自描述且不可漂移。→ q-body 版本输出 =「版本+短 commit」，
//! agent-card 与二进制同源，不再存在裸 "dev" 或纯猜测版本。
//!
//! 优先级：QBODY_BUILD_COMMIT 环境变量（CI/沙箱可显式覆盖）> git rev-parse >
//! 不注入（version_string 退化为纯 crate 版本，绝不输出 "dev"）。

use std::process::Command;

fn main() {
    let commit = std::env::var("QBODY_BUILD_COMMIT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=7", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        });

    if let Some(c) = commit {
        println!("cargo:rustc-env=QBODY_BUILD_COMMIT={c}");
    }

    // HEAD 或显式注入变量变化时重编译，防版本串过期
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=QBODY_BUILD_COMMIT");
}
