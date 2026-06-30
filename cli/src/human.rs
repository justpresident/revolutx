//! Human-readable presentation of a [`CommandOutput`] — the CLI's tables, reused
//! by the interactive REPL. Pairs with the shared [`JsonPresenter`] for `--json`.

use revolutx::Result;
use revolutx::commands::{CommandOutput, JsonPresenter, Presenter};

/// Renders `output` as human text (tables) or, with `json`, as pretty JSON.
/// Returns the text block without a trailing newline (the caller prints it).
pub fn render(json: bool, output: &CommandOutput) -> Result<String> {
    if json {
        JsonPresenter.present(output)
    } else {
        HumanPresenter.present(output)
    }
}

/// Renders a [`CommandOutput`] as aligned human-readable text.
pub struct HumanPresenter;

impl Presenter for HumanPresenter {
    // A flat dispatch over result variants; each renders its own small table.
    #[allow(clippy::too_many_lines)]
    fn present(&self, output: &CommandOutput) -> Result<String> {
        let mut lines: Vec<String> = Vec::new();
        match output {
            CommandOutput::Balances(balances) => {
                lines.push(row4("CCY", "AVAILABLE", "RESERVED", "TOTAL"));
                for b in balances {
                    lines.push(row4(&b.currency, b.available, b.reserved, b.total));
                }
            }
            CommandOutput::Currencies(currencies) => {
                for (_, c) in sorted(currencies) {
                    lines.push(format!(
                        "{:<8} {:<24} scale {:<3} {:?} {:?}",
                        c.symbol, c.name, c.scale, c.asset_type, c.status
                    ));
                }
            }
            CommandOutput::Pairs(pairs) => {
                for (_, p) in sorted(pairs) {
                    // Show the hyphenated symbol the endpoints accept (the map key
                    // is the slash form), so it's the obvious thing to copy.
                    let symbol = p.symbol();
                    lines.push(format!(
                        "{symbol:<12} min {} max {} {:?}",
                        p.min_order_size, p.max_order_size, p.status
                    ));
                }
            }
            CommandOutput::Tickers(tickers) => {
                lines.push(format!(
                    "{:<12} {:>16} {:>16} {:>16}",
                    "SYMBOL", "BID", "ASK", "LAST"
                ));
                for t in &tickers.tickers {
                    lines.push(format!(
                        "{:<12} {:>16} {:>16} {:>16}",
                        t.symbol, t.bid, t.ask, t.last_price
                    ));
                }
            }
            CommandOutput::OrderBook(book) => {
                lines.push(format!("order book @ {}", book.timestamp));
                push_levels(&mut lines, book);
            }
            CommandOutput::Candles(candles) => {
                lines.push(format!(
                    "{:<16} {:>12} {:>12} {:>12} {:>12} {:>16}",
                    "START(ms)", "OPEN", "HIGH", "LOW", "CLOSE", "VOLUME"
                ));
                for c in candles {
                    lines.push(format!(
                        "{:<16} {:>12} {:>12} {:>12} {:>12} {:>16}",
                        c.start.unix_millis(),
                        c.open,
                        c.high,
                        c.low,
                        c.close,
                        c.volume
                    ));
                }
            }
            CommandOutput::LastTrades(last) => {
                for t in &last.trades {
                    lines.push(format!(
                        "{}  {} {} @ {} {}",
                        t.traded_at, t.quantity, t.asset_id, t.price, t.price_currency
                    ));
                }
            }
            CommandOutput::AllTrades(page) => {
                for t in &page.items {
                    lines.push(format!(
                        "{}  {} {} @ {} {}",
                        t.traded_at, t.quantity, t.asset_id, t.price, t.price_currency
                    ));
                }
            }
            CommandOutput::PrivateTrades(page) => {
                for f in &page.items {
                    lines.push(format!(
                        "{}  {} {} {} @ {} (maker={})",
                        f.traded_at, f.side, f.quantity, f.asset_id, f.price, f.is_maker
                    ));
                }
            }
            CommandOutput::Orders(page) => {
                lines.push(format!(
                    "{:<12} {:<5} {:<8} {:<16} {:>16} {:>16} {:>16}  {}",
                    "SYMBOL", "SIDE", "TYPE", "STATUS", "QTY", "FILLED", "PRICE", "ID"
                ));
                for o in &page.items {
                    // `price` is the limit price — absent for market orders; fall
                    // back to the average fill price so the row isn't blank.
                    let price = o
                        .price
                        .or(o.average_fill_price)
                        .map_or_else(|| "-".to_owned(), |p| p.to_string());
                    lines.push(format!(
                        "{:<12} {:<5} {:<8} {:<16} {:>16} {:>16} {:>16}  {}",
                        o.symbol,
                        o.side,
                        o.order_type.as_str(),
                        o.status.as_str(),
                        o.quantity,
                        o.filled_quantity,
                        price,
                        o.id
                    ));
                }
                if let Some(cursor) = &page.next_cursor {
                    lines.push(format!("next_cursor: {cursor}"));
                }
            }
            CommandOutput::Order(o) => {
                lines.push(format!("id:        {}", o.id));
                lines.push(format!("symbol:    {}", o.symbol));
                lines.push(format!("side/type: {} {}", o.side, o.order_type.as_str()));
                lines.push(format!("status:    {}", o.status.as_str()));
                lines.push(format!(
                    "quantity:  {} (filled {})",
                    o.quantity, o.filled_quantity
                ));
                if let Some(price) = o.price {
                    lines.push(format!("price:     {price}"));
                }
                if let Some(fee) = &o.total_fee {
                    lines.push(format!(
                        "fee:       {fee} {}",
                        o.fee_currency.as_deref().unwrap_or("")
                    ));
                }
                lines.push(format!("created:   {}", o.created_date));
            }
            CommandOutput::Fills(fills) => {
                for f in fills {
                    lines.push(format!(
                        "{}  {} {} @ {} (maker={})",
                        f.trade_id, f.quantity, f.asset_id, f.price, f.is_maker
                    ));
                }
            }
            CommandOutput::Ack(ack) => {
                lines.push(format!(
                    "order {} placed (client id {}), state {}",
                    ack.venue_order_id,
                    ack.client_order_id,
                    ack.state.as_str()
                ));
            }
            CommandOutput::Cancelled(ack) => lines.push(format!("cancelled {}", ack.order_id)),
            CommandOutput::AllCancelled(_) => lines.push("all active orders cancelled".to_owned()),
        }
        Ok(lines.join("\n"))
    }
}

fn row4(
    a: impl std::fmt::Display,
    b: impl std::fmt::Display,
    c: impl std::fmt::Display,
    d: impl std::fmt::Display,
) -> String {
    format!("{a:<8} {b:>20} {c:>20} {d:>20}")
}

fn push_levels(lines: &mut Vec<String>, book: &revolutx::model::market_data::OrderBook) {
    lines.push("  asks:".to_owned());
    for level in &book.asks {
        lines.push(format!("    {:>16} x {:>16}", level.price, level.quantity));
    }
    lines.push("  bids:".to_owned());
    for level in &book.bids {
        lines.push(format!("    {:>16} x {:>16}", level.price, level.quantity));
    }
}

/// Collects a map's entries sorted by key, for stable human output.
fn sorted<V>(map: &std::collections::HashMap<String, V>) -> Vec<(&String, &V)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
}
