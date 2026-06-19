//! Account balance models.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A single currency balance for the account.
///
/// All amounts are returned by the exchange as decimal strings and parsed into
/// [`Decimal`] to avoid floating-point rounding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    /// The currency this balance is denominated in, e.g. `BTC`.
    pub currency: String,
    /// Available (free) funds.
    #[serde(with = "rust_decimal::serde::str")]
    pub available: Decimal,
    /// Staked funds currently earning rewards, when present.
    #[serde(
        with = "rust_decimal::serde::str_option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub staked: Option<Decimal>,
    /// Reserved (locked) funds.
    #[serde(with = "rust_decimal::serde::str")]
    pub reserved: Decimal,
    /// Sum of available, reserved, and staked funds.
    #[serde(with = "rust_decimal::serde::str")]
    pub total: Decimal,
}
