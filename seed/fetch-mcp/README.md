# fetch-mcp

MCP server for HTTP: fetch a URL and return status, headers, and body text.

Pure Python standard library — no dependencies, no API keys.

## Usage

```bash
xelian run xelian/fetch-mcp      # stdio MCP server
xelian gateway add xelian/fetch-mcp      # or serve it via the gateway
```

Point any MCP client at it, or use the Xelian gateway for a single endpoint.
