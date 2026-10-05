//! `leave_better` (design D.8.7, §10.3, A-63, A-100): what to tidy next, read from the sets of
//! suggestible nodes, `ix.unheld_flagged_children` and `ix.unheld_flagged_by_part`, so it never
//! scans flagged nodes it would skip.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::next::{Caller, eligible};
use super::{Meter, NoMeter, SCAN_MAX};
use crate::fnv::fnv_id;
use crate::state::{Cand, Node, Origin, Role, SignalKey, State, UqState};
use crate::types::{Flag, NodeId, QuestionId, SessionId, SignalKind, Time};
use crate::words::{Doc, alike, doc_freq, doc_words};

/// What to tidy next (§10.3), at most `leave_better.max` nodes.
///
/// Anchored at a released or closed node `n`: `n`'s siblings, then its parent, then the parent's
/// siblings, and so on up to ROOT; within a level the ancestor first, then its siblings by id;
/// `n` itself never; 1.11 (§10.3, §11.3): nor a review (a closed node with an open objection)
/// that the caller's class is kept off, refused or backed off, as a deferral of it does. Without
/// an anchor, for an empty `next`: a merge by id of the suggestible nodes in the caller's eligible
/// partitions (D.8.5 step 1, an entry of the horizon's boundary bucket only when its `est` fits),
/// skipping the nodes the caller's class excludes (D.8.14). 1.12 (§10.4): either way, never the
/// review of an objection the caller raised (`own_review`), nor the node it released as
/// `handoff` while that keeps it from the caller (`handed_back`).
#[must_use]
pub fn leave_better(
    s: &State,
    anchor: Option<NodeId>,
    caller: Option<crate::types::SessionId>,
) -> Vec<NodeId> {
    let max = usize::try_from(s.params.leave_better_max).unwrap_or(usize::MAX);
    let mut out: Vec<NodeId> = Vec::new();
    match anchor {
        Some(n) => {
            // 1.11: the caller's class, whose deferred reviews the walk leaves out.
            let class = caller.and_then(|x| s.sessions.get(&x)).map(|x| &x.class);
            let now = s.last_now;
            let kept_off = |m: NodeId| {
                class.is_some_and(|c| deferred_review(s, m, c, now))
                    || caller.is_some_and(|x| put_down(s, x, m, now))
            };
            let mut cur = n;
            let mut steps = 0_usize;
            while let Some(p) = s.nodes.get(&cur).and_then(Node::parent) {
                if out.len() >= max || steps > s.nodes.len() {
                    break;
                }
                steps = steps.saturating_add(1);
                let level = s.ix.unheld_flagged_children.get(&p);
                if cur != n && level.is_some_and(|x| x.contains(&cur)) && !kept_off(cur) {
                    out.push(cur);
                }
                for &m in level.into_iter().flatten() {
                    if out.len() >= max {
                        break;
                    }
                    if m != cur && m != n && !kept_off(m) {
                        out.push(m);
                    }
                }
                cur = p;
            }
        }
        None => {
            let Some(c) = caller.and_then(|x| Caller::of(s, x)) else {
                return out;
            };
            let el = eligible(s, c.caps, c.horizon, &mut NoMeter);
            let now = s.last_now;
            let sets: Vec<(&BTreeSet<NodeId>, bool)> = el
                .parts
                .iter()
                .filter_map(|(p, b)| s.ix.unheld_flagged_by_part.get(p).map(|x| (x, *b)))
                .collect();
            let mut iters: Vec<_> = sets
                .iter()
                .map(|(x, b)| (x.iter().peekable(), *b))
                .collect();
            while out.len() < max {
                let mut best: Option<(NodeId, usize)> = None;
                for (i, (it, _)) in iters.iter_mut().enumerate() {
                    if let Some(&&m) = it.peek()
                        && best.is_none_or(|(b, _)| m < b)
                    {
                        best = Some((m, i));
                    }
                }
                let Some((m, i)) = best else { break };
                let boundary = iters.get_mut(i).is_some_and(|(it, b)| {
                    it.next();
                    *b
                });
                if crate::refusals::excluded(s, m, c.class, now) || put_down(s, c.sess, m, now) {
                    continue;
                }
                if boundary
                    && let (Some(e), Some(h)) = (s.nodes.get(&m).and_then(|x| x.est), c.horizon)
                    && e > h
                {
                    continue;
                }
                out.push(m);
            }
        }
    }
    out.truncate(max);
    out
}

/// 1.12 (§10.4): what the caller put down, which `next` keeps from it: the review of an objection
/// it raised, and the node it released as `handoff`, until another hold of it or `lease.first`.
fn put_down(s: &State, x: SessionId, m: NodeId, now: Time) -> bool {
    crate::state::own_review(s, x, m) || crate::state::handed_back(s, x, m, now)
}

/// 1.11 (§10.3, §11.3): `m` is a review, a closed node with an open objection, that class `c` is
/// kept off at `now`: it has refused it, or is backed off from it (D.8.14), as a deferral of its
/// review backs it off. `next` and `hello`'s offers exclude it for `c` as they exclude any node so
/// refused; the anchored walk of `leave_better`, which lists flagged nodes whatever the caller's
/// refusals, leaves out this one case. In the lab's sixth round (l5), a release's next call gave a
/// worker back the review it had found it could not decide.
fn deferred_review(s: &State, m: NodeId, c: &crate::types::ClassName, now: Time) -> bool {
    s.nodes.get(&m).is_some_and(|x| !x.is_open())
        && s.ix
            .items
            .get(&m)
            .is_some_and(|it| !it.open_objections.is_empty())
        && crate::refusals::excluded(s, m, c, now)
}

// ------------------------------------------------------------------------------------------------
// The forum's tidy items (1.9)

/// Tidy items an idle worker gets, at most (§20.9 rule 13, §22's fixed values).
pub const TIDY_MAX: usize = 3;

/// Visits the forum's tidy scan makes at most (§18): flagged nodes, candidates, children, and
/// pairs compared, each one visit.
pub const TIDY_SCAN: u32 = SCAN_MAX;

/// What a stale offer's tidy item asks (1.10, §16), by the questions for the user about it that
/// the core holds: none, so ask its owners; or one answered or defaulted, the newest, so take it
/// with that news. With one pending or lapsed, there is no item: its owners are being asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaleStep {
    /// `talk ask {at, to: user, options: [keep, drop], default: drop}`.
    Ask,
    /// `hold take`, once this question has its answer or its default.
    Answered(QuestionId),
}

/// The step a stale offer's tidy item asks for (1.10, §16), or none while a question for the user
/// about it is pending or has lapsed (its `proceed_on_assumption` then asks for a decision on the
/// node): read from the node's user questions, every one the core still holds (a finished one for
/// `FINISHED_RETAIN`, §24).
#[must_use]
pub fn stale_step(s: &State, n: NodeId) -> Option<StaleStep> {
    let mut answered = None;
    for q in
        s.ix.items
            .get(&n)
            .into_iter()
            .flat_map(|it| it.uquestions.iter())
    {
        match s.uquestions.get(q).map(|u| u.state) {
            Some(UqState::Pending | UqState::Lapsed) => return None,
            Some(UqState::Answered { .. } | UqState::Defaulted) => answered = Some(*q),
            None => {}
        }
    }
    Some(answered.map_or(StaleStep::Ask, StaleStep::Answered))
}

/// The session that created `n` (§5 `created_by`): by `node create`, or as a missing ancestor;
/// none for a node the server made.
fn creator(s: &State, n: NodeId) -> Option<SessionId> {
    match s.nodes.get(&n)?.origin {
        Origin::Agent(x) | Origin::Ancestor(x) => Some(x),
        Origin::Reserved | Origin::Occurrence | Origin::Signal => None,
    }
}

/// Whether one session created both `a` and `b` (1.10, §16): two nodes of one author are its plan,
/// not a duplicate. In the lab, a lead split an epic into tasks that touched one file, and two idle
/// workers in turn were sent to compare two of them.
#[must_use]
pub fn same_creator(s: &State, a: NodeId, b: NodeId) -> bool {
    creator(s, a).is_some_and(|x| creator(s, b) == Some(x))
}

/// One tidy item of the whole forum (1.9, §10.3, §16).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TidyItem {
    /// A stale offer (§14) the session can take, and what its item asks (1.10).
    Stale(NodeId, StaleStep),
    /// Possible duplicates: two open nodes under one parent that share rare words (§16). `from` is
    /// the node §16's merge would merge away, `to` the one that would remain.
    Alike {
        /// The newer node, unless only it is held.
        from: NodeId,
        /// The other.
        to: NodeId,
        /// The rare words they share (`words::alike`), at least `words::ALIKE_MIN`.
        shared: u32,
    },
}

/// The forum's tidy items and whether the scan was cut.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tidy {
    /// At most `TIDY_MAX`, a stale offer and a possible duplicate in turn, a stale offer first.
    pub items: Vec<TidyItem>,
    /// The scan stopped short, or found more than it gives: these are not all (§18).
    pub cut: bool,
}

/// Whether `n` may be one of a pair of possible duplicates: an open, plain node (not a container,
/// an occurrence, or a signal node).
fn alike_ok(s: &State, n: NodeId) -> bool {
    !n.is_reserved()
        && s.nodes
            .get(&n)
            .is_some_and(|x| x.is_open() && matches!(x.role, Role::Plain) && !x.is_container())
}

/// Whether `a` and `b` are linked, either way, by any relation, or carry an open duplicate signal:
/// someone has already looked at the pair.
fn related(s: &State, a: NodeId, b: NodeId) -> bool {
    let links = |x: NodeId, y: NodeId| {
        s.nodes
            .get(&x)
            .is_some_and(|n| n.links.iter().any(|(_, t)| *t == y))
    };
    links(a, b)
        || links(b, a)
        || s.signals.contains_key(&SignalKey {
            kind: SignalKind::Duplicate,
            a: a.min(b),
            b: a.max(b),
        })
}

/// A visit of the tidy scan, against its bound.
struct Budget<'m, M: Meter> {
    left: u32,
    m: &'m mut M,
}

impl<M: Meter> Budget<'_, M> {
    /// Counts one visit; false once the bound is spent.
    fn visit(&mut self) -> bool {
        if self.left == 0 {
            return false;
        }
        self.left = self.left.saturating_sub(1);
        self.m.tick(1);
        true
    }
}

/// The order of possible duplicates: the most rare words shared first, then FNV-1a-64 of the node
/// a merge would merge away (`0x00 ‖ id`), then its id, then the other's.
#[must_use]
pub fn pair_order(a: &(u32, NodeId, NodeId), b: &(u32, NodeId, NodeId)) -> core::cmp::Ordering {
    b.0.cmp(&a.0)
        .then(fnv_id(a.1.0).cmp(&fnv_id(b.1.0)))
        .then(a.1.cmp(&b.1))
        .then(a.2.cmp(&b.2))
}

/// The forum's tidy items for session `x` at `now` (1.9, §10.3, §16, §18, §20.9 rule 13), which an
/// idle worker that `next` offers nothing gets where `end` would come:
/// - stale offers (§14), by id: open nodes flagged `stale` that are `next` candidates nothing holds
///   back (no pause, no open `depends_on` target), and that `x` can take; whatever its caps,
///   horizon, and refusals, since asking about or closing an offer needs none of them; 1.10: each
///   with the step `stale_step` gives, and none while its owners are being asked;
/// - possible duplicates (§16): for each candidate node of `next` that is an open, plain node,
///   newest first, its parent's open, plain children, with their words (`words::doc_words`: the
///   stems of the title and of the body's start); each pair of one parent that shares at least
///   `ALIKE_MIN` rare words, a word being rare when at most `RARE_DF` of all the nodes read carry it,
///   with one of them in each title (`words::alike`), unless the two are linked or carry an open
///   duplicate signal, or (1.10) one session created both; the most rare words shared first, ties
///   as `pair_order` breaks them.
///
/// At most `TIDY_MAX` in all, a stale offer and a possible duplicate in turn. The scan makes at
/// most `TIDY_SCAN` visits; `cut` says that it stopped short, or found more than it gives, so the
/// next calls' `total` is `≥k` (§18).
#[must_use]
pub fn tidy<M: Meter>(s: &State, x: SessionId, now: Time, m: &mut M) -> Tidy {
    tidy_within(s, x, now, TIDY_SCAN, m)
}

/// `tidy` with a bound of `bound` visits (the tests' smaller ones).
#[must_use]
pub(crate) fn tidy_within<M: Meter>(
    s: &State,
    x: SessionId,
    now: Time,
    bound: u32,
    m: &mut M,
) -> Tidy {
    let mut b = Budget { left: bound, m };
    let mut cut = false;
    // Stale offers, by id.
    let mut stale: Vec<(NodeId, StaleStep)> = Vec::new();
    for &n in &s.ix.flagged {
        if !b.visit() {
            cut = true;
            break;
        }
        let stale_flag =
            s.ix.node
                .get(&n)
                .is_some_and(|i| i.flags.contains(Flag::Stale));
        let c = Cand::Node(n);
        if !stale_flag
            || !s.ix.next.of.contains_key(&c)
            || s.ix.next.held.contains_key(&c)
            || !crate::helpers::holds::can_take(s, x, n, now)
        {
            continue;
        }
        // 1.10: none while its owners are being asked.
        let Some(step) = stale_step(s, n) else {
            continue;
        };
        if stale.len() >= TIDY_MAX {
            cut = true;
            break;
        }
        stale.push((n, step));
    }
    // Possible duplicates: the parents of `next`'s candidates, newest first, and their open, plain
    // children with their words.
    let mut groups: Vec<Vec<(NodeId, Doc)>> = Vec::new();
    let mut parents: BTreeSet<NodeId> = BTreeSet::new();
    let range =
        s.ix.next
            .of
            .range(Cand::Node(NodeId(0))..=Cand::Node(NodeId(u64::MAX)));
    'scan: for (c, _) in range.rev() {
        if !b.visit() {
            cut = true;
            break;
        }
        let Cand::Node(n) = *c else { continue };
        if !alike_ok(s, n) {
            continue;
        }
        let Some(p) = s.nodes.get(&n).and_then(Node::parent) else {
            continue;
        };
        if !parents.insert(p) {
            continue;
        }
        let mut members: Vec<(NodeId, Doc)> = Vec::new();
        for &k in s.ix.children.get(&p).into_iter().flatten() {
            if !b.visit() {
                cut = true;
                break 'scan;
            }
            if alike_ok(s, k)
                && let Some(node) = s.nodes.get(&k)
            {
                let w = doc_words(&node.title.0, &node.body.0);
                if !w.is_empty() {
                    members.push((k, w));
                }
            }
        }
        groups.push(members);
    }
    // A word's rarity is counted over every node read.
    let df = doc_freq(groups.iter().flatten().map(|(_, w)| w));
    let mut pairs: Vec<(u32, NodeId, NodeId)> = Vec::new();
    'pairs: for members in &groups {
        for (i, (a, wa)) in members.iter().enumerate() {
            for (bn, wb) in members.iter().skip(i.saturating_add(1)) {
                if !b.visit() {
                    cut = true;
                    break 'pairs;
                }
                let shared = alike(wa, wb, &df);
                if shared > 0 && !related(s, *a, *bn) && !same_creator(s, *a, *bn) {
                    let (from, to) = super::guide::merge_pair(s, (*a).min(*bn), (*a).max(*bn));
                    pairs.push((shared, from, to));
                }
            }
        }
    }
    pairs.sort_by(pair_order);
    let (items, more) = interleave(&stale, &pairs);
    Tidy {
        items,
        cut: cut || more,
    }
}

/// A stale offer and a possible duplicate in turn, a stale offer first, at most `TIDY_MAX`; and
/// whether any was left out.
#[must_use]
pub fn interleave(
    stale: &[(NodeId, StaleStep)],
    pairs: &[(u32, NodeId, NodeId)],
) -> (Vec<TidyItem>, bool) {
    let mut out = Vec::new();
    let mut st = stale.iter();
    let mut pr = pairs.iter();
    loop {
        let mut any = false;
        if let Some(&(n, step)) = st.next() {
            out.push(TidyItem::Stale(n, step));
            any = true;
        }
        if let Some(&(shared, from, to)) = pr.next() {
            out.push(TidyItem::Alike { from, to, shared });
            any = true;
        }
        if !any {
            break;
        }
    }
    let more = out.len() > TIDY_MAX;
    out.truncate(TIDY_MAX);
    (out, more)
}
