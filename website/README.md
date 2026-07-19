# Xelian website

Browse, search, and publish Xelian packages. A Next.js client of the public
registry API — the same endpoints the CLI uses (SPEC §14.9); there is no
privileged control plane.

## Develop

```bash
npm install
NEXT_PUBLIC_REGISTRY_URL=http://localhost:8000 npm run dev
```

Start the registry first:

```bash
cd ../registry && uv run uvicorn app.main:app --port 8000
```

## Deploy (Vercel free tier)

Set one environment variable:

- `NEXT_PUBLIC_REGISTRY_URL` — the public registry base URL.

## Pages

- `/` — hero, search, package grid (agents / MCP servers)
- `/packages/[owner]/[name]` — README, metadata, declared permissions and
  features, version history
- `/login`, `/signup` — registry-native accounts
- `/new` — upload a `.xelian` archive + `xelian.lock` to publish

Auth tokens are registry-issued bearer tokens stored in localStorage; README
markdown is rendered through `rehype-sanitize`.
