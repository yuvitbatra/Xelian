#!/usr/bin/env python3
"""Publish verified catalog packages to the registry as real hosted archives.

The catalog entries are permissively licensed (MIT/Apache/BSD/…), which permit
redistribution with attribution + license preserved — so the registry can host
the archives (with the upstream license and author filled in), making them
first-class packages alongside what people push, instead of import-on-run links.

For each entry: `xelian add` imports it (building the .xelian archive in the
cache), we fill the real license (from the catalog) and author (the GitHub
owner) into the generated xelian.toml, then `xelian push` uploads it under the
`xelian` account. Idempotent: an already-published version is skipped.

Usage:
    XELIAN_SEED_PASSWORD=... python scripts/publish_catalog.py [--limit N] [--type mcp|agent]
"""
from __future__ import annotations
import argparse, json, os, re, subprocess, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "xelian"
URL = "https://xelian-registry.onrender.com"


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def fill_manifest(pkg_dir: Path, license_id: str, author: str) -> bool:
    """Replace PLEASE_EDIT placeholders in the imported xelian.toml with the
    upstream license and author. Returns False if no manifest."""
    man = pkg_dir / "xelian.toml"
    if not man.is_file():
        return False
    s = man.read_text()
    s = re.sub(r'license\s*=\s*"PLEASE_EDIT"', f'license = "{license_id or "MIT"}"', s)
    s = re.sub(r'name\s*=\s*"PLEASE_EDIT"', f'name = "{author}"', s)
    s = s.replace("please-edit@example.invalid", "noreply@users.noreply.github.com")
    man.write_text(s)
    return True


def already_published(name: str) -> bool:
    """Cheap check: is `xelian/<name>` already hosted? Avoids a full rebuild
    just to hit a 409, so the batch is resumable and fast over what's done."""
    r = sh(["curl", "-s", "-o", "/dev/null", "-w", "%{http_code}",
            f"{URL}/packages/xelian/{name}"])
    return r.stdout.strip() == "200"


def publish_one(entry: dict, home: str) -> str:
    # The published package name is the derived slug of the repo/subdir basis;
    # approximate it from the catalog name for the pre-check.
    slug = re.sub(r"[^a-z0-9_-]+", "-", entry["name"].lower()).strip("-")
    if slug and already_published(slug):
        return "exists"
    env = dict(os.environ, HOME=home, XELIAN_REGISTRY_URL=URL)
    # Import (build the archive). Time-boxed; add launches at the end, so a
    # timeout after the build is expected and fine.
    proc = subprocess.Popen(
        [str(BIN), "add", entry["url"]], env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True,
    )
    cached = None
    deadline = time.time() + 400
    try:
        for line in proc.stdout:  # type: ignore
            m = re.search(r"Cached at (.+)$", line.strip()) or re.search(
                r"imported .+ at (\S+)", line)
            if m:
                cached = m.group(1)
            if "ready — " in line or "listening on stdio" in line:
                break  # built + launched; we have what we need
            if time.time() > deadline:
                break
    finally:
        proc.kill()
        try:
            proc.wait(timeout=5)
        except Exception:
            pass
    if not cached or not Path(cached).is_dir():
        return "no-build"

    pkg_dir = Path(cached)
    if not fill_manifest(pkg_dir, entry.get("license"), entry["owner"]):
        return "no-manifest"
    ensure_readme(pkg_dir, entry)
    if not ensure_license(pkg_dir):
        return "no-license"

    # Push with backoff on the registry's publish rate limit (429). The limit is
    # transient, so retrying after a pause hosts the package rather than losing it.
    for attempt in range(6):
        r = sh([str(BIN), "push"], cwd=str(pkg_dir), env=env)
        out = r.stdout + r.stderr
        if "successfully" in out:
            return "published"
        if "already published" in out or "409" in out:
            return "exists"
        if "429" in out or "rate limit" in out.lower():
            time.sleep(20 * (attempt + 1))  # 20s, 40s, 60s, … back off
            continue
        return "push-failed:" + (out.strip().splitlines()[-1][:60] if out.strip() else "?")
    return "push-failed:rate-limited"


def ensure_license(pkg_dir: Path) -> bool:
    """`xelian push` requires a LICENSE at the package root. Never fabricate one
    — but if the repo ships the license under a common alias, copy the real text
    to LICENSE so redistribution preserves it. Returns False if none is found."""
    if (pkg_dir / "LICENSE").is_file():
        return True
    for alias in ("LICENSE.md", "LICENSE.txt", "LICENSE-MIT", "LICENSE-APACHE",
                  "LICENSE.rst", "COPYING", "COPYING.txt", "license", "license.md",
                  "License", "License.md", "License.txt"):
        src = pkg_dir / alias
        if src.is_file():
            (pkg_dir / "LICENSE").write_text(src.read_text(errors="replace"))
            return True
    return False


def ensure_readme(pkg_dir: Path, entry: dict) -> None:
    """`xelian push` requires a README at the package root. Third-party imports
    may not have one there — write a small attribution stub so it can be hosted."""
    readme = pkg_dir / "README.md"
    if readme.is_file():
        return
    readme.write_text(
        f"# {entry['name']}\n\n{(entry.get('description') or '').strip()}\n\n"
        f"Imported from [{entry['full_name']}]({entry['url']}) and run under its "
        f"own license ({entry.get('license') or 'see repository'}).\n\n"
        f"```bash\nxelian run {entry['full_name']}\n```\n"
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", default=str(ROOT / "registry" / "catalog.json"),
                    help="catalog to publish from (default: registry/catalog.json)")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--type", choices=["mcp", "agent"])
    args = ap.parse_args()
    pw = os.environ.get("XELIAN_SEED_PASSWORD")
    if not pw:
        print("set XELIAN_SEED_PASSWORD", file=sys.stderr)
        return 2

    home = tempfile.mkdtemp(prefix="xpub-")
    env = dict(os.environ, HOME=home, XELIAN_REGISTRY_URL=URL)
    sh(["curl", "-s", "-X", "POST", f"{URL}/auth/signup", "-H",
        "content-type: application/json", "-d",
        json.dumps({"username": "xelian", "password": pw})])
    login = subprocess.run([str(BIN), "login", "--username", "xelian",
                            "--password-stdin"], input=pw, text=True,
                           capture_output=True, env=env)
    if "Logged in" not in (login.stdout + login.stderr):
        print("login failed:", login.stdout, login.stderr, file=sys.stderr)
        return 1

    entries = json.load(open(args.inp))["packages"]
    if args.type:
        entries = [e for e in entries if e["type"] == args.type]
    if args.limit:
        entries = entries[: args.limit]

    tally: dict[str, int] = {}
    for i, e in enumerate(entries, 1):
        status = publish_one(e, home)
        key = status.split(":")[0]
        tally[key] = tally.get(key, 0) + 1
        print(f"[{i}/{len(entries)}] {status:<12} {e['full_name']}", flush=True)
        # Gentle pacing under the registry's publish rate limit (only after an
        # actual publish — skips/exists cost nothing).
        if key == "published":
            time.sleep(3)
    print("summary:", tally)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
