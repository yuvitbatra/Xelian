# MCP Gateway design (H-260, owner request 2026-07-19)

## Problem

Using N MCP servers from an IDE (Cursor) or agent framework (CrewAI,
LangChain) means N hardcoded config entries. A crashed or re-schema'd server
breaks the consumer, and observability is N terminal tabs.

## Solution

`xelian gateway` — a single local Streamable-HTTP MCP endpoint
(`http://127.0.0.1:11432/mcp`, Ollama-style fixed local port) that fronts
every configured Xelian MCP package.

- **Config**: `~/.xelian/gateway.toml` (`packages = ["owner/name", ...]`,
  optional `port`), edited via `xelian gateway add|remove|list`.
- **Serve**: each backend goes through the standard run pipeline (resolve →
  download/cache → env provisioning → permission check → env vars), then is
  spawned as a stdio child with stderr redirected to
  `~/.xelian/logs/gateway/<owner>-<name>.log`. The gateway performs the MCP
  initialize handshake with each backend.
- **Routing**: tools are exposed as `<package>__<tool>` (charset-sanitized;
  alias collisions are a startup error). `tools/list` fans out and merges;
  `tools/call` strips the prefix and forwards. Per-backend mutex + id
  remapping; 120 s call timeout via a reader thread + channel.
- **Resilience**: a dead backend is respawned on the next request that needs
  it; restart counts are reported.
- **Status**: `GET /status` (JSON) and `xelian gateway status` (CLI) show
  up/down, version, restarts, and log path per backend. `xelian gateway
  logs [owner/name]` tails the unified logs.

## Protocol scope (MVP, deliberate)

`initialize`, `ping`, `tools/list`, `tools/call`; client notifications
accepted (202) and dropped; everything else -32601. The gateway declares only
the `tools` capability, so conforming clients won't request
resources/prompts. Responses are plain JSON (no SSE); server→client requests
from backends are dropped. Extend only when a real client needs more.

## Implementation

`crates/xelian-cli/src/gateway.rs` (~700 lines, sync, `tiny_http` + 4 worker
threads — no async runtime). The spawn path mirrors `launch.rs` but with
piped stdio, since the gateway owns its children.

## Verified

Live: real package pushed to a local registry, curl MCP session
(initialize → tools/list namespaced → tools/call routed), kill child →
next call auto-respawns (restarts: 1), alias collision and unknown-tool
errors are actionable. Unit tests cover message handling.
