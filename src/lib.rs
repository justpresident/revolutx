//! Unofficial Rust SDK for the Revolut X Crypto Exchange REST API.
//!
//! The crate is designed as a handwritten, domain-oriented SDK for trading bots
//! and automation. The local OpenAPI specification is used as a contract and
//! regression-test source, but generated OpenAPI types are not part of the
//! public API.
//!
//! Trading automation carries financial risk. This crate handles API access,
//! typing, request signing, and error reporting; it does not provide trading
//! strategy or risk management.

pub mod api;
pub mod auth;
pub mod client;
pub mod error;
pub mod model;

pub use client::{ClientBuilder, Environment, RevolutXClient};
pub use error::{Error, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_client_builder() {
        let builder = RevolutXClient::builder();
        assert_eq!(builder.selected_environment(), Environment::Production);
    }
}
