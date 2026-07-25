# Smart Run, Provider Setup & Local UI — Implementation Plan

> **For agentic workers:** implement task-by-task, TDD where a unit boundary
> exists, commit after each task. Spec:
> `docs/superpowers/specs/2026-07-25-smart-run-and-local-ui-design.md`.

**Goal:** Make agents "just work" — infer what a package needs and how it runs
(README + code), give a smart provider/key menu with a free/local path, drop
CLI agents into a warm REPL, add a local management UI, and fix website README
rendering.

**Tech Stack:** Rust (`xelian-core`, `xelian-cli`), `ureq`, `tiny_http`,
Next.js + Tailwind v4 (website).

## Global Constraints

- No architecture change: run pipeline stays prepare-then-launch; we add an
  inference step and a warm loop, never replace.
- Offline / deterministic: v1 inference is heuristic, no LLM, no network.
- Single static binary: the UI is served embedded via `tiny_http`, not Next.js.
- Never write Claude co-author attribution in commits.
- State lives under `~/.xelian/`, never inside the checksum-verified package cache.

---

### Task 1: README rendering fix (website)

**Files:** `website/package.json`, `website/app/globals.css`,
`website/app/c/[owner]/[name]/page.tsx`.

- Add `@tailwindcss/typography` dependency; `@plugin "@tailwindcss/typography";`
  in `globals.css`; ensure `prose` code-block styling reads well.
- **Verify:** build the site; open a code-heavy `/c/<owner>/<name>` page; headings,
  lists, tables, fenced code render styled.

### Task 2: Package Intelligence layer

**Files:** create `crates/xelian-core/src/run/inspect.rs`; register in
`crates/xelian-core/src/run/mod.rs`.

**Produces:**
```rust
pub struct PackageInsights {
    pub providers: Vec<Provider>,      // reuse provider::Provider (Task 3) — or plain enum if built first
    pub required_env: Vec<String>,
    pub runs_free_local: bool,
    pub run_style: RunStyle,           // Repl | OneShot | Server
    pub subcommands: Vec<String>,
    pub usage: Option<String>,
}
pub fn inspect_package(dir: &Path) -> PackageInsights;
```
- Heuristics: code scan (env reads, provider imports, argparse/click/commander),
  dependency files, README usage/free-tier/ollama mentions.
- Unit tests over fixture dirs (Python getenv+openai; Node process.env; argparse subcommands).
- **Verify:** `cargo test -p xelian-core inspect`.

### Task 3: Provider/key setup + `xelian key`

**Files:** create `crates/xelian-core/src/run/provider.rs`; extend
`crates/xelian-core/src/run/model.rs` (running-Ollama check); extend
`crates/xelian-core/src/secrets.rs` (list/remove + accessor); extend
`crates/xelian-cli/src/main.rs` (`Command::Key`, handler, run-time menu wiring).

**Produces:** `Provider` enum (Ollama/OpenAI/Anthropic) with key-var + base-url-var
mapping + `is_free`; `ollama_running() -> bool` (GET localhost:11434/api/tags).
- Run-time menu when insights need a model and nothing configured; free/local = no input.
- `xelian key set|list|rm|use`.
- **Verify:** unit tests for provider mapping + secret list/remove; manual key round-trip.

### Task 4: Warm REPL

**Files:** `crates/xelian-core/src/run/launch.rs` (warm loop helper reusing
`build_launch_command`); `crates/xelian-cli/src/main.rs`
(`prepare_env_and_launch_inner` gates loop vs single launch).
- No args + run_style Repl/OneShot ⇒ warm loop; `-- args` and MCP Server unchanged.
- Banner/`:help` from insights; `:exit` quits.
- **Verify:** run a CLI-style agent, issue two commands without re-prep; one-shot still exits.

### Task 5: Local UI (`xelian ui`, port 2106)

**Files:** create `crates/xelian-cli/src/ui.rs` (tiny_http server + JSON API +
embedded HTML/JS); wire `Command::Ui { port }` in `main.rs`; enumerate via
`cache::CachedPackage`.
- Endpoints: list / run / status / keys. Localhost only. Default port 2106, `--port`.
- **Verify:** `xelian ui`; load localhost:2106; list shows cached packages; run streams; key set persists.
