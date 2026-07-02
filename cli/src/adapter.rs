//! Converts parsed clap arguments into the shared, parse-neutral command model.
//!
//! This is the CLI's only "what command did the user mean" logic; the one-shot
//! path and the interactive REPL both go through it, so they stay in lockstep.

use std::str::FromStr;

use revolutx::api::market_data::CandlesQuery;
use revolutx::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use revolutx::api::trades::TradesQuery;
use revolutx::commands::{
    Command, PlaceLimit, PlaceMarket, ReplaceOrder, candle_interval, parse_decimal,
};
use revolutx::{ClientOrderId, OrderId, Side};

use crate::args::{Command as ArgCommand, ConfigCmd, MarketCmd, OrderCmd, SideArg, TradeCmd};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// What the CLI should do with one parsed command.
#[derive(Debug)]
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
        ArgCommand::Vault { .. } | ArgCommand::Agent { .. } | ArgCommand::Cli { .. } => {
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
                size: parse_decimal("size", &size)?,
                price: parse_decimal("price", &price)?,
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
                size: parse_decimal("size", &size)?,
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
            command: Command::Replace(ReplaceOrder {
                id: OrderId::new(&id),
                size: size.map(|s| parse_decimal("size", &s)).transpose()?,
                price: price.map(|p| parse_decimal("price", &p)).transpose()?,
                in_quote: quote,
                post_only,
                client_order_id: None,
            }),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::args::Cli;
    use clap::Parser;

    /// Parses a CLI line through the real clap grammar and adapts it.
    fn adapt_line(args: &[&str]) -> Res<Action> {
        adapt(Cli::try_parse_from(args).unwrap().command)
    }

    #[test]
    fn limit_order_threads_every_flag() {
        // A dropped `--quote`/`--post-only`/`--yes` (or swapped size/price) would
        // fail here — the mapping had no coverage before.
        let action = adapt_line(&[
            "revolutx",
            "orders",
            "limit",
            "buy",
            "BTC-USD",
            "0.1",
            "50000",
            "--quote",
            "--post-only",
            "--yes",
        ])
        .unwrap();
        let Action::Run {
            command: Command::PlaceLimit(o),
            confirmed,
        } = action
        else {
            panic!("expected a PlaceLimit run action");
        };
        assert_eq!(o.symbol, "BTC-USD");
        assert_eq!(o.side, Side::Buy);
        assert_eq!(o.size, parse_decimal("size", "0.1").unwrap());
        assert_eq!(o.price, parse_decimal("price", "50000").unwrap());
        assert!(o.in_quote);
        assert!(o.post_only);
        assert!(confirmed);
    }

    #[test]
    fn a_bad_decimal_names_the_offending_field() {
        let err = adapt_line(&[
            "revolutx",
            "orders",
            "limit",
            "buy",
            "BTC-USD",
            "not-a-number",
            "50000",
            "--yes",
        ])
        .unwrap_err();
        assert!(err.to_string().contains("size"), "message was: {err}");
    }

    #[test]
    fn replace_maps_optional_size_and_price() {
        let action = adapt_line(&[
            "revolutx", "orders", "replace", "oid-1", "--price", "51000", "--yes",
        ])
        .unwrap();
        let Action::Run {
            command: Command::Replace(r),
            confirmed,
        } = action
        else {
            panic!("expected a Replace run action");
        };
        assert_eq!(r.id.as_str(), "oid-1");
        assert_eq!(r.price, Some(parse_decimal("price", "51000").unwrap()));
        assert!(r.size.is_none());
        assert!(confirmed);
    }

    #[test]
    fn market_watch_is_a_watch_action() {
        let action =
            adapt_line(&["revolutx", "market", "watch", "BTC-USD", "--interval", "5"]).unwrap();
        assert!(matches!(action, Action::Watch { interval: 5, .. }));
    }
}
