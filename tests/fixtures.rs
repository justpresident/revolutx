//! Spec-backed serialization fixtures.
//!
//! These tests load the official `components.examples` from the local OpenAPI
//! document and deserialize each one into the corresponding handwritten model.
//! A failure here means the SDK model has drifted from the documented JSON
//! shape. Request examples are additionally round-tripped (deserialize then
//! re-serialize) and compared semantically.
//!
//! The tests are offline and deterministic; they read the spec file directly
//! and never contact Revolut.

use std::collections::HashMap;

use revolutx::model::balances::Balance;
use revolutx::model::configuration::{Currency, CurrencyPair};
use revolutx::model::market_data::{Candle, LastTrades, OrderBook, Tickers, Trade};
use revolutx::model::orders::{Order, OrderAck, OrderPlacementRequest, OrderReplacementRequest};
use revolutx::model::trades::Fill;
use revolutx::{Page, RawPage};
use serde::de::DeserializeOwned;
use serde_json::Value;

fn spec() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/revolut-openapi/json/revolut-x.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("could not read spec at {path}: {e}; did you init the git submodule?")
    });
    serde_json::from_str(&text).expect("spec is valid JSON")
}

fn example(spec: &Value, name: &str) -> Value {
    let value = &spec["components"]["examples"][name]["value"];
    if value.is_null() {
        panic!("example '{name}' not found in spec");
    }
    value.clone()
}

fn de<T: DeserializeOwned>(value: &Value, ctx: &str) -> T {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("failed to deserialize {ctx}: {e}\nvalue: {value}"))
}

/// Deserialize a response example whose payload is wrapped in `{ "data": ... }`.
fn de_data<T: DeserializeOwned>(spec: &Value, name: &str) -> T {
    de(&example(spec, name)["data"], name)
}

#[test]
fn balances_example_deserializes() {
    let spec = spec();
    let balances: Vec<Balance> = de(
        &example(&spec, "AccountBalanceExample"),
        "AccountBalanceExample",
    );
    assert_eq!(balances.len(), 3);
    assert_eq!(balances[1].currency, "ETH");
    assert!(balances[1].staked.is_some());
}

#[test]
fn currencies_example_deserializes() {
    let spec = spec();
    let currencies: HashMap<String, Currency> = de(
        &example(&spec, "CurrenciesResponseExample"),
        "CurrenciesResponseExample",
    );
    assert_eq!(currencies["BTC"].scale, 8);
}

#[test]
fn currency_pairs_example_deserializes() {
    let spec = spec();
    let pairs: HashMap<String, CurrencyPair> = de(
        &example(&spec, "CurrencyPairsResponseExample"),
        "CurrencyPairsResponseExample",
    );
    assert!(pairs.contains_key("BTC/USD"));
}

#[test]
fn tickers_example_deserializes() {
    let spec = spec();
    let tickers: Tickers = de(
        &example(&spec, "TickersResponseExample"),
        "TickersResponseExample",
    );
    assert_eq!(tickers.tickers.len(), 2);
}

#[test]
fn order_book_examples_deserialize_millis_and_iso() {
    let spec = spec();
    let private: OrderBook = de(&example(&spec, "OrderBook"), "OrderBook");
    assert_eq!(private.asks.len(), 2);
    assert_eq!(private.bids.len(), 2);

    // The public order book uses ISO-8601 timestamps; the same model handles it.
    let public: OrderBook = de(
        &example(&spec, "OrderBookPublicResponseExample"),
        "OrderBookPublicResponseExample",
    );
    assert_eq!(public.asks.len(), 2);
}

#[test]
fn candles_example_deserializes() {
    let spec = spec();
    let candles: Vec<Candle> = de_data(&spec, "CandlesResponseExample");
    assert_eq!(candles.len(), 2);
}

#[test]
fn last_trades_example_deserializes() {
    let spec = spec();
    let last: LastTrades = de(
        &example(&spec, "LastTradesResponseExample"),
        "LastTradesResponseExample",
    );
    assert_eq!(last.trades.len(), 2);
}

#[test]
fn order_placement_ack_example_deserializes() {
    let spec = spec();
    // The `data` field is a single-element array in the example.
    let value = example(&spec, "OrderPlacementResponseExample");
    let ack: OrderAck = de(&value["data"][0], "OrderPlacementResponseExample.data[0]");
    assert_eq!(
        ack.venue_order_id.as_str(),
        "7a52e92e-8639-4fe1-abaa-68d3a2d5234b"
    );
}

#[test]
fn order_detail_examples_deserialize() {
    let spec = spec();
    for name in ["OrderDetails", "TpslOrder", "ConditionalOrder"] {
        let order: Order = de(&example(&spec, name)["data"], name);
        // created_date is required and present in every example.
        assert!(order.created_date.unix_millis() > 0, "{name} created_date");
    }
}

#[test]
fn order_fills_example_deserializes() {
    let spec = spec();
    let fills: Vec<Fill> = de_data(&spec, "OrderFillsResponseExample");
    assert_eq!(fills.len(), 1);
    assert!(!fills[0].is_maker);
}

#[test]
fn paginated_order_examples_deserialize() {
    let spec = spec();
    for name in [
        "ActiveOrdersPaginatedResponseExample",
        "ActiveTpslOrdersPaginatedResponseExample",
        "ActiveConditionalOrdersPaginatedResponseExample",
        "HistoricalOrdersPaginatedResponseExample",
    ] {
        // The wire envelope parses into RawPage, then converts to the flat Page.
        let page: Page<Order> = de::<RawPage<Order>>(&example(&spec, name), name).into();
        assert_eq!(page.items.len(), 1, "{name}");
    }
}

#[test]
fn paginated_trade_examples_deserialize() {
    let spec = spec();
    let trades: Page<Trade> = de::<RawPage<Trade>>(
        &example(&spec, "TradesPaginatedResponseExample"),
        "TradesPaginatedResponseExample",
    )
    .into();
    assert_eq!(trades.items.len(), 1);

    let client_trades: Page<Fill> = de::<RawPage<Fill>>(
        &example(&spec, "ClientTradesPaginatedResponseExample"),
        "ClientTradesPaginatedResponseExample",
    )
    .into();
    assert_eq!(client_trades.items.len(), 1);
}

#[test]
fn order_placement_request_examples_round_trip() {
    let spec = spec();
    for name in [
        "LimitOrderByBaseSizeExample",
        "LimitOrderByQuoteSizeExample",
        "MarketOrderByQuoteSizeExample",
    ] {
        let original = example(&spec, name);
        let request: OrderPlacementRequest = de(&original, name);
        let reserialized = serde_json::to_value(&request).unwrap();
        assert_eq!(reserialized, original, "{name} did not round-trip");
    }
}

#[test]
fn order_replacement_request_examples_round_trip() {
    let spec = spec();
    for name in ["ReplaceOrderExample", "ReplaceOrderExampleSingleParam"] {
        let original = example(&spec, name);
        let request: OrderReplacementRequest = de(&original, name);
        let reserialized = serde_json::to_value(&request).unwrap();
        assert_eq!(reserialized, original, "{name} did not round-trip");
    }
}
