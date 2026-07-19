"""H-215 — SDK integration tests against the real built binary and a real
local registry: publish two fixture packages, then drive install/run/agent/mcp
exactly as an SDK user would.

Run from the repo with the registry's Python (has fastapi/uvicorn):

    cd sdk && ../registry/.venv/bin/python -m pytest tests -q

Requires target/debug/xelian to be built.
"""

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import textwrap
import time
import urllib.request
from pathlib import Path

import pytest

SDK_DIR = Path(__file__).resolve().parents[1]
ROOT = SDK_DIR.parent
BINARY = ROOT / "target" / "debug" / "xelian"

sys.path.insert(0, str(SDK_DIR))

import xelian  # noqa: E402
import xelian._cli  # noqa: E402


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _wait_http(url: str, timeout: float = 30.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2):
                return
        except OSError:
            time.sleep(0.3)
    raise RuntimeError(f"{url} did not come up within {timeout}s")


def _write_package(root: Path, name: str, package_type: str, entry_body: str):
    pkg = root / name
    (pkg / "src").mkdir(parents=True)
    (pkg / "xelian.toml").write_text(textwrap.dedent(f'''\
        spec-version = 1
        name = "{name}"
        version = "0.1.0"
        description = "SDK integration fixture ({package_type})"
        package-type = "{package_type}"
        language = "python"
        runtime = ">=3.11,<4"
        entrypoint = "src/main.py"
        license = "MIT"
        permissions = []
        features = []

        [author]
        name = "SDK Test"
        email = "sdk@example.com"

        [dependencies]
        manifest = "pyproject.toml"
    '''))
    (pkg / "src" / "main.py").write_text(entry_body)
    (pkg / "pyproject.toml").write_text(textwrap.dedent(f'''\
        [project]
        name = "{name}"
        version = "0.1.0"
        requires-python = ">=3.11"
    '''))
    (pkg / "README.md").write_text(f"# {name}\n")
    (pkg / "LICENSE").write_text("MIT\n")
    subprocess.run(["git", "init", "-q", "."], cwd=pkg, check=True)
    subprocess.run(["git", "add", "-A"], cwd=pkg, check=True)
    return pkg


ECHO_AGENT = textwrap.dedent('''\
    import sys

    for line in sys.stdin:
        print(f"echo: {line.strip()}", flush=True)
''')

CALC_MCP = textwrap.dedent('''\
    import json
    import sys

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if "id" not in msg:
            continue
        method = msg.get("method")
        if method == "initialize":
            result = {"protocolVersion": msg["params"].get("protocolVersion", ""),
                      "capabilities": {"tools": {}},
                      "serverInfo": {"name": "sdk-calc", "version": "0.1.0"}}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
''')


@pytest.fixture(scope="session")
def live_env():
    """Boot a real registry, publish fixture packages with the real binary,
    and point both the SDK and the CLI at an isolated XELIAN home."""
    if not BINARY.is_file():
        pytest.fail(f"build the CLI first: cargo build (missing {BINARY})")

    tmp = Path(tempfile.mkdtemp(prefix="xelian-sdk-it-"))
    port = _free_port()
    registry_url = f"http://127.0.0.1:{port}"

    env = os.environ.copy()
    env["XELIAN_REGISTRY_ROOT"] = str(tmp / "registry-root")
    registry = subprocess.Popen(
        [sys.executable, "-m", "uvicorn", "app.main:app", "--port", str(port)],
        cwd=ROOT / "registry",
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_http(f"{registry_url}/health")

        # Isolated CLI home + registry URL for every subprocess and SDK call.
        os.environ["HOME"] = str(tmp / "home")
        os.environ["XELIAN_REGISTRY_URL"] = registry_url
        (tmp / "home").mkdir()

        req = urllib.request.Request(
            f"{registry_url}/auth/signup",
            data=json.dumps({"username": "sdktest", "password": "password123"}).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req) as resp:
            assert resp.status == 201

        login = subprocess.run(
            [str(BINARY), "login", "--username", "sdktest", "--password-stdin"],
            input="password123",
            text=True,
            capture_output=True,
        )
        assert login.returncode == 0, login.stderr

        for name, ptype, body in [
            ("sdk-echo-agent", "agent", ECHO_AGENT),
            ("sdk-calc-mcp", "mcp", CALC_MCP),
        ]:
            pkg = _write_package(tmp, name, ptype, body)
            push = subprocess.run(
                [str(BINARY), "push"], cwd=pkg, capture_output=True, text=True
            )
            assert push.returncode == 0, f"{name}: {push.stderr or push.stdout}"

        yield {"registry_url": registry_url, "tmp": tmp}
    finally:
        registry.terminate()
        registry.wait(timeout=10)
        shutil.rmtree(tmp, ignore_errors=True)


def test_install_prepares_without_launching(live_env):
    info = xelian.install("sdktest/sdk-echo-agent")
    assert info.package_type == "agent"
    assert (info.package_dir / "xelian.toml").is_file()
    assert (info.env_dir / "bin" / "python").is_file()


def test_agent_chat_round_trips(live_env):
    with xelian.run("sdktest/sdk-echo-agent") as agent:
        assert agent.chat("hello") == "echo: hello"
        assert agent.chat("second message") == "echo: second message"


def test_agent_helper_asserts_type(live_env):
    with pytest.raises(xelian.TypeMismatchError) as exc:
        xelian.agent("sdktest/sdk-calc-mcp")
    assert "expected 'agent'" in str(exc.value)


def test_mcp_helper_asserts_type(live_env):
    with pytest.raises(xelian.TypeMismatchError) as exc:
        xelian.mcp("sdktest/sdk-echo-agent")
    assert "expected 'mcp'" in str(exc.value)


def test_mcp_expose_transport_is_usable(live_env):
    with xelian.mcp("sdktest/sdk-calc-mcp") as server:
        transport = server.expose()
        assert transport["transport"] == "stdio"
        # Drive a real MCP initialize over the exposed pipes.
        request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "pytest", "version": "0"}},
        }
        transport["stdin"].write(json.dumps(request) + "\n")
        transport["stdin"].flush()
        response = json.loads(transport["stdout"].readline())
        assert response["result"]["serverInfo"]["name"] == "sdk-calc"


def test_unknown_package_error_is_clear(live_env):
    with pytest.raises(xelian.InstallError) as exc:
        xelian.install("sdktest/does-not-exist")
    assert "does-not-exist" in str(exc.value)


def test_missing_binary_error_is_clear(monkeypatch):
    monkeypatch.setattr(xelian._cli.shutil, "which", lambda _name: None)
    monkeypatch.setattr(xelian._cli.Path, "is_file", lambda _self: False)
    with pytest.raises(FileNotFoundError) as exc:
        xelian._cli.find_binary()
    assert "PATH" in str(exc.value)
