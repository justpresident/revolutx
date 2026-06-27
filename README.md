# revolutx

![CI](https://github.com/justpresident/revolutx/actions/workflows/ci.yml/badge.svg?branch=master)
![Coverage](https://raw.githubusercontent.com/justpresident/revolutx/master/.github/badges/coverage.svg)
[![Crates.io](https://img.shields.io/crates/v/revolutx.svg)](https://crates.io/crates/revolutx)
[![Docs.rs](https://docs.rs/revolutx/badge.svg)](https://docs.rs/revolutx)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Unofficial Rust SDK for the [Revolut X](https://exchange.revolut.com/) crypto
exchange REST API.

## **NOTE: This project is in active development stage - public API will likely change in next versions**

`revolutx` is a custom implementation of Revolut X API. It is not generated from OpenAPI,
but designed from scratch to provide clean, domain-oriented SDK aimed at trading bots and
automation. It models trading concepts — symbols, sides, orders, fills,
balances, order books, candles, tickers — as Rust types, signs requests
automatically with Ed25519, and never represents money or quantities as `f64`
(all decimals use [`rust_decimal`]).

> **Not affiliated with Revolut.** Trading automation carries financial risk.
> You are responsible for your own validation, risk controls, and credential
> security. This crate handles API access, typing, signing, and error reporting
> only — it does not provide trading strategy or risk management.

## Features

- Domain-oriented, handwritten API — not a generated OpenAPI client.
- Automatic Ed25519 request signing (`X-Revx-API-Key` / `X-Revx-Timestamp` /
  `X-Revx-Signature`); callers never build these headers.
- Decimal-safe values everywhere via `rust_decimal::Decimal` (re-exported as
  `revolutx::Decimal`).
- Full endpoint coverage: balances, configuration, market data, orders, and
  trades — verified against the OpenAPI spec by a drift test.
- Safe order builders that prevent obviously invalid requests.
- Typed errors that distinguish configuration, auth, rate-limit, and API errors.
- Async (`reqwest` + `rustls`), with optional unauthenticated access to the
  public market-data endpoints.

## Installation

```toml
[dependencies]
revolutx = "0.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Cargo features

| Feature | Default | What it enables |
|---|---|---|
| `rest` | ✅ | The REST API: Ed25519-signed HTTP client and all endpoint groups (pulls `reqwest` + Ed25519). |
| `fix` | — | FIX 4.4 client (market data + trading). **Reserved, not yet implemented.** |
| `keystore` | — | Encrypted credential vault exposed as a `Signer`, stored in [`rcypher`]'s multi-factor `SecretStore` format (Argon2id + AES-256-CBC + HMAC; password and/or FIDO2; manageable with the `rcypher` CLI). Implies `rest`. |
| `agent` | — | Signing-agent proxy (unix-only): a `serve()` daemon plus an `AgentExecutor` client, so a headless process can delegate signing + HTTP to an agent that holds the keystore. Implies `rest`. |

[`rcypher`]: https://crates.io/crates/rcypher

The domain models and error types are always available, independent of features.
A FIX-only consumer (e.g. a market maker) can drop the HTTP/TLS dependency tree
entirely:

```toml
revolutx = { version = "0.2", default-features = false, features = ["fix"] }
```

## Generating an API key

Create an Ed25519 key pair and register the public key in the
[Revolut X web app](https://exchange.revolut.com/). Generate one from the SDK:

```rust,no_run
let pair = revolutx::generate_key_pair()?;
println!("{}", pair.public_pem);  // register this public key in the web app
// Keep pair.private_pem secret — e.g. store it in the encrypted vault (the CLI).
# Ok::<(), revolutx::Error>(())
```

…or with `openssl`:

```sh
openssl genpkey -algorithm ed25519 -out private.pem
openssl pkey -in private.pem -pubout -out public.pem
```

Keep the private key secret. The SDK loads it via
[`ClientBuilder::private_key_pem`], or — encrypted at rest — from the vault
managed by [`revolutx-cli`](cli).

## Quick start

```rust,no_run
use revolutx::{Environment, RevolutXClient};

#[tokio::main]
async fn main() -> revolutx::Result<()> {
    let client = RevolutXClient::builder()
        .api_key("your-api-key")
        .private_key_pem(std::fs::read_to_string("private.pem").unwrap())
        .environment(Environment::Production)
        .build()?;

    // Account
    let balances = client.balances().get_all().await?;
    for b in &balances {
        println!("{}: {} available", b.currency, b.available);
    }

    // Market data
    let book = client.market_data().order_book("BTC-USD").await?;
    println!("{} bids / {} asks", book.bids.len(), book.asks.len());

    Ok(())
}
```

### Public market data (no credentials)

```rust,no_run
# async fn run() -> revolutx::Result<()> {
let client = revolutx::RevolutXClient::builder().build()?;
let book = client.market_data().public_order_book("BTC-USD").await?;
let last = client.market_data().last_trades().await?;
println!("{} bid levels, {} recent trades", book.bids.len(), last.trades.len());
# Ok(())
# }
```

### Placing orders

Order builders validate the request (positive price/size, non-empty symbol,
exactly one configuration and size) and sign it for you:

```rust,no_run
use revolutx::{Decimal, RevolutXClient};
use std::str::FromStr;

# async fn run(client: RevolutXClient) -> revolutx::Result<()> {
// Post-only limit buy of 0.1 BTC at 50,000.50 quote.
let ack = client
    .orders()
    .limit_buy("BTC-USD", Decimal::from_str("0.1").unwrap(), Decimal::from_str("50000.50").unwrap())
    .post_only()
    .send()
    .await?;
println!("order {} -> {:?}", ack.venue_order_id, ack.state);

// Market sell using a quote amount.
let ack = client
    .orders()
    .market_sell_quote("BTC-USD", Decimal::from_str("100").unwrap())
    .send()
    .await?;

// Manage orders.
let active = client.orders().active(&Default::default()).await?;
client.orders().cancel_all().await?;
# let _ = (ack, active);
# Ok(())
# }
```

### Error handling and rate limits

```rust,no_run
# async fn run(client: revolutx::RevolutXClient) {
match client.balances().get_all().await {
    Ok(balances) => println!("{} balances", balances.len()),
    Err(e) if e.is_rate_limited() => {
        if let Some(delay) = e.retry_after() {
            eprintln!("rate limited; retry after {delay:?}");
        }
    }
    Err(e) if e.is_auth_error() => eprintln!("auth problem: {e}"),
    Err(e) => eprintln!("request failed: {e}"),
}
# }
```

## Endpoint groups

| Group | Methods |
|-------|---------|
| `client.balances()` | `get_all` |
| `client.configuration()` | `currencies`, `pairs` |
| `client.market_data()` | `order_book`, `order_book_with_limit`, `public_order_book`, `candles`, `tickers`, `tickers_for`, `last_trades` |
| `client.orders()` | `limit_buy`/`limit_sell`(`_quote`), `market_buy`/`market_sell`(`_quote`), `place`, `replace`, `get`, `active`, `historical`, `cancel`, `cancel_all`, `fills` |
| `client.trades()` | `all`, `private` |

See [`docs/openapi-inventory.md`](docs/openapi-inventory.md) for the full
operation/schema mapping.

## Examples

Runnable examples live in [`examples/`](examples):

```sh
cargo run --example market_data -- BTC-USD          # public, no credentials
cargo run --example generate_keypair                # make an Ed25519 key to register
REVOLUTX_API_KEY=... cargo run --example get_balances
```

`examples/place_limit_order.rs` performs **real trading** and is guarded by
`REVOLUTX_CONFIRM_PLACE_ORDER=yes`.

## Related crates

- [`revolutx-cli`](cli) — a command-line interface over this SDK: an encrypted
  credential vault (rcypher `SecretStore`; password and/or FIDO2), every endpoint
  as a command, and a signing **agent** that holds the keystore so headless
  clients can delegate signing (`cargo install revolutx-cli`).
- [`revolutx-mcp`](mcp) — a Model Context Protocol (MCP) server that exposes
  this SDK to LLM clients such as Claude Desktop, talking to the agent over a
  one-time-token-authenticated socket (`cargo install revolutx-mcp`).

## Testing

The default test suite is fast, deterministic, and offline:

```sh
cargo test
```

Opt-in, read-only live smoke tests require credentials and are ignored by
default:

```sh
export REVOLUTX_API_KEY=... REVOLUTX_PRIVATE_KEY_PEM="$(cat private.pem)"
cargo test --test live_smoke -- --ignored --nocapture
```

## MSRV

Rust 1.87 (edition 2024). The `keystore` and `agent` features pull in
[`rcypher`], which requires 1.87.

## License

Licensed under the [Apache License 2.0](LICENSE).
