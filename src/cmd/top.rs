//! `wsjump top <query...>`: print the absolute path of the first matching
//! workspace (lexicographic by id).

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::{filter, query, scan};

/// Run the `top` subcommand. `args` is the query token list (may be empty,
/// meaning "match everything").
pub fn run(config: &Config, args: &[String]) -> AppResult<()> {
    let workspaces = scan::scan(config)?;
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
