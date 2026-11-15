//! JSON-RPC 参数严格校验（借鉴 yologdev/yoyo-evolve #1111 fail-fast）
//!
//! 问题：handle_reflect / handle_journal_record / handle_list_tasks 之前只读
//! 自己认识的字段，未知参数（拼写错误 / 客户端发错键名 / 新版本别名被旧端误发）
//! 会被静默忽略——调用方以为参数生效了，实则是空操作。SOUL §25「检查的真伪由
//! 攻击者视角决定」：一个永远不会失败的解析就是最好的藏错处。
//!
//! 改法：`reject_unknown_keys(params, allowed)` 在业务逻辑之前拒绝任何不在
//! 白名单里的键，返回带 `unknown_key` 的结构化错误说明——fail-fast，绝不静默
//! 忽略当 operand。未知键名进错误消息（不泄内部路径，trust-boundary 同款纪律）。

/// 结构化的未知参数错误信息：标出具体键名，让调用方一眼看清改什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKeyError {
    /// 未在白名单里的键名（首个命中的）
    pub key: String,
    /// 允许的键名列表（仅供错误文案展示，不含内部信息）
    pub allowed: Vec<String>,
}

impl std::fmt::Display for UnknownKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown parameter '{}' (allowed: {})",
            self.key,
            self.allowed.join(", ")
        )
    }
}

/// 参数必须是 JSON 对象，且其中每个键都在 `allowed` 白名单内。
///
/// - 参数不是对象（数组/字符串/数字/null）→ `None`（由调用方按缺参/畸形处理）。
/// - 命中未知键 → `Some(Err(err))`。
/// - 全部在名单内 → `Some(Ok(()))`。
///
/// 语义：只负责「拒绝未知」，不负责「字段该不该是那种类型」——后者交给各
/// handler 自己的 `.as_str()`/`.as_f64()` 解析（那里已有合法/缺省/非法三段分档）。
pub fn reject_unknown_keys(
    params: &serde_json::Value,
    allowed: &[&str],
) -> Option<Result<(), UnknownKeyError>> {
    let obj = params.as_object()?;
    for (k, _v) in obj {
        if !allowed.contains(&k.as_str()) {
            return Some(Err(UnknownKeyError {
                key: k.clone(),
                allowed: allowed.iter().map(|s| s.to_string()).collect(),
            }));
        }
    }
    Some(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_all_known_keys_pass() {
        let p = json!({"text": "hi", "threshold": 0.7});
        assert_eq!(reject_unknown_keys(&p, &["text", "threshold"]), Some(Ok(())));
    }

    #[test]
    fn test_unknown_key_rejected_with_name() {
        let p = json!({"text": "hi", "threshhold": 0.7}); // 拼写错误
        match reject_unknown_keys(&p, &["text", "threshold"]) {
            Some(Err(e)) => {
                assert_eq!(e.key, "threshhold");
                assert!(e.allowed.contains(&"threshold".to_string()));
            }
            other => panic!("expected Some(Err), got {other:?}"),
        }
    }

    #[test]
    fn test_non_object_params_return_none() {
        // 数组 / 字符串 / null 不是对象——由调用方按缺参/畸形路径处理，本闸不越权
        assert_eq!(reject_unknown_keys(&json!([1, 2]), &["a"]), None);
        assert_eq!(reject_unknown_keys(&json!("str"), &["a"]), None);
        assert_eq!(reject_unknown_keys(&json!(null), &["a"]), None);
    }

    #[test]
    fn test_empty_object_passes() {
        // 允许的键一个没给 = 合法（缺省回落逻辑在 handler 层），但未知键仍拒
        assert_eq!(reject_unknown_keys(&json!({}), &["text"]), Some(Ok(())));
    }

    #[test]
    fn test_multiple_unknown_picks_an_unknown() {
        // serde_json object 是 BTreeMap，键按字典序——不假设「第一个」是哪个未知键，
        // 只断言命中其中一个未知键
        let p = json!({"foo": 1, "bar": 2});
        match reject_unknown_keys(&p, &["text"]) {
            Some(Err(e)) => {
                assert!(
                    e.key == "foo" || e.key == "bar",
                    "expected one of unknown keys, got {:?}",
                    e.key
                );
            }
            other => panic!("expected Some(Err), got {other:?}"),
        }
    }
}