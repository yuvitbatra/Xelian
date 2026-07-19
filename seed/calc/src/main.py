"""A calculator with functions: '2*(3+4)**2', 'sqrt(2)*pi', 'log2(4096)'."""
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
