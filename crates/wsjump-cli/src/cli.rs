//! Argv parsing: identify the subcommand and its arguments.

use wsjump_core::error::{AppError, AppResult};
use wsjump_core::query::SortOrder;

/// A parsed command line: which subcommand to run, with its arguments.
#[derive(Debug)]
pub enum Command {
    /// `top [--sort=<order>] <query...>`
    Top(Query),
    /// `ls [--sort=<order>] <query...>`
    Ls(Query),
    /// `new <slug...>`
    New(Vec<String>),
    /// `touch [<path>]`; no path means the current directory.
    Touch(Option<String>),
    /// `sync [<path>]`; no path means the whole directory.
    Sync(Option<String>),
    /// `tags [--sort=<order>]`; no sort means the configured default.
    Tags(Option<SortOrder>),
    /// `help` or no arguments at all.
    Help,
}

/// A query plus how to order its results.
#[derive(Debug)]
pub struct Query {
    /// Query tokens, possibly empty (meaning "match everything").
    pub tokens: Vec<String>,
    /// Explicit `--sort`, or `None` to use the configured default.
    pub sort: Option<SortOrder>,
}

/// Parse process arguments (excluding argv[0]) into a `Command`.
///
/// No arguments -> `Help`. An unknown subcommand is a usage error.
pub fn parse(args: &[String]) -> AppResult<Command> {
    let (command, rest) = match args.split_first() {
        Some(split) => split,
        None => return Ok(Command::Help),
    };

    match command.as_str() {
        "top" => Ok(Command::Top(parse_query(rest)?)),
        "ls" => Ok(Command::Ls(parse_query(rest)?)),
        "new" => Ok(Command::New(rest.to_vec())),
        "touch" => parse_touch(rest),
        "sync" => parse_sync(rest),
        "tags" => parse_tags(rest),
        "help" => Ok(Command::Help),
        other => Err(AppError::usage(format!("unknown command: {}", other))),
    }
}

/// Parse a query, extracting any `--sort` option.
///
/// A lone `--` ends option parsing so a query token may start with a dash;
/// without it, searching for a slug like `-wip` would be unusable.
fn parse_query(args: &[String]) -> AppResult<Query> {
    let mut tokens = Vec::new();
    let mut sort = None;
    let mut options_done = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if options_done {
            tokens.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => options_done = true,
            // Accept both `--sort=x` and `--sort x`, since both spellings are
            // habitual and silently ignoring one would be surprising.
            _ if arg.starts_with("--sort=") => {
                let value = arg.trim_start_matches("--sort=");
                sort = Some(parse_sort(value)?);
            }
            "--sort" => {
                let value = iter.next().ok_or_else(|| {
                    AppError::usage("--sort requires a value: id or recency".to_string())
                })?;
                sort = Some(parse_sort(value)?);
            }
            // An unknown dashed argument is a mistake, not a query token: taking
            // it as a token would silently return the wrong results.
            _ if arg.starts_with('-') && arg.len() > 1 => {
                return Err(AppError::usage(format!("unknown option: {}", arg)));
            }
            _ => tokens.push(arg.clone()),
        }
    }

    Ok(Query { tokens, sort })
}

/// Parse a `--sort` value.
fn parse_sort(value: &str) -> AppResult<SortOrder> {
    SortOrder::parse(value).ok_or_else(|| {
        AppError::usage(format!(
            "invalid sort order '{}': expected id or recency",
            value
        ))
    })
}

/// Parse `touch [<path>]`, which takes at most one path.
fn parse_touch(args: &[String]) -> AppResult<Command> {
    match args {
        [] => Ok(Command::Touch(None)),
        [path] => Ok(Command::Touch(Some(path.clone()))),
        _ => Err(AppError::usage(
            "touch takes at most one path".to_string(),
        )),
    }
}

/// Parse `sync [<path>]`, which takes at most one path.
///
/// Unlike `touch`, no path means the *whole* directory rather than the current
/// one. The defaults differ deliberately: there is no such thing as touching
/// every workspace, while touching the current one is the common case; syncing is
/// the other way round, and `wsjump sync .` covers the rare case.
fn parse_sync(args: &[String]) -> AppResult<Command> {
    match args {
        [] => Ok(Command::Sync(None)),
        [path] => Ok(Command::Sync(Some(path.clone()))),
        _ => Err(AppError::usage("sync takes at most one path".to_string())),
    }
}

/// Parse `tags [--sort=<order>]`, which takes no positional arguments.
fn parse_tags(args: &[String]) -> AppResult<Command> {
    let mut sort = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            _ if arg.starts_with("--sort=") => {
                let value = arg.trim_start_matches("--sort=");
                sort = Some(parse_sort(value)?);
            }
            "--sort" => {
                let value = iter.next().ok_or_else(|| {
                    AppError::usage("--sort requires a value: id or recency".to_string())
                })?;
                sort = Some(parse_sort(value)?);
            }
            _ => {
                return Err(AppError::usage(format!(
                    "unexpected argument for tags: {}",
                    arg
                )));
            }
        }
    }
    Ok(Command::Tags(sort))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse from string literals, as a shell would supply them.
    fn parse_args(args: &[&str]) -> AppResult<Command> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        parse(&owned)
    }

    /// The query of a `ls` command, or panic.
    fn ls_query(args: &[&str]) -> Query {
        match parse_args(args).unwrap() {
            Command::Ls(query) => query,
            _ => panic!("expected ls"),
        }
    }

    #[test]
    fn no_arguments_is_help() {
        assert!(matches!(parse_args(&[]).unwrap(), Command::Help));
    }

    #[test]
    fn unknown_command_is_a_usage_error() {
        let err = parse_args(&["frobnicate"]).unwrap_err();
        assert_eq!(err.code, 2);
    }

    #[test]
    fn query_tokens_are_collected() {
        let query = ls_query(&["ls", "alpha", ":tag"]);
        assert_eq!(query.tokens, vec!["alpha".to_string(), ":tag".to_string()]);
        assert!(query.sort.is_none(), "no --sort means use the configured default");
    }

    #[test]
    fn sort_accepts_both_spellings() {
        assert_eq!(
            ls_query(&["ls", "--sort=id"]).sort,
            Some(SortOrder::Id),
            "--sort=value must be accepted"
        );
        assert_eq!(
            ls_query(&["ls", "--sort", "id"]).sort,
            Some(SortOrder::Id),
            "--sort value must be accepted too"
        );
    }

    #[test]
    fn sort_accepts_recency() {
        assert_eq!(ls_query(&["ls", "--sort=recency"]).sort, Some(SortOrder::Recency));
    }

    #[test]
    fn sort_does_not_swallow_query_tokens() {
        let query = ls_query(&["ls", "--sort=id", "alpha"]);
        assert_eq!(query.tokens, vec!["alpha".to_string()]);
    }

    #[test]
    fn invalid_sort_value_is_a_usage_error() {
        let err = parse_args(&["ls", "--sort=sideways"]).unwrap_err();
        assert_eq!(err.code, 2);
    }

    #[test]
    fn sort_without_a_value_is_a_usage_error() {
        let err = parse_args(&["ls", "--sort"]).unwrap_err();
        assert_eq!(err.code, 2);
    }

    #[test]
    fn unknown_option_is_a_usage_error() {
        // Silently treating this as a query token would return wrong results.
        let err = parse_args(&["ls", "--nope"]).unwrap_err();
        assert_eq!(err.code, 2);
    }

    #[test]
    fn double_dash_allows_a_dashed_query_token() {
        let query = ls_query(&["ls", "--", "--nope"]);
        assert_eq!(query.tokens, vec!["--nope".to_string()]);
    }

    #[test]
    fn a_bare_dash_is_a_query_token() {
        // Not an option, so it must stay searchable.
        let query = ls_query(&["ls", "-"]);
        assert_eq!(query.tokens, vec!["-".to_string()]);
    }

    #[test]
    fn touch_without_a_path_means_the_current_directory() {
        assert!(matches!(
            parse_args(&["touch"]).unwrap(),
            Command::Touch(None)
        ));
    }

    #[test]
    fn touch_takes_a_path() {
        match parse_args(&["touch", "G:/ws/w/20260730-wsjump"]).unwrap() {
            Command::Touch(Some(path)) => assert_eq!(path, "G:/ws/w/20260730-wsjump"),
            _ => panic!("expected touch with a path"),
        }
    }

    #[test]
    fn touch_rejects_extra_paths() {
        let err = parse_args(&["touch", "one", "two"]).unwrap_err();
        assert_eq!(err.code, 2);
    }

    #[test]
    fn sync_without_a_path_means_everything() {
        // Deliberately the opposite default from touch: there is no such thing as
        // touching every workspace, but syncing all of them is the main use.
        assert!(matches!(parse_args(&["sync"]).unwrap(), Command::Sync(None)));
    }

    #[test]
    fn sync_takes_a_path() {
        match parse_args(&["sync", "."]).unwrap() {
            Command::Sync(Some(path)) => assert_eq!(path, "."),
            _ => panic!("expected sync with a path"),
        }
    }

    #[test]
    fn sync_rejects_extra_paths() {
        let err = parse_args(&["sync", "one", "two"]).unwrap_err();
        assert_eq!(err.code, 2);
    }

    #[test]
    fn new_keeps_all_arguments() {
        match parse_args(&["new", "my", "slug"]).unwrap() {
            Command::New(args) => assert_eq!(args, vec!["my".to_string(), "slug".to_string()]),
            _ => panic!("expected new"),
        }
    }

    #[test]
    fn tags_without_arguments_uses_the_default_order() {
        assert!(matches!(parse_args(&["tags"]).unwrap(), Command::Tags(None)));
    }

    #[test]
    fn tags_accepts_a_sort_order() {
        match parse_args(&["tags", "--sort", "id"]).unwrap() {
            Command::Tags(Some(SortOrder::Id)) => {}
            _ => panic!("expected tags with id sort"),
        }
    }

    #[test]
    fn tags_rejects_positional_arguments() {
        let err = parse_args(&["tags", "rust"]).unwrap_err();
        assert_eq!(err.code, 2);
    }
}
