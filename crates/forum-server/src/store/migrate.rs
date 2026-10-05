//! Upgrades are migrations of the state tables and of the shell's tables (design D.7.6, D.11.8,
//! CS-28): pure row-to-row steps that startup runs in one transaction before it loads the state,
//! recording a history transaction of kind `migrate` that names both versions. A migration never
//! reads the history: a value only the events determine would make an upgrade a replay, which a
//! type change must therefore avoid.
//!
//! This release is `state_version` 1 and `shell_version` 1, the first of each, so the released
//! step lists are empty. A later release appends a step `v1_to_v2` to `STATE_STEPS` (a function of
//! one stored row to the rows that replace it) or to `SHELL_STEPS` (statements that rewrite the
//! shell's tables row to row), and raises the version in `schema`.

use forum_core::Time;
use rusqlite::Connection;

use super::history::{TxKind, TxMeta, insert_tx};
use super::schema::{
    ASIDE, SHELL_VERSION, STATE_VERSION, drop_aside_state_tables, get_version, meta, set_meta,
    set_state_tables_aside,
};
use super::state::{Row, Table, Which, each_row_in, fact_label, put_row, stored_last_now};
use super::{Result, StoreError};

/// One step of the state tables, from version `from` to `from + 1`: a pure function of one stored
/// row (the fact's kind, its key, its value, and its table's columns) to the rows that replace it
/// (D.7.6: `v1_to_v2(fact, key, value) -> Vec<(Fact, key, value)>`).
pub type StateStep = fn(Row) -> std::result::Result<Vec<Row>, String>;

/// One step of the shell's tables, from version `from` to `from + 1`: statements that rewrite
/// them row to row, never reading the history.
pub type ShellStep = fn(&Connection) -> Result<()>;

/// A sequence of steps and the versions it ends at.
#[derive(Clone, Copy)]
pub struct Steps {
    /// `(from, step)`, ascending, one per version.
    pub state: &'static [(u32, StateStep)],
    /// `(from, step)`, ascending, one per version.
    pub shell: &'static [(u32, ShellStep)],
    /// The state version the steps end at.
    pub state_version: u32,
    /// The shell version the steps end at.
    pub shell_version: u32,
}

/// The released steps: none yet (`state_version` 1 and `shell_version` 1).
pub const RELEASED: Steps = Steps {
    state: &[],
    shell: &[],
    state_version: STATE_VERSION,
    shell_version: SHELL_VERSION,
};

/// What one migration did.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Migrated {
    /// `state_version` or `shell_version`.
    pub what: &'static str,
    /// The version before.
    pub from: u32,
    /// The version after.
    pub to: u32,
    /// Rows read (state) or 0 (shell).
    pub rows: u64,
}

/// Brings the state and shell tables to the versions of `steps`, inside the caller's transaction,
/// recording one `migrate` transaction per table group that changed (D.7.6). A newer version than
/// `steps` ends at stops the server. `at` is the clock's reading: the transactions are stamped
/// with it clamped to the stored `last_now`, as every `txs.at` is (D.11.6, T1).
///
/// # Errors
///
/// `Newer`, a missing step, a row a step refuses, or a failed statement.
pub fn migrate(conn: &Connection, steps: &Steps, at: Time) -> Result<Vec<Migrated>> {
    let mut done = Vec::new();
    let found = get_version(conn, meta::STATE_VERSION)?;
    if found > steps.state_version {
        return Err(StoreError::Newer {
            what: meta::STATE_VERSION,
            found,
            supported: steps.state_version,
        });
    }
    let shell_found = get_version(conn, meta::SHELL_VERSION)?;
    if shell_found > steps.shell_version {
        return Err(StoreError::Newer {
            what: meta::SHELL_VERSION,
            found: shell_found,
            supported: steps.shell_version,
        });
    }
    if found < steps.state_version {
        // Every step is found before anything changes.
        let chain = (found..steps.state_version)
            .map(|v| {
                steps
                    .state
                    .iter()
                    .find(|(from, _)| *from == v)
                    .map(|(_, f)| *f)
                    .ok_or_else(|| StoreError::Invalid(format!("no state step from version {v}")))
            })
            .collect::<Result<Vec<StateStep>>>()?;
        let read = migrate_state_rows(conn, &chain)?;
        set_meta(conn, meta::STATE_VERSION, &steps.state_version.to_string())?;
        done.push(Migrated {
            what: meta::STATE_VERSION,
            from: found,
            to: steps.state_version,
            rows: read,
        });
    }
    if shell_found < steps.shell_version {
        for v in shell_found..steps.shell_version {
            let step = steps
                .shell
                .iter()
                .find(|(from, _)| *from == v)
                .map(|(_, f)| *f)
                .ok_or_else(|| StoreError::Invalid(format!("no shell step from version {v}")))?;
            step(conn)?;
        }
        set_meta(conn, meta::SHELL_VERSION, &steps.shell_version.to_string())?;
        done.push(Migrated {
            what: meta::SHELL_VERSION,
            from: shell_found,
            to: steps.shell_version,
            rows: 0,
        });
    }
    if !done.is_empty() {
        // The scalar row is of this release's format by now.
        let at = stored_last_now(conn)?.map_or(at, |last| at.max(last));
        for m in &done {
            record(conn, at, m.what, m.from, m.to)?;
        }
    }
    Ok(done)
}

/// Rewrites every row of the state tables through `chain` (D.7.6), one row at a time: the tables
/// are set aside, each of their rows is read in the load order of `each_row` and its migrated rows
/// are written at once into this release's empty tables, and the set-aside tables are dropped. A
/// migration therefore holds one row and what it becomes, not the state tables, whose rows that
/// left memory and `op_id` responses grow without bound (D.4.11). The load order puts every node
/// before its hot row and its holds; the rows of a consistent state meet no constraint in any
/// order. Returns how many rows were read.
fn migrate_state_rows(conn: &Connection, chain: &[StateStep]) -> Result<u64> {
    set_state_tables_aside(conn)?;
    let mut read = 0_u64;
    each_row_in(conn, ASIDE, Which::All, |r| {
        read = read.saturating_add(1);
        for out in through(chain, r)? {
            put_row(conn, &out)?;
        }
        Ok(())
    })?;
    drop_aside_state_tables(conn)?;
    Ok(read)
}

/// One stored row through every step of a chain.
fn through(chain: &[StateStep], r: Row) -> Result<Vec<Row>> {
    let mut rows = vec![r];
    for step in chain {
        let mut next = Vec::with_capacity(rows.len());
        for r in rows {
            let key = if r.table() == Table::Facts {
                fact_label(r.kind, &r.key)
            } else {
                r.key.clone()
            };
            next.extend(step(r).map_err(|msg| StoreError::Decode {
                table: "migration",
                key,
                msg,
            })?);
        }
        rows = next;
    }
    Ok(rows)
}

/// The `migrate` transaction that names both versions.
fn record(conn: &Connection, at: Time, what: &str, from: u32, to: u32) -> Result<()> {
    let mut m = TxMeta::new(TxKind::Migrate, at);
    m.detail = Some(format!("{what} {from} -> {to}"));
    insert_tx(conn, &m)?;
    Ok(())
}
