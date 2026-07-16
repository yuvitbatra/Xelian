# Phases 0–5: What was built, how it was verified, how to see it yourself

Date: 2026-07-16. All 19 backlog tasks H-001 → H-052 (Phases 0–5) are complete and
checked off in `BACKLOG.md`. 112 automated tests pass; the full author→consumer
loop was also exercised manually end-to-end with the real binary.

## What was built

**Phase 0 — workspace & CLI skeleton** (`H-001..003`)
- Cargo workspace: `crates/harbor-cli` (thin dispatcher, binary named `harbor`) +
  `crates/harbor-core` (all logic). `harbor --help` lists all nine V1 commands with
  the spec'd flags (`rm --env/--all`, `yank --version/--undo`); unimplemented ones
  exit non-zero with "not implemented"; `-V` prints the version.
- `cache.rs`: `HarborHome` lazily creates `~/.harbor/{packages,runtimes,envs,models,logs,tmp}`;
  never touches `credentials.toml` (§11.3). Cache is source-namespaced per the
  2026-07-16 decision: `packages/local/<name>/<version>/` (registry/github later).

**Phase 1 — manifest parse & validate** (`H-010..014`)
- `manifest.rs` + `errors.rs`: typed structs for every §6.1/§6.2 field, opaque
  `[config]`, unknown keys accepted but never interpreted. Single entry point
  `validate_manifest()` enforcing: supported `spec-version`; SemVer version;
  closed permissions enum (hard error) vs. features list (warning only, §17);
  `os` set; §19.3 name rules; `required=true`+`default` conflict; `runtime`
  captured but never parsed. The §6.4 spec example is a passing golden test.

**Phase 2 — `harbor init`** (`H-020/021`)
- Writes a valid-by-construction `harbor.toml` (name from directory when §19.3-valid,
  placeholders otherwise) + a `harbor.lock` skeleton. No network. Refuses to clobber
  either existing file without `--force`.

**Phase 3 — lockfile & checksums** (`H-030..032`)
- `checksum.rs`: streaming SHA-256 (`sha256:<hex>`), plus a hashing writer.
- `lockfile.rs`: all §7.2 keys; `native-lock-checksum` only when a native lockfile is
  declared. **Design decision (documented in code + scratch notes):** `package-checksum`
  is computed over the *logical file set* — sorted `path\0sha256(contents)\n` lines,
  excluding `harbor.lock` — not raw archive bytes. This satisfies both §7.3 (no circular
  hash) and §9.4 (recomputable by any downloader), and is immune to tar/gzip metadata
  nondeterminism. Proven by tests: mutating `harbor.lock` never changes the checksum;
  mutating any other file always does.

**Phase 4 — packaging & the §8.1 pipeline** (`H-040..042`)
- `package.rs`: file collection honoring `.gitignore` exactly as git does (`ignore`
  crate, works outside git repos, nested + negation cases tested); `.git/`, prior
  `*.harbor` archives, and crashed `*.harbor.tmp` staging files always excluded; no
  force-include. Deterministic tar.gz (sorted entries, zeroed metadata): building
  twice yields identical bytes.
- `validate.rs`: §8.1 steps 1–8 in order, fail-fast, staged-temp-then-rename so a
  failure never leaves a partial archive. `[commands]` values are presence-checked
  as non-empty strings and **never executed** (§8.4). A gitignored entrypoint fails
  validation. `harbor push` runs this pipeline and stops with "upload not yet
  implemented" (Phase 15) after building the archive.

**Phase 5 — local run** (`H-050..052`)
- `run/mod.rs` + `run/extract.rs`: `harbor run ./pkg.harbor` reads the archive,
  recomputes the package checksum from its entries, and aborts **before extraction**
  on mismatch; validates the manifest before using its name/version for cache
  addressing (plus a destination-confinement check — a crafted manifest cannot
  escape `~/.harbor/packages/`); rejects `..`/absolute tar paths; extracts via
  `tmp/` + atomic rename (interrupted or concurrent runs can't corrupt the cache);
  skips re-extraction of an already-cached version; re-validates the extracted
  manifest and enforces the `os` check (§9.6.1). Exits 0 with
  "launch not yet implemented (Phase 8)".

## How I know it works

**Automated: 112 tests, 0 failures** (`cargo test` from the repo root; also clean
`cargo clippy --workspace --all-targets`):
- harbor-core lib: 91 unit tests (manifest rules table-driven incl. the §6.4 golden
  manifest; checksum determinism + harbor.lock-exclusion; gitignore nesting/negation;
  archive byte-determinism; path-traversal and checksum-tamper rejection; skip-if-cached;
  executable-bit round-trip; rename-race; OS gating).
- harbor-cli integration tests (21) spawn the **compiled binary**: help/version/flag
  constraints (9), `init` create/clobber/force (3), `push` builds a spec-conforming
  archive verified with the system `tar` (5), `run` end-to-end against a real built
  archive under an isolated `$HOME` (4).

**Process:** each wave was implemented by a fresh subagent, then reviewed by an
independent reviewer subagent against SPEC.md (spec compliance + code quality), and
every Critical/Important finding was fixed and re-reviewed before moving on. Findings
caught and fixed this way: a short-write hashing bug, an incomplete init clobber
guard, `.harbor.tmp` re-ingestion, and — most importantly — a cache-escape path
traversal via a crafted manifest name, plus executable-bit loss, an unbounded
pre-allocation, and a concurrent-extraction race.

**Manual end-to-end (real binary, isolated `$HOME`):**
init → push → tar-inspect → run → cached re-run → tamper-reject, all behaving per spec
(gitignored `secret.txt` stayed out of the archive; a flipped byte inside the archive
was rejected before extraction with both checksums printed).

## See it yourself (2 minutes)

```bash
cd ~/Desktop/School/summer/harbor
cargo test                       # 112 pass
cargo build

mkdir /tmp/weather-agent && cd /tmp/weather-agent
~/Desktop/School/summer/harbor/target/debug/harbor init
printf '# demo\n' > README.md && printf 'MIT\n' > LICENSE
mkdir src && printf 'print("hi")\n' > src/main.py
printf '[project]\nname="x"\nversion="0.1.0"\n' > pyproject.toml
printf 'secret.txt\n' > .gitignore && printf 'sshh\n' > secret.txt

~/Desktop/School/summer/harbor/target/debug/harbor push   # builds weather-agent-0.1.0.harbor, exits 1 at the (unimplemented) upload step
tar -tzf weather-agent-0.1.0.harbor                        # spec layout; no secret.txt
~/Desktop/School/summer/harbor/target/debug/harbor run ./weather-agent-0.1.0.harbor   # verifies, extracts to ~/.harbor/packages/local/..., exit 0
~/Desktop/School/summer/harbor/target/debug/harbor run ./weather-agent-0.1.0.harbor   # instant: "(cached)"
```

Git history is one commit per backlog task (`git log --oneline`), plus one commit per
review-fix round.

## Known limitations / notes for next phases

- **Deferred minor review findings** (cosmetic/perf, none blocking): duplicated §19.3
  name-check between `init.rs` and `manifest.rs`; `sha256_file` double-buffering; two
  weak test assertions. Worth a cleanup pass before Phase 6.
- **Spec drift to amend in SPEC.md** (already flagged in IMPLEMENTATION.md): §9.2 needs
  the local-path target form; §11.1 needs the source-based cache layout
  (`packages/local|github|registry/...`).
- Local packages are cache-keyed on `(name, version)` per the 2026-07-16 decision — two
  different local projects with the same name+version share a cache slot. Fine for V1,
  worth revisiting when registry/GitHub namespaces land.
- `harbor push` currently exits 1 by design after building (upload is Phase 15).
