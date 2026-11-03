//! In-memory filtering of workspaces against parsed query tokens.
//!
//! This is the fallback used when the database is unusable. It deliberately
//! mirrors the fast query's semantics (see `db::query`): case-insensitive
//! substring matching against complete names and tags, tokens combined with AND,
//! an empty token list keeping everything. Were the two to disagree, the same
//! query would return different results depending on whether the database
//! happened to be working.

use crate::query::Token;
use crate::workspace::Workspace;

/// Return the subset of `workspaces` matching all `tokens`, preserving input
/// order.
pub fn filter<'a>(workspaces: &'a [Workspace], tokens: &[Token]) -> Vec<&'a Workspace> {
    workspaces
        .iter()
        .filter(|ws| matches_all(ws, tokens))
        .collect()
}

/// Does a workspace match every token?
pub fn matches_all(ws: &Workspace, tokens: &[Token]) -> bool {
    tokens.iter().all(|token| matches_token(ws, token))
}

/// Does a single token match the workspace?
fn matches_token(ws: &Workspace, token: &Token) -> bool {
    match token {
        // A ':'-prefixed token deliberately narrows to tags only.
        Token::Tag(needle) => matches_tags(ws, needle),
        // Descriptions and path keys are display/location data, not query text.
        Token::Text(needle) => contains(&ws.name, needle) || matches_tags(ws, needle),
    }
}

/// Does any tag contain `needle`?
fn matches_tags(ws: &Workspace, needle: &str) -> bool {
    ws.tags.iter().any(|tag| contains(tag, needle))
}

/// Case-insensitive substring test.
fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query;
    use std::path::PathBuf;

    /// A flat workspace with the given name and tags.
    fn workspace(name: &str, tags: &[&str]) -> Workspace {
        Workspace {
            key: format!("w/{}", name),
            name: name.to_string(),
            path: PathBuf::from(format!("G:/ws/w/{}", name)),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            desc: String::new(),
            last_access: None,
        }
    }

    /// Keys kept when filtering `workspaces` by `query`.
    fn kept(workspaces: &[Workspace], query: &str) -> Vec<String> {
        let tokens = query::parse_str(query);
        filter(workspaces, &tokens)
            .into_iter()
            .map(|ws| ws.key.clone())
            .collect()
    }

    #[test]
    fn empty_query_keeps_everything() {
        let all = [workspace("alpha", &[]), workspace("beta", &[])];
        assert_eq!(kept(&all, "").len(), 2);
    }

    #[test]
    fn text_token_matches_name_substring_anywhere() {
        let all = [workspace("wsjump", &[])];
        assert_eq!(kept(&all, "jump").len(), 1);
        assert_eq!(kept(&all, "wsj").len(), 1);
        // Substring, not subsequence: matching `wj` is the slow query's job.
        assert!(kept(&all, "wj").is_empty());
    }

    #[test]
    fn matching_ignores_case() {
        let all = [workspace("WsJump", &["Tool"])];
        assert_eq!(kept(&all, "wsjump").len(), 1);
        assert_eq!(kept(&all, ":tool").len(), 1);
    }

    #[test]
    fn text_token_matches_tags_too() {
        let all = [workspace("alpha", &["rust"])];
        assert_eq!(kept(&all, "rust").len(), 1);
    }

    #[test]
    fn text_token_does_not_match_desc() {
        let mut ws = workspace("alpha", &[]);
        ws.desc = "a note about databases".to_string();
        let all = [ws];
        assert!(kept(&all, "databases").is_empty());
    }

    #[test]
    fn text_token_searches_the_complete_name_including_optional_dates() {
        let all = [workspace("20260730-alpha", &[])];
        assert_eq!(kept(&all, "20260730"), ["w/20260730-alpha"]);
    }

    #[test]
    fn tag_token_searches_tags_only() {
        let all = [workspace("rust", &[]), workspace("alpha", &["rust"])];
        // ':rust' must ignore the workspace whose name is "rust".
        assert_eq!(kept(&all, ":rust"), vec!["w/alpha".to_string()]);
    }

    #[test]
    fn multiple_tokens_combine_with_and() {
        let all = [
            workspace("alpha", &["rust", "tool"]),
            workspace("beta", &["rust"]),
        ];
        assert_eq!(kept(&all, ":rust :tool"), vec!["w/alpha".to_string()]);
    }

    #[test]
    fn wildcard_characters_are_matched_literally() {
        // The SQL path escapes LIKE wildcards; plain substring matching must
        // agree rather than treating them as patterns.
        let all = [workspace("plain", &[]), workspace("a_b", &[])];
        assert_eq!(kept(&all, "a_b"), vec!["w/a_b".to_string()]);
        assert!(kept(&all, "%").is_empty());
    }
}
