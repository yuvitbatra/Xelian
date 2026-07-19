# sysinfo-mcp

MCP server for system info: OS, CPU, Python, and disk usage.

Pure Python standard library — no dependencies, no API keys.

## Usage

```bash
xelian run xelian/sysinfo-mcp      # stdio MCP server
xelian gateway add xelian/sysinfo-mcp      # or serve it via the gateway
```

Point any MCP client at it, or use the Xelian gateway for a single endpoint.
