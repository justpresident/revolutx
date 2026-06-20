//! Shared helpers for the `revolutx` interface crates (MCP, CLI, …).
//!
//! This crate centralizes logic that every interface needs but that doesn't
//! belong in the SDK itself — starting with reading credentials and the target
//! environment from CLI flags / `REVOLUTX_*` environment variables and building
//! a [`revolutx::RevolutXClient`].
//!
//! It is *downstream* of `revolutx`: the SDK's own examples and tests cannot use
//! it (that would form a dependency cycle), so they keep their inline loaders.

mod config;

pub use config::{ClientConfig, ConfigError, client_from_env};
