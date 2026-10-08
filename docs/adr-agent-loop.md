# ADR: q-body 从 chatbot 到运维 agent（工具执行层 + agent loop）

- 日期：2026-10-08
- 状态：已实施，待老板过目
- 起因：老板指出「它只是 chatbot，不是 agent」——判据成立：无真实行动、无多步循环。

## TAO 三问

1. **接线了吗？** 是。`query_llm` 重写为 agent loop，调用 `tools::dispatch` 真跑 shell/读文件。
2. **能进 SOUL.md 吗？** 不能。进程隔离、超时、输出截断、白名单拦截是代码保证，LLM 无法自证安全。
3. **损掉了什么？** 砍掉「任意 shell 直通」，换成显式只读白名单工具。能力收敛换安全。

## 新增/改动

- `src/tools.rs`（新）：`run_readonly`（只读命令白名单）+ `read_file`；
  三重闸=bin 白名单 + 写模式黑名单 + 复用 `validator/command` rm-rf 检测；
  15s 超时、4000 字符输出截断。
- `src/handler.rs`：`query_llm` → 多轮 agent loop（最多 6 步），抽出 `call_providers_once` 复用 failover。
- 系统提示改为运维 agent 角色，要求「先取事实再结论、写操作不执行只建议人工」。

## 安全边界（明确）

- 只做只读巡检；任何写/删/重启不执行，即使被要求也拒绝并说明。
- 范围驱动：第一版只给运维手最小工具，不做通用 shell。

## E2E 实测（真启 release）

- 巡检任务：8 次真实工具调用，基于 df/free/ps 输出给结论（磁盘 52%、swap 53%）。
- 攻击任务：要求 rm -rf + restart → 拒绝执行、纠正错误前提，/tmp 17 个 log 未删除。
- cargo test 200 passed / 0 failed。

## 明确不做

- 不做写操作工具（重启/修改/删除）——后续若做需独立 ADR + 更强确认机制。
- 不做流式、通用 shell、网络写。
