"""Time conversions: 'now', '1700000000', '2026-07-19T10:00:00Z', 'now local'."""
import sys
from datetime import datetime, timezone

for line in sys.stdin:
    q = line.strip()
    try:
        if q in ("now", "", "now utc"):
            now = datetime.now(timezone.utc)
            print(f"{now.isoformat()}  (epoch {int(now.timestamp())})", flush=True)
        elif q == "now local":
            now = datetime.now().astimezone()
            print(f"{now.isoformat()}  (epoch {int(now.timestamp())})", flush=True)
        elif q.replace(".", "", 1).isdigit():
            ts = float(q)
            if ts > 1e12:
                ts /= 1000  # milliseconds
            dt = datetime.fromtimestamp(ts, tz=timezone.utc)
            print(dt.isoformat(), flush=True)
        else:
            dt = datetime.fromisoformat(q.replace("Z", "+00:00"))
            print(f"epoch {int(dt.timestamp())}", flush=True)
    except Exception as e:
        print(f"usage: 'now' | 'now local' | '<epoch>' | '<ISO-8601>' — {e}", flush=True)
