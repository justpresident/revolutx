//! Exchange configuration models: supported currencies and trading pairs.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Whether an asset is a fiat or crypto currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetType {
    /// A government-issued (fiat) currency such as USD or EUR.
    Fiat,
    /// A crypto asset such as BTC or ETH.
    Crypto,
}

/// Whether a currency or trading pair is currently tradable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListingStatus {
    /// Available for use.
    Active,
    /// Not currently available.
    Inactive,
}

/// A supported currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Currency {
    /// The symbol of the currency, e.g. `BTC`.
    pub symbol: String,
    /// The full name of the currency, e.g. `Bitcoin`.
    pub name: String,
    /// Number of decimal places used for the currency's smallest unit.
    pub scale: u32,
    /// Whether the currency is fiat or crypto.
    pub asset_type: AssetType,
    /// Whether the currency is available for use.
    pub status: ListingStatus,
}

/// A supported trading pair and its exchange constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrencyPair {
    /// The base currency.
    pub base: String,
    /// The quote currency.
    pub quote: String,
    /// Minimal step for changing the quantity in the base currency.
    #[serde(with = "rust_decimal::serde::str")]
    pub base_step: Decimal,
    /// Minimal step for changing the amount in the quote currency.
    #[serde(with = "rust_decimal::serde::str")]
    pub quote_step: Decimal,
    /// Minimal order quantity (in the base currency) accepted by the exchange.
    #[serde(with = "rust_decimal::serde::str")]
    pub min_order_size: Decimal,
    /// Maximal order quantity (in the base currency) accepted by the exchange.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_order_size: Decimal,
    /// Minimal order quantity (in the quote currency) accepted by the exchange.
    #[serde(with = "rust_decimal::serde::str")]
    pub min_order_size_quote: Decimal,
    /// Whether the pair is available for use.
    pub status: ListingStatus,
}

/// The supported currencies, keyed by currency code (e.g. `BTC`).
pub type Currencies = HashMap<String, Currency>;

/// The supported trading pairs, keyed by pair code (e.g. `BTC/USD`).
pub type CurrencyPairs = HashMap<String, CurrencyPair>;
