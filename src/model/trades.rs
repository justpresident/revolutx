//! Private trade / fill models.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::common::{OrderId, Side, Timestamp};

/// One execution against the account's own order (a "fill").
///
/// Returned by `GET /orders/fills/{id}` and `GET /trades/private/{symbol}`.
/// It carries the same fields as a public [`crate::model::market_data::Trade`]
/// plus the originating order id, the order side, and a maker/taker flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    /// Trade date and time (`tdt`).
    #[serde(rename = "tdt")]
    pub traded_at: Timestamp,
    /// Crypto-asset ID code (`aid`).
    #[serde(rename = "aid")]
    pub asset_id: String,
    /// Crypto-asset full name (`anm`).
    #[serde(rename = "anm")]
    pub asset_name: String,
    /// Trade price (`p`).
    #[serde(rename = "p", with = "rust_decimal::serde::str")]
    pub price: Decimal,
    /// Price currency (`pc`).
    #[serde(rename = "pc")]
    pub price_currency: String,
    /// Price notation (`pn`).
    #[serde(rename = "pn")]
    pub price_notation: String,
    /// Traded quantity (`q`).
    #[serde(rename = "q", with = "rust_decimal::serde::str")]
    pub quantity: Decimal,
    /// Quantity currency (`qc`).
    #[serde(rename = "qc")]
    pub quantity_currency: String,
    /// Quantity notation (`qn`).
    #[serde(rename = "qn")]
    pub quantity_notation: String,
    /// Venue of execution (`ve`), always `REVX`.
    #[serde(rename = "ve")]
    pub venue_of_execution: String,
    /// Publication date and time (`pdt`).
    #[serde(rename = "pdt")]
    pub published_at: Timestamp,
    /// Venue of publication (`vp`), always `REVX`.
    #[serde(rename = "vp")]
    pub venue_of_publication: String,
    /// Transaction identification code (`tid`).
    #[serde(rename = "tid")]
    pub trade_id: String,
    /// Id of the order this fill belongs to (`oid`).
    #[serde(rename = "oid")]
    pub order_id: OrderId,
    /// The side of the order (`s`).
    #[serde(rename = "s")]
    pub side: Side,
    /// Whether this execution was a maker (`true`) or taker (`false`) (`im`).
    #[serde(rename = "im")]
    pub is_maker: bool,
}
