//! Endpoint groups.
//!
//! Each endpoint group should expose a small domain-oriented surface and defer
//! shared HTTP, auth, and error handling to the client/transport layer.

pub mod balances;
pub mod configuration;
pub mod market_data;
pub mod orders;
pub mod trades;
