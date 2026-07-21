#!/usr/bin/env bash
# One-command tour of Xelian. Builds the CLI, then demonstrates the two things
# it does, each verified as it runs:
#
#   1. `xelian add <github-url>`  — import and run any public repo, no packaging
#   2. the registry loop          — push a package, then run it from a clean
#                                    machine, exactly like `ollama run`
#
# Everything runs locally in a throwaway directory and a local Postgres; nothing
# touches your real ~/.xelian or any remote service. Re-runnable and safe.
#
# Usage:
#   scripts/try-it.sh            # full tour (needs postgres for the registry half)
#   scripts/try-it.sh add        # just the `xelian add` half (no postgres needed)
#
# Requirements: a Rust toolchain (to build), git, and — for the registry half —
# a local `postgres`/`pg_ctl`/`initdb` (e.g. `brew install postgresql`).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/xelian"
MODE="${1:-full}"

say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m✓ %s\033[0m\n' "$*"; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

# --- build -----------------------------------------------------------------
say "Building the xelian CLI (release)"
( cd "$ROOT" && cargo build --release -q ) || die "build failed"
ok "built $BIN"

# --- half 1: xelian add ----------------------------------------------------
ADD_HOME="$(mktemp -d)"
say "xelian add — import and run a real MCP server straight from GitHub"
echo "    (fetch-mcp: a TypeScript server that Xelian builds automatically)"
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"try-it","version":"1"}}}'
LIST='{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
reply="$(printf '%s\n%s\n' "$INIT" "$LIST" \
  | HOME="$ADD_HOME" timeout 400 "$BIN" add https://github.com/zcaceres/fetch-mcp 2>/dev/null | head -c 400 || true)"
echo "$reply" | grep -q '"result"' \
  && ok "fetch-mcp launched and answered a real MCP handshake" \
  || die "fetch-mcp did not complete a handshake"

if [ "$MODE" = "add" ]; then
  echo; ok "add demo complete. Try your own:  xelian add https://github.com/<owner>/<repo>"
  exit 0
fi

# --- half 2: the registry loop --------------------------------------------
command -v initdb >/dev/null || die "postgres not found (brew install postgresql), or run: scripts/try-it.sh add"

PGROOT="$(mktemp -d)"
SOCK="$(mktemp -d /tmp/xpg.XXXX)"     # short path: postgres sockets have a 103-char limit
PGPORT=5456
REG_PORT=8055
REG_URL="http://127.0.0.1:$REG_PORT"
STORE="$(mktemp -d)"

cleanup() {
  [ -n "${REG_PID:-}" ] && kill "$REG_PID" 2>/dev/null || true
  pg_ctl -D "$PGROOT/data" stop -m fast >/dev/null 2>&1 || true
  rm -rf "$PGROOT" "$SOCK" "$STORE" "$ADD_HOME" "${SEED_HOME:-}" "${PULL_HOME:-}" 2>/dev/null || true
}
trap cleanup EXIT

say "Starting a throwaway local Postgres + registry"
# initdb under a UTF-8 locale (NOT LC_ALL=C): a C-locale cluster makes some
# psycopg/SQLAlchemy versions read Postgres's version string as bytes and crash
# on connect. LC_ALL=C is only needed to dodge a macOS "postmaster became
# multithreaded" abort at *start* time, so scope it to that call alone.
initdb -D "$PGROOT/data" -U postgres --auth=trust --encoding=UTF8 >/dev/null 2>&1 || die "initdb failed"
LC_ALL=C pg_ctl -D "$PGROOT/data" -o "-p $PGPORT -k $SOCK -h 127.0.0.1" -l "$SOCK/pg.log" start >/dev/null 2>&1 \
  || { cat "$SOCK/pg.log"; die "postgres failed to start"; }
sleep 2
LC_ALL=C psql -h 127.0.0.1 -p "$PGPORT" -U postgres -c "CREATE DATABASE xelian;" >/dev/null 2>&1
ok "postgres up on 127.0.0.1:$PGPORT"

export DATABASE_URL="postgresql+psycopg://postgres@127.0.0.1:$PGPORT/xelian"
export XELIAN_REGISTRY_ROOT="$STORE"
PYTHON="$ROOT/registry/.venv/bin/python"; [ -x "$PYTHON" ] || PYTHON=python3
( cd "$ROOT/registry" && "$PYTHON" -m uvicorn app.main:app --host 127.0.0.1 --port "$REG_PORT" >"$SOCK/reg.log" 2>&1 ) &
REG_PID=$!
for _ in $(seq 1 20); do curl -sf "$REG_URL/health" >/dev/null 2>&1 && break; sleep 0.5; done
curl -sf "$REG_URL/health" >/dev/null || { cat "$SOCK/reg.log"; die "registry did not come up"; }
ok "registry up at $REG_URL"

export XELIAN_REGISTRY_URL="$REG_URL"
SEED_HOME="$(mktemp -d)"
say "Publishing a package (xelian push)"
curl -s -X POST "$REG_URL/auth/signup" -H 'content-type: application/json' \
  -d '{"username":"demo","password":"demopass123"}' >/dev/null
echo "demopass123" | HOME="$SEED_HOME" "$BIN" login --username demo --password-stdin >/dev/null 2>&1
( cd "$ROOT/seed/calc" && HOME="$SEED_HOME" "$BIN" push >/dev/null 2>&1 ) \
  && ok "pushed demo/calc to the registry" || die "push failed"

PULL_HOME="$(mktemp -d)"
say "Running it from a *clean* machine (xelian run demo/calc)"
echo "    input:  2*(3+4)**2"
answer="$(echo '2*(3+4)**2' | HOME="$PULL_HOME" timeout 200 "$BIN" run demo/calc 2>/dev/null | tail -1)"
echo "    output: $answer"
[ "$answer" = "98" ] && ok "clean-machine pull + run returned the correct answer (98)" \
  || die "expected 98, got: $answer"

echo
printf '\033[1;32m════════════════════════════════════════════════════════\033[0m\n'
ok  "Everything works: add a repo, publish a package, run it anywhere."
printf '\033[1;32m════════════════════════════════════════════════════════\033[0m\n'
echo "Now try your own:"
echo "  $BIN add https://github.com/<owner>/<repo>"
echo "  cd <your-package> && $BIN push && $BIN run <you>/<pkg>"
