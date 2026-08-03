//! Filtering workspaces against parsed query tokens.
//!
//! Matching is case-insensitive substring containment. Tokens are combined
//! with AND: a workspace is kept only if every token matches. An empty token
//! list keeps everything.

use crate::query::Token;
use crate::workspace::Workspace;

/// Return the subset of `workspaces` matching all `tokens`, preserving the
/// input order (which is already sorted by id from the scan step).
pub fn filter<'a>(workspaces: &'a [Workspace], tokens: &[Token]) -> Vec<&'a Workspace> {
    workspaces
        .iter()
        .filter(|ws| tokens.iter().all(|token| matches_token(ws, token)))
        .collect()
}

/// Like [`filter`], but returns the indices of matching workspaces instead of
/// references. Useful for UIs that own the workspace list and track selection
/// by index. Uses the same matching rules.
pub fn filter_indices(workspaces: &[Workspace], tokens: &[Token]) -> Vec<usize> {
    workspaces
        .iter()
        .enumerate()
        .filter(|(_, ws)| tokens.iter().all(|token| matches_token(ws, token)))
        .map(|(i, _)| i)
        .collect()
}

/// Does a single token match the workspace?
fn matches_token(ws: &Workspace, token: &Token) -> bool {
    match token {
        // Tag token: match against tags only.
        Token::Tag(needle) => {
            let needle = needle.to_lowercase();
            ws.tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&needle))
        }
        // Text token: match against id + desc + tags.
        Token::Text(needle) => {
            let needle = needle.to_lowercase();
            if ws.id.to_lowercase().contains(&needle) {
                return true;
            }
            if ws.desc.to_lowercase().contains(&needle) {
                return true;
            }
            ws.tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&needle))
        }
    }
}
