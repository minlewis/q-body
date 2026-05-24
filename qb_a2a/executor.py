"""
QBAgentExecutor — q-body 的 A2A AgentExecutor 实现

负责解析 A2A Task（用户发来的消息），调用 q-body 的能力返回结果。

当前阶段（v1）：
- 接收 Message -> 解析 query -> 返回 Hello World 响应
- 验证 A2A 整条链路通不通

后续阶段：
- v2: 接入 coding agent 能力（运行代码、读写文件、搜索）
- v3: 接入自进化能力（差距分析、技能改进）
- v4: 形成完整的 q-body 服务
"""

import asyncio
import logging

from a2a.server.agent_execution.agent_executor import AgentExecutor
from a2a.server.agent_execution.context import RequestContext
from a2a.server.events.event_queue import EventQueue
from a2a.server.tasks.task_updater import TaskUpdater
from a2a.types import (
    Part,
    Task,
    TaskState,
    TaskStatus,
)

logger = logging.getLogger(__name__)


class QBAgentExecutor(AgentExecutor):
    """q-body 的 A2A AgentExecutor。

    接收来自外部的 Task，解析用户消息，执行后返回结果。
    """

    def __init__(self) -> None:
        self.running_tasks: set[str] = set()

    async def cancel(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        task_id = context.task_id
        if task_id and task_id in self.running_tasks:
            self.running_tasks.remove(task_id)

        updater = TaskUpdater(
            event_queue=event_queue,
            task_id=task_id or "",
            context_id=context.context_id or "",
        )
        await updater.cancel()

    async def execute(
        self, context: RequestContext, event_queue: EventQueue
    ) -> None:
        """处理 A2A Task 的核心逻辑。"""
        user_message = context.message
        task_id = context.task_id
        context_id = context.context_id

        if not user_message or not task_id or not context_id:
            return

        self.running_tasks.add(task_id)

        logger.info(
            "[QBAgentExecutor] Task %s received: %s (context: %s)",
            task_id,
            user_message.message_id,
            context_id,
        )

        # 1. 标记为 SUBMITTED
        await event_queue.enqueue_event(
            Task(
                id=task_id,
                context_id=context_id,
                status=TaskStatus(state=TaskState.TASK_STATE_SUBMITTED),
                history=[user_message],
            )
        )

        updater = TaskUpdater(
            event_queue=event_queue,
            task_id=task_id,
            context_id=context_id,
        )

        # 2. 标记为 WORKING
        working_message = updater.new_agent_message(
            parts=[Part(text="Processing...")]
        )
        await updater.start_work(message=working_message)

        # 3. 获取用户输入并处理
        query = context.get_user_input()
        logger.info("[QBAgentExecutor] Query: %s", query)

        # 模拟处理时间
        await asyncio.sleep(0.5)

        # 4. 生成回复
        reply = self._handle_query(query)

        # 5. 如果任务没被取消，发送结果
        if task_id not in self.running_tasks:
            return

        await updater.add_artifact(
            parts=[Part(text=reply)],
            name="response",
            last_chunk=True,
        )
        await updater.complete()

        logger.info(
            "[QBAgentExecutor] Task %s completed", task_id
        )

    def _handle_query(self, query: str) -> str:
        """解析 query 并生成回复。当前是 Hello World 版。

        v2 将接入真正的 coding agent 能力。
        """
        if not query:
            return "Hello from q-body! Send me a message to get started."

        ql = query.lower().strip()

        # 简单问候
        if any(g in ql for g in ["hello", "hi", "你好"]):
            return "Hello! This is q-body speaking via A2A protocol. Ready to evolve! 🫧"

        # 技能查询
        if "skill" in ql or "capability" in ql or "能力" in ql:
            return (
                "My current capabilities:\n"
                "  ✦ A2A Task Processing (v1)\n"
                "  ✦ Self-Evolution (via evolution_log)\n"
                "  ✦ Soul: SOUL.md\n\n"
                "Coming soon:\n"
                "  ⌛ Coding agent (Python + Rust)\n"
                "  ⌛ Gap analysis & self-improvement\n"
                "  ⌛ Daily self-check execution"
            )

        # 默认回应
        return f"q-body received: '{query}'. A2A protocol working. What's next, boss? 🫧"