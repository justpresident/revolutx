//! Market data endpoints.

use crate::api::comma_joined;
use crate::client::RevolutXClient;
use crate::error::Result;
use crate::model::Data;
use crate::model::common::dash_symbol;
use crate::model::market_data::{Candle, LastTrades, OrderBook, Tickers};
use crate::transport::{RequestSpec, encode_component};

/// A supported candle interval. The wire value is the interval length in
/// minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandleInterval {
    /// 1 minute.
    OneMinute,
    /// 5 minutes.
    FiveMinutes,
    /// 15 minutes.
    FifteenMinutes,
    /// 30 minutes.
    ThirtyMinutes,
    /// 1 hour.
    OneHour,
    /// 4 hours.
    FourHours,
    /// 1 day.
    OneDay,
    /// 2 days.
    TwoDays,
    /// 4 days.
    FourDays,
    /// 1 week.
    OneWeek,
    /// 2 weeks.
    TwoWeeks,
    /// 4 weeks.
    FourWeeks,
}

impl CandleInterval {
    /// The interval length in minutes, as sent in the `interval` query
    /// parameter.
    pub const fn minutes(self) -> i32 {
        match self {
            Self::OneMinute => 1,
            Self::FiveMinutes => 5,
            Self::FifteenMinutes => 15,
            Self::ThirtyMinutes => 30,
            Self::OneHour => 60,
            Self::FourHours => 240,
            Self::OneDay => 1440,
            Self::TwoDays => 2880,
            Self::FourDays => 5760,
            Self::OneWeek => 10080,
            Self::TwoWeeks => 20160,
            Self::FourWeeks => 40320,
        }
    }
}

/// Optional filters for `GET /candles/{symbol}`. Timestamps are Unix epoch
/// milliseconds.
#[derive(Debug, Clone, Default)]
pub struct CandlesQuery {
    /// Candle interval (defaults to 5 minutes server-side when unset).
    pub interval: Option<CandleInterval>,
    /// Only return candles at or after this time.
    pub since: Option<i64>,
    /// Only return candles at or before this time.
    pub until: Option<i64>,
}

impl CandlesQuery {
    fn to_query(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        if let Some(interval) = self.interval {
            query.push(("interval".into(), interval.minutes().to_string()));
        }
        if let Some(since) = self.since {
            query.push(("since".into(), since.to_string()));
        }
        if let Some(until) = self.until {
            query.push(("until".into(), until.to_string()));
        }
        query
    }
}

/// Market data endpoints, reached via [`RevolutXClient::market_data`].
///
/// The `public_order_book` and `last_trades` methods hit unauthenticated
/// endpoints and work even on a client built without credentials.
pub struct MarketDataApi<'a> {
    client: &'a RevolutXClient,
}

impl<'a> MarketDataApi<'a> {
    pub(crate) const fn new(client: &'a RevolutXClient) -> Self {
        Self { client }
    }

    /// `GET /order-book/{symbol}` — order book snapshot (authenticated).
    pub async fn order_book(&self, symbol: &str) -> Result<OrderBook> {
        let path = format!("/order-book/{}", encode_component(&dash_symbol(symbol)));
        self.client.send_json(RequestSpec::get(path)).await
    }

    /// `GET /order-book/{symbol}?limit=` — order book snapshot capped at `limit`
    /// levels per side (1–20).
    pub async fn order_book_with_limit(&self, symbol: &str, limit: u32) -> Result<OrderBook> {
        let path = format!("/order-book/{}", encode_component(&dash_symbol(symbol)));
        self.client
            .send_json(RequestSpec::get(path).with_query(vec![("limit".into(), limit.to_string())]))
            .await
    }

    /// `GET /public/order-book/{symbol}` — order book snapshot from the public
    /// (unauthenticated) endpoint.
    pub async fn public_order_book(&self, symbol: &str) -> Result<OrderBook> {
        let path = format!(
            "/public/order-book/{}",
            encode_component(&dash_symbol(symbol))
        );
        self.client.send_json(RequestSpec::get(path).public()).await
    }

    /// `GET /candles/{symbol}` — historical OHLCV candles.
    pub async fn candles(&self, symbol: &str, query: &CandlesQuery) -> Result<Vec<Candle>> {
        let path = format!("/candles/{}", encode_component(&dash_symbol(symbol)));
        let data: Data<Vec<Candle>> = self
            .client
            .send_json(RequestSpec::get(path).with_query(query.to_query()))
            .await?;
        Ok(data.data)
    }

    /// `GET /tickers` — tickers for all trading pairs.
    pub async fn tickers(&self) -> Result<Tickers> {
        self.client.send_json(RequestSpec::get("/tickers")).await
    }

    /// `GET /tickers?symbols=` — tickers for the given trading pairs.
    pub async fn tickers_for<S: AsRef<str> + Sync>(&self, symbols: &[S]) -> Result<Tickers> {
        let query = comma_joined("symbols", symbols.iter().map(|s| dash_symbol(s.as_ref())));
        self.client
            .send_json(RequestSpec::get("/tickers").with_query(query))
            .await
    }

    /// `GET /public/last-trades` — the latest public trades (unauthenticated).
    pub async fn last_trades(&self) -> Result<LastTrades> {
        self.client
            .send_json(RequestSpec::get("/public/last-trades").public())
            .await
    }
}
