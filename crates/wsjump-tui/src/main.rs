//! wsjump-tui: interactive picker front-end for wsjump.
//!
//! No arguments (or `--output <file>`) launches an inline terminal picker; the
//! selected workspace's absolute path goes to stdout (the UI is drawn to
//! stderr), or to the file named by `--output` when present. The bash `ju`
//! function uses the file form so stdout stays on the terminal -- the cursor
//! query that the inline viewport needs is answered only while stdout reaches
//! the terminal, and a command substitution would pipe it away. `init
//! [<shell>]` prints the integration snippet.

mod app;
mod cli;
mod init;
mod picker;
mod render;

use std::process::ExitCode;

use cli::Command;
use picker::Outcome;
use wsjump_core::error::{AppError, AppResult};

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
/// Picker: selection -> print or file the path + success; abort -> exit code
/// 1 with no path, so the shell function knows not to jump.
fn run(args: &[String]) -> AppResult<ExitCode> {
    match cli::parse(args)? {
        Command::Init(shell) => {
            init::run(shell)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Picker(output) => match picker::run()? {
            Outcome::Selected(path) => {
                if let Some(file) = output {
                    std::fs::write(&file, format!("{}\n", path)).map_err(|e| {
                        AppError::runtime(format!(
                            "cannot write selection to '{}': {}",
                            file.display(),
                            e
                        ))
                    })?;
                } else {
                    println!("{}", path);
                }
                Ok(ExitCode::SUCCESS)
            }
            Outcome::Aborted => Ok(ExitCode::from(1)),
        },
    }
}
