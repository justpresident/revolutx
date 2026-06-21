# revolutx-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) (MCP) server that
exposes the [Revolut X](https://exchange.revolut.com/) crypto exchange to LLM
clients (Claude Desktop, etc.) over stdio. Built on the
[`revolutx`](https://crates.io/crates/revolutx) SDK.

> **Not affiliated with Revolut.** Order placement is **real trading** and is
> disabled by default. You are responsible for your own risk controls and
> credential security.

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
the target environment and the trading policy from the agent at connect time.

The only configuration is a single, optional, non-sensitive variable:

| Variable | Purpose |
|---|---|
| `REVOLUTX_AGENT_SOCKET` | Path to the agent's unix socket. Optional — defaults to `$XDG_RUNTIME_DIR/revolutx-agent.sock`. |

### Setup

Create an encrypted vault once, and start the agent (it prompts for the master
password); the MCP then connects to it:

```sh
revolutx vault init --key-file private.pem   # one-time, prompts for API key + password
revolutx agent start                         # read-only
# ...or, to permit order placement/cancellation through the MCP:
revolutx agent start --enable-trading        # REAL TRADING
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
listens somewhere other than the default. The agent serves a single client and
exits when it disconnects, so start it alongside the MCP; if the MCP restarts,
restart the agent too.

**Whether trading is allowed is the agent's decision** (`--enable-trading`), not
the MCP's — nothing in the MCP's environment can enable it.

## Tools

Read-only (always available): `get_balances`, `get_currencies`, `get_pairs`,
`get_tickers`, `get_order_book`, `get_public_order_book`, `get_candles`,
`get_last_trades`, `get_all_trades`, `get_private_trades`, `get_active_orders`,
`get_historical_orders`, `get_order`, `get_order_fills`.

Order mutation (only when the agent was started with `--enable-trading`):
`place_limit_order`, `place_market_order`, `cancel_order`, `cancel_all_orders`.

Tool results are returned as pretty-printed JSON of the corresponding SDK
response. Decimal values are preserved as strings (never floats).

## Safety

- The MCP holds **no key material** — the agent does all signing and HTTP, so a
  compromised MCP environment cannot leak credentials.
- Order-mutating tools are not even listed unless the agent enabled trading, and
  the agent itself refuses any order request when trading is off — the MCP cannot
  override that gate.
- All diagnostics go to stderr; stdout carries only the JSON-RPC protocol.

## License

Licensed under the [Apache License 2.0](../LICENSE).
