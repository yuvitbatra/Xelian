# Starter Xelian agent for `demo`.
#
# `xelian run <you>/demo` connects your terminal straight to this program's
# stdin/stdout. Replace the echo below with your agent's real logic.
import sys


def main() -> None:
    # Xelian prints the readiness banner and the `> ` prompt before handing
    # over the terminal, so an agent should not print its own — two "ready"
    # lines read as a glitch. Just print a `> ` before each subsequent turn.
    for line in sys.stdin:
        message = line.rstrip("\n")
        if not message:
            continue
        print(f"you said: {message}", flush=True)
        print("> ", end="", flush=True)


if __name__ == "__main__":
    main()
