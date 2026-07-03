//! Probe whether `POST /orders` accepts the `tpsl` order type. **This can
//! perform real trading** (an accepted probe creates a live order, which the
//! probe then cancels).
//!
//! The OpenAPI contract documents `tpsl` orders only for *reading* — the
//! exchange's own web UI creates them, but `OrderPlacementRequest` covers only
//! `limit`/`market`. This example sweeps candidate placement shapes through
//! `orders().place_raw()`, alongside canary probes that classify the server's
//! error behaviour: if an unknown configuration key fails with the same error
//! as the tpsl shapes, the tpsl key was never parsed.
//!
//! Probed in production on 2026-07-02: every shape below was rejected with the
//! identical "Order configuration is incomplete" error the canaries got, so
//! the endpoint drops unrecognized configuration keys unparsed and **tpsl
//! placement is not supported**. Re-run this after exchange updates.
//!
//! Every body is built so that if the server ignores its distinguishing keys
//! the request is *incomplete* — no candidate embeds a valid `limit`/`market`
//! configuration that could execute as a plain order. Triggers sit 50% away
//! from the last price, so even an accepted probe cannot fire before it is
//! cancelled.
//!
//! ```sh
//! export REVOLUTX_API_KEY="<your api key>"
//! export REVOLUTX_PRIVATE_KEY_PATH="private.pem"
//! export REVOLUTX_ENVIRONMENT="production"   # or "dev" (the default)
//! export REVOLUTX_CONFIRM_TPSL_PROBE="yes"
//! cargo run --example tpsl_probe
//! ```

use revolutx::{ClientOrderId, Decimal};
use serde_json::{Value, json};

/// The pair to probe and the (tiny) size the candidate bodies carry. A sell
/// tpsl protects a long position, so the account must hold at least this much
/// of the base currency for the exchange to even consider accepting one.
const SYMBOL: &str = "BTC-USD";
const SIZE: &str = "0.0001";

#[tokio::main]
async fn main() -> revolutx::Result<()> {
    if std::env::var("REVOLUTX_CONFIRM_TPSL_PROBE").as_deref() != Ok("yes") {
        eprintln!("Refusing to probe: set REVOLUTX_CONFIRM_TPSL_PROBE=yes to proceed.");
        return Ok(());
    }
    let client = revolutx::client_from_env()?;

    // Reference price, so the triggers sit where they cannot fire.
    let tickers = client.market_data().tickers_for(&[SYMBOL]).await?;
    let ticker = tickers.tickers.first().expect("ticker for the probe pair");
    let reference = ticker.last_price;
    let offset = reference / Decimal::TWO;
    let tp = (reference + offset).round_dp(reference.scale()).to_string();
    let sl = (reference - offset).round_dp(reference.scale()).to_string();
    println!("{SYMBOL}: last {reference}; sell probe with tp {tp}, sl {sl}");

    // Read-model-shaped triggers for a sell: take-profit fires when the price
    // rises (`ge`), stop-loss when it falls (`le`).
    let tp_trigger = json!({
        "trigger_price": tp, "type": "limit", "trigger_direction": "ge",
        "limit_price": tp, "time_in_force": "gtc",
        "execution_instructions": ["allow_taker"],
    });
    let sl_trigger = json!({
        "trigger_price": sl, "type": "market", "trigger_direction": "le",
        "time_in_force": "ioc", "execution_instructions": ["allow_taker"],
    });
    let with_config =
        |config: Value| json!({ "symbol": SYMBOL, "side": "sell", "order_configuration": config });

    // (name, body). The two canaries calibrate the error text for "the server
    // saw no recognized configuration key".
    let probes: Vec<(&str, Value)> = vec![
        ("canary/empty-config", with_config(json!({}))),
        (
            "canary/unknown-key",
            with_config(json!({ "nonexistent_probe_key": { "base_size": SIZE } })),
        ),
        (
            "tpsl/base_size",
            with_config(
                json!({ "tpsl": { "base_size": SIZE, "take_profit": tp_trigger, "stop_loss": sl_trigger } }),
            ),
        ),
        (
            "tpsl/quantity",
            with_config(
                json!({ "tpsl": { "quantity": SIZE, "take_profit": tp_trigger, "stop_loss": sl_trigger } }),
            ),
        ),
        (
            "config/triggers-inline",
            with_config(
                json!({ "base_size": SIZE, "take_profit": tp_trigger, "stop_loss": sl_trigger }),
            ),
        ),
    ];

    for (name, mut body) in probes {
        body["client_order_id"] = json!(ClientOrderId::random().to_string());
        println!("\n--- {name}\nbody: {body}");
        match client.orders().place_raw(&body).await {
            Ok(ack) => {
                println!(
                    "ACCEPTED: order {} — inspect and clean up",
                    ack.venue_order_id
                );
                let order = client.orders().get(&ack.venue_order_id).await?;
                println!(
                    "read back: {}",
                    serde_json::to_string_pretty(&order).unwrap()
                );
                client.orders().cancel(&ack.venue_order_id).await?;
                println!(
                    "cancelled. RESULT: shape `{name}` works — the contract is underspecified."
                );
                return Ok(());
            }
            Err(e) => println!("rejected: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    println!(
        "\nRESULT: all shapes rejected. If the tpsl errors match the canaries', the\n\
         server never parsed them and the public API does not support tpsl placement."
    );
    Ok(())
}
