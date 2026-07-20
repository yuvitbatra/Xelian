#!/usr/bin/env bash
# Publish every package under seed/ to a registry under the official `xelian`
# namespace (Phase 24 seed content).
#
# Usage:  scripts/publish_seed.sh            # against http://localhost:8000
#         XELIAN_REGISTRY_URL=... scripts/publish_seed.sh
#
# The account password MUST come from XELIAN_SEED_PASSWORD. It is never written
# to disk (C-2): a plaintext password file is a standing credential leak. On the
# very first run, generate one, print it once, and tell the operator to store it
# in a password manager (and export it for future runs). After the first login
# the CLI's revocable token in ~/.xelian/credentials.toml carries subsequent
# pushes, so the password is only needed to (re)authenticate.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="$ROOT/target/debug/xelian"
[ -f "$BINARY" ] || { echo "FATAL: build first — missing $BINARY"; exit 1; }

OWNER="xelian"
REGISTRY_URL="${XELIAN_REGISTRY_URL:-http://localhost:8000}"
export XELIAN_REGISTRY_URL="$REGISTRY_URL"

# One-time migration: remove any plaintext password left by older versions.
LEGACY_PASS_FILE="$HOME/.xelian/seed-account-password.txt"
if [ -f "$LEGACY_PASS_FILE" ]; then
  echo "note: removing legacy plaintext password file $LEGACY_PASS_FILE (C-2)."
  echo "      if you still need it, copy it into a password manager NOW, then re-run."
  rm -f "$LEGACY_PASS_FILE"
fi

if [ -n "${XELIAN_SEED_PASSWORD:-}" ]; then
  PASSWORD="$XELIAN_SEED_PASSWORD"
else
  PASSWORD="$(openssl rand -hex 12)"
  echo "Generated a password for the '$OWNER' account (shown once — save it now):"
  echo
  echo "    $PASSWORD"
  echo
  echo "Store it in a password manager, then for future runs:"
  echo "    export XELIAN_SEED_PASSWORD='<that password>'"
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
