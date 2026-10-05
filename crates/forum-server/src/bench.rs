//! The scale gates' instruments (feature `bench`, WP-F4, design D.12.8), which no release build
//! has. `serve --bench-stats` writes what they count once a second, with the engine's counters:
//!
//! - [`count_wal`]: the frames the writer's commits append to the WAL (`sqlite3_wal_hook`), so
//!   that gate G-8 measures the bytes written per append, not the file's growth (a row rewritten
//!   in place reuses its pages and barely grows the file);
//! - [`trace_startup`] and [`end_startup`]: the statements startup runs, with their virtual-machine
//!   steps (`sqlite3_trace_v2`), those on the history tables apart, so that gate G-7 checks that
//!   startup reads no history;
//! - [`searched`]: the time of each search, by the call that searched, measured on the server,
//!   so that gate G-9 bounds the search itself by its budget, apart from the call around it;
//! - [`set_sizes`]: the sizes of the collections a retention bounds, in the state and in the
//!   shell's maps, so that gate G-6 checks that each stops growing;
//! - [`commit_probe`]: the time of one durable SQLite commit made as the store makes it (WAL,
//!   `synchronous = FULL`), for the machine record of every gate.

use std::collections::BTreeMap;
use std::ffi::c_int;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::hooks::Wal;
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Connection, StatementStatus};
use serde_json::{Value, json};

use crate::clock::Mono;
use crate::search::Query;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

// ---------------------------------------------------------------------------------------------
// The WAL

/// Frames appended to the WAL since the server started.
static WAL_FRAMES: AtomicU64 = AtomicU64::new(0);
/// Frames in the WAL after the last commit (the hook is told the total, not the frames added).
static WAL_LAST: AtomicU64 = AtomicU64::new(0);
/// The database's page size: a frame is a page and a 24-byte header.
static PAGE_SIZE: AtomicU64 = AtomicU64::new(0);

/// Counts the WAL frames that commits on `conn` append. Call it after the connection's PRAGMAs:
/// `wal_autocheckpoint` replaces the hook. A WAL left by an earlier run is counted once, at the
/// first commit; the gates read the counter's changes over a window, after startup.
///
/// # Errors
///
/// The page size could not be read.
pub fn count_wal(conn: &Connection) -> rusqlite::Result<()> {
    let page: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    PAGE_SIZE.store(u64::try_from(page).unwrap_or(0), Ordering::Relaxed);
    conn.wal_hook(Some(on_wal));
    Ok(())
}

fn on_wal(_: &Wal, pages: c_int) -> rusqlite::Result<()> {
    let now = u64::try_from(pages).unwrap_or(0);
    let last = WAL_LAST.swap(now, Ordering::Relaxed);
    // After a checkpoint the next commit restarts the WAL from its beginning.
    let added = if now >= last { now - last } else { now };
    WAL_FRAMES.fetch_add(added, Ordering::Relaxed);
    Ok(())
}

/// Bytes the commits have appended to the WAL: frames times (page size + 24).
#[must_use]
pub fn wal_bytes() -> u64 {
    WAL_FRAMES
        .load(Ordering::Relaxed)
        .saturating_mul(PAGE_SIZE.load(Ordering::Relaxed).saturating_add(24))
}

// ---------------------------------------------------------------------------------------------
// Startup's statements

/// The history: tables that grow with every call and that startup must not read (D.11.8).
pub const HISTORY_TABLES: [&str; 4] = ["events", "txs", "event_nodes", "entries"];

/// Per statement text, the most virtual-machine steps seen when it finished. A cached statement's
/// counter adds up over its runs, so its largest reading is its total; a statement prepared anew
/// each time counts its largest run.
static STARTUP: Mutex<Option<BTreeMap<String, i64>>> = Mutex::new(None);
/// What [`end_startup`] found, as JSON.
static STARTUP_REPORT: Mutex<Option<Value>> = Mutex::new(None);

/// Whether `sql` names a history table.
#[must_use]
pub fn reads_history(sql: &str) -> bool {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|w| HISTORY_TABLES.iter().any(|t| w.eq_ignore_ascii_case(t)))
}

/// Starts recording the statements that run on `conn` (startup, gate G-7).
pub fn trace_startup(conn: &Connection) {
    *lock(&STARTUP) = Some(BTreeMap::new());
    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, Some(on_profile));
}

fn on_profile(e: TraceEvent<'_>) {
    if let TraceEvent::Profile(stmt, _) = e {
        let steps = i64::from(stmt.get_status(StatementStatus::VmStep));
        if let Some(m) = lock(&STARTUP).as_mut() {
            let e = m.entry(stmt.sql().into_owned()).or_insert(0);
            *e = (*e).max(steps);
        }
    }
}

/// Stops recording on `conn` and keeps the summary: statements and steps, all and on the history.
pub fn end_startup(conn: &Connection) {
    conn.trace_v2(TraceEventCodes::empty(), None);
    let Some(m) = lock(&STARTUP).take() else {
        return;
    };
    let (mut steps, mut history_steps) = (0_i64, 0_i64);
    let mut history = Vec::new();
    for (sql, s) in &m {
        steps = steps.saturating_add(*s);
        if reads_history(sql) {
            history_steps = history_steps.saturating_add(*s);
            history.push(json!({"sql": sql, "steps": s}));
        }
    }
    *lock(&STARTUP_REPORT) = Some(json!({
        "statements": m.len(),
        "steps": steps,
        "history_statements": history.len(),
        "history_steps": history_steps,
        "history": history,
    }));
}

/// The summary of startup's statements, once startup has ended.
#[must_use]
pub fn startup() -> Value {
    lock(&STARTUP_REPORT).clone().unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------------------------
// Searches

/// Searches to keep per kind between two reports: a second's worth at any gate's rate.
const SEARCHES_KEPT: usize = 100_000;
/// Each search since the last report, by the call that searched: its microseconds and the tag of
/// its text ([`text_tag`]).
static SEARCHES: Mutex<BTreeMap<&'static str, Vec<[u64; 2]>>> = Mutex::new(BTreeMap::new());
/// Searches not kept (more than [`SEARCHES_KEPT`] of a kind in a second).
static SEARCHES_DROPPED: AtomicU64 = AtomicU64::new(0);

/// The kind of call a query belongs to.
#[must_use]
pub fn search_kind(q: &Query) -> &'static str {
    match q {
        Query::Hello(_) => "hello",
        Query::FromUser(_) => "from_user",
        Query::Create(_) => "create",
        Query::Find(_) => "find",
    }
}

/// The tag of a search text: its FNV-1a hash, 32 bits, so that a gate can pick out the searches
/// of the texts it chose (G-9's worst case) from the others.
#[must_use]
pub fn text_tag(text: &str) -> u64 {
    let mut h: u32 = 0x811c_9dc5;
    for b in text.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    u64::from(h)
}

/// Records one search of `q` that took `took` (gate G-9).
pub fn searched(q: &Query, took: Duration) {
    let us = u64::try_from(took.as_micros()).unwrap_or(u64::MAX);
    let text = match q {
        Query::Hello(t) | Query::FromUser(t) | Query::Create(t) | Query::Find(t) => t,
    };
    let mut m = lock(&SEARCHES);
    let v = m.entry(search_kind(q)).or_default();
    if v.len() < SEARCHES_KEPT {
        v.push([us, text_tag(text)]);
    } else {
        SEARCHES_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// The searches since the last call, by kind, as microseconds and text tags, and how many were
/// not kept.
#[must_use]
pub fn take_searches() -> (BTreeMap<&'static str, Vec<[u64; 2]>>, u64) {
    (
        std::mem::take(&mut *lock(&SEARCHES)),
        SEARCHES_DROPPED.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------------------------
// Sizes

static SIZES: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// Records the sizes of the collections a retention bounds (gate G-6).
pub fn set_sizes(items: &[(&'static str, usize)]) {
    let mut m = lock(&SIZES);
    for (k, n) in items {
        m.insert(k, u64::try_from(*n).unwrap_or(u64::MAX));
    }
}

/// The sizes last recorded.
#[must_use]
pub fn sizes() -> BTreeMap<&'static str, u64> {
    lock(&SIZES).clone()
}

// ---------------------------------------------------------------------------------------------
// The commit probe

/// Times `n` commits of a 4 KiB row each on a database of its own in `dir`, with the store's
/// PRAGMAs (`crate::store::schema::apply_pragmas`: WAL, `synchronous = FULL`): the sync the
/// server's commits pay, whatever the OS calls it (on macOS SQLite calls `fsync`, not
/// `F_FULLFSYNC`, unless `PRAGMA fullfsync` is on, and the store leaves it off). Returns the
/// microseconds of each commit and the `fullfsync` setting.
///
/// # Errors
///
/// The probe's database could not be made or written.
pub fn commit_probe(dir: &Path, n: u32) -> Result<(Vec<u64>, i64), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!(".commit-probe-{}.db", std::process::id()));
    let remove = |p: &Path| {
        for suffix in ["", "-wal", "-shm"] {
            let mut f = p.as_os_str().to_owned();
            f.push(suffix);
            let _ = std::fs::remove_file(std::path::PathBuf::from(f));
        }
    };
    remove(&path);
    let result = (|| -> Result<(Vec<u64>, i64), String> {
        let conn = Connection::open(&path).map_err(|e| e.to_string())?;
        crate::store::schema::apply_pragmas(&conn).map_err(|e| e.to_string())?;
        let full: i64 = conn
            .query_row("PRAGMA fullfsync", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("CREATE TABLE probe (id INTEGER PRIMARY KEY, b BLOB NOT NULL)")
            .map_err(|e| e.to_string())?;
        let block = vec![0x5a_u8; 4_096];
        let mut out = Vec::new();
        for _ in 0..n {
            let t = Mono::now();
            conn.execute("INSERT INTO probe (b) VALUES (?1)", [&block])
                .map_err(|e| e.to_string())?;
            out.push(u64::try_from(Mono::now().since(t).as_micros()).unwrap_or(u64::MAX));
        }
        Ok((out, full))
    })();
    remove(&path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_tables_are_named_as_words() {
        assert!(reads_history("SELECT max(seq) FROM events"));
        assert!(reads_history("insert into TXS (id) values (?1)"));
        assert!(reads_history("SELECT node FROM entries WHERE eid = ?1"));
        assert!(!reads_history("SELECT v FROM st_nodes"));
        assert!(!reads_history("SELECT v FROM st_events_like"));
        // FNV-1a, 32 bits: swarm-sim computes the same tag.
        assert_eq!(text_tag(""), 0x811c_9dc5);
        assert_eq!(text_tag("a"), 0xe40c_292c);
    }

    #[test]
    fn the_wal_hook_counts_the_frames_commits_append() {
        let dir = std::env::temp_dir().join(format!("forum-bench-wal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("w.db")).unwrap();
        crate::store::schema::apply_pragmas(&conn).unwrap();
        count_wal(&conn).unwrap();
        let before = wal_bytes();
        conn.execute_batch("CREATE TABLE t (b BLOB)").unwrap();
        conn.execute("INSERT INTO t VALUES (?1)", [vec![1_u8; 20_000]])
            .unwrap();
        let written = wal_bytes() - before;
        assert!(written >= 20_000, "{written}");
        let (times, _) = commit_probe(&dir, 3).unwrap();
        assert_eq!(times.len(), 3);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
