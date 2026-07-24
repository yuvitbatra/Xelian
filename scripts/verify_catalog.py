#!/usr/bin/env python3
"""Verify every catalog entry actually RUNS via `xelian run`, and keep only the
ones that genuinely work — correctly labeled by what inference finds.

This is the fix for catalog quality. The previous version of this script only
checked that an *entrypoint could be inferred* — it killed the CLI right after
the inference line, before dependencies were even installed. That let three
classes of broken packages leak into the "verified" catalog:

  1. build/install failures      (deps don't resolve — e.g. RD-Agent)
  2. nonsense entrypoints         (install.sh inferred as an "agent")
  3. framework/research code      (imports fine, crashes at launch)

So the catalog claimed "verified runnable" while many entries failed the very
first `xelian run owner/name` a user tries. This version holds every entry to a
real bar — it must BUILD and then START CLEANLY:

  Phase A — build:   `xelian run owner/name --prepare` must exit 0 (downloads,
                     installs all dependencies, runs the build). Reveals the
                     inference verdict and the package TYPE (mcp / agent).
  Phase B — launch:  `xelian run owner/name` is actually launched:
                       • mcp   → we speak the MCP `initialize` handshake over
                                 stdio and require a JSON-RPC reply.
                       • agent → the REPL must boot and stay alive (or exit 0),
                                 not crash on startup.

An entry is kept only if BOTH phases pass. Servers that merely need an API key
to do real work still pass — they start and answer the handshake — which is the
realistic "it works" bar. Frameworks, libraries, monorepo roots, build
failures, nonsense entrypoints, and startup crashes are all dropped.

A single shared HOME is reused across every entry so the language runtimes
(uv / node) are downloaded once, not 370 times.

Usage:
    python scripts/verify_catalog.py [--in registry/catalog.json]
                                     [--out registry/catalog.verified.json]
                                     [--rejects /tmp/catalog_rejects.txt]
                                     [--home <dir>] [--registry <url>]
                                     [--limit N] [--jobs 3]
                                     [--build-timeout 360] [--launch-timeout 25]
"""

from __future__ import annotations
import argparse
import json
import os
import re
import select
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "xelian"

# Inference verdicts printed during Phase A, before the (slow) dependency
# install. A negative verdict lets us reject an entry cheaply without building.
INFER_RE = re.compile(r"Inferred (agent|mcp) entrypoint")
DESCEND_RE = re.compile(r"is a monorepo; using its (agent|mcp) package")
INSTALLED_RE = re.compile(r"^XELIAN_INSTALLED\|[^|]*\|[^|]*\|(agent|mcp)\|", re.M)

NEGATIVE_VERDICTS = [
    (re.compile("could not determine how to run"), "library"),
    (re.compile("monorepo containing"), "monorepo"),
    (re.compile("could not detect project language"), "nolang"),
    (re.compile("unsupported language"), "unsupported"),
]

# A well-formed MCP client `initialize` request (newline-delimited JSON, the
# MCP stdio transport). A server that is up answers with a JSON-RPC reply.
INITIALIZE = (
    json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "xelian-verify", "version": "0.1"},
            },
        }
    )
    + "\n"
)


def _child_env(home: str, registry: str) -> dict:
    env = dict(os.environ)
    env["HOME"] = home
    env["XELIAN_REGISTRY_URL"] = registry
    # Keep child agents/servers from opening a pager or expecting a TTY.
    env["PAGER"] = "cat"
    env["CI"] = "1"

    # Authenticate git for the child WITHOUT touching any credential store:
    # inject config purely through the process environment (GIT_CONFIG_*), which
    # git reads but never persists — no keychain, no ~/.gitconfig write. This
    # gives authenticated `git ls-remote` (higher rate limits, reliable at high
    # concurrency) while keeping the token ephemeral to this process tree.
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        pairs = [
            ("credential.helper", ""),  # disable osxkeychain et al.
            (f"url.https://x-access-token:{token}@github.com/.insteadOf",
             "https://github.com/"),
        ]
        env["GIT_CONFIG_COUNT"] = str(len(pairs))
        for i, (k, v) in enumerate(pairs):
            env[f"GIT_CONFIG_KEY_{i}"] = k
            env[f"GIT_CONFIG_VALUE_{i}"] = v
        env["GIT_TERMINAL_PROMPT"] = "0"
    return env


def phase_a_build(name: str, env: dict, timeout: float) -> tuple[str, str | None]:
    """Run `xelian run <name> --prepare`: download, install deps, build.

    Returns (status, pkg_type). status is one of:
      "ok"                       -> built; pkg_type is "mcp" or "agent"
      "library"/"monorepo"/...   -> inference gave a negative verdict
      "build"                    -> inference ok but build/install failed
      "timeout"                  -> did not finish in `timeout` seconds
    """
    proc = subprocess.Popen(
        [str(BIN), "run", name, "--prepare"],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    out_lines: list[str] = []
    negative: str | None = None
    inferred_type: str | None = None
    deadline = time.time() + timeout
    try:
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                proc.kill()
                return "timeout", None
            ready, _, _ = select.select([proc.stdout], [], [], min(remaining, 5.0))
            if ready:
                line = proc.stdout.readline()  # type: ignore[union-attr]
                if line == "":
                    break  # EOF: process finished
                out_lines.append(line)
                if negative is None:
                    for rx, st in NEGATIVE_VERDICTS:
                        if rx.search(line):
                            negative = st
                            proc.kill()
                            return negative, None
                m = INFER_RE.search(line) or DESCEND_RE.search(line)
                if m and inferred_type is None:
                    inferred_type = m.group(1)
            elif proc.poll() is not None:
                # drain any remaining buffered output
                rest = proc.stdout.read()  # type: ignore[union-attr]
                if rest:
                    out_lines.extend(rest.splitlines(keepends=True))
                break
    finally:
        try:
            proc.wait(timeout=10)
        except Exception:
            proc.kill()

    text = "".join(out_lines)
    m = INSTALLED_RE.search(text)
    if proc.returncode == 0 and m:
        return "ok", m.group(1)
    # inference may still tell us the type even if the descriptor is absent
    if negative:
        return negative, None
    return "build", inferred_type


def phase_b_launch(name: str, pkg_type: str, env: dict, timeout: float) -> str:
    """Actually launch `xelian run <name>` (now fully cached) and confirm it
    starts cleanly. Returns "ok", "launch-crash", or "no-handshake"."""
    proc = subprocess.Popen(
        [str(BIN), "run", name],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        if pkg_type == "mcp":
            return _smoke_mcp(proc, timeout)
        return _smoke_agent(proc, timeout)
    finally:
        _terminate(proc)


def _smoke_mcp(proc: subprocess.Popen, timeout: float) -> str:
    """Speak the MCP `initialize` handshake; a JSON-RPC reply on stdout means
    the server is up and speaking the protocol."""
    try:
        proc.stdin.write(INITIALIZE)  # type: ignore[union-attr]
        proc.stdin.flush()  # type: ignore[union-attr]
    except Exception:
        # stdin closed already -> the process died before it could read.
        return "launch-crash"

    deadline = time.time() + timeout
    while True:
        remaining = deadline - time.time()
        if remaining <= 0:
            return "no-handshake"
        ready, _, _ = select.select([proc.stdout], [], [], min(remaining, 2.0))
        if ready:
            line = proc.stdout.readline()  # type: ignore[union-attr]
            if line == "":
                # stdout closed: if it also exited nonzero it crashed.
                return "launch-crash" if (proc.poll() or 0) != 0 else "no-handshake"
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except Exception:
                continue  # non-JSON log noise on stdout; keep looking
            if isinstance(msg, dict) and msg.get("jsonrpc") == "2.0" and (
                "result" in msg or "error" in msg or "method" in msg
            ):
                return "ok"
        elif proc.poll() is not None:
            return "launch-crash" if proc.returncode != 0 else "no-handshake"


def _smoke_agent(proc: subprocess.Popen, timeout: float) -> str:
    """An agent must boot to its REPL and wait for input (stay alive), or exit
    cleanly. A fast nonzero exit is a startup crash."""
    try:
        proc.stdin.close()  # type: ignore[union-attr]
    except Exception:
        pass
    try:
        rc = proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        return "ok"  # still alive after the window -> REPL awaiting input
    return "ok" if rc == 0 else "launch-crash"


def _terminate(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()
        try:
            proc.wait(timeout=5)
        except Exception:
            pass


# Words that mark a repo as a reading/list/learning resource rather than a
# runnable package — even if some script inside it happens to start. Keeping
# these out preserves the catalog's promise: every entry is an actual agent or
# MCP server you run, not an "awesome list" or tutorial.
_NON_PACKAGE_MARKERS = (
    "awesome", "tutorial", "tutorials", "guide", "roadmap", "cheatsheet",
    "cheat-sheet", "handbook", "curated list", "course", "curriculum",
    "interview", "bootcamp", "study-notes", "learning-path", "book",
    "examples collection", "collection of", "list of", "resources",
)


def looks_unrunnable_repo(entry: dict) -> bool:
    """True for list/tutorial/learning repos that aren't real packages."""
    name = entry.get("name", "").lower().replace("_", "-")
    text = (name + " " + (entry.get("description") or "").lower()
            + " " + " ".join(entry.get("topics", [])).lower())
    # Whole-word-ish name signals (awesome-*, *-tutorial, JavaGuide-style).
    for w in ("awesome", "tutorial", "roadmap", "cheatsheet", "handbook",
              "curriculum", "interview", "bootcamp"):
        if w in name:
            return True
    return any(m in text for m in _NON_PACKAGE_MARKERS)


def classify(entry: dict, home: str, registry: str,
             build_timeout: float, launch_timeout: float) -> tuple[str, str | None]:
    """Full two-phase verification for one entry. Returns (status, pkg_type).
    status == "ok" only if it built AND started cleanly.

    Drive by the GitHub URL, not the catalog name: `xelian run <url>` imports
    and builds directly from source, so a candidate does NOT need to already be
    in the deployed registry catalog (using `owner/name` would 404 for anything
    not yet published to the live index)."""
    name = entry["url"]
    env = _child_env(home, registry)
    status, pkg_type = phase_a_build(name, env, build_timeout)
    if status != "ok" or pkg_type is None:
        return status, None
    launch = phase_b_launch(name, pkg_type, env, launch_timeout)
    if launch == "ok":
        return "ok", pkg_type
    return launch, pkg_type


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", default="registry/catalog.json")
    ap.add_argument("--out", default="registry/catalog.verified.json")
    ap.add_argument("--rejects", default="/tmp/catalog_rejects.txt")
    ap.add_argument("--home", default="", help="shared XELIAN HOME (default: a temp dir)")
    ap.add_argument("--registry", default=os.environ.get(
        "XELIAN_REGISTRY_URL", "https://xelian-registry.onrender.com"))
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--build-timeout", type=float, default=360)
    ap.add_argument("--launch-timeout", type=float, default=25)
    ap.add_argument("--progress", default="", help="write incremental JSONL results here")
    ap.add_argument("--already", default="", help="a prior verified catalog whose "
                    "survivors are trusted (kept, not re-checked) and seed the total")
    ap.add_argument("--skip", default="", help="a rejects file whose full_names are "
                    "known failures and are skipped")
    ap.add_argument("--target", type=int, default=0, help="stop verifying new entries "
                    "once the total kept (already + new) reaches this count")
    args = ap.parse_args()

    if not BIN.is_file():
        print(f"error: CLI binary not found at {BIN} — run `cargo build --release` first",
              file=sys.stderr)
        return 2

    data = json.load(open(args.inp))
    entries = data["packages"]

    # Seed already-verified survivors (trusted, not re-checked) and known
    # failures, so an incremental run only spends compute on NEW candidates.
    verified: list[dict] = []
    seen: set[str] = set()
    if args.already:
        for p in json.load(open(args.already)).get("packages", []):
            verified.append(p)
            seen.add(p["full_name"].lower())
    if args.skip and Path(args.skip).is_file():
        for line in Path(args.skip).read_text().splitlines():
            if line.startswith("#") or "\t" not in line:
                continue
            seen.add(line.split("\t", 1)[1].strip().lower())

    # New candidates: unseen, best-first (harvester already sorts by stars).
    entries = [e for e in entries if e["full_name"].lower() not in seen]
    # Drop list/tutorial/learning repos up front — they aren't runnable packages
    # and would only waste build time before being rejected anyway.
    rejects: list[str] = []
    kept_entries = []
    for e in entries:
        if looks_unrunnable_repo(e):
            rejects.append(f"not-a-package\t{e['full_name']}")
        else:
            kept_entries.append(e)
    entries = kept_entries
    entries.sort(key=lambda p: p.get("stars", 0), reverse=True)
    if args.limit:
        entries = entries[: args.limit]

    home = args.home or tempfile.mkdtemp(prefix="xverify-home-")
    Path(home).mkdir(parents=True, exist_ok=True)
    print(f"seeded {len(verified)} already-verified; pre-filtered "
          f"{sum(1 for r in rejects if r.startswith('not-a-package'))} non-packages; "
          f"verifying up to {len(entries)} new "
          f"(jobs={args.jobs}, target={args.target or '-'}, home={home})", file=sys.stderr)

    lock = threading.Lock()
    counter = {"done": 0}
    stop = threading.Event()
    progress_fh = open(args.progress, "a") if args.progress else None

    def work(entry: dict) -> None:
        if stop.is_set():
            return
        status, pkg_type = classify(
            entry, home, args.registry, args.build_timeout, args.launch_timeout)
        with lock:
            counter["done"] += 1
            done = counter["done"]
            if status == "ok":
                entry = dict(entry)
                entry["type"] = pkg_type  # trust inference over the harvest guess
                verified.append(entry)
                if args.target and len(verified) >= args.target:
                    stop.set()
            else:
                rejects.append(f"{status}\t{entry['full_name']}")
            if progress_fh:
                progress_fh.write(json.dumps(
                    {"name": entry["full_name"], "status": status, "type": pkg_type}) + "\n")
                progress_fh.flush()
            if done % 10 == 0:
                print(f"  new {done}/{len(entries)} — total kept {len(verified)}",
                      file=sys.stderr)

    # A worker pool. HOME is shared so runtimes are cached once; per-package
    # and per-env caches are keyed by owner/repo/sha, so concurrent entries do
    # not collide. Warm the first entry serially so the initial runtime
    # download isn't raced by `jobs` workers at once.
    import concurrent.futures
    if entries and not stop.is_set():
        work(entries[0])
    # Bounded submission: keep at most `jobs` in flight and check `stop` between
    # each, so `--target` actually halts work instead of queueing everything.
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        pending = entries[1:]
        i = 0
        inflight: set = set()
        while (i < len(pending) or inflight) and not stop.is_set():
            while len(inflight) < args.jobs and i < len(pending) and not stop.is_set():
                inflight.add(ex.submit(work, pending[i]))
                i += 1
            if not inflight:
                break
            done_set, inflight = concurrent.futures.wait(
                inflight, return_when=concurrent.futures.FIRST_COMPLETED)
            for f in done_set:
                f.result()

    if progress_fh:
        progress_fh.close()

    verified.sort(key=lambda p: p.get("stars", 0), reverse=True)
    mcp = [p for p in verified if p["type"] == "mcp"]
    agents = [p for p in verified if p["type"] == "agent"]
    out = {
        "generated_at": data.get("generated_at"),
        "note": (
            "Every entry verified to actually run: `xelian run owner/name` "
            "builds it and it starts cleanly (MCP servers answer the "
            "`initialize` handshake; agents boot their REPL). Frameworks, "
            "libraries, monorepo roots, build failures, and startup crashes "
            "are excluded. Types are what Xelian's inference detected. "
            "Third-party projects, each run under its own license."
        ),
        "counts": {"total": len(verified), "mcp": len(mcp), "agents": len(agents)},
        "packages": verified,
    }
    json.dump(out, open(args.out, "w"), indent=2)

    # A reason breakdown makes it obvious *why* things were dropped.
    from collections import Counter
    reasons = Counter(r.split("\t", 1)[0] for r in rejects)
    Path(args.rejects).write_text(
        "# reject reasons: " + ", ".join(f"{k}={v}" for k, v in reasons.most_common())
        + "\n" + "\n".join(sorted(rejects)) + "\n")

    print(f"\nverified {len(verified)}/{len(entries)} runnable "
          f"({len(mcp)} mcp, {len(agents)} agents) -> {args.out}")
    print(f"rejected {len(rejects)} -> {args.rejects}")
    print("  " + ", ".join(f"{k}={v}" for k, v in reasons.most_common()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
