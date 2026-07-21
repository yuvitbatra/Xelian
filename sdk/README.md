# Xelian Python SDK

Run AI agents and MCP servers from Python with one call — backed by the
[Xelian](https://github.com/yuvitbatra/Xelian) registry and runtime.

```bash
pip install xelian-sdk
```

The SDK shells out to the `xelian` CLI, so install that too (one line, no Rust
toolchain needed):

```bash
curl -fsSL https://raw.githubusercontent.com/yuvitbatra/Xelian/main/scripts/install.sh | sh
```

(or point `XELIAN_BIN` at a `xelian` binary you already have.)

## Use it

```python
import xelian

# Run any of hundreds of catalog packages by name, or any GitHub repo by URL —
# resolved through the registry and launched locally.
server = xelian.mcp("QuantGeekDev/docker-mcp")
info = server.expose()          # {"transport": "stdio", "stdin": ..., "stdout": ...}
# ... speak MCP JSON-RPC over info["stdin"]/info["stdout"] ...
server.close()

# Chat with an agent package:
agent = xelian.run("some-owner/some-agent")
print(agent.chat("hello"))
```

## API

- `xelian.run(target)` → an `AgentHandle` (with `.chat()`) or `MCPHandle`.
- `xelian.mcp(target)` → an `MCPHandle` (`.expose()` returns stdio/HTTP transport).
- `xelian.install(target, prepare=False)` → prepare without launching.

`target` is a registry/catalog name (`owner/name`), a GitHub URL, or a local
`.xelian` archive.

## License

MIT. Catalog packages are third-party projects, each run under its own license.
