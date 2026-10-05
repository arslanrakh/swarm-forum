//! Facts (design D.6.1, D.11.6, FQ-2): one stored object of the base or history part of the state,
//! the unit of the shell's state tables.
//!
//! `evolve_tracked`, `apply_tracked`, `settle_tracked`, and `restore_tracked` add to a
//! [`Tracked`] every fact their events inserted, changed, or removed, and the last value of every
//! entity that left memory during the job; the shell writes each fact's current value, or deletes
//! it, or stores a departed entity's last value as not resident, in the transaction of the events.
//! [`changed_facts`] is the naive oracle of the dirty set: it compares two states map by map, and
//! the tests require it to be a subset of what `evolve_tracked` reported (D.12.3, item 11).

use alloc::collections::{BTreeMap, BTreeSet};

use crate::state::{
    BasisTarget, Cand, Commitment, Decision, DigestEntry, EntityRef, EntryMeta, Node, Objection,
    PQuestion, Request, Role, Said, SignalKey, State, UQuestion, UserEntry,
};
use crate::types::{
    ClassName, CommitmentId, DecisionId, DigestId, Eid, KeyStr, Label, LinkRel, Name, NodeId,
    NoteId, ObjectionId, OpId, PrincipalId, PrincipalKey, QuestionId, RequestId, SaidId, SessionId,
    Token, UserEntryId, Version,
};

/// One stored object of the base or history part (D.4.2): the unit of the shell's state tables
/// (D.11.6, FQ-2). `Params`, `last_now`, `counters`, `anomalies`, and the small parts of `Stats`
/// (`done_minutes`) are `Scalars`; every map entry is its own fact. A node is four kinds of fact
/// (D.4.3): its hot row, its content row, one per key, and one per outgoing link; a class is one fact
/// per caps profile (D.4.6).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Fact {
    /// `params`, `last_now`, `counters`, `anomalies`, `stats.done_minutes`.
    Scalars,
    /// A node's counters, life, wait, intent, marks, tags, `due_fired`, role state, (1.8)
    /// `touched_at` and `stale_fired`, (1.11) `retracted_decision`, and (1.12) `pr_named`.
    NodeHot(NodeId),
    /// A node's place, origin, texts, settings, and role definition.
    NodeContent(NodeId),
    /// One key of a node.
    Key(NodeId, KeyStr),
    /// One outgoing link of a node.
    Link(NodeId, LinkRel, NodeId),
    /// A live name.
    Name(NodeId, Name),
    /// A former name.
    Alias(NodeId, Name),
    /// A hold.
    Hold(Token),
    /// A feed entry's metadata.
    Entry(Eid),
    /// A request.
    Request(RequestId),
    /// A question to a person.
    PQuestion(QuestionId),
    /// A question between agents.
    AQuestion(QuestionId),
    /// A question for the user.
    UQuestion(QuestionId),
    /// A commitment.
    Commitment(CommitmentId),
    /// A `said` entry.
    Said(SaidId),
    /// A decision.
    Decision(DecisionId),
    /// An objection.
    Objection(ObjectionId),
    /// A `from_user` entry.
    UserEntry(UserEntryId),
    /// A digest entry.
    Digest(DigestId),
    /// An addressed message.
    Note(NoteId),
    /// The approvals of one node version.
    Approval(NodeId, Version),
    /// A retracted target.
    Retracted(BasisTarget),
    /// The refusals of one class on one node.
    Refusal(NodeId, ClassName),
    /// One caps profile of a class.
    ClassProfile(ClassName, BTreeSet<Label>),
    /// An open signal.
    Signal(SignalKey),
    /// An active pause.
    Pause(NodeId),
    /// A session.
    Session(SessionId),
    /// An interned principal.
    Principal(PrincipalKey),
    /// An `op_id` record.
    Op(ClassName, OpId),
    /// The statistics of one day.
    Day(u64),
    /// The session starts of one minute.
    Start(u64),
    /// One (caps, horizon bit length) profile.
    Profile(BTreeSet<Label>, u8),
    /// A node whose estimate is in the demand.
    DemandCounted(NodeId),
    /// When a candidate became one.
    Since(Cand),
    /// One principal's message times.
    UserMsgs(PrincipalId),
    /// When an entity leaves memory.
    Leaving(EntityRef),
    /// The digest entries of a principal that left the pending list unshown.
    DigestLeft(PrincipalId),
}

/// What a tracked job hands the writer (D.6.1, D.11.6): `facts`, every fact its events inserted,
/// changed, or removed, the prunes included; and `departed`, for each entity that left memory
/// during the job (D.4.12), its value at that moment, recorded before it was removed. The writer
/// stores that value in the entity's row with `resident = 0`, so that a later restore reads the
/// entity as it last was, also when the job changed it, or created it, before it left (a settle
/// batch spans the retain times after downtime, D.8.1, review E1-spec-1). A missing fact is a bug
/// the persist-and-reload test finds (D.12.6); a superfluous one only rewrites a row.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tracked {
    /// The facts the job touched.
    pub facts: BTreeSet<Fact>,
    /// The last value of every entity that left memory during the job, by its fact.
    pub departed: BTreeMap<Fact, Departed>,
}

impl Tracked {
    /// An empty tracker.
    #[must_use]
    pub fn new() -> Tracked {
        Tracked::default()
    }
}

/// The value an entity had when it left memory (D.4.12): what its not-resident row holds, and what
/// a restore brings back. One variant per kind of fact that leaves memory.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Departed {
    /// A request.
    Request(Request),
    /// A people question.
    PQuestion(PQuestion),
    /// A user question.
    UQuestion(UQuestion),
    /// A commitment.
    Commitment(Commitment),
    /// A `said` entry.
    Said(Said),
    /// A decision.
    Decision(Decision),
    /// An objection.
    Objection(Objection),
    /// A user entry.
    UserEntry(UserEntry),
    /// A digest entry.
    Digest(DigestEntry),
    /// Entry metadata.
    Meta(EntryMeta),
}

/// The value of fact `f` in `s` as a departed entity's (D.4.12): Some for a resident entity of a
/// kind that leaves memory (the entities of `EntityRef` and the entry metadata), None otherwise. The
/// oracle of `Tracked::departed` (D.12.3, item 11): an entity resident before a job of one instant
/// and gone after it left with its value before.
#[must_use]
pub fn departed_of(s: &State, f: &Fact) -> Option<Departed> {
    match f {
        Fact::Request(i) => s.requests.get(i).cloned().map(Departed::Request),
        Fact::PQuestion(i) => s.pquestions.get(i).cloned().map(Departed::PQuestion),
        Fact::UQuestion(i) => s.uquestions.get(i).cloned().map(Departed::UQuestion),
        Fact::Commitment(i) => s.commitments.get(i).cloned().map(Departed::Commitment),
        Fact::Said(i) => s.saids.get(i).cloned().map(Departed::Said),
        Fact::Decision(i) => s.decisions.get(i).cloned().map(Departed::Decision),
        Fact::Objection(i) => s.objections.get(i).cloned().map(Departed::Objection),
        Fact::UserEntry(i) => s.user_entries.get(i).cloned().map(Departed::UserEntry),
        Fact::Digest(i) => s.digest.get(i).cloned().map(Departed::Digest),
        Fact::Entry(e) => s.entries.get(e).copied().map(Departed::Meta),
        Fact::Scalars
        | Fact::NodeHot(_)
        | Fact::NodeContent(_)
        | Fact::Key(..)
        | Fact::Link(..)
        | Fact::Name(..)
        | Fact::Alias(..)
        | Fact::Hold(_)
        | Fact::AQuestion(_)
        | Fact::Note(_)
        | Fact::Approval(..)
        | Fact::Retracted(_)
        | Fact::Refusal(..)
        | Fact::ClassProfile(..)
        | Fact::Signal(_)
        | Fact::Pause(_)
        | Fact::Session(_)
        | Fact::Principal(_)
        | Fact::Op(..)
        | Fact::Day(_)
        | Fact::Start(_)
        | Fact::Profile(..)
        | Fact::DemandCounted(_)
        | Fact::Since(_)
        | Fact::UserMsgs(_)
        | Fact::Leaving(_)
        | Fact::DigestLeft(_) => None,
    }
}

/// The facts whose values differ between `before` and `after`, compared map by map over the base
/// and history parts (never the indices). The naive oracle of the dirty set (D.12.3, item 11).
#[must_use]
pub fn changed_facts(before: &State, after: &State) -> BTreeSet<Fact> {
    let mut out = BTreeSet::new();
    if before.params != after.params
        || before.last_now != after.last_now
        || before.counters != after.counters
        || before.anomalies != after.anomalies
        || before.stats.done_minutes != after.stats.done_minutes
    {
        out.insert(Fact::Scalars);
    }
    // Nodes: the hot row, the content row, the keys, and the links.
    for id in union_keys(&before.nodes, &after.nodes) {
        let (a, b) = (before.nodes.get(&id), after.nodes.get(&id));
        match (a, b) {
            (Some(a), Some(b)) => {
                if hot(a) != hot(b) {
                    out.insert(Fact::NodeHot(id));
                }
                if content(a) != content(b) {
                    out.insert(Fact::NodeContent(id));
                }
                diff_maps(&a.keys, &b.keys, &mut out, |k| Fact::Key(id, k.clone()));
                diff_sets(&a.links, &b.links, &mut out, |&(rel, to)| {
                    Fact::Link(id, rel, to)
                });
                // A key row's `open` column follows the node's life (D.11.6).
                if a.is_open() != b.is_open() {
                    for k in a.keys.keys().chain(b.keys.keys()) {
                        out.insert(Fact::Key(id, k.clone()));
                    }
                }
            }
            (Some(x), None) | (None, Some(x)) => {
                out.insert(Fact::NodeHot(id));
                out.insert(Fact::NodeContent(id));
                for k in x.keys.keys() {
                    out.insert(Fact::Key(id, k.clone()));
                }
                for &(rel, to) in &x.links {
                    out.insert(Fact::Link(id, rel, to));
                }
            }
            (None, None) => {}
        }
    }
    diff_maps(&before.names, &after.names, &mut out, |(p, n)| {
        Fact::Name(*p, n.clone())
    });
    diff_maps(&before.aliases, &after.aliases, &mut out, |(p, n)| {
        Fact::Alias(*p, n.clone())
    });
    diff_maps(&before.entries, &after.entries, &mut out, |e| {
        Fact::Entry(*e)
    });
    diff_maps(&before.holds, &after.holds, &mut out, |t| Fact::Hold(*t));
    diff_maps(&before.requests, &after.requests, &mut out, |r| {
        Fact::Request(*r)
    });
    diff_maps(&before.pquestions, &after.pquestions, &mut out, |q| {
        Fact::PQuestion(*q)
    });
    diff_maps(&before.aquestions, &after.aquestions, &mut out, |q| {
        Fact::AQuestion(*q)
    });
    diff_maps(&before.uquestions, &after.uquestions, &mut out, |q| {
        Fact::UQuestion(*q)
    });
    diff_maps(&before.commitments, &after.commitments, &mut out, |c| {
        Fact::Commitment(*c)
    });
    diff_maps(&before.saids, &after.saids, &mut out, |x| Fact::Said(*x));
    diff_maps(&before.decisions, &after.decisions, &mut out, |d| {
        Fact::Decision(*d)
    });
    diff_maps(&before.objections, &after.objections, &mut out, |o| {
        Fact::Objection(*o)
    });
    diff_maps(&before.user_entries, &after.user_entries, &mut out, |e| {
        Fact::UserEntry(*e)
    });
    diff_maps(&before.digest, &after.digest, &mut out, |d| {
        Fact::Digest(*d)
    });
    diff_maps(&before.notes, &after.notes, &mut out, |x| Fact::Note(*x));
    diff_maps(&before.approvals, &after.approvals, &mut out, |(n, v)| {
        Fact::Approval(*n, *v)
    });
    diff_sets(&before.retracted, &after.retracted, &mut out, |t| {
        Fact::Retracted(*t)
    });
    diff_maps(&before.refusals, &after.refusals, &mut out, |(n, c)| {
        Fact::Refusal(*n, c.clone())
    });
    for c in union_keys(&before.classes, &after.classes) {
        let empty = BTreeMap::new();
        let a = before.classes.get(&c).map_or(&empty, |x| &x.caps_seen);
        let b = after.classes.get(&c).map_or(&empty, |x| &x.caps_seen);
        diff_maps(a, b, &mut out, |caps| {
            Fact::ClassProfile(c.clone(), caps.clone())
        });
    }
    diff_maps(&before.signals, &after.signals, &mut out, |k| {
        Fact::Signal(*k)
    });
    diff_maps(&before.pauses, &after.pauses, &mut out, |n| Fact::Pause(*n));
    diff_maps(&before.sessions, &after.sessions, &mut out, |s| {
        Fact::Session(*s)
    });
    diff_maps(&before.principal_ids, &after.principal_ids, &mut out, |k| {
        Fact::Principal(k.clone())
    });
    diff_maps(&before.ops, &after.ops, &mut out, |(c, o)| {
        Fact::Op(c.clone(), o.clone())
    });
    diff_maps(&before.stats.days, &after.stats.days, &mut out, |d| {
        Fact::Day(*d)
    });
    diff_maps(&before.stats.starts, &after.stats.starts, &mut out, |m| {
        Fact::Start(*m)
    });
    diff_maps(
        &before.stats.profiles,
        &after.stats.profiles,
        &mut out,
        |(caps, bits)| Fact::Profile(caps.clone(), *bits),
    );
    diff_sets(
        &before.stats.demand_counted,
        &after.stats.demand_counted,
        &mut out,
        |n| Fact::DemandCounted(*n),
    );
    diff_maps(&before.since, &after.since, &mut out, |c| Fact::Since(*c));
    diff_maps(&before.user_msgs, &after.user_msgs, &mut out, |p| {
        Fact::UserMsgs(*p)
    });
    diff_maps(&before.leaving, &after.leaving, &mut out, |x| {
        Fact::Leaving(*x)
    });
    diff_maps(&before.digest_left, &after.digest_left, &mut out, |p| {
        Fact::DigestLeft(*p)
    });
    out
}

/// The fields of a node's hot row (D.4.3), with the role's state.
type Hot<'a> = (
    [u64; 10],
    (
        Option<Eid>,
        Option<crate::state::ExpiredHold>,
        &'a crate::state::Life,
        &'a Option<crate::state::Wait>,
        &'a Option<crate::state::Intent>,
        &'a BTreeSet<QuestionId>,
        &'a BTreeSet<QuestionId>,
        &'a BTreeSet<Label>,
        bool,
        RoleState,
    ),
    (Option<crate::types::Time>, bool, Option<DecisionId>, bool),
);

/// The state part of a role (D.4.3): an occurrence's phase and window marker, a series'
/// `last_missed`.
#[derive(PartialEq, Eq)]
enum RoleState {
    None,
    Series(bool),
    Occurrence(crate::state::OccPhase, bool),
}

fn role_state(r: &Role) -> RoleState {
    match r {
        Role::Plain | Role::Signal(_) => RoleState::None,
        Role::Series(x) => RoleState::Series(x.last_missed),
        Role::Occurrence(o) => RoleState::Occurrence(o.phase, o.window_fired),
    }
}

fn hot(n: &Node) -> Hot<'_> {
    (
        [
            n.version.0,
            n.digest_version.0,
            n.content_version.0,
            n.fence.0,
            n.seq.0,
            u64::from(n.attempts),
            u64::from(n.checkpoints),
            n.held_ms,
            n.people_left.0,
            u64::from(n.left),
        ],
        (
            n.last_checkpoint,
            n.expired_hold,
            &n.life,
            &n.wait,
            &n.open_intent,
            &n.recheck_marks,
            &n.assume_marks,
            &n.tags,
            n.due_fired,
            role_state(&n.role),
        ),
        (
            n.touched_at,
            n.stale_fired,
            n.retracted_decision,
            n.pr_named,
        ),
    )
}

/// A node with its hot fields, keys, and links reset: what is left is its content row.
fn content(n: &Node) -> Node {
    let mut c = n.clone();
    c.version = Version::default();
    c.digest_version = Version::default();
    c.content_version = Version::default();
    c.fence = crate::types::Fence::default();
    c.seq = crate::types::Seq::default();
    c.attempts = 0;
    c.checkpoints = 0;
    c.held_ms = 0;
    c.people_left = crate::types::Seq::default();
    c.left = 0;
    c.last_checkpoint = None;
    c.expired_hold = None;
    c.life = crate::state::Life::Open;
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
            o.phase = crate::state::OccPhase::Waiting;
            o.window_fired = false;
        }
    }
    c
}

fn union_keys<K: Ord + Clone, V>(a: &BTreeMap<K, V>, b: &BTreeMap<K, V>) -> BTreeSet<K> {
    a.keys().chain(b.keys()).cloned().collect()
}

fn diff_maps<K: Ord, V: PartialEq>(
    a: &BTreeMap<K, V>,
    b: &BTreeMap<K, V>,
    out: &mut BTreeSet<Fact>,
    fact: impl Fn(&K) -> Fact,
) {
    for (k, v) in a {
        if b.get(k) != Some(v) {
            out.insert(fact(k));
        }
    }
    for k in b.keys() {
        if !a.contains_key(k) {
            out.insert(fact(k));
        }
    }
}

fn diff_sets<K: Ord>(
    a: &BTreeSet<K>,
    b: &BTreeSet<K>,
    out: &mut BTreeSet<Fact>,
    fact: impl Fn(&K) -> Fact,
) {
    for k in a.symmetric_difference(b) {
        out.insert(fact(k));
    }
}
