# Contributing to revolutx

Thanks for your interest in improving `revolutx`.

## Getting started

The OpenAPI contract is a git submodule, so clone with submodules (the
spec-backed tests read it):

```sh
git clone --recurse-submodules <repo-url>
# or, in an existing checkout:
git submodule update --init
```

## Before opening a pull request

Run the same checks CI does:

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo doc --no-deps
```

The default test suite is offline and deterministic — it must never require
credentials or network access. Gate any live test behind `#[ignore]` and
environment variables (see `tests/live_smoke.rs`). No test may place, replace,
or cancel orders.

## Guidelines

- Keep the public API handwritten and domain-oriented. Do not expose generated
  OpenAPI types.
- Never use `f64` for prices, quantities, balances, fees, or order-book values —
  use `rust_decimal::Decimal`.
- Keep request signing automatic and confined to the auth layer.
- When the OpenAPI spec changes, update the coverage list in
  `tests/spec_coverage.rs` and add fixtures/tests for new operations.
- Add a regression test for every bug fixed or API mismatch discovered.
- Keep dependencies minimal and justified.

## Architecture

See `docs/design.md` for the rationale and `docs/openapi-inventory.md` for the
operation/schema-to-model mapping.
