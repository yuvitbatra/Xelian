import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple


class InstallInfo(NamedTuple):
    name: str
    version: str
    package_type: str
    language: str
    package_dir: Path
    env_dir: Path
    bin_dir: Path


def find_binary() -> str:
    # 1. Explicit override always wins (M-8): a user or test can pin the exact
    #    binary via XELIAN_BIN, and it takes precedence over any discovery.
    override = os.environ.get("XELIAN_BIN")
    if override:
        if Path(override).is_file():
            return override
        raise FileNotFoundError(f"XELIAN_BIN points at {override!r}, which is not a file")

    # 2. An installed `xelian` on PATH — the normal case for end users.
    which = shutil.which("xelian")
    if which:
        return which

    # 3. Development fallback: a locally-built binary in this repo's target/,
    #    so the SDK works from a source checkout with no install step.
    if getattr(sys, "frozen", False):
        base = Path(sys.executable).parent
    else:
        base = Path(__file__).resolve().parent.parent.parent
    for candidate in [
        base / "target" / "debug" / "xelian",
        base / "target" / "release" / "xelian",
    ]:
        if candidate.is_file():
            return str(candidate)

    raise FileNotFoundError(
        "xelian binary not found. Install the `xelian` CLI and ensure it is on "
        "your PATH, set XELIAN_BIN to its path, or build it from a source "
        "checkout (cargo build) so target/debug/xelian exists."
    )


def run_install(target: str, prepare: bool = False) -> InstallInfo:
    """Invoke the xelian binary to prepare a package without launching it.

    ``prepare=False`` runs pipeline steps 1-9 (``--install-only``), matching
    ``xelian.install()`` (SPEC §15.2). ``prepare=True`` runs steps 1-10
    (``--prepare``) so model provisioning and permission disclosure happen in
    the binary before the SDK spawns the process for ``run()/agent()/mcp()``.
    """
    binary = find_binary()

    flag = "--prepare" if prepare else "--install-only"
    result = subprocess.run(
        [binary, "run", target, flag],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        stderr = result.stderr.strip()
        msg = stderr or f"xelian exited with code {result.returncode}"
        raise RuntimeError(f"xelian install failed: {msg}")

    for line in result.stdout.strip().split("\n"):
        line = line.strip()
        if line.startswith("XELIAN_INSTALLED|"):
            parts = line.split("|")
            if len(parts) == 8:
                return InstallInfo(
                    name=parts[1],
                    version=parts[2],
                    package_type=parts[3],
                    language=parts[4],
                    package_dir=Path(parts[5]),
                    env_dir=Path(parts[6]),
                    bin_dir=Path(parts[7]),
                )

    raise RuntimeError(
        f"Failed to parse xelian install output: {result.stdout.strip()}"
    )
