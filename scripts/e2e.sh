#!/usr/bin/env bash
# H-211 — the real push→run loop, driven through the actual binary against the
# actual registry: signup → login → push → duplicate-push(409) → clean-cache
# run (agent responds) → yank → run fails → unyank → run works → rm.
#
# This is the test class that catches cross-implementation drift (like the
# checksum-dialect bug that once shipped green): the two sides are exercised
# against each other, never only against themselves.
#
# Usage: scripts/e2e.sh   (from the repo root; binary must be built)
# Env:   PYTHON — interpreter with registry deps (default registry/.venv/bin/python, then python3)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="$ROOT/target/debug/xelian"
[ -f "$BINARY" ] || { echo "FATAL: build first — missing $BINARY"; exit 1; }

PYTHON="${PYTHON:-}"
if [ -z "$PYTHON" ]; then
  if [ -x "$ROOT/registry/.venv/bin/python" ]; then PYTHON="$ROOT/registry/.venv/bin/python"; else PYTHON=python3; fi
fi

WORK="$(mktemp -d)"
PORT=$((20000 + RANDOM % 20000))
REGISTRY_URL="http://127.0.0.1:$PORT"
RUN_ID="$RANDOM$RANDOM"
USER_NAME="e2e-$RUN_ID"
export HOME="$WORK/home"
export XELIAN_REGISTRY_URL="$REGISTRY_URL"
export XELIAN_REGISTRY_ROOT="$WORK/registry-root"
# Postgres is required (no SQLite fallback — decision of record). Default to
# the local dev container; CI provides its own service DATABASE_URL.
export DATABASE_URL="${DATABASE_URL:-postgresql+psycopg://postgres:postgres@localhost:5433/xelian}"
mkdir -p "$HOME"

cleanup() {
  [ -n "${REGISTRY_PID:-}" ] && kill "$REGISTRY_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

step() { echo; echo "==> $*"; }

step "boot registry on $PORT"
(cd "$ROOT/registry" && "$PYTHON" -m uvicorn app.main:app --port "$PORT" >"$WORK/registry.log" 2>&1) &
REGISTRY_PID=$!
for _ in $(seq 1 60); do
  curl -sf "$REGISTRY_URL/health" >/dev/null 2>&1 && break
  sleep 0.5
done
curl -sf "$REGISTRY_URL/health" >/dev/null || { echo "FATAL: registry did not come up"; cat "$WORK/registry.log"; exit 1; }

step "signup + non-interactive login"
curl -sf -X POST "$REGISTRY_URL/auth/signup" -H 'Content-Type: application/json' \
  -d "{\"username\":\"$USER_NAME\",\"password\":\"password123\"}" >/dev/null
echo password123 | "$BINARY" login --username "$USER_NAME" --password-stdin

step "author a real agent package"
PKG="$WORK/e2e-agent"
mkdir -p "$PKG/src"
cat > "$PKG/xelian.toml" <<'EOF'
spec-version = 1
name = "e2e-agent"
version = "0.1.0"
description = "E2E fixture agent"
package-type = "agent"
language = "python"
runtime = ">=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = []
features = []

[author]
name = "E2E"
email = "e2e@example.com"

[dependencies]
manifest = "pyproject.toml"
EOF
cat > "$PKG/src/main.py" <<'EOF'
import sys
for line in sys.stdin:
    print(f"echo: {line.strip()}", flush=True)
EOF
printf '[project]\nname = "e2e-agent"\nversion = "0.1.0"\nrequires-python = ">=3.11"\n' > "$PKG/pyproject.toml"
printf '# e2e-agent\n' > "$PKG/README.md"
printf 'MIT\n' > "$PKG/LICENSE"
git -C "$PKG" init -q && git -C "$PKG" add -A

step "push"
(cd "$PKG" && "$BINARY" push)

step "duplicate push must fail with the 409 immutability error"
if (cd "$PKG" && "$BINARY" push) >"$WORK/dup.log" 2>&1; then
  echo "FATAL: duplicate push succeeded"; exit 1
fi
grep -q "already published" "$WORK/dup.log" || { echo "FATAL: wrong duplicate-push error:"; cat "$WORK/dup.log"; exit 1; }

step "clean-cache run: agent must respond"
RESPONSE="$(printf 'hi\n' | "$BINARY" run "$USER_NAME/e2e-agent" 2>"$WORK/run.log")"
echo "agent said: $RESPONSE"
[ "$RESPONSE" = "echo: hi" ] || { echo "FATAL: expected 'echo: hi'"; cat "$WORK/run.log"; exit 1; }

step "xelian search finds the package"
"$BINARY" search e2e-agent | grep -q "$USER_NAME/e2e-agent" || { echo "FATAL: search did not find the package"; exit 1; }

step "yank, then run must fail with no-resolvable-version"
"$BINARY" yank "$USER_NAME/e2e-agent" --version 0.1.0
"$BINARY" rm "$USER_NAME/e2e-agent" >/dev/null   # drop the cache so resolution is exercised
if printf 'hi\n' | "$BINARY" run "$USER_NAME/e2e-agent" >"$WORK/yanked.log" 2>&1; then
  echo "FATAL: run succeeded on a fully-yanked package"; exit 1
fi
grep -qi "no resolvable" "$WORK/yanked.log" || { echo "FATAL: wrong yanked-run error:"; cat "$WORK/yanked.log"; exit 1; }

step "unyank, run works again"
"$BINARY" yank "$USER_NAME/e2e-agent" --version 0.1.0 --undo
RESPONSE="$(printf 'back\n' | "$BINARY" run "$USER_NAME/e2e-agent" 2>/dev/null)"
[ "$RESPONSE" = "echo: back" ] || { echo "FATAL: run after unyank failed"; exit 1; }

step "rm cleans the cache"
"$BINARY" rm "$USER_NAME/e2e-agent"
"$BINARY" list | grep -q e2e-agent && { echo "FATAL: rm left the package cached"; exit 1; }

echo
echo "E2E PASS: signup -> login -> push -> 409 -> run -> yank -> fail -> unyank -> run -> rm"
