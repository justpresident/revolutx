//! Order placement and management endpoints.
//!
//! Order placement is expressed through small consuming builders that guarantee
//! a valid request: a symbol, a side, exactly one order configuration, and
//! exactly one size field. Prices and sizes are validated to be positive when
//! the request is built.

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::client::RevolutXClient;
use crate::error::{Error, Result};
use crate::model::common::{ClientOrderId, OrderId, Price, Quantity, Side, Symbol};
use crate::model::orders::{
    ExecutionInstruction, LimitOrderConfiguration, MarketOrderConfiguration, Order, OrderAck,
    OrderConfiguration, OrderDetails, OrderPlacementRequest, OrderReplacementRequest, OrderStatus,
    OrderType,
};
use crate::model::trades::Fill;
use crate::model::{Data, OneOrMany, Page};
use crate::transport::{RequestSpec, encode_component};

/// Filters for `GET /orders/active`.
#[derive(Debug, Clone, Default)]
pub struct ActiveOrdersQuery {
    /// Restrict to these symbols.
    pub symbols: Vec<String>,
    /// Restrict to these order states (`pending_new`, `new`, `partially_filled`).
    pub order_states: Vec<OrderStatus>,
    /// Restrict to these order types (`limit`, `conditional`, `tpsl`).
    pub order_types: Vec<OrderType>,
    /// Restrict to one side.
    pub side: Option<Side>,
    /// Pagination cursor from a previous page.
    pub cursor: Option<String>,
    /// Maximum number of orders to return (1–300).
    pub limit: Option<u32>,
}

impl ActiveOrdersQuery {
    fn to_query(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        for symbol in &self.symbols {
            query.push(("symbols".into(), symbol.clone()));
        }
        for state in &self.order_states {
            query.push(("order_states".into(), state.as_str().to_string()));
        }
        for order_type in &self.order_types {
            query.push(("order_types".into(), order_type.as_str().to_string()));
        }
        if let Some(side) = self.side {
            query.push(("side".into(), side.as_str().to_string()));
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

/// Filters for `GET /orders/historical`. Timestamps are Unix epoch
/// milliseconds.
#[derive(Debug, Clone, Default)]
pub struct HistoricalOrdersQuery {
    /// Restrict to these symbols.
    pub symbols: Vec<String>,
    /// Restrict to these order states (`filled`, `cancelled`, `rejected`,
    /// `replaced`).
    pub order_states: Vec<OrderStatus>,
    /// Restrict to these order types (`market`, `limit`).
    pub order_types: Vec<OrderType>,
    /// Start of the query range.
    pub start_date: Option<i64>,
    /// End of the query range.
    pub end_date: Option<i64>,
    /// Pagination cursor from a previous page.
    pub cursor: Option<String>,
    /// Maximum number of orders to return (1–1900).
    pub limit: Option<u32>,
}

impl HistoricalOrdersQuery {
    fn to_query(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        for symbol in &self.symbols {
            query.push(("symbols".into(), symbol.clone()));
        }
        for state in &self.order_states {
            query.push(("order_states".into(), state.as_str().to_string()));
        }
        for order_type in &self.order_types {
            query.push(("order_types".into(), order_type.as_str().to_string()));
        }
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

/// Order placement and management endpoints, reached via
/// [`RevolutXClient::orders`].
pub struct OrdersApi<'a> {
    client: &'a RevolutXClient,
}

impl<'a> OrdersApi<'a> {
    pub(crate) const fn new(client: &'a RevolutXClient) -> Self {
        Self { client }
    }

    // --- Placement builders -------------------------------------------------

    /// Buy `size` units of the base currency with a limit `price`.
    pub fn limit_buy(
        &self,
        symbol: impl Into<String>,
        size: impl Into<Decimal>,
        price: impl Into<Decimal>,
    ) -> LimitOrderBuilder<'a> {
        LimitOrderBuilder::new(self.client, symbol, Side::Buy, SizeKind::Base, size, price)
    }

    /// Sell `size` units of the base currency with a limit `price`.
    pub fn limit_sell(
        &self,
        symbol: impl Into<String>,
        size: impl Into<Decimal>,
        price: impl Into<Decimal>,
    ) -> LimitOrderBuilder<'a> {
        LimitOrderBuilder::new(self.client, symbol, Side::Sell, SizeKind::Base, size, price)
    }

    /// Buy using `quote_size` units of the quote currency with a limit `price`.
    pub fn limit_buy_quote(
        &self,
        symbol: impl Into<String>,
        quote_size: impl Into<Decimal>,
        price: impl Into<Decimal>,
    ) -> LimitOrderBuilder<'a> {
        LimitOrderBuilder::new(
            self.client,
            symbol,
            Side::Buy,
            SizeKind::Quote,
            quote_size,
            price,
        )
    }

    /// Sell using `quote_size` units of the quote currency with a limit `price`.
    pub fn limit_sell_quote(
        &self,
        symbol: impl Into<String>,
        quote_size: impl Into<Decimal>,
        price: impl Into<Decimal>,
    ) -> LimitOrderBuilder<'a> {
        LimitOrderBuilder::new(
            self.client,
            symbol,
            Side::Sell,
            SizeKind::Quote,
            quote_size,
            price,
        )
    }

    /// Market-buy `size` units of the base currency.
    pub fn market_buy(
        &self,
        symbol: impl Into<String>,
        size: impl Into<Decimal>,
    ) -> MarketOrderBuilder<'a> {
        MarketOrderBuilder::new(self.client, symbol, Side::Buy, SizeKind::Base, size)
    }

    /// Market-sell `size` units of the base currency.
    pub fn market_sell(
        &self,
        symbol: impl Into<String>,
        size: impl Into<Decimal>,
    ) -> MarketOrderBuilder<'a> {
        MarketOrderBuilder::new(self.client, symbol, Side::Sell, SizeKind::Base, size)
    }

    /// Market-buy using `quote_size` units of the quote currency.
    pub fn market_buy_quote(
        &self,
        symbol: impl Into<String>,
        quote_size: impl Into<Decimal>,
    ) -> MarketOrderBuilder<'a> {
        MarketOrderBuilder::new(self.client, symbol, Side::Buy, SizeKind::Quote, quote_size)
    }

    /// Market-sell using `quote_size` units of the quote currency.
    pub fn market_sell_quote(
        &self,
        symbol: impl Into<String>,
        quote_size: impl Into<Decimal>,
    ) -> MarketOrderBuilder<'a> {
        MarketOrderBuilder::new(self.client, symbol, Side::Sell, SizeKind::Quote, quote_size)
    }

    // --- Management ---------------------------------------------------------

    /// `POST /orders` — places a fully-constructed order request.
    pub async fn place(&self, request: &OrderPlacementRequest) -> Result<OrderAck> {
        let raw: RawAck = self
            .client
            .send_json(RequestSpec::post_json("/orders", request)?)
            .await?;
        raw.into_ack()
    }

    /// `PUT /orders/{id}` — atomically replaces an existing order.
    pub async fn replace(
        &self,
        id: &OrderId,
        request: &OrderReplacementRequest,
    ) -> Result<OrderAck> {
        let path = format!("/orders/{}", encode_component(id.as_str()));
        let raw: RawAck = self
            .client
            .send_json(RequestSpec::put_json(path, request)?)
            .await?;
        raw.into_ack()
    }

    /// `GET /orders/{id}` — fetches a single order, including fee details.
    pub async fn get(&self, id: &OrderId) -> Result<OrderDetails> {
        let path = format!("/orders/{}", encode_component(id.as_str()));
        let data: Data<OrderDetails> = self.client.send_json(RequestSpec::get(path)).await?;
        Ok(data.data)
    }

    /// `GET /orders/active` — a page of currently active orders.
    pub async fn active(&self, query: &ActiveOrdersQuery) -> Result<Page<Order>> {
        self.client
            .send_json(RequestSpec::get("/orders/active").with_query(query.to_query()))
            .await
    }

    /// `GET /orders/historical` — a page of historical orders.
    pub async fn historical(&self, query: &HistoricalOrdersQuery) -> Result<Page<Order>> {
        self.client
            .send_json(RequestSpec::get("/orders/historical").with_query(query.to_query()))
            .await
    }

    /// `DELETE /orders/{id}` — cancels a single order.
    pub async fn cancel(&self, id: &OrderId) -> Result<()> {
        let path = format!("/orders/{}", encode_component(id.as_str()));
        self.client.send_no_content(RequestSpec::delete(path)).await
    }

    /// `DELETE /orders` — cancels all active orders.
    pub async fn cancel_all(&self) -> Result<()> {
        self.client
            .send_no_content(RequestSpec::delete("/orders"))
            .await
    }

    /// `GET /orders/fills/{id}` — the fills (executions) of an order.
    pub async fn fills(&self, id: &OrderId) -> Result<Vec<Fill>> {
        let path = format!("/orders/fills/{}", encode_component(id.as_str()));
        let data: Data<Vec<Fill>> = self.client.send_json(RequestSpec::get(path)).await?;
        Ok(data.data)
    }
}

/// Which currency an order size is denominated in.
#[derive(Debug, Clone, Copy)]
enum SizeKind {
    Base,
    Quote,
}

/// Builder for a limit order. Create one via the `limit_*` methods on
/// [`OrdersApi`], then `.send()` it (or `.build()` to inspect the request).
pub struct LimitOrderBuilder<'a> {
    client: &'a RevolutXClient,
    symbol: String,
    side: Side,
    size_kind: SizeKind,
    size: Decimal,
    price: Decimal,
    client_order_id: Option<ClientOrderId>,
    execution_instructions: Option<Vec<ExecutionInstruction>>,
}

impl<'a> LimitOrderBuilder<'a> {
    // `side` (buy/sell) and `size` (quantity) are both core order terms; neither
    // can be renamed without obscuring the domain.
    #[allow(clippy::similar_names)]
    fn new(
        client: &'a RevolutXClient,
        symbol: impl Into<String>,
        side: Side,
        size_kind: SizeKind,
        size: impl Into<Decimal>,
        price: impl Into<Decimal>,
    ) -> Self {
        Self {
            client,
            symbol: symbol.into(),
            side,
            size_kind,
            size: size.into(),
            price: price.into(),
            client_order_id: None,
            execution_instructions: None,
        }
    }

    /// Sets the client order id (idempotency key). A random one is generated if
    /// this is not called.
    #[must_use]
    pub const fn client_order_id(mut self, id: ClientOrderId) -> Self {
        self.client_order_id = Some(id);
        self
    }

    /// Replaces the execution instructions.
    #[must_use]
    pub fn execution_instructions(
        mut self,
        instructions: impl IntoIterator<Item = ExecutionInstruction>,
    ) -> Self {
        self.execution_instructions = Some(instructions.into_iter().collect());
        self
    }

    /// Marks the order post-only (it will never take liquidity).
    #[must_use]
    pub fn post_only(self) -> Self {
        self.execution_instructions([ExecutionInstruction::PostOnly])
    }

    /// Explicitly allows taker execution.
    #[must_use]
    pub fn allow_taker(self) -> Self {
        self.execution_instructions([ExecutionInstruction::AllowTaker])
    }

    /// Builds the validated request without sending it.
    pub fn build(self) -> Result<OrderPlacementRequest> {
        let symbol = Symbol::new(self.symbol)?;
        let price = Price::new(self.price)?;
        let size = Quantity::new(self.size)?;
        let (base_size, quote_size) = match self.size_kind {
            SizeKind::Base => (Some(size), None),
            SizeKind::Quote => (None, Some(size)),
        };
        Ok(OrderPlacementRequest {
            client_order_id: self.client_order_id.unwrap_or_default(),
            symbol,
            side: self.side,
            order_configuration: OrderConfiguration {
                limit: Some(LimitOrderConfiguration {
                    base_size,
                    quote_size,
                    price,
                    execution_instructions: self.execution_instructions,
                }),
                market: None,
            },
        })
    }

    /// Validates, signs, and sends the order.
    pub async fn send(self) -> Result<OrderAck> {
        let client = self.client;
        let request = self.build()?;
        OrdersApi::new(client).place(&request).await
    }
}

/// Builder for a market order. Create one via the `market_*` methods on
/// [`OrdersApi`], then `.send()` it (or `.build()` to inspect the request).
pub struct MarketOrderBuilder<'a> {
    client: &'a RevolutXClient,
    symbol: String,
    side: Side,
    size_kind: SizeKind,
    size: Decimal,
    client_order_id: Option<ClientOrderId>,
}

impl<'a> MarketOrderBuilder<'a> {
    // `side` (buy/sell) and `size` (quantity) are both core order terms; neither
    // can be renamed without obscuring the domain.
    #[allow(clippy::similar_names)]
    fn new(
        client: &'a RevolutXClient,
        symbol: impl Into<String>,
        side: Side,
        size_kind: SizeKind,
        size: impl Into<Decimal>,
    ) -> Self {
        Self {
            client,
            symbol: symbol.into(),
            side,
            size_kind,
            size: size.into(),
            client_order_id: None,
        }
    }

    /// Sets the client order id (idempotency key). A random one is generated if
    /// this is not called.
    #[must_use]
    pub const fn client_order_id(mut self, id: ClientOrderId) -> Self {
        self.client_order_id = Some(id);
        self
    }

    /// Builds the validated request without sending it.
    pub fn build(self) -> Result<OrderPlacementRequest> {
        let symbol = Symbol::new(self.symbol)?;
        let size = Quantity::new(self.size)?;
        let (base_size, quote_size) = match self.size_kind {
            SizeKind::Base => (Some(size), None),
            SizeKind::Quote => (None, Some(size)),
        };
        Ok(OrderPlacementRequest {
            client_order_id: self.client_order_id.unwrap_or_default(),
            symbol,
            side: self.side,
            order_configuration: OrderConfiguration {
                limit: None,
                market: Some(MarketOrderConfiguration {
                    base_size,
                    quote_size,
                }),
            },
        })
    }

    /// Validates, signs, and sends the order.
    pub async fn send(self) -> Result<OrderAck> {
        let client = self.client;
        let request = self.build()?;
        OrdersApi::new(client).place(&request).await
    }
}

/// Wire shape of the placement/replacement acknowledgement. The `data` field is
/// documented as an object but appears as a single-element array in the
/// official examples, so both are accepted.
#[derive(Deserialize)]
struct RawAck {
    data: OneOrMany<OrderAck>,
}

impl RawAck {
    fn into_ack(self) -> Result<OrderAck> {
        self.data.into_first().ok_or_else(|| Error::Unexpected {
            status: 200,
            body: "order acknowledgement response contained no data".to_string(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::client::RevolutXClient;
    use std::str::FromStr;

    fn client() -> RevolutXClient {
        RevolutXClient::builder()
            .api_key("k")
            .private_key_bytes([9u8; 32])
            .build()
            .unwrap()
    }

    #[test]
    fn limit_buy_builds_expected_request() {
        let c = client();
        let request = c
            .orders()
            .limit_buy(
                "BTC-USD",
                Decimal::from_str("0.1").unwrap(),
                Decimal::from_str("50000.50").unwrap(),
            )
            .client_order_id("3fa85f64-5717-4562-b3fc-2c963f66afa6".parse().unwrap())
            .post_only()
            .build()
            .unwrap();

        let json = serde_json::to_value(&request).unwrap();
        let expected = serde_json::json!({
            "client_order_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            "symbol": "BTC-USD",
            "side": "buy",
            "order_configuration": {
                "limit": {
                    "base_size": "0.1",
                    "price": "50000.50",
                    "execution_instructions": ["post_only"]
                }
            }
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn market_sell_quote_builds_expected_request() {
        let c = client();
        let request = c
            .orders()
            .market_sell_quote("BTC-USD", Decimal::from_str("0.1").unwrap())
            .client_order_id("3fa85f64-5717-4562-b3fc-2c963f66afa6".parse().unwrap())
            .build()
            .unwrap();

        let json = serde_json::to_value(&request).unwrap();
        let expected = serde_json::json!({
            "client_order_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            "symbol": "BTC-USD",
            "side": "sell",
            "order_configuration": { "market": { "quote_size": "0.1" } }
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn non_positive_size_is_rejected() {
        let c = client();
        let err = c
            .orders()
            .limit_buy("BTC-USD", Decimal::ZERO, Decimal::from_str("100").unwrap())
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }));
    }

    #[test]
    fn empty_symbol_is_rejected() {
        let c = client();
        let err = c
            .orders()
            .market_buy("", Decimal::from_str("1").unwrap())
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }));
    }

    #[test]
    fn random_client_order_id_is_generated_when_unset() {
        let c = client();
        let request = c
            .orders()
            .limit_sell(
                "ETH-USD",
                Decimal::from_str("1").unwrap(),
                Decimal::from_str("2000").unwrap(),
            )
            .build()
            .unwrap();
        // A valid UUID string is produced.
        assert_eq!(request.client_order_id.to_string().len(), 36);
    }
}
