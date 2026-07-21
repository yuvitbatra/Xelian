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
| CLI (`xelian`) | ✅ Released — v0.1.1 binaries (macOS + Linux, arm64/x86_64), baked to the live registry |
| One-line install (`curl \| sh`) | ✅ Live & verified from the internet |
| `xelian add` (GitHub importer) | ✅ Live — ~30 real repos verified, incl. MCP handshakes |
| `xelian run owner/name` (public) | ✅ **LIVE** — resolves published archives AND the 847-package catalog |
| Registry service (public) | ✅ **LIVE** — https://xelian-registry.onrender.com |
| Discovery catalog | ✅ 847 curated entries (500 servers + 347 agents), all permissive licenses |
| Website (`/explore` + catalog on home) | ✅ Builds; deploy to Vercel when ready |
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

## The registry is LIVE

- **URL:** https://xelian-registry.onrender.com (`/health`, `/catalog`, `/packages`)
- **847 packages** are browsable (`/catalog`) and **runnable by name**:
  `xelian run owner/repo` resolves via the catalog to the GitHub source and runs
  it under its own license (Xelian hosts no third-party code).
- v0.1.1 binaries are baked to this URL, so `curl | sh` install → `xelian run`
  "just works".

### What still needs YOU (each is one action)
1. **Publish the 16 example packages** so `xelian run xelian/calc` works: I need
   the `xelian` account password you set (or your OK to reset it — its old
   archives were lost to Render's first ephemeral disk). Then:
   `XELIAN_SEED_PASSWORD=… scripts/publish_seed.sh`
2. **Deploy the website** (Vercel, free) so the 847 are browsable in a UI. Set
   `NEXT_PUBLIC_REGISTRY_URL=https://xelian-registry.onrender.com`.
3. **Homebrew tap** (`Formula/xelian.rb` is ready): create a public repo
   `homebrew-xelian`, put the formula in it, then `brew tap yuvitbatra/xelian &&
   brew install xelian`.
4. **SDK on PyPI** (verified build-ready — wheel + sdist pass `twine check`,
   and `xelian.mcp()` works against the live registry): `cd sdk &&
   python -m build && twine upload dist/*` (needs a PyPI account/token).

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
