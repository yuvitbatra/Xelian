# Contributing to Xelian

Thanks for your interest. Xelian is early — issues, packages, and small
focused PRs are all welcome.

## Building

Prerequisites: Rust (stable), Python 3.11+, [uv](https://docs.astral.sh/uv/),
Node 20+ (website only).

```bash
# CLI + runtime
cargo build --workspace

# Registry
cd registry && uv sync && uv run uvicorn app.main:app --port 8000

# Website
cd website && npm install && NEXT_PUBLIC_REGISTRY_URL=http://localhost:8000 npm run dev

# SDK (wraps the CLI binary — build the CLI first)
cd sdk && python -m pip install -e .
```

## Testing

```bash
cargo test --workspace          # Rust unit + CLI integration tests
cd registry && uv run pytest    # registry API tests
cd website && npm run build     # website type-check + production build
```

All tests must pass before a PR is merged. If you change the package
checksum, manifest, or lockfile logic, change it on **both** sides (Rust CLI
and Python registry) and add a test proving they agree — the two sides of
every contract are tested against each other, never only against themselves.

## What to work on

- Check open issues, especially those labeled `good first issue`.
- Publishing packages to the registry is as valuable as code — an empty
  registry is a dead registry.
- New features should directly improve the package format, runtime, registry,
  or developer experience; open an issue to discuss before building.

## Style

- Rust: `cargo fmt` and `cargo clippy` clean.
- Keep the CLI surface small and obvious — it should feel like git/cargo/uv.
- Prefer the simplest solution that works.
