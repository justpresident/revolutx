# Revolut X OpenAPI inventory

Snapshot of the local contract (`revolut-openapi/json/revolut-x.json`,
OpenAPI 3.1.0) and how it maps onto the handwritten SDK. The
`tests/spec_coverage.rs` drift guard fails if the operation set here diverges
from the spec, so treat that test — not this document — as the source of truth.

- Production base URL: `https://revx.revolut.com/api/1.0`
- Dev base URL: `https://revx.revolut.codes/api/1.0`
- Auth: API key header `X-Revx-API-Key` + `X-Revx-Timestamp` + `X-Revx-Signature`
  (Ed25519). The two `public/*` operations are unauthenticated.
- 18 operations, 33 schemas, 25 component examples.

## Operations → SDK methods

| Method | Path | operationId | SDK method | Auth |
|--------|------|-------------|------------|------|
| GET | `/balances` | getAllBalances | `client.balances().get_all()` | yes |
| GET | `/configuration/currencies` | getAllCurrencies | `client.configuration().currencies()` | yes |
| GET | `/configuration/pairs` | getAllCurrencyPairs | `client.configuration().pairs()` | yes |
| POST | `/orders` | placeOrder | `client.orders().place()` / `limit_buy`/`market_sell`/… builders | yes |
| DELETE | `/orders` | cancelAllOrders | `client.orders().cancel_all()` | yes |
| GET | `/orders/active` | getActiveOrders | `client.orders().active()` | yes |
| GET | `/orders/historical` | getHistoricalOrders | `client.orders().historical()` | yes |
| GET | `/orders/{venue_order_id}` | getOrder | `client.orders().get()` | yes |
| DELETE | `/orders/{venue_order_id}` | cancelOrder | `client.orders().cancel()` | yes |
| PUT | `/orders/{venue_order_id}` | replaceOrder | `client.orders().replace()` | yes |
| GET | `/orders/fills/{venue_order_id}` | getOrderFills | `client.orders().fills()` | yes |
| GET | `/trades/all/{symbol}` | getAllTrades | `client.trades().all()` | yes |
| GET | `/trades/private/{symbol}` | getPrivateTrades | `client.trades().private()` | yes |
| GET | `/order-book/{symbol}` | getOrderBook | `client.market_data().order_book()` | yes |
| GET | `/candles/{symbol}` | getCandles | `client.market_data().candles()` | yes |
| GET | `/tickers` | getTicker | `client.market_data().tickers()` / `tickers_for()` | yes |
| GET | `/public/last-trades` | getLastTrades | `client.market_data().last_trades()` | no |
| GET | `/public/order-book/{symbol}` | getPublicOrderBook | `client.market_data().public_order_book()` | no |

## Schemas → SDK models

| Schema(s) | SDK type |
|-----------|----------|
| `ErrorResponse` | `error::ApiError` (parsed payload) |
| `AccountBalance` / `AccountBalanceEntry` | `model::balances::Balance` (returned as `Vec`) |
| `Currency` / `CurrenciesResponse` | `model::configuration::Currency`, `Currencies` map |
| `CurrencyPair` / `CurrencyPairsResponse` | `model::configuration::CurrencyPair`, `CurrencyPairs` map |
| `Order` / `OrderDetails` | `model::orders::Order` (fee fields optional; `OrderDetails` is an alias) |
| `OrderTrigger`, `ExecutionInstructions`, `Side` | `model::orders::{OrderTrigger, ExecutionInstruction}`, `model::common::Side` |
| `OrderPlacementRequest` / `OrderReplacementRequest` | `model::orders::{OrderPlacementRequest, OrderReplacementRequest}` (+ builders) |
| `LimitOrderConfiguration` / `MarketOrderConfiguration` | `model::orders::{LimitOrderConfiguration, MarketOrderConfiguration, OrderConfiguration}` |
| `OrderPlacementResponse` / `OrderReplacementResponse` | `model::orders::OrderAck` |
| `ActiveOrdersPaginatedResponse` / `HistoricalOrdersPaginatedResponse` | `model::Page<Order>` |
| `Trade` / `PublicTrade` / `LastTrades` | `model::market_data::{Trade, LastTrades}` |
| `ClientTrade` | `model::trades::Fill` |
| `AllTradesPaginatedResponse` / `PrivateTradesPaginatedResponse` | `model::Page<Trade>` / `model::Page<Fill>` |
| `OrderBook` / `OrderBookPublic` / `OrderBookPriceLevel` / `OrderBookPublicPriceLevel` | `model::market_data::{OrderBook, OrderBookLevel, BookSide}` |
| `Candle` | `model::market_data::Candle` |
| `Ticker` / `TickersResponse` | `model::market_data::{Ticker, Tickers}` |

## Spec / example inconsistencies handled

These are points where the schema and the official examples disagree; the SDK
follows the examples (which appear closer to real responses) and stays tolerant:

- **Order placement/replacement `data`**: the schema types `data` as an object,
  but every example wraps it in a single-element array. `OrderAck` parsing
  accepts either shape (`model::OneOrMany`).
- **`tpsl` placement is unsupported, not just undocumented**: `Order.type`
  includes `tpsl` and the read examples show web-UI-created TP/SL orders, yet
  `OrderPlacementRequest` documents only `limit`/`market` configurations.
  Probed in production 2026-07-02 (`examples/tpsl_probe.rs`, built on
  `orders().place_raw()`): every candidate placement shape — the read model
  mirrored under `order_configuration.tpsl` with `base_size` or `quantity`
  sizing, and triggers inline or top-level — was rejected with the same
  "Order configuration is incomplete" error an unknown configuration key gets,
  so the endpoint drops unrecognized keys unparsed. TP/SL orders can currently
  be created only through the exchange's own UI; the SDK therefore ships no
  typed `tpsl` placement API. Re-run the probe after exchange updates.
- **`Order.price` required vs. optional**: `price` is in the schema's `required`
  list, yet the `tpsl`/`conditional` examples omit it. The SDK models `price`
  as optional.
- **Timestamps**: authenticated endpoints use epoch-millisecond integers; the
  `public/*` endpoints use ISO-8601 strings. `model::common::Timestamp`
  deserializes from either into one `OffsetDateTime`.
- **Order-book side token**: the buy side is spelled `BUYI` (not `BUY`) on the
  wire; `BookSide` maps it to `Buy`.
- **Numeric strings**: decimals and the order-book `no` count arrive as JSON
  strings and are parsed into `Decimal` / `u64`.
