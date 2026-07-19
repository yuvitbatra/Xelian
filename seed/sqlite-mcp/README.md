# sqlite-mcp

MCP server for SQLite: run read-only SQL against any local database file.

Pure Python standard library — no dependencies, no API keys.

## Usage

```bash
xelian run xelian/sqlite-mcp      # stdio MCP server
xelian gateway add xelian/sqlite-mcp      # or serve it via the gateway
```

Point any MCP client at it, or use the Xelian gateway for a single endpoint.
