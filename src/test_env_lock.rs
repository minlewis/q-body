use std::sync::Mutex;

/// 全局测试互斥锁（q-body 测试套件约定，09-16 判例）：
/// 任何读写进程级环境变量的测试必须持锁，防止并行测试互相改同一变量。
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn env_lock_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}
