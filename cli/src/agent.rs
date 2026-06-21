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

use revolutx::agent::{default_socket_path, serve};

use crate::args::{AgentCmd, GlobalOpts};
use crate::creds;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Runs an `agent` subcommand.
pub fn run(global: &GlobalOpts, command: AgentCmd) -> Res<()> {
    match command {
        AgentCmd::Start {
            socket,
            idle_timeout,
        } => start(global, socket, idle_timeout),
    }
}

fn start(global: &GlobalOpts, socket: Option<PathBuf>, idle_timeout: u64) -> Res<()> {
    let socket_path = socket.unwrap_or_else(default_socket_path);

    // Unlock the vault (prompts for the master password) BEFORE building the
    // runtime — the hardening + ptrace fork already happened in `main`.
    let client = creds::client(global, true)?;
    let executor = client.executor();

    // The watchdog stops counting toward the idle timeout once a client connects.
    let connected = Arc::new(AtomicBool::new(false));
    spawn_watchdog(
        !global.insecure_allow_debugging,
        Arc::clone(&connected),
        idle_timeout,
        socket_path.clone(),
    );
    let on_connect = {
        let connected = Arc::clone(&connected);
        move || connected.store(true, Ordering::Relaxed)
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    eprintln!("revolutx-agent: listening on {}", socket_path.display());
    let result = runtime.block_on(async {
        tokio::select! {
            served = serve(executor, &socket_path, on_connect) => served.map_err(Into::into),
            _ = tokio::signal::ctrl_c() => {
                eprintln!("revolutx-agent: shutting down");
                Ok(())
            }
        }
    });
    // Best-effort cleanup so the next start does not see a stale socket.
    let _ = std::fs::remove_file(&socket_path);
    result
}

/// Spawns the continuous security watchdog (a dedicated thread, ticking every
/// second). It exits the process on a clock anomaly, an attached debugger, or
/// the pre-connection idle timeout. No thread is spawned when nothing needs
/// watching.
fn spawn_watchdog(
    check_debugger: bool,
    connected: Arc<AtomicBool>,
    idle_timeout: u64,
    socket_path: PathBuf,
) {
    if !check_debugger && idle_timeout == 0 {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("revolutx-agent-watchdog".to_owned())
        .spawn(move || watchdog_loop(check_debugger, &connected, idle_timeout, &socket_path));
}

fn watchdog_loop(
    check_debugger: bool,
    connected: &AtomicBool,
    idle_timeout: u64,
    socket_path: &std::path::Path,
) -> ! {
    let start = Instant::now();
    let mut prev = start;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = Instant::now();

        // A frozen or rewound monotonic clock is consistent with a debugger
        // pause or VM time manipulation.
        if now.saturating_duration_since(prev).is_zero() {
            exit(socket_path, 1, "clock anomaly");
        }
        prev = now;

        if check_debugger && revolutx::keystore::is_debugger_attached() {
            exit(socket_path, 1, "debugger detected");
        }

        // The idle timeout only applies until the first client connects; an
        // established client is never timed out for being idle.
        if idle_timeout > 0
            && !connected.load(Ordering::Relaxed)
            && now.saturating_duration_since(start).as_secs() >= idle_timeout
        {
            exit(
                socket_path,
                0,
                "no client connected before the idle timeout",
            );
        }
    }
}

fn exit(socket_path: &std::path::Path, code: i32, reason: &str) -> ! {
    eprintln!("revolutx-agent: {reason}, locking and exiting");
    // process::exit skips the daemon's normal cleanup, so remove the socket here.
    let _ = std::fs::remove_file(socket_path);
    std::process::exit(code);
}
