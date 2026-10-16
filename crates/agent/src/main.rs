//! `dbcrust-agent`: the credential-free DBCrust client for AI agents.
//!
//! Talks to the DBCrust broker (which owns the credentials) over a Unix
//! socket using `dbcrust-agent-protocol`. Deliberately links no keystore, no
//! database driver and no config storage.

// Without Unix sockets the transport code is unreachable (every command exits
// with `unsupported_platform`) but still has to compile.
#![cfg_attr(not(unix), allow(dead_code))]

mod app;
mod cli;
mod client;
mod error;
mod mcp;
mod output;
mod socket;

#[cfg(test)]
mod testutil;

use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = match cli::parse(std::env::args_os()) {
        Ok(cli) => cli,
        Err(cli::ParseExit::Output(text)) => {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(cli::ParseExit::Usage(text)) => {
            eprint!("{text}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("error: cannot start the async runtime: {e}");
            return ExitCode::from(1);
        }
    };
    ExitCode::from(runtime.block_on(app::run(cli, socket::Env::from_process())))
}
