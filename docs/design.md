# RevolutX Rust SDK Design

## Purpose

This repository is intended to become a public Rust SDK for the Revolut X Crypto
Exchange REST API. The primary audience is developers building trading bots,
automation, dashboards, and other exchange integrations.

The SDK should be generic and production-quality, but it should not pretend to
manage trading risk for users. It should make API access correct, typed,
testable, and ergonomic.

## Core Decision

Build a handwritten public SDK and use the OpenAPI spec as a contract and test
source.

The local `revolut-openapi` submodule remains important. It defines the HTTP
contract: paths, methods, request bodies, query parameters, response schemas,
examples, servers, and authentication requirements. The SDK should conform to
that contract, but the public Rust API should be designed around trading-domain
concepts rather than generated OpenAPI shapes.

This means:
- Do not expose generated OpenAPI types as the public API.
- Do not start with a full generated OpenAPI client.
- Keep generation optional and internal if it is introduced later.
- Use spec-backed tests to catch drift between Revolut's API definition and the
  handwritten SDK.

## Why Not A Generated Public Client

Generated clients are useful for broad mechanical coverage, but they tend to
mirror the HTTP document instead of the user's domain.

For trading bot authors, a generated API often exposes too many raw strings,
optional fields, weakly modeled request variants, and transport details. It can
also make RevolutX authentication awkward because the signature depends on the
exact timestamp, method, path, query string, and minified JSON body that are sent
on the wire.

A public SDK should make the common correct path obvious:

```rust
let client = RevolutXClient::builder()
    .api_key(api_key)
    .private_key_pem(private_key_pem)
    .environment(Environment::Production)
    .build()?;

let balances = client.balances().get_all().await?;
let book = client.market_data().order_book("BTC-USD").await?;
```

For orders, the API should guide users toward valid requests:

```rust
let order = client
    .orders()
    .limit_buy("BTC-USD", size, price)
    .client_order_id(client_order_id)
    .send()
    .await?;
```

That shape is hard to get from generic OpenAPI generation.

## Rejected Alternatives

### Full OpenAPI-generated public client

Pros:
- Fastest initial endpoint coverage.
- Mechanical alignment with the spec.

Cons:
- Poorer user experience for trading workflows.
- Generated naming and optionality become public API commitments.
- Custom Ed25519 signing is difficult to model cleanly.
- Domain invariants are harder to enforce.

### Generated internal DTOs first

Pros:
- Can reduce manual schema drift.
- Keeps the generated surface away from users if carefully contained.

Cons:
- Adds mapping layers before they are clearly needed.
- Generated shapes can still leak into internal design.
- RevolutX is small enough that handwritten models plus fixture tests are
  practical.

This can be reconsidered later if schema drift becomes expensive.

### Extending the nearby `/workspace/revolut` crate

Pros:
- Existing local Rust Revolut code and codegen patterns.
- Potentially one broader Revolut crate.

Cons:
- RevolutX has different authentication.
- A trading SDK benefits from a focused API and crate identity.
- The goal here is a clean public RevolutX implementation.

## API Design Principles

The public API should optimize for clarity, correctness, and bot ergonomics.

Prefer endpoint groups:
- `client.balances()`
- `client.configuration()`
- `client.market_data()`
- `client.orders()`
- `client.trades()`

Prefer domain types where they improve safety:
- `Symbol`
- `Price`
- `Quantity`
- `Side`
- `OrderId`
- `ClientOrderId`
- `OrderStatus`
- `OrderBookLevel`
- `Candle`
- `Trade`
- `Fill`

Avoid `f64` for any exchange value involving money, price, quantity, balance,
fee, or order-book size. Use decimal-safe values, likely
`rust_decimal::Decimal`, and serialize them in the string format expected by the
API.

Constructors and builders should prevent obvious invalid states. For example,
normal public APIs should not allow users to create an order request with no
order configuration, both mutually exclusive size fields, or a non-positive
price.

Do not overuse type-state builders unless they clearly improve safety without
making the API hard to read. Simple constructors and consuming builders are
preferred.

## Authentication Design

RevolutX uses custom Ed25519 signing, not a simple bearer token.

Every authenticated request must include:
- `X-Revx-API-Key`
- `X-Revx-Timestamp`
- `X-Revx-Signature`

The signed message is the exact concatenation of:
1. timestamp in Unix epoch milliseconds
2. uppercase HTTP method
3. request path starting from `/api/1.0`
4. query string without the leading `?`, if present
5. minified JSON body, if present

The signature is produced with the user's Ed25519 private key and then
base64-encoded.

Important implementation rule: the JSON bytes used for signing must be the same
bytes sent on the wire. The query string used for signing must be the same query
string sent on the wire, in the same order.

Auth should be isolated in `auth.rs` and shared by every endpoint. Endpoint code
should never ask users to provide timestamps or signatures manually.

The SDK should support private PEM files generated by the documented OpenSSL
flow:

```bash
openssl genpkey -algorithm ed25519 -out private.pem
openssl pkey -in private.pem -pubout -out public.pem
```

An additional raw-key constructor may be useful for tests or advanced users, but
the PEM path should be first-class.

## Transport Design

The transport layer should be responsible for:
- base URL selection
- path joining
- query serialization
- JSON body serialization
- signing hook
- sending via `reqwest`
- response status classification
- response deserialization

Endpoint modules should be thin and domain-focused. They should call a shared
internal request abstraction instead of reimplementing HTTP details.

Support at least:
- production environment
- dev environment
- custom base URL for tests or advanced deployments
- optional custom `reqwest::Client`
- sensible timeout configuration

## Error Design

The public error model should help bot authors decide what to do next.

Represent at least:
- configuration/build errors
- private key parsing errors
- signing errors
- HTTP transport errors
- request serialization errors
- response deserialization errors
- Revolut API error responses
- rate-limit responses
- unexpected status/body shapes

Preserve useful context: HTTP status, method/path, parsed error payload when
available, and body text when parsing fails. Avoid panics for normal API
failures.

## Spec And Endpoint Coverage

Use `revolut-openapi/json/revolut-x.json` as the local source of truth.

At the time this document was written, the observed operations were:

```text
GET    /balances
GET    /configuration/currencies
GET    /configuration/pairs
POST   /orders
DELETE /orders
GET    /orders/active
GET    /orders/historical
GET    /orders/{venue_order_id}
DELETE /orders/{venue_order_id}
PUT    /orders/{venue_order_id}
GET    /orders/fills/{venue_order_id}
GET    /trades/all/{symbol}
GET    /trades/private/{symbol}
GET    /order-book/{symbol}
GET    /candles/{symbol}
GET    /tickers
GET    /public/last-trades
GET    /public/order-book/{symbol}
```

Add a fast test that parses the spec and compares the current operation set to
an explicit SDK coverage list. The test should fail when Revolut adds, removes,
or renames an operation without a conscious SDK update.

This test is not a replacement for endpoint tests. It is a drift alarm.

## Testing Strategy

Default tests must be fast, deterministic, and offline.

Test layers:
- Unit tests for domain type validation and decimal serialization.
- Unit tests for signing message construction and Ed25519 signatures using a
  deterministic key and timestamp.
- Unit tests for base URL/path/query/body construction.
- Response/error parser tests for common status classes.
- Spec-backed fixture tests using official examples from
  `revolut-openapi/json/revolut-x.json`.
- Mock/local HTTP tests for endpoint methods, asserting method, path, query,
  body, and auth headers.
- Ignored or feature-gated live smoke tests for real credentials.

Live tests must never run by default. Read-only live tests are acceptable behind
an explicit opt-in. Any test that places, replaces, or cancels orders must have a
separate and very explicit dangerous opt-in.

## Dependency Policy

Keep dependencies minimal and defensible. Expected dependencies may include:
- `reqwest` with rustls for HTTP
- `serde` and `serde_json`
- `thiserror`
- `ed25519-dalek`
- `base64`
- `rust_decimal`
- `time` or `chrono`
- `uuid` only if useful for client order IDs or spec compatibility

Avoid heavy OpenAPI parsing or generation dependencies unless a task proves the
need. Raw `serde_json` is enough for spec inventory and coverage tests.

## Release Quality Bar

Before public release:
- `cargo fmt --check` passes.
- `cargo clippy --all-targets --all-features -- -D warnings` passes.
- `cargo test --all-features` passes without secrets or live network calls.
- docs build cleanly.
- README and examples show read-only usage and order usage with clear risk
  warnings.
- crate metadata is complete.
- `cargo package --list` includes only intended files.
- `cargo publish --dry-run` succeeds.

## Open Questions

These should be resolved during implementation:
- Final crate name: `revolutx` versus `revolut-x`.
- Whether to use `time` or `chrono`.
- Whether `ClientOrderId` should be UUID-specific or a more general string
  wrapper.
- Whether to expose a custom signer trait for HSM/remote-signing users.
- Whether to include a low-level raw request escape hatch.
- How much local validation to perform for symbols and exchange constraints
  versus relying on server-side validation.
