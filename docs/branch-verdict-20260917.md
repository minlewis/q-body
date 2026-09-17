# q-body 分支裁决报告（2026-09-17，全部数字来自 git 实测）

## 判据（TAO 三问门禁）

三问：(1) 接线了吗 (2) 能进 SOUL.md 吗 (3) 损掉了什么。
git 事实：main 已含 issue-96/90/98 的内容且为超集（见下），纯类型准备层分支无运行时接线。

## 一、内容已被 main 包含（关闭分支，零信息损失）

| 分支 | git 证据 |
|---|---|
| issue-96-evolution-gate | 分支版 293 行 vs main 版 305 行，main 版是超集（多了零token申报谓词修复 + gate_audit 钉死，diff 仅 12 行且全部是 main 新增） |
| issue-90-recency-guard | 分支版 139 行 vs main 140 行，main 版含 `<` 边界修复（793fde5），分支版是修复前的旧版 |
| issue-98-error-sanitize | git cherry 显示 commit 1d948f5 已在 main |
| issue-94-llm-parse-retry | 基于 failover 重写前的旧 query_llm，main 已被 105 分支的 failover 链取代 |
| issue-81-llm-retry-counter | 同上，基于旧 query_llm，retry 语义已被 queue-parse-log 结构化计数取代 |

## 二、纯类型准备层（无接线计划 → 关闭，判例沉淀进 LEARNINGS.md）

issue-10, 12, 19, 21, 27, 29, 31, 33, 35, 37, 40, 42, 45(×2), 49, 52, 54, 56, 58, 60, 61, 62, 71, 81-skills-hot-reload, 84, 86, 88, 92

共 27 条。共同特征：单文件纯函数模块 + 单测，无 handler 接线。其中若干与 main 现有模块重叠（journal/synthesize/skills/tool-schema 各有 main 替代或已有 clamp/queue/darkroom/backlog/trust_input 落地）。

## 三、有实质功能，逐条过三问（下轮处理）

| 分支 | 实质 | 行数 | 初判 |
|---|---|---|---|
| issue-39-journal-jsonl-persistence | journal JSONL 持久化 | +916 | TAO 记账「P0=journal JSONL」正主，**最可能值得复活**，但会顶爆行数预算（当前 1691+916=2607） |
| issue-77-journal-synthesize | 每日压缩 journal→active_context | +824 | 依赖 39，与 39 二选一或链式 |
| v0.1.4-config-journal-health | M3 provider + config journal | +1165 | M3 部分 Anthropic 协议已有独立价值，但 1691+1165=2856 贴近 3000 上限 |
| proactive-memory-trigger | PMA 模式 spike | +536 | 实验性质，老板哲学：spike 结束应记判例不进主干 |
| fix/honesty-ci-first-tests | CI workflow + a2a 扩展 | +265 | CI yml 部分有独立价值（可单独拆） |
| issue-0-evolution-journal-snapshot | journal 快照 | +427 | 与 39/77 同族 |
| issue-17/23/25 | seen-state/cycle-id/assessment 链 | 538+628+710 | 与 main 已有 queue/seen_state 语义重叠，疑被后续 standalone 模块取代 |

## TAO 账目

当前 main：1691 行 Rust / 15 src 文件（已达 ≤12 上限）/ 13 依赖（≤15 内）。
**全合会击穿双上限**——这是逐条裁决而非批量合并的硬理由。
