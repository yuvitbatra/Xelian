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

    which = shutil.which("xelian")
    if which:
        return which

    raise FileNotFoundError(
        "xelian binary not found. Make sure 'xelian' is installed and on your PATH, "
        "or run this from the project root with a built binary in target/"
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
