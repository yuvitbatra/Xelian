# files-mcp

MCP server for read-only file access: list directories, read files, stat paths.

Pure Python standard library — no dependencies, no API keys.

## Usage

```bash
xelian run xelian/files-mcp      # stdio MCP server
xelian gateway add xelian/files-mcp      # or serve it via the gateway
```

Point any MCP client at it, or use the Xelian gateway for a single endpoint.
