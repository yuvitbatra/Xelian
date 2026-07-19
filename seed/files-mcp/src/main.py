"""MCP server for read-only file access: list directories, read files, stat paths."""
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
    return "\n".join(rows) or "(empty)"


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
