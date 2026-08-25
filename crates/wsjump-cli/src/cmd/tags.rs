//! `wsjump tags [--sort=<order>]`: print every distinct tag, space-separated.

use wsjump_core::config::Config;
use wsjump_core::error::AppResult;
use wsjump_core::index::Index;
use wsjump_core::progress::StderrProgress;
use wsjump_core::query::SortOrder;
use wsjump_core::workspace::all_tags;

/// Run the `tags` subcommand. Prints every distinct tag in the chosen order,
/// separated by spaces; no tags means an empty line and success.
pub fn run(config: &Config, sort: Option<SortOrder>) -> AppResult<()> {
    let index = Index::open(config, &mut StderrProgress::new())?;
    let order = sort.unwrap_or_else(|| config.default_sort());
    let workspaces = index.query_all(&[], order)?;
    println!("{}", all_tags(&workspaces, order).join(" "));
    Ok(())
}
