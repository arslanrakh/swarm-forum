//! Frozen time-zone tables (design D.11.10).
//!
//! `table(name, from_ms, to_ms)` builds a [`TzTable`] from jiff's bundled time-zone database: the
//! offset in force at `from_ms`, then every change of the offset up to `to_ms`. The database is
//! compiled into the binary (`tzdb-bundle-always`), and this module asks it by name through
//! `TimeZoneDatabase::bundled()`, so neither `TZ`, nor `TZDIR`, nor the system's zoneinfo is ever
//! read. The table travels in the command and the event, so replay and settling never consult a
//! time-zone database (§24), and an updated database changes only the nodes created or updated
//! afterwards.

use forum_cron::{MAX_TRANSITIONS, Transition, TzError, TzTable};
use jiff::Timestamp;
use jiff::tz::TimeZoneDatabase;

/// The zone of a `repeat` without `tz` (§12).
pub const DEFAULT_ZONE: &str = "UTC";

/// How far ahead the shell freezes a table: 50 years (D.11.10).
pub const AHEAD_MS: u64 = 50 * 365 * 86_400_000 + 12 * 86_400_000;

/// The version of the bundled database, recorded in every table (provenance only).
#[must_use]
pub fn tzdb_version() -> &'static str {
    jiff_tzdb::VERSION.unwrap_or("unknown")
}

/// Builds the table of zone `name` for the instants `from_ms..=to_ms` (milliseconds since the
/// epoch). The zone is looked up in the bundled database only; its IANA name, as the database
/// spells it, becomes the table's name. A transition that keeps the offset is left out, since only
/// offsets matter to `forum_cron`; past `to_ms` the last offset holds. More than
/// `MAX_TRANSITIONS` changes within the range is `TooManyTransitions`.
pub fn table(name: &str, from_ms: u64, to_ms: u64) -> Result<TzTable, TzError> {
    let zone = TimeZoneDatabase::bundled()
        .get(name)
        .map_err(|_| TzError::UnknownZone)?;
    let from = timestamp_of(from_ms);
    let to = timestamp_of(to_ms.max(from_ms));
    let initial = zone.to_offset(from).seconds();
    let mut last = initial;
    let mut transitions: Vec<Transition> = Vec::new();
    for t in zone.following(from) {
        if t.timestamp() > to {
            break;
        }
        let offset = t.offset().seconds();
        if offset == last {
            continue;
        }
        if transitions.len() >= MAX_TRANSITIONS {
            return Err(TzError::TooManyTransitions);
        }
        transitions.push(Transition {
            at: t.timestamp().as_second(),
            offset,
        });
        last = offset;
    }
    Ok(TzTable {
        name: zone.iana_name().unwrap_or(name).to_string(),
        tzdb_version: tzdb_version().to_string(),
        initial,
        transitions,
    })
}

/// The instant of `ms`, clamped to the range jiff represents.
fn timestamp_of(ms: u64) -> Timestamp {
    let ms = i64::try_from(ms).unwrap_or(i64::MAX);
    Timestamp::from_millisecond(ms).unwrap_or(if ms < 0 {
        Timestamp::MIN
    } else {
        Timestamp::MAX
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// 2026-10-01T00:00:00Z.
    const NOW: u64 = 1_790_812_800_000;
    const WINDOW: u64 = 28 * 86_400_000;

    const ZONES: [&str; 8] = [
        "UTC",
        "Europe/Berlin",
        "America/New_York",
        "Australia/Lord_Howe",
        "Asia/Kathmandu",
        "Pacific/Apia",
        "Asia/Tokyo",
        "europe/berlin",
    ];

    /// One line per zone: the table as JSON, as the child process prints it too.
    fn dump() -> String {
        let mut out = String::new();
        for z in ZONES {
            let t = table(z, NOW - WINDOW, NOW + AHEAD_MS);
            out.push_str(&format!("{z}={}\n", serde_json::to_string(&t).unwrap()));
        }
        out
    }

    #[test]
    fn berlin_has_its_daylight_saving_shifts() {
        let t = table("Europe/Berlin", NOW - WINDOW, NOW + AHEAD_MS).unwrap();
        assert_eq!(t.name, "Europe/Berlin");
        assert_eq!(t.initial, 7_200, "CEST on 2026-09-03");
        // 2026-10-25T01:00:00Z: back to CET.
        assert_eq!(
            t.transitions.first(),
            Some(&Transition {
                at: 1_792_890_000,
                offset: 3_600
            })
        );
        // About two shifts a year for 50 years, all offsets whole minutes.
        assert!(t.transitions.len() >= 98 && t.transitions.len() <= MAX_TRANSITIONS);
        assert!(forum_cron::valid_table(&t));
        assert_eq!(t.tzdb_version, tzdb_version());
    }

    #[test]
    fn utc_and_kathmandu_have_fixed_offsets() {
        let utc = table(DEFAULT_ZONE, NOW - WINDOW, NOW + AHEAD_MS).unwrap();
        assert_eq!(utc.initial, 0);
        assert!(utc.transitions.is_empty());
        let ktm = table("Asia/Kathmandu", NOW - WINDOW, NOW + AHEAD_MS).unwrap();
        assert_eq!(ktm.initial, 5 * 3_600 + 45 * 60);
        assert!(ktm.transitions.is_empty());
        let lh = table("Australia/Lord_Howe", NOW - WINDOW, NOW + AHEAD_MS).unwrap();
        assert!(
            lh.transitions
                .iter()
                .any(|t| (t.offset - lh.initial).abs() == 1_800),
            "a 30-minute shift"
        );
    }

    #[test]
    fn names_are_looked_up_without_case_and_spelt_as_the_database_does() {
        let t = table("europe/berlin", NOW, NOW + 86_400_000).unwrap();
        assert_eq!(t.name, "Europe/Berlin");
    }

    #[test]
    fn unknown_zones_and_offset_storms_are_refused() {
        assert_eq!(
            table("Mars/Olympus_Mons", NOW, NOW + 1),
            Err(TzError::UnknownZone)
        );
        assert_eq!(table("", NOW, NOW + 1), Err(TzError::UnknownZone));
        // A system path is a name, not a file to read.
        assert_eq!(
            table("/etc/localtime", NOW, NOW + 1),
            Err(TzError::UnknownZone)
        );
        // Two shifts a year from 1970 to the end of jiff's range are far more than MAX_TRANSITIONS.
        assert_eq!(
            table("Europe/Berlin", 0, u64::MAX),
            Err(TzError::TooManyTransitions)
        );
    }

    #[test]
    fn a_range_that_runs_backwards_is_the_offset_at_its_start() {
        let t = table("Europe/Berlin", NOW, NOW - 1).unwrap();
        assert!(t.transitions.is_empty());
        assert_eq!(t.initial, 7_200);
    }

    /// Run only by `the_environment_changes_nothing`, in a child process with `TZ` unset and
    /// `TZDIR` pointing at a bogus directory.
    #[test]
    #[ignore = "run by the_environment_changes_nothing in a child process"]
    fn dump_tables_for_the_parent() {
        print!("<<<TABLES\n{}TABLES>>>\n", dump());
    }

    fn child_dump(configure: impl FnOnce(&mut Command)) -> String {
        let exe = std::env::current_exe().unwrap();
        let mut cmd = Command::new(exe);
        cmd.args([
            "--exact",
            "tz::tests::dump_tables_for_the_parent",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ]);
        configure(&mut cmd);
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        let start = stdout.find("<<<TABLES\n").unwrap() + "<<<TABLES\n".len();
        let end = stdout.find("TABLES>>>").unwrap();
        stdout[start..end].to_string()
    }

    /// D.12.1: the tz builder with `TZ` unset and `TZDIR` pointing at a bogus directory gives the
    /// same tables (no system time-zone access).
    #[test]
    fn the_environment_changes_nothing() {
        let here = dump();
        let bogus = std::env::temp_dir().join("forum-tz-bogus-zoneinfo-does-not-exist");
        let unset = child_dump(|c| {
            c.env_remove("TZ").env("TZDIR", &bogus);
        });
        assert_eq!(unset, here);
        let odd = child_dump(|c| {
            c.env("TZ", "Pacific/Apia").env("TZDIR", &bogus);
        });
        assert_eq!(odd, here);
    }
}
