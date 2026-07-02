//! Offline mock-HTTP tests for the endpoint methods.
//!
//! Each test starts a one-shot [`MockServer`], drives a real client (and thus
//! the real transport + signing path) against it, and asserts the method, path,
//! query, auth headers, body, and the parsed response. Authenticated requests
//! are verified end-to-end: the signature header is checked against the public
//! key derived from the test seed.

mod support;

use std::str::FromStr;
use std::time::Duration;

use revolutx::api::market_data::{CandleInterval, CandlesQuery};
use revolutx::api::orders::ActiveOrdersQuery;
use revolutx::api::trades::TradesQuery;
use revolutx::model::orders::OrderStatus;
use revolutx::{Decimal, OrderId, RevolutXClient, Side};
use support::MockServer;

const SEED: [u8; 32] = [3u8; 32];

fn auth_client(base_url: &str) -> RevolutXClient {
    RevolutXClient::builder()
        .api_key("test-key")
        .private_key_bytes(SEED)
        .base_url(base_url)
        .build()
        .unwrap()
}

fn public_client(base_url: &str) -> RevolutXClient {
    RevolutXClient::builder()
        .base_url(base_url)
        .build()
        .unwrap()
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[tokio::test]
async fn balances_get_all_signs_request_and_parses_response() {
    let body = r#"[
        {"currency":"BTC","available":"1.25000000","reserved":"0.10000000","total":"1.35000000"},
        {"currency":"ETH","available":"10.00000000","reserved":"0.00000000","staked":"32.00000000","total":"42.00000000"},
        {"currency":"USD","available":"5400.50","reserved":"100.00","total":"5500.50"}
    ]"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    let balances = client.balances().get_all().await.unwrap();

    let req = server.recorded();
    assert_eq!(req.method, "GET");
    assert_eq!(req.path(), "/api/1.0/balances");
    assert_eq!(req.query(), "");
    assert_eq!(req.header("X-Revx-API-Key"), Some("test-key"));
    req.assert_signed_with(SEED);

    assert_eq!(balances.len(), 3);
    assert_eq!(balances[0].currency, "BTC");
    assert_eq!(balances[0].available, dec("1.25000000"));
    assert_eq!(balances[0].staked, None);
    assert_eq!(balances[1].staked, Some(dec("32.00000000")));
}

#[tokio::test]
async fn currencies_parses_keyed_map() {
    let body = r#"{
        "BTC":{"symbol":"BTC","name":"Bitcoin","scale":8,"asset_type":"crypto","status":"active"},
        "ETH":{"symbol":"ETH","name":"Ethereum","scale":8,"asset_type":"crypto","status":"active"}
    }"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    let currencies = client.configuration().currencies().await.unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/configuration/currencies");
    assert_eq!(currencies.len(), 2);
    assert_eq!(currencies["BTC"].name, "Bitcoin");
}

#[tokio::test]
async fn place_limit_order_sends_minified_signed_body() {
    let resp = r#"{"data":[{"venue_order_id":"7a52e92e-8639-4fe1-abaa-68d3a2d5234b","client_order_id":"984a4d8a-2a9b-4950-822f-2a40037f02bd","state":"new"}]}"#;
    let server = MockServer::start(200, resp);
    let client = auth_client(server.base_url());

    let ack = client
        .orders()
        .limit_buy_quote("BTC-USD", dec("0.1"), dec("50000.50"))
        .client_order_id("3fa85f64-5717-4562-b3fc-2c963f66afa6".parse().unwrap())
        .send()
        .await
        .unwrap();

    let req = server.recorded();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path(), "/api/1.0/orders");
    assert_eq!(req.header("content-type"), Some("application/json"));

    // Exact minified body — these bytes are what was signed.
    let expected = r#"{"client_order_id":"3fa85f64-5717-4562-b3fc-2c963f66afa6","symbol":"BTC-USD","side":"buy","order_configuration":{"limit":{"quote_size":"0.1","price":"50000.50"}}}"#;
    assert_eq!(req.body_string(), expected);
    req.assert_signed_with(SEED);

    assert_eq!(ack.state, OrderStatus::New);
    assert_eq!(
        ack.venue_order_id.as_str(),
        "7a52e92e-8639-4fe1-abaa-68d3a2d5234b"
    );
}

#[tokio::test]
async fn order_book_encodes_symbol_in_path() {
    let body = r#"{
        "data":{
            "asks":[{"aid":"ETH","anm":"Ethereum","s":"SELL","p":"4600","pc":"USD","pn":"MONE","q":"17","qc":"ETH","qn":"UNIT","ve":"REVX","no":"3","ts":"CLOB","pdt":3318215482991}],
            "bids":[{"aid":"ETH","anm":"Ethereum","s":"BUYI","p":"4550","pc":"USD","pn":"MONE","q":"0.25","qc":"ETH","qn":"UNIT","ve":"REVX","no":"1","ts":"CLOB","pdt":3318215482991}]
        },
        "metadata":{"timestamp":3318215482991}
    }"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    let book = client.market_data().order_book("BTC-USD").await.unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/order-book/BTC-USD");
    req.assert_signed_with(SEED);

    assert_eq!(book.asks.len(), 1);
    assert_eq!(book.bids.len(), 1);
    assert_eq!(book.asks[0].price, dec("4600"));
    assert_eq!(book.asks[0].number_of_orders, 3);
    assert_eq!(book.timestamp.unix_millis(), 3318215482991);
}

#[tokio::test]
async fn order_book_normalizes_slash_symbol_to_dash_path() {
    // A pairs-map key (`BTC/USD`) reaches the wire in the dash form the exchange
    // accepts, not percent-encoded (`BTC%2FUSD`).
    let body = r#"{"data":{"asks":[],"bids":[]},"metadata":{"timestamp":1}}"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    client.market_data().order_book("BTC/USD").await.unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/order-book/BTC-USD");
    req.assert_signed_with(SEED);
}

#[tokio::test]
async fn dot_segment_order_id_is_rejected_not_panicked() {
    // A `.`/`..` id would normalize under URL assembly, desyncing the signed
    // path from the wire path; the transport refuses it locally (no panic, no
    // request sent) rather than sign one path and send another.
    let server = MockServer::start(204, "");
    let client = auth_client(server.base_url());

    let err = client
        .orders()
        .cancel(&OrderId::new(".."))
        .await
        .expect_err("a dot-segment order id must be rejected");
    assert!(
        matches!(err, revolutx::Error::InvalidRequest { .. }),
        "expected InvalidRequest, got {err:?}"
    );
}

#[tokio::test]
async fn active_orders_serializes_query_and_paginates() {
    let body = r#"{
        "data":[{"id":"7a52e92e-8639-4fe1-abaa-68d3a2d5234b","client_order_id":"984a4d8a-2a9b-4950-822f-2a40037f02bd","symbol":"BTC/USD","side":"buy","type":"limit","quantity":"0.002","filled_quantity":"0","leaves_quantity":"0.002","price":"98745","status":"new","time_in_force":"gtc","execution_instructions":["allow_taker"],"created_date":3318215482991,"updated_date":3318215482991}],
        "metadata":{"timestamp":3318215482991,"next_cursor":"abc123"}
    }"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    let query = ActiveOrdersQuery {
        symbols: vec!["BTC-USD".into()],
        order_states: vec![OrderStatus::New],
        limit: Some(100),
        ..Default::default()
    };
    let page = client.orders().active(&query).await.unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/orders/active");
    assert_eq!(req.query(), "symbols=BTC-USD&order_states=new&limit=100");
    req.assert_signed_with(SEED);

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].side, Side::Buy);
    assert_eq!(page.next_cursor.as_deref(), Some("abc123"));
}

#[tokio::test]
async fn active_orders_comma_joins_multi_value_filters() {
    let body = r#"{"data":[],"metadata":{"timestamp":1,"next_cursor":""}}"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    // Multiple symbols/states go out as single comma-joined parameters, and a
    // slash-form symbol is dash-normalized (spec `form`/`explode=false` style).
    let query = ActiveOrdersQuery {
        symbols: vec!["BTC-USD".into(), "ETH/USD".into()],
        order_states: vec![OrderStatus::New, OrderStatus::PartiallyFilled],
        ..Default::default()
    };
    client.orders().active(&query).await.unwrap();

    let req = server.recorded();
    assert_eq!(
        req.query(),
        "symbols=BTC-USD,ETH-USD&order_states=new,partially_filled"
    );
    req.assert_signed_with(SEED);
}

#[tokio::test]
async fn active_orders_rejects_unknown_state_filter() {
    let server = MockServer::start(200, r#"{"data":[],"metadata":{"timestamp":1}}"#);
    let client = auth_client(server.base_url());

    let query = ActiveOrdersQuery {
        order_states: vec![OrderStatus::Unknown],
        ..Default::default()
    };
    let err = client
        .orders()
        .active(&query)
        .await
        .expect_err("an unknown-state filter must be rejected locally");
    assert!(matches!(err, revolutx::Error::InvalidRequest { .. }));
}

#[tokio::test]
async fn get_order_exposes_fee_details() {
    let body = r#"{"data":{"id":"7a52e92e-8639-4fe1-abaa-68d3a2d5234b","client_order_id":"984a4d8a-2a9b-4950-822f-2a40037f02bd","symbol":"BTC/USD","side":"buy","type":"limit","quantity":"0.002","filled_quantity":"0","leaves_quantity":"0.002","amount":"197.49","filled_amount":"0","price":"98745","average_fill_price":"89794.51","total_fee":"0.01","status":"new","time_in_force":"gtc","fee_currency":"USD","execution_instructions":["allow_taker"],"created_date":3318215482991,"updated_date":3318215482991}}"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    let id = OrderId::new("7a52e92e-8639-4fe1-abaa-68d3a2d5234b");
    let order = client.orders().get(&id).await.unwrap();

    let req = server.recorded();
    assert_eq!(
        req.path(),
        "/api/1.0/orders/7a52e92e-8639-4fe1-abaa-68d3a2d5234b"
    );
    assert_eq!(order.total_fee, Some(dec("0.01")));
    assert_eq!(order.fee_currency.as_deref(), Some("USD"));
    assert_eq!(order.status, OrderStatus::New);
}

#[tokio::test]
async fn cancel_order_succeeds_on_204_no_content() {
    let server = MockServer::start(204, "");
    let client = auth_client(server.base_url());

    let id = OrderId::new("7a52e92e-8639-4fe1-abaa-68d3a2d5234b");
    client.orders().cancel(&id).await.unwrap();

    let req = server.recorded();
    assert_eq!(req.method, "DELETE");
    assert_eq!(
        req.path(),
        "/api/1.0/orders/7a52e92e-8639-4fe1-abaa-68d3a2d5234b"
    );
    req.assert_signed_with(SEED);
}

#[tokio::test]
async fn candles_serialize_interval_and_since() {
    let body = r#"{"data":[
        {"start":3318215482991,"open":"92087.81","high":"92133.89","low":"92052.39","close":"92067.31","volume":"0.00067964"},
        {"start":3318215782991,"open":"90390.46","high":"90395","low":"90358.84","close":"90395","volume":"0.00230816"}
    ]}"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    let query = CandlesQuery {
        interval: Some(CandleInterval::FiveMinutes),
        since: Some(3318215482991),
        until: None,
    };
    let candles = client
        .market_data()
        .candles("BTC-USD", &query)
        .await
        .unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/candles/BTC-USD");
    assert_eq!(req.query(), "interval=5&since=3318215482991");
    assert_eq!(candles.len(), 2);
    assert_eq!(candles[0].close, dec("92067.31"));
}

#[tokio::test]
async fn tickers_for_comma_joins_symbols() {
    let body = r#"{"data":[{"symbol":"BTC/USD","bid":"0.02","ask":"0.02","mid":"0.02","last_price":"0.02"}],"metadata":{"timestamp":1770201294631}}"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    // A slash-form symbol is normalized to dash form, and the array is sent as a
    // single comma-joined parameter (the spec's `form`/`explode=false` style).
    let tickers = client
        .market_data()
        .tickers_for(&["BTC-USD", "ETH/USD"])
        .await
        .unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/tickers");
    assert_eq!(req.query(), "symbols=BTC-USD,ETH-USD");
    req.assert_signed_with(SEED);
    assert_eq!(tickers.tickers.len(), 1);
}

#[tokio::test]
async fn private_trades_parse_fill_fields() {
    let body = r#"{"data":[{"tdt":3318215482991,"aid":"BTC","anm":"Bitcoin","p":"125056.76","pc":"USD","pn":"MONE","q":"0.00003999","qc":"BTC","qn":"UNIT","ve":"REVX","pdt":3318215482991,"vp":"REVX","tid":"80654a036323311cb0ea28462b42db6d","oid":"2affb2ac-4cf7-4bbf-b7b2-fc1e885bdc2c","s":"buy","im":false}],"metadata":{"timestamp":3318215482991,"next_cursor":""}}"#;
    let server = MockServer::start(200, body);
    let client = auth_client(server.base_url());

    let page = client
        .trades()
        .private("BTC-USD", &TradesQuery::default())
        .await
        .unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/trades/private/BTC-USD");
    req.assert_signed_with(SEED);

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].side, Some(Side::Buy));
    assert!(!page.items[0].is_maker);
    assert_eq!(
        page.items[0].order_id.as_str(),
        "2affb2ac-4cf7-4bbf-b7b2-fc1e885bdc2c"
    );
    // Empty next_cursor is normalized to None.
    assert_eq!(page.next_cursor, None);
}

#[tokio::test]
async fn public_last_trades_is_unauthenticated() {
    let body = r#"{"data":[
        {"tdt":"2025-08-08T21:40:35.133962Z","aid":"BTC","anm":"Bitcoin","p":"116243.32","pc":"USD","pn":"MONE","q":"0.24521000","qc":"BTC","qn":"UNIT","ve":"REVX","pdt":"2025-08-08T21:40:35.133962Z","vp":"REVX","tid":"5ef9648f658149f7ababedc97a6401f8"}
    ],"metadata":{"timestamp":"2025-08-08T21:40:36.684333Z"}}"#;
    let server = MockServer::start(200, body);
    // No credentials configured at all.
    let client = public_client(server.base_url());

    let last = client.market_data().last_trades().await.unwrap();

    let req = server.recorded();
    assert_eq!(req.path(), "/api/1.0/public/last-trades");
    assert!(!req.has_header("X-Revx-Signature"));
    assert!(!req.has_header("X-Revx-API-Key"));
    assert!(!req.has_header("X-Revx-Timestamp"));

    assert_eq!(last.trades.len(), 1);
    assert_eq!(last.trades[0].price, dec("116243.32"));
}

#[tokio::test]
async fn authenticated_call_without_credentials_fails_fast() {
    // No server needed: the request never leaves the client.
    let client = public_client("http://127.0.0.1:1/api/1.0");
    let err = client.balances().get_all().await.unwrap_err();
    assert!(err.is_auth_error());
    assert!(matches!(err, revolutx::Error::MissingCredentials));
}

#[tokio::test]
async fn rate_limit_response_is_classified() {
    let body = r#"{"message":"Rate Limit Exceeded","error_id":"7d85b5e7-d0f0-4696-b7b5-a300d0d03a5e","timestamp":3318215482991}"#;
    let server = MockServer::start_with_headers(429, body, &[("Retry-After", "5000")]);
    let client = auth_client(server.base_url());

    let err = client.balances().get_all().await.unwrap_err();
    let _ = server.recorded();

    assert!(err.is_rate_limited());
    assert_eq!(err.status(), Some(429));
    assert_eq!(err.retry_after(), Some(Duration::from_millis(5000)));
}

#[tokio::test]
async fn not_found_error_is_classified() {
    let body = r#"{"message":"Order with ID 'x' not found.","error_id":"7d85b5e7-d0f0-4696-b7b5-a300d0d03a5e","timestamp":3318215482991}"#;
    let server = MockServer::start(404, body);
    let client = auth_client(server.base_url());

    let id = OrderId::new("missing");
    let err = client.orders().get(&id).await.unwrap_err();
    let _ = server.recorded();

    assert_eq!(err.status(), Some(404));
    let api = err.api_error().unwrap();
    assert_eq!(api.message, "Order with ID 'x' not found.");
}
