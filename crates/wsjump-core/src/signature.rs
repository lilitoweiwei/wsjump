//! Cheap change-detection signature for a workspace's `wsmeta.txt`.
//!
//! Syncing the index used to re-read every `wsmeta.txt` on every run, which is
//! where nearly all of its cost was. Instead we `stat` each file (much cheaper
//! than opening it) and re-read only those whose signature differs from the one
//! stored in the database.
//!
//! # `None` means exactly one thing
//!
//! [`MetaSignature`] is wrapped in an `Option` where `None` means "this
//! workspace has no `wsmeta.txt`" and *nothing else*. It must never also stand
//! for "unknown" or "could not encode", because that overloading has no correct
//! resolution:
//!
//! - if `None == None` counts as unchanged, a file we failed to encode is
//!   indistinguishable from an absent one, and real edits are missed;
//! - if `None` always counts as changed, such a workspace is re-read on every
//!   single sync and never converges -- a permanent slow path.
//!
//! So the encoding below is *total*: it is defined for every `SystemTime` a
//! filesystem can report, and never falls back to a sentinel.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use crate::meta::META_FILE_NAME;

/// Number of nanoseconds in the tick unit used by [`MetaSignature::mtime_ticks`].
///
/// 100ns is deliberate, not arbitrary: it is exactly the resolution of Windows'
/// `FILETIME`, so on NTFS the encoding is lossless. Nanoseconds would not be:
/// `i64` nanoseconds only spans 1970 +/- 292 years, and a timestamp past that
/// silently *wraps* into a plausible-looking value. Ticks span +/- 29227 years.
const NANOS_PER_TICK: u128 = 100;

/// What a `wsmeta.txt` looked like last time we read it.
///
/// Two fields rather than one because they come from the same `stat` call, so
/// the second is free and strictly widens detection: `size` catches edits that
/// deliberately preserve `mtime` (`cp -p`, some sync tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaSignature {
    /// Modification time as signed 100ns ticks since the Unix epoch.
    ///
    /// Signed because times *before* the epoch are reachable: NTFS accepts a
    /// 1960 mtime, and an unsigned encoding would have to report failure for it,
    /// reintroducing the sentinel this type exists to avoid.
    pub mtime_ticks: i64,
    /// Size in bytes.
    pub size: u64,
}

impl MetaSignature {
    /// Encode a modification time as signed 100ns ticks since the Unix epoch.
    ///
    /// Total by construction: times at or after the epoch encode positive, times
    /// before it negative, so every input has a representation.
    pub fn ticks_of(mtime: SystemTime) -> i64 {
        match mtime.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(since) => (since.as_nanos() / NANOS_PER_TICK) as i64,
            // `Err` carries how far *before* the epoch the time is.
            Err(before) => -((before.duration().as_nanos() / NANOS_PER_TICK) as i64),
        }
    }
}

/// Read the current signature of `<workspace_dir>/wsmeta.txt`.
///
/// `None` means the file is absent (or unreadable, which is treated the same
/// way: [`crate::meta::load`] already falls back to empty metadata, so the two
/// cases must agree). Note that an *empty* file yields `Some` with `size == 0`,
/// which is a different state from an absent one -- going from one to the other
/// is a real change and must be detected.
pub fn read(workspace_dir: &Path) -> Option<MetaSignature> {
    let meta = fs::metadata(workspace_dir.join(META_FILE_NAME)).ok()?;
    let mtime = meta.modified().ok()?;
    Some(MetaSignature {
        mtime_ticks: MetaSignature::ticks_of(mtime),
        size: meta.len(),
    })
}

/// Whether the metadata must be re-read, given what the database has stored.
///
/// Compared for *inequality*, never ordering: an mtime moving backwards is a
/// real scenario (restoring a backup, `git checkout` of an older revision,
/// setting timestamps by hand) and a `>` test would silently miss it.
pub fn is_stale(stored: Option<MetaSignature>, current: Option<MetaSignature>) -> bool {
    stored != current
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A `SystemTime` the given number of seconds after the epoch.
    fn after_epoch(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// A `SystemTime` the given number of seconds before the epoch.
    fn before_epoch(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH - Duration::from_secs(secs)
    }

    #[test]
    fn epoch_encodes_to_zero() {
        assert_eq!(MetaSignature::ticks_of(SystemTime::UNIX_EPOCH), 0);
    }

    #[test]
    fn times_after_the_epoch_encode_positive() {
        assert_eq!(MetaSignature::ticks_of(after_epoch(1)), 10_000_000);
    }

    #[test]
    fn times_before_the_epoch_encode_negative() {
        // Reachable in practice: NTFS accepts a pre-1970 mtime. Encoding must
        // not need a sentinel for it, or `None` becomes ambiguous.
        assert_eq!(MetaSignature::ticks_of(before_epoch(1)), -10_000_000);
    }

    #[test]
    fn encoding_is_ordered_across_the_epoch() {
        let earlier = MetaSignature::ticks_of(before_epoch(1));
        let epoch = MetaSignature::ticks_of(SystemTime::UNIX_EPOCH);
        let later = MetaSignature::ticks_of(after_epoch(1));
        assert!(earlier < epoch && epoch < later);
    }

    #[test]
    fn a_pre_epoch_time_is_distinguishable_from_the_epoch() {
        // The bug this guards against: encoding pre-epoch times as a sentinel
        // that collides with some other meaning.
        assert_ne!(
            MetaSignature::ticks_of(before_epoch(315_619_200)),
            MetaSignature::ticks_of(SystemTime::UNIX_EPOCH)
        );
    }

    #[test]
    fn far_future_times_do_not_wrap() {
        // Year ~2286. In *nanoseconds* this exceeds i64 and wraps to a negative
        // value, which would order before every real timestamp and make a
        // changed file look unchanged. The tick unit is what prevents this, so
        // shrinking it must fail here.
        //
        // The specific year matters: some far-future values happen to wrap back
        // to a positive number, which a `> 0` assertion would not catch.
        let ticks = MetaSignature::ticks_of(after_epoch(10_000_000_000));
        assert!(ticks > 0, "far-future mtime wrapped to {}", ticks);
    }

    #[test]
    fn the_tick_unit_is_not_silently_widened() {
        // Pins the encoding itself. Without this, enlarging NANOS_PER_TICK would
        // quietly coarsen change detection -- edits closer together than one
        // tick would stop being noticed -- and every other test here would still
        // pass, since they only compare encodings against each other.
        assert_eq!(
            MetaSignature::ticks_of(after_epoch(1)),
            10_000_000,
            "one second must be 10^7 ticks of 100ns"
        );
    }

    #[test]
    fn one_tick_apart_is_distinguishable() {
        let a = SystemTime::UNIX_EPOCH + Duration::from_nanos(1_000_000_000);
        let b = SystemTime::UNIX_EPOCH + Duration::from_nanos(1_000_000_100);
        assert_ne!(MetaSignature::ticks_of(a), MetaSignature::ticks_of(b));
    }

    #[test]
    fn identical_signatures_are_not_stale() {
        let sig = MetaSignature {
            mtime_ticks: 42,
            size: 7,
        };
        assert!(!is_stale(Some(sig), Some(sig)));
    }

    #[test]
    fn a_differing_mtime_is_stale() {
        let stored = MetaSignature {
            mtime_ticks: 42,
            size: 7,
        };
        let current = MetaSignature {
            mtime_ticks: 43,
            size: 7,
        };
        assert!(is_stale(Some(stored), Some(current)));
    }

    #[test]
    fn an_mtime_moving_backwards_is_stale() {
        // Guards the choice of `!=` over `>`: restoring an older copy changes
        // the file while moving its mtime earlier.
        let stored = MetaSignature {
            mtime_ticks: 100,
            size: 7,
        };
        let current = MetaSignature {
            mtime_ticks: 50,
            size: 7,
        };
        assert!(is_stale(Some(stored), Some(current)));
    }

    #[test]
    fn a_differing_size_alone_is_stale() {
        let stored = MetaSignature {
            mtime_ticks: 42,
            size: 7,
        };
        let current = MetaSignature {
            mtime_ticks: 42,
            size: 8,
        };
        assert!(is_stale(Some(stored), Some(current)));
    }

    #[test]
    fn gaining_or_losing_the_file_is_stale() {
        let sig = MetaSignature {
            mtime_ticks: 42,
            size: 7,
        };
        assert!(is_stale(None, Some(sig)), "file appeared");
        assert!(is_stale(Some(sig), None), "file was deleted");
    }

    #[test]
    fn absent_on_both_sides_is_not_stale() {
        // Must converge: a workspace that never had metadata is not re-read
        // forever.
        assert!(!is_stale(None, None));
    }

    #[test]
    fn an_empty_file_is_not_the_same_as_no_file() {
        let empty = MetaSignature {
            mtime_ticks: 42,
            size: 0,
        };
        assert!(is_stale(Some(empty), None));
    }
}
