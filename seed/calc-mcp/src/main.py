"""MCP server for math: safe expression evaluation and list statistics."""
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
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) \
                and node.func.id in _FUNCS and not node.keywords:
            return _FUNCS[node.func.id](*[walk(a) for a in node.args])
        if isinstance(node, ast.Tuple):
            return tuple(walk(e) for e in node.elts)
        raise ValueError(f"unsupported expression: {ast.dump(node)[:60]}")

    return walk(ast.parse(expr.strip(), mode="eval"))

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
