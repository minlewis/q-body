//! 模型定价与 id 解析的单一事实源 — 借鉴 yologdev/yoyo-evolve
//! "price model ids through ModelConfig + delete duplicate preset ids"
//!
//! 背景：q-body 成本账曾出现 1.7~3.3x 定价漂移（09-23 对官方价目表实测修正后
//! 收敛），根因之一是模型 id 与定价散落多处（handler.rs LLM_PROVIDERS 内联
//! 模型 id 字面量 + cost.rs 全局单值定价），一次定价改动难以知道改哪、且
//! 无法按模型区分成本。本模块把「模型 id ↔ 定价」绑定为唯一事实源，任何
//! 建模/定价改动只改这里。
//!
//! 纯函数模块，不触碰 handler 生产回路（借思路不借宿主先例）。宿主内联 id
//! 落 main 后接线时，LLM_PROVIDERS 的多种字段可直接派生自本表。

/// 模型 id 的单一事实源常量（与 handler.rs LLM_PROVIDERS 内联字面量对齐）
pub const MODEL_ARK: &str = "deepseek-v4-flash";
pub const MODEL_DEEPSEEK: &str = "deepseek-chat";

/// 单一模型的定价配置（美元 / 1M tokens，峰值 cache-miss 保守档）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelConfig {
    /// 模型 id（与 provider 请求体 `model` 字段同一字符串）
    pub model: &'static str,
    /// input 单价（美元 / 1M tokens）
    pub input_usd_per_mtok: f64,
    /// output 单价（美元 / 1M tokens）
    pub output_usd_per_mtok: f64,
}

impl ModelConfig {
    pub const fn new(model: &'static str, input: f64, output: f64) -> Self {
        Self {
            model,
            input_usd_per_mtok: input,
            output_usd_per_mtok: output,
        }
    }
}

/// 定价查表单一事实源。
///
/// 价格对齐官方价目表 2026-08-16 重定价（peak cache-miss 档，保守估计）：
/// - deepseek-v4-flash（ark 主 provider）：input $0.44 / output $1.32
/// - deepseek-chat（deepseek-platform 备）：input $0.30 / output $1.20
/// 来源：DeepSeek 官方定价页 + 第三方核验页（10-27 PR #171 web 交叉核验确认）。
/// 同源数字也在 cost.rs，本表是结构化绑定；改价只改这里，cost.rs 可后续派生。
pub const MODEL_CONFIGS: &[ModelConfig] = &[
    ModelConfig::new(MODEL_ARK, 0.44, 1.32),
    ModelConfig::new(MODEL_DEEPSEEK, 0.30, 1.20),
];

/// 按模型 id 查定价。未知模型回落第一个（主模型）档，保守不漏判。
pub fn price_for_model(model: &str) -> Option<ModelConfig> {
    MODEL_CONFIGS
        .iter()
        .find(|c| c.model == model)
        .copied()
        .or_else(|| MODEL_CONFIGS.first().copied())
}

/// 已知模型 id 列表（id 解析收口：判断某 id 是否受本表管理）
pub fn known_models() -> Vec<&'static str> {
    MODEL_CONFIGS.iter().map(|c| c.model).collect()
}

/// 按模型区分成本估算（美元），未知模型回落主模型档保守估算
pub fn estimate_for_model(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let cfg = price_for_model(model).expect("price_for_model always falls back to first");
    input_tokens as f64 * cfg.input_usd_per_mtok / 1_000_000.0
        + output_tokens as f64 * cfg.output_usd_per_mtok / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_configs_cover_both_providers() {
        // id 单一事实源与 handler.rs 内联字面量对齐（接线前用手动 grep 背书）
        let ids = known_models();
        assert!(ids.contains(&MODEL_ARK));
        assert!(ids.contains(&MODEL_DEEPSEEK));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_ark_pricing_matches_official() {
        // left: 官方峰值档；right: 10-27 PR #171 web 核验档
        let ark = price_for_model(MODEL_ARK).unwrap();
        assert!((ark.input_usd_per_mtok - 0.44).abs() < 1e-9);
        assert!((ark.output_usd_per_mtok - 1.32).abs() < 1e-9);
        let ds = price_for_model(MODEL_DEEPSEEK).unwrap();
        assert!((ds.input_usd_per_mtok - 0.30).abs() < 1e-9);
        assert!((ds.output_usd_per_mtok - 1.20).abs() < 1e-9);
    }

    #[test]
    fn test_unknown_model_falls_back_to_primary() {
        // 未知模型回落主模型档（保守不漏判，§15 三信号：走查表真路径非 copy）
        let cfg = price_for_model("nonexistent-model").unwrap();
        assert_eq!(cfg.model, MODEL_ARK);
        assert!((cfg.input_usd_per_mtok - 0.44).abs() < 1e-9);
    }

    #[test]
    fn test_estimate_for_model_uses_row() {
        // 1M input = $0.44（ark）；deepseek-chat 1M input = $0.30
        assert!((estimate_for_model(MODEL_ARK, 1_000_000, 0) - 0.44).abs() < 1e-9);
        assert!((estimate_for_model(MODEL_DEEPSEEK, 1_000_000, 0) - 0.30).abs() < 1e-9);
    }

    #[test]
    fn test_price_for_model_exact_arm_nonempty() {
        // 非空断言：查表不是空表（§15 三信号：实现不可为空洞常量）
        assert!(!MODEL_CONFIGS.is_empty());
    }
}
