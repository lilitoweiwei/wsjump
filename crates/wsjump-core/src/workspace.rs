//! Workspace data model and directory-name parsing.

use std::path::PathBuf;

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// A single workspace, either discovered on disk or read back from the database.
pub struct Workspace {
    /// The whole directory name, e.g. "20260730-wsjump".
    pub id: String,
    /// Creation date parsed from the 8-digit prefix (a real calendar date).
    pub date: Date,
    /// The part after the first '-'; may be empty.
    pub slug: String,
    /// Absolute path to the workspace directory.
    pub path: PathBuf,
    /// Tags from the first tag line of `wsmeta.txt` (leading ':' stripped).
    pub tags: Vec<String>,
    /// Description body: all `wsmeta.txt` content except the tag line.
    pub desc: String,
    /// Unix seconds of the last recorded access.
    ///
    /// `None` means "not known here": a freshly scanned workspace carries no
    /// access history, since that history lives only in the database. Rows read
    /// from the database always have `Some`, because the column is `NOT NULL`
    /// and is initialised on insert from the directory date.
    pub last_access: Option<i64>,
}

impl Workspace {
    /// Tags rendered as the flat space-separated string stored in the database.
    ///
    /// Tags cannot contain whitespace (they come from splitting a line on it),
    /// so joining is lossless and the string can be split back apart.
    pub fn tags_joined(&self) -> String {
        self.tags.join(" ")
    }

    /// Unix seconds of midnight on the workspace's creation date, in `offset`.
    ///
    /// Used to seed `last_access` when a workspace first enters the database:
    /// creating a workspace is itself a form of contact, so its creation date is
    /// a meaningful initial access time rather than a sentinel.
    pub fn date_midnight(&self, offset: UtcOffset) -> i64 {
        PrimitiveDateTime::new(self.date, Time::MIDNIGHT)
            .assume_offset(offset)
            .unix_timestamp()
    }
}

/// Split a flat space-separated tag string back into tags, as stored by
/// [`Workspace::tags_joined`]. Empty input yields no tags.
pub fn split_tags(joined: &str) -> Vec<String> {
    joined.split_whitespace().map(str::to_string).collect()
}

/// Current time as unix seconds; the timestamp written by an access record.
pub fn now_timestamp() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// Sort workspaces by `id`, ascending.
///
/// Used by callers that must present a deterministic order without the
/// database: `id` is derived from the directory name, so this ordering is always
/// available even when the database is unusable.
pub fn sort_by_id(workspaces: &mut [Workspace]) {
    workspaces.sort_by(|a, b| a.id.cmp(&b.id));
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

    let date = parse_date(prefix)?;

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
pub fn parse_date(prefix: &str) -> Option<Date> {
    if prefix.len() != 8 || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = prefix[0..4].parse().ok()?;
    let month_num: u8 = prefix[4..6].parse().ok()?;
    let day: u8 = prefix[6..8].parse().ok()?;

    let month = Month::try_from(month_num).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

/// Render a date as the 8-digit `YYYYMMDD` form used in directory names and in
/// the database's `date` column. Inverse of [`parse_date`].
pub fn format_date(date: Date) -> String {
    format!(
        "{:04}{:02}{:02}",
        date.year(),
        date.month() as u8,
        date.day()
    )
}
