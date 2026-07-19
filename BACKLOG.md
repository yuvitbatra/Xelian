# Xelian Backlog

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
  - Acceptance: `xelian-cli` (bin) + `xelian-core` (lib) build under one
    workspace; `cargo build` succeeds; binary is named `xelian`.

- [x] **H-002 — Wire clap command surface (all 9 commands + flags)**
  - Difficulty: M · Duration: 4h · Deps: H-001
  - Acceptance: `xelian --help` lists `init, push, run, add, list, rm, login,
    logout, yank` with correct flags — `rm` has `--env`/`--all` (§13.6), `yank`
    has `--version`/`--undo` (§13.9); each stub exits non-zero with "not
    implemented"; `-V` prints the binary version.

- [x] **H-003 — Cache layout module for `~/.xelian/`**
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

## Phase 2 — xelian init

- [x] **H-020 — Generate `xelian.toml` skeleton**
  - Difficulty: M · Duration: 4h · Deps: H-014
  - Acceptance: `xelian init` writes a `xelian.toml` with valid defaults +
    clearly marked placeholders (name from dir if §19.3-valid, else placeholder);
    output parses under H-014 (modulo intentional placeholders); no network
    (§13.1).

- [x] **H-021 — Generate `xelian.lock` skeleton + clobber guard**
  - Difficulty: S · Duration: 2h · Deps: H-020
  - Acceptance: a valid `xelian.lock` shell is written; existing `xelian.toml`
    is not silently overwritten (documented flag/prompt behavior); no network.

---

## Phase 3 — xelian.lock & checksums

- [x] **H-030 — SHA-256 helper + native-lock-checksum**
  - Difficulty: S · Duration: 3h · Deps: H-001
  - Acceptance: SHA-256 of a native lockfile matches independent `sha256sum`;
    computed only when a native lockfile is declared (§7.2, §7.3).

- [x] **H-031 — `package-checksum` excluding `xelian.lock`**
  - Difficulty: M · Duration: 4h · Deps: H-030
  - Acceptance: checksum is deterministic for identical inputs; mutating
    `xelian.lock` does NOT change `package-checksum` (§7.3); `xelian.lock` is
    never itself hashed.

- [x] **H-032 — Populate all `xelian.lock` keys**
  - Difficulty: M · Duration: 3h · Deps: H-031, H-010
  - Acceptance: every §7.2 key is present with correct values
    (`spec-version`, `xelian-version`, `package-version` copied from manifest,
    `generated-at` ISO 8601 UTC, native paths, both checksums).

---

## Phase 4 — Packaging / archive build

- [x] **H-040 — tar.gz archive builder**
  - Difficulty: M · Duration: 4h · Deps: H-032
  - Acceptance: builds a `.xelian` inspectable via `tar -tzf` with the §5.2/§5.3
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
  - Acceptance: `xelian run ./x.xelian` recomputes SHA-256 and aborts before
    extraction on mismatch (§9.4); a tampered archive never extracts.

- [x] **H-051 — Safe extraction into version-scoped cache**
  - Difficulty: M · Duration: 4h · Deps: H-050, H-003
  - Acceptance: extracts into the source-based cache (decision 2026-07-16):
    `~/.xelian/packages/local/<name>/<version>/` for local archives (registry
    and GitHub sources get their own namespaces later); rejects `..`/absolute
    tar paths; skips re-extraction of an already-present version (§9.5).

- [x] **H-052 — Manifest re-validation + OS check**
  - Difficulty: S · Duration: 3h · Deps: H-051, H-014
  - Acceptance: re-parses/re-validates `xelian.toml` (§9.6); if `os` is declared
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
    delegated to `uv`, not parsed in Xelian (§6.1).

- [x] **H-062 — Node path: ensure Node + `npm`**
  - Difficulty: M · Duration: 5h · Deps: H-060
  - Acceptance: auto-installs Node if absent; selects a runtime satisfying the
    SemVer range via `npm` (§9.7, §10.2).

---

## Phase 7 — Environment + dependency install

- [x] **H-070 — Environment cache keyed on (name, version)**
  - Difficulty: M · Duration: 4h · Deps: H-061
  - Acceptance: exactly one env per `(name, version)` under `~/.xelian/envs/`,
    mirroring the source-based cache layout (decision 2026-07-16); key is
    `(name, version)` only, no dependency hash (§9.8); reused on subsequent runs.

- [x] **H-071 — Delegate dependency install to native manager**
  - Difficulty: M · Duration: 5h · Deps: H-070
  - Acceptance: deps installed via `uv`/`npm` against the native manifest+lockfile
    (§6.1.2, §9.8); Xelian never re-declares/re-resolves deps; interrupted
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
    terminal as an interactive REPL; `xelian run` blocks for the session
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
    `~/.xelian/models/`/Ollama store and reused on later runs (§9.9, §18);
    correctly sequenced as pipeline step 10 before launch (§9.1).

---

## Phase 11 — GitHub import (xelian add)

- [x] **H-110 — Resolve default branch to SHA + download at SHA**
  - Difficulty: M · Duration: 5h · Deps: H-101
  - Acceptance: default branch resolved to a commit SHA; repo downloaded at that
    SHA; cached at `packages/github/<owner>/<repo>/<sha>/` — by SHA, not branch
    (§12.2 step 1; source-based cache decision 2026-07-16).

- [x] **H-111 — Language detection by precedence**
  - Difficulty: S · Duration: 3h · Deps: H-110
  - Acceptance: `pyproject.toml`→python, `package.json`→node, `Cargo.toml`→clear
    "unsupported language" error (§12.2 step 2); detection list is extensible.

- [x] **H-112 — Infer `xelian.toml` with placeholders**
  - Difficulty: L · Duration: 6h · Deps: H-111
  - Acceptance: infers `language`, `runtime`, `entrypoint`, `dependencies`;
    non-inferable fields get placeholders; import does not fail on placeholders
    (§12.2 step 3); nothing is published (§12.3).

- [x] **H-113 — Build package + run from step 6 onward**
  - Difficulty: M · Duration: 4h · Deps: H-112, H-042
  - Acceptance: generates `xelian.lock` + `.xelian` (§12.2 steps 4–5), caches by
    SHA (step 6), and runs via the existing pipeline from manifest validation
    onward (step 7, §9.6+).

---

## Phase 12 — xelian list & xelian rm

- [x] **H-120 — `xelian list` (local cache only)**
  - Difficulty: S · Duration: 3h · Deps: H-113
  - Acceptance: lists locally cached packages only, no registry search (§13.5,
    §22).

- [x] **H-121 — `xelian rm` variants + credential isolation**
  - Difficulty: M · Duration: 4h · Deps: H-120
  - Acceptance: `rm owner/package` removes cached versions but keeps envs;
    `--env` also removes the env; `--all` clears `packages/`/`envs/`/`runtimes/`/
    `models/` but never `credentials.toml` (§13.6, §11.3); deletes are guarded to
    stay within `~/.xelian/`; never touches the registry.

---

## Phase 13 — Registry backend (FastAPI)

- [x] **H-130 — FastAPI project + data model**
  - Difficulty: M · Duration: 4h · Deps: none (Python; can start after H-042 exists to produce test archives)
  - Acceptance: `Account`/`Package`/`Versions[]` modeled per §14.2 with storage
    for archive, checksum, `xelian.lock`, README, metadata, `published_at`,
    `yanked`.

- [x] **H-131 — `POST /packages` with publish-time checks**
  - Difficulty: L · Duration: 6h · Deps: H-130
  - Acceptance: accepts an upload; verifies archive SHA-256 matches
    `package-checksum` in the accompanying `xelian.lock` (§14.5); rejects an
    already-published `(name, version)` (§14.5, §19.2); never executes the
    package (§14.1). Request/response schema documented as the client contract
    (§14.8).

- [x] **H-132 — `GET /packages/{owner}/{package}` + resolution**
  - Difficulty: M · Duration: 4h · Deps: H-131
  - Acceptance: returns metadata for the resolved latest version = highest
    SemVer that is not yanked and not pre-release; clear error if none (§14.3,
    §19.1).

- [x] **H-133 — `GET /download/{owner}/{package}/{version}`**
  - Difficulty: S · Duration: 3h · Deps: H-131
  - Acceptance: returns the exact version's archive bytes (§14.8); immutability
    holds (no in-place edit path, §14.6/§19.2).

---

## Phase 14 — login/logout + credentials

- [x] **H-140 — Registry auth route(s) (OAuth token/callback)**
  - Difficulty: M · Duration: 5h · Deps: H-130
  - Acceptance: `POST /auth/token` issues bearer tokens via env-var-configured
    credentials. Auth middleware protects `POST /packages`. Owner namespace
    enforcement matches authenticated user (§14.4).

- [x] **H-141 — `xelian login` browser flow + `credentials.toml` (0600)**
  - Difficulty: M · Duration: 5h · Deps: H-140, H-003
  - Acceptance: prompts for username/password, exchanges for token at
    `POST /auth/token`, writes `~/.xelian/credentials.toml` at `0600` atomically.
    Stores registry URL for reuse.

- [x] **H-142 — `xelian logout`**
  - Difficulty: S · Duration: 2h · Deps: H-141
  - Acceptance: removes `credentials.toml`; `xelian rm --all` preserves it
    (cross-check H-121 passes). Idempotent — no error if already logged out.

---

## Phase 15 — xelian push end-to-end

- [x] **H-150 — Registry HTTP client (authenticated)**
  - Difficulty: M · Duration: 4h · Deps: H-142
  - Acceptance: `RegistryClient` in `registry_client.rs` with typed `login()`
    and `publish()` methods. Multipart upload with `Authorization: Bearer`
    header. Error handling for 401/403/409/422 responses.

- [x] **H-151 — `xelian push`: validate then upload**
  - Difficulty: M · Duration: 5h · Deps: H-150, H-042, H-131
  - Acceptance: reads credentials first (fails with "not logged in" if missing),
    runs full §8.1 validation before any network call, uploads to
    `POST /packages` with authenticated user as owner. Republishing an existing
    `(name, version)` returns 409; wrong-namespace returns 403.

---

## Phase 16 — xelian run from registry

- [x] **H-160 — Target-form discrimination**
  - Difficulty: S · Duration: 3h · Deps: H-052
  - Acceptance: registry-ref vs. GitHub-URL vs. local `.xelian` path (decision
     2026-07-16) distinguished syntactically before resolution; any other input
     fails clearly rather than guessing (§9.2); no `@version` pin syntax accepted
     (§9.2, §22).

- [x] **H-161 — Registry resolution + cache-check + download**
  - Difficulty: M · Duration: 5h · Deps: H-160, H-150, H-132, H-133
  - Acceptance: `owner/package` resolves to latest stable non-yanked
     non-pre-release (§9.2, §14.3); cache is checked before any network request
     and download is skipped on hit (§9.3); missing archive downloaded via
     `GET /download/...` staged through `tmp/` (§9.3, §11.1).

- [x] **H-162 — Wire download into local run pipeline**
  - Difficulty: M · Duration: 4h · Deps: H-161, H-101
  - Acceptance: downloaded archive is checksum-verified (§9.4) then flows through
     the existing extract→…→launch pipeline; end-to-end `xelian push` then, on a
     clean cache, `xelian run owner/package` launches (Appendix C.1–C.2); cache
     persists (§9.11).

---

## Phase 17 — xelian yank

- [x] **H-170 — Registry yank/unyank route (owner-authorized)**
  - Difficulty: M · Duration: 4h · Deps: H-132, H-140
  - Acceptance: a route marks `yanked = true`/`false` for a version (TODO-15
     sketch: `PATCH /packages/{owner}/{package}/{version}`); authorized only for
     the owning account (§14.4); never deletes archive/checksum/metadata
     (§14.7.1).

- [x] **H-171 — `xelian yank` CLI (+ `--undo`)**
  - Difficulty: S · Duration: 3h · Deps: H-170, H-150
  - Acceptance: `xelian yank owner/package --version <v>` yanks; `--undo`
     reverses (§13.9); after yanking the latest, `xelian run` resolves to the next
     non-yanked version or fails clearly (§14.3, §14.7.1); already-cached clients
     unaffected; no hard delete (§14.7.2, §22).

---

## Phase 18 — Python SDK

- [x] **H-180 — SDK skeleton wrapping the CLI**
  - Difficulty: M · Duration: 4h · Deps: H-162
  - Acceptance: `sdk/` Python package shells out to the `xelian` binary; no
    reimplementation of resolution/validation/execution (§15.1).

- [x] **H-181 — `install` / `run` / `agent` / `mcp` entry points**
  - Difficulty: M · Duration: 5h · Deps: H-180
  - Acceptance: `install` performs steps 1–9 without launching (§15.2); `run`
    performs the full pipeline and returns a type-appropriate handle; `agent`/
    `mcp` raise on package-type mismatch (§15.2).

- [x] **H-182 — Handles: `.chat()` and `.expose()`**
  - Difficulty: L · Duration: 6h · Deps: H-181
  - Acceptance: agent handle `.chat()` returns a response; MCP handle `.expose()`
    makes the local server available to an MCP client (§15.2); surface beyond
    these two methods deferred (TODO-29).

---

## Phase 19 — Website (read-only)

> Done 2026-07-19, extended by owner decision to include accounts and
> publishing through the same public API (§14.9 respected — the site has no
> privileged path; it calls the exact endpoints the CLI uses). Registry
> gained `POST /auth/signup` (scrypt-hashed passwords, disk-persisted hashed
> tokens, no more admin/admin default) and public `GET /packages` for the
> browse surface. Verified end-to-end with a headless browser: CLI push →
> site shows package; UI signup/login → website publish → 409 on duplicate.

- [x] **H-190 — Next.js app + registry API client**
  - Difficulty: M · Duration: 5h · Deps: H-132, H-133
  - Acceptance: site reads the registry API read-only (§14.9); no write path
    beyond the public API (§14.9).

- [x] **H-191 — Browse/search + package page**
  - Difficulty: L · Duration: 6h · Deps: H-190
  - Acceptance: lists/searches published packages; a package page renders README
    and metadata (§14.2), including declared permissions (§16.3) and features
    (§17).

---

> **Phases 20–25 below are the productionization & launch plan** (added
> 2026-07-18; infra decision revised same day). V1 code is feature-complete;
> these phases make it shippable, discoverable, and adopted. Infra decision of
> record: **$0/month until real traction** — GitHub (code/CI/releases), Vercel
> (website), **Neon Postgres free tier as the one and only database from day
> one (no SQLite anywhere)** via a single `DATABASE_URL`, Cloudflare R2
> (archive storage, 10 GB free), PyPI (SDK), and the registry API on a free
> host (e.g. Render free tier) with idle spin-down cold starts **accepted** as
> the cost of free — mitigate cheaply with a free uptime pinger (e.g.
> UptimeRobot on `/health` every 5 min keeps the instance warm) and revisit
> paid always-on hosting only after users/funding justify it. Archives are
> NEVER stored in the database — metadata rows only.

## Phase 20 — Rename & repo foundation

> Code-side complete 2026-07-19. Decision of record: the project is renamed
> **Xelian** (resolves the CNCF Harbor collision; see
> docs/superpowers/specs/2026-07-19-xelian-rename-website-design.md). Binary
> `xelian`, crates `xelian-cli`/`xelian-core`, `xelian.toml`/`xelian.lock`,
> `.xelian` archives, `~/.xelian`, `XELIAN_*` env vars, SDK package `xelian`.
>
> **Remaining owner-only actions (cannot be done from this machine):**
> - [ ] Claim GitHub org/repo `xelian`, PyPI `xelian`, crates.io `xelian`,
>       Homebrew formula name; confirm availability first.
> - [ ] Claim the domain (GitHub Student Pack Namecheap `.me`, fallback
>       `is-a.dev`/`eu.org`) BEFORE the first release (H-225 compiles the
>       registry URL into every binary).
> - [ ] Record the ≤30s install→run→chat GIF (asciinema + agg; placeholder
>       marked in README.md) .
> - [ ] Enable GitHub Discussions; set repo description + topics
>       (mcp, ai-agents, registry, ollama, rust); add CI badges once
>       Phase 21 workflows exist.

- [x] **H-200 — Project rename decision + asset grab** *(code/docs done;
  name/domain claiming = owner actions above)*
  - Difficulty: S · Duration: 3h · Deps: none — **gates every public artifact;
    do first**
  - Acceptance: name collision with CNCF Harbor (goharbor.io — a famous
    registry where `harbor push` already means something else) is explicitly
    resolved: either a new name is chosen or keeping "Xelian" is a recorded
    decision. For the chosen name: GitHub org/repo, domain, PyPI, crates.io,
    and Homebrew formula names are confirmed available and claimed; binary
    name, `~/.xelian` dir name, and docs updated if renamed. Domain at $0:
    claim the GitHub Student Developer Pack free domain (Namecheap `.me`, 1
    year) — fall back to `is-a.dev`/`eu.org` if unavailable; Vercel/Render
    free subdomains are acceptable for the website but the domain MUST be
    owned before the first release, because the registry URL is compiled into
    every shipped binary (H-225) and a later domain change strands old
    installs (a domain you own can redirect forever; a host subdomain you
    don't control cannot).

- [x] **H-201 — Great README** *(GIF recording + CI badges pending on owner
  actions / Phase 21)*
  - Difficulty: M · Duration: 5h · Deps: H-200
  - Acceptance: README has (top to bottom): one-line pitch, a ≤30s terminal
    GIF/asciinema of install→run→chat, a 3-command quickstart that works on a
    clean machine, "package your own agent in 5 minutes" section, SDK snippet,
    package-format overview linking SPEC.md, CI/release badges. A stranger can
    go from zero to a running agent using only the README.

- [x] **H-202 — Repo hygiene for open source** *(Discussions/topics are
  GitHub-side owner actions above)*
  - Difficulty: S · Duration: 3h · Deps: H-200
  - Acceptance: LICENSE (MIT or Apache-2.0) at root and in both Python
    packages; CONTRIBUTING.md (build/test instructions); SECURITY.md (report
    channel); issue templates (bug/feature/package-report) + PR template;
    GitHub Discussions enabled; repo description + topics set (mcp, ai-agents,
    registry, ollama, rust); `.github/` committed.

---

## Phase 21 — CI & insane testing

> Done 2026-07-19 except H-213 (blocked on the H-230 install script, Phase
> 23). `.github/workflows/ci.yml` runs: Rust fmt/clippy(-D warnings)/tests on
> macOS + Linux, registry pytest (47 tests incl. abuse/fuzz, load, golden
> interop), SDK compile/import check, website build, and an E2E job
> (`scripts/e2e.sh`, verified green locally) driving the real binary against
> the real registry through the full loop, plus SDK integration tests
> against the real binary. Also added `xelian login --username
> --password-stdin` (pulled forward from H-222) because the E2E loop
> requires non-interactive login.

- [x] **H-210 — Core CI workflow**
  - Difficulty: M · Duration: 4h · Deps: none
  - Acceptance: on every PR + main push: `cargo fmt --check`, `cargo clippy`
    (deny warnings), `cargo test --workspace` on macOS + Linux runners,
    registry `pytest`, SDK syntax/import check. Red CI blocks merge.

- [x] **H-211 — E2E CI job: real push→run loop**
  - Difficulty: M · Duration: 5h · Deps: H-210
  - Acceptance: CI boots the actual FastAPI registry, then drives the real
    binary through: login (non-interactive) → push → duplicate-push (expect
    409) → clean-cache run (agent responds) → yank → run (expect no-version
    failure) → unyank → rm. This is the test class that would have caught the
    checksum-dialect bug that shipped with 100% green unit tests — the two
    sides must be tested against each other, never only against themselves.

- [x] **H-212 — Cross-implementation checksum interop test**
  - Difficulty: S · Duration: 2h · Deps: H-210
  - Acceptance: a shared golden fixture (archive + expected §7.3 checksum) is
    asserted identical by BOTH the Rust `compute_package_checksum` tests and
    the registry's Python `compute_package_checksum` tests; any drift in
    either implementation fails CI.

- [ ] **H-213 — Clean-machine quickstart test** *(blocked on H-230 — the
  public install script does not exist yet)*
  - Difficulty: M · Duration: 4h · Deps: H-230
  - Acceptance: a CI job runs the public install script inside a bare Docker
    image (no Rust, no repo checkout), then `xelian run <seed-package>` against
    a live registry and asserts a response — proving the README quickstart
    verbatim. Runs nightly + before every release.

- [x] **H-214 — Abuse & fuzz suite (registry + archive handling)**
  - Difficulty: M · Duration: 6h · Deps: H-210
  - Acceptance: tests cover tar decompression bombs, oversized uploads
    (rejected at the declared cap), malformed/truncated archives, malformed
    manifests/lockfiles, path-traversal payloads in every route param and tar
    entry name, and concurrent duplicate publishes (exactly one 201). Registry
    never crashes, never writes outside its storage root.

- [x] **H-215 — SDK integration tests**
  - Difficulty: S · Duration: 3h · Deps: H-210
  - Acceptance: pytest suite runs `xelian.install/run/agent/mcp` against the
    real built binary and a local registry: agent `.chat()` round-trips, mcp
    `.expose()` returns usable transport info, type mismatch raises,
    missing-binary and not-logged-in errors are clear. Wired into CI.

- [x] **H-216 — Registry load sanity test**
  - Difficulty: S · Duration: 3h · Deps: H-211
  - Acceptance: a scripted burst (e.g. 50 concurrent downloads + metadata
    reads of a mid-size package) completes with zero 5xx and bounded memory —
    guards the streaming work (H-222) against regression.

---

## Phase 22 — Production registry (accounts, DB, limits, deploy)

- [ ] **H-220 — Data layer: Postgres via SQLAlchemy + `DATABASE_URL`**
  - Difficulty: M · Duration: 6h · Deps: H-130..H-133 (done)
  - Acceptance: accounts/tokens/packages/versions move from JSON-on-disk to a
    Postgres data layer configured by a single `DATABASE_URL` (**Postgres
    only — no SQLite fallback anywhere**); Neon free tier in production;
    local dev + CI run against a real Postgres (Docker service container in
    CI, `docker run postgres` or a Neon dev branch locally) so tests exercise
    the exact engine prod uses. Archive bytes stay on disk/object-storage,
    never in the DB. Existing pytest suite passes against Postgres in CI.

- [ ] **H-221 — Real accounts + persisted tokens**
  - Difficulty: L · Duration: 8h · Deps: H-220
  - Acceptance: signup endpoint (username/password, hashed with
    argon2/bcrypt); per-user bearer tokens persisted in the DB (survive server
    restart — today's in-memory dict logs everyone out on redeploy) with
    expiry + revocation; the `admin`/`admin` env-var fallback is REMOVED — the
    server refuses to start with default/unset credentials; owner-namespace
    enforcement unchanged (§14.4).

- [ ] **H-222 — Registry limits + streaming**
  - Difficulty: M · Duration: 5h · Deps: H-221
  - Acceptance: upload size cap (start ~100 MB, configurable) enforced before
    buffering; decompression-bomb guard on server-side tar reads; downloads
    streamed (FileResponse) not buffered; request timeouts; basic per-IP rate
    limiting on auth + publish; CLI streams downloads to disk instead of
    holding whole archives in memory.

- [ ] **H-223 — Non-interactive auth + URL precedence**
  - Difficulty: S · Duration: 3h · Deps: H-221
  - Acceptance: `xelian login --username X --password-stdin` and
    `XELIAN_TOKEN` env var work without a TTY (rpassword currently hard-fails
    in scripts/CI/SDK contexts); `xelian login` honors `XELIAN_REGISTRY_URL`
    over the stored credentials URL (today the env var is silently ignored
    once any credentials file exists); signup reachable from CLI or website.

- [ ] **H-224 — Search: registry endpoint + `xelian search`**
  - Difficulty: M · Duration: 5h · Deps: H-220
  - Acceptance: `GET /search?q=` over name/description/tags (SQL LIKE is
    fine); `xelian search <term>` renders results; website search (H-191)
    consumes the same endpoint. Without this there is literally no way to
    discover what exists.

- [ ] **H-225 — Deploy the registry publicly (free tier)**
  - Difficulty: M · Duration: 5h · Deps: H-221, H-222
  - Acceptance: registry live at `registry.<domain>` over HTTPS on a free
    host (e.g. Render free tier); Neon `DATABASE_URL` in prod; archives on
    R2 (free hosts have ephemeral disks — object storage is REQUIRED here,
    not optional, or published packages vanish on redeploy); automated
    DB + storage backups; the CLI's compiled-in default registry URL points
    at the real domain (not `http://localhost:8000`); a free uptime pinger
    hits `/health` every 5 min to keep the instance warm (cold starts
    accepted by decision of record until traction justifies paid hosting).

- [ ] **H-226 — Ops baseline**
  - Difficulty: S · Duration: 3h · Deps: H-225
  - Acceptance: structured request logging; error alerting (Sentry free tier
    or equivalent); a documented restore-from-backup drill actually performed
    once; uptime check on `/health`.

---

## Phase 23 — Distribution (binaries, install, PyPI)

- [ ] **H-230 — Release workflow + install script**
  - Difficulty: L · Duration: 8h · Deps: H-210, H-200
  - Acceptance: tagging a release cross-compiles macOS (arm64 + x86_64) and
    Linux (x86_64, arm64) binaries, attaches them to GitHub Releases with
    checksums; `curl -fsSL https://get.<domain>/install.sh | sh` detects
    OS/arch, installs to PATH, and prints a success next-step; the script is
    treated as product-quality UX (clear errors, no sudo surprises).

- [ ] **H-231 — Homebrew tap**
  - Difficulty: S · Duration: 2h · Deps: H-230
  - Acceptance: `brew install <org>/tap/<name>` works on both Mac
    architectures; formula auto-bumped by the release workflow.

- [ ] **H-232 — SDK on PyPI**
  - Difficulty: S · Duration: 3h · Deps: H-200, H-215
  - Acceptance: package published under the confirmed name; `pip install` +
    two-line quickstart works; `find_binary` is PATH-first with a clear
    install hint when missing (the current `target/debug` repo-relative lookup
    is dev-only and must not ship); SDK version pinned/compatible with a
    stated minimum CLI version.

- [ ] **H-233 — Windows: verify or explicitly scope out**
  - Difficulty: M · Duration: 6h · Deps: H-230
  - Acceptance: either Windows binaries ship with paths/permissions/runtime
    provisioning verified in CI, or the README + install script state
    "macOS/Linux now, Windows soon" and fail gracefully — no silent breakage.

---

## Phase 24 — Seed content & private beta

- [ ] **H-240 — Seed the registry (15–20 real packages)**
  - Difficulty: L · Duration: 3d · Deps: H-225
  - Acceptance: an official namespace publishes the most-wanted MCP servers
    (filesystem, fetch, github, slack, postgres, memory, …) plus 3–5 genuinely
    fun/useful agents; each verified end-to-end on a clean machine; each has a
    README rendered on its package page. An empty registry is a dead registry
    — this outranks any remaining code polish.

- [ ] **H-241 — Private beta: 5–10 clean-machine users**
  - Difficulty: M · Duration: ongoing 1w · Deps: H-240, H-230
  - Acceptance: 5–10 people you don't share a laptop with run the README
    quickstart on their own machines; every failure or point of confusion is
    filed and fixed; at least 2 of them publish a package of their own with no
    help beyond the docs.

- [ ] **H-242 — Pre-package influencer/tool-author projects**
  - Difficulty: M · Duration: 4h · Deps: H-240
  - Acceptance: 10+ packages that wrap tools built by the specific people you
    intend to contact in H-252 (`xelian run <them>/<their-tool>` works) — the
    outreach hook is "your project already runs on this," which converts far
    better than "please try my thing."

---

## Phase 25 — Launch & growth

- [ ] **H-250 — Launch assets**
  - Difficulty: M · Duration: 5h · Deps: H-201, H-240
  - Acceptance: ≤30s terminal GIF (install → run → chat); a launch blog post
    ("agents need a package manager" / "ollama-for-MCP-servers" angle) hosted
    on the website; both linked from the README.

- [ ] **H-251 — Coordinated launch: Show HN + Reddit + X**
  - Difficulty: M · Duration: 1d + follow-up · Deps: H-241, H-250, H-213
  - Acceptance: Show HN post (Tue–Thu, US morning) with the one-liner demo;
    r/LocalLLaMA + r/mcp + r/selfhosted + r/rust posts within the same 48h;
    X thread; you are available to answer comments all day; quickstart
    verified green in CI that morning.

- [ ] **H-252 — Influencer/creator outreach (DMs)**
  - Difficulty: M · Duration: 6h + follow-up · Deps: H-242, H-250
  - Acceptance: a tracked list of 20–30 targets (local-AI YouTubers, MCP
    tutorial writers, AI-newsletter authors, agent-framework maintainers);
    each gets a personalized DM/email with (a) the 30s GIF, (b) the hook that
    their own tool is already runnable via `xelian run them/their-tool`
    (H-242), (c) early-access/founder framing and an offer of a walkthrough.
    Track sends/replies/coverage; follow up once, politely, after ~5 days.
    Expect a 10–20% reply rate — that's 3–6 pieces of coverage, which is a
    successful campaign.

- [ ] **H-253 — MCP ecosystem placement**
  - Difficulty: M · Duration: 4h + ongoing · Deps: H-240
  - Acceptance: PRs merged into `awesome-mcp-servers`-style lists; listed in
    MCP directories; at least one "set up MCP with xelian" tutorial published
    (yours counts, a third party's counts double). Goal: `xelian run x/y`
    appears as the easy path in MCP setup docs — the tutorial-default position
    is how Ollama became Ollama.

- [ ] **H-254 — Second-wave: Product Hunt + framework docs**
  - Difficulty: S · Duration: 3h · Deps: H-251
  - Acceptance: Product Hunt launch ~1 week after HN; PRs/examples submitted
    to agent-framework ecosystems (LangChain/CrewAI/etc.) showing the SDK
    two-liner.

- [ ] **H-255 — Post-launch operating loop (first month)**
  - Difficulty: M · Duration: ongoing · Deps: H-251
  - Acceptance: issues get a first response <24h; a visible release ships
    weekly; the first 10 external publishers get hands-on help; a simple
    dashboard/query tracks the one metric that matters — weekly `xelian run`
    downloads from the registry (stars measure attention; pulls measure
    product).

---

## Added post-plan — MCP Gateway (owner request, 2026-07-19)

- [x] **H-260 — `xelian gateway`: one local MCP endpoint for all MCP servers**
  - Difficulty: L · Duration: 8h · Deps: H-161
  - What: instead of wiring N MCP servers into every IDE/agent config, a
    client connects to a single Streamable-HTTP endpoint
    (`http://127.0.0.1:11432/mcp`). `xelian gateway add owner/name` configures
    backends (`~/.xelian/gateway.toml`); `serve` runs each through the
    standard prepare pipeline, spawns them as stdio children, namespaces
    tools as `<package>__<tool>`, routes `tools/call`, and **respawns dead
    backends on the next call**. `status` shows up/down + restart counts +
    log paths (`GET /status`); `logs` tails unified backend stderr from
    `~/.xelian/logs/gateway/`.
  - Scope guard: tools-only MVP (initialize/ping/tools/list/tools/call);
    resources/prompts/SSE deferred until a real client needs them.
  - Verified: live curl MCP session against a pushed package, kill →
    auto-respawn (restarts counter), alias-collision + bad-name errors.

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
- **Launch critical path (Phases 20–25):**
  **H-200 (rename — gates all public artifacts) → H-210/H-211 (CI + E2E) →
  H-201/H-202 (README/hygiene) → H-220 → H-221 → H-222/H-223 → H-225 (live
  registry) → H-230 (binaries + install) → H-232 (PyPI) → H-240 (seed
  content) → H-241 (beta) → H-250 → H-251 (launch) → H-252/H-253 (outreach).**
  H-210–H-216 (testing) and H-224 (search) can run in parallel with the
  registry work. Do not announce anything publicly before H-213 (clean-machine
  quickstart) is green — the launch-day audience gets exactly one first
  impression.
- Suggested cadence: Phases 20–21 in week 1, Phases 22–23 in week 2, Phase 24
  in week 3, launch (Phase 25) when H-241 has produced two independent
  publishers — not before.
