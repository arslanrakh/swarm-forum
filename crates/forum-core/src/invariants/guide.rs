//! `check_guide` (G1, design D.9.2, §26 "Responses"): the next calls of one response, on the state
//! the response was computed on.
//!
//! - (a) they are `query::guide` recomputed on that state, 1.11: with the first row of the call's
//!   `related` (`guide_with`), at most `NEXT_CALLS_MAX`, every `now`
//!   item before every `later` one, rule numbers ascending within each, no two the same call,
//!   `total` at least their number and `total_of` the whole list (1.9: `≥k` below the cap exactly
//!   when an item came from a tidy scan that was cut), and a one-shot item shown whenever one was
//!   cut;
//! - (b) every item, instantiated with the standard fill (`query::guide::standard_fill`), names
//!   objects that exist and passes `validate`, made at once by the session (a `session hello` as a
//!   new session): the check runs the call through `decide` on the state and requires it not to be
//!   refused, apart from the shell's `write.per_hour` and busy, and a tree limit met by the path a
//!   `CreateFor` leaves to the agent;
//! - (c) none is the call just made, and after a refusal whose remedy §21 names and the session
//!   can make now, the first item is that remedy (computed here from §21's table, not from
//!   `guide`'s code);
//! - (d) a close is suggested only when `validate`'s plan of it raises none of
//!   `unsettled_requests`, `open_commitments`, `open_objections`, and `held_by_other`, on a node
//!   with no open child and no open intent of another session; no item closes or merges a reserved
//!   node, a series, or a person or team; and (1.8) none closes or merges away a node that another
//!   session holds live and the caller does not, nor replies on (a `to_user` naming its request, a
//!   `said` of kind `result` there) or decides with `settles` its requests; and (1.10) none decides
//!   with `settles` a request left unfinished, which `spec::left_unfinished` reads naively; and
//!   (1.12) a `now` close of a node whose work waits for its merge (`Node::merge_wait`: its last
//!   `intent_done`, or its open intent, names a pull request) comes only while every unsettled
//!   request there is answered and none is left unfinished, or it carries `ready_to_close` with
//!   none, or the session's take reopened it: never because a decision was recorded or a last
//!   request settled; and (1.12's review) no due checkpoint is `now` beside a `now` close of its
//!   node, which it would leave with its requests unfinished;
//! - (e) (1.7) `end` comes only with no item, after a `hold next` that found nothing and whose
//!   `leave_better` is empty, or after a `session hello`; (1.8) or after a `hold release`, `talk
//!   resolve`, or `node reopen` by a worker that holds no live hold and to which `next` would offer
//!   nothing;
//! - (f) (1.12) no item of rule 13 gives the session what it put down, the review of an objection
//!   it raised or the node it released as `handoff` while that keeps it from the session, and a
//!   tidy take names a node whose needs the session's caps meet and whose `est` its horizon covers.

use alloc::vec::Vec;

use crate::cmd::{Cmd, Ctx, HoldCmd, NodeCmd, SessionCtx, TalkCmd};
use crate::error::{Error, Hint, LimitKind, What, Why};
use crate::invariants::{Inv, Violation, ViolationDetail};
use crate::query::guide::guide_with;
use crate::query::guide::{
    End, Guidance, Moment, NEXT_CALLS_MAX, NEXT_TOTAL_CAP, Skeleton, all_items, same_call,
    same_skeleton, standard_fill,
};
use crate::query::{NoMeter, resolve};
use crate::reply::{Done, Total, WarningCode};
use crate::state::State;
use crate::types::{ClassName, NodeId, NodeRef, QuestionId, SaidKind, SessionId, Time};

/// 1.8 (G1 (d)): another session holds `n` live, and `sess` does not.
fn others(s: &State, sess: Option<SessionId>, now: Time, n: NodeId) -> bool {
    sess.is_some_and(|_| crate::helpers::holds::held_by_another(s, n, sess, now))
}

fn bad(what: &'static str) -> Violation {
    Violation {
        inv: Inv::G1,
        detail: ViolationDetail::Text(what),
    }
}

/// A violation at node `n`.
fn at(n: NodeId) -> Violation {
    Violation {
        inv: Inv::G1,
        detail: ViolationDetail::Node(n),
    }
}

/// The context of a call by `sess` (or by a new session) at `now`.
fn ctx_of(s: &State, sess: Option<SessionId>, now: Time, session: SessionCtx) -> Ctx {
    Ctx {
        now,
        class: sess
            .and_then(|x| s.sessions.get(&x))
            .map_or_else(ClassName::unknown, |x| x.class.clone()),
        caps: None,
        session,
        op: None,
    }
}

/// Whether the call a skeleton describes, made by `sess` with the standard fill, is accepted:
/// `Ok` with its warnings, or the refusal.
fn attempt(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    sk: &Skeleton,
) -> Option<Result<Done, Error>> {
    let (sc, cmd) = standard_fill(s, sess, sk)?;
    let ctx = ctx_of(s, sess, now, sc);
    Some(crate::decide(s, &ctx, &cmd).1)
}

/// The refusals G1 (b) excuses: the shell's `write.per_hour` and busy, which the core does not
/// see, and a tree limit met by a `CreateFor`'s path, which the agent writes whole.
fn excused(sk: &Skeleton, e: &Error) -> bool {
    matches!(
        e.hint,
        Hint::Limit(LimitKind::WritesPerHour | LimitKind::Busy)
    ) || (matches!(sk, Skeleton::CreateFor { .. })
        && matches!(e.hint, Hint::Limit(LimitKind::Depth | LimitKind::Children)))
}

/// §21's remedy of a refusal, read from the table: the candidate skeletons, in preference order
/// (a take before a `view node` when the take passes). None for a refusal with no remedy.
fn remedies(s: &State, cmd: &Cmd, e: &Error) -> Vec<Skeleton> {
    let take_or_view =
        |n: NodeId| alloc::vec![Skeleton::Take { target: n }, Skeleton::ViewNode { node: n }];
    let target = |r: &NodeRef| resolve(s, r);
    match &e.hint {
        Hint::NotFound(What::Session) | Hint::Invalid(Why::ClassMismatch) => {
            alloc::vec![Skeleton::Hello]
        }
        Hint::StaleToken { token, .. } => {
            let n = if let Cmd::Node(NodeCmd::Update { at, .. } | NodeCmd::Close { at, .. }) = cmd {
                target(at)
            } else {
                Some(token.node)
            };
            n.map(take_or_view).unwrap_or_default()
        }
        Hint::StaleVersion { .. } | Hint::Invalid(Why::MissingDigestVersion) => {
            if let Cmd::Node(NodeCmd::Update { at, .. }) = cmd {
                target(at)
                    .map(|n| alloc::vec![Skeleton::ViewNode { node: n }])
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        Hint::Invalid(Why::VersionAhead) => {
            let n = if let Cmd::Talk(TalkCmd::Approve { on, .. }) = cmd {
                target(on)
            } else if let Cmd::Talk(TalkCmd::Decide { basis, .. }) = cmd {
                basis.iter().find_map(|b| {
                    if let crate::cmd::BasisArg::Node {
                        at,
                        version: Some(v),
                    } = b
                    {
                        target(at).filter(|n| s.nodes.get(n).is_some_and(|x| *v > x.version))
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            n.map(|n| alloc::vec![Skeleton::ViewNode { node: n }])
                .unwrap_or_default()
        }
        Hint::Closed {
            merged_into: Some(m),
            ..
        } => take_or_view(*m),
        // 1.9 (§21): a take of a closed node with an open objection: the review, a `resolve` of
        // its first open objection.
        Hint::Closed {
            node,
            merged_into: None,
        } if matches!(cmd, Cmd::Hold(HoldCmd::Take { .. }))
            && s.nodes.get(node).is_some_and(|x| !x.is_open()) =>
        {
            s.objections
                .iter()
                .find(|(_, o)| o.node == *node && o.state == crate::state::ObjectionState::Open)
                .map(|(o, _)| alloc::vec![Skeleton::Resolve { objection: *o }])
                .unwrap_or_default()
        }
        Hint::Invalid(Why::IntentOpen) => {
            if let Cmd::Hold(HoldCmd::Checkpoint { token, .. }) = cmd {
                alloc::vec![Skeleton::CheckpointSwap { token: *token }]
            } else {
                Vec::new()
            }
        }
        Hint::Limit(LimitKind::Renew) => {
            if let Cmd::Hold(HoldCmd::Renew { token, .. }) = cmd {
                alloc::vec![Skeleton::Checkpoint {
                    token: *token,
                    intent_done: false,
                    intent: false,
                }]
            } else {
                Vec::new()
            }
        }
        Hint::Invalid(Why::NotPeopleQuestion | Why::NotAgentQuestion | Why::NotUserQuestion) => {
            let Cmd::Talk(
                TalkCmd::Heard {
                    answers: Some(q), ..
                }
                | TalkCmd::FromUser {
                    answers: Some(q), ..
                }
                | TalkCmd::Route {
                    answers: Some(q), ..
                }
                | TalkCmd::Answer { question: q, .. },
            ) = cmd
            else {
                return Vec::new();
            };
            let q: QuestionId = *q;
            if let Some(pq) = s.pquestions.get(&q) {
                alloc::vec![Skeleton::Heard {
                    at: pq.node,
                    answers: q,
                    where_: pq.conv.clone(),
                }]
            } else if s.aquestions.contains_key(&q) {
                alloc::vec![Skeleton::AnswerAgent { question: q }]
            } else if let Some(uq) = s.uquestions.get(&q) {
                // The addressee is named only when the session serves several persons; either form
                // is the remedy.
                let to = uq
                    .to
                    .get(usize::from(uq.stage))
                    .copied()
                    .unwrap_or(crate::types::USER_PRINCIPAL);
                alloc::vec![
                    Skeleton::AnswerUser {
                        question: q,
                        principal: None,
                    },
                    Skeleton::AnswerUser {
                        question: q,
                        principal: Some(to),
                    }
                ]
            } else {
                Vec::new()
            }
        }
        Hint::Full { .. }
        | Hint::HoldsLimit { .. }
        | Hint::Paused { .. }
        | Hint::Cycle { .. }
        | Hint::Closed { .. }
        | Hint::OpIdConflict
        | Hint::Limit(_)
        | Hint::Invalid(_)
        | Hint::NotFound(_) => Vec::new(),
    }
}

/// 1.12 (G1 (d)): a `now` close of `n`, whose work waits for its merge, stands on
/// what rule 8 keeps standing: every unsettled request there answered, none left unfinished (read
/// naively, `spec::left_unfinished`), or `ready_to_close` with none; or the session's take, `token`,
/// reopened it.
fn merge_close_stands(
    s: &State,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    now: Time,
    n: NodeId,
    token: crate::types::Token,
) -> bool {
    let ok = out.as_ref().ok().map(|d| &d.reply);
    let unsettled: Vec<crate::types::RequestId> = s
        .requests
        .iter()
        .filter(|(_, q)| q.node == n && q.settled.is_none())
        .map(|(r, _)| *r)
        .collect();
    let answered = !unsettled.is_empty()
        && unsettled.iter().all(|r| {
            s.requests.get(r).is_some_and(|q| q.answered_by.is_some())
                && !crate::spec::left_unfinished(s, cmd, ok, *r)
        });
    let ready = unsettled.is_empty()
        && crate::spec::flags(s, n, now).contains(crate::types::Flag::ReadyToClose);
    let reopened = s.holds.get(&token).is_some_and(|h| h.reopened.is_some());
    answered || ready || reopened
}

/// G1 over one response's next calls (D.9.2).
pub(super) fn check_guide(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
    g: &Guidance,
) -> Result<(), Vec<Violation>> {
    let mut v = Vec::new();
    // (a)
    let again = guide_with(s, sess, now, cmd, out, related, &mut NoMeter);
    if again != *g {
        v.push(bad("G1 (a): the next calls are not guide's on this state"));
    }
    if g.items.len() > NEXT_CALLS_MAX {
        v.push(bad("G1 (a): more than NEXT_CALLS_MAX next calls"));
    }
    let mut later = false;
    let mut last: Option<(Moment, u8)> = None;
    for it in &g.items {
        if it.when == Moment::Later {
            later = true;
        } else if later {
            v.push(bad("G1 (a): a now item after a later one"));
        }
        if let Some((m, n)) = last
            && m == it.when
            && it.rule.number() < n
        {
            v.push(bad("G1 (a): rule numbers not ascending"));
        }
        last = Some((it.when, it.rule.number()));
    }
    for (i, a) in g.items.iter().enumerate() {
        if g.items
            .iter()
            .skip(i.saturating_add(1))
            .any(|b| same_skeleton(&a.call, &b.call))
        {
            v.push(bad("G1 (a): two next calls are the same call"));
        }
    }
    let (all, cut) = all_items(s, sess, now, cmd, out, related);
    let shown = u32::try_from(g.items.len()).unwrap_or(u32::MAX);
    let n = |t: u32| usize::try_from(t).is_ok_and(|t| t == all.len());
    let total_ok = match g.total {
        Total::Exact(t) => t >= shown && n(t) && t < NEXT_TOTAL_CAP && !cut,
        // 1.9 (§18): `≥k` below the cap only when a tidy scan was cut.
        Total::AtLeast(t) => {
            t >= shown
                && ((t == NEXT_TOTAL_CAP
                    && all.len() >= usize::try_from(NEXT_TOTAL_CAP).unwrap_or(usize::MAX))
                    || (cut && n(t) && t < NEXT_TOTAL_CAP))
        }
    };
    if !total_ok {
        v.push(bad("G1 (a): total does not count every item"));
    }
    if !g.items.iter().any(|i| i.once) && all.iter().skip(NEXT_CALLS_MAX).any(|i| i.once) {
        v.push(bad("G1 (a): a one-shot item was cut and none is shown"));
    }
    // (b), (c), and (d) for each item.
    for it in &g.items {
        if same_call(s, &it.call, cmd) {
            v.push(bad("G1 (c): a next call repeats the call just made"));
        }
        match attempt(s, sess, now, &it.call) {
            None => v.push(bad(
                "G1 (b): a next call names an object that does not exist",
            )),
            Some(Err(e)) => {
                if !excused(&it.call, &e) {
                    v.push(bad("G1 (b): a next call would be refused"));
                }
            }
            Some(Ok(done)) => {
                if let Skeleton::Close { node, .. } = &it.call
                    && done.warnings.iter().any(|w| {
                        matches!(
                            w.code,
                            WarningCode::UnsettledRequests
                                | WarningCode::OpenCommitments
                                | WarningCode::OpenObjections
                                | WarningCode::HeldByOther
                        )
                    })
                {
                    v.push(at(*node));
                }
            }
        }
        match &it.call {
            Skeleton::Close { node, token, .. } => {
                // 1.12: on a node waiting for its merge, `now` only while it stands.
                if it.when == Moment::Now
                    && s.nodes
                        .get(node)
                        .is_some_and(crate::state::Node::merge_wait)
                    && !merge_close_stands(s, cmd, out, now, *node, *token)
                {
                    v.push(at(*node));
                }
                let container = s
                    .nodes
                    .get(node)
                    .is_none_or(crate::state::Node::is_container);
                let open_child = s.ix.node.get(node).is_some_and(|i| i.open_children > 0);
                let other_intent = s
                    .nodes
                    .get(node)
                    .and_then(|x| x.open_intent)
                    .is_some_and(|i| Some(i.session) != sess);
                if container || open_child || other_intent || node.is_reserved() {
                    v.push(at(*node));
                }
            }
            Skeleton::Merge { from, into } => {
                for n in [from, into] {
                    if s.nodes.get(n).is_none_or(crate::state::Node::is_container) {
                        v.push(at(*n));
                    }
                }
                if others(s, sess, now, *from) {
                    v.push(at(*from));
                }
            }
            // 1.8: another's node is never closed, merged away, replied on, or decided. 1.10: nor
            // is a request left unfinished decided (§20.9 Terms), read naively.
            Skeleton::Decide { at: n, settles, .. } => {
                let theirs = |r: &crate::types::RequestId| {
                    s.requests
                        .get(r)
                        .is_some_and(|q| others(s, sess, now, q.node))
                };
                let ok = out.as_ref().ok().map(|d| &d.reply);
                let unfinished =
                    |r: &crate::types::RequestId| crate::spec::left_unfinished(s, cmd, ok, *r);
                if !settles.is_empty()
                    && (others(s, sess, now, *n)
                        || settles.iter().any(theirs)
                        || settles.iter().any(unfinished))
                {
                    v.push(at(*n));
                }
            }
            Skeleton::ToUser { re, .. } => {
                for r in re {
                    if let Some(q) = s.requests.get(r)
                        && others(s, sess, now, q.node)
                    {
                        v.push(at(q.node));
                    }
                }
            }
            Skeleton::Said {
                at: n,
                kind: SaidKind::Result,
                ..
            } => {
                if others(s, sess, now, *n) {
                    v.push(at(*n));
                }
            }
            // 1.12's review: a due checkpoint is never `now` beside a `now` close of its node,
            // which it would leave with its requests unfinished.
            Skeleton::Checkpoint { token, .. } => {
                if it.when == Moment::Now
                    && matches!(it.because, crate::query::guide::Because::LeaseDue { .. })
                    && g.items.iter().any(|c| {
                        c.when == Moment::Now
                            && matches!(c.call, Skeleton::Close { token: t, .. } if t == *token)
                    })
                {
                    v.push(at(token.node));
                }
            }
            Skeleton::Hello
            | Skeleton::LogFromUser { .. }
            | Skeleton::ViewNode { .. }
            | Skeleton::CheckpointSwap { .. }
            | Skeleton::Release { .. }
            | Skeleton::Take { .. }
            | Skeleton::Next
            | Skeleton::CreateFor { .. }
            | Skeleton::Split { .. }
            | Skeleton::AddOffer { .. }
            | Skeleton::SayTo { .. }
            | Skeleton::Said { .. }
            | Skeleton::Heard { .. }
            | Skeleton::Route { .. }
            | Skeleton::AnswerUser { .. }
            | Skeleton::AnswerAgent { .. }
            | Skeleton::Resolve { .. }
            | Skeleton::AskOwners { .. }
            | Skeleton::LinkDuplicate { .. }
            | Skeleton::Wait { .. }
            | Skeleton::Reopen { .. } => {}
        }
    }
    // (f) (1.12): rule 13 gives back nothing the session put down; its tidy take fits the session.
    if let Some(x) = sess {
        for it in g
            .items
            .iter()
            .filter(|i| i.rule == crate::query::guide::Rule::Tidy)
        {
            if let Skeleton::Take { target } = &it.call {
                let fits = s.sessions.get(&x).is_some_and(|y| {
                    s.nodes.get(target).is_some_and(|n| {
                        n.needs.is_subset(&y.caps)
                            && !matches!((n.est, y.horizon), (Some(e), Some(h)) if e > h)
                    })
                });
                if crate::state::handed_back(s, x, *target, now)
                    || (matches!(it.because, crate::query::guide::Because::Tidy { .. }) && !fits)
                {
                    v.push(at(*target));
                }
            }
            if let Skeleton::Resolve { objection } = &it.call
                && s.entries
                    .get(objection)
                    .is_some_and(|m| m.by == crate::state::Author::Session(x))
            {
                v.push(bad(
                    "G1 (f): rule 13 gives a session the review of its own objection",
                ));
            }
        }
    }
    // (c): after a refusal, §21's remedy first, when the session can make it now.
    if let Err(e) = out
        && let Some(remedy) =
            remedies(s, cmd, e)
                .into_iter()
                .find(|r| match attempt(s, sess, now, r) {
                    // A take of a node the session holds already answers with that hold: it does not
                    // succeed as a take (`can_take`, D.8.26), so the remedy is the `view node`.
                    Some(Ok(d)) => {
                        !matches!(d.reply, crate::reply::Reply::Taken { existing: true, .. })
                    }
                    Some(Err(_)) | None => false,
                })
        && !g
            .items
            .first()
            .is_some_and(|i| same_skeleton(&i.call, &remedy))
    {
        // `limit` for a text over `text.max`, busy, and `write.per_hour` carry no next calls, and
        // have no remedy either.
        v.push(bad(
            "G1 (c): the first next call is not the refusal's remedy",
        ));
    }
    // (e) (1.7): `end` only with no item, after a `next` that found nothing with an empty
    // `leave_better`, or a `hello` with no offer; 1.10: or a `hello` refused for a `from_user`
    // that names no principal.
    if let Some(end) = g.end {
        let reply = out.as_ref().ok().map(|d| &d.reply);
        let fits = match end {
            End::AfterNext => matches!(
                reply,
                Some(crate::reply::Reply::Next(crate::reply::NextReply::Empty { leave_better, .. }))
                    if leave_better.is_empty()
            ),
            End::AtHello => {
                matches!(reply, Some(crate::reply::Reply::Hello { .. }))
                    || crate::query::guide::hello_refused_unnamed(cmd, out)
            }
            End::Idle => {
                matches!(
                    reply,
                    Some(
                        crate::reply::Reply::Released { .. }
                            | crate::reply::Reply::ObjectionResolved { .. }
                            | crate::reply::Reply::Reopened { .. }
                    )
                ) && sess.is_some_and(|x| {
                    crate::query::guide::idle_worker(s, x, now)
                        && crate::query::ranked(
                            s,
                            x,
                            now,
                            &crate::cmd::NextQuery::default(),
                            1,
                            &mut NoMeter,
                        )
                        .0
                        .is_empty()
                })
            }
        };
        if !g.items.is_empty() || !fits {
            v.push(bad("G1 (e): end comes with an item, or after another call"));
        }
    }
    if v.is_empty() { Ok(()) } else { Err(v) }
}
