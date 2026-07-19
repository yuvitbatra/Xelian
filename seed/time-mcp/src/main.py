"""MCP server for time: current time, epoch and ISO-8601 conversion."""
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
