# revolutx-common

Shared helpers for the [`revolutx`](https://crates.io/crates/revolutx) interface
crates (the MCP server, the CLI, and any future ones).

It currently centralizes **credential and environment configuration**: reading
`REVOLUTX_API_KEY`, `REVOLUTX_PRIVATE_KEY_PEM` / `REVOLUTX_PRIVATE_KEY_PATH`, and
`REVOLUTX_ENVIRONMENT` (with explicit overrides, e.g. CLI flags, taking
precedence) and building a `revolutx::RevolutXClient`.

```rust,no_run
// Everything from the environment:
let client = revolutx_common::client_from_env()?;

// Or flags first, env as fallback:
use revolutx_common::ClientConfig;
let client = ClientConfig { api_key: Some("…".into()), ..Default::default() }
    .or_env()
    .build()?;
# Ok::<(), revolutx_common::ConfigError>(())
```

This crate is downstream of `revolutx`; the SDK's own examples and tests do not
use it (that would form a dependency cycle).

## License

Licensed under the [Apache License 2.0](../LICENSE).
