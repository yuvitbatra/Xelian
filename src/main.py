# Starter Xelian agent for `harbor`.
#
# `xelian run <you>/harbor` connects your terminal straight to this program's
# stdin/stdout. Replace the echo below with your agent's real logic.
import sys


def main() -> None:
    print("harbor: ready — type a message, Ctrl-D to exit.", flush=True)
    for line in sys.stdin:
        message = line.rstrip("\n")
        if not message:
            continue
        print(f"you said: {message}", flush=True)


if __name__ == "__main__":
    main()
