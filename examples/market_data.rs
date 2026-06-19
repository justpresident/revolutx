//! Read public market data — no credentials required.
//!
//! ```sh
//! cargo run --example market_data -- BTC-USD
//! ```

use revolutx::RevolutXClient;

#[tokio::main]
async fn main() -> revolutx::Result<()> {
    // The public market-data endpoints are unauthenticated, so the client can
    // be built without an API key or private key.
    let client = RevolutXClient::builder().build()?;

    let symbol = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "BTC-USD".to_string());

    let book = client.market_data().public_order_book(&symbol).await?;
    println!(
        "{symbol} order book @ {}: {} bids, {} asks",
        book.timestamp,
        book.bids.len(),
        book.asks.len()
    );
    if let Some(bid) = book.bids.first() {
        println!("  top bid: {} x {}", bid.price, bid.quantity);
    }
    if let Some(ask) = book.asks.first() {
        println!("  top ask: {} x {}", ask.price, ask.quantity);
    }

    let last = client.market_data().last_trades().await?;
    println!("{} recent public trades", last.trades.len());
    if let Some(trade) = last.trades.first() {
        println!(
            "  latest: {} {} {} @ {}",
            trade.quantity, trade.asset_id, trade.price, trade.traded_at
        );
    }

    Ok(())
}
