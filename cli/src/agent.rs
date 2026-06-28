//! The `revolutx agent start` subcommand: a single-client signing-agent daemon.
//!
//! It unlocks the vault once (interactive password) and serves a full proxy over
//! a unix socket — it signs and performs every forwarded request, so the private
//! key and API key never leave this process. The agent accepts exactly one
//! client and refuses the rest; when that client disconnects, the daemon exits
//! and the vault is re-locked.
//!
//! A dedicated watchdog thread re-checks for an attached debugger and enforces
//! the pre-connection idle timeout (auto-lock if no client ever connects). It is
//! spawned only after `main` has hardened the process (and after the
//! `enable_ptrace_protection` fork) and before the async runtime starts —
//! forking a multithreaded process is undefined behavior.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use revolutx::agent::{AuthToken, default_socket_path, serve};
use tokio::sync::Notify;

use crate::args::{AgentCmd, GlobalOpts};
use crate::creds;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Error shown when `agent start` is run without `--auth-token` (the only auth
/// method today).
const AUTH_TOKEN_REQUIRED: &str = "pass --auth-token: the agent needs a one-time token to \
     authenticate its client (it is the only auth method today)";

/// Runs an `agent` subcommand.
pub fn run(global: &GlobalOpts, command: AgentCmd) -> Res<()> {
    match command {
        AgentCmd::Start {
            socket,
            auth_token,
            idle_timeout,
        } => start(global, socket, auth_token, idle_timeout),
    }
}

fn start(
    global: &GlobalOpts,
    socket: Option<PathBuf>,
    auth_token: bool,
    idle_timeout: u64,
) -> Res<()> {
    // A client can only ever be authenticated with a one-time token, so refuse to
    // start without one — before touching the vault. (Interactive operator
    // approval, the future no-token path, is not yet implemented.)
    if !auth_token {
        return Err(AUTH_TOKEN_REQUIRED.into());
    }
    let socket_path = socket.unwrap_or_else(default_socket_path);

    // Generate the one-time token before unlocking the vault, then print it once
    // for the operator to hand to the client out of band. It is never accepted as
    // a CLI value (that would expose it via /proc and ps).
    let token = AuthToken::generate()?;

    // Unlock the vault (prompts for the master password) BEFORE building the
    // runtime — the hardening + ptrace fork already happened in `main`.
    let client = creds::client(global, true)?;
    let executor = client.executor();

    // The watchdog stops counting toward the idle timeout once a client
    // authenticates (on_connect fires on authentication, not on TCP accept).
    let connected = Arc::new(AtomicBool::new(false));
    // The idle timeout shuts down *gracefully* (so the keystore's key zeroizes on
    // Drop); only a debugger/clock anomaly hard-exits immediately.
    let idle_shutdown = Arc::new(Notify::new());
    spawn_watchdog(
        !global.insecure_allow_debugging,
        Arc::clone(&connected),
        idle_timeout,
        socket_path.clone(),
        Arc::clone(&idle_shutdown),
    );
    let on_connect = {
        let connected = Arc::clone(&connected);
        move || connected.store(true, Ordering::Relaxed)
    };

    // An exposed client gets least privilege by default: market data only.
    let access = global.access_or(revolutx::AccessLevel::Market);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    print_token(token.as_str());
    eprintln!(
        "revolutx-agent: listening on {} (access: {})",
        socket_path.display(),
        access.as_str(),
    );
    let result = runtime.block_on(async {
        tokio::select! {
            served = serve(executor, &socket_path, access, token, on_connect) => served.map_err(Into::into),
            _ = tokio::signal::ctrl_c() => {
                eprintln!("revolutx-agent: shutting down");
                Ok(())
            }
            () = idle_shutdown.notified() => {
                eprintln!("revolutx-agent: idle timeout, locking and exiting");
                Ok(())
            }
        }
    });
    // Best-effort cleanup so the next start does not see a stale socket.
    let _ = std::fs::remove_file(&socket_path);
    result
}

/// Prints the one-time authentication token prominently to stderr (the
/// operator's terminal), to be copied out of band to the connecting client.
fn print_token(token: &str) {
    eprintln!();
    eprintln!("revolutx-agent: one-time authentication token — give this to your client:");
    eprintln!();
    eprintln!("    {token}");
    eprintln!();
    eprintln!(
        "revolutx-agent: the client must present it before the agent serves any \
         request; it is single-use."
    );
}

/// Spawns the continuous security watchdog (a dedicated thread, ticking every
/// second). It exits the process on a clock anomaly, an attached debugger, or
/// the pre-authentication idle timeout. No thread is spawned when nothing needs
/// watching.
fn spawn_watchdog(
    check_debugger: bool,
    connected: Arc<AtomicBool>,
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
                &connected,
                idle_timeout,
                &socket_path,
                &idle_shutdown,
            );
        });
}

fn watchdog_loop(
    check_debugger: bool,
    connected: &AtomicBool,
    idle_timeout: u64,
    socket_path: &std::path::Path,
    idle_shutdown: &Notify,
) {
    let start = Instant::now();
    let mut prev = start;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = Instant::now();

        // A frozen or rewound monotonic clock is consistent with a debugger
        // pause or VM time manipulation — exit *now*, no graceful unwind.
        if now.saturating_duration_since(prev).is_zero() {
            exit(socket_path, 1, "clock anomaly");
        }
        prev = now;

        if check_debugger && revolutx::keystore::is_debugger_attached() {
            exit(socket_path, 1, "debugger detected");
        }

        // The idle timeout only applies until the first client authenticates; an
        // established client is never timed out for being idle. No attacker is
        // present on this path, so shut down *gracefully* (via the runtime) so
        // the keystore's key is zeroized on Drop, rather than hard-exiting.
        if idle_timeout > 0
            && !connected.load(Ordering::Relaxed)
            && now.saturating_duration_since(start).as_secs() >= idle_timeout
        {
            idle_shutdown.notify_one();
            return;
        }
    }
}

fn exit(socket_path: &std::path::Path, code: i32, reason: &str) -> ! {
    eprintln!("revolutx-agent: {reason}, locking and exiting");
    // process::exit skips the daemon's normal cleanup, so remove the socket here.
    let _ = std::fs::remove_file(socket_path);
    std::process::exit(code);
}
