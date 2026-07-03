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
                lines.push(balance_row(
                    "CCY",
                    "AVAILABLE",
                    "RESERVED",
                    "STAKED",
                    "TOTAL",
                ));
                for b in balances {
                    // `total` includes staked funds, so a staked balance is shown
                    // explicitly — otherwise AVAILABLE + RESERVED would not equal
                    // TOTAL with no visible reason.
                    let staked = b.staked.map_or_else(|| "-".to_owned(), |s| s.to_string());
                    lines.push(balance_row(
                        &b.currency,
                        b.available,
                        b.reserved,
                        staked,
                        b.total,
                    ));
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
                    let side = f.side.map_or("-", revolutx::Side::as_str);
                    lines.push(format!(
                        "{}  {} {} {} @ {} (maker={})",
                        f.traded_at, side, f.quantity, f.asset_id, f.price, f.is_maker
                    ));
                }
            }
            CommandOutput::Orders(page) if page.items.is_empty() => {
                // A bare header row reads like a rendering bug; say it plainly.
                lines.push("(no orders)".to_owned());
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
                if let Some(t) = &o.conditional {
                    lines.push(format!("condition: {}", trigger_summary(t)));
                }
                if let Some(t) = &o.take_profit {
                    lines.push(format!("t/profit:  {}", trigger_summary(t)));
                }
                if let Some(t) = &o.stop_loss {
                    lines.push(format!("s/loss:    {}", trigger_summary(t)));
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

fn balance_row(
    ccy: impl std::fmt::Display,
    available: impl std::fmt::Display,
    reserved: impl std::fmt::Display,
    staked: impl std::fmt::Display,
    total: impl std::fmt::Display,
) -> String {
    format!("{ccy:<8} {available:>20} {reserved:>20} {staked:>20} {total:>20}")
}

/// One line describing an order trigger, e.g.
/// `limit when price >= 120000 (limit 119500), gtc`.
fn trigger_summary(t: &revolutx::model::orders::OrderTrigger) -> String {
    use revolutx::model::orders::TriggerDirection;
    let direction = match t.trigger_direction {
        TriggerDirection::Ge => ">=",
        TriggerDirection::Le => "<=",
        TriggerDirection::Unknown => "?",
    };
    let limit = t
        .limit_price
        .map(|limit| format!(" (limit {limit})"))
        .unwrap_or_default();
    format!(
        "{} when price {direction} {}{limit}, {}",
        t.trigger_type.as_str(),
        t.trigger_price,
        t.time_in_force.as_str()
    )
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use revolutx::Decimal;
    use revolutx::model::balances::Balance;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn tpsl_order_details_show_both_triggers() {
        // The contract's tpsl read example (a web-UI-created order).
        let order: revolutx::model::orders::OrderDetails = serde_json::from_str(
            r#"{
                "id": "7a52e92e-8639-4fe1-abaa-68d3a2d5234b",
                "client_order_id": "7a52e92e-8639-4fe1-abaa-68d3a2d5234b",
                "symbol": "BTC/USD",
                "side": "sell",
                "type": "tpsl",
                "quantity": "0.002",
                "filled_quantity": "0",
                "leaves_quantity": "0.002",
                "status": "new",
                "time_in_force": "gtc",
                "execution_instructions": [],
                "take_profit": {
                    "trigger_price": "0.003",
                    "type": "limit",
                    "trigger_direction": "ge",
                    "limit_price": "0.004",
                    "time_in_force": "gtc",
                    "execution_instructions": ["allow_taker"]
                },
                "stop_loss": {
                    "trigger_price": "0.001",
                    "type": "market",
                    "trigger_direction": "le",
                    "time_in_force": "ioc",
                    "execution_instructions": ["allow_taker"]
                },
                "created_date": 1770197897742,
                "updated_date": 1770197897742
            }"#,
        )
        .unwrap();
        let text = render(false, &CommandOutput::Order(Box::new(order))).unwrap();
        assert!(
            text.contains("t/profit:  limit when price >= 0.003 (limit 0.004), gtc"),
            "take-profit line missing:\n{text}"
        );
        assert!(
            text.contains("s/loss:    market when price <= 0.001, ioc"),
            "stop-loss line missing:\n{text}"
        );
    }

    #[test]
    fn balances_show_staked_and_a_dash_when_absent() {
        let output = CommandOutput::Balances(vec![
            Balance {
                currency: "BTC".into(),
                available: dec("1.0"),
                staked: Some(dec("0.5")),
                reserved: dec("0.2"),
                total: dec("1.7"),
            },
            Balance {
                currency: "USD".into(),
                available: dec("100"),
                staked: None,
                reserved: dec("0"),
                total: dec("100"),
            },
        ]);
        let text = render(false, &output).unwrap();
        assert!(text.contains("STAKED"), "header missing STAKED:\n{text}");
        assert!(text.contains("0.5"), "staked amount missing:\n{text}");
        // A currency with no staked funds renders `-`, not a blank cell.
        let usd = text.lines().find(|l| l.contains("USD")).unwrap();
        assert!(
            usd.contains('-'),
            "expected a dash for no staked funds: {usd}"
        );
    }
}
