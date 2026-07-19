"""Test regexes interactively: '<pattern> ::: <text>' prints all matches."""
import re
import sys

for line in sys.stdin:
    raw = line.rstrip("\n")
    if ":::" not in raw:
        print("usage: <pattern> ::: <text>", flush=True)
        continue
    pattern, _, text = raw.partition(":::")
    try:
        matches = [m.group(0) for m in re.finditer(pattern.strip(), text.strip())]
        if matches:
            print(f"{len(matches)} match(es): " + " | ".join(matches[:20]), flush=True)
        else:
            print("no matches", flush=True)
    except re.error as e:
        print(f"invalid pattern: {e}", flush=True)
