"""MCP server for system info: OS, CPU, Python, and disk usage."""
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
import platform
import shutil


def overview(args):
    return "\n".join([
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
