//! Market data models: order books, candles, tickers, and public trades.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::common::{Timestamp, string_u64};

/// Which side of the order book a price level belongs to.
///
/// Note the exchange spells the buy side `BUYI` on the wire (not `BUY`); the
/// SDK maps it to [`BookSide::Buy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BookSide {
    /// A resting sell order (ask).
    #[serde(rename = "SELL")]
    Sell,
    /// A resting buy order (bid).
    #[serde(rename = "BUYI")]
    Buy,
    /// A side this SDK version does not recognize (forward compatibility) — so a
    /// new wire value does not fail deserialization of a whole order book.
    #[serde(other)]
    Unknown,
}

/// A single aggregated price level in an order book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderBookLevel {
    /// Crypto-asset ID code (`aid`), e.g. `ETH`.
    #[serde(rename = "aid")]
    pub asset_id: String,
    /// Crypto-asset full name (`anm`), e.g. `Ethereum`.
    #[serde(rename = "anm")]
    pub asset_name: String,
    /// Which side of the book this level is on (`s`).
    #[serde(rename = "s")]
    pub side: BookSide,
    /// Price in major currency units (`p`).
    #[serde(rename = "p", with = "rust_decimal::serde::str")]
    pub price: Decimal,
    /// Price currency (`pc`), e.g. `USD`.
    #[serde(rename = "pc")]
    pub price_currency: String,
    /// Price notation (`pn`), e.g. `MONE`.
    #[serde(rename = "pn")]
    pub price_notation: String,
    /// Aggregated quantity at this price level (`q`).
    #[serde(rename = "q", with = "rust_decimal::serde::str")]
    pub quantity: Decimal,
    /// Quantity currency (`qc`).
    #[serde(rename = "qc")]
    pub quantity_currency: String,
    /// Quantity notation (`qn`), e.g. `UNIT`.
    #[serde(rename = "qn")]
    pub quantity_notation: String,
    /// Venue of execution (`ve`), always `REVX`.
    #[serde(rename = "ve")]
    pub venue_of_execution: String,
    /// Number of orders aggregated at this level (`no`).
    #[serde(rename = "no", with = "string_u64")]
    pub number_of_orders: u64,
    /// Trading system (`ts`), always `CLOB`.
    #[serde(rename = "ts")]
    pub trading_system: String,
    /// Publication time of this level (`pdt`).
    #[serde(rename = "pdt")]
    pub published_at: Timestamp,
}

/// An order book snapshot.
///
/// Used by both the authenticated and public order-book endpoints; the only
/// wire difference (millisecond vs ISO-8601 timestamps) is absorbed by
/// [`Timestamp`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawOrderBook")]
pub struct OrderBook {
    /// Ask (sell) levels.
    pub asks: Vec<OrderBookLevel>,
    /// Bid (buy) levels.
    pub bids: Vec<OrderBookLevel>,
    /// When the snapshot was generated.
    pub timestamp: Timestamp,
}

#[derive(Deserialize)]
struct RawOrderBook {
    data: RawOrderBookData,
    metadata: RawTimestampMeta,
}

#[derive(Deserialize)]
struct RawOrderBookData {
    asks: Vec<OrderBookLevel>,
    bids: Vec<OrderBookLevel>,
}

#[derive(Deserialize)]
struct RawTimestampMeta {
    timestamp: Timestamp,
}

impl From<RawOrderBook> for OrderBook {
    fn from(raw: RawOrderBook) -> Self {
        Self {
            asks: raw.data.asks,
            bids: raw.data.bids,
            timestamp: raw.metadata.timestamp,
        }
    }
}

/// A single OHLCV candle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candle {
    /// Start time of the candle.
    pub start: Timestamp,
    /// Opening price.
    #[serde(with = "rust_decimal::serde::str")]
    pub open: Decimal,
    /// Highest price during the interval.
    #[serde(with = "rust_decimal::serde::str")]
    pub high: Decimal,
    /// Lowest price during the interval.
    #[serde(with = "rust_decimal::serde::str")]
    pub low: Decimal,
    /// Closing price.
    #[serde(with = "rust_decimal::serde::str")]
    pub close: Decimal,
    /// Total trading volume during the interval.
    #[serde(with = "rust_decimal::serde::str")]
    pub volume: Decimal,
}

/// A real-time best bid/ask and last-trade snapshot for a trading pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticker {
    /// The trading pair, e.g. `BTC/USD`.
    pub symbol: String,
    /// Best bid price.
    #[serde(with = "rust_decimal::serde::str")]
    pub bid: Decimal,
    /// Best ask price.
    #[serde(with = "rust_decimal::serde::str")]
    pub ask: Decimal,
    /// Midpoint between best bid and best ask.
    #[serde(with = "rust_decimal::serde::str")]
    pub mid: Decimal,
    /// Price of the most recent trade.
    #[serde(with = "rust_decimal::serde::str")]
    pub last_price: Decimal,
}

/// A set of tickers and the time they were captured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawTickers")]
pub struct Tickers {
    /// The per-pair tickers.
    pub tickers: Vec<Ticker>,
    /// When the data was captured.
    pub timestamp: Timestamp,
}

#[derive(Deserialize)]
struct RawTickers {
    data: Vec<Ticker>,
    metadata: RawTimestampMeta,
}

impl From<RawTickers> for Tickers {
    fn from(raw: RawTickers) -> Self {
        Self {
            tickers: raw.data,
            timestamp: raw.metadata.timestamp,
        }
    }
}

/// A single public market trade.
///
/// Returned by both `GET /trades/all/{symbol}` (millisecond timestamps) and
/// `GET /public/last-trades` (ISO-8601 timestamps); [`Timestamp`] absorbs the
/// difference. The exchange uses short JSON keys, which the SDK renames to
/// readable fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trade {
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
}

/// The latest public trades and the time they were captured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawLastTrades")]
pub struct LastTrades {
    /// The most recent trades.
    pub trades: Vec<Trade>,
    /// When the data was generated.
    pub timestamp: Timestamp,
}

#[derive(Deserialize)]
struct RawLastTrades {
    data: Vec<Trade>,
    metadata: RawTimestampMeta,
}

impl From<RawLastTrades> for LastTrades {
    fn from(raw: RawLastTrades) -> Self {
        Self {
            trades: raw.data,
            timestamp: raw.metadata.timestamp,
        }
    }
}
