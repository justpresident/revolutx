//! Unofficial Rust SDK for the [Revolut X](https://exchange.revolut.com/) crypto
//! exchange REST API.
//!
//! `revolutx` is a handwritten, domain-oriented SDK aimed at trading bots and
//! automation. The local `OpenAPI` specification is used as a contract and
//! regression-test source, but generated `OpenAPI` types are **not** part of the
//! public API. Prices, quantities, balances, and fees use
//! [`rust_decimal::Decimal`] (re-exported as [`Decimal`]); they are never `f64`.
//!
//! # Authentication
//!
//! Authenticated requests are signed automatically with Ed25519 — callers never
//! construct the `X-Revx-Timestamp` or `X-Revx-Signature` headers themselves.
//! Generate a key pair with:
//!
//! ```sh
//! openssl genpkey -algorithm ed25519 -out private.pem
//! openssl pkey -in private.pem -pubout -out public.pem
//! ```
//!
//! # Example
//!
//! ```no_run
//! use revolutx::{Environment, RevolutXClient};
//!
//! # async fn run() -> revolutx::Result<()> {
//! let client = RevolutXClient::builder()
//!     .api_key("your-api-key")
//!     .private_key_pem(std::fs::read_to_string("private.pem").unwrap())
//!     .environment(Environment::Production)
//!     .build()?;
//!
//! // Read-only: fetch balances and an order book.
//! let balances = client.balances().get_all().await?;
//! let book = client.market_data().order_book("BTC-USD").await?;
//! println!("{} balances, {} bid levels", balances.len(), book.bids.len());
//! # Ok(())
//! # }
//! ```
//!
//! Public market data needs no credentials:
//!
//! ```no_run
//! # async fn run() -> revolutx::Result<()> {
//! let client = revolutx::RevolutXClient::builder().build()?;
//! let book = client.market_data().public_order_book("BTC-USD").await?;
//! println!("best bid level count: {}", book.bids.len());
//! # Ok(())
//! # }
//! ```
//!
//! # Disclaimer
//!
//! This crate is not affiliated with Revolut. Trading automation carries
//! financial risk; callers are responsible for their own validation, risk
//! controls, and credential security. The SDK handles API access, typing,
//! signing, and error reporting only — it does not provide trading strategy or
//! risk management.
#![warn(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::dbg_macro,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate
)]
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::multiple_crate_versions,
    clippy::missing_panics_doc
)]

// The REST client (default `rest` feature): HTTP transport, Ed25519 signing,
// and the endpoint groups. A build without `rest` (e.g. a future FIX-only
// build) drops the `reqwest`/Ed25519 dependency tree and exposes only the
// shared domain models and error types.
#[cfg(feature = "rest")]
pub mod api;
#[cfg(feature = "rest")]
pub mod client;
#[cfg(feature = "rest")]
pub mod config;
pub mod error;
#[cfg(feature = "keystore")]
pub mod keystore;
pub mod model;

/// FIX 4.4 client (market data and trading). Enabled by the `fix` feature.
///
/// Not yet implemented — this module is a placeholder so the feature and module
/// layout are in place ahead of the FIX work (see the `fix` backlog task).
#[cfg(feature = "fix")]
pub mod fix;

#[cfg(feature = "rest")]
mod auth;
#[cfg(feature = "rest")]
mod keygen;
#[cfg(feature = "rest")]
pub mod transport;

#[cfg(feature = "agent")]
pub mod agent;

#[cfg(feature = "agent")]
pub use agent::{AgentExecutor, AuthToken, default_socket_path, serve};
#[cfg(feature = "rest")]
pub use auth::{Ed25519Signer, RequestAuth, Signer};
#[cfg(feature = "rest")]
pub use client::{ClientBuilder, Environment, RevolutXClient};
#[cfg(feature = "rest")]
pub use config::{ClientConfig, ConfigError, client_from_env};
pub use error::{ApiError, ApiErrorKind, Error, Result};
#[cfg(feature = "rest")]
pub use keygen::{GeneratedKeyPair, generate_key_pair};
#[cfg(feature = "keystore")]
pub use keystore::{Keystore, KeystoreError};
pub use model::Page;
#[cfg(feature = "rest")]
pub use model::RawPage;
pub use model::common::{ClientOrderId, OrderId, Price, Quantity, Side, Symbol, Timestamp};
pub use rust_decimal::Decimal;
#[cfg(feature = "rest")]
pub use transport::{LocalExecutor, RawResponse, RequestExecutor, RequestSpec};

#[cfg(all(test, feature = "rest"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn exposes_client_builder() {
        let builder = RevolutXClient::builder();
        assert_eq!(builder.selected_environment(), Environment::Production);
    }

    #[test]
    fn public_client_builds_without_credentials() {
        let client = RevolutXClient::builder().build().unwrap();
        assert!(!client.is_authenticated());
    }
}
