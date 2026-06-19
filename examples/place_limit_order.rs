//! Place a small post-only limit order. **This performs real trading.**
//!
//! It is guarded twice: it targets the dev environment by default and refuses
//! to run unless `REVOLUTX_CONFIRM_PLACE_ORDER=yes` is set. Review the price and
//! size below before running — the SDK does not manage trading risk for you.
//!
//! ```sh
//! export REVOLUTX_API_KEY="<your api key>"
//! export REVOLUTX_PRIVATE_KEY_PATH="private.pem"
//! export REVOLUTX_CONFIRM_PLACE_ORDER="yes"
//! cargo run --example place_limit_order
//! ```

use std::str::FromStr;

use revolutx::{Decimal, Environment, RevolutXClient};

#[tokio::main]
async fn main() -> revolutx::Result<()> {
    if std::env::var("REVOLUTX_CONFIRM_PLACE_ORDER").as_deref() != Ok("yes") {
        eprintln!(
            "Refusing to place a real order. Set REVOLUTX_CONFIRM_PLACE_ORDER=yes to proceed."
        );
        return Ok(());
    }

    let api_key = std::env::var("REVOLUTX_API_KEY").expect("set REVOLUTX_API_KEY");
    let key_path =
        std::env::var("REVOLUTX_PRIVATE_KEY_PATH").unwrap_or_else(|_| "private.pem".to_string());
    let pem = std::fs::read_to_string(&key_path)
        .unwrap_or_else(|e| panic!("could not read private key at {key_path}: {e}"));

    let client = RevolutXClient::builder()
        .api_key(api_key)
        .private_key_pem(pem)
        // Dev environment by default; switch to Production deliberately.
        .environment(Environment::Dev)
        .build()?;

    // A small, far-from-market, post-only buy: unlikely to fill immediately.
    let ack = client
        .orders()
        .limit_buy(
            "BTC-USD",
            Decimal::from_str("0.0001").unwrap(),
            Decimal::from_str("1000").unwrap(),
        )
        .post_only()
        .send()
        .await?;

    println!(
        "placed order {} (client id {}), state {:?}",
        ack.venue_order_id, ack.client_order_id, ack.state
    );
    Ok(())
}
