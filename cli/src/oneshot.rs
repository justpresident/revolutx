//! One-shot command driver: adapt → confirm → execute → present.
//!
//! Replaces the old hand-written dispatch; the SDK calls and result rendering
//! now live in the shared `revolutx::commands` layer (also used by the REPL and,
//! later, the MCP).

use revolutx::RevolutXClient;
use revolutx::commands;

use crate::adapter::{Action, adapt};
use crate::args::{Command, GlobalOpts};
use crate::human::render;
use crate::watch::{enter_pressed, poll_order_book};

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
        Action::Watch { symbol, interval } => {
            // `market watch` needs credentials, so it runs under the SIGINT
            // hardening `main` installs; Enter (not Ctrl-C) is the stop signal.
            println!("watching {symbol} — press Enter to stop.");
            poll_order_book(global.json, client, &symbol, interval, enter_pressed()).await;
            Ok(())
        }
    }
}
