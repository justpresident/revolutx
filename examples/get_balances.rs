//! Fetch and print account balances (read-only, authenticated).
//!
//! ```sh
//! export REVOLUTX_API_KEY="<your api key>"
//! export REVOLUTX_PRIVATE_KEY_PATH="private.pem"   # default: private.pem
//! cargo run --example get_balances
//! ```

use revolutx::{Environment, RevolutXClient};

#[tokio::main]
async fn main() -> revolutx::Result<()> {
    let api_key = std::env::var("REVOLUTX_API_KEY").expect("set REVOLUTX_API_KEY");
    let key_path =
        std::env::var("REVOLUTX_PRIVATE_KEY_PATH").unwrap_or_else(|_| "private.pem".to_string());
    let pem = std::fs::read_to_string(&key_path)
        .unwrap_or_else(|e| panic!("could not read private key at {key_path}: {e}"));

    let client = RevolutXClient::builder()
        .api_key(api_key)
        .private_key_pem(pem)
        .environment(Environment::Production)
        .build()?;

    println!(
        "{:<8} {:>18} {:>18} {:>18}",
        "CCY", "AVAILABLE", "RESERVED", "TOTAL"
    );
    for balance in client.balances().get_all().await? {
        println!(
            "{:<8} {:>18} {:>18} {:>18}",
            balance.currency, balance.available, balance.reserved, balance.total
        );
    }
    Ok(())
}
