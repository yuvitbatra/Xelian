"""Convert units from the chat line: '10 km in miles', '72 f in c', '512 mb in gb'."""
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
