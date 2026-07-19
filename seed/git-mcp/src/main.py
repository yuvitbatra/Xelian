"""MCP server for git: status, log, and diff of any local repository."""
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
