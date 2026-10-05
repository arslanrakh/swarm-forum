//! The shell's tables (design D.11.6): what rendering needs and the core does not hold, derived
//! from the events and written in their transaction. They point at events (`ev` columns are
//! `seq`s) rather than copy their texts, and `rebuild_shell_tables` derives them again from the
//! history, offline, as a repair (never at startup or in an upgrade, D.11.8).
//!
//! `shell_rows` reads only the event: nothing in these tables depends on the state, so the rebuild
//! needs no fold through `evolve`.

use forum_core::{EntryBody, EntryType, Ev, Event, HideTarget, Requester, SystemNote};
use rusqlite::{Connection, params};

use super::history::events_after;
use super::{Result, exec};

/// The §6 name of an entry type, the `entries.type` column.
#[must_use]
pub fn entry_type_name(t: EntryType) -> &'static str {
    match t {
        EntryType::Note => "note",
        EntryType::Request => "request",
        EntryType::Question => "question",
        EntryType::Answer => "answer",
        EntryType::Decision => "decision",
        EntryType::Checkpoint => "checkpoint",
        EntryType::Said => "said",
        EntryType::Heard => "heard",
        EntryType::FromUser => "from_user",
        EntryType::ToUser => "to_user",
        EntryType::Approve => "approve",
        EntryType::Object => "object",
        EntryType::Resolve => "resolve",
        EntryType::Retract => "retract",
        EntryType::System => "system",
        EntryType::Late => "late",
    }
}

fn i(v: u64) -> Result<i64> {
    i64::try_from(v)
        .map_err(|_| super::StoreError::Invalid(format!("{v} does not fit an SQLite integer")))
}

/// Writes the shell's rows one event implies (D.11.6, step 2), `seq` being the event's own:
/// - `Appended` inserts into `entries`, and a request's origin, with its normaliser version, into
///   `origins` (a second request with one origin violates its primary key: Q1 across memory);
///   `Appended(System(Paused))` upserts its event into `pauses` for its node;
/// - `Hidden` sets `entries.hidden_ev`, or upserts into `hidden_files`;
/// - `SessionOpened`, `SessionHello`, and `SessionStatus` write `sessions`, and `SessionRetired`
///   deletes its row;
/// - a `SaidConfirmed` with a link writes `said_links`;
/// - `Unpaused` and `PauseExpired` delete from `pauses`.
///
/// # Errors
///
/// A failed statement, a constraint among them.
pub fn shell_rows(conn: &Connection, seq: u64, e: &Event) -> Result<()> {
    let ev = i(seq)?;
    match &e.ev {
        Ev::Appended {
            eid,
            node,
            seq: node_seq,
            by,
            body,
            ..
        } => {
            let session = match by {
                forum_core::Author::Session(s) => Some(i(s.0)?),
                forum_core::Author::Server => None,
            };
            let principal = match body {
                EntryBody::FromUser { principal } | EntryBody::ToUser { principal, .. } => {
                    Some(principal.0)
                }
                EntryBody::Request {
                    requester: Requester::User(p),
                    ..
                } => Some(p.0),
                _ => None,
            };
            conn.prepare_cached(
                "INSERT INTO entries (eid, node, seq, type, session, principal, ev, hidden_ev)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
            )?
            .execute(params![
                i(eid.0)?,
                i(node.0)?,
                i(node_seq.0)?,
                entry_type_name(body.entry_type()),
                session,
                principal,
                ev,
            ])?;
            match body {
                EntryBody::Request { origin, .. } => {
                    conn.prepare_cached(
                        "INSERT INTO origins (origin, nv, request) VALUES (?1, ?2, ?3)",
                    )?
                    .execute(params![
                        origin.key.as_str(),
                        origin.nv.0,
                        i(eid.0)?
                    ])?;
                }
                EntryBody::System(SystemNote::Paused { .. }) => {
                    exec(
                        conn,
                        "INSERT INTO pauses (scope, ev) VALUES (?1, ?2)
                         ON CONFLICT (scope) DO UPDATE SET ev = excluded.ev",
                        params![i(node.0)?, ev],
                    )?;
                }
                _ => {}
            }
        }
        Ev::Hidden { target, .. } => match target {
            HideTarget::Entry(eid) => {
                exec(
                    conn,
                    "UPDATE entries SET hidden_ev = ?2 WHERE eid = ?1",
                    params![i(eid.0)?, ev],
                )?;
            }
            HideTarget::File { node, name } => {
                exec(
                    conn,
                    "INSERT INTO hidden_files (node, name, ev) VALUES (?1, ?2, ?3)
                     ON CONFLICT (node, name) DO UPDATE SET ev = excluded.ev",
                    params![i(node.0)?, name.0, ev],
                )?;
            }
        },
        Ev::SessionOpened {
            session,
            class,
            intent,
            ..
        } => {
            exec(
                conn,
                "INSERT INTO sessions (id, class, started_at, intent_ev, status_ev)
                 VALUES (?1, ?2, ?3, ?4, NULL)",
                params![
                    i(session.0)?,
                    class.as_str(),
                    i(e.at.0)?,
                    intent.as_ref().map(|_| ev)
                ],
            )?;
        }
        Ev::SessionHello {
            session,
            intent: Some(_),
            ..
        } => {
            exec(
                conn,
                "UPDATE sessions SET intent_ev = ?2 WHERE id = ?1",
                params![i(session.0)?, ev],
            )?;
        }
        Ev::SessionStatus { session, .. } => {
            exec(
                conn,
                "UPDATE sessions SET status_ev = ?2 WHERE id = ?1",
                params![i(session.0)?, ev],
            )?;
        }
        Ev::SessionRetired { session } => {
            exec(conn, "DELETE FROM sessions WHERE id = ?1", [i(session.0)?])?;
        }
        Ev::SaidConfirmed {
            said,
            link: Some(_),
            ..
        } => {
            exec(
                conn,
                "INSERT INTO said_links (said, ev) VALUES (?1, ?2)
                 ON CONFLICT (said) DO UPDATE SET ev = excluded.ev",
                params![i(said.0)?, ev],
            )?;
        }
        Ev::Unpaused { scope } | Ev::PauseExpired { scope } => {
            exec(conn, "DELETE FROM pauses WHERE scope = ?1", [i(scope.0)?])?;
        }
        _ => {}
    }
    Ok(())
}

/// The request of an origin from the `origins` table, which outlives the request's memory: the
/// shell restores it before a `request` with that origin, so that Q1 holds across memory (D.4.12).
///
/// # Errors
///
/// A failed query.
pub fn origin_request(conn: &Connection, origin: &str) -> Result<Option<forum_core::RequestId>> {
    use rusqlite::OptionalExtension;
    let r: Option<i64> = conn
        .query_row(
            "SELECT request FROM origins WHERE origin = ?1",
            [origin],
            |r| r.get(0),
        )
        .optional()?;
    Ok(r.and_then(|x| u64::try_from(x).ok()).map(forum_core::Eid))
}

/// The node and author of an entry from the `entries` table: what `hide`, `retract`, and
/// `talk answer` to an agent question that left memory add to their command (D.4.12, D.5.2).
///
/// # Errors
///
/// A failed query.
pub fn entry_node(
    conn: &Connection,
    eid: forum_core::Eid,
) -> Result<Option<(forum_core::NodeId, Option<forum_core::SessionId>)>> {
    use rusqlite::OptionalExtension;
    let row: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT node, session FROM entries WHERE eid = ?1",
            [i(eid.0)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(n, s)| {
        (
            forum_core::NodeId(u64::try_from(n).unwrap_or(0)),
            s.and_then(|x| u64::try_from(x).ok())
                .map(forum_core::SessionId),
        )
    }))
}

/// The seq of the last entry of `node` recorded at or before entry `eid`, 0 for none: `view feed
/// {since: eN}` lists the node's entries after it (1.6, §20.4). Entry ids grow with time, as each
/// node's seqs do, so `eid` may be an entry of any node.
///
/// # Errors
///
/// A failed query.
pub fn seq_through(
    conn: &Connection,
    node: forum_core::NodeId,
    eid: forum_core::Eid,
) -> Result<u64> {
    let seq: i64 = conn.query_row(
        "SELECT coalesce(max(seq), 0) FROM entries WHERE node = ?1 AND eid <= ?2",
        [i(node.0)?, i(eid.0)?],
        |r| r.get(0),
    )?;
    Ok(u64::try_from(seq).unwrap_or(0))
}

/// Empties the shell's tables and derives them again from the whole history, in `seq` order: the
/// offline repair `forum-server rebuild-shell-tables` (D.11.6), inside the caller's transaction.
///
/// # Errors
///
/// A failed statement, or an event that does not decode.
pub fn rebuild_shell_tables(conn: &Connection) -> Result<u64> {
    conn.execute_batch(
        "DELETE FROM entries; DELETE FROM sessions; DELETE FROM said_links;
         DELETE FROM hidden_files; DELETE FROM pauses; DELETE FROM origins;",
    )?;
    let mut after = 0_u64;
    let mut n = 0_u64;
    loop {
        let page = events_after(conn, after, 1_000)?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.seq;
        for x in &page {
            shell_rows(conn, x.seq, &x.event)?;
            n = n.saturating_add(1);
        }
    }
    Ok(n)
}
