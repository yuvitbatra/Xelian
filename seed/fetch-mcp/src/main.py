"""MCP server for HTTP: fetch a URL and return status, headers, and body text."""
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
        return f"HTTP {resp.status} {ctype}\n\n{text}"


serve("fetch-mcp", {
    "http_get": ("GET a URL (15s timeout) and return status + body text (capped)",
                 {"type": "object", "properties": {
                     "url": {"type": "string"},
                     "max_bytes": {"type": "number"}},
                  "required": ["url"]}, http_get),
})
