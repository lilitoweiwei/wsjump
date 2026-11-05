//! `wsjump tags [--sort=<order>] [--json]`: print every distinct tag,
//! space-separated, or one JSON document carrying the tags and the category
//! tree.

use wsjump_core::categories;
use wsjump_core::config::Config;
use wsjump_core::error::AppResult;
use wsjump_core::index::Index;
use wsjump_core::json;
use wsjump_core::progress::StderrProgress;
use wsjump_core::query::SortOrder;
use wsjump_core::workspace::all_tags;

/// Run the `tags` subcommand. Prints every distinct tag in the chosen order,
/// separated by spaces; no tags means an empty line and success. With `--json`
/// the output is `{"tags": [...], "categories": [...]}`, where categories are
/// nested `[name, [children]]` pairs in name order.
pub fn run(config: &Config, sort: Option<SortOrder>, json_output: bool) -> AppResult<()> {
    let index = Index::open(config, &mut StderrProgress::new())?;
    let order = sort.unwrap_or_else(|| config.default_sort());
    let workspaces = index.query_all(&[], order)?;
    let tags = all_tags(&workspaces, order);
    if json_output {
        let tree = categories::scan(&config.root_dir)?;
        println!(
            "{{\"tags\":{},\"categories\":{}}}",
            json::array(&tags),
            categories::to_json(&tree)
        );
        return Ok(());
    }
    println!("{}", tags.join(" "));
    Ok(())
}
