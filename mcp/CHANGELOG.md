# Changelog — revolutx-mcp

All notable changes to the `revolutx-mcp` server are documented here, following
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The `revolutx` library
it builds on has its own changelog at [`../CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.3.1] - 2026-07-01

### Fixed

- Builds on revolutx 0.5.1: reads larger agent responses (up to 8 MiB) and fails fast on
  a broken agent connection instead of reading misframed data.

## [0.3.0] - 2026-07-01

### Added

- Interactive authorization: call `authenticate` with **no token** to request operator
  approval. The MCP appears in the agent console (labelled `revolutx-mcp`) for the
  operator to `grant`/`deny`; it replies "awaiting operator approval", and a second
  `authenticate` completes once approved (the pending connection is reused, so it stays
  one console entry). An optional `access` argument sets the requested tier (default
  `view`).

### Changed

- Builds on `revolutx` 0.5; the `token` argument to `authenticate` is now optional (the
  token path itself is unchanged). The agent it connects to is now persistent and
  multi-client, so it no longer exits the moment the MCP disconnects.

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
