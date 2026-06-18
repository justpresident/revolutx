# revolutx

Unofficial Rust SDK for the Revolut X Crypto Exchange REST API.

This crate is planned as a handwritten, domain-oriented SDK for trading bots
and automation. The local `revolut-openapi` submodule is used as the API
contract and test source, but generated OpenAPI types should not become the
public API.

Development context lives in:

- `AGENTS.md` for the operational handoff.
- `docs/design.md` for architecture rationale.
- `ta list --ready` for the executable task backlog.

The SDK is not affiliated with Revolut. Trading automation carries financial
risk; callers are responsible for their own validation, risk controls, and
credential security.

