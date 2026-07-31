//! Parsing of a workspace's optional `wsmeta.txt` file.

use std::fs;
use std::path::Path;

/// Result of parsing a `wsmeta.txt`: tags and a description body.
pub struct Meta {
    pub tags: Vec<String>,
    pub desc: String,
}

impl Meta {
    /// Empty metadata, used when no file exists or on read failure.
    fn empty() -> Self {
        Meta {
            tags: Vec::new(),
            desc: String::new(),
        }
    }
}

/// File name of the per-workspace metadata file.
const META_FILE_NAME: &str = "wsmeta.txt";

/// Load and parse `<workspace>/wsmeta.txt`.
///
/// - Missing file -> empty metadata (the file is optional).
/// - Read failure (invalid UTF-8 / IO error) -> warn to stderr, treat as empty
///   and continue; a single bad file must never abort the whole scan.
pub fn load(workspace_dir: &Path) -> Meta {
    let path = workspace_dir.join(META_FILE_NAME);
    if !path.exists() {
        return Meta::empty();
    }
    match fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(e) => {
            eprintln!(
                "wsjump: warning: cannot read '{}': {}; ignoring metadata",
                path.display(),
                e
            );
            Meta::empty()
        }
    }
}

/// Parse metadata text.
///
/// - The FIRST line whose first non-whitespace character is ':' is the tag
///   line. It is split on whitespace; each token has a single leading ':'
///   stripped (if present) to form a tag. Only the first such line counts.
/// - Every other line (including blank lines and any later ':' lines) is part
///   of the description, joined by '\n'. Content is preserved only for
///   substring matching; layout is irrelevant.
fn parse(text: &str) -> Meta {
    let mut tags: Vec<String> = Vec::new();
    let mut tag_line_found = false;
    let mut desc_lines: Vec<&str> = Vec::new();

    for line in text.lines() {
        if !tag_line_found && line.trim_start().starts_with(':') {
            tag_line_found = true;
            for token in line.split_whitespace() {
                let tag = token.strip_prefix(':').unwrap_or(token);
                if !tag.is_empty() {
                    tags.push(tag.to_string());
                }
            }
        } else {
            desc_lines.push(line);
        }
    }

    Meta {
        tags,
        desc: desc_lines.join("\n"),
    }
}
