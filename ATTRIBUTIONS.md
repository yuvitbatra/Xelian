# Attributions & Credits

Xelian is a runtime and registry. It **imports and runs** open-source agents
and MCP servers built by other people; it does not copy their source into this
repository, and it does not relicense their work. Every project listed here
remains the property of its authors and is used **under its own license**, with
gratitude.

When you run `xelian add <github-url>`, Xelian downloads that repository at a
pinned commit into your local cache (`~/.xelian`), infers how to run it, and
launches it on your machine. The code is the authors', unmodified. This file
credits the projects Xelian is known to work with and records the license each
one grants.

If you are the author of a project listed here and would like the attribution
changed or removed, please open an issue.

## How licenses are respected

- **No relicensing.** Xelian never changes a project's license. An imported
  package carries the upstream license; `xelian push` refuses to publish while
  the `license` field still holds the `PLEASE_EDIT` placeholder, precisely so a
  republisher must set the upstream project's real license before sharing.
- **Attribution preserved.** Imported packages keep their `LICENSE`,
  `README`, and author metadata in the built archive.
- **Pinned by commit.** Imports are addressed by commit SHA, so what you run is
  exactly what the author published at that revision.

## Projects Xelian works with

All of the following are permissively licensed (MIT or Apache-2.0). Licenses
verified against each repository's `LICENSE` file on 2026-07-21; the upstream
license is authoritative if it has since changed.

### MCP servers

| Project | Author | License |
|---|---|---|
| [modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers) | Anthropic, PBC | MIT |
| [modelcontextprotocol/servers-archived](https://github.com/modelcontextprotocol/servers-archived) | Anthropic, PBC | MIT |
| [microsoft/playwright-mcp](https://github.com/microsoft/playwright-mcp) | Microsoft | Apache-2.0 |
| [executeautomation/mcp-playwright](https://github.com/executeautomation/mcp-playwright) | ExecuteAutomation | MIT |
| [GLips/Figma-Context-MCP](https://github.com/GLips/Figma-Context-MCP) | Graham Lipsman | MIT |
| [mendableai/firecrawl-mcp-server](https://github.com/mendableai/firecrawl-mcp-server) | Mendable / Firecrawl | MIT |
| [exa-labs/exa-mcp-server](https://github.com/exa-labs/exa-mcp-server) | Exa Labs | MIT |
| [chroma-core/chroma-mcp](https://github.com/chroma-core/chroma-mcp) | Chroma | Apache-2.0 |
| [sooperset/mcp-atlassian](https://github.com/sooperset/mcp-atlassian) | Soomin Kim | MIT |
| [zcaceres/fetch-mcp](https://github.com/zcaceres/fetch-mcp) | Zach Caceres | MIT |
| [QuantGeekDev/docker-mcp](https://github.com/QuantGeekDev/docker-mcp) | Alex Andru | MIT |
| [wonderwhy-er/DesktopCommanderMCP](https://github.com/wonderwhy-er/DesktopCommanderMCP) | Eduard Ruzga | MIT |
| [tavily-ai/tavily-mcp](https://github.com/tavily-ai/tavily-mcp) | Tavily | MIT |
| [upstash/context7](https://github.com/upstash/context7) | Upstash | MIT |
| [supabase/mcp](https://github.com/supabase/mcp) | Supabase | Apache-2.0 |
| [awslabs/mcp](https://github.com/awslabs/mcp) | Amazon Web Services | Apache-2.0 |

### AI agents / frameworks

| Project | Author | License |
|---|---|---|
| [princeton-nlp/SWE-agent](https://github.com/princeton-nlp/SWE-agent) | Princeton NLP | MIT |
| [AntonOsika/gpt-engineer](https://github.com/AntonOsika/gpt-engineer) | Anton Osika | MIT |
| [FoundationAgents/MetaGPT](https://github.com/FoundationAgents/MetaGPT) | Foundation Agents | MIT |
| [stitionai/devika](https://github.com/stitionai/devika) | Stition AI | MIT |
| [TransformerOptimus/SuperAGI](https://github.com/TransformerOptimus/SuperAGI) | TransformerOptimus | MIT |
| [crewAIInc/crewAI](https://github.com/crewAIInc/crewAI) | crewAI, Inc. | MIT |
| [openai/swarm](https://github.com/openai/swarm) | OpenAI | MIT |
| [MineDojo/Voyager](https://github.com/MineDojo/Voyager) | MineDojo | MIT |
| [OpenHands/OpenHands](https://github.com/OpenHands/OpenHands) | All-Hands-AI | MIT |
| [microsoft/JARVIS](https://github.com/microsoft/JARVIS) | Microsoft | MIT |

## Tools Xelian provisions

To run packages with zero setup, Xelian downloads and runs these tools into
`~/.xelian` (never touching system state):

- [astral-sh/uv](https://github.com/astral-sh/uv) — Python runtime & installer (Apache-2.0 OR MIT)
- [Node.js](https://nodejs.org) — JavaScript runtime (MIT and others; see Node's licensing)
- [oven-sh/bun](https://github.com/oven-sh/bun), [pnpm](https://github.com/pnpm/pnpm), [yarn](https://github.com/yarnpkg/berry) — provisioned only when a package's build script requires them (MIT)

## Bundled Rust dependencies

The Xelian binary links a number of Rust crates. Their licenses (predominantly
MIT/Apache-2.0) are recorded in `Cargo.lock` and reproduced in full by
`cargo about` / `cargo license`. See `Cargo.toml` for the direct dependency
set.

---

Xelian itself is licensed under the [MIT License](LICENSE). Using Xelian to run
a third-party package does not change that package's license or yours.
