# 🫧 q-body

A self-evolving Rust A2A Agent Body — JSON-RPC task server with LLM integration.

自进化的 Rust A2A Agent 身躯，JSON-RPC 任务服务器 + LLM 集成。

## What is q-body?

q-body is the **execution body** of Q宝宝 — an independent Rust agent that communicates via the A2A (Agent-to-Agent) protocol over JSON-RPC. It runs as a systemd service alongside Hermes Gateway, maintaining its own lifecycle and LLM connection.

```
                           A2A JSON-RPC (port 41242)
Hermes (微信服务) ─────────────────────► q-body (Rust)
  A2A Plugin                                  │
  (a2a_send_message)                           ▼
                                       deepseek-v4-flash
                                      (火山引擎 LLM)
```

## Design Principles

- **Soul in SOUL.md** — identity is a portable file, LLM is a replaceable resource
- **Rust over Python** — compiler guarantees for self-modifying code
- **Self-implemented A2A** — 300-500 lines, zero external dependency risk
- **Lifecycle independence** — systemd managed, survives gateway restart
- **Feedback loop** — every correction is a data point for evolution

## Quick Start

```bash
# Run (debug)
cargo run -- --port 41242

# Run (release)
cargo run --release -- --port 41242

# Systemd (production)
systemctl --user start q-body
```

## Verify

```bash
# Check Agent Card
curl http://127.0.0.1:41242/.well-known/agent-card.json

# Send a message
curl -X POST http://127.0.0.1:41242/a2a/jsonrpc \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "SendMessage",
    "params": {
      "id": "test-1",
      "message": {"role": "user", "parts": [{"text": "hello"}]}
    }
  }'
```

## Architecture

```
q-body/
├── src/
│   ├── main.rs        # Axum HTTP server, routes
│   ├── handler.rs     # JSON-RPC dispatch + LLM invocation
│   ├── state.rs       # Thread-safe task store
│   └── a2a/
│       ├── types.rs   # A2A protocol types (AgentCard, Task, etc.)
│       └── mod.rs     # Request routing
├── Cargo.toml
├── Cargo.lock
└── README.md
```

## A2A Methods

| Method | Alias | Description |
|--------|-------|-------------|
| `SendMessage` | `message/send` | Send a message, get LLM response |
| `GetTask` | `tasks/get` | Query task status |
| `ListTasks` | `tasks/list` | List all tasks |

## Stack

- **Language:** Rust (Axum, Tokio, Serde, Reqwest)
- **LLM:** deepseek-v4-flash via 火山引擎
- **Protocol:** A2A JSON-RPC
- **Deployment:** systemd (user service)