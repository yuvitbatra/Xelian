# Design: Make `xelian add <github-url>` work for real-world repos

Date: 2026-07-20
Status: Approved

## Problem

`xelian add` succeeds on ~1 of 25 real GitHub repos (measured, see below).
The design of SPEC §12 is sound; its inference *tables* are too narrow, and
there are two ordering/caching bugs.

Measured failures (25 URLs in `test_add.txt`, run against the release binary):

| Cause | Repos affected |
|---|---|
| Entrypoint inference too narrow (4 hardcoded filenames) | ~14 |
| Language markers missing (`setup.py`, `requirements.txt`) | 3 |
| Language unsupported (`go.mod`) but error says "could not detect" | 1 |
| Subdirectory URLs (`/tree/main/src/x`) rejected outright | 5 |
| TS entrypoint (`dist/index.js`) only exists after a build | 5 |
| `package-type` hardcoded to `agent` | all MCP servers |

## Non-goals

- Universality. Libraries with no runnable entrypoint (Voyager, swarm) and
  repos with no manifest at all (JARVIS) cannot be run by any inference.
- New language runtimes (Go, Rust). Out of V1 per CLAUDE.md.
- A resolver plugin system. That is the architecture rewrite CLAUDE.md forbids.

## Approach

Keep SPEC §12's marker-table + inference + placeholder architecture. Extend the
tables; fix the bugs. SPEC §12.2 explicitly sanctions this: detection is
"extensible ... by appending additional manifest-file checks, not by
redesigning the precedence mechanism."

## Changes

1. **URL parsing** — accept `/tree/<ref>/<subdir>`. Subdir becomes the package
   root and joins the cache key. `<ref>` resolves to a SHA via `git ls-remote`.
2. **Language detection** — append `setup.py`, `requirements.txt` → Python;
   `go.mod` → clear unsupported-language error. Tiebreak: when `package.json`
   coexists with a Python marker, prefer Python unless package.json has
   `main`/`bin`/`scripts.build`.
3. **Entrypoint inference** — ordered strategies per language.
   - Python: `[project.scripts]` console script → module file; `<pkg>/__main__.py`;
     then the existing filename list.
   - Node: `main`; `bin`; `index.js`. Accepted even if absent when a `build`
     script exists.
4. **Build step** — if the inferred entrypoint is missing and `package.json`
   has a `build` script, run it after install, then re-check.
5. **Ordering** — verify entrypoint is inferable/buildable *before* dependency
   install. Un-inferable repos fail in seconds, not minutes.
6. **`package-type`** — detect `mcp` from an MCP SDK dependency
   (`@modelcontextprotocol/sdk`, `mcp`) or an `mcp-`/`-mcp` name.
7. **Cache-poisoning fix** — a failed import currently leaves the checkout
   cached; the retry then hits `from_cache: true`, skips inference, and fails
   worse than the first attempt. Gate the cache-hit path on `xelian.toml` +
   `xelian.lock` presence, not directory non-emptiness.
8. **Push** — `PLEASE_EDIT` placeholders must block publish (today only
   `TODO`/`you@example.com` are checked, so imports publish placeholder
   license/author silently).
9. **Launch** — `python -m <pkg>` when the entrypoint is a `__main__.py`
   (direct-path execution breaks relative imports); keeps the manifest's
   entrypoint-as-path contract. Emit a clear readiness banner before handing
   over to an agent REPL or MCP server.

## File organization

`github.rs` (1256 lines) splits into `github/{url,detect,entrypoint,build,mod}.rs`.
Mechanical; no behavior change beyond the above.

## Trust boundary

Running build scripts executes repo-authored code. `npm install` already runs
arbitrary postinstall hooks, so this widens rather than creates the boundary.
Accepted deliberately.

## Expected outcome

~15-18 of 25 URLs running (from 1). The remainder fail in seconds with the
cached path and the single field to edit.
