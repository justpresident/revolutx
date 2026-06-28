//! High-level command layer (`commands` feature).
//!
//! One parse-neutral [`Command`] model, one [`execute`] dispatcher onto a
//! [`RevolutXClient`](crate::RevolutXClient), one structured [`CommandOutput`],
//! and a [`Presenter`] seam. Surfaces (the CLI one-shot path, the interactive
//! REPL, and the MCP) parse their own input into a `Command` and present the
//! `CommandOutput` their own way; everything in between is shared, so the command
//! set and its dispatch live in exactly one place.

mod command;
mod execute;
mod output;
mod present;

#[cfg(test)]
mod tests;

pub use command::{Command, PlaceLimit, PlaceMarket, candle_interval};
pub use execute::execute;
pub use output::{CancelAck, CancelAllAck, CommandOutput};
pub use present::{JsonPresenter, Presenter};
