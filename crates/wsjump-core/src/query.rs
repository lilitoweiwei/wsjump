//! Query vocabulary: token classification and result ordering.

use serde::Deserialize;

/// A single query token.
pub enum Token {
    /// A ':'-prefixed token; matched only against tags (the ':' is stripped).
    Tag(String),
    /// A plain token; matched against id + desc + tags.
    Text(String),
}

/// Turn raw query arguments into typed tokens.
///
/// A token starting with ':' becomes a `Tag` token (leading ':' removed);
/// anything else becomes a `Text` token. An empty argument list yields no
/// tokens, which downstream means "match everything" (no constraints).
pub fn parse(args: &[String]) -> Vec<Token> {
    args.iter()
        .map(|arg| match arg.strip_prefix(':') {
            Some(rest) => Token::Tag(rest.to_string()),
            None => Token::Text(arg.clone()),
        })
        .collect()
}

/// Parse a single raw query string by splitting on whitespace, then applying
/// the same token rules as [`parse`]. Used by interactive front-ends where the
/// query is one edited string rather than pre-split argv.
pub fn parse_str(input: &str) -> Vec<Token> {
    let args: Vec<String> = input.split_whitespace().map(str::to_string).collect();
    parse(&args)
}

/// How query results are ordered.
///
/// Both orders are total (ties broken by `id`), so paging with a keyset cursor
/// is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    /// Most recently accessed first; ties broken by descending id.
    Recency,
    /// Lexicographic by directory name, ascending.
    Id,
}

impl SortOrder {
    /// The default order for every front-end: what the user most often wants.
    pub const DEFAULT: SortOrder = SortOrder::Recency;

    /// Parse the spelling accepted by `--sort=` and the `default_sort` config
    /// key. Returns `None` for anything else so callers can phrase their own
    /// error (usage vs. config).
    pub fn parse(value: &str) -> Option<SortOrder> {
        match value {
            "recency" => Some(SortOrder::Recency),
            "id" => Some(SortOrder::Id),
            _ => None,
        }
    }

    /// The canonical spelling, as accepted by [`SortOrder::parse`].
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::Recency => "recency",
            SortOrder::Id => "id",
        }
    }

    /// The other order; used by the TUI's toggle key.
    pub fn toggled(self) -> SortOrder {
        match self {
            SortOrder::Recency => SortOrder::Id,
            SortOrder::Id => SortOrder::Recency,
        }
    }
}

impl Default for SortOrder {
    fn default() -> Self {
        SortOrder::DEFAULT
    }
}
