//! The shape of the UserQuestion firing (design D.8.2, D.9.1, CS-2): `check_fire`'s arm for
//! WP-D5a's firing. Like the other step checks, it reads the state before the firing and the
//! events only, scanning the base state where the core reads an index.
//!
//! The digest entries themselves are D1's (their audience, `spec::audience` on `before`), and the
//! chain of a question created in the firing is U1's; this arm checks the order of D.8.2's table
//! around them: with a default, the `Defaulted` entry, `UqDefaulted`, and then the digest entries
//! or the refusal default; without one, exactly `UqReaddressed` to the next stage with its
//! addressee's own due (D.8.12's `due_for`, computed here by scanning), or, past the last stage,
//! `UqLapsed`, the `Lapsed` entries, and the assume marks on the waiting nodes, or on the open
//! `about` node, or else the assumption recorded at once.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::event::{EntryBody, Ev, Event, SystemNote};
use crate::spec;
use crate::state::{Author, DigestItem, Life, State, UqSource, UqState, WaitOn};
use crate::types::{
    Dur, Mark, NodeId, Outcome, PrincipalId, QuestionId, RequestId, Time, USER_DECISIONS,
    USER_DIGEST,
};

/// The state just before event `i` of a step: `before` with the step's earlier events applied.
/// U1 compares a question's chain with `spec::chain` on it (§26: the chain "at creation"), which
/// reads only the base maps, so `evolve`'s incremental maintenance plays no part in the comparison
/// (D.9.1). Only a step that creates a question for the user pays for the copy.
pub(super) fn state_at(before: &State, events: &[Event], i: usize) -> State {
    let mut s = before.clone();
    for e in events.iter().take(i) {
        crate::evolve::evolve_mut(&mut s, e);
    }
    s
}

/// A digest entry for `item`.
fn digest_of(e: &Ev, item: DigestItem) -> bool {
    matches!(e, Ev::Appended {
        node: USER_DIGEST,
        by: Author::Server,
        body: EntryBody::System(SystemNote::Digest { item: i, .. }),
        ..
    } if *i == item)
}

/// The events after the leading ones that match `pred`.
fn split_while<'a, 'b>(evs: &'a [&'b Ev], pred: impl Fn(&Ev) -> bool) -> &'a [&'b Ev] {
    let n = evs.iter().take_while(|e| pred(e)).count();
    evs.get(n..).unwrap_or_default()
}

/// The open nodes whose wait is on user question `q` (a scan of the nodes).
fn waiters(before: &State, q: QuestionId) -> BTreeSet<NodeId> {
    before
        .nodes
        .iter()
        .filter(|(_, x)| {
            matches!(x.life, Life::Open)
                && x.wait
                    .as_ref()
                    .is_some_and(|w| w.on == WaitOn::UserQuestion(q))
        })
        .map(|(&n, _)| n)
        .collect()
}

/// `due_for(p, stage, source, at)` of D.8.12 on `before`, naively: the aliases and the `gone` tag
/// by `spec`'s scans of the directory, every message time of every alias filtered by a scan, and
/// the nearest rank as `⌈0.9 × len⌉`. An absent addressee (gone, or no message of any alias at or
/// after `at − user_gap.window`) gets `due_absent`, unless the question is a refusal question;
/// stage 0 gets `min(max(due, p90), capacity.window)` (1.5, O-1); a later stage gets `due`.
fn due_for(before: &State, p: PrincipalId, stage: usize, source: UqSource, at: Time) -> Time {
    let params = &before.params;
    let from = at.minus(params.user_gap_window);
    let mut msgs: Vec<Time> = spec::aliases(before, p)
        .iter()
        .filter_map(|a| before.user_msgs.get(a))
        .flatten()
        .copied()
        .filter(|t| *t >= from)
        .collect();
    let absent = spec::gone(before, p) || msgs.is_empty();
    if absent && !matches!(source, UqSource::Refusal { .. }) {
        return at.plus(params.user_question_due_absent);
    }
    if stage != 0 {
        return at.plus(params.user_question_due);
    }
    msgs.sort_unstable();
    let mut gaps: Vec<Dur> = msgs
        .iter()
        .zip(msgs.iter().skip(1))
        .map(|(a, b)| b.since(*a))
        .collect();
    gaps.sort_unstable();
    let rank = gaps.len().saturating_mul(9).div_ceil(10);
    let p90 = gaps
        .get(rank.saturating_sub(1))
        .copied()
        .unwrap_or_default();
    at.plus(
        params
            .user_question_due
            .max(p90)
            .min(params.capacity_window),
    )
}

/// The events of the UserQuestion firing of `q` at `at`, after its `Fired`, follow D.8.2's table.
pub(super) fn fire_shape(before: &State, q: QuestionId, at: Time, rest: &[&Ev]) -> bool {
    let Some(u) = before.uquestions.get(&q) else {
        return false;
    };
    if u.state != UqState::Pending || u.due != at {
        return false;
    }
    if u.has_default {
        let head = matches!(rest.first(), Some(Ev::Appended {
            node: USER_DECISIONS,
            by: Author::Server,
            body: EntryBody::System(SystemNote::Defaulted { question }),
            ..
        }) if *question == q)
            && matches!(rest.get(1), Some(Ev::UqDefaulted { q: x }) if *x == q);
        let tail = rest.get(2..).unwrap_or_default();
        return head && defaulted_tail(before, q, u.source, tail);
    }
    let next = usize::from(u.stage).saturating_add(1);
    if let Some(&p) = u.to.get(next) {
        // The next stage, due at its addressee's own `due_for` (D.8.2's table).
        let want = due_for(before, p, next, u.source, at);
        return rest.len() == 1
            && matches!(rest.first(), Some(Ev::UqReaddressed { q: x, stage, due })
                if *x == q && usize::from(*stage) == next && *due == want);
    }
    if !matches!(rest.first(), Some(Ev::UqLapsed { q: x }) if *x == q) {
        return false;
    }
    let after = split_while(rest.get(1..).unwrap_or_default(), |e| {
        digest_of(e, DigestItem::Lapsed { question: q })
    });
    let mut marked = waiters(before, q);
    if marked.is_empty()
        && let Some(n) = u.about
        && !n.is_reserved()
        && before
            .nodes
            .get(&n)
            .is_some_and(crate::state::Node::is_open)
    {
        marked.insert(n);
    }
    if marked.is_empty() {
        // `assume_now`: a server decision recording the assumption on `about`, if it is a node
        // that is not reserved, else on `@user/decisions`, and `UqAssumed`.
        let home = u
            .about
            .filter(|a| !a.is_reserved() && before.nodes.contains_key(a))
            .unwrap_or(USER_DECISIONS);
        return after.len() == 2
            && matches!(after.first(), Some(Ev::Appended {
                node,
                by: Author::Server,
                body: EntryBody::Decision { settles, assumes, .. },
                ..
            }) if *node == home && settles.is_empty() && assumes.len() == 1 && assumes.contains(&q))
            && matches!(after.get(1), Some(Ev::UqAssumed { q: x, .. }) if *x == q);
    }
    let marks: Vec<NodeId> = after
        .iter()
        .filter_map(|e| {
            if let Ev::MarkSet {
                node,
                mark: Mark::Assume,
                question,
                on: true,
            } = e
                && *question == q
            {
                Some(*node)
            } else {
                None
            }
        })
        .collect();
    marks.len() == after.len() && marks == marked.into_iter().collect::<Vec<_>>()
}

/// After `UqDefaulted`: for a refusal question, `RefusalsCleared`, then the drop of an open, unheld
/// node (a server decision on it listing its unsettled requests, its close as `dropped`, and the
/// `RefusalDrop` entries last) or the `DefaultApplied` entries; for any other source, the
/// `DefaultApplied` entries only.
fn defaulted_tail(before: &State, q: QuestionId, source: UqSource, tail: &[&Ev]) -> bool {
    let applied = |evs: &[&Ev]| {
        evs.iter()
            .all(|e| digest_of(e, DigestItem::DefaultApplied { question: q }))
    };
    let UqSource::Refusal { node: n } = source else {
        return applied(tail);
    };
    if !matches!(tail.first(), Some(Ev::RefusalsCleared { node, by }) if *node == n && *by == q) {
        return false;
    }
    let tail = tail.get(1..).unwrap_or_default();
    let open = before
        .nodes
        .get(&n)
        .is_some_and(crate::state::Node::is_open);
    let unheld = !before.holds.keys().any(|t| t.node == n);
    if !(open && unheld) {
        return applied(tail);
    }
    let unsettled: BTreeSet<RequestId> = before
        .requests
        .iter()
        .filter(|(_, r)| r.node == n && r.settled.is_none())
        .map(|(&r, _)| r)
        .collect();
    let decision = matches!(tail.first(), Some(Ev::Appended {
        node,
        by: Author::Server,
        body: EntryBody::Decision { settles, .. },
        ..
    }) if *node == n && *settles == unsettled);
    let closed = tail
        .iter()
        .any(|e| matches!(e, Ev::NodeClosed { node, outcome: Outcome::Dropped, .. } if *node == n));
    let drop = DigestItem::RefusalDrop {
        node: n,
        question: q,
    };
    let entries = tail.iter().rev().take_while(|e| digest_of(e, drop)).count();
    let last_close = tail
        .iter()
        .rposition(|e| matches!(e, Ev::NodeClosed { node, .. } if *node == n));
    decision
        && closed
        && entries >= 1
        && last_close.is_some_and(|i| i.saturating_add(entries) < tail.len())
}

// ------------------------------------------------------------------------------------------------
// N1's step clause (1.11)

/// N1 [step] (1.11, §8.2): a `from_user` with none of `to`, `answers`, and `clarifies`, logged by a
/// session whose only live hold before the call is on a node with an unsettled request of the
/// entry's person, under any alias, is routed as a clarification of the newest such request, read
/// among the node's newest `query::SCAN_MAX` unsettled requests (§18); with none, it is not routed
/// as a clarification. Read naively from `before`'s base maps: the session's live holds, the
/// node's requests, and `spec::same_person`. The entry's person is that of a principal `before`
/// already knew; a principal the call interned is nobody's requester yet.
pub(super) fn n1(
    before: &State,
    cmd: &crate::cmd::Cmd,
    out: &Result<crate::reply::Done, crate::error::Error>,
    after: &State,
    now: Time,
) -> bool {
    use crate::cmd::{Cmd, SessionCmd, TalkCmd};
    use crate::reply::Reply;
    use crate::state::Routing;
    use crate::types::Requester;
    let untargeted = match cmd {
        Cmd::Talk(TalkCmd::FromUser {
            to: None,
            answers: None,
            clarifies: None,
            ..
        }) => true,
        Cmd::Session(SessionCmd::Hello {
            from_user: Some(f), ..
        }) => f.to.is_none() && f.answers.is_none() && f.clarifies.is_none(),
        Cmd::Session(_)
        | Cmd::View(_)
        | Cmd::Node(_)
        | Cmd::Hold(_)
        | Cmd::Talk(_)
        | Cmd::Control(_)
        | Cmd::Admin(_) => false,
    };
    let entry = match out.as_ref().ok().map(|d| &d.reply) {
        Some(Reply::FromUser { entry, .. }) => Some(*entry),
        Some(Reply::Hello {
            user_entry: Some(u),
            ..
        }) => Some(u.entry),
        _ => None,
    };
    let (true, Some(e)) = (untargeted, entry) else {
        return true;
    };
    let Some(u) = after.user_entries.get(&e) else {
        return true;
    };
    let holds: Vec<NodeId> = before
        .holds
        .iter()
        .filter(|(_, h)| h.session == u.session && now < h.lease_until)
        .map(|(t, _)| t.node)
        .collect();
    let [n] = holds.as_slice() else {
        return true;
    };
    let known = before.principal_ids.values().any(|p| *p == u.principal);
    let max = usize::try_from(crate::query::SCAN_MAX).unwrap_or(usize::MAX);
    let mut unsettled: Vec<RequestId> = before
        .requests
        .iter()
        .filter(|(_, r)| r.node == *n && r.settled.is_none())
        .map(|(&r, _)| r)
        .collect();
    unsettled.sort_unstable();
    let newest = unsettled.iter().rev().take(max).copied().find(|r| {
        known
            && matches!(
                before.requests.get(r).map(|q| q.requester),
                Some(Requester::User(q)) if spec::same_person(before, q, u.principal)
            )
    });
    match newest {
        Some(r) => u.routing == Routing::Clarifies { request: r },
        None => !matches!(u.routing, Routing::Clarifies { .. }),
    }
}
