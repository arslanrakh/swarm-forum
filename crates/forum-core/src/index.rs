//! The derived indices (design D.4.10) and their incremental maintenance by `evolve` (D.7.4).
//!
//! Two kinds of function live here:
//! - pure helpers that both `evolve` and the recount use, because they *define* a derived value
//!   rather than maintain one: the candidate classification of D.8.5 ([`classify`]), where a
//!   candidate sits ([`cand_at`], [`part_of`]), what holds it back ([`held_of`]), the demand estimate
//!   of §17, and the bit lengths of `est` and `horizon`. The invariant checks compare them with the
//!   naive definitions of `spec` (W1, F, and the proptests of D.12.3), so sharing them between
//!   `evolve` and `recount` hides no bug from the checks;
//! - the incremental maintenance itself, which `recount` does not share: each entity's
//!   contribution to every index is added or removed by one function with a `add` flag (`ix_*`), so
//!   that a transition removes the entity's old contribution, changes it, and adds the new one; and
//!   the node refresh of D.7.4 (flags, candidates, and the sets derived from them).
//!
//! Every set-valued index drops a key whose set becomes empty, and every count a key that reaches
//! zero, so that an index equals the recount, which creates no empty entry (R1).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::facts::Fact;
use crate::flags::{T0, flags_of, stale_decision, waiting};
use crate::fnv::fnv_id;
use crate::health::FALLBACK_EST_MIN;
use crate::state::{
    Cand, CandAt, CommitmentState, DecisionState, DigestState, EntityRef, Held, Ix, Life,
    NeedsTrie, NextIx, NextKey, Node, NodeItems, ObjectionState, OccPhase, Origin, PartKey,
    PqState, Role, Routing, SaidState, State, Subtree, UqRank, UqSource, UqState, WaitKey, WaitOn,
};
use crate::types::{
    Addressee, DAY, Eid, EntryType, FlagSet, HOUR, KeyStr, Label, NodeId, Outcome, PEOPLE,
    PrincipalId, RequestId, Requester, SessionId, Time, Token,
};

// ------------------------------------------------------------------------------------------------
// Pure helpers shared with the recount

/// The bit length of `x`: 0 for 0, else 1..=32.
#[must_use]
pub fn bit_len(x: u32) -> u8 {
    u8::try_from(u32::BITS.saturating_sub(x.leading_zeros())).unwrap_or(32)
}

/// A candidate's `est` bucket (D.4.10): 0 without `est`, else the bit length of `est`.
#[must_use]
pub fn est_bucket(est: Option<u32>) -> u8 {
    est.map_or(0, bit_len)
}

/// A session profile's horizon key (D.4.9): the bit length of the horizon, 33 for none.
#[must_use]
pub fn horizon_bits(h: Option<u32>) -> u8 {
    h.map_or(33, bit_len)
}

/// `day(t) = t / 86 400 000`: the UTC day index of the statistics (D.2.1).
#[must_use]
pub fn day_of(t: Time) -> u64 {
    t.0.checked_div(DAY.0).unwrap_or(0)
}

/// The UTC minute of a session start (D.4.9).
#[must_use]
pub fn minute_of(t: Time) -> u64 {
    t.0.checked_div(60_000).unwrap_or(0)
}

/// The partition of a node candidate: its needs and its `est` bucket (D.4.10).
#[must_use]
pub fn part_of(node: &Node) -> PartKey {
    PartKey {
        needs: node.needs.clone(),
        est_bucket: est_bucket(node.est),
    }
}

/// The partition of the routing items: no needs, bucket 0 (D.4.10).
#[must_use]
pub fn route_part() -> PartKey {
    PartKey::default()
}

/// Origin `Agent` or `Ancestor`: the children that the child limit counts (CS-10).
#[must_use]
pub fn is_agent_made(o: Origin) -> bool {
    matches!(o, Origin::Agent(_) | Origin::Ancestor(_))
}

/// A node's own contribution to the `sub` of each of its ancestors (CS-32): one count by its state,
/// and one in `flagged` when its flags are not empty.
#[must_use]
pub fn own_sub(node: &Node, flagged: bool) -> Subtree {
    let mut u = Subtree::default();
    match &node.life {
        Life::Open => u.open = 1,
        Life::Closed(c) => match c.outcome {
            Outcome::Done => u.done = 1,
            Outcome::Failed => u.failed = 1,
            Outcome::Dropped | Outcome::Superseded | Outcome::Missed | Outcome::Other(_) => {
                u.other = 1;
            }
        },
    }
    if flagged {
        u.flagged = 1;
    }
    u
}

/// `a + b` (`add`) or `a − b`, field by field, saturating.
#[must_use]
pub fn sub_apply(a: Subtree, b: Subtree, add: bool) -> Subtree {
    let f = |x: u64, y: u64| {
        if add {
            x.saturating_add(y)
        } else {
            x.saturating_sub(y)
        }
    };
    Subtree {
        open: f(a.open, b.open),
        done: f(a.done, b.done),
        failed: f(a.failed, b.failed),
        other: f(a.other, b.other),
        flagged: f(a.flagged, b.flagged),
    }
}

/// The candidate classification of D.8.5 for node `id` with flags `flags` and `live` live holds on
/// itself: the first row that matches. `Cand::Node` for an offered, flagged, or available node
/// with a free slot, or a closed node with a flag of `CLOSED_TAKE` and no open objection;
/// `Cand::Status`, an item with no hold, for a waiting node with `open_commitment` (a status
/// item), or, 1.9 (§10.4), a closed node with an open objection (a review item); `None`
/// otherwise.
#[must_use]
pub fn classify(
    s: &State,
    id: NodeId,
    node: &Node,
    flags: FlagSet,
    live: u32,
    now: Time,
) -> Option<Cand> {
    let inputs = class_inputs(s, id, node, live);
    classify_from(&inputs, waiting(s, node, now), flags).map(|k| match k {
        CandKind::Node => Cand::Node(id),
        CandKind::Status => Cand::Status(id),
    })
}

/// Whether node `id` belongs to `ix.next.waiting` (1.4, §10.4, D.4.10, D.8.5): open, waiting, not a
/// status item, and a `Cand::Node` by the row after the wait's, with its flags as they are.
#[must_use]
pub fn waits_for_work(
    s: &State,
    id: NodeId,
    node: &Node,
    flags: FlagSet,
    live: u32,
    now: Time,
) -> bool {
    waiting(s, node, now) && waits_from(&class_inputs(s, id, node, live), flags)
}

/// What the classification reads of node `id` with `live` live holds, besides its flags and wait.
fn class_inputs(s: &State, id: NodeId, node: &Node, live: u32) -> ClassInputs {
    ClassInputs {
        reserved: node.origin == Origin::Reserved || id.is_reserved(),
        open: node.is_open(),
        occ_waiting: matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting),
        occ_locked: s.occ_locked(id),
        merged: node.merged_into().is_some(),
        upheld: matches!(&node.life, Life::Closed(c) if c.upheld.is_some()),
        capacity: node.capacity,
        live,
        offered: node.tags.iter().any(|t| t.as_str() == crate::types::OFFER)
            && !node.is_container(),
        available: matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Available),
    }
}

/// The row "n open, waiting(n)" of D.8.5 over its inputs, for a node that waits: whether the next
/// row would make it a `Cand::Node` but for the wait (1.4, `ix.next.waiting`). A status item (a
/// waiting node with `open_commitment`) is a candidate, and is not counted.
#[must_use]
pub fn waits_from(c: &ClassInputs, flags: FlagSet) -> bool {
    !c.reserved
        && c.open
        && !c.occ_waiting
        && !flags.contains(crate::types::Flag::OpenCommitment)
        && c.live < c.capacity
        && (c.offered || !flags.is_empty() || c.available)
}

/// The kind of a node candidate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CandKind {
    /// Taken with a hold (`Cand::Node`).
    Node,
    /// An item with no hold (`Cand::Status`): a status item of a waiting node, or, 1.9 (§10.4),
    /// the review item of a closed node with an open objection.
    Status,
}

/// What the classification of D.8.5 reads of a node, besides its flags and its wait.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one input per row of D.8.5's table"
)]
pub struct ClassInputs {
    /// A reserved node.
    pub reserved: bool,
    /// Open.
    pub open: bool,
    /// An occurrence in phase `Waiting`.
    pub occ_waiting: bool,
    /// `occ_locked`: an occurrence of a series that is open and still a series.
    pub occ_locked: bool,
    /// Closed with `merged_into`.
    pub merged: bool,
    /// 1.8 (§5, §11.3): closed and marked upheld, never a candidate.
    pub upheld: bool,
    /// Slots.
    pub capacity: u32,
    /// Live holds on the node.
    pub live: u32,
    /// Tagged `offer` and not a container.
    pub offered: bool,
    /// An occurrence in phase `Available`.
    pub available: bool,
}

/// The table of D.8.5 over its inputs: the first row that matches.
#[must_use]
pub fn classify_from(c: &ClassInputs, waiting: bool, flags: FlagSet) -> Option<CandKind> {
    if c.reserved {
        return None;
    }
    if !c.open {
        if c.occ_locked || c.merged || c.upheld {
            return None;
        }
        // 1.9 (§10.4, §11.3): an open objection makes it a review, offered with no hold and never
        // reopened by `next`.
        if flags.contains(crate::types::Flag::Objection) {
            return Some(CandKind::Status);
        }
        return (c.capacity > 0 && !flags.intersection(crate::flags::CLOSED_TAKE).is_empty())
            .then_some(CandKind::Node);
    }
    if c.occ_waiting {
        return None;
    }
    if waiting {
        return flags
            .contains(crate::types::Flag::OpenCommitment)
            .then_some(CandKind::Status);
    }
    (c.live < c.capacity && (c.offered || !flags.is_empty() || c.available))
        .then_some(CandKind::Node)
}

/// What holds a node candidate back for every caller (D.8.5): an open `depends_on` target, the
/// nearest paused ancestor-or-self, and, 1.11, for a series, its open occurrence waiting for its
/// slot (`series_before_slot`). `None` when nothing does.
#[must_use]
pub fn held_of(
    s: &State,
    series_open: &BTreeMap<NodeId, BTreeSet<NodeId>>,
    id: NodeId,
    node: &Node,
) -> Option<Held> {
    let depends = node
        .depends_on
        .iter()
        .any(|d| s.nodes.get(d).is_some_and(Node::is_open));
    let paused_by = if s.pauses.contains_key(&id) {
        Some(id)
    } else {
        ancestors(s, id)
            .into_iter()
            .find(|a| s.pauses.contains_key(a))
    };
    let before_slot = series_before_slot(s, series_open, id, node);
    (depends || paused_by.is_some() || before_slot).then_some(Held {
        depends,
        paused_by,
        before_slot,
    })
}

/// 1.11 (§10.4, §12): whether `node`, an open series, has its open occurrence waiting for its slot:
/// its work is that occurrence, which cannot be taken yet, so the series is held back from `next`
/// for every caller until the occurrence becomes available. Read from `series_open`, the index's
/// (`refresh_node`) or the one a recount is building, which never reads `s.ix`.
#[must_use]
pub fn series_before_slot(
    s: &State,
    series_open: &BTreeMap<NodeId, BTreeSet<NodeId>>,
    id: NodeId,
    node: &Node,
) -> bool {
    node.is_open()
        && matches!(node.role, Role::Series(_))
        && series_open.get(&id).is_some_and(|os| {
            os.iter().any(|o| {
                s.nodes.get(o).is_some_and(
                    |x| matches!(&x.role, Role::Occurrence(r) if r.phase == OccPhase::Waiting),
                )
            })
        })
}

/// The tier of a candidate (D.8.5): 0 for a routing item, a flag of `T0`, or an available
/// occurrence whose window is still open; 1 for a positive priority or any flag but `stale` (1.8);
/// 2 otherwise. 1.11 (§10.4): a review item's tier is its node's without `objection`, the flag
/// that makes it a review rather than urgent work.
#[must_use]
pub fn tier_of(node: Option<&Node>, flags: FlagSet, cand: Cand) -> u8 {
    let Some(node) = node else { return 0 };
    let flags = if is_review(node, cand) {
        flags.without(crate::types::Flag::Objection)
    } else {
        flags
    };
    if matches!(cand, Cand::Route(_)) || !flags.intersection(T0).is_empty() {
        return 0;
    }
    if matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Available && !o.window_fired)
    {
        return 0;
    }
    if node.priority > 0 || !flags.without(crate::types::Flag::Stale).is_empty() {
        1
    } else {
        2
    }
}

/// How far above every other hour group the group of a stale candidate lies (1.8, §10.4): 2⁴⁷
/// hours (1.11; 2⁴² before), beyond every other band below, so that within its tier a stale offer
/// comes after the rest, and among stale ones the bands below order them. Hour groups lie within
/// ±2³³ (hours since 1970 and an `i32` priority).
pub const STALE_SHIFT: i64 = 1 << 47;

/// How far above the others the group of a candidate without a request lies (1.11, §10.4): 2⁴⁵
/// hours, beyond the recheck, needs, and review bands below and within the stale shift, so that
/// within its tier and stale band a candidate flagged `unanswered_request`, or a routing item,
/// comes first.
pub const NO_REQUEST_SHIFT: i64 = 1 << 45;

/// How far below every other hour group the group of a recheck-due candidate lies (1.7, §10.4):
/// 2⁴³ hours (1.11; 2⁴⁰ before), past the needs and review bands and any `⌊since / 1 h⌋ −
/// priority`, so that within its tier and request band such a node comes before the others, and
/// among themselves they keep the order of the bands below, their groups, and the ring.
pub const RECHECK_SHIFT: i64 = 1 << 43;

/// The needs a candidate's `needs` component counts, at most (1.11, §10.4).
pub const NEEDS_MAX: usize = 15;

/// How far above the next the group of a candidate lies for each label its node needs fewer than
/// `NEEDS_MAX` (1.11, §10.4): 2³⁸ hours, beyond the review band and any `⌊since / 1 h⌋ − priority`,
/// so that within its tier, request band, and recheck band, a candidate whose node needs more
/// labels comes first; `NEEDS_MAX` of them span less than 2⁴², within the recheck shift.
pub const NEEDS_SHIFT: i64 = 1 << 38;

/// The band of a candidate's hour group (1.11, §10.4): the sum of the shifts of `stale`,
/// `request`, `recheck`, and `needs` above or below the group of its age or review, so that the
/// order of groups within a tier is `(stale, request, recheck, needs, review, −aged)`.
#[must_use]
pub fn band(stale: bool, request: bool, recheck: bool, needs: usize) -> i64 {
    let fewer = i64::try_from(NEEDS_MAX.saturating_sub(needs.min(NEEDS_MAX))).unwrap_or(0);
    let mut b = NEEDS_SHIFT.saturating_mul(fewer);
    if recheck {
        b = b.saturating_sub(RECHECK_SHIFT);
    }
    if !request {
        b = b.saturating_add(NO_REQUEST_SHIFT);
    }
    if stale {
        b = b.saturating_add(STALE_SHIFT);
    }
    b
}

/// 1.11 (§10.4): whether a candidate counts as a request for `next`'s order: a routing item, or a
/// candidate flagged `unanswered_request`.
#[must_use]
pub fn is_request(flags: FlagSet, cand: Cand) -> bool {
    matches!(cand, Cand::Route(_)) || flags.contains(crate::types::Flag::UnansweredRequest)
}

/// Where the group of a review item lies (1.10, §10.4): 2³⁶ hours below 0, less its priority, the
/// same for every review of a priority, so that within its tier and the bands of `band` (1.11) the
/// reviews come before the others (within ±2³³), among themselves by
/// priority and then on the ring (`h`), not by age: a review needs no hold, so only the ring keeps
/// concurrent sessions at different reviews. A review's `since` still gives `why_this.aged`.
pub const REVIEW_SHIFT: i64 = 1 << 36;

/// Whether candidate `cand` of node `node` is a review item (1.9, §10.4): an item with no hold on
/// a closed node, the review of its open objection.
#[must_use]
pub fn is_review(node: &Node, cand: Cand) -> bool {
    matches!(cand, Cand::Status(_)) && !node.is_open()
}

/// Whether node `n` is *recheck due* (1.7, §10.4): its wait is `{external}` and has resolved at its
/// `recheck_after` (the wait's one-shot deadline has fired, T3); a release replaces the wait and a
/// close clears it.
#[must_use]
pub fn recheck_due(node: &Node) -> bool {
    node.wait
        .as_ref()
        .is_some_and(|w| matches!(w.on, WaitOn::External(_)) && w.fired)
}

/// The hour group of a candidate: `⌊since / 1 h⌋ − priority` (D.4.10, CS-12).
#[must_use]
pub fn group_of(since: Time, priority: i32) -> i64 {
    i64::try_from(since.0.checked_div(HOUR.0).unwrap_or(0))
        .unwrap_or(i64::MAX)
        .saturating_sub(i64::from(priority))
}

/// The `since` a candidate gets when it appears at `at` (§10.4 `candidate_since`, D.8.5): the
/// time it became a candidate, except that a node to take that is an occurrence has its slot,
/// each time it appears (after a hold on it ends, say), as §10.4 and D.8.5 say "for an occurrence,
/// its slot". A slot after `at` (a closed occurrence dropped before its slot, offered for a flag)
/// gives `at`, so that no `since` lies after `now` (R1).
#[must_use]
pub fn first_since(s: &State, cand: Cand, at: Time) -> Time {
    match cand {
        Cand::Node(n) => match s.nodes.get(&n).map(|x| &x.role) {
            Some(crate::state::Role::Occurrence(o)) => o.slot.min(at),
            _ => at,
        },
        Cand::Status(_) | Cand::Route(_) => at,
    }
}

/// Where a candidate sits in `ix.next` (D.4.10), with the node's current flags from `ix.node`.
/// `None` for a node candidate whose node does not exist.
#[must_use]
pub fn cand_at(s: &State, cand: Cand, since: Time) -> Option<CandAt> {
    let flags = match cand {
        Cand::Node(n) | Cand::Status(n) => s.ix.node.get(&n).map_or(FlagSet::EMPTY, |x| x.flags),
        Cand::Route(_) => FlagSet::EMPTY,
    };
    cand_at_with(s, flags, cand, since)
}

/// `cand_at` with the node's flags given, for the recount (D.4.10).
#[must_use]
pub fn cand_at_with(s: &State, flags: FlagSet, cand: Cand, since: Time) -> Option<CandAt> {
    match cand {
        Cand::Node(n) | Cand::Status(n) => {
            let node = s.nodes.get(&n)?;
            let recheck = matches!(cand, Cand::Node(_)) && recheck_due(node);
            let stale = flags.contains(crate::types::Flag::Stale);
            // 1.10 (§10.4): a review ranks by priority and the ring, not by age.
            let group = if is_review(node, cand) {
                REVIEW_SHIFT
                    .saturating_neg()
                    .saturating_sub(i64::from(node.priority))
            } else {
                group_of(since, node.priority)
            };
            // 1.11 (§10.4): the request, recheck, and needs bands.
            let band = band(stale, is_request(flags, cand), recheck, node.needs.len());
            Some(CandAt {
                since,
                part: part_of(node),
                key: NextKey {
                    tier: tier_of(Some(node), flags, cand),
                    group: group.saturating_add(band),
                    fnv: fnv_id(n.0),
                    id: n.0,
                    kind: if matches!(cand, Cand::Node(_)) { 0 } else { 1 },
                },
            })
        }
        Cand::Route(e) => Some(CandAt {
            since,
            part: route_part(),
            key: NextKey {
                tier: 0,
                group: group_of(since, 0).saturating_add(band(false, true, false, 0)),
                fnv: fnv_id(e.0),
                id: e.0,
                kind: 2,
            },
        }),
    }
}

/// The median of the held minutes of the last nodes closed `done` (§17, A-41): the middle value,
/// or the mean of the two middle values, rounded down, for an even count; `None` with none.
#[must_use]
pub fn median_done(done: &[u64]) -> Option<u64> {
    let mut v = done.to_vec();
    v.sort_unstable();
    let len = v.len();
    let mid = len.checked_div(2).unwrap_or(0);
    if len == 0 {
        None
    } else if len % 2 == 1 {
        v.get(mid).copied()
    } else {
        let a = v.get(mid.saturating_sub(1)).copied().unwrap_or(0);
        let b = v.get(mid).copied().unwrap_or(0);
        Some(mean_floor(a, b))
    }
}

/// `⌊(a + b) / 2⌋` without overflow: the common bits, plus half the differing ones.
#[must_use]
pub fn mean_floor(a: u64, b: u64) -> u64 {
    (a & b).wrapping_add((a ^ b).checked_shr(1).unwrap_or(0))
}

/// The estimate a node adds to the demand when it first becomes a candidate (§17, A-41): its
/// `est`, or the median of completed work, or `FALLBACK_EST_MIN`.
#[must_use]
pub fn demand_estimate(s: &State, node: &Node) -> u64 {
    node.est.map_or_else(
        || median_done(&s.stats.done_minutes).unwrap_or(FALLBACK_EST_MIN),
        u64::from,
    )
}

/// The `hello` rank of a pending user question (CS-33): the `about` node's priority, highest
/// first (0 without `about`), then the due, then the id.
#[must_use]
pub fn uq_rank(s: &State, q: Eid, about: Option<NodeId>, due: Time) -> UqRank {
    let priority = about
        .and_then(|n| s.nodes.get(&n))
        .map_or(0, |n| i64::from(n.priority));
    UqRank {
        neg_priority: priority.saturating_neg(),
        due,
        q,
    }
}

/// Whether a node is a person of the directory for `person_of` (D.2.2): an open child of
/// `@people`.
#[must_use]
pub fn is_person_node(node: &Node) -> bool {
    node.is_open() && node.parent() == Some(PEOPLE)
}

/// The interned principals of a person node's `person:` keys (D.2.2), empty for a node that is not
/// a person.
#[must_use]
pub fn person_principals(s: &State, node: &Node) -> BTreeSet<PrincipalId> {
    if !is_person_node(node) {
        return BTreeSet::new();
    }
    node.keys
        .keys()
        .filter_map(KeyStr::person)
        .filter_map(|k| s.principal_ids.get(&k).copied())
        .collect()
}

/// The entry types whose metadata `ix.people` indexes (D.4.10): request, said, heard, question,
/// and answer.
#[must_use]
pub fn people_entry(ty: EntryType) -> bool {
    matches!(
        ty,
        EntryType::Request
            | EntryType::Said
            | EntryType::Heard
            | EntryType::Question
            | EntryType::Answer
    )
}

/// The entry types whose metadata bears no entity and leaves memory `FINISHED_RETAIN` after its own
/// time (D.4.12): answer, heard, `to_user`, approve, resolve, and retract.
#[must_use]
pub fn bears_none(ty: EntryType) -> bool {
    matches!(
        ty,
        EntryType::Answer
            | EntryType::Heard
            | EntryType::ToUser
            | EntryType::Approve
            | EntryType::Resolve
            | EntryType::Retract
    )
}

/// The node an entity's `left` counts on (D.4.12): its node, `about` for a user question (else
/// `@user/decisions`), `@user` for a user entry, `@user/digest` for a digest entry. None for an entry's
/// metadata.
#[must_use]
pub fn home_of(s: &State, x: EntityRef) -> Option<NodeId> {
    match x {
        EntityRef::Request(r) => s.requests.get(&r).map(|r| r.node),
        EntityRef::PQuestion(q) => s.pquestions.get(&q).map(|q| q.node),
        EntityRef::UQuestion(q) => s
            .uquestions
            .get(&q)
            .map(|u| u.about.unwrap_or(crate::types::USER_DECISIONS)),
        EntityRef::Commitment(c) => s.commitments.get(&c).map(|c| c.node),
        EntityRef::Said(x) => s.saids.get(&x).map(|x| x.node),
        EntityRef::Decision(d) => s.decisions.get(&d).map(|d| d.node),
        EntityRef::Objection(o) => s.objections.get(&o).map(|o| o.node),
        EntityRef::UserEntry(_) => Some(crate::types::USER),
        EntityRef::Digest(_) => Some(crate::types::USER_DIGEST),
        EntityRef::Meta(_) => None,
    }
}

/// Whether an entity with this id is resident in one of the entity maps.
#[must_use]
pub fn bears_entity(s: &State, e: Eid) -> bool {
    s.requests.contains_key(&e)
        || s.pquestions.contains_key(&e)
        || s.aquestions.contains_key(&e)
        || s.uquestions.contains_key(&e)
        || s.commitments.contains_key(&e)
        || s.saids.contains_key(&e)
        || s.decisions.contains_key(&e)
        || s.objections.contains_key(&e)
        || s.user_entries.contains_key(&e)
        || s.digest.contains_key(&e)
        || s.notes.contains_key(&e)
}

/// The `said` entries of session `x` that are still `Pending`, ascending: what the Silence firing
/// turns `Unconfirmed` (D.8.2, §9.2), read from `ix.open_saids_by[x]`, which also holds the
/// `Unconfirmed` ones (1.3, D.4.10; rule 2 of the next calls reads only the `Pending` ones since
/// 1.5).
#[must_use]
pub fn pending_saids(s: &State, x: SessionId) -> Vec<Eid> {
    s.ix.open_saids_by
        .get(&x)
        .into_iter()
        .flatten()
        .copied()
        .filter(|a| {
            s.saids
                .get(a)
                .is_some_and(|sd| sd.state == SaidState::Pending)
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// The dirty-fact tracker of `evolve_tracked`

/// Where `evolve_tracked` collects the facts an event touched, and the last value of each entity
/// that left memory (D.4.12, review E1-spec-1); `evolve_mut` passes none.
#[derive(Debug)]
pub(crate) struct Dirt<'a>(pub(crate) Option<&'a mut crate::facts::Tracked>);

impl Dirt<'_> {
    /// Marks the fact `f()` dirty; builds it only when tracking.
    pub(crate) fn mark(&mut self, f: impl FnOnce() -> Fact) {
        if let Some(d) = self.0.as_mut() {
            d.facts.insert(f());
        }
    }

    /// Records that the entity of fact `f` left memory with the value `v()`, and marks `f` dirty;
    /// builds the value only when tracking.
    pub(crate) fn depart(&mut self, f: Fact, v: impl FnOnce() -> crate::facts::Departed) {
        if let Some(d) = self.0.as_mut() {
            d.facts.insert(f.clone());
            d.departed.insert(f, v());
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Small set and count helpers: an empty set or a zero count leaves its map

/// Adds or removes `v` in the set at `k`, dropping the key when its set is empty.
pub(crate) fn put_in<K: Ord, V: Ord>(m: &mut BTreeMap<K, BTreeSet<V>>, k: K, v: V, add: bool) {
    if add {
        m.entry(k).or_default().insert(v);
    } else if let Some(set) = m.get_mut(&k) {
        set.remove(&v);
        if set.is_empty() {
            m.remove(&k);
        }
    }
}

/// Adds or removes `v` in a set.
pub(crate) fn put<V: Ord>(set: &mut BTreeSet<V>, v: V, add: bool) {
    if add {
        set.insert(v);
    } else {
        set.remove(&v);
    }
}

/// The node's `items`, created on demand.
fn items(ix: &mut Ix, n: NodeId) -> &mut NodeItems {
    ix.items.entry(n).or_default()
}

/// Drops the node's `items` when every set in it is empty.
fn items_gc(ix: &mut Ix, n: NodeId) {
    if ix.items.get(&n).is_some_and(|x| *x == NodeItems::default()) {
        ix.items.remove(&n);
    }
}

// ------------------------------------------------------------------------------------------------
// Entities: one function per kind adds (`add`) or removes the entity's whole contribution, read
// from its current value in the state. A transition calls it with `false`, changes the entity,
// and calls it with `true`.

/// A request's contribution: `origins`, `unsettled_requests`, `requests_by`, `unsettled_by`, and
/// `items(node)` with its open subsets, `requesters_by_age`, and the owed sets.
pub(crate) fn ix_request(s: &mut State, r: RequestId, add: bool) {
    let Some(req) = s.requests.get(&r).cloned() else {
        return;
    };
    let ix = &mut s.ix;
    if add {
        ix.origins.insert(req.origin.key.clone(), r);
    } else if ix.origins.get(&req.origin.key) == Some(&r) {
        ix.origins.remove(&req.origin.key);
    }
    put(&mut items(ix, req.node).requests, r, add);
    let unsettled = req.settled.is_none();
    if unsettled {
        put(&mut ix.unsettled_requests, r, add);
        put(&mut items(ix, req.node).unsettled, r, add);
        if req.answered_by.is_none() {
            let it = items(ix, req.node);
            put_in(&mut it.unanswered, req.conv.clone(), r, add);
        }
    }
    if let Requester::User(p) = req.requester {
        put_in(&mut ix.requests_by, p, r, add);
        if unsettled {
            put_in(&mut ix.unsettled_by, p, r, add);
        }
    }
    if add {
        fix_owed(s, r);
    } else {
        let it = items(&mut s.ix, req.node);
        it.owed_reply.remove(&r);
        it.owed_decision.remove(&r);
    }
    if let Requester::User(p) = req.requester {
        fix_requester(s, req.node, p);
    }
    items_gc(&mut s.ix, req.node);
}

/// Recomputes `r`'s membership in its node's `owed_reply` (unsettled, unanswered, and listed by a
/// decision that is not retracted) and `owed_decision` (unsettled with an answer) (D.4.10, §20.9).
pub(crate) fn fix_owed(s: &mut State, r: RequestId) {
    let Some(req) = s.requests.get(&r) else {
        return;
    };
    let node = req.node;
    let unsettled = req.settled.is_none();
    let answered = req.answered_by.is_some();
    let listed = s.ix.settling.get(&r).is_some_and(|x| !x.is_empty());
    let it = items(&mut s.ix, node);
    put(&mut it.owed_reply, r, unsettled && !answered && listed);
    put(&mut it.owed_decision, r, unsettled && answered);
    items_gc(&mut s.ix, node);
}

/// Recomputes the entry of principal `p` in `items(node).requesters_by_age`: its oldest unsettled
/// user request on the node, if any (D.4.10).
pub(crate) fn fix_requester(s: &mut State, node: NodeId, p: PrincipalId) {
    let oldest = s.ix.unsettled_by.get(&p).and_then(|rs| {
        rs.iter()
            .copied()
            .find(|r| s.requests.get(r).is_some_and(|x| x.node == node))
    });
    let it = items(&mut s.ix, node);
    let stale: Vec<(RequestId, PrincipalId)> = it
        .requesters_by_age
        .iter()
        .filter(|(_, q)| *q == p)
        .copied()
        .collect();
    for x in stale {
        it.requesters_by_age.remove(&x);
    }
    if let Some(r) = oldest {
        it.requesters_by_age.insert((r, p));
    }
    items_gc(&mut s.ix, node);
}

/// A people question's contribution: `items(node).pquestions`, and `open_pq` while `Open`.
pub(crate) fn ix_pquestion(s: &mut State, q: Eid, add: bool) {
    let Some(pq) = s.pquestions.get(&q).cloned() else {
        return;
    };
    let it = items(&mut s.ix, pq.node);
    put(&mut it.pquestions, q, add);
    if pq.state == PqState::Open {
        put_in(&mut it.open_pq, pq.conv.clone(), q, add);
    }
    items_gc(&mut s.ix, pq.node);
}

/// An agent question's contribution: `items(node).aquestions`.
pub(crate) fn ix_aquestion(s: &mut State, q: Eid, add: bool) {
    let Some(aq) = s.aquestions.get(&q).copied() else {
        return;
    };
    put(&mut items(&mut s.ix, aq.node).aquestions, q, add);
    items_gc(&mut s.ix, aq.node);
}

/// A user question's contribution: `items(about).uquestions`, and, while `Pending`,
/// `pending_uq_by[to[stage]]` and `refusal_question` for a refusal question.
pub(crate) fn ix_uquestion(s: &mut State, q: Eid, add: bool) {
    let Some(uq) = s.uquestions.get(&q).cloned() else {
        return;
    };
    if let Some(n) = uq.about {
        put(&mut items(&mut s.ix, n).uquestions, q, add);
        items_gc(&mut s.ix, n);
    }
    if uq.state == UqState::Pending {
        let rank = uq_rank(s, q, uq.about, uq.due);
        if let Some(&p) = uq.to.get(usize::from(uq.stage)) {
            put_in(&mut s.ix.pending_uq_by, p, rank, add);
        }
        if let UqSource::Refusal { node } = uq.source {
            if add {
                s.ix.refusal_question.insert(node, q);
            } else if s.ix.refusal_question.get(&node) == Some(&q) {
                s.ix.refusal_question.remove(&node);
            }
        }
    }
}

/// A commitment's contribution: `items(node).commitments`, and `open_commitments` while `Open`.
pub(crate) fn ix_commitment(s: &mut State, c: Eid, add: bool) {
    let Some(x) = s.commitments.get(&c).cloned() else {
        return;
    };
    let it = items(&mut s.ix, x.node);
    put(&mut it.commitments, c, add);
    if x.state == CommitmentState::Open {
        put(&mut it.open_commitments, c, add);
    }
    items_gc(&mut s.ix, x.node);
}

/// A `said` entry's contribution: `items(node).saids`, `open_saids` and `unconfirmed`, and
/// `open_saids_by[session]` for a session still in `sessions` (D.4.10).
pub(crate) fn ix_said(s: &mut State, x: Eid, add: bool) {
    let Some(sd) = s.saids.get(&x).cloned() else {
        return;
    };
    let open = matches!(sd.state, SaidState::Pending | SaidState::Unconfirmed);
    let it = items(&mut s.ix, sd.node);
    put(&mut it.saids, x, add);
    if open {
        put(&mut it.open_saids, x, add);
    }
    if sd.state == SaidState::Unconfirmed {
        put(&mut it.unconfirmed, x, add);
    }
    items_gc(&mut s.ix, sd.node);
    if open && s.sessions.contains_key(&sd.session) {
        put_in(&mut s.ix.open_saids_by, sd.session, x, add);
    }
}

/// A decision's contribution: `items(node).decisions`, `settling` while not retracted (with the owed
/// sets of the requests it lists), and, while `Active`, `active_decisions`, `cited_by`, and
/// `stale_basis` (D.4.10, D.8.17).
pub(crate) fn ix_decision(s: &mut State, d: Eid, add: bool) {
    let Some(dec) = s.decisions.get(&d).cloned() else {
        return;
    };
    put(&mut items(&mut s.ix, dec.node).decisions, d, add);
    let retracted = matches!(dec.state, DecisionState::Retracted { .. });
    if !retracted {
        for &r in &dec.settles {
            put_in(&mut s.ix.settling, r, d, add);
            fix_owed(s, r);
        }
    }
    if dec.state == DecisionState::Active {
        put(&mut items(&mut s.ix, dec.node).active_decisions, d, add);
        for t in crate::flags::basis_targets(s, &dec) {
            put_in(&mut s.ix.cited_by, t, d, add);
        }
        let stale = add && stale_decision(s, &dec);
        let it = items(&mut s.ix, dec.node);
        put(&mut it.stale_basis, d, stale);
    }
    items_gc(&mut s.ix, dec.node);
}

/// An objection's contribution: `items(node).objections`, and `open_objections` while `Open`.
pub(crate) fn ix_objection(s: &mut State, o: Eid, add: bool) {
    let Some(x) = s.objections.get(&o).copied() else {
        return;
    };
    let it = items(&mut s.ix, x.node);
    put(&mut it.objections, o, add);
    if x.state == ObjectionState::Open {
        put(&mut it.open_objections, o, add);
    }
    items_gc(&mut s.ix, x.node);
}

/// A user entry's contribution: `unrouted` and `unrouted_by[session]` while unrouted, and
/// `clarifications` for a clarification.
pub(crate) fn ix_user_entry(s: &mut State, e: Eid, add: bool) {
    let Some(u) = s.user_entries.get(&e).copied() else {
        return;
    };
    match u.routing {
        Routing::Unrouted => {
            put(&mut s.ix.unrouted, e, add);
            if s.sessions.contains_key(&u.session) {
                put_in(&mut s.ix.unrouted_by, u.session, e, add);
            }
        }
        Routing::Clarifies { request } => put_in(&mut s.ix.clarifications, request, e, add),
        Routing::Request { .. } | Routing::Answers { .. } | Routing::NoAction => {}
    }
}

/// A digest entry's contribution: `pending_digest_by[to]` while pending.
pub(crate) fn ix_digest(s: &mut State, d: Eid, add: bool) {
    let Some(x) = s.digest.get(&d).copied() else {
        return;
    };
    if x.state == DigestState::Pending {
        put_in(&mut s.ix.pending_digest_by, x.to, d, add);
    }
}

/// A note's contribution: `notes_until`, the bound inboxes of its unread recipients in `sessions`,
/// `pending_holder`, `branch_notes`, and `user_notes` (D.4.10, D.8.16).
pub(crate) fn ix_note(s: &mut State, x: Eid, add: bool) {
    let Some(note) = s.notes.get(&x).cloned() else {
        return;
    };
    put(&mut s.ix.notes_until, (note.until, x), add);
    for r in &note.recipients {
        if s.sessions.contains_key(r) && !note.read_by.contains(r) {
            put_in(&mut s.ix.inbox, *r, x, add);
        }
    }
    match note.to {
        Addressee::Holder(n) if note.pending_holder => put_in(&mut s.ix.pending_holder, n, x, add),
        Addressee::Holders(a) => put_in(&mut s.ix.branch_notes, a, x, add),
        Addressee::Principal(p) => put_in(&mut s.ix.user_notes, p, (note.until, x), add),
        Addressee::Holder(_) | Addressee::Session(_) => {}
    }
}

/// An entry's metadata contribution: `ix.people` for a people entry (CS-17).
pub(crate) fn ix_meta(s: &mut State, e: Eid, add: bool) {
    let Some(m) = s.entries.get(&e).copied() else {
        return;
    };
    if people_entry(m.ty) {
        if add {
            s.ix.people.insert((m.node, m.seq), e);
        } else if s.ix.people.get(&(m.node, m.seq)) == Some(&e) {
            s.ix.people.remove(&(m.node, m.seq));
        }
    }
}

/// A hold's contribution: `holds_by_session[session]`.
pub(crate) fn ix_hold(s: &mut State, t: Token, add: bool) {
    let Some(h) = s.holds.get(&t) else {
        return;
    };
    let session = h.session;
    put_in(&mut s.ix.holds_by_session, session, t, add);
}

/// A session's contribution: `live_sessions` and `live_by_class` while not silent.
pub(crate) fn ix_session(s: &mut State, x: SessionId, add: bool) {
    let Some(session) = s.sessions.get(&x) else {
        return;
    };
    if session.silent {
        return;
    }
    let class = session.class.clone();
    put(&mut s.ix.live_sessions, x, add);
    let count = s.ix.live_by_class.entry(class.clone()).or_insert(0);
    *count = if add {
        count.saturating_add(1)
    } else {
        count.saturating_sub(1)
    };
    if *count == 0 {
        s.ix.live_by_class.remove(&class);
    }
}

/// The indices a node derives from its own fields: `keys_all`, `keys_open` while open, `backlinks`,
/// `dependents`, `waiters`, `marked_by_question`, `series_open` while it is an open occurrence, and
/// `agreements` while its agreements are not empty.
pub(crate) fn ix_node_own(s: &mut State, n: NodeId, add: bool) {
    let Some(node) = s.nodes.get(&n) else {
        return;
    };
    let open = node.is_open();
    let keys: Vec<KeyStr> = node.keys.keys().cloned().collect();
    let links: Vec<_> = node.links.iter().copied().collect();
    let depends: Vec<NodeId> = node.depends_on.iter().copied().collect();
    let wait = node.wait.as_ref().map(|w| w.on);
    let marks: BTreeSet<Eid> = node
        .recheck_marks
        .iter()
        .chain(node.assume_marks.iter())
        .copied()
        .collect();
    let series = match &node.role {
        Role::Occurrence(o) if open => Some(o.series),
        Role::Occurrence(_) | Role::Plain | Role::Series(_) | Role::Signal(_) => None,
    };
    let agreements = !node.agreements.is_empty();
    let ix = &mut s.ix;
    if agreements {
        if add {
            ix.agreements.insert(n);
        } else {
            ix.agreements.remove(&n);
        }
    }
    for k in keys {
        put_in(&mut ix.keys_all, k.clone(), n, add);
        if open {
            if add {
                ix.keys_open.insert(k, n);
            } else if ix.keys_open.get(&k) == Some(&n) {
                ix.keys_open.remove(&k);
            }
        }
    }
    for (rel, to) in links {
        put_in(&mut ix.backlinks, to, (rel, n), add);
    }
    for d in depends {
        put_in(&mut ix.dependents, d, n, add);
    }
    match wait {
        Some(WaitOn::Node(t)) => put_in(&mut ix.waiters, WaitKey::Node(t), n, add),
        Some(WaitOn::Question { q, .. } | WaitOn::UserQuestion(q)) => {
            put_in(&mut ix.waiters, WaitKey::Question(q), n, add);
        }
        Some(WaitOn::Time(_) | WaitOn::External(_)) | None => {}
    }
    for q in marks {
        put_in(&mut ix.marked_by_question, q, n, add);
    }
    if let Some(x) = series {
        put_in(&mut ix.series_open, x, n, add);
    }
}

/// Recomputes the directory's `person_of` and `aliases_of` for node `n` (D.2.2): its interned
/// `person:` principals while it is an open child of `@people`, none otherwise.
pub(crate) fn fix_person(s: &mut State, n: NodeId) {
    let new = s
        .nodes
        .get(&n)
        .map(|node| person_principals(s, node))
        .unwrap_or_default();
    let old = s.ix.aliases_of.get(&n).cloned().unwrap_or_default();
    for p in old.difference(&new) {
        if s.ix.person_of.get(p) == Some(&n) {
            s.ix.person_of.remove(p);
        }
    }
    for p in new.difference(&old) {
        s.ix.person_of.insert(*p, n);
    }
    if new.is_empty() {
        s.ix.aliases_of.remove(&n);
    } else {
        s.ix.aliases_of.insert(n, new);
    }
}

// ------------------------------------------------------------------------------------------------
// The tree's rollups (D.4.10): a node's summary counts on its parent and on every ancestor's `sub`

/// The ancestors of `n`, nearest first, at most `nodes.len()` of them (D.8.4).
#[must_use]
pub fn ancestors(s: &State, n: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = s.nodes.get(&n).and_then(Node::parent);
    while let Some(p) = cur {
        if out.len() >= s.nodes.len() {
            break;
        }
        out.push(p);
        cur = s.nodes.get(&p).and_then(Node::parent);
    }
    out
}

/// Adds (`add`) or removes the contribution of node `n` to its parent's counters and sets
/// (`children`, `open_children`, `open_agent_children`, `held_children`, `flagged_children`,
/// `ix.children`, `ix.flagged_children`, `ix.attended_children`) and to the `sub` of every
/// ancestor: its own state, and with `with_sub` also its own `sub` (a move carries the subtree).
pub(crate) fn node_contrib(s: &mut State, n: NodeId, add: bool, with_sub: bool) {
    let Some(node) = s.nodes.get(&n) else {
        return;
    };
    let Some(parent) = node.parent() else {
        return;
    };
    let nix = s.ix.node.get(&n).copied().unwrap_or_default();
    let flagged = !nix.flags.is_empty();
    let mut unit = own_sub(node, flagged);
    if with_sub {
        unit = sub_apply(unit, nix.sub, true);
    }
    let open = node.is_open();
    let agent = is_agent_made(node.origin);
    let held = nix.live > 0;
    let attended = nix.subtree_live > 0;
    let step = |x: u32, on: bool| -> u32 {
        match (on, add) {
            (false, _) => x,
            (true, true) => x.saturating_add(1),
            (true, false) => x.saturating_sub(1),
        }
    };
    let chain = ancestors(s, n);
    if let Some(p) = s.ix.node.get_mut(&parent) {
        p.children = step(p.children, true);
        p.open_children = step(p.open_children, open);
        p.open_agent_children = step(p.open_agent_children, open && agent);
        p.held_children = step(p.held_children, held);
        p.flagged_children = step(p.flagged_children, flagged);
    }
    put_in(&mut s.ix.children, parent, n, add);
    if flagged {
        put_in(&mut s.ix.flagged_children, parent, n, add);
    }
    if attended {
        put_in(&mut s.ix.attended_children, parent, n, add);
    }
    for a in chain {
        if let Some(x) = s.ix.node.get_mut(&a) {
            x.sub = sub_apply(x.sub, unit, add);
        }
    }
}

/// Adds or removes `delta` live holds on node `n` (D.4.10): its `live`, the `subtree_live` of it
/// and of every ancestor, the parent's `held_children` when `live` crosses 0, and each
/// `attended_children` membership whose `subtree_live` crosses 0.
pub(crate) fn add_live(s: &mut State, n: NodeId, add: bool) {
    let Some(nix) = s.ix.node.get(&n).copied() else {
        return;
    };
    let parent = s.nodes.get(&n).and_then(Node::parent);
    let live = if add {
        nix.live.saturating_add(1)
    } else {
        nix.live.saturating_sub(1)
    };
    if (nix.live > 0) != (live > 0)
        && let Some(p) = parent
        && let Some(px) = s.ix.node.get_mut(&p)
    {
        px.held_children = if live > 0 {
            px.held_children.saturating_add(1)
        } else {
            px.held_children.saturating_sub(1)
        };
    }
    if let Some(x) = s.ix.node.get_mut(&n) {
        x.live = live;
    }
    let mut chain = Vec::with_capacity(8);
    chain.push(n);
    chain.extend(ancestors(s, n));
    shift_subtree_live(s, &chain, 1, add);
}

/// Adds or removes `amount` from the `subtree_live` of every node of `chain` (a node and its
/// ancestors, nearest first), keeping `attended_children` as the counts cross 0.
pub(crate) fn shift_subtree_live(s: &mut State, chain: &[NodeId], amount: u32, add: bool) {
    if amount == 0 {
        return;
    }
    for &x in chain {
        let Some(xi) = s.ix.node.get(&x).copied() else {
            continue;
        };
        let new = if add {
            xi.subtree_live.saturating_add(amount)
        } else {
            xi.subtree_live.saturating_sub(amount)
        };
        if let Some(m) = s.ix.node.get_mut(&x) {
            m.subtree_live = new;
        }
        if (xi.subtree_live > 0) != (new > 0)
            && let Some(p) = s.nodes.get(&x).and_then(Node::parent)
        {
            put_in(&mut s.ix.attended_children, p, x, new > 0);
        }
    }
}

/// Sets the `depth` of `n` and of its whole subtree from its parent's (ROOT 0), walking
/// `ix.children` with a visited bound of `nodes.len()`.
pub(crate) fn fix_depths(s: &mut State, n: NodeId) {
    let base = s
        .nodes
        .get(&n)
        .and_then(Node::parent)
        .and_then(|p| s.ix.node.get(&p))
        .map_or(0, |p| p.depth.saturating_add(1));
    let mut stack = alloc::vec![(n, base)];
    let mut steps = 0_usize;
    while let Some((x, d)) = stack.pop() {
        steps = steps.saturating_add(1);
        if steps > s.nodes.len() {
            break;
        }
        if let Some(xi) = s.ix.node.get_mut(&x) {
            xi.depth = d;
        }
        if let Some(cs) = s.ix.children.get(&x) {
            for &c in cs {
                stack.push((c, d.saturating_add(1)));
            }
        }
    }
}

/// The nodes of the subtree of `n`, `n` included, through `ix.children`, at most `nodes.len()`.
#[must_use]
pub fn subtree(s: &State, n: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = alloc::vec![n];
    while let Some(x) = stack.pop() {
        if out.len() >= s.nodes.len() {
            break;
        }
        out.push(x);
        if let Some(cs) = s.ix.children.get(&x) {
            stack.extend(cs.iter().copied());
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------
// The candidate index (CS-16)

/// Puts a candidate into `ix.next`: `of`, and its partition or `held` (D.4.10).
pub(crate) fn next_put(next: &mut NextIx, cand: Cand, at: CandAt, held: Option<Held>) {
    if matches!(cand, Cand::Node(_) | Cand::Status(_)) {
        next.nodes = next.nodes.saturating_add(1);
    }
    match held {
        Some(h) => {
            next.held.insert(cand, h);
        }
        None => {
            let set = next.parts.entry(at.part.clone()).or_default();
            let was_empty = set.is_empty();
            set.insert(at.key);
            if was_empty {
                trie_add(&mut next.needs_trie, &at.part.needs, at.part.est_bucket);
            }
        }
    }
    next.of.insert(cand, at);
}

/// Takes a candidate out of `ix.next`, returning where it was.
pub(crate) fn next_take(next: &mut NextIx, cand: &Cand) -> Option<(CandAt, Option<Held>)> {
    let at = next.of.remove(cand)?;
    if matches!(cand, Cand::Node(_) | Cand::Status(_)) {
        next.nodes = next.nodes.saturating_sub(1);
    }
    if let Some(h) = next.held.remove(cand) {
        return Some((at, Some(h)));
    }
    if let Some(set) = next.parts.get_mut(&at.part) {
        set.remove(&at.key);
        if set.is_empty() {
            next.parts.remove(&at.part);
            trie_del(&mut next.needs_trie, &at.part.needs, at.part.est_bucket);
        }
    }
    Some((at, None))
}

/// Adds the partition `(needs, bucket)` to the needs trie (D.4.10).
pub fn trie_add(t: &mut NeedsTrie, needs: &BTreeSet<Label>, bucket: u8) {
    let mut cur = t;
    for l in needs {
        cur = cur.next.entry(l.clone()).or_default();
    }
    cur.buckets.insert(bucket);
}

/// Removes the partition `(needs, bucket)` from the needs trie, pruning the nodes left empty.
pub(crate) fn trie_del(t: &mut NeedsTrie, needs: &BTreeSet<Label>, bucket: u8) {
    fn go(t: &mut NeedsTrie, mut rest: alloc::collections::btree_set::Iter<'_, Label>, bucket: u8) {
        match rest.next() {
            None => {
                t.buckets.remove(&bucket);
            }
            Some(l) => {
                if let Some(child) = t.next.get_mut(l) {
                    go(child, rest, bucket);
                    if child.buckets.is_empty() && child.next.is_empty() {
                        t.next.remove(l);
                    }
                }
            }
        }
    }
    go(t, needs.iter(), bucket);
}

// ------------------------------------------------------------------------------------------------
// The node refresh of D.7.4: flags, candidates, and what is derived from them

/// The statistics of `day` (D.4.9), created on first use with `flagged_end` carried forward:
/// `|ix.flagged_active|`, which every change of it records in its day, so a day that starts with
/// another counter holds the count it began with, and a recorded 0 is a value (D.8.22: `f(d)` is
/// carried forward over days without a change; `FlagsPilingUp` reads it).
pub(crate) fn day_stats(s: &mut State, day: u64) -> &mut crate::state::DayStats {
    let carried = u32::try_from(s.ix.flagged_active.len()).unwrap_or(u32::MAX);
    s.stats
        .days
        .entry(day)
        .or_insert_with(|| crate::state::DayStats {
            flagged_end: carried,
            ..crate::state::DayStats::default()
        })
}

/// Recomputes node `n`'s flags at `at` and its candidate classification, and updates every index
/// derived from them: `flagged`, the parent's flagged sets and counts, `sub.flagged` of every
/// ancestor, `ix.next` with `State::since` and the demand of §17, `flagged_active` with the day's
/// `flagged_end`, and the `unheld_flagged` sets (D.7.4, D.8.7).
pub(crate) fn refresh_node(s: &mut State, n: NodeId, at: Time, dirt: &mut Dirt<'_>) {
    if !s.nodes.contains_key(&n) {
        return;
    }
    // Flags.
    let old = s.ix.node.get(&n).map_or(FlagSet::EMPTY, |x| x.flags);
    let new = flags_of(s, n, at);
    if old != new {
        let crossed = old.is_empty() != new.is_empty();
        if crossed {
            node_contrib(s, n, false, false);
        }
        if let Some(x) = s.ix.node.get_mut(&n) {
            x.flags = new;
        }
        put(&mut s.ix.flagged, n, !new.is_empty());
        if let Some(Role::Occurrence(o)) = s.nodes.get(&n).map(|x| &x.role) {
            let series = o.series;
            put_in(&mut s.ix.flagged_occurrences, series, n, !new.is_empty());
        }
        if crossed {
            node_contrib(s, n, true, false);
        }
    }
    // The candidates of the node, and the partitions it was suggested from.
    let mut old_parts: Vec<PartKey> = Vec::new();
    for c in [Cand::Node(n), Cand::Status(n)] {
        if let Some(a) = s.ix.next.of.get(&c)
            && !s.ix.next.held.contains_key(&c)
        {
            old_parts.push(a.part.clone());
        }
    }
    let Some(node) = s.nodes.get(&n) else {
        return;
    };
    let live = s.ix.node.get(&n).map_or(0, |x| x.live);
    let want = classify(s, n, node, new, live, at);
    let waits = waits_for_work(s, n, node, new, live, at);
    put(&mut s.ix.next.waiting, n, waits);
    let held = held_of(s, &s.ix.series_open, n, node);
    for c in [Cand::Node(n), Cand::Status(n)] {
        let had = s.ix.next.of.get(&c).cloned();
        if want == Some(c) {
            let first = first_since(s, c, at);
            let since = match (&had, s.since.get(&c)) {
                (Some(_), Some(&t)) => t,
                _ => first,
            };
            if had.is_none() {
                s.since.insert(c, first);
                dirt.mark(|| Fact::Since(c));
                if let Cand::Node(_) = c
                    && !s.stats.demand_counted.contains(&n)
                {
                    let est = s.nodes.get(&n).map_or(0, |x| demand_estimate(s, x));
                    let day = day_of(at);
                    let d = day_stats(s, day);
                    d.demand_min = d.demand_min.saturating_add(est);
                    s.stats.demand_counted.insert(n);
                    dirt.mark(|| Fact::Day(day));
                    dirt.mark(|| Fact::DemandCounted(n));
                }
            }
            let Some(new_at) = cand_at(s, c, since) else {
                continue;
            };
            let cur_held = s.ix.next.held.get(&c).copied();
            if had.as_ref() != Some(&new_at) || cur_held != held || had.is_none() {
                next_take(&mut s.ix.next, &c);
                next_put(&mut s.ix.next, c, new_at, held);
            }
        } else if had.is_some() {
            next_take(&mut s.ix.next, &c);
            if s.since.remove(&c).is_some() {
                dirt.mark(|| Fact::Since(c));
            }
        }
    }
    // flagged_active (CS-31).
    let is_cand =
        s.ix.next.of.contains_key(&Cand::Node(n)) || s.ix.next.of.contains_key(&Cand::Status(n));
    let active = !new.is_empty() && (is_cand || live > 0);
    if s.ix.flagged_active.contains(&n) != active {
        put(&mut s.ix.flagged_active, n, active);
        let day = day_of(at);
        let count = u32::try_from(s.ix.flagged_active.len()).unwrap_or(u32::MAX);
        day_stats(s, day).flagged_end = count;
        dirt.mark(|| Fact::Day(day));
    }
    // unheld_flagged (D.8.7).
    let parent = s.nodes.get(&n).and_then(Node::parent);
    if let Some(p) = parent {
        put_in(&mut s.ix.unheld_flagged_children, p, n, false);
    }
    for part in old_parts {
        put_in(&mut s.ix.unheld_flagged_by_part, part, n, false);
    }
    let mut unheld_part = None;
    for c in [Cand::Node(n), Cand::Status(n)] {
        if let Some(a) = s.ix.next.of.get(&c)
            && !s.ix.next.held.contains_key(&c)
        {
            unheld_part = Some(a.part.clone());
        }
    }
    if !new.is_empty()
        && live == 0
        && let Some(part) = unheld_part
    {
        if let Some(p) = parent {
            put_in(&mut s.ix.unheld_flagged_children, p, n, true);
        }
        put_in(&mut s.ix.unheld_flagged_by_part, part, n, true);
    }
}

/// Recomputes the routing item of user entry `e` (D.8.5): a `Cand::Route(e)` while the entry is
/// unrouted, with `since` the time it appeared.
pub(crate) fn refresh_route(s: &mut State, e: Eid, at: Time, dirt: &mut Dirt<'_>) {
    let c = Cand::Route(e);
    let want = s
        .user_entries
        .get(&e)
        .is_some_and(|u| u.routing == Routing::Unrouted);
    let had = s.ix.next.of.get(&c).cloned();
    if want {
        let since = match (&had, s.since.get(&c)) {
            (Some(_), Some(&t)) => t,
            _ => at,
        };
        if had.is_none() {
            s.since.insert(c, at);
            dirt.mark(|| Fact::Since(c));
        }
        if let Some(new_at) = cand_at(s, c, since)
            && had.as_ref() != Some(&new_at)
        {
            next_take(&mut s.ix.next, &c);
            next_put(&mut s.ix.next, c, new_at, None);
        }
    } else if had.is_some() {
        next_take(&mut s.ix.next, &c);
        if s.since.remove(&c).is_some() {
            dirt.mark(|| Fact::Since(c));
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::cast_possible_wrap,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;

    #[test]
    fn bit_lengths() {
        assert_eq!(bit_len(0), 0);
        assert_eq!(bit_len(1), 1);
        assert_eq!(bit_len(64), 7);
        assert_eq!(bit_len(65), 7);
        assert_eq!(bit_len(u32::MAX), 32);
        assert_eq!(est_bucket(None), 0);
        assert_eq!(horizon_bits(None), 33);
        assert_eq!(horizon_bits(Some(64)), 7);
    }

    #[test]
    fn medians() {
        assert_eq!(median_done(&[]), None);
        assert_eq!(median_done(&[7]), Some(7));
        assert_eq!(median_done(&[9, 1, 5]), Some(5));
        assert_eq!(median_done(&[1, 4, 2, 9]), Some(3));
        assert_eq!(median_done(&[u64::MAX, u64::MAX]), Some(u64::MAX));
        assert_eq!(median_done(&[u64::MAX - 1, u64::MAX]), Some(u64::MAX - 1));
        assert_eq!(mean_floor(3, 4), 3);
    }

    #[test]
    fn hour_groups_order_like_minus_aged() {
        // A candidate that arrived at 10:59 is one group older than one at 11:00 (D.8.5).
        let a = group_of(Time(10 * HOUR.0 + 59 * 60_000), 0);
        let b = group_of(Time(11 * HOUR.0), 0);
        assert!(a < b);
        assert_eq!(group_of(Time(11 * HOUR.0), 2), 9);
    }

    #[test]
    fn the_day_minute_and_hour_group_at_their_edges() {
        for (t, day, minute) in [
            (0_u64, 0_u64, 0_u64),
            (59_999, 0, 0),
            (60_000, 0, 1),
            (DAY.0 - 1, 0, 1_439),
            (DAY.0, 1, 1_440),
            (u64::MAX, u64::MAX / DAY.0, u64::MAX / 60_000),
        ] {
            assert_eq!(day_of(Time(t)), day);
            assert_eq!(minute_of(Time(t)), minute);
        }
        assert_eq!(
            group_of(Time(u64::MAX), i32::MIN),
            (u64::MAX / HOUR.0) as i64 + 2_147_483_648
        );
        assert_eq!(group_of(Time(0), i32::MAX), -2_147_483_647);
    }

    #[test]
    fn the_trie_prunes_what_it_empties() {
        let mut t = NeedsTrie::default();
        let x: BTreeSet<Label> = [Label::new("x").unwrap(), Label::new("y").unwrap()].into();
        trie_add(&mut t, &x, 3);
        trie_add(&mut t, &BTreeSet::new(), 0);
        trie_del(&mut t, &x, 3);
        let mut want = NeedsTrie::default();
        want.buckets.insert(0);
        assert_eq!(t, want);
        trie_del(&mut t, &BTreeSet::new(), 0);
        assert_eq!(t, NeedsTrie::default());
    }

    #[test]
    fn sub_arithmetic_saturates() {
        let one = Subtree {
            open: 1,
            ..Subtree::default()
        };
        assert_eq!(
            sub_apply(Subtree::default(), one, false),
            Subtree::default()
        );
        assert_eq!(sub_apply(one, one, true).open, 2);
    }
}
