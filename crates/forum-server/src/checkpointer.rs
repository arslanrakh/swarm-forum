//! WAL checkpoints off the writer (design D.11.2, CS-13).
//!
//! The writer's connection sets `wal_autocheckpoint = 0` (D.11.6): with SQLite's default the
//! committing connection checkpoints, and at tens of GB of writes a day the writer would stall
//! every few seconds. This thread, with its own connection, runs `PRAGMA wal_checkpoint(PASSIVE)`
//! every [`CHECKPOINT_EVERY`], which never blocks the writer, and `RESTART` when the WAL file
//! passes [`WAL_RESTART`], so that the writer starts the WAL again from its beginning. The writer's
//! connection carries `journal_size_limit = WAL_RESTART`, so the file it then reuses is cut back to
//! that size.
//!
//! A `RESTART` holds the write lock while it waits for readers to reach the latest snapshot, so
//! its connection waits at most [`CHECKPOINT_BUSY`] for a lock: a reader that holds an old
//! snapshot makes it give up for the round (`busy`, counted apart, and tried again the next round)
//! instead of holding the writer back, whose own busy timeout is longer (`engine::WRITER_BUSY`;
//! review E4-robust-1). A WAL that stays past the threshold is logged, with what keeps it there.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::Connection;

use crate::store::{Result, StoreError};

/// How often the checkpointer runs a passive checkpoint (D.11.2).
pub const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);
/// The WAL size past which it runs `RESTART` (D.11.2): 1 GiB.
pub const WAL_RESTART: u64 = 1 << 30;
/// How long a checkpoint waits for a lock before it gives up for the round: the writer waits at
/// most this long behind a `RESTART` (review E4-robust-1).
pub const CHECKPOINT_BUSY: Duration = Duration::from_millis(10);
/// Rounds between two warnings about a WAL that stays past the threshold.
const WARN_EVERY: u32 = 60;

/// The WAL file of the database at `db`.
#[must_use]
pub fn wal_path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-wal");
    PathBuf::from(p)
}

/// The size of the WAL file of `db`, 0 without one.
#[must_use]
pub fn wal_size(db: &Path) -> u64 {
    std::fs::metadata(wal_path(db)).map_or(0, |m| m.len())
}

/// What the checkpointer has done.
#[derive(Debug, Default)]
pub struct Stats {
    /// Passive checkpoints run.
    pub passive: AtomicU64,
    /// Restart checkpoints run to the end.
    pub restarts: AtomicU64,
    /// Restart checkpoints that gave up for the round: a reader held an old snapshot, or the
    /// writer the lock.
    pub restarts_busy: AtomicU64,
}

#[derive(Debug, Default)]
struct Stop {
    stop: Mutex<bool>,
    cv: Condvar,
}

/// The running checkpointer.
#[derive(Debug)]
pub struct Checkpointer {
    stop: Arc<Stop>,
    stats: Arc<Stats>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// What a round left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    /// The WAL is within the threshold, or was restarted.
    Done,
    /// The WAL is past the threshold and the restart gave up for the round: a reader held an old
    /// snapshot, or the writer the lock.
    Busy,
}

/// The `busy` column of a `wal_checkpoint` row: 1 when it could not finish.
fn checkpoint(conn: &Connection, mode: &str) -> Result<bool> {
    let busy: i64 = conn.query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |r| r.get(0))?;
    Ok(busy != 0)
}

/// One round: a passive checkpoint, and a restart one if the WAL is past `restart_at`. A restart
/// that could not finish is counted apart and reported, not taken for done.
///
/// # Errors
///
/// A checkpoint SQLite refused.
pub fn round(conn: &Connection, db: &Path, restart_at: u64, stats: &Stats) -> Result<Round> {
    checkpoint(conn, "PASSIVE")?;
    stats.passive.fetch_add(1, Ordering::Relaxed);
    if wal_size(db) <= restart_at {
        return Ok(Round::Done);
    }
    if checkpoint(conn, "RESTART")? {
        stats.restarts_busy.fetch_add(1, Ordering::Relaxed);
        Ok(Round::Busy)
    } else {
        stats.restarts.fetch_add(1, Ordering::Relaxed);
        Ok(Round::Done)
    }
}

/// The checkpointer's connection to the database at `db`, with its short busy timeout.
///
/// # Errors
///
/// When the database cannot be opened.
pub fn connect(db: &Path) -> Result<Connection> {
    let conn = Connection::open(db)?;
    conn.busy_timeout(CHECKPOINT_BUSY)?;
    Ok(conn)
}

impl Checkpointer {
    /// Starts the thread on the database at `db`: a round every `every`, restarting past
    /// `restart_at` bytes of WAL ([`CHECKPOINT_EVERY`] and [`WAL_RESTART`] in production).
    ///
    /// # Errors
    ///
    /// When the database cannot be opened.
    pub fn start(db: &Path, every: Duration, restart_at: u64) -> Result<Checkpointer> {
        let conn = connect(db)?;
        let stop = Arc::new(Stop::default());
        let stats = Arc::new(Stats::default());
        let (s, st, path) = (Arc::clone(&stop), Arc::clone(&stats), db.to_path_buf());
        let thread = std::thread::Builder::new()
            .name("checkpointer".into())
            .spawn(move || {
                let mut last_error = None;
                // Rounds in a row that left the WAL past the threshold.
                let mut stuck = 0_u32;
                loop {
                    {
                        let g = match s.stop.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        let (g, _) = match s.cv.wait_timeout_while(g, every, |stop| !*stop) {
                            Ok(x) => x,
                            Err(p) => p.into_inner(),
                        };
                        if *g {
                            return;
                        }
                    }
                    match round(&conn, &path, restart_at, &st) {
                        Err(e) => {
                            let msg = e.to_string();
                            if last_error.as_ref() != Some(&msg) {
                                tracing::warn!(error = %msg, "a WAL checkpoint failed; it retries");
                            }
                            last_error = Some(msg);
                        }
                        Ok(Round::Busy) => {
                            last_error = None;
                            if stuck.is_multiple_of(WARN_EVERY) {
                                tracing::warn!(
                                    wal = wal_size(&path),
                                    rounds = stuck + 1,
                                    "the WAL stays past its restart size: a reader holds an old \
                                     snapshot; the restart is tried again each round"
                                );
                            }
                            stuck = stuck.saturating_add(1);
                        }
                        Ok(Round::Done) => {
                            last_error = None;
                            if stuck > 0 {
                                tracing::info!(rounds = stuck, "the WAL was restarted");
                            }
                            stuck = 0;
                        }
                    }
                }
            })
            .map_err(StoreError::Io)?;
        Ok(Checkpointer {
            stop,
            stats,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// What it has done.
    #[must_use]
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Stops the thread and waits for it.
    pub fn stop(&self) {
        {
            let mut g = match self.stop.stop.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *g = true;
            self.stop.cv.notify_all();
        }
        let t = match self.thread.lock() {
            Ok(mut g) => g.take(),
            Err(p) => p.into_inner().take(),
        };
        if let Some(t) = t {
            let _ = t.join();
        }
    }
}

impl Drop for Checkpointer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testkit::TempDb;

    fn writer(db: &Path, limit: u64) -> Connection {
        let c = Connection::open(db).unwrap();
        crate::store::schema::apply_pragmas(&c).unwrap();
        c.pragma_update(None, "journal_size_limit", i64::try_from(limit).unwrap())
            .unwrap();
        c.execute_batch("CREATE TABLE t (x BLOB)").unwrap();
        c
    }

    /// Writes `rounds` transactions of 64 KiB each, sampling the WAL size after each.
    fn load(c: &Connection, db: &Path, rounds: usize) -> u64 {
        let blob = vec![7_u8; 64 * 1024];
        let mut max = 0;
        for i in 0..rounds {
            c.execute("INSERT INTO t (x) VALUES (?1)", [&blob]).unwrap();
            if i % 8 == 0 {
                c.execute("DELETE FROM t", []).unwrap();
            }
            max = max.max(wal_size(db));
            std::thread::sleep(Duration::from_millis(2));
        }
        max
    }

    #[test]
    fn the_checkpointer_keeps_the_wal_bounded_under_load() {
        let restart_at = 512 * 1024;
        // Without it, the writer's WAL grows with every write (wal_autocheckpoint = 0).
        let a = TempDb::new("wal-unbounded");
        let c = writer(&a.path, restart_at);
        let grown = load(&c, &a.path, 150);
        assert!(grown > 4 * restart_at, "{grown}");
        // With it, the WAL is restarted past the threshold and cut back to it.
        let b = TempDb::new("wal-bounded");
        let c = writer(&b.path, restart_at);
        let cp = Checkpointer::start(&b.path, Duration::from_millis(10), restart_at).unwrap();
        let max = load(&c, &b.path, 150);
        assert!(max < 4 * restart_at, "{max}");
        assert!(cp.stats().passive.load(Ordering::Relaxed) > 0);
        assert!(cp.stats().restarts.load(Ordering::Relaxed) > 0);
        cp.stop();
    }

    /// A reader with an old snapshot keeps a `RESTART` from finishing; the restart gives up for
    /// the round within `CHECKPOINT_BUSY` and says so, instead of holding the write lock for its
    /// whole busy timeout while the writer's `BEGIN IMMEDIATE` waits and then fails (review
    /// E4-robust-1: 5.3 s, and "database is locked" in 2 of 6 attempts).
    #[test]
    fn a_restart_held_up_by_a_reader_does_not_hold_the_writer_back() {
        let restart_at = 64 * 1024;
        let db = TempDb::new("restart-reader");
        let w = writer(&db.path, restart_at);
        load(&w, &db.path, 4);
        // A reader holds a snapshot in the WAL; more writes come after it.
        let r = Connection::open(&db.path).unwrap();
        r.execute_batch("BEGIN").unwrap();
        let _: i64 = r
            .query_row("SELECT count(*) FROM t", [], |x| x.get(0))
            .unwrap();
        load(&w, &db.path, 8);
        assert!(wal_size(&db.path) > restart_at);
        let cp = connect(&db.path).unwrap();
        let path = db.path.clone();
        let rounds = std::thread::spawn(move || {
            let stats = Stats::default();
            let start = crate::clock::Mono::now();
            let out = round(&cp, &path, restart_at, &stats).unwrap();
            (out, crate::clock::Mono::now().since(start), stats)
        });
        std::thread::sleep(Duration::from_millis(2));
        let start = crate::clock::Mono::now();
        let begun = w.execute_batch("BEGIN IMMEDIATE");
        let waited = crate::clock::Mono::now().since(start);
        assert!(begun.is_ok(), "{begun:?}");
        w.execute_batch("COMMIT").unwrap();
        let (out, took, stats) = rounds.join().unwrap();
        assert!(
            waited < Duration::from_secs(1),
            "the writer waited {waited:?}"
        );
        assert!(took < Duration::from_secs(1), "the round took {took:?}");
        assert_eq!(out, Round::Busy, "not taken for done");
        assert_eq!(stats.restarts.load(Ordering::Relaxed), 0);
        assert_eq!(stats.restarts_busy.load(Ordering::Relaxed), 1);
        // Once the reader is gone, the next round restarts the WAL.
        r.execute_batch("COMMIT").unwrap();
        let cp = connect(&db.path).unwrap();
        let stats = Stats::default();
        assert_eq!(
            round(&cp, &db.path, restart_at, &stats).unwrap(),
            Round::Done
        );
        assert_eq!(stats.restarts.load(Ordering::Relaxed), 1);
    }
}
