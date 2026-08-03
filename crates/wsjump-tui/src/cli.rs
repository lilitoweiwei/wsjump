//! Argv parsing for the TUI binary.
//!
//! The TUI binary has a tiny surface: no arguments launches the interactive
//! picker, `init` prints the shell integration snippet.

use wsjump_core::error::{AppError, AppResult};

/// What the TUI binary was asked to do.
pub enum Command {
    /// Launch the interactive picker (no arguments).
    Picker,
    /// Print the PowerShell integration snippet.
    Init,
}

/// Parse process arguments (excluding argv[0]).
///
/// No arguments -> `Picker`. `init` -> `Init`. Anything else is a usage error.
pub fn parse(args: &[String]) -> AppResult<Command> {
    match args.split_first() {
        None => Ok(Command::Picker),
        Some((cmd, rest)) if cmd == "init" && rest.is_empty() => Ok(Command::Init),
        Some((other, _)) => Err(AppError::usage(format!("unknown command: {}", other))),
    }
}
