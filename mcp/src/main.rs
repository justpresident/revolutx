//! `revolutx-mcp` — a Model Context Protocol server exposing the Revolut X
//! crypto exchange to LLM clients over stdio.
//!
//! Configuration is via environment variables (see [`server::Server::from_env`]).
//! All diagnostics go to stderr; stdout carries only newline-delimited JSON-RPC
//! messages, as required by the MCP stdio transport.
#![warn(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo,
    clippy::unwrap_used,
    clippy::panic,
    clippy::dbg_macro,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate
)]
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::multiple_crate_versions,
    clippy::missing_panics_doc
)]

mod protocol;
mod server;
mod tools;

use server::Server;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() {
    eprintln!(
        "revolutx-mcp {} starting (stdio transport)",
        env!("CARGO_PKG_VERSION")
    );

    let server = match Server::from_env() {
        Ok(server) => server,
        Err(e) => {
            eprintln!("revolutx-mcp: configuration error: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "revolutx-mcp ready (authenticated: {}, trading enabled: {})",
        server.is_authenticated(),
        server.trading_enabled()
    );

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(response) = server.handle_line(&line).await {
                    if stdout.write_all(response.as_bytes()).await.is_err()
                        || stdout.write_all(b"\n").await.is_err()
                        || stdout.flush().await.is_err()
                    {
                        eprintln!("revolutx-mcp: stdout closed, exiting");
                        break;
                    }
                }
            }
            Ok(None) => break, // EOF on stdin: client disconnected.
            Err(e) => {
                eprintln!("revolutx-mcp: stdin read error: {e}");
                break;
            }
        }
    }
}
