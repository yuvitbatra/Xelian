import subprocess
from typing import Optional, Union


class AgentHandle:
    """Handle for a running agent process. Provides .chat() for interaction."""

    def __init__(self, process: subprocess.Popen, manifest: dict):
        self._process = process
        self._manifest = manifest

    def chat(self, message: str) -> str:
        """Send a message to the agent and return its response.

        Writes the message followed by a newline to the agent's stdin,
        then reads one line from its stdout as the response.
        """
        if self._process.stdin is None or self._process.stdout is None:
            raise RuntimeError("Agent process is not running or I/O is unavailable")

        self._process.stdin.write(message + "\n")
        self._process.stdin.flush()

        response = self._process.stdout.readline()
        return response.rstrip("\n")

    def close(self) -> None:
        """Terminate the agent process."""
        if self._process.poll() is None:
            self._process.terminate()
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()

    @property
    def process(self) -> subprocess.Popen:
        """The underlying subprocess. Direct access for advanced use cases."""
        return self._process

    def __enter__(self) -> "AgentHandle":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()


class MCPHandle:
    """Handle for a running MCP server process. Provides .expose() for access."""

    def __init__(
        self,
        process: subprocess.Popen,
        manifest: dict,
        port: Optional[int] = None,
    ):
        self._process = process
        self._manifest = manifest
        self._port = port

    def expose(self) -> dict:
        """Return connection information for the running MCP server.

        If the server declared a port and one was allocated, returns
        a URL and port number. Otherwise returns stdio transport info.
        """
        if self._port is not None:
            return {"url": f"http://127.0.0.1:{self._port}", "port": self._port}
        return {
            "transport": "stdio",
            "stdin": self._process.stdin,
            "stdout": self._process.stdout,
        }

    def close(self) -> None:
        """Terminate the MCP server process."""
        if self._process.poll() is None:
            self._process.terminate()
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()

    @property
    def process(self) -> subprocess.Popen:
        """The underlying subprocess. Direct access for advanced use cases."""
        return self._process

    def __enter__(self) -> "MCPHandle":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()
