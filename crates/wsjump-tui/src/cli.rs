//! Argv parsing for the TUI binary.
//!
//! The TUI binary has a tiny surface: no arguments launches the interactive
//! picker, `init [<shell>]` prints the shell-integration snippet.

use wsjump_core::error::{AppError, AppResult};

/// What the TUI binary was asked to do.
#[derive(Debug)]
pub enum Command {
    /// Launch the interactive picker (no arguments).
    Picker,
    /// Print the shell-integration snippet for the given shell.
    Init(Shell),
}

/// The shells `init` can emit an integration snippet for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Powershell,
}

impl Shell {
    /// Parse a shell name; `None` when unknown.
    pub fn parse(name: &str) -> Option<Shell> {
        match name {
            "bash" => Some(Shell::Bash),
            "powershell" => Some(Shell::Powershell),
            _ => None,
        }
    }

    /// The canonical name, `init`'s `--help`-worthy display form.
    pub fn name(&self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Powershell => "powershell",
        }
    }
}

/// Parse process arguments (excluding argv[0]).
///
/// No arguments -> `Picker`. `init` -> `Init(Powershell)` (kept for backwards
/// compatibility with the original Windows-centric invocation); `init <shell>`
/// -> `Init(shell)`. Anything else is a usage error.
pub fn parse(args: &[String]) -> AppResult<Command> {
    match args.split_first() {
        None => Ok(Command::Picker),
        Some((cmd, rest)) if cmd == "init" => match rest {
            [] => Ok(Command::Init(Shell::Powershell)),
            [name] => Shell::parse(name).map(Command::Init).ok_or_else(|| {
                AppError::usage(format!(
                    "unknown shell: {} (expected: {})",
                    name,
                    ["bash", "powershell"]
                        .into_iter()
                        .filter_map(|n| Shell::parse(n))
                        .map(|s| s.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }),
            _ => Err(AppError::usage(
                "init takes at most one argument: the shell",
            )),
        },
        Some((other, _)) => Err(AppError::usage(format!("unknown command: {}", other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_launches_the_picker() {
        assert!(matches!(parse(&args(&[])), Ok(Command::Picker)));
    }

    #[test]
    fn bare_init_defaults_to_powershell() {
        match parse(&args(&["init"])) {
            Ok(Command::Init(Shell::Powershell)) => {}
            other => panic!("expected Init(Powershell), got {:?}", other),
        }
    }

    #[test]
    fn init_accepts_bash() {
        match parse(&args(&["init", "bash"])) {
            Ok(Command::Init(Shell::Bash)) => {}
            other => panic!("expected Init(Bash), got {:?}", other),
        }
    }

    #[test]
    fn init_rejects_unknown_shells() {
        assert!(parse(&args(&["init", "zsh"])).is_err());
    }

    #[test]
    fn init_rejects_extra_arguments() {
        assert!(parse(&args(&["init", "bash", "extra"])).is_err());
    }

    #[test]
    fn unknown_commands_are_usage_errors() {
        assert!(parse(&args(&["frobnicate"])).is_err());
    }
}
