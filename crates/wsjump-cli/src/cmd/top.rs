//! `wsjump top [--sort=<order>] <query...>`: print the absolute path of the
//! first matching workspace.

use wsjump_core::config::Config;
use wsjump_core::error::{AppError, AppResult};
use wsjump_core::index::Index;
use wsjump_core::query;

use crate::cli::Query;
use crate::progress::StderrProgress;

/// Run the `top` subcommand. An empty query matches everything, so this prints
/// whichever workspace the chosen order puts first.
pub fn run(config: &Config, args: &Query) -> AppResult<()> {
    let index = Index::open(config, &mut StderrProgress::new())?;
    let tokens = query::parse(&args.tokens);
    let sort = args.sort.unwrap_or_else(|| config.default_sort());

    // Only the first row is ever printed, so ask for exactly one.
    let page = index.query(&tokens, sort, 1, None)?;

    match page.workspaces.first() {
        Some(ws) => {
            println!("{}", ws.path.display());
            Ok(())
        }
        None => Err(AppError::runtime(format!(
            "no workspace matches: {}",
            args.tokens.join(" ")
        ))),
    }
}
