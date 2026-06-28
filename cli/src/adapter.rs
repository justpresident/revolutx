//! Converts parsed clap arguments into the shared, parse-neutral command model.
//!
//! This is the CLI's only "what command did the user mean" logic; the one-shot
//! path and the interactive REPL both go through it, so they stay in lockstep.

use std::str::FromStr;

use revolutx::api::market_data::CandlesQuery;
use revolutx::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use revolutx::api::trades::TradesQuery;
use revolutx::commands::{Command, PlaceLimit, PlaceMarket, candle_interval};
use revolutx::model::orders::{ExecutionInstruction, OrderReplacementRequest};
use revolutx::{ClientOrderId, Decimal, OrderId, Price, Quantity, Side};

use crate::args::{Command as ArgCommand, ConfigCmd, MarketCmd, OrderCmd, SideArg, TradeCmd};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// What the CLI should do with one parsed command.
pub enum Action {
    /// Run it through the shared dispatcher. `confirmed` is whether `--yes` was
    /// supplied; real-trading commands gate on it (the one-shot path errors, the
    /// REPL prompts).
    Run { command: Command, confirmed: bool },
    /// `market watch` — a CLI-only polling loop, not a shared command.
    Watch { symbol: String, interval: u64 },
}

/// A read/market command needs no confirmation.
const fn run(command: Command) -> Action {
    Action::Run {
        command,
        confirmed: true,
    }
}

/// Maps a parsed CLI command to an [`Action`]. `Vault`/`Agent` are handled
/// before this and never reach here.
pub fn adapt(command: ArgCommand) -> Res<Action> {
    Ok(match command {
        ArgCommand::Vault { .. } | ArgCommand::Agent { .. } | ArgCommand::Cli => {
            unreachable!("vault, agent, and cli are handled before adapt")
        }
        ArgCommand::Balances => run(Command::Balances),
        ArgCommand::Config { command } => run(match command {
            ConfigCmd::Currencies => Command::Currencies,
            ConfigCmd::Pairs => Command::Pairs,
        }),
        ArgCommand::Market { command } => market(command)?,
        ArgCommand::Orders { command } => orders(command)?,
        ArgCommand::Trades { command } => run(trades(command)),
    })
}

fn market(command: MarketCmd) -> Res<Action> {
    Ok(match command {
        MarketCmd::OrderBook { symbol, limit } => run(Command::OrderBook { symbol, limit }),
        MarketCmd::PublicOrderBook { symbol } => run(Command::PublicOrderBook { symbol }),
        MarketCmd::Tickers { symbols } => run(Command::Tickers { symbols }),
        MarketCmd::Candles {
            symbol,
            interval,
            since,
            until,
        } => run(Command::Candles {
            symbol,
            query: CandlesQuery {
                interval: interval.map(candle_interval).transpose()?,
                since,
                until,
            },
        }),
        MarketCmd::LastTrades => run(Command::LastTrades),
        MarketCmd::Watch { symbol, interval } => Action::Watch { symbol, interval },
    })
}

fn trades(command: TradeCmd) -> Command {
    match command {
        TradeCmd::All {
            symbol,
            start_date,
            end_date,
            cursor,
            limit,
        } => Command::AllTrades {
            symbol,
            query: TradesQuery {
                start_date,
                end_date,
                cursor,
                limit,
            },
        },
        TradeCmd::Mine {
            symbol,
            start_date,
            end_date,
            cursor,
            limit,
        } => Command::PrivateTrades {
            symbol,
            query: TradesQuery {
                start_date,
                end_date,
                cursor,
                limit,
            },
        },
    }
}

fn orders(command: OrderCmd) -> Res<Action> {
    Ok(match command {
        OrderCmd::Active {
            symbol,
            side,
            cursor,
            limit,
        } => run(Command::ActiveOrders(ActiveOrdersQuery {
            symbols: symbol,
            side: side.map(side_of),
            cursor,
            limit,
            ..Default::default()
        })),
        OrderCmd::Historical {
            symbol,
            start_date,
            end_date,
            cursor,
            limit,
        } => run(Command::HistoricalOrders(HistoricalOrdersQuery {
            symbols: symbol,
            start_date,
            end_date,
            cursor,
            limit,
            ..Default::default()
        })),
        OrderCmd::Get { id } => run(Command::GetOrder(OrderId::new(id))),
        OrderCmd::Fills { id } => run(Command::OrderFills(OrderId::new(id))),
        OrderCmd::Limit {
            side,
            symbol,
            size,
            price,
            quote,
            post_only,
            client_order_id,
            yes,
        } => Action::Run {
            command: Command::PlaceLimit(PlaceLimit {
                symbol,
                side: side_of(side),
                size: Decimal::from_str(&size)?,
                price: Decimal::from_str(&price)?,
                in_quote: quote,
                post_only,
                client_order_id: parse_client_order_id(client_order_id)?,
            }),
            confirmed: yes,
        },
        OrderCmd::Market {
            side,
            symbol,
            size,
            quote,
            client_order_id,
            yes,
        } => Action::Run {
            command: Command::PlaceMarket(PlaceMarket {
                symbol,
                side: side_of(side),
                size: Decimal::from_str(&size)?,
                in_quote: quote,
                client_order_id: parse_client_order_id(client_order_id)?,
            }),
            confirmed: yes,
        },
        OrderCmd::Replace {
            id,
            size,
            price,
            quote,
            post_only,
            yes,
        } => Action::Run {
            command: Command::Replace {
                id: OrderId::new(&id),
                request: replacement(size, price, quote, post_only)?,
            },
            confirmed: yes,
        },
        OrderCmd::Cancel { id, yes } => Action::Run {
            command: Command::Cancel(OrderId::new(&id)),
            confirmed: yes,
        },
        OrderCmd::CancelAll { yes } => Action::Run {
            command: Command::CancelAll,
            confirmed: yes,
        },
    })
}

/// Builds an order-replacement request from the optional `--size`/`--price`
/// (validating positivity via the SDK's `Quantity`/`Price`), refusing an empty
/// replacement.
fn replacement(
    size: Option<String>,
    price: Option<String>,
    quote: bool,
    post_only: bool,
) -> Res<OrderReplacementRequest> {
    let size = size.map(|s| Decimal::from_str(&s)).transpose()?;
    let (base_size, quote_size) = match (size, quote) {
        (Some(amount), false) => (Some(Quantity::new(amount)?), None),
        (Some(amount), true) => (None, Some(Quantity::new(amount)?)),
        (None, _) => (None, None),
    };
    let price = price
        .map(|p| Decimal::from_str(&p))
        .transpose()?
        .map(Price::new)
        .transpose()?;
    if base_size.is_none() && quote_size.is_none() && price.is_none() {
        return Err("replace needs at least one of --size or --price".into());
    }
    Ok(OrderReplacementRequest {
        client_order_id: ClientOrderId::default(),
        base_size,
        quote_size,
        price,
        execution_instructions: post_only.then(|| vec![ExecutionInstruction::PostOnly]),
    })
}

const fn side_of(side: SideArg) -> Side {
    match side {
        SideArg::Buy => Side::Buy,
        SideArg::Sell => Side::Sell,
    }
}

/// Parses an optional client-order-id (a UUID); `None` lets the SDK generate one.
fn parse_client_order_id(value: Option<String>) -> Res<Option<ClientOrderId>> {
    Ok(value.map(|s| ClientOrderId::from_str(&s)).transpose()?)
}
