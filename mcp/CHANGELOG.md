# Changelog — revolutx-mcp

All notable changes to the `revolutx-mcp` server are documented here, following
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The `revolutx` library
it builds on has its own changelog at [`../CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.2.0] - 2026-06-28

First release on the `revolutx` 0.3 SDK: runs on the shared command layer and
decouples the server's lifecycle from the agent's.

### Added

- **`authenticate` tool.** The LLM must call it with the agent's one-time token
  before any other tool works; on success the server reports the agent's
  environment and `--access` tier.

### Changed

- **Runs on the shared command layer.** Each tool maps onto a `revolutx::commands`
  `Command`, dispatched through one `execute` + `JsonPresenter`, so the MCP's JSON
  is byte-identical to the CLI's `--json` and all three surfaces parse and dispatch
  identically.
- **Connects to the agent lazily, independent of its lifecycle.** The server no
  longer connects at startup — it starts cleanly with no agent running and never
  touches the socket file until needed, opening the socket only when `authenticate`
  is called and reconnecting on each call. So the agent can be started, stopped, or
  restarted at any time; after a restart, call `authenticate` again with the new
  token to resume.
- All tools are advertised unconditionally and the agent enforces the `--access`
  gate, so an out-of-tier call returns "access denied" naming the tier needed
  (account reads need `--access view`, order placement/cancellation `--access
  trading`).

## [0.1.0] - 2026-06-19

Initial release: a Model Context Protocol stdio server exposing the Revolut X
crypto exchange to LLM clients.
