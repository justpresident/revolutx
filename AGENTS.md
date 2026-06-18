<!-- BEGIN TASKA INTEGRATION v5 hash:f517fefb -->
## Task tracking (taska)

This repo tracks work in a local, git-native store (`.taska/`) - drive it through the `ta` CLI, never hand-edit `.taska/` and never `git restore` it out from under in-flight work (either corrupts the append-only log). Field names, statuses, task types, and relationships are defined by `.taska/config.toml` and vary per repo, so run `ta prime` for THIS store's schema and copy-paste-ready examples, and `ta <command> --help` for a command's flags.

```bash
ta list --ready                     # actionable work: not done, all deps done
ta show <id> --full                 # one task - every field, full notes
ta create <id> <field>=<value> ...  # file new work (the status field defaults - don't set it)
ta update <id> <field>=<value> ...  # =, +=, -=  (set / append / remove)
ta dep add <id> <type>=<target>     # link a dependency
ta status                           # counts
```

Working habits:
- File a task for each distinct piece of work, before or as you start it, with `notes` rich enough for someone else to act on: the goal, intended approach/implementation details, and any open or design questions.
- For long or multi-line values, read from stdin (`<field>=@-`) or a file (`<field>=@FILE`) instead of quoting on the command line (`+=`/`-=` accept `@` too).
- Set prerequisites with `ta dep add`, and append progress to related tasks (`<field>+=...`) as things change so the trail stays current.
- Read a task's full, untruncated notes with `ta show <id> --full`.
- Commit the `.taska/` change in the same commit as the code it describes; if the store has pending changes unrelated to what you're starting, commit those first.
<!-- END TASKA INTEGRATION -->

## Project direction

This repository is for a public, generic Rust SDK for the Revolut X Crypto
Exchange REST API, suitable for trading-bot authors and other automation users.
The local `revolut-openapi` submodule is the source contract for Revolut API
shape, but the SDK itself should be a handwritten, idiomatic Rust API rather
than a generated public OpenAPI client.

For durable architecture rationale, read `docs/design.md`. This file is the
operational quick-start for agents; the design doc explains the why behind the
main decisions.

The key decision already made: build a clean bot-facing SDK by hand and use the
OpenAPI spec for validation/regression tests. Do not expose generated OpenAPI
types as the public API. If generation is introduced later, keep it internal
and subordinate to the stable domain API.

Relevant local context:
- Repo root: `/workspace/revolutx`
- OpenAPI submodule: `revolut-openapi`
- Revolut X JSON spec: `revolut-openapi/json/revolut-x.json`
- Revolut X YAML spec: `revolut-openapi/yaml/revolut-x.yml`
- Production base URL: `https://revx.revolut.com/api/1.0`
- Dev base URL: `https://revx.revolut.codes/api/1.0`
- Current spec is small enough for a handwritten SDK: about 18 operations and
  33 schemas.

## Architecture goals

Optimize for a public crate that is pleasant and safe to use from trading bots:
- Domain API first: expose concepts such as symbols, prices, quantities, sides,
  orders, fills, balances, order books, candles, tickers, and trades.
- Avoid raw strings where a domain type improves safety or clarity.
- Never use `f64` for prices, quantities, balances, fees, or order-book values.
  Use decimal-safe representations, likely `rust_decimal::Decimal`.
- Signing must be automatic; users should never manually construct
  `X-Revx-Timestamp` or `X-Revx-Signature`.
- Keep endpoint modules thin. They should use shared client/transport/auth/error
  infrastructure rather than duplicate HTTP or signing logic.
- Keep dependencies minimal and defensible for a public SDK.
- Default tests must be fast, deterministic, and offline. Live Revolut calls
  must be explicit opt-in/ignored.

Likely module shape:

```text
src/
  lib.rs
  client.rs
  auth.rs
  error.rs
  model/
    balances.rs
    configuration.rs
    market_data.rs
    orders.rs
    trades.rs
  api/
    balances.rs
    configuration.rs
    market_data.rs
    orders.rs
    trades.rs
```

This layout is guidance, not a hard requirement. Follow the existing code once
it exists, and prefer cohesive modules over a mechanically generated structure.

## Authentication requirements

Revolut X uses custom Ed25519 request signing.

Every authenticated request must include:
- `X-Revx-API-Key`
- `X-Revx-Timestamp`
- `X-Revx-Signature`

The signature message is the exact concatenation of:
1. timestamp in Unix epoch milliseconds
2. uppercase HTTP method
3. request path starting from `/api/1.0`
4. query string without the leading `?`, if present
5. minified JSON body, if present

Sign the message with the user's Ed25519 private key, then base64-encode the
signature bytes. The body bytes used for signing must be exactly the bytes sent
on the wire. Query ordering must also match what is sent.

The OpenAPI spec describes generating keys with:

```bash
openssl genpkey -algorithm ed25519 -out private.pem
openssl pkey -in private.pem -pubout -out public.pem
```

The SDK should support loading that private PEM format. Consider accepting raw
key bytes as an additional constructor if it helps tests or advanced users.

## Endpoint coverage

Track implementation against the OpenAPI spec. At the time this guidance was
written, the observed operations were:

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

Add a spec-drift/coverage test early. It should parse
`revolut-openapi/json/revolut-x.json` and fail when operations are added,
removed, or renamed without a conscious SDK update.

## Development plan

The taska backlog is the source of truth for planned work. Start with:

```bash
ta list --ready
ta show rx-002 --full
ta dep tree
```

The current high-level plan is:
1. Scaffold the crate and module layout.
2. Inventory the OpenAPI contract and add coverage tracking.
3. Build shared error, transport, and Ed25519 auth layers.
4. Define decimal-safe domain models.
5. Add spec-backed serialization/deserialization fixtures.
6. Implement endpoint families incrementally: balances/configuration, market
   data, order builders, order management, trades.
7. Add mock HTTP tests, opt-in live smoke tests, docs/examples, CI, and
   publishing metadata.

When implementing a task, read the task's full notes first. They contain design
context and acceptance criteria intended for another agent to execute without
reconstructing prior discussion.

## Public API style

Prefer a user-facing API like:

```rust
let client = RevolutXClient::builder()
    .api_key(api_key)
    .private_key_pem(private_key_pem)
    .environment(Environment::Production)
    .build()?;

let balances = client.balances().get_all().await?;
let book = client.market_data().order_book("BTC-USD").await?;
```

For orders, prefer safe builders or constructors that prevent impossible
requests:

```rust
let order = client
    .orders()
    .limit_buy("BTC-USD", size, price)
    .client_order_id(client_order_id)
    .send()
    .await?;
```

Do not blindly mirror OpenAPI request structs in public APIs when a better
domain method is clearer. Keep lower-level escape hatches only if there is a
real use case and they do not compromise the main API.

## Testing expectations

Default `cargo test` should be offline and deterministic:
- Unit-test signing message construction and signatures with deterministic keys
  and timestamps.
- Unit-test path joining, query serialization, minified JSON body handling, and
  error classification.
- Deserialize/serialize official examples from the OpenAPI spec where possible.
- Use mock/local HTTP tests for endpoint methods.
- Gate live API tests behind `#[ignore]`, a feature flag, or explicit
  environment variables. Live tests must never place or cancel orders unless
  separately and very explicitly opted in.

## Dependency guidance

Keep the crate lean. Expected dependencies may include:
- `reqwest` with rustls for HTTP
- `serde` and `serde_json`
- `thiserror`
- `ed25519-dalek`
- `base64`
- `rust_decimal`
- `time` or `chrono`
- `uuid` only if useful for client order IDs or spec compatibility

Do not add a heavy OpenAPI parser or generator unless a task explicitly needs
it. Raw `serde_json` is enough for spec inventory and coverage tests.
