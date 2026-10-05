//! Naive definitions over the base maps only (design D.9.1), written directly from the
//! specification and never optimised: the oracles of the invariant checks and of the proptests.
//!
//! They share no predicate code with `apply` or `evolve`: `conv_node` walks `nodes[*].keys` rather
//! than `ix.keys_open` (§9.1); `flags` evaluates §14 over every entity rather than `ix.items`, and
//! `basis_changed` by `stale` over every reference of every decision rather than `cited_by`;
//! `all_refused` and `active` scan `refusals` and `classes` (§10.3); `chain` and `audience` walk
//! ancestors over `nodes` and scan `requests` (§8.4, §8.5); `classify` is the table of D.8.5 over
//! the base state; `ranked` sorts every candidate by the key of D.8.5 (the oracle of `ix.next`,
//! CS-16); `settled` is §26 Settled; `hello_items` scans every pending user question and digest
//! entry (U1's query property); `guide` is §20.9 over the base maps (`spec_guide.rs`, WP-D7).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::cmd::NextQuery;
use crate::fnv::tie_hash;
use crate::state::{
    BasisRef, BasisTarget, Cand, CommitmentState, DecisionState, DigestItem, DigestState,
    EntityRef, Life, Node, ObjectionState, OccPhase, Origin, PqState, Role, Routing, SaidState,
    State, UqSource, UqState, WaitOn,
};
use crate::types::{
    ClassName, Eid, EntryType, Flag, FlagSet, GONE, HOUR, KeyStr, Label, NodeId, NodeRef, OFFER,
    Outcome, PEOPLE, PrincipalId, RequestId, Requester, SessionId, TEAMS, Time, USER,
    USER_PRINCIPAL,
};
use crate::user::{UQ_OWNERS_MAX, UQ_REQUESTERS_MAX};

// `spec::guide` (§20.9, D.8.26): WP-D7's, in its own file.
#[path = "spec_guide.rs"]
mod naive_guide;
pub use naive_guide::{guide, guide_with, left_unfinished};

// ------------------------------------------------------------------------------------------------
// The tree

/// The parent walk of `n` over `nodes`, nearest first, at most `nodes.len()` steps.
#[must_use]
pub fn ancestors(s: &State, n: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = s
        .nodes
        .get(&n)
        .and_then(|x| x.place.as_ref())
        .map(|p| p.parent);
    while let Some(p) = cur {
        if out.len() >= s.nodes.len() {
            break;
        }
        out.push(p);
        cur = s
            .nodes
            .get(&p)
            .and_then(|x| x.place.as_ref())
            .map(|p| p.parent);
    }
    out
}

/// `n` and its ancestors, nearest first.
#[must_use]
pub fn ancestors_or_self(s: &State, n: NodeId) -> Vec<NodeId> {
    let mut out = alloc::vec![n];
    out.extend(ancestors(s, n));
    out
}

/// Resolves a node reference (§4): a path step by step through live names, then aliases; an id
/// must exist.
#[must_use]
pub fn resolve(s: &State, r: &NodeRef) -> Option<NodeId> {
    match r {
        NodeRef::Id(n) => s.nodes.contains_key(n).then_some(*n),
        NodeRef::Path(path) => {
            let mut cur = crate::types::ROOT;
            for step in &path.0 {
                let key = (cur, step.clone());
                cur = *s.names.get(&key).or_else(|| s.aliases.get(&key))?;
            }
            Some(cur)
        }
    }
}

/// Follows `merged_into` to the node a merged node lives on in, at most `nodes.len()` steps.
#[must_use]
pub fn resolve_merged(s: &State, n: NodeId) -> NodeId {
    let mut cur = n;
    for _ in 0..s.nodes.len() {
        match s.nodes.get(&cur).map(|x| &x.life) {
            Some(Life::Closed(c)) if c.merged_into.is_some() => {
                cur = c.merged_into.unwrap_or(cur);
            }
            Some(Life::Closed(_) | Life::Open) | None => break,
        }
    }
    cur
}

fn open(s: &State, n: NodeId) -> bool {
    s.nodes
        .get(&n)
        .is_some_and(|x| matches!(x.life, Life::Open))
}

// ------------------------------------------------------------------------------------------------
// Holds, waits, and flags (§10, §14)

/// The live holds on `n` (§26 `live(h, now)`).
#[must_use]
pub fn live_holds(s: &State, n: NodeId, now: Time) -> usize {
    s.holds
        .iter()
        .filter(|(t, h)| t.node == n && now < h.lease_until)
        .count()
}

/// A live hold on `n` or on a descendant of `n`.
#[must_use]
pub fn attended(s: &State, n: NodeId, now: Time) -> bool {
    s.holds
        .iter()
        .any(|(t, h)| now < h.lease_until && ancestors_or_self(s, t.node).contains(&n))
}

/// `resolved(w, now)` of §10.3, over the entity maps.
#[must_use]
pub fn resolved(s: &State, w: &WaitOn, now: Time) -> bool {
    match *w {
        WaitOn::Question { q, rechecks_at_set } => match s.pquestions.get(&q) {
            Some(pq) => pq.state != PqState::Open || pq.rechecks > rechecks_at_set,
            None => true,
        },
        WaitOn::UserQuestion(q) => match s.uquestions.get(&q) {
            Some(uq) => uq.state != UqState::Pending,
            None => true,
        },
        WaitOn::Node(t) => !open(s, t),
        WaitOn::Time(t) | WaitOn::External(t) => now >= t,
    }
}

/// `waiting(n, now)`: the node's wait is unresolved.
#[must_use]
pub fn waiting(s: &State, n: NodeId, now: Time) -> bool {
    s.nodes
        .get(&n)
        .and_then(|x| x.wait.as_ref())
        .is_some_and(|w| !resolved(s, &w.on, now))
}

/// `stale(s, b)` of §15 (D.8.17), with its own `resolve_merged`.
#[must_use]
pub fn stale(s: &State, b: &BasisRef) -> bool {
    match b {
        BasisRef::Node { id, life_at, .. } => {
            let f = resolve_merged(s, *id);
            let retracted = s.retracted.contains(&BasisTarget::Node(*id))
                || s.retracted.contains(&BasisTarget::Node(f));
            let changed = match s.nodes.get(&f).map(|x| &x.life) {
                Some(Life::Closed(c)) => {
                    Some(&c.outcome) != Some(life_at.as_ref().unwrap_or(&Outcome::Done))
                }
                Some(Life::Open) | None => false,
            };
            retracted || changed
        }
        BasisRef::Entry(e) => {
            s.retracted.contains(&BasisTarget::Entry(*e))
                || matches!(s.decisions.get(e), Some(d) if d.state != DecisionState::Active)
        }
        BasisRef::External { .. } => false,
    }
}

/// The fourteen flags of §14 on node `n` at `now`, each quantifier over every entity.
#[must_use]
pub fn flags(s: &State, n: NodeId, now: Time) -> FlagSet {
    let Some(node) = s.nodes.get(&n) else {
        return FlagSet::EMPTY;
    };
    let is_open = matches!(node.life, Life::Open);
    let att = attended(s, n, now);
    let held_self = live_holds(s, n, now) > 0;
    let wait = waiting(s, n, now);
    let mut f = FlagSet::EMPTY;
    if n == USER
        && s.user_entries
            .values()
            .any(|u| u.routing == Routing::Unrouted)
    {
        f.insert(Flag::Unrouted);
    }
    if s.requests
        .values()
        .any(|r| r.node == n && r.settled.is_none())
        && (!is_open || (!att && !wait))
    {
        f.insert(Flag::UnansweredRequest);
    }
    let open_pq = |conv: &KeyStr| {
        s.pquestions
            .values()
            .any(|q| q.node == n && q.state == PqState::Open && q.conv == *conv)
    };
    if s.commitments.values().any(|c| {
        c.node == n
            && c.state == CommitmentState::Open
            && (!is_open || (!att && now >= c.followup_at && !open_pq(&c.conv)))
    }) {
        f.insert(Flag::OpenCommitment);
    }
    if s.saids
        .values()
        .any(|x| x.node == n && x.state == SaidState::Unconfirmed)
        && !att
    {
        f.insert(Flag::UnconfirmedSaid);
    }
    if node.open_intent.is_some() && !held_self && !wait {
        f.insert(Flag::OpenIntent);
    }
    if !node.recheck_marks.is_empty() {
        f.insert(Flag::QuestionRecheck);
    }
    if !node.assume_marks.is_empty() {
        f.insert(Flag::ProceedOnAssumption);
    }
    if s.objections
        .values()
        .any(|o| o.node == n && o.state == ObjectionState::Open)
    {
        f.insert(Flag::Objection);
    }
    let merged = matches!(&node.life, Life::Closed(c) if c.merged_into.is_some());
    if !merged
        && s.decisions.values().any(|d| {
            d.node == n && d.state == DecisionState::Active && d.basis.iter().any(|b| stale(s, b))
        })
    {
        f.insert(Flag::BasisChanged);
    }
    if is_open {
        if node.attempts >= s.params.too_big_attempts {
            f.insert(Flag::TooBig);
        }
        if node.due.is_some_and(|d| now >= d) {
            f.insert(Flag::Overdue);
        }
        let children: Vec<&Node> = s
            .nodes
            .values()
            .filter(|c| c.place.as_ref().is_some_and(|p| p.parent == n))
            .collect();
        // 1.6 (§14): only a node held at least once, which its fence counts; a folder never is.
        if node.origin != Origin::Reserved
            && node.fence.0 > 0
            && !children.is_empty()
            && !children.iter().any(|c| matches!(c.life, Life::Open))
        {
            f.insert(Flag::ReadyToClose);
        }
        if matches!(&node.role, Role::Series(x) if x.last_missed) {
            f.insert(Flag::Missed);
        }
        // 1.8 (§14): an offer of a node that is not a container, with a slot, unheld, not
        // waiting, and untouched for `offer.stale`.
        let directory = node
            .place
            .as_ref()
            .is_some_and(|p| p.parent == PEOPLE || p.parent == TEAMS);
        let container =
            node.origin == Origin::Reserved || matches!(node.role, Role::Series(_)) || directory;
        if node.tags.iter().any(|t| t.as_str() == OFFER)
            && node.capacity > 0
            && !container
            && !held_self
            && !wait
            && s.params.stale_at(node.touched_at).is_some_and(|t| now >= t)
        {
            f.insert(Flag::Stale);
        }
    }
    f
}

// ------------------------------------------------------------------------------------------------
// Candidates and their order (§10.4, D.8.5)

/// The classification of D.8.5 over the base state: the first row that matches. 1.9 (§10.4): a
/// closed node with an open objection is a review item, a `Cand::Status` with no hold.
#[must_use]
pub fn classify(s: &State, n: NodeId, now: Time) -> Option<Cand> {
    let node = s.nodes.get(&n)?;
    if node.origin == Origin::Reserved {
        return None;
    }
    let fl = flags(s, n, now);
    let occ_waiting = matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting);
    match &node.life {
        Life::Closed(c) => {
            if c.upheld.is_some() {
                return None;
            }
            let locked = match &node.role {
                Role::Occurrence(o) => s.nodes.get(&o.series).is_some_and(|x| {
                    matches!(x.life, Life::Open) && matches!(x.role, Role::Series(_))
                }),
                Role::Plain | Role::Series(_) | Role::Signal(_) => false,
            };
            let take = [
                Flag::UnansweredRequest,
                Flag::OpenCommitment,
                Flag::Objection,
                Flag::OpenIntent,
                Flag::UnconfirmedSaid,
                Flag::BasisChanged,
            ]
            .iter()
            .any(|f| fl.contains(*f));
            if locked || c.merged_into.is_some() {
                return None;
            }
            // 1.9 (§10.4): a closed node with an open objection is a review item, with no hold.
            if fl.contains(Flag::Objection) {
                return Some(Cand::Status(n));
            }
            (node.capacity > 0 && take).then_some(Cand::Node(n))
        }
        Life::Open => {
            if occ_waiting {
                return None;
            }
            if waiting(s, n, now) {
                return fl.contains(Flag::OpenCommitment).then_some(Cand::Status(n));
            }
            let directory = node
                .place
                .as_ref()
                .is_some_and(|p| p.parent == PEOPLE || p.parent == TEAMS);
            let container = matches!(node.role, Role::Series(_)) || directory;
            let offered = node.tags.iter().any(|t| t.as_str() == OFFER) && !container;
            let available =
                matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Available);
            let free = u32::try_from(live_holds(s, n, now)).unwrap_or(u32::MAX) < node.capacity;
            (free && (offered || !fl.is_empty() || available)).then_some(Cand::Node(n))
        }
    }
}

/// `ix.next.waiting` over the base state (1.4, §10.4, D.8.5): the open nodes that `classify` would
/// make a `Cand::Node` but for an unresolved wait, that is, waiting nodes that are not reserved, not
/// occurrences before their slot, not status items (`open_commitment`), with a free slot, and
/// offered (not a container), flagged, or an available occurrence.
#[must_use]
pub fn waiting_nodes(s: &State, now: Time) -> BTreeSet<NodeId> {
    let mut out = BTreeSet::new();
    for (&n, node) in &s.nodes {
        if node.origin == Origin::Reserved || !matches!(node.life, Life::Open) {
            continue;
        }
        if matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting) {
            continue;
        }
        if !waiting(s, n, now) {
            continue;
        }
        let fl = flags(s, n, now);
        if fl.contains(Flag::OpenCommitment) {
            continue;
        }
        let directory = node
            .place
            .as_ref()
            .is_some_and(|p| p.parent == PEOPLE || p.parent == TEAMS);
        let container = matches!(node.role, Role::Series(_)) || directory;
        let offered = node.tags.iter().any(|t| t.as_str() == OFFER) && !container;
        let available = matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Available);
        let free = u32::try_from(live_holds(s, n, now)).unwrap_or(u32::MAX) < node.capacity;
        if free && (offered || !fl.is_empty() || available) {
            out.insert(n);
        }
    }
    out
}

/// `paused(n, now)` of D.8.21: a pause over `n` or an ancestor that has not ended.
#[must_use]
pub fn paused(s: &State, n: NodeId, now: Time) -> bool {
    ancestors_or_self(s, n)
        .iter()
        .any(|a| s.pauses.get(a).is_some_and(|p| p.until > now))
}

/// The key of D.8.5: `(longer, tier, stale, est, request, recheck, needs, review, −aged, h, id,
/// kind)`, `needs` negated (1.11), `longer` 1 for a node longer than every session's horizon
/// (1.12).
type RankKey = (u8, u8, u8, u8, u8, u8, i64, u8, i64, u64, u64, u8);

/// 1.12 (§10.4), naively: the longest horizon of the session profiles of `stats.profiles` seen
/// within `capacity.window` before `now` whose caps meet `needs`: None when one of them has no
/// horizon, 0 when none meets them.
#[must_use]
pub fn longest_horizon(s: &State, needs: &BTreeSet<Label>, now: Time) -> Option<u32> {
    let since = now.minus(s.params.capacity_window);
    let mut longest = Some(0_u32);
    for ((caps, _), p) in &s.stats.profiles {
        if p.last_seen < since || !needs.is_subset(caps) {
            continue;
        }
        longest = match (longest, p.max_horizon) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (None, _) | (_, None) => None,
        };
    }
    longest
}

/// Every candidate that session `sess` could get from `next {query}` at `now`, best first, by a
/// brute-force sort on the key `(tier, stale, est, request, recheck, needs, review, −aged, h, id,
/// kind)` of D.8.5, 1.7, 1.8, 1.10, and 1.11 (`stale` 1 for a node flagged `stale`, `est` 1 for a
/// node to take without `est` when the session has a horizon, `request` 0 for a candidate flagged
/// `unanswered_request` and for a routing item, `recheck` 0 for a node whose `{external}` wait has
/// resolved by `now`, `needs` the number of its node's needs, at most 15, negated, `review` 0 for an
/// item with no hold on a closed node, whose `aged` is its priority alone and whose tier is its
/// node's without `objection`): the oracle of `ix.next` (CS-16). Candidates held back (an open
/// `depends_on` target, a pause, 1.11: a series whose open occurrence waits for its slot) and those
/// excluded for the caller (its class's refusals, needs, horizon, its own holds, the query) are
/// left out. 1.12 (§10.4): so is what the caller put down, the review of an objection it raised
/// and the node it released as `handoff` (until another hold of it, or `lease.first`); and a node
/// to take whose `est` exceeds the horizon of the caller and of every profile seen within
/// `capacity.window` that meets its non-empty needs is kept, after every other candidate.
#[must_use]
pub fn ranked(s: &State, sess: SessionId, now: Time, q: &NextQuery) -> Vec<Cand> {
    let Some(session) = s.sessions.get(&sess) else {
        return Vec::new();
    };
    let under = q.under.as_ref().map(|r| resolve(s, r));
    if matches!(under, Some(None)) {
        return Vec::new();
    }
    let under = under.flatten();
    let query_empty = q.under.is_none() && q.kind.is_none() && q.tags.is_empty();
    let hour = |t: Time| i64::try_from(t.0.checked_div(HOUR.0).unwrap_or(0)).unwrap_or(i64::MAX);
    let mut out: Vec<(RankKey, Cand)> = Vec::new();
    for (&n, node) in &s.nodes {
        let Some(c) = classify(s, n, now) else {
            continue;
        };
        let depends = node.depends_on.iter().any(|d| open(s, *d));
        // 1.11 (§10.4, §12): a series whose open occurrence waits for its slot.
        let before_slot = node.is_open()
            && matches!(node.role, Role::Series(_))
            && s.nodes.values().any(|x| {
                x.is_open()
                    && matches!(&x.role, Role::Occurrence(o) if o.series == n && o.phase == OccPhase::Waiting)
            });
        if depends || before_slot || paused(s, n, now) {
            continue;
        }
        // 1.12: over the caller's horizon, kept only when longer than every session's.
        let over = matches!((node.est, session.horizon), (Some(e), Some(h)) if e > h);
        let longer = over
            && matches!(c, Cand::Node(_))
            && !node.needs.is_empty()
            && node
                .est
                .is_some_and(|e| longest_horizon(s, &node.needs, now).is_some_and(|l| e > l));
        if excluded(s, n, &session.class, now)
            || !node.needs.is_subset(&session.caps)
            || (over && !longer)
        {
            continue;
        }
        // 1.12: the review of the caller's own objection, and the node it handed off.
        let own_objection = matches!(c, Cand::Status(_))
            && !node.is_open()
            && s.objections
                .iter()
                .filter(|(_, o)| o.node == n && o.state == ObjectionState::Open)
                .map(|(o, _)| *o)
                .min()
                .and_then(|o| s.entries.get(&o))
                .is_some_and(|e| e.by == crate::state::Author::Session(sess));
        let handed = matches!(c, Cand::Node(_))
            && session
                .handed_off
                .is_some_and(|h| h.token.node == n && now < h.until && node.fence == h.token.fence);
        if own_objection || handed {
            continue;
        }
        if matches!(c, Cand::Node(_))
            && s.holds
                .iter()
                .any(|(t, h)| t.node == n && h.session == sess && now < h.lease_until)
        {
            continue;
        }
        if under.is_some_and(|u| !ancestors_or_self(s, n).contains(&u))
            || q.kind
                .as_ref()
                .is_some_and(|k| node.kind.as_ref() != Some(k))
            || !q.tags.is_subset(&node.tags)
        {
            continue;
        }
        let fl = flags(s, n, now);
        // 1.10 (§10.4): a review item; 1.11: its tier is its node's without `objection`.
        let review = matches!(c, Cand::Status(_)) && !node.is_open();
        let tf = if review {
            fl.without(Flag::Objection)
        } else {
            fl
        };
        let t0 = [
            Flag::Unrouted,
            Flag::UnansweredRequest,
            Flag::OpenCommitment,
            Flag::UnconfirmedSaid,
            Flag::OpenIntent,
            Flag::QuestionRecheck,
            Flag::ProceedOnAssumption,
            Flag::BasisChanged,
        ]
        .iter()
        .any(|f| tf.contains(*f));
        let window = matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Available && !o.window_fired);
        let tier = if t0 || window {
            0
        } else if node.priority > 0 || !tf.without(Flag::Stale).is_empty() {
            1
        } else {
            2
        };
        let stale = u8::from(fl.contains(Flag::Stale));
        let since = s.since.get(&c).copied().unwrap_or(now);
        // 1.11 (§10.4): work someone asked for, and work that needs more of the caller's caps.
        let request = u8::from(!fl.contains(Flag::UnansweredRequest));
        let needs = i64::try_from(node.needs.len().min(15))
            .unwrap_or(15)
            .saturating_neg();
        // 1.10 (§10.4): a review ranks by priority and the ring, not by age.
        let aged = if review {
            i64::from(node.priority)
        } else {
            i64::from(node.priority)
                .saturating_add(hour(now))
                .saturating_sub(hour(since))
        };
        let kind = if matches!(c, Cand::Node(_)) { 0 } else { 1 };
        // 1.10 (§10.4): under a horizon, a node to take without `est` after the estimated ones.
        let unestimated =
            u8::from(matches!(c, Cand::Node(_)) && node.est.is_none() && session.horizon.is_some());
        let recheck = u8::from(
            !(matches!(c, Cand::Node(_))
                && matches!(node.wait.as_ref().map(|w| w.on), Some(WaitOn::External(t)) if t <= now)),
        );
        out.push((
            (
                u8::from(longer),
                tier,
                stale,
                unestimated,
                request,
                recheck,
                needs,
                u8::from(!review),
                aged.saturating_neg(),
                tie_hash(sess, n.0),
                n.0,
                kind,
            ),
            c,
        ));
    }
    if query_empty {
        for (&e, u) in &s.user_entries {
            if u.routing != Routing::Unrouted {
                continue;
            }
            let c = Cand::Route(e);
            let since = s.since.get(&c).copied().unwrap_or(now);
            let aged = hour(now).saturating_sub(hour(since));
            out.push((
                (
                    0,
                    0,
                    0,
                    0,
                    0,
                    1,
                    0,
                    1,
                    aged.saturating_neg(),
                    tie_hash(sess, e.0),
                    e.0,
                    2,
                ),
                c,
            ));
        }
    }
    out.sort();
    out.into_iter().map(|(_, c)| c).collect()
}

// ------------------------------------------------------------------------------------------------
// Conversations (§9.1)

/// `conv_node(at, k)` of §9.1: the nearest node from `at` up to the root that carries `k`, open or
/// closed; else the open node that carries it; else `at`.
#[must_use]
pub fn conv_node(s: &State, at: NodeId, k: &KeyStr) -> NodeId {
    for m in ancestors_or_self(s, at) {
        if s.nodes.get(&m).is_some_and(|x| x.keys.contains_key(k)) {
            return m;
        }
    }
    for (&m, x) in &s.nodes {
        if matches!(x.life, Life::Open) && x.keys.contains_key(k) {
            return m;
        }
    }
    at
}

// ------------------------------------------------------------------------------------------------
// Refusals (§10.3, CS-7)

/// The class has refused `n`: its record has at least `quorum` live entries.
#[must_use]
pub fn class_refused(s: &State, n: NodeId, c: &ClassName, now: Time) -> bool {
    s.refusals.iter().any(|((m, k), r)| {
        *m == n
            && k == c
            && r.by.values().filter(|x| x.expires_at > now).count()
                >= usize::try_from(r.quorum).unwrap_or(usize::MAX)
    })
}

/// One of the class's sessions refused `n` less than `refusal.backoff` ago.
#[must_use]
pub fn backed_off(s: &State, n: NodeId, c: &ClassName, now: Time) -> bool {
    s.refusals
        .iter()
        .any(|((m, k), r)| *m == n && k == c && r.by.values().any(|x| x.backoff_until > now))
}

/// What `next` skips for a session of class `c` (D.8.14).
#[must_use]
pub fn excluded(s: &State, n: NodeId, c: &ClassName, now: Time) -> bool {
    class_refused(s, n, c, now) || backed_off(s, n, c, now)
}

/// `active(n, now)`: the classes with a caps profile ⊇ `n.needs` whose window ends after `now`.
#[must_use]
pub fn active(s: &State, n: NodeId, now: Time) -> BTreeSet<ClassName> {
    let Some(node) = s.nodes.get(&n) else {
        return BTreeSet::new();
    };
    s.classes
        .iter()
        .filter(|(_, a)| {
            a.caps_seen
                .iter()
                .any(|(caps, end)| *end > now && node.needs.is_subset(caps))
        })
        .map(|(c, _)| c.clone())
        .collect()
}

/// `all_refused(n, now)`: open, some class has refused it, and every active class has (§10.3).
#[must_use]
pub fn all_refused(s: &State, n: NodeId, now: Time) -> bool {
    if !open(s, n) {
        return false;
    }
    let refusers: BTreeSet<ClassName> = s
        .refusals
        .keys()
        .filter(|(m, c)| *m == n && class_refused(s, n, c, now))
        .map(|(_, c)| c.clone())
        .collect();
    !refusers.is_empty() && active(s, n, now).is_subset(&refusers)
}

// ------------------------------------------------------------------------------------------------
// Principals and the directory (§4, §8, D-3)

/// The person node of principal `p`: an open child of `@people` that carries `person:<key of p>`.
#[must_use]
pub fn person_of(s: &State, p: PrincipalId) -> Option<NodeId> {
    let key = s.principal_ids.iter().find(|(_, id)| **id == p)?.0;
    s.nodes.iter().find_map(|(&m, x)| {
        let person = matches!(x.life, Life::Open)
            && x.place.as_ref().is_some_and(|pl| pl.parent == PEOPLE)
            && x.keys.keys().any(|k| k.person().as_ref() == Some(key));
        person.then_some(m)
    })
}

/// The interned principals of a person node's `person:` keys.
fn principals_of(s: &State, m: NodeId) -> BTreeSet<PrincipalId> {
    let Some(x) = s.nodes.get(&m) else {
        return BTreeSet::new();
    };
    x.keys
        .keys()
        .filter_map(KeyStr::person)
        .filter_map(|k| s.principal_ids.get(&k).copied())
        .collect()
}

/// Every alias of `p`'s person, or `{p}`.
#[must_use]
pub fn aliases(s: &State, p: PrincipalId) -> BTreeSet<PrincipalId> {
    match person_of(s, p) {
        Some(m) => {
            let mut a = principals_of(s, m);
            a.insert(p);
            a
        }
        None => [p].into_iter().collect(),
    }
}

/// `p` and `q` are the same person (D.8.11).
#[must_use]
pub fn same_person(s: &State, p: PrincipalId, q: PrincipalId) -> bool {
    p == q || matches!((person_of(s, p), person_of(s, q)), (Some(a), Some(b)) if a == b)
}

/// `p`'s person is tagged `gone`.
#[must_use]
pub fn gone(s: &State, p: PrincipalId) -> bool {
    person_of(s, p)
        .and_then(|m| s.nodes.get(&m))
        .is_some_and(|x| x.tags.iter().any(|t| t.as_str() == GONE))
}

/// The requesters of the unsettled user requests on `m` alone, oldest request first, one stage per
/// person, skipping gone persons if asked, at most `UQ_REQUESTERS_MAX` (D.8.12).
#[must_use]
pub fn requesters_on(s: &State, m: NodeId, skip_gone: bool) -> Vec<PrincipalId> {
    let mut out: Vec<PrincipalId> = Vec::new();
    for r in s.requests.values() {
        if out.len() >= UQ_REQUESTERS_MAX {
            break;
        }
        let Requester::User(p) = r.requester else {
            continue;
        };
        if r.node != m || r.settled.is_some() {
            continue;
        }
        if out.iter().any(|&q| same_person(s, q, p)) || (skip_gone && gone(s, p)) {
            continue;
        }
        out.push(p);
    }
    out
}

/// The nearest node among `n` and its ancestors with an unsettled user request.
#[must_use]
pub fn nearest_requested(s: &State, n: Option<NodeId>) -> Option<NodeId> {
    let n = n?;
    ancestors_or_self(s, n).into_iter().find(|&m| {
        s.requests.values().any(|r| {
            r.node == m && r.settled.is_none() && matches!(r.requester, Requester::User(_))
        })
    })
}

/// The lowest interned alias of person node `m`.
fn rep(s: &State, m: NodeId) -> Option<PrincipalId> {
    principals_of(s, m).into_iter().next()
}

fn is_gone_node(s: &State, m: NodeId) -> bool {
    s.nodes
        .get(&m)
        .is_some_and(|x| x.tags.iter().any(|t| t.as_str() == GONE))
}

/// The owners of D.8.12: the `owners` of the nearest owned node among `n` (ROOT for none) and its
/// ancestors, then ROOT's, persons as themselves and teams as their own members, skipping gone
/// persons, one stage per person, at most `UQ_OWNERS_MAX`.
#[must_use]
pub fn owners(s: &State, n: Option<NodeId>) -> Vec<PrincipalId> {
    let start = n.unwrap_or(crate::types::ROOT);
    let nearest = ancestors_or_self(s, start)
        .into_iter()
        .find(|m| s.nodes.get(m).is_some_and(|x| !x.owners.is_empty()));
    let mut lists: Vec<BTreeSet<NodeId>> = Vec::new();
    if let Some(m) = nearest {
        lists.push(
            s.nodes
                .get(&m)
                .map(|x| x.owners.clone())
                .unwrap_or_default(),
        );
        if m != crate::types::ROOT {
            lists.push(
                s.nodes
                    .get(&crate::types::ROOT)
                    .map(|x| x.owners.clone())
                    .unwrap_or_default(),
            );
        }
    }
    let mut out: Vec<PrincipalId> = Vec::new();
    let push = |out: &mut Vec<PrincipalId>, p: PrincipalId| {
        if out.len() < UQ_OWNERS_MAX && !out.iter().any(|&q| same_person(s, q, p)) {
            out.push(p);
        }
    };
    let is_child_of = |x: NodeId, parent: NodeId| {
        s.nodes.get(&x).is_some_and(|y| {
            matches!(y.life, Life::Open) && y.place.as_ref().is_some_and(|pl| pl.parent == parent)
        })
    };
    for list in lists {
        for x in list {
            if is_child_of(x, PEOPLE) {
                if !is_gone_node(s, x)
                    && let Some(p) = rep(s, x)
                {
                    push(&mut out, p);
                }
            } else if is_child_of(x, TEAMS) {
                let members = s
                    .nodes
                    .get(&x)
                    .map(|y| y.owners.clone())
                    .unwrap_or_default();
                for m in members {
                    if is_child_of(m, PEOPLE)
                        && !is_gone_node(s, m)
                        && let Some(p) = rep(s, m)
                    {
                        push(&mut out, p);
                    }
                }
            }
        }
    }
    out
}

/// The addressee chain of a question for the user (D.8.12, U1): the explicit addressee, then the
/// requesters (a refusal question's own node's only), then the owners, then `user`, with no second
/// stage for one person and no stage before the last that is `user` or an alias of it.
#[must_use]
pub fn chain(
    s: &State,
    about: Option<NodeId>,
    explicit: Option<PrincipalId>,
    refusal: bool,
) -> Vec<PrincipalId> {
    let mut out: Vec<PrincipalId> = Vec::new();
    let push = |out: &mut Vec<PrincipalId>, p: PrincipalId| {
        if !out.iter().any(|&q| same_person(s, q, p)) && !same_person(s, p, USER_PRINCIPAL) {
            out.push(p);
        }
    };
    if let Some(p) = explicit {
        push(&mut out, p);
    }
    let requesters = if refusal {
        about.map(|n| requesters_on(s, n, true)).unwrap_or_default()
    } else {
        nearest_requested(s, about)
            .map(|m| requesters_on(s, m, true))
            .unwrap_or_default()
    };
    for p in requesters {
        push(&mut out, p);
    }
    for p in owners(s, about) {
        push(&mut out, p);
    }
    out.push(USER_PRINCIPAL);
    out
}

/// The audience of a digest item (D.8.13): the requesters whose work it concerns; else the persons
/// the question was put to; else `user`. Never empty.
#[must_use]
pub fn audience(s: &State, item: &DigestItem) -> BTreeSet<PrincipalId> {
    let nearest = |n: Option<NodeId>| {
        nearest_requested(s, n)
            .map(|m| requesters_on(s, m, false))
            .unwrap_or_default()
    };
    let req: Vec<PrincipalId> = match *item {
        DigestItem::RefusalDrop { node, .. } => {
            let mut out: Vec<PrincipalId> = Vec::new();
            for r in s.requests.values() {
                if let Requester::User(p) = r.requester
                    && r.node == node
                    && r.settled.is_none()
                    && !out.iter().any(|&q| same_person(s, q, p))
                {
                    out.push(p);
                }
            }
            out
        }
        DigestItem::DefaultApplied { question } | DigestItem::Lapsed { question } => {
            nearest(s.uquestions.get(&question).and_then(|u| u.about))
        }
        DigestItem::Assumption { question } => nearest(s.pquestions.get(&question).map(|q| q.node)),
    };
    if !req.is_empty() {
        return req.into_iter().collect();
    }
    let asked: Vec<PrincipalId> = match *item {
        DigestItem::DefaultApplied { question }
        | DigestItem::Lapsed { question }
        | DigestItem::RefusalDrop { question, .. } => {
            // The distinct persons asked, one principal each, the first in chain order: two stages
            // fixed at creation may have become aliases of one person since (D.8.13, D-3).
            let mut out: Vec<PrincipalId> = Vec::new();
            if let Some(u) = s.uquestions.get(&question) {
                for &p in u.to.iter().take(usize::from(u.stage).saturating_add(1)) {
                    if !same_person(s, p, USER_PRINCIPAL)
                        && !out.iter().any(|&q| same_person(s, q, p))
                    {
                        out.push(p);
                    }
                }
            }
            out
        }
        DigestItem::Assumption { question } => s
            .pquestions
            .get(&question)
            .map(|q| chain(s, Some(q.node), None, false))
            .and_then(|c| c.into_iter().find(|&p| !same_person(s, p, USER_PRINCIPAL)))
            .into_iter()
            .collect(),
    };
    if !asked.is_empty() {
        return asked.into_iter().collect();
    }
    [USER_PRINCIPAL].into_iter().collect()
}

// ------------------------------------------------------------------------------------------------
// Requests (§9.4, §26 Settled)

/// `settled(r)` as §26 defines when it holds: the request has an answer later than itself, and a
/// decision that is not retracted lists it (a superseded one counts).
#[must_use]
pub fn settled(s: &State, r: RequestId) -> bool {
    let Some(req) = s.requests.get(&r) else {
        return false;
    };
    req.answered_by.is_some_and(|a| a > r)
        && s.decisions
            .values()
            .any(|d| !matches!(d.state, DecisionState::Retracted { .. }) && d.settles.contains(&r))
}

// ------------------------------------------------------------------------------------------------
// hello's user items (§20.3, U1)

/// The pending questions and digest entries addressed to the persons of a session's principals,
/// under any alias, and the digest entries of those persons that left the pending list unshown.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HelloItems {
    /// Pending user questions whose current addressee is one of the persons.
    pub questions: BTreeSet<Eid>,
    /// Pending digest entries for one of the persons.
    pub digest: BTreeSet<Eid>,
    /// `Σ digest_left` over every alias of those persons.
    pub older: u64,
}

/// U1's query property, by a scan of every pending question and digest entry.
#[must_use]
pub fn hello_items(s: &State, sess: SessionId) -> HelloItems {
    let Some(x) = s.sessions.get(&sess) else {
        return HelloItems::default();
    };
    let serves = |p: PrincipalId| x.principals.iter().any(|&q| same_person(s, q, p));
    let mut out = HelloItems::default();
    for (&q, uq) in &s.uquestions {
        if uq.state == UqState::Pending
            && uq.to.get(usize::from(uq.stage)).is_some_and(|&p| serves(p))
        {
            out.questions.insert(q);
        }
    }
    for (&d, e) in &s.digest {
        if e.state == DigestState::Pending && serves(e.to) {
            out.digest.insert(d);
        }
    }
    let mut all: BTreeSet<PrincipalId> = BTreeSet::new();
    for &p in &x.principals {
        all.extend(aliases(s, p));
    }
    for p in all {
        out.older = out
            .older
            .saturating_add(s.digest_left.get(&p).copied().unwrap_or(0));
    }
    out
}

// ------------------------------------------------------------------------------------------------
// Memory (D.4.12, T3)

/// The resident entities that `leaving` must hold (T3, D.4.12): each that is finished and not held
/// back, the metadata of each entry that bears no entity and is not held back, and every pending
/// digest entry.
#[must_use]
pub fn leaving_expected(s: &State) -> BTreeSet<EntityRef> {
    let answered: BTreeSet<Eid> = s.requests.values().filter_map(|r| r.answered_by).collect();
    // The questions a wait or a mark names, and the entries an active decision cites: one pass each.
    let mut named: BTreeSet<Eid> = BTreeSet::new();
    for n in s.nodes.values() {
        named.extend(n.recheck_marks.iter().copied());
        named.extend(n.assume_marks.iter().copied());
        if let Some(w) = &n.wait
            && let WaitOn::Question { q, .. } | WaitOn::UserQuestion(q) = w.on
        {
            named.insert(q);
        }
    }
    let mut cited: BTreeSet<Eid> = BTreeSet::new();
    for d in s.decisions.values() {
        if d.state == DecisionState::Active {
            for b in &d.basis {
                if let BasisRef::Entry(e) = b {
                    cited.insert(*e);
                }
            }
        }
    }
    let named_by_wait_or_mark = |q: Eid| named.contains(&q);
    let uq_done = |u: &crate::state::UQuestion| match u.state {
        UqState::Answered { .. } | UqState::Defaulted => true,
        UqState::Lapsed => u.assumed_by.is_some(),
        UqState::Pending => false,
    };
    let mut out = BTreeSet::new();
    for (&r, x) in &s.requests {
        if x.settled.is_some() {
            out.insert(EntityRef::Request(r));
        }
    }
    for (&q, x) in &s.pquestions {
        let done = match x.state {
            PqState::Answered { .. } | PqState::Superseded { .. } | PqState::Withdrawn { .. } => {
                true
            }
            PqState::Expired => x.assumed_by.is_some(),
            PqState::Escalated { uq } => s.uquestions.get(&uq).is_none_or(uq_done),
            PqState::Open => false,
        };
        if done && !named_by_wait_or_mark(q) {
            out.insert(EntityRef::PQuestion(q));
        }
    }
    // A lapsed escalation whose people question was answered after all (Y-1): resident and
    // `Answered`, or gone, since only an answered one leaves while its escalation lapsed.
    let answered_late = |u: &crate::state::UQuestion| match u.source {
        UqSource::Escalation { question } => {
            u.state == UqState::Lapsed
                && u.assumed_by.is_none()
                && s.pquestions
                    .get(&question)
                    .is_none_or(|p| matches!(p.state, PqState::Answered { .. }))
        }
        UqSource::Agent { .. } | UqSource::Refusal { .. } => false,
    };
    for (&q, x) in &s.uquestions {
        let escalation = matches!(x.source, UqSource::Escalation { question } if s.pquestions.contains_key(&question));
        if (uq_done(x) || answered_late(x)) && !named_by_wait_or_mark(q) && !escalation {
            out.insert(EntityRef::UQuestion(q));
        }
    }
    for (&c, x) in &s.commitments {
        if matches!(x.state, CommitmentState::Fulfilled { .. }) {
            out.insert(EntityRef::Commitment(c));
        }
    }
    for (&a, x) in &s.saids {
        if matches!(x.state, SaidState::Confirmed | SaidState::Withheld) && !answered.contains(&a) {
            out.insert(EntityRef::Said(a));
        }
    }
    for (&d, x) in &s.decisions {
        if x.state == DecisionState::Active {
            continue;
        }
        let cited = cited.contains(&d);
        let lists = !matches!(x.state, DecisionState::Retracted { .. })
            && x.settles
                .iter()
                .any(|r| s.requests.get(r).is_some_and(|y| y.settled.is_none()));
        if !cited && !lists {
            out.insert(EntityRef::Decision(d));
        }
    }
    for (&o, x) in &s.objections {
        if matches!(x.state, ObjectionState::Resolved { .. }) {
            out.insert(EntityRef::Objection(o));
        }
    }
    for (&e, x) in &s.user_entries {
        let done = match x.routing {
            Routing::Unrouted => false,
            Routing::Request { request, .. } | Routing::Clarifies { request } => {
                s.requests.get(&request).is_none_or(|r| r.settled.is_some())
            }
            Routing::Answers { .. } | Routing::NoAction => true,
        };
        if done {
            out.insert(EntityRef::UserEntry(e));
        }
    }
    for &d in s.digest.keys() {
        out.insert(EntityRef::Digest(d));
    }
    for (&e, m) in &s.entries {
        let none = matches!(
            m.ty,
            EntryType::Answer
                | EntryType::Heard
                | EntryType::ToUser
                | EntryType::Approve
                | EntryType::Resolve
                | EntryType::Retract
        );
        if none && !answered.contains(&e) {
            out.insert(EntityRef::Meta(e));
        }
    }
    out
}
