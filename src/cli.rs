//! Argv parsing: identify the subcommand and its arguments.

use crate::error::{AppError, AppResult};

/// A parsed command line: which subcommand to run, with its trailing args.
pub enum Command {
    /// `top <query...>`
    Top(Vec<String>),
    /// `ls <query...>`
    Ls(Vec<String>),
    /// `new <slug...>`
    New(Vec<String>),
    /// `help` or no arguments at all.
    Help,
}

/// Parse process arguments (excluding argv[0]) into a `Command`.
///
/// No arguments -> `Help`. An unknown subcommand is a usage error.
pub fn parse(args: &[String]) -> AppResult<Command> {
    let (command, rest) = match args.split_first() {
        Some(split) => split,
        None => return Ok(Command::Help),
    };

    match command.as_str() {
        "top" => Ok(Command::Top(rest.to_vec())),
        "ls" => Ok(Command::Ls(rest.to_vec())),
        "new" => Ok(Command::New(rest.to_vec())),
        "help" => Ok(Command::Help),
        other => Err(AppError::usage(format!("unknown command: {}", other))),
    }
}
