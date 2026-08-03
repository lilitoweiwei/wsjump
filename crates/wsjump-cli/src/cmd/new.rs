//! `wsjump new <slug...>`: create a new workspace directory named
//! `YYYYMMDD[-slug]` under `w/` and print its absolute path.

use wsjump_core::config::Config;
use wsjump_core::create::create_workspace;
use wsjump_core::error::AppResult;

/// Run the `new` subcommand. All positional args are joined with '-' to form
/// the slug (empty if none); creation and dating live in core.
pub fn run(config: &Config, args: &[String]) -> AppResult<()> {
    let slug = args.join("-");
    let path = create_workspace(config, &slug)?;
    println!("{}", path.display());
    Ok(())
}
