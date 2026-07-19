# git-mcp

MCP server for git: status, log, and diff of any local repository.

Pure Python standard library — no dependencies, no API keys.

## Usage

```bash
xelian run xelian/git-mcp      # stdio MCP server
xelian gateway add xelian/git-mcp      # or serve it via the gateway
```

Point any MCP client at it, or use the Xelian gateway for a single endpoint.
