# Xelian — Handoff & Status

Last updated: 2026-07-21. This is the single source of truth for where the
project stands, what's live, what's left, and how to operate it.

## What Xelian is

A local-first registry + runtime for AI agents and MCP servers — "Hugging Face
+ Ollama for agents." Two things it does:

1. **`xelian add <github-url>`** — import and run any public GitHub repo
   (Python/TS/JS) locally with zero setup. **Live now.**
2. **`xelian push` / `xelian run owner/name`** — publish packages and run them
   anywhere, Ollama-style. Works locally; goes public once the registry is
   deployed (see `DEPLOY_RENDER.md`).

## Status at a glance

| Piece | State |
|-------|-------|
| CLI (`xelian`) | ✅ Released — v0.1.0 binaries for macOS + Linux (arm64/x86_64) |
| One-line install (`curl \| sh`) | ✅ Live & verified from the internet |
| `xelian add` (GitHub importer) | ✅ Live — ~30 real repos verified, incl. MCP handshakes |
| `xelian push`/`run` (local) | ✅ Verified against real Postgres + R2 |
| Registry service (public) | ⏳ Ready to deploy — needs your Render account (`DEPLOY_RENDER.md`) |
| Discovery catalog | ✅ 847 curated entries (500 servers + 347 agents), all permissive licenses |
| Website (`/explore`) | ✅ Builds; deploy to Vercel when ready |
| R2 storage | ✅ Verified against real credentials |
| Tests | ✅ 288 Rust + 55 registry, clippy clean |

## What's LIVE right now

- **Release:** https://github.com/yuvitbatra/Xelian/releases/tag/v0.1.0
- **Install:**
  ```bash
  curl -fsSL https://raw.githubusercontent.com/yuvitbatra/Xelian/main/scripts/install.sh | sh
  ```
- **Use immediately (no registry needed):**
  ```bash
  xelian add https://github.com/zcaceres/fetch-mcp
  xelian add https://github.com/modelcontextprotocol/servers/tree/main/src/git
  ```

## The ONE thing left: deploy the registry

Everything is ready and verified; it needs your accounts (Neon + Render, both
free). **Follow `DEPLOY_RENDER.md`** — it's written click-by-click. ~30 min.
After that, `xelian push` and `xelian run you/pkg` work publicly.

Storage (Cloudflare R2) and the DB driver are already proven working, so the
deploy is low-risk.

## After deploy — the remaining polish (optional, in priority order)

1. **Bake the production URL into the binaries** so users don't set
   `XELIAN_REGISTRY_URL`. Ask me to wire `XELIAN_DEFAULT_REGISTRY_URL` into
   `.github/workflows/release.yml`, then cut `v0.1.1`.
2. **Seed the live registry** with the 16 example packages
   (`scripts/publish_seed.sh`) — Part 5 of the Render guide.
3. **Deploy the website** (`website/`) to Vercel; set
   `NEXT_PUBLIC_REGISTRY_URL` to your registry URL. Its `/explore` page renders
   the 847-entry catalog with one-click `xelian add` commands.
4. **Dogfood** on a friend's clean machine; fix any confusion (backlog H-241).
5. **Grow the catalog** — `GITHUB_TOKEN=… python scripts/harvest_catalog.py`
   (the token is in `registry/.env`).

## Known limitations (honest)

- **Not every repo runs, by nature.** Libraries with no runnable entrypoint
  (`openai/swarm`, `MineDojo/Voyager`, SDKs like `tavily-python`), repos with no
  manifest (`microsoft/JARVIS`), one that doesn't compile upstream
  (`servers-archived/.../gitlab`), and Docker-only projects (`OpenHands`) can't
  be run — no inference invents an entrypoint that doesn't exist. `xelian add`
  fails these fast with a clear message.
- **Not V1 languages:** Go/Rust/Java repos are rejected with a clear
  "unsupported language" (V1 runs Python + Node).
- **pnpm/yarn `workspace:` deps** in an extracted subpackage aren't installable
  yet (needs workspace-aware install). Tracked as H-120; fails with a clear
  message + workaround.
- **410→847 catalog entries** are license-verified and quality/spam-filtered,
  and a broad sample runs, but not every one is individually run end-to-end.
  Bad entries degrade to a clear error at `add` time, never a crash.

## Repo map

| Path | What |
|------|------|
| `crates/xelian-cli`, `crates/xelian-core` | the `xelian` binary + runtime (Rust) |
| `registry/` | registry API (Python/FastAPI), `Dockerfile`, `render.yaml`, `catalog.json` |
| `website/` | registry website (Next.js), incl. `/explore` |
| `sdk/` | Python SDK wrapping the CLI |
| `scripts/` | `install.sh`, `try-it.sh`, `harvest_catalog.py`, `publish_seed.sh`, `e2e.sh` |
| `seed/` | 16 runnable example packages |

## Key docs

- `DEPLOY_RENDER.md` — deploy the registry (start here to go fully live)
- `LAUNCH.md` — full launch checklist
- `TRY_IT.md` + `scripts/try-it.sh` — one-command tour to verify everything
- `ATTRIBUTIONS.md` — credits + licenses for every referenced project
- `BACKLOG.md` — task history + remaining items (H-120, H-240, H-241, …)

## How to verify everything yourself, right now

```bash
scripts/try-it.sh        # build → add a real repo → local registry → push → run (self-contained)
```

## Secrets

`registry/.env` holds `DATABASE_URL`, the R2 keys, and `GITHUB_TOKEN`. It is
**gitignored and never committed** (verified). Never paste its contents into a
chat, issue, or commit. Rotate the R2 keys and token if they're ever exposed.
