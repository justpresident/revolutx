//! MCP tool definitions and dispatch onto the `revolutx` SDK.
//!
//! Every tool is forwarded to the signing agent, which is the single
//! authoritative gate: it refuses all requests until the session has
//! authenticated (via the `authenticate` tool) and refuses order-mutating
//! (`place_*`, `cancel_*`) requests unless it was started with trading enabled
//! (`--enable-trading`). The tool catalog is therefore advertised unconditionally
//! — the agent, not the MCP, decides what actually runs.

use std::str::FromStr;

use revolutx::api::market_data::{CandleInterval, CandlesQuery};
use revolutx::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use revolutx::api::trades::TradesQuery;
use revolutx::{ClientOrderId, Decimal, OrderId, RevolutXClient, Side};
use serde::Serialize;
use serde_json::{Value, json};

/// Tool name: authenticate the session with the agent's one-time token. The
/// single source of truth for both the catalog entry and the dispatch in
/// [`crate::server`].
pub const AUTHENTICATE: &str = "authenticate";
/// Argument key carrying the one-time token for the [`AUTHENTICATE`] tool.
pub const ARG_TOKEN: &str = "token";

// Tool names — the single source of truth for each tool's catalog entry
// (`list`) and its dispatch arm (`call`).
const GET_BALANCES: &str = "get_balances";
const GET_CURRENCIES: &str = "get_currencies";
const GET_PAIRS: &str = "get_pairs";
const GET_TICKERS: &str = "get_tickers";
const GET_ORDER_BOOK: &str = "get_order_book";
const GET_PUBLIC_ORDER_BOOK: &str = "get_public_order_book";
const GET_CANDLES: &str = "get_candles";
const GET_LAST_TRADES: &str = "get_last_trades";
const GET_ALL_TRADES: &str = "get_all_trades";
const GET_PRIVATE_TRADES: &str = "get_private_trades";
const GET_ACTIVE_ORDERS: &str = "get_active_orders";
const GET_HISTORICAL_ORDERS: &str = "get_historical_orders";
const GET_ORDER: &str = "get_order";
const GET_ORDER_FILLS: &str = "get_order_fills";
const PLACE_LIMIT_ORDER: &str = "place_limit_order";
const PLACE_MARKET_ORDER: &str = "place_market_order";
const CANCEL_ORDER: &str = "cancel_order";
const CANCEL_ALL_ORDERS: &str = "cancel_all_orders";

/// Returns the tool definitions exposed via `tools/list`. The catalog is fixed:
/// the agent enforces authentication and the trading gate at call time.
// A flat, declarative catalog of tool schemas; splitting it would not aid clarity.
#[allow(clippy::too_many_lines)]
pub fn list() -> Vec<Value> {
    let mut tools = vec![
        tool(
            AUTHENTICATE,
            "Authenticate this session with the signing agent's one-time token. Call this FIRST: the operator starts the agent with `revolutx agent start --auth-token`, which prints a token to paste here. Until you call it successfully, every other tool fails with \"authenticate first\". The token is single-use.",
            json!({
                "type": "object",
                "properties": {
                    ARG_TOKEN: { "type": "string", "description": "The one-time token printed by the agent at startup." }
                },
                "required": [ARG_TOKEN]
            }),
        ),
        tool(
            GET_BALANCES,
            "Get all account balances (available, reserved, staked, total) per currency. Requires credentials.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            GET_CURRENCIES,
            "Get the exchange's supported currencies and their metadata. Requires credentials.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            GET_PAIRS,
            "Get the supported trading pairs and their constraints (step sizes, min/max order size). Requires credentials.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            GET_TICKERS,
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
            GET_ORDER_BOOK,
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
            GET_PUBLIC_ORDER_BOOK,
            "Get an order book snapshot for a symbol from the public (unauthenticated) endpoint.",
            symbol_schema("Trading pair, e.g. BTC-USD"),
        ),
        tool(
            GET_CANDLES,
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
            GET_LAST_TRADES,
            "Get the latest public trades across the exchange (unauthenticated).",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            GET_ALL_TRADES,
            "Get public market trade history for a symbol (paginated).",
            trades_schema(),
        ),
        tool(
            GET_PRIVATE_TRADES,
            "Get the account's own executions for a symbol (paginated). Requires credentials.",
            trades_schema(),
        ),
        tool(
            GET_ACTIVE_ORDERS,
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
            GET_HISTORICAL_ORDERS,
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
            GET_ORDER,
            "Get a single order by its venue order id, including fee details. Requires credentials.",
            order_id_schema(),
        ),
        tool(
            GET_ORDER_FILLS,
            "Get the fills (executions) of an order by its venue order id. Requires credentials.",
            order_id_schema(),
        ),
    ];

    // Order-mutating tools are always advertised; the agent refuses them unless
    // it was started with --enable-trading (and refuses everything pre-auth).
    tools.push(tool(
        PLACE_LIMIT_ORDER,
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
            PLACE_MARKET_ORDER,
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
        CANCEL_ORDER,
        "Cancel a single order by its venue order id. REAL TRADING.",
        order_id_schema(),
    ));
    tools.push(tool(
        CANCEL_ALL_ORDERS,
        "Cancel all active orders. REAL TRADING.",
        json!({ "type": "object", "properties": {}, "required": [] }),
    ));

    tools
}

/// Executes a tool call, returning the text payload on success or a
/// human-readable error message on failure. The signing agent enforces both the
/// authentication gate and the trading gate, so failures (including "authenticate
/// first" and "trading is disabled") surface here as the agent's error message.
// A flat dispatch over tool names; `side`/`size` are both core order terms.
#[allow(clippy::too_many_lines, clippy::similar_names)]
pub async fn call(client: &RevolutXClient, name: &str, args: &Value) -> Result<String, String> {
    match name {
        // --- read-only ------------------------------------------------------
        GET_BALANCES => ok_json(&sdk(client.balances().get_all().await)?),
        GET_CURRENCIES => ok_json(&sdk(client.configuration().currencies().await)?),
        GET_PAIRS => ok_json(&sdk(client.configuration().pairs().await)?),
        GET_TICKERS => {
            let symbols = opt_str_vec(args, "symbols");
            let tickers = if symbols.is_empty() {
                sdk(client.market_data().tickers().await)?
            } else {
                sdk(client.market_data().tickers_for(&symbols).await)?
            };
            ok_json(&tickers)
        }
        GET_ORDER_BOOK => {
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
        GET_PUBLIC_ORDER_BOOK => {
            let symbol = req_str(args, "symbol")?;
            ok_json(&sdk(client.market_data().public_order_book(&symbol).await)?)
        }
        GET_CANDLES => {
            let symbol = req_str(args, "symbol")?;
            let query = CandlesQuery {
                interval: opt_candle_interval(args)?,
                since: opt_i64(args, "since"),
                until: opt_i64(args, "until"),
            };
            ok_json(&sdk(client.market_data().candles(&symbol, &query).await)?)
        }
        GET_LAST_TRADES => ok_json(&sdk(client.market_data().last_trades().await)?),
        GET_ALL_TRADES => {
            let symbol = req_str(args, "symbol")?;
            ok_json(&sdk(client
                .trades()
                .all(&symbol, &trades_query(args))
                .await)?)
        }
        GET_PRIVATE_TRADES => {
            let symbol = req_str(args, "symbol")?;
            ok_json(&sdk(client
                .trades()
                .private(&symbol, &trades_query(args))
                .await)?)
        }
        GET_ACTIVE_ORDERS => {
            let query = ActiveOrdersQuery {
                symbols: opt_str_vec(args, "symbols"),
                side: opt_side(args)?,
                cursor: opt_str(args, "cursor"),
                limit: opt_u32(args, "limit"),
                ..Default::default()
            };
            ok_json(&sdk(client.orders().active(&query).await)?)
        }
        GET_HISTORICAL_ORDERS => {
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
        GET_ORDER => {
            let id = OrderId::new(req_str(args, "order_id")?);
            ok_json(&sdk(client.orders().get(&id).await)?)
        }
        GET_ORDER_FILLS => {
            let id = OrderId::new(req_str(args, "order_id")?);
            ok_json(&sdk(client.orders().fills(&id).await)?)
        }

        // --- order mutation (gated) ----------------------------------------
        PLACE_LIMIT_ORDER => {
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
        PLACE_MARKET_ORDER => {
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
        CANCEL_ORDER => {
            let id = OrderId::new(req_str(args, "order_id")?);
            sdk(client.orders().cancel(&id).await)?;
            ok_json(&json!({ "status": "cancelled", "order_id": id.as_str() }))
        }
        CANCEL_ALL_ORDERS => {
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
    fn catalog_includes_authenticate_reads_and_trading_tools() {
        // The catalog is fixed; the agent gates auth + trading at call time.
        let catalog = list();
        let names: Vec<&str> = catalog
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"authenticate"));
        assert!(names.contains(&"get_balances"));
        assert!(names.contains(&"get_order_book"));
        assert!(names.contains(&"place_limit_order"));
        assert!(names.contains(&"place_market_order"));
        assert!(names.contains(&"cancel_order"));
        assert!(names.contains(&"cancel_all_orders"));
    }

    #[test]
    fn every_tool_has_an_object_input_schema() {
        for t in list() {
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
