# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo features: `rest` (default) gates the REST client; `fix` is reserved for
  a future FIX 4.4 client. The REST dependency tree (`reqwest`, Ed25519) is now
  optional, so a `default-features = false, features = ["fix"]` build drops it.
  The domain models and error types remain available regardless of features.
- `Serialize` for the response wrapper types `OrderBook`, `Tickers`,
  `LastTrades`, and `Page<T>` (they were `Deserialize`-only in 0.1.0), so every
  response model now round-trips through serde.
- `config` module (`ClientConfig`, `client_from_env`) for building a client from
  CLI flags / `REVOLUTX_*` environment variables, under the `rest` feature.
- `keystore` feature: an `rcypher`-backed encrypted credential vault
  (`Keystore`, Argon2id + AES-256-CBC + HMAC) that implements `Signer` by
  decrypting → signing → zeroizing on **every** request, so credentials are
  never held decrypted between requests. Re-exports rcypher's process-hardening
  helpers for the unlocking binary to call. Optional dependency, gated.
- Pluggable auth and transport seams: a `Signer` trait (default `Ed25519Signer`,
  set via `ClientBuilder::signer`) authenticating each request through a single
  `authenticate()` call (so a decrypting signer needs one decrypt per request),
  and a `RequestExecutor` trait (default `LocalExecutor`, set via
  `ClientBuilder::executor` / `RevolutXClient::with_executor`) for custom
  execution backends (e.g. a signing agent in another process). The `transport`
  module, `RequestSpec`, and `RawResponse` are now public. `RequestAuth.api_key`
  is `Zeroizing<String>` and `ed25519-dalek`'s `zeroize` is enabled, so
  credentials are wiped from
  memory on drop.

## [0.1.0] - 2026-06-19

Initial release. Licensed under Apache-2.0; MSRV 1.85 (edition 2024).

### Added

- Initial public SDK for the Revolut X Crypto Exchange REST API.
- `RevolutXClient` with a builder: API key + Ed25519 private key (PEM or raw
  seed), environment selection (production/dev), custom base URL, timeout, and
  custom `reqwest::Client`. Clients may be built without credentials for public
  market data.
- Automatic Ed25519 request signing isolated in the auth layer.
- Endpoint groups: `balances`, `configuration`, `market_data`, `orders`,
  `trades` — covering all 18 spec operations.
- Safe order placement builders (`limit_buy`/`limit_sell`(`_quote`),
  `market_buy`/`market_sell`(`_quote`)) plus full order management.
- Decimal-safe domain types (`Decimal`, `Price`, `Quantity`, `Symbol`, `Side`,
  `OrderId`, `ClientOrderId`, `Timestamp`) and response models validated against
  the OpenAPI examples.
- Typed error model with rate-limit and auth-error helpers.
- OpenAPI coverage drift guard, spec-backed fixtures, offline mock-HTTP endpoint
  tests, and opt-in read-only live smoke tests.

[0.1.0]: https://github.com/justpresident/revolutx/releases/tag/v0.1.0
