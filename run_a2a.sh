#!/bin/bash
# q-body A2A 服务启动脚本
# 使用 Hermes venv 的 Python 运行

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HERMES_VENV="$HOME/.hermes/hermes-agent/venv"

export PYTHONPATH="$SCRIPT_DIR:$PYTHONPATH"

exec "$HERMES_VENV/bin/python" "$SCRIPT_DIR/qb_a2a/server.py" "$@"