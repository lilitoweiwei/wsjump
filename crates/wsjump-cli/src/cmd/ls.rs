//! `wsjump ls [--sort=<order>] <query...>`: print one absolute path per matching
//! workspace.

use wsjump_core::config::Config;
use wsjump_core::error::AppResult;
use wsjump_core::index::Index;
use wsjump_core::query;

use crate::cli::Query;
use wsjump_core::progress::StderrProgress;

/// Run the `ls` subcommand. An empty query lists all workspaces; an empty
/// result set is normal and exits successfully with no output.
pub fn run(config: &Config, args: &Query) -> AppResult<()> {
    let index = Index::open(config, &mut StderrProgress::new())?;
    let tokens = query::parse(&args.tokens);
    let sort = args.sort.unwrap_or_else(|| config.default_sort());

    // Listing is inherently unbounded, so paging would buy nothing here.
    for ws in index.query_all(&tokens, sort)? {
        println!("{}", ws.path.display());
    }
    Ok(())
}
