# Xelian rename + website (Phase 19) + repo foundation (Phase 20) — design

Date: 2026-07-19. Decided by the project owner; recorded here per H-200.

## 1. Rename decision (H-200)

The name collision with CNCF Harbor (goharbor.io) is resolved by renaming the
project to **Xelian**. The rename is total — nothing has been published yet, so
there is no backward compatibility surface:

| Surface | Before | After |
|---|---|---|
| Binary / CLI | `harbor` | `xelian` |
| Crates | `harbor-cli`, `harbor-core` | `xelian-cli`, `xelian-core` |
| Manifest file | `harbor.toml` | `xelian.toml` |
| Lockfile | `harbor.lock` | `xelian.lock` |
| Archive extension | `.harbor` | `.xelian` |
| User cache dir | `~/.harbor` | `~/.xelian` |
| Registry storage dir | `~/.harbor-registry` | `~/.xelian-registry` |
| Env vars | `HARBOR_*` | `XELIAN_*` |
| Python SDK package | `harbor` (dist `harbor-sdk`) | `xelian` (dist `xelian-sdk`) |
| Registry service | Harbor Registry | Xelian Registry |

Mechanics: case-aware replace (`harbor`→`xelian`, `Harbor`→`Xelian`,
`HARBOR`→`XELIAN`) across code, tests, and docs (SPEC.md, BACKLOG.md,
IMPLEMENTATION.md, CLAUDE.md), plus `git mv` of the crate/SDK directories.
`.venv/` and `target/` are excluded. All existing tests must pass afterward.

Manual actions only the owner can do (listed in BACKLOG under H-200): claim
GitHub org/repo, domain (GitHub Student Pack Namecheap `.me`), PyPI `xelian`,
crates.io `xelian`, Homebrew formula name.

## 2. Website (Phase 19, extended with authenticated publishing)

`website/` — Next.js (App Router) + Tailwind, deployable to Vercel free tier.

**Design language** (Hugging Face-inspired, per owner): white background, subtle
gray borders, rounded cards, one accent color, clean sans-serif type. No
emojis, no gradients. Dark text on light surfaces; simple and modern.

**Pages**
- `/` — hero with install one-liner, search bar, package grid (cards: name,
  type badge agent/mcp-server, description, tags, latest version).
- `/packages/[owner]/[name]` — rendered README (sanitized markdown), metadata
  sidebar (version, license, language, runtime, author), declared permissions
  (§16.3) and features (§17) shown prominently, versions list with yank state,
  copyable `xelian run owner/name` command.
- `/login`, `/signup` — registry-native auth.
- `/new` — authenticated publish: upload `.xelian` archive + `xelian.lock`,
  POST to the same public `/packages` endpoint the CLI uses.

**§14.9 compliance**: the site is a client of the public registry API only —
same endpoints an authenticated CLI account uses; no privileged control plane.

**Auth choice — registry-native, not Firebase.** Firebase would create a second
identity system the CLI cannot use, violating §14.9's "same API client" rule
and adding a paid-tier cloud dependency risk. Instead the registry gains real
accounts (below); the website stores the bearer token in localStorage and talks
to the registry directly. README markdown is sanitized (rehype-sanitize) so no
untrusted script runs on the site, which is the XSS vector that would threaten
a stored token. Cost: $0.

**Search**: a new public read-only `GET /packages` listing endpoint; the site
filters client-side (registry is small in V1). This is the "website-only read
surface" answer to the Phase 19 open question.

## 3. Registry: safe accounts (pulled forward from H-221, minimal)

- `POST /auth/signup {username, password}` — creates an account; username
  becomes the publish namespace (§14.4 unchanged: owner must equal user).
  Passwords hashed with bcrypt. Username charset = §19.3 safe segment.
- Users and tokens persist on disk under the registry storage root (JSON files,
  same storage engine the registry already uses; Postgres migration stays in
  Phase 22). Tokens are stored hashed (SHA-256) — a leaked store does not leak
  bearer tokens.
- The `admin/admin` env fallback is removed: env bootstrap credentials are
  honored only when explicitly set; otherwise auth is signup-based only.
- `GET /packages` — public list of all packages (latest-version summaries) for
  the website. Read-only, no auth.

## 4. Phase 20 (H-200, H-201, H-202)

- H-200: this document + BACKLOG note record the decision; name-claiming
  checklist added for the owner.
- H-201: README rewrite — one-line pitch, quickstart (3 commands), demo
  section (GIF placeholder + asciinema instructions, since recording needs a
  human), "package your agent in 5 minutes", SDK snippet, format overview
  linking SPEC.md.
- H-202: MIT LICENSE (root + registry + sdk), CONTRIBUTING.md, SECURITY.md,
  `.github/` issue templates (bug/feature/package-report) + PR template.
  Discussions/topics are GitHub-side manual actions, listed for the owner.

## 5. Testing

- Full Rust + registry test suites green post-rename.
- New registry tests: signup, login with hashed password, token persistence
  across restart, publish after signup, list endpoint.
- Website: production build passes; manual flow against a local registry
  (signup → publish via UI → package appears → README/permissions render).
