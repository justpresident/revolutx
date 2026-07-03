//! Trade history endpoints (public market trades and private executions).

use crate::api::range;
use crate::client::RevolutXClient;
use crate::error::Result;
use crate::model::common::dash_symbol;
use crate::model::market_data::Trade;
use crate::model::trades::Fill;
use crate::model::{Page, RawPage};
use crate::transport::{RequestSpec, encode_component};

/// Optional filters for the trade-history endpoints. Timestamps are Unix epoch
/// milliseconds; `cursor` is taken from the previous [`Page::next_cursor`].
///
/// The same server-side range rules as the historical-orders endpoint apply:
/// one query answers at most 30 days, a missing `start_date` defaults to
/// 7 days before `end_date`, and a missing `end_date` defaults to
/// `start_date` + 7 days when `start_date` is given (not "now").
#[derive(Debug, Clone, Default)]
pub struct TradesQuery {
    /// Start of the query range (server default: 7 days before `end_date`).
    pub start_date: Option<i64>,
    /// End of the query range (server default: `start_date` + 7 days when
    /// `start_date` is given, otherwise now).
    pub end_date: Option<i64>,
    /// Pagination cursor from a previous page.
    pub cursor: Option<String>,
    /// Maximum number of trades to return (1–1900).
    pub limit: Option<u32>,
}

impl TradesQuery {
    pub(crate) fn to_query(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        if let Some(start) = self.start_date {
            query.push(("start_date".into(), start.to_string()));
        }
        if let Some(end) = self.end_date {
            query.push(("end_date".into(), end.to_string()));
        }
        if let Some(cursor) = &self.cursor {
            query.push(("cursor".into(), cursor.clone()));
        }
        if let Some(limit) = self.limit {
            query.push(("limit".into(), limit.to_string()));
        }
        query
    }
}

/// Trade history endpoints, reached via [`RevolutXClient::trades`].
pub struct TradesApi<'a> {
    client: &'a RevolutXClient,
}

impl<'a> TradesApi<'a> {
    pub(crate) const fn new(client: &'a RevolutXClient) -> Self {
        Self { client }
    }

    /// `GET /trades/all/{symbol}` — public market trades for a symbol (either
    /// form: `BTC-USD` or `BTC/USD`).
    pub async fn all(&self, symbol: &str, query: &TradesQuery) -> Result<Page<Trade>> {
        let path = format!("/trades/all/{}", encode_component(&dash_symbol(symbol)));
        let raw: RawPage<Trade> = self
            .client
            .send_json(RequestSpec::get(path).with_query(query.to_query()))
            .await?;
        Ok(raw.into())
    }

    /// `GET /trades/private/{symbol}` — the account's own executions for a
    /// symbol (either form: `BTC-USD` or `BTC/USD`).
    pub async fn private(&self, symbol: &str, query: &TradesQuery) -> Result<Page<Fill>> {
        let path = format!("/trades/private/{}", encode_component(&dash_symbol(symbol)));
        let raw: RawPage<Fill> = self
            .client
            .send_json(RequestSpec::get(path).with_query(query.to_query()))
            .await?;
        Ok(raw.into())
    }

    /// `GET /trades/private/{symbol}` over an **arbitrary** date range.
    ///
    /// Same motivation and semantics as
    /// [`OrdersApi::historical_range`](crate::api::orders::OrdersApi::historical_range):
    /// one query answers at most 30 days and a missing `end_date` defaults
    /// server-side to `start_date` + 7 days, so this walks the whole range
    /// instead — `end_date` defaults to *now*, windows are fetched newest
    /// first with every pagination cursor followed, duplicates on window
    /// boundaries are dropped (by trade id), and the merged result is sorted
    /// newest-first by execution time with `next_cursor` always `None`.
    ///
    /// `query.start_date` is required and `query.cursor` must be unset;
    /// `query.limit` caps the **total** number of fills returned, newest
    /// first. Mid-walk 429s are retried after the server-advised delay
    /// (bounded per walk); ranges wider than ~8 years are refused locally.
    pub async fn private_range(&self, symbol: &str, query: &TradesQuery) -> Result<Page<Fill>> {
        let plan = range::plan(
            "private_range",
            query.start_date,
            query.end_date,
            query.cursor.is_some(),
            query.limit,
        )?;
        range::walk(
            &plan,
            |window_start, window_end, cursor| {
                let window_query = TradesQuery {
                    start_date: Some(window_start),
                    end_date: Some(window_end),
                    cursor,
                    limit: query.limit,
                };
                async move { self.private(symbol, &window_query).await }
            },
            |fill| fill.trade_id.clone(),
            |fill| fill.traded_at,
        )
        .await
    }
}
