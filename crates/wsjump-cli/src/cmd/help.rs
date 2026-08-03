//! `wsjump help` / `wsjump` (no args): print usage.

use wsjump_core::error::AppResult;

/// Usage text listing every subcommand with a one-line description.
const USAGE: &str = "\
wsjump - workspace query tool

USAGE:
    wsjump <command> [args...]

COMMANDS:
    top <query...>    Print the absolute path of the first matching workspace.
    ls <query...>     Print one absolute path per matching workspace.
    new <slug...>     Create a new workspace (YYYYMMDD[-slug]) and print its path.
    help              Show this help.

QUERY:
    A query is a list of space-separated tokens combined with AND.
    A ':'-prefixed token (e.g. :ai) matches tags only; other tokens match the
    directory name, description and tags. Matching is case-insensitive
    substring. An empty query matches all workspaces.";

/// Print usage to stdout and succeed.
pub fn run() -> AppResult<()> {
    println!("{}", USAGE);
    Ok(())
}
