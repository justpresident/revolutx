//! MCP tool definitions and dispatch onto the `revolutx` SDK.
//!
//! Read-only tools are always available. Order-mutating tools
//! (`place_*`, `cancel_*`) are only listed and callable when the connected
//! signing agent was started with trading enabled (`--enable-trading`); the
//! agent enforces this regardless of what the MCP believes.

use std::str::FromStr;

use revolutx::api::market_data::{CandleInterval, CandlesQuery};
use revolutx::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use revolutx::api::trades::TradesQuery;
use revolutx::{ClientOrderId, Decimal, OrderId, RevolutXClient, Side};
use serde::Serialize;
use serde_json::{Value, json};

const TRADING_DISABLED: &str = "trading is disabled on the signing agent; restart it with `revolutx agent start --enable-trading` to allow order placement and cancellation";

/// Returns the tool definitions exposed via `tools/list`, filtered by whether
/// trading is enabled.
// A flat, declarative catalog of tool schemas; splitting it would not aid clarity.
#[allow(clippy::too_many_lines)]
pub fn list(trading_enabled: bool) -> Vec<Value> {
    let mut tools = vec![
        tool(
            "get_balances",
            "Get all account balances (available, reserved, staked, total) per currency. Requires credentials.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            "get_currencies",
            "Get the exchange's supported currencies and their metadata. Requires credentials.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            "get_pairs",
            "Get the supported trading pairs and their constraints (step sizes, min/max order size). Requires credentials.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            "get_tickers",
            "Get best bid/ask/mid and last price for trading pairs. Requires credentials.",
            json!({
                "type": "object",
                "properties": {
                    "symbols": { "type": "array", "items": { "type": "string" },
                        "description": "Optional list of symbols (e.g. [\"BTC-USD\"]); omit for all pairs." }
                },
                "required": []
            }),
        ),
        tool(
            "get_order_book",
            "Get an order book snapshot for a symbol (authenticated endpoint).",
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Trading pair, e.g. BTC-USD" },
                    "limit": { "type": "integer", "description": "Levels per side, 1-20" }
                },
                "required": ["symbol"]
            }),
        ),
        tool(
            "get_public_order_book",
            "Get an order book snapshot for a symbol from the public (unauthenticated) endpoint.",
            symbol_schema("Trading pair, e.g. BTC-USD"),
        ),
        tool(
            "get_candles",
            "Get historical OHLCV candles for a symbol.",
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Trading pair, e.g. BTC-USD" },
                    "interval_minutes": { "type": "integer",
                        "description": "Candle interval in minutes; one of 1,5,15,30,60,240,1440,2880,5760,10080,20160,40320" },
                    "since": { "type": "integer", "description": "Start time, Unix epoch milliseconds" },
                    "until": { "type": "integer", "description": "End time, Unix epoch milliseconds" }
                },
                "required": ["symbol"]
            }),
        ),
        tool(
            "get_last_trades",
            "Get the latest public trades across the exchange (unauthenticated).",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            "get_all_trades",
            "Get public market trade history for a symbol (paginated).",
            trades_schema(),
        ),
        tool(
            "get_private_trades",
            "Get the account's own executions for a symbol (paginated). Requires credentials.",
            trades_schema(),
        ),
        tool(
            "get_active_orders",
            "Get currently active orders (paginated). Requires credentials.",
            json!({
                "type": "object",
                "properties": {
                    "symbols": { "type": "array", "items": { "type": "string" } },
                    "side": { "type": "string", "enum": ["buy", "sell"] },
                    "cursor": { "type": "string" },
                    "limit": { "type": "integer", "description": "1-300" }
                },
                "required": []
            }),
        ),
        tool(
            "get_historical_orders",
            "Get historical (finished) orders (paginated). Requires credentials.",
            json!({
                "type": "object",
                "properties": {
                    "symbols": { "type": "array", "items": { "type": "string" } },
                    "start_date": { "type": "integer", "description": "Unix epoch milliseconds" },
                    "end_date": { "type": "integer", "description": "Unix epoch milliseconds" },
                    "cursor": { "type": "string" },
                    "limit": { "type": "integer", "description": "1-1900" }
                },
                "required": []
            }),
        ),
        tool(
            "get_order",
            "Get a single order by its venue order id, including fee details. Requires credentials.",
            order_id_schema(),
        ),
        tool(
            "get_order_fills",
            "Get the fills (executions) of an order by its venue order id. Requires credentials.",
            order_id_schema(),
        ),
    ];

    if trading_enabled {
        tools.push(tool(
            "place_limit_order",
            "Place a limit order. REAL TRADING. Requires credentials and trading enabled.",
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Trading pair, e.g. BTC-USD" },
                    "side": { "type": "string", "enum": ["buy", "sell"] },
                    "size": { "type": "string", "description": "Order size as a decimal string" },
                    "price": { "type": "string", "description": "Limit price as a decimal string" },
                    "size_in_quote": { "type": "boolean",
                        "description": "If true, size is in the quote currency instead of the base currency" },
                    "post_only": { "type": "boolean", "description": "Reject if the order would take liquidity" },
                    "client_order_id": { "type": "string", "description": "Optional UUID idempotency key" }
                },
                "required": ["symbol", "side", "size", "price"]
            }),
        ));
        tools.push(tool(
            "place_market_order",
            "Place a market order. REAL TRADING. Requires credentials and trading enabled.",
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Trading pair, e.g. BTC-USD" },
                    "side": { "type": "string", "enum": ["buy", "sell"] },
                    "size": { "type": "string", "description": "Order size as a decimal string" },
                    "size_in_quote": { "type": "boolean",
                        "description": "If true, size is in the quote currency instead of the base currency" },
                    "client_order_id": { "type": "string", "description": "Optional UUID idempotency key" }
                },
                "required": ["symbol", "side", "size"]
            }),
        ));
        tools.push(tool(
            "cancel_order",
            "Cancel a single order by its venue order id. REAL TRADING.",
            order_id_schema(),
        ));
        tools.push(tool(
            "cancel_all_orders",
            "Cancel all active orders. REAL TRADING.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ));
    }

    tools
}

/// Executes a tool call, returning the text payload on success or a
/// human-readable error message on failure.
// A flat dispatch over tool names; `side`/`size` are both core order terms.
#[allow(clippy::too_many_lines, clippy::similar_names)]
pub async fn call(
    client: &RevolutXClient,
    trading_enabled: bool,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    match name {
        // --- read-only ------------------------------------------------------
        "get_balances" => ok_json(&sdk(client.balances().get_all().await)?),
        "get_currencies" => ok_json(&sdk(client.configuration().currencies().await)?),
        "get_pairs" => ok_json(&sdk(client.configuration().pairs().await)?),
        "get_tickers" => {
            let symbols = opt_str_vec(args, "symbols");
            let tickers = if symbols.is_empty() {
                sdk(client.market_data().tickers().await)?
            } else {
                sdk(client.market_data().tickers_for(&symbols).await)?
            };
            ok_json(&tickers)
        }
        "get_order_book" => {
            let symbol = req_str(args, "symbol")?;
            let book = match opt_u32(args, "limit") {
                Some(limit) => sdk(client
                    .market_data()
                    .order_book_with_limit(&symbol, limit)
                    .await)?,
                None => sdk(client.market_data().order_book(&symbol).await)?,
            };
            ok_json(&book)
        }
        "get_public_order_book" => {
            let symbol = req_str(args, "symbol")?;
            ok_json(&sdk(client.market_data().public_order_book(&symbol).await)?)
        }
        "get_candles" => {
            let symbol = req_str(args, "symbol")?;
            let query = CandlesQuery {
                interval: opt_candle_interval(args)?,
                since: opt_i64(args, "since"),
                until: opt_i64(args, "until"),
            };
            ok_json(&sdk(client.market_data().candles(&symbol, &query).await)?)
        }
        "get_last_trades" => ok_json(&sdk(client.market_data().last_trades().await)?),
        "get_all_trades" => {
            let symbol = req_str(args, "symbol")?;
            ok_json(&sdk(client
                .trades()
                .all(&symbol, &trades_query(args))
                .await)?)
        }
        "get_private_trades" => {
            let symbol = req_str(args, "symbol")?;
            ok_json(&sdk(client
                .trades()
                .private(&symbol, &trades_query(args))
                .await)?)
        }
        "get_active_orders" => {
            let query = ActiveOrdersQuery {
                symbols: opt_str_vec(args, "symbols"),
                side: opt_side(args)?,
                cursor: opt_str(args, "cursor"),
                limit: opt_u32(args, "limit"),
                ..Default::default()
            };
            ok_json(&sdk(client.orders().active(&query).await)?)
        }
        "get_historical_orders" => {
            let query = HistoricalOrdersQuery {
                symbols: opt_str_vec(args, "symbols"),
                start_date: opt_i64(args, "start_date"),
                end_date: opt_i64(args, "end_date"),
                cursor: opt_str(args, "cursor"),
                limit: opt_u32(args, "limit"),
                ..Default::default()
            };
            ok_json(&sdk(client.orders().historical(&query).await)?)
        }
        "get_order" => {
            let id = OrderId::new(req_str(args, "order_id")?);
            ok_json(&sdk(client.orders().get(&id).await)?)
        }
        "get_order_fills" => {
            let id = OrderId::new(req_str(args, "order_id")?);
            ok_json(&sdk(client.orders().fills(&id).await)?)
        }

        // --- order mutation (gated) ----------------------------------------
        "place_limit_order" => {
            if !trading_enabled {
                return Err(TRADING_DISABLED.to_string());
            }
            let symbol = req_str(args, "symbol")?;
            let side = req_side(args)?;
            let size = req_decimal(args, "size")?;
            let price = req_decimal(args, "price")?;
            let in_quote = opt_bool(args, "size_in_quote").unwrap_or(false);
            let mut builder = match (side, in_quote) {
                (Side::Buy, false) => client.orders().limit_buy(symbol, size, price),
                (Side::Buy, true) => client.orders().limit_buy_quote(symbol, size, price),
                (Side::Sell, false) => client.orders().limit_sell(symbol, size, price),
                (Side::Sell, true) => client.orders().limit_sell_quote(symbol, size, price),
            };
            if opt_bool(args, "post_only").unwrap_or(false) {
                builder = builder.post_only();
            }
            if let Some(coid) = opt_client_order_id(args)? {
                builder = builder.client_order_id(coid);
            }
            ok_json(&sdk(builder.send().await)?)
        }
        "place_market_order" => {
            if !trading_enabled {
                return Err(TRADING_DISABLED.to_string());
            }
            let symbol = req_str(args, "symbol")?;
            let side = req_side(args)?;
            let size = req_decimal(args, "size")?;
            let in_quote = opt_bool(args, "size_in_quote").unwrap_or(false);
            let mut builder = match (side, in_quote) {
                (Side::Buy, false) => client.orders().market_buy(symbol, size),
                (Side::Buy, true) => client.orders().market_buy_quote(symbol, size),
                (Side::Sell, false) => client.orders().market_sell(symbol, size),
                (Side::Sell, true) => client.orders().market_sell_quote(symbol, size),
            };
            if let Some(coid) = opt_client_order_id(args)? {
                builder = builder.client_order_id(coid);
            }
            ok_json(&sdk(builder.send().await)?)
        }
        "cancel_order" => {
            if !trading_enabled {
                return Err(TRADING_DISABLED.to_string());
            }
            let id = OrderId::new(req_str(args, "order_id")?);
            sdk(client.orders().cancel(&id).await)?;
            ok_json(&json!({ "status": "cancelled", "order_id": id.as_str() }))
        }
        "cancel_all_orders" => {
            if !trading_enabled {
                return Err(TRADING_DISABLED.to_string());
            }
            sdk(client.orders().cancel_all().await)?;
            ok_json(&json!({ "status": "all_cancelled" }))
        }

        other => Err(format!("unknown tool: {other}")),
    }
}

// --- helpers ---------------------------------------------------------------

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    // `input_schema` is moved into the object rather than re-serialized.
    let mut map = serde_json::Map::with_capacity(3);
    map.insert("name".to_owned(), name.into());
    map.insert("description".to_owned(), description.into());
    map.insert("inputSchema".to_owned(), input_schema);
    Value::Object(map)
}

fn symbol_schema(desc: &str) -> Value {
    json!({
        "type": "object",
        "properties": { "symbol": { "type": "string", "description": desc } },
        "required": ["symbol"]
    })
}

fn order_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "order_id": { "type": "string", "description": "Venue order id (UUID)" } },
        "required": ["order_id"]
    })
}

fn trades_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "symbol": { "type": "string", "description": "Trading pair, e.g. BTC-USD" },
            "start_date": { "type": "integer", "description": "Unix epoch milliseconds" },
            "end_date": { "type": "integer", "description": "Unix epoch milliseconds" },
            "cursor": { "type": "string" },
            "limit": { "type": "integer", "description": "1-1900" }
        },
        "required": ["symbol"]
    })
}

fn trades_query(args: &Value) -> TradesQuery {
    TradesQuery {
        start_date: opt_i64(args, "start_date"),
        end_date: opt_i64(args, "end_date"),
        cursor: opt_str(args, "cursor"),
        limit: opt_u32(args, "limit"),
    }
}

/// Maps a `revolutx` SDK result into the tool error channel (a `String`).
fn sdk<T>(result: revolutx::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

fn ok_json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}

fn req_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required string argument '{key}'"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn opt_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

fn opt_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

fn opt_str_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Accepts a decimal supplied either as a JSON string or a JSON number.
fn req_decimal(args: &Value, key: &str) -> Result<Decimal, String> {
    let raw = match args.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(_) => {
            return Err(format!(
                "argument '{key}' must be a decimal string or number"
            ));
        }
        None => return Err(format!("missing required argument '{key}'")),
    };
    Decimal::from_str(&raw).map_err(|e| format!("invalid decimal for '{key}': {e}"))
}

fn req_side(args: &Value) -> Result<Side, String> {
    match req_str(args, "side")?.as_str() {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        other => Err(format!(
            "'side' must be \"buy\" or \"sell\", got \"{other}\""
        )),
    }
}

fn opt_side(args: &Value) -> Result<Option<Side>, String> {
    if args.get("side").is_none() {
        return Ok(None);
    }
    req_side(args).map(Some)
}

fn opt_client_order_id(args: &Value) -> Result<Option<ClientOrderId>, String> {
    opt_str(args, "client_order_id").map_or(Ok(None), |raw| {
        ClientOrderId::from_str(&raw)
            .map(Some)
            .map_err(|e| e.to_string())
    })
}

fn opt_candle_interval(args: &Value) -> Result<Option<CandleInterval>, String> {
    let Some(minutes) = opt_i64(args, "interval_minutes") else {
        return Ok(None);
    };
    let interval = match minutes {
        1 => CandleInterval::OneMinute,
        5 => CandleInterval::FiveMinutes,
        15 => CandleInterval::FifteenMinutes,
        30 => CandleInterval::ThirtyMinutes,
        60 => CandleInterval::OneHour,
        240 => CandleInterval::FourHours,
        1440 => CandleInterval::OneDay,
        2880 => CandleInterval::TwoDays,
        5760 => CandleInterval::FourDays,
        10080 => CandleInterval::OneWeek,
        20160 => CandleInterval::TwoWeeks,
        40320 => CandleInterval::FourWeeks,
        other => {
            return Err(format!(
                "invalid interval_minutes {other}; allowed: 1,5,15,30,60,240,1440,2880,5760,10080,20160,40320"
            ));
        }
    };
    Ok(Some(interval))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn read_tools_present_and_trading_tools_gated() {
        let read = list(false);
        let names: Vec<&str> = read.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"get_balances"));
        assert!(names.contains(&"get_order_book"));
        assert!(!names.contains(&"place_limit_order"));
        assert!(!names.contains(&"cancel_all_orders"));

        let with_trading = list(true);
        let names: Vec<&str> = with_trading
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"place_limit_order"));
        assert!(names.contains(&"place_market_order"));
        assert!(names.contains(&"cancel_order"));
        assert!(names.contains(&"cancel_all_orders"));
    }

    #[test]
    fn every_tool_has_an_object_input_schema() {
        for t in list(true) {
            assert!(t["name"].is_string());
            assert!(t["description"].is_string());
            assert_eq!(t["inputSchema"]["type"], "object", "tool {}", t["name"]);
        }
    }

    #[test]
    fn req_decimal_accepts_string_and_number() {
        let args = json!({ "a": "0.1", "b": 5 });
        assert_eq!(
            req_decimal(&args, "a").unwrap(),
            Decimal::from_str("0.1").unwrap()
        );
        assert_eq!(
            req_decimal(&args, "b").unwrap(),
            Decimal::from_str("5").unwrap()
        );
        assert!(req_decimal(&args, "missing").is_err());
    }

    #[test]
    fn candle_interval_maps_known_values() {
        assert_eq!(
            opt_candle_interval(&json!({ "interval_minutes": 5 })).unwrap(),
            Some(CandleInterval::FiveMinutes)
        );
        assert_eq!(opt_candle_interval(&json!({})).unwrap(), None);
        assert!(opt_candle_interval(&json!({ "interval_minutes": 7 })).is_err());
    }

    #[test]
    fn side_parsing() {
        assert_eq!(req_side(&json!({ "side": "buy" })).unwrap(), Side::Buy);
        assert_eq!(req_side(&json!({ "side": "sell" })).unwrap(), Side::Sell);
        assert!(req_side(&json!({ "side": "hodl" })).is_err());
        assert_eq!(opt_side(&json!({})).unwrap(), None);
    }
}
