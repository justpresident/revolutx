//! The `revolutx agent start` subcommand: a persistent, multi-client signing agent
//! with an interactive operator console.
//!
//! It unlocks the vault once (interactive password) and serves a full proxy over a
//! unix socket — it signs and performs every forwarded request, so the private key
//! and API key never leave this process. It serves **many** connections at once;
//! each is authorized either by a one-time token (`--auth-token`, e.g. for the MCP)
//! or interactively from this console (`list`, `grant <id> [tier]`, `deny <id>`).
//! The operator sees each peer's uid/gid/pid before deciding.
//!
//! A dedicated watchdog thread re-checks for an attached debugger and enforces the
//! idle timeout (auto-lock once no client is authorized for the timeout). It is
//! spawned only after `main` has hardened the process (and after the
//! `enable_ptrace_protection` fork) and before the async runtime starts — forking a
//! multithreaded process is undefined behavior.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use revolutx::AccessLevel;
use revolutx::agent::{
    AgentControl, AgentServer, AuthMethod, AuthToken, ConnState, default_socket_path,
};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use tokio::sync::Notify;

use crate::agentlog;
use crate::args::{AgentCmd, GlobalOpts};
use crate::creds;
use crate::term::{RestoreOnDrop, SavedTerminal};

/// Operator-console command names offered by completion (aliases like `ls`/`?`
/// still work but are not suggested).
const CONSOLE_COMMANDS: [&str; 5] = ["list", "grant", "deny", "help", "quit"];
/// Access tiers completed after `grant <id>`.
const CONSOLE_TIERS: [&str; 3] = ["market", "view", "trading"];

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Runs an `agent` subcommand.
pub fn run(global: &GlobalOpts, command: AgentCmd) -> Res<()> {
    match command {
        AgentCmd::Start {
            socket,
            auth_token,
            idle_timeout,
            access,
        } => start(global, socket, auth_token, idle_timeout, access.into()),
    }
}

fn start(
    global: &GlobalOpts,
    socket: Option<PathBuf>,
    auth_token: bool,
    idle_timeout: u64,
    access: AccessLevel,
) -> Res<()> {
    let socket_path = socket.unwrap_or_else(default_socket_path);

    // The console below holds the terminal in raw mode while blocked in
    // readline; every exit path must restore it or the user's shell is left
    // raw and the farewell prints garbled. Capture the state up front; the
    // guard covers panic unwinds.
    let terminal = SavedTerminal::capture();
    let _restore_on_panic = RestoreOnDrop(terminal);

    // Open the optional event log (REVOLUTX_AGENT_LOG) and route panics through
    // a hook that restores the terminal AND records the panic — so an abnormal
    // exit is diagnosable from the file even when the console is wrecked.
    agentlog::init();
    install_panic_hook(terminal);

    // An optional one-time token for headless clients (e.g. the MCP). When omitted,
    // connections are authorized only interactively from the console below.
    let token = if auth_token {
        Some(AuthToken::generate()?)
    } else {
        None
    };

    // Unlock the vault (prompts for the master password) BEFORE building the
    // runtime — the hardening + ptrace fork already happened in `main`.
    let client = creds::client(global, true)?;
    let executor = client.executor();

    // Print the token (if any) before it moves into the server; it never appears
    // as a CLI value, so the operator copies it out of band.
    if let Some(token) = &token {
        print_token(token.as_str());
    }
    let (server, control) = AgentServer::new(executor, access, token);

    // The watchdog shuts down *gracefully* (so the keystore key zeroizes on Drop);
    // only a debugger/clock anomaly hard-exits immediately.
    let idle_shutdown = Arc::new(Notify::new());
    spawn_watchdog(
        !global.insecure_allow_debugging,
        control.clone(),
        idle_timeout,
        socket_path.clone(),
        Arc::clone(&idle_shutdown),
        terminal,
    );
    // The operator console reads stdin on its own thread and drives `control`.
    spawn_console(control);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    eprintln!(
        "revolutx-agent: listening on {} (access ceiling: {})",
        socket_path.display(),
        access.as_str(),
    );
    eprintln!(
        "revolutx-agent: operator console ready — type `help` (list, grant <id> [tier], deny <id>, quit)"
    );
    agentlog::event(&format!(
        "listening: socket={} access={}",
        socket_path.display(),
        access.as_str(),
    ));
    let result: Res<Option<&str>> = runtime.block_on(async {
        tokio::select! {
            outcome = server.run(&socket_path) => outcome.map(|()| None).map_err(Into::into),
            _ = tokio::signal::ctrl_c() => Ok(Some("shutting down")),
            () = idle_shutdown.notified() => Ok(Some("idle timeout — locking and exiting")),
        }
    });
    match &result {
        Ok(Some(reason)) => agentlog::event(&format!("shutdown: {reason}")),
        Ok(None) => agentlog::event("shutdown: server stopped"),
        Err(e) => agentlog::event(&format!("shutdown: error: {e}")),
    }
    // Leave raw mode BEFORE the farewell (the console thread may still be
    // blocked in readline), so the message renders legibly and the shell the
    // user returns to works.
    terminal.restore();
    // Best-effort cleanup so the next start does not see a stale socket.
    let _ = std::fs::remove_file(&socket_path);
    if let Some(reason) = result? {
        eprintln!("revolutx-agent: {reason}");
    }
    Ok(())
}

/// Installs a panic hook that restores the terminal FIRST (so the panic message
/// renders legibly and the shell is usable) and records the panic in the event
/// log, then chains to the default hook. `SavedTerminal` is `Copy` and its
/// `restore` is safe from the panicking thread.
fn install_panic_hook(terminal: SavedTerminal) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        terminal.restore();
        agentlog::event(&format!("PANIC: {info}"));
        default(info);
    }));
}

/// Prints the one-time authentication token prominently to stderr (the operator's
/// terminal), to be copied out of band to the connecting client.
fn print_token(token: &str) {
    eprintln!();
    eprintln!("revolutx-agent: one-time authentication token — give this to a token client:");
    eprintln!();
    eprintln!("    {token}");
    eprintln!();
    eprintln!(
        "revolutx-agent: it is single-use; other clients can instead be authorized from the console."
    );
}

// --- operator console ------------------------------------------------------

/// Spawns the console thread. It reads commands from stdin and drives `control`.
fn spawn_console(control: AgentControl) {
    let _ = std::thread::Builder::new()
        .name("revolutx-agent-console".to_owned())
        .spawn(move || console_loop(&control));
}

/// Drives the operator console. An interactive terminal gets a line editor
/// (a `> ` prompt, session history, and command/id/tier completion); a piped or
/// redirected stdin falls back to plain line reading.
fn console_loop(control: &AgentControl) {
    if std::io::stdin().is_terminal() {
        interactive_console(control);
    } else {
        plain_console(control);
    }
}

/// Interactive console: a `rustyline` line editor with a `> ` prompt, in-memory
/// (session-only, never persisted to disk) history, and completion.
fn interactive_console(control: &AgentControl) {
    // `List` shows all candidates at once (like a shell) rather than cycling
    // through them in place on each Tab (the default `Circular`).
    let config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .build();
    let mut editor = match Editor::<ConsoleHelper, DefaultHistory>::with_config(config) {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("revolutx-agent: line editor unavailable ({e}); reading plainly");
            plain_console(control);
            return;
        }
    };
    editor.set_helper(Some(ConsoleHelper {
        control: control.clone(),
    }));
    loop {
        match editor.readline("> ") {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                // Session-only history: kept in memory for this run, not written
                // to disk (a secure history store is planned separately).
                let _ = editor.add_history_entry(input);
                if handle_command(control, input) {
                    return; // `quit` requested; the server is stopping.
                }
            }
            // In raw mode rustyline receives Ctrl-C as a keystroke, not a process
            // SIGINT, so drive the graceful shutdown here (mirroring the signal
            // path in `start`).
            Err(ReadlineError::Interrupted) => {
                control.shutdown();
                return;
            }
            // Ctrl-D (Eof) or a read error closes the console but leaves the agent
            // serving, matching the piped-stdin (EOF) behavior below.
            Err(_) => return,
        }
    }
}

/// Plain console for a non-interactive stdin (piped/redirected): read lines until
/// EOF, which stops reading but leaves the agent serving.
fn plain_console(control: &AgentControl) {
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) | Err(_) => return, // EOF or error: stop reading, keep serving.
            Ok(_) => {}
        }
        if handle_command(control, line.trim()) {
            return; // `quit` requested; the server is stopping.
        }
    }
}

/// Completion for the operator console: command names first, then live connection
/// ids for `grant`/`deny`, then access tiers for `grant <id> <tier>`.
struct ConsoleHelper {
    control: AgentControl,
}

impl Completer for ConsoleHelper {
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

        let candidates: Vec<String> = match prior.first().copied() {
            // First word: a console command.
            None => CONSOLE_COMMANDS.iter().map(|s| (*s).to_owned()).collect(),
            // `grant <id>` / `deny <id>`: offer the live connection ids.
            Some("grant" | "deny") if prior.len() == 1 => self
                .control
                .list()
                .iter()
                .map(|c| c.id.to_string())
                .collect(),
            // `grant <id> <tier>`: offer the access tiers.
            Some("grant") if prior.len() == 2 => {
                CONSOLE_TIERS.iter().map(|s| (*s).to_owned()).collect()
            }
            _ => Vec::new(),
        };
        let pairs = candidates
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

impl Hinter for ConsoleHelper {
    type Hint = String;
}
impl Highlighter for ConsoleHelper {}
impl Validator for ConsoleHelper {}
impl Helper for ConsoleHelper {}

/// Handles one console command. Returns `true` if the agent should quit.
fn handle_command(control: &AgentControl, input: &str) -> bool {
    let mut parts = input.split_whitespace();
    let Some(cmd) = parts.next() else {
        return false;
    };
    match cmd {
        "list" | "ls" => print_connections(control),
        "grant" => match parse_grant(&mut parts) {
            Ok((id, access)) => match control.grant(id, access) {
                Ok(level) => println!("granted #{id} at {}", level.as_str()),
                Err(e) => println!("error: {e}"),
            },
            Err(msg) => println!("usage: grant <id> [market|view|trading]  ({msg})"),
        },
        "deny" => match parts.next().and_then(|s| s.parse::<u64>().ok()) {
            Some(id) => match control.deny(id) {
                Ok(()) => println!("denied #{id}"),
                Err(e) => println!("error: {e}"),
            },
            None => println!("usage: deny <id>"),
        },
        "help" | "?" => print_help(),
        "quit" | "exit" => {
            control.shutdown();
            return true;
        }
        other => println!("unknown command `{other}` — type `help`"),
    }
    false
}

fn parse_grant<'a>(
    parts: &mut impl Iterator<Item = &'a str>,
) -> Result<(u64, Option<AccessLevel>), String> {
    let id = parts
        .next()
        .ok_or_else(|| "missing id".to_owned())?
        .parse::<u64>()
        .map_err(|_| "id must be a number".to_owned())?;
    let access = match parts.next() {
        None => None,
        Some(s) => Some(s.parse::<AccessLevel>().map_err(|e| e.to_string())?),
    };
    Ok((id, access))
}

fn print_connections(control: &AgentControl) {
    let conns = control.list();
    if conns.is_empty() {
        println!("(no connections)");
        return;
    }
    println!(
        "{:>3}  {:>6} {:>6} {:>7}  {:<15}  {:<7}  {:<20}  LABEL",
        "ID", "UID", "GID", "PID", "NAME", "METHOD", "STATE"
    );
    for c in conns {
        let pid = c.peer.pid.map_or_else(|| "-".to_owned(), |p| p.to_string());
        let name = process_name(c.peer.pid);
        let method = c.method.map_or("-", AuthMethod::as_str);
        // Sanitize the client-supplied label at the point it reaches the terminal
        // (the agent also bounds/sanitizes it on ingest): replace control
        // characters so a crafted label cannot inject ANSI/CR/newline sequences to
        // forge or hide console rows the operator relies on when granting.
        let label = c
            .label
            .as_deref()
            .map_or_else(|| "-".to_owned(), display_label);
        println!(
            "{:>3}  {:>6} {:>6} {:>7}  {:<15}  {:<7}  {:<20}  {}",
            c.id,
            c.peer.uid,
            c.peer.gid,
            pid,
            name,
            method,
            state_str(c.state),
            label
        );
    }
}

/// Best-effort process name for `pid`, read from `/proc/<pid>/comm` (Linux). The
/// pid is the peer captured at connect; for a live connection it is the connecting
/// process. Returns `-` when unavailable (no pid, process gone, or non-Linux), and
/// sanitizes the value like a label since it comes from another process.
fn process_name(pid: Option<i32>) -> String {
    let Some(pid) = pid else {
        return "-".to_owned();
    };
    std::fs::read_to_string(format!("/proc/{pid}/comm")).map_or_else(
        |_| "-".to_owned(),
        |name| {
            let name = name.trim();
            if name.is_empty() {
                "-".to_owned()
            } else {
                display_label(name)
            }
        },
    )
}

/// Renders a connection label safe for the operator's terminal: control
/// characters become `·`.
fn display_label(label: &str) -> String {
    label
        .chars()
        .map(|ch| if ch.is_control() { '·' } else { ch })
        .collect()
}

fn state_str(state: ConnState) -> String {
    match state {
        ConnState::Connected => "connected".to_owned(),
        ConnState::Pending { requested } => format!("pending({})", requested.as_str()),
        ConnState::Authorized { access } => format!("authorized({})", access.as_str()),
        ConnState::Denied => "denied".to_owned(),
    }
}

fn print_help() {
    println!("operator commands:");
    println!("  list | ls           show all connections (id, uid/gid/pid, method, state, label)");
    println!(
        "  grant <id> [tier]   authorize #id at market|view|trading (default: what it requested)"
    );
    println!("  deny <id>           refuse #id");
    println!("  help | ?            this help");
    println!("  quit | exit         lock the vault and stop the agent");
}

// --- watchdog --------------------------------------------------------------

/// Spawns the continuous security watchdog (a dedicated thread, ticking every
/// second). It exits the process on a clock anomaly or an attached debugger, and
/// signals a graceful idle auto-lock once no client has been authorized for
/// `idle_timeout` seconds. No thread is spawned when nothing needs watching.
fn spawn_watchdog(
    check_debugger: bool,
    control: AgentControl,
    idle_timeout: u64,
    socket_path: PathBuf,
    idle_shutdown: Arc<Notify>,
    terminal: SavedTerminal,
) {
    if !check_debugger && idle_timeout == 0 {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("revolutx-agent-watchdog".to_owned())
        .spawn(move || {
            watchdog_loop(
                check_debugger,
                &control,
                idle_timeout,
                &socket_path,
                &idle_shutdown,
                terminal,
            );
        });
}

fn watchdog_loop(
    check_debugger: bool,
    control: &AgentControl,
    idle_timeout: u64,
    socket_path: &std::path::Path,
    idle_shutdown: &Notify,
    terminal: SavedTerminal,
) {
    let mut prev = Instant::now();
    // When the last authorized client leaves (or none ever arrives), we start
    // counting toward the idle auto-lock; any authorized client clears it.
    let mut idle_since: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = Instant::now();

        // A frozen or rewound monotonic clock is consistent with a debugger pause
        // or VM time manipulation — exit *now*, no graceful unwind.
        if now.saturating_duration_since(prev).is_zero() {
            exit(socket_path, terminal, 1, "clock anomaly");
        }
        prev = now;

        if check_debugger && revolutx::keystore::is_debugger_attached() {
            exit(socket_path, terminal, 1, "debugger detected");
        }

        // Idle auto-lock: shut down *gracefully* (via the runtime) so the keystore
        // key zeroizes on Drop, rather than hard-exiting — no attacker on this path.
        if idle_timeout > 0 {
            if control.active_count() == 0 {
                let since = *idle_since.get_or_insert(now);
                if now.saturating_duration_since(since).as_secs() >= idle_timeout {
                    idle_shutdown.notify_one();
                    return;
                }
            } else {
                idle_since = None;
            }
        }
    }
}

fn exit(socket_path: &std::path::Path, terminal: SavedTerminal, code: i32, reason: &str) -> ! {
    agentlog::event(&format!("hard-exit: {reason} (code {code})"));
    // Leave the console's raw mode before printing, or the reason renders
    // garbled and the user's shell is left broken (process::exit runs no
    // destructors, so rustyline would never restore the terminal).
    terminal.restore();
    eprintln!("revolutx-agent: {reason}, locking and exiting");
    // process::exit skips the daemon's normal cleanup, so remove the socket here.
    let _ = std::fs::remove_file(socket_path);
    std::process::exit(code);
}
