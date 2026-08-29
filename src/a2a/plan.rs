//! Plan 结构化模板注入 — RED/GREEN/REFACTOR TDD 骨架
//!
//! 借鉴：yologdev/yoyo-evolve — PR #583 `/plan --deep`
//! 当 `--deep` 标记出现在消息中时，自动生成三阶段 TDD 骨架模板，
//! 提升计划的可验证性和覆盖率。

/// TDD 阶段
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Red,
    Green,
    Refactor,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Red => "RED",
            Phase::Green => "GREEN",
            Phase::Refactor => "REFACTOR",
        }
    }
}

/// 一个 plan 阶段
#[derive(Debug, Clone)]
pub struct PlanStage {
    pub phase: Phase,
    pub description: String,
    pub todo_items: Vec<String>,
}

/// 结构化 deep plan
#[derive(Debug, Clone)]
pub struct DeepPlan {
    pub stages: Vec<PlanStage>,
}

/// 检测消息中是否包含 `--deep` 标记
///
/// 返回 `Some(DeepPlan)` 当输入包含 `--deep` 标记时，
/// 否则返回 `None`。
pub fn detect_deep_plan(input: &str) -> Option<DeepPlan> {
    if !input.contains("--deep") {
        return None;
    }

    // 提取标记后的任务描述
    let task_desc = input
        .split("--deep")
        .nth(1)
        .map(|s| {
            s.trim()
                .trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == ':')
        })
        .unwrap_or("")
        .to_string();

    let task_desc = if task_desc.is_empty() {
        "未指定任务".to_string()
    } else {
        task_desc
    };

    Some(DeepPlan {
        stages: vec![
            PlanStage {
                phase: Phase::Red,
                description: format!("编写失败测试 — {}", task_desc),
                todo_items: vec![
                    format!("定义 {} 的接口/签名", task_desc),
                    format!("编写最小断言验证预期行为 — 预期编译失败或测试失败"),
                ],
            },
            PlanStage {
                phase: Phase::Green,
                description: format!("实现最小通过 — {}", task_desc),
                todo_items: vec![
                    format!("编写最简实现使 RED 测试通过"),
                    format!("不做额外优化，仅满足测试"),
                ],
            },
            PlanStage {
                phase: Phase::Refactor,
                description: format!("重构优化 — {}", task_desc),
                todo_items: vec![
                    format!("消除重复/冗余，保持测试通过"),
                    format!("提取公共逻辑，优化命名和结构"),
                ],
            },
        ],
    })
}

/// 生成 plan 模板文本
///
/// 将 `DeepPlan` 渲染为可读的 todo 模板字符串。
/// 若输入不含 `--deep`，返回空字符串。
pub fn generate_deep_plan(input: &str) -> String {
    let plan = match detect_deep_plan(input) {
        Some(p) => p,
        None => return String::new(),
    };

    let mut output = String::new();
    output.push_str("## Plan (--deep TDD 骨架)\n\n");

    for stage in &plan.stages {
        output.push_str(&format!(
            "### {} — {}\n",
            stage.phase.as_str(),
            stage.description
        ));
        for item in &stage.todo_items {
            output.push_str(&format!("- [ ] {}\n", item));
        }
        output.push('\n');
    }

    output.push_str("---\n*生成于每次回灌周期 — 参照 RED/GREEN/REFACTOR 循环*\n");

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_deep_flag() {
        let result = detect_deep_plan("请实现一个斐波那契函数 --deep");
        assert!(result.is_some());
        let plan = result.unwrap();
        assert_eq!(plan.stages.len(), 3);
        assert_eq!(plan.stages[0].phase, Phase::Red);
        assert_eq!(plan.stages[1].phase, Phase::Green);
        assert_eq!(plan.stages[2].phase, Phase::Refactor);
    }

    #[test]
    fn test_no_deep_flag_returns_none() {
        let result = detect_deep_plan("请实现一个斐波那契函数");
        assert!(result.is_none());
    }

    #[test]
    fn test_generate_template_contains_all_phases() {
        let template = generate_deep_plan("实现排序算法 --deep");
        assert!(!template.is_empty());
        assert!(template.contains("RED"));
        assert!(template.contains("GREEN"));
        assert!(template.contains("REFACTOR"));
        assert!(template.contains("[ ]"));
    }

    #[test]
    fn test_no_deep_returns_empty() {
        let template = generate_deep_plan("实现排序算法");
        assert!(template.is_empty());
    }

    #[test]
    fn test_deep_plan_with_complex_input() {
        // 测试 `--deep` 出现在不同位置
        let result = detect_deep_plan("--deep 实现用户注册功能");
        assert!(result.is_some());
        let plan = result.unwrap();
        assert_eq!(plan.stages.len(), 3);
        assert!(plan.stages[0].description.contains("实现用户注册功能"));
    }
}
