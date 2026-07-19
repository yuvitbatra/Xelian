"""MCP server for SQLite: run read-only SQL against any local database file."""
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
        return "\n".join(f"{t}  {n}" for n, t in cur.fetchall()) or "(no tables)"


serve("sqlite-mcp", {
    "query": ("Run read-only SQL (first 200 rows as JSON)",
              {"type": "object", "properties": {
                  "db_path": {"type": "string"}, "sql": {"type": "string"}},
               "required": ["db_path", "sql"]}, query),
    "tables": ("List tables and views in a database",
               {"type": "object", "properties": {"db_path": {"type": "string"}},
                "required": ["db_path"]}, tables),
})
