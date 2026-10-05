//! The state tables (design D.11.6, D.6.1, D.7.6, FQ-2): one row per fact of the core's base and
//! history parts, and the two-pass writing of a job's dirty facts.
//!
//! A fact's row is its key, the canonical JSON of the map key (a tuple as a JSON array), and its
//! value, the canonical JSON of the Rust value. Most facts live in `st_facts`, keyed by the `Fact`
//! variant's index (the `fact` column) and the key; the facts that carry a constraint of §24, and the
//! node rows, have tables of their own with the constraint's columns:
//!
//! | Fact | Table | Columns beside the value |
//! |---|---|---|
//! | `Scalars` | `st_scalars` | — |
//! | `NodeContent(n)` | `st_nodes` | `capacity` (the capacity triggers) |
//! | `NodeHot(n)` | `st_node_hot` | — |
//! | `Key(n, k)` | `st_keys` | `open`: the node is open (K1's partial unique index) |
//! | `Name`, `Alias` | `st_names` | `alias` (K3: one primary key for both) |
//! | `Hold(t)` | `st_holds` | — (holders ≤ capacity) |
//! | `Op(c, o)` | `st_ops` | `masking_version`, `response` (O1) |
//! | every other fact | `st_facts` | `resident`: 0 once it left memory (D.4.12) |
//!
//! A node is two rows here (D.4.3): the hot row holds the counters, life, wait, intent, marks, tags,
//! `due_fired`, and the role's state; the content row holds the rest, with the hot fields at their
//! defaults and no keys or links, which are rows of their own. The split mirrors
//! `forum_core::facts`, whose `changed_facts` oracle the core's tests hold `evolve_tracked`'s dirty
//! set to, so a hot change never rewrites the content row.

use std::collections::{BTreeMap, BTreeSet};

use forum_core::{
    Counters, Departed, Eid, EntityRef, ExpiredHold, Fact, Fence, Intent, Label, Life, Node,
    NodeId, OccPhase, OpRecord, Params, QuestionId, Role, Seq, State, Time, Version, Wait,
};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Result, StoreError, exec};

/// The variant names of `Fact`, in declaration order: index `i` is the `fact` column of the
/// variant whose `kind` is `i` (D.11.6). Variants are only appended (D.7.6).
pub const FACT_NAMES: [&str; 37] = [
    "Scalars",
    "NodeHot",
    "NodeContent",
    "Key",
    "Link",
    "Name",
    "Alias",
    "Hold",
    "Entry",
    "Request",
    "PQuestion",
    "AQuestion",
    "UQuestion",
    "Commitment",
    "Said",
    "Decision",
    "Objection",
    "UserEntry",
    "Digest",
    "Note",
    "Approval",
    "Retracted",
    "Refusal",
    "ClassProfile",
    "Signal",
    "Pause",
    "Session",
    "Principal",
    "Op",
    "Day",
    "Start",
    "Profile",
    "DemandCounted",
    "Since",
    "UserMsgs",
    "Leaving",
    "DigestLeft",
];

/// The index of a fact's variant: its `fact` column. Exhaustive, so a new variant cannot be
/// stored before it has a column value.
#[must_use]
pub fn kind(f: &Fact) -> u8 {
    match f {
        Fact::Scalars => 0,
        Fact::NodeHot(_) => 1,
        Fact::NodeContent(_) => 2,
        Fact::Key(..) => 3,
        Fact::Link(..) => 4,
        Fact::Name(..) => 5,
        Fact::Alias(..) => 6,
        Fact::Hold(_) => 7,
        Fact::Entry(_) => 8,
        Fact::Request(_) => 9,
        Fact::PQuestion(_) => 10,
        Fact::AQuestion(_) => 11,
        Fact::UQuestion(_) => 12,
        Fact::Commitment(_) => 13,
        Fact::Said(_) => 14,
        Fact::Decision(_) => 15,
        Fact::Objection(_) => 16,
        Fact::UserEntry(_) => 17,
        Fact::Digest(_) => 18,
        Fact::Note(_) => 19,
        Fact::Approval(..) => 20,
        Fact::Retracted(_) => 21,
        Fact::Refusal(..) => 22,
        Fact::ClassProfile(..) => 23,
        Fact::Signal(_) => 24,
        Fact::Pause(_) => 25,
        Fact::Session(_) => 26,
        Fact::Principal(_) => 27,
        Fact::Op(..) => 28,
        Fact::Day(_) => 29,
        Fact::Start(_) => 30,
        Fact::Profile(..) => 31,
        Fact::DemandCounted(_) => 32,
        Fact::Since(_) => 33,
        Fact::UserMsgs(_) => 34,
        Fact::Leaving(_) => 35,
        Fact::DigestLeft(_) => 36,
    }
}

const K_SCALARS: u8 = 0;
const K_NODE_HOT: u8 = 1;
const K_NODE_CONTENT: u8 = 2;
const K_KEY: u8 = 3;
const K_NAME: u8 = 5;
const K_ALIAS: u8 = 6;
const K_HOLD: u8 = 7;
const K_OP: u8 = 28;

/// Which table holds a fact's row.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Table {
    /// `st_scalars`.
    Scalars,
    /// `st_nodes`.
    Nodes,
    /// `st_node_hot`.
    NodeHot,
    /// `st_names`.
    Names,
    /// `st_holds`.
    Holds,
    /// `st_facts`.
    Facts,
    /// `st_ops`.
    Ops,
    /// `st_keys`.
    Keys,
}

impl Table {
    /// The SQL name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Table::Scalars => "st_scalars",
            Table::Nodes => "st_nodes",
            Table::NodeHot => "st_node_hot",
            Table::Names => "st_names",
            Table::Holds => "st_holds",
            Table::Facts => "st_facts",
            Table::Ops => "st_ops",
            Table::Keys => "st_keys",
        }
    }
}

/// The table of a fact kind.
#[must_use]
pub fn table_of_kind(k: u8) -> Table {
    match k {
        K_SCALARS => Table::Scalars,
        K_NODE_CONTENT => Table::Nodes,
        K_NODE_HOT => Table::NodeHot,
        K_KEY => Table::Keys,
        K_NAME | K_ALIAS => Table::Names,
        K_HOLD => Table::Holds,
        K_OP => Table::Ops,
        _ => Table::Facts,
    }
}

/// Whether a fact's absence from the state means that it left memory (D.4.12), so that its row is
/// marked not resident instead of deleted: the entities of `EntityRef` and the entry metadata.
/// `evolve` removes them only in the memory prune.
#[must_use]
pub fn leaves_memory(f: &Fact) -> bool {
    matches!(
        f,
        Fact::Entry(_)
            | Fact::Request(_)
            | Fact::PQuestion(_)
            | Fact::UQuestion(_)
            | Fact::Commitment(_)
            | Fact::Said(_)
            | Fact::Decision(_)
            | Fact::Objection(_)
            | Fact::UserEntry(_)
            | Fact::Digest(_)
    )
}

/// The facts that hold an entity of `EntityRef`, keyed the same way.
#[must_use]
pub fn fact_of_entity(x: EntityRef) -> Fact {
    match x {
        EntityRef::Request(i) => Fact::Request(i),
        EntityRef::PQuestion(i) => Fact::PQuestion(i),
        EntityRef::UQuestion(i) => Fact::UQuestion(i),
        EntityRef::Commitment(i) => Fact::Commitment(i),
        EntityRef::Said(i) => Fact::Said(i),
        EntityRef::Decision(i) => Fact::Decision(i),
        EntityRef::Objection(i) => Fact::Objection(i),
        EntityRef::UserEntry(i) => Fact::UserEntry(i),
        EntityRef::Digest(i) => Fact::Digest(i),
        EntityRef::Meta(i) => Fact::Entry(i),
    }
}

// ------------------------------------------------------------------------------------------------
// Keys

/// The canonical JSON of a fact's map key (D.7.6): the variant's payload, a tuple as an array, a
/// struct with its fields in declaration order; `null` for `Scalars`.
///
/// The payload is cut out of the fact's own serialisation, `{"<variant>":<payload>}`, rather than
/// passed through `serde_json::Value`: a `Value` object sorts its fields unless serde_json's
/// `preserve_order` feature is on, which any crate of the build may switch on, and the stored `k`
/// must not depend on a Cargo feature.
///
/// # Errors
///
/// A key that does not serialise (none does).
pub fn key_json(f: &Fact) -> Result<String> {
    if matches!(f, Fact::Scalars) {
        return Ok("null".to_owned());
    }
    let whole = serde_json::to_string(f)?;
    let name = FACT_NAMES
        .get(usize::from(kind(f)))
        .ok_or_else(|| StoreError::Invalid(format!("no name for {f:?}")))?;
    whole
        .strip_prefix(&format!("{{\"{name}\":"))
        .and_then(|rest| rest.strip_suffix('}'))
        .map(str::to_owned)
        .ok_or_else(|| StoreError::Invalid(format!("a fact serialised as {whole}")))
}

/// The fact of a `(fact, key)` pair, the inverse of `kind` and `key_json`.
///
/// # Errors
///
/// An unknown kind, or a key that does not decode into that variant's payload.
pub fn fact_of(kind: u8, key: &str) -> std::result::Result<Fact, String> {
    let name = FACT_NAMES
        .get(usize::from(kind))
        .ok_or_else(|| format!("unknown fact kind {kind}"))?;
    if kind == K_SCALARS {
        return Ok(Fact::Scalars);
    }
    let payload: Value = serde_json::from_str(key).map_err(|e| e.to_string())?;
    let mut m = serde_json::Map::new();
    m.insert((*name).to_owned(), payload);
    serde_json::from_value(Value::Object(m)).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------------------------------------
// Values

/// The scalar row (D.11.6): `params`, `last_now`, `counters`, `anomalies`, and
/// `stats.done_minutes`.
#[derive(Serialize, Deserialize)]
struct ScalarsRow {
    params: Params,
    last_now: Time,
    counters: Counters,
    anomalies: u64,
    done_minutes: Vec<u64>,
}

/// A node's hot row (D.4.3): the fields `forum_core::facts` counts as hot.
#[derive(Serialize, Deserialize)]
struct HotRow {
    version: Version,
    digest_version: Version,
    content_version: Version,
    fence: Fence,
    seq: Seq,
    attempts: u32,
    checkpoints: u32,
    held_ms: u64,
    people_left: Seq,
    left: u32,
    last_checkpoint: Option<Eid>,
    expired_hold: Option<ExpiredHold>,
    life: Life,
    wait: Option<Wait>,
    open_intent: Option<Intent>,
    recheck_marks: BTreeSet<QuestionId>,
    assume_marks: BTreeSet<QuestionId>,
    tags: BTreeSet<Label>,
    due_fired: bool,
    role: RoleHot,
    /// 1.8: appended with a default, so that a row written before reads as a node never touched,
    /// which no deadline makes stale until it is (D.7.6).
    #[serde(default)]
    touched_at: Option<Time>,
    #[serde(default)]
    stale_fired: bool,
    /// 1.11: appended with a default, so that a row written before reads as a node none of whose
    /// decisions was retracted since 1.11 (D.7.6).
    #[serde(default)]
    retracted_decision: Option<Eid>,
    /// 1.12: appended with a default, so that a row written before reads as a node whose last
    /// `intent_done` named no pull request (§20.9 rule 8).
    #[serde(default)]
    pr_named: bool,
}

/// The state part of a node's role.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
enum RoleHot {
    None,
    Series { last_missed: bool },
    Occurrence { phase: OccPhase, window_fired: bool },
}

fn role_hot(r: &Role) -> RoleHot {
    match r {
        Role::Plain | Role::Signal(_) => RoleHot::None,
        Role::Series(x) => RoleHot::Series {
            last_missed: x.last_missed,
        },
        Role::Occurrence(o) => RoleHot::Occurrence {
            phase: o.phase,
            window_fired: o.window_fired,
        },
    }
}

fn hot_row(n: &Node) -> HotRow {
    HotRow {
        version: n.version,
        digest_version: n.digest_version,
        content_version: n.content_version,
        fence: n.fence,
        seq: n.seq,
        attempts: n.attempts,
        checkpoints: n.checkpoints,
        held_ms: n.held_ms,
        people_left: n.people_left,
        left: n.left,
        last_checkpoint: n.last_checkpoint,
        expired_hold: n.expired_hold,
        life: n.life.clone(),
        wait: n.wait.clone(),
        open_intent: n.open_intent,
        recheck_marks: n.recheck_marks.clone(),
        assume_marks: n.assume_marks.clone(),
        tags: n.tags.clone(),
        due_fired: n.due_fired,
        role: role_hot(&n.role),
        touched_at: n.touched_at,
        stale_fired: n.stale_fired,
        retracted_decision: n.retracted_decision,
        pr_named: n.pr_named,
    }
}

/// A node with its hot fields at their defaults and no keys or links: its content row.
fn content_row(n: &Node) -> Node {
    let mut c = n.clone();
    c.version = Version::default();
    c.digest_version = Version::default();
    c.content_version = Version::default();
    c.fence = Fence::default();
    c.seq = Seq::default();
    c.attempts = 0;
    c.checkpoints = 0;
    c.held_ms = 0;
    c.people_left = Seq::default();
    c.left = 0;
    c.last_checkpoint = None;
    c.expired_hold = None;
    c.life = Life::Open;
    c.wait = None;
    c.open_intent = None;
    c.recheck_marks = BTreeSet::new();
    c.assume_marks = BTreeSet::new();
    c.tags = BTreeSet::new();
    c.due_fired = false;
    c.touched_at = None;
    c.stale_fired = false;
    c.retracted_decision = None;
    c.pr_named = false;
    c.keys = BTreeMap::new();
    c.links = BTreeSet::new();
    match &mut c.role {
        Role::Plain | Role::Signal(_) => {}
        Role::Series(x) => x.last_missed = false,
        Role::Occurrence(o) => {
            o.phase = OccPhase::Waiting;
            o.window_fired = false;
        }
    }
    c
}

/// Puts a hot row onto a node read from its content row.
fn apply_hot(n: &mut Node, h: HotRow) -> std::result::Result<(), String> {
    n.version = h.version;
    n.digest_version = h.digest_version;
    n.content_version = h.content_version;
    n.fence = h.fence;
    n.seq = h.seq;
    n.attempts = h.attempts;
    n.checkpoints = h.checkpoints;
    n.held_ms = h.held_ms;
    n.people_left = h.people_left;
    n.left = h.left;
    n.last_checkpoint = h.last_checkpoint;
    n.expired_hold = h.expired_hold;
    n.life = h.life;
    n.wait = h.wait;
    n.open_intent = h.open_intent;
    n.recheck_marks = h.recheck_marks;
    n.assume_marks = h.assume_marks;
    n.tags = h.tags;
    n.due_fired = h.due_fired;
    n.touched_at = h.touched_at;
    n.stale_fired = h.stale_fired;
    n.retracted_decision = h.retracted_decision;
    n.pr_named = h.pr_named;
    match (&mut n.role, h.role) {
        (Role::Plain | Role::Signal(_), RoleHot::None) => Ok(()),
        (Role::Series(x), RoleHot::Series { last_missed }) => {
            x.last_missed = last_missed;
            Ok(())
        }
        (
            Role::Occurrence(o),
            RoleHot::Occurrence {
                phase,
                window_fired,
            },
        ) => {
            o.phase = phase;
            o.window_fired = window_fired;
            Ok(())
        }
        (_, other) => Err(format!(
            "the hot row's role state {other:?} does not fit the content row's role"
        )),
    }
}

fn json<T: Serialize + ?Sized>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}

fn opt<T: Serialize>(v: Option<&T>) -> Result<Option<String>> {
    v.map(json).transpose()
}

fn present(yes: bool) -> Option<String> {
    yes.then(|| "null".to_owned())
}

/// The canonical JSON of a departed entity's last value (D.4.12, review E1-spec-1): the value its
/// not-resident row holds, the same JSON `value_json` gives the entity while it is resident.
///
/// # Errors
///
/// A value that does not serialise (none does).
pub fn departed_json(v: &Departed) -> Result<String> {
    match v {
        Departed::Request(x) => json(x),
        Departed::PQuestion(x) => json(x),
        Departed::UQuestion(x) => json(x),
        Departed::Commitment(x) => json(x),
        Departed::Said(x) => json(x),
        Departed::Decision(x) => json(x),
        Departed::Objection(x) => json(x),
        Departed::UserEntry(x) => json(x),
        Departed::Digest(x) => json(x),
        Departed::Meta(x) => json(x),
    }
}

/// The canonical JSON of a fact's current value in `s`, or None when the fact is not in `s`.
///
/// # Errors
///
/// A value that does not serialise (none does).
pub fn value_json(s: &State, f: &Fact) -> Result<Option<String>> {
    match f {
        Fact::Scalars => Ok(Some(json(&ScalarsRow {
            params: s.params.clone(),
            last_now: s.last_now,
            counters: s.counters,
            anomalies: s.anomalies,
            done_minutes: s.stats.done_minutes.clone(),
        })?)),
        Fact::NodeHot(n) => s.nodes.get(n).map(|x| json(&hot_row(x))).transpose(),
        Fact::NodeContent(n) => s.nodes.get(n).map(|x| json(&content_row(x))).transpose(),
        Fact::Key(n, k) => opt(s.nodes.get(n).and_then(|x| x.keys.get(k))),
        Fact::Link(n, rel, to) => Ok(present(
            s.nodes
                .get(n)
                .is_some_and(|x| x.links.contains(&(*rel, *to))),
        )),
        Fact::Name(p, name) => opt(s.names.get(&(*p, name.clone()))),
        Fact::Alias(p, name) => opt(s.aliases.get(&(*p, name.clone()))),
        Fact::Hold(t) => opt(s.holds.get(t)),
        Fact::Entry(e) => opt(s.entries.get(e)),
        Fact::Request(i) => opt(s.requests.get(i)),
        Fact::PQuestion(i) => opt(s.pquestions.get(i)),
        Fact::AQuestion(i) => opt(s.aquestions.get(i)),
        Fact::UQuestion(i) => opt(s.uquestions.get(i)),
        Fact::Commitment(i) => opt(s.commitments.get(i)),
        Fact::Said(i) => opt(s.saids.get(i)),
        Fact::Decision(i) => opt(s.decisions.get(i)),
        Fact::Objection(i) => opt(s.objections.get(i)),
        Fact::UserEntry(i) => opt(s.user_entries.get(i)),
        Fact::Digest(i) => opt(s.digest.get(i)),
        Fact::Note(i) => opt(s.notes.get(i)),
        Fact::Approval(n, v) => opt(s.approvals.get(&(*n, *v))),
        Fact::Retracted(t) => Ok(present(s.retracted.contains(t))),
        Fact::Refusal(n, c) => opt(s.refusals.get(&(*n, c.clone()))),
        Fact::ClassProfile(c, caps) => opt(s.classes.get(c).and_then(|x| x.caps_seen.get(caps))),
        Fact::Signal(k) => opt(s.signals.get(k)),
        Fact::Pause(n) => opt(s.pauses.get(n)),
        Fact::Session(i) => opt(s.sessions.get(i)),
        Fact::Principal(k) => opt(s.principal_ids.get(k)),
        Fact::Op(c, o) => opt(s.ops.get(&(c.clone(), o.clone()))),
        Fact::Day(d) => opt(s.stats.days.get(d)),
        Fact::Start(m) => opt(s.stats.starts.get(m)),
        Fact::Profile(caps, bits) => opt(s.stats.profiles.get(&(caps.clone(), *bits))),
        Fact::DemandCounted(n) => Ok(present(s.stats.demand_counted.contains(n))),
        Fact::Since(c) => opt(s.since.get(c)),
        Fact::UserMsgs(p) => opt(s.user_msgs.get(p)),
        Fact::Leaving(x) => opt(s.leaving.get(x)),
        Fact::DigestLeft(p) => opt(s.digest_left.get(p)),
    }
}

fn de<T: DeserializeOwned>(v: &str) -> std::result::Result<T, String> {
    serde_json::from_str(v).map_err(|e| e.to_string())
}

fn unit(v: &str) -> std::result::Result<(), String> {
    if v == "null" {
        Ok(())
    } else {
        Err(format!("a set member's value is {v}, not null"))
    }
}

fn node_mut(s: &mut State, n: NodeId) -> std::result::Result<&mut Node, String> {
    s.nodes
        .get_mut(&n)
        .ok_or_else(|| format!("node {} has no content row", n.0))
}

/// Puts one decoded row into `s`: the inverse of `value_json`. The node rows must come first:
/// a hot row, a key, or a link needs its node's content row.
///
/// # Errors
///
/// A value that does not decode into the fact's type, or a node row that is missing.
pub fn insert_fact(s: &mut State, f: Fact, v: &str) -> std::result::Result<(), String> {
    match f {
        Fact::Scalars => {
            let r: ScalarsRow = de(v)?;
            s.params = r.params;
            s.last_now = r.last_now;
            s.counters = r.counters;
            s.anomalies = r.anomalies;
            s.stats.done_minutes = r.done_minutes;
        }
        Fact::NodeContent(n) => {
            let node: Node = de(v)?;
            if s.nodes.insert(n, node).is_some() {
                return Err("a second content row".to_owned());
            }
        }
        Fact::NodeHot(n) => {
            let h: HotRow = de(v)?;
            apply_hot(node_mut(s, n)?, h)?;
        }
        Fact::Key(n, k) => {
            let nv = de(v)?;
            node_mut(s, n)?.keys.insert(k, nv);
        }
        Fact::Link(n, rel, to) => {
            unit(v)?;
            node_mut(s, n)?.links.insert((rel, to));
        }
        Fact::Name(p, name) => {
            s.names.insert((p, name), de(v)?);
        }
        Fact::Alias(p, name) => {
            s.aliases.insert((p, name), de(v)?);
        }
        Fact::Hold(t) => {
            s.holds.insert(t, de(v)?);
        }
        Fact::Entry(e) => {
            s.entries.insert(e, de(v)?);
        }
        Fact::Request(i) => {
            s.requests.insert(i, de(v)?);
        }
        Fact::PQuestion(i) => {
            s.pquestions.insert(i, de(v)?);
        }
        Fact::AQuestion(i) => {
            s.aquestions.insert(i, de(v)?);
        }
        Fact::UQuestion(i) => {
            s.uquestions.insert(i, de(v)?);
        }
        Fact::Commitment(i) => {
            s.commitments.insert(i, de(v)?);
        }
        Fact::Said(i) => {
            s.saids.insert(i, de(v)?);
        }
        Fact::Decision(i) => {
            s.decisions.insert(i, de(v)?);
        }
        Fact::Objection(i) => {
            s.objections.insert(i, de(v)?);
        }
        Fact::UserEntry(i) => {
            s.user_entries.insert(i, de(v)?);
        }
        Fact::Digest(i) => {
            s.digest.insert(i, de(v)?);
        }
        Fact::Note(i) => {
            s.notes.insert(i, de(v)?);
        }
        Fact::Approval(n, ver) => {
            s.approvals.insert((n, ver), de(v)?);
        }
        Fact::Retracted(t) => {
            unit(v)?;
            s.retracted.insert(t);
        }
        Fact::Refusal(n, c) => {
            s.refusals.insert((n, c), de(v)?);
        }
        Fact::ClassProfile(c, caps) => {
            let t: Time = de(v)?;
            s.classes
                .entry(c)
                .or_insert_with(|| forum_core::ClassActivity {
                    caps_seen: BTreeMap::new(),
                })
                .caps_seen
                .insert(caps, t);
        }
        Fact::Signal(k) => {
            s.signals.insert(k, de(v)?);
        }
        Fact::Pause(n) => {
            s.pauses.insert(n, de(v)?);
        }
        Fact::Session(i) => {
            s.sessions.insert(i, de(v)?);
        }
        Fact::Principal(k) => {
            s.principal_ids.insert(k, de(v)?);
        }
        Fact::Op(c, o) => {
            let r: OpRecord = de(v)?;
            s.ops.insert((c, o), r);
        }
        Fact::Day(d) => {
            s.stats.days.insert(d, de(v)?);
        }
        Fact::Start(m) => {
            s.stats.starts.insert(m, de(v)?);
        }
        Fact::Profile(caps, bits) => {
            s.stats.profiles.insert((caps, bits), de(v)?);
        }
        Fact::DemandCounted(n) => {
            unit(v)?;
            s.stats.demand_counted.insert(n);
        }
        Fact::Since(c) => {
            s.since.insert(c, de(v)?);
        }
        Fact::UserMsgs(p) => {
            s.user_msgs.insert(p, de(v)?);
        }
        Fact::Leaving(x) => {
            s.leaving.insert(x, de(v)?);
        }
        Fact::DigestLeft(p) => {
            s.digest_left.insert(p, de(v)?);
        }
    }
    Ok(())
}

/// Every fact of the base and history parts of `s`, in `Fact` order: what a whole state writes.
#[must_use]
pub fn facts_of(s: &State) -> BTreeSet<Fact> {
    let mut out = BTreeSet::new();
    out.insert(Fact::Scalars);
    for (&id, n) in &s.nodes {
        out.insert(Fact::NodeHot(id));
        out.insert(Fact::NodeContent(id));
        for k in n.keys.keys() {
            out.insert(Fact::Key(id, k.clone()));
        }
        for &(rel, to) in &n.links {
            out.insert(Fact::Link(id, rel, to));
        }
    }
    out.extend(s.names.keys().map(|(p, n)| Fact::Name(*p, n.clone())));
    out.extend(s.aliases.keys().map(|(p, n)| Fact::Alias(*p, n.clone())));
    out.extend(s.holds.keys().map(|t| Fact::Hold(*t)));
    out.extend(s.entries.keys().map(|e| Fact::Entry(*e)));
    out.extend(s.requests.keys().map(|i| Fact::Request(*i)));
    out.extend(s.pquestions.keys().map(|i| Fact::PQuestion(*i)));
    out.extend(s.aquestions.keys().map(|i| Fact::AQuestion(*i)));
    out.extend(s.uquestions.keys().map(|i| Fact::UQuestion(*i)));
    out.extend(s.commitments.keys().map(|i| Fact::Commitment(*i)));
    out.extend(s.saids.keys().map(|i| Fact::Said(*i)));
    out.extend(s.decisions.keys().map(|i| Fact::Decision(*i)));
    out.extend(s.objections.keys().map(|i| Fact::Objection(*i)));
    out.extend(s.user_entries.keys().map(|i| Fact::UserEntry(*i)));
    out.extend(s.digest.keys().map(|i| Fact::Digest(*i)));
    out.extend(s.notes.keys().map(|i| Fact::Note(*i)));
    out.extend(s.approvals.keys().map(|(n, v)| Fact::Approval(*n, *v)));
    out.extend(s.retracted.iter().map(|t| Fact::Retracted(*t)));
    out.extend(s.refusals.keys().map(|(n, c)| Fact::Refusal(*n, c.clone())));
    for (c, a) in &s.classes {
        out.extend(
            a.caps_seen
                .keys()
                .map(|caps| Fact::ClassProfile(c.clone(), caps.clone())),
        );
    }
    out.extend(s.signals.keys().map(|k| Fact::Signal(*k)));
    out.extend(s.pauses.keys().map(|n| Fact::Pause(*n)));
    out.extend(s.sessions.keys().map(|i| Fact::Session(*i)));
    out.extend(s.principal_ids.keys().map(|k| Fact::Principal(k.clone())));
    out.extend(s.ops.keys().map(|(c, o)| Fact::Op(c.clone(), o.clone())));
    out.extend(s.stats.days.keys().map(|d| Fact::Day(*d)));
    out.extend(s.stats.starts.keys().map(|m| Fact::Start(*m)));
    out.extend(
        s.stats
            .profiles
            .keys()
            .map(|(caps, bits)| Fact::Profile(caps.clone(), *bits)),
    );
    out.extend(
        s.stats
            .demand_counted
            .iter()
            .map(|n| Fact::DemandCounted(*n)),
    );
    out.extend(s.since.keys().map(|c| Fact::Since(*c)));
    out.extend(s.user_msgs.keys().map(|p| Fact::UserMsgs(*p)));
    out.extend(s.leaving.keys().map(|x| Fact::Leaving(*x)));
    out.extend(s.digest_left.keys().map(|p| Fact::DigestLeft(*p)));
    out
}

// ------------------------------------------------------------------------------------------------
// Rows

/// What a row carries beside its key and value.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Extra {
    /// Nothing; for an `st_ops` row, a row read without its masking version and response
    /// (`Which::Resident`), which `put_row` refuses to write.
    None,
    /// `st_keys.open`.
    Open(bool),
    /// `st_ops.masking_version` and `st_ops.response`.
    Op {
        /// The masking version of the record's digest (D.11.5).
        masking_version: u32,
        /// The rendered response (O1).
        response: Vec<u8>,
    },
}

/// One row of the state tables in a table-independent form: the fact's kind and key, its value,
/// whether it is resident, and the table's own columns. Migrations rewrite these (D.7.6).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Row {
    /// The `Fact` variant's index.
    pub kind: u8,
    /// The canonical JSON of the fact's key.
    pub key: String,
    /// The canonical JSON of its value.
    pub value: String,
    /// False once it left memory (`st_facts` only).
    pub resident: bool,
    /// The table's own columns.
    pub extra: Extra,
}

impl Row {
    /// The table that holds the row.
    #[must_use]
    pub fn table(&self) -> Table {
        table_of_kind(self.kind)
    }
}

/// The key of a node row.
fn node_key(n: NodeId) -> Result<String> {
    json(&n)
}

#[derive(Deserialize)]
struct CapacityOnly {
    capacity: u32,
}

fn sql_u64(v: u64) -> Result<i64> {
    i64::try_from(v).map_err(|_| StoreError::Invalid(format!("{v} does not fit an SQLite integer")))
}

/// Writes one row, updating it in place when it exists. A row of `st_nodes` or `st_holds` is
/// updated first and inserted only when no row changed, so that the capacity triggers see an
/// insert only for a new hold (an upsert would fire the insert trigger on a full node). A row of
/// `st_names` is updated only over a row of its own kind, name or alias, and a row of `st_ops` is
/// only inserted, so that their primary keys fire on a second live row (K3, O1; §24).
///
/// # Errors
///
/// A key that does not fit its table's columns, or a statement that fails, a constraint among
/// them.
pub fn put_row(conn: &Connection, r: &Row) -> Result<()> {
    match r.table() {
        Table::Scalars => {
            exec(
                conn,
                "INSERT INTO st_scalars (one, v) VALUES (0, ?1)
                 ON CONFLICT (one) DO UPDATE SET v = excluded.v",
                [&r.value],
            )?;
        }
        Table::Nodes => {
            let id: u64 = serde_json::from_str(&r.key)?;
            let cap: CapacityOnly = serde_json::from_str(&r.value)?;
            let id = sql_u64(id)?;
            let n = exec(
                conn,
                "UPDATE st_nodes SET capacity = ?2, v = ?3 WHERE id = ?1",
                params![id, cap.capacity, r.value],
            )?;
            if n == 0 {
                exec(
                    conn,
                    "INSERT INTO st_nodes (id, capacity, v) VALUES (?1, ?2, ?3)",
                    params![id, cap.capacity, r.value],
                )?;
            }
        }
        Table::NodeHot => {
            let id = sql_u64(serde_json::from_str(&r.key)?)?;
            let n = exec(
                conn,
                "UPDATE st_node_hot SET v = ?2 WHERE id = ?1",
                params![id, r.value],
            )?;
            if n == 0 {
                exec(
                    conn,
                    "INSERT INTO st_node_hot (id, v) VALUES (?1, ?2)",
                    params![id, r.value],
                )?;
            }
        }
        Table::Keys => {
            let (node, key): (u64, String) = serde_json::from_str(&r.key)?;
            let nv: u16 = serde_json::from_str(&r.value)?;
            let open = match r.extra {
                Extra::Open(o) => o,
                Extra::None | Extra::Op { .. } => {
                    return Err(StoreError::Invalid(format!(
                        "a key row without its open column: {}",
                        r.key
                    )));
                }
            };
            exec(
                conn,
                "INSERT INTO st_keys (key, node, nv, open) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (key, node) DO UPDATE SET nv = excluded.nv, open = excluded.open",
                params![key, sql_u64(node)?, nv, open],
            )?;
        }
        Table::Names => {
            // A name and an alias share the primary key (K3): a row of the same kind is updated
            // in place, and a row of the other kind at the same (parent, name) is never
            // overwritten, so that the primary key fires on it. When a (parent, name) switches
            // kind legitimately, the old kind's fact is dirty and gone, and pass 1 has deleted its
            // row already.
            let (parent, name): (u64, String) = serde_json::from_str(&r.key)?;
            let node: u64 = serde_json::from_str(&r.value)?;
            let (parent, node, alias) = (sql_u64(parent)?, sql_u64(node)?, r.kind == K_ALIAS);
            let n = exec(
                conn,
                "UPDATE st_names SET node = ?3 WHERE parent = ?1 AND name = ?2 AND alias = ?4",
                params![parent, name, node, alias],
            )?;
            if n == 0 {
                exec(
                    conn,
                    "INSERT INTO st_names (parent, name, node, alias) VALUES (?1, ?2, ?3, ?4)",
                    params![parent, name, node, alias],
                )?;
            }
        }
        Table::Holds => {
            let t: forum_core::Token = serde_json::from_str(&r.key)?;
            let (node, fence) = (sql_u64(t.node.0)?, sql_u64(t.fence.0)?);
            let n = exec(
                conn,
                "UPDATE st_holds SET v = ?3 WHERE node = ?1 AND fence = ?2",
                params![node, fence, r.value],
            )?;
            if n == 0 {
                exec(
                    conn,
                    "INSERT INTO st_holds (node, fence, v) VALUES (?1, ?2, ?3)",
                    params![node, fence, r.value],
                )?;
            }
        }
        Table::Ops => {
            let (class, op): (String, String) = serde_json::from_str(&r.key)?;
            let Extra::Op {
                masking_version,
                response,
            } = &r.extra
            else {
                return Err(StoreError::Invalid(format!(
                    "an op row without its response: {}",
                    r.key
                )));
            };
            // A plain insert: pass 1 has deleted the row of every dirty record, so a second row of
            // a live (class, op_id), which `evolve` refused and did not mark dirty, trips the
            // primary key (O1, §24).
            exec(
                conn,
                "INSERT INTO st_ops (class, op_id, v, masking_version, response)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![class, op, r.value, masking_version, response],
            )?;
        }
        Table::Facts => {
            exec(
                conn,
                "INSERT INTO st_facts (fact, k, v, resident) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (fact, k) DO UPDATE SET v = excluded.v, resident = excluded.resident",
                params![r.kind, r.key, r.value, r.resident],
            )?;
        }
    }
    Ok(())
}

/// Deletes the row of a fact. A name and an alias share their primary key (K3), so each deletes
/// only a row of its own kind.
///
/// # Errors
///
/// A key that does not fit its table, or a failed statement.
pub fn delete_row(conn: &Connection, kind: u8, key: &str) -> Result<()> {
    delete_rows(conn, kind, key).map(|_| ())
}

/// `delete_row`, returning how many rows went (0 or 1).
fn delete_rows(conn: &Connection, kind: u8, key: &str) -> Result<usize> {
    match table_of_kind(kind) {
        Table::Scalars => exec(conn, "DELETE FROM st_scalars", params![]),
        Table::Nodes => {
            let id: u64 = serde_json::from_str(key)?;
            exec(conn, "DELETE FROM st_nodes WHERE id = ?1", [sql_u64(id)?])
        }
        Table::NodeHot => {
            let id: u64 = serde_json::from_str(key)?;
            exec(
                conn,
                "DELETE FROM st_node_hot WHERE id = ?1",
                [sql_u64(id)?],
            )
        }
        Table::Keys => {
            let (node, k): (u64, String) = serde_json::from_str(key)?;
            exec(
                conn,
                "DELETE FROM st_keys WHERE key = ?1 AND node = ?2",
                params![k, sql_u64(node)?],
            )
        }
        Table::Names => {
            let (parent, name): (u64, String) = serde_json::from_str(key)?;
            exec(
                conn,
                "DELETE FROM st_names WHERE parent = ?1 AND name = ?2 AND alias = ?3",
                params![sql_u64(parent)?, name, kind == K_ALIAS],
            )
        }
        Table::Holds => {
            let t: forum_core::Token = serde_json::from_str(key)?;
            exec(
                conn,
                "DELETE FROM st_holds WHERE node = ?1 AND fence = ?2",
                params![sql_u64(t.node.0)?, sql_u64(t.fence.0)?],
            )
        }
        Table::Ops => {
            let (class, op): (String, String) = serde_json::from_str(key)?;
            exec(
                conn,
                "DELETE FROM st_ops WHERE class = ?1 AND op_id = ?2",
                params![class, op],
            )
        }
        Table::Facts => exec(
            conn,
            "DELETE FROM st_facts WHERE fact = ?1 AND k = ?2",
            params![kind, key],
        ),
    }
}

/// The row of a fact as `s` holds it, or None when the fact is not in `s`. An `Op` row gets an
/// empty response: its whole row is written by `put_op` (D.11.6, step 4).
///
/// # Errors
///
/// A value that does not serialise.
pub fn row_of(s: &State, f: &Fact) -> Result<Option<Row>> {
    let Some(value) = value_json(s, f)? else {
        return Ok(None);
    };
    let extra = match f {
        Fact::Key(n, _) => Extra::Open(s.nodes.get(n).is_some_and(Node::is_open)),
        Fact::Op(..) => Extra::Op {
            masking_version: 0,
            response: Vec::new(),
        },
        _ => Extra::None,
    };
    Ok(Some(Row {
        kind: kind(f),
        key: key_json(f)?,
        value,
        resident: true,
        extra,
    }))
}

/// What the two passes did, for the tests and the logs.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Written {
    /// Rows deleted.
    pub deleted: u32,
    /// Rows written not resident: a departed entity's last value, or a flag flipped.
    pub left: u32,
    /// Entities that left memory with no value given and no row to mark: a core bug, which a debug
    /// or test build refuses (review E1-spec-1); a release build logs it and goes on.
    pub left_without_row: Vec<Fact>,
    /// Rows written.
    pub upserted: u32,
}

/// The order of pass 1's deletes: holds before the nodes they reference (nodes never leave the
/// state, but the order keeps the foreign keys right if they did).
fn delete_rank(t: Table) -> u8 {
    match t {
        Table::Holds => 0,
        Table::Keys => 1,
        Table::Names => 2,
        Table::Facts => 3,
        Table::Ops => 4,
        Table::NodeHot => 5,
        Table::Nodes => 6,
        Table::Scalars => 7,
    }
}

/// The order of pass 2's writes: the scalars, then nodes before their hot rows and holds; the key
/// rows whose `open` is 1 come last of all, after this order.
fn write_rank(t: Table) -> u8 {
    match t {
        Table::Scalars => 0,
        Table::Nodes => 1,
        Table::NodeHot => 2,
        Table::Names => 3,
        Table::Holds => 4,
        Table::Facts => 5,
        Table::Ops => 6,
        Table::Keys => 7,
    }
}

/// Writes the state rows of a job's dirty facts in two passes (D.11.6, step 3), so that no
/// statement sees a transient violation of the four constraints, which therefore fire only on a
/// real one, a core bug (§24):
/// 1. every row whose fact is no longer in `s` is deleted (a pruned `op_id` record's row
///    included), and so is the row of every dirty `Fact::Op` still in `s`, a record this job
///    wrote, whose whole row `put_op` inserts in step 4; a fact that left memory (D.4.12) has its
///    row written with its last value from `departed`, which the core recorded before it removed
///    the entity, and `resident = 0` (review E1-spec-1: the job may have changed it, or created it,
///    before it left), or, given no such value, its stored row marked `resident = 0`; then the key
///    rows whose `open` is 0 are written;
/// 2. every other dirty fact still in `s` is written with its current value (a restored row marked
///    `resident = 1`), nodes before holds, and the key rows whose `open` is 1 last. `Fact::Op` is
///    skipped: `put_op` writes its whole row (step 4).
///
/// `evolve` marks an `Op` dirty only when it records or prunes it, so a live record it refused to
/// record again keeps its row, and `put_op`'s insert of a second one trips the primary key (O1).
///
/// # Errors
///
/// A failed statement, a constraint violation among them (the caller aborts the process).
pub fn write_dirty(
    conn: &Connection,
    s: &State,
    dirty: &BTreeSet<Fact>,
    departed: &BTreeMap<Fact, Departed>,
) -> Result<Written> {
    let mut gone: Vec<(Table, u8, String)> = Vec::new();
    let mut left: Vec<(Fact, String)> = Vec::new();
    let mut last: Vec<Row> = Vec::new();
    let mut closed_keys: Vec<Row> = Vec::new();
    let mut open_keys: Vec<Row> = Vec::new();
    let mut rest: Vec<Row> = Vec::new();
    for f in dirty {
        if let Fact::Op(..) = f {
            gone.push((Table::Ops, kind(f), key_json(f)?));
            continue;
        }
        match row_of(s, f)? {
            None => {
                let key = key_json(f)?;
                if leaves_memory(f) {
                    match departed.get(f) {
                        Some(v) => last.push(Row {
                            kind: kind(f),
                            key,
                            value: departed_json(v)?,
                            resident: false,
                            extra: Extra::None,
                        }),
                        None => left.push((f.clone(), key)),
                    }
                } else {
                    gone.push((table_of_kind(kind(f)), kind(f), key));
                }
            }
            Some(row) => match row.extra {
                Extra::Open(true) => open_keys.push(row),
                Extra::Open(false) => closed_keys.push(row),
                Extra::None | Extra::Op { .. } => rest.push(row),
            },
        }
    }
    let mut w = Written::default();
    // Pass 1.
    gone.sort_by_key(|(t, _, _)| delete_rank(*t));
    for (_, k, key) in &gone {
        let n = delete_rows(conn, *k, key)?;
        w.deleted = w
            .deleted
            .saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
    }
    for r in &last {
        put_row(conn, r)?;
        w.left = w.left.saturating_add(1);
    }
    for (f, key) in left {
        let n = exec(
            conn,
            "UPDATE st_facts SET resident = 0 WHERE fact = ?1 AND k = ?2",
            params![kind(&f), key],
        )?;
        if n == 0 {
            // No value and no row: a later restore could not bring the entity back (D.12.6). The
            // core hands every departure's value to the writer, so this is a core bug, which a
            // debug or test build refuses at once.
            if cfg!(any(debug_assertions, test)) {
                return Err(StoreError::Invalid(format!(
                    "{f:?} left memory with neither its last value nor a row"
                )));
            }
            tracing::warn!(?f, "an entity left memory before its row was ever written");
            w.left_without_row.push(f);
        } else {
            w.left = w.left.saturating_add(1);
        }
    }
    for r in &closed_keys {
        put_row(conn, r)?;
        w.upserted = w.upserted.saturating_add(1);
    }
    // Pass 2.
    rest.sort_by_key(|r| write_rank(r.table()));
    for r in rest.iter().chain(open_keys.iter()) {
        put_row(conn, r)?;
        w.upserted = w.upserted.saturating_add(1);
    }
    Ok(w)
}

/// Writes the whole `st_ops` row of a live `op_id` record: its value, the masking version of its
/// digest, and the rendered response, which exists only once the response is rendered (D.11.6,
/// step 4; O1).
///
/// # Errors
///
/// When `s` has no such record, or a failed statement.
pub fn put_op(
    conn: &Connection,
    s: &State,
    class: &forum_core::ClassName,
    op: &forum_core::OpId,
    masking_version: u32,
    response: &[u8],
) -> Result<()> {
    let f = Fact::Op(class.clone(), op.clone());
    let Some(mut row) = row_of(s, &f)? else {
        return Err(StoreError::Invalid(format!(
            "no live op_id record ({}, {})",
            class.as_str(),
            op.as_str()
        )));
    };
    row.extra = Extra::Op {
        masking_version,
        response: response.to_vec(),
    };
    put_row(conn, &row)
}

/// A stored `op_id` answer: the rendered response and the masking version of its digest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StoredOp {
    /// The record's value (canonical JSON of `OpRecord`).
    pub value: String,
    /// The masking version of its digest (D.11.5).
    pub masking_version: u32,
    /// The rendered response, returned verbatim to a repeat (O1).
    pub response: Vec<u8>,
}

/// The `st_ops` row of `(class, op_id)`, if any: a repeat's response (D.8.1) and the masking
/// version its digest is computed with (D.11.5, CS-35).
///
/// # Errors
///
/// A failed query.
pub fn stored_op(conn: &Connection, class: &str, op: &str) -> Result<Option<StoredOp>> {
    Ok(conn
        .query_row(
            "SELECT v, masking_version, response FROM st_ops WHERE class = ?1 AND op_id = ?2",
            params![class, op],
            |r| {
                Ok(StoredOp {
                    value: r.get(0)?,
                    masking_version: r.get(1)?,
                    response: r.get(2)?,
                })
            },
        )
        .optional()?)
}

// ------------------------------------------------------------------------------------------------
// Reading

/// Which rows a read returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    /// Only the resident rows of `st_facts`, and the `st_ops` rows without their masking version
    /// and response (`Extra::None`): what startup loads (D.11.8, step 2), which needs only the
    /// records. A repeat reads its response through `stored_op`.
    Resident,
    /// Every row, whole: what a migration rewrites and a comparison dumps.
    All,
}

/// A column's value as an error message shows it.
fn shown(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".to_owned(),
        ValueRef::Integer(x) => x.to_string(),
        ValueRef::Real(_) => "a real".to_owned(),
        ValueRef::Text(t) => format!("'{}'", String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => format!("a blob of {} bytes", b.len()),
    }
}

/// The error of a column that does not hold what its table declares, named by its table and its
/// row's key (D.11.8, step 2).
fn bad_column(
    r: &rusqlite::Row<'_>,
    i: usize,
    table: &'static str,
    key: &str,
    want: &str,
) -> StoreError {
    let column = r.as_ref().column_name(i).unwrap_or("?");
    let found = r.get_ref(i).map_or_else(|e| e.to_string(), shown);
    StoreError::Decode {
        table,
        key: key.to_owned(),
        msg: format!("column {column} holds {found}, not {want}"),
    }
}

/// A column's value rendered as part of a key, for a row whose key columns do not decode.
fn raw(r: &rusqlite::Row<'_>, i: usize) -> String {
    r.get_ref(i).map_or_else(
        |e| e.to_string(),
        |v| match v {
            ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
            other => shown(other),
        },
    )
}

fn int_col(r: &rusqlite::Row<'_>, i: usize, table: &'static str, key: &str) -> Result<i64> {
    match r.get_ref(i)? {
        ValueRef::Integer(x) => Ok(x),
        _ => Err(bad_column(r, i, table, key, "an integer")),
    }
}

/// An integer column that must fit `T` (an id, a count, a version, a kind).
fn uint_col<T: TryFrom<i64>>(
    r: &rusqlite::Row<'_>,
    i: usize,
    table: &'static str,
    key: &str,
) -> Result<T> {
    let x = int_col(r, i, table, key)?;
    T::try_from(x).map_err(|_| {
        bad_column(
            r,
            i,
            table,
            key,
            &format!("an integer within {}", std::any::type_name::<T>()),
        )
    })
}

/// A boolean column: 0 or 1.
fn bool_col(r: &rusqlite::Row<'_>, i: usize, table: &'static str, key: &str) -> Result<bool> {
    match int_col(r, i, table, key)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(bad_column(r, i, table, key, "0 or 1")),
    }
}

fn text_col(r: &rusqlite::Row<'_>, i: usize, table: &'static str, key: &str) -> Result<String> {
    match r.get_ref(i)? {
        ValueRef::Text(t) => std::str::from_utf8(t)
            .map(str::to_owned)
            .map_err(|_| bad_column(r, i, table, key, "UTF-8 text")),
        _ => Err(bad_column(r, i, table, key, "text")),
    }
}

fn blob_col(r: &rusqlite::Row<'_>, i: usize, table: &'static str, key: &str) -> Result<Vec<u8>> {
    match r.get_ref(i)? {
        ValueRef::Blob(b) => Ok(b.to_vec()),
        _ => Err(bad_column(r, i, table, key, "a blob")),
    }
}

/// How a row of `st_facts` is named in an error: its fact's variant and its key (`Session 3`), as
/// about thirty kinds of fact share the table.
#[must_use]
pub fn fact_label(kind: u8, key: &str) -> String {
    FACT_NAMES
        .get(usize::from(kind))
        .map_or_else(|| format!("fact {kind} {key}"), |n| format!("{n} {key}"))
}

/// Visits every row of the state tables in their load order (D.11.8): scalars, nodes, hot rows,
/// keys, names, holds, ops, then `st_facts` by `(fact, k)`, one row at a time, so that a load holds
/// one row beside the state rather than all of them. A column that does not hold what its table
/// declares stops the read with its table and its row's key.
///
/// # Errors
///
/// A failed query, a column that does not fit its type, or the visitor's error.
pub fn each_row(conn: &Connection, which: Which, f: impl FnMut(Row) -> Result<()>) -> Result<()> {
    each_row_in(conn, "", which, f)
}

/// `each_row` over the state tables whose names carry `prefix`: `""`, or the prefix of the tables
/// a migration set aside (D.7.6).
pub(super) fn each_row_in(
    conn: &Connection,
    prefix: &str,
    which: Which,
    mut f: impl FnMut(Row) -> Result<()>,
) -> Result<()> {
    let mut st = conn.prepare(&format!("SELECT v FROM {prefix}st_scalars ORDER BY one"))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        f(Row {
            kind: K_SCALARS,
            key: "null".to_owned(),
            value: text_col(r, 0, "st_scalars", "null")?,
            resident: true,
            extra: Extra::None,
        })?;
    }
    let mut st = conn.prepare(&format!(
        "SELECT id, capacity, v FROM {prefix}st_nodes ORDER BY id"
    ))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let id: u64 = uint_col(r, 0, "st_nodes", &raw(r, 0))?;
        let key = node_key(NodeId(id))?;
        let column = int_col(r, 1, "st_nodes", &key)?;
        let value = text_col(r, 2, "st_nodes", &key)?;
        let cap: CapacityOnly = serde_json::from_str(&value).map_err(|e| StoreError::Decode {
            table: "st_nodes",
            key: key.clone(),
            msg: e.to_string(),
        })?;
        if i64::from(cap.capacity) != column {
            return Err(StoreError::Decode {
                table: "st_nodes",
                key,
                msg: format!(
                    "the capacity column {column} differs from the row's capacity {}",
                    cap.capacity
                ),
            });
        }
        f(Row {
            kind: K_NODE_CONTENT,
            key,
            value,
            resident: true,
            extra: Extra::None,
        })?;
    }
    let mut st = conn.prepare(&format!(
        "SELECT id, v FROM {prefix}st_node_hot ORDER BY id"
    ))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let id: u64 = uint_col(r, 0, "st_node_hot", &raw(r, 0))?;
        let key = node_key(NodeId(id))?;
        let value = text_col(r, 1, "st_node_hot", &key)?;
        f(Row {
            kind: K_NODE_HOT,
            key,
            value,
            resident: true,
            extra: Extra::None,
        })?;
    }
    let mut st = conn.prepare(&format!(
        "SELECT key, node, nv, open FROM {prefix}st_keys ORDER BY node, key"
    ))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let k = text_col(r, 0, "st_keys", &format!("[{},{}]", raw(r, 1), raw(r, 0)))?;
        let node: u64 = uint_col(r, 1, "st_keys", &format!("[{},{}]", raw(r, 1), json(&k)?))?;
        let key = json(&(node, &k))?;
        let nv: u16 = uint_col(r, 2, "st_keys", &key)?;
        let open = bool_col(r, 3, "st_keys", &key)?;
        f(Row {
            kind: K_KEY,
            key,
            value: json(&nv)?,
            resident: true,
            extra: Extra::Open(open),
        })?;
    }
    let mut st = conn.prepare(&format!(
        "SELECT parent, name, node, alias FROM {prefix}st_names ORDER BY alias, parent, name"
    ))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let name = text_col(r, 1, "st_names", &format!("[{},{}]", raw(r, 0), raw(r, 1)))?;
        let parent: u64 = uint_col(
            r,
            0,
            "st_names",
            &format!("[{},{}]", raw(r, 0), json(&name)?),
        )?;
        let key = json(&(parent, &name))?;
        let node: u64 = uint_col(r, 2, "st_names", &key)?;
        let alias = bool_col(r, 3, "st_names", &key)?;
        f(Row {
            kind: if alias { K_ALIAS } else { K_NAME },
            key,
            value: json(&node)?,
            resident: true,
            extra: Extra::None,
        })?;
    }
    let mut st = conn.prepare(&format!(
        "SELECT node, fence, v FROM {prefix}st_holds ORDER BY node, fence"
    ))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let raw_key = format!("{{\"node\":{},\"fence\":{}}}", raw(r, 0), raw(r, 1));
        let node: u64 = uint_col(r, 0, "st_holds", &raw_key)?;
        let fence: u64 = uint_col(r, 1, "st_holds", &raw_key)?;
        let key = json(&forum_core::Token {
            node: NodeId(node),
            fence: Fence(fence),
        })?;
        let value = text_col(r, 2, "st_holds", &key)?;
        f(Row {
            kind: K_HOLD,
            key,
            value,
            resident: true,
            extra: Extra::None,
        })?;
    }
    let sql = match which {
        Which::Resident => {
            format!("SELECT class, op_id, v FROM {prefix}st_ops ORDER BY class, op_id")
        }
        Which::All => format!(
            "SELECT class, op_id, v, masking_version, response FROM {prefix}st_ops
             ORDER BY class, op_id"
        ),
    };
    let mut st = conn.prepare(&sql)?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let class = text_col(r, 0, "st_ops", &format!("[{},{}]", raw(r, 0), raw(r, 1)))?;
        let op = text_col(
            r,
            1,
            "st_ops",
            &format!("[{},{}]", json(&class)?, raw(r, 1)),
        )?;
        let key = json(&(class, op))?;
        let value = text_col(r, 2, "st_ops", &key)?;
        let extra = match which {
            Which::Resident => Extra::None,
            Which::All => Extra::Op {
                masking_version: uint_col(r, 3, "st_ops", &key)?,
                response: blob_col(r, 4, "st_ops", &key)?,
            },
        };
        f(Row {
            kind: K_OP,
            key,
            value,
            resident: true,
            extra,
        })?;
    }
    // `resident >= 1` rather than `= 1`, in the order of the index `st_facts_resident`: a
    // resident column that holds anything but 0 or 1 (a text sorts above every integer) is read
    // and stops the load, instead of hiding its row.
    let sql = match which {
        Which::Resident => format!(
            "SELECT fact, k, v, resident FROM {prefix}st_facts WHERE resident >= 1
             ORDER BY resident, fact, k"
        ),
        Which::All => format!("SELECT fact, k, v, resident FROM {prefix}st_facts ORDER BY fact, k"),
    };
    let mut st = conn.prepare(&sql)?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let k = text_col(
            r,
            1,
            "st_facts",
            &format!("fact {} {}", raw(r, 0), raw(r, 1)),
        )?;
        let kind: u8 = uint_col(r, 0, "st_facts", &format!("fact {} {k}", raw(r, 0)))?;
        let label = fact_label(kind, &k);
        let value = text_col(r, 2, "st_facts", &label)?;
        let resident = bool_col(r, 3, "st_facts", &label)?;
        f(Row {
            kind,
            key: k,
            value,
            resident,
            extra: Extra::None,
        })?;
    }
    Ok(())
}

/// Every row of the state tables in their load order (`each_row`).
///
/// # Errors
///
/// As `each_row`.
pub fn read_rows(conn: &Connection, which: Which) -> Result<Vec<Row>> {
    let mut out = Vec::new();
    each_row(conn, which, |r| {
        out.push(r);
        Ok(())
    })?;
    Ok(out)
}

/// Builds the base and history parts of a state from rows in load order, one row at a time,
/// without the indices. A row that does not decode is named by its table and key, a row of
/// `st_facts` by its fact's variant and key (`fact_label`).
#[derive(Default)]
pub struct Loader {
    s: State,
    hot: BTreeSet<NodeId>,
    scalars: bool,
}

impl Loader {
    /// Puts one row into the state.
    ///
    /// # Errors
    ///
    /// The row does not decode, or does not fit the rows before it (a key row whose `open` column
    /// disagrees with its node's life, a hot row or key without its node's content row, a second
    /// scalar row).
    pub fn push(&mut self, r: &Row) -> Result<()> {
        let table = r.table().name();
        let key = if r.table() == Table::Facts {
            fact_label(r.kind, &r.key)
        } else {
            r.key.clone()
        };
        let bad = |msg: String| StoreError::Decode {
            table,
            key: key.clone(),
            msg,
        };
        let f = fact_of(r.kind, &r.key).map_err(bad)?;
        if r.table() == Table::Facts && !matches!(r.extra, Extra::None) {
            return Err(bad("an st_facts row with extra columns".to_owned()));
        }
        match &f {
            Fact::NodeHot(n) => {
                self.hot.insert(*n);
            }
            Fact::Scalars => {
                if self.scalars {
                    return Err(bad("a second scalar row".to_owned()));
                }
                self.scalars = true;
            }
            _ => {}
        }
        let key_open = match (&f, &r.extra) {
            (Fact::Key(n, _), Extra::Open(o)) => Some((*n, *o)),
            _ => None,
        };
        insert_fact(&mut self.s, f, &r.value).map_err(bad)?;
        // The open column follows the node's life, which the hot row already gave (D.11.6).
        if let Some((n, open)) = key_open
            && self.s.nodes.get(&n).is_some_and(|x| x.is_open() != open)
        {
            return Err(bad(format!(
                "open = {open} while the node is {}",
                if open { "closed" } else { "open" }
            )));
        }
        Ok(())
    }

    /// The state, once it has its scalar row and every node has its hot row.
    ///
    /// # Errors
    ///
    /// No scalar row (the parameters, `last_now`, and the counters: without them the first write
    /// would reuse event numbers), or a node without its hot row.
    pub fn finish(self) -> Result<State> {
        if !self.scalars {
            return Err(StoreError::Decode {
                table: "st_scalars",
                key: "null".to_owned(),
                msg: "missing".to_owned(),
            });
        }
        if let Some(n) = self.s.nodes.keys().find(|n| !self.hot.contains(n)) {
            return Err(StoreError::Decode {
                table: "st_node_hot",
                key: json(n)?,
                msg: "a node without its hot row".to_owned(),
            });
        }
        Ok(self.s)
    }
}

/// Builds the base and history parts of a state from rows in load order (`read_rows`).
///
/// # Errors
///
/// The first row that does not decode, or a node whose hot or content row is missing.
pub fn state_from_rows(rows: &[Row]) -> Result<State> {
    let mut l = Loader::default();
    for r in rows {
        l.push(r)?;
    }
    l.finish()
}

/// Loads the base and history parts from the resident rows (D.11.8, step 2), without the indices,
/// streaming the rows.
///
/// # Errors
///
/// A failed query, or the first row that does not decode, named by its table and key.
pub fn load_facts(conn: &Connection) -> Result<State> {
    let mut l = Loader::default();
    each_row(conn, Which::Resident, |r| l.push(&r))?;
    l.finish()
}

/// The value of one fact's row and whether it is resident: what `restore` reads (D.4.12).
///
/// # Errors
///
/// A failed query.
pub fn fact_row(conn: &Connection, f: &Fact) -> Result<Option<(String, bool)>> {
    if table_of_kind(kind(f)) != Table::Facts {
        return Err(StoreError::Invalid(format!(
            "{f:?} is not kept in st_facts"
        )));
    }
    Ok(conn
        .query_row(
            "SELECT v, resident FROM st_facts WHERE fact = ?1 AND k = ?2",
            params![kind(f), key_json(f)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// `last_now` of the stored scalar row, if there is one: what a `migrate` transaction's time is
/// clamped to (D.11.6: `txs.at` is the clamped now).
///
/// # Errors
///
/// A failed query, or a scalar row that does not decode.
pub fn stored_last_now(conn: &Connection) -> Result<Option<Time>> {
    #[derive(Deserialize)]
    struct LastNow {
        last_now: Time,
    }
    let v: Option<String> = conn
        .query_row("SELECT v FROM st_scalars", [], |r| r.get(0))
        .optional()?;
    v.map(|v| {
        serde_json::from_str::<LastNow>(&v)
            .map(|x| x.last_now)
            .map_err(|e| StoreError::Decode {
                table: "st_scalars",
                key: "null".to_owned(),
                msg: e.to_string(),
            })
    })
    .transpose()
}

/// Deletes every row of the state tables, in the order of their foreign keys. A migration does not
/// use it: it sets the tables aside and streams their rows (D.7.6, `migrate`).
///
/// # Errors
///
/// A failed statement.
pub fn clear_state_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM st_holds; DELETE FROM st_node_hot; DELETE FROM st_keys; DELETE FROM st_names;
         DELETE FROM st_ops; DELETE FROM st_facts; DELETE FROM st_nodes; DELETE FROM st_scalars;",
    )?;
    Ok(())
}

/// Writes whole rows into empty state tables, nodes before their hot rows and holds, and the key
/// rows last, closed before open. It holds every row given: for a few rows, not for a whole
/// state (a migration streams, `migrate`).
///
/// # Errors
///
/// A failed statement.
pub fn put_rows(conn: &Connection, rows: &[Row]) -> Result<()> {
    let mut ordered: Vec<&Row> = rows.iter().collect();
    ordered.sort_by_key(|r| {
        let open = matches!(r.extra, Extra::Open(true));
        (write_rank(r.table()), open)
    });
    for r in ordered {
        put_row(conn, r)?;
    }
    Ok(())
}
