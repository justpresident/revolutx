# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/curriedsoftware/revolutx
