//! The state invariants of §26 (design D.9.2, the rows of kind S): `check_invariants`, the subset
//! `check_startup` that is linear in the state (D.9.1), and K2's shell half `check_keys`.
//!
//! Every check computes from the base state, through the recount (R1, T4) and the naive
//! definitions of `spec` (F, W1, E1, M1, N1, Settled), never through `evolve`'s incremental code.
//! Every state invariant ranges over the resident state (D.4.12).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::deadlines::{deadline_at_of, recount_deadlines};
use crate::invariants::{Inv, Violation, ViolationDetail};
use crate::recount::recount;
use crate::spec;
use crate::state::{
    CommitmentState, DeadlineKey, DeadlineKind, DigestState, EntityRef, Life, NodeIx, OccPhase,
    Origin, Owner, PqState, Role, Routing, SaidState, State, UqSource, UqState, WaitOn,
};
use crate::types::{
    EntryType, Flag, FlagSet, KeyStr, NodeId, NormVer, PEOPLE, PrincipalId, QuestionId, ROOT,
    Requester, SIGNALS, SYS, SaidKind, TEAMS, Time, USER, USER_DECISIONS, USER_DIGEST,
    USER_PRINCIPAL,
};
use crate::user::UQ_CHAIN_MAX;

/// Collected violations.
struct Out(Vec<Violation>);

impl Out {
    fn push(&mut self, inv: Inv, detail: ViolationDetail) {
        self.0.push(Violation { inv, detail });
    }

    fn check(&mut self, ok: bool, inv: Inv, detail: ViolationDetail) {
        if !ok {
            self.push(inv, detail);
        }
    }

    fn done(self) -> Result<(), Vec<Violation>> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(self.0)
        }
    }
}

/// Every state invariant of D.9.2 on a state settled at `now`: Total, H1 to H6, O1, O2, R1, K1 to
/// K6, K8, Q1, S1, N1, Settled, F, E1, W1, R2, D1, M1, U1, T1, T3, T4, T5, L1, and P1 (the full
/// check of D.9.1).
///
/// # Errors
///
/// Every violation found.
pub fn check_invariants(s: &State, now: Time) -> Result<(), Vec<Violation>> {
    let mut o = Out(Vec::new());
    linear(s, now, &mut o);
    r1(s, now, &mut o);
    o2(s, &mut o);
    k2(s, &mut o);
    q1(s, &mut o);
    n1(s, &mut o);
    settled(s, &mut o);
    f(s, now, &mut o);
    e1_full(s, now, &mut o);
    w1(s, now, &mut o);
    r2(s, &mut o);
    d1(s, &mut o);
    m1(s, now, &mut o);
    t1(s, &mut o);
    t5(s, &mut o);
    l1(s, &mut o);
    o.done()
}

/// The subset that runs at every startup, on the state just loaded and its freshly recounted
/// indices (D.9.1, G0-a): Total; H1 to H6; K1, K3, K4, K5, K6; K8 (A-118); S1; T3; T4; U1; P1;
/// E1 (b) and (e); and O1's state half. Linear in the state, up to its depth.
///
/// # Errors
///
/// Every violation found.
pub fn check_startup(s: &State, now: Time) -> Result<(), Vec<Violation>> {
    let mut o = Out(Vec::new());
    linear(s, now, &mut o);
    o.done()
}

/// K2's shell half (D.9.2): every stored key and every resident request's origin made by a
/// normaliser (version above 0) is normal under its version, by the shell's oracle `is_normal`
/// (`forum_keys`, D.11.14).
///
/// # Errors
///
/// The keys and origins that are not normal.
pub fn check_keys(
    s: &State,
    is_normal: &dyn Fn(&KeyStr, NormVer) -> bool,
) -> Result<(), Vec<Violation>> {
    let mut o = Out(Vec::new());
    for node in s.nodes.values() {
        for (k, &nv) in &node.keys {
            if nv != NormVer(0) && !is_normal(k, nv) {
                o.push(Inv::K2, ViolationDetail::Key(k.clone()));
            }
        }
    }
    for r in s.requests.values() {
        if r.origin.nv != NormVer(0) && !is_normal(&r.origin.key, r.origin.nv) {
            o.push(Inv::K2, ViolationDetail::Key(r.origin.key.clone()));
        }
    }
    o.done()
}

/// The checks of `check_startup`.
fn linear(s: &State, now: Time, o: &mut Out) {
    o.check(
        s.anomalies == 0,
        Inv::Total,
        ViolationDetail::Text("anomalies"),
    );
    holds(s, now, o);
    o1(s, o);
    k1(s, o);
    k3(s, o);
    k4(s, o);
    k5(s, o);
    k6(s, o);
    k8(s, o);
    s1(s, o);
    t3(s, now, o);
    t4(s, now, o);
    u1(s, o);
    p1(s, now, o);
    e1_linear(s, now, o);
}

// ------------------------------------------------------------------------------------------------
// Slots and tokens

/// H1, H2 (state half), H3, H5, H6.
fn holds(s: &State, now: Time, o: &mut Out) {
    let mut per_node: BTreeMap<NodeId, u32> = BTreeMap::new();
    let mut per_session: BTreeMap<crate::types::SessionId, u32> = BTreeMap::new();
    for (t, h) in &s.holds {
        if now < h.lease_until {
            let c = per_node.entry(t.node).or_insert(0);
            *c = c.saturating_add(1);
            let c = per_session.entry(h.session).or_insert(0);
            *c = c.saturating_add(1);
        }
        // H2: the taker exists, retires after the hold's ceiling, and is not silent (1.4, R27).
        match s.sessions.get(&h.session) {
            Some(x) => {
                o.check(
                    x.retire_at > h.taken_at.plus(h.terms.ceiling),
                    Inv::H2,
                    ViolationDetail::Hold(*t),
                );
                o.check(!x.silent, Inv::H2, ViolationDetail::Hold(*t));
            }
            None => o.push(Inv::H2, ViolationDetail::Hold(*t)),
        }
        // H3: the token's fence is at most its node's.
        let node = s.nodes.get(&t.node);
        o.check(
            node.is_some_and(|n| t.fence <= n.fence),
            Inv::H3,
            ViolationDetail::Hold(*t),
        );
        // H5, with the hold's own terms (A-102).
        let tm = &h.terms;
        let ok = h.taken_at < h.lease_until
            && h.lease_until
                <= h.taken_at
                    .plus(tm.ceiling)
                    .min(h.last_extended.plus(tm.max))
            && (h.checkpointed || h.lease_until <= h.taken_at.plus(tm.first))
            && h.renews <= tm.renew_max;
        o.check(ok, Inv::H5, ViolationDetail::Hold(*t));
        // H6: no hold on a closed node.
        o.check(
            node.is_some_and(crate::state::Node::is_open),
            Inv::H6,
            ViolationDetail::Hold(*t),
        );
    }
    for (n, c) in per_node {
        let cap = s.nodes.get(&n).map_or(0, |x| x.capacity);
        o.check(c <= cap, Inv::H1, ViolationDetail::Node(n));
    }
    for (x, c) in per_session {
        o.check(
            c <= s.params.hold_per_session,
            Inv::H2,
            ViolationDetail::Session(x),
        );
    }
}

// ------------------------------------------------------------------------------------------------
// History and retries

/// O1, state half: every stored record expires after `last_now` (CS-8).
fn o1(s: &State, o: &mut Out) {
    for r in s.ops.values() {
        o.check(
            r.expires_at > s.last_now,
            Inv::O1,
            ViolationDetail::Session(r.session),
        );
    }
}

/// O2, state half: every kept entry's `seq` is at most its node's; every node's version is at
/// least 1 (the fences of its holds are H3's).
fn o2(s: &State, o: &mut Out) {
    for (&e, m) in &s.entries {
        o.check(
            s.nodes.get(&m.node).is_some_and(|n| m.seq <= n.seq),
            Inv::O2,
            ViolationDetail::Entry(e),
        );
    }
    for (&n, node) in &s.nodes {
        o.check(node.version.0 >= 1, Inv::O2, ViolationDetail::Node(n));
    }
}

/// R1: every index equals the recount over the resident state, except the flags (F) and the
/// deadline queue (T4); `ix.next.of`'s key set equals `State::since`'s, each `since` is the stored
/// one, and none lies after `now`.
#[allow(clippy::too_many_lines, reason = "one comparison per index of D.4.10")]
fn r1(s: &State, now: Time, o: &mut Out) {
    let rc = recount(s, now);
    let ix = &s.ix;
    let masked = |m: &BTreeMap<NodeId, NodeIx>| -> BTreeMap<NodeId, NodeIx> {
        m.iter()
            .map(|(&k, v)| {
                (
                    k,
                    NodeIx {
                        flags: FlagSet::EMPTY,
                        ..*v
                    },
                )
            })
            .collect()
    };
    let mut cmp = |ok: bool, name: &'static str| o.check(ok, Inv::R1, ViolationDetail::Index(name));
    cmp(masked(&ix.node) == masked(&rc.node), "node");
    cmp(ix.children == rc.children, "children");
    cmp(
        ix.flagged_children == rc.flagged_children,
        "flagged_children",
    );
    cmp(
        ix.attended_children == rc.attended_children,
        "attended_children",
    );
    cmp(
        ix.unheld_flagged_children == rc.unheld_flagged_children,
        "unheld_flagged_children",
    );
    cmp(
        ix.unheld_flagged_by_part == rc.unheld_flagged_by_part,
        "unheld_flagged_by_part",
    );
    cmp(ix.keys_open == rc.keys_open, "keys_open");
    cmp(ix.keys_all == rc.keys_all, "keys_all");
    cmp(ix.origins == rc.origins, "origins");
    cmp(
        ix.unsettled_requests == rc.unsettled_requests,
        "unsettled_requests",
    );
    cmp(ix.requests_by == rc.requests_by, "requests_by");
    cmp(ix.unsettled_by == rc.unsettled_by, "unsettled_by");
    cmp(ix.people == rc.people, "people");
    cmp(ix.items == rc.items, "items");
    cmp(ix.settling == rc.settling, "settling");
    cmp(ix.clarifications == rc.clarifications, "clarifications");
    cmp(ix.flagged == rc.flagged, "flagged");
    cmp(ix.flagged_active == rc.flagged_active, "flagged_active");
    cmp(ix.next.of == rc.next.of, "next.of");
    cmp(ix.next.parts == rc.next.parts, "next.parts");
    cmp(ix.next.held == rc.next.held, "next.held");
    cmp(ix.next.nodes == rc.next.nodes, "next.nodes");
    cmp(ix.next.needs_trie == rc.next.needs_trie, "next.needs_trie");
    cmp(ix.next.waiting == rc.next.waiting, "next.waiting");
    cmp(ix.series_open == rc.series_open, "series_open");
    cmp(
        ix.flagged_occurrences == rc.flagged_occurrences,
        "flagged_occurrences",
    );
    cmp(ix.agreements == rc.agreements, "agreements");
    cmp(ix.unrouted == rc.unrouted, "unrouted");
    cmp(ix.pending_uq_by == rc.pending_uq_by, "pending_uq_by");
    cmp(
        ix.refusal_question == rc.refusal_question,
        "refusal_question",
    );
    cmp(
        ix.pending_digest_by == rc.pending_digest_by,
        "pending_digest_by",
    );
    cmp(ix.waiters == rc.waiters, "waiters");
    cmp(ix.dependents == rc.dependents, "dependents");
    cmp(ix.backlinks == rc.backlinks, "backlinks");
    cmp(ix.cited_by == rc.cited_by, "cited_by");
    cmp(
        ix.marked_by_question == rc.marked_by_question,
        "marked_by_question",
    );
    cmp(
        ix.holds_by_session == rc.holds_by_session,
        "holds_by_session",
    );
    cmp(ix.open_saids_by == rc.open_saids_by, "open_saids_by");
    cmp(ix.unrouted_by == rc.unrouted_by, "unrouted_by");
    cmp(ix.refused == rc.refused, "refused");
    cmp(ix.live_sessions == rc.live_sessions, "live_sessions");
    cmp(ix.live_by_class == rc.live_by_class, "live_by_class");
    cmp(ix.ops_expiry == rc.ops_expiry, "ops_expiry");
    cmp(ix.notes_until == rc.notes_until, "notes_until");
    cmp(ix.leaving_by_time == rc.leaving_by_time, "leaving_by_time");
    cmp(ix.inbox == rc.inbox, "inbox");
    cmp(ix.pending_holder == rc.pending_holder, "pending_holder");
    cmp(ix.branch_notes == rc.branch_notes, "branch_notes");
    cmp(ix.user_notes == rc.user_notes, "user_notes");
    cmp(ix.principal_key == rc.principal_key, "principal_key");
    cmp(ix.person_of == rc.person_of, "person_of");
    cmp(ix.aliases_of == rc.aliases_of, "aliases_of");
    cmp(
        ix.profiles_by_time == rc.profiles_by_time,
        "profiles_by_time",
    );
    cmp(ix.class_ends == rc.class_ends, "class_ends");
    let of: BTreeSet<_> = ix.next.of.keys().copied().collect();
    let since: BTreeSet<_> = s.since.keys().copied().collect();
    cmp(of == since, "since keys");
    cmp(
        ix.next
            .of
            .iter()
            .all(|(c, a)| s.since.get(c) == Some(&a.since)),
        "since values",
    );
    cmp(s.since.values().all(|&t| t <= now), "since after now");
}

/// The indices whose recount R1 compares, by name: the list `r1` checks (for the tests).
#[must_use]
pub fn r1_differences(s: &State, now: Time) -> Vec<&'static str> {
    let mut o = Out(Vec::new());
    r1(s, now, &mut o);
    o.0.into_iter()
        .filter_map(|v| match v.detail {
            ViolationDetail::Index(n) => Some(n),
            ViolationDetail::Node(_)
            | ViolationDetail::Hold(_)
            | ViolationDetail::Session(_)
            | ViolationDetail::Entry(_)
            | ViolationDetail::Key(_)
            | ViolationDetail::Deadline(_)
            | ViolationDetail::Text(_) => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Keys and names

/// K1: a key is carried by at most one open node, counted over `nodes[*].keys`.
fn k1(s: &State, o: &mut Out) {
    let mut carriers: BTreeMap<&KeyStr, u32> = BTreeMap::new();
    for node in s.nodes.values() {
        if node.is_open() {
            for k in node.keys.keys() {
                let c = carriers.entry(k).or_insert(0);
                *c = c.saturating_add(1);
            }
        }
    }
    for (k, c) in carriers {
        o.check(c <= 1, Inv::K1, ViolationDetail::Key(k.clone()));
    }
}

/// K2's core part: every key or origin of version 0 is a canonical `user:e<n>` with `n` below
/// `next_eid`, or a canonical `repeat:<id>:<slot>` whose id names a node and whose slot name
/// decodes to a minute; no key or origin of another version has the `repeat` scheme (A-90).
///
/// §26 K2 and D.9.2 say the id names "a series" (and §26 "one of its slots"). That reading fails
/// on reachable states, so the check takes the weaker one (an open question): an
/// occurrence keeps its `repeat:` key when `repeat` is removed from its series, which detaches an
/// available or held occurrence and turns the series `Plain` (§12, `OccurrenceDetached`,
/// `NodeUpdated`); a closed series whose `repeat` is removed turns `Plain` with its past
/// occurrences' keys; a merge from an available occurrence moves its key to the surviving node
/// (§16); and a change of the schedule leaves the earlier occurrences' slots outside the new one.
/// The state keeps no record that a `Plain` node was a series, so nothing stronger than "names a
/// node" can be checked; only the server makes `repeat:` keys (§7), and only for an occurrence of
/// a series, which is what the stronger reading guards.
fn k2(s: &State, o: &mut Out) {
    let check = |o: &mut Out, k: &KeyStr, nv: NormVer| {
        if nv == NormVer(0) {
            let user = k
                .user_entry()
                .is_some_and(|e| KeyStr::of_user_entry(e) == *k && e < s.counters.next_eid);
            let repeat = parse_repeat(k).is_some_and(|(series, slot)| {
                KeyStr::of_occurrence(series, slot) == *k && s.nodes.contains_key(&series)
            });
            o.check(user || repeat, Inv::K2, ViolationDetail::Key(k.clone()));
        } else {
            o.check(!k.is_reserved(), Inv::K2, ViolationDetail::Key(k.clone()));
        }
    };
    for node in s.nodes.values() {
        for (k, &nv) in &node.keys {
            check(o, k, nv);
        }
    }
    for r in s.requests.values() {
        check(o, &r.origin.key, r.origin.nv);
    }
}

/// The series and slot of a key `repeat:<id>:<YYYYMMDDTHHMMZ>`.
fn parse_repeat(k: &KeyStr) -> Option<(NodeId, Time)> {
    let rest = k.as_str().strip_prefix("repeat:")?;
    let (id, name) = rest.split_once(':')?;
    let id: u64 = id.parse().ok()?;
    let b = name.as_bytes();
    if b.len() != 14 || b.get(8) != Some(&b'T') || b.get(13) != Some(&b'Z') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { name.get(from..to)?.parse().ok() };
    let (y, mo, d, h, mi) = (
        num(0, 4)?,
        num(4, 6)?,
        num(6, 8)?,
        num(9, 11)?,
        num(11, 13)?,
    );
    let days = forum_cron::days_from_civil(y, u8::try_from(mo).ok()?, u8::try_from(d).ok()?);
    let minutes = days
        .checked_mul(1440)?
        .checked_add(h.checked_mul(60)?)?
        .checked_add(mi)?;
    let ms = u64::try_from(minutes.checked_mul(60_000)?).ok()?;
    Some((NodeId(id), Time(ms)))
}

/// K3: live names and aliases are disjoint; every node but ROOT has its place as a live name that
/// maps to it; every live name maps to a node with that place; every alias maps to a node.
fn k3(s: &State, o: &mut Out) {
    for (key, &n) in &s.names {
        o.check(
            !s.aliases.contains_key(key),
            Inv::K3,
            ViolationDetail::Node(n),
        );
        let place_ok = s
            .nodes
            .get(&n)
            .and_then(|x| x.place.as_ref())
            .is_some_and(|p| p.parent == key.0 && p.name == key.1);
        o.check(place_ok, Inv::K3, ViolationDetail::Node(n));
    }
    for (&n, node) in &s.nodes {
        match &node.place {
            Some(p) => o.check(
                s.names.get(&(p.parent, p.name.clone())) == Some(&n),
                Inv::K3,
                ViolationDetail::Node(n),
            ),
            None => o.check(n == ROOT, Inv::K3, ViolationDetail::Node(n)),
        }
    }
    for &n in s.aliases.values() {
        o.check(s.nodes.contains_key(&n), Inv::K3, ViolationDetail::Node(n));
    }
}

/// K4: from every node, the parent walk reaches ROOT within `nodes.len()` steps.
fn k4(s: &State, o: &mut Out) {
    for &n in s.nodes.keys() {
        let mut cur = n;
        let mut ok = false;
        for _ in 0..=s.nodes.len() {
            if cur == ROOT {
                ok = true;
                break;
            }
            match s.nodes.get(&cur).and_then(crate::state::Node::parent) {
                Some(p) => cur = p,
                None => break,
            }
        }
        o.check(ok, Inv::K4, ViolationDetail::Node(n));
    }
}

/// K5: `merged_into` only on closed `superseded` nodes, naming a node; every chain ends within
/// `nodes.len()` steps at a node that was not merged.
fn k5(s: &State, o: &mut Out) {
    for (&n, node) in &s.nodes {
        if let Life::Closed(c) = &node.life
            && let Some(i) = c.merged_into
        {
            o.check(
                c.outcome == crate::types::Outcome::Superseded && s.nodes.contains_key(&i),
                Inv::K5,
                ViolationDetail::Node(n),
            );
        }
        let mut cur = n;
        let mut ended = false;
        for _ in 0..=s.nodes.len() {
            match s.nodes.get(&cur).and_then(crate::state::Node::merged_into) {
                Some(next) => cur = next,
                None => {
                    ended = true;
                    break;
                }
            }
        }
        o.check(ended, Inv::K5, ViolationDetail::Node(n));
    }
}

/// K6: an open series has exactly one open occurrence, a closed series none.
fn k6(s: &State, o: &mut Out) {
    let mut open_occ: BTreeMap<NodeId, u32> = BTreeMap::new();
    for node in s.nodes.values() {
        if let (Life::Open, Role::Occurrence(x)) = (&node.life, &node.role) {
            let c = open_occ.entry(x.series).or_insert(0);
            *c = c.saturating_add(1);
        }
    }
    for (&x, node) in &s.nodes {
        if let Role::Series(_) = node.role {
            let c = open_occ.get(&x).copied().unwrap_or(0);
            let want = u32::from(node.is_open());
            o.check(c == want, Inv::K6, ViolationDetail::Node(x));
        }
    }
}

/// K8: the eight reserved nodes exist with origin `Reserved`, open, at their Genesis places, with
/// capacity 0, role `Plain`, and no key; no other node has origin `Reserved` (§4, A-118).
fn k8(s: &State, o: &mut Out) {
    let places: [(NodeId, Option<(NodeId, &str)>); 8] = [
        (ROOT, None),
        (USER, Some((ROOT, "@user"))),
        (USER_DECISIONS, Some((USER, "decisions"))),
        (USER_DIGEST, Some((USER, "digest"))),
        (SYS, Some((ROOT, "sys"))),
        (SIGNALS, Some((SYS, "signals"))),
        (PEOPLE, Some((ROOT, "@people"))),
        (TEAMS, Some((ROOT, "@teams"))),
    ];
    for (id, place) in places {
        let ok = s.nodes.get(&id).is_some_and(|n| {
            let at_place = match (&n.place, place) {
                (None, None) => true,
                (Some(p), Some((parent, name))) => p.parent == parent && p.name.as_str() == name,
                (Some(_), None) | (None, Some(_)) => false,
            };
            n.origin == Origin::Reserved
                && n.is_open()
                && at_place
                && n.capacity == 0
                && n.role == Role::Plain
                && n.keys.is_empty()
        });
        o.check(ok, Inv::K8, ViolationDetail::Node(id));
    }
    for (&n, node) in &s.nodes {
        if node.origin == Origin::Reserved {
            o.check(n.is_reserved(), Inv::K8, ViolationDetail::Node(n));
        }
    }
}

/// Q1: request origins, the messages they came in, are pairwise distinct (A-61).
fn q1(s: &State, o: &mut Out) {
    let mut seen: BTreeSet<&KeyStr> = BTreeSet::new();
    for (&r, x) in &s.requests {
        o.check(
            seen.insert(&x.origin.key),
            Inv::Q1,
            ViolationDetail::Entry(r),
        );
    }
}

/// S1: at most one open signal node per key, and `signals` equals the open signal nodes with their
/// keys (A-89).
fn s1(s: &State, o: &mut Out) {
    let mut open: BTreeMap<crate::state::SignalKey, Vec<NodeId>> = BTreeMap::new();
    for (&n, node) in &s.nodes {
        if let (Life::Open, Role::Signal(k)) = (&node.life, &node.role) {
            open.entry(*k).or_default().push(n);
        }
    }
    for (k, ns) in &open {
        o.check(
            ns.len() == 1,
            Inv::S1,
            ViolationDetail::Text("two open signals"),
        );
        for n in ns {
            o.check(
                s.signals.get(k) == Some(n),
                Inv::S1,
                ViolationDetail::Node(*n),
            );
        }
    }
    for (k, n) in &s.signals {
        o.check(
            open.get(k).is_some_and(|ns| ns.contains(n)),
            Inv::S1,
            ViolationDetail::Node(*n),
        );
        o.check(k.a < k.b, Inv::S1, ViolationDetail::Node(*n));
    }
}

// ------------------------------------------------------------------------------------------------
// Nothing is lost

/// N1: `unrouted ∈ flags(@user)`, the materialised flags, exactly when some user entry is
/// unrouted.
fn n1(s: &State, o: &mut Out) {
    let any = s
        .user_entries
        .values()
        .any(|u| u.routing == Routing::Unrouted);
    let flagged =
        s.ix.node
            .get(&USER)
            .is_some_and(|x| x.flags.contains(Flag::Unrouted));
    o.check(flagged == any, Inv::N1, ViolationDetail::Node(USER));
}

/// Settled, state half, over the resident requests.
///
/// Every state invariant ranges over the resident state (D.4.12), so a clause about an entity
/// that is not resident is not evaluated, as K2 and D1 do (D.9.2): the decision once it left
/// memory, and the answer once neither its metadata nor its `said` is resident. The answer is
/// held back while its request is resident (D.4.12), but a command that names a settled request
/// whose answer has left too restores the request alone (D.4.12 lists only the ids the command
/// names), and the request is then resident without its answer until it leaves again. The answer
/// was checked while it was resident: only a finished entity leaves memory.
fn settled(s: &State, o: &mut Out) {
    for (&r, req) in &s.requests {
        match req.settled {
            Some(st) => {
                let listed = s
                    .decisions
                    .get(&st.decision)
                    .is_none_or(|d| d.settles.contains(&r));
                let answer = Some(st.answer) == req.answered_by && st.answer > r;
                let a = st.answer;
                let kind = match req.requester {
                    Requester::External => {
                        let resident = s.entries.contains_key(&a) || s.saids.contains_key(&a);
                        !resident
                            || s.entries.get(&a).is_some_and(|m| m.ty == EntryType::Said)
                                && s.saids.get(&a).is_some_and(|x| {
                                    matches!(
                                        x.kind,
                                        SaidKind::Reply | SaidKind::Result | SaidKind::Correction
                                    ) && x.conv == req.conv
                                })
                    }
                    Requester::User(_) => s
                        .entries
                        .get(&a)
                        .is_none_or(|m| m.ty == EntryType::ToUser && m.principal.is_some()),
                };
                o.check(
                    listed && answer && kind,
                    Inv::Settled,
                    ViolationDetail::Entry(r),
                );
            }
            None => o.check(
                !spec::settled(s, r),
                Inv::Settled,
                ViolationDetail::Entry(r),
            ),
        }
    }
}

/// F: F1, the materialised flags equal `spec::flags` at `now`; F2, no flag changes before the next
/// queued deadline (D.8.3).
fn f(s: &State, now: Time, o: &mut Out) {
    let next = s.ix.deadlines.first().map_or(Time(u64::MAX), |k| k.at);
    let before_next = Time(next.0.saturating_sub(1)).max(now);
    for &n in s.nodes.keys() {
        let fl = spec::flags(s, n, now);
        let stored = s.ix.node.get(&n).map_or(FlagSet::EMPTY, |x| x.flags);
        o.check(stored == fl, Inv::F, ViolationDetail::Node(n));
        o.check(
            spec::flags(s, n, before_next) == fl,
            Inv::F,
            ViolationDetail::Node(n),
        );
    }
}

/// E1 (b) and (e), which `check_startup` runs.
fn e1_linear(s: &State, now: Time, o: &mut Out) {
    // (b)
    for ((n, _), r) in &s.refusals {
        o.check(!r.by.is_empty(), Inv::E1, ViolationDetail::Node(*n));
        for x in r.by.values() {
            o.check(
                x.backoff_until <= x.expires_at && x.expires_at > now,
                Inv::E1,
                ViolationDetail::Node(*n),
            );
        }
    }
    for a in s.classes.values() {
        o.check(
            !a.caps_seen.is_empty() && a.caps_seen.values().all(|&t| t > now),
            Inv::E1,
            ViolationDetail::Text("class profile"),
        );
    }
    // (e)
    for (&q, uq) in &s.uquestions {
        if uq.state == UqState::Pending {
            o.check(
                s.ix.deadlines.contains(&DeadlineKey {
                    at: uq.due,
                    kind: DeadlineKind::UserQuestion,
                    owner: Owner::Entry(q),
                }),
                Inv::E1,
                ViolationDetail::Entry(q),
            );
        }
    }
}

/// User question `uq` was made (its id is below `next_eid`) and is not resident: it left memory,
/// which only a finished user question does (D.4.12). It is held back while its escalated people
/// question is resident, but a command that names that people question after both have left
/// restores the people question alone (D.4.12 lists only the ids the command names), as `heard
/// {answers}` and `hold release {waits_for: {question}}` do. The user question was checked while it
/// was resident.
fn left_memory(s: &State, uq: crate::types::Eid) -> bool {
    uq < s.counters.next_eid && !s.uquestions.contains_key(&uq)
}

/// Whether user question `uq`, with source `Escalation { q }`, was answered after all (E1 (f), M1,
/// D.8.15): `q` is `Answered`, or `q` has left memory, which a people question escalated to a user
/// question that lapsed with no assumption does only once answered (D.4.12, the year simulation's
/// Y-1: E1 (f) and M1 hold after `q` leaves).
fn answered_after_all(s: &State, uq: &crate::state::UQuestion) -> bool {
    match uq.source {
        UqSource::Escalation { question } => match s.pquestions.get(&question) {
            Some(pq) => matches!(pq.state, PqState::Answered { .. }),
            None => question < s.counters.next_eid,
        },
        UqSource::Agent { .. } | UqSource::Refusal { .. } => false,
    }
}

/// An assume mark of `q` on an open node (reserved allowed or not).
fn assume_marked(s: &State, q: crate::types::Eid, reserved_ok: bool) -> bool {
    s.nodes.iter().any(|(n, x)| {
        x.is_open() && x.assume_marks.contains(&q) && (reserved_ok || !n.is_reserved())
    })
}

/// E1 (a), (c), (d), and (f).
fn e1_full(s: &State, now: Time, o: &mut Out) {
    // (a)
    for &n in s.nodes.keys() {
        if spec::all_refused(s, n, now) {
            let covered = s
                .uquestions
                .values()
                .any(|u| u.state == UqState::Pending && u.source == UqSource::Refusal { node: n });
            o.check(covered, Inv::E1, ViolationDetail::Node(n));
        }
    }
    // (c)
    for (&q, pq) in &s.pquestions {
        if pq.state == PqState::Expired && pq.assumed_by.is_none() {
            o.check(
                assume_marked(s, q, true),
                Inv::E1,
                ViolationDetail::Entry(q),
            );
        }
    }
    for (&n, node) in &s.nodes {
        if !node.is_open() {
            o.check(
                node.assume_marks.is_empty(),
                Inv::E1,
                ViolationDetail::Node(n),
            );
        }
    }
    // (d), over the resident state (D.4.12): an escalation whose user question has left memory is
    // not evaluated (see `left_memory`).
    for (&q, pq) in &s.pquestions {
        if let PqState::Escalated { uq } = pq.state {
            let has = s
                .uquestions
                .values()
                .any(|u| u.source == UqSource::Escalation { question: q });
            o.check(
                has || left_memory(s, uq),
                Inv::E1,
                ViolationDetail::Entry(q),
            );
        }
    }
    // (f)
    for (&q, uq) in &s.uquestions {
        let answered_after_all = answered_after_all(s, uq);
        if uq.state == UqState::Lapsed && uq.assumed_by.is_none() && !answered_after_all {
            o.check(
                assume_marked(s, q, false),
                Inv::E1,
                ViolationDetail::Entry(q),
            );
        }
    }
    // (g) 1.12 (§9.3): a withdrawn question to a person reaches nobody more: no recheck of it is
    // queued, and no node's wait on it is unresolved. One pass over the nodes' waits.
    let mut waited: BTreeSet<QuestionId> = BTreeSet::new();
    for node in s.nodes.values() {
        if let Some(w) = &node.wait
            && let WaitOn::Question { q, .. } = w.on
            && !spec::resolved(s, &w.on, now)
        {
            waited.insert(q);
        }
    }
    for (&q, pq) in &s.pquestions {
        if matches!(pq.state, PqState::Withdrawn { .. }) {
            o.check(
                !s.ix
                    .deadline_at
                    .contains_key(&(DeadlineKind::Recheck, Owner::Entry(q)))
                    && !waited.contains(&q),
                Inv::E1,
                ViolationDetail::Entry(q),
            );
        }
    }
}

/// W1: a waiting node carries neither `unanswered_request` nor `open_intent`, is not a node
/// candidate, and is a status candidate exactly while it carries `open_commitment`.
fn w1(s: &State, now: Time, o: &mut Out) {
    for &n in s.nodes.keys() {
        if !spec::waiting(s, n, now) {
            continue;
        }
        let fl = spec::flags(s, n, now);
        let status = s.ix.next.of.contains_key(&crate::state::Cand::Status(n));
        let ok = !fl.contains(Flag::UnansweredRequest)
            && !fl.contains(Flag::OpenIntent)
            && !s.ix.next.of.contains_key(&crate::state::Cand::Node(n))
            && status == fl.contains(Flag::OpenCommitment);
        o.check(ok, Inv::W1, ViolationDetail::Node(n));
    }
}

/// R2, state half: no node closed `missed` has a checkpoint.
fn r2(s: &State, o: &mut Out) {
    for (&n, node) in &s.nodes {
        if let Life::Closed(c) = &node.life
            && c.outcome == crate::types::Outcome::Missed
        {
            o.check(node.checkpoints == 0, Inv::R2, ViolationDetail::Node(n));
        }
    }
}

/// D1, state half, per principal over the resident entries (CS-5): every entry's principal is
/// interned, every pending one is in `pending_digest_by[to]`, and every shown one names a resident
/// `to_user` when its entry is resident.
fn d1(s: &State, o: &mut Out) {
    for (&d, e) in &s.digest {
        o.check(
            s.ix.principal_key.contains_key(&e.to) || s.principal_ids.values().any(|&p| p == e.to),
            Inv::D1,
            ViolationDetail::Entry(d),
        );
        match e.state {
            DigestState::Pending => o.check(
                s.ix.pending_digest_by
                    .get(&e.to)
                    .is_some_and(|x| x.contains(&d)),
                Inv::D1,
                ViolationDetail::Entry(d),
            ),
            DigestState::Shown { by } => {
                if let Some(m) = s.entries.get(&by) {
                    o.check(
                        m.ty == EntryType::ToUser,
                        Inv::D1,
                        ViolationDetail::Entry(d),
                    );
                }
            }
        }
    }
}

/// Whether `(kind, owner)` has a queued key after `now`.
fn queued(s: &State, kind: DeadlineKind, owner: &Owner, now: Time) -> bool {
    s.ix.deadline_at
        .get(&(kind, owner.clone()))
        .is_some_and(|&t| t > now)
}

/// M1a (D.9.3): every waiting object has its server-guaranteed mover: a queued deadline, or a flag,
/// a live hold, or an unresolved wait of the node it belongs to or marks.
#[allow(
    clippy::too_many_lines,
    reason = "one row per waiting object of the table of D.9.3"
)]
fn m1(s: &State, now: Time, o: &mut Out) {
    let fl = |n: NodeId| spec::flags(s, n, now);
    let uq_mover = |u: crate::types::Eid| match s.uquestions.get(&u) {
        Some(uq) => match uq.state {
            UqState::Pending => queued(s, DeadlineKind::UserQuestion, &Owner::Entry(u), now),
            UqState::Lapsed => uq.assumed_by.is_some() || assume_marked(s, u, true),
            UqState::Answered { .. } | UqState::Defaulted => true,
        },
        // A finished user question needs no mover (D.4.12, `left_memory`).
        None => left_memory(s, u),
    };
    for (&n, node) in &s.nodes {
        if !node.is_open() {
            continue;
        }
        // Waits.
        if let Some(w) = &node.wait
            && !spec::resolved(s, &w.on, now)
        {
            let ok = match w.on {
                WaitOn::Time(_) | WaitOn::External(_) => {
                    queued(s, DeadlineKind::Wait, &Owner::Node(n), now)
                }
                WaitOn::Question { q, .. } => {
                    s.pquestions
                        .get(&q)
                        .is_some_and(|x| x.state == PqState::Open)
                        && queued(s, DeadlineKind::Recheck, &Owner::Entry(q), now)
                }
                WaitOn::UserQuestion(q) => {
                    s.uquestions
                        .get(&q)
                        .is_some_and(|x| x.state == UqState::Pending)
                        && queued(s, DeadlineKind::UserQuestion, &Owner::Entry(q), now)
                }
                WaitOn::Node(t) => s.nodes.get(&t).is_some_and(crate::state::Node::is_open),
            };
            o.check(ok, Inv::M1, ViolationDetail::Node(n));
        }
        // Open occurrences.
        if let Role::Occurrence(_) = node.role {
            let f = fl(n);
            let ok = queued(s, DeadlineKind::OccurrenceSlot, &Owner::Node(n), now)
                || queued(s, DeadlineKind::OccurrenceWindow, &Owner::Node(n), now)
                || spec::live_holds(s, n, now) > 0
                || f.contains(Flag::Overdue)
                || f.contains(Flag::OpenIntent);
            o.check(ok, Inv::M1, ViolationDetail::Node(n));
        }
    }
    // Open intents.
    for (&n, node) in &s.nodes {
        if node.open_intent.is_some() {
            let ok = fl(n).contains(Flag::OpenIntent)
                || spec::live_holds(s, n, now) > 0
                || spec::waiting(s, n, now);
            o.check(ok, Inv::M1, ViolationDetail::Node(n));
        }
    }
    // People questions.
    for (&q, pq) in &s.pquestions {
        let ok = match pq.state {
            PqState::Open => queued(s, DeadlineKind::Recheck, &Owner::Entry(q), now),
            PqState::Expired => pq.assumed_by.is_some() || assume_marked(s, q, true),
            PqState::Escalated { uq } => uq_mover(uq),
            PqState::Answered { .. } | PqState::Superseded { .. } | PqState::Withdrawn { .. } => {
                true
            }
        };
        o.check(ok, Inv::M1, ViolationDetail::Entry(q));
    }
    // Commitments.
    for (&c, x) in &s.commitments {
        if x.state != CommitmentState::Open {
            continue;
        }
        let ok = queued(s, DeadlineKind::Commitment, &Owner::Entry(c), now)
            || fl(x.node).contains(Flag::OpenCommitment)
            || spec::attended(s, x.node, now)
            || s.pquestions
                .values()
                .any(|q| q.node == x.node && q.conv == x.conv && q.state == PqState::Open);
        o.check(ok, Inv::M1, ViolationDetail::Entry(c));
    }
    // `said` entries.
    for (&a, x) in &s.saids {
        let ok = match x.state {
            SaidState::Pending => queued(s, DeadlineKind::Silence, &Owner::Session(x.session), now),
            SaidState::Unconfirmed => {
                fl(x.node).contains(Flag::UnconfirmedSaid) || spec::attended(s, x.node, now)
            }
            // 1.9 (§9.2): nobody is to send a withheld `said`, so it needs no mover.
            SaidState::Confirmed | SaidState::Withheld => true,
        };
        o.check(ok, Inv::M1, ViolationDetail::Entry(a));
    }
    // Questions for the user.
    for (&q, uq) in &s.uquestions {
        let answered_after_all = answered_after_all(s, uq);
        let ok = match uq.state {
            UqState::Pending => queued(s, DeadlineKind::UserQuestion, &Owner::Entry(q), now),
            UqState::Lapsed if uq.assumed_by.is_none() && !answered_after_all => {
                assume_marked(s, q, true)
            }
            UqState::Lapsed | UqState::Answered { .. } | UqState::Defaulted => true,
        };
        o.check(ok, Inv::M1, ViolationDetail::Entry(q));
    }
    // Refusals and pauses.
    for (n, c) in s.refusals.keys() {
        o.check(
            queued(
                s,
                DeadlineKind::RefusalExpiry,
                &Owner::Refusal(*n, c.clone()),
                now,
            ),
            Inv::M1,
            ViolationDetail::Node(*n),
        );
    }
    for &p in s.pauses.keys() {
        o.check(
            queued(s, DeadlineKind::Pause, &Owner::Node(p), now),
            Inv::M1,
            ViolationDetail::Node(p),
        );
    }
}

/// U1, state half, over the resident user questions: the chain is non-empty, without duplicates,
/// at most `UQ_CHAIN_MAX` long, and ends with `user`; the stage lies within it; a pending question
/// is in `pending_uq_by` of its current addressee and of no other principal.
fn u1(s: &State, o: &mut Out) {
    let mut listed: BTreeMap<crate::types::Eid, Vec<PrincipalId>> = BTreeMap::new();
    for (&p, ranks) in &s.ix.pending_uq_by {
        for r in ranks {
            listed.entry(r.q).or_default().push(p);
        }
    }
    for (&q, uq) in &s.uquestions {
        let distinct: BTreeSet<PrincipalId> = uq.to.iter().copied().collect();
        let ok = !uq.to.is_empty()
            && distinct.len() == uq.to.len()
            && uq.to.len() <= UQ_CHAIN_MAX
            && uq.to.last() == Some(&USER_PRINCIPAL)
            && usize::from(uq.stage) < uq.to.len();
        o.check(ok, Inv::U1, ViolationDetail::Entry(q));
        if uq.state == UqState::Pending {
            let at = uq.to.get(usize::from(uq.stage)).copied();
            let ps = listed.get(&q).cloned().unwrap_or_default();
            o.check(
                at.is_some() && ps.len() == 1 && ps.first().copied() == at,
                Inv::U1,
                ViolationDetail::Entry(q),
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Time and limits

/// T1, state half: every entry's time is at most `last_now`.
fn t1(s: &State, o: &mut Out) {
    for (&e, m) in &s.entries {
        o.check(m.at <= s.last_now, Inv::T1, ViolationDetail::Entry(e));
    }
}

/// T3: the state is settled at `now`, and every marker equals its predicate (D.4.8, D.9.2).
#[allow(clippy::too_many_lines, reason = "one clause per marker of D.9.2's T3")]
fn t3(s: &State, now: Time, o: &mut Out) {
    o.check(
        now >= s.last_now,
        Inv::T3,
        ViolationDetail::Text("now before last_now"),
    );
    if let Some(k) = s.ix.deadlines.first()
        && k.at <= now
    {
        o.push(Inv::T3, ViolationDetail::Deadline(k.clone()));
    }
    let mut holding: BTreeSet<crate::types::SessionId> = BTreeSet::new();
    for (t, h) in &s.holds {
        o.check(h.lease_until > now, Inv::T3, ViolationDetail::Hold(*t));
        holding.insert(h.session);
    }
    for (&n, node) in &s.nodes {
        let is_open = node.is_open();
        if is_open
            && let Some(w) = &node.wait
            && let WaitOn::Time(t) | WaitOn::External(t) = w.on
        {
            o.check(w.fired == (t <= now), Inv::T3, ViolationDetail::Node(n));
        }
        if is_open && let Some(d) = node.due {
            o.check(
                node.due_fired == (d <= now),
                Inv::T3,
                ViolationDetail::Node(n),
            );
        }
        // 1.8: the Stale marker of an open node that is not reserved.
        if is_open
            && node.origin != crate::state::Origin::Reserved
            && let Some(t) = s.params.stale_at(node.touched_at)
        {
            o.check(
                node.stale_fired == (t <= now),
                Inv::T3,
                ViolationDetail::Node(n),
            );
        }
        if let (true, Role::Occurrence(x)) = (is_open, &node.role) {
            o.check(
                (x.phase == OccPhase::Available) == (x.slot <= now)
                    && x.window_fired == (x.window_end <= now),
                Inv::T3,
                ViolationDetail::Node(n),
            );
        }
    }
    for (&c, x) in &s.commitments {
        if x.state == CommitmentState::Open {
            o.check(
                x.followup_fired == (x.followup_at <= now),
                Inv::T3,
                ViolationDetail::Entry(c),
            );
        }
    }
    for a in s.classes.values() {
        o.check(
            a.caps_seen.values().all(|&t| t > now),
            Inv::T3,
            ViolationDetail::Text("class profile ended"),
        );
    }
    for (&x, session) in &s.sessions {
        let silent = session.silence_at <= now && !holding.contains(&x);
        o.check(
            session.silent == silent,
            Inv::T3,
            ViolationDetail::Session(x),
        );
        o.check(
            session.retire_at > now,
            Inv::T3,
            ViolationDetail::Session(x),
        );
    }
    for (&p, x) in &s.pauses {
        o.check(x.until > now, Inv::T3, ViolationDetail::Node(p));
    }
    for ((n, _), r) in &s.refusals {
        o.check(
            r.by.values().all(|x| x.expires_at > now),
            Inv::T3,
            ViolationDetail::Node(*n),
        );
    }
    for (&x, note) in &s.notes {
        o.check(note.until > s.last_now, Inv::T3, ViolationDetail::Entry(x));
    }
    for &t in s.leaving.values() {
        o.check(
            t > s.last_now,
            Inv::T3,
            ViolationDetail::Text("leaving time"),
        );
    }
    let want = spec::leaving_expected(s);
    let have: BTreeSet<EntityRef> = s.leaving.keys().copied().collect();
    o.check(want == have, Inv::T3, ViolationDetail::Index("leaving"));
}

/// T4: the queue equals its recount, `deadline_at` agrees with it, and no key lies at or before
/// `now`.
fn t4(s: &State, now: Time, o: &mut Out) {
    let rc = recount_deadlines(s);
    o.check(
        s.ix.deadlines == rc,
        Inv::T4,
        ViolationDetail::Index("deadlines"),
    );
    o.check(
        s.ix.deadline_at == deadline_at_of(&s.ix.deadlines),
        Inv::T4,
        ViolationDetail::Index("deadline_at"),
    );
    if let Some(k) = s.ix.deadlines.first()
        && k.at <= now
    {
        o.push(Inv::T4, ViolationDetail::Deadline(k.clone()));
    }
}

/// T5: the graph of `{node}` waits (resolved or not) and `depends_on` edges is acyclic, by an
/// iterative depth-first search with three colours.
fn t5(s: &State, o: &mut Out) {
    let edges = |n: NodeId| -> Vec<NodeId> {
        let Some(x) = s.nodes.get(&n) else {
            return Vec::new();
        };
        let mut out: Vec<NodeId> = x.depends_on.iter().copied().collect();
        if let Some(w) = &x.wait
            && let WaitOn::Node(t) = w.on
        {
            out.push(t);
        }
        out
    };
    // 0 white, 1 grey, 2 black.
    let mut colour: BTreeMap<NodeId, u8> = BTreeMap::new();
    for &start in s.nodes.keys() {
        if colour.get(&start).copied().unwrap_or(0) != 0 {
            continue;
        }
        let mut stack: Vec<(NodeId, Vec<NodeId>)> = alloc::vec![(start, edges(start))];
        colour.insert(start, 1);
        while let Some((n, rest)) = stack.last_mut() {
            match rest.pop() {
                Some(m) => match colour.get(&m).copied().unwrap_or(0) {
                    0 => {
                        colour.insert(m, 1);
                        let e = edges(m);
                        stack.push((m, e));
                    }
                    1 => {
                        o.push(Inv::T5, ViolationDetail::Node(m));
                        return;
                    }
                    _ => {}
                },
                None => {
                    colour.insert(*n, 2);
                    stack.pop();
                }
            }
        }
    }
}

/// L1, state half: every node's depth is at most `tree.max_depth`.
fn l1(s: &State, o: &mut Out) {
    for &n in s.nodes.keys() {
        let depth = u32::try_from(spec::ancestors(s, n).len()).unwrap_or(u32::MAX);
        o.check(
            depth <= s.params.tree_max_depth,
            Inv::L1,
            ViolationDetail::Node(n),
        );
    }
}

/// P1: every stored pause has `now < until <= last_renewed + ttl`.
fn p1(s: &State, now: Time, o: &mut Out) {
    for (&p, x) in &s.pauses {
        o.check(
            now < x.until && x.until <= x.last_renewed.plus(x.ttl),
            Inv::P1,
            ViolationDetail::Node(p),
        );
    }
}
