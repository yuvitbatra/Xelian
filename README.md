# Xelian

**Run AI agents like you run models.** Xelian is a local-first registry and
runtime for AI agents and MCP servers — package once, publish with one
command, and anyone can run it locally with zero setup.

```bash
xelian run xelian/calc
```

<!-- TODO(owner): record the 30s install→run→chat GIF with asciinema:
     `asciinema rec demo.cast`, run the quickstart, then convert with agg:
     `agg demo.cast demo.gif` and embed it here. -->

## Why

Models converged on standard artifacts (GGUF); agents never did. Every agent
repo has its own setup: clone, create a venv, install dependencies, read the
README, export the right env vars, hope. Xelian defines a standard package
format for locally runnable agents and MCP servers, plus a runtime that makes
running one feel like `ollama run llama3`.

## Try it in one command

```bash
scripts/try-it.sh        # build → import a real GitHub repo → publish → run from a clean machine
```

Self-contained and safe (throwaway registry, never touches your real
`~/.xelian`). See [TRY_IT.md](TRY_IT.md) for step-by-step commands covering
every capability.

## Quickstart

Install the prebuilt binary — no Rust toolchain needed (once a release is
tagged; the [release workflow](.github/workflows/release.yml) produces the
assets and [`scripts/install.sh`](scripts/install.sh) fetches them):

```bash
curl -fsSL https://raw.githubusercontent.com/yuvitbatra/Xelian/main/scripts/install.sh | sh
```

Or build from source (Rust toolchain required):

```bash
cargo install --path crates/xelian-cli   # 1. install the CLI
xelian run xelian/calc                    # 2. download + run an agent
# 3. chat — agents open a REPL; MCP servers start and expose locally
#    e.g. type  2*(3+4)**2  and press enter
```

`xelian run` resolves the package on the registry, downloads and verifies the
archive against its lockfile checksum, provisions the language runtime
(CPython via uv, or Node) in an isolated cached environment, installs
dependencies, asks consent for the package's declared permissions, and
launches it. Subsequent runs start from cache in seconds.

## Package your own agent in 5 minutes

```bash
mkdir my-agent && cd my-agent
xelian init          # writes xelian.toml + xelian.lock skeletons
# edit xelian.toml: name, description, entrypoint, permissions
xelian login         # once — sign up at the registry website
xelian push          # validate, build the .xelian archive, publish
```

Anyone can now `xelian run <you>/my-agent`.

## Run anything on GitHub, with no packaging step

Most agents and MCP servers were never packaged for Xelian. `xelian add` takes
a plain GitHub URL and works out the rest — language, entrypoint, whether it's
an agent or an MCP server, and how to build it:

```bash
xelian add https://github.com/zcaceres/fetch-mcp
```

That single command resolves the default branch to a commit SHA, downloads and
caches the repo at that SHA, infers a `xelian.toml`, provisions Python or Node,
installs dependencies, runs the project's own build if its entrypoint is a
build output, and launches it. TypeScript servers that compile to `dist/` work
without you knowing they were TypeScript.

**Monorepo subpackages** — how most MCP servers actually ship — work by pasting
the URL GitHub gives you when you browse to the folder:

```bash
xelian add https://github.com/modelcontextprotocol/servers/tree/main/src/git
```

Imports are addressed by commit SHA, so re-running is reproducible and starts
from cache.

### What it can and cannot infer

`xelian add` reads `[project.scripts]`, Poetry's `[tool.poetry.scripts]`,
`setup.py` entry points, `<package>/__main__.py`, and `package.json`'s `main`
and `bin` — including entrypoints that don't exist yet because a build produces
them.

It cannot invent an entrypoint that isn't there. Libraries with no CLI
(`openai/swarm`), and projects that only run via Docker or `make`
(`OpenHands`), stop with the cache path and the single field to set:

```
could not determine how to run this package.

Imported and cached at:
  ~/.xelian/packages/github/openai/swarm/<sha>

Xelian inferred everything except `entrypoint`. To finish the import:
  1. edit <path>/xelian.toml and set `entrypoint` to the file that starts the program
  2. re-run the same `xelian add` command
```

That check runs *before* dependency installation, so an un-inferable repo fails
in seconds rather than after a multi-minute install. Languages without a v1
runtime (Go, Rust) say so by name.

Verified against real repositories, including the whole
`modelcontextprotocol/servers` family, `mcp-atlassian`, `playwright-mcp`,
`firecrawl-mcp-server`, `exa-mcp-server` and `chroma-mcp` — each imported from
a bare URL and confirmed to complete an MCP `initialize` + `tools/list`
handshake.

## One endpoint for all your MCP servers

Wiring ten MCP servers into an IDE means ten config entries that all break
differently. The Xelian gateway gives your IDE or agent framework exactly one
connection:

```bash
xelian gateway add alice/github-mcp
xelian gateway add bob/postgres-mcp
xelian gateway serve          # single MCP endpoint: http://127.0.0.1:11432/mcp
```

Point Cursor, Claude Code, CrewAI, or LangChain at that one URL. The gateway
namespaces tools as `<package>__<tool>`, routes each call to the right
server, restarts servers that crash, and gives you one place to look instead
of five terminal tabs:

```bash
xelian gateway status         # every backend: up/down, restarts, log path
xelian gateway logs           # unified stderr logs from all backends
```

## Python SDK

Install the SDK from a source checkout (not yet on PyPI). It shells out to the
`xelian` CLI, so make sure that's installed too (or point `XELIAN_BIN` at a
built binary):

```bash
pip install ./sdk        # from the repo root
```

```python
import xelian

agent = xelian.run("xelian/calc")
print(agent.chat("2*(3+4)**2"))

server = xelian.mcp("xelian/time-mcp")
server.expose()   # local MCP transport for any MCP client
```

## The package format

A Xelian package is a `.xelian` archive (tar.gz) containing your project plus
two files:

- **`xelian.toml`** — the manifest: name, version, `package-type`
  (`agent` | `mcp`), language, runtime constraint, entrypoint,
  declared `permissions` and `features`, author, and the native dependency
  manifest to install from (e.g. `pyproject.toml`, `package.json`).
- **`xelian.lock`** — the lockfile: a deterministic content checksum over
  every file in the archive, verified by the registry on publish and by the
  CLI on every download.

The runtime abstracts the implementation language away — a Python agent, a
Node MCP server, and anything else with a manifest all expose the same
interface. The full specification lives in [SPEC.md](SPEC.md).

## Environment variables

| Variable | Used by | What it does |
|----------|---------|--------------|
| `XELIAN_REGISTRY_URL` | CLI | Registry base URL for this shell. Overrides the built-in default (`http://localhost:8000` unless a release build baked one in). |
| `XELIAN_DEFAULT_REGISTRY_URL` | CLI (build time) | Bakes the production registry URL into a release binary at compile time. |
| `XELIAN_TOKEN` / `XELIAN_USERNAME` | CLI | Non-interactive auth (CI): use these instead of `xelian login`. |
| `XELIAN_BIN` | Python SDK | Absolute path to the `xelian` binary, if it isn't on `PATH`. |
| `DATABASE_URL` | Registry | Postgres connection string (required; no SQLite fallback). |
| `XELIAN_R2_BUCKET` / `XELIAN_R2_ENDPOINT` / `XELIAN_R2_ACCESS_KEY_ID` / `XELIAN_R2_SECRET_ACCESS_KEY` | Registry | Cloudflare R2 archive storage. Required in production (free-tier disks are ephemeral); falls back to local disk if unset. |
| `XELIAN_REGISTRY_ROOT` | Registry | Local-disk archive directory when R2 is not configured (default `~/.xelian-registry`). |
| `PORT` | Registry | Port to bind (Render/Fly/Cloud Run inject it; default 8000). |
| `XELIAN_MAX_ARCHIVE_MB` / `XELIAN_MAX_UNCOMPRESSED_MB` / `XELIAN_RATE_LIMIT_AUTH` / `XELIAN_RATE_LIMIT_PUBLISH` | Registry | Upload-size caps and per-IP rate-limit tuning. |

## Repository layout

| Path | What |
|------|------|
| `crates/xelian-cli` | the `xelian` binary (Rust) |
| `crates/xelian-core` | runtime, cache, package pipeline (Rust) |
| `registry/` | registry API (Python, FastAPI) |
| `sdk/` | Python SDK wrapping the CLI |
| `website/` | registry website (Next.js) |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build and test instructions, and
[SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## Credits

Xelian runs open-source agents and MCP servers built by other people — it
imports and launches their work unmodified, under each project's own license,
and never relicenses it. See [ATTRIBUTIONS.md](ATTRIBUTIONS.md) for the
projects Xelian works with and the license each one grants.

## License

[MIT](LICENSE)
