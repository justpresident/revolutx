//! The parse-neutral command model.

use crate::access::AccessLevel;
use crate::api::market_data::{CandleInterval, CandlesQuery};
use crate::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use crate::api::trades::TradesQuery;
use crate::error::{Error, Result};
use crate::model::orders::OrderReplacementRequest;
use crate::{ClientOrderId, Decimal, OrderId, Side};

/// One SDK operation plus its typed arguments, independent of how it was parsed.
///
/// Each surface (CLI, REPL, MCP) normalizes its own input into this; [`execute`](super::execute)
/// runs it against a client. Surface-only concerns — confirmation (`--yes`),
/// output format (`--json`) — are deliberately *not* modelled here. It is
/// intentionally *not* `#[non_exhaustive]`: adding a command should force every
/// surface's adapter and presenter to update (a compile error), not be silently
/// skipped.
#[derive(Debug)]
pub enum Command {
    /// All account balances.
    Balances,
    /// Supported currencies.
    Currencies,
    /// Supported trading pairs.
    Pairs,
    /// Tickers for the given symbols, or all when empty.
    Tickers { symbols: Vec<String> },
    /// Order-book snapshot (authenticated); `limit` levels per side when set.
    OrderBook { symbol: String, limit: Option<u32> },
    /// Order-book snapshot from the public (unauthenticated) endpoint.
    PublicOrderBook { symbol: String },
    /// Historical OHLCV candles.
    Candles { symbol: String, query: CandlesQuery },
    /// Latest public trades across the exchange.
    LastTrades,
    /// Public market trades for a symbol.
    AllTrades { symbol: String, query: TradesQuery },
    /// The account's own executions for a symbol.
    PrivateTrades { symbol: String, query: TradesQuery },
    /// Active orders.
    ActiveOrders(ActiveOrdersQuery),
    /// Historical (finished) orders.
    HistoricalOrders(HistoricalOrdersQuery),
    /// A single order by id.
    GetOrder(OrderId),
    /// An order's fills.
    OrderFills(OrderId),
    /// Place a limit order (real trading).
    PlaceLimit(PlaceLimit),
    /// Place a market order (real trading).
    PlaceMarket(PlaceMarket),
    /// Atomically replace a resting order (real trading).
    Replace {
        id: OrderId,
        request: OrderReplacementRequest,
    },
    /// Cancel an order by id (real trading).
    Cancel(OrderId),
    /// Cancel all active orders (real trading).
    CancelAll,
}

/// A limit-order placement. `size`/`price` stay raw [`Decimal`] so the SDK
/// builder remains the single place order inputs are validated.
#[derive(Debug)]
pub struct PlaceLimit {
    pub symbol: String,
    pub side: Side,
    pub size: Decimal,
    pub price: Decimal,
    /// Interpret `size` as the quote-currency amount.
    pub in_quote: bool,
    /// Reject if the order would take liquidity.
    pub post_only: bool,
    pub client_order_id: Option<ClientOrderId>,
}

/// A market-order placement.
#[derive(Debug)]
pub struct PlaceMarket {
    pub symbol: String,
    pub side: Side,
    pub size: Decimal,
    /// Interpret `size` as the quote-currency amount.
    pub in_quote: bool,
    pub client_order_id: Option<ClientOrderId>,
}

impl Command {
    /// The minimum [`AccessLevel`] required to run this command: public market data
    /// and exchange reference lookups need [`Market`](AccessLevel::Market); reading
    /// personal account data needs [`View`](AccessLevel::View); placing, replacing,
    /// or cancelling orders needs [`Trading`](AccessLevel::Trading).
    ///
    /// This is the command-side mirror of [`required_access_for`](crate::access::required_access_for),
    /// which classifies the same operations by their HTTP request; the two are kept
    /// in lockstep by a test. A surface (the CLI directly, the agent at its trust
    /// boundary) refuses a command whose `min_access` exceeds the session's tier.
    #[must_use]
    pub const fn min_access(&self) -> AccessLevel {
        match self {
            Self::Tickers { .. }
            | Self::OrderBook { .. }
            | Self::PublicOrderBook { .. }
            | Self::Candles { .. }
            | Self::LastTrades
            | Self::AllTrades { .. }
            | Self::Currencies
            | Self::Pairs => AccessLevel::Market,
            Self::Balances
            | Self::PrivateTrades { .. }
            | Self::ActiveOrders(_)
            | Self::HistoricalOrders(_)
            | Self::GetOrder(_)
            | Self::OrderFills(_) => AccessLevel::View,
            Self::PlaceLimit(_)
            | Self::PlaceMarket(_)
            | Self::Replace { .. }
            | Self::Cancel(_)
            | Self::CancelAll => AccessLevel::Trading,
        }
    }

    /// Whether this command places, replaces, or cancels real orders — so a
    /// surface can gate it behind a confirmation. Reads and market data are not.
    #[must_use]
    pub const fn is_real_trading(&self) -> bool {
        matches!(self.min_access(), AccessLevel::Trading)
    }
}

/// Maps a candle interval in minutes to a [`CandleInterval`]. Shared by the CLI
/// and MCP adapters so the allowed-values table lives in one place.
pub fn candle_interval(minutes: i64) -> Result<CandleInterval> {
    use CandleInterval::{
        FifteenMinutes, FiveMinutes, FourDays, FourHours, FourWeeks, OneDay, OneHour, OneMinute,
        OneWeek, ThirtyMinutes, TwoDays, TwoWeeks,
    };
    Ok(match minutes {
        1 => OneMinute,
        5 => FiveMinutes,
        15 => FifteenMinutes,
        30 => ThirtyMinutes,
        60 => OneHour,
        240 => FourHours,
        1440 => OneDay,
        2880 => TwoDays,
        5760 => FourDays,
        10080 => OneWeek,
        20160 => TwoWeeks,
        40320 => FourWeeks,
        other => {
            return Err(Error::InvalidRequest {
                message: format!(
                    "invalid candle interval {other} (allowed: 1,5,15,30,60,240,1440,2880,5760,10080,20160,40320)"
                ),
            });
        }
    })
}
