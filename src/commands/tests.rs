//! Offline tests for the shared command layer: routing (via a request-recording
//! executor), result parsing, and JSON-presenter parity. No network.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use super::{Command, CommandOutput, JsonPresenter, PlaceLimit, PlaceMarket, Presenter, execute};
use crate::api::market_data::CandlesQuery;
use crate::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use crate::api::trades::TradesQuery;
use crate::model::orders::OrderReplacementRequest;
use crate::transport::{BoxFuture, RawResponse, RequestExecutor, RequestSpec};
use crate::{ClientOrderId, Decimal, OrderId, Result, RevolutXClient, Side};

/// Records the `(method, path)` the SDK built and returns a canned response.
struct Recorder {
    status: u16,
    body: Vec<u8>,
    last: Mutex<Option<(String, String)>>,
}

impl RequestExecutor for Recorder {
    fn execute(&self, request: RequestSpec) -> BoxFuture<'_, Result<RawResponse>> {
        *self.last.lock().unwrap() = Some((
            request.method().as_str().to_owned(),
            request.path().to_owned(),
        ));
        let response = RawResponse {
            status: self.status,
            retry_after: None,
            body: self.body.clone(),
        };
        Box::pin(async move { Ok(response) })
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn base_url(&self) -> &str {
        "http://stub/api/1.0"
    }
    fn is_authenticated(&self) -> bool {
        true
    }
}

fn client_with(status: u16, body: &str) -> (RevolutXClient, Arc<Recorder>) {
    let recorder = Arc::new(Recorder {
        status,
        body: body.as_bytes().to_vec(),
        last: Mutex::new(None),
    });
    (RevolutXClient::with_executor(recorder.clone()), recorder)
}

/// Runs `command` and returns the `(method, path)` it routed to. The canned body
/// may fail to parse for a typed result — routing is recorded before that, so we
/// ignore the `execute` outcome here.
async fn route(command: Command) -> (String, String) {
    let (client, recorder) = client_with(200, "[]");
    let _ = execute(&client, command).await;
    recorder.last.lock().unwrap().clone().unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // a flat list of route assertions; splitting wouldn't help
async fn routes_every_command_to_its_endpoint() {
    let get = |p: &str| ("GET".to_owned(), p.to_owned());
    assert_eq!(route(Command::Balances).await, get("/balances"));
    assert_eq!(
        route(Command::Currencies).await,
        get("/configuration/currencies")
    );
    assert_eq!(route(Command::Pairs).await, get("/configuration/pairs"));
    assert_eq!(
        route(Command::Tickers { symbols: vec![] }).await,
        get("/tickers")
    );
    assert_eq!(
        route(Command::OrderBook {
            symbol: "BTC-USD".into(),
            limit: None
        })
        .await,
        get("/order-book/BTC-USD")
    );
    assert_eq!(
        route(Command::PublicOrderBook {
            symbol: "BTC-USD".into()
        })
        .await,
        get("/public/order-book/BTC-USD")
    );
    assert_eq!(
        route(Command::Candles {
            symbol: "BTC-USD".into(),
            query: CandlesQuery {
                interval: None,
                since: None,
                until: None
            },
        })
        .await,
        get("/candles/BTC-USD")
    );
    assert_eq!(route(Command::LastTrades).await, get("/public/last-trades"));
    assert_eq!(
        route(Command::AllTrades {
            symbol: "BTC-USD".into(),
            query: TradesQuery::default()
        })
        .await,
        get("/trades/all/BTC-USD")
    );
    assert_eq!(
        route(Command::PrivateTrades {
            symbol: "BTC-USD".into(),
            query: TradesQuery::default()
        })
        .await,
        get("/trades/private/BTC-USD")
    );
    assert_eq!(
        route(Command::ActiveOrders(ActiveOrdersQuery::default())).await,
        get("/orders/active")
    );
    assert_eq!(
        route(Command::HistoricalOrders(HistoricalOrdersQuery::default())).await,
        get("/orders/historical")
    );
    assert_eq!(
        route(Command::GetOrder(OrderId::new("abc"))).await,
        get("/orders/abc")
    );
    assert_eq!(
        route(Command::OrderFills(OrderId::new("abc"))).await,
        get("/orders/fills/abc")
    );
    assert_eq!(
        route(Command::Cancel(OrderId::new("abc"))).await,
        ("DELETE".to_owned(), "/orders/abc".to_owned())
    );
    assert_eq!(
        route(Command::CancelAll).await,
        ("DELETE".to_owned(), "/orders".to_owned())
    );

    // Placements need a valid (positive) size/price so the builder's `build()`
    // passes and the request is actually sent.
    let limit = Command::PlaceLimit(PlaceLimit {
        symbol: "BTC-USD".into(),
        side: Side::Buy,
        size: Decimal::from(1),
        price: Decimal::from(1),
        in_quote: false,
        post_only: false,
        client_order_id: None,
    });
    assert_eq!(
        route(limit).await,
        ("POST".to_owned(), "/orders".to_owned())
    );
    let market = Command::PlaceMarket(PlaceMarket {
        symbol: "BTC-USD".into(),
        side: Side::Sell,
        size: Decimal::from(1),
        in_quote: false,
        client_order_id: None,
    });
    assert_eq!(
        route(market).await,
        ("POST".to_owned(), "/orders".to_owned())
    );
    let replace = Command::Replace {
        id: OrderId::new("abc"),
        request: OrderReplacementRequest {
            client_order_id: ClientOrderId::default(),
            base_size: None,
            quote_size: None,
            price: None,
            execution_instructions: None,
        },
    };
    assert_eq!(
        route(replace).await,
        ("PUT".to_owned(), "/orders/abc".to_owned())
    );
}

#[tokio::test]
async fn min_access_agrees_with_request_classification() {
    use crate::access::{AccessLevel, required_access_for};

    // For every command, the variant-side classifier (`min_access`) must agree
    // with the HTTP-path classifier the agent's gate uses (`required_access_for`),
    // so the CLI's local gate and the agent's authoritative gate never diverge.
    async fn check(command: Command, expected: AccessLevel) {
        assert_eq!(command.min_access(), expected, "min_access for {command:?}");
        let (method, path) = route(command).await;
        assert_eq!(
            required_access_for(&method, &path),
            expected,
            "required_access_for {method} {path}"
        );
    }

    use AccessLevel::{Market, Trading, View};
    let q = TradesQuery::default;
    let cq = || CandlesQuery {
        interval: None,
        since: None,
        until: None,
    };
    let sym = || "BTC-USD".to_owned();

    check(Command::Tickers { symbols: vec![] }, Market).await;
    check(Command::Currencies, Market).await;
    check(Command::Pairs, Market).await;
    check(
        Command::OrderBook {
            symbol: sym(),
            limit: None,
        },
        Market,
    )
    .await;
    check(Command::PublicOrderBook { symbol: sym() }, Market).await;
    check(
        Command::Candles {
            symbol: sym(),
            query: cq(),
        },
        Market,
    )
    .await;
    check(Command::LastTrades, Market).await;
    check(
        Command::AllTrades {
            symbol: sym(),
            query: q(),
        },
        Market,
    )
    .await;

    check(Command::Balances, View).await;
    check(
        Command::PrivateTrades {
            symbol: sym(),
            query: q(),
        },
        View,
    )
    .await;
    check(Command::ActiveOrders(ActiveOrdersQuery::default()), View).await;
    check(
        Command::HistoricalOrders(HistoricalOrdersQuery::default()),
        View,
    )
    .await;
    check(Command::GetOrder(OrderId::new("abc")), View).await;
    check(Command::OrderFills(OrderId::new("abc")), View).await;

    check(Command::Cancel(OrderId::new("abc")), Trading).await;
    check(Command::CancelAll, Trading).await;
    check(
        Command::PlaceMarket(PlaceMarket {
            symbol: sym(),
            side: Side::Sell,
            size: Decimal::from(1),
            in_quote: false,
            client_order_id: None,
        }),
        Trading,
    )
    .await;
}

#[tokio::test]
async fn parses_balances_into_output() {
    let (client, _) = client_with(
        200,
        r#"[{"currency":"BTC","available":"1.25","reserved":"0.10","total":"1.35"}]"#,
    );
    let out = execute(&client, Command::Balances).await.unwrap();
    let CommandOutput::Balances(balances) = &out else {
        panic!("expected Balances, got {out:?}");
    };
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].currency, "BTC");
}

#[tokio::test]
async fn cancel_acks_serialize_to_the_mcp_shape() {
    let (client, _) = client_with(200, "");
    let out = execute(&client, Command::Cancel(OrderId::new("ord-1")))
        .await
        .unwrap();
    assert_eq!(
        JsonPresenter.present(&out).unwrap(),
        "{\n  \"status\": \"cancelled\",\n  \"order_id\": \"ord-1\"\n}"
    );

    let (client, _) = client_with(200, "");
    let out = execute(&client, Command::CancelAll).await.unwrap();
    assert_eq!(
        JsonPresenter.present(&out).unwrap(),
        "{\n  \"status\": \"all_cancelled\"\n}"
    );
}

#[tokio::test]
async fn json_presenter_emits_the_bare_inner_value() {
    // Untagged: presenting a CommandOutput equals serializing the inner value —
    // so a surface's JSON is byte-identical to the raw SDK response.
    let (client, _) = client_with(
        200,
        r#"[{"currency":"BTC","available":"1","reserved":"0","total":"1"}]"#,
    );
    let out = execute(&client, Command::Balances).await.unwrap();
    let CommandOutput::Balances(balances) = &out else {
        panic!("expected Balances");
    };
    assert_eq!(
        JsonPresenter.present(&out).unwrap(),
        serde_json::to_string_pretty(balances).unwrap()
    );
}
