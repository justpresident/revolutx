//! Shared `market watch` poll loop for the one-shot path and the interactive REPL.
//!
//! Both surfaces poll the order book on an interval and stop when the user presses
//! **Enter**. Ctrl-C is deliberately not the stop key: `market watch` needs
//! credentials, so it runs under the ptrace/SIGINT hardening `main` installs — a
//! real SIGINT is blocked (or would kill the watchdog parent), which is why the
//! previous one-shot Ctrl-C handler was unreachable. Reading a line is the
//! reliable, hardening-safe stop signal.

use std::future::Future;
use std::time::Duration;

use revolutx::RevolutXClient;
use revolutx::commands::{self, Command};

use crate::human::render;

/// Polls the order book for `symbol` every `interval` seconds (minimum 1),
/// rendering each snapshot, until `stop` resolves. Transient errors are logged
/// and the loop continues. The single home of the watch loop, shared so the
/// one-shot path and the REPL cannot drift.
pub async fn poll_order_book(
    json: bool,
    client: &RevolutXClient,
    symbol: &str,
    interval: u64,
    stop: impl Future<Output = ()>,
) {
    let interval = interval.max(1);
    let poll = async {
        loop {
            let command = Command::OrderBook {
                symbol: symbol.to_owned(),
                limit: None,
            };
            match commands::execute(client, command).await {
                Ok(output) => {
                    if !json {
                        println!("--- {symbol} ---");
                    }
                    match render(json, &output) {
                        Ok(text) => println!("{text}"),
                        Err(e) => eprintln!("watch: {e}"),
                    }
                }
                Err(e) => eprintln!("watch: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
    };
    tokio::pin!(stop);
    tokio::select! {
        () = poll => {}
        () = stop => {}
    }
}

/// A future that resolves when the user presses Enter (reads one line on the
/// blocking pool). Used as the stop signal for [`poll_order_book`].
pub async fn enter_pressed() {
    let _ = tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);
    })
    .await;
}
