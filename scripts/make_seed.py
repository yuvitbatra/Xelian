#!/usr/bin/env python3
"""Generate the seed packages (Phase 24 content) under seed/.

Eight agents + eight MCP servers, all pure-Python stdlib — zero setup, no API
keys, everything genuinely works offline (fetch-mcp needs the network, as it
declares). Regenerate with:  python3 scripts/make_seed.py
"""

import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SEED = ROOT / "seed"

MANIFEST = """\
spec-version = 1
name = "{name}"
version = "0.1.0"
description = "{description}"
package-type = "{ptype}"
language = "python"
runtime = ">=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = {permissions}
features = {features}
tags = {tags}

[author]
name = "Xelian"
email = "yuvitbatra@gmail.com"

[dependencies]
manifest = "pyproject.toml"
"""

PYPROJECT = """\
[project]
name = "{name}"
version = "0.1.0"
requires-python = ">=3.11"
"""

LICENSE = "MIT License\n\nCopyright (c) 2026 Yuvit Batra\n"

# Shared safe arithmetic evaluator (agents and MCP servers embed it).
SAFE_EVAL = '''
import ast
import math
import operator as _op

_OPS = {
    ast.Add: _op.add, ast.Sub: _op.sub, ast.Mult: _op.mul,
    ast.Div: _op.truediv, ast.FloorDiv: _op.floordiv, ast.Mod: _op.mod,
    ast.Pow: _op.pow, ast.USub: _op.neg, ast.UAdd: _op.pos,
}
_FUNCS = {
    "sqrt": math.sqrt, "abs": abs, "round": round, "min": min, "max": max,
    "floor": math.floor, "ceil": math.ceil, "log": math.log, "log2": math.log2,
    "log10": math.log10, "sin": math.sin, "cos": math.cos, "tan": math.tan,
}
_NAMES = {"pi": math.pi, "e": math.e, "tau": math.tau}


def safe_eval(expr: str):
    """Evaluate an arithmetic expression without touching eval()."""
    def walk(node):
        if isinstance(node, ast.Expression):
            return walk(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
            return node.value
        if isinstance(node, ast.BinOp) and type(node.op) in _OPS:
            return _OPS[type(node.op)](walk(node.left), walk(node.right))
        if isinstance(node, ast.UnaryOp) and type(node.op) in _OPS:
            return _OPS[type(node.op)](walk(node.operand))
        if isinstance(node, ast.Name) and node.id in _NAMES:
            return _NAMES[node.id]
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) \\
                and node.func.id in _FUNCS and not node.keywords:
            return _FUNCS[node.func.id](*[walk(a) for a in node.args])
        if isinstance(node, ast.Tuple):
            return tuple(walk(e) for e in node.elts)
        raise ValueError(f"unsupported expression: {ast.dump(node)[:60]}")

    return walk(ast.parse(expr.strip(), mode="eval"))
'''

# Shared minimal MCP stdio server loop.
MCP_SERVE = '''
import json
import sys


def serve(server_name, tools):
    """Minimal MCP stdio server. tools: name -> (description, schema, handler)."""
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "id" not in msg:
            continue  # notification
        mid = msg["id"]
        method = msg.get("method")
        try:
            if method == "initialize":
                proto = msg.get("params", {}).get("protocolVersion", "2025-06-18")
                result = {
                    "protocolVersion": proto,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": server_name, "version": "0.1.0"},
                }
            elif method == "ping":
                result = {}
            elif method == "tools/list":
                result = {"tools": [
                    {"name": n, "description": d, "inputSchema": s}
                    for n, (d, s, _h) in tools.items()
                ]}
            elif method == "tools/call":
                params = msg.get("params", {})
                name = params.get("name")
                if name not in tools:
                    raise ValueError(f"unknown tool: {name}")
                text = tools[name][2](params.get("arguments") or {})
                result = {"content": [{"type": "text", "text": text}]}
            else:
                raise ValueError(f"unsupported method: {method}")
            reply = {"jsonrpc": "2.0", "id": mid, "result": result}
        except Exception as e:  # tool errors become JSON-RPC errors, never crashes
            reply = {"jsonrpc": "2.0", "id": mid,
                     "error": {"code": -32000, "message": str(e)}}
        print(json.dumps(reply), flush=True)
'''


def obj(*pairs):
    props = {k: {"type": t, **({"description": d} if d else {})} for k, t, d, _r in pairs}
    required = [k for k, _t, _d, r in pairs if r]
    schema = {"type": "object", "properties": props}
    if required:
        schema["required"] = required
    return json.dumps(schema)


import json  # noqa: E402  (used by obj at generation time)

AGENTS = {
    "unit-convert": {
        "description": "Convert units from the chat line: '10 km in miles', '72 f in c', '512 mb in gb'",
        "tags": ["utility", "conversion"],
        "main": '''
import sys

LENGTH = {"m": 1.0, "km": 1000.0, "cm": 0.01, "mm": 0.001, "mi": 1609.344,
          "miles": 1609.344, "mile": 1609.344, "ft": 0.3048, "feet": 0.3048,
          "in": 0.0254, "yd": 0.9144}
MASS = {"kg": 1.0, "g": 0.001, "mg": 1e-6, "lb": 0.45359237, "lbs": 0.45359237,
        "oz": 0.028349523125, "t": 1000.0}
DATA = {"b": 1.0, "kb": 1e3, "mb": 1e6, "gb": 1e9, "tb": 1e12,
        "kib": 1024.0, "mib": 1024.0**2, "gib": 1024.0**3, "tib": 1024.0**4}
TEMps = ("c", "f", "k", "celsius", "fahrenheit", "kelvin")


def to_c(v, u):
    u = u[0]
    return v if u == "c" else (v - 32) * 5 / 9 if u == "f" else v - 273.15


def from_c(v, u):
    u = u[0]
    return v if u == "c" else v * 9 / 5 + 32 if u == "f" else v + 273.15


def convert(value, src, dst):
    src, dst = src.lower(), dst.lower()
    if src in TEMps and dst in TEMps:
        return from_c(to_c(value, src), dst)
    for table in (LENGTH, MASS, DATA):
        if src in table and dst in table:
            return value * table[src] / table[dst]
    raise ValueError(f"cannot convert {src} -> {dst}")


for line in sys.stdin:
    parts = line.strip().replace(" to ", " in ").split()
    try:
        i = parts.index("in")
        value = float(parts[0])
        result = convert(value, parts[1], parts[i + 1])
        print(f"{value:g} {parts[1]} = {result:g} {parts[i + 1]}", flush=True)
    except (ValueError, IndexError) as e:
        print(f"usage: '<value> <unit> in <unit>' (length/mass/data/temperature) — {e}", flush=True)
''',
    },
    "password-gen": {
        "description": "Generate strong passwords and passphrases: '16', 'passphrase 4', 'pin 6'",
        "tags": ["utility", "security"],
        "main": '''
import secrets
import string
import sys

WORDS = ("anchor apple arrow autumn basil beacon breeze candle canyon cedar "
         "cloud comet copper coral crane crystal delta drift ember falcon "
         "fern flint forest garnet glacier grove harbor hazel indigo iris "
         "jasper juniper kestrel lagoon lantern lark linen lotus maple "
         "meadow mesa nectar nova oasis obsidian onyx opal orchard osprey "
         "pearl pebble pine plume prairie quartz quill raven reef ridge "
         "river rowan saffron sage sequoia shadow sierra slate sparrow "
         "spruce summit thistle timber topaz tulip tundra velvet violet "
         "walnut willow wren zephyr zinc").split()
ALPHABET = string.ascii_letters + string.digits + "!@#$%^&*-_=+"

for line in sys.stdin:
    parts = line.strip().lower().split()
    try:
        if not parts or parts[0].isdigit():
            n = int(parts[0]) if parts else 20
            n = max(8, min(n, 128))
            print("".join(secrets.choice(ALPHABET) for _ in range(n)), flush=True)
        elif parts[0] == "passphrase":
            n = max(3, min(int(parts[1]) if len(parts) > 1 else 4, 12))
            print("-".join(secrets.choice(WORDS) for _ in range(n)), flush=True)
        elif parts[0] == "pin":
            n = max(4, min(int(parts[1]) if len(parts) > 1 else 6, 12))
            print("".join(secrets.choice(string.digits) for _ in range(n)), flush=True)
        else:
            print("usage: '<length>' | 'passphrase <words>' | 'pin <digits>'", flush=True)
    except ValueError:
        print("usage: '<length>' | 'passphrase <words>' | 'pin <digits>'", flush=True)
''',
    },
    "hashkit": {
        "description": "Hash and encode text: 'sha256 hello', 'md5 x', 'b64 secret', 'unb64 c2VjcmV0'",
        "tags": ["utility", "crypto"],
        "main": '''
import base64
import hashlib
import sys

for line in sys.stdin:
    cmd, _, rest = line.strip().partition(" ")
    cmd = cmd.lower()
    try:
        if cmd in ("sha256", "sha1", "sha512", "md5"):
            print(getattr(hashlib, cmd)(rest.encode()).hexdigest(), flush=True)
        elif cmd == "b64":
            print(base64.b64encode(rest.encode()).decode(), flush=True)
        elif cmd == "unb64":
            print(base64.b64decode(rest.encode()).decode("utf-8", "replace"), flush=True)
        elif cmd == "hex":
            print(rest.encode().hex(), flush=True)
        elif cmd == "unhex":
            print(bytes.fromhex(rest).decode("utf-8", "replace"), flush=True)
        else:
            print("usage: sha256|sha1|sha512|md5|b64|unb64|hex|unhex <text>", flush=True)
    except Exception as e:
        print(f"error: {e}", flush=True)
''',
    },
    "timekit": {
        "description": "Time conversions: 'now', '1700000000', '2026-07-19T10:00:00Z', 'now local'",
        "tags": ["utility", "time"],
        "main": '''
import sys
from datetime import datetime, timezone

for line in sys.stdin:
    q = line.strip()
    try:
        if q in ("now", "", "now utc"):
            now = datetime.now(timezone.utc)
            print(f"{now.isoformat()}  (epoch {int(now.timestamp())})", flush=True)
        elif q == "now local":
            now = datetime.now().astimezone()
            print(f"{now.isoformat()}  (epoch {int(now.timestamp())})", flush=True)
        elif q.replace(".", "", 1).isdigit():
            ts = float(q)
            if ts > 1e12:
                ts /= 1000  # milliseconds
            dt = datetime.fromtimestamp(ts, tz=timezone.utc)
            print(dt.isoformat(), flush=True)
        else:
            dt = datetime.fromisoformat(q.replace("Z", "+00:00"))
            print(f"epoch {int(dt.timestamp())}", flush=True)
    except Exception as e:
        print(f"usage: 'now' | 'now local' | '<epoch>' | '<ISO-8601>' — {e}", flush=True)
''',
    },
    "idgen": {
        "description": "Generate identifiers: 'uuid', 'uuid 3', 'hex 32', 'slug'",
        "tags": ["utility"],
        "main": '''
import secrets
import sys
import uuid

ADJ = "brisk calm deft eager fleet keen lucid noble prime quiet swift vivid".split()
NOUN = "atlas comet delta ember flare grove lumen orbit quartz ridge spark vertex".split()

for line in sys.stdin:
    parts = line.strip().lower().split()
    try:
        kind = parts[0] if parts else "uuid"
        n = int(parts[1]) if len(parts) > 1 else 1
        if kind == "uuid":
            print(" ".join(str(uuid.uuid4()) for _ in range(max(1, min(n, 20)))), flush=True)
        elif kind == "hex":
            print(secrets.token_hex(max(4, min(n, 64))), flush=True)
        elif kind == "slug":
            print(f"{secrets.choice(ADJ)}-{secrets.choice(NOUN)}-{secrets.token_hex(2)}", flush=True)
        else:
            print("usage: 'uuid [n]' | 'hex [bytes]' | 'slug'", flush=True)
    except ValueError:
        print("usage: 'uuid [n]' | 'hex [bytes]' | 'slug'", flush=True)
''',
    },
    "calc": {
        "description": "A calculator with functions: '2*(3+4)**2', 'sqrt(2)*pi', 'log2(4096)'",
        "tags": ["utility", "math"],
        "main": SAFE_EVAL + '''
import sys

for line in sys.stdin:
    expr = line.strip()
    if not expr:
        continue
    try:
        result = safe_eval(expr)
        print(f"{result:g}" if isinstance(result, float) else str(result), flush=True)
    except Exception as e:
        print(f"error: {e}", flush=True)
''',
    },
    "text-stats": {
        "description": "Paste a line of text, get characters, words, unique words, and reading time",
        "tags": ["utility", "text"],
        "main": '''
import re
import sys

for line in sys.stdin:
    text = line.rstrip("\\n")
    words = re.findall(r"[\\w'-]+", text)
    sentences = [s for s in re.split(r"[.!?]+", text) if s.strip()]
    seconds = round(len(words) / 3.5, 1)  # ~210 wpm
    print(
        f"chars={len(text)} words={len(words)} unique={len({w.lower() for w in words})} "
        f"sentences={len(sentences)} reading~{seconds}s",
        flush=True,
    )
''',
    },
    "regex-lab": {
        "description": "Test regexes interactively: '<pattern> ::: <text>' prints all matches",
        "tags": ["utility", "regex", "developer"],
        "main": '''
import re
import sys

for line in sys.stdin:
    raw = line.rstrip("\\n")
    if ":::" not in raw:
        print("usage: <pattern> ::: <text>", flush=True)
        continue
    pattern, _, text = raw.partition(":::")
    try:
        matches = [m.group(0) for m in re.finditer(pattern.strip(), text.strip())]
        if matches:
            print(f"{len(matches)} match(es): " + " | ".join(matches[:20]), flush=True)
        else:
            print("no matches", flush=True)
    except re.error as e:
        print(f"invalid pattern: {e}", flush=True)
''',
    },
}

MCPS = {
    "time-mcp": {
        "description": "MCP server for time: current time, epoch and ISO-8601 conversion",
        "tags": ["mcp", "time"],
        "permissions": [],
        "main": MCP_SERVE + '''
from datetime import datetime, timezone


def now(args):
    tz = (args.get("timezone") or "utc").lower()
    dt = datetime.now(timezone.utc) if tz == "utc" else datetime.now().astimezone()
    return f"{dt.isoformat()} (epoch {int(dt.timestamp())})"


def epoch_to_iso(args):
    ts = float(args["epoch"])
    if ts > 1e12:
        ts /= 1000
    return datetime.fromtimestamp(ts, tz=timezone.utc).isoformat()


def iso_to_epoch(args):
    dt = datetime.fromisoformat(str(args["iso"]).replace("Z", "+00:00"))
    return str(int(dt.timestamp()))


serve("time-mcp", {
    "now": ("Current time (timezone: 'utc' or 'local')",
            {"type": "object", "properties": {"timezone": {"type": "string"}}}, now),
    "epoch_to_iso": ("Convert a unix epoch (s or ms) to ISO-8601 UTC",
                     {"type": "object", "properties": {"epoch": {"type": "number"}},
                      "required": ["epoch"]}, epoch_to_iso),
    "iso_to_epoch": ("Convert an ISO-8601 timestamp to unix epoch seconds",
                     {"type": "object", "properties": {"iso": {"type": "string"}},
                      "required": ["iso"]}, iso_to_epoch),
})
''',
    },
    "calc-mcp": {
        "description": "MCP server for math: safe expression evaluation and list statistics",
        "tags": ["mcp", "math"],
        "permissions": [],
        "main": MCP_SERVE + SAFE_EVAL + '''
import statistics


def evaluate(args):
    result = safe_eval(str(args["expression"]))
    return f"{result:g}" if isinstance(result, float) else str(result)


def stats(args):
    nums = [float(x) for x in args["numbers"]]
    if not nums:
        raise ValueError("numbers must be non-empty")
    parts = [f"n={len(nums)}", f"mean={statistics.fmean(nums):g}",
             f"median={statistics.median(nums):g}",
             f"min={min(nums):g}", f"max={max(nums):g}"]
    if len(nums) > 1:
        parts.append(f"stdev={statistics.stdev(nums):g}")
    return " ".join(parts)


serve("calc-mcp", {
    "eval": ("Evaluate an arithmetic expression (sqrt, log, trig, pi, e supported)",
             {"type": "object", "properties": {"expression": {"type": "string"}},
              "required": ["expression"]}, evaluate),
    "stats": ("Mean/median/min/max/stdev of a list of numbers",
              {"type": "object", "properties": {"numbers": {"type": "array",
               "items": {"type": "number"}}}, "required": ["numbers"]}, stats),
})
''',
    },
    "fetch-mcp": {
        "description": "MCP server for HTTP: fetch a URL and return status, headers, and body text",
        "tags": ["mcp", "http", "web"],
        "permissions": ["network"],
        "main": MCP_SERVE + '''
import urllib.request


def http_get(args):
    url = str(args["url"])
    if not url.startswith(("http://", "https://")):
        raise ValueError("only http(s) URLs are supported")
    max_bytes = min(int(args.get("max_bytes") or 65536), 1024 * 1024)
    req = urllib.request.Request(url, headers={"User-Agent": "xelian-fetch-mcp/0.1"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        body = resp.read(max_bytes)
        ctype = resp.headers.get("Content-Type", "")
        text = body.decode("utf-8", "replace")
        return f"HTTP {resp.status} {ctype}\\n\\n{text}"


serve("fetch-mcp", {
    "http_get": ("GET a URL (15s timeout) and return status + body text (capped)",
                 {"type": "object", "properties": {
                     "url": {"type": "string"},
                     "max_bytes": {"type": "number"}},
                  "required": ["url"]}, http_get),
})
''',
    },
    "files-mcp": {
        "description": "MCP server for read-only file access: list directories, read files, stat paths",
        "tags": ["mcp", "filesystem"],
        "permissions": ["filesystem"],
        "main": MCP_SERVE + '''
import os
from pathlib import Path


def _p(args):
    return Path(os.path.expanduser(str(args["path"]))).resolve()


def list_dir(args):
    path = _p(args)
    rows = []
    for entry in sorted(path.iterdir()):
        kind = "dir " if entry.is_dir() else "file"
        size = entry.stat().st_size if entry.is_file() else ""
        rows.append(f"{kind}  {size:>10}  {entry.name}")
    return "\\n".join(rows) or "(empty)"


def read_file(args):
    path = _p(args)
    max_bytes = min(int(args.get("max_bytes") or 65536), 1024 * 1024)
    data = path.read_bytes()[:max_bytes]
    return data.decode("utf-8", "replace")


def file_info(args):
    path = _p(args)
    st = path.stat()
    kind = "directory" if path.is_dir() else "file"
    return f"{path} — {kind}, {st.st_size} bytes, mtime {int(st.st_mtime)}"


serve("files-mcp", {
    "list_dir": ("List a directory",
                 {"type": "object", "properties": {"path": {"type": "string"}},
                  "required": ["path"]}, list_dir),
    "read_file": ("Read a file as text (capped)",
                  {"type": "object", "properties": {
                      "path": {"type": "string"}, "max_bytes": {"type": "number"}},
                   "required": ["path"]}, read_file),
    "file_info": ("Size/type/mtime for a path",
                  {"type": "object", "properties": {"path": {"type": "string"}},
                   "required": ["path"]}, file_info),
})
''',
    },
    "sqlite-mcp": {
        "description": "MCP server for SQLite: run read-only SQL against any local database file",
        "tags": ["mcp", "sqlite", "database"],
        "permissions": ["filesystem"],
        "main": MCP_SERVE + '''
import json as _json
import os
import sqlite3


def query(args):
    db_path = os.path.expanduser(str(args["db_path"]))
    if not os.path.isfile(db_path):
        raise ValueError(f"no database at {db_path}")
    uri = f"file:{db_path}?mode=ro"
    with sqlite3.connect(uri, uri=True, timeout=5) as conn:
        conn.row_factory = sqlite3.Row
        cur = conn.execute(str(args["sql"]))
        rows = [dict(r) for r in cur.fetchmany(200)]
    return _json.dumps(rows, indent=2, default=str)


def tables(args):
    db_path = os.path.expanduser(str(args["db_path"]))
    uri = f"file:{db_path}?mode=ro"
    with sqlite3.connect(uri, uri=True, timeout=5) as conn:
        cur = conn.execute(
            "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') ORDER BY name")
        return "\\n".join(f"{t}  {n}" for n, t in cur.fetchall()) or "(no tables)"


serve("sqlite-mcp", {
    "query": ("Run read-only SQL (first 200 rows as JSON)",
              {"type": "object", "properties": {
                  "db_path": {"type": "string"}, "sql": {"type": "string"}},
               "required": ["db_path", "sql"]}, query),
    "tables": ("List tables and views in a database",
               {"type": "object", "properties": {"db_path": {"type": "string"}},
                "required": ["db_path"]}, tables),
})
''',
    },
    "git-mcp": {
        "description": "MCP server for git: status, log, and diff of any local repository",
        "tags": ["mcp", "git", "developer"],
        "permissions": ["filesystem"],
        "main": MCP_SERVE + '''
import os
import subprocess


def _git(repo, *argv):
    repo = os.path.expanduser(str(repo))
    out = subprocess.run(
        ["git", "-C", repo, *argv], capture_output=True, text=True, timeout=20
    )
    if out.returncode != 0:
        raise ValueError(out.stderr.strip() or f"git exited {out.returncode}")
    return out.stdout.strip() or "(clean)"


def status(args):
    return _git(args["repo"], "status", "--short", "--branch")


def log(args):
    n = str(min(int(args.get("n") or 10), 100))
    return _git(args["repo"], "log", "--oneline", "-n", n)


def diff(args):
    return _git(args["repo"], "diff", "--stat") or "(no changes)"


serve("git-mcp", {
    "status": ("git status --short --branch",
               {"type": "object", "properties": {"repo": {"type": "string"}},
                "required": ["repo"]}, status),
    "log": ("git log --oneline (last n commits)",
            {"type": "object", "properties": {
                "repo": {"type": "string"}, "n": {"type": "number"}},
             "required": ["repo"]}, log),
    "diff": ("git diff --stat of the working tree",
             {"type": "object", "properties": {"repo": {"type": "string"}},
              "required": ["repo"]}, diff),
})
''',
    },
    "sysinfo-mcp": {
        "description": "MCP server for system info: OS, CPU, Python, and disk usage",
        "tags": ["mcp", "system"],
        "permissions": [],
        "main": MCP_SERVE + '''
import os
import platform
import shutil


def overview(args):
    return "\\n".join([
        f"os: {platform.system()} {platform.release()} ({platform.machine()})",
        f"python: {platform.python_version()}",
        f"cpus: {os.cpu_count()}",
        f"hostname: {platform.node()}",
    ])


def disk(args):
    path = os.path.expanduser(str(args.get("path") or "/"))
    usage = shutil.disk_usage(path)
    gb = 1024 ** 3
    return (f"{path}: total {usage.total / gb:.1f} GiB, "
            f"used {usage.used / gb:.1f} GiB, free {usage.free / gb:.1f} GiB")


serve("sysinfo-mcp", {
    "overview": ("OS, Python, CPU count, hostname",
                 {"type": "object", "properties": {}}, overview),
    "disk": ("Disk usage for a path (default /)",
             {"type": "object", "properties": {"path": {"type": "string"}}}, disk),
})
''',
    },
    "text-mcp": {
        "description": "MCP server for text: regex search/replace and unified diffs",
        "tags": ["mcp", "text", "developer"],
        "permissions": [],
        "main": MCP_SERVE + '''
import difflib
import re


def regex_search(args):
    matches = [m.group(0) for m in re.finditer(str(args["pattern"]), str(args["text"]))]
    return "\\n".join(matches[:100]) or "(no matches)"


def regex_replace(args):
    result, n = re.subn(str(args["pattern"]), str(args["replacement"]), str(args["text"]))
    return f"({n} replacement(s))\\n{result}"


def unified_diff(args):
    diff = difflib.unified_diff(
        str(args["a"]).splitlines(), str(args["b"]).splitlines(),
        lineterm="", fromfile="a", tofile="b",
    )
    return "\\n".join(diff) or "(identical)"


serve("text-mcp", {
    "regex_search": ("All regex matches in a text",
                     {"type": "object", "properties": {
                         "pattern": {"type": "string"}, "text": {"type": "string"}},
                      "required": ["pattern", "text"]}, regex_search),
    "regex_replace": ("Regex substitution over a text",
                      {"type": "object", "properties": {
                          "pattern": {"type": "string"},
                          "replacement": {"type": "string"},
                          "text": {"type": "string"}},
                       "required": ["pattern", "replacement", "text"]}, regex_replace),
    "unified_diff": ("Unified diff between two texts",
                     {"type": "object", "properties": {
                         "a": {"type": "string"}, "b": {"type": "string"}},
                      "required": ["a", "b"]}, unified_diff),
})
''',
    },
}


def readme_for(name, spec, ptype):
    if ptype == "agent":
        usage = f"```bash\nxelian run xelian/{name}\n```\n"
        extra = "Then type into the REPL — one line in, one line out.\n"
    else:
        usage = (
            f"```bash\nxelian run xelian/{name}      # stdio MCP server\n"
            f"xelian gateway add xelian/{name}      # or serve it via the gateway\n```\n"
        )
        extra = "Point any MCP client at it, or use the Xelian gateway for a single endpoint.\n"
    return (
        f"# {name}\n\n{spec['description']}.\n\n"
        f"Pure Python standard library — no dependencies, no API keys.\n\n"
        f"## Usage\n\n{usage}\n{extra}"
    )


def write_package(name, spec, ptype):
    pkg = SEED / name
    (pkg / "src").mkdir(parents=True, exist_ok=True)
    manifest = MANIFEST.format(
        name=name,
        description=spec["description"],
        ptype=ptype,
        permissions=json.dumps(spec.get("permissions", [])),
        features=json.dumps(["tools"] if ptype == "mcp" else []),
        tags=json.dumps(spec["tags"]),
    )
    (pkg / "xelian.toml").write_text(manifest)
    (pkg / "src" / "main.py").write_text(
        f'"""{spec["description"]}."""\n' + textwrap.dedent(spec["main"]).lstrip()
    )
    (pkg / "pyproject.toml").write_text(PYPROJECT.format(name=name))
    (pkg / "README.md").write_text(readme_for(name, spec, ptype))
    (pkg / "LICENSE").write_text(LICENSE)


def main():
    for name, spec in AGENTS.items():
        write_package(name, spec, "agent")
    for name, spec in MCPS.items():
        write_package(name, spec, "mcp")
    print(f"wrote {len(AGENTS)} agents + {len(MCPS)} MCP servers under {SEED}")


if __name__ == "__main__":
    main()
