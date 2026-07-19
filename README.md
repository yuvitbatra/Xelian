# Xelian

**Run AI agents like you run models.** Xelian is a local-first registry and
runtime for AI agents and MCP servers — package once, publish with one
command, and anyone can run it locally with zero setup.

```bash
xelian run demo/echo-agent
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

## Quickstart

Until binary releases ship, build from source (Rust toolchain required):

```bash
cargo install --path crates/xelian-cli   # 1. install the CLI
xelian run demo/echo-agent               # 2. download + run an agent
# 3. chat — agents open a REPL; MCP servers start and expose locally
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

Already have an agent on GitHub? Import and run it directly:

```bash
xelian add https://github.com/user/repo
```

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

```python
import xelian

agent = xelian.run("demo/echo-agent")
print(agent.chat("hello"))

server = xelian.mcp("someone/mcp-server")
server.expose()   # local MCP transport for any MCP client
```

## The package format

A Xelian package is a `.xelian` archive (tar.gz) containing your project plus
two files:

- **`xelian.toml`** — the manifest: name, version, `package-type`
  (`agent` | `mcp-server`), language, runtime constraint, entrypoint,
  declared `permissions` and `features`, author, and the native dependency
  manifest to install from (e.g. `pyproject.toml`, `package.json`).
- **`xelian.lock`** — the lockfile: a deterministic content checksum over
  every file in the archive, verified by the registry on publish and by the
  CLI on every download.

The runtime abstracts the implementation language away — a Python agent, a
Node MCP server, and anything else with a manifest all expose the same
interface. The full specification lives in [SPEC.md](SPEC.md).

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

## License

[MIT](LICENSE)
