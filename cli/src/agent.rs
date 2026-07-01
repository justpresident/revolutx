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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use revolutx::AccessLevel;
use revolutx::agent::{
    AgentControl, AgentServer, AuthMethod, AuthToken, ConnState, default_socket_path,
};
use tokio::sync::Notify;

use crate::args::{AgentCmd, GlobalOpts};
use crate::creds;

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
    let result = runtime.block_on(async {
        tokio::select! {
            outcome = server.run(&socket_path) => outcome.map_err(Into::into),
            _ = tokio::signal::ctrl_c() => {
                eprintln!("revolutx-agent: shutting down");
                Ok(())
            }
            () = idle_shutdown.notified() => {
                eprintln!("revolutx-agent: idle timeout — locking and exiting");
                Ok(())
            }
        }
    });
    // Best-effort cleanup so the next start does not see a stale socket.
    let _ = std::fs::remove_file(&socket_path);
    result
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

/// Spawns the console thread. It reads commands from stdin and drives `control`; on
/// EOF it simply stops reading (the agent keeps serving).
fn spawn_console(control: AgentControl) {
    let _ = std::thread::Builder::new()
        .name("revolutx-agent-console".to_owned())
        .spawn(move || console_loop(&control));
}

fn console_loop(control: &AgentControl) {
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
        Some(s) => Some(parse_access(s).ok_or_else(|| format!("unknown tier `{s}`"))?),
    };
    Ok((id, access))
}

fn parse_access(s: &str) -> Option<AccessLevel> {
    match s {
        "market" => Some(AccessLevel::Market),
        "view" => Some(AccessLevel::View),
        "trading" => Some(AccessLevel::Trading),
        _ => None,
    }
}

fn print_connections(control: &AgentControl) {
    let conns = control.list();
    if conns.is_empty() {
        println!("(no connections)");
        return;
    }
    println!(
        "{:>3}  {:>6} {:>6} {:>7}  {:<7}  {:<20}  LABEL",
        "ID", "UID", "GID", "PID", "METHOD", "STATE"
    );
    for c in conns {
        let pid = c.peer.pid.map_or_else(|| "-".to_owned(), |p| p.to_string());
        let method = c.method.map_or("-", AuthMethod::as_str);
        let label = c.label.as_deref().unwrap_or("-");
        println!(
            "{:>3}  {:>6} {:>6} {:>7}  {:<7}  {:<20}  {}",
            c.id,
            c.peer.uid,
            c.peer.gid,
            pid,
            method,
            state_str(c.state),
            label
        );
    }
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
            );
        });
}

fn watchdog_loop(
    check_debugger: bool,
    control: &AgentControl,
    idle_timeout: u64,
    socket_path: &std::path::Path,
    idle_shutdown: &Notify,
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
            exit(socket_path, 1, "clock anomaly");
        }
        prev = now;

        if check_debugger && revolutx::keystore::is_debugger_attached() {
            exit(socket_path, 1, "debugger detected");
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

fn exit(socket_path: &std::path::Path, code: i32, reason: &str) -> ! {
    eprintln!("revolutx-agent: {reason}, locking and exiting");
    // process::exit skips the daemon's normal cleanup, so remove the socket here.
    let _ = std::fs::remove_file(socket_path);
    std::process::exit(code);
}
