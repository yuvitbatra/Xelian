#!/usr/bin/env bash
# Reset the live registry's database and re-seed the 16 example packages so
# `xelian run xelian/calc` works. Run this yourself — it performs a destructive
# wipe of the production database, which the assistant's safety guard (rightly)
# won't do automatically.
#
# What it does:
#   1. Wipes all rows (users/tokens/packages/versions) — the old `xelian/*`
#      archives were lost to Render's first ephemeral disk, so their DB rows
#      point at bytes that no longer exist.
#   2. Re-creates the `xelian` account with a password you choose.
#   3. Publishes all 16 seed/ packages to the LIVE registry (archives -> R2).
#   4. Smoke-tests `xelian run xelian/calc` (expects 98).
#
# Prereqs: run from the repo root; registry/.env must have DATABASE_URL (Neon)
# and the XELIAN_R2_* keys; a release binary at target/release/xelian.
#
# Usage:
#   XELIAN_SEED_PASSWORD='choose-a-strong-password' scripts/reset_and_seed.sh

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/xelian"
URL="https://xelian-registry.onrender.com"

[ -f "$BIN" ] || cargo build --release
# publish_seed.sh uses the debug binary; make sure it exists too.
[ -f "$ROOT/target/debug/xelian" ] || cargo build
[ -f "$ROOT/registry/.env" ] || { echo "missing registry/.env"; exit 1; }
: "${XELIAN_SEED_PASSWORD:?set XELIAN_SEED_PASSWORD to a strong password}"

echo "==> 1/4 Wiping the production database (destructive)"
( cd "$ROOT/registry" && DATABASE_URL="$(grep '^DATABASE_URL=' .env | cut -d= -f2-)" \
  uv run python - <<'PY'
import os, sys
sys.path.insert(0, '.')
from app import db
from sqlalchemy import text
with db.session() as s:
    for t in ['versions', 'packages', 'tokens', 'users']:
        s.execute(text(f"DELETE FROM {t}"))
    s.commit()
    print("   wiped:", {t: s.execute(text(f'SELECT count(*) FROM {t}')).scalar()
                        for t in ['users','tokens','packages','versions']})
PY
)

echo "==> 2/4 Creating the 'xelian' account on the live registry"
export XELIAN_REGISTRY_URL="$URL"
curl -s -X POST "$URL/auth/signup" -H 'content-type: application/json' \
  -d "{\"username\":\"xelian\",\"password\":\"$XELIAN_SEED_PASSWORD\"}" >/dev/null || true

echo "==> 3/4 Publishing the 16 example packages (archives -> R2)"
"$ROOT/scripts/publish_seed.sh"

echo "==> 4/4 Smoke test: xelian run xelian/calc (expect 98)"
tmp="$(mktemp -d)"
ans="$(echo '2*(3+4)**2' | HOME="$tmp" XELIAN_REGISTRY_URL="$URL" "$BIN" run xelian/calc 2>/dev/null | tail -1)"
echo "   result: $ans"
[ "$ans" = "98" ] && echo "SUCCESS — xelian/calc runs from the live registry" \
  || { echo "unexpected result"; exit 1; }
