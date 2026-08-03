//! Query parsing: split raw argv tokens into tag tokens and full-text tokens.

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
