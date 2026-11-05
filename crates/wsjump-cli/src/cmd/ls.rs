//! `wsjump ls [--sort=<order>] [--json] <query...>`: print one absolute path per
//! matching workspace, or one JSON array of workspace objects.

use wsjump_core::config::Config;
use wsjump_core::error::AppResult;
use wsjump_core::index::Index;
use wsjump_core::json;
use wsjump_core::query;

use crate::cli::Query;
use wsjump_core::progress::StderrProgress;

/// Run the `ls` subcommand. An empty query lists all workspaces; an empty
/// result set is normal and prints nothing (or `[]` with `--json`).
pub fn run(config: &Config, args: &Query) -> AppResult<()> {
    let index = Index::open(config, &mut StderrProgress::new())?;
    let tokens = query::parse(&args.tokens);
    let sort = args.sort.unwrap_or_else(|| config.default_sort());

    // Listing is inherently unbounded, so paging would buy nothing here.
    let workspaces = index.query_all(&tokens, sort)?;
    if args.json {
        println!("{}", json::workspaces(&workspaces));
        return Ok(());
    }
    for ws in workspaces {
        println!("{}", ws.path.display());
    }
    Ok(())
}
