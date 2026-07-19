"""Hash and encode text: 'sha256 hello', 'md5 x', 'b64 secret', 'unb64 c2VjcmV0'."""
import base64
import hashlib
import sys

for line in sys.stdin:
    cmd, _, rest = line.strip().partition(" ")
    cmd = cmd.lower()
    try:
        if cmd in ("sha256", "sha1", "sha512", "md5"):
            print(getattr(hashlib, cmd)(rest.encode()).hexdigest(), flush=True)
        elif cmd == "b64":
            print(base64.b64encode(rest.encode()).decode(), flush=True)
        elif cmd == "unb64":
            print(base64.b64decode(rest.encode()).decode("utf-8", "replace"), flush=True)
        elif cmd == "hex":
            print(rest.encode().hex(), flush=True)
        elif cmd == "unhex":
            print(bytes.fromhex(rest).decode("utf-8", "replace"), flush=True)
        else:
            print("usage: sha256|sha1|sha512|md5|b64|unb64|hex|unhex <text>", flush=True)
    except Exception as e:
        print(f"error: {e}", flush=True)
