//! `evolve` (design D.7.4, D.7.5): one event, one transition of the state.
//!
//! `evolve(s, e)`:
//! 1. `last_now := max(last_now, e.at)`, `counters.events += 1`, then the prunes: the `op_id` records
//!    whose expiry has come, the notes whose `until` has come (with an agent question's
//!    `AQuestion`), the entities whose `leaving` time has come (D.4.12), and the statistics' days
//!    outside `capacity.window`. Each is monotone in `at`, so it commutes with T2.
//! 2. Checks the event's precondition (the table of D.7.4). If it fails, `anomalies += 1` and the base
//!    change is skipped, except that an id the event carries still advances its counter.
//! 3. Applies the base change.
//! 4. Refreshes the derived indices: the structural indices of the event's own objects in the arm,
//!    then the basis refresh, the deadline sync, the flag and candidate refresh of every touched
//!    node in ascending id order, the routing items, and the leaving times (D.4.12).
//!
//! `evolve_tracked` also collects every fact (D.6.1) that steps 1 to 4 changed in the base and
//! history parts; `evolve_mut` is the same code with no set. `evolve` is total: it never panics,
//! arithmetic saturates, and every loop is bounded by a collection (D.7.5).
//!
//! 1.4 (D.13.1, WP-D1.4): `HoldIssued` stores `Hold::reopened`; `Appended(ToUser)` inserts its
//! principal into the author session's `told`; the flag and candidate refresh keeps
//! `ix.next.waiting`; a hold that expires without a checkpoint is the node's `expired_hold` until its
//! next checkpoint; `create` and `update` stamp `agreements_at`. The Silence key at the end of the
//! session's last lease (R27): `HoldIssued`, `HoldExtended`, and `HoldEnded` sync the owner
//! `Session(h.session)`. While tracking, every entity that leaves memory is recorded with its last
//! value (`Tracked::departed`) before it is removed, so the writer can store it (review E1-spec-1).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::deadlines::{deadline_of, sync_owner};
use crate::event::{EntryBody, Ev, Event, HoldEnd, NodeInit, NodePatch, QuestionBody, SystemNote};
use crate::facts::{Departed, Fact, Tracked};
use crate::flags::{resolve_merged, stale_decision};
use crate::health::DONE_RING;
use crate::index::{
    Dirt, add_live, bears_entity, bears_none, day_of, fix_depths, fix_person, home_of,
    horizon_bits, ix_aquestion, ix_commitment, ix_decision, ix_digest, ix_hold, ix_meta,
    ix_node_own, ix_note, ix_objection, ix_pquestion, ix_request, ix_said, ix_session,
    ix_uquestion, ix_user_entry, minute_of, node_contrib, people_entry, put, put_in, refresh_node,
    refresh_route, shift_subtree_live, subtree,
};
use crate::state::{
    AQuestion, AqState, Author, BasisRef, BasisTarget, Cand, ClassActivity, Closure, Commitment,
    CommitmentState, DeadlineKind, Decision, DecisionState, DigestEntry, DigestState, EntityRef,
    EntryMeta, FINISHED_RETAIN, Hold, Intent, Lapse, Life, Node, NodeIx, Note, Objection,
    ObjectionState, OccPhase, OpRecord, Origin, Owner, PQuestion, Pause, Place, PqState,
    ProfileSeen, Refusal, RefusalEntry, Request, Restored, Role, Routing, Said, SaidState, Series,
    Session, Settlement, State, UQuestion, UqSource, UqState, UserEntry, Wait, WaitKey, WaitOn,
};
use crate::types::{
    Addressee, Change, ClassName, DecisionId, Dur, Eid, Fence, KeyStr, Label, Name, NodeId,
    NormVer, OFFER, Outcome, PEOPLE, PrincipalId, ROOT, SIGNALS, SYS, Seq, SessionId, TEAMS, Text,
    Time, USER, USER_DECISIONS, USER_DIGEST, USER_PRINCIPAL, UserEntryId, Version,
};
use crate::user::DIGEST_RETAIN;

/// Total: never panics, whatever the event and the state (D.7.5).
#[must_use]
pub fn evolve(mut s: State, e: &Event) -> State {
    evolve_mut(&mut s, e);
    s
}

/// `evolve` in place.
pub fn evolve_mut(s: &mut State, e: &Event) {
    run(s, e, Dirt(None));
}

/// `evolve_mut` that also adds to `dirty` every fact the event inserted, changed, or removed, the
/// prunes included (D.6.1); the shell writes each one's current value, or deletes it, in the
/// transaction of the event (D.11.6).
pub fn evolve_tracked(s: &mut State, e: &Event, dirty: &mut Tracked) {
    run(s, e, Dirt(Some(dirty)));
}

/// Brings entities that left memory back before a command that names them (D.4.12, D.6.1): `items`
/// are the shell's decoded state rows; an item whose id is not below `next_eid` is dropped, and so
/// is a second item of the same entity. An item that is resident is dropped too, unless its
/// leaving time has come by `now`: no event has pruned it yet (nothing was due since), and the
/// command's first event would, so the next event that names it would fail its precondition. Such
/// an item is kept, with the value of its row, which is current: the `Restored` event's step 1
/// prunes it, and its arm re-inserts it with a new leaving time (D.4.12's fix of the restore plan,
/// WP-D6). Emits one `Restored` event at `now` (clamped to `last_now`, as `apply` does), or none.
pub fn restore_tracked(
    s: &mut State,
    now: Time,
    items: Vec<Restored>,
    dirty: &mut Tracked,
) -> Vec<Event> {
    let now = now.max(s.last_now);
    let mut seen: BTreeSet<EntityRef> = BTreeSet::new();
    let kept: Vec<Restored> = items
        .into_iter()
        .filter(|x| {
            (restorable(s, x) || leaving_due(s, restored_ref(x), now))
                && seen.insert(restored_ref(x))
        })
        .collect();
    if kept.is_empty() {
        return Vec::new();
    }
    let ev = Event {
        at: now,
        ev: Ev::Restored { items: kept },
    };
    evolve_tracked(s, &ev, dirty);
    alloc::vec![ev]
}

/// Whether entity `x` is resident with its leaving time at or before `now` (D.4.12): the first
/// event at `now` prunes it. The shell's restore plan treats such an entity as departed.
#[must_use]
pub fn leaving_due(s: &State, x: EntityRef, now: Time) -> bool {
    s.leaving.get(&x).is_some_and(|t| *t <= now)
}

/// The entity a restored item brings back.
fn restored_ref(x: &Restored) -> EntityRef {
    match x {
        Restored::Request(id, ..) => EntityRef::Request(*id),
        Restored::PQuestion(id, ..) => EntityRef::PQuestion(*id),
        Restored::UQuestion(id, ..) => EntityRef::UQuestion(*id),
        Restored::Commitment(id, ..) => EntityRef::Commitment(*id),
        Restored::Said(id, ..) => EntityRef::Said(*id),
        Restored::Decision(id, ..) => EntityRef::Decision(*id),
        Restored::Objection(id, ..) => EntityRef::Objection(*id),
        Restored::UserEntry(id, ..) => EntityRef::UserEntry(*id),
        Restored::Digest(id, ..) => EntityRef::Digest(*id),
        Restored::Meta(id, _) => EntityRef::Meta(*id),
    }
}

/// `Restored`'s precondition for one item: absent from its map, and its id below `next_eid`.
fn restorable(s: &State, x: &Restored) -> bool {
    let id = match restored_ref(x) {
        EntityRef::Request(i)
        | EntityRef::PQuestion(i)
        | EntityRef::UQuestion(i)
        | EntityRef::Commitment(i)
        | EntityRef::Said(i)
        | EntityRef::Decision(i)
        | EntityRef::Objection(i)
        | EntityRef::UserEntry(i)
        | EntityRef::Digest(i)
        | EntityRef::Meta(i) => i,
    };
    if id >= s.counters.next_eid {
        return false;
    }
    match x {
        Restored::Request(i, ..) => !s.requests.contains_key(i),
        Restored::PQuestion(i, ..) => !s.pquestions.contains_key(i),
        Restored::UQuestion(i, ..) => !s.uquestions.contains_key(i),
        Restored::Commitment(i, ..) => !s.commitments.contains_key(i),
        Restored::Said(i, ..) => !s.saids.contains_key(i),
        Restored::Decision(i, ..) => !s.decisions.contains_key(i),
        Restored::Objection(i, ..) => !s.objections.contains_key(i),
        Restored::UserEntry(i, ..) => !s.user_entries.contains_key(i),
        Restored::Digest(i, ..) => !s.digest.contains_key(i),
        Restored::Meta(i, _) => !s.entries.contains_key(i),
    }
}

fn run(s: &mut State, e: &Event, dirt: Dirt<'_>) {
    let events_before = s.counters.events;
    let old_last = s.last_now;
    s.last_now = s.last_now.max(e.at);
    s.counters.events = s.counters.events.saturating_add(1);
    let mut cx = Cx {
        s,
        at: e.at,
        dirt,
        nodes: BTreeSet::new(),
        owners: BTreeSet::new(),
        restale: BTreeSet::new(),
        routes: BTreeSet::new(),
        leave: BTreeSet::new(),
    };
    cx.dirt.mark(|| Fact::Scalars);
    cx.prune();
    if cx.s.last_now > old_last {
        cx.silence_floor();
    }
    if !cx.apply(&e.ev, events_before) {
        cx.s.anomalies = cx.s.anomalies.saturating_add(1);
    }
    cx.finish();
}

/// One event in progress: the state, the event's time, the dirty facts, and what the arm asks the
/// refresh of step 4 to recompute.
struct Cx<'a, 'd> {
    s: &'a mut State,
    at: Time,
    dirt: Dirt<'d>,
    /// Nodes whose flags and candidates are recomputed.
    nodes: BTreeSet<NodeId>,
    /// Deadline owners whose keys are re-read.
    owners: BTreeSet<Owner>,
    /// Decisions whose staleness is recomputed (the basis refresh).
    restale: BTreeSet<DecisionId>,
    /// User entries whose routing item is recomputed.
    routes: BTreeSet<UserEntryId>,
    /// Entities whose leaving time is recomputed.
    leave: BTreeSet<EntityRef>,
}

/// A node with every field at its start: open, versions 1, no content, no settings.
fn blank_node(place: Option<Place>, origin: Origin, at: Time) -> Node {
    Node {
        place,
        origin,
        created_at: at,
        title: Text::default(),
        body: Text::default(),
        fields: alloc::collections::BTreeMap::new(),
        files: alloc::collections::BTreeMap::new(),
        status: Text::default(),
        digest: Text::default(),
        agreements: Text::default(),
        agreements_at: None,
        kind: None,
        tags: BTreeSet::new(),
        capacity: 0,
        priority: 0,
        est: None,
        needs: BTreeSet::new(),
        due: None,
        due_fired: false,
        owners: BTreeSet::new(),
        keys: alloc::collections::BTreeMap::new(),
        links: BTreeSet::new(),
        depends_on: BTreeSet::new(),
        wait: None,
        role: Role::Plain,
        life: Life::Open,
        version: Version(1),
        digest_version: Version(1),
        fence: Fence(0),
        seq: Seq(0),
        content_version: Version(1),
        attempts: 0,
        checkpoints: 0,
        last_checkpoint: None,
        expired_hold: None,
        open_intent: None,
        recheck_marks: BTreeSet::new(),
        assume_marks: BTreeSet::new(),
        held_ms: 0,
        people_left: Seq(0),
        left: 0,
        touched_at: None,
        stale_fired: false,
        retracted_decision: None,
        pr_named: false,
    }
}

/// A node as `NodeCreated` makes it from `init` (D.7.4): counters at their start, open, no files,
/// `due_fired := due <= at`.
fn node_from_init(place: Place, origin: Origin, at: Time, init: &NodeInit) -> Node {
    Node {
        title: init.title.clone(),
        body: init.body.clone(),
        fields: init.fields.clone(),
        status: init.status.clone(),
        digest: init.digest.clone(),
        agreements: init.agreements.clone(),
        agreements_at: (!init.agreements.is_empty()).then_some(at),
        kind: init.kind.clone(),
        tags: init.tags.clone(),
        capacity: init.capacity,
        priority: init.priority,
        est: init.est,
        needs: init.needs.clone(),
        due: init.due,
        due_fired: init.due.is_some_and(|d| d <= at),
        owners: init.owners.clone(),
        keys: init.keys.clone(),
        links: init.links.clone(),
        depends_on: init.depends_on.clone(),
        role: init.role.clone(),
        ..blank_node(Some(place), origin, at)
    }
}

/// A reserved node as Genesis makes it (D.7.4): origin `Reserved`, open, capacity 0, title = name
/// (ROOT's is empty), versions 1, no owners.
fn reserved_node(place: Option<Place>, at: Time) -> Node {
    let title = place.as_ref().map_or_else(Text::default, |p| {
        Text(alloc::string::String::from(p.name.as_str()))
    });
    Node {
        title,
        ..blank_node(place, Origin::Reserved, at)
    }
}

/// The session that wrote an entry: the author's, or `SessionId(0)` for the server (which writes
/// no `said`, user entry, or agent question).
fn author_session(by: Author) -> SessionId {
    match by {
        Author::Session(s) => s,
        Author::Server => SessionId(0),
    }
}

/// `same_person(p, q)` of D.8.11 over the directory's `person_of`.
fn same_person(s: &State, p: PrincipalId, q: PrincipalId) -> bool {
    p == q
        || matches!(
            (s.ix.person_of.get(&p), s.ix.person_of.get(&q)),
            (Some(a), Some(b)) if a == b
        )
}

impl Cx<'_, '_> {
    fn hot(&mut self, n: NodeId) {
        self.dirt.mark(|| Fact::NodeHot(n));
    }

    fn content(&mut self, n: NodeId) {
        self.dirt.mark(|| Fact::NodeContent(n));
    }

    fn refresh(&mut self, n: NodeId) {
        self.nodes.insert(n);
    }

    fn refresh_chain(&mut self, n: NodeId) {
        self.nodes.insert(n);
        for a in crate::index::ancestors(self.s, n) {
            self.nodes.insert(a);
        }
    }

    fn owner(&mut self, o: Owner) {
        self.owners.insert(o);
    }

    /// 1.8 (§5, D.7.4): node `n` was touched at the event's time: `touched_at := at`, and the
    /// Stale deadline's marker clears, so the deadline moves to `at + offer.stale`. Without
    /// `offer.stale`, in a history written before 1.8, nothing is tracked (D.7.6), and the node is
    /// only refreshed.
    fn touch(&mut self, n: NodeId) {
        let at = self.at;
        let tracked = self.s.params.offer_stale.is_some();
        if tracked && let Some(node) = self.s.nodes.get_mut(&n) {
            node.touched_at = Some(at);
            node.stale_fired = false;
        }
        self.hot(n);
        self.owner(Owner::Node(n));
        self.refresh(n);
    }

    fn leave(&mut self, x: EntityRef) {
        self.leave.insert(x);
    }

    fn interned(&self, p: PrincipalId) -> bool {
        self.s.ix.principal_key.contains_key(&p)
    }

    /// The nodes that wait on `k` (`ix.waiters`).
    fn refresh_waiters(&mut self, k: WaitKey) {
        let ws: Vec<NodeId> = self
            .s
            .ix
            .waiters
            .get(&k)
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        for w in ws {
            self.refresh(w);
        }
    }

    /// The flagged occurrences of `n`, whose classification depends on `n` being an open series
    /// (`occ_locked`, D.4.3, D.7.4), wherever they are: `ix.flagged_occurrences[n]`. An occurrence
    /// moved elsewhere is no longer among `n`'s children (WP-D6's full-universe proptest found one
    /// that stayed out of `next` after its series closed), so `flagged_children` would miss it; the
    /// cost stays bounded by the series' flagged occurrences (§18).
    fn refresh_locked(&mut self, n: NodeId) {
        let cs: Vec<NodeId> = self
            .s
            .ix
            .flagged_occurrences
            .get(&n)
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        for c in cs {
            self.refresh(c);
        }
    }

    /// The candidates under `scope` (a pause set or lifted): the cheaper of a walk of the scope's
    /// subtree and a pass over `ix.next.of` (D.8.21, CS-16).
    fn refresh_under(&mut self, scope: NodeId) {
        let sub = self.s.ix.node.get(&scope).map_or(0, |x| {
            x.sub
                .open
                .saturating_add(x.sub.done)
                .saturating_add(x.sub.failed)
                .saturating_add(x.sub.other)
        });
        let cands = u64::try_from(self.s.ix.next.of.len()).unwrap_or(u64::MAX);
        let found: Vec<NodeId> = if sub < cands {
            subtree(self.s, scope)
        } else {
            self.s
                .ix
                .next
                .of
                .keys()
                .filter_map(|c| match c {
                    Cand::Node(n) | Cand::Status(n) => Some(*n),
                    Cand::Route(_) => None,
                })
                .filter(|&n| n == scope || crate::index::ancestors(self.s, n).contains(&scope))
                .collect()
        };
        for n in found {
            self.refresh(n);
        }
    }

    // --------------------------------------------------------------------------------------------
    // Step 1: the prunes

    fn prune(&mut self) {
        let at = self.at;
        // op_id records (CS-8).
        while let Some(first) = self.s.ix.ops_expiry.first().cloned() {
            if first.0 > at {
                break;
            }
            self.s.ix.ops_expiry.remove(&first);
            let key = (first.1.clone(), first.2.clone());
            if self.s.ops.remove(&key).is_some() {
                self.dirt.mark(|| Fact::Op(first.1, first.2));
            }
        }
        // Notes, with an agent question's AQuestion, and their entry's metadata (CS-17).
        while let Some(&(until, x)) = self.s.ix.notes_until.first() {
            if until > at {
                break;
            }
            self.s.ix.notes_until.remove(&(until, x));
            if self.s.notes.contains_key(&x) {
                ix_note(self.s, x, false);
                self.s.notes.remove(&x);
                self.dirt.mark(|| Fact::Note(x));
            }
            if self.s.aquestions.contains_key(&x) {
                ix_aquestion(self.s, x, false);
                self.s.aquestions.remove(&x);
                self.dirt.mark(|| Fact::AQuestion(x));
            }
            self.meta_after(x);
        }
        // Entities that leave memory (D.4.12).
        while let Some(&(t, x)) = self.s.ix.leaving_by_time.first() {
            if t > at {
                break;
            }
            self.s.ix.leaving_by_time.remove(&(t, x));
            if self.s.leaving.get(&x) == Some(&t) {
                self.s.leaving.remove(&x);
                self.dirt.mark(|| Fact::Leaving(x));
                self.leave_memory(x);
            }
        }
        // The statistics' days within capacity.window of `at` (D.4.9).
        let cutoff = day_of(at.minus(self.s.params.capacity_window));
        while let Some((&d, _)) = self.s.stats.days.first_key_value() {
            if d >= cutoff {
                break;
            }
            self.s.stats.days.remove(&d);
            self.dirt.mark(|| Fact::Day(d));
        }
    }

    /// The Silence keys that lie before the new `last_now` move to it (D.4.8: the key is floored at
    /// `last_now`). On a settled state there are none.
    fn silence_floor(&mut self) {
        let last = self.s.last_now;
        let mut late = Vec::new();
        for k in &self.s.ix.deadlines {
            if k.at >= last {
                break;
            }
            if k.kind == DeadlineKind::Silence {
                late.push(k.owner.clone());
            }
        }
        for o in late {
            self.owner(o);
        }
    }

    /// Removes an entity whose leaving time has come, with every index, raising its home's `left`
    /// (D.4.12).
    fn leave_memory(&mut self, x: EntityRef) {
        if let Some(home) = home_of(self.s, x)
            && let Some(n) = self.s.nodes.get_mut(&home)
        {
            n.left = n.left.saturating_add(1);
            self.hot(home);
        }
        match x {
            EntityRef::Request(r) => {
                let answered = self.s.requests.get(&r).and_then(|q| q.answered_by);
                ix_request(self.s, r, false);
                if let Some(v) = self.s.requests.remove(&r) {
                    self.dirt.depart(Fact::Request(r), || Departed::Request(v));
                }
                self.meta_after(r);
                if let Some(a) = answered {
                    self.leave(EntityRef::Said(a));
                    self.leave(EntityRef::Meta(a));
                }
            }
            EntityRef::PQuestion(q) => {
                ix_pquestion(self.s, q, false);
                if let Some(v) = self.s.pquestions.remove(&q) {
                    self.dirt
                        .depart(Fact::PQuestion(q), || Departed::PQuestion(v));
                }
                self.meta_after(q);
                let escalations: Vec<Eid> = self
                    .s
                    .uquestions
                    .iter()
                    .filter(|(_, u)| u.source == UqSource::Escalation { question: q })
                    .map(|(&u, _)| u)
                    .collect();
                for u in escalations {
                    self.leave(EntityRef::UQuestion(u));
                }
            }
            EntityRef::UQuestion(q) => {
                ix_uquestion(self.s, q, false);
                if let Some(v) = self.s.uquestions.remove(&q) {
                    self.dirt
                        .depart(Fact::UQuestion(q), || Departed::UQuestion(v));
                }
                self.meta_after(q);
            }
            EntityRef::Commitment(c) => {
                ix_commitment(self.s, c, false);
                if let Some(v) = self.s.commitments.remove(&c) {
                    self.dirt
                        .depart(Fact::Commitment(c), || Departed::Commitment(v));
                }
                self.meta_after(c);
            }
            EntityRef::Said(a) => {
                ix_said(self.s, a, false);
                if let Some(v) = self.s.saids.remove(&a) {
                    self.dirt.depart(Fact::Said(a), || Departed::Said(v));
                }
                self.meta_after(a);
            }
            EntityRef::Decision(d) => {
                ix_decision(self.s, d, false);
                if let Some(v) = self.s.decisions.remove(&d) {
                    self.dirt
                        .depart(Fact::Decision(d), || Departed::Decision(v));
                }
                self.meta_after(d);
            }
            EntityRef::Objection(o) => {
                ix_objection(self.s, o, false);
                if let Some(v) = self.s.objections.remove(&o) {
                    self.dirt
                        .depart(Fact::Objection(o), || Departed::Objection(v));
                }
                self.meta_after(o);
            }
            EntityRef::UserEntry(e) => {
                ix_user_entry(self.s, e, false);
                if let Some(v) = self.s.user_entries.remove(&e) {
                    self.dirt
                        .depart(Fact::UserEntry(e), || Departed::UserEntry(v));
                }
                self.meta_after(e);
            }
            EntityRef::Digest(d) => {
                if let Some(x) = self.s.digest.get(&d).copied()
                    && x.state == DigestState::Pending
                {
                    let c = self.s.digest_left.entry(x.to).or_insert(0);
                    *c = c.saturating_add(1);
                    self.dirt.mark(|| Fact::DigestLeft(x.to));
                }
                ix_digest(self.s, d, false);
                if let Some(v) = self.s.digest.remove(&d) {
                    self.dirt.depart(Fact::Digest(d), || Departed::Digest(v));
                }
                self.meta_after(d);
            }
            EntityRef::Meta(e) => self.drop_meta(e),
        }
    }

    /// After an entity born from entry `e` left: the entry's metadata leaves too once it bears no
    /// resident entity, unless it is of a type that bears none (those leave by their own time).
    fn meta_after(&mut self, e: Eid) {
        let Some(m) = self.s.entries.get(&e).copied() else {
            return;
        };
        if bears_none(m.ty) || bears_entity(self.s, e) {
            return;
        }
        self.drop_meta(e);
    }

    fn drop_meta(&mut self, e: Eid) {
        let Some(m) = self.s.entries.get(&e).copied() else {
            return;
        };
        ix_meta(self.s, e, false);
        self.s.entries.remove(&e);
        self.dirt.depart(Fact::Entry(e), || Departed::Meta(m));
        if people_entry(m.ty)
            && let Some(n) = self.s.nodes.get_mut(&m.node)
            && m.seq > n.people_left
        {
            n.people_left = m.seq;
            self.hot(m.node);
        }
        if let Some(t) = self.s.leaving.remove(&EntityRef::Meta(e)) {
            self.s.ix.leaving_by_time.remove(&(t, EntityRef::Meta(e)));
            self.dirt.mark(|| Fact::Leaving(EntityRef::Meta(e)));
        }
    }

    // --------------------------------------------------------------------------------------------
    // Step 4: the refresh

    fn finish(&mut self) {
        // The basis refresh (D-4).
        let restale: Vec<DecisionId> = core::mem::take(&mut self.restale).into_iter().collect();
        for d in restale {
            let Some(dec) = self.s.decisions.get(&d) else {
                continue;
            };
            let node = dec.node;
            let stale = stale_decision(self.s, dec);
            if dec.state == DecisionState::Active {
                let it = self.s.ix.items.entry(node).or_default();
                put(&mut it.stale_basis, d, stale);
                if *it == crate::state::NodeItems::default() {
                    self.s.ix.items.remove(&node);
                }
            }
            self.refresh(node);
        }
        // The deadline sync.
        let owners: Vec<Owner> = core::mem::take(&mut self.owners).into_iter().collect();
        for o in owners {
            sync_owner(self.s, &o);
        }
        // The flag and candidate refresh, in ascending id order. 1.11 (§10.4): a series is held
        // back while its open occurrence waits, so a refreshed occurrence refreshes its series.
        let mut refreshed = core::mem::take(&mut self.nodes);
        let series: Vec<NodeId> = refreshed
            .iter()
            .filter_map(|n| match self.s.nodes.get(n).map(|x| &x.role) {
                Some(Role::Occurrence(o)) => Some(o.series),
                _ => None,
            })
            .filter(|x| self.s.nodes.contains_key(x))
            .collect();
        refreshed.extend(series);
        let nodes: Vec<NodeId> = refreshed.into_iter().collect();
        for n in nodes {
            refresh_node(self.s, n, self.at, &mut self.dirt);
        }
        let routes: Vec<UserEntryId> = core::mem::take(&mut self.routes).into_iter().collect();
        for e in routes {
            refresh_route(self.s, e, self.at, &mut self.dirt);
        }
        // Leaving (D.4.12).
        let leave: Vec<EntityRef> = core::mem::take(&mut self.leave).into_iter().collect();
        for x in leave {
            self.sync_leaving(x);
        }
    }

    /// Sets or clears `leaving[x]` from the entity's state (D.4.12): `FINISHED_RETAIN` from now for a
    /// resident entity that is finished and not held back, `DIGEST_RETAIN` from now for a pending
    /// digest entry; an existing time is kept.
    fn sync_leaving(&mut self, x: EntityRef) {
        let want = leaving_retain(self.s, x);
        let have = self.s.leaving.get(&x).copied();
        match (want, have) {
            (Some(d), None) => {
                let t = self.at.plus(d);
                self.s.leaving.insert(x, t);
                self.s.ix.leaving_by_time.insert((t, x));
                self.dirt.mark(|| Fact::Leaving(x));
            }
            (None, Some(t)) => {
                self.s.leaving.remove(&x);
                self.s.ix.leaving_by_time.remove(&(t, x));
                self.dirt.mark(|| Fact::Leaving(x));
            }
            (Some(_), Some(_)) | (None, None) => {}
        }
    }

    /// Clears `leaving[x]`, so that the next sync sets it from now (a digest entry shown).
    fn reset_leaving(&mut self, x: EntityRef) {
        if let Some(t) = self.s.leaving.remove(&x) {
            self.s.ix.leaving_by_time.remove(&(t, x));
            self.dirt.mark(|| Fact::Leaving(x));
        }
        self.leave(x);
    }

    // --------------------------------------------------------------------------------------------
    // Steps 2 and 3: the arms

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per event variant, the table of D.7.4"
    )]
    fn apply(&mut self, ev: &Ev, events_before: u64) -> bool {
        match ev {
            Ev::Genesis { params } => self.genesis(params, events_before),
            Ev::ParamsChanged { params } => {
                self.s.params = params.clone();
                let all: Vec<NodeId> = self.s.nodes.keys().copied().collect();
                let at = self.at;
                let params = self.s.params.clone();
                for n in all {
                    // 1.8: the Stale marker follows the new `offer.stale`, so that no Stale key
                    // lies at or before `at` and none is lost (T4).
                    if let Some(node) = self.s.nodes.get_mut(&n)
                        && node.is_open()
                    {
                        let fired = params.stale_at(node.touched_at).is_some_and(|t| t <= at);
                        if node.stale_fired != fired {
                            node.stale_fired = fired;
                            self.hot(n);
                        }
                    }
                    self.owner(Owner::Node(n));
                    self.refresh(n);
                }
                true
            }
            Ev::Fired { kind, owner } => self.fired(*kind, owner),
            Ev::SessionOpened {
                session,
                class,
                caps,
                horizon,
                intent: _,
                silence_at,
                retire_at,
                principal,
            } => self.session_opened(
                *session,
                class,
                caps,
                *horizon,
                *silence_at,
                *retire_at,
                *principal,
            ),
            Ev::SessionHello {
                session,
                caps,
                horizon,
                intent: _,
                principal,
            } => self.session_hello(*session, caps.as_ref(), *horizon, *principal),
            Ev::PrincipalSeen { id, key } => self.principal_seen(*id, key),
            Ev::Touched {
                session,
                silence_at,
                retire_at,
            } => self.touched(*session, *silence_at, *retire_at),
            Ev::SessionRetired { session } => self.session_retired(*session),
            Ev::SessionStatus { session, text: _ } => self.s.sessions.contains_key(session),
            Ev::InboxRead { session, notes } => self.inbox_read(*session, notes),
            Ev::Shown { session, node, seq } => self.shown(*session, *node, *seq),
            Ev::NextCalled {
                session,
                outcome,
                open,
                class_window_end,
            } => self.next_called(*session, *outcome, *open, *class_window_end),
            Ev::NodeCreated {
                id,
                parent,
                name,
                origin,
                init,
            } => self.node_created(*id, *parent, name, *origin, init),
            Ev::NodeUpdated { node, patch } => self.node_updated(*node, patch),
            Ev::KeyAdded { node, key } => self.key_added(*node, &key.key, key.nv),
            Ev::KeyRemoved { node, key } => self.key_removed(*node, key),
            Ev::LinkAdded { node, rel, to } => self.link_added(*node, *rel, *to),
            Ev::NodeMoved { node, parent, name } => self.node_moved(*node, *parent, name),
            Ev::NodeClosed {
                node,
                outcome,
                merged_into,
                by,
            } => self.node_closed(*node, outcome, *merged_into, *by),
            Ev::NodeReopened { node } => self.node_reopened(*node),
            Ev::OfferSet { node, on } => self.offer_set(*node, *on),
            Ev::WaitSet { node, wait } => self.wait_set(*node, *wait),
            Ev::DependsRedirected { node, from, to } => self.depends_redirected(*node, *from, *to),
            Ev::MarkSet {
                node,
                mark,
                question,
                on,
            } => self.mark_set(*node, *mark, *question, *on),
            Ev::IntentOpened { node, intent } => self.intent_opened(*node, *intent),
            Ev::IntentClosed { node, end: _ } => self.intent_closed(*node),
            Ev::IntentMoved { from, to } => self.intent_moved(*from, *to),
            Ev::SeriesOutcome { series, missed } => self.series_outcome(*series, *missed),
            Ev::OccurrenceAvailable { node } => self.occurrence_available(*node),
            Ev::OccurrenceDetached { node } => self.occurrence_detached(*node),
            Ev::SignalDetached { node } => self.signal_detached(*node),
            Ev::HoldIssued {
                token,
                session,
                lease_until,
                terms,
                l,
                reopened,
            } => self.hold_issued(*token, *session, *lease_until, *terms, *l, reopened.clone()),
            Ev::HoldExtended {
                token,
                lease_until,
                by,
            } => self.hold_extended(*token, *lease_until, *by),
            Ev::HoldEnded { token, end } => self.hold_ended(*token, *end),
            Ev::Appended {
                eid,
                node,
                seq,
                by,
                body,
                text: _,
            } => self.appended(*eid, *node, *seq, *by, body),
            Ev::RequestAnswered { request, by } => self.request_answered(*request, *by),
            Ev::RequestSettled {
                request,
                decision,
                answer,
            } => self.request_settled(*request, *decision, *answer),
            Ev::RequestMoved { request, to } => self.request_moved(*request, *to),
            Ev::PqRecheckAt { q, at } => self.pq_change(*q, |pq, _| {
                if pq.state != PqState::Open {
                    return false;
                }
                pq.recheck_at = *at;
                true
            }),
            Ev::PqRechecked { q, rechecks, next } => self.pq_change(*q, |pq, _| {
                if pq.state != PqState::Open {
                    return false;
                }
                pq.rechecks = *rechecks;
                pq.recheck_at = *next;
                true
            }),
            Ev::PqExpired { q } => self.pq_change(*q, |pq, _| {
                if pq.state != PqState::Open {
                    return false;
                }
                pq.state = PqState::Expired;
                pq.rechecks = pq.rechecks.saturating_add(1);
                pq.lapse = Some(Lapse::Expired);
                true
            }),
            Ev::PqEscalated { q, uq } => self.pq_change(*q, |pq, _| {
                if pq.state != PqState::Open {
                    return false;
                }
                pq.state = PqState::Escalated { uq: *uq };
                pq.rechecks = pq.rechecks.saturating_add(1);
                pq.lapse = Some(Lapse::Escalated);
                true
            }),
            Ev::PqAnswered { q, by } => self.pq_change(*q, |pq, _| {
                if !matches!(
                    pq.state,
                    PqState::Open | PqState::Expired | PqState::Escalated { .. }
                ) {
                    return false;
                }
                pq.state = PqState::Answered { by: *by };
                true
            }),
            Ev::PqSuperseded { q, by } => self.pq_change(*q, |pq, _| {
                if pq.state != PqState::Open {
                    return false;
                }
                pq.state = PqState::Superseded { by: *by };
                true
            }),
            Ev::PqWithdrawn { q, by } => self.pq_change(*q, |pq, _| {
                if pq.state != PqState::Open {
                    return false;
                }
                pq.state = PqState::Withdrawn { by: *by };
                true
            }),
            Ev::PqAssumed { q, decision } => self.pq_change(*q, |pq, _| {
                if pq.state != PqState::Expired || pq.assumed_by.is_some() {
                    return false;
                }
                pq.assumed_by = Some(*decision);
                true
            }),
            Ev::PqMoved { q, to } => self.pq_change(*q, |pq, _| {
                pq.moved_from = Some(pq.node);
                pq.node = *to;
                true
            }),
            Ev::AqAnswered { q, by } => {
                let Some(aq) = self.s.aquestions.get_mut(q) else {
                    return false;
                };
                if aq.state != AqState::Open {
                    return false;
                }
                aq.state = AqState::Answered { by: *by };
                let q = *q;
                self.dirt.mark(|| Fact::AQuestion(q));
                true
            }
            Ev::UqAnswered { q, by } => self.uq_change(*q, |uq, _| {
                if uq.state != UqState::Pending {
                    return false;
                }
                uq.state = UqState::Answered { by: *by };
                true
            }),
            Ev::UqDefaulted { q } => self.uq_change(*q, |uq, _| {
                if uq.state != UqState::Pending || !uq.has_default {
                    return false;
                }
                uq.state = UqState::Defaulted;
                true
            }),
            Ev::UqReaddressed { q, stage, due } => self.uq_change(*q, |uq, at| {
                let next = uq.stage.checked_add(1);
                if uq.state != UqState::Pending
                    || uq.has_default
                    || next != Some(*stage)
                    || usize::from(*stage) >= uq.to.len()
                    || *due <= at
                {
                    return false;
                }
                uq.stage = *stage;
                uq.due = *due;
                true
            }),
            Ev::UqLapsed { q } => self.uq_change(*q, |uq, _| {
                let last = usize::from(uq.stage).checked_add(1) == Some(uq.to.len());
                if uq.state != UqState::Pending || uq.has_default || !last {
                    return false;
                }
                uq.state = UqState::Lapsed;
                true
            }),
            Ev::UqAssumed { q, decision } => self.uq_change(*q, |uq, _| {
                if uq.state != UqState::Lapsed || uq.assumed_by.is_some() {
                    return false;
                }
                uq.assumed_by = Some(*decision);
                true
            }),
            Ev::CommitmentOpened {
                c,
                node,
                conv,
                followup_at,
            } => self.commitment_opened(*c, *node, conv, *followup_at),
            Ev::CommitmentRefreshed { c, followup_at } => self.commitment_change(*c, |x, at| {
                if x.state != CommitmentState::Open {
                    return false;
                }
                x.last_ack = at;
                x.followup_at = *followup_at;
                x.followup_fired = false;
                true
            }),
            Ev::CommitmentFulfilled { c, by } => self.commitment_change(*c, |x, _| {
                if x.state != CommitmentState::Open {
                    return false;
                }
                x.state = CommitmentState::Fulfilled { by: *by };
                true
            }),
            Ev::CommitmentMoved { c, to } => self.commitment_change(*c, |x, _| {
                x.moved_from = Some(x.node);
                x.node = *to;
                true
            }),
            Ev::SaidConfirmed { said, via, link: _ } => self.said_change(*said, |x| {
                // 1.9 (§9.2): a withheld `said` its author sent after all is confirmed too.
                if !matches!(
                    x.state,
                    SaidState::Pending | SaidState::Unconfirmed | SaidState::Withheld
                ) {
                    return false;
                }
                x.state = SaidState::Confirmed;
                // 1.7 (§9.2): a confirm by the session's release or close is `with_reply`.
                x.with_reply = !matches!(via, crate::event::ConfirmVia::Explicit);
                true
            }),
            Ev::SaidUnconfirmed { said } => self.said_change(*said, |x| {
                if x.state != SaidState::Pending {
                    return false;
                }
                x.state = SaidState::Unconfirmed;
                true
            }),
            Ev::SaidWithheld { said } => self.said_change(*said, |x| {
                if x.state != SaidState::Pending {
                    return false;
                }
                x.state = SaidState::Withheld;
                true
            }),
            Ev::SaidMoved { said, to } => self.said_change(*said, |x| {
                x.moved_from = Some(x.node);
                x.node = *to;
                true
            }),
            Ev::EntryRouted {
                entry,
                routing,
                reason: _,
            } => self.entry_routed(*entry, *routing),
            Ev::DecisionSuperseded { d, by } => {
                self.decision_end(*d, DecisionState::Superseded { by: *by })
            }
            Ev::DecisionRetracted { d, by, node } => {
                // 1.11 (§11.3, §24): the decision's node keeps it as its newest retracted one,
                // whatever leaves memory later. A node the event names that is not the decision's,
                // or does not exist, is a precondition that fails: nothing is applied.
                if let Some(n) = node
                    && (self.s.decisions.get(d).map(|x| x.node) != Some(*n)
                        || !self.s.nodes.contains_key(n))
                {
                    return false;
                }
                if !self.decision_end(*d, DecisionState::Retracted { by: *by }) {
                    return false;
                }
                if let Some(n) = node
                    && let Some(x) = self.s.nodes.get_mut(n)
                {
                    x.retracted_decision = Some(x.retracted_decision.map_or(*d, |e| e.max(*d)));
                    self.hot(*n);
                }
                true
            }
            Ev::ObjectionResolved { o, by, upheld } => self.objection_change(*o, |x| {
                if x.state != ObjectionState::Open {
                    return false;
                }
                x.state = if *upheld {
                    ObjectionState::Upheld { by: *by }
                } else {
                    ObjectionState::Resolved { by: *by }
                };
                true
            }),
            Ev::ObjectionMoved { o, to } => self.objection_change(*o, |x| {
                x.moved_from = Some(x.node);
                x.node = *to;
                true
            }),
            Ev::DigestShown { entry, by } => self.digest_shown(*entry, *by),
            Ev::Refused {
                node,
                class,
                session,
                backoff_until,
                expires_at,
                quorum,
            } => self.refused(*node, class, *session, *backoff_until, *expires_at, *quorum),
            Ev::RefusalExpired { node, class } => self.refusal_expired(*node, class),
            Ev::RefusalsCleared { node, by: _ } => {
                self.refusals_cleared(*node);
                true
            }
            Ev::SignalOpened { key, node } => {
                if !self.s.nodes.contains_key(node) || self.s.signals.contains_key(key) {
                    return false;
                }
                self.s.signals.insert(*key, *node);
                let k = *key;
                self.dirt.mark(|| Fact::Signal(k));
                true
            }
            Ev::SignalResolved { key } => {
                if self.s.signals.remove(key).is_none() {
                    return false;
                }
                let k = *key;
                self.dirt.mark(|| Fact::Signal(k));
                true
            }
            Ev::Paused {
                scope,
                until,
                ttl,
                by,
                class,
                renewed: _,
            } => self.paused(*scope, *until, *ttl, *by, class),
            Ev::Unpaused { scope } | Ev::PauseExpired { scope } => self.unpaused(*scope),
            Ev::NoteBound { note, session } => self.note_bound(*note, *session),
            Ev::Hidden { target, reason: _ } => match target {
                crate::event::HideTarget::Entry(e) => *e < self.s.counters.next_eid,
                crate::event::HideTarget::File { node, name } => self
                    .s
                    .nodes
                    .get(node)
                    .is_some_and(|n| n.files.contains_key(name)),
            },
            Ev::OpRecorded {
                class,
                op,
                digest,
                session,
                expires_at,
                outcome: _,
            } => self.op_recorded(class, op, *digest, *session, *expires_at),
            Ev::Restored { items } => self.restored(items),
            Ev::NodeCorrected {
                node,
                outcome,
                by,
                was,
            } => self.node_corrected(*node, outcome, *by, was.as_ref()),
            Ev::CorrectionWithdrawn { node, outcome } => self.correction_withdrawn(*node, outcome),
        }
    }

    fn genesis(&mut self, params: &crate::params::Params, events_before: u64) -> bool {
        if events_before != 0 {
            return false;
        }
        self.s.params = params.clone();
        let reserved: [(NodeId, Option<(NodeId, &str)>); 8] = [
            (ROOT, None),
            (USER, Some((ROOT, "@user"))),
            (USER_DECISIONS, Some((USER, "decisions"))),
            (USER_DIGEST, Some((USER, "digest"))),
            (SYS, Some((ROOT, "sys"))),
            (SIGNALS, Some((SYS, "signals"))),
            (PEOPLE, Some((ROOT, "@people"))),
            (TEAMS, Some((ROOT, "@teams"))),
        ];
        for (id, place) in reserved {
            let place =
                place.and_then(|(parent, name)| Name::new(name).map(|name| Place { parent, name }));
            if let Some(p) = &place {
                self.s.names.insert((p.parent, p.name.clone()), id);
                let (pp, pn) = (p.parent, p.name.clone());
                self.dirt.mark(|| Fact::Name(pp, pn));
            }
            self.s.nodes.insert(id, reserved_node(place, self.at));
            self.s.ix.node.insert(id, NodeIx::default());
            fix_depths(self.s, id);
            node_contrib(self.s, id, true, false);
            self.hot(id);
            self.content(id);
            self.touch(id);
        }
        if let Some(user) = crate::types::PrincipalKey::new(crate::types::USER_KEY) {
            self.s.principal_ids.insert(user.clone(), USER_PRINCIPAL);
            self.s.ix.principal_key.insert(USER_PRINCIPAL, user.clone());
            self.dirt.mark(|| Fact::Principal(user));
        }
        if self.s.counters.next_principal.0 < 1 {
            self.s.counters.next_principal = PrincipalId(1);
        }
        true
    }

    fn fired(&mut self, kind: DeadlineKind, owner: &Owner) -> bool {
        if deadline_of(self.s, kind, owner) != Some(self.at) {
            return false;
        }
        let at = self.at;
        match (kind, owner) {
            (DeadlineKind::Wait, Owner::Node(n)) => {
                if let Some(w) = self.s.nodes.get_mut(n).and_then(|x| x.wait.as_mut()) {
                    w.fired = true;
                }
                // 1.8 (§5): a wait that resolves at its time touches its node.
                self.touch(*n);
            }
            (DeadlineKind::Stale, Owner::Node(n)) => {
                if let Some(x) = self.s.nodes.get_mut(n) {
                    x.stale_fired = true;
                }
                self.hot(*n);
                self.refresh(*n);
            }
            (DeadlineKind::Commitment, Owner::Entry(c)) => {
                if let Some(x) = self.s.commitments.get_mut(c) {
                    x.followup_fired = true;
                    let node = x.node;
                    let c = *c;
                    self.dirt.mark(|| Fact::Commitment(c));
                    self.refresh(node);
                }
            }
            (DeadlineKind::OccurrenceWindow, Owner::Node(o)) => {
                if let Some(Role::Occurrence(x)) = self.s.nodes.get_mut(o).map(|x| &mut x.role) {
                    x.window_fired = true;
                }
                self.hot(*o);
                self.refresh(*o);
            }
            (DeadlineKind::Due, Owner::Node(n)) => {
                if let Some(x) = self.s.nodes.get_mut(n) {
                    x.due_fired = true;
                }
                self.hot(*n);
                self.refresh(*n);
            }
            (DeadlineKind::Silence, Owner::Session(x)) => {
                ix_session(self.s, *x, false);
                if let Some(session) = self.s.sessions.get_mut(x) {
                    session.silent = true;
                }
                ix_session(self.s, *x, true);
                let x = *x;
                self.dirt.mark(|| Fact::Session(x));
            }
            (DeadlineKind::RefusalExpiry, Owner::Class(c)) => {
                let ended: Vec<(BTreeSet<Label>, Time)> = self
                    .s
                    .classes
                    .get(c)
                    .map(|a| {
                        a.caps_seen
                            .iter()
                            .filter(|(_, t)| **t <= at)
                            .map(|(k, t)| (k.clone(), *t))
                            .collect()
                    })
                    .unwrap_or_default();
                for (caps, t) in ended {
                    if let Some(a) = self.s.classes.get_mut(c) {
                        a.caps_seen.remove(&caps);
                    }
                    put_in(
                        &mut self.s.ix.class_ends,
                        c.clone(),
                        (t, caps.clone()),
                        false,
                    );
                    let c = c.clone();
                    self.dirt.mark(|| Fact::ClassProfile(c, caps));
                }
                if self
                    .s
                    .classes
                    .get(c)
                    .is_some_and(|a| a.caps_seen.is_empty())
                {
                    self.s.classes.remove(c);
                }
            }
            (
                DeadlineKind::Lease
                | DeadlineKind::Wait
                | DeadlineKind::Recheck
                | DeadlineKind::UserQuestion
                | DeadlineKind::Commitment
                | DeadlineKind::OccurrenceSlot
                | DeadlineKind::OccurrenceWindow
                | DeadlineKind::RefusalExpiry
                | DeadlineKind::Due
                | DeadlineKind::Pause
                | DeadlineKind::Silence
                | DeadlineKind::Retire
                | DeadlineKind::Stale,
                _,
            ) => {}
        }
        self.owner(owner.clone());
        true
    }

    // --------------------------------------------------------------------------------------------
    // Sessions

    /// The profile of `stats.profiles` under `(caps, bitlen(horizon))`: `last_seen := at` and the
    /// larger horizon; then the profiles last seen before `at − capacity.window` leave (D.4.9).
    fn profile_seen(&mut self, caps: &BTreeSet<Label>, horizon: Option<u32>) {
        let at = self.at;
        let key = (caps.clone(), horizon_bits(horizon));
        let old = self.s.stats.profiles.get(&key).copied();
        if let Some(o) = old {
            self.s
                .ix
                .profiles_by_time
                .remove(&(o.last_seen, key.clone()));
        }
        let max_horizon = old.map_or(horizon, |o| o.max_horizon.max(horizon));
        self.s.stats.profiles.insert(
            key.clone(),
            ProfileSeen {
                max_horizon,
                last_seen: at,
            },
        );
        self.s.ix.profiles_by_time.insert((at, key.clone()));
        self.dirt.mark(|| Fact::Profile(key.0, key.1));
        let cutoff = at.minus(self.s.params.capacity_window);
        while let Some(first) = self.s.ix.profiles_by_time.first().cloned() {
            if first.0 >= cutoff {
                break;
            }
            self.s.ix.profiles_by_time.remove(&first);
            if self
                .s
                .stats
                .profiles
                .get(&first.1)
                .is_some_and(|p| p.last_seen == first.0)
            {
                self.s.stats.profiles.remove(&first.1);
                let (caps, bits) = first.1;
                self.dirt.mark(|| Fact::Profile(caps, bits));
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the fields of SessionOpened, D.7.2"
    )]
    fn session_opened(
        &mut self,
        session: SessionId,
        class: &ClassName,
        caps: &BTreeSet<Label>,
        horizon: Option<u32>,
        silence_at: Time,
        retire_at: Time,
        principal: Option<PrincipalId>,
    ) -> bool {
        let ok = session == self.s.counters.next_session
            && principal.is_none_or(|p| self.interned(p))
            && retire_at > self.at;
        let next = SessionId(session.0.saturating_add(1));
        if next > self.s.counters.next_session {
            self.s.counters.next_session = next;
        }
        if !ok {
            return false;
        }
        let at = self.at;
        self.s.sessions.insert(
            session,
            Session {
                class: class.clone(),
                started_at: at,
                last_seen: at,
                silence_at,
                retire_at,
                silent: false,
                caps: caps.clone(),
                horizon,
                principals: principal.into_iter().collect(),
                shown: alloc::collections::BTreeMap::new(),
                branch_seen: alloc::collections::BTreeMap::new(),
                told: BTreeSet::new(),
                handed_off: None,
            },
        );
        ix_session(self.s, session, true);
        self.dirt.mark(|| Fact::Session(session));
        self.profile_seen(caps, horizon);
        // Session starts per minute, pruned to coverage.window (CS-17).
        let m = minute_of(at);
        let c = self.s.stats.starts.entry(m).or_insert(0);
        *c = c.saturating_add(1);
        self.dirt.mark(|| Fact::Start(m));
        let cutoff = minute_of(at.minus(self.s.params.coverage_window));
        while let Some((&first, _)) = self.s.stats.starts.first_key_value() {
            if first >= cutoff {
                break;
            }
            self.s.stats.starts.remove(&first);
            self.dirt.mark(|| Fact::Start(first));
        }
        self.owner(Owner::Session(session));
        true
    }

    fn session_hello(
        &mut self,
        session: SessionId,
        caps: Option<&BTreeSet<Label>>,
        horizon: Option<u32>,
        principal: Option<PrincipalId>,
    ) -> bool {
        if !self.s.sessions.contains_key(&session) || !principal.is_none_or(|p| self.interned(p)) {
            return false;
        }
        let Some(x) = self.s.sessions.get_mut(&session) else {
            return false;
        };
        if let Some(c) = caps {
            x.caps = c.clone();
        }
        if let Some(h) = horizon {
            x.horizon = Some(h);
        }
        if let Some(p) = principal {
            x.principals.insert(p);
        }
        let (caps_now, horizon_now) = (x.caps.clone(), x.horizon);
        self.dirt.mark(|| Fact::Session(session));
        if caps.is_some() || horizon.is_some() {
            self.profile_seen(&caps_now, horizon_now);
        }
        true
    }

    fn principal_seen(&mut self, id: PrincipalId, key: &crate::types::PrincipalKey) -> bool {
        let ok = id == self.s.counters.next_principal && !self.s.principal_ids.contains_key(key);
        let next = PrincipalId(id.0.saturating_add(1));
        if next > self.s.counters.next_principal {
            self.s.counters.next_principal = next;
        }
        if !ok {
            return false;
        }
        self.s.principal_ids.insert(key.clone(), id);
        self.s.ix.principal_key.insert(id, key.clone());
        let k = key.clone();
        self.dirt.mark(|| Fact::Principal(k));
        // A person node that already carries `person:<key>` gains the alias (D.2.2).
        let mut person = alloc::string::String::from("person:");
        person.push_str(key.as_str());
        if let Some(pk) = KeyStr::new(&person) {
            let carriers: Vec<NodeId> = self
                .s
                .ix
                .keys_all
                .get(&pk)
                .map(|x| x.iter().copied().collect())
                .unwrap_or_default();
            for n in carriers {
                fix_person(self.s, n);
            }
        }
        true
    }

    fn touched(&mut self, session: SessionId, silence_at: Time, retire_at: Time) -> bool {
        let Some(x) = self.s.sessions.get(&session) else {
            return false;
        };
        if retire_at < x.retire_at {
            return false;
        }
        ix_session(self.s, session, false);
        if let Some(x) = self.s.sessions.get_mut(&session) {
            x.last_seen = self.at;
            x.silence_at = silence_at;
            x.retire_at = retire_at;
            x.silent = false;
        }
        ix_session(self.s, session, true);
        self.dirt.mark(|| Fact::Session(session));
        self.owner(Owner::Session(session));
        true
    }

    fn session_retired(&mut self, session: SessionId) -> bool {
        let Some(x) = self.s.sessions.get(&session) else {
            return false;
        };
        let holds = self
            .s
            .ix
            .holds_by_session
            .get(&session)
            .is_some_and(|h| !h.is_empty());
        if !x.silent || x.retire_at > self.at || holds {
            return false;
        }
        ix_session(self.s, session, false);
        self.s.sessions.remove(&session);
        self.s.ix.inbox.remove(&session);
        self.s.ix.open_saids_by.remove(&session);
        self.s.ix.unrouted_by.remove(&session);
        self.s.ix.holds_by_session.remove(&session);
        self.dirt.mark(|| Fact::Session(session));
        self.owner(Owner::Session(session));
        true
    }

    fn inbox_read(&mut self, session: SessionId, notes: &BTreeSet<Eid>) -> bool {
        if !self.s.sessions.contains_key(&session)
            || !notes.iter().all(|x| self.s.notes.contains_key(x))
        {
            return false;
        }
        for &x in notes {
            let to = self.s.notes.get(&x).map(|n| n.to);
            if let Some(Addressee::Holders(a)) = to {
                if let Some(sx) = self.s.sessions.get_mut(&session) {
                    let cur = sx.branch_seen.entry(a).or_insert(Eid(0));
                    if x > *cur {
                        *cur = x;
                    }
                }
            } else {
                ix_note(self.s, x, false);
                if let Some(n) = self.s.notes.get_mut(&x) {
                    n.read_by.insert(session);
                }
                ix_note(self.s, x, true);
                self.dirt.mark(|| Fact::Note(x));
            }
        }
        self.dirt.mark(|| Fact::Session(session));
        true
    }

    fn shown(&mut self, session: SessionId, node: NodeId, seq: Seq) -> bool {
        if !self.s.nodes.contains_key(&node) {
            return false;
        }
        let Some(x) = self.s.sessions.get_mut(&session) else {
            return false;
        };
        if seq <= x.shown.get(&node).copied().unwrap_or(Seq(0)) {
            return false;
        }
        x.shown.insert(node, seq);
        self.dirt.mark(|| Fact::Session(session));
        true
    }

    fn next_called(
        &mut self,
        session: SessionId,
        outcome: crate::event::NextOutcome,
        open: bool,
        class_window_end: Time,
    ) -> bool {
        let Some(x) = self.s.sessions.get(&session) else {
            return false;
        };
        if open && class_window_end <= self.at {
            return false;
        }
        let (class, caps) = (x.class.clone(), x.caps.clone());
        let day = day_of(self.at);
        let d = crate::index::day_stats(self.s, day);
        d.next_calls = d.next_calls.saturating_add(1);
        // 1.7 (§17): the day's first distinct callers, for `Underused`.
        if !d.next_callers.contains(&Some(session))
            && let Some(slot) = d.next_callers.iter_mut().find(|x| x.is_none())
        {
            *slot = Some(session);
        }
        match outcome {
            crate::event::NextOutcome::Item => {}
            crate::event::NextOutcome::Nothing => d.next_empty = d.next_empty.saturating_add(1),
            crate::event::NextOutcome::NothingForYou => {
                d.next_empty_for_you = d.next_empty_for_you.saturating_add(1);
            }
        }
        self.dirt.mark(|| Fact::Day(day));
        if open {
            let a = self
                .s
                .classes
                .entry(class.clone())
                .or_insert_with(|| ClassActivity {
                    caps_seen: alloc::collections::BTreeMap::new(),
                });
            let old = a.caps_seen.insert(caps.clone(), class_window_end);
            if let Some(t) = old {
                put_in(
                    &mut self.s.ix.class_ends,
                    class.clone(),
                    (t, caps.clone()),
                    false,
                );
            }
            put_in(
                &mut self.s.ix.class_ends,
                class.clone(),
                (class_window_end, caps.clone()),
                true,
            );
            let c = class.clone();
            self.dirt.mark(|| Fact::ClassProfile(c, caps));
            self.owner(Owner::Class(class));
        }
        true
    }

    // --------------------------------------------------------------------------------------------
    // Nodes

    fn node_created(
        &mut self,
        id: NodeId,
        parent: NodeId,
        name: &Name,
        origin: Origin,
        init: &NodeInit,
    ) -> bool {
        let ok = id == self.s.counters.next_node
            && self.s.nodes.contains_key(&parent)
            && !self.s.names.contains_key(&(parent, name.clone()))
            && !self.s.nodes.contains_key(&id)
            && init
                .keys
                .keys()
                .all(|k| !self.s.ix.keys_open.contains_key(k));
        let next = NodeId(id.0.saturating_add(1));
        if next > self.s.counters.next_node {
            self.s.counters.next_node = next;
        }
        if !ok {
            return false;
        }
        let place = Place {
            parent,
            name: name.clone(),
        };
        self.s
            .nodes
            .insert(id, node_from_init(place, origin, self.at, init));
        self.s.names.insert((parent, name.clone()), id);
        let (p, nm) = (parent, name.clone());
        self.dirt.mark(|| Fact::Name(p, nm.clone()));
        if self.s.aliases.remove(&(parent, name.clone())).is_some() {
            self.dirt.mark(|| Fact::Alias(p, nm));
        }
        self.s.ix.node.insert(id, NodeIx::default());
        fix_depths(self.s, id);
        node_contrib(self.s, id, true, false);
        ix_node_own(self.s, id, true);
        fix_person(self.s, id);
        self.hot(id);
        self.content(id);
        // 1.8 (§5): a node is touched at its creation.
        self.touch(id);
        for k in init.keys.keys() {
            let k = k.clone();
            self.dirt.mark(|| Fact::Key(id, k));
        }
        for &(rel, to) in &init.links {
            self.dirt.mark(|| Fact::Link(id, rel, to));
        }
        self.owner(Owner::Node(id));
        self.refresh(id);
        self.refresh(parent);
        true
    }

    fn node_updated(&mut self, n: NodeId, p: &NodePatch) -> bool {
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        if !patch_differs(node, p) {
            return false;
        }
        let pending_about: Vec<Eid> = if p.priority.is_some() {
            self.s
                .ix
                .items
                .get(&n)
                .map(|it| {
                    it.uquestions
                        .iter()
                        .copied()
                        .filter(|q| {
                            self.s
                                .uquestions
                                .get(q)
                                .is_some_and(|u| u.state == UqState::Pending)
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        for &q in &pending_about {
            ix_uquestion(self.s, q, false);
        }
        ix_node_own(self.s, n, false);
        let at = self.at;
        let repeat_changed = !matches!(p.repeat, Change::Keep);
        if let Some(node) = self.s.nodes.get_mut(&n) {
            let bumps = counter_bumps(p);
            if let Some(v) = &p.title {
                node.title = v.clone();
            }
            if let Some(v) = &p.body {
                node.body = v.clone();
            }
            if let Some(v) = &p.fields {
                node.fields = v.clone();
            }
            if let Some(v) = &p.files {
                node.files = v.clone();
            }
            if let Some(v) = &p.status {
                node.status = v.clone();
            }
            if let Some(v) = &p.digest {
                node.digest = v.clone();
            }
            if bumps.digest_version {
                node.digest_version = Version(node.digest_version.0.saturating_add(1));
            }
            if let Some(v) = &p.agreements {
                node.agreements = v.clone();
                node.agreements_at = Some(at);
            }
            match &p.kind {
                Change::Keep => {}
                Change::Clear => node.kind = None,
                Change::Set(v) => node.kind = Some(v.clone()),
            }
            if let Some(v) = &p.tags {
                node.tags = v.clone();
            }
            if let Some(v) = p.capacity {
                node.capacity = v;
            }
            if let Some(v) = p.priority {
                node.priority = v;
            }
            match &p.est {
                Change::Keep => {}
                Change::Clear => node.est = None,
                Change::Set(v) => node.est = Some(*v),
            }
            if let Some(v) = &p.needs {
                node.needs = v.clone();
            }
            match &p.due {
                Change::Keep => {}
                Change::Clear => {
                    node.due = None;
                    node.due_fired = false;
                }
                Change::Set(v) => {
                    node.due = Some(*v);
                    node.due_fired = *v <= at;
                }
            }
            if let Some(v) = &p.depends_on {
                node.depends_on = v.clone();
            }
            if let Some(v) = &p.owners {
                node.owners = v.clone();
            }
            match &p.repeat {
                Change::Keep => {}
                Change::Clear => node.role = Role::Plain,
                Change::Set(r) => match &mut node.role {
                    Role::Series(x) => x.repeat = r.clone(),
                    Role::Plain | Role::Occurrence(_) | Role::Signal(_) => {
                        node.role = Role::Series(Series {
                            repeat: r.clone(),
                            last_missed: false,
                        });
                    }
                },
            }
            if bumps.version {
                node.version = Version(node.version.0.saturating_add(1));
            }
            if bumps.content_version {
                node.content_version = node.version;
            }
        }
        ix_node_own(self.s, n, true);
        for &q in &pending_about {
            ix_uquestion(self.s, q, true);
        }
        self.hot(n);
        self.content(n);
        self.touch(n);
        if let Some(parent) = self.s.nodes.get(&n).and_then(Node::parent) {
            self.refresh(parent);
        }
        if repeat_changed {
            self.refresh_locked(n);
        }
        true
    }

    fn key_added(&mut self, n: NodeId, key: &KeyStr, nv: NormVer) -> bool {
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        let person_ok = key
            .person()
            .is_none_or(|pk| self.s.principal_ids.contains_key(&pk));
        if node.keys.contains_key(key)
            || (node.is_open() && self.s.ix.keys_open.contains_key(key))
            || !person_ok
        {
            return false;
        }
        ix_node_own(self.s, n, false);
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.keys.insert(key.clone(), nv);
            node.version = Version(node.version.0.saturating_add(1));
        }
        ix_node_own(self.s, n, true);
        fix_person(self.s, n);
        let k = key.clone();
        self.dirt.mark(|| Fact::Key(n, k));
        self.touch(n);
        true
    }

    fn key_removed(&mut self, n: NodeId, key: &KeyStr) -> bool {
        if !self
            .s
            .nodes
            .get(&n)
            .is_some_and(|x| x.keys.contains_key(key))
        {
            return false;
        }
        ix_node_own(self.s, n, false);
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.keys.remove(key);
            node.version = Version(node.version.0.saturating_add(1));
        }
        ix_node_own(self.s, n, true);
        fix_person(self.s, n);
        let k = key.clone();
        self.dirt.mark(|| Fact::Key(n, k));
        self.touch(n);
        true
    }

    fn link_added(&mut self, n: NodeId, rel: crate::types::LinkRel, to: NodeId) -> bool {
        if !self.s.nodes.contains_key(&to)
            || self
                .s
                .nodes
                .get(&n)
                .is_none_or(|x| x.links.contains(&(rel, to)))
        {
            return false;
        }
        ix_node_own(self.s, n, false);
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.links.insert((rel, to));
            node.version = Version(node.version.0.saturating_add(1));
        }
        ix_node_own(self.s, n, true);
        self.dirt.mark(|| Fact::Link(n, rel, to));
        self.touch(n);
        true
    }

    /// `NodeMoved`. Besides the table's precondition, a move into the node's own subtree is an
    /// anomaly: it would break K4, which `decide` refuses with `cycle`.
    fn node_moved(&mut self, n: NodeId, parent: NodeId, name: &Name) -> bool {
        let Some(old) = self.s.nodes.get(&n).and_then(|x| x.place.clone()) else {
            return false;
        };
        if n == ROOT
            || !self.s.nodes.contains_key(&parent)
            || self.s.names.contains_key(&(parent, name.clone()))
            || parent == n
            || crate::index::ancestors(self.s, parent).contains(&n)
        {
            return false;
        }
        let old_chain = crate::index::ancestors(self.s, n);
        let live = self.s.ix.node.get(&n).map_or(0, |x| x.subtree_live);
        let paused_before = old_chain.iter().any(|a| self.s.pauses.contains_key(a))
            || self.s.pauses.contains_key(&n);
        node_contrib(self.s, n, false, true);
        shift_subtree_live(self.s, &old_chain, live, false);
        put_in(&mut self.s.ix.unheld_flagged_children, old.parent, n, false);
        self.s.names.remove(&(old.parent, old.name.clone()));
        self.s.aliases.insert((old.parent, old.name.clone()), n);
        self.s.names.insert((parent, name.clone()), n);
        let alias_removed = self.s.aliases.remove(&(parent, name.clone())).is_some();
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.place = Some(Place {
                parent,
                name: name.clone(),
            });
        }
        fix_depths(self.s, n);
        node_contrib(self.s, n, true, true);
        let new_chain = crate::index::ancestors(self.s, n);
        shift_subtree_live(self.s, &new_chain, live, true);
        fix_person(self.s, n);
        let paused_after = new_chain.iter().any(|a| self.s.pauses.contains_key(a));
        {
            let (op, on) = (old.parent, old.name.clone());
            self.dirt.mark(|| Fact::Name(op, on.clone()));
            self.dirt.mark(|| Fact::Alias(op, on));
            let nm = name.clone();
            self.dirt.mark(|| Fact::Name(parent, nm.clone()));
            if alias_removed {
                self.dirt.mark(|| Fact::Alias(parent, nm));
            }
        }
        self.content(n);
        self.refresh(n);
        for a in old_chain.into_iter().chain(new_chain) {
            self.refresh(a);
        }
        if paused_before || paused_after {
            for x in subtree(self.s, n) {
                if self.s.ix.next.of.contains_key(&Cand::Node(x))
                    || self.s.ix.next.of.contains_key(&Cand::Status(x))
                {
                    self.refresh(x);
                }
            }
        }
        true
    }

    fn node_closed(
        &mut self,
        n: NodeId,
        outcome: &Outcome,
        merged_into: Option<NodeId>,
        by: Option<crate::types::SessionId>,
    ) -> bool {
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        if !node.is_open() {
            return false;
        }
        if matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting)
            && *outcome != Outcome::Dropped
        {
            return false;
        }
        let citers: Vec<DecisionId> = self
            .s
            .ix
            .cited_by
            .get(&BasisTarget::Node(n))
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        node_contrib(self.s, n, false, false);
        ix_node_own(self.s, n, false);
        let at = self.at;
        let mut old_wait = None;
        let mut keys = Vec::new();
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.life = Life::Closed(Closure {
                at,
                outcome: outcome.clone(),
                merged_into,
                upheld: None,
                corrected_from: None,
                by,
            });
            node.version = Version(node.version.0.saturating_add(1));
            old_wait = node.wait.take().map(|w| w.on);
            node.tags.retain(|t| t.as_str() != OFFER);
            keys = node.keys.keys().cloned().collect();
            if *outcome == Outcome::Done {
                let minutes = node.held_ms.checked_div(60_000).unwrap_or(0);
                self.s.stats.done_minutes.push(minutes);
                while self.s.stats.done_minutes.len() > DONE_RING {
                    self.s.stats.done_minutes.remove(0);
                }
            }
        }
        if self.s.stats.demand_counted.remove(&n) {
            self.dirt.mark(|| Fact::DemandCounted(n));
        }
        node_contrib(self.s, n, true, false);
        ix_node_own(self.s, n, true);
        fix_person(self.s, n);
        if merged_into.is_some() {
            self.refile(&citers, BasisTarget::Node(n));
        }
        for d in citers {
            self.restale.insert(d);
        }
        for k in keys {
            self.dirt.mark(|| Fact::Key(n, k));
        }
        self.hot(n);
        self.owner(Owner::Node(n));
        self.refresh(n);
        if let Some(parent) = self.s.nodes.get(&n).and_then(Node::parent) {
            self.refresh(parent);
        }
        self.refresh_waiters(WaitKey::Node(n));
        self.refresh_dependents(n);
        self.refresh_locked(n);
        match old_wait {
            Some(WaitOn::Question { q, .. }) => self.leave(EntityRef::PQuestion(q)),
            Some(WaitOn::UserQuestion(q)) => self.leave(EntityRef::UQuestion(q)),
            Some(WaitOn::Node(_) | WaitOn::Time(_) | WaitOn::External(_)) | None => {}
        }
        true
    }

    /// Re-files decisions in `ix.cited_by` after a merge or a reopen changed where their node
    /// references resolve: each leaves `old` unless a reference still resolves there, and joins the
    /// targets it resolves to now (D.8.17).
    fn refile(&mut self, citers: &[DecisionId], old: BasisTarget) {
        for &d in citers {
            let Some(dec) = self.s.decisions.get(&d) else {
                continue;
            };
            if dec.state != DecisionState::Active {
                continue;
            }
            let targets = crate::flags::basis_targets(self.s, dec);
            if !targets.contains(&old) {
                put_in(&mut self.s.ix.cited_by, old, d, false);
            }
            for t in targets {
                put_in(&mut self.s.ix.cited_by, t, d, true);
            }
        }
    }

    fn refresh_dependents(&mut self, n: NodeId) {
        let ds: Vec<NodeId> = self
            .s
            .ix
            .dependents
            .get(&n)
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        for d in ds {
            self.refresh(d);
        }
    }

    fn node_reopened(&mut self, n: NodeId) -> bool {
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        if node.is_open()
            || matches!(node.role, Role::Occurrence(_))
            || node
                .keys
                .keys()
                .any(|k| self.s.ix.keys_open.get(k).is_some_and(|&o| o != n))
        {
            return false;
        }
        let merged = node.merged_into().is_some();
        let filed = BasisTarget::Node(if merged { resolve_merged(self.s, n) } else { n });
        let citers: Vec<DecisionId> = self
            .s
            .ix
            .cited_by
            .get(&filed)
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        node_contrib(self.s, n, false, false);
        ix_node_own(self.s, n, false);
        let at = self.at;
        let mut keys = Vec::new();
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.life = Life::Open;
            node.version = Version(node.version.0.saturating_add(1));
            node.due_fired = node.due.is_some_and(|d| d <= at);
            // 1.12 (§5): a pull request named in an earlier round of work is not this round's.
            node.pr_named = false;
            keys = node.keys.keys().cloned().collect();
        }
        node_contrib(self.s, n, true, false);
        ix_node_own(self.s, n, true);
        fix_person(self.s, n);
        // 1.8 (§5): a reopen touches the node.
        self.touch(n);
        if merged {
            self.refile(&citers, filed);
        }
        for d in citers {
            self.restale.insert(d);
        }
        for k in keys {
            self.dirt.mark(|| Fact::Key(n, k));
        }
        self.hot(n);
        self.owner(Owner::Node(n));
        self.refresh(n);
        if let Some(parent) = self.s.nodes.get(&n).and_then(Node::parent) {
            self.refresh(parent);
        }
        self.refresh_waiters(WaitKey::Node(n));
        self.refresh_dependents(n);
        self.refresh_locked(n);
        true
    }

    /// `NodeCorrected` (1.8, §5, §11.3): a closed node takes its corrected outcome and the upheld
    /// mark, and, 1.10, `corrected_from := was`, the outcome a withdrawal restores (None from a log
    /// written before 1.10). Its own subtree count moves to the new outcome; the decisions that
    /// cite it have their staleness recomputed (a reference that recorded `done` goes stale,
    /// D.8.17). No counter changes (O2).
    fn node_corrected(
        &mut self,
        n: NodeId,
        outcome: &Outcome,
        by: Eid,
        was: Option<&Outcome>,
    ) -> bool {
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        if node.is_open() {
            return false;
        }
        self.set_outcome(n, outcome, Some((by, was.cloned())))
    }

    /// `CorrectionWithdrawn` (1.10, §5, §11.3): a closed node marked upheld takes back the outcome
    /// it was closed with and loses the mark; precondition: closed and marked. As a correction,
    /// it moves the subtree count and recomputes the staleness of the decisions that cite it (a
    /// reference that recorded the corrected outcome goes stale). No counter changes (O2).
    fn correction_withdrawn(&mut self, n: NodeId, outcome: &Outcome) -> bool {
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        if !matches!(&node.life, Life::Closed(c) if c.upheld.is_some()) {
            return false;
        }
        self.set_outcome(n, outcome, None)
    }

    /// A closed node's outcome, upheld mark, and `corrected_from`, for `NodeCorrected` (`mark`
    /// Some) and `CorrectionWithdrawn` (None, which clears both).
    fn set_outcome(
        &mut self,
        n: NodeId,
        outcome: &Outcome,
        mark: Option<(Eid, Option<Outcome>)>,
    ) -> bool {
        let citers: Vec<DecisionId> = self
            .s
            .ix
            .cited_by
            .get(&BasisTarget::Node(n))
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        node_contrib(self.s, n, false, false);
        if let Some(node) = self.s.nodes.get_mut(&n)
            && let Life::Closed(c) = &mut node.life
        {
            match mark {
                Some((by, was)) => {
                    c.upheld = Some(by);
                    c.corrected_from = was;
                }
                None => {
                    c.upheld = None;
                    c.corrected_from = None;
                }
            }
            c.outcome = outcome.clone();
        }
        node_contrib(self.s, n, true, false);
        for d in citers {
            self.restale.insert(d);
        }
        self.hot(n);
        self.refresh(n);
        if let Some(parent) = self.s.nodes.get(&n).and_then(Node::parent) {
            self.refresh(parent);
        }
        true
    }

    fn offer_set(&mut self, n: NodeId, on: bool) -> bool {
        let Some(node) = self.s.nodes.get_mut(&n) else {
            return false;
        };
        let has = node.tags.iter().any(|t| t.as_str() == OFFER);
        if has == on {
            return false;
        }
        if on {
            if let Some(l) = Label::new(OFFER) {
                node.tags.insert(l);
            }
        } else {
            node.tags.retain(|t| t.as_str() != OFFER);
        }
        self.hot(n);
        self.refresh(n);
        true
    }

    fn wait_set(&mut self, n: NodeId, wait: Option<WaitOn>) -> bool {
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        if !node.is_open() {
            return false;
        }
        let old = node.wait.as_ref().map(|w| w.on);
        ix_node_own(self.s, n, false);
        let at = self.at;
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.wait = wait.map(|on| Wait {
                on,
                set_at: at,
                fired: match on {
                    WaitOn::Time(t) | WaitOn::External(t) => t <= at,
                    WaitOn::Question { .. } | WaitOn::UserQuestion(_) | WaitOn::Node(_) => false,
                },
            });
        }
        ix_node_own(self.s, n, true);
        self.hot(n);
        self.owner(Owner::Node(n));
        self.refresh(n);
        for w in [old, wait].into_iter().flatten() {
            match w {
                WaitOn::Question { q, .. } => self.leave(EntityRef::PQuestion(q)),
                WaitOn::UserQuestion(q) => self.leave(EntityRef::UQuestion(q)),
                WaitOn::Node(_) | WaitOn::Time(_) | WaitOn::External(_) => {}
            }
        }
        true
    }

    fn depends_redirected(&mut self, n: NodeId, from: NodeId, to: Option<NodeId>) -> bool {
        if !self
            .s
            .nodes
            .get(&n)
            .is_some_and(|x| x.depends_on.contains(&from))
        {
            return false;
        }
        ix_node_own(self.s, n, false);
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.depends_on.remove(&from);
            if let Some(t) = to {
                node.depends_on.insert(t);
            }
        }
        ix_node_own(self.s, n, true);
        self.content(n);
        self.refresh(n);
        true
    }

    fn mark_set(&mut self, n: NodeId, mark: crate::types::Mark, q: Eid, on: bool) -> bool {
        if !self.s.nodes.contains_key(&n)
            || !(self.s.pquestions.contains_key(&q) || self.s.uquestions.contains_key(&q))
        {
            return false;
        }
        ix_node_own(self.s, n, false);
        if let Some(node) = self.s.nodes.get_mut(&n) {
            let set = match mark {
                crate::types::Mark::Recheck => &mut node.recheck_marks,
                crate::types::Mark::Assume => &mut node.assume_marks,
            };
            put(set, q, on);
        }
        ix_node_own(self.s, n, true);
        self.hot(n);
        self.refresh(n);
        if self.s.pquestions.contains_key(&q) {
            self.leave(EntityRef::PQuestion(q));
        }
        if self.s.uquestions.contains_key(&q) {
            self.leave(EntityRef::UQuestion(q));
        }
        true
    }

    fn intent_opened(&mut self, n: NodeId, intent: Intent) -> bool {
        let Some(node) = self.s.nodes.get_mut(&n) else {
            return false;
        };
        if node.open_intent.is_some() {
            return false;
        }
        node.open_intent = Some(intent);
        self.hot(n);
        self.refresh(n);
        true
    }

    fn intent_closed(&mut self, n: NodeId) -> bool {
        let Some(node) = self.s.nodes.get_mut(&n) else {
            return false;
        };
        if node.open_intent.take().is_none() {
            return false;
        }
        self.hot(n);
        self.refresh(n);
        true
    }

    fn intent_moved(&mut self, from: NodeId, to: NodeId) -> bool {
        let intent = self.s.nodes.get(&from).and_then(|x| x.open_intent);
        let free = self
            .s
            .nodes
            .get(&to)
            .is_some_and(|x| x.open_intent.is_none());
        let (Some(intent), true) = (intent, free) else {
            return false;
        };
        if let Some(x) = self.s.nodes.get_mut(&from) {
            x.open_intent = None;
        }
        if let Some(x) = self.s.nodes.get_mut(&to) {
            x.open_intent = Some(intent);
        }
        self.hot(from);
        self.hot(to);
        self.refresh(from);
        self.refresh(to);
        true
    }

    fn series_outcome(&mut self, n: NodeId, missed: bool) -> bool {
        let Some(Role::Series(x)) = self.s.nodes.get_mut(&n).map(|x| &mut x.role) else {
            return false;
        };
        x.last_missed = missed;
        self.hot(n);
        self.refresh(n);
        true
    }

    fn occurrence_available(&mut self, o: NodeId) -> bool {
        let Some(node) = self.s.nodes.get_mut(&o) else {
            return false;
        };
        if !node.is_open() {
            return false;
        }
        let Role::Occurrence(x) = &mut node.role else {
            return false;
        };
        if x.phase != OccPhase::Waiting {
            return false;
        }
        x.phase = OccPhase::Available;
        self.hot(o);
        self.owner(Owner::Node(o));
        self.refresh(o);
        true
    }

    fn occurrence_detached(&mut self, o: NodeId) -> bool {
        let Some(Role::Occurrence(x)) = self.s.nodes.get(&o).map(|x| &x.role) else {
            return false;
        };
        let series = x.series;
        ix_node_own(self.s, o, false);
        crate::index::put_in(&mut self.s.ix.flagged_occurrences, series, o, false);
        if let Some(node) = self.s.nodes.get_mut(&o) {
            node.role = Role::Plain;
        }
        ix_node_own(self.s, o, true);
        self.hot(o);
        self.content(o);
        self.owner(Owner::Node(o));
        self.refresh(o);
        if self.s.nodes.contains_key(&series) {
            self.refresh(series);
        }
        true
    }

    fn signal_detached(&mut self, n: NodeId) -> bool {
        let Some(Role::Signal(key)) = self.s.nodes.get(&n).map(|x| &x.role) else {
            return false;
        };
        if self.s.signals.get(key) == Some(&n) {
            return false;
        }
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.role = Role::Plain;
        }
        self.content(n);
        self.refresh(n);
        true
    }

    // --------------------------------------------------------------------------------------------
    // Holds

    fn hold_issued(
        &mut self,
        token: crate::types::Token,
        session: SessionId,
        lease_until: Time,
        terms: crate::state::LeaseTerms,
        l: Dur,
        reopened: Option<Outcome>,
    ) -> bool {
        let n = token.node;
        let Some(node) = self.s.nodes.get(&n) else {
            return false;
        };
        let waiting_occ = matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting);
        let session_ok = self
            .s
            .sessions
            .get(&session)
            .is_some_and(|x| x.retire_at > self.at.plus(terms.ceiling));
        if !node.is_open()
            || waiting_occ
            || token.fence.0 != node.fence.0.saturating_add(1)
            || !session_ok
            || self.s.holds.contains_key(&token)
        {
            return false;
        }
        let at = self.at;
        self.s.holds.insert(
            token,
            Hold {
                session,
                taken_at: at,
                lease_until,
                last_extended: at,
                terms,
                l,
                renews: 0,
                checkpointed: false,
                reopened,
            },
        );
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.fence = token.fence;
        }
        ix_hold(self.s, token, true);
        add_live(self.s, n, true);
        self.dirt.mark(|| Fact::Hold(token));
        self.hot(n);
        self.owner(Owner::Hold(token));
        self.owner(Owner::Session(session));
        self.refresh_chain(n);
        true
    }

    fn hold_extended(
        &mut self,
        token: crate::types::Token,
        lease_until: Time,
        by: crate::event::Extension,
    ) -> bool {
        let at = self.at;
        let Some(h) = self.s.holds.get_mut(&token) else {
            return false;
        };
        if lease_until < h.lease_until {
            return false;
        }
        h.lease_until = lease_until;
        h.last_extended = at;
        match by {
            crate::event::Extension::Checkpoint => h.renews = 0,
            crate::event::Extension::Renew => h.renews = h.renews.saturating_add(1),
        }
        let session = h.session;
        self.dirt.mark(|| Fact::Hold(token));
        self.owner(Owner::Hold(token));
        self.owner(Owner::Session(session));
        self.refresh(token.node);
        true
    }

    fn hold_ended(&mut self, token: crate::types::Token, end: HoldEnd) -> bool {
        let Some(h) = self.s.holds.get(&token).cloned() else {
            return false;
        };
        let n = token.node;
        ix_hold(self.s, token, false);
        add_live(self.s, n, false);
        self.s.holds.remove(&token);
        let at = self.at;
        let held = at.since(h.taken_at).0;
        let attempt = matches!(
            end,
            HoldEnd::Expired | HoldEnd::EndedByClose(crate::event::EndCause::Move)
        ) && !h.checkpointed;
        let offer = matches!(
            end,
            HoldEnd::Released
                | HoldEnd::Expired
                | HoldEnd::EndedByClose(crate::event::EndCause::Move)
        );
        if let Some(node) = self.s.nodes.get_mut(&n) {
            node.held_ms = node.held_ms.saturating_add(held);
            if attempt {
                node.attempts = node.attempts.saturating_add(1);
            }
            // 1.4 (§10.4, re-run R30): the last hold that expired after the node's last checkpoint
            // without a checkpoint of its own, which the brief's section 2 shows.
            if end == HoldEnd::Expired && !h.checkpointed {
                node.expired_hold = Some(crate::state::ExpiredHold {
                    token,
                    session: h.session,
                    at,
                });
            }
            if offer
                && node.is_open()
                && !node.is_container()
                && let Some(l) = Label::new(OFFER)
            {
                node.tags.insert(l);
            }
            self.dirt.mark(|| Fact::NodeHot(n));
        }
        let day = day_of(at);
        let d = crate::index::day_stats(self.s, day);
        if attempt {
            d.lost_ms = d.lost_ms.saturating_add(held);
        } else {
            d.supply_ms = d.supply_ms.saturating_add(held);
        }
        self.dirt.mark(|| Fact::Day(day));
        self.dirt.mark(|| Fact::Hold(token));
        self.owner(Owner::Hold(token));
        self.owner(Owner::Session(h.session));
        self.refresh_chain(n);
        true
    }

    // --------------------------------------------------------------------------------------------
    // The feed

    fn appended(&mut self, eid: Eid, node: NodeId, seq: Seq, by: Author, body: &EntryBody) -> bool {
        let principal = match body {
            EntryBody::FromUser { principal } | EntryBody::ToUser { principal, .. } => {
                Some(*principal)
            }
            EntryBody::Note { .. }
            | EntryBody::Request { .. }
            | EntryBody::Question(_)
            | EntryBody::Answer { .. }
            | EntryBody::Decision { .. }
            | EntryBody::Checkpoint { .. }
            | EntryBody::Said { .. }
            | EntryBody::Heard { .. }
            | EntryBody::Approve { .. }
            | EntryBody::Object
            | EntryBody::Resolve { .. }
            | EntryBody::Retract { .. }
            | EntryBody::System(_)
            | EntryBody::Late { .. }
            | EntryBody::IntentChecked { .. } => None,
        };
        let ok = eid == self.s.counters.next_eid
            && self
                .s
                .nodes
                .get(&node)
                .is_some_and(|n| n.seq.0.checked_add(1) == Some(seq.0))
            && principal.is_none_or(|p| self.interned(p))
            && !bears_entity(self.s, eid)
            && !self.s.entries.contains_key(&eid);
        let next = Eid(eid.0.saturating_add(1));
        if next > self.s.counters.next_eid {
            self.s.counters.next_eid = next;
        }
        if !ok {
            return false;
        }
        let at = self.at;
        if crate::event::keeps_meta(body) {
            let ty = body.entry_type();
            self.s.entries.insert(
                eid,
                EntryMeta {
                    node,
                    seq,
                    at,
                    by,
                    ty,
                    principal,
                },
            );
            ix_meta(self.s, eid, true);
            self.dirt.mark(|| Fact::Entry(eid));
            if bears_none(ty) {
                self.leave(EntityRef::Meta(eid));
            }
        }
        if let Some(n) = self.s.nodes.get_mut(&node) {
            n.seq = seq;
        }
        self.hot(node);
        // 1.8 (§5): an entry touches its node, except a write made with a stale token (H4).
        if !matches!(body, EntryBody::Late { .. }) {
            self.touch(node);
        }
        match body {
            EntryBody::Note {
                to: Some(to),
                until,
                recipients,
                pending_holder,
                expects: _,
                reply_to: _,
            } => {
                let until = until.unwrap_or_else(|| at.plus(self.s.params.deliver_window));
                self.insert_note(eid, node, *to, until, recipients.clone(), *pending_holder);
            }
            EntryBody::Note { to: None, .. }
            | EntryBody::Heard { .. }
            | EntryBody::Resolve {
                deferred: false, ..
            }
            | EntryBody::Late { .. } => {}
            // 1.12 (§10.2, §20.9 rule 8): a `hold check`'s text is its `intent_done`, and the entry
            // says whether it names a pull request (false in a log written before 1.12, D.7.6).
            EntryBody::IntentChecked { pr_named, .. } => {
                if let Some(n) = self.s.nodes.get_mut(&node) {
                    n.pr_named = *pr_named;
                }
            }
            // 1.11 (§11.3): a deferral marks its objection while it is open; it decides nothing.
            EntryBody::Resolve {
                objection,
                deferred: true,
                ..
            } => {
                let o = *objection;
                if let Some(x) = self.s.objections.get_mut(&o)
                    && x.state == ObjectionState::Open
                {
                    x.deferred = Some(eid);
                    self.dirt.mark(|| Fact::Objection(o));
                }
            }
            EntryBody::Request {
                origin,
                conv,
                from_user,
                requester,
            } => {
                self.s.requests.insert(
                    eid,
                    Request {
                        node,
                        origin: origin.clone(),
                        conv: conv.clone(),
                        from_user: *from_user,
                        requester: *requester,
                        answered_by: None,
                        settled: None,
                        moved_from: None,
                    },
                );
                ix_request(self.s, eid, true);
                self.dirt.mark(|| Fact::Request(eid));
                self.refresh(node);
            }
            EntryBody::Question(QuestionBody::People {
                conv,
                blocking,
                said,
                supersedes: _,
                recheck_at,
            }) => {
                self.s.pquestions.insert(
                    eid,
                    PQuestion {
                        node,
                        conv: conv.clone(),
                        blocking: *blocking,
                        said: *said,
                        state: PqState::Open,
                        rechecks: 0,
                        recheck_at: *recheck_at,
                        lapse: None,
                        assumed_by: None,
                        moved_from: None,
                    },
                );
                ix_pquestion(self.s, eid, true);
                self.dirt.mark(|| Fact::PQuestion(eid));
                self.owner(Owner::Entry(eid));
                self.refresh(node);
            }
            EntryBody::Question(QuestionBody::Agent {
                to,
                until,
                recipients,
                pending_holder,
            }) => {
                self.s.aquestions.insert(
                    eid,
                    AQuestion {
                        node,
                        asker: author_session(by),
                        state: AqState::Open,
                    },
                );
                ix_aquestion(self.s, eid, true);
                self.dirt.mark(|| Fact::AQuestion(eid));
                self.insert_note(eid, node, *to, *until, recipients.clone(), *pending_holder);
            }
            EntryBody::Question(QuestionBody::User {
                source,
                about,
                to,
                due,
                has_default,
            }) => {
                self.s.uquestions.insert(
                    eid,
                    UQuestion {
                        source: *source,
                        about: *about,
                        to: to.clone(),
                        stage: 0,
                        due: *due,
                        has_default: *has_default,
                        state: UqState::Pending,
                        assumed_by: None,
                    },
                );
                ix_uquestion(self.s, eid, true);
                self.dirt.mark(|| Fact::UQuestion(eid));
                self.owner(Owner::Entry(eid));
            }
            EntryBody::Answer {
                question: _,
                recipient,
                until,
            } => {
                self.insert_note(
                    eid,
                    node,
                    Addressee::Session(*recipient),
                    *until,
                    [*recipient].into_iter().collect(),
                    false,
                );
            }
            EntryBody::Decision {
                settles,
                supersedes: _,
                assumes: _,
                evidence: _,
                basis,
            } => {
                self.s.decisions.insert(
                    eid,
                    Decision {
                        node,
                        settles: settles.clone(),
                        state: DecisionState::Active,
                        basis: basis.clone(),
                    },
                );
                ix_decision(self.s, eid, true);
                self.dirt.mark(|| Fact::Decision(eid));
                self.refresh(node);
                for b in basis {
                    if let BasisRef::Entry(e) = b {
                        self.leave(EntityRef::Decision(*e));
                    }
                }
            }
            EntryBody::Checkpoint {
                token,
                release: _,
                pr_named: pr,
                handed_off_until,
            } => {
                // 1.12 (§10.2, §20.9 rule 8): whether its `intent_done`, if it closes an intent,
                // names a pull request, as the entry says (None in a log written before 1.12,
                // which replays to the state it wrote, D.7.6).
                let pr = *pr;
                if let Some(n) = self.s.nodes.get_mut(&node) {
                    n.checkpoints = n.checkpoints.saturating_add(1);
                    n.attempts = 0;
                    n.last_checkpoint = Some(eid);
                    // 1.4 (§10.4): a hold that expired before this checkpoint is no longer after
                    // the node's last one.
                    n.expired_hold = None;
                    if let Some(pr) = pr {
                        n.pr_named = pr;
                    }
                }
                // 1.12 (§10.4): a `handoff` release keeps the node from its releaser until the
                // time its entry carries, `lease.first` of the hold's terms after it, or until the
                // node's next hold (none in a log written before 1.12, D.7.6).
                if let Some(until) = *handed_off_until
                    && let Author::Session(x) = by
                    && let Some(y) = self.s.sessions.get_mut(&x)
                {
                    y.handed_off = Some(crate::state::HandedOff {
                        token: *token,
                        until,
                    });
                    self.dirt.mark(|| Fact::Session(x));
                }
                if let Some(h) = self.s.holds.get_mut(token) {
                    h.checkpointed = true;
                    let t = *token;
                    self.dirt.mark(|| Fact::Hold(t));
                }
                self.refresh(node);
            }
            EntryBody::Said {
                conv,
                kind,
                corrects: _,
                voice: _,
            } => {
                self.s.saids.insert(
                    eid,
                    Said {
                        node,
                        conv: conv.clone(),
                        kind: *kind,
                        session: author_session(by),
                        state: SaidState::Pending,
                        moved_from: None,
                        with_reply: false,
                    },
                );
                ix_said(self.s, eid, true);
                self.dirt.mark(|| Fact::Said(eid));
                self.refresh(node);
            }
            EntryBody::FromUser { principal } => {
                let session = author_session(by);
                self.s.user_entries.insert(
                    eid,
                    UserEntry {
                        session,
                        principal: *principal,
                        at,
                        routing: Routing::Unrouted,
                    },
                );
                ix_user_entry(self.s, eid, true);
                self.dirt.mark(|| Fact::UserEntry(eid));
                let cutoff = at.minus(self.s.params.user_gap_window);
                let msgs = self.s.user_msgs.entry(*principal).or_default();
                msgs.push(at);
                msgs.retain(|&t| t >= cutoff);
                let p = *principal;
                self.dirt.mark(|| Fact::UserMsgs(p));
                if let Some(x) = self.s.sessions.get_mut(&session)
                    && x.principals.insert(p)
                {
                    self.dirt.mark(|| Fact::Session(session));
                }
                self.routes.insert(eid);
                self.refresh(USER);
            }
            EntryBody::ToUser { principal, .. } => {
                // 1.4 (R13): the author session's `told` gains the principal, as named, which ends
                // rule 12 for its person (D.4.7, D.8.26).
                if let Author::Session(x) = by
                    && let Some(sess) = self.s.sessions.get_mut(&x)
                    && sess.told.insert(*principal)
                {
                    self.dirt.mark(|| Fact::Session(x));
                }
            }
            EntryBody::Approve { version } => {
                let c = self.s.approvals.entry((node, *version)).or_insert(0);
                *c = c.saturating_add(1);
                let v = *version;
                self.dirt.mark(|| Fact::Approval(node, v));
            }
            EntryBody::Object => {
                self.s.objections.insert(
                    eid,
                    Objection {
                        node,
                        state: ObjectionState::Open,
                        moved_from: None,
                        deferred: None,
                    },
                );
                ix_objection(self.s, eid, true);
                self.dirt.mark(|| Fact::Objection(eid));
                self.refresh(node);
            }
            EntryBody::Retract { target } => {
                if self.s.retracted.insert(*target) {
                    let t = *target;
                    self.dirt.mark(|| Fact::Retracted(t));
                }
                let mut targets = alloc::vec![*target];
                if let BasisTarget::Node(t) = target {
                    targets.push(BasisTarget::Node(resolve_merged(self.s, *t)));
                }
                for t in targets {
                    if let Some(ds) = self.s.ix.cited_by.get(&t) {
                        self.restale.extend(ds.iter().copied());
                    }
                }
            }
            EntryBody::System(SystemNote::Digest { item, to }) => {
                self.s.digest.insert(
                    eid,
                    DigestEntry {
                        item: *item,
                        to: *to,
                        state: DigestState::Pending,
                    },
                );
                ix_digest(self.s, eid, true);
                self.dirt.mark(|| Fact::Digest(eid));
                self.leave(EntityRef::Digest(eid));
            }
            EntryBody::System(
                SystemNote::Closed { .. }
                | SystemNote::Reopened
                | SystemNote::KeysDropped { .. }
                | SystemNote::MergedFrom { .. }
                | SystemNote::Moved { .. }
                | SystemNote::Renewed { .. }
                | SystemNote::ScheduleEnded
                | SystemNote::Paused { .. }
                | SystemNote::Unpaused
                | SystemNote::PauseExpired
                | SystemNote::Hidden(_)
                | SystemNote::Defaulted { .. },
            ) => {}
        }
        true
    }

    fn insert_note(
        &mut self,
        x: Eid,
        node: NodeId,
        to: Addressee,
        until: Time,
        recipients: BTreeSet<SessionId>,
        pending_holder: bool,
    ) {
        self.s.notes.insert(
            x,
            Note {
                node,
                to,
                until,
                recipients,
                pending_holder,
                read_by: BTreeSet::new(),
            },
        );
        ix_note(self.s, x, true);
        self.dirt.mark(|| Fact::Note(x));
    }

    // --------------------------------------------------------------------------------------------
    // Entity transitions

    fn request_answered(&mut self, r: Eid, by: Eid) -> bool {
        if self
            .s
            .requests
            .get(&r)
            .is_none_or(|x| x.answered_by.is_some())
        {
            return false;
        }
        ix_request(self.s, r, false);
        if let Some(x) = self.s.requests.get_mut(&r) {
            x.answered_by = Some(by);
        }
        ix_request(self.s, r, true);
        self.dirt.mark(|| Fact::Request(r));
        if let Some(x) = self.s.requests.get(&r) {
            let n = x.node;
            self.refresh(n);
        }
        self.leave(EntityRef::Said(by));
        self.leave(EntityRef::Meta(by));
        true
    }

    fn request_settled(&mut self, r: Eid, decision: DecisionId, answer: Eid) -> bool {
        let Some(x) = self.s.requests.get(&r) else {
            return false;
        };
        if x.settled.is_some()
            || !self
                .s
                .ix
                .settling
                .get(&r)
                .is_some_and(|ds| ds.contains(&decision))
        {
            return false;
        }
        ix_request(self.s, r, false);
        if let Some(x) = self.s.requests.get_mut(&r) {
            x.settled = Some(Settlement { decision, answer });
        }
        ix_request(self.s, r, true);
        self.dirt.mark(|| Fact::Request(r));
        if let Some(x) = self.s.requests.get(&r) {
            let (n, from_user) = (x.node, x.from_user);
            self.refresh(n);
            if let Some(e) = from_user {
                self.leave(EntityRef::UserEntry(e));
            }
        }
        let clar: Vec<Eid> = self
            .s
            .ix
            .clarifications
            .get(&r)
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        for e in clar {
            self.leave(EntityRef::UserEntry(e));
        }
        let ds: Vec<DecisionId> = self
            .s
            .ix
            .settling
            .get(&r)
            .map(|x| x.iter().copied().collect())
            .unwrap_or_default();
        for d in ds {
            self.leave(EntityRef::Decision(d));
        }
        self.leave(EntityRef::Request(r));
        true
    }

    fn request_moved(&mut self, r: Eid, to: NodeId) -> bool {
        let Some(from) = self.s.requests.get(&r).map(|x| x.node) else {
            return false;
        };
        ix_request(self.s, r, false);
        if let Some(x) = self.s.requests.get_mut(&r) {
            x.moved_from = Some(from);
            x.node = to;
        }
        ix_request(self.s, r, true);
        self.dirt.mark(|| Fact::Request(r));
        self.refresh(from);
        self.refresh(to);
        true
    }

    /// A transition of people question `q` by `f` (which returns whether the precondition holds),
    /// with its indices, deadline, waiters, and leaving time.
    fn pq_change(&mut self, q: Eid, f: impl FnOnce(&mut PQuestion, Time) -> bool) -> bool {
        let Some(mut pq) = self.s.pquestions.get(&q).cloned() else {
            return false;
        };
        let from = pq.node;
        if !f(&mut pq, self.at) {
            return false;
        }
        ix_pquestion(self.s, q, false);
        let to = pq.node;
        self.s.pquestions.insert(q, pq);
        ix_pquestion(self.s, q, true);
        self.dirt.mark(|| Fact::PQuestion(q));
        self.owner(Owner::Entry(q));
        self.refresh(from);
        self.refresh(to);
        self.refresh_waiters(WaitKey::Question(q));
        self.leave(EntityRef::PQuestion(q));
        true
    }

    /// A transition of user question `q` by `f`, with its indices, deadline, waiters, and leaving
    /// time, and the leaving time of the people question it escalates.
    fn uq_change(&mut self, q: Eid, f: impl FnOnce(&mut UQuestion, Time) -> bool) -> bool {
        let Some(mut uq) = self.s.uquestions.get(&q).cloned() else {
            return false;
        };
        if !f(&mut uq, self.at) {
            return false;
        }
        let source = uq.source;
        ix_uquestion(self.s, q, false);
        self.s.uquestions.insert(q, uq);
        ix_uquestion(self.s, q, true);
        self.dirt.mark(|| Fact::UQuestion(q));
        self.owner(Owner::Entry(q));
        self.refresh_waiters(WaitKey::Question(q));
        self.leave(EntityRef::UQuestion(q));
        if let UqSource::Escalation { question } = source {
            self.leave(EntityRef::PQuestion(question));
        }
        true
    }

    fn commitment_opened(
        &mut self,
        c: Eid,
        node: NodeId,
        conv: &KeyStr,
        followup_at: Time,
    ) -> bool {
        let ack = self
            .s
            .saids
            .get(&c)
            .is_some_and(|x| x.kind == crate::types::SaidKind::Ack);
        if !ack || self.s.commitments.contains_key(&c) {
            return false;
        }
        self.s.commitments.insert(
            c,
            Commitment {
                node,
                conv: conv.clone(),
                last_ack: self.at,
                followup_at,
                followup_fired: false,
                state: CommitmentState::Open,
                moved_from: None,
            },
        );
        ix_commitment(self.s, c, true);
        self.dirt.mark(|| Fact::Commitment(c));
        self.owner(Owner::Entry(c));
        self.refresh(node);
        true
    }

    fn commitment_change(&mut self, c: Eid, f: impl FnOnce(&mut Commitment, Time) -> bool) -> bool {
        let Some(mut x) = self.s.commitments.get(&c).cloned() else {
            return false;
        };
        let from = x.node;
        if !f(&mut x, self.at) {
            return false;
        }
        ix_commitment(self.s, c, false);
        let to = x.node;
        self.s.commitments.insert(c, x);
        ix_commitment(self.s, c, true);
        self.dirt.mark(|| Fact::Commitment(c));
        self.owner(Owner::Entry(c));
        self.refresh(from);
        self.refresh(to);
        self.leave(EntityRef::Commitment(c));
        true
    }

    fn said_change(&mut self, x: Eid, f: impl FnOnce(&mut Said) -> bool) -> bool {
        let Some(mut sd) = self.s.saids.get(&x).cloned() else {
            return false;
        };
        let from = sd.node;
        if !f(&mut sd) {
            return false;
        }
        ix_said(self.s, x, false);
        let to = sd.node;
        self.s.saids.insert(x, sd);
        ix_said(self.s, x, true);
        self.dirt.mark(|| Fact::Said(x));
        self.refresh(from);
        self.refresh(to);
        self.leave(EntityRef::Said(x));
        true
    }

    fn objection_change(&mut self, o: Eid, f: impl FnOnce(&mut Objection) -> bool) -> bool {
        let Some(mut x) = self.s.objections.get(&o).copied() else {
            return false;
        };
        let from = x.node;
        if !f(&mut x) {
            return false;
        }
        ix_objection(self.s, o, false);
        let to = x.node;
        self.s.objections.insert(o, x);
        ix_objection(self.s, o, true);
        self.dirt.mark(|| Fact::Objection(o));
        self.refresh(from);
        self.refresh(to);
        self.leave(EntityRef::Objection(o));
        true
    }

    fn entry_routed(&mut self, e: Eid, routing: Routing) -> bool {
        if self
            .s
            .user_entries
            .get(&e)
            .is_none_or(|u| u.routing != Routing::Unrouted)
        {
            return false;
        }
        ix_user_entry(self.s, e, false);
        if let Some(u) = self.s.user_entries.get_mut(&e) {
            u.routing = routing;
        }
        ix_user_entry(self.s, e, true);
        self.dirt.mark(|| Fact::UserEntry(e));
        self.routes.insert(e);
        self.refresh(USER);
        self.leave(EntityRef::UserEntry(e));
        true
    }

    /// `DecisionSuperseded` and `DecisionRetracted`: from `Active` to `state`, leaving `cited_by`,
    /// `stale_basis`, and, when retracted, `settling` (D.7.4).
    fn decision_end(&mut self, d: DecisionId, state: DecisionState) -> bool {
        let Some(dec) = self.s.decisions.get(&d).cloned() else {
            return false;
        };
        if dec.state != DecisionState::Active {
            return false;
        }
        ix_decision(self.s, d, false);
        if let Some(x) = self.s.decisions.get_mut(&d) {
            x.state = state;
        }
        ix_decision(self.s, d, true);
        self.dirt.mark(|| Fact::Decision(d));
        self.refresh(dec.node);
        if let Some(ds) = self.s.ix.cited_by.get(&BasisTarget::Entry(d)) {
            self.restale.extend(ds.iter().copied());
        }
        self.leave(EntityRef::Decision(d));
        for b in &dec.basis {
            if let BasisRef::Entry(e) = b {
                self.leave(EntityRef::Decision(*e));
            }
        }
        true
    }

    fn digest_shown(&mut self, d: Eid, by: Eid) -> bool {
        let Some(x) = self.s.digest.get(&d).copied() else {
            return false;
        };
        let to_user = self.s.entries.get(&by).is_some_and(|m| {
            m.ty == crate::types::EntryType::ToUser
                && m.principal.is_some_and(|p| same_person(self.s, p, x.to))
        });
        if x.state != DigestState::Pending || !to_user {
            return false;
        }
        ix_digest(self.s, d, false);
        if let Some(e) = self.s.digest.get_mut(&d) {
            e.state = DigestState::Shown { by };
        }
        ix_digest(self.s, d, true);
        self.dirt.mark(|| Fact::Digest(d));
        self.reset_leaving(EntityRef::Digest(d));
        true
    }

    // --------------------------------------------------------------------------------------------
    // Refusals, pauses, notes, retries

    fn refused(
        &mut self,
        n: NodeId,
        class: &ClassName,
        session: SessionId,
        backoff_until: Time,
        expires_at: Time,
        quorum: u32,
    ) -> bool {
        if !(self.s.nodes.contains_key(&n)
            && self.at < backoff_until
            && backoff_until <= expires_at)
        {
            return false;
        }
        let r = self
            .s
            .refusals
            .entry((n, class.clone()))
            .or_insert_with(|| Refusal {
                quorum,
                by: alloc::collections::BTreeMap::new(),
            });
        r.by.insert(
            session,
            RefusalEntry {
                backoff_until,
                expires_at,
            },
        );
        put_in(&mut self.s.ix.refused, n, class.clone(), true);
        let c = class.clone();
        self.dirt.mark(|| Fact::Refusal(n, c));
        self.owner(Owner::Refusal(n, class.clone()));
        true
    }

    fn refusal_expired(&mut self, n: NodeId, class: &ClassName) -> bool {
        let key = (n, class.clone());
        let at = self.at;
        let Some(r) = self.s.refusals.get_mut(&key) else {
            return false;
        };
        if !r.by.values().any(|x| x.expires_at <= at) {
            return false;
        }
        r.by.retain(|_, x| x.expires_at > at);
        if r.by.is_empty() {
            self.s.refusals.remove(&key);
            put_in(&mut self.s.ix.refused, n, class.clone(), false);
        }
        let c = class.clone();
        self.dirt.mark(|| Fact::Refusal(n, c));
        self.owner(Owner::Refusal(n, class.clone()));
        true
    }

    fn refusals_cleared(&mut self, n: NodeId) {
        let classes: Vec<ClassName> = self
            .s
            .refusals
            .keys()
            .filter(|(m, _)| *m == n)
            .map(|(_, c)| c.clone())
            .collect();
        for c in classes {
            self.s.refusals.remove(&(n, c.clone()));
            let cc = c.clone();
            self.dirt.mark(|| Fact::Refusal(n, cc));
            self.owner(Owner::Refusal(n, c));
        }
        self.s.ix.refused.remove(&n);
    }

    fn paused(
        &mut self,
        scope: NodeId,
        until: Time,
        ttl: Dur,
        by: SessionId,
        class: &ClassName,
    ) -> bool {
        if !self.s.nodes.contains_key(&scope) {
            return false;
        }
        let new = !self.s.pauses.contains_key(&scope);
        self.s.pauses.insert(
            scope,
            Pause {
                until,
                last_renewed: self.at,
                ttl,
                by,
                class: class.clone(),
            },
        );
        self.dirt.mark(|| Fact::Pause(scope));
        self.owner(Owner::Node(scope));
        if new {
            self.refresh_under(scope);
        }
        true
    }

    fn unpaused(&mut self, scope: NodeId) -> bool {
        if self.s.pauses.remove(&scope).is_none() {
            return false;
        }
        self.dirt.mark(|| Fact::Pause(scope));
        self.owner(Owner::Node(scope));
        self.refresh_under(scope);
        true
    }

    fn note_bound(&mut self, x: Eid, session: SessionId) -> bool {
        if !self.s.notes.get(&x).is_some_and(|n| n.pending_holder) {
            return false;
        }
        ix_note(self.s, x, false);
        if let Some(n) = self.s.notes.get_mut(&x) {
            n.recipients.insert(session);
            n.pending_holder = false;
        }
        ix_note(self.s, x, true);
        self.dirt.mark(|| Fact::Note(x));
        true
    }

    fn op_recorded(
        &mut self,
        class: &ClassName,
        op: &crate::types::OpId,
        digest: crate::types::ReqDigest,
        session: SessionId,
        expires_at: Time,
    ) -> bool {
        let key = (class.clone(), op.clone());
        if self.s.ops.contains_key(&key)
            || expires_at <= self.at
            || !self.s.sessions.contains_key(&session)
        {
            return false;
        }
        self.s.ops.insert(
            key,
            OpRecord {
                digest,
                session,
                expires_at,
            },
        );
        self.s
            .ix
            .ops_expiry
            .insert((expires_at, class.clone(), op.clone()));
        let (c, o) = (class.clone(), op.clone());
        self.dirt.mark(|| Fact::Op(c, o));
        true
    }

    // --------------------------------------------------------------------------------------------
    // Memory (D-5)

    fn restored(&mut self, items: &[Restored]) -> bool {
        let mut seen = BTreeSet::new();
        if !items
            .iter()
            .all(|x| restorable(self.s, x) && seen.insert(restored_ref(x)))
        {
            return false;
        }
        for item in items {
            let x = restored_ref(item);
            match item {
                Restored::Request(id, v, m) => {
                    self.s.requests.insert(*id, v.clone());
                    self.restore_meta(*id, m);
                    ix_request(self.s, *id, true);
                    self.dirt.mark(|| Fact::Request(*id));
                    self.refresh(v.node);
                    // Its answer is held back again (D.4.12).
                    if let Some(a) = v.answered_by {
                        self.leave(EntityRef::Said(a));
                        self.leave(EntityRef::Meta(a));
                    }
                }
                Restored::PQuestion(id, v, m) => {
                    self.s.pquestions.insert(*id, v.clone());
                    self.restore_meta(*id, m);
                    ix_pquestion(self.s, *id, true);
                    self.dirt.mark(|| Fact::PQuestion(*id));
                    self.owner(Owner::Entry(*id));
                    self.refresh(v.node);
                    // The user question it escalated to is held back again (D.4.12).
                    let escalations: Vec<Eid> = self
                        .s
                        .uquestions
                        .iter()
                        .filter(|(_, u)| u.source == UqSource::Escalation { question: *id })
                        .map(|(&u, _)| u)
                        .collect();
                    for u in escalations {
                        self.leave(EntityRef::UQuestion(u));
                    }
                }
                Restored::UQuestion(id, v, m) => {
                    self.s.uquestions.insert(*id, v.clone());
                    self.restore_meta(*id, m);
                    ix_uquestion(self.s, *id, true);
                    self.dirt.mark(|| Fact::UQuestion(*id));
                    self.owner(Owner::Entry(*id));
                }
                Restored::Commitment(id, v, m) => {
                    self.s.commitments.insert(*id, v.clone());
                    self.restore_meta(*id, m);
                    ix_commitment(self.s, *id, true);
                    self.dirt.mark(|| Fact::Commitment(*id));
                    self.owner(Owner::Entry(*id));
                    self.refresh(v.node);
                }
                Restored::Said(id, v, m) => {
                    self.s.saids.insert(*id, v.clone());
                    self.restore_meta(*id, m);
                    ix_said(self.s, *id, true);
                    self.dirt.mark(|| Fact::Said(*id));
                    self.refresh(v.node);
                }
                Restored::Decision(id, v, m) => {
                    self.s.decisions.insert(*id, v.clone());
                    self.restore_meta(*id, m);
                    ix_decision(self.s, *id, true);
                    self.dirt.mark(|| Fact::Decision(*id));
                    self.refresh(v.node);
                    // A decision that is no longer active makes its citers' references stale
                    // (D.8.17): the basis refresh of `Entry(id)`.
                    if let Some(ds) = self.s.ix.cited_by.get(&BasisTarget::Entry(*id)) {
                        self.restale.extend(ds.iter().copied());
                    }
                }
                Restored::Objection(id, v, m) => {
                    self.s.objections.insert(*id, *v);
                    self.restore_meta(*id, m);
                    ix_objection(self.s, *id, true);
                    self.dirt.mark(|| Fact::Objection(*id));
                    self.refresh(v.node);
                }
                Restored::UserEntry(id, v, m) => {
                    self.s.user_entries.insert(*id, *v);
                    self.restore_meta(*id, m);
                    ix_user_entry(self.s, *id, true);
                    self.dirt.mark(|| Fact::UserEntry(*id));
                    self.routes.insert(*id);
                    self.refresh(USER);
                }
                Restored::Digest(id, v, m) => {
                    self.s.digest.insert(*id, *v);
                    self.restore_meta(*id, m);
                    ix_digest(self.s, *id, true);
                    self.dirt.mark(|| Fact::Digest(*id));
                    if v.state == DigestState::Pending
                        && let Some(c) = self.s.digest_left.get_mut(&v.to)
                    {
                        *c = c.saturating_sub(1);
                        if *c == 0 {
                            self.s.digest_left.remove(&v.to);
                        }
                        let to = v.to;
                        self.dirt.mark(|| Fact::DigestLeft(to));
                    }
                }
                Restored::Meta(id, m) => self.restore_meta(*id, m),
            }
            if let Some(home) = home_of(self.s, x)
                && let Some(n) = self.s.nodes.get_mut(&home)
            {
                n.left = n.left.saturating_sub(1);
                self.hot(home);
            }
            self.leave(x);
        }
        true
    }

    fn restore_meta(&mut self, e: Eid, m: &EntryMeta) {
        if self.s.entries.contains_key(&e) {
            return;
        }
        self.s.entries.insert(e, *m);
        ix_meta(self.s, e, true);
        self.dirt.mark(|| Fact::Entry(e));
        if bears_none(m.ty) {
            self.leave(EntityRef::Meta(e));
        }
    }
}

/// `NodeUpdated`'s precondition: every present field differs from the current value (O2). A
/// `repeat` on an occurrence or a signal node is not a role change of D.7.4 and counts as an
/// anomaly (`decide` refuses it with `invalid(RoleConflict)`).
pub(crate) fn patch_differs(node: &Node, p: &NodePatch) -> bool {
    fn change<T: PartialEq + Clone>(c: &Change<T>, cur: Option<&T>) -> bool {
        match c {
            Change::Keep => true,
            Change::Clear => cur.is_some(),
            Change::Set(v) => cur != Some(v),
        }
    }
    let repeat = match (&p.repeat, &node.role) {
        (Change::Keep, _) => true,
        (Change::Clear, Role::Series(_)) => true,
        (Change::Set(r), Role::Series(x)) => *r != x.repeat,
        (Change::Set(_), Role::Plain) => true,
        (Change::Clear, Role::Plain)
        | (Change::Clear | Change::Set(_), Role::Occurrence(_) | Role::Signal(_)) => false,
    };
    p.title.as_ref().is_none_or(|v| *v != node.title)
        && p.body.as_ref().is_none_or(|v| *v != node.body)
        && p.fields.as_ref().is_none_or(|v| *v != node.fields)
        && p.files.as_ref().is_none_or(|v| *v != node.files)
        && p.status.as_ref().is_none_or(|v| *v != node.status)
        && p.digest.as_ref().is_none_or(|v| *v != node.digest)
        && p.agreements.as_ref().is_none_or(|v| *v != node.agreements)
        && change(&p.kind, node.kind.as_ref())
        && p.tags.as_ref().is_none_or(|v| *v != node.tags)
        && p.capacity.is_none_or(|v| v != node.capacity)
        && p.priority.is_none_or(|v| v != node.priority)
        && change(&p.est, node.est.as_ref())
        && p.needs.as_ref().is_none_or(|v| *v != node.needs)
        && change(&p.due, node.due.as_ref())
        && p.depends_on.as_ref().is_none_or(|v| *v != node.depends_on)
        && p.owners.as_ref().is_none_or(|v| *v != node.owners)
        && repeat
}

/// Which counters a `NodeUpdated` moves (§26 O2, A-3, CS-34).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(clippy::struct_excessive_bools, reason = "one flag per counter of O2")]
pub(crate) struct Bumps {
    /// `version += 1`: title, body, fields, files, or status present.
    pub(crate) version: bool,
    /// `content_version := version` after the bump: title, body, fields, or files present.
    pub(crate) content_version: bool,
    /// `digest_version += 1`: the digest present.
    pub(crate) digest_version: bool,
}

/// The counters a patch moves (O2): `NodeUpdated` carries only the fields whose values differ, so
/// a present field is a changed one (D.7.2). Kind, tags, capacity, priority, `est`, needs, due,
/// `depends_on`, owners, `repeat`, and agreements move none (A-3).
pub(crate) fn counter_bumps(p: &NodePatch) -> Bumps {
    let content = p.title.is_some() || p.body.is_some() || p.fields.is_some() || p.files.is_some();
    Bumps {
        version: content || p.status.is_some(),
        content_version: content,
        digest_version: p.digest.is_some(),
    }
}

// ------------------------------------------------------------------------------------------------
// Leaving (D.4.12)

/// How long from now entity `x` stays in memory, if it is resident and leaves at all: a pending
/// digest entry `DIGEST_RETAIN`; an entity that is finished and not held back, and the metadata of
/// an entry that bears no entity and is not held back, `FINISHED_RETAIN` (the table of D.4.12).
pub(crate) fn leaving_retain(s: &State, x: EntityRef) -> Option<Dur> {
    let finished = match x {
        EntityRef::Request(r) => s.requests.get(&r)?.settled.is_some(),
        EntityRef::PQuestion(q) => {
            let pq = s.pquestions.get(&q)?;
            let done = match pq.state {
                PqState::Answered { .. }
                | PqState::Superseded { .. }
                | PqState::Withdrawn { .. } => true,
                PqState::Expired => pq.assumed_by.is_some(),
                PqState::Escalated { uq } => s.uquestions.get(&uq).is_none_or(uq_finished),
                PqState::Open => false,
            };
            done && !question_held(s, q)
        }
        EntityRef::UQuestion(q) => {
            let uq = s.uquestions.get(&q)?;
            let escalation_resident = matches!(
                uq.source,
                UqSource::Escalation { question } if s.pquestions.contains_key(&question)
            );
            (uq_finished(uq) || answered_late(s, uq))
                && !question_held(s, q)
                && !escalation_resident
        }
        EntityRef::Commitment(c) => {
            matches!(
                s.commitments.get(&c)?.state,
                CommitmentState::Fulfilled { .. }
            )
        }
        EntityRef::Said(a) => {
            matches!(
                s.saids.get(&a)?.state,
                SaidState::Confirmed | SaidState::Withheld
            ) && !answers(s, a)
        }
        EntityRef::Decision(d) => {
            let dec = s.decisions.get(&d)?;
            let done = dec.state != DecisionState::Active;
            let cited =
                s.ix.cited_by
                    .get(&BasisTarget::Entry(d))
                    .is_some_and(|x| !x.is_empty());
            let lists = !matches!(dec.state, DecisionState::Retracted { .. })
                && dec
                    .settles
                    .iter()
                    .any(|r| s.requests.get(r).is_some_and(|x| x.settled.is_none()));
            done && !cited && !lists
        }
        EntityRef::Objection(o) => {
            matches!(s.objections.get(&o)?.state, ObjectionState::Resolved { .. })
        }
        EntityRef::UserEntry(e) => {
            let u = s.user_entries.get(&e)?;
            let request = match u.routing {
                Routing::Unrouted => return None,
                Routing::Request { request, .. } | Routing::Clarifies { request } => Some(request),
                Routing::Answers { .. } | Routing::NoAction => None,
            };
            request.is_none_or(|r| s.requests.get(&r).is_none_or(|x| x.settled.is_some()))
        }
        EntityRef::Digest(d) => {
            return match s.digest.get(&d)?.state {
                DigestState::Pending => Some(DIGEST_RETAIN),
                DigestState::Shown { .. } => Some(FINISHED_RETAIN),
            };
        }
        EntityRef::Meta(e) => {
            let m = s.entries.get(&e)?;
            bears_none(m.ty) && !answers(s, e)
        }
    };
    finished.then_some(FINISHED_RETAIN)
}

/// A user question is finished once answered or defaulted, or lapsed with its assumption recorded.
fn uq_finished(uq: &UQuestion) -> bool {
    match uq.state {
        UqState::Answered { .. } | UqState::Defaulted => true,
        UqState::Lapsed => uq.assumed_by.is_some(),
        UqState::Pending => false,
    }
}

/// A lapsed escalation whose people question was answered after all is finished (D.4.12, the
/// year simulation's Y-1): the late answer withdrew the assumption. Its people question `q` holds
/// it back while `q` is resident; once `q` has left memory it was answered, since an escalated
/// question leaves only once its user question is finished, and only an open one is superseded.
fn answered_late(s: &State, uq: &UQuestion) -> bool {
    let UqSource::Escalation { question } = uq.source else {
        return false;
    };
    uq.state == UqState::Lapsed
        && uq.assumed_by.is_none()
        && s.pquestions
            .get(&question)
            .is_none_or(|pq| matches!(pq.state, PqState::Answered { .. }))
}

/// A wait or a mark names question `q` (D.4.12: held back).
fn question_held(s: &State, q: Eid) -> bool {
    s.ix.waiters
        .get(&WaitKey::Question(q))
        .is_some_and(|x| !x.is_empty())
        || s.ix
            .marked_by_question
            .get(&q)
            .is_some_and(|x| !x.is_empty())
}

/// Entry `a` is the `answered_by` of a resident request (D.4.12: held back). The frozen indices
/// have no reverse of `answered_by`, so this scans the resident requests (see the package report).
fn answers(s: &State, a: Eid) -> bool {
    s.requests.values().any(|r| r.answered_by == Some(a))
}

#[cfg(test)]
#[path = "evolve_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "evolve_random_tests.rs"]
mod random_tests;
