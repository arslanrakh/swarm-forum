//! The schema of the one SQLite file (design D.11.6): the state tables, the history, and the shell's
//! tables, the connection's PRAGMAs, the `meta` rows, and the probe of SQLite's build (D.11.1).
//!
//! The DDL is D.11.6's, with the two append-only triggers on each of the three history tables. The
//! four constraints of §24 live on the state tables: the partial unique index of `st_keys`, the
//! primary key of `st_names`, the primary key of `st_ops`, and the two capacity triggers.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, StoreError};

/// The version of the state tables' rows (D.7.6): 1 in this release. A change to a base or history
/// type that an older row does not decode into bumps it and adds a step to `migrate.rs`.
pub const STATE_VERSION: u32 = 1;

/// The version of the shell's tables (D.7.6, D.11.8): 1 in this release, migrated row to row.
pub const SHELL_VERSION: u32 = 1;

/// The `format` of every event this release writes (D.7.6).
pub const EVENT_FORMAT: i64 = 1;

/// The zstd level of the event bodies (D.11.6).
pub const ZSTD_LEVEL: i32 = 3;

/// The keys of the `meta` table (D.11.6).
pub mod meta {
    /// The version of the state tables.
    pub const STATE_VERSION: &str = "state_version";
    /// The semantic version of the core that wrote the genesis transaction.
    pub const CORE_SEMVER: &str = "core_semver";
    /// `forum_keys::VERSION` when the database was created.
    pub const KEYS_VERSION: &str = "keys_version";
    /// The masking version in force (D.11.5, D.11.13).
    pub const MASKING_VERSION: &str = "masking_version";
    /// The version of the shell's tables.
    pub const SHELL_VERSION: &str = "shell_version";
}

/// The state tables (D.11.6), in the order they are loaded.
pub const STATE_TABLES: [&str; 8] = [
    "st_scalars",
    "st_nodes",
    "st_node_hot",
    "st_keys",
    "st_names",
    "st_holds",
    "st_ops",
    "st_facts",
];

/// The history tables (D.11.6): append-only, never read to load the state.
pub const HISTORY_TABLES: [&str; 3] = ["txs", "events", "event_nodes"];

/// The shell's tables (D.11.6), derived from the events.
pub const SHELL_TABLES: [&str; 6] = [
    "entries",
    "sessions",
    "said_links",
    "hidden_files",
    "pauses",
    "origins",
];

/// The `meta` table.
const META_DDL: &str = "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// The state tables, with their four constraints (§24).
const STATE_DDL: &str = "
CREATE TABLE st_scalars (one INTEGER PRIMARY KEY CHECK (one = 0), v TEXT NOT NULL);
CREATE TABLE st_nodes (id INTEGER PRIMARY KEY, capacity INTEGER NOT NULL, v TEXT NOT NULL);
CREATE TABLE st_node_hot (id INTEGER PRIMARY KEY REFERENCES st_nodes(id), v TEXT NOT NULL);
CREATE TABLE st_keys (key TEXT NOT NULL, node INTEGER NOT NULL, nv INTEGER NOT NULL,
                      open INTEGER NOT NULL,
                      PRIMARY KEY (key, node)) WITHOUT ROWID;
CREATE UNIQUE INDEX st_keys_one_open ON st_keys(key) WHERE open = 1;
CREATE TABLE st_names (parent INTEGER NOT NULL, name TEXT NOT NULL, node INTEGER NOT NULL,
                       alias INTEGER NOT NULL,
                       PRIMARY KEY (parent, name)) WITHOUT ROWID;
CREATE TABLE st_holds (node INTEGER NOT NULL REFERENCES st_nodes(id), fence INTEGER NOT NULL,
                       v TEXT NOT NULL, PRIMARY KEY (node, fence)) WITHOUT ROWID;
CREATE TRIGGER st_holds_within_capacity BEFORE INSERT ON st_holds
  WHEN (SELECT count(*) FROM st_holds WHERE node = NEW.node)
       >= (SELECT capacity FROM st_nodes WHERE id = NEW.node)
  BEGIN SELECT RAISE(ABORT, 'holders exceed capacity'); END;
CREATE TRIGGER st_capacity_not_below_holds BEFORE UPDATE OF capacity ON st_nodes
  WHEN (SELECT count(*) FROM st_holds WHERE node = NEW.id) > NEW.capacity
  BEGIN SELECT RAISE(ABORT, 'capacity below live holds'); END;
CREATE TABLE st_ops (class TEXT NOT NULL, op_id TEXT NOT NULL, v TEXT NOT NULL,
                     masking_version INTEGER NOT NULL,
                     response BLOB NOT NULL,
                     PRIMARY KEY (class, op_id)) WITHOUT ROWID;
CREATE TABLE st_facts (fact INTEGER NOT NULL, k TEXT NOT NULL, v TEXT NOT NULL,
                       resident INTEGER NOT NULL DEFAULT 1,
                       PRIMARY KEY (fact, k)) WITHOUT ROWID;
CREATE INDEX st_facts_resident ON st_facts(resident, fact);
";

/// The history: append-only (§16, O3).
const HISTORY_DDL: &str = "
CREATE TABLE txs (id INTEGER PRIMARY KEY, kind INTEGER NOT NULL, at INTEGER NOT NULL,
                  class TEXT, session INTEGER, op_id TEXT, detail TEXT);
CREATE TABLE events (seq INTEGER PRIMARY KEY, tx INTEGER NOT NULL REFERENCES txs(id),
                     at INTEGER NOT NULL, format INTEGER NOT NULL, z BLOB NOT NULL);
CREATE TABLE event_nodes (node INTEGER NOT NULL, seq INTEGER NOT NULL,
                          PRIMARY KEY (node, seq)) WITHOUT ROWID;
CREATE TRIGGER txs_no_update BEFORE UPDATE ON txs
  BEGIN SELECT RAISE(ABORT, 'the history is append-only'); END;
CREATE TRIGGER txs_no_delete BEFORE DELETE ON txs
  BEGIN SELECT RAISE(ABORT, 'the history is append-only'); END;
CREATE TRIGGER events_no_update BEFORE UPDATE ON events
  BEGIN SELECT RAISE(ABORT, 'the history is append-only'); END;
CREATE TRIGGER events_no_delete BEFORE DELETE ON events
  BEGIN SELECT RAISE(ABORT, 'the history is append-only'); END;
CREATE TRIGGER event_nodes_no_update BEFORE UPDATE ON event_nodes
  BEGIN SELECT RAISE(ABORT, 'the history is append-only'); END;
CREATE TRIGGER event_nodes_no_delete BEFORE DELETE ON event_nodes
  BEGIN SELECT RAISE(ABORT, 'the history is append-only'); END;
";

/// The shell's tables, version 1: they point at events rather than copy their texts.
pub(crate) const SHELL_DDL_V1: &str = "
CREATE TABLE entries (eid INTEGER PRIMARY KEY, node INTEGER NOT NULL, seq INTEGER NOT NULL,
                      type TEXT NOT NULL, session INTEGER, principal INTEGER,
                      ev INTEGER NOT NULL, hidden_ev INTEGER,
                      UNIQUE (node, seq));
CREATE INDEX entries_principal ON entries(principal, eid) WHERE principal IS NOT NULL;
CREATE TABLE sessions (id INTEGER PRIMARY KEY, class TEXT NOT NULL, started_at INTEGER NOT NULL,
                       intent_ev INTEGER, status_ev INTEGER);
CREATE TABLE said_links (said INTEGER PRIMARY KEY, ev INTEGER NOT NULL);
CREATE TABLE hidden_files (node INTEGER NOT NULL, name TEXT NOT NULL, ev INTEGER NOT NULL,
                           PRIMARY KEY (node, name)) WITHOUT ROWID;
CREATE TABLE pauses (scope INTEGER PRIMARY KEY, ev INTEGER NOT NULL);
CREATE TABLE origins (origin TEXT PRIMARY KEY, nv INTEGER NOT NULL, request INTEGER NOT NULL);
";

/// Sets the PRAGMAs of D.11.6 on a connection: WAL, full synchronisation, foreign keys, and no
/// automatic checkpoints on the writer (the checkpointer thread checkpoints, D.11.2, CS-13), and
/// room for the writer's prepared statements. An in-memory database keeps its own journal mode.
///
/// # Errors
///
/// A PRAGMA that SQLite refuses.
pub fn apply_pragmas(conn: &Connection) -> Result<()> {
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let _pages: i64 = conn.query_row("PRAGMA wal_autocheckpoint = 0", [], |r| r.get(0))?;
    // The writer's statements: the state rows, the history, and the shell's rows.
    conn.set_prepared_statement_cache_capacity(64);
    Ok(())
}

/// Whether the database has the schema: the `meta` table exists.
///
/// # Errors
///
/// A failed query.
pub fn has_schema(conn: &Connection) -> Result<bool> {
    let found: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Creates every table of D.11.6 in an empty database (inside the caller's transaction).
///
/// # Errors
///
/// A statement that fails, for example because a table already exists.
pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(META_DDL)?;
    conn.execute_batch(STATE_DDL)?;
    conn.execute_batch(HISTORY_DDL)?;
    conn.execute_batch(SHELL_DDL_V1)?;
    Ok(())
}

/// The prefix of the state tables a migration sets aside while it writes the migrated rows into
/// new ones (D.7.6).
pub(crate) const ASIDE: &str = "aside_";

/// Sets the state tables aside for a migration (D.7.6), inside the caller's transaction: drops
/// their indexes and triggers, renames each `st_x` to `aside_st_x` (SQLite rewrites the foreign
/// keys between them), and creates this release's empty state tables with their constraints. The
/// set-aside tables keep their primary keys, so a migration reads them row by row in key order.
///
/// # Errors
///
/// A failed statement, for example a state table that does not exist.
pub(crate) fn set_state_tables_aside(conn: &Connection) -> Result<()> {
    let list = STATE_TABLES
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut st = conn.prepare(&format!(
        "SELECT type, name FROM sqlite_master
         WHERE type IN ('index', 'trigger') AND tbl_name IN ({list})
           AND name NOT LIKE 'sqlite_%'
         ORDER BY type, name"
    ))?;
    let objects: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(st);
    for (ty, name) in &objects {
        let what = if ty == "index" { "INDEX" } else { "TRIGGER" };
        conn.execute_batch(&format!("DROP {what} \"{name}\""))?;
    }
    for t in STATE_TABLES {
        conn.execute_batch(&format!("ALTER TABLE {t} RENAME TO {ASIDE}{t}"))?;
    }
    conn.execute_batch(STATE_DDL)?;
    Ok(())
}

/// Drops the state tables a migration set aside, the tables that reference others first.
///
/// # Errors
///
/// A failed statement.
pub(crate) fn drop_aside_state_tables(conn: &Connection) -> Result<()> {
    for t in [
        "st_holds",
        "st_node_hot",
        "st_keys",
        "st_names",
        "st_ops",
        "st_facts",
        "st_nodes",
        "st_scalars",
    ] {
        conn.execute_batch(&format!("DROP TABLE {ASIDE}{t}"))?;
    }
    Ok(())
}

/// The value of a `meta` row.
///
/// # Errors
///
/// A failed query.
pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

/// Sets a `meta` row.
///
/// # Errors
///
/// A failed statement.
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// A version number kept in `meta`.
///
/// # Errors
///
/// A missing row, or one that is not a number.
pub fn get_version(conn: &Connection, key: &'static str) -> Result<u32> {
    let raw = get_meta(conn, key)?.ok_or_else(|| StoreError::Decode {
        table: "meta",
        key: key.to_owned(),
        msg: "missing".to_owned(),
    })?;
    raw.parse::<u32>().map_err(|e| StoreError::Decode {
        table: "meta",
        key: key.to_owned(),
        msg: e.to_string(),
    })
}

/// What the probe of SQLite's build found (D.11.1): FTS5 is compiled in, and a contentless FTS5
/// table with `contentless_delete` accepts a delete, which the search file needs (D.11.12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fts5Probe {
    /// The SQLite version.
    pub version: String,
    /// `PRAGMA compile_options` lists `ENABLE_FTS5`.
    pub compiled: bool,
    /// The contentless-delete table accepted an insert, a match, and a delete.
    pub contentless_delete: bool,
}

/// Probes the linked SQLite on a scratch in-memory database (D.11.1, WP-E1).
///
/// # Errors
///
/// When the scratch database cannot be opened or queried at all.
pub fn probe_fts5() -> Result<Fts5Probe> {
    let conn = Connection::open_in_memory()?;
    let version: String = conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    let mut stmt = conn.prepare("PRAGMA compile_options")?;
    let options: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    let compiled = options.iter().any(|o| o == "ENABLE_FTS5");
    let contentless_delete = conn
        .execute_batch(
            "CREATE VIRTUAL TABLE probe USING fts5(body, content='', contentless_delete=1);
             INSERT INTO probe (rowid, body) VALUES (1, 'alpha beta'), (2, 'beta gamma');
             DELETE FROM probe WHERE rowid = 1;",
        )
        .is_ok()
        && conn
            .query_row(
                "SELECT count(*) FROM probe WHERE probe MATCH 'beta'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .is_ok_and(|n| n == 1);
    Ok(Fts5Probe {
        version,
        compiled,
        contentless_delete,
    })
}
