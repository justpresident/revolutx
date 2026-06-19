//! OpenAPI operation coverage guard.
//!
//! Parses the local OpenAPI document and compares its full set of operations
//! (method + path + operationId) against an explicit list of what this SDK
//! covers. The test fails when Revolut adds, removes, or renames an operation,
//! forcing a conscious SDK update.
//!
//! This is a drift alarm, not a behavioral test — the endpoint and fixture
//! tests cover behavior.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// Every operation in the spec, mapped to the SDK method that covers it.
/// `(HTTP method, path, operationId, SDK method)`.
const COVERAGE: &[(&str, &str, &str, &str)] = &[
    ("GET", "/balances", "getAllBalances", "balances().get_all()"),
    (
        "GET",
        "/configuration/currencies",
        "getAllCurrencies",
        "configuration().currencies()",
    ),
    (
        "GET",
        "/configuration/pairs",
        "getAllCurrencyPairs",
        "configuration().pairs()",
    ),
    (
        "POST",
        "/orders",
        "placeOrder",
        "orders().place() / *_buy/*_sell builders",
    ),
    (
        "DELETE",
        "/orders",
        "cancelAllOrders",
        "orders().cancel_all()",
    ),
    (
        "GET",
        "/orders/active",
        "getActiveOrders",
        "orders().active()",
    ),
    (
        "GET",
        "/orders/historical",
        "getHistoricalOrders",
        "orders().historical()",
    ),
    (
        "GET",
        "/orders/{venue_order_id}",
        "getOrder",
        "orders().get()",
    ),
    (
        "DELETE",
        "/orders/{venue_order_id}",
        "cancelOrder",
        "orders().cancel()",
    ),
    (
        "PUT",
        "/orders/{venue_order_id}",
        "replaceOrder",
        "orders().replace()",
    ),
    (
        "GET",
        "/orders/fills/{venue_order_id}",
        "getOrderFills",
        "orders().fills()",
    ),
    (
        "GET",
        "/trades/all/{symbol}",
        "getAllTrades",
        "trades().all()",
    ),
    (
        "GET",
        "/trades/private/{symbol}",
        "getPrivateTrades",
        "trades().private()",
    ),
    (
        "GET",
        "/order-book/{symbol}",
        "getOrderBook",
        "market_data().order_book()",
    ),
    (
        "GET",
        "/candles/{symbol}",
        "getCandles",
        "market_data().candles()",
    ),
    ("GET", "/tickers", "getTicker", "market_data().tickers()"),
    (
        "GET",
        "/public/last-trades",
        "getLastTrades",
        "market_data().last_trades()",
    ),
    (
        "GET",
        "/public/order-book/{symbol}",
        "getPublicOrderBook",
        "market_data().public_order_book()",
    ),
];

const HTTP_METHODS: &[&str] = &[
    "get", "put", "post", "delete", "patch", "head", "options", "trace",
];

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

/// Collects `(METHOD, path) -> operationId` from a spec value.
fn operations(spec: &Value) -> BTreeMap<(String, String), String> {
    let mut ops = BTreeMap::new();
    let paths = spec["paths"].as_object().expect("spec has paths");
    for (path, item) in paths {
        let item = item.as_object().expect("path item is an object");
        for (method, op) in item {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let operation_id = op["operationId"].as_str().unwrap_or("").to_owned();
            ops.insert((method.to_uppercase(), path.clone()), operation_id);
        }
    }
    ops
}

#[test]
fn sdk_coverage_matches_spec_operations() {
    let spec = spec();
    let actual = operations(&spec);

    let expected: BTreeMap<(String, String), String> = COVERAGE
        .iter()
        .map(|(method, path, op_id, _)| ((method.to_string(), path.to_string()), op_id.to_string()))
        .collect();

    let actual_keys: BTreeSet<_> = actual.keys().cloned().collect();
    let expected_keys: BTreeSet<_> = expected.keys().cloned().collect();

    let missing: Vec<_> = expected_keys.difference(&actual_keys).collect();
    let unexpected: Vec<_> = actual_keys.difference(&expected_keys).collect();

    assert!(
        missing.is_empty(),
        "SDK coverage lists operations no longer in the spec (renamed/removed?): {missing:?}"
    );
    assert!(
        unexpected.is_empty(),
        "spec has operations the SDK coverage list does not track (new operations?): {unexpected:?}.\n\
         Add them to COVERAGE in tests/spec_coverage.rs and implement them."
    );

    // operationIds must match too, catching renames at the same path/method.
    for (key, op_id) in &actual {
        assert_eq!(
            expected.get(key),
            Some(op_id),
            "operationId mismatch for {key:?}: spec='{op_id}', coverage='{:?}'",
            expected.get(key)
        );
    }
}

#[test]
fn coverage_list_has_no_duplicates() {
    let mut seen = BTreeSet::new();
    for (method, path, _, _) in COVERAGE {
        assert!(
            seen.insert((method, path)),
            "duplicate coverage entry: {method} {path}"
        );
    }
    assert_eq!(seen.len(), COVERAGE.len());
}

#[test]
fn drift_alarm_detects_an_added_operation() {
    // Sanity-check the guard itself: injecting a fake operation must be seen as
    // an untracked operation.
    let mut spec = spec();
    spec["paths"]["/totally-new-endpoint"] =
        serde_json::json!({ "get": { "operationId": "getSomethingNew" } });

    let actual = operations(&spec);
    let expected: BTreeSet<(String, String)> = COVERAGE
        .iter()
        .map(|(method, path, _, _)| (method.to_string(), path.to_string()))
        .collect();
    let actual_keys: BTreeSet<(String, String)> = actual.keys().cloned().collect();

    let unexpected: Vec<_> = actual_keys.difference(&expected).collect();
    assert_eq!(
        unexpected,
        vec![&("GET".to_string(), "/totally-new-endpoint".to_string())]
    );
}
