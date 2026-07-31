//! `wsjump ls <query...>`: print one absolute path per matching workspace.

use crate::config::Config;
use crate::error::AppResult;
use crate::{filter, pathutil, query, scan};

/// Run the `ls` subcommand. An empty query lists all workspaces; an empty
/// result set is normal and exits successfully with no output.
pub fn run(config: &Config, args: &[String]) -> AppResult<()> {
    let workspaces = scan::scan(config)?;
    let tokens = query::parse(args);
    let matches = filter::filter(&workspaces, &tokens);

    for ws in matches {
        println!("{}", pathutil::display(&ws.path));
    }
    Ok(())
}
