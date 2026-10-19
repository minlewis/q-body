//! Env 读取的单一事实源 + 测试隔离。
//!
//! 存在的理由：q-body 测试进程会继承 runner 的环境变量。`LLM_PROVIDERS` 里的
//! key 名（ARK_API_KEY / DEEPSEEK_API_KEY）是固定的常量，如果 runner 恰好设了
//! 这些 key，`call_providers_once` 的「key 缺失 → 跳过 / failover」语义就会被
//! 运行环境污染——同一份代码在不同机器上得到不同的测试结果。
//!
//! 对策：
//! 1. 所有 provider key 读取收敛到 `lookup_api_key`（单一事实源，语义只写一遍）。
//! 2. 测试一律使用**独占 key 名**（`QBODY_TEST_ENV_ISOLATION_KEY`），不触碰
//!    ARK_API_KEY / DEEPSEEK_API_KEY，因此无论 runner 设了什么 key，测试结论一致。
//!
//! 借鉴：yologdev/yoyo-evolve — config_fifo tests stop inheriting the runner's key
//! → q-body config/env 测试独立环境，杜绝 test 假绿。

/// 读取 provider 的 API key。
///
/// - 未设置 → `None`（provider 应被跳过，failover 到下一个）
/// - 空串 / 纯空白 → `None`（等同于未配置，不把空白当凭据发出去）
/// - 有值 → `Some(原始值)`（trim 只用于判空，不改写实际发送的 key）
pub fn lookup_api_key(env_key: &str) -> Option<String> {
    match std::env::var(env_key) {
        Ok(k) if !k.trim().is_empty() => Some(k),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独占 key 名：刻意不用 ARK_API_KEY / DEEPSEEK_API_KEY，
    /// 保证测试结论不受 runner 真实凭据影响。
    const ISOLATED_KEY: &str = "QBODY_TEST_ENV_ISOLATION_KEY";

    fn clear() {
        // SAFETY: 只改本进程的独占测试变量，真实 provider key 不受影响
        unsafe {
            std::env::remove_var(ISOLATED_KEY);
        }
    }

    #[test]
    fn test_missing_key_is_not_inherited() {
        // 独占 key 从未设置 → None；证明测试不靠 runner 环境也能判定「未配置」
        clear();
        assert_eq!(lookup_api_key(ISOLATED_KEY), None);
    }

    #[test]
    fn test_blank_key_treated_as_unset() {
        // 空串与纯空白都不得被当成有效凭据
        for blank in ["", "   ", "\t\n"] {
            unsafe {
                std::env::set_var(ISOLATED_KEY, blank);
            }
            assert_eq!(lookup_api_key(ISOLATED_KEY), None, "blank={blank:?}");
        }
        clear();
    }

    #[test]
    fn test_present_key_is_returned_verbatim() {
        unsafe {
            std::env::set_var(ISOLATED_KEY, "test-key-value");
        }
        assert_eq!(
            lookup_api_key(ISOLATED_KEY),
            Some("test-key-value".to_string())
        );
        clear();
    }

    #[test]
    fn test_env_does_not_leak_after_cleanup() {
        // 测试自己注入、自己清干净——不把污染留给后续测试
        unsafe {
            std::env::set_var(ISOLATED_KEY, "leaky");
        }
        assert!(lookup_api_key(ISOLATED_KEY).is_some());
        clear();
        assert_eq!(lookup_api_key(ISOLATED_KEY), None);
    }
}
