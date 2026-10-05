//! The full recount of every index from the base and history parts of the state (design D.4.10).
//!
//! R1 compares `ix` with `recount(s, now)`, F compares the flags with `spec::flags`, and T4 the queue
//! with `recount_deadlines`; the shell recounts the indices at startup from the state tables
//! (D.11.8). The recount shares no maintenance code with `evolve`: each index is computed here by a
//! plain pass over what it indexes. It shares only the definitions of derived values (the flag
//! predicates, the classification, a candidate's key), which the checks compare with `spec`.
//!
//! Every index ranges over the resident state (D.4.12). `ix.next`'s `since` values come from
//! `State::since`, which is history; a candidate with none gets the `since` it would get on
//! appearing at `now` (`first_since`: `now`, or an occurrence's slot), and R1 reports the
//! difference of the key sets.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::deadlines::{deadline_at_of, recount_deadlines};
use crate::flags::{basis_targets, flags_in, stale_decision};
use crate::index::{
    cand_at_with, classify, first_since, held_of, is_agent_made, own_sub, people_entry, sub_apply,
    trie_add, uq_rank, waits_for_work,
};
use crate::state::{
    Cand, CommitmentState, DecisionState, DigestState, Ix, NodeItems, NodeIx, ObjectionState,
    PqState, Role, Routing, SaidState, State, UqSource, UqState, WaitKey, WaitOn,
};
use crate::types::{Addressee, KeyStr, NodeId, PEOPLE, Requester, Time};

/// Full recount of every index from the base state at `now` (D.4.10). R1, F, and T4 compare with it.
#[must_use]
pub fn recount(s: &State, now: Time) -> Ix {
    let mut ix = Ix::default();
    tree(s, &mut ix);
    holds(s, &mut ix);
    node_own(s, &mut ix);
    entities(s, &mut ix);
    sessions(s, &mut ix);
    flags_and_next(s, &mut ix, now);
    ix.deadlines = recount_deadlines(s);
    ix.deadline_at = deadline_at_of(&ix.deadlines);
    ix
}

/// The ancestors of `n` over `nodes`, nearest first, at most `nodes.len()`.
fn ancestors(s: &State, n: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = s.nodes.get(&n).and_then(crate::state::Node::parent);
    while let Some(p) = cur {
        if out.len() >= s.nodes.len() {
            break;
        }
        out.push(p);
        cur = s.nodes.get(&p).and_then(crate::state::Node::parent);
    }
    out
}

/// `ix.node` (depth and the child counts) and `ix.children`.
fn tree(s: &State, ix: &mut Ix) {
    for &id in s.nodes.keys() {
        let depth = u32::try_from(ancestors(s, id).len()).unwrap_or(u32::MAX);
        ix.node.insert(
            id,
            NodeIx {
                depth,
                ..NodeIx::default()
            },
        );
    }
    for (&id, node) in &s.nodes {
        let Some(p) = node.parent() else { continue };
        ix.children.entry(p).or_default().insert(id);
        if let Some(px) = ix.node.get_mut(&p) {
            px.children = px.children.saturating_add(1);
            if node.is_open() {
                px.open_children = px.open_children.saturating_add(1);
                if is_agent_made(node.origin) {
                    px.open_agent_children = px.open_agent_children.saturating_add(1);
                }
            }
        }
    }
}

/// `live`, `subtree_live`, `held_children`, `attended_children`, and `holds_by_session`.
fn holds(s: &State, ix: &mut Ix) {
    for (t, h) in &s.holds {
        ix.holds_by_session.entry(h.session).or_default().insert(*t);
        if let Some(x) = ix.node.get_mut(&t.node) {
            x.live = x.live.saturating_add(1);
        }
        let mut chain = alloc::vec![t.node];
        chain.extend(ancestors(s, t.node));
        for a in chain {
            if let Some(x) = ix.node.get_mut(&a) {
                x.subtree_live = x.subtree_live.saturating_add(1);
            }
        }
    }
    for (&id, node) in &s.nodes {
        let Some(p) = node.parent() else { continue };
        let x = ix.node.get(&id).copied().unwrap_or_default();
        if x.live > 0
            && let Some(px) = ix.node.get_mut(&p)
        {
            px.held_children = px.held_children.saturating_add(1);
        }
        if x.subtree_live > 0 {
            ix.attended_children.entry(p).or_default().insert(id);
        }
    }
}

/// What each node derives from its own fields: keys, links, dependencies, waits, marks, open
/// occurrences, and the directory.
fn node_own(s: &State, ix: &mut Ix) {
    for (&id, node) in &s.nodes {
        for k in node.keys.keys() {
            ix.keys_all.entry(k.clone()).or_default().insert(id);
            if node.is_open() {
                ix.keys_open.insert(k.clone(), id);
            }
        }
        for &(rel, to) in &node.links {
            ix.backlinks.entry(to).or_default().insert((rel, id));
        }
        for &d in &node.depends_on {
            ix.dependents.entry(d).or_default().insert(id);
        }
        if let Some(w) = &node.wait {
            match w.on {
                WaitOn::Node(t) => {
                    ix.waiters.entry(WaitKey::Node(t)).or_default().insert(id);
                }
                WaitOn::Question { q, .. } | WaitOn::UserQuestion(q) => {
                    ix.waiters
                        .entry(WaitKey::Question(q))
                        .or_default()
                        .insert(id);
                }
                WaitOn::Time(_) | WaitOn::External(_) => {}
            }
        }
        for q in node.recheck_marks.iter().chain(node.assume_marks.iter()) {
            ix.marked_by_question.entry(*q).or_default().insert(id);
        }
        if let Role::Occurrence(o) = &node.role
            && node.is_open()
        {
            ix.series_open.entry(o.series).or_default().insert(id);
        }
        if !node.agreements.is_empty() {
            ix.agreements.insert(id);
        }
        if node.is_open() && node.parent() == Some(PEOPLE) {
            for k in node.keys.keys() {
                if let Some(pk) = KeyStr::person(k)
                    && let Some(&p) = s.principal_ids.get(&pk)
                {
                    ix.person_of.insert(p, id);
                    ix.aliases_of.entry(id).or_default().insert(p);
                }
            }
        }
    }
    for (k, &p) in &s.principal_ids {
        ix.principal_key.insert(p, k.clone());
    }
}

fn items(ix: &mut Ix, n: NodeId) -> &mut NodeItems {
    ix.items.entry(n).or_default()
}

/// The entities: `items` with every open subset, and every index over requests, questions,
/// commitments, `said` entries, decisions, objections, user entries, digest entries, notes,
/// entries, refusals, classes, profiles, op records, and leaving times.
#[allow(
    clippy::too_many_lines,
    reason = "one plain pass per entity map, in the order of D.4.10"
)]
fn entities(s: &State, ix: &mut Ix) {
    for (&r, req) in &s.requests {
        ix.origins.insert(req.origin.key.clone(), r);
        let it = items(ix, req.node);
        it.requests.insert(r);
        if req.settled.is_none() {
            it.unsettled.insert(r);
            if req.answered_by.is_none() {
                it.unanswered.entry(req.conv.clone()).or_default().insert(r);
            }
            ix.unsettled_requests.insert(r);
        }
        if let Requester::User(p) = req.requester {
            ix.requests_by.entry(p).or_default().insert(r);
            if req.settled.is_none() {
                ix.unsettled_by.entry(p).or_default().insert(r);
                let it = items(ix, req.node);
                let older = it.requesters_by_age.iter().any(|&(x, q)| q == p && x < r);
                if !older {
                    it.requesters_by_age.retain(|&(_, q)| q != p);
                    it.requesters_by_age.insert((r, p));
                }
            }
        }
    }
    for (&d, dec) in &s.decisions {
        let it = items(ix, dec.node);
        it.decisions.insert(d);
        if !matches!(dec.state, DecisionState::Retracted { .. }) {
            for &r in &dec.settles {
                ix.settling.entry(r).or_default().insert(d);
            }
        }
        if dec.state == DecisionState::Active {
            items(ix, dec.node).active_decisions.insert(d);
            for t in basis_targets(s, dec) {
                ix.cited_by.entry(t).or_default().insert(d);
            }
            if stale_decision(s, dec) {
                items(ix, dec.node).stale_basis.insert(d);
            }
        }
    }
    // The owed sets read `settling`, so they follow the decisions.
    for (&r, req) in &s.requests {
        if req.settled.is_some() {
            continue;
        }
        let it = items(ix, req.node);
        if req.answered_by.is_some() {
            it.owed_decision.insert(r);
        }
    }
    for (&r, req) in &s.requests {
        if req.settled.is_none()
            && req.answered_by.is_none()
            && ix.settling.get(&r).is_some_and(|x| !x.is_empty())
        {
            items(ix, req.node).owed_reply.insert(r);
        }
    }
    for (&q, pq) in &s.pquestions {
        let it = items(ix, pq.node);
        it.pquestions.insert(q);
        if pq.state == PqState::Open {
            it.open_pq.entry(pq.conv.clone()).or_default().insert(q);
        }
    }
    for (&q, aq) in &s.aquestions {
        items(ix, aq.node).aquestions.insert(q);
    }
    for (&q, uq) in &s.uquestions {
        if let Some(n) = uq.about {
            items(ix, n).uquestions.insert(q);
        }
        if uq.state == UqState::Pending {
            if let Some(&p) = uq.to.get(usize::from(uq.stage)) {
                ix.pending_uq_by
                    .entry(p)
                    .or_default()
                    .insert(uq_rank(s, q, uq.about, uq.due));
            }
            if let UqSource::Refusal { node } = uq.source {
                ix.refusal_question.insert(node, q);
            }
        }
    }
    for (&c, x) in &s.commitments {
        let it = items(ix, x.node);
        it.commitments.insert(c);
        if x.state == CommitmentState::Open {
            it.open_commitments.insert(c);
        }
    }
    for (&x, sd) in &s.saids {
        let it = items(ix, sd.node);
        it.saids.insert(x);
        let open = matches!(sd.state, SaidState::Pending | SaidState::Unconfirmed);
        if open {
            it.open_saids.insert(x);
        }
        if sd.state == SaidState::Unconfirmed {
            it.unconfirmed.insert(x);
        }
        if open && s.sessions.contains_key(&sd.session) {
            ix.open_saids_by.entry(sd.session).or_default().insert(x);
        }
    }
    for (&o, x) in &s.objections {
        let it = items(ix, x.node);
        it.objections.insert(o);
        if x.state == ObjectionState::Open {
            it.open_objections.insert(o);
        }
    }
    for (&e, u) in &s.user_entries {
        match u.routing {
            Routing::Unrouted => {
                ix.unrouted.insert(e);
                if s.sessions.contains_key(&u.session) {
                    ix.unrouted_by.entry(u.session).or_default().insert(e);
                }
            }
            Routing::Clarifies { request } => {
                ix.clarifications.entry(request).or_default().insert(e);
            }
            Routing::Request { .. } | Routing::Answers { .. } | Routing::NoAction => {}
        }
    }
    for (&d, x) in &s.digest {
        if x.state == DigestState::Pending {
            ix.pending_digest_by.entry(x.to).or_default().insert(d);
        }
    }
    for (&x, note) in &s.notes {
        ix.notes_until.insert((note.until, x));
        for r in &note.recipients {
            if s.sessions.contains_key(r) && !note.read_by.contains(r) {
                ix.inbox.entry(*r).or_default().insert(x);
            }
        }
        match note.to {
            Addressee::Holder(n) if note.pending_holder => {
                ix.pending_holder.entry(n).or_default().insert(x);
            }
            Addressee::Holders(a) => {
                ix.branch_notes.entry(a).or_default().insert(x);
            }
            Addressee::Principal(p) => {
                ix.user_notes.entry(p).or_default().insert((note.until, x));
            }
            Addressee::Holder(_) | Addressee::Session(_) => {}
        }
    }
    for (&e, m) in &s.entries {
        if people_entry(m.ty) {
            ix.people.insert((m.node, m.seq), e);
        }
    }
    for (n, c) in s.refusals.keys() {
        ix.refused.entry(*n).or_default().insert(c.clone());
    }
    for (c, a) in &s.classes {
        for (caps, &t) in &a.caps_seen {
            ix.class_ends
                .entry(c.clone())
                .or_default()
                .insert((t, caps.clone()));
        }
    }
    for (key, p) in &s.stats.profiles {
        ix.profiles_by_time.insert((p.last_seen, key.clone()));
    }
    for ((c, op), r) in &s.ops {
        ix.ops_expiry.insert((r.expires_at, c.clone(), op.clone()));
    }
    for (x, &t) in &s.leaving {
        ix.leaving_by_time.insert((t, *x));
    }
    ix.items.retain(|_, it| *it != NodeItems::default());
}

/// `live_sessions` and `live_by_class`: the sessions that are not silent.
fn sessions(s: &State, ix: &mut Ix) {
    for (&x, session) in &s.sessions {
        if !session.silent {
            ix.live_sessions.insert(x);
            let c = ix.live_by_class.entry(session.class.clone()).or_insert(0);
            *c = c.saturating_add(1);
        }
    }
}

/// The flags of every node at `now`, the flagged sets and counts, the subtree rollups, the candidate
/// index with its partitions and needs trie, `flagged_active`, and the `unheld_flagged` sets.
fn flags_and_next(s: &State, ix: &mut Ix, now: Time) {
    let mut flags = BTreeMap::new();
    for &id in s.nodes.keys() {
        flags.insert(id, flags_in(s, ix, id, now));
    }
    for (&id, &f) in &flags {
        if let Some(x) = ix.node.get_mut(&id) {
            x.flags = f;
        }
        if !f.is_empty() {
            ix.flagged.insert(id);
            if let Some(Role::Occurrence(o)) = s.nodes.get(&id).map(|x| &x.role) {
                ix.flagged_occurrences
                    .entry(o.series)
                    .or_default()
                    .insert(id);
            }
        }
    }
    for (&id, node) in &s.nodes {
        let f = flags.get(&id).copied().unwrap_or_default();
        if let Some(p) = node.parent()
            && !f.is_empty()
        {
            ix.flagged_children.entry(p).or_default().insert(id);
            if let Some(px) = ix.node.get_mut(&p) {
                px.flagged_children = px.flagged_children.saturating_add(1);
            }
        }
        let unit = own_sub(node, !f.is_empty());
        for a in ancestors(s, id) {
            if let Some(x) = ix.node.get_mut(&a) {
                x.sub = sub_apply(x.sub, unit, true);
            }
        }
    }
    // The candidates.
    let mut parts: BTreeSet<crate::state::PartKey> = BTreeSet::new();
    for (&id, node) in &s.nodes {
        let f = flags.get(&id).copied().unwrap_or_default();
        let live = ix.node.get(&id).map_or(0, |x| x.live);
        if waits_for_work(s, id, node, f, live, now) {
            ix.next.waiting.insert(id);
        }
        let Some(c) = classify(s, id, node, f, live, now) else {
            continue;
        };
        let since = s
            .since
            .get(&c)
            .copied()
            .unwrap_or_else(|| first_since(s, c, now));
        let Some(at) = cand_at_with(s, f, c, since) else {
            continue;
        };
        ix.next.nodes = ix.next.nodes.saturating_add(1);
        match held_of(s, &ix.series_open, id, node) {
            Some(h) => {
                ix.next.held.insert(c, h);
            }
            None => {
                ix.next
                    .parts
                    .entry(at.part.clone())
                    .or_default()
                    .insert(at.key);
                parts.insert(at.part.clone());
                if !f.is_empty() && live == 0 {
                    if let Some(p) = node.parent() {
                        ix.unheld_flagged_children.entry(p).or_default().insert(id);
                    }
                    ix.unheld_flagged_by_part
                        .entry(at.part.clone())
                        .or_default()
                        .insert(id);
                }
            }
        }
        ix.next.of.insert(c, at);
        if !f.is_empty() {
            ix.flagged_active.insert(id);
        }
    }
    for (&id, &f) in &flags {
        if !f.is_empty() && ix.node.get(&id).is_some_and(|x| x.live > 0) {
            ix.flagged_active.insert(id);
        }
    }
    for (&e, u) in &s.user_entries {
        if u.routing != Routing::Unrouted {
            continue;
        }
        let c = Cand::Route(e);
        let since = s.since.get(&c).copied().unwrap_or(now);
        if let Some(at) = cand_at_with(s, crate::types::FlagSet::EMPTY, c, since) {
            ix.next
                .parts
                .entry(at.part.clone())
                .or_default()
                .insert(at.key);
            parts.insert(at.part.clone());
            ix.next.of.insert(c, at);
        }
    }
    for p in parts {
        trie_add(&mut ix.next.needs_trie, &p.needs, p.est_bucket);
    }
}
