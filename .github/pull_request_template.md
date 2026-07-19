## What

## Why

## Testing

- [ ] `cargo test --workspace` passes
- [ ] `cd registry && uv run pytest` passes (if registry touched)
- [ ] `cd website && npm run build` passes (if website touched)
- [ ] Contract changes (checksum/manifest/lockfile) are covered by a test
      that exercises BOTH the Rust and Python implementations
