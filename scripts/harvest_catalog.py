#!/usr/bin/env python3
"""Harvest a catalog of permissively-licensed MCP servers and AI agents from
GitHub for the Xelian registry index.

Xelian's registry is an *index*: it lists discoverable packages that each run
via `xelian add <github-url>` (commit-pinned, credited), rather than
republishing anyone's code. This script produces that index — a catalog of
public repositories whose license permits redistribution/use (MIT, Apache-2.0,
BSD, ISC, MPL-2.0, Unlicense), in the languages Xelian runs (Python, TS, JS).

License and all metadata come *inline* from the GitHub search API (each result
carries its `license` and `owner`), so no per-repository calls are needed — the
whole harvest costs a handful of search requests.

Usage:
    GITHUB_TOKEN=... python scripts/harvest_catalog.py          # authenticated (recommended)
    python scripts/harvest_catalog.py                            # unauthenticated (rate-limited)
    python scripts/harvest_catalog.py --out catalog.json --min-stars 20

Output: a JSON catalog at --out (default: registry/catalog.json).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

PERMISSIVE = {
    "mit",
    "apache-2.0",
    "bsd-2-clause",
    "bsd-3-clause",
    "isc",
    "mpl-2.0",
    "unlicense",
    "0bsd",
}

# Search queries, each already scoped to a permissive license + a language.
# GitHub ANDs the qualifiers; we run the cross-product of intent × language.
SERVER_INTENTS = [
    "mcp-server in:name,description",
    "mcp in:name",
    "topic:mcp",
    "topic:mcp-server",
    "topic:mcp-servers",
    '"model context protocol" in:name,description,readme',
    "topic:model-context-protocol",
    "topic:modelcontextprotocol",
    "mcp server in:name,description",
]
AGENT_INTENTS = [
    "topic:ai-agent",
    "topic:ai-agents",
    "topic:llm-agent",
    "topic:llm-agents",
    "topic:autonomous-agents",
    "topic:agents",
    "topic:agent in:name,description",
    "ai agent in:name,description",
    "topic:agentic",
    "topic:multi-agent",
    "topic:autonomous-ai",
]
LANGUAGES = ["python", "typescript", "javascript"]

API = "https://api.github.com/search/repositories"


def _headers() -> dict:
    h = {"Accept": "application/vnd.github+json", "User-Agent": "xelian-harvester"}
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        h["Authorization"] = f"Bearer {token}"
    return h


def search(query: str, pages: int = 2, per_page: int = 100) -> list[dict]:
    """Run one search query across `pages`, honoring rate limits politely."""
    out: list[dict] = []
    for page in range(1, pages + 1):
        params = urllib.parse.urlencode(
            {"q": query, "sort": "stars", "order": "desc", "per_page": per_page, "page": page}
        )
        req = urllib.request.Request(f"{API}?{params}", headers=_headers())
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                data = json.load(r)
        except urllib.error.HTTPError as e:
            if e.code in (403, 429):  # rate limited: back off and retry once
                reset = e.headers.get("X-RateLimit-Reset")
                wait = max(2, int(reset) - int(time.time())) if reset else 60
                print(f"  rate-limited; waiting {wait}s...", file=sys.stderr)
                time.sleep(min(wait, 65))
                try:
                    with urllib.request.urlopen(req, timeout=30) as r:
                        data = json.load(r)
                except Exception as e2:
                    print(f"  giving up on this page: {e2}", file=sys.stderr)
                    break
            else:
                print(f"  HTTP {e.code} for {query!r}", file=sys.stderr)
                break
        except Exception as e:
            print(f"  error for {query!r}: {e}", file=sys.stderr)
            break

        items = data.get("items", [])
        out.extend(items)
        if len(items) < per_page:
            break  # last page
        time.sleep(2)  # stay under the search rate limit
    return out


def classify(repo: dict, default: str) -> str:
    """agent vs mcp, from name/description/topics; falls back to the query's intent."""
    text = " ".join(
        [
            repo.get("name", ""),
            repo.get("description") or "",
            " ".join(repo.get("topics", [])),
        ]
    ).lower()
    if "mcp" in text or "model context protocol" in text or "model-context-protocol" in text:
        return "mcp"
    if "agent" in text:
        return "agent"
    return default


def language_ok(repo: dict) -> bool:
    return (repo.get("language") or "").lower() in {"python", "typescript", "javascript"}


def harvest(min_stars: int) -> list[dict]:
    seen: dict[str, dict] = {}

    def collect(intents: list[str], kind: str):
        for intent in intents:
            for lang in LANGUAGES:
                q = f"{intent} language:{lang} stars:>={min_stars}"
                # License is filtered from results (a single `license:` qualifier
                # cannot express "any permissive one", so filter post-hoc).
                print(f"search: {q}", file=sys.stderr)
                for repo in search(q):
                    lic = ((repo.get("license") or {}).get("spdx_id") or "").lower()
                    if lic not in PERMISSIVE:
                        continue
                    if not language_ok(repo):
                        continue
                    if repo.get("archived") or repo.get("fork"):
                        continue
                    full = repo["full_name"]
                    if full in seen:
                        continue
                    seen[full] = {
                        "name": repo["name"],
                        "owner": repo["owner"]["login"],
                        "full_name": full,
                        "url": repo["html_url"],
                        "description": (repo.get("description") or "").strip(),
                        "stars": repo.get("stargazers_count", 0),
                        "language": repo.get("language"),
                        "license": (repo.get("license") or {}).get("spdx_id"),
                        "topics": repo.get("topics", []),
                        "type": classify(repo, kind),
                    }
                time.sleep(2)

    collect(SERVER_INTENTS, "mcp")
    collect(AGENT_INTENTS, "agent")
    return sorted(seen.values(), key=lambda r: r["stars"], reverse=True)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="registry/catalog.json")
    ap.add_argument("--min-stars", type=int, default=15)
    args = ap.parse_args()

    if not os.environ.get("GITHUB_TOKEN"):
        print(
            "warning: no GITHUB_TOKEN — unauthenticated search is limited to ~10/min, "
            "so this harvest will be partial. Export a token for the full run.",
            file=sys.stderr,
        )

    catalog = harvest(args.min_stars)
    servers = [r for r in catalog if r["type"] == "mcp"]
    agents = [r for r in catalog if r["type"] == "agent"]

    payload = {
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "note": (
            "Index of permissively-licensed public repositories runnable via "
            "`xelian add`. Xelian does not host this code; it links to and runs "
            "each project under its own license, with attribution."
        ),
        "counts": {"total": len(catalog), "mcp": len(servers), "agents": len(agents)},
        "packages": catalog,
    }
    with open(args.out, "w") as f:
        json.dump(payload, f, indent=2)
    print(
        f"wrote {len(catalog)} packages ({len(servers)} servers, {len(agents)} agents) to {args.out}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
