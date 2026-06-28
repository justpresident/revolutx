//! The rustyline helper: command/symbol autocomplete for the REPL. Hinting,
//! highlighting, and validation use the defaults.

use std::sync::Arc;

use clap::CommandFactory;
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

use crate::args::Cli;

/// Completes commands (from the clap grammar) and trading symbols (fetched once
/// at REPL start).
pub struct ReplHelper {
    symbols: Arc<[String]>,
}

impl ReplHelper {
    pub const fn new(symbols: Arc<[String]>) -> Self {
        Self { symbols }
    }

    /// The candidate completions for the word being typed, given the already
    /// complete tokens before it.
    fn candidates(&self, prior: &[&str], word: &str) -> Vec<String> {
        let cli = Cli::command();

        // First word: a top-level command (or a shell built-in).
        let Some((head, rest)) = prior.split_first() else {
            let mut names = subcommand_names(&cli);
            names.extend(["help".to_owned(), "exit".to_owned(), "quit".to_owned()]);
            return names;
        };

        let Some(top) = cli.find_subcommand(head) else {
            return Vec::new();
        };
        let has_subs = top.get_subcommands().next().is_some();
        // The command node whose flags apply: the subcommand if one was given.
        let node = rest
            .first()
            .and_then(|sub| top.find_subcommand(sub))
            .unwrap_or(top);

        // A flag.
        if word.starts_with('-') {
            return node
                .get_arguments()
                .filter_map(|arg| arg.get_long().map(|long| format!("--{long}")))
                .collect();
        }
        // The subcommand-name position (e.g. after `market`).
        if has_subs && rest.is_empty() {
            return subcommand_names(top);
        }
        // A positional that takes a symbol → offer the cached symbols.
        if takes_symbol(node) {
            return self.symbols.to_vec();
        }
        Vec::new()
    }
}

fn subcommand_names(command: &clap::Command) -> Vec<String> {
    command
        .get_subcommands()
        .map(|s| s.get_name().to_owned())
        .collect()
}

/// Whether `command` has a positional argument that is a trading symbol.
fn takes_symbol(command: &clap::Command) -> bool {
    command.get_positionals().any(|arg| {
        let id = arg.get_id().as_str();
        id == "symbol" || id == "symbols"
    })
}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let head = &line[..pos];
        let word_start = head.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        let word = &head[word_start..];
        let prior: Vec<&str> = head[..word_start].split_whitespace().collect();

        let pairs = self
            .candidates(&prior, word)
            .into_iter()
            .filter(|c| c.starts_with(word))
            .map(|c| Pair {
                display: c.clone(),
                replacement: c,
            })
            .collect();
        Ok((word_start, pairs))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;
}
impl Highlighter for ReplHelper {}
impl Validator for ReplHelper {}
impl Helper for ReplHelper {}
