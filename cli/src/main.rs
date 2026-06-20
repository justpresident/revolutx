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

mod args;
mod commands;
mod creds;
mod output;

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

    // Vault management is synchronous and needs no client/runtime.
    if let Command::Vault { command } = &command {
        return finish(creds::run_vault(&global, command));
    }

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

    finish(runtime.block_on(commands::run(&global, command, &client)))
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
