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

The server is configured entirely through environment variables:

| Variable | Purpose |
|---|---|
| `REVOLUTX_API_KEY` | API key. Optional — without credentials only the public tools work. |
| `REVOLUTX_PRIVATE_KEY_PEM` | Ed25519 private key (PKCS#8 PEM contents). |
| `REVOLUTX_PRIVATE_KEY_PATH` | Path to the PEM file (used if `_PEM` is unset). |
| `REVOLUTX_ENVIRONMENT` | `production` (default) or `dev`. |
| `REVOLUTX_MCP_ENABLE_TRADING` | Set to `1`/`true` to expose the order-mutating tools. Default: off (read-only). |

Generate a key pair with:

```sh
openssl genpkey -algorithm ed25519 -out private.pem
openssl pkey -in private.pem -pubout -out public.pem
```

### Claude Desktop

Add to `claude_desktop_config.json` (read-only by default):

```json
{
  "mcpServers": {
    "revolutx": {
      "command": "revolutx-mcp",
      "env": {
        "REVOLUTX_API_KEY": "your-api-key",
        "REVOLUTX_PRIVATE_KEY_PATH": "/absolute/path/to/private.pem"
      }
    }
  }
}
```

To allow placing and cancelling orders, add `"REVOLUTX_MCP_ENABLE_TRADING": "1"`
to `env` — only do this if you understand it performs real trades.

## Tools

Read-only (always available): `get_balances`, `get_currencies`, `get_pairs`,
`get_tickers`, `get_order_book`, `get_public_order_book`, `get_candles`,
`get_last_trades`, `get_all_trades`, `get_private_trades`, `get_active_orders`,
`get_historical_orders`, `get_order`, `get_order_fills`.

Order mutation (only when `REVOLUTX_MCP_ENABLE_TRADING` is set):
`place_limit_order`, `place_market_order`, `cancel_order`, `cancel_all_orders`.

Tool results are returned as pretty-printed JSON of the corresponding SDK
response. Decimal values are preserved as strings (never floats).

## Safety

- Order-mutating tools are not even listed unless trading is explicitly enabled,
  and calling one while disabled returns an error instead of trading.
- The public market-data tools (`get_public_order_book`, `get_last_trades`) work
  without any credentials.
- All diagnostics go to stderr; stdout carries only the JSON-RPC protocol.

## License

Licensed under the [Apache License 2.0](../LICENSE).
