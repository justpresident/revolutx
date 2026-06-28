# revolutx-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) (MCP) server that
exposes the [Revolut X](https://exchange.revolut.com/) crypto exchange to LLM
clients (Claude Desktop, etc.) over stdio. Built on the
[`revolutx`](https://crates.io/crates/revolutx) SDK.

> **Not affiliated with Revolut.** Order placement is **real trading** and is
> disabled by default. You are responsible for your own risk controls and
> credential security.

## **NOTE: This project is in active development stage - public API will likely change in next versions**

## Install

```sh
cargo install revolutx-mcp
```

Or run from a checkout: `cargo run -p revolutx-mcp`.

## Configuration

The MCP **never handles credentials itself.** It connects to a running
[`revolutx agent`](../cli/README.md#signing-agent-headless--no-tty-clients), which
holds the encrypted keystore and does all signing and HTTP. The MCP therefore
has no API key, no private key, and no environment setting of its own — it learns
the target environment and the access policy from the agent at connect time.

The only configuration is a single, optional, non-sensitive variable:

| Variable | Purpose |
|---|---|
| `REVOLUTX_AGENT_SOCKET` | Path to the agent's unix socket. Optional — defaults to `$XDG_RUNTIME_DIR/revolutx-agent.sock`. |

### Setup

Create an encrypted vault once, and start the agent (it prompts for the master
password). The agent requires `--auth-token`: it prints a **one-time token** that
the LLM must present before the agent will serve any request — so another
process running as your user cannot use the signing oracle.

```sh
revolutx vault init                          # one-time: generates a key, guides you to create the API key
revolutx agent start --auth-token            # market data only; prints the one-time token
# ...widen what the MCP may do with --access:
revolutx agent start --auth-token --access view      # also read-only account data (balances, orders, trades)
revolutx agent start --auth-token --access trading   # also order placement/cancellation (REAL TRADING)
```

```json
{
  "mcpServers": {
    "revolutx": {
      "command": "revolutx-mcp"
    }
  }
}
```

Add `"env": { "REVOLUTX_AGENT_SOCKET": "/path/to/agent.sock" }` only if the agent
listens somewhere other than the default. The agent serves a single authenticated
client and exits when it disconnects, so start it alongside the MCP; if the MCP
restarts, restart the agent too (the token is single-use, so each session needs a
freshly started agent).

### Authenticating the session

The connection starts **unauthenticated**: every tool except `authenticate`
returns *"authenticate first"*. Copy the token the agent printed and ask the
assistant to authenticate, e.g. *"authenticate with token `<paste>`"*. The LLM
calls the `authenticate` tool; on success the other tools become usable for the
rest of the session.

**How much the MCP may do is the agent's decision** (`--access`), not the MCP's —
nothing in the MCP's environment can widen it. On `authenticate`, the agent
reports its tier so the assistant knows which tools will work.

## Tools

Authentication: `authenticate` (call first, with the agent's one-time token).

Market data (`--access market`, the default): `get_tickers`, `get_order_book`,
`get_public_order_book`, `get_candles`, `get_last_trades`, `get_all_trades`,
`get_currencies`, `get_pairs`.

Account reads (`--access view`): `get_balances`, `get_private_trades`,
`get_active_orders`, `get_historical_orders`, `get_order`, `get_order_fills`.

Order mutation (`--access trading`): `place_limit_order`, `place_market_order`,
`cancel_order`, `cancel_all_orders`.

Every tool is advertised unconditionally and forwarded to the agent, which is the
single authoritative gate: it refuses all requests until the session has
authenticated, then permits only the tools its `--access` tier allows (an
out-of-tier call comes back as an "access denied" error naming the tier needed).

Tool results are returned as pretty-printed JSON of the corresponding SDK
response. Decimal values are preserved as strings (never floats).

## Safety

- The MCP holds **no key material** — the agent does all signing and HTTP, so a
  compromised MCP environment cannot leak credentials.
- A one-time token authenticates the connecting peer before the oracle is
  exposed, so another same-UID process that races to the socket cannot trade as
  you. The token is constant-time compared and single-use.
- The agent serves only its `--access` tier and refuses anything above it — the
  MCP cannot widen that gate.
- All diagnostics go to stderr; stdout carries only the JSON-RPC protocol.

## License

Licensed under the [Apache License 2.0](../LICENSE).
