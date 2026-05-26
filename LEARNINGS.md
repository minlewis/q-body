# 🫧 q-body 学习笔记

> 从开源社区和同类项目中学习，持续进化。

---

## 2026-05-26 — 对标学习：yoyo-evolve 源码深潜

### 背景

老板让我学 [yoyo-evolve](https://github.com/yologdev/yoyo-evolve)（1787★，Day 86）的源码，
找出 q-body 的进化方向。yoyo 是一个自进化的 AI 编码 agent，跟 q-body 同为 Rust 项目，
但工程成熟度差距很大。

### 学到的关键模式

#### 1. 测试结构

yoyo 有三种测试位置：
- `src/*.rs` inline `#[cfg(test)]` — 模块级单元测试
- `tests/integration.rs` — 集成测试
- `#[serial_test]` — env-var 依赖测试

**最有启发的案例：** `help_data.rs` 的 12 个测试（纯数据验证模式）
- `test_every_known_command_has_help` — 每个命令都有帮助文本
- `test_short_descriptions_are_actually_short` — ≤80 字符
- `test_no_duplicate_short_descriptions` — 无重复描述
- `test_help_entries_start_with_command_name` — 帮助文本提及自身

**q-body 可以抄**：`types.rs` 的类型定义可以加 inline test 验证 JSON-RPC 序列化一致性。

#### 2. 配置系统

3 层配置查找：`.yoyo.toml`（项目）→ `~/.yoyo.toml`（家目录）→ XDG 目录

可配置项：model、provider、thinking、temperature、auto_commit、auto_watch、
permissions（bash 命令白名单）、directories（文件访问限制）、MCP servers

**q-body 当前状态**：全部硬编码在 `handler.rs:14-15`（LLM_API_URL、LLM_MODEL）
和 `handler.rs:151-154`（system prompt）。连端口都只做了简陋的 `--port=` 解析。

#### 3. 自治流水线

3 阶段架构，每小时 GitHub Actions cron 触发：

| 阶段 | 做什么 | 时间限制 |
|------|--------|---------|
| A — Planning | A1 评估 + A2 计划 → task_01.md 等 | 1200s |
| B — Implementation | 逐 task 实现 → build+test 门控(10次) → Eval(9次) → 失败则 revert | 20min/task |
| C — Journal | 写 JOURNAL.md + memory/learnings.jsonl | — |
| D — Wrap-up | cargo fmt → build+test+clippy → 批量 commit → tag dayN → push | — |

**每天 10+ commit 的来源**：~3 sessions/day × 3-5 commits/session

#### 4. 额外工作流
- **skill-evolve**（每小时:30）— 精炼/创建/退役非核心技能
- **synthesize**（每日）— 压缩记忆归档到活跃上下文
- **sponsors-refresh**（每小时）— 更新赞助商状态

### 最重要的认知

> yoyo 能跑 86 天不间断不仅是代码，更是 **GitHub Actions 基建 + 失败重试机制 + 自我修复回路** 的组合。
> q-body 当前只是"跑了个服务"而已，连 CI/CD 都没有。
> **从"工具"到"项目"的质变，差距不在代码量，在工程体系。**

### 差距分析

| 维度 | 理想画像 | q-body 当前 |
|------|---------|-------------|
| 认知层 | SOUL.md 可移植、LLM 可替换 | LLM 硬编码、无配置系统 |
| 能力层 | 自检+差距分析、自我验证、记忆驱动 | 只靠 Hermes 侧 skill |
| 身体层 | Rust A2A body 独立运行 | v0.1.0 可跑但功能极简 |

### 重构路线图

- **Phase 1 — 配置化**：q-body.toml，可配置 port/provider/model/system_prompt，启动前校验
- **Phase 2 — 持久化**：SQLite TaskStore，重启不丢对话
- **Phase 3 — 可观测性**：/health 端点 + 测试补齐 + 结构化日志
- **Phase 4 — Streaming + Tool calling**：SSE 流式返回 + 工具注册框架
---

## 2026-05-26 — 对标学习：AgentScope 2.0（25622★）

> 阿里出品，生产级 Python agent 框架。内置 A2A/MCP 支持、事件流、Toolkit、记忆系统。

### 关键发现

#### 1. 架构：Middleware Onion（中间件洋葱）

AgentScope 的核心是一个 `Agent` 类，用多层中间件包裹 `reply` 流程：

```
reply_stream → mw1 → mw2 → _reply_impl
                              ↓
                       _reasoning (LLM call)
                              ↓
                       _execute_tool_call
                              ↓
                       _acting (toolkit.call_tool)
```

**q-body 可以抄**：用 `trait AgentHook` + `Pin<Box<dyn Stream>>` 在 Rust 里实现同样的中间件链。

#### 2. A2A 集成方式

AgentScope 没有专门的 A2A 客户端——它用 `ProtocolMiddlewareBase` 做协议转换：

```
AgentEvent 流 → ProtocolMiddlewareBase._convert_to_protocol() → A2A JSON/SSE
```

**对 q-body 的启示**：内部用统一的 `AgentEvent` 枚举，外部通过中间件转成 A2A 协议。
q-body 当前直接把外部请求映射到 LLM 调用，缺少中间的事件抽象层。

#### 3. Toolkit 系统（最值得复用的模式）

三层抽象：
- **ToolBase trait** — name, description, input_schema(JSON Schema), async call
- **Adapters** — FunctionTool（包装任意函数）、MCPTool（包装 MCP 客户端）
- **Toolkit registry** — 按组管理，支持动态激活/停用

**q-body 的差距**：目前没有 tool calling 能力，只能"调 LLM 回复"。
AgentScope 的 toolkit 设计可以直接翻译成 Rust 的 trait 体系。

#### 4. Event Streaming 事件系统

AgentScope 定义了一整套 AgentEvent 区分联合：

| Event | 对应 A2A |
|-------|---------|
| ReplyStartEvent | TaskStatusUpdateEvent(status=WORKING) |
| TextBlockDeltaEvent | SSE text delta |
| ToolCallStartEvent | TaskArtifactUpdateEvent |
| ReplyEndEvent | TaskStatusUpdateEvent(status=COMPLETED) |

**对 q-body 的启示**：事件驱动的架构天然适配 A2A 的 SSE 协议。
q-body 的 Phase 4（Streaming）应该先用 AgentEvent 枚举统一内部事件模型，
再通过中间件转成 SSE 输出。

#### 5. 记忆系统

两层记忆：
- **Working context**: `Vec<Msg>` — 当前对话轮次
- **Compressed summary**: LLM 生成的 5 字段结构化摘要（task_overview, current_state, important_discoveries, next_steps, context_to_preserve）

**对 q-body 的启示**：TaskStore 不仅要存对话历史，还要支持上下文压缩。
这比纯 SQLite 持久化更有价值——是让 q-body 拥有"记忆"的关键模式。

### 更新后的重构路线图

基于 yoyo + AgentScope 的双重学习，调整优先级：

```
Phase 1 — 配置化（q-body.toml）
Phase 2 — Event 系统 + Toolkit 框架（先于持久化！）
  原因：AgentScope 的实践表明，事件模型是 A2A SSE 和 tool calling 的共同基础
Phase 3 — SQLite 持久化
Phase 4 — 上下文压缩 + 结构化摘要（记忆系统）
Phase 5 — SSE Streaming
```
