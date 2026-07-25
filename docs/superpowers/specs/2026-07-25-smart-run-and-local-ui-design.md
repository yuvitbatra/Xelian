# Smart Run, Provider Setup & Local UI — Design

**Date:** 2026-07-25
**Status:** Approved for planning

## Context

Xelian runs agents and MCP servers locally, but four rough edges hurt the "it
just works" experience the product promises:

1. **API keys / providers are clumsy.** Every credential is an opaque named env
   var in `~/.xelian/secrets.toml`. There is no provider concept, no notion of a
   free tier, and no way to switch a stored key short of hand-editing TOML. A
   user who wants to run against a free local model, or pick between OpenAI /
   Anthropic / Ollama, gets no help.
2. **CLI-style agents are tedious.** Xelian spawns the child once with inherited
   stdio; there is no real REPL. A package that behaves like a command-line tool
   forces the user to re-run the entire `xelian run pkg -- <cmd>` pipeline for
   every single command.
3. **No local management UI.** There is no quick way to see what's cached, run
   it, or manage keys outside the CLI.
4. **README renders poorly** on agent pages of the website.

The through-line for #1 and #2 is the same: **Xelian should read the package
(README + actual code) and infer how it is meant to run and what it needs**, so
setup approaches zero. That inference layer is the keystone; the provider menu
and the warm REPL are UX built on top of it.

Design principles honored: local-first (inference is offline + deterministic),
convention over configuration, simplicity, single static binary (the UI ships
embedded, not as the Next.js site). No architecture change: the run pipeline
stays an environment-manager that prepares once and launches; we add a warm loop
and an inference step, we do not replace anything.

## Sequencing

Each feature ships independently. Order is dictated by dependencies:

1. **README rendering fix** (website only, zero coupling) — ship first as a warm-up.
2. **Package Intelligence layer** (foundation for 3 & 4).
3. **Smart provider / key setup** (consumes insights).
4. **Warm REPL** (consumes insights).
5. **Local UI** (consumes cache + insights + provider layer).

---

## Component 0 — Package Intelligence layer (foundation)

**Goal:** given an extracted package directory, produce structured, cached
insights about what the package needs and how it runs. Works identically for
registry packages *and* `xelian add <github>` repos (which have no Xelian
manifest), so it operates on the extracted files, not the manifest.

**New module:** `crates/xelian-core/src/run/inspect.rs` (sibling of
`model.rs`, `env_vars.rs`, `launch.rs`).

**Output struct (sketch):**

```rust
pub struct PackageInsights {
    pub providers: Vec<DetectedProvider>, // e.g. OpenAI, Anthropic, Ollama
    pub required_env: Vec<String>,        // env vars the code actually reads
    pub runs_free_local: bool,            // can run against local Ollama / no key
    pub run_style: RunStyle,              // Repl | OneShot | Server
    pub subcommands: Vec<String>,         // inferred CLI subcommands, for :help
    pub usage: Option<String>,            // README quickstart/usage excerpt
}
```

**Inference sources (deterministic heuristics, offline — no LLM in v1):**

- **Code scan** of the extracted dir: env-var reads (`os.getenv("X")`,
  `os.environ["X"]`, `process.env.X`), provider SDK imports (`anthropic`,
  `openai`, `ollama`, `google.generativeai`), and argparse / click / commander
  command definitions → `subcommands`.
- **Dependency files:** `requirements.txt`, `pyproject.toml`, `package.json` →
  which provider SDKs are present.
- **README scan:** quickstart / usage section (for `usage`), and mentions of
  "free tier" / "Ollama" / key names.

**Caching:** insights are computed once per package version and cached next to
the extracted package under `~/.xelian/` (mirroring how grant/env state lives
under the home root, never inside the checksum-verified package cache — see
`cache.rs` grants-path pattern). Reuse `XelianHome` for path resolution.

**Explicitly out of scope for v1:** LLM-assisted parsing. Heuristics only.
Recorded as a future enhancement. Inference is *assistive and transparent* — it
pre-fills and suggests; it never silently makes a wrong choice unattended.

**Design invariant:** if inference finds nothing, behavior is exactly today's
(prompt for declared env vars, launch once). Inference only *adds* affordances.

---

## Component 1 — README rendering fix (website)

**Problem (diagnosed):** the agent page
`website/app/c/[owner]/[name]/page.tsx` (README block ~line 119) uses Tailwind
`prose` classes, but `@tailwindcss/typography` is **not installed** and not
registered, so under Tailwind v4's reset the markdown collapses to an
unstyled wall of text. (The registry page
`website/app/packages/[owner]/[name]/page.tsx` looks fine because it uses a
hand-written `.readme` class in `globals.css`.)

**Fix:**
1. Add `@tailwindcss/typography` to `website/package.json`.
2. Add `@plugin "@tailwindcss/typography";` to `website/app/globals.css` (Tailwind
   v4 plugin registration).
3. Keep the existing `prose prose-sm …` wrapper; add fenced-code-block styling so
   code-heavy READMEs read cleanly.

Both detail pages already use `react-markdown` + `remark-gfm` +
`rehype-sanitize`, so markdown parsing itself is fine — this is purely styling.

**Verification:** run the website locally, open a `/c/<owner>/<name>` agent page
with a code-heavy README, confirm headings/lists/tables/code render styled.

---

## Component 2 — Smart provider / key setup

**Goal:** "option 2, kept extremely simple" — a real provider concept, but the
user types nothing to use a free/local model, and can switch anytime.

**Provider model (new, small):** a `Provider` enum in a new
`crates/xelian-core/src/run/provider.rs`, each variant mapping to the env vars it
needs (API-key var + base-URL var) and whether it is free/local:

```rust
pub enum Provider { Ollama, OpenAI, Anthropic /* extensible */ }
```

**Ollama running-detection:** extend `run/model.rs` (which today only finds the
*binary* via `find_ollama`) with a health check against
`http://localhost:11434/api/tags` to detect a *running* daemon. `ureq` is
already a dependency.

**Run-time flow:** when insights say the package needs a model and nothing is
configured, present one menu (to stderr, matching `env_vars.rs` prompt
convention):

```
This agent needs a model. How do you want to run it?
❯ Ollama — local, free            (running ✓)
  OpenAI — paste key
  Anthropic — paste key
```

- Choosing Ollama/free requires **no input**: Xelian sets the agent's base-URL
  env var to point at local Ollama and ensures the model via existing
  `model::ensure_model`.
- Choosing a paid provider prompts for the key once and stores it via the
  existing `SecretStore` (`secrets.rs`).
- The resolved env pairs flow through the **unchanged** injection path
  (`env_vars` → `launch.rs` `env_pairs`). No change to the injection layer.

**New CLI command:** `xelian key` with `set` / `list` / `rm` / `use`
subcommands (mirror the existing `GatewayAction` sub-enum pattern in
`crates/xelian-cli/src/main.rs`). `use` switches the active provider/key without
hand-editing TOML. Requires exposing a `list`/`remove` accessor on `SecretStore`
(the inner map is currently private).

**Precedence note:** `env_vars.rs::resolve_with_sources` currently puts a stored
secret ahead of any prompt, so a stored key is never re-asked. The provider menu
runs *before* that resolution when insights indicate a model is needed; `xelian
key use` provides the explicit override path.

---

## Component 3 — Warm REPL (smooth CLI agents)

**Goal:** after `xelian run pkg` with no trailing args, drop into a persistent
prompt so the user stops re-typing `xelian run pkg -- …` and stops re-paying the
prepare pipeline per command.

**Behavior:**
- `prepare_env_and_launch_inner` (`main.rs`) prepares the environment **once**
  (download/extract/permissions/model/keys — all as today), then, instead of a
  single `launch::launch` call, enters a **warm read-eval loop** when insights
  report `run_style = Repl`/`OneShot` and no `-- args` were given.
- Each line the user types is spliced onto the entrypoint argv (reusing
  `launch.rs::build_launch_command`) and run against the already-prepared
  environment; output streams through; loop continues. Prep is not repeated.
- The banner + `:help` are populated from `PackageInsights.usage` and
  `.subcommands` — "use the README heavily" so the user knows what to type.
- `:help` / `:exit` are Xelian meta-commands; everything else goes to the tool.
- **Unchanged:** `xelian run pkg -- <cmd>` one-shot mode (args present ⇒ single
  launch, exactly as today). MCP `Server` run style is unchanged (stdio
  transport). `Invocation::user_driven()` already distinguishes args-present.

**Touch points:** `crates/xelian-core/src/run/launch.rs` (warm loop helper,
reusing `build_launch_command`), `crates/xelian-cli/src/main.rs`
(`prepare_env_and_launch_inner` gates loop vs single launch).

**Explicitly NOT doing in v1:** keeping one long-lived child process for
stateful memory between turns (would require the packaged agent to cooperate by
looping on stdin). The warm loop re-runs the entrypoint per line against a warm
env — covers the retyping friction without requiring package changes. Stateful
single-process sessions are a future enhancement.

---

## Component 4 — Local UI (manage + run, port 2106)

**Goal:** a small local control panel to see cached packages, run them, and
manage keys/providers from the browser.

**Architecture:** a new `xelian ui` command that serves a **single-page control
panel embedded in the Rust binary** via the existing `tiny_http` dependency —
*not* the Next.js website — to preserve offline + single-static-binary
principles. Default port **2106** (configurable via `--port`).

**Surfaces:**
- **List** cached packages by enumerating `~/.xelian/packages/` — reuse the
  existing `CachedPackage` / `PackageSource` types in `cache.rs`.
- **Run** a package with one click (streams output; a warm session where
  applicable).
- **Status** of what's currently running.
- **Keys/providers:** view, set, switch — backed by the same `SecretStore` +
  `Provider` layer from Component 2 (no second source of truth).

**Local JSON API:** the embedded page talks to a small local HTTP+JSON API
served by the same `xelian ui` process (endpoints for list / run / status /
keys). All localhost-only.

**Scope discipline:** read + run + key management only. No auth, no remote
access, no multi-user — it's a personal localhost panel.

---

## Testing / Verification (per component)

- **Intelligence:** unit tests over fixture package dirs (Python with
  `os.getenv` + `openai` import; Node with `process.env`; a repo with argparse
  subcommands) asserting the resulting `PackageInsights`.
- **README:** run the site, open a code-heavy `/c/...` agent page, eyeball
  headings/lists/tables/code.
- **Provider/keys:** run a package needing a model with (a) Ollama running →
  no-prompt free path, (b) no Ollama → paid menu + stored key; `xelian key
  list/use/rm` round-trip.
- **Warm REPL:** `xelian run <cli-agent>` → type two commands without re-running
  the pipeline; confirm `-- args` one-shot still exits after one command.
- **UI:** `xelian ui`, load `localhost:2106`, list shows cached packages, run
  streams output, key set/switch persists to `secrets.toml`.

## Open questions / future enhancements (not v1)

- LLM-assisted README/code inference (v1 is heuristic-only).
- Stateful single-process REPL sessions (v1 re-runs entrypoint per line).
- Manifest-baked insights at push time (v1 infers at run time and caches).
