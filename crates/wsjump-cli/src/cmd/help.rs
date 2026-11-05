//! `wsjump help` / `wsjump` (no args): print usage.

use wsjump_core::error::AppResult;

/// Usage text listing every subcommand with a one-line description.
const USAGE: &str = r#"wsjump v3 - workspace query tool

USAGE:
    wsjump <command> [args...]

COMMANDS:
    top [options] <query...>   Print the absolute path of the first matching workspace.
    ls [options] <query...>    Print one absolute path per matching workspace.
    new <name> [categories]    Create in w/ or c/<categories>; create missing categories.
    touch [<path>]             Record a visit to a workspace (default: current directory).
    sync [<path>]              Re-read workspace metadata (default: all workspaces).
    tags [options]             Print every distinct tag, space-separated.
    help                       Show this help.

OPTIONS:
    --sort=<order>    Result order: 'recency' (most recently visited first) or
                       'name' (alphabetical). Defaults to the configured
                      default_sort, or recency.
    --json            Machine-readable output (see JSON below).
    --                End of options; later dashed arguments are query tokens.

JSON:
    'ls --json' / 'top --json'   array of workspace objects:
        {"key":"c/_ai/demo","name":"demo","path":"/ws/c/_ai/demo",
         "tags":["ai","tool"],"desc":"one line","last_access":1700000000}
    'tags --json'                tag set plus the category tree:
        {"tags":["ai","tool"],"categories":[["_ai",[["_opencode",[]]]]]}

    README.md documents each field and the ordering rules.

QUERY:
    A query is a list of space-separated tokens combined with AND.
    A ':'-prefixed token (e.g. :ai) matches tags only; other tokens match the
    complete name and tags. Matching is case-insensitive substring. An empty query
    matches all workspaces.

    Tags include ancestor categories and wsmeta.txt tags. Descriptions and
    path keys are not searched. No date prefix is required.

NEW:
    wsjump new scratch
    wsjump new my-plugin _ai/_opencode
    Names contain no whitespace. Category paths are relative to c/ and each
    component is an _tag directory. Missing categories are created automatically.

SYNC:
    The index keeps itself up to date, re-reading only metadata files whose
    size or timestamp changed, so 'sync' is rarely needed. Run it if a
    workspace's tags or description look stale despite the file being
    correct on disk."#;

/// Print usage to stdout and succeed.
pub fn run() -> AppResult<()> {
    println!("{}", USAGE);
    Ok(())
}
