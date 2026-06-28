# Changelog — revolutx (library)

All notable changes to the `revolutx` library are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The binaries track their changes in their own changelogs:
[`cli/CHANGELOG.md`](cli/CHANGELOG.md) (`revolutx-cli`) and
[`mcp/CHANGELOG.md`](mcp/CHANGELOG.md) (`revolutx-mcp`). Through 0.3.0, library and
binary changes were logged together in this file.

## [Unreleased]

## [0.3.0] - 2026-06-28

Builds three consistent surfaces on the 0.2 SDK — a shared command layer, an
interactive CLI shell, and an MCP server — hardens the signing agent with peer
authentication, and replaces the agent's trading on/off switch with a
`market | view | trading` access ladder.

### Added

- **Shared command layer** (`commands` feature, opt-in): a parse-neutral
  `Command` model, a single `execute` dispatcher onto a `RevolutXClient`, a
  structured `CommandOutput` (serializes to the bare SDK response), and a
  `Presenter` seam with a shared `JsonPresenter`. Adds no dependencies. All three
  surfaces — the CLI one-shot path, the interactive shell, and the MCP — parse and
  dispatch through it and differ only in presentation.
- **`revolutx cli` interactive shell**: unlocks the vault once, then a REPL
  running the same commands with history, line editing, and Tab-completion of
  commands and trading symbols. Real-trading commands prompt for confirmation
  instead of requiring `--yes`; `market watch` streams until Ctrl-C.
- **Signing-agent peer authentication.** The agent authenticates the connecting
  peer with a one-time, high-entropy token before exposing the signing oracle,
  closing the gap where another same-UID process could race to the `0600` socket
  and trade as you. `AuthToken` (generate / constant-time verify) is exported from
  the `agent` feature.
- **`--access market | view | trading` capability tiers** (`revolutx::access`): a
  cumulative ladder — `market` (public market data only), `view` (adds read-only
  account data: balances, your orders/trades, fills), `trading` (adds order
  placement and cancellation). `revolutx agent start --access` sets the tier the
  agent serves and enforces as the authoritative gate (default `market`, least
  privilege); `revolutx cli --access` gates the shell locally (default `view`) so
  an agent policy can be rehearsed. Out-of-tier requests are refused with a message
  naming the tier needed. One-shot CLI commands are run by the credential owner and
  are not gated.
- CLI parity flags so its commands match the shared model: `--side`/`--cursor` on
  `orders active`, dates + `--cursor` on `orders historical` and `trades`, and
  `--client-order-id` on `orders limit`/`market`. Date/time flags accept human
  forms (`2024-01-31`, `"2024-01-31 14:30"`, `7d`, RFC 3339) as well as epoch
  milliseconds.

### Changed

- **Credential vault adopts rcypher 0.3's `SecretStore` format.** The vault is now
  rcypher's standard multi-factor store, so the same file can be inspected and
  managed with the `rcypher` CLI — enrol a FIDO2 security key, change the
  password, set a multi-factor policy (`pass or key`, `pass and key`, …), or
  rotate records. Each value is encrypted under the store's data key *inside* the
  encrypted container ("double" encryption), and the `Keystore` `Signer` decrypts
  one record per request as before. **No migration:** old-format vaults are not
  read; re-run `revolutx vault init`.
- `revolutx-cli` gains a default-on `fido2` feature (`--no-default-features` for
  hosts without `libudev`/`hidapi`); vault unlock now drives rcypher's interactive
  multi-factor loop (password and/or security key).
- `generate_key_pair` / `GeneratedKeyPair` moved from the `keystore` feature to
  `rest` (they are pure Ed25519, no rcypher), so generating a signing key no
  longer pulls the encrypted-vault stack. It now returns `Error::KeyGeneration`.
- The agent accepts connections **concurrently** and holds each unauthenticated:
  the only request an unauthenticated peer may issue is `Authenticate`, and the
  token is consumed on first valid use, so exactly one client can authenticate.
  Its capabilities (base URL, access tier) are revealed only in the authentication
  reply — an unauthenticated peer learns nothing. `on_connect` (the watchdog's
  idle-lock cancel) fires on authentication, not on TCP accept.
- `revolutx agent start` requires `--auth-token`: it prints the one-time token for
  the operator to hand to the client out of band (never accepted as an argument
  value, which would expose it via `/proc` and `ps`).
- **`revolutx-mcp` runs on the shared command layer and connects to the agent
  lazily.** Each tool maps onto a shared `Command`, dispatched through `execute` +
  `JsonPresenter` (byte-identical JSON to the CLI `--json`). The server no longer
  connects at startup: it opens the socket only when the `authenticate` tool is
  called, and reconnects on each call — so the agent can be started, stopped, or
  restarted independently, and the client just re-runs `authenticate` with the new
  token. All tools are advertised unconditionally; the agent enforces the
  `--access` gate.
- MSRV is **1.87** (the `Cargo.toml` `rust-version`): the `keystore`/`agent`
  features depend on `rcypher`, which uses `const fn` over `Vec::len` (stable
  since Rust 1.87). 0.2.0 shipped declaring `1.85` by mistake — it does not build
  on 1.85 with those features.

### Removed

- The agent's `--enable-trading` flag, superseded by `--access trading` (the tier
  ladder also gates account reads behind `--access view`, which the boolean did
  not).
- `KeystoreOptions` — the vault cipher's trace-detection is now rcypher's (it
  allows the hardening watchdog and refuses only a foreign debugger), so
  `--insecure-allow-debugging` no longer toggles it.

## [0.2.0] - 2026-06-22

Adds optional credential-security features — an encrypted keystore and a
full-proxy signing agent — on top of the REST client, and tightens several
response-model and error-handling behaviors. This is the `revolutx` version the
`revolutx-cli` and `revolutx-mcp` binary crates depend on.

### Added

- Cargo features: `rest` (default) gates the REST client; `fix` is reserved for a
  future FIX 4.4 client. The REST dependency tree (`reqwest`, Ed25519) is now
  optional, so a `default-features = false, features = ["fix"]` build drops it;
  the domain models and error types remain available regardless of features.
- `config` module (`ClientConfig`, `client_from_env`) for building a client from
  CLI flags / `REVOLUTX_*` environment variables, under the `rest` feature.
  `ClientConfig`'s `Debug` redacts the API key and private key.
- Pluggable auth and transport seams: a `Signer` trait (default `Ed25519Signer`,
  set via `ClientBuilder::signer`) that authenticates each request through a
  single `authenticate()` call (so a decrypting signer needs one decrypt per
  request), and a `RequestExecutor` trait (default `LocalExecutor`, set via
  `ClientBuilder::executor` / `RevolutXClient::with_executor`) for custom
  execution backends. The `transport` module, `RequestSpec`, and `RawResponse`
  are now public. `RequestAuth::api_key` is `Zeroizing<String>` and
  `ed25519-dalek`'s `zeroize` is enabled, so key material is wiped on drop.
- `keystore` feature: an `rcypher`-backed encrypted credential vault (Argon2id +
  AES-256-CBC + HMAC). `Keystore` is an in-memory record store —
  `open`/`open_with` initialize it from a master password (starting an empty
  vault if the file is absent), `get`/`set` read/write individual records
  (decrypting transiently and zeroizing the plaintext), and `save` writes the
  encrypted blob to disk atomically (`0600`). It implements `Signer`, reading the
  `Keystore::API_KEY` and `PRIVATE_KEY_PEM` records and zeroizing the decrypted
  contents on every request. `generate_key_pair` (+ `GeneratedKeyPair`) creates a
  fresh Ed25519 key pair (PKCS#8 private PEM, SPKI public PEM) without touching
  the disk. rcypher's process-hardening helpers (`disable_core_dumps`,
  `enable_ptrace_protection`, `is_debugger_attached`) are re-exported for the
  unlocking binary to call. Optional, gated; implies `rest`.
- `agent` feature (unix-only): a full-proxy signing agent. `serve()` runs a
  daemon that owns the keystore and performs all signing **and** HTTP; a
  client-side `AgentExecutor` (plug into `ClientBuilder::executor`) forwards
  `RequestSpec`s over a unix socket and receives only response bytes — neither
  the private key nor the API key crosses the socket. A capabilities handshake
  reports the agent's base URL and trading policy; the agent serves a single
  client, refuses the rest, and authoritatively gates order mutations.
  `default_socket_path` gives the conventional socket location. Optional, gated;
  implies `rest`.
- `RawPage<T>` — the raw `{ data, metadata }` pagination envelope the API
  returns, converted into the flat `Page<T>` at the endpoint boundary.
- Forward-compatible response enums: `OrderType`, `OrderStatus`, `TimeInForce`,
  `ExecutionInstruction`, `AssetType`, and `ListingStatus` gain an `Unknown`
  catch-all (`#[serde(other)]`), so a new server value no longer fails the whole
  response parse.

### Changed

- `Page<T>` is now a flat domain struct that serializes and deserializes through
  its own fields (it round-trips); the API's wire envelope is the separate
  `RawPage<T>`. (In 0.1.0 `Page` was `Deserialize`-only and read the envelope
  directly.)
- Error classification: a `4xx`/`5xx` response now maps to a structured
  `Error::Api` by status code even when the body is not the expected JSON shape,
  and preserves the parsed `Retry-After`. `is_rate_limited()`, `is_auth_error()`,
  and `retry_after()` therefore fire on unstructured error bodies too.

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

[0.2.0]: https://github.com/justpresident/revolutx/releases/tag/v0.2.0
[0.1.0]: https://github.com/justpresident/revolutx/releases/tag/v0.1.0
