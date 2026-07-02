//! `revolutx` command-line interface.
//!
//! Security ordering: process hardening (`disable_core_dumps`,
//! `enable_ptrace_protection`) must run **before** the async runtime is built,
//! because `enable_ptrace_protection` forks on Linux and forking a multithreaded
//! process is undefined behavior. So this is a plain `fn main()` that hardens (for
//! commands that touch secrets) and only then builds the Tokio runtime.
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

mod adapter;
mod agent;
mod args;
mod creds;
mod datetime;
mod helper;
mod human;
mod line;
mod oneshot;
mod progress;
mod repl;
mod watch;

use std::process::ExitCode;

use args::{Cli, Command};
use clap::Parser;

fn main() -> ExitCode {
    // Cheap, no fork — do it first.
    let _ = revolutx::keystore::disable_core_dumps();

    let cli = Cli::parse();
    let global = cli.global;
    let command = cli.command;

    // Harden before any threads/runtime exist (the fork happens here).
    if command.needs_secrets() && !global.insecure_allow_debugging {
        // The interactive shell must survive Ctrl-C (it stops `watch` with Enter,
        // not a signal). BLOCK SIGINT *before* the ptrace-protection fork: a SIGINT
        // delivered to the traced child triggers a ptrace signal-delivery-stop,
        // which the watchdog parent treats as an attack and kills the child —
        // orphaning the shell. Blocking prevents delivery (so no stop), and the
        // parent's `waitpid` is likewise uninterrupted. Inherited by both fork
        // halves. (Ignoring the signal is not enough — the stop happens first.)
        if matches!(&command, Command::Cli { .. }) {
            block_sigint();
        }
        if revolutx::keystore::enable_ptrace_protection().is_err() {
            eprintln!(
                "error: a debugger/tracer is attached (ptrace protection failed); \
                 pass --insecure-allow-debugging to override"
            );
            return ExitCode::FAILURE;
        }
        if revolutx::keystore::is_debugger_attached() {
            eprintln!("error: a debugger/tracer is attached");
            return ExitCode::FAILURE;
        }
    }

    match command {
        // Vault management is synchronous and needs no client/runtime.
        Command::Vault { command } => finish(creds::run_vault(&global, &command)),
        // The agent daemon owns its own runtime + watchdog (the watchdog thread
        // must be spawned after the fork above, before any runtime).
        Command::Agent { command } => finish(agent::run(&global, command)),
        // The interactive shell unlocks the vault once and owns its own runtime.
        Command::Cli { access } => finish(repl::run(&global, access.into())),
        // Every other command is a one-shot over a `RevolutXClient`.
        command => {
            // Resolve credentials (may prompt) before entering the async runtime.
            let client = match creds::client(&global, command.needs_secrets()) {
                Ok(client) => client,
                Err(e) => return fail(e.as_ref()),
            };

            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("error: could not start the async runtime: {e}");
                    return ExitCode::FAILURE;
                }
            };

            finish(runtime.block_on(oneshot::run(&global, command, &client)))
        }
    }
}

fn finish(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e.as_ref()),
    }
}

fn fail(e: &dyn std::error::Error) -> ExitCode {
    eprintln!("error: {e}");
    ExitCode::FAILURE
}

/// Blocks SIGINT for this process and any later fork, so Ctrl-C neither
/// interrupts the ptrace-protection watchdog parent nor triggers a ptrace
/// signal-delivery-stop that would have it kill the (traced) child. The
/// interactive shell stops `watch` with Enter, and rustyline reads Ctrl-C as a
/// keystroke (raw mode), so no real SIGINT is needed.
fn block_sigint() {
    // SAFETY: sigprocmask and the sigset ops are async-signal-safe; we are
    // single-threaded and pre-runtime here. `sigemptyset` initializes the set.
    unsafe {
        let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        libc::sigemptyset(set.as_mut_ptr());
        libc::sigaddset(set.as_mut_ptr(), libc::SIGINT);
        libc::sigprocmask(libc::SIG_BLOCK, set.as_ptr(), std::ptr::null_mut());
    }
}
