//! wsjump: a non-interactive workspace query tool.
//!
//! Scans the `w/` and `c/` containers, filters workspaces by a query, and prints
//! results (paths). It never changes the shell's current directory and never
//! modifies user config; jumping is a TUI-stage concern.

mod cli;
mod cmd;

use std::process::ExitCode;

use cli::Command;
use wsjump_core::config::Config;
use wsjump_core::error::AppResult;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wsjump: {}", e);
            ExitCode::from(e.code as u8)
        }
    }
}

/// Dispatch to the requested subcommand.
///
/// Config is loaded lazily: `help` needs no config, so it stays usable even
/// when the config file is missing.
fn run(args: &[String]) -> AppResult<()> {
    let command = cli::parse(args)?;
    match command {
        Command::Help => cmd::help::run(),
        Command::Top(query) => cmd::top::run(&Config::load()?, &query),
        Command::Ls(query) => cmd::ls::run(&Config::load()?, &query),
        Command::New { name, categories } => {
            cmd::new::run(&Config::load()?, &name, categories.as_deref())
        }
        Command::Touch(path) => cmd::touch::run(&Config::load()?, path.as_ref()),
        Command::Sync(path) => cmd::sync::run(&Config::load()?, path.as_ref()),
        Command::Tags(sort) => cmd::tags::run(&Config::load()?, sort),
    }
}
