//! The single dispatcher: a [`Command`] against a [`RevolutXClient`].

use crate::RevolutXClient;
use crate::Side;
use crate::error::{Error, Result};
use crate::model::common::{Price, Quantity};
use crate::model::orders::{ExecutionInstruction, OrderAck, OrderReplacementRequest};

use super::command::{Command, PlaceLimit, PlaceMarket, ReplaceOrder};
use super::output::{CancelAck, CancelAllAck, CommandOutput};

/// Runs `command` against `client`, returning its structured result.
///
/// This is the one place a command maps to SDK calls; it does **no**
/// presentation, confirmation, or `--json` handling — those belong to the
/// calling surface.
pub async fn execute(client: &RevolutXClient, command: Command) -> Result<CommandOutput> {
    Ok(match command {
        Command::Balances => CommandOutput::Balances(client.balances().get_all().await?),
        Command::Currencies => {
            CommandOutput::Currencies(client.configuration().currencies().await?)
        }
        Command::Pairs => CommandOutput::Pairs(client.configuration().pairs().await?),
        Command::Tickers { symbols } => {
            let tickers = if symbols.is_empty() {
                client.market_data().tickers().await?
            } else {
                client.market_data().tickers_for(&symbols).await?
            };
            CommandOutput::Tickers(tickers)
        }
        Command::OrderBook { symbol, limit } => {
            let book = match limit {
                Some(limit) => {
                    client
                        .market_data()
                        .order_book_with_limit(&symbol, limit)
                        .await?
                }
                None => client.market_data().order_book(&symbol).await?,
            };
            CommandOutput::OrderBook(Box::new(book))
        }
        Command::PublicOrderBook { symbol } => CommandOutput::OrderBook(Box::new(
            client.market_data().public_order_book(&symbol).await?,
        )),
        Command::Candles { symbol, query } => {
            CommandOutput::Candles(client.market_data().candles(&symbol, &query).await?)
        }
        Command::LastTrades => CommandOutput::LastTrades(client.market_data().last_trades().await?),
        Command::AllTrades { symbol, query } => {
            CommandOutput::AllTrades(client.trades().all(&symbol, &query).await?)
        }
        Command::PrivateTrades { symbol, query } => {
            // Same server range rules as historical orders (30-day cap, end
            // defaulting to start + 7 days): an explicit start without a
            // manual cursor walks the whole range.
            let page = if query.start_date.is_some() && query.cursor.is_none() {
                client.trades().private_range(&symbol, &query).await?
            } else {
                client.trades().private(&symbol, &query).await?
            };
            CommandOutput::PrivateTrades(page)
        }
        Command::ActiveOrders(query) => {
            CommandOutput::Orders(client.orders().active(&query).await?)
        }
        Command::HistoricalOrders(query) => {
            // One query answers at most 30 days, and a missing end_date
            // defaults server-side to start_date + 7 days — so "orders since
            // May" would be silently truncated. With an explicit start and no
            // manual cursor, walk the whole range; a cursor query is manual
            // pagination and passes through verbatim.
            let page = if query.start_date.is_some() && query.cursor.is_none() {
                client.orders().historical_range(&query).await?
            } else {
                client.orders().historical(&query).await?
            };
            CommandOutput::Orders(page)
        }
        Command::GetOrder(id) => CommandOutput::Order(Box::new(client.orders().get(&id).await?)),
        Command::OrderFills(id) => CommandOutput::Fills(client.orders().fills(&id).await?),
        Command::PlaceLimit(order) => {
            CommandOutput::Ack(Box::new(place_limit(client, order).await?))
        }
        Command::PlaceMarket(order) => {
            CommandOutput::Ack(Box::new(place_market(client, order).await?))
        }
        Command::Replace(order) => {
            let (id, request) = build_replacement(order)?;
            CommandOutput::Ack(Box::new(client.orders().replace(&id, &request).await?))
        }
        Command::Cancel(id) => {
            client.orders().cancel(&id).await?;
            CommandOutput::Cancelled(CancelAck::new(&id))
        }
        Command::CancelAll => {
            client.orders().cancel_all().await?;
            CommandOutput::AllCancelled(CancelAllAck::new())
        }
    })
}

async fn place_limit(client: &RevolutXClient, order: PlaceLimit) -> Result<OrderAck> {
    let PlaceLimit {
        symbol,
        side,
        size,
        price,
        in_quote,
        post_only,
        client_order_id,
    } = order;
    let mut builder = match (side, in_quote) {
        (Side::Buy, false) => client.orders().limit_buy(symbol, size, price),
        (Side::Buy, true) => client.orders().limit_buy_quote(symbol, size, price),
        (Side::Sell, false) => client.orders().limit_sell(symbol, size, price),
        (Side::Sell, true) => client.orders().limit_sell_quote(symbol, size, price),
    };
    if post_only {
        builder = builder.post_only();
    }
    if let Some(id) = client_order_id {
        builder = builder.client_order_id(id);
    }
    builder.send().await
}

/// Assembles a validated [`OrderReplacementRequest`] from a [`ReplaceOrder`],
/// enforcing "at least one of size/price" — the single place any surface's
/// replace command is turned into a request (the CLI and MCP only parse input).
fn build_replacement(order: ReplaceOrder) -> Result<(crate::OrderId, OrderReplacementRequest)> {
    let ReplaceOrder {
        id,
        size,
        price,
        in_quote,
        post_only,
        client_order_id,
    } = order;
    let (base_size, quote_size) = match (size, in_quote) {
        (Some(amount), false) => (Some(Quantity::new(amount)?), None),
        (Some(amount), true) => (None, Some(Quantity::new(amount)?)),
        (None, _) => (None, None),
    };
    let price = price.map(Price::new).transpose()?;
    if base_size.is_none() && quote_size.is_none() && price.is_none() {
        return Err(Error::invalid_request(
            "replace needs at least one of size or price",
        ));
    }
    let request = OrderReplacementRequest {
        client_order_id: client_order_id.unwrap_or_default(),
        base_size,
        quote_size,
        price,
        execution_instructions: post_only.then(|| vec![ExecutionInstruction::PostOnly]),
    };
    Ok((id, request))
}

async fn place_market(client: &RevolutXClient, order: PlaceMarket) -> Result<OrderAck> {
    let PlaceMarket {
        symbol,
        side,
        size,
        in_quote,
        client_order_id,
    } = order;
    let mut builder = match (side, in_quote) {
        (Side::Buy, false) => client.orders().market_buy(symbol, size),
        (Side::Buy, true) => client.orders().market_buy_quote(symbol, size),
        (Side::Sell, false) => client.orders().market_sell(symbol, size),
        (Side::Sell, true) => client.orders().market_sell_quote(symbol, size),
    };
    if let Some(id) = client_order_id {
        builder = builder.client_order_id(id);
    }
    builder.send().await
}
