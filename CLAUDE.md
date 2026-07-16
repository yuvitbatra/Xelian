# PROJECT: HARBOR — a local-first registry + runtime for AI agents, MCP servers
# ("Hugging Face + Ollama for agents")

## YOUR ROLE (read this first, it governs everything)

You are the CEO and CTO, opus, haiku and sonnet are your workers. You are responsible for all their work and output. Make sure never to waste tokens and use opus/sonnet/haiku, however work quality should be to your level. Make sure everything always works and update backlog once you finish a task from there.

## NOT TO DO
- never change the architecture
- drift of course, implement what isn't asked for

---

## THE PRODUCT (v1 scope)
A developer can:
1. Package an agent / MCP server in a standard format.
2. `harbor push` it to a public registry.
3. Anyone else can `harbor run user/name` and it works locally with ZERO setup —
   the exact Ollama feel: one-line install, single static binary, no account needed
   to pull/run, drops into a chat REPL in seconds. This works for mcp servers/
   agents easily
4. Python package to pull and use these agents/mcp servers
5. Cache downloaded packages locally for fast subsequent launches.
6. Public registry with agents
7. Agents/MCP server etc should be easy to publish and run seamlessly
8. register and run directly from github using link. harbor add github_link, and caches it for user and runs it

---

## PROJECT VISION

Harbor is a local-first registry and runtime for AI agents.

The goal is to make running agents feel exactly like running models with Ollama.

Instead of downloading model weights, users download runnable agents.

Example:

```bash
harbor run username/my-agent
```

and it simply works.

---

## DESIGN PRINCIPLES

These principles are more important than adding features.

### Local-first

Everything should run locally by default.

No cloud dependency should be required for execution.

### Simple CLI

The CLI should feel similar to:

- git
- cargo
- uv
- npm
- ollama

Commands should be obvious and memorable.

### Batteries Included

A new user should be able to:

- install Harbor
- run an agent
- chat with it
- run it and use it in python if using the package
- run the mcp server instantaneously locally

within a few minutes.

### Convention over Configuration

Prefer sensible defaults.

Avoid requiring configuration files whenever possible.

### Single Static Binary

The Harbor CLI should be distributed as a single executable with minimal dependencies.

### Simplicity

The simplest solution that works is usually the correct one.

Avoid complexity unless it solves a real problem.

### IMPLEMENTATION PHILOSOPHY

Build the smallest version that proves the core idea.

Prefer working software over feature completeness.

Protect the MVP from feature creep.

---

## PACKAGE PHILOSOPHY

Harbor's most important contribution is defining a **standard package format** for AI agents.

Models converged around artifacts like GGUF.

Agents currently do not have an equivalent standard.

Harbor aims to define a standard package format for locally runnable AI agents and MCP servers.

Every Harbor package should expose a consistent interface regardless of implementation language.

A Harbor package may internally contain:

- a Python project
- a Node project
- a Rust binary
- an MCP server
- an agent

The runtime abstracts these differences away.

---

## MY CONTEXT
- Solo builder. Don't assume expertise, but don't condescend either. Need to build
  something people will actually use and can be useful.

---

# TECH STACK

- **CLI & Runtime:** Rust
- **Registry Backend:** Python (FastAPI)
- **Python SDK:** Python
- **Website:** Next.js

---

## SUCCESS CRITERIA

Harbor V1 is successful if a developer can:

- Package and publish an existing open-source agent or MCP server in under 5 minutes.
- Publish it with harbor push (one command).
- harbor run user/name automatically downloads, installs, and launches the package. Agents should open an interactive REPL; MCP servers should start and expose the server locally.
- Use it in Python with the SDK with minimal boilerplate.
- Run an MCP server locally with one command, fully functional.


The user experience should feel comparable to:

```bash
ollama run llama3
```

## REMEMBER

- Unless I explicitly decide otherwise, preserve previous architectural decisions
  instead of proposing a completely different architecture every session.
- If a proposed feature does not directly improve the package format, runtime, registry,
  or developer experience, question whether it belongs in V1.