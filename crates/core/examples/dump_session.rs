//! Headless harness: run a command under a PTY and dump its parsed block list.
//!
//! Usage: `cargo run -p SpaceTerm-core --example dump_session -- <program> [args...]`
//! Example: `cargo run -p SpaceTerm-core --example dump_session -- ls -la`

use std::env;
use std::process::ExitCode;

use spaceterm_core::run_to_completion;
use portable_pty::CommandBuilder;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: dump_session <program> [args...]");
        return ExitCode::FAILURE;
    };

    let mut command = CommandBuilder::new(program);
    for arg in args {
        command.arg(arg);
    }

    match run_to_completion(command) {
        Ok(terminal) => {
            for (index, block) in terminal.scrollback().blocks().iter().enumerate() {
                println!("--- block {index} ---");
                println!("{block:#?}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
