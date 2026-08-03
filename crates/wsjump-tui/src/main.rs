//! wsjump-tui: interactive picker front-end for wsjump.
//!
//! No arguments launches an inline terminal picker; the selected workspace's
//! absolute path is written to stdout (the UI is drawn to stderr) so a shell
//! function can `Set-Location` to it. `init` prints the integration snippet.

mod cli;
mod init;
mod picker;

use std::process::ExitCode;

use cli::Command;
use picker::Outcome;
use wsjump_core::error::AppResult;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("wsjump-tui: {}", e);
            ExitCode::from(e.code as u8)
        }
    }
}

/// Dispatch and translate outcomes into exit codes.
///
/// Picker: selection -> print path + success; abort -> exit code 1 with no
/// stdout, so the shell function knows not to jump.
fn run(args: &[String]) -> AppResult<ExitCode> {
    match cli::parse(args)? {
        Command::Init => {
            init::run()?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Picker => match picker::run()? {
            Outcome::Selected(path) => {
                println!("{}", path);
                Ok(ExitCode::SUCCESS)
            }
            Outcome::Aborted => Ok(ExitCode::from(1)),
        },
    }
}
