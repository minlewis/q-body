# q-body

> The execution body of Q宝宝 — a self-evolving Rust A2A Agent.

q-body is an **A2A (Agent-to-Agent) protocol agent** written in Rust. It communicates via JSON-RPC over HTTP, runs as a systemd service alongside Hermes Gateway, and maintains its own LLM connection (via deepseek-v4-flash on Volcengine Ark).

```text
                          A2A JSON-RPC (port 41242)
Hermes (Chat Gateway) ──────────────────────► q-body (Rust)
  A2A Plugin                                          │
  (send_message)                                       ▼
                                                Volcengine Ark
                                              (deepseek-v4-flash)
```

## Project Constitution

q-body follows a Taoist engineering philosophy: **为学日益，为道日损** (Learning accumulates; the Way subtracts).

- **Learning has no ceiling** — LEARNINGS.md only grows. Study every major agent framework, absorb every pattern.
- **Code has a hard ceiling** — ≤3000 lines of Rust, ≤15 dependencies. Every PR must answer: *what did this subtract?*
- **Soul over body** — If a feature can live in SOUL.md (prompt/memory), it doesn't belong in src/.

Read the full constitution: [soul/TAO.md](soul/TAO.md)

## Architecture

```
q-body/
├── src/
│   ├── main.rs          # Entry point: Axum server + routes
│   ├── handler.rs       # JSON-RPC method dispatch
│   ├── state.rs         # Application state + LLM client
│   └── a2a/
│       ├── mod.rs       # A2A protocol service layer
│       └── types.rs     # AgentCard, Task, Message, Part types
├── soul/
│   ├── TAO.md           # Project constitution (this file)
│   └── SOUL.md          # Portable identity file (Chinese)
├── Cargo.toml
└── README.md
```

## Key Design Decisions

| Principle | Rationale |
|-----------|-----------|
| **Soul in SOUL.md** | Identity is a portable file; the LLM is a replaceable compute resource |
| **Rust over Python** | Compiler guarantees for self-modifying code — less drama than Python |
| **Self-implemented A2A** | ~300 lines of Rust, zero external agent framework dependencies |
| **systemd-managed** | Lifecycle independent of Hermes Gateway — survives gateway restarts |
| **Feedback loop** | Every correction is a data point for evolution |

## Quick Start

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs/))
- A Volcengine Ark API key with access to `deepseek-v4-flash`

### Build & Run

```bash
git clone https://github.com/minlewis/q-body.git
cd q-body

# Set your LLM endpoint
export ARK_API_KEY="your-key-here"
export ARK_BASE_URL="https://ark.cn-beijing.volces.com/api/plan/v3"

# Build and run
cargo build --release
./target/release/q-body

# Or install as a systemd service
sudo cp q-body.service /etc/systemd/system/
sudo systemctl enable --now q-body
```

### Verify

```bash
# Check Agent Card
curl http://127.0.0.1:41242/.well-known/agent-card.json

# Send a message via A2A JSON-RPC
curl -X POST http://127.0.0.1:41242/a2a/jsonrpc \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "method": "message/send",
    "params": {
      "id": "test-1",
      "message": {
        "role": "user",
        "parts": [{"type": "text", "text": "Hello"}]
      }
    },
    "id": 1
  }'
```

## A2A Protocol Support

Implements the [A2A Protocol](https://github.com/a2aproject/A2A) core methods:

| Method | Endpoint | Description |
|--------|----------|-------------|
| `message/send` | `POST /a2a/jsonrpc` | Send message, get response |
| `tasks/send` | `POST /a2a/jsonrpc` | Create a task |
| `tasks/get` | `POST /a2a/jsonrpc` | Get task status |
| `tasks/list` | `POST /a2a/jsonrpc` | List active tasks |
| Agent Card | `GET /.well-known/agent-card.json` | Agent discovery |

## soul/

`soul/SOUL.md` is the identity file of Q宝宝 (written in Chinese). It contains the agent's core personality, cognitive framework, and evolution principles. Any framework that loads this file inherits the agent's soul.

## License

MIT