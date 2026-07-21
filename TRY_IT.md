# Try Xelian yourself

Copy-paste commands to verify everything Xelian does. Every command here has
been run and confirmed to work.

## Fastest: one command

```bash
scripts/try-it.sh          # full tour: build → add a repo → push → run from a clean machine
scripts/try-it.sh add      # just the `xelian add` half (no Postgres needed)
```

The script is self-contained: it builds the CLI, spins up a throwaway local
Postgres + registry, and cleans everything up on exit. Nothing touches your
real `~/.xelian` or any remote service.

---

## Or step by step

### 0. Build

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

### 1. Run any public repo from GitHub — no packaging (`xelian add`)

```bash
# TypeScript MCP server — Xelian builds it automatically
xelian add https://github.com/zcaceres/fetch-mcp

# Python MCP server from a monorepo subdirectory
xelian add https://github.com/modelcontextprotocol/servers/tree/main/src/git

# Monorepo *root* — Xelian finds the runnable package inside it
xelian add https://github.com/upstash/context7

# A library with no entrypoint — fails fast and tells you the one field to set
xelian add https://github.com/openai/swarm
```

**Prove a launched server is real** (completes an MCP handshake):

```bash
printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"me","version":"1"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | xelian add https://github.com/zcaceres/fetch-mcp 2>/dev/null
```

You'll see the server's real tool list come back as JSON-RPC.

See [test_add.txt](test_add.txt) for ~30 repos with their measured status, and
[ATTRIBUTIONS.md](ATTRIBUTIONS.md) for who built them and under which license.

### 2. Package and run your own agent

```bash
mkdir my-agent && cd my-agent
xelian init                       # scaffolds a runnable echo agent
xelian run ./my-agent-0.1.0.xelian   # (build it first with `xelian push`, or run the archive)
```

When it launches you'll see a readiness line and a `> ` prompt — type a message
and press enter.

### 3. The registry loop (like `ollama run`)

Needs a local Postgres (`brew install postgresql`). The `try-it.sh` script does
all of this for you; here it is by hand:

```bash
# start Postgres + the registry
initdb -D /tmp/xpgdata -U postgres --auth=trust --encoding=UTF8
pg_ctl -D /tmp/xpgdata -o "-p 5456" -l /tmp/xpg.log start
psql -h localhost -p 5456 -U postgres -c "CREATE DATABASE xelian;"

export DATABASE_URL="postgresql+psycopg://postgres@localhost:5456/xelian"
cd registry && uv run uvicorn app.main:app --port 8000 &

# publish, then run from a clean machine
export XELIAN_REGISTRY_URL=http://localhost:8000
curl -s -X POST localhost:8000/auth/signup -H 'content-type: application/json' \
     -d '{"username":"me","password":"mypassword"}'
echo mypassword | xelian login --username me --password-stdin
( cd seed/calc && xelian push )
echo '2*(3+4)**2' | xelian run me/calc        # → 98
```

Publish all 16 bundled example packages at once:

```bash
XELIAN_SEED_PASSWORD=mypassword scripts/publish_seed.sh
```

---

## Run the tests

```bash
cargo test --workspace          # Rust: CLI + runtime (285 tests)
cargo clippy --workspace --all-targets
scripts/e2e.sh                  # full push→run→yank→rm lifecycle against a live registry

# registry test suite — uses a throwaway DB whose name must contain "test"
cd registry
XELIAN_TEST_DATABASE_URL="postgresql+psycopg://postgres@localhost:5456/xelian_test" \
  uv run pytest -q
```
