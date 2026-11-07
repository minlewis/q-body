//! journal::soul_context — SOUL 注入切片（按字符预算取最近 N 条事件）。
//! 纯移动自 journal.rs，无行为变化。

use super::*;

impl Journal {
    /// SOUL 注入切片：从 events 尾部（最新优先）按字符预算取条目，勿全量。
    ///
    /// 借鉴来源：yologdev/yoyo-evolve — #886 arg-gated 分档（查询/注入类
    /// 零成本直达，计费路径才写代码）。TAO P0 后半的消费端：journal JSONL
    /// core 落地后，LLM system prompt 注入 journal 尾部预算切片而非全量历史。
    ///
    /// 规则：从最新事件往前逐条计入，单条完整纳入（不做条内截断），
    /// 超出 `max_chars` 即停；返回按时间升序拼接的切片（ oldest first ），
    /// 即「最近 N 条按原始顺序」。空 journal 或 max_chars == 0 返回空串。
    pub fn soul_context_slice(&self, max_chars: usize) -> String {
        if max_chars == 0 {
            return String::new();
        }
        let mut picked: Vec<&EvolutionEvent> = Vec::new();
        let mut used = 0usize;
        for ev in self.events.iter().rev() {
            let cost = ev.suggestion.chars().count() + ev.source.chars().count() + 32;
            if used + cost > max_chars {
                break;
            }
            used += cost;
            picked.push(ev);
        }
        picked
            .iter()
            .rev()
            .map(|ev| {
                format!(
                    "- [{}] {:?} from {}: {}",
                    ev.timestamp.format("%m-%d"),
                    ev.signal,
                    ev.source,
                    ev.suggestion
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
