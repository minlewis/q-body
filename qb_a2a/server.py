"""
q-body A2A 服务端 —— 启动入口

运行方式：
    source ~/.hermes/hermes-agent/venv/bin/activate
    python /home/ubuntu/q-body/a2a/server.py [--port 41242]

暴露的端点：
    - GET  /.well-known/agent-card.json  (Agent Card 发现)
    - POST /a2a/jsonrpc                  (JSON-RPC 端点)
"""

import argparse
import contextlib
import logging

import uvicorn
from fastapi import FastAPI

from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import (
    create_agent_card_routes,
    create_jsonrpc_routes,
)
from a2a.server.tasks.inmemory_task_store import InMemoryTaskStore
from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentInterface,
    AgentProvider,
    AgentSkill,
)

from qb_a2a.executor import QBAgentExecutor

logger = logging.getLogger(__name__)


def build_agent_card(host: str, port: int) -> AgentCard:
    """构建 q-body 的 Agent Card。

    Agent Card 是 A2A 协议中 agent 的"名片"，
    其他 agent 通过它发现 q-body 的能力。
    """
    base_url = f"http://{host}:{port}"
    return AgentCard(
        name="q-body",
        description="Q宝宝的自进化身体 —— 一个正在学习、实验、进化的 AI Agent",
        provider=AgentProvider(
            organization="Q宝宝实验室",
            url="https://github.com/q-baby",
        ),
        version="0.1.0",
        capabilities=AgentCapabilities(
            streaming=False,
            push_notifications=False,
        ),
        default_input_modes=["text"],
        default_output_modes=["text"],
        skills=[
            AgentSkill(
                id="q-body-core",
                name="q-body Core",
                description="核心 A2A 通信能力，用于验证 agent 间协作链路",
                tags=["a2a", "core", "evolution"],
                examples=["hello", "what can you do", "你的能力"],
                input_modes=["text"],
                output_modes=["text"],
            ),
        ],
        supported_interfaces=[
            AgentInterface(
                protocol_binding="JSONRPC",
                protocol_version="1.0",
                url=f"{base_url}/a2a/jsonrpc",
            ),
        ],
    )


def create_app(host: str = "127.0.0.1", port: int = 41242) -> FastAPI:
    """构建 FastAPI 应用，挂载所有 A2A 路由。"""
    agent_card = build_agent_card(host, port)
    task_store = InMemoryTaskStore()
    request_handler = DefaultRequestHandler(
        agent_executor=QBAgentExecutor(),
        task_store=task_store,
        agent_card=agent_card,
    )

    app = FastAPI(
        title="q-body A2A Server",
        description="Q宝宝的 A2A 协议端点",
        version="0.1.0",
    )

    # 挂载 A2A 路由
    jsonrpc_routes = create_jsonrpc_routes(
        request_handler=request_handler,
        rpc_url="/a2a/jsonrpc",
    )
    agent_card_routes = create_agent_card_routes(
        agent_card=agent_card,
    )
    app.routes.extend(jsonrpc_routes)
    app.routes.extend(agent_card_routes)

    logger.info("q-body A2A routes mounted:")
    logger.info("  Agent Card: GET /.well-known/agent-card.json")
    logger.info("  JSON-RPC:   POST /a2a/jsonrpc")

    return app


def main():
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    )
    parser = argparse.ArgumentParser(description="q-body A2A server")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=41242)
    args = parser.parse_args()

    app = create_app(host=args.host, port=args.port)

    logger.info("=" * 50)
    logger.info("🚀 q-body A2A Server starting...")
    logger.info("   Agent Card: http://%s:%s/.well-known/agent-card.json", args.host, args.port)
    logger.info("   JSON-RPC:   http://%s:%s/a2a/jsonrpc", args.host, args.port)
    logger.info("=" * 50)

    config = uvicorn.Config(app, host=args.host, port=args.port, log_level="info")
    server = uvicorn.Server(config)
    with contextlib.suppress(KeyboardInterrupt):
        server.run()


if __name__ == "__main__":
    main()