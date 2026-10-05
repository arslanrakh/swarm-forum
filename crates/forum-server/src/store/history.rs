//! The history (design D.11.6, D.7.6, FQ-2): transactions and events, append-only, never read to
//! load the state.
//!
//! Each event is stored once, as the zstd of its canonical JSON (`format` 1), with its gapless
//! `seq`: the core's event count after it (`Counters::events`, which every applied event raises,
//! Genesis's included), so that the writer needs no read of the history to number an event. One
//! `event_nodes` row per node the event names serves `view history`.

use std::collections::BTreeSet;

use forum_core::{
    Addressee, BasisRef, BasisTarget, ClassName, DigestItem, EntryBody, EntryMeta, Ev, Event,
    HideTarget, NodeId, OpId, Owner, QuestionBody, Restored, Role, Routing, SessionId, SystemNote,
    Time, UqSource, WaitOn,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::schema::{EVENT_FORMAT, ZSTD_LEVEL};
use super::{Result, StoreError, exec};

/// The kind of a history transaction (D.11.6).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum TxKind {
    /// The first transaction: Genesis.
    Genesis,
    /// Deadline firings (D.8.2).
    Settle,
    /// A command.
    Cmd,
    /// `Cmd::Admin` (D.11.8, step 6).
    Admin,
    /// A migration of the state or shell tables (D.7.6): no events, the two versions in `detail`.
    Migrate,
    /// `Restored` before a command (D.4.12).
    Restore,
}

impl TxKind {
    /// The `kind` column.
    #[must_use]
    pub fn code(self) -> i64 {
        match self {
            TxKind::Genesis => 0,
            TxKind::Settle => 1,
            TxKind::Cmd => 2,
            TxKind::Admin => 3,
            TxKind::Migrate => 4,
            TxKind::Restore => 5,
        }
    }

    /// The kind of a `kind` column.
    #[must_use]
    pub fn of_code(code: i64) -> Option<TxKind> {
        match code {
            0 => Some(TxKind::Genesis),
            1 => Some(TxKind::Settle),
            2 => Some(TxKind::Cmd),
            3 => Some(TxKind::Admin),
            4 => Some(TxKind::Migrate),
            5 => Some(TxKind::Restore),
            _ => None,
        }
    }
}

/// A `txs` row before it is written. The transaction keeps no command and no outcome (D.11.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TxMeta {
    /// The kind.
    pub kind: TxKind,
    /// The clamped `now` of the call, or of the settle.
    pub at: Time,
    /// The call's class.
    pub class: Option<ClassName>,
    /// The call's session.
    pub session: Option<SessionId>,
    /// The call's `op_id`.
    pub op_id: Option<OpId>,
    /// Free detail: a migration's two versions.
    pub detail: Option<String>,
}

impl TxMeta {
    /// A transaction of `kind` at `at` with no call attached.
    #[must_use]
    pub fn new(kind: TxKind, at: Time) -> TxMeta {
        TxMeta {
            kind,
            at,
            class: None,
            session: None,
            op_id: None,
            detail: None,
        }
    }
}

/// A `txs` row as read back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TxRow {
    /// The id.
    pub id: i64,
    /// The written fields.
    pub meta: TxMeta,
}

fn sql_u64(v: u64) -> Result<i64> {
    i64::try_from(v).map_err(|_| StoreError::Invalid(format!("{v} does not fit an SQLite integer")))
}

/// The zstd of an event's canonical JSON (D.7.6).
///
/// # Errors
///
/// When the event does not serialise or compress.
pub fn encode_event(e: &Event) -> Result<Vec<u8>> {
    let raw = serde_json::to_vec(e)?;
    Ok(zstd::encode_all(raw.as_slice(), ZSTD_LEVEL)?)
}

/// The event of a stored body of `format` 1.
///
/// # Errors
///
/// An unknown format, or a body that does not decompress or decode.
pub fn decode_event(format: i64, z: &[u8]) -> std::result::Result<Event, String> {
    if format != EVENT_FORMAT {
        return Err(format!("unknown event format {format}"));
    }
    let raw = zstd::decode_all(z).map_err(|e| e.to_string())?;
    serde_json::from_slice(&raw).map_err(|e| e.to_string())
}

/// Writes a `txs` row and returns its id.
///
/// # Errors
///
/// A failed statement.
pub fn insert_tx(conn: &Connection, meta: &TxMeta) -> Result<i64> {
    exec(
        conn,
        "INSERT INTO txs (kind, at, class, session, op_id, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            meta.kind.code(),
            sql_u64(meta.at.0)?,
            meta.class.as_ref().map(ClassName::as_str),
            meta.session.map(|s| sql_u64(s.0)).transpose()?,
            meta.op_id.as_ref().map(OpId::as_str),
            meta.detail,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Appends one event as `seq` of transaction `tx`, with its `event_nodes` rows.
///
/// # Errors
///
/// When the event does not encode, or a failed statement (a `seq` already taken among them).
pub fn insert_event(conn: &Connection, tx: i64, seq: u64, e: &Event) -> Result<()> {
    let z = encode_event(e)?;
    let seq = sql_u64(seq)?;
    exec(
        conn,
        "INSERT INTO events (seq, tx, at, format, z) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![seq, tx, sql_u64(e.at.0)?, EVENT_FORMAT, z],
    )?;
    let mut st =
        conn.prepare_cached("INSERT OR IGNORE INTO event_nodes (node, seq) VALUES (?1, ?2)")?;
    for n in nodes_named(e) {
        st.execute(params![sql_u64(n.0)?, seq])?;
    }
    Ok(())
}

/// The last `seq` the history holds, 0 when it holds none. Only the tests and the replay read it:
/// the writer numbers events from the state's event count.
///
/// # Errors
///
/// A failed query.
pub fn last_seq(conn: &Connection) -> Result<u64> {
    let v: Option<i64> = conn.query_row("SELECT max(seq) FROM events", [], |r| r.get(0))?;
    Ok(v.and_then(|x| u64::try_from(x).ok()).unwrap_or(0))
}

/// One stored event.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StoredEvent {
    /// Its `seq`.
    pub seq: u64,
    /// Its transaction.
    pub tx: i64,
    /// The event.
    pub event: Event,
}

/// The events with `seq > after`, at most `limit` of them, in `seq` order (`control export`
/// pages them so, A-78).
///
/// # Errors
///
/// A failed query, or a body that does not decode (named by its `seq`).
pub fn events_after(conn: &Connection, after: u64, limit: u32) -> Result<Vec<StoredEvent>> {
    let mut st = conn.prepare_cached(
        "SELECT seq, tx, format, z FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
    )?;
    let mut rows = st.query(params![sql_u64(after)?, limit])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let seq: i64 = r.get(0)?;
        let tx: i64 = r.get(1)?;
        let format: i64 = r.get(2)?;
        let z: Vec<u8> = r.get(3)?;
        let event = decode_event(format, &z).map_err(|msg| StoreError::Decode {
            table: "events",
            key: seq.to_string(),
            msg,
        })?;
        out.push(StoredEvent {
            seq: u64::try_from(seq).unwrap_or(0),
            tx,
            event,
        });
    }
    Ok(out)
}

/// One stored event by its `seq`.
///
/// # Errors
///
/// A failed query, or a body that does not decode.
pub fn event_at(conn: &Connection, seq: u64) -> Result<Option<Event>> {
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT format, z FROM events WHERE seq = ?1",
            [sql_u64(seq)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(format, z)| {
        decode_event(format, &z).map_err(|msg| StoreError::Decode {
            table: "events",
            key: seq.to_string(),
            msg,
        })
    })
    .transpose()
}

/// The `txs` rows with `id > after`, at most `limit`, in id order.
///
/// # Errors
///
/// A failed query, or an unknown kind.
pub fn txs_after(conn: &Connection, after: i64, limit: u32) -> Result<Vec<TxRow>> {
    let mut st = conn.prepare_cached(
        "SELECT id, kind, at, class, session, op_id, detail FROM txs WHERE id > ?1 ORDER BY id
         LIMIT ?2",
    )?;
    let mut rows = st.query(params![after, limit])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let bad = |msg: String| StoreError::Decode {
            table: "txs",
            key: id.to_string(),
            msg,
        };
        let code: i64 = r.get(1)?;
        let kind = TxKind::of_code(code).ok_or_else(|| bad(format!("unknown kind {code}")))?;
        let at: i64 = r.get(2)?;
        let class: Option<String> = r.get(3)?;
        let session: Option<i64> = r.get(4)?;
        let op_id: Option<String> = r.get(5)?;
        out.push(TxRow {
            id,
            meta: TxMeta {
                kind,
                at: Time(u64::try_from(at).map_err(|e| bad(e.to_string()))?),
                class: class
                    .map(|c| ClassName::new(&c).ok_or_else(|| bad(format!("class {c}"))))
                    .transpose()?,
                session: session
                    .map(|s| u64::try_from(s).map(SessionId))
                    .transpose()
                    .map_err(|e| bad(e.to_string()))?,
                op_id: op_id
                    .map(|o| OpId::new(&o).ok_or_else(|| bad(format!("op_id {o}"))))
                    .transpose()?,
                detail: r.get(6)?,
            },
        });
    }
    Ok(out)
}

/// The events of one transaction, in `seq` order.
///
/// # Errors
///
/// A failed query, or a body that does not decode.
pub fn events_of_tx(conn: &Connection, tx: i64) -> Result<Vec<StoredEvent>> {
    let mut st =
        conn.prepare_cached("SELECT seq, format, z FROM events WHERE tx = ?1 ORDER BY seq")?;
    let mut rows = st.query([tx])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let seq: i64 = r.get(0)?;
        let format: i64 = r.get(1)?;
        let z: Vec<u8> = r.get(2)?;
        let event = decode_event(format, &z).map_err(|msg| StoreError::Decode {
            table: "events",
            key: seq.to_string(),
            msg,
        })?;
        out.push(StoredEvent {
            seq: u64::try_from(seq).unwrap_or(0),
            tx,
            event,
        });
    }
    Ok(out)
}

/// A node's `event_nodes` page for `view history` (A-78): the `seq`s below `before` (all without
/// it), newest first, at most `limit`.
///
/// # Errors
///
/// A failed query.
pub fn node_history(
    conn: &Connection,
    node: NodeId,
    before: Option<u64>,
    limit: u32,
) -> Result<Vec<u64>> {
    let before = before.map_or(Ok(i64::MAX), sql_u64)?;
    let mut st = conn.prepare_cached(
        "SELECT seq FROM event_nodes WHERE node = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3",
    )?;
    let rows = st.query_map(params![sql_u64(node.0)?, before, limit], |r| {
        r.get::<_, i64>(0)
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(u64::try_from(r?).unwrap_or(0));
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------
// The nodes an event names

/// The nodes an event names (D.11.6): every node id in the event itself, an entry's body
/// included, with a hold token's or a deadline owner's node. An event about an entity (a request
/// answered, a decision superseded) names the entity, not its node, and so files under no node; an
/// `OpRecorded`'s stored outcome is a copy of the reply and names none.
#[must_use]
pub fn nodes_named(e: &Event) -> BTreeSet<NodeId> {
    let mut out = BTreeSet::new();
    let mut add = |n: NodeId| {
        out.insert(n);
    };
    match &e.ev {
        Ev::Genesis { .. }
        | Ev::ParamsChanged { .. }
        | Ev::SessionOpened { .. }
        | Ev::SessionHello { .. }
        | Ev::PrincipalSeen { .. }
        | Ev::Touched { .. }
        | Ev::SessionRetired { .. }
        | Ev::SessionStatus { .. }
        | Ev::InboxRead { .. }
        | Ev::NextCalled { .. }
        | Ev::RequestAnswered { .. }
        | Ev::RequestSettled { .. }
        | Ev::PqRecheckAt { .. }
        | Ev::PqRechecked { .. }
        | Ev::PqExpired { .. }
        | Ev::PqEscalated { .. }
        | Ev::PqAnswered { .. }
        | Ev::PqSuperseded { .. }
        | Ev::PqWithdrawn { .. }
        | Ev::PqAssumed { .. }
        | Ev::AqAnswered { .. }
        | Ev::UqAnswered { .. }
        | Ev::UqDefaulted { .. }
        | Ev::UqReaddressed { .. }
        | Ev::UqLapsed { .. }
        | Ev::UqAssumed { .. }
        | Ev::CommitmentRefreshed { .. }
        | Ev::CommitmentFulfilled { .. }
        | Ev::SaidConfirmed { .. }
        | Ev::SaidUnconfirmed { .. }
        | Ev::SaidWithheld { .. }
        | Ev::DecisionSuperseded { .. }
        | Ev::DecisionRetracted { .. }
        | Ev::ObjectionResolved { .. }
        | Ev::DigestShown { .. }
        | Ev::NoteBound { .. }
        | Ev::OpRecorded { .. } => {}
        Ev::Fired { owner, .. } => match owner {
            Owner::Hold(t) => add(t.node),
            Owner::Node(n) | Owner::Refusal(n, _) => add(*n),
            Owner::Entry(_) | Owner::Class(_) | Owner::Session(_) => {}
        },
        Ev::EntryRouted { routing, .. } => {
            if let Routing::Request { node, .. } = routing {
                add(*node);
            }
        }
        Ev::NodeUpdated { node, patch } => {
            add(*node);
            patch
                .depends_on
                .iter()
                .flatten()
                .copied()
                .for_each(&mut add);
            patch.owners.iter().flatten().copied().for_each(&mut add);
        }
        Ev::Shown { node, .. }
        | Ev::KeyAdded { node, .. }
        | Ev::KeyRemoved { node, .. }
        | Ev::NodeReopened { node }
        | Ev::OfferSet { node, .. }
        | Ev::IntentOpened { node, .. }
        | Ev::IntentClosed { node, .. }
        | Ev::OccurrenceAvailable { node }
        | Ev::OccurrenceDetached { node }
        | Ev::SignalDetached { node }
        | Ev::RefusalExpired { node, .. }
        | Ev::RefusalsCleared { node, .. }
        | Ev::Refused { node, .. } => add(*node),
        Ev::NodeCreated {
            id, parent, init, ..
        } => {
            add(*id);
            add(*parent);
            init.depends_on.iter().copied().for_each(&mut add);
            init.owners.iter().copied().for_each(&mut add);
            init.links.iter().for_each(|&(_, to)| add(to));
            role_nodes(&init.role, &mut add);
        }
        Ev::LinkAdded { node, to, .. } => {
            add(*node);
            add(*to);
        }
        Ev::NodeMoved { node, parent, .. } => {
            add(*node);
            add(*parent);
        }
        Ev::NodeClosed {
            node, merged_into, ..
        } => {
            add(*node);
            merged_into.iter().copied().for_each(&mut add);
        }
        Ev::WaitSet { node, wait } => {
            add(*node);
            if let Some(WaitOn::Node(m)) = wait {
                add(*m);
            }
        }
        Ev::DependsRedirected { node, from, to } => {
            add(*node);
            add(*from);
            to.iter().copied().for_each(&mut add);
        }
        Ev::MarkSet { node, .. } => add(*node),
        Ev::IntentMoved { from, to } => {
            add(*from);
            add(*to);
        }
        Ev::SeriesOutcome { series, .. } => add(*series),
        Ev::NodeCorrected { node, .. } | Ev::CorrectionWithdrawn { node, .. } => add(*node),
        Ev::CommitmentOpened { node, .. } => add(*node),
        Ev::HoldIssued { token, .. }
        | Ev::HoldExtended { token, .. }
        | Ev::HoldEnded { token, .. } => add(token.node),
        Ev::Appended { node, body, .. } => {
            add(*node);
            body_nodes(body, &mut add);
        }
        Ev::RequestMoved { to, .. }
        | Ev::PqMoved { to, .. }
        | Ev::CommitmentMoved { to, .. }
        | Ev::SaidMoved { to, .. }
        | Ev::ObjectionMoved { to, .. } => add(*to),
        Ev::SignalOpened { key, node } => {
            add(*node);
            add(key.a);
            add(key.b);
        }
        Ev::SignalResolved { key } => {
            add(key.a);
            add(key.b);
        }
        Ev::Paused { scope, .. } | Ev::Unpaused { scope } | Ev::PauseExpired { scope } => {
            add(*scope);
        }
        Ev::Hidden { target, .. } => {
            if let HideTarget::File { node, .. } = target {
                add(*node);
            }
        }
        Ev::Restored { items } => {
            for x in items {
                restored_nodes(x, &mut add);
            }
        }
    }
    out
}

fn role_nodes(r: &Role, add: &mut impl FnMut(NodeId)) {
    match r {
        Role::Plain | Role::Series(_) => {}
        Role::Occurrence(o) => add(o.series),
        Role::Signal(k) => {
            add(k.a);
            add(k.b);
        }
    }
}

fn addressee_nodes(a: &Addressee, add: &mut impl FnMut(NodeId)) {
    match a {
        Addressee::Holder(n) | Addressee::Holders(n) => add(*n),
        Addressee::Session(_) | Addressee::Principal(_) => {}
    }
}

fn body_nodes(b: &EntryBody, add: &mut impl FnMut(NodeId)) {
    match b {
        EntryBody::Note { to, .. } => {
            if let Some(a) = to {
                addressee_nodes(a, add);
            }
        }
        EntryBody::Question(q) => match q {
            QuestionBody::People { .. } => {}
            QuestionBody::Agent { to, .. } => addressee_nodes(to, add),
            QuestionBody::User { source, about, .. } => {
                if let UqSource::Refusal { node } = source {
                    add(*node);
                }
                about.iter().copied().for_each(add);
            }
        },
        EntryBody::Decision { basis, .. } => {
            for r in basis {
                if let BasisRef::Node { id, .. } = r {
                    add(*id);
                }
            }
        }
        EntryBody::Checkpoint { token, .. } | EntryBody::Late { token, .. } => add(token.node),
        EntryBody::IntentChecked { .. } => {}
        EntryBody::Retract { target } => {
            if let BasisTarget::Node(n) = target {
                add(*n);
            }
        }
        EntryBody::System(note) => match note {
            SystemNote::Digest { item, .. } => {
                if let DigestItem::RefusalDrop { node, .. } = item {
                    add(*node);
                }
            }
            SystemNote::KeysDropped { keys } => keys.iter().for_each(|(_, n)| add(*n)),
            SystemNote::MergedFrom { from, .. } => add(*from),
            SystemNote::Moved { from } => add(from.parent),
            SystemNote::Renewed { token } => add(token.node),
            SystemNote::Hidden(HideTarget::File { node, .. }) => add(*node),
            SystemNote::Closed { .. }
            | SystemNote::Reopened
            | SystemNote::ScheduleEnded
            | SystemNote::Paused { .. }
            | SystemNote::Unpaused
            | SystemNote::PauseExpired
            | SystemNote::Hidden(HideTarget::Entry(_))
            | SystemNote::Defaulted { .. } => {}
        },
        EntryBody::Request { .. }
        | EntryBody::Answer { .. }
        | EntryBody::Said { .. }
        | EntryBody::Heard { .. }
        | EntryBody::FromUser { .. }
        | EntryBody::ToUser { .. }
        | EntryBody::Approve { .. }
        | EntryBody::Object
        | EntryBody::Resolve { .. } => {}
    }
}

fn meta_node(m: &EntryMeta, add: &mut impl FnMut(NodeId)) {
    add(m.node);
}

fn restored_nodes(x: &Restored, add: &mut impl FnMut(NodeId)) {
    match x {
        Restored::Request(_, v, m) => {
            add(v.node);
            meta_node(m, add);
        }
        Restored::PQuestion(_, v, m) => {
            add(v.node);
            meta_node(m, add);
        }
        Restored::UQuestion(_, v, m) => {
            v.about.iter().copied().for_each(&mut *add);
            meta_node(m, add);
        }
        Restored::Commitment(_, v, m) => {
            add(v.node);
            meta_node(m, add);
        }
        Restored::Said(_, v, m) => {
            add(v.node);
            meta_node(m, add);
        }
        Restored::Decision(_, v, m) => {
            add(v.node);
            meta_node(m, add);
        }
        Restored::Objection(_, v, m) => {
            add(v.node);
            meta_node(m, add);
        }
        Restored::UserEntry(_, _, m) | Restored::Digest(_, _, m) | Restored::Meta(_, m) => {
            meta_node(m, add);
        }
    }
}
