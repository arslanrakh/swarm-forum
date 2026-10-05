//! `next`'s ranking from the candidate index (design D.8.5, CS-16).
//!
//! A session's candidates are read from `ix.next` without ranking the backlog: the eligible
//! partitions are found in the needs trie by descending only through the caller's caps; within
//! each partition the entries of one `(tier, group)` are read in ring order from the session's
//! point; a merge over the partitions' heads yields the candidates in `key` order, each skipped if
//! a caller exclusion applies (its class's refusals, its own holds, the query, or an `est` over
//! the horizon in the boundary bucket). 1.10 (§10.4): a review item's hour group puts the reviews of
//! a tier in a band of their own, ordered by priority and the ring (`index::REVIEW_SHIFT`); and for a
//! caller with a horizon, a partition without `est` is read as two streams, its nodes to take, which
//! the merge puts after the estimated candidates of their tier, and its other items. `spec::ranked`
//! sorts every candidate instead and is the oracle the tests compare with (D.12.3). 1.12 (§10.4):
//! the caller's own exclusions take in what it put down (the review of its own objection, the node
//! it handed off); after every fitting candidate, a second merge reads the partitions over the
//! caller's horizon for work longer than every session's (`longer`); and the first candidate
//! carries `beyond_horizon`, the work its horizon excludes.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::ops::Bound::{Excluded, Included, Unbounded};

use super::{Meter, Ranked, SCAN_MAX};
use crate::cmd::NextQuery;
use crate::fnv::fnv_session;
use crate::index::bit_len;
use crate::reply::{EmptyWhy, Reason, WhyThis};
use crate::state::{Cand, CandAt, NeedsTrie, NextKey, OccPhase, PartKey, Role, State};
use crate::types::{ClassName, Eid, FlagSet, HOUR, Label, NodeId, SessionId, Time};

/// Who asks `next`: what its exclusions read (D.8.5).
#[derive(Clone, Debug)]
pub(crate) struct Caller<'a> {
    /// The session.
    pub(crate) sess: SessionId,
    /// Its class.
    pub(crate) class: &'a ClassName,
    /// Its caps.
    pub(crate) caps: &'a BTreeSet<Label>,
    /// Its horizon, in minutes.
    pub(crate) horizon: Option<u32>,
}

impl<'a> Caller<'a> {
    /// The caller that session `sess` is, if it exists.
    pub(crate) fn of(s: &'a State, sess: SessionId) -> Option<Caller<'a>> {
        let x = s.sessions.get(&sess)?;
        Some(Caller {
            sess,
            class: &x.class,
            caps: &x.caps,
            horizon: x.horizon,
        })
    }
}

/// The partitions a caller may take from (D.8.5 step 1), each with whether it is the boundary
/// bucket of the caller's horizon (its entries' `est` checked one by one), and the counts of what
/// the caller's needs and horizon exclude.
#[derive(Clone, Debug, Default)]
pub(crate) struct Eligible {
    /// The eligible partitions, and whether each is the boundary bucket.
    pub(crate) parts: Vec<(PartKey, bool)>,
    /// Entries in partitions whose needs are within the caller's caps.
    pub(crate) needs_ok: u64,
    /// Entries in those partitions whose bucket lies above the horizon's.
    pub(crate) over_horizon: u64,
    /// 1.12 (§10.4): those partitions.
    pub(crate) over: Vec<PartKey>,
}

/// The eligible partitions of a caller with `caps` and `horizon`, found in the needs trie by
/// descending only through the labels in `caps`.
pub(crate) fn eligible<M: Meter>(
    s: &State,
    caps: &BTreeSet<Label>,
    horizon: Option<u32>,
    m: &mut M,
) -> Eligible {
    let mut out = Eligible::default();
    let hb = horizon.map(bit_len);
    let mut stack: Vec<(&NeedsTrie, Vec<Label>)> = alloc::vec![(&s.ix.next.needs_trie, Vec::new())];
    while let Some((t, path)) = stack.pop() {
        m.tick(1);
        if !t.buckets.is_empty() {
            let needs: BTreeSet<Label> = path.iter().cloned().collect();
            for &b in &t.buckets {
                let part = PartKey {
                    needs: needs.clone(),
                    est_bucket: b,
                };
                let size =
                    s.ix.next
                        .parts
                        .get(&part)
                        .map_or(0, |x| u64::try_from(x.len()).unwrap_or(u64::MAX));
                out.needs_ok = out.needs_ok.saturating_add(size);
                match hb {
                    None => out.parts.push((part, false)),
                    Some(_) if b == 0 => out.parts.push((part, false)),
                    Some(h) if b < h => out.parts.push((part, false)),
                    Some(h) if b == h => out.parts.push((part, true)),
                    Some(_) => {
                        out.over_horizon = out.over_horizon.saturating_add(size);
                        out.over.push(part);
                    }
                }
            }
        }
        for (l, child) in &t.next {
            if caps.contains(l) {
                let mut p = path.clone();
                p.push(l.clone());
                stack.push((child, p));
            }
        }
    }
    out
}

/// The first key of block `(tier, group)` in ring order from `point`: the smallest fnv at or
/// above the point, else the smallest below it.
fn block_first(set: &BTreeSet<NextKey>, tier: u8, group: i64, point: u64) -> Option<NextKey> {
    let pt = NextKey {
        tier,
        group,
        fnv: point,
        id: 0,
        kind: 0,
    };
    let hi = NextKey {
        tier,
        group,
        fnv: u64::MAX,
        id: u64::MAX,
        kind: u8::MAX,
    };
    if let Some(k) = set.range((Included(pt), Included(hi))).next() {
        return Some(*k);
    }
    let lo = NextKey {
        tier,
        group,
        fnv: 0,
        id: 0,
        kind: 0,
    };
    set.range((Included(lo), Excluded(pt))).next().copied()
}

/// The key after `prev` in ring order from `point`: within its block, the rest of the part at or
/// above the point, then the part below it; then the next block.
fn ring_after(set: &BTreeSet<NextKey>, prev: NextKey, point: u64) -> Option<NextKey> {
    let (tier, group) = (prev.tier, prev.group);
    let hi = NextKey {
        tier,
        group,
        fnv: u64::MAX,
        id: u64::MAX,
        kind: u8::MAX,
    };
    let pt = NextKey {
        tier,
        group,
        fnv: point,
        id: 0,
        kind: 0,
    };
    if prev.fnv >= point {
        if let Some(k) = set.range((Excluded(prev), Included(hi))).next() {
            return Some(*k);
        }
        let lo = NextKey {
            tier,
            group,
            fnv: 0,
            id: 0,
            kind: 0,
        };
        if let Some(k) = set.range((Included(lo), Excluded(pt))).next() {
            return Some(*k);
        }
    } else if let Some(k) = set.range((Excluded(prev), Excluded(pt))).next() {
        return Some(*k);
    }
    let next = set.range((Excluded(hi), Unbounded)).next()?;
    block_first(set, next.tier, next.group, point)
}

/// The merge order of D.8.5: `(tier, stale, est, group, h, id, kind)`, `h` the ring distance from
/// the session's point. `stale` is read from the group, which already orders the stale ones last
/// within a tier (1.8); `est` (1.10, §10.4) is 1 for a node to take without `est` when the caller
/// has a horizon, which no partition key knows, so it follows the group's stale band and precedes
/// the rest of it.
type MergeKey = (u8, bool, u8, i64, u64, u64, u8, usize);

/// Whether a group lies in the stale band (1.8): `STALE_SHIFT` above the others.
fn stale_band(group: i64) -> bool {
    group >= crate::index::STALE_SHIFT / 2
}

fn merge_key(k: &NextKey, point: u64, part: usize, only: Only) -> MergeKey {
    (
        k.tier,
        stale_band(k.group),
        u8::from(only == Only::Nodes),
        k.group,
        k.fnv.wrapping_sub(point),
        k.id,
        k.kind,
        part,
    )
}

/// Which keys of a partition a stream of the merge reads (1.10): under a horizon, a partition
/// without `est` is read as two streams, its nodes to take, which `est` puts after the estimated
/// candidates, and its other items, so that each stream's keys come in merge order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Only {
    /// Every key.
    All,
    /// The nodes to take (kind 0).
    Nodes,
    /// The status, review, and routing items.
    Others,
}

impl Only {
    fn takes(self, k: &NextKey) -> bool {
        match self {
            Only::All => true,
            Only::Nodes => k.kind == 0,
            Only::Others => k.kind != 0,
        }
    }
}

/// The first key at or after `from`, in ring order from `point`, that `only` takes; each key passed
/// over counts a visit.
fn first_taken<M: Meter>(
    set: &BTreeSet<NextKey>,
    from: Option<NextKey>,
    point: u64,
    only: Only,
    m: &mut M,
) -> Option<NextKey> {
    let mut k = from?;
    while !only.takes(&k) {
        m.tick(1);
        k = ring_after(set, k, point)?;
    }
    Some(k)
}

/// The candidate a key names.
fn cand_of(k: &NextKey) -> Cand {
    match k.kind {
        0 => Cand::Node(NodeId(k.id)),
        1 => Cand::Status(NodeId(k.id)),
        _ => Cand::Route(Eid(k.id)),
    }
}

/// Whether the caller holds `n` with a live hold (§10.4 `own`, A-115).
fn holds_node(s: &State, sess: SessionId, n: NodeId, now: Time) -> bool {
    s.ix.holds_by_session.get(&sess).is_some_and(|ts| {
        ts.iter()
            .any(|t| t.node == n && s.holds.get(t).is_some_and(|h| now < h.lease_until))
    })
}

/// Whether node `n` passes the query filter: `under` (ancestor-or-self), `kind`, `tags`.
fn query_ok(s: &State, n: NodeId, under: Option<NodeId>, q: &NextQuery) -> bool {
    let Some(node) = s.nodes.get(&n) else {
        return false;
    };
    under.is_none_or(|u| super::is_under(s, n, u))
        && q.kind
            .as_ref()
            .is_none_or(|k| node.kind.as_ref() == Some(k))
        && q.tags.is_subset(&node.tags)
}

/// The hour index of a time, as `aged` counts (CS-12).
fn hour(t: Time) -> i64 {
    i64::try_from(t.0.checked_div(HOUR.0).unwrap_or(0)).unwrap_or(i64::MAX)
}

/// `why_this = {tier, flags, aged, reason, est?, needs?}` of a candidate (D.8.5): `reason` the first that
/// applies of `Routing`, `Review` (1.9: an item with no hold on a closed node), `Status`,
/// `ClosedFlagged`, `Occurrence`, `Signal`, `Flagged`, `Offered`; 1.10: `no_estimate` for a node to
/// take without `est`, offered to a caller with a horizon. 1.11's `needs` of a review item, its
/// node's needs, is read from the node when the shell renders it.
pub(crate) fn why_this(
    s: &State,
    c: Cand,
    at: &CandAt,
    now: Time,
    horizon: Option<u32>,
) -> WhyThis {
    let (node, flags) = match c {
        Cand::Node(n) | Cand::Status(n) => (
            s.nodes.get(&n),
            s.ix.node.get(&n).map_or(FlagSet::EMPTY, |x| x.flags),
        ),
        Cand::Route(_) => (None, FlagSet::EMPTY),
    };
    let priority = node.map_or(0, |x| i64::from(x.priority));
    let aged = priority
        .saturating_add(hour(now))
        .saturating_sub(hour(at.since));
    let reason = match (c, node) {
        (Cand::Route(_), _) => Reason::Routing,
        (Cand::Status(_), Some(x)) if !x.is_open() => Reason::Review,
        (Cand::Status(_), _) => Reason::Status,
        (Cand::Node(_), Some(x)) if !x.is_open() => Reason::ClosedFlagged,
        (Cand::Node(_), Some(x)) if matches!(&x.role, Role::Occurrence(o) if o.phase == OccPhase::Available) => {
            Reason::Occurrence
        }
        (Cand::Node(_), Some(x)) if matches!(x.role, Role::Signal(_)) => Reason::Signal,
        (Cand::Node(_), _) if !flags.is_empty() => Reason::Flagged,
        (Cand::Node(_), _) => Reason::Offered,
    };
    WhyThis {
        tier: at.key.tier,
        flags,
        aged,
        reason,
        no_estimate: matches!(c, Cand::Node(_))
            && horizon.is_some()
            && node.is_some_and(|x| x.est.is_none()),
        longer: false,
        beyond_horizon: None,
    }
}

/// The counts of what every caller is held back from, read from `ix.next.held` (CS-16): open
/// dependencies, pauses, and the paused scopes by the number they excluded, most first (CS-9); and
/// the first node an open dependency holds back, with that target (1.6).
fn held_counts(s: &State, why: &mut EmptyWhy) {
    let mut scopes: BTreeMap<NodeId, u32> = BTreeMap::new();
    for (c, h) in &s.ix.next.held {
        if h.depends {
            why.depends = why.depends.saturating_add(1);
            if why.first.depends.is_none()
                && let Cand::Node(n) | Cand::Status(n) = *c
            {
                why.first.depends = open_dependency(s, n).map(|t| (n, t));
            }
        }
        if h.before_slot {
            why.before_slot = why.before_slot.saturating_add(1);
        }
        if let Some(p) = h.paused_by {
            why.paused = why.paused.saturating_add(1);
            let c = scopes.entry(p).or_insert(0);
            *c = c.saturating_add(1);
        }
    }
    let mut by_count: Vec<(u32, NodeId)> = scopes.into_iter().map(|(n, c)| (c, n)).collect();
    by_count.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    why.pause_scopes = by_count.into_iter().map(|(_, n)| n).collect();
}

/// The first open `depends_on` target of `n`, by id.
fn open_dependency(s: &State, n: NodeId) -> Option<NodeId> {
    s.nodes.get(&n).and_then(|x| {
        x.depends_on
            .iter()
            .copied()
            .find(|t| s.nodes.get(t).is_some_and(|y| y.is_open()))
    })
}

/// A key the scan keeps when it is before the one kept so far (1.6, `FirstExcluded`).
fn keep_first<T>(kept: &mut Option<(NextKey, T)>, key: NextKey, value: T) {
    if kept.as_ref().is_none_or(|(k, _)| key < *k) {
        *kept = Some((key, value));
    }
}

/// The first node of each exclusion that the caller's caps and horizon make in the partitions it
/// cannot take (1.6, §10.4): over every partition, the first key of each whose needs the caps do
/// not cover, and of each whose est bucket lies above the horizon's; with `boundary`, the first
/// key the scan skipped in the boundary bucket for an `est` over the horizon. Read only for an
/// empty answer, so an answer with candidates costs nothing more.
fn first_unfit<M: Meter>(
    s: &State,
    caller: &Caller<'_>,
    boundary: Option<(NextKey, u32)>,
    why: &mut EmptyWhy,
    m: &mut M,
) {
    let hb = caller.horizon.map(bit_len);
    let mut needs: Option<(NextKey, BTreeSet<Label>)> = None;
    let mut over: Option<(NextKey, u32)> = boundary;
    for (part, set) in &s.ix.next.parts {
        m.tick(1);
        let Some(first) = set.first().copied() else {
            continue;
        };
        if first.kind > 1 {
            continue;
        }
        if !part.needs.is_subset(caller.caps) {
            let lacks: BTreeSet<Label> = part.needs.difference(caller.caps).cloned().collect();
            keep_first(&mut needs, first, lacks);
        } else if let Some(h) = hb
            && part.est_bucket != 0
            && part.est_bucket > h
        {
            let est = s.nodes.get(&NodeId(first.id)).and_then(|x| x.est);
            if let Some(e) = est {
                keep_first(&mut over, first, e);
            }
        }
    }
    why.first.needs = needs.map(|(k, l)| (NodeId(k.id), l));
    why.first.horizon = over
        .zip(caller.horizon)
        .map(|((k, e), h)| (NodeId(k.id), e, h));
}

/// The first `k` candidates for `caller` in the order of D.8.5, read from `ix.next`, and the
/// counts of what excluded the others (CS-16, CS-31).
pub(crate) fn rank<M: Meter>(
    s: &State,
    caller: &Caller<'_>,
    now: Time,
    q: &NextQuery,
    k: usize,
    m: &mut M,
) -> (Vec<Ranked>, EmptyWhy) {
    let mut why = EmptyWhy {
        candidates: u32::try_from(s.ix.next.of.len()).unwrap_or(u32::MAX),
        // 1.4 (§10.4, R27): the nodes that wait, whatever the caller and the query.
        waiting: u32::try_from(s.ix.next.waiting.len()).unwrap_or(u32::MAX),
        ..EmptyWhy::default()
    };
    held_counts(s, &mut why);
    why.first.waiting = s.ix.next.waiting.first().copied();
    let query_empty = q.under.is_none() && q.kind.is_none() && q.tags.is_empty();
    let under = match &q.under {
        Some(r) => match super::resolve(s, r) {
            Some(u) => Some(u),
            None => return (Vec::new(), why),
        },
        None => None,
    };
    let el = eligible(s, caller.caps, caller.horizon, m);
    let in_parts =
        u64::try_from(s.ix.next.of.len().saturating_sub(s.ix.next.held.len())).unwrap_or(u64::MAX);
    why.needs = u32::try_from(in_parts.saturating_sub(el.needs_ok)).unwrap_or(u32::MAX);
    why.horizon = u32::try_from(el.over_horizon).unwrap_or(u32::MAX);
    let point = fnv_session(caller.sess);
    // 1.10 (§10.4): under a horizon, a partition without `est` is read as two streams.
    let horizon = caller.horizon.is_some();
    let sets: Vec<(&BTreeSet<NextKey>, bool, Only)> = el
        .parts
        .iter()
        .filter_map(|(p, boundary)| s.ix.next.parts.get(p).map(|x| (p, x, *boundary)))
        .flat_map(|(p, x, boundary)| {
            if horizon && p.est_bucket == 0 {
                alloc::vec![(x, boundary, Only::Nodes), (x, boundary, Only::Others)]
            } else {
                alloc::vec![(x, boundary, Only::All)]
            }
        })
        .collect();
    let mut heads: BTreeMap<MergeKey, NextKey> = BTreeMap::new();
    for (i, (set, _, only)) in sets.iter().enumerate() {
        let start = set
            .first()
            .and_then(|first| block_first(set, first.tier, first.group, point));
        if let Some(f) = first_taken(set, start, point, *only, m) {
            heads.insert(merge_key(&f, point, i, *only), f);
        }
    }
    let mut out: Vec<Ranked> = Vec::new();
    // 1.6 (§10.4): the first refused node and the first the horizon skipped in its boundary bucket.
    let mut refused: Option<(NextKey, ())> = None;
    let mut boundary_over: Option<(NextKey, u32)> = None;
    while out.len() < k {
        let Some((mk, key)) = heads.pop_first() else {
            break;
        };
        m.tick(1);
        let part = mk.7;
        if let Some((set, _, only)) = sets.get(part)
            && let Some(nk) = first_taken(set, ring_after(set, key, point), point, *only, m)
        {
            heads.insert(merge_key(&nk, point, part, *only), nk);
        }
        let c = cand_of(&key);
        let boundary = sets.get(part).is_some_and(|x| x.1);
        match c {
            Cand::Route(_) => {
                if !query_empty {
                    continue;
                }
            }
            Cand::Node(n) | Cand::Status(n) => {
                if crate::refusals::excluded(s, n, caller.class, now) {
                    why.refused = why.refused.saturating_add(1);
                    keep_first(&mut refused, key, ());
                    continue;
                }
                if matches!(c, Cand::Node(_)) && holds_node(s, caller.sess, n, now) {
                    why.own = why.own.saturating_add(1);
                    continue;
                }
                // 1.12 (§10.4): what the caller put down, which someone else is to pick up.
                if put_down(s, caller.sess, c, now) {
                    why.own = why.own.saturating_add(1);
                    continue;
                }
                if !query_ok(s, n, under, q) {
                    why.query = why.query.saturating_add(1);
                    if why.query >= SCAN_MAX {
                        why.query_capped = true;
                        break;
                    }
                    continue;
                }
                if boundary
                    && let (Some(e), Some(h)) =
                        (s.nodes.get(&n).and_then(|x| x.est), caller.horizon)
                    && e > h
                {
                    why.horizon = why.horizon.saturating_add(1);
                    keep_first(&mut boundary_over, key, e);
                    continue;
                }
            }
        }
        if let Some(at) = s.ix.next.of.get(&c) {
            out.push(Ranked {
                cand: c,
                why: why_this(s, c, at, now, caller.horizon),
            });
        }
    }
    // 1.12 (§10.4): work longer than every session's horizon, after every fitting candidate.
    if out.len() < k
        && let Some(h) = caller.horizon
    {
        let long = longer(
            s,
            caller,
            h,
            &el,
            point,
            now,
            (q, under),
            k.saturating_sub(out.len()),
            m,
        );
        out.extend(long);
    }
    // 1.12 (§10.4): the first candidate names the work the caller's horizon excludes.
    if let Some(first) = out.first_mut()
        && let Some(h) = caller.horizon
    {
        first.why.beyond_horizon = beyond_horizon(s, &el, h, m);
    }
    if out.is_empty() {
        why.first.refused = refused.map(|(key, ())| NodeId(key.id));
        first_unfit(s, caller, boundary_over, &mut why, m);
    }
    (out, why)
}

/// 1.12 (§10.4): what the caller put down, which `next` keeps from it: the review of an objection
/// it raised (`own_review`), and the node it released as `handoff` until another hold of it or
/// `lease.first` (`handed_back`).
pub(crate) fn put_down(s: &State, sess: SessionId, c: Cand, now: Time) -> bool {
    match c {
        Cand::Node(n) => crate::state::handed_back(s, sess, n, now),
        Cand::Status(n) => crate::state::own_review(s, sess, n),
        Cand::Route(_) => false,
    }
}

/// The smallest estimate of `est` bucket `b`: `2^(b−1)`, 0 for bucket 0 (no estimate).
fn bucket_low(b: u8) -> u64 {
    if b == 0 {
        0
    } else {
        1_u64
            .checked_shl(u32::from(b).saturating_sub(1))
            .unwrap_or(u64::MAX)
    }
}

/// 1.12 (§10.4): up to `k` nodes to take whose `est` exceeds the caller's horizon `h` and the
/// horizon of every session profile seen within `capacity.window` whose caps meet their non-empty
/// needs (`health::Horizons`), in the merge order of the rest, with the caller's exclusions:
/// work that no session's horizon covers, offered after every fitting candidate, so that a part of
/// it is done and handed off. It reads the partitions over the caller's horizon and its boundary
/// bucket, of non-empty needs, each only when a profile's longest horizon leaves room for one.
#[allow(
    clippy::too_many_arguments,
    reason = "rank's inputs that the second merge reads"
)]
fn longer<M: Meter>(
    s: &State,
    caller: &Caller<'_>,
    h: u32,
    el: &Eligible,
    point: u64,
    now: Time,
    (q, under): (&NextQuery, Option<NodeId>),
    k: usize,
    m: &mut M,
) -> Vec<Ranked> {
    let horizons = crate::health::Horizons::of(s, now);
    // Each partition with the estimate a node of it must exceed: the caller's horizon, and the
    // longest of the profiles that meet its needs; none when one of them has no horizon.
    let boundary = el
        .parts
        .iter()
        .filter(|(_, b)| *b)
        .map(|(p, _)| p)
        .chain(el.over.iter());
    let mut sets: Vec<(&BTreeSet<NextKey>, u64)> = Vec::new();
    for p in boundary {
        m.tick(1);
        if p.needs.is_empty() {
            continue;
        }
        let Some(longest) = horizons.longest(&p.needs) else {
            continue;
        };
        let above = u64::from(h.max(longest));
        // A bucket all of whose estimates are at or below `above` holds none.
        let high = bucket_low(p.est_bucket).saturating_mul(2).saturating_sub(1);
        if p.est_bucket == 0 || high <= above {
            continue;
        }
        if let Some(set) = s.ix.next.parts.get(p) {
            sets.push((set, above));
        }
    }
    let mut heads: BTreeMap<MergeKey, NextKey> = BTreeMap::new();
    for (i, (set, _)) in sets.iter().enumerate() {
        let start = set
            .first()
            .and_then(|first| block_first(set, first.tier, first.group, point));
        if let Some(f) = first_taken(set, start, point, Only::Nodes, m) {
            heads.insert(merge_key(&f, point, i, Only::Nodes), f);
        }
    }
    let mut out = Vec::new();
    while out.len() < k {
        let Some((mk, key)) = heads.pop_first() else {
            break;
        };
        m.tick(1);
        let part = mk.7;
        let Some(&(set, above)) = sets.get(part) else {
            continue;
        };
        if let Some(nk) = first_taken(set, ring_after(set, key, point), point, Only::Nodes, m) {
            heads.insert(merge_key(&nk, point, part, Only::Nodes), nk);
        }
        let n = NodeId(key.id);
        let c = Cand::Node(n);
        if s.nodes
            .get(&n)
            .and_then(|x| x.est)
            .is_none_or(|e| u64::from(e) <= above)
            || crate::refusals::excluded(s, n, caller.class, now)
            || holds_node(s, caller.sess, n, now)
            || put_down(s, caller.sess, c, now)
            || !query_ok(s, n, under, q)
        {
            continue;
        }
        if let Some(at) = s.ix.next.of.get(&c) {
            let mut why = why_this(s, c, at, now, caller.horizon);
            why.longer = true;
            out.push(Ranked { cand: c, why });
        }
    }
    out
}

/// 1.12 (§10.4): the nodes whose non-empty needs the caller's caps meet and whose `est` exceeds its
/// horizon `h`: whole buckets above the horizon's by their size, and the bucket the horizon falls in
/// entry by entry, at most `SCAN_MAX` entries, `capped` when that scan stopped there (§18); the
/// first node to take among them in the index's order, never a status or review item. None when
/// there is no such node.
fn beyond_horizon<M: Meter>(
    s: &State,
    el: &Eligible,
    h: u32,
    m: &mut M,
) -> Option<crate::reply::BeyondHorizon> {
    let mut count: u32 = 0;
    let mut first: Option<(NextKey, u32)> = None;
    let est = |k: &NextKey| s.nodes.get(&NodeId(k.id)).and_then(|x| x.est);
    for p in &el.over {
        m.tick(1);
        if p.needs.is_empty() {
            continue;
        }
        let Some(set) = s.ix.next.parts.get(p) else {
            continue;
        };
        count = count.saturating_add(u32::try_from(set.len()).unwrap_or(u32::MAX));
        // 1.12's review: the first is a node to take (kind 0), never a status or review item.
        if let Some(k) = set.iter().find(|k| k.kind == 0)
            && let Some(e) = est(k)
        {
            keep_first(&mut first, *k, e);
        }
    }
    let mut visits: u32 = 0;
    let mut capped = false;
    for (p, boundary) in &el.parts {
        if !*boundary || p.needs.is_empty() {
            continue;
        }
        let Some(set) = s.ix.next.parts.get(p) else {
            continue;
        };
        for k in set {
            if visits >= SCAN_MAX {
                // 1.12's review (§18): the count is then a lower bound, `≥k`.
                capped = true;
                break;
            }
            visits = visits.saturating_add(1);
            m.tick(1);
            if let Some(e) = est(k).filter(|e| *e > h) {
                count = count.saturating_add(1);
                if k.kind == 0 {
                    keep_first(&mut first, *k, e);
                }
            }
        }
    }
    first.map(|(k, e)| crate::reply::BeyondHorizon {
        count,
        capped,
        first: NodeId(k.id),
        est: e,
        horizon: h,
    })
}

/// `rank` with the ring component broken by id instead (`TieBreak::Id`, D.6.1, D.10.6): the
/// candidates `spec::ranked` admits, sorted by `(tier, stale, est, group, id, kind)`. Only with the
/// test-only feature `conformance`.
#[cfg(feature = "conformance")]
pub(crate) fn rank_by_id(
    s: &State,
    caller: &Caller<'_>,
    now: Time,
    q: &NextQuery,
    k: usize,
) -> (Vec<Ranked>, EmptyWhy) {
    let (_, why) = rank(s, caller, now, q, usize::MAX, &mut super::NoMeter);
    let mut all: Vec<(NextKey, Cand)> = crate::spec::ranked(s, caller.sess, now, q)
        .into_iter()
        .filter_map(|c| s.ix.next.of.get(&c).map(|a| (a.key, c)))
        .collect();
    let unestimated = |c: &Cand| {
        caller.horizon.is_some()
            && matches!(c, Cand::Node(n) if s.nodes.get(n).is_some_and(|x| x.est.is_none()))
    };
    // 1.12 (§10.4): a node over the caller's horizon that `spec::ranked` admits is longer than
    // every session's, after every other candidate.
    let longer = |c: &Cand| {
        matches!(c, Cand::Node(n) if matches!(
            (s.nodes.get(n).and_then(|x| x.est), caller.horizon),
            (Some(e), Some(h)) if e > h
        ))
    };
    all.sort_by_key(|(key, c)| {
        (
            longer(c),
            key.tier,
            stale_band(key.group),
            unestimated(c),
            key.group,
            key.id,
            key.kind,
        )
    });
    let out = all
        .into_iter()
        .take(k)
        .filter_map(|(_, c)| {
            s.ix.next.of.get(&c).map(|at| Ranked {
                cand: c,
                why: why_this(s, c, at, now, caller.horizon),
            })
        })
        .collect();
    (out, why)
}
