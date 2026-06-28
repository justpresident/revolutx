//! The interactive `cli` shell: unlock the vault once, then run the same
//! commands the one-shot CLI does — with history and command/symbol autocomplete.
//!
//! Each line is tokenized, parsed by the *same* clap grammar (so the shell and
//! one-shot accept identical commands), adapted to a shared `Command`, and run on
//! a single reused runtime + unlocked client. Real-trading commands prompt for
//! confirmation instead of requiring `--yes`.

use std::sync::Arc;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use revolutx::commands;
use revolutx::{AccessLevel, RevolutXClient};
use rustyline::Editor;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use tokio::runtime::Runtime;

use crate::adapter::{Action, adapt};
use crate::args::{Cli, Command as ArgCommand, GlobalOpts};
use crate::helper::ReplHelper;
use crate::human::render;
use crate::{creds, line};

type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// One line of REPL input: a per-line `--json` toggle plus the shared command
/// grammar. `no_binary_name` so the first token is the command, not argv[0].
#[derive(Parser)]
#[command(no_binary_name = true, about = "Run a revolutx command in the shell.")]
struct ReplLine {
    /// Print raw JSON for this command.
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    command: ArgCommand,
}

/// Unlocks the vault and runs the interactive shell at the session's `access`
/// tier (from `revolutx cli --access`, default `view`), which gates every line so
/// the shell can rehearse the policy an agent would enforce.
pub fn run(global: &GlobalOpts, access: AccessLevel) -> Res {
    // Unlock the vault once (prompts), then reuse the client for the session.
    let client = creds::client(global, true)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let symbols = runtime.block_on(fetch_symbols(&client));

    // `List` shows all candidates (like a shell), rather than cycling through them
    // in place on each Tab (the default `Circular`).
    let config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .build();
    let mut editor: Editor<ReplHelper, DefaultHistory> = Editor::with_config(config)?;
    editor.set_helper(Some(ReplHelper::new(symbols)));

    eprintln!("revolutx interactive shell — run a command, `help`, or `exit` (Ctrl-D to quit).");
    loop {
        match editor.readline("revolutx> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);
                if matches!(line, "exit" | "quit") {
                    break;
                }
                if let Err(e) = run_line(global, access, &runtime, &client, line) {
                    eprintln!("error: {e}");
                }
            }
            // Ctrl-C abandons the current line; Ctrl-D / EOF ends the session.
            Err(ReadlineError::Interrupted) => {}
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("error: {e}");
                break;
            }
        }
    }
    Ok(())
}

/// Parses and runs one input line, gated by the session's `access` tier.
fn run_line(
    global: &GlobalOpts,
    access: AccessLevel,
    runtime: &Runtime,
    client: &RevolutXClient,
    line: &str,
) -> Res {
    let tokens = line::tokenize(line);
    if tokens.is_empty() {
        return Ok(());
    }
    // Reuse the one-shot grammar. clap reports `--help` and parse errors via an
    // `Err`. Let clap print the error itself (the specific reason), but on a usage
    // error also print the full command help — clap's error usage is terse, and
    // the shell wants the same option list `--help` shows.
    let parsed = match ReplLine::try_parse_from(tokens.iter().map(String::as_str)) {
        Ok(parsed) => parsed,
        Err(e) => {
            use clap::error::ErrorKind::{
                DisplayHelp, DisplayHelpOnMissingArgumentOrSubcommand, DisplayVersion,
            };
            let _ = e.print();
            if !matches!(
                e.kind(),
                DisplayHelp | DisplayVersion | DisplayHelpOnMissingArgumentOrSubcommand
            ) {
                print_command_help(&tokens);
            }
            return Ok(());
        }
    };
    if matches!(
        parsed.command,
        ArgCommand::Vault { .. } | ArgCommand::Agent { .. } | ArgCommand::Cli { .. }
    ) {
        return Err("`vault`, `agent`, and `cli` are not available inside the shell".into());
    }

    let json = global.json || parsed.json;
    match adapt(parsed.command)? {
        Action::Run { command, confirmed } => {
            // The shell's `--access` tier gates each line so a policy can be
            // rehearsed; refuse before prompting for a trade we wouldn't run anyway.
            let required = command.min_access();
            if !access.permits(required) {
                return Err(revolutx::access::access_denied(required, access).into());
            }
            if command.is_real_trading() && !confirmed && !confirm()? {
                println!("cancelled.");
                return Ok(());
            }
            let output = runtime.block_on(commands::execute(client, command))?;
            println!("{}", render(json, &output)?);
        }
        Action::Watch { symbol, interval } => run_watch(json, runtime, client, &symbol, interval),
    }
    Ok(())
}

/// `market watch` in the shell: poll-and-render until the user presses **Enter**.
///
/// Ctrl-C is deliberately not the stop key — under ptrace protection a real SIGINT
/// would kill the watchdog parent and orphan the shell (which is why `main`
/// ignores SIGINT for the `cli` command). A background thread reads one line and
/// signals the poll loop to stop.
fn run_watch(json: bool, runtime: &Runtime, client: &RevolutXClient, symbol: &str, interval: u64) {
    println!("watching {symbol} — press Enter to stop.");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);
        let _ = tx.send(());
    });

    let interval = interval.max(1);
    runtime.block_on(async move {
        let poll = async {
            loop {
                let command = commands::Command::OrderBook {
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
        tokio::select! {
            () = poll => {}
            _ = rx => {}
        }
    });
    let _ = reader.join();
}

/// Prints the full help for the deepest subcommand named by the leading tokens,
/// so a usage error shows the same option list as `--help`. The command path is
/// set as the bin name so the usage line reads e.g. `trades all`, not `all`.
fn print_command_help(tokens: &[String]) {
    let mut node = Cli::command();
    let mut path: Vec<&str> = Vec::new();
    for token in tokens {
        if token.starts_with('-') {
            break;
        }
        let Some(sub) = node.find_subcommand(token).cloned() else {
            break;
        };
        path.push(token);
        node = sub;
    }
    if !path.is_empty() {
        node = node.bin_name(path.join(" "));
    }
    let _ = node.print_help();
}

/// Prompts for confirmation of a real-trading command (reads `/dev/tty`).
fn confirm() -> Res<bool> {
    Ok(rcypher::cli::read_tty_confirmation(
        "This is REAL trading. Proceed? [y/N]: ",
    )?)
}

/// Fetches the trading-pair symbols for autocomplete; degrades to none on error.
///
/// The pairs map is keyed in slash form (`BTC/USD`), but every endpoint takes the
/// hyphenated symbol (`BTC-USD`), so build that from each pair's base/quote.
async fn fetch_symbols(client: &RevolutXClient) -> Arc<[String]> {
    match client.configuration().pairs().await {
        Ok(pairs) => {
            let mut symbols: Vec<String> = pairs
                .values()
                .map(|p| format!("{}-{}", p.base, p.quote))
                .collect();
            symbols.sort();
            symbols.dedup();
            symbols.into()
        }
        Err(e) => {
            eprintln!("(symbol autocomplete unavailable: {e})");
            Vec::new().into()
        }
    }
}
