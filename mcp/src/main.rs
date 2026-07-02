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

use std::io;

use server::Server;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Upper bound on a single JSON-RPC line (bytes). Generous for any real request,
/// but caps memory so a newline-less flood on stdin cannot grow the buffer
/// without limit and OOM the process.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

#[tokio::main]
async fn main() {
    eprintln!(
        "revolutx-mcp {} starting (stdio transport)",
        env!("CARGO_PKG_VERSION")
    );

    let server = Server::from_env();
    eprintln!(
        "revolutx-mcp ready — call the `authenticate` tool with the agent's one-time \
         token. The agent connects lazily and may be (re)started independently."
    );

    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();

    loop {
        match read_bounded_line(&mut reader, MAX_LINE_BYTES).await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let Some(response) = server.handle_line(&line).await else {
                    continue;
                };
                if stdout.write_all(response.as_bytes()).await.is_err()
                    || stdout.write_all(b"\n").await.is_err()
                    || stdout.flush().await.is_err()
                {
                    eprintln!("revolutx-mcp: stdout closed, exiting");
                    break;
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

/// Reads one `\n`-terminated line as UTF-8, refusing to buffer more than `max`
/// bytes before a newline arrives. Unlike `AsyncBufReadExt::lines`, a client that
/// streams megabytes without a newline is rejected rather than allowed to grow
/// the buffer unbounded. Returns `Ok(None)` at EOF.
async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> io::Result<Option<String>> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // EOF: return a trailing unterminated line if any, else None.
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(newline) = available.iter().position(|&b| b == b'\n') {
            if buf.len() + newline > max {
                return Err(oversized_line());
            }
            buf.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            break;
        }
        let taken = available.len();
        if buf.len() + taken > max {
            return Err(oversized_line());
        }
        buf.extend_from_slice(available);
        reader.consume(taken);
    }
    String::from_utf8(buf).map(Some).map_err(io::Error::other)
}

fn oversized_line() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "input line exceeds the maximum size",
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{BufReader, read_bounded_line};

    #[tokio::test]
    async fn reads_lines_then_eof() {
        let mut r = BufReader::new(b"hello\nworld\n".as_slice());
        assert_eq!(
            read_bounded_line(&mut r, 1024).await.unwrap().unwrap(),
            "hello"
        );
        assert_eq!(
            read_bounded_line(&mut r, 1024).await.unwrap().unwrap(),
            "world"
        );
        assert!(read_bounded_line(&mut r, 1024).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn returns_a_trailing_unterminated_line() {
        let mut r = BufReader::new(b"abc".as_slice());
        assert_eq!(
            read_bounded_line(&mut r, 1024).await.unwrap().unwrap(),
            "abc"
        );
        assert!(read_bounded_line(&mut r, 1024).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rejects_a_line_over_the_cap() {
        // No newline within the cap → rejected rather than buffered unbounded.
        let mut r = BufReader::new(b"aaaaaaaaaaaaaaaa".as_slice());
        assert!(read_bounded_line(&mut r, 4).await.is_err());
    }
}
