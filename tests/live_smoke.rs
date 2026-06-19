//! Opt-in, read-only live smoke tests against the real Revolut X API.
//!
//! Every test here is `#[ignore]`, so a plain `cargo test` never runs them and
//! they never require credentials or network access. To run them explicitly:
//!
//! ```sh
//! export REVOLUTX_API_KEY="<your api key>"
//! export REVOLUTX_PRIVATE_KEY_PEM="$(cat private.pem)"   # or REVOLUTX_PRIVATE_KEY_PATH=private.pem
//! export REVOLUTX_ENVIRONMENT="dev"                       # "dev" (default) or "production"
//! export REVOLUTX_TEST_SYMBOL="BTC-USD"                   # optional
//! cargo test --test live_smoke -- --ignored --nocapture
//! ```
//!
//! These tests are strictly read-only: they fetch balances and market data and
//! never place, replace, or cancel orders. They print only counts and shapes —
//! never key material or full secrets. If the required environment variables
//! are not set, each test prints a skip notice and returns without failing.

use revolutx::{Environment, RevolutXClient};

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn environment() -> Environment {
    match env_var("REVOLUTX_ENVIRONMENT").as_deref() {
        Some("production") | Some("prod") => Environment::Production,
        _ => Environment::Dev,
    }
}

fn test_symbol() -> String {
    env_var("REVOLUTX_TEST_SYMBOL").unwrap_or_else(|| "BTC-USD".to_string())
}

/// Builds an authenticated client from environment variables, or returns `None`
/// (with a skip notice) if credentials are not configured.
fn authenticated_client() -> Option<RevolutXClient> {
    let api_key = env_var("REVOLUTX_API_KEY")?;
    let pem = match env_var("REVOLUTX_PRIVATE_KEY_PEM") {
        Some(pem) => pem,
        None => {
            let path = env_var("REVOLUTX_PRIVATE_KEY_PATH")?;
            std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("could not read REVOLUTX_PRIVATE_KEY_PATH ({path}): {e}")
            })
        }
    };
    Some(
        RevolutXClient::builder()
            .api_key(api_key)
            .private_key_pem(pem)
            .environment(environment())
            .build()
            .expect("live client builds from provided credentials"),
    )
}

#[tokio::test]
#[ignore = "live: requires REVOLUTX_API_KEY + private key; run with --ignored"]
async fn read_only_account_and_market_data() {
    let Some(client) = authenticated_client() else {
        eprintln!(
            "skipping: set REVOLUTX_API_KEY and REVOLUTX_PRIVATE_KEY_PEM (or REVOLUTX_PRIVATE_KEY_PATH)"
        );
        return;
    };

    let balances = client.balances().get_all().await.expect("get balances");
    eprintln!("balances: {} entries", balances.len());

    let currencies = client
        .configuration()
        .currencies()
        .await
        .expect("get currencies");
    eprintln!("currencies: {}", currencies.len());

    let pairs = client.configuration().pairs().await.expect("get pairs");
    eprintln!("pairs: {}", pairs.len());

    let tickers = client.market_data().tickers().await.expect("get tickers");
    eprintln!("tickers: {}", tickers.tickers.len());

    let symbol = test_symbol();
    let book = client
        .market_data()
        .order_book(&symbol)
        .await
        .expect("get order book");
    eprintln!(
        "order book {symbol}: {} bids / {} asks",
        book.bids.len(),
        book.asks.len()
    );
}

#[tokio::test]
#[ignore = "live: hits the public (unauthenticated) endpoints; run with --ignored"]
async fn public_market_data_without_credentials() {
    let client = RevolutXClient::builder()
        .environment(environment())
        .build()
        .expect("public client builds without credentials");

    let symbol = test_symbol();
    let book = client
        .market_data()
        .public_order_book(&symbol)
        .await
        .expect("get public order book");
    eprintln!(
        "public order book {symbol}: {} bids / {} asks",
        book.bids.len(),
        book.asks.len()
    );

    let last = client
        .market_data()
        .last_trades()
        .await
        .expect("get last trades");
    eprintln!("last trades: {}", last.trades.len());
}

// NOTE: there are intentionally no live tests that place, replace, or cancel
// orders. Exercising order mutation against a real account must be done
// deliberately and separately (e.g. a local binary with its own explicit
// confirmation), never as part of the test suite.
