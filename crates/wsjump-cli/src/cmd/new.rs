//! `wsjump new <name> [categories]`: create one workspace and print its path.

use wsjump_core::config::Config;
use wsjump_core::create::create_workspace;
use wsjump_core::error::AppResult;
use wsjump_core::index::Index;
use wsjump_core::progress::StderrProgress;
use wsjump_core::workspace::key_for_path;

/// Create in `w/` or in the supplied category chain, then record a visit.
pub fn run(config: &Config, name: &str, categories: Option<&str>) -> AppResult<()> {
    let path = create_workspace(config, name, categories)?;

    // A just-created workspace is almost certainly about to be used, so record a
    // visit so the next picker puts it among the recently contacted workspaces.
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
    match key_for_path(&config.root_dir, path) {
        Ok(Some(key)) => index.record_visit(&key),
        Ok(None) => {}
        Err(e) => eprintln!("wsjump: warning: {}", e),
    }
}
