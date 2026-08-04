//! `wsjump top <query...>`: print the absolute path of the first matching
//! workspace (lexicographic by id).

use wsjump_core::config::Config;
use wsjump_core::error::{AppError, AppResult};
use wsjump_core::workspace::sort_by_id;
use wsjump_core::{filter, query, scan};

/// Run the `top` subcommand. `args` is the query token list (may be empty,
/// meaning "match everything").
pub fn run(config: &Config, args: &[String]) -> AppResult<()> {
    let mut workspaces = scan::scan(config)?;
    sort_by_id(&mut workspaces);
    let tokens = query::parse(args);
    let matches = filter::filter(&workspaces, &tokens);

    match matches.first() {
        Some(ws) => {
            println!("{}", ws.path.display());
            Ok(())
        }
        None => Err(AppError::runtime(format!(
            "no workspace matches: {}",
            args.join(" ")
        ))),
    }
}
