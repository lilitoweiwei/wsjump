//! Workspace data model and directory-name parsing.

use std::path::PathBuf;

use time::{Date, Month};

/// A single workspace discovered under `w/`.
pub struct Workspace {
    /// The whole directory name, e.g. "20260730-wsjump".
    pub id: String,
    /// Creation date parsed from the 8-digit prefix (a real calendar date).
    #[allow(dead_code)]
    pub date: Date,
    /// The part after the first '-'; may be empty.
    #[allow(dead_code)]
    pub slug: String,
    /// Absolute path to the workspace directory.
    pub path: PathBuf,
    /// Tags from the first tag line of `wsmeta.txt` (leading ':' stripped).
    pub tags: Vec<String>,
    /// Description body: all `wsmeta.txt` content except the tag line.
    pub desc: String,
}

/// Parse a directory name into its `(date, slug)` components.
///
/// Rules:
/// - The first 8 characters must parse as a valid calendar date `YYYYMMDD`.
/// - If a 9th character exists it must be '-'; everything after it is the slug
///   (which may itself contain '-', and may be empty).
/// - Returns `None` for any name that does not match (caller warns + skips).
///
/// Valid: "20260730", "20260730-", "20260730-wsjump", "20260730-a-b-c".
pub fn parse_dir_name(name: &str) -> Option<(Date, String)> {
    // The prefix must be exactly 8 ASCII digits; index by bytes since digits
    // are single-byte in UTF-8.
    if name.len() < 8 {
        return None;
    }
    let (prefix, rest) = name.split_at(8);
    if !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    let date = parse_date_prefix(prefix)?;

    // No slug part at all: "20260730".
    if rest.is_empty() {
        return Some((date, String::new()));
    }

    // A 9th character exists: it must be the '-' separator.
    let mut chars = rest.chars();
    if chars.next() != Some('-') {
        return None;
    }
    let slug = chars.as_str().to_string();
    Some((date, slug))
}

/// Parse an 8-digit `YYYYMMDD` string into a real calendar date.
/// Returns `None` if the digits do not form a valid date (e.g. "20261340").
fn parse_date_prefix(prefix: &str) -> Option<Date> {
    let year: i32 = prefix[0..4].parse().ok()?;
    let month_num: u8 = prefix[4..6].parse().ok()?;
    let day: u8 = prefix[6..8].parse().ok()?;

    let month = Month::try_from(month_num).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}
