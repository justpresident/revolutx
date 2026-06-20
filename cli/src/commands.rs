//! Async command handlers: parsed args → SDK calls → human or JSON output.

use std::str::FromStr;
use std::time::Duration;

use revolutx::api::market_data::{CandleInterval, CandlesQuery};
use revolutx::api::orders::{ActiveOrdersQuery, HistoricalOrdersQuery};
use revolutx::api::trades::TradesQuery;
use revolutx::model::market_data::OrderBook;
use revolutx::model::orders::{Order, OrderAck};
use revolutx::{Decimal, OrderId, Page, RevolutXClient, Side};

use crate::args::{Command, ConfigCmd, GlobalOpts, MarketCmd, OrderCmd, SideArg, TradeCmd};
use crate::output;

type Res = Result<(), Box<dyn std::error::Error>>;

/// Dispatches a one-shot command. `vault` and `agent` are handled in `main`.
pub async fn run(global: &GlobalOpts, command: Command, client: &RevolutXClient) -> Res {
    match command {
        Command::Vault { .. } | Command::Agent { .. } => {
            unreachable!("vault and agent are handled before the runtime")
        }
        Command::Balances => balances(global, client).await,
        Command::Config { command } => config(global, client, command).await,
        Command::Market { command } => market(global, client, command).await,
        Command::Orders { command } => orders(global, client, command).await,
        Command::Trades { command } => trades(global, client, command).await,
    }
}

async fn balances(global: &GlobalOpts, client: &RevolutXClient) -> Res {
    let balances = client.balances().get_all().await?;
    if global.json {
        return output::json(&balances);
    }
    println!(
        "{:<8} {:>20} {:>20} {:>20}",
        "CCY", "AVAILABLE", "RESERVED", "TOTAL"
    );
    for b in &balances {
        println!(
            "{:<8} {:>20} {:>20} {:>20}",
            b.currency, b.available, b.reserved, b.total
        );
    }
    Ok(())
}

async fn config(global: &GlobalOpts, client: &RevolutXClient, command: ConfigCmd) -> Res {
    match command {
        ConfigCmd::Currencies => {
            let map = client.configuration().currencies().await?;
            if global.json {
                return output::json(&map);
            }
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (_, c) in entries {
                println!(
                    "{:<8} {:<24} scale {:<3} {:?} {:?}",
                    c.symbol, c.name, c.scale, c.asset_type, c.status
                );
            }
            Ok(())
        }
        ConfigCmd::Pairs => {
            let map = client.configuration().pairs().await?;
            if global.json {
                return output::json(&map);
            }
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (k, p) in entries {
                println!(
                    "{:<12} {}/{}  min {} max {} {:?}",
                    k, p.base, p.quote, p.min_order_size, p.max_order_size, p.status
                );
            }
            Ok(())
        }
    }
}

async fn market(global: &GlobalOpts, client: &RevolutXClient, command: MarketCmd) -> Res {
    match command {
        MarketCmd::OrderBook { symbol, limit } => {
            let book = match limit {
                Some(limit) => {
                    client
                        .market_data()
                        .order_book_with_limit(&symbol, limit)
                        .await?
                }
                None => client.market_data().order_book(&symbol).await?,
            };
            print_book(global, &symbol, &book)
        }
        MarketCmd::PublicOrderBook { symbol } => {
            let book = client.market_data().public_order_book(&symbol).await?;
            print_book(global, &symbol, &book)
        }
        MarketCmd::Tickers { symbols } => {
            let tickers = if symbols.is_empty() {
                client.market_data().tickers().await?
            } else {
                client.market_data().tickers_for(&symbols).await?
            };
            if global.json {
                return output::json(&tickers);
            }
            println!(
                "{:<12} {:>16} {:>16} {:>16}",
                "SYMBOL", "BID", "ASK", "LAST"
            );
            for t in &tickers.tickers {
                println!(
                    "{:<12} {:>16} {:>16} {:>16}",
                    t.symbol, t.bid, t.ask, t.last_price
                );
            }
            Ok(())
        }
        MarketCmd::Candles {
            symbol,
            interval,
            since,
            until,
        } => {
            let query = CandlesQuery {
                interval: interval.map(candle_interval).transpose()?,
                since,
                until,
            };
            let candles = client.market_data().candles(&symbol, &query).await?;
            if global.json {
                return output::json(&candles);
            }
            println!(
                "{:<16} {:>12} {:>12} {:>12} {:>12} {:>16}",
                "START(ms)", "OPEN", "HIGH", "LOW", "CLOSE", "VOLUME"
            );
            for c in &candles {
                println!(
                    "{:<16} {:>12} {:>12} {:>12} {:>12} {:>16}",
                    c.start.unix_millis(),
                    c.open,
                    c.high,
                    c.low,
                    c.close,
                    c.volume
                );
            }
            Ok(())
        }
        MarketCmd::LastTrades => {
            let last = client.market_data().last_trades().await?;
            if global.json {
                return output::json(&last);
            }
            for t in &last.trades {
                println!(
                    "{}  {} {} @ {} {}",
                    t.traded_at, t.quantity, t.asset_id, t.price, t.price_currency
                );
            }
            Ok(())
        }
        MarketCmd::Watch { symbol, interval } => loop {
            let book = client.market_data().order_book(&symbol).await?;
            println!("--- {symbol} @ {} ---", book.timestamp);
            print_book_levels(&book);
            tokio::time::sleep(Duration::from_secs(interval)).await;
        },
    }
}

async fn orders(global: &GlobalOpts, client: &RevolutXClient, command: OrderCmd) -> Res {
    match command {
        OrderCmd::Active { symbol, limit } => {
            let query = ActiveOrdersQuery {
                symbols: symbol,
                limit,
                ..Default::default()
            };
            let page = client.orders().active(&query).await?;
            print_orders(global, &page)
        }
        OrderCmd::Historical { symbol, limit } => {
            let query = HistoricalOrdersQuery {
                symbols: symbol,
                limit,
                ..Default::default()
            };
            let page = client.orders().historical(&query).await?;
            print_orders(global, &page)
        }
        OrderCmd::Get { id } => {
            let order = client.orders().get(&OrderId::new(id)).await?;
            if global.json {
                return output::json(&order);
            }
            print_order(&order);
            Ok(())
        }
        OrderCmd::Fills { id } => {
            let fills = client.orders().fills(&OrderId::new(id)).await?;
            if global.json {
                return output::json(&fills);
            }
            for f in &fills {
                println!(
                    "{}  {} {} @ {} (maker={})",
                    f.trade_id, f.quantity, f.asset_id, f.price, f.is_maker
                );
            }
            Ok(())
        }
        OrderCmd::Limit {
            side,
            symbol,
            size,
            price,
            quote,
            post_only,
            yes,
        } => {
            confirm(yes, "place a limit order")?;
            let size = Decimal::from_str(&size)?;
            let price = Decimal::from_str(&price)?;
            let mut builder = match (side_of(side), quote) {
                (Side::Buy, false) => client.orders().limit_buy(symbol, size, price),
                (Side::Buy, true) => client.orders().limit_buy_quote(symbol, size, price),
                (Side::Sell, false) => client.orders().limit_sell(symbol, size, price),
                (Side::Sell, true) => client.orders().limit_sell_quote(symbol, size, price),
            };
            if post_only {
                builder = builder.post_only();
            }
            let ack = builder.send().await?;
            print_ack(global, &ack)
        }
        OrderCmd::Market {
            side,
            symbol,
            size,
            quote,
            yes,
        } => {
            confirm(yes, "place a market order")?;
            let size = Decimal::from_str(&size)?;
            let builder = match (side_of(side), quote) {
                (Side::Buy, false) => client.orders().market_buy(symbol, size),
                (Side::Buy, true) => client.orders().market_buy_quote(symbol, size),
                (Side::Sell, false) => client.orders().market_sell(symbol, size),
                (Side::Sell, true) => client.orders().market_sell_quote(symbol, size),
            };
            let ack = builder.send().await?;
            print_ack(global, &ack)
        }
        OrderCmd::Cancel { id, yes } => {
            confirm(yes, "cancel an order")?;
            client.orders().cancel(&OrderId::new(&id)).await?;
            println!("cancelled {id}");
            Ok(())
        }
        OrderCmd::CancelAll { yes } => {
            confirm(yes, "cancel ALL active orders")?;
            client.orders().cancel_all().await?;
            println!("all active orders cancelled");
            Ok(())
        }
    }
}

async fn trades(global: &GlobalOpts, client: &RevolutXClient, command: TradeCmd) -> Res {
    match command {
        TradeCmd::All { symbol, limit } => {
            let query = TradesQuery {
                limit,
                ..Default::default()
            };
            let page = client.trades().all(&symbol, &query).await?;
            if global.json {
                return output::json(&page);
            }
            for t in &page.items {
                println!(
                    "{}  {} {} @ {} {}",
                    t.traded_at, t.quantity, t.asset_id, t.price, t.price_currency
                );
            }
            Ok(())
        }
        TradeCmd::Mine { symbol, limit } => {
            let query = TradesQuery {
                limit,
                ..Default::default()
            };
            let page = client.trades().private(&symbol, &query).await?;
            if global.json {
                return output::json(&page);
            }
            for f in &page.items {
                println!(
                    "{}  {} {} {} @ {} (maker={})",
                    f.traded_at, f.side, f.quantity, f.asset_id, f.price, f.is_maker
                );
            }
            Ok(())
        }
    }
}

// --- helpers ---------------------------------------------------------------

fn confirm(yes: bool, action: &str) -> Res {
    if yes {
        Ok(())
    } else {
        Err(format!("refusing to {action}: this is real trading — pass --yes to confirm").into())
    }
}

const fn side_of(side: SideArg) -> Side {
    match side {
        SideArg::Buy => Side::Buy,
        SideArg::Sell => Side::Sell,
    }
}

fn candle_interval(minutes: i64) -> Result<CandleInterval, Box<dyn std::error::Error>> {
    use CandleInterval::{
        FifteenMinutes, FiveMinutes, FourDays, FourHours, FourWeeks, OneDay, OneHour, OneMinute,
        OneWeek, ThirtyMinutes, TwoDays, TwoWeeks,
    };
    Ok(match minutes {
        1 => OneMinute,
        5 => FiveMinutes,
        15 => FifteenMinutes,
        30 => ThirtyMinutes,
        60 => OneHour,
        240 => FourHours,
        1440 => OneDay,
        2880 => TwoDays,
        5760 => FourDays,
        10080 => OneWeek,
        20160 => TwoWeeks,
        40320 => FourWeeks,
        other => {
            return Err(format!(
                "invalid interval {other} (allowed: 1,5,15,30,60,240,1440,2880,5760,10080,20160,40320)"
            )
            .into());
        }
    })
}

fn print_book(global: &GlobalOpts, symbol: &str, book: &OrderBook) -> Res {
    if global.json {
        return output::json(book);
    }
    println!("{symbol} order book @ {}", book.timestamp);
    print_book_levels(book);
    Ok(())
}

fn print_book_levels(book: &OrderBook) {
    println!("  asks:");
    for level in &book.asks {
        println!("    {:>16} x {:>16}", level.price, level.quantity);
    }
    println!("  bids:");
    for level in &book.bids {
        println!("    {:>16} x {:>16}", level.price, level.quantity);
    }
}

fn print_orders(global: &GlobalOpts, page: &Page<Order>) -> Res {
    if global.json {
        return output::json(page);
    }
    println!(
        "{:<38} {:<5} {:<8} {:<16} {:>16} {:>16}",
        "ID", "SIDE", "TYPE", "STATUS", "QTY", "PRICE"
    );
    for o in &page.items {
        println!(
            "{:<38} {:<5} {:<8} {:<16} {:>16} {:>16}",
            o.id,
            o.side,
            o.order_type.as_str(),
            o.status.as_str(),
            o.quantity,
            o.price.map(|p| p.to_string()).unwrap_or_default()
        );
    }
    if let Some(cursor) = &page.next_cursor {
        println!("next_cursor: {cursor}");
    }
    Ok(())
}

fn print_order(o: &Order) {
    println!("id:        {}", o.id);
    println!("symbol:    {}", o.symbol);
    println!("side/type: {} {}", o.side, o.order_type.as_str());
    println!("status:    {}", o.status.as_str());
    println!("quantity:  {} (filled {})", o.quantity, o.filled_quantity);
    if let Some(price) = o.price {
        println!("price:     {price}");
    }
    if let Some(fee) = &o.total_fee {
        println!(
            "fee:       {fee} {}",
            o.fee_currency.as_deref().unwrap_or("")
        );
    }
    println!("created:   {}", o.created_date);
}

fn print_ack(global: &GlobalOpts, ack: &OrderAck) -> Res {
    if global.json {
        return output::json(ack);
    }
    println!(
        "order {} placed (client id {}), state {}",
        ack.venue_order_id,
        ack.client_order_id,
        ack.state.as_str()
    );
    Ok(())
}
