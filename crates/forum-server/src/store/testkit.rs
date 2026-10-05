//! Test support for the store: temporary database files, a world that applies events through
//! `evolve_tracked` and writes each transaction through `store::write` as the writer would, and a
//! deterministic random event generator adapted from the core's own (`forum-core`'s
//! `evolve_random_tests.rs`, D.12.3), restricted to events that `evolve` accepts and that keep the
//! four constraints of §24, as every event of a real history does.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "test code"
)]

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use forum_core::{
    Addressee, Author, BasisRef, BasisTarget, ClassName, CommitmentState, ConfirmVia, DAY,
    DeadlineKind, DigestItem, DigestState, Dur, Eid, EndCause, EntityRef, EntryBody, EntryText,
    EntryType, Ev, Event, Extension, Fact, Fence, HOUR, HoldEnd, IntentEnd, Key, KeyStr, Label,
    LeaseTerms, LinkRel, MINUTE, Mark, Name, NextOutcome, NodeId, NodeInit, NodePatch, NormVer,
    OccPhase, Occurrence, OpId, Origin, Outcome, Owner, Params, PqState, PrincipalId, PrincipalKey,
    QuestionBody, ReqDigest, Requester, Restored, Role, Routing, SaidKind, Seq, SessionId,
    SignalKey, SignalKind, State, SystemNote, Text, Time, ToUserKind, Token, Tracked,
    USER_PRINCIPAL, UqSource, UqState, WaitOn, evolve_tracked,
};
use rusqlite::Connection;

use super::history::{TxKind, TxMeta};
use super::load::{Genesis, Opened, open};
use super::migrate::RELEASED;
use super::state::{put_op, value_json};
use super::write;

// ------------------------------------------------------------------------------------------------
// Temporary files

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// A database file in the system's temporary directory, removed with its WAL when dropped.
pub struct TempDb {
    pub path: PathBuf,
}

impl TempDb {
    pub fn new(tag: &str) -> TempDb {
        let n = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "forum-store-test-{}-{tag}-{n}.db",
            std::process::id()
        ));
        let t = TempDb { path };
        t.remove();
        t
    }

    fn remove(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut p = self.path.clone().into_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        self.remove();
    }
}

/// The start of every test history.
pub const START: Time = Time(1_790_000_000_000);

/// The parameters of the test histories: two sessions make a class refuse.
pub fn test_params() -> Params {
    Params {
        refusal_quorum: 2,
        ..Params::default()
    }
}

pub fn genesis_args(params: Params) -> Genesis {
    Genesis {
        params,
        now: START,
        masking_version: 1,
    }
}

/// Opens a fresh in-memory database with Genesis written.
pub fn fresh_memory(params: Params) -> Opened {
    super::load::open_connection(
        Connection::open_in_memory().unwrap(),
        &genesis_args(params),
        &RELEASED,
    )
    .unwrap()
}

/// Opens a fresh database file with Genesis written.
pub fn fresh_file(db: &TempDb, params: Params) -> Opened {
    open(&db.path, &genesis_args(params), &RELEASED).unwrap()
}

// ------------------------------------------------------------------------------------------------
// A world that writes

/// A forum under test whose every transaction is written to a database.
pub struct World {
    pub s: State,
    pub clock: Time,
    pub conn: Connection,
    /// The last value of every entity row that left memory and has not come back: its value just
    /// before the event that removed it.
    pub grave: BTreeMap<Fact, String>,
    /// Transactions written.
    pub txs: u32,
    /// Entities brought back by `restore_some`.
    pub restored: usize,
}

impl World {
    /// A world over a fresh in-memory database.
    pub fn memory(params: Params) -> World {
        let o = fresh_memory(params);
        World::from_opened(o)
    }

    pub fn from_opened(o: Opened) -> World {
        World {
            clock: o.state.last_now,
            s: o.state,
            conn: o.store.into_connection(),
            grave: BTreeMap::new(),
            txs: 1,
            restored: 0,
        }
    }

    pub fn at(&mut self, t: Time) {
        self.clock = self.clock.max(t);
    }

    /// Applies `evs` at the clock as one transaction of `kind` and writes it; panics when an
    /// event is not applied as given or a write fails. Returns the events.
    pub fn tx(&mut self, kind: TxKind, evs: Vec<Ev>) -> Vec<Event> {
        let events: Vec<Event> = evs
            .into_iter()
            .map(|ev| Event { at: self.clock, ev })
            .collect();
        self.tx_events(kind, events)
    }

    /// `tx` with the events' own times.
    pub fn tx_events(&mut self, kind: TxKind, events: Vec<Event>) -> Vec<Event> {
        let before = self.s.clone();
        let mut dirty = Tracked::new();
        for e in &events {
            let a = self.s.anomalies;
            evolve_tracked(&mut self.s, e, &mut dirty);
            assert_eq!(a, self.s.anomalies, "not applied as given: {e:?}");
        }
        self.write_applied(kind, &before, &events, &dirty);
        events
    }

    /// Writes events already applied to `self.s` (from `before`), with what they tracked. Every
    /// entity that left memory has its row written with its last value (D.12.6, review E1-spec-1).
    pub fn write_applied(
        &mut self,
        kind: TxKind,
        before: &State,
        events: &[Event],
        dirty: &Tracked,
    ) {
        let at = events.last().map_or(self.clock, |e| e.at);
        let tx = self.conn.transaction().unwrap();
        let wrote = write(&tx, &TxMeta::new(kind, at), events, &self.s, dirty)
            .unwrap_or_else(|e| panic!("write failed: {e} for {events:?}"));
        if let Some(w) = &wrote {
            assert!(
                w.facts.left_without_row.is_empty(),
                "entities left memory without a row: {:?} in {events:?}",
                w.facts.left_without_row
            );
        }
        for e in events {
            if let Ev::OpRecorded { class, op, .. } = &e.ev {
                put_op(
                    &tx,
                    &self.s,
                    class,
                    op,
                    1,
                    format!("response {}", op.as_str()).as_bytes(),
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
        if wrote.is_some() {
            self.txs += 1;
        }
        self.bury(before, events);
    }

    /// Records the last values of the entities that left memory, each the value just before the
    /// event that removed it, by folding `events` from `before` one at a time (a write may change
    /// an entity and then prune it, D.12.6); and forgets the restored ones.
    fn bury(&mut self, before: &State, events: &[Event]) {
        let mut s = before.clone();
        for e in events {
            let pre = s.clone();
            forum_core::evolve_mut(&mut s, e);
            for f in super::state::facts_of(&pre) {
                if super::state::leaves_memory(&f) && value_json(&s, &f).unwrap().is_none() {
                    self.grave
                        .insert(f.clone(), value_json(&pre, &f).unwrap().unwrap());
                }
            }
        }
        assert_eq!(
            super::replay::differences(&s, &self.s),
            Vec::<&str>::new(),
            "the events do not lead from the state before to the state written"
        );
        let back: Vec<Fact> = self
            .grave
            .keys()
            .filter(|f| value_json(&self.s, f).unwrap().is_some())
            .cloned()
            .collect();
        for f in back {
            self.grave.remove(&f);
        }
    }

    /// Restores up to `n` buried entities through their rows (`restore::items_of` and the core's
    /// `restore_tracked`), as a `restore` transaction. Returns how many came back.
    pub fn restore_some(&mut self, mut pick: impl FnMut(usize) -> bool) -> usize {
        let refs: Vec<EntityRef> = self
            .grave
            .keys()
            .enumerate()
            .filter(|(i, _)| pick(*i))
            .filter_map(|(_, f)| entity_of(f))
            .collect();
        if refs.is_empty() {
            return 0;
        }
        let items = super::restore::items_of(&self.conn, &self.s, &refs).unwrap();
        // Every item equals the last value the entity had before it left.
        for x in &items {
            let f = super::state::fact_of_entity(restored_ref(x));
            let want = self.grave.get(&f).cloned();
            let have = restored_value(x);
            assert_eq!(Some(have), want, "the restored row of {f:?}");
        }
        let before = self.s.clone();
        let mut dirty = Tracked::new();
        let a = self.s.anomalies;
        let evs = forum_core::restore_tracked(&mut self.s, self.clock, items, &mut dirty);
        assert_eq!(a, self.s.anomalies, "a restore was refused");
        if evs.is_empty() {
            return 0;
        }
        let n = match &evs[0].ev {
            Ev::Restored { items } => items.len(),
            _ => 0,
        };
        self.write_applied(TxKind::Restore, &before, &evs, &dirty);
        self.restored += n;
        n
    }

    /// Fires every deadline due at or before `t`, each at its own time and each with the event its
    /// arm would emit after it, as one `settle` transaction per firing (a stand-in for the core's
    /// `settle`, WP-D3). Returns false when a firing is refused (the state stays overdue).
    pub fn catch_up(&mut self, t: Time) -> bool {
        self.catch_up_in(t, 1)
    }

    /// `catch_up` with up to `batch` firings per `settle` transaction, as the engine writes a settle
    /// batch (D.8.1, D.8.2): each firing carries its own time, so one write may span a leaving time
    /// (FINISHED_RETAIN, DIGEST_RETAIN) and both change an entity and prune it. A stand-in for
    /// `settle_tracked`, whose Recheck, UserQuestion, and OccurrenceSlot arms are still stubs.
    pub fn catch_up_batched(&mut self, t: Time, batch: u32) -> bool {
        self.catch_up_in(t, batch.max(1))
    }

    fn catch_up_in(&mut self, t: Time, batch: u32) -> bool {
        let mut fired = 0_u32;
        while fired < 5_000 {
            let before = self.s.clone();
            let mut dirty = Tracked::new();
            let mut evs = Vec::new();
            let mut end = None;
            for _ in 0..batch {
                match self.next_firing(t) {
                    Firing::Settled => end = Some(true),
                    Firing::Refused => end = Some(false),
                    Firing::Due(k, firing) => {
                        for e in &firing {
                            evolve_tracked(&mut self.s, e, &mut dirty);
                        }
                        evs.extend(firing);
                        fired += 1;
                        if self.s.ix.deadlines.contains(&k) {
                            end = Some(false);
                        }
                    }
                }
                if end.is_some() {
                    break;
                }
            }
            if !evs.is_empty() {
                self.write_applied(TxKind::Settle, &before, &evs, &dirty);
            }
            if let Some(settled) = end {
                return settled;
            }
        }
        false
    }

    /// The next deadline due at or before `t` and the events of its firing: `Fired`, then the event
    /// its arm would emit, tried on a copy first, so that a firing whose transition is refused is
    /// not applied.
    fn next_firing(&self, t: Time) -> Firing {
        let Some(k) = self.s.ix.deadlines.first().cloned() else {
            return Firing::Settled;
        };
        if k.at > t {
            return Firing::Settled;
        }
        if k.at < self.s.last_now {
            return Firing::Refused;
        }
        let mut evs = vec![Event {
            at: k.at,
            ev: Ev::Fired {
                kind: k.kind,
                owner: k.owner.clone(),
            },
        }];
        let follow = match (&k.kind, &k.owner) {
            (DeadlineKind::Lease, Owner::Hold(tk)) => Some(Ev::HoldEnded {
                token: *tk,
                end: HoldEnd::Expired,
            }),
            (DeadlineKind::Pause, Owner::Node(n)) => Some(Ev::PauseExpired { scope: *n }),
            (DeadlineKind::RefusalExpiry, Owner::Refusal(n, c)) => Some(Ev::RefusalExpired {
                node: *n,
                class: c.clone(),
            }),
            (DeadlineKind::OccurrenceSlot, Owner::Node(n)) => {
                Some(Ev::OccurrenceAvailable { node: *n })
            }
            (DeadlineKind::Recheck, Owner::Entry(q)) => Some(Ev::PqExpired { q: *q }),
            (DeadlineKind::UserQuestion, Owner::Entry(q)) => {
                self.s.uquestions.get(q).cloned().map(|u| {
                    if u.has_default {
                        Ev::UqDefaulted { q: *q }
                    } else if usize::from(u.stage) + 1 < u.to.len() {
                        Ev::UqReaddressed {
                            q: *q,
                            stage: u.stage + 1,
                            due: k.at.plus(HOUR),
                        }
                    } else {
                        Ev::UqLapsed { q: *q }
                    }
                })
            }
            (DeadlineKind::Retire, Owner::Session(x)) => Some(Ev::SessionRetired { session: *x }),
            _ => None,
        };
        if let Some(ev) = follow {
            evs.push(Event { at: k.at, ev });
        }
        let mut probe = self.s.clone();
        let a = probe.anomalies;
        for e in &evs {
            forum_core::evolve_mut(&mut probe, e);
        }
        if probe.anomalies == a {
            Firing::Due(k, evs)
        } else {
            Firing::Refused
        }
    }
}

/// What `next_firing` found.
enum Firing {
    /// Nothing is due.
    Settled,
    /// The next firing would be refused, or a deadline lies before `last_now`.
    Refused,
    /// A key and the events of its firing.
    Due(forum_core::DeadlineKey, Vec<Event>),
}

fn entity_of(f: &Fact) -> Option<EntityRef> {
    match f {
        Fact::Request(i) => Some(EntityRef::Request(*i)),
        Fact::PQuestion(i) => Some(EntityRef::PQuestion(*i)),
        Fact::UQuestion(i) => Some(EntityRef::UQuestion(*i)),
        Fact::Commitment(i) => Some(EntityRef::Commitment(*i)),
        Fact::Said(i) => Some(EntityRef::Said(*i)),
        Fact::Decision(i) => Some(EntityRef::Decision(*i)),
        Fact::Objection(i) => Some(EntityRef::Objection(*i)),
        Fact::UserEntry(i) => Some(EntityRef::UserEntry(*i)),
        Fact::Digest(i) => Some(EntityRef::Digest(*i)),
        Fact::Entry(i) => Some(EntityRef::Meta(*i)),
        _ => None,
    }
}

fn restored_ref(x: &Restored) -> EntityRef {
    match x {
        Restored::Request(i, ..) => EntityRef::Request(*i),
        Restored::PQuestion(i, ..) => EntityRef::PQuestion(*i),
        Restored::UQuestion(i, ..) => EntityRef::UQuestion(*i),
        Restored::Commitment(i, ..) => EntityRef::Commitment(*i),
        Restored::Said(i, ..) => EntityRef::Said(*i),
        Restored::Decision(i, ..) => EntityRef::Decision(*i),
        Restored::Objection(i, ..) => EntityRef::Objection(*i),
        Restored::UserEntry(i, ..) => EntityRef::UserEntry(*i),
        Restored::Digest(i, ..) => EntityRef::Digest(*i),
        Restored::Meta(i, _) => EntityRef::Meta(*i),
    }
}

fn restored_value(x: &Restored) -> String {
    match x {
        Restored::Request(_, v, _) => serde_json::to_string(v),
        Restored::PQuestion(_, v, _) => serde_json::to_string(v),
        Restored::UQuestion(_, v, _) => serde_json::to_string(v),
        Restored::Commitment(_, v, _) => serde_json::to_string(v),
        Restored::Said(_, v, _) => serde_json::to_string(v),
        Restored::Decision(_, v, _) => serde_json::to_string(v),
        Restored::Objection(_, v, _) => serde_json::to_string(v),
        Restored::UserEntry(_, v, _) => serde_json::to_string(v),
        Restored::Digest(_, v, _) => serde_json::to_string(v),
        Restored::Meta(_, m) => serde_json::to_string(m),
    }
    .unwrap()
}

/// Whether the four constraints of §24 hold in `s`: holders within capacity, one open carrier per
/// key, unique names and aliases per parent, and unique `(class, op_id)` (a map key already).
pub fn constraints_hold(s: &State) -> bool {
    let mut per_node: BTreeMap<NodeId, u32> = BTreeMap::new();
    for t in s.holds.keys() {
        *per_node.entry(t.node).or_insert(0) += 1;
    }
    if per_node
        .iter()
        .any(|(n, c)| s.nodes.get(n).is_none_or(|x| *c > x.capacity))
    {
        return false;
    }
    let mut open: BTreeSet<&KeyStr> = BTreeSet::new();
    for n in s.nodes.values().filter(|n| n.is_open()) {
        for k in n.keys.keys() {
            if !open.insert(k) {
                return false;
            }
        }
    }
    s.names.keys().all(|k| !s.aliases.contains_key(k))
}

// ------------------------------------------------------------------------------------------------
// The random generator (adapted from forum-core's evolve_random_tests.rs)

/// xorshift64*: a small deterministic generator.
pub struct Rng(pub u64);

impl Rng {
    pub fn seeded(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    pub fn pick<T: Clone>(&mut self, v: &[T]) -> Option<T> {
        if v.is_empty() {
            None
        } else {
            Some(v[self.below(v.len() as u64) as usize].clone())
        }
    }
    pub fn subset<T: Clone + Ord>(&mut self, v: &[T]) -> BTreeSet<T> {
        v.iter().filter(|_| self.chance(30)).cloned().collect()
    }
}

const NAMES: [&str; 5] = ["a", "b", "c", "d", "e"];
const KEYS: [&str; 6] = [
    "chat:x/1",
    "chat:x/2",
    "issue:p#1",
    "lean:t1",
    "person:mail:a@x",
    "person:mail:b@x",
];
const PRINCIPALS: [&str; 3] = ["mail:a@x", "mail:b@x", "slack:t/u"];

pub fn label(s: &str) -> Label {
    Label::new(s).unwrap()
}

pub fn class(s: &str) -> ClassName {
    ClassName::new(s).unwrap()
}

pub fn keystr(s: &str) -> KeyStr {
    KeyStr::new(s).unwrap()
}

struct Pool {
    nodes: Vec<NodeId>,
    sessions: Vec<SessionId>,
    principals: Vec<PrincipalId>,
    holds: Vec<Token>,
    requests: Vec<Eid>,
    pqs: Vec<Eid>,
    aqs: Vec<Eid>,
    uqs: Vec<Eid>,
    commitments: Vec<Eid>,
    saids: Vec<Eid>,
    decisions: Vec<Eid>,
    objections: Vec<Eid>,
    user_entries: Vec<Eid>,
    notes: Vec<Eid>,
    entries: Vec<Eid>,
    signals: Vec<SignalKey>,
    pauses: Vec<NodeId>,
    refusals: Vec<(NodeId, ClassName)>,
    series: Vec<NodeId>,
    occurrences: Vec<NodeId>,
    signal_nodes: Vec<NodeId>,
    acks: Vec<Eid>,
    open_pqs: Vec<Eid>,
    pending_uqs: Vec<Eid>,
    to_users: Vec<Eid>,
    pending_digest: Vec<Eid>,
    open_commitments: Vec<Eid>,
    files: Vec<(NodeId, Text)>,
}

impl Pool {
    fn of(s: &State) -> Pool {
        let role_nodes = |f: fn(&Role) -> bool| -> Vec<NodeId> {
            s.nodes
                .iter()
                .filter(|(_, n)| f(&n.role))
                .map(|(&i, _)| i)
                .collect()
        };
        Pool {
            // A node and a session that never exist, as in the core's generator.
            nodes: s.nodes.keys().copied().chain([NodeId(999)]).collect(),
            sessions: s.sessions.keys().copied().chain([SessionId(999)]).collect(),
            principals: s.principal_ids.values().copied().collect(),
            holds: s.holds.keys().copied().collect(),
            requests: s.requests.keys().copied().collect(),
            pqs: s.pquestions.keys().copied().collect(),
            aqs: s.aquestions.keys().copied().collect(),
            uqs: s.uquestions.keys().copied().collect(),
            commitments: s.commitments.keys().copied().collect(),
            saids: s.saids.keys().copied().collect(),
            decisions: s.decisions.keys().copied().collect(),
            objections: s.objections.keys().copied().collect(),
            user_entries: s.user_entries.keys().copied().collect(),
            notes: s.notes.keys().copied().collect(),
            entries: s.entries.keys().copied().collect(),
            signals: s.signals.keys().copied().collect(),
            pauses: s.pauses.keys().copied().collect(),
            refusals: s.refusals.keys().cloned().collect(),
            series: role_nodes(|r| matches!(r, Role::Series(_))),
            occurrences: role_nodes(|r| matches!(r, Role::Occurrence(_))),
            signal_nodes: role_nodes(|r| matches!(r, Role::Signal(_))),
            acks: s
                .saids
                .iter()
                .filter(|(_, x)| x.kind == SaidKind::Ack)
                .map(|(&i, _)| i)
                .collect(),
            open_pqs: s
                .pquestions
                .iter()
                .filter(|(_, x)| x.state == PqState::Open)
                .map(|(&i, _)| i)
                .collect(),
            pending_uqs: s
                .uquestions
                .iter()
                .filter(|(_, x)| x.state == UqState::Pending)
                .map(|(&i, _)| i)
                .collect(),
            to_users: s
                .entries
                .iter()
                .filter(|(_, m)| m.ty == EntryType::ToUser)
                .map(|(&i, _)| i)
                .collect(),
            pending_digest: s
                .digest
                .iter()
                .filter(|(_, x)| x.state == DigestState::Pending)
                .map(|(&i, _)| i)
                .collect(),
            open_commitments: s
                .commitments
                .iter()
                .filter(|(_, x)| x.state == CommitmentState::Open)
                .map(|(&i, _)| i)
                .collect(),
            files: s
                .nodes
                .iter()
                .flat_map(|(&i, n)| n.files.keys().map(move |f| (i, f.clone())))
                .collect(),
        }
    }
}

fn eid_of(rng: &mut Rng, v: &[Eid]) -> Eid {
    rng.pick(v).unwrap_or(Eid(9_999))
}

thread_local! {
    /// Whether `later` sometimes draws a time weeks ahead (`Settling::Batched`), so that one settle
    /// batch can finish an entity and, past FINISHED_RETAIN or DIGEST_RETAIN, prune it.
    static FAR: Cell<bool> = const { Cell::new(false) };
}

fn later(rng: &mut Rng, now: Time) -> Time {
    let near = now.plus(Dur(rng.below(6) * 10 * MINUTE.0));
    if FAR.with(Cell::get) && rng.chance(30) {
        near.plus(Dur(DAY.0 * (7 + rng.below(30))))
    } else {
        near
    }
}

fn random_init(rng: &mut Rng, p: &Pool, now: Time) -> NodeInit {
    let labels = [label("x"), label("y")];
    let keys: Vec<KeyStr> = KEYS.iter().map(|k| keystr(k)).collect();
    let mut tags = BTreeSet::new();
    if rng.chance(40) {
        tags.insert(label("offer"));
    }
    let role = match rng.below(10) {
        0 => Role::Occurrence(Occurrence {
            series: rng.pick(&p.nodes).unwrap(),
            slot: later(rng, now),
            window_end: later(rng, now).plus(HOUR),
            phase: if rng.chance(50) {
                OccPhase::Waiting
            } else {
                OccPhase::Available
            },
            window_fired: false,
        }),
        1 => {
            let (a, b) = (rng.pick(&p.nodes).unwrap(), rng.pick(&p.nodes).unwrap());
            Role::Signal(SignalKey {
                kind: SignalKind::Duplicate,
                a: a.min(b),
                b: a.max(b),
            })
        }
        2 => Role::Series(forum_core::Series {
            repeat: forum_core::Repeat {
                cron_text: Text("0 7 * * *".into()),
                cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
                tz: forum_cron::TzTable {
                    name: String::from("UTC"),
                    tzdb_version: String::new(),
                    initial: 0,
                    transitions: Vec::new(),
                },
                run_within: HOUR,
            },
            last_missed: rng.chance(30),
        }),
        _ => Role::Plain,
    };
    let mut fields = BTreeMap::new();
    if rng.chance(30) {
        fields.insert(Text("f".into()), Text("{\"v\":1}".into()));
    }
    NodeInit {
        title: Text(format!("title {}", rng.below(100))),
        body: if rng.chance(30) {
            Text("a body ".repeat(1 + rng.below(40) as usize))
        } else {
            Text::default()
        },
        fields,
        status: Text::default(),
        digest: Text::default(),
        agreements: Text::default(),
        kind: if rng.chance(20) {
            Some(label("k"))
        } else {
            None
        },
        tags,
        capacity: rng.below(3) as u32,
        priority: rng.below(3) as i32 - 1,
        est: match rng.below(3) {
            0 => None,
            1 => Some(5),
            _ => Some(100),
        },
        needs: rng.subset(&labels),
        due: if rng.chance(30) {
            Some(later(rng, now))
        } else {
            None
        },
        depends_on: rng.subset(&p.nodes[..p.nodes.len() - 1]),
        owners: rng.subset(&p.nodes[..p.nodes.len() - 1]),
        keys: rng
            .subset(&keys)
            .into_iter()
            .map(|k| (k, NormVer(1)))
            .collect(),
        links: if rng.chance(30) {
            [(LinkRel::DerivedFrom, rng.pick(&p.nodes).unwrap())].into()
        } else {
            BTreeSet::new()
        },
        role,
    }
}

fn random_body(rng: &mut Rng, p: &Pool, s: &State, now: Time) -> EntryBody {
    let conv = keystr(rng.pick(&KEYS[..3]).unwrap());
    let sess = rng.pick(&p.sessions).unwrap();
    match rng.below(20) {
        0 | 1 => {
            let to = match rng.below(6) {
                0 => None,
                1 => Some(Addressee::Session(sess)),
                2 => Some(Addressee::Holder(rng.pick(&p.nodes).unwrap())),
                3 => Some(Addressee::Holders(rng.pick(&p.nodes).unwrap())),
                _ => Some(Addressee::Principal(
                    rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
                )),
            };
            EntryBody::Note {
                to,
                until: Some(later(rng, now).plus(MINUTE)),
                recipients: rng.subset(&p.sessions),
                pending_holder: rng.chance(30),
                expects: None,
                reply_to: None,
            }
        }
        2 | 3 => {
            let n = s.counters.next_eid.0;
            let user = rng.pick(&p.user_entries);
            let requester = match (user, rng.pick(&p.principals)) {
                (Some(_), Some(x)) if rng.chance(50) => Requester::User(x),
                _ => Requester::External,
            };
            EntryBody::Request {
                origin: Key {
                    key: keystr(&format!("chat:m/{n}")),
                    nv: NormVer(1),
                },
                conv,
                from_user: if matches!(requester, Requester::User(_)) {
                    user
                } else {
                    None
                },
                requester,
            }
        }
        4 => EntryBody::Question(QuestionBody::People {
            conv,
            blocking: rng.chance(50),
            said: eid_of(rng, &p.saids),
            supersedes: None,
            recheck_at: later(rng, now).plus(MINUTE),
        }),
        5 => EntryBody::Question(QuestionBody::Agent {
            to: Addressee::Session(sess),
            until: later(rng, now).plus(MINUTE),
            recipients: [sess].into(),
            pending_holder: false,
        }),
        6 => {
            let mut to: Vec<PrincipalId> = Vec::new();
            if let Some(x) = rng.pick(&p.principals)
                && x != USER_PRINCIPAL
            {
                to.push(x);
            }
            to.push(USER_PRINCIPAL);
            let source = match rng.below(3) {
                0 => UqSource::Agent {
                    session: sess,
                    digest: ReqDigest([0; 32]),
                },
                1 => UqSource::Escalation {
                    question: eid_of(rng, &p.pqs),
                },
                _ => {
                    let n = rng.pick(&p.nodes).unwrap();
                    if s.ix.refusal_question.contains_key(&n) {
                        UqSource::Agent {
                            session: sess,
                            digest: ReqDigest([0; 32]),
                        }
                    } else {
                        UqSource::Refusal { node: n }
                    }
                }
            };
            EntryBody::Question(QuestionBody::User {
                source,
                about: if rng.chance(70) {
                    rng.pick(&p.nodes)
                } else {
                    None
                },
                to,
                due: later(rng, now).plus(MINUTE),
                has_default: rng.chance(50),
            })
        }
        7 => EntryBody::Answer {
            question: eid_of(rng, &p.aqs),
            recipient: sess,
            until: later(rng, now).plus(MINUTE),
        },
        8 | 9 => {
            let mut basis = Vec::new();
            if rng.chance(60) {
                basis.push(BasisRef::Node {
                    id: rng.pick(&p.nodes).unwrap(),
                    version: None,
                    life_at: match rng.below(3) {
                        0 => None,
                        1 => Some(Outcome::Done),
                        _ => Some(Outcome::Failed),
                    },
                });
            }
            if rng.chance(40) {
                basis.push(BasisRef::Entry(eid_of(rng, &p.decisions)));
            }
            EntryBody::Decision {
                settles: rng.subset(&p.requests),
                supersedes: None,
                assumes: BTreeSet::new(),
                evidence: None,
                basis,
            }
        }
        10 => EntryBody::Checkpoint {
            token: rng.pick(&p.holds).unwrap_or(Token {
                node: NodeId(0),
                fence: Fence(0),
            }),
            release: None,
            pr_named: None,
            handed_off_until: None,
        },
        11 | 12 => EntryBody::Said {
            conv,
            kind: match rng.below(4) {
                0 => SaidKind::Ack,
                1 => SaidKind::Reply,
                2 => SaidKind::Result,
                _ => SaidKind::Correction,
            },
            corrects: None,
            // 1.11: the voice by its cap, stored only.
            voice: rng.chance(20),
        },
        13 => EntryBody::FromUser {
            principal: rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
        },
        14 => EntryBody::ToUser {
            kind: ToUserKind::Result,
            verbatim: true,
            re: BTreeSet::new(),
            principal: rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
        },
        15 => match rng.below(4) {
            0 => EntryBody::Object,
            1 => EntryBody::Approve {
                version: forum_core::Version(1),
            },
            2 => EntryBody::Heard {
                conv,
                answers: None,
            },
            _ => {
                let upheld = rng.chance(30);
                EntryBody::Resolve {
                    objection: eid_of(rng, &p.objections),
                    upheld,
                    // 1.11: a deferral, which marks an open objection.
                    deferred: !upheld && rng.chance(25),
                }
            }
        },
        16 => EntryBody::Retract {
            target: if rng.chance(50) {
                BasisTarget::Node(rng.pick(&p.nodes).unwrap())
            } else {
                BasisTarget::Entry(eid_of(rng, &p.decisions))
            },
        },
        17 => EntryBody::System(SystemNote::Paused {
            until: later(rng, now).plus(HOUR),
        }),
        18 => EntryBody::Note {
            to: None,
            until: None,
            recipients: BTreeSet::new(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        },
        _ => EntryBody::System(SystemNote::Digest {
            item: DigestItem::Lapsed {
                question: eid_of(rng, &p.uqs),
            },
            to: rng.pick(&p.principals).unwrap_or(USER_PRINCIPAL),
        }),
    }
}

/// One random event, mostly valid on `s` at `now`.
pub fn random_event(rng: &mut Rng, s: &State, now: Time) -> Ev {
    let p = Pool::of(s);
    let params = &s.params;
    let node = rng.pick(&p.nodes).unwrap();
    let sess = rng.pick(&p.sessions).unwrap_or(SessionId(999));
    let terms = LeaseTerms {
        first: params.lease_first,
        default: params.lease_default,
        max: params.lease_max,
        ceiling: params.lease_ceiling,
        renew_max: params.renew_max,
    };
    match rng.below(68) {
        0..=3 => Ev::SessionOpened {
            session: s.counters.next_session,
            class: class(if rng.chance(50) { "a" } else { "b" }),
            caps: rng.subset(&[label("x"), label("y")]),
            horizon: if rng.chance(50) { None } else { Some(60) },
            intent: if rng.chance(30) {
                Some(Text("an intent".into()))
            } else {
                None
            },
            silence_at: now.plus(params.presence_silence),
            retire_at: now.plus(params.session_retain),
            principal: rng.pick(&p.principals).filter(|_| rng.chance(30)),
        },
        4 => Ev::SessionHello {
            session: sess,
            caps: if rng.chance(50) {
                Some(rng.subset(&[label("x"), label("y")]))
            } else {
                None
            },
            horizon: if rng.chance(50) { Some(30) } else { None },
            intent: if rng.chance(50) {
                Some(Text("hello".into()))
            } else {
                None
            },
            principal: rng.pick(&p.principals).filter(|_| rng.chance(50)),
        },
        5 => Ev::PrincipalSeen {
            id: s.counters.next_principal,
            key: PrincipalKey::new(rng.pick(&PRINCIPALS).unwrap()).unwrap(),
        },
        6 => Ev::Touched {
            session: sess,
            silence_at: now.plus(params.presence_silence),
            retire_at: s
                .sessions
                .get(&sess)
                .map_or(now, |x| x.retire_at)
                .max(now.plus(params.session_retain)),
        },
        7 => Ev::InboxRead {
            session: sess,
            notes: rng.subset(&p.notes),
        },
        8 => Ev::Shown {
            session: sess,
            node,
            seq: s.nodes.get(&node).map_or(Seq(1), |n| n.seq),
        },
        9 => Ev::NextCalled {
            session: sess,
            outcome: NextOutcome::Nothing,
            open: rng.chance(60),
            class_window_end: now.plus(params.refusal_window),
        },
        10..=14 => {
            let n = if rng.chance(30) {
                String::from(rng.pick(&NAMES).unwrap())
            } else {
                format!("n{}", s.counters.next_node.0)
            };
            Ev::NodeCreated {
                id: s.counters.next_node,
                parent: if rng.chance(70) {
                    rng.pick(&p.nodes[..p.nodes.len() - 1]).unwrap()
                } else {
                    node
                },
                name: Name::new(&n).unwrap(),
                origin: match rng.below(4) {
                    0 => Origin::Ancestor(sess),
                    1 => Origin::Occurrence,
                    _ => Origin::Agent(sess),
                },
                init: random_init(rng, &p, now),
            }
        }
        15 | 16 => {
            let mut patch = NodePatch::default();
            match rng.below(11) {
                0 => patch.title = Some(Text(format!("t{}", rng.below(9)))),
                1 => patch.status = Some(Text(format!("s{}", rng.below(9)))),
                2 => patch.capacity = Some(rng.below(3) as u32),
                3 => patch.priority = Some(rng.below(4) as i32),
                4 => patch.needs = Some(rng.subset(&[label("x"), label("y")])),
                5 => patch.est = forum_core::Change::Set(rng.below(200) as u32),
                6 => patch.due = forum_core::Change::Set(later(rng, now)),
                7 => patch.depends_on = Some(rng.subset(&p.nodes)),
                8 => {
                    let mut files = BTreeMap::new();
                    files.insert(Text("spec.md".into()), Text("https://x/spec".into()));
                    patch.files = Some(files);
                }
                9 => patch.body = Some(Text("b".repeat(1 + rng.below(3_000) as usize))),
                _ => patch.tags = Some(rng.subset(&[label("offer"), label("x")])),
            }
            Ev::NodeUpdated { node, patch }
        }
        17 => Ev::KeyAdded {
            node,
            key: Key {
                key: keystr(rng.pick(&KEYS).unwrap()),
                nv: NormVer(1),
            },
        },
        18 => Ev::KeyRemoved {
            node,
            key: s
                .nodes
                .get(&node)
                .and_then(|n| n.keys.keys().next().cloned())
                .unwrap_or_else(|| keystr("chat:x/1")),
        },
        19 => Ev::LinkAdded {
            node,
            rel: LinkRel::Continues,
            to: rng.pick(&p.nodes).unwrap(),
        },
        20 | 21 => Ev::NodeMoved {
            node,
            parent: rng.pick(&p.nodes).unwrap(),
            name: Name::new(rng.pick(&NAMES).unwrap()).unwrap(),
        },
        22 | 23 => Ev::NodeClosed {
            node,
            outcome: match rng.below(4) {
                0 => Outcome::Done,
                1 => Outcome::Failed,
                2 => Outcome::Dropped,
                _ => Outcome::Superseded,
            },
            merged_into: if rng.chance(30) {
                let open: Vec<NodeId> = s
                    .nodes
                    .iter()
                    .filter(|(m, n)| **m != node && n.is_open())
                    .map(|(&m, _)| m)
                    .collect();
                rng.pick(&open)
            } else {
                None
            },
            // 1.11: the closing session, or the server.
            by: if rng.chance(70) {
                rng.pick(&p.sessions)
            } else {
                None
            },
        },
        24 => Ev::NodeReopened { node },
        25 => Ev::OfferSet {
            node,
            on: rng.chance(50),
        },
        26 | 27 => Ev::WaitSet {
            node,
            wait: match rng.below(6) {
                0 => None,
                1 => Some(WaitOn::Time(later(rng, now))),
                2 => Some(WaitOn::External(later(rng, now))),
                3 => Some(WaitOn::Node(rng.pick(&p.nodes).unwrap())),
                4 => Some(WaitOn::Question {
                    q: eid_of(rng, &p.pqs),
                    rechecks_at_set: 0,
                }),
                _ => Some(WaitOn::UserQuestion(eid_of(rng, &p.uqs))),
            },
        },
        28 => Ev::DependsRedirected {
            node,
            from: s
                .nodes
                .get(&node)
                .and_then(|n| n.depends_on.iter().next().copied())
                .unwrap_or(NodeId(0)),
            to: rng.pick(&p.nodes).filter(|_| rng.chance(50)),
        },
        29 => {
            let mut qs = p.pqs.clone();
            qs.extend(p.uqs.iter().copied());
            Ev::MarkSet {
                node,
                mark: if rng.chance(50) {
                    Mark::Recheck
                } else {
                    Mark::Assume
                },
                question: eid_of(rng, &qs),
                on: rng.chance(70),
            }
        }
        30 => Ev::IntentOpened {
            node,
            intent: forum_core::Intent {
                eid: eid_of(rng, &p.entries),
                session: sess,
                pr_named: false,
            },
        },
        31 => Ev::IntentClosed {
            node,
            end: IntentEnd::ResolvedByClose,
        },
        32 => Ev::IntentMoved {
            from: node,
            to: rng.pick(&p.nodes).unwrap(),
        },
        33 => match rng.below(4) {
            0 => Ev::SeriesOutcome {
                series: rng.pick(&p.series).unwrap_or(node),
                missed: rng.chance(50),
            },
            1 => Ev::OccurrenceAvailable {
                node: rng.pick(&p.occurrences).unwrap_or(node),
            },
            2 => Ev::OccurrenceDetached {
                node: rng.pick(&p.occurrences).unwrap_or(node),
            },
            _ => Ev::SignalDetached {
                node: rng.pick(&p.signal_nodes).unwrap_or(node),
            },
        },
        34..=36 => Ev::HoldIssued {
            token: Token {
                node,
                fence: Fence(s.nodes.get(&node).map_or(1, |n| n.fence.0 + 1)),
            },
            session: sess,
            lease_until: now.plus(params.lease_first),
            terms,
            l: params.lease_default,
            reopened: rng.chance(20).then_some(Outcome::Failed),
        },
        37 => {
            let t = rng.pick(&p.holds).unwrap_or(Token {
                node,
                fence: Fence(1),
            });
            Ev::HoldExtended {
                token: t,
                lease_until: s
                    .holds
                    .get(&t)
                    .map_or(now, |h| h.lease_until)
                    .max(now.plus(params.lease_default)),
                by: if rng.chance(50) {
                    Extension::Checkpoint
                } else {
                    Extension::Renew
                },
            }
        }
        38 => Ev::HoldEnded {
            token: rng.pick(&p.holds).unwrap_or(Token {
                node,
                fence: Fence(1),
            }),
            end: match rng.below(4) {
                0 => HoldEnd::Released,
                1 => HoldEnd::Expired,
                2 => HoldEnd::EndedByClose(EndCause::Move),
                _ => HoldEnd::EndedByClose(EndCause::Close),
            },
        },
        39..=46 => {
            let n = s.nodes.get(&node);
            Ev::Appended {
                eid: s.counters.next_eid,
                node,
                seq: Seq(n.map_or(1, |n| n.seq.0 + 1)),
                by: if rng.chance(90) {
                    Author::Session(sess)
                } else {
                    Author::Server
                },
                body: random_body(rng, &p, s, now),
                text: EntryText {
                    main: Text(format!("text {}", rng.below(1_000))),
                    extra: Vec::new(),
                },
            }
        }
        47 => {
            let r = eid_of(rng, &p.requests);
            let mut answers = p.saids.clone();
            answers.extend(p.entries.iter().copied());
            Ev::RequestAnswered {
                request: r,
                by: eid_of(rng, &answers),
            }
        }
        48 => {
            let r = eid_of(rng, &p.requests);
            let d =
                s.ix.settling
                    .get(&r)
                    .and_then(|x| x.iter().next().copied())
                    .unwrap_or(Eid(1));
            Ev::RequestSettled {
                request: r,
                decision: d,
                answer: s
                    .requests
                    .get(&r)
                    .and_then(|x| x.answered_by)
                    .unwrap_or(Eid(1)),
            }
        }
        49 => match rng.below(4) {
            0 => Ev::RequestMoved {
                request: eid_of(rng, &p.requests),
                to: node,
            },
            1 => Ev::PqMoved {
                q: eid_of(rng, &p.pqs),
                to: node,
            },
            2 => Ev::CommitmentMoved {
                c: eid_of(rng, &p.commitments),
                to: node,
            },
            _ => Ev::SaidMoved {
                said: eid_of(rng, &p.saids),
                to: node,
            },
        },
        50 | 51 => {
            let q = if rng.chance(70) {
                eid_of(rng, &p.open_pqs)
            } else {
                eid_of(rng, &p.pqs)
            };
            match rng.below(7) {
                0 => Ev::PqRecheckAt {
                    q,
                    at: later(rng, now).plus(MINUTE),
                },
                1 => Ev::PqRechecked {
                    q,
                    rechecks: s.pquestions.get(&q).map_or(1, |x| x.rechecks + 1),
                    next: later(rng, now).plus(MINUTE),
                },
                2 => Ev::PqExpired { q },
                3 => Ev::PqEscalated {
                    q,
                    uq: s
                        .uquestions
                        .iter()
                        .find(|(_, u)| u.source == UqSource::Escalation { question: q })
                        .map_or(Eid(9_999), |(&u, _)| u),
                },
                4 => Ev::PqAnswered {
                    q,
                    by: eid_of(rng, &p.entries),
                },
                5 => Ev::PqSuperseded {
                    q,
                    by: eid_of(rng, &p.pqs),
                },
                _ => Ev::PqAssumed {
                    q,
                    decision: eid_of(rng, &p.decisions),
                },
            }
        }
        52 | 53 => {
            let q = if rng.chance(70) {
                eid_of(rng, &p.pending_uqs)
            } else {
                eid_of(rng, &p.uqs)
            };
            match rng.below(6) {
                0 => Ev::UqAnswered {
                    q,
                    by: eid_of(rng, &p.user_entries),
                },
                1 => Ev::UqDefaulted { q },
                2 => Ev::UqReaddressed {
                    q,
                    stage: s.uquestions.get(&q).map_or(1, |x| x.stage + 1),
                    due: later(rng, now).plus(MINUTE),
                },
                3 => Ev::UqLapsed { q },
                4 => Ev::UqAssumed {
                    q,
                    decision: eid_of(rng, &p.decisions),
                },
                _ => Ev::AqAnswered {
                    q: eid_of(rng, &p.aqs),
                    by: eid_of(rng, &p.entries),
                },
            }
        }
        54 | 55 => {
            let c = if rng.chance(70) {
                eid_of(rng, &p.open_commitments)
            } else {
                eid_of(rng, &p.commitments)
            };
            match rng.below(4) {
                0 => {
                    let a = eid_of(rng, &p.acks);
                    let (n, conv) = s
                        .saids
                        .get(&a)
                        .map_or((node, keystr("chat:x/1")), |x| (x.node, x.conv.clone()));
                    Ev::CommitmentOpened {
                        c: a,
                        node: n,
                        conv,
                        followup_at: later(rng, now).plus(MINUTE),
                    }
                }
                1 => Ev::CommitmentRefreshed {
                    c,
                    followup_at: later(rng, now).plus(MINUTE),
                },
                _ => Ev::CommitmentFulfilled {
                    c,
                    by: eid_of(rng, &p.saids),
                },
            }
        }
        56 => {
            let a = eid_of(rng, &p.saids);
            if rng.chance(60) {
                Ev::SaidConfirmed {
                    said: a,
                    via: ConfirmVia::Explicit,
                    link: if rng.chance(50) {
                        Some(Text("https://chat/m/1".into()))
                    } else {
                        None
                    },
                }
            } else if rng.chance(50) {
                Ev::SaidUnconfirmed { said: a }
            } else {
                Ev::SaidWithheld { said: a }
            }
        }
        57 => {
            let entry = eid_of(rng, &p.user_entries);
            let own: Vec<Eid> = s
                .requests
                .iter()
                .filter(|(_, r)| r.from_user == Some(entry))
                .map(|(&r, _)| r)
                .collect();
            Ev::EntryRouted {
                entry,
                routing: match rng.below(4) {
                    0 => Routing::NoAction,
                    1 => Routing::Clarifies {
                        request: eid_of(rng, &p.requests),
                    },
                    2 => Routing::Answers {
                        question: eid_of(rng, &p.uqs),
                    },
                    _ => match rng.pick(&own) {
                        Some(request) => Routing::Request { node, request },
                        None => Routing::NoAction,
                    },
                },
                reason: None,
            }
        }
        58 => {
            let d = eid_of(rng, &p.decisions);
            if rng.chance(50) {
                Ev::DecisionSuperseded {
                    d,
                    by: eid_of(rng, &p.decisions),
                }
            } else {
                Ev::DecisionRetracted {
                    d,
                    by: eid_of(rng, &p.entries),
                    // 1.11: the decision's node, which keeps it; at times a node of a pre-1.11
                    // log (none), or a wrong one, which is an anomaly.
                    node: match rng.below(4) {
                        0 => None,
                        1 => rng.pick(&p.nodes),
                        _ => s.decisions.get(&d).map(|x| x.node),
                    },
                }
            }
        }
        59 => {
            let o = eid_of(rng, &p.objections);
            match rng.below(5) {
                0 => Ev::ObjectionResolved {
                    o,
                    by: eid_of(rng, &p.entries),
                    upheld: rng.chance(30),
                },
                1 => Ev::ObjectionMoved { o, to: node },
                2 => Ev::NodeCorrected {
                    node,
                    outcome: forum_core::Outcome::Failed,
                    by: eid_of(rng, &p.entries),
                    was: Some(forum_core::Outcome::Done),
                },
                3 => Ev::CorrectionWithdrawn {
                    node,
                    outcome: forum_core::Outcome::Done,
                },
                _ => Ev::DigestShown {
                    entry: eid_of(rng, &p.pending_digest),
                    by: eid_of(rng, &p.to_users),
                },
            }
        }
        60 => {
            let c = class(if rng.chance(50) { "a" } else { "b" });
            match rng.below(3) {
                0 => Ev::Refused {
                    node,
                    class: c,
                    session: sess,
                    backoff_until: now.plus(params.refusal_backoff),
                    expires_at: now.plus(params.refusal_ttl),
                    quorum: 1 + rng.below(2) as u32,
                },
                1 => {
                    let (n, c) = rng.pick(&p.refusals).unwrap_or((node, c));
                    Ev::RefusalExpired { node: n, class: c }
                }
                _ => Ev::RefusalsCleared {
                    node,
                    by: eid_of(rng, &p.entries),
                },
            }
        }
        61 => {
            let other = rng.pick(&p.nodes).unwrap();
            let key = SignalKey {
                kind: SignalKind::Conflict,
                a: node.min(other),
                b: node.max(other),
            };
            if rng.chance(60) {
                Ev::SignalOpened { key, node }
            } else {
                Ev::SignalResolved {
                    key: rng.pick(&p.signals).unwrap_or(key),
                }
            }
        }
        62 => match rng.below(3) {
            0 => Ev::Paused {
                scope: node,
                until: now.plus(params.pause_ttl),
                ttl: params.pause_ttl,
                by: sess,
                class: class("a"),
                renewed: false,
            },
            1 => Ev::Unpaused {
                scope: rng.pick(&p.pauses).unwrap_or(node),
            },
            _ => Ev::NoteBound {
                note: eid_of(rng, &p.notes),
                session: sess,
            },
        },
        63 => Ev::Hidden {
            target: match rng.pick(&p.files) {
                Some((node, name)) if rng.chance(40) => forum_core::HideTarget::File { node, name },
                _ => forum_core::HideTarget::Entry(eid_of(rng, &p.entries)),
            },
            reason: Text("a secret slipped in".into()),
        },
        64 => Ev::SessionStatus {
            session: sess,
            text: Text("working".into()),
        },
        _ => Ev::OpRecorded {
            class: class("a"),
            op: OpId::new(&format!("op-{}", rng.below(4))).unwrap(),
            digest: ReqDigest([7; 32]),
            session: sess,
            expires_at: now.plus(params.op_retain),
            outcome: Err(forum_core::Error {
                code: forum_core::ErrorCode::OpIdConflict,
                hint: forum_core::Hint::OpIdConflict,
            }),
        },
    }
}

/// Draws events until one that `evolve` accepts on `s` and that keeps the constraints of §24, or
/// gives up after `tries`.
pub fn valid_event(rng: &mut Rng, s: &State, now: Time, tries: u32) -> Option<Ev> {
    for _ in 0..tries {
        let ev = random_event(rng, s, now);
        let mut probe = s.clone();
        forum_core::evolve_mut(
            &mut probe,
            &Event {
                at: now,
                ev: ev.clone(),
            },
        );
        if probe.anomalies == s.anomalies && constraints_hold(&probe) {
            return Some(ev);
        }
    }
    None
}

/// How `random_history_with` settles a jump of the clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Settling {
    /// One `settle` transaction per firing (`World::catch_up`).
    PerFiring,
    /// Up to this many firings per `settle` transaction (`World::catch_up_batched`), with jumps
    /// and some deadlines weeks ahead, past FINISHED_RETAIN and DIGEST_RETAIN, so that one write
    /// may change an entity and then prune it.
    Batched(u32),
}

/// A history of `steps` random transactions written through the store: each transaction one to
/// three valid events at the clock, with time jumps (settled through `catch_up`, a day to a month
/// sometimes, so that finished entities leave memory) and restores through the rows. `check` runs
/// after every transaction.
pub fn random_history(seed: u64, steps: usize, check: impl FnMut(&World, usize)) -> World {
    random_history_with(seed, steps, Settling::PerFiring, check)
}

/// `random_history`, settling as `settling` says.
pub fn random_history_with(
    seed: u64,
    steps: usize,
    settling: Settling,
    mut check: impl FnMut(&World, usize),
) -> World {
    /// Puts `FAR` back when the history ends, a panic included.
    struct Near;
    impl Drop for Near {
        fn drop(&mut self) {
            FAR.with(|f| f.set(false));
        }
    }
    let _near = Near;
    FAR.with(|f| f.set(settling != Settling::PerFiring));
    let mut rng = Rng::seeded(seed);
    let mut w = World::memory(test_params());
    for step in 0..steps {
        if rng.chance(10) {
            let jump = match rng.below(20) {
                0 => DAY.0 * (1 + rng.below(30)),
                1 => HOUR.0 * (1 + rng.below(30)),
                2..=9 if settling != Settling::PerFiring => DAY.0 * (7 + rng.below(40)),
                _ => rng.below(4) * 15 * MINUTE.0,
            };
            let to = w.clock.plus(Dur(jump));
            let settled = match settling {
                Settling::PerFiring => w.catch_up(to),
                Settling::Batched(b) => w.catch_up_batched(to, b),
            };
            if settled {
                w.at(to);
            }
            // T1: the firings of a batch that stopped at a refused one moved `last_now` past the
            // clock; the server's calls are clamped to it (D.8.1), so the history's are too.
            w.at(w.s.last_now);
        }
        if !w.grave.is_empty() && rng.chance(6) {
            let mut r2 = Rng(rng.next() | 1);
            w.restore_some(|_| r2.chance(40));
        } else {
            let n = 1 + rng.below(3);
            let mut s = w.s.clone();
            let mut evs = Vec::new();
            for _ in 0..n {
                if let Some(ev) = valid_event(&mut rng, &s, w.clock, 40) {
                    forum_core::evolve_mut(
                        &mut s,
                        &Event {
                            at: w.clock,
                            ev: ev.clone(),
                        },
                    );
                    evs.push(ev);
                }
            }
            if !evs.is_empty() {
                w.tx(TxKind::Cmd, evs);
            }
        }
        check(&w, step);
    }
    w
}

// ------------------------------------------------------------------------------------------------
// Events as the commands emit them

/// A state and a clock that event helpers read ids and times from.
pub trait Now {
    fn st(&self) -> &State;
    fn now(&self) -> Time;
}

impl Now for World {
    fn st(&self) -> &State {
        &self.s
    }
    fn now(&self) -> Time {
        self.clock
    }
}

/// The events of one transaction being drafted: each is applied to a scratch state as it is
/// pushed, so that the next one reads the counters it left.
pub struct Draft {
    pub s: State,
    pub clock: Time,
    pub evs: Vec<Ev>,
}

impl Now for Draft {
    fn st(&self) -> &State {
        &self.s
    }
    fn now(&self) -> Time {
        self.clock
    }
}

impl Draft {
    /// Applies `ev` to the scratch state (it must be applied as given) and keeps it.
    pub fn push(&mut self, ev: Ev) {
        let a = self.s.anomalies;
        forum_core::evolve_mut(
            &mut self.s,
            &Event {
                at: self.clock,
                ev: ev.clone(),
            },
        );
        assert_eq!(a, self.s.anomalies, "not applied as given: {ev:?}");
        self.evs.push(ev);
    }
}

impl World {
    /// A draft of a transaction at the clock.
    pub fn draft(&self) -> Draft {
        Draft {
            s: self.s.clone(),
            clock: self.clock,
            evs: Vec::new(),
        }
    }

    /// Writes a drafted transaction.
    pub fn commit(&mut self, kind: TxKind, d: Draft) -> Vec<Event> {
        assert_eq!(d.clock, self.clock);
        self.tx(kind, d.evs)
    }

    /// Drafts and writes one transaction.
    pub fn run(&mut self, kind: TxKind, f: impl FnOnce(&mut Draft)) -> Vec<Event> {
        let mut d = self.draft();
        f(&mut d);
        self.commit(kind, d)
    }
}

pub fn session_opened(w: &impl Now, class_name: &str) -> Ev {
    let p = &w.st().params;
    Ev::SessionOpened {
        session: w.st().counters.next_session,
        class: class(class_name),
        caps: BTreeSet::new(),
        horizon: None,
        intent: Some(Text("work".into())),
        silence_at: w.now().plus(p.presence_silence),
        retire_at: w.now().plus(p.session_retain),
        principal: None,
    }
}

pub fn touched(w: &impl Now, sess: SessionId) -> Ev {
    let p = &w.st().params;
    Ev::Touched {
        session: sess,
        silence_at: w.now().plus(p.presence_silence),
        retire_at: w.st().sessions[&sess]
            .retire_at
            .max(w.now().plus(p.session_retain)),
    }
}

pub fn init(capacity: u32, keys: &[&str]) -> NodeInit {
    NodeInit {
        title: Text("a node".into()),
        body: Text("its body".into()),
        fields: BTreeMap::new(),
        status: Text::default(),
        digest: Text::default(),
        agreements: Text::default(),
        kind: None,
        tags: BTreeSet::new(),
        capacity,
        priority: 0,
        est: None,
        needs: BTreeSet::new(),
        due: None,
        depends_on: BTreeSet::new(),
        owners: BTreeSet::new(),
        keys: keys.iter().map(|k| (keystr(k), NormVer(1))).collect(),
        links: BTreeSet::new(),
        role: Role::Plain,
    }
}

pub fn created(w: &impl Now, parent: NodeId, name: &str, by: SessionId, init: NodeInit) -> Ev {
    Ev::NodeCreated {
        id: w.st().counters.next_node,
        parent,
        name: Name::new(name).unwrap(),
        origin: Origin::Agent(by),
        init,
    }
}

pub fn terms(p: &Params) -> LeaseTerms {
    LeaseTerms {
        first: p.lease_first,
        default: p.lease_default,
        max: p.lease_max,
        ceiling: p.lease_ceiling,
        renew_max: p.renew_max,
    }
}

/// The next hold on `node` for `by`.
pub fn issued(w: &impl Now, node: NodeId, by: SessionId) -> Ev {
    let p = &w.st().params;
    Ev::HoldIssued {
        token: Token {
            node,
            fence: Fence(w.st().nodes[&node].fence.0 + 1),
        },
        session: by,
        lease_until: w.now().plus(p.lease_first),
        terms: terms(p),
        l: p.lease_default,
        reopened: None,
    }
}

pub fn appended(w: &impl Now, node: NodeId, by: SessionId, body: EntryBody, text: &str) -> Ev {
    Ev::Appended {
        eid: w.st().counters.next_eid,
        node,
        seq: Seq(w.st().nodes[&node].seq.0 + 1),
        by: Author::Session(by),
        body,
        text: EntryText {
            main: Text(text.into()),
            extra: Vec::new(),
        },
    }
}

/// A world with one session and its id.
pub fn with_session() -> (World, SessionId) {
    let mut w = World::memory(test_params());
    let s1 = w.s.counters.next_session;
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    (w, s1)
}

/// Creates a node in its own transaction and returns it.
pub fn create_node(
    w: &mut World,
    parent: NodeId,
    name: &str,
    by: SessionId,
    i: NodeInit,
) -> NodeId {
    let id = w.s.counters.next_node;
    w.run(TxKind::Cmd, |d| {
        let ev = created(d, parent, name, by, i);
        d.push(ev);
    });
    id
}

/// A small forum built from the events its commands would emit, valid at every step: two
/// sessions, a held node with a key, a request answered by a confirmed reply and settled by a
/// decision, a checkpoint and a release, a child moved away (an alias), a pause and its expiry, a
/// note between the sessions, `op_id` records, and, eight days later, a third session after which
/// the settled request and its entry's metadata have left memory, and a restore that brings the
/// request back. Settled at its last event, it passes `check_invariants`.
pub fn scenario(w: &mut World) {
    let s1 = w.s.counters.next_session;
    let digest = ReqDigest([3; 32]);
    let done = |reply| {
        Ok(forum_core::Done {
            reply,
            warnings: Vec::new(),
            opened: None,
            stale_session: false,
            delivered: Vec::new(),
        })
    };
    // s-1: hello, with an op_id.
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
        let p = d.s.params.clone();
        d.push(Ev::OpRecorded {
            class: class("coder"),
            op: OpId::new("hello-1").unwrap(),
            digest,
            session: s1,
            expires_at: d.clock.plus(p.op_retain),
            outcome: done(forum_core::Reply::Status),
        });
    });
    // s-1 creates /work with its conversation key and takes it.
    w.at(w.clock.plus(MINUTE));
    let work = w.s.counters.next_node;
    w.run(TxKind::Cmd, |d| {
        let ev = touched(d, s1);
        d.push(ev);
        let ev = created(d, forum_core::ROOT, "work", s1, init(1, &["chat:team/1"]));
        d.push(ev);
        let ev = issued(d, work, s1);
        d.push(ev);
    });
    let token = Token {
        node: work,
        fence: Fence(1),
    };
    // Bob's request in the conversation.
    w.at(w.clock.plus(MINUTE));
    let request = w.s.counters.next_eid;
    w.run(TxKind::Cmd, |d| {
        let ev = appended(
            d,
            work,
            s1,
            EntryBody::Request {
                origin: Key {
                    key: keystr("chat:team/1/m1"),
                    nv: NormVer(1),
                },
                conv: keystr("chat:team/1"),
                from_user: None,
                requester: Requester::External,
            },
            "please audit the parser",
        );
        d.push(ev);
    });
    // A checkpoint.
    w.at(w.clock.plus(Dur(3 * MINUTE.0)));
    w.run(TxKind::Cmd, |d| {
        let ev = touched(d, s1);
        d.push(ev);
        let ev = appended(
            d,
            work,
            s1,
            EntryBody::Checkpoint {
                token,
                release: None,
                pr_named: None,
                handed_off_until: None,
            },
            "read the parser",
        );
        d.push(ev);
        let l = d.s.params.lease_default;
        d.push(Ev::HoldExtended {
            token,
            lease_until: d.clock.plus(l),
            by: forum_core::Extension::Checkpoint,
        });
    });
    // The reply to Bob answers his request, a decision settles it, and the reply is confirmed.
    w.at(w.clock.plus(MINUTE));
    let reply = w.s.counters.next_eid;
    w.run(TxKind::Cmd, |d| {
        let ev = appended(
            d,
            work,
            s1,
            EntryBody::Said {
                conv: keystr("chat:team/1"),
                kind: SaidKind::Reply,
                corrects: None,
                voice: false,
            },
            "audited: two bugs, both fixed",
        );
        d.push(ev);
        d.push(Ev::RequestAnswered { request, by: reply });
    });
    w.at(w.clock.plus(MINUTE));
    let decision = w.s.counters.next_eid;
    w.run(TxKind::Cmd, |d| {
        let ev = appended(
            d,
            work,
            s1,
            EntryBody::Decision {
                settles: [request].into(),
                supersedes: None,
                assumes: BTreeSet::new(),
                evidence: None,
                basis: Vec::new(),
            },
            "the audit is done",
        );
        d.push(ev);
        d.push(Ev::RequestSettled {
            request,
            decision,
            answer: reply,
        });
        let p = d.s.params.clone();
        d.push(Ev::OpRecorded {
            class: class("coder"),
            op: OpId::new("decide-1").unwrap(),
            digest,
            session: s1,
            expires_at: d.clock.plus(p.op_retain),
            outcome: done(forum_core::Reply::Status),
        });
    });
    w.at(w.clock.plus(MINUTE));
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::SaidConfirmed {
            said: reply,
            via: forum_core::ConfirmVia::Explicit,
            link: Some(Text("https://chat.example/team/1/m2".into())),
        });
    });
    // s-1 releases /work.
    w.at(w.clock.plus(MINUTE));
    w.run(TxKind::Cmd, |d| {
        let ev = appended(
            d,
            work,
            s1,
            EntryBody::Checkpoint {
                token,
                release: Some(forum_core::ReleaseReason::Handoff),
                pr_named: None,
                handed_off_until: None,
            },
            "handing off",
        );
        d.push(ev);
        d.push(Ev::HoldEnded {
            token,
            end: HoldEnd::Released,
        });
    });
    // s-2 creates /work/sub, moves it to /moved (an alias), pauses /work, and gets a note.
    w.at(w.clock.plus(MINUTE));
    let s2 = w.s.counters.next_session;
    let sub = w.s.counters.next_node;
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "reviewer");
        d.push(ev);
        let ev = created(d, work, "sub", s2, init(2, &["issue:proj#7"]));
        d.push(ev);
    });
    w.at(w.clock.plus(MINUTE));
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::NodeMoved {
            node: sub,
            parent: forum_core::ROOT,
            name: Name::new("moved").unwrap(),
        });
        let ev = appended(
            d,
            sub,
            s2,
            EntryBody::System(SystemNote::Moved {
                from: forum_core::Place {
                    parent: work,
                    name: Name::new("sub").unwrap(),
                },
            }),
            "",
        );
        d.push(ev);
    });
    w.at(w.clock.plus(MINUTE));
    w.run(TxKind::Cmd, |d| {
        let p = d.s.params.clone();
        let until = d.clock.plus(p.pause_ttl);
        d.push(Ev::Paused {
            scope: work,
            until,
            ttl: p.pause_ttl,
            by: s2,
            class: class("reviewer"),
            renewed: false,
        });
        let ev = appended(
            d,
            work,
            s2,
            EntryBody::System(SystemNote::Paused { until }),
            "hold on while the release is cut",
        );
        d.push(ev);
    });
    w.at(w.clock.plus(MINUTE));
    w.run(TxKind::Cmd, |d| {
        let ev = touched(d, s1);
        d.push(ev);
        let p = d.s.params.clone();
        let ev = appended(
            d,
            work,
            s1,
            EntryBody::Note {
                to: Some(forum_core::Addressee::Session(s2)),
                until: Some(d.clock.plus(p.deliver_window)),
                recipients: [s2].into(),
                pending_holder: false,
                expects: None,
                reply_to: None,
            },
            "the parser is yours",
        );
        d.push(ev);
    });
    // Eight days on: the sessions fall silent and retire, the pause expires, the note and the
    // op_id records are dropped, and the settled request leaves memory at the next event.
    let later = w.clock.plus(Dur(8 * DAY.0));
    assert!(w.catch_up(later), "the scenario's deadlines fire");
    w.at(later);
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    assert!(
        !w.s.requests.contains_key(&request),
        "the settled request left memory"
    );
    // A command that names the request restores it first; the rest stays in its rows.
    let at = w
        .grave
        .keys()
        .position(|f| *f == Fact::Request(request))
        .expect("the request is buried");
    assert!(w.grave.len() > 1, "more than the request left memory");
    let n = w.restore_some(|i| i == at);
    assert!(n > 0, "the request came back");
    assert!(w.s.requests.contains_key(&request));
    let _ = sub;
}

// ------------------------------------------------------------------------------------------------
// Fixtures

/// The whole database as SQL: the tables, their rows, then the indexes and triggers, so that
/// loading it fires no trigger. What a fixture of a released version is frozen as (D.12.6).
pub fn dump_sql(conn: &Connection) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "-- A fixture database (design D.12.6), written by the ignored test write_the_v1_fixture.\n\
         -- Generated; do not edit.\n",
    );
    let objects: Vec<(String, String, Option<String>)> = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY rowid",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (ty, name, sql) in &objects {
        if ty != "table" {
            continue;
        }
        let Some(sql) = sql else { continue };
        writeln!(out, "{sql};").unwrap();
        let mut st = conn.prepare(&format!("SELECT * FROM {name}")).unwrap();
        let n = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let mut vals = Vec::with_capacity(n);
            for i in 0..n {
                let v: rusqlite::types::Value = r.get(i).unwrap();
                vals.push(match v {
                    rusqlite::types::Value::Null => "NULL".to_owned(),
                    rusqlite::types::Value::Integer(x) => x.to_string(),
                    rusqlite::types::Value::Real(_) => panic!("no floating point in the store"),
                    rusqlite::types::Value::Text(t) => format!("'{}'", t.replace('\'', "''")),
                    rusqlite::types::Value::Blob(b) => {
                        let mut s = String::from("X'");
                        for byte in b {
                            write!(s, "{byte:02X}").unwrap();
                        }
                        s.push('\'');
                        s
                    }
                });
            }
            writeln!(out, "INSERT INTO {name} VALUES ({});", vals.join(", ")).unwrap();
        }
    }
    for (ty, _, sql) in &objects {
        if ty == "table" {
            continue;
        }
        if let Some(sql) = sql {
            writeln!(out, "{sql};").unwrap();
        }
    }
    out
}
