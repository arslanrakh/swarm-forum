//! Questions for the user, the digest, and E1 (design D.8.12, D.8.13, D.8.14, CS-2, CS-5, CS-7).
//! Bodies: WP-D5a.
//!
//! One lifecycle serves one user, many, and none (CS-2): a question for the user is addressed to
//! one person at a time along a chain fixed when it is created (`chain`), each stage with its own
//! due (`due_for`); at a due, a default applies, or the question moves to the next stage, or past
//! the last stage it lapses (`fire_question`, `lapse`). What the swarm decided without the humans
//! concerned goes to the digest, one entry per audience principal (`audience`, `add_digest`). E1
//! asks the user when every active class has refused a node (`e1_check`), and its default drops the
//! node (`refusal_default`).
//!
//! Every function reads the state through the indices of D.4.10 (`items(m).requesters_by_age`,
//! `person_of`, `aliases_of`, `waiters`, `refusal_question`, `pending_uq_by`); the checks compare
//! the chain and the audience with `spec`'s scans (U1, D1).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use super::Exec;
use super::close::{Closing, assume_marks_off, assume_now, close_node};
use super::conversation::try_settle;
use super::principals::{aliases, gone, intern, same_person};
use crate::event::{EndCause, EntryBody, EntryText, Ev, QuestionBody, SystemNote, TextField};
use crate::index::ancestors;
use crate::state::{Author, DigestItem, State, UqSource, UqState, WaitKey};
use crate::types::{
    Dur, GONE, Mark, NodeId, Outcome, PEOPLE, PrincipalId, PrincipalKey, QuestionId, ROOT,
    RequestId, Requester, TEAMS, Text, Time, USER_DECISIONS, USER_DIGEST, USER_PRINCIPAL, When,
};
use crate::user::{UQ_OWNERS_MAX, UQ_REQUESTERS_MAX};

/// The options of E1's question (§10.3): the first is the recommendation and the default.
pub const REFUSAL_OPTIONS: [&str; 2] =
    ["drop the requests and tell the requesters", "keep it open"];

/// The text of E1's question (§10.3); the rendering names the requesters (D.8.12).
pub const REFUSAL_QUESTION: &str = "Every class active for this node has refused it: drop its \
                                    requests and tell the requesters, or keep it open?";

/// The summary of a refusal drop's close and the text of its decision (D.8.14).
pub const REFUSAL_DROPPED: &str = "dropped after every active class refused";

/// The arguments of D.8.12's `create_uq(source, about, has_default, explicit_to, explicit_due)`,
/// and the text of its entry (the question, its options, recommendation, and default).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct NewUq {
    /// Why it is asked.
    pub source: UqSource,
    /// The node it is about.
    pub about: Option<NodeId>,
    /// The default text is in the entry.
    pub has_default: bool,
    /// `principal:<key>` of `talk ask`: the chain's first stage, interned by the command.
    pub explicit_to: Option<PrincipalKey>,
    /// An explicit due, clamped to `>= now` (A-68).
    pub explicit_due: Option<When>,
    /// The entry's text.
    pub text: EntryText,
}

/// `create_uq` (D.8.12): `Appended(Question(User { source, about, to: chain(..), due, has_default
/// }))` on `USER_DECISIONS`, with stage 0 due at the explicit due, clamped, or `due_for(to[0], 0,
/// source, now)`. Returns the question. An explicit `principal:<key>` is interned first (only a
/// command passes one; a firing never interns, D.8.12). The author is the calling session, or the
/// server in a firing.
pub fn create_uq(ex: &mut Exec<'_>, q: NewUq) -> QuestionId {
    let explicit = q.explicit_to.as_ref().map(|k| intern(ex, k));
    let refusal = matches!(q.source, UqSource::Refusal { .. });
    let to = chain(ex.s(), q.about, explicit, refusal);
    let now = ex.now();
    let first = to.first().copied().unwrap_or(USER_PRINCIPAL);
    let due = match q.explicit_due {
        Some(w) => w.resolve(now).max(now),
        None => due_for(ex.s(), first, 0, &q.source, now),
    };
    let by = ex.author();
    ex.append(
        USER_DECISIONS,
        by,
        EntryBody::Question(QuestionBody::User {
            source: q.source,
            about: q.about,
            to,
            due,
            has_default: q.has_default,
        }),
        q.text,
    )
}

/// `n` and its ancestors, nearest first (at most `nodes.len()` steps, D.8.4).
fn up_from(s: &State, n: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    core::iter::once(n).chain(ancestors(s, n))
}

/// `nearest_requested(n)` (D.8.12): the first node among `n` and its ancestors, nearest first,
/// with an unsettled user request (`items(m).requesters_by_age` not empty).
#[must_use]
pub fn nearest_requested(s: &State, n: Option<NodeId>) -> Option<NodeId> {
    let n = n?;
    up_from(s, n).find(|m| {
        s.ix.items
            .get(m)
            .is_some_and(|i| !i.requesters_by_age.is_empty())
    })
}

/// `requesters_on(m, skip_gone)` (D.8.12): the principals of the unsettled user requests on `m`
/// alone, oldest request first, without a second alias of one person, skipping gone persons if
/// asked, at most `UQ_REQUESTERS_MAX`.
#[must_use]
pub fn requesters_on(s: &State, m: NodeId, skip_gone: bool) -> Vec<PrincipalId> {
    let mut out: Vec<PrincipalId> = Vec::new();
    for &(_, p) in
        s.ix.items
            .get(&m)
            .map(|i| &i.requesters_by_age)
            .into_iter()
            .flatten()
    {
        if out.len() >= UQ_REQUESTERS_MAX {
            break;
        }
        if out.iter().any(|&q| same_person(s, q, p)) || (skip_gone && gone(s, p)) {
            continue;
        }
        out.push(p);
    }
    out
}

/// `chain(about, explicit, refusal)` (D.8.12, CS-2, D-3): the explicit addressee; the requesters
/// (of `about`'s own requests for a refusal question); the owners; then `user`. Non-empty, no
/// duplicates and no two aliases of one person, at most `UQ_CHAIN_MAX` stages, ending with `user`
/// (U1). A person one of whose aliases is `user`, an explicit addressee included, takes no stage
/// before the last (§8.4).
#[must_use]
pub fn chain(
    s: &State,
    about: Option<NodeId>,
    explicit: Option<PrincipalId>,
    refusal: bool,
) -> Vec<PrincipalId> {
    let mut out: Vec<PrincipalId> = Vec::new();
    let push = |out: &mut Vec<PrincipalId>, p: PrincipalId| {
        if !same_person(s, p, USER_PRINCIPAL) && !out.iter().any(|&q| same_person(s, q, p)) {
            out.push(p);
        }
    };
    if let Some(p) = explicit {
        // Even a gone explicit addressee takes its stage: its due is then `due_absent`.
        push(&mut out, p);
    }
    let reqs = if refusal {
        // A refusal question's own node only: the requests its default would drop (§8.4, §10.3).
        about.map(|n| requesters_on(s, n, true)).unwrap_or_default()
    } else {
        requesters(s, about)
    };
    for p in reqs {
        push(&mut out, p);
    }
    for p in owners(s, about) {
        push(&mut out, p);
    }
    out.push(USER_PRINCIPAL);
    out
}

/// `requesters(n)` (D.8.12): the requesters of the unsettled user requests on the nearest node from
/// `n` up that has any, oldest first, one per person, gone persons skipped, at most
/// `UQ_REQUESTERS_MAX`.
#[must_use]
pub fn requesters(s: &State, n: Option<NodeId>) -> Vec<PrincipalId> {
    nearest_requested(s, n)
        .map(|m| requesters_on(s, m, true))
        .unwrap_or_default()
}

/// A person node of the directory (an open child of `@people`) that is not tagged `gone`.
fn present_person(s: &State, m: NodeId) -> bool {
    s.nodes.get(&m).is_some_and(|x| {
        crate::index::is_person_node(x) && !x.tags.iter().any(|t| t.as_str() == GONE)
    })
}

/// A team node of the directory: an open child of `@teams`.
fn is_team(s: &State, m: NodeId) -> bool {
    s.nodes
        .get(&m)
        .is_some_and(|x| x.is_open() && x.parent() == Some(TEAMS))
}

/// `rep(m)` (D.8.11): a person's stage, the lowest id among its aliases.
fn rep(s: &State, m: NodeId) -> Option<PrincipalId> {
    s.ix.aliases_of.get(&m).and_then(|a| a.first().copied())
}

/// `owners(n)` (D.8.12, D-3): the persons among the owners of the nearest owned node from `n` (ROOT
/// if None) up, then ROOT's, a team standing for the persons of its own `owners` (A-116), gone
/// persons skipped, one per person, at most `UQ_OWNERS_MAX`.
#[must_use]
pub fn owners(s: &State, n: Option<NodeId>) -> Vec<PrincipalId> {
    let start = n.unwrap_or(ROOT);
    let nearest = up_from(s, start).find(|m| s.nodes.get(m).is_some_and(|x| !x.owners.is_empty()));
    let mut lists: Vec<&BTreeSet<NodeId>> = Vec::new();
    if let Some(m) = nearest {
        lists.extend(s.nodes.get(&m).map(|x| &x.owners));
        if m != ROOT {
            lists.extend(s.nodes.get(&ROOT).map(|x| &x.owners));
        }
    }
    let mut out: Vec<PrincipalId> = Vec::new();
    let mut push = |p: PrincipalId| {
        if out.len() < UQ_OWNERS_MAX && !out.iter().any(|&q| same_person(s, q, p)) {
            out.push(p);
        }
    };
    for list in lists {
        for &x in list {
            if s.nodes.get(&x).is_some_and(|y| y.parent() == Some(PEOPLE)) {
                if present_person(s, x)
                    && let Some(p) = rep(s, x)
                {
                    push(p);
                }
            } else if is_team(s, x) {
                // The team's own `owners`, not inherited: a team with none has no members (A-116).
                for &m in s.nodes.get(&x).map(|y| &y.owners).into_iter().flatten() {
                    if present_person(s, m)
                        && let Some(p) = rep(s, m)
                    {
                        push(p);
                    }
                }
            }
        }
    }
    out
}

/// `due_for(p, stage, source, now)` (D.8.12): `now + user_question.due_absent` for an absent
/// addressee (unless a refusal question); else `now + min(max(user_question.due, p90_gap),
/// capacity.window)` at stage 0 (1.5, O-1: a monthly writer does not hold a question for weeks);
/// else `now + user_question.due`.
#[must_use]
pub fn due_for(s: &State, p: PrincipalId, stage: u8, source: &UqSource, now: Time) -> Time {
    let params = &s.params;
    if absent(s, p, now) && !matches!(source, UqSource::Refusal { .. }) {
        now.plus(params.user_question_due_absent)
    } else if stage == 0 {
        now.plus(
            params
                .user_question_due
                .max(p90_gap(s, p, now))
                .min(params.capacity_window),
        )
    } else {
        now.plus(params.user_question_due)
    }
}

/// The message times of every alias of `p` at or after `now − user_gap.window`, merged, ascending
/// (a binary search in each list, which is ascending, D.4.2).
fn recent_msgs(s: &State, p: PrincipalId, now: Time) -> Vec<Time> {
    let from = now.minus(s.params.user_gap_window);
    let mut out: Vec<Time> = Vec::new();
    for a in aliases(s, p) {
        if let Some(ts) = s.user_msgs.get(&a) {
            let i = ts.partition_point(|t| *t < from);
            out.extend(ts.iter().skip(i).copied());
        }
    }
    out.sort_unstable();
    out
}

/// `absent(p, now)` (D.8.12): `p` is gone, or no alias of `p` wrote within `user_gap.window`.
#[must_use]
pub fn absent(s: &State, p: PrincipalId, now: Time) -> bool {
    if gone(s, p) {
        return true;
    }
    let from = now.minus(s.params.user_gap_window);
    !aliases(s, p).iter().any(|a| {
        s.user_msgs
            .get(a)
            .and_then(|ts| ts.last())
            .is_some_and(|t| *t >= from)
    })
}

/// The nearest rank of the 90th percentile among `len` sorted values, 1-based: `⌈0.9 × len⌉`,
/// computed as `(9 × len + 9) / 10` (A-46). It lies within `1..=len` for every `len >= 1`.
#[must_use]
pub fn p90_rank(len: usize) -> usize {
    len.saturating_mul(9)
        .saturating_add(9)
        .checked_div(10)
        .unwrap_or(0)
}

/// The nearest-rank 90th percentile of `gaps`, `Dur(0)` with none (A-46). Sorts `gaps`.
#[must_use]
pub fn p90_of(gaps: &mut [Dur]) -> Dur {
    gaps.sort_unstable();
    let rank = p90_rank(gaps.len());
    gaps.get(rank.saturating_sub(1))
        .copied()
        .unwrap_or_default()
}

/// `p90_gap(s, p, now)` (D.8.12, A-46): the nearest-rank 90th percentile of the gaps between the
/// messages of `p`'s aliases within `user_gap.window`, `Dur(0)` with no gap.
#[must_use]
pub fn p90_gap(s: &State, p: PrincipalId, now: Time) -> Dur {
    let msgs = recent_msgs(s, p, now);
    let mut gaps: Vec<Dur> = msgs
        .iter()
        .zip(msgs.iter().skip(1))
        .map(|(a, b)| b.since(*a))
        .collect();
    p90_of(&mut gaps)
}

/// `lapse(uq)` (D.8.12, CS-2): the last stage's due passed without a default: `UqLapsed`, a
/// `Lapsed` digest entry per audience principal, and the assume marks on the waiting nodes, or the
/// open `about` node, or `assume_now`.
pub fn lapse(ex: &mut Exec<'_>, uq: QuestionId) {
    let s = ex.s();
    let Some(about) = s.uquestions.get(&uq).map(|u| u.about) else {
        return;
    };
    // Read first: the waits resolve by predicate once the question leaves `Pending`.
    let mut marked: BTreeSet<NodeId> =
        s.ix.waiters
            .get(&WaitKey::Question(uq))
            .cloned()
            .unwrap_or_default();
    let item = DigestItem::Lapsed { question: uq };
    let aud = audience(s, &item);
    ex.emit(Ev::UqLapsed { q: uq });
    add_digest(ex, item, &aud);
    if marked.is_empty()
        && let Some(n) = about
        && !n.is_reserved()
        && ex
            .s()
            .nodes
            .get(&n)
            .is_some_and(crate::state::Node::is_open)
    {
        marked.insert(n);
    }
    if marked.is_empty() {
        assume_now(ex, uq);
    } else {
        for node in marked {
            ex.emit(Ev::MarkSet {
                node,
                mark: Mark::Assume,
                question: uq,
                on: true,
            });
        }
    }
}

/// `add_digest(item, aud)` (D.8.13): one `Appended(System(Digest { item, to: a }))` on
/// `USER_DIGEST` per audience principal, ascending, by the server. `aud` is `audience(item)`, read
/// by the caller before any event that could change it.
pub fn add_digest(ex: &mut Exec<'_>, item: DigestItem, aud: &BTreeSet<PrincipalId>) {
    for &to in aud {
        ex.append(
            USER_DIGEST,
            Author::Server,
            EntryBody::System(SystemNote::Digest { item, to }),
            EntryText::default(),
        );
    }
}

/// One principal per person, in the order given.
fn one_per_person(s: &State, ps: impl IntoIterator<Item = PrincipalId>) -> Vec<PrincipalId> {
    let mut out: Vec<PrincipalId> = Vec::new();
    for p in ps {
        if !out.iter().any(|&q| same_person(s, q, p)) {
            out.push(p);
        }
    }
    out
}

/// `audience(item)` (D.8.13, CS-5): the requesters whose work it concerns (the nearest node's, at
/// most 3; for a refusal drop, those of the user requests it settles), gone persons included; else
/// the persons the question was put to; else `{user}`; never empty.
#[must_use]
pub fn audience(s: &State, item: &DigestItem) -> BTreeSet<PrincipalId> {
    let nearest = |n: Option<NodeId>| {
        nearest_requested(s, n)
            .map(|m| requesters_on(s, m, false))
            .unwrap_or_default()
    };
    let req: Vec<PrincipalId> = match *item {
        DigestItem::RefusalDrop { node, .. } => {
            let users =
                s.ix.items
                    .get(&node)
                    .map(|i| &i.unsettled)
                    .into_iter()
                    .flatten()
                    .filter_map(|r| match s.requests.get(r).map(|x| x.requester) {
                        Some(Requester::User(p)) => Some(p),
                        Some(Requester::External) | None => None,
                    });
            one_per_person(s, users)
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
        | DigestItem::RefusalDrop { question, .. } => s
            .uquestions
            .get(&question)
            .map(|u| {
                let upto = usize::from(u.stage).saturating_add(1);
                one_per_person(
                    s,
                    u.to.iter()
                        .take(upto)
                        .copied()
                        .filter(|&p| !same_person(s, p, USER_PRINCIPAL)),
                )
            })
            .unwrap_or_default(),
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
    BTreeSet::from([USER_PRINCIPAL])
}

/// The entry text of E1's question: the question, its two options, and the first as both the
/// recommendation and the default (§10.3, D.8.12).
fn refusal_text() -> EntryText {
    let t = |x: &str| Text(String::from(x));
    let [drop, keep] = REFUSAL_OPTIONS;
    EntryText {
        main: t(REFUSAL_QUESTION),
        extra: alloc::vec![
            (TextField::Option, t(drop)),
            (TextField::Option, t(keep)),
            (TextField::Recommendation, t(drop)),
            (TextField::Default, t(drop)),
        ],
    }
}

/// `e1_check(n)` (D.8.14, CS-7): when every active class has refused `n` and no refusal question
/// covers it, `create_uq(Refusal { n }, about: n, has_default: true, None, None)`, with the options
/// of §10.3.
pub fn e1_check(ex: &mut Exec<'_>, n: NodeId) {
    let now = ex.now();
    let s = ex.s();
    if !crate::refusals::all_refused(s, n, now) || s.ix.refusal_question.contains_key(&n) {
        return;
    }
    create_uq(
        ex,
        NewUq {
            source: UqSource::Refusal { node: n },
            about: Some(n),
            has_default: true,
            explicit_to: None,
            explicit_due: None,
            text: refusal_text(),
        },
    );
}

/// `refusal_default(n, uq)` (D.8.14): the default "drop the request and tell the requester":
/// `RefusalsCleared`; on an open, unheld node a server decision settling its requests, `close_node`
/// as dropped, and a `RefusalDrop` digest entry; otherwise a `DefaultApplied` entry.
pub fn refusal_default(ex: &mut Exec<'_>, n: NodeId, uq: QuestionId) {
    ex.emit(Ev::RefusalsCleared { node: n, by: uq });
    let s = ex.s();
    let open = s.nodes.get(&n).is_some_and(crate::state::Node::is_open);
    let unheld = s.ix.node.get(&n).is_none_or(|x| x.live == 0);
    if !(open && unheld) {
        // Held or already closed: nothing to drop.
        let item = DigestItem::DefaultApplied { question: uq };
        let aud = audience(s, &item);
        add_digest(ex, item, &aud);
        return;
    }
    let rs: BTreeSet<RequestId> =
        s.ix.items
            .get(&n)
            .map(|i| i.unsettled.clone())
            .unwrap_or_default();
    // Read now, before any of `rs` can settle (D.8.13).
    let item = DigestItem::RefusalDrop {
        node: n,
        question: uq,
    };
    let aud = audience(s, &item);
    let marks = s
        .nodes
        .get(&n)
        .map(|x| x.assume_marks.clone())
        .unwrap_or_default();
    let text = |x: &str| EntryText {
        main: Text(String::from(x)),
        extra: Vec::new(),
    };
    // Steps 3 and 4 of `talk decide` (D.8.17): the assumptions, then the settling.
    let d = ex.append(
        n,
        Author::Server,
        EntryBody::Decision {
            settles: rs.clone(),
            supersedes: None,
            assumes: marks.clone(),
            evidence: None,
            basis: Vec::new(),
        },
        text(REFUSAL_DROPPED),
    );
    assume_marks_off(ex, n, d, &marks);
    for r in rs {
        try_settle(ex, r);
    }
    close_node(
        ex,
        n,
        Closing {
            outcome: Outcome::Dropped,
            by: Author::Server,
            merged_into: None,
            cause: EndCause::Close,
            text: text(REFUSAL_DROPPED),
            reslot: None,
        },
    );
    add_digest(ex, item, &aud);
}

/// The UserQuestion firing of `uq` (D.8.2, CS-2), after its `Fired`. With a default:
/// `Appended(System(Defaulted))` on `USER_DECISIONS`, `UqDefaulted`, then a `DefaultApplied` digest
/// entry per audience principal, or, for a refusal question, `refusal_default`. Without one: the
/// next stage, `UqReaddressed` with its addressee's own due, or past the last stage `lapse`. A
/// question that is not resident or not pending is left alone (its key is then not queued, T4).
pub(crate) fn fire_question(ex: &mut Exec<'_>, uq: QuestionId) {
    let Some(u) = ex.s().uquestions.get(&uq).cloned() else {
        return;
    };
    if u.state != UqState::Pending {
        return;
    }
    if u.has_default {
        ex.append(
            USER_DECISIONS,
            Author::Server,
            EntryBody::System(SystemNote::Defaulted { question: uq }),
            EntryText::default(),
        );
        ex.emit(Ev::UqDefaulted { q: uq });
        match u.source {
            UqSource::Refusal { node } => refusal_default(ex, node, uq),
            // An escalation never has a default (A-59); D1 asks for its entries all the same.
            UqSource::Agent { .. } | UqSource::Escalation { .. } => {
                let item = DigestItem::DefaultApplied { question: uq };
                let aud = audience(ex.s(), &item);
                add_digest(ex, item, &aud);
            }
        }
        return;
    }
    let next = u.stage.checked_add(1);
    match next.and_then(|k| u.to.get(usize::from(k)).map(|&p| (k, p))) {
        Some((stage, p)) => {
            let now = ex.now();
            let due = due_for(ex.s(), p, stage, &u.source, now);
            ex.emit(Ev::UqReaddressed { q: uq, stage, due });
        }
        None => lapse(ex, uq),
    }
}

/// 1.11 (§8.2): the newest unsettled request on `n` whose requester is the person of `principal`,
/// under any alias, with that requester: what a `from_user` with no target clarifies by default.
/// The node's unsettled requests are read newest first, at most `query::SCAN_MAX` (§18). A
/// principal not interned yet is nobody's requester.
pub(crate) fn follow_up_of(
    s: &crate::state::State,
    n: crate::types::NodeId,
    principal: &crate::helpers::principals::PrincipalRef,
) -> Option<(crate::types::RequestId, crate::types::PrincipalId)> {
    let crate::helpers::principals::PrincipalRef::Id(p) = principal else {
        return None;
    };
    let max = usize::try_from(crate::query::SCAN_MAX).unwrap_or(usize::MAX);
    s.ix.items
        .get(&n)?
        .unsettled
        .iter()
        .rev()
        .take(max)
        .find_map(|r| match s.requests.get(r).map(|q| q.requester) {
            Some(crate::types::Requester::User(q))
                if crate::helpers::principals::same_person(s, q, *p) =>
            {
                Some((*r, q))
            }
            _ => None,
        })
}

#[cfg(test)]
#[path = "user_tests.rs"]
pub(crate) mod tests;
