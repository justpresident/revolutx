//! MCP tool definitions and their mapping onto the shared `revolutx::commands`
//! layer — the same dispatcher and JSON the CLI runs, so all three surfaces parse
//! and execute identically and differ only in presentation.
//!
//! Every tool is forwarded to the signing agent, which is the single
//! authoritative gate: it refuses all requests until the session has
//! authenticated (via the `authenticate` tool), and then permits only the tools
//! its `--access` tier allows — account reads (`get_balances`, `get_*_orders`,
//! private trades) need `--access view`, and order mutations (`place_*`,
//! `cancel_*`) need `--access trading`. The tool catalog is therefore advertised
//! unconditionally — the agent, not the MCP, decides what actually runs.

use std::str::FromStr;

use revolutx::api::market_data::{CandleInterval, CandlesQuery};
use revolutx::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use revolutx::api::trades::TradesQuery;
use revolutx::commands::{Command, PlaceLimit, PlaceMarket};
use revolutx::{ClientOrderId, Decimal, OrderId, Side};
use serde_json::{Value, json};

/// Tool name: authenticate the session with the agent's one-time token. The
/// single source of truth for both the catalog entry and the dispatch in
/// [`crate::server`].
pub const AUTHENTICATE: &str = "authenticate";
/// Argument key carrying the one-time token for the [`AUTHENTICATE`] tool.
pub const ARG_TOKEN: &str = "token";
/// Argument key carrying the requested access tier for interactive
/// authorization in the [`AUTHENTICATE`] tool.
pub const ARG_ACCESS: &str = "access";

// Tool names — the single source of truth for each tool's catalog entry
// (`list`) and its `to_command` mapping arm.
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
/// the agent enforces authentication and the access gate at call time.
// A flat, declarative catalog of tool schemas; splitting it would not aid clarity.
#[allow(clippy::too_many_lines)]
pub fn list() -> Vec<Value> {
    let mut tools = vec![
        tool(
            AUTHENTICATE,
            "Authenticate this session with the signing agent. Call this FIRST, one of two ways: (a) pass the one-time `token` the operator got from `revolutx agent start --auth-token`; or (b) omit the token to request INTERACTIVE approval — the operator sees this connection in the agent console and runs `grant`. In case (b) you get an \"awaiting operator approval\" reply; once the operator approves, call `authenticate` again to complete. Until authenticated, every other tool fails with \"authenticate first\".",
            json!({
                "type": "object",
                "properties": {
                    ARG_TOKEN: { "type": "string", "description": "One-time token from `agent start --auth-token`. Omit to request interactive operator approval instead." },
                    ARG_ACCESS: { "type": "string", "enum": ["market", "view", "trading"], "description": "Access tier to request when using interactive approval (default: view). The operator grants at or below the agent's ceiling." }
                }
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
    // it was started with --access trading (and refuses everything pre-auth).
    tools.push(tool(
        PLACE_LIMIT_ORDER,
            "Place a limit order. REAL TRADING. Requires the agent at --access trading.",
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
            "Place a market order. REAL TRADING. Requires the agent at --access trading.",
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

/// Builds the shared [`Command`] for a tool call, or a human-readable error for a
/// bad argument or unknown tool. The server runs it via `revolutx::commands::execute`
/// and presents the result as JSON. The signing agent stays the authoritative gate —
/// its "authenticate first" / "access denied" refusals surface as the command's
/// error.
// A flat dispatch over tool names; `side`/`size` are both core order terms.
#[allow(clippy::too_many_lines, clippy::similar_names)]
pub fn to_command(name: &str, args: &Value) -> Result<Command, String> {
    Ok(match name {
        GET_BALANCES => Command::Balances,
        GET_CURRENCIES => Command::Currencies,
        GET_PAIRS => Command::Pairs,
        GET_TICKERS => Command::Tickers {
            symbols: opt_str_vec(args, "symbols"),
        },
        GET_ORDER_BOOK => Command::OrderBook {
            symbol: req_str(args, "symbol")?,
            limit: opt_u32(args, "limit"),
        },
        GET_PUBLIC_ORDER_BOOK => Command::PublicOrderBook {
            symbol: req_str(args, "symbol")?,
        },
        GET_CANDLES => Command::Candles {
            symbol: req_str(args, "symbol")?,
            query: CandlesQuery {
                interval: opt_candle_interval(args)?,
                since: opt_i64(args, "since"),
                until: opt_i64(args, "until"),
            },
        },
        GET_LAST_TRADES => Command::LastTrades,
        GET_ALL_TRADES => Command::AllTrades {
            symbol: req_str(args, "symbol")?,
            query: trades_query(args),
        },
        GET_PRIVATE_TRADES => Command::PrivateTrades {
            symbol: req_str(args, "symbol")?,
            query: trades_query(args),
        },
        GET_ACTIVE_ORDERS => Command::ActiveOrders(ActiveOrdersQuery {
            symbols: opt_str_vec(args, "symbols"),
            side: opt_side(args)?,
            cursor: opt_str(args, "cursor"),
            limit: opt_u32(args, "limit"),
            ..Default::default()
        }),
        GET_HISTORICAL_ORDERS => Command::HistoricalOrders(HistoricalOrdersQuery {
            symbols: opt_str_vec(args, "symbols"),
            start_date: opt_i64(args, "start_date"),
            end_date: opt_i64(args, "end_date"),
            cursor: opt_str(args, "cursor"),
            limit: opt_u32(args, "limit"),
            ..Default::default()
        }),
        GET_ORDER => Command::GetOrder(OrderId::new(req_str(args, "order_id")?)),
        GET_ORDER_FILLS => Command::OrderFills(OrderId::new(req_str(args, "order_id")?)),
        PLACE_LIMIT_ORDER => Command::PlaceLimit(PlaceLimit {
            symbol: req_str(args, "symbol")?,
            side: req_side(args)?,
            size: req_decimal(args, "size")?,
            price: req_decimal(args, "price")?,
            in_quote: opt_bool(args, "size_in_quote").unwrap_or(false),
            post_only: opt_bool(args, "post_only").unwrap_or(false),
            client_order_id: opt_client_order_id(args)?,
        }),
        PLACE_MARKET_ORDER => Command::PlaceMarket(PlaceMarket {
            symbol: req_str(args, "symbol")?,
            side: req_side(args)?,
            size: req_decimal(args, "size")?,
            in_quote: opt_bool(args, "size_in_quote").unwrap_or(false),
            client_order_id: opt_client_order_id(args)?,
        }),
        CANCEL_ORDER => Command::Cancel(OrderId::new(req_str(args, "order_id")?)),
        CANCEL_ALL_ORDERS => Command::CancelAll,
        other => return Err(format!("unknown tool: {other}")),
    })
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
    // Reuse the shared minutes→interval mapping so it lives in one place.
    opt_i64(args, "interval_minutes").map_or(Ok(None), |minutes| {
        revolutx::commands::candle_interval(minutes)
            .map(Some)
            .map_err(|e| e.to_string())
    })
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

    #[test]
    fn to_command_builds_neutral_commands() {
        // A no-arg read tool.
        assert!(matches!(
            to_command(GET_BALANCES, &json!({})),
            Ok(Command::Balances)
        ));
        // A trading tool: every required field is threaded through.
        let limit = to_command(
            PLACE_LIMIT_ORDER,
            &json!({ "symbol": "BTC-USD", "side": "buy", "size": "0.1", "price": "50000" }),
        )
        .unwrap();
        match limit {
            Command::PlaceLimit(o) => {
                assert_eq!(o.symbol, "BTC-USD");
                assert_eq!(o.side, Side::Buy);
                assert!(!o.post_only);
            }
            other => panic!("expected PlaceLimit, got {other:?}"),
        }
    }

    #[test]
    fn to_command_rejects_missing_and_unknown() {
        // A required argument missing → Err, not a malformed command.
        assert!(to_command(GET_ORDER_BOOK, &json!({})).is_err());
        // An unrecognized tool name → Err.
        assert!(to_command("get_nothing", &json!({})).is_err());
    }
}
