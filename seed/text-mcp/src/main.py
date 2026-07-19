"""MCP server for text: regex search/replace and unified diffs."""
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

import difflib
import re


def regex_search(args):
    matches = [m.group(0) for m in re.finditer(str(args["pattern"]), str(args["text"]))]
    return "\n".join(matches[:100]) or "(no matches)"


def regex_replace(args):
    result, n = re.subn(str(args["pattern"]), str(args["replacement"]), str(args["text"]))
    return f"({n} replacement(s))\n{result}"


def unified_diff(args):
    diff = difflib.unified_diff(
        str(args["a"]).splitlines(), str(args["b"]).splitlines(),
        lineterm="", fromfile="a", tofile="b",
    )
    return "\n".join(diff) or "(identical)"


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
