//! Trade history endpoints (public market trades and private executions).

use crate::client::RevolutXClient;
use crate::error::Result;
use crate::model::Page;
use crate::model::market_data::Trade;
use crate::model::trades::Fill;
use crate::transport::{RequestSpec, encode_component};

/// Optional filters for the trade-history endpoints. Timestamps are Unix epoch
/// milliseconds; `cursor` is taken from the previous [`Page::next_cursor`].
#[derive(Debug, Clone, Default)]
pub struct TradesQuery {
    /// Start of the query range.
    pub start_date: Option<i64>,
    /// End of the query range.
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
    pub(crate) fn new(client: &'a RevolutXClient) -> Self {
        Self { client }
    }

    /// `GET /trades/all/{symbol}` — public market trades for a symbol.
    pub async fn all(&self, symbol: &str, query: &TradesQuery) -> Result<Page<Trade>> {
        let path = format!("/trades/all/{}", encode_component(symbol));
        self.client
            .transport()
            .send_json(RequestSpec::get(path).with_query(query.to_query()))
            .await
    }

    /// `GET /trades/private/{symbol}` — the account's own executions for a
    /// symbol.
    pub async fn private(&self, symbol: &str, query: &TradesQuery) -> Result<Page<Fill>> {
        let path = format!("/trades/private/{}", encode_component(symbol));
        self.client
            .transport()
            .send_json(RequestSpec::get(path).with_query(query.to_query()))
            .await
    }
}
