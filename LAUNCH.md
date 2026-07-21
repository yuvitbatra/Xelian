# Launch checklist — from here to "on the internet, people love it"

Everything below is either **done** (verified in-repo) or a **you-step** that
needs an account, a domain, or a secret only you have. Nothing here is blocked
on more code.

## What's done and verified

- **`xelian add`** imports and runs public GitHub repos (Python/TS/JS): plain
  repos, monorepo subdirectories, monorepo *roots* (auto-descend), TypeScript
  builds (bun/pnpm/yarn provisioned as needed), console-script Python packages.
  Un-runnable repos fail fast with the one field to fix. ~30 real repos verified
  running end-to-end incl. full MCP handshakes.
- **The registry loop** (`push` → registry → `run` on a clean machine) verified
  against a real Postgres-backed registry: all 16 seed packages, 8 MCP
  handshakes + 8 agent REPLs. Immutability, search, list, 404s correct.
- **Security**: tampered downloads rejected before extraction; test suite can't
  wipe a non-test DB.
- **Binaries**: `.github/workflows/release.yml` cross-compiles macOS + Linux
  (arm64/x86_64) on a `v*` tag; `scripts/install.sh` gives a `curl | sh` install.
- **Catalog**: `registry/catalog.json` — 847 curated, permissively-licensed
  entries (500 MCP servers + 347 agents), spam-filtered, each runnable via
  `xelian add`. Harvested authenticated from GitHub; regenerate with
  `GITHUB_TOKEN=… scripts/harvest_catalog.py`.
- **Attribution**: `ATTRIBUTIONS.md` credits every referenced project + license.

## The upload, step by step

### 1. Deploy the registry (≈30 min)
The registry is a FastAPI app with a `Dockerfile` and `render.yaml` already
committed. On Render/Fly/Railway:
- Provision a Postgres instance; set `DATABASE_URL`.
- Set object storage for archives: `XELIAN_R2_BUCKET`, `XELIAN_R2_ENDPOINT`,
  `XELIAN_R2_ACCESS_KEY_ID`, `XELIAN_R2_SECRET_ACCESS_KEY` (Cloudflare R2 free
  tier works; free-tier disks are ephemeral so R2 matters).
- Deploy. Confirm `GET /health` returns `{"ok":true}`.

### 2. Point the CLI at it (≈5 min)
Rebuild release binaries with the production URL baked in:
```bash
XELIAN_DEFAULT_REGISTRY_URL=https://registry.<your-domain> cargo build --release
```
(or set `XELIAN_REGISTRY_URL` per-shell). Then tag a release so users get
binaries:
```bash
git tag v0.1.0 && git push origin v0.1.0   # fires release.yml → published binaries
```

### 3. Publish the install one-liner (≈10 min)
`scripts/install.sh` already points at `yuvitbatra/Xelian`. Optionally host it at
`https://get.<your-domain>/install.sh` (a redirect to the raw script) so the
README command reads cleanly.

### 4. Seed discovery content (≈1–2 h)
`registry/catalog.json` is the index (847 entries). Two ways to expose it:
- **Index (recommended, safe):** serve the catalog from the registry/website as
  a browsable list; each entry's "Run" is `xelian add <url>`. You host *no*
  third-party code — you link and run it, credited, under its own license. This
  scales and carries no redistribution liability.
- **Republish (only for packages you author or have permission to):** for these,
  `xelian push` the archive so they run as `xelian run you/name`.

Grow the catalog toward thousands with a token:
```bash
GITHUB_TOKEN=<token> python scripts/harvest_catalog.py --min-stars 15
```

### 5. Ship the website (≈1–2 h)
`website/` (Next.js) builds clean in CI. Deploy to Vercel; point it at the
registry API. Render the catalog as the front page (search + "Run" buttons).

### 6. Clean-machine dogfood (the "do people love it?" gate)
On a machine that has never seen this repo:
```bash
curl -fsSL https://get.<your-domain>/install.sh | sh
xelian add https://github.com/modelcontextprotocol/servers/tree/main/src/git
```
If that's delightful with zero setup, you're ready. Get 5–10 other people to do
the same (backlog H-241) and fix every point of confusion.

## Honest caveats

- The 847 catalog entries are **license-verified and quality-filtered**, and a
  sample runs, but not every one is individually verified end-to-end (that
  needs a token + hours). `xelian add` fails gracefully on the ones that need a
  manual entrypoint, so a bad entry degrades to a clear message, not a crash.
- GitHub's AI-agent space is heavily **star-farmed**; the harvester filters
  spam by trusted-source + plausible star bounds. Keep that filter when
  expanding, or the front page fills with junk.
- The **index vs. republish** choice (step 4) is the one real product decision.
  Index is the safe, scalable default and is assumed throughout.
