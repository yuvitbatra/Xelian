#!/usr/bin/env bash
# Publish every package under seed/ to a registry under the official `xelian`
# namespace (Phase 24 seed content).
#
# Usage:  scripts/publish_seed.sh            # against http://localhost:8000
#         XELIAN_REGISTRY_URL=... scripts/publish_seed.sh
#
# The account password comes from XELIAN_SEED_PASSWORD, or is generated on
# first run and stored (0600) in ~/.xelian/seed-account-password.txt.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="$ROOT/target/debug/xelian"
[ -f "$BINARY" ] || { echo "FATAL: build first — missing $BINARY"; exit 1; }

OWNER="xelian"
REGISTRY_URL="${XELIAN_REGISTRY_URL:-http://localhost:8000}"
export XELIAN_REGISTRY_URL="$REGISTRY_URL"

PASS_FILE="$HOME/.xelian/seed-account-password.txt"
if [ -n "${XELIAN_SEED_PASSWORD:-}" ]; then
  PASSWORD="$XELIAN_SEED_PASSWORD"
elif [ -f "$PASS_FILE" ]; then
  PASSWORD="$(cat "$PASS_FILE")"
else
  PASSWORD="$(openssl rand -hex 12)"
  mkdir -p "$HOME/.xelian"
  (umask 077 && printf '%s' "$PASSWORD" > "$PASS_FILE")
  echo "Generated password for the '$OWNER' account -> $PASS_FILE"
fi

curl -sf "$REGISTRY_URL/health" > /dev/null || {
  echo "FATAL: no registry at $REGISTRY_URL — start it first:"
  echo "  cd registry && DATABASE_URL=postgresql+psycopg://postgres:postgres@localhost:5433/xelian uv run uvicorn app.main:app --port 8000"
  exit 1
}

# Sign up (409 = account already exists, fine), then log the CLI in.
code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$REGISTRY_URL/auth/signup" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"$OWNER\",\"password\":\"$PASSWORD\"}")
case "$code" in
  201) echo "Created account '$OWNER'";;
  409) echo "Account '$OWNER' already exists";;
  *)   echo "FATAL: signup returned $code"; exit 1;;
esac
printf '%s' "$PASSWORD" | "$BINARY" login --username "$OWNER" --password-stdin

published=0 skipped=0
for pkg in "$ROOT"/seed/*/; do
  name="$(basename "$pkg")"
  # Push from a scratch copy so seed/ stays pristine (push writes the
  # lockfile + archive into the package directory).
  work="$(mktemp -d)"
  cp -R "$pkg"/. "$work/"
  git -C "$work" init -q && git -C "$work" add -A
  if out=$(cd "$work" && "$BINARY" push 2>&1); then
    echo "published $OWNER/$name"
    published=$((published + 1))
  elif echo "$out" | grep -q "already published"; then
    echo "skipped   $OWNER/$name (version already published)"
    skipped=$((skipped + 1))
  else
    echo "FAILED    $OWNER/$name:"; echo "$out"; rm -rf "$work"; exit 1
  fi
  rm -rf "$work"
done

echo
echo "Done: $published published, $skipped already present."
"$BINARY" search mcp | head -20 || true
