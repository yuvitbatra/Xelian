import os
import sys
import socket
import subprocess
import tomllib
from pathlib import Path
from typing import Union

from ._cli import find_binary, run_install, InstallInfo
from .handles import AgentHandle, MCPHandle


class HarborError(Exception):
    pass


class InstallError(HarborError):
    pass


class TypeMismatchError(HarborError):
    pass


def install(target: str, prepare: bool = False) -> InstallInfo:
    try:
        return run_install(target, prepare=prepare)
    except FileNotFoundError as e:
        raise InstallError(str(e)) from e
    except RuntimeError as e:
        raise InstallError(str(e)) from e


def _load_manifest(package_dir: Path) -> dict:
    manifest_path = package_dir / "harbor.toml"
    if not manifest_path.exists():
        raise InstallError(f"harbor.toml not found at {manifest_path}")
    with open(manifest_path, "rb") as f:
        return tomllib.load(f)


def _resolve_env_vars(environment: dict) -> list[tuple[str, str]]:
    result = []
    for name, spec in environment.items():
        val = os.environ.get(name)
        if val is not None:
            result.append((name, val))
        elif spec.get("required", False):
            raise HarborError(
                f"Required environment variable {name!r} is not set; cannot launch"
            )
        elif "default" in spec:
            result.append((name, spec["default"]))
    return result


def _resolve_port(requested: int) -> int:
    if requested == 0:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("127.0.0.1", 0))
            return s.getsockname()[1]

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("127.0.0.1", requested))
            return requested
    except OSError:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
            print(
                f"MCP server: assigned PORT={port} (requested port {requested} is busy)",
                file=sys.stderr,
            )
            return port


def _launch_entrypoint(info: InstallInfo) -> Union[AgentHandle, MCPHandle]:
    manifest = _load_manifest(info.package_dir)
    package_type = manifest.get("package-type", info.package_type)
    language = manifest.get("language", info.language)
    entrypoint = manifest.get("entrypoint", "")
    environment = manifest.get("environment", {})

    env_pairs = _resolve_env_vars(environment)

    if language == "python":
        python_bin = info.env_dir / "bin" / "python"
        if not python_bin.is_file():
            raise InstallError(f"Python binary not found at {python_bin}")
        entrypoint_path = info.package_dir / entrypoint
        cmd = [str(python_bin), str(entrypoint_path)]
        cwd = str(info.package_dir)
    elif language == "node":
        node_bin = info.bin_dir / "node"
        if not node_bin.is_file():
            raise InstallError(f"Node binary not found at {node_bin}")
        script = info.env_dir / entrypoint
        if not script.is_file():
            raise InstallError(f"Entrypoint script not found at {script}")
        cmd = [
            str(node_bin),
            "--preserve-symlinks",
            "--preserve-symlinks-main",
            str(script),
        ]
        cwd = str(info.env_dir)
    else:
        raise InstallError(f"Unsupported language: {language}")

    child_env = os.environ.copy()
    for key, val in env_pairs:
        child_env[key] = val

    port = None
    if package_type == "mcp":
        requested_port = manifest.get("port")
        if requested_port is not None:
            port = _resolve_port(requested_port)
            child_env["PORT"] = str(port)

    process = subprocess.Popen(
        cmd,
        cwd=cwd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        env=child_env,
        text=True,
    )

    if package_type == "agent":
        return AgentHandle(process, manifest)
    elif package_type == "mcp":
        return MCPHandle(process, manifest, port=port)
    else:
        process.terminate()
        raise InstallError(f"Unknown package type: {package_type}")


def run(target: str) -> Union[AgentHandle, MCPHandle]:
    """Download (if needed), install, and run a package.

    Returns a handle appropriate to the package's type:
    - AgentHandle with .chat() for agents
    - MCPHandle with .expose() for MCP servers
    """
    info = install(target, prepare=True)
    return _launch_entrypoint(info)


def agent(target: str) -> AgentHandle:
    """Run a package, asserting it is an agent.

    Raises TypeMismatchError if the resolved package's type is not 'agent'.
    """
    info = install(target, prepare=True)
    if info.package_type != "agent":
        raise TypeMismatchError(
            f"Package {target} has type {info.package_type!r}, expected 'agent'"
        )
    handle = _launch_entrypoint(info)
    assert isinstance(handle, AgentHandle)
    return handle


def mcp(target: str) -> MCPHandle:
    """Run a package, asserting it is an MCP server.

    Raises TypeMismatchError if the resolved package's type is not 'mcp'.
    """
    info = install(target, prepare=True)
    if info.package_type != "mcp":
        raise TypeMismatchError(
            f"Package {target} has type {info.package_type!r}, expected 'mcp'"
        )
    handle = _launch_entrypoint(info)
    assert isinstance(handle, MCPHandle)
    return handle
