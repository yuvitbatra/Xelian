"""Generate identifiers: 'uuid', 'uuid 3', 'hex 32', 'slug'."""
import secrets
import sys
import uuid

ADJ = "brisk calm deft eager fleet keen lucid noble prime quiet swift vivid".split()
NOUN = "atlas comet delta ember flare grove lumen orbit quartz ridge spark vertex".split()

for line in sys.stdin:
    parts = line.strip().lower().split()
    try:
        kind = parts[0] if parts else "uuid"
        n = int(parts[1]) if len(parts) > 1 else 1
        if kind == "uuid":
            print(" ".join(str(uuid.uuid4()) for _ in range(max(1, min(n, 20)))), flush=True)
        elif kind == "hex":
            print(secrets.token_hex(max(4, min(n, 64))), flush=True)
        elif kind == "slug":
            print(f"{secrets.choice(ADJ)}-{secrets.choice(NOUN)}-{secrets.token_hex(2)}", flush=True)
        else:
            print("usage: 'uuid [n]' | 'hex [bytes]' | 'slug'", flush=True)
    except ValueError:
        print("usage: 'uuid [n]' | 'hex [bytes]' | 'slug'", flush=True)
