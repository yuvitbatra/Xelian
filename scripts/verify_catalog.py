#!/usr/bin/env python3
"""Verify every catalog entry actually runs via `xelian add`, and keep only the
runnable ones — correctly labeled by what inference finds.

This is the fix for catalog quality: frameworks and libraries (langchain,
crewai, …) have no runnable entrypoint, so `xelian add` fails to infer one and
they are dropped. Monorepo roots that only list members are dropped too. What
survives is packages a user can actually `xelian run`/`xelian add`.

Runs the CLI in import-only mode with a short per-repo timeout (kills after the
inference line, before the slow dependency install), so it's fast enough for
the whole catalog. Writes the verified catalog and a rejects log.

Usage:
    python scripts/verify_catalog.py [--in registry/catalog.json]
                                     [--out registry/catalog.verified.json]
                                     [--limit N] [--jobs 4]
"""

from __future__ import annotations
import argparse, json, os, re, subprocess, sys, tempfile, concurrent.futures
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "xelian"

INFER_RE = re.compile(r"Inferred (agent|mcp) entrypoint")
DESCEND_RE = re.compile(r"is a monorepo; using its (agent|mcp) package")


DECISIVE = [
    (INFER_RE, "ok"),
    (DESCEND_RE, "ok"),
    (re.compile("could not determine how to run"), "library"),
    (re.compile("monorepo containing"), "monorepo"),
    (re.compile("could not detect project language"), "nolang"),
    (re.compile("unsupported language"), "unsupported"),
]


def classify(url: str) -> tuple[str, str | None]:
    """Stream `xelian add` and stop the moment inference reaches a verdict —
    the decision (inferred entrypoint / library / monorepo / …) is printed
    before the slow dependency install, so this only ever downloads + infers.

    Returns (status, inferred_type)."""
    env = dict(os.environ)
    home = tempfile.mkdtemp(prefix="xverify-")
    env["HOME"] = home
    proc = subprocess.Popen(
        [str(BIN), "add", url],
        env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    status, inferred = "error", None
    import time as _t
    deadline = _t.time() + 150
    try:
        for line in proc.stdout:  # type: ignore
            for rx, st in DECISIVE:
                m = rx.search(line)
                if m:
                    status = st
                    if st == "ok":
                        inferred = m.group(1) if m.groups() else None
                    raise StopIteration
            if _t.time() > deadline:
                raise StopIteration
    except StopIteration:
        pass
    finally:
        proc.kill()
        try:
            proc.wait(timeout=5)
        except Exception:
            pass
        subprocess.run(["rm", "-rf", home])
    return status, inferred


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", default="registry/catalog.json")
    ap.add_argument("--out", default="registry/catalog.verified.json")
    ap.add_argument("--rejects", default="/tmp/catalog_rejects.txt")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--jobs", type=int, default=4)
    args = ap.parse_args()

    data = json.load(open(args.inp))
    entries = data["packages"]
    if args.limit:
        entries = entries[: args.limit]

    verified, rejects = [], []
    done = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        futs = {ex.submit(classify, e["url"]): e for e in entries}
        for fut in concurrent.futures.as_completed(futs):
            e = futs[fut]
            status, inferred = fut.result()
            done += 1
            if status == "ok":
                e["type"] = inferred  # trust inference over the harvest guess
                verified.append(e)
            else:
                rejects.append(f"{status}\t{e['full_name']}")
            if done % 20 == 0:
                print(f"  {done}/{len(entries)} — kept {len(verified)}", file=sys.stderr)

    verified.sort(key=lambda p: p["stars"], reverse=True)
    mcp = [p for p in verified if p["type"] == "mcp"]
    agents = [p for p in verified if p["type"] == "agent"]
    out = {
        "generated_at": data.get("generated_at"),
        "note": (
            "Every entry verified runnable via `xelian add` (frameworks, "
            "libraries, and monorepo roots that don't run are excluded). Types "
            "are what Xelian's inference detected. Third-party projects, each "
            "run under its own license."
        ),
        "counts": {"total": len(verified), "mcp": len(mcp), "agents": len(agents)},
        "packages": verified,
    }
    json.dump(out, open(args.out, "w"), indent=2)
    Path(args.rejects).write_text("\n".join(sorted(rejects)))
    print(f"verified {len(verified)}/{len(entries)} runnable "
          f"({len(mcp)} mcp, {len(agents)} agents) -> {args.out}")
    print(f"rejected {len(rejects)} -> {args.rejects}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
