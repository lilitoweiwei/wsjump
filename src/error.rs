//! Error type and process exit-code mapping.
//!
//! Exit codes are part of the CLI contract:
//! - 0: success
//! - 1: runtime error (no match, missing config, dir conflict, IO, ...)
//! - 2: usage error (unknown subcommand, invalid arguments)

use std::fmt;

/// Top-level error carrying a human-readable message plus the exit code that
/// should be returned to the OS. Messages are always written to stderr.
#[derive(Debug)]
pub struct AppError {
    pub message: String,
    pub code: i32,
}

impl AppError {
    /// Runtime error (exit code 1): the command is well-formed but failed at
    /// runtime (e.g. no workspace matched, config missing, IO failure).
    pub fn runtime(message: impl Into<String>) -> Self {
        AppError {
            message: message.into(),
            code: 1,
        }
    }

    /// Usage error (exit code 2): the command line itself is malformed
    /// (unknown subcommand, missing / invalid argument).
    pub fn usage(message: impl Into<String>) -> Self {
        AppError {
            message: message.into(),
            code: 2,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

/// Convenience alias for fallible operations across the crate.
pub type AppResult<T> = Result<T, AppError>;
