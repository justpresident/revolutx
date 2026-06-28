//! One-shot command driver: adapt → confirm → execute → present.
//!
//! Replaces the old hand-written dispatch; the SDK calls and result rendering
//! now live in the shared `revolutx::commands` layer (also used by the REPL and,
//! later, the MCP).

use std::time::Duration;

use revolutx::RevolutXClient;
use revolutx::commands::{self, Command as Op};

use crate::adapter::{Action, adapt};
use crate::args::{Command, GlobalOpts};
use crate::human::render;

type Res = Result<(), Box<dyn std::error::Error>>;

/// Runs a one-shot command. `vault`/`agent`/`cli` are handled in `main`.
pub async fn run(global: &GlobalOpts, command: Command, client: &RevolutXClient) -> Res {
    // Resolve `?` before any await so the non-`Send` error temporary isn't held
    // across an await point (keeps the future `Send`).
    let action = adapt(command)?;
    match action {
        Action::Run { command, confirmed } => {
            if command.is_real_trading() && !confirmed {
                return Err("refusing real trading: pass --yes to confirm this order".into());
            }
            let output = commands::execute(client, command).await?;
            println!("{}", render(global.json, &output)?);
            Ok(())
        }
        Action::Watch { symbol, interval } => watch(global.json, client, &symbol, interval).await,
    }
}

/// `market watch`: poll the order book and re-render until Ctrl-C. Tolerates
/// transient errors (log and keep going) and honors `--json`. Shared by the
/// one-shot path and the REPL (where Ctrl-C returns to the prompt).
pub async fn watch(json: bool, client: &RevolutXClient, symbol: &str, interval: u64) -> Res {
    let interval = interval.max(1);
    let poll = async {
        loop {
            match commands::execute(
                client,
                Op::OrderBook {
                    symbol: symbol.to_owned(),
                    limit: None,
                },
            )
            .await
            {
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
    tokio::select! {
        () = poll => {}
        _ = tokio::signal::ctrl_c() => eprintln!("\n(watch stopped)"),
    }
    Ok(())
}
