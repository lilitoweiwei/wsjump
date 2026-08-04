//! `wsjump help` / `wsjump` (no args): print usage.

use wsjump_core::error::AppResult;

/// Usage text listing every subcommand with a one-line description.
const USAGE: &str = "\
wsjump - workspace query tool

USAGE:
    wsjump <command> [args...]

COMMANDS:
    top [options] <query...>   Print the absolute path of the first matching workspace.
    ls [options] <query...>    Print one absolute path per matching workspace.
    new <slug...>              Create a new workspace (YYYYMMDD[-slug]) and print its path.
    touch [<path>]             Record a visit to a workspace (default: current directory).
    help                       Show this help.

OPTIONS:
    --sort=<order>    Result order: 'recency' (most recently visited first) or
                      'id' (by directory name). Defaults to the configured
                      default_sort, or recency.
    --                End of options; later dashed arguments are query tokens.

QUERY:
    A query is a list of space-separated tokens combined with AND.
    A ':'-prefixed token (e.g. :ai) matches tags only; other tokens match the
    slug and tags. Matching is case-insensitive substring. An empty query
    matches all workspaces.

    The date prefix and the description are not searched, so finding a
    workspace never requires typing a date.";

/// Print usage to stdout and succeed.
pub fn run() -> AppResult<()> {
    println!("{}", USAGE);
    Ok(())
}
