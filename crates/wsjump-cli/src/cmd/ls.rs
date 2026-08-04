//! `wsjump ls <query...>`: print one absolute path per matching workspace.

use wsjump_core::config::Config;
use wsjump_core::error::AppResult;
use wsjump_core::workspace::sort_by_id;
use wsjump_core::{filter, query, scan};

/// Run the `ls` subcommand. An empty query lists all workspaces; an empty
/// result set is normal and exits successfully with no output.
pub fn run(config: &Config, args: &[String]) -> AppResult<()> {
    let mut workspaces = scan::scan(config)?;
    sort_by_id(&mut workspaces);
    let tokens = query::parse(args);
    let matches = filter::filter(&workspaces, &tokens);

    for ws in matches {
        println!("{}", ws.path.display());
    }
    Ok(())
}
