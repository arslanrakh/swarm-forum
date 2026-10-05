//! Storage (design D.11.6, D.11.8, D.7.6, D.4.12; WP-E1): one SQLite file with three groups of
//! tables, all written in the one transaction that commits the events which change them (FQ-2).
//!
//! - `schema`: the DDL, the PRAGMAs, the `meta` rows, and the probe of SQLite's FTS5;
//! - `state`: the state tables, the source of truth: one row per fact of the core's base and
//!   history parts (D.6.1), and the two-pass writing of a job's dirty facts;
//! - `history`: transactions and events, append-only, each event compressed once;
//! - `shell_tables`: the rows rendering needs, derived from the events, and their rebuild;
//! - `load`: startup's open, genesis, migration, load, recount, and checks (D.11.8);
//! - `migrate`: the versions of the state and shell tables and their row-to-row steps (D.7.6);
//! - `replay`: the history folded through `evolve`, compared with the state tables, and checked
//!   against the invariants, and `replay --into` (D.12.6);
//! - `restore`: the entities a command names that left memory, read back from their rows
//!   (D.4.12).
//!
//! The writer thread (WP-E4) owns the one read-write connection: for each transaction of D.8.1 it
//! calls [`write`] with the events, the state after them, and their dirty facts, then
//! [`state::put_op`] with the rendered response when the call carried an `op_id`, and commits
//! once per group. Any error after `evolve` ran is fatal (crash-only, D.11.2).

pub mod history;
pub mod load;
pub mod migrate;
pub mod replay;
pub mod restore;
pub mod schema;
pub mod shell_tables;
pub mod state;

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;

use std::fmt;

use forum_core::{Event, State, Tracked, Violation};
use rusqlite::Connection;

pub use history::{TxKind, TxMeta};

/// Everything that can go wrong in storage.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite refused a statement: a constraint of §24 among them (a core bug, crash-only).
    Sql(rusqlite::Error),
    /// A value that did not serialise or deserialise.
    Json(serde_json::Error),
    /// zstd failed.
    Io(std::io::Error),
    /// A stored row that does not decode, named by its table and key (D.11.8, step 2).
    Decode {
        /// The table.
        table: &'static str,
        /// The row's key.
        key: String,
        /// What was wrong.
        msg: String,
    },
    /// The database was written by a newer release: the server stops (D.11.8, step 1).
    Newer {
        /// `state_version` or `shell_version`.
        what: &'static str,
        /// The version found.
        found: u32,
        /// The newest this release reads.
        supported: u32,
    },
    /// The loaded state violates invariants (D.11.8, step 4): the server stops.
    Check(Vec<Violation>),
    /// The history does not replay to the state tables (D.12.6).
    Replay(String),
    /// An argument this module cannot store.
    Invalid(String),
}

impl StoreError {
    /// Whether SQLite refused the statement for a constraint: a primary key, a unique index, or a
    /// trigger's `RAISE(ABORT)`.
    #[must_use]
    pub fn is_constraint(&self) -> bool {
        matches!(
            self,
            StoreError::Sql(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation
        )
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sql(e) => write!(f, "sqlite: {e}"),
            StoreError::Json(e) => write!(f, "json: {e}"),
            StoreError::Io(e) => write!(f, "zstd: {e}"),
            StoreError::Decode { table, key, msg } => {
                write!(f, "a row of {table} with key {key} does not decode: {msg}")
            }
            StoreError::Newer {
                what,
                found,
                supported,
            } => write!(
                f,
                "{what} {found} is newer than this release reads ({supported})"
            ),
            StoreError::Check(v) => {
                write!(f, "the state violates {} invariant checks: {v:?}", v.len())
            }
            StoreError::Replay(m) => write!(f, "the history does not replay to the state: {m}"),
            StoreError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> StoreError {
        StoreError::Sql(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> StoreError {
        StoreError::Json(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> StoreError {
        StoreError::Io(e)
    }
}

/// The result of storage.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Runs a statement through the connection's cache of prepared statements: the writer runs the
/// same few statements for every event and every dirty fact of every job.
pub(crate) fn exec(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<usize> {
    Ok(conn.prepare_cached(sql)?.execute(p)?)
}

/// What one [`write`] stored.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Wrote {
    /// The `txs` row.
    pub tx: i64,
    /// The `seq` of the first event.
    pub first_seq: u64,
    /// What the two passes did with the dirty facts.
    pub facts: state::Written,
}

/// Writes one transaction of events (D.11.6, "Writing a job", steps 1 to 3), inside the caller's
/// SQLite transaction: the `txs` row, each event compressed with its `event_nodes` rows, the shell's
/// rows the events imply, and the state rows of `dirty` in two passes, each departed entity's row with
/// its last value (D.4.12, review E1-spec-1). `s` is the state right after
/// the last of `events`, so their `seq`s end at `s.counters.events`. A job that appended no events
/// writes nothing, not even a `txs` row (CS-14): the result is then None. The `st_ops` row of a
/// call with an `op_id` is step 4, [`state::put_op`], once the response is rendered.
///
/// # Errors
///
/// More events than the state ever applied, or a failed statement (a constraint among them: the
/// caller aborts the process, crash-only).
pub fn write(
    conn: &Connection,
    meta: &TxMeta,
    events: &[Event],
    s: &State,
    dirty: &Tracked,
) -> Result<Option<Wrote>> {
    if events.is_empty() {
        return Ok(None);
    }
    let n = u64::try_from(events.len()).unwrap_or(u64::MAX);
    let first_seq = s
        .counters
        .events
        .checked_sub(n)
        .and_then(|x| x.checked_add(1))
        .ok_or_else(|| {
            StoreError::Invalid(format!(
                "{n} events, but the state applied only {}",
                s.counters.events
            ))
        })?;
    let tx = history::insert_tx(conn, meta)?;
    let mut seq = first_seq;
    for e in events {
        history::insert_event(conn, tx, seq, e)?;
        shell_tables::shell_rows(conn, seq, e)?;
        seq = seq.saturating_add(1);
    }
    let facts = state::write_dirty(conn, s, &dirty.facts, &dirty.departed)?;
    Ok(Some(Wrote {
        tx,
        first_seq,
        facts,
    }))
}

/// The one read-write connection of the writer thread (D.11.2).
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Wraps an open connection whose schema is current.
    #[must_use]
    pub fn from_connection(conn: Connection) -> Store {
        Store { conn }
    }

    /// The connection, for reads and for statements inside a transaction.
    #[must_use]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Begins an immediate transaction (D.8.1: `BEGIN IMMEDIATE`).
    ///
    /// # Errors
    ///
    /// When SQLite cannot begin it.
    pub fn begin(&mut self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?)
    }

    /// The connection back.
    #[must_use]
    pub fn into_connection(self) -> Connection {
        self.conn
    }
}
