//! The `revolutx agent` subcommand: a signing-agent daemon (`start`) and a
//! liveness check (`ping`).
//!
//! `start` unlocks the vault once (interactive password) and serves a full proxy
//! over a unix socket — it signs and performs every forwarded request, so the
//! private key and API key never leave this process. A dedicated watchdog thread
//! re-checks for an attached debugger and enforces the idle auto-lock.
//!
//! The watchdog thread is spawned only after `main` has hardened the process
//! (and after the `enable_ptrace_protection` fork) and before the async runtime
//! starts — forking a multithreaded process is undefined behavior.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use revolutx::agent::{AgentExecutor, default_socket_path, serve};

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
        AgentCmd::Ping { socket } => ping(global, socket),
    }
}

fn start(global: &GlobalOpts, socket: Option<PathBuf>, idle_timeout: u64) -> Res<()> {
    let socket_path = socket.unwrap_or_else(default_socket_path);

    // Unlock the vault (prompts for the master password) BEFORE building the
    // runtime — the hardening + ptrace fork already happened in `main`.
    let client = creds::client(global, true)?;
    let executor = client.executor();

    // Activity timestamp for the idle auto-lock, bumped on every request.
    let last_activity = Arc::new(AtomicU64::new(now_secs()));
    spawn_watchdog(
        !global.insecure_allow_debugging,
        Arc::clone(&last_activity),
        idle_timeout,
    );

    let on_request: Arc<dyn Fn() + Send + Sync> = {
        let last_activity = Arc::clone(&last_activity);
        Arc::new(move || last_activity.store(now_secs(), Ordering::Relaxed))
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    eprintln!("revolutx-agent: listening on {}", socket_path.display());
    let result = runtime.block_on(async {
        tokio::select! {
            served = serve(executor, &socket_path, on_request) => served.map_err(Into::into),
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

fn ping(global: &GlobalOpts, socket: Option<PathBuf>) -> Res<()> {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let base_url = creds::environment(global).base_url();
    let executor = AgentExecutor::new(&socket_path, base_url);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(executor.ping())?;
    println!("agent OK at {}", socket_path.display());
    Ok(())
}

/// Spawns the continuous security watchdog (a dedicated thread, ticking every
/// second). It exits the process on a clock anomaly, an attached debugger, or
/// the idle timeout. No thread is spawned when nothing needs watching.
fn spawn_watchdog(check_debugger: bool, last_activity: Arc<AtomicU64>, idle_timeout: u64) {
    if !check_debugger && idle_timeout == 0 {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("revolutx-agent-watchdog".to_owned())
        .spawn(move || watchdog_loop(check_debugger, &last_activity, idle_timeout));
}

fn watchdog_loop(check_debugger: bool, last_activity: &AtomicU64, idle_timeout: u64) -> ! {
    let mut prev = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = Instant::now();
        // A frozen or rewound monotonic clock is consistent with a debugger
        // pause or VM time manipulation.
        if now.saturating_duration_since(prev).is_zero() {
            eprintln!("revolutx-agent: clock anomaly, locking and exiting");
            std::process::exit(1);
        }
        prev = now;

        if check_debugger && revolutx::keystore::is_debugger_attached() {
            eprintln!("revolutx-agent: debugger detected, locking and exiting");
            std::process::exit(1);
        }

        if idle_timeout > 0 {
            let idle = now_secs().saturating_sub(last_activity.load(Ordering::Relaxed));
            if idle >= idle_timeout {
                eprintln!("revolutx-agent: idle for {idle}s, locking and exiting");
                std::process::exit(0);
            }
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
