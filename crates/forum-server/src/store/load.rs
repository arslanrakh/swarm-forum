//! Startup: loading the state tables (design D.11.8). Startup loads the state; it never replays
//! the history (FQ-2), and there is no state hash, hash chain, or snapshot: the state tables are
//! written in the transaction of the events that change them, so the last commit is the state.
//!
//! [`open`] runs steps 1 and 2: it opens the database, creates the schema and writes the genesis
//! transaction when the file is empty, migrates older state and shell tables in one transaction
//! (a newer version stops the server), and loads the resident rows into a `State` whose indices it
//! recounts. Step 3 (settle to `last_now`) needs the core's `settle` and belongs to the engine,
//! which then runs step 4, [`check_loaded`], before it applies the configured parameters (step 6)
//! and serves.

use std::path::Path;

use forum_core::{
    Params, State, Time, check_invariants, check_keys, check_startup, genesis, recount,
};
use rusqlite::Connection;

use super::history::{TxKind, TxMeta};
use super::migrate::{Migrated, Steps, migrate};
use super::schema::{
    SHELL_VERSION, STATE_VERSION, apply_pragmas, create_tables, has_schema, meta, set_meta,
};
use super::state::load_facts;
use super::{Result, Store, StoreError, write};

/// What startup needs to create a database.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Genesis {
    /// The parameters from the configuration (D.11.9).
    pub params: Params,
    /// The time of the genesis event.
    pub now: Time,
    /// The masking version in force (D.11.13), recorded in `meta`.
    pub masking_version: u32,
}

/// An opened database and its loaded state.
#[derive(Debug)]
pub struct Opened {
    /// The writer's connection.
    pub store: Store,
    /// The state loaded from the resident rows, with its indices recounted (D.11.8, step 2).
    pub state: State,
    /// The database was empty and Genesis was written.
    pub created: bool,
    /// The migrations step 1 ran.
    pub migrated: Vec<Migrated>,
}

/// Creates the schema and writes the genesis transaction into an empty database: its event, its
/// shell rows, and the state rows it produces, with the `meta` rows (D.11.8, step 1; D.7.6).
///
/// # Errors
///
/// A failed statement, for example on a database that already has the schema.
pub fn create(conn: &Connection, g: &Genesis) -> Result<State> {
    create_tables(conn)?;
    set_meta(conn, meta::STATE_VERSION, &STATE_VERSION.to_string())?;
    set_meta(conn, meta::SHELL_VERSION, &SHELL_VERSION.to_string())?;
    set_meta(conn, meta::CORE_SEMVER, env!("CARGO_PKG_VERSION"))?;
    set_meta(conn, meta::KEYS_VERSION, &forum_keys::VERSION.to_string())?;
    set_meta(conn, meta::MASKING_VERSION, &g.masking_version.to_string())?;
    let mut s = State::default();
    let e = genesis(g.params.clone(), g.now);
    let mut dirty = forum_core::Tracked::new();
    forum_core::evolve_tracked(&mut s, &e, &mut dirty);
    if s.anomalies != 0 {
        return Err(StoreError::Invalid("Genesis was not applied".to_owned()));
    }
    write(
        conn,
        &TxMeta::new(TxKind::Genesis, g.now),
        std::slice::from_ref(&e),
        &s,
        &dirty,
    )?;
    Ok(s)
}

/// Loads the state from the resident rows and recounts every index (D.11.8, step 2), `ix.next`
/// with the stored `since` values included. Reads no history table.
///
/// # Errors
///
/// A failed query, or the first row that does not decode, named by its table and key.
pub fn load_state(conn: &Connection) -> Result<State> {
    let mut s = load_facts(conn)?;
    s.ix = recount(&s, s.last_now);
    Ok(s)
}

/// Opens (or creates) the database at `path` and loads its state: steps 1 and 2 of D.11.8, with
/// the migrations of `steps` (`migrate::RELEASED` in production).
///
/// # Errors
///
/// `Newer` for a database of a later release, a failed migration, or a row that does not decode;
/// each stops the server.
pub fn open(path: &Path, g: &Genesis, steps: &Steps) -> Result<Opened> {
    let conn = Connection::open(path)?;
    open_connection(conn, g, steps)
}

/// `open` on a connection already made (an in-memory database in tests).
///
/// # Errors
///
/// As `open`.
pub fn open_connection(conn: Connection, g: &Genesis, steps: &Steps) -> Result<Opened> {
    apply_pragmas(&conn)?;
    let mut store = Store::from_connection(conn);
    let tx = store.begin()?;
    let created = if has_schema(&tx)? {
        false
    } else {
        create(&tx, g)?;
        true
    };
    let migrated = migrate(&tx, steps, g.now)?;
    tx.commit()?;
    let state = load_state(store.conn())?;
    Ok(Opened {
        store,
        state,
        created,
        migrated,
    })
}

/// How much of D.9.1 step 4 runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Checks {
    /// The linear subset, at every startup, release builds included.
    Startup,
    /// Also K2 over every stored key and origin, and the full `check_invariants`: debug builds,
    /// `--check`, and `forum-server check` on a copy.
    Full,
}

/// Step 4 of D.11.8 on a loaded and settled state: `check_startup`, and with `Checks::Full` also
/// `check_keys` with `forum_keys::is_normal` and `check_invariants`. A violation stops the server.
///
/// # Errors
///
/// `Check` with every violation found.
pub fn check_loaded(s: &State, checks: Checks) -> Result<()> {
    let mut all = Vec::new();
    match checks {
        Checks::Startup => {
            if let Err(v) = check_startup(s, s.last_now) {
                all.extend(v);
            }
        }
        Checks::Full => {
            let oracle = |k: &forum_core::KeyStr, nv: forum_core::NormVer| {
                forum_keys::is_normal(k.as_str(), nv.0)
            };
            if let Err(v) = check_keys(s, &oracle) {
                all.extend(v);
            }
            // The full check includes the startup subset.
            if let Err(v) = check_invariants(s, s.last_now) {
                all.extend(v);
            }
        }
    }
    if all.is_empty() {
        Ok(())
    } else {
        Err(StoreError::Check(all))
    }
}
