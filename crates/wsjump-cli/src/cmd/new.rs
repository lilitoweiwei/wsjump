//! `wsjump new <slug...>`: create a new workspace directory named
//! `YYYYMMDD[-slug]` under `w/` and print its absolute path.

use wsjump_core::config::Config;
use wsjump_core::create::create_workspace;
use wsjump_core::error::AppResult;
use wsjump_core::index::Index;
use wsjump_core::workspace::id_for_path;
use crate::progress::StderrProgress;

/// Run the `new` subcommand. All positional args are joined with '-' to form
/// the slug (empty if none); creation and dating live in core.
pub fn run(config: &Config, args: &[String]) -> AppResult<()> {
    let slug = args.join("-");
    let path = create_workspace(config, &slug)?;

    // A just-created workspace is almost certainly about to be used, so record a
    // visit: that puts it at the top of recency order rather than level with
    // everything else created today, since a visit stores the current time while
    // a new row is seeded with the day's midnight.
    record_visit(config, &path);

    println!("{}", path.display());
    Ok(())
}

/// Record a visit to the newly created workspace.
///
/// Best effort: the directory exists and its path has been printed, so a
/// bookkeeping failure must not fail the command. Warnings come from the
/// functions themselves.
fn record_visit(config: &Config, path: &std::path::Path) {
    let Ok(index) = Index::open(config, &mut StderrProgress::new()) else {
        return;
    };
    match id_for_path(&config.workspaces_dir, path) {
        Ok(Some(id)) => index.record_visit(&id),
        Ok(None) => {}
        Err(e) => eprintln!("wsjump: warning: {}", e),
    }
}
