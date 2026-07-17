# Harbor Backlog

> A flat, strictly ordered task list derived from `IMPLEMENTATION.md` and
> `SPEC.md`. Work top to bottom: pick the next unchecked task whose dependencies
> are all checked, complete it, check it off, repeat — you should always make
> forward progress.
>
> - **ID:** stable (`H-NNN`); never renumber.
> - **Difficulty:** S / M / L.
> - **Duration:** target 2–8 hours; larger work is split.
> - **Deps:** task IDs that must be done first.
> - **Acceptance:** how you know it's done.
>
> Section references (`§9.4`, …) point at `SPEC.md`.

---

## Phase 0 — Workspace & CLI skeleton

- [x] **H-001 — Initialize Cargo workspace**
  - Difficulty: S · Duration: 2h · Deps: none
  - Acceptance: `harbor-cli` (bin) + `harbor-core` (lib) build under one
    workspace; `cargo build` succeeds; binary is named `harbor`.

- [x] **H-002 — Wire clap command surface (all 9 commands + flags)**
  - Difficulty: M · Duration: 4h · Deps: H-001
  - Acceptance: `harbor --help` lists `init, push, run, add, list, rm, login,
    logout, yank` with correct flags — `rm` has `--env`/`--all` (§13.6), `yank`
    has `--version`/`--undo` (§13.9); each stub exits non-zero with "not
    implemented"; `-V` prints the binary version.

- [x] **H-003 — Cache layout module for `~/.harbor/`**
  - Difficulty: S · Duration: 3h · Deps: H-001
  - Acceptance: helper resolves and lazily creates `packages/`, `runtimes/`,
    `envs/`, `models/`, `logs/`, `tmp/` under a resolved home dir (§11.1);
    `credentials.toml` is NOT created here (§11.3); unit tests pass under a temp
    `$HOME`.

---

## Phase 1 — Manifest parse & validate

- [x] **H-010 — Manifest structs (serde/toml)**
  - Difficulty: M · Duration: 4h · Deps: H-001
  - Acceptance: typed structs cover all §6.1/§6.2 fields plus `[author]`,
    `[dependencies]`, `[environment]`, `[commands]`, opaque `[config]`; the §6.4
    example deserializes; `[config]` is captured but never interpreted (§6.3).

- [x] **H-011 — Required-field + spec-version validation**
  - Difficulty: M · Duration: 3h · Deps: H-010
  - Acceptance: missing required field yields a distinct per-field error;
    unsupported `spec-version` is rejected (§6.1, §8.1 step 1).

- [x] **H-012 — Closed-enum validation: permissions, features, os**
  - Difficulty: M · Duration: 3h · Deps: H-010
  - Acceptance: `permissions` outside §16.1 fails; `features` outside §17 warns
    (not fail, §17); unrecognized `os` values rejected (§8.1 step 1).

- [x] **H-013 — Naming + environment-conflict rules**
  - Difficulty: S · Duration: 3h · Deps: H-010
  - Acceptance: names enforced to lowercase ASCII/digits/`_`/`-`, length 3–64
    (§19.3); a variable declaring both `required = true` and `default` is
    rejected (§6.2.1); `runtime` string is captured but NOT parsed (§6.1).

- [x] **H-014 — Single `validate_manifest()` entry point + tests**
  - Difficulty: S · Duration: 3h · Deps: H-011, H-012, H-013
  - Acceptance: one reusable entry point (used later by push §8.1 and run §9.6);
    table-driven tests cover one valid + one invalid case per rule.

---

## Phase 2 — harbor init

- [x] **H-020 — Generate `harbor.toml` skeleton**
  - Difficulty: M · Duration: 4h · Deps: H-014
  - Acceptance: `harbor init` writes a `harbor.toml` with valid defaults +
    clearly marked placeholders (name from dir if §19.3-valid, else placeholder);
    output parses under H-014 (modulo intentional placeholders); no network
    (§13.1).

- [x] **H-021 — Generate `harbor.lock` skeleton + clobber guard**
  - Difficulty: S · Duration: 2h · Deps: H-020
  - Acceptance: a valid `harbor.lock` shell is written; existing `harbor.toml`
    is not silently overwritten (documented flag/prompt behavior); no network.

---

## Phase 3 — harbor.lock & checksums

- [x] **H-030 — SHA-256 helper + native-lock-checksum**
  - Difficulty: S · Duration: 3h · Deps: H-001
  - Acceptance: SHA-256 of a native lockfile matches independent `sha256sum`;
    computed only when a native lockfile is declared (§7.2, §7.3).

- [x] **H-031 — `package-checksum` excluding `harbor.lock`**
  - Difficulty: M · Duration: 4h · Deps: H-030
  - Acceptance: checksum is deterministic for identical inputs; mutating
    `harbor.lock` does NOT change `package-checksum` (§7.3); `harbor.lock` is
    never itself hashed.

- [x] **H-032 — Populate all `harbor.lock` keys**
  - Difficulty: M · Duration: 3h · Deps: H-031, H-010
  - Acceptance: every §7.2 key is present with correct values
    (`spec-version`, `harbor-version`, `package-version` copied from manifest,
    `generated-at` ISO 8601 UTC, native paths, both checksums).

---

## Phase 4 — Packaging / archive build

- [x] **H-040 — tar.gz archive builder**
  - Difficulty: M · Duration: 4h · Deps: H-032
  - Acceptance: builds a `.harbor` inspectable via `tar -tzf` with the §5.2/§5.3
    layout; deterministic enough to keep H-031 checksums stable.

- [x] **H-041 — `.gitignore` exclusion (use `ignore` crate)**
  - Difficulty: L · Duration: 6h · Deps: H-040
  - Acceptance: files matched by `.gitignore` never appear in the archive
    (§5.4), including nested cases; `.git/` and build scratch always excluded
    (§5.4); no force-include path exists.

- [x] **H-042 — §8.1 validation pipeline (ordered, fail-fast)**
  - Difficulty: L · Duration: 6h · Deps: H-041, H-014
  - Acceptance: steps 1–8 run in order (§8.1); required files (§5.3) and
    entrypoint existence/non-exclusion (§5.4/step 4) enforced; `[commands]`
    values checked as non-empty strings without execution (step 5, §8.4);
    entrypoint excluded by `.gitignore` fails; first failure stops with non-zero
    exit and no partial archive (§8.2). No network.

---

## Phase 5 — Local run: extract / re-validate / OS check

- [x] **H-050 — Local-archive run entry + checksum verify**
  - Difficulty: M · Duration: 4h · Deps: H-042
  - Acceptance: `harbor run ./x.harbor` recomputes SHA-256 and aborts before
    extraction on mismatch (§9.4); a tampered archive never extracts.

- [x] **H-051 — Safe extraction into version-scoped cache**
  - Difficulty: M · Duration: 4h · Deps: H-050, H-003
  - Acceptance: extracts into the source-based cache (decision 2026-07-16):
    `~/.harbor/packages/local/<name>/<version>/` for local archives (registry
    and GitHub sources get their own namespaces later); rejects `..`/absolute
    tar paths; skips re-extraction of an already-present version (§9.5).

- [x] **H-052 — Manifest re-validation + OS check**
  - Difficulty: S · Duration: 3h · Deps: H-051, H-014
  - Acceptance: re-parses/re-validates `harbor.toml` (§9.6); if `os` is declared
    and current OS not listed, fails immediately with a clear message and goes no
    further (§9.6.1).

---

## Phase 6 — Runtime management (uv/npm)

- [x] **H-060 — Extensible runtime-manager dispatch**
  - Difficulty: M · Duration: 4h · Deps: H-052
  - Acceptance: dispatch keyed on `language` (§10.4); adding a third language is
    a new case, not a rewrite; compiles with a placeholder third arm.

- [x] **H-061 — Python path: ensure `uv` + CPython**
  - Difficulty: L · Duration: 6h · Deps: H-060
  - Acceptance: auto-installs `uv` if absent; provisions a CPython satisfying the
    `runtime` PEP 440 constraint via `uv` (§9.7, §10.1); constraint matching is
    delegated to `uv`, not parsed in Harbor (§6.1).

- [x] **H-062 — Node path: ensure Node + `npm`**
  - Difficulty: M · Duration: 5h · Deps: H-060
  - Acceptance: auto-installs Node if absent; selects a runtime satisfying the
    SemVer range via `npm` (§9.7, §10.2).

---

## Phase 7 — Environment + dependency install

- [x] **H-070 — Environment cache keyed on (name, version)**
  - Difficulty: M · Duration: 4h · Deps: H-061
  - Acceptance: exactly one env per `(name, version)` under `~/.harbor/envs/`,
    mirroring the source-based cache layout (decision 2026-07-16); key is
    `(name, version)` only, no dependency hash (§9.8); reused on subsequent runs.

- [x] **H-071 — Delegate dependency install to native manager**
  - Difficulty: M · Duration: 5h · Deps: H-070
  - Acceptance: deps installed via `uv`/`npm` against the native manifest+lockfile
    (§6.1.2, §9.8); Harbor never re-declares/re-resolves deps; interrupted
    installs do not leave a corrupt env used as valid (stage in `tmp/`, §11.1).

---

## Phase 8 — Launch (env vars, agent REPL, MCP stdio + port)

- [x] **H-080 — Required/default env-var resolution before launch**
  - Difficulty: S · Duration: 3h · Deps: H-071
  - Acceptance: missing `required = true` var aborts before launch with a clear
    message; `default` applied to unset non-required vars (§6.2.1, §9.10).

- [x] **H-081 — Agent launch: attach REPL to terminal**
  - Difficulty: M · Duration: 5h · Deps: H-080
  - Acceptance: `agent` entrypoint runs with stdin/stdout/stderr attached to the
    terminal as an interactive REPL; `harbor run` blocks for the session
    (§9.10.1).

- [x] **H-082 — MCP launch: stdio server + port fallback**
  - Difficulty: L · Duration: 6h · Deps: H-080
  - Acceptance: `mcp` entrypoint launches over stdio (§9.10.2); `port` governs
    local HTTP exposure (decision 2026-07-16 — resolve the bridge-vs-passthrough
    sub-question in IMPLEMENTATION.md before starting); if the declared `port`
    is busy, a free port is auto-selected and reported to the user (§9.10.2).

---

## Phase 9 — Permissions first-run prompt

- [x] **H-090 — First-run permission prompt (disclosure-only)**
  - Difficulty: S · Duration: 3h · Deps: H-081
  - Acceptance: first run of a `(name, version)` prompts grant/deny per declared
    permission (§16.1/§16.2); decision persisted; no re-prompt on later runs of
    that version; no technical enforcement (§16.2, §20.4). Resolve the
    deny-behavior open question (see IMPLEMENTATION.md) before starting.

---

## Phase 10 — Ollama model management

- [x] **H-100 — Auto-install Ollama when absent**
  - Difficulty: M · Duration: 4h · Deps: H-081
  - Acceptance: if Ollama binary/daemon is absent, it is installed automatically
    before any model download (§9.9, §18).

- [x] **H-101 — Download + cache `primary-model` before launch**
  - Difficulty: M · Duration: 4h · Deps: H-100
  - Acceptance: declared `primary-model` is downloaded if missing from
    `~/.harbor/models/`/Ollama store and reused on later runs (§9.9, §18);
    correctly sequenced as pipeline step 10 before launch (§9.1).

---

## Phase 11 — GitHub import (harbor add)

- [x] **H-110 — Resolve default branch to SHA + download at SHA**
  - Difficulty: M · Duration: 5h · Deps: H-101
  - Acceptance: default branch resolved to a commit SHA; repo downloaded at that
    SHA; cached at `packages/github/<owner>/<repo>/<sha>/` — by SHA, not branch
    (§12.2 step 1; source-based cache decision 2026-07-16).

- [x] **H-111 — Language detection by precedence**
  - Difficulty: S · Duration: 3h · Deps: H-110
  - Acceptance: `pyproject.toml`→python, `package.json`→node, `Cargo.toml`→clear
    "unsupported language" error (§12.2 step 2); detection list is extensible.

- [x] **H-112 — Infer `harbor.toml` with placeholders**
  - Difficulty: L · Duration: 6h · Deps: H-111
  - Acceptance: infers `language`, `runtime`, `entrypoint`, `dependencies`;
    non-inferable fields get placeholders; import does not fail on placeholders
    (§12.2 step 3); nothing is published (§12.3).

- [ ] **H-113 — Build package + run from step 6 onward**
  - Difficulty: M · Duration: 4h · Deps: H-112, H-042
  - Acceptance: generates `harbor.lock` + `.harbor` (§12.2 steps 4–5), caches by
    SHA (step 6), and runs via the existing pipeline from manifest validation
    onward (step 7, §9.6+).

---

## Phase 12 — harbor list & harbor rm

- [ ] **H-120 — `harbor list` (local cache only)**
  - Difficulty: S · Duration: 3h · Deps: H-113
  - Acceptance: lists locally cached packages only, no registry search (§13.5,
    §22).

- [ ] **H-121 — `harbor rm` variants + credential isolation**
  - Difficulty: M · Duration: 4h · Deps: H-120
  - Acceptance: `rm owner/package` removes cached versions but keeps envs;
    `--env` also removes the env; `--all` clears `packages/`/`envs/`/`runtimes/`/
    `models/` but never `credentials.toml` (§13.6, §11.3); deletes are guarded to
    stay within `~/.harbor/`; never touches the registry.

---

## Phase 13 — Registry backend (FastAPI)

- [ ] **H-130 — FastAPI project + data model**
  - Difficulty: M · Duration: 4h · Deps: none (Python; can start after H-042 exists to produce test archives)
  - Acceptance: `Account`/`Package`/`Versions[]` modeled per §14.2 with storage
    for archive, checksum, `harbor.lock`, README, metadata, `published_at`,
    `yanked`.

- [ ] **H-131 — `POST /packages` with publish-time checks**
  - Difficulty: L · Duration: 6h · Deps: H-130
  - Acceptance: accepts an upload; verifies archive SHA-256 matches
    `package-checksum` in the accompanying `harbor.lock` (§14.5); rejects an
    already-published `(name, version)` (§14.5, §19.2); never executes the
    package (§14.1). Request/response schema documented as the client contract
    (§14.8).

- [ ] **H-132 — `GET /packages/{owner}/{package}` + resolution**
  - Difficulty: M · Duration: 4h · Deps: H-131
  - Acceptance: returns metadata for the resolved latest version = highest
    SemVer that is not yanked and not pre-release; clear error if none (§14.3,
    §19.1).

- [ ] **H-133 — `GET /download/{owner}/{package}/{version}`**
  - Difficulty: S · Duration: 3h · Deps: H-131
  - Acceptance: returns the exact version's archive bytes (§14.8); immutability
    holds (no in-place edit path, §14.6/§19.2).

---

## Phase 14 — login/logout + credentials

- [ ] **H-140 — Registry auth route(s) (OAuth token/callback)**
  - Difficulty: M · Duration: 5h · Deps: H-130
  - Acceptance: OAuth token/callback route pair exists (TODO-15 sketch,
    non-normative); issues a credential the CLI can store and later present
    (§14.4).

- [ ] **H-141 — `harbor login` browser flow + `credentials.toml` (0600)**
  - Difficulty: M · Duration: 5h · Deps: H-140, H-003
  - Acceptance: `harbor login` completes a browser OAuth flow (§13.7) and writes
    `~/.harbor/credentials.toml` at `0600`, top-level (§11.1, §14.4/§11.3).

- [ ] **H-142 — `harbor logout`**
  - Difficulty: S · Duration: 2h · Deps: H-141
  - Acceptance: removes the stored credential (§13.8); confirmed that
    `harbor rm --all` still leaves it intact (cross-check H-121).

---

## Phase 15 — harbor push end-to-end

- [ ] **H-150 — Registry HTTP client (authenticated)**
  - Difficulty: M · Duration: 4h · Deps: H-142
  - Acceptance: client can send authenticated requests using the stored
    credential (§14.4); handles archive upload.

- [ ] **H-151 — `harbor push`: validate then upload**
  - Difficulty: M · Duration: 5h · Deps: H-150, H-042, H-131
  - Acceptance: runs full §8.1 validation before any network call (§13.2, §8.2 —
    assert no socket opened on validation failure); uploads to `POST /packages`;
    republishing an existing `(name, version)` fails (§19.2); pushing to a
    non-owned namespace is rejected server-side (§14.4).

---

## Phase 16 — harbor run from registry

- [ ] **H-160 — Target-form discrimination**
  - Difficulty: S · Duration: 3h · Deps: H-052
  - Acceptance: registry-ref vs. GitHub-URL vs. local `.harbor` path (decision
    2026-07-16) distinguished syntactically before resolution; any other input
    fails clearly rather than guessing (§9.2); no `@version` pin syntax accepted
    (§9.2, §22).

- [ ] **H-161 — Registry resolution + cache-check + download**
  - Difficulty: M · Duration: 5h · Deps: H-160, H-150, H-132, H-133
  - Acceptance: `owner/package` resolves to latest stable non-yanked
    non-pre-release (§9.2, §14.3); cache is checked before any network request
    and download is skipped on hit (§9.3); missing archive downloaded via
    `GET /download/...` staged through `tmp/` (§9.3, §11.1).

- [ ] **H-162 — Wire download into local run pipeline**
  - Difficulty: M · Duration: 4h · Deps: H-161, H-101
  - Acceptance: downloaded archive is checksum-verified (§9.4) then flows through
    the existing extract→…→launch pipeline; end-to-end `harbor push` then, on a
    clean cache, `harbor run owner/package` launches (Appendix C.1–C.2); cache
    persists (§9.11).

---

## Phase 17 — harbor yank

- [ ] **H-170 — Registry yank/unyank route (owner-authorized)**
  - Difficulty: M · Duration: 4h · Deps: H-132, H-140
  - Acceptance: a route marks `yanked = true`/`false` for a version (TODO-15
    sketch: `PATCH /packages/{owner}/{package}/{version}`); authorized only for
    the owning account (§14.4); never deletes archive/checksum/metadata
    (§14.7.1).

- [ ] **H-171 — `harbor yank` CLI (+ `--undo`)**
  - Difficulty: S · Duration: 3h · Deps: H-170, H-150
  - Acceptance: `harbor yank owner/package --version <v>` yanks; `--undo`
    reverses (§13.9); after yanking the latest, `harbor run` resolves to the next
    non-yanked version or fails clearly (§14.3, §14.7.1); already-cached clients
    unaffected; no hard delete (§14.7.2, §22).

---

## Phase 18 — Python SDK

- [ ] **H-180 — SDK skeleton wrapping the CLI**
  - Difficulty: M · Duration: 4h · Deps: H-162
  - Acceptance: `sdk/` Python package shells out to the `harbor` binary; no
    reimplementation of resolution/validation/execution (§15.1).

- [ ] **H-181 — `install` / `run` / `agent` / `mcp` entry points**
  - Difficulty: M · Duration: 5h · Deps: H-180
  - Acceptance: `install` performs steps 1–9 without launching (§15.2); `run`
    performs the full pipeline and returns a type-appropriate handle; `agent`/
    `mcp` raise on package-type mismatch (§15.2).

- [ ] **H-182 — Handles: `.chat()` and `.expose()`**
  - Difficulty: L · Duration: 6h · Deps: H-181
  - Acceptance: agent handle `.chat()` returns a response; MCP handle `.expose()`
    makes the local server available to an MCP client (§15.2); surface beyond
    these two methods deferred (TODO-29).

---

## Phase 19 — Website (read-only)

- [ ] **H-190 — Next.js app + registry API client**
  - Difficulty: M · Duration: 5h · Deps: H-132, H-133
  - Acceptance: site reads the registry API read-only (§14.9); no write path
    beyond the public API (§14.9).

- [ ] **H-191 — Browse/search + package page**
  - Difficulty: L · Duration: 6h · Deps: H-190
  - Acceptance: lists/searches published packages; a package page renders README
    and metadata (§14.2), including declared permissions (§16.3) and features
    (§17).

---

## Notes on ordering and parallelism

- The strict critical path is
  **H-001 → H-010/011/012/013 → H-014 → H-030 → H-031 → H-032 → H-040 → H-041 →
  H-042 → H-050 → H-051 → H-052 → H-060 → H-061 → H-070 → H-071 → H-080 → H-081 →
  H-130 → H-131 → H-132/133 → H-140 → H-141 → H-142 → H-150 → H-151 → H-160 →
  H-161 → H-162.** Completing that chain proves the full publish→run-anywhere loop.
- H-130 (FastAPI backend) is the one task that can begin out of Rust order once
  H-042 can produce real test archives; it is otherwise independent until push
  (H-151) and run-from-registry (H-161) integrate against it.
- Everything else (init H-020/021, permissions H-090, models H-100/101, GitHub
  import H-11x, list/rm H-12x, yank H-17x, SDK H-18x, website H-19x) is off the
  strict core-loop path but required for full V1 spec conformance.
