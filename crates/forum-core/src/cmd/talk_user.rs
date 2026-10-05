//! The commands of `talk from_user`, `to_user`, `route`, and `ask` to a human; `hello {from_user}` (design D.13.1, D.13.2). Owner: WP-D5a.
//!
//! Each command is a pair: a validation that reads the state and returns the plan or the error,
//! and an execution of the plan that emits the events and cannot fail. Every validation follows
//! D.8.24's order (D.14.5): the target node (`not_found(Node)`, the `node` of `from_user`, the
//! `to` of `route`, the `at` of `ask`), the other references (`not_found`), then the parameter and
//! state checks (`invalid`).
//!
//! - `from_user` (D.8.11, §8.1, §8.2) records a human's message on `@user` and routes it by the
//!   first of `answers`, `clarifies`, and `to` (a node, or none with a reason, 1.7), else to the
//!   session's only live hold, 1.11: as a clarification of the newest unsettled request there of the
//!   entry's person, with the `follow_up` warning, when it has one; its reply's `routed` says which
//!   branch applied (1.4, R3). A clarification of a settled request is a new
//!   request on that request's node, which `routed` names with `follows` (D.14.5). In a session
//!   that serves no principal, it names one, the call's or `hello`'s, or is `invalid(NoPrincipal)`
//!   (1.9, §8.1); one filed for `user` in a session that did not serve `user` before warns
//!   `unnamed_user` (1.6), and one whose text looks like a client's injected instructions warns
//!   `client_context` (1.9).
//! - `to_user` (D.8.11, CS-4, A-23) records the swarm's report to a principal, shows the digest
//!   entries of that person it names, and, of kind `result` only, answers that person's requests it
//!   names; a `result` that names none of that person's requests, and none of that person's
//!   `from_user` entries, warns `result_answered_nothing` (1.4, 1.5, §21). 1.9 (§8.2): it routes no
//!   `from_user` entry it names, which 1.7 routed to none; and a `result` with neither `re` nor
//!   `at`, from a session that serves its principal, names that person's unsettled user requests
//!   on the nodes the session holds (§8.1).
//! - `route` (D.8.11) routes an unrouted entry by the first of `answers`, `clarifies`, and `to` (a
//!   node, or nothing), as `from_user` does (1.7); the first routing wins.
//! - `ask` to a human (D.8.12, §8.4, A-16) creates a question for the user with its chain fixed now,
//!   or returns the same question pending for the same first addressee; `options` may be empty
//!   (1.4, R27), and `user_saturated` warns when the first addressee already has more than
//!   `user_question.max_pending` pending questions (CS-33).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::apply::target;
use crate::cmd::{AskTo, Cmd, FromUserArg, RouteTo, SessionCtx, TalkCmd};
use crate::error::{Error, Field, What, Why};
use crate::event::{EntryBody, EntryText, Ev, TextField};
use crate::helpers::Exec;
use crate::helpers::conversation::{answer_pq, route_to, try_settle};
use crate::helpers::holds::live_holds_of;
use crate::helpers::principals::{
    PrincipalRef, aliases, intern_ref, principal_ref, resolve_principal, same_person, serves_none,
};
use crate::helpers::user::{NewUq, chain, create_uq};
use crate::reply::{
    Reply, RoutedReply, UnroutedWhy, UserEntryReply, Warning, WarningCode, WarningDetail,
};
use crate::state::{DigestState, Routing, State, UqSource, UqState};
use crate::types::{
    DigestId, Eid, EntryType, KeyStr, NodeId, PrincipalId, PrincipalKey, QuestionId, RequestId,
    Requester, SessionId, Text, Time, ToUserKind, USER, USER_DECISIONS, USER_PRINCIPAL,
    UserEntryId,
};

use super::Vcx;

// ------------------------------------------------------------------------------------------------
// from_user and hello {from_user} (D.8.11)

/// How a `from_user` entry is routed, as validation found it on the pre-state.
#[derive(Debug)]
pub(crate) enum Route {
    /// `answers: uq`, `uq` pending: the answer, and its source's consequence.
    Answers {
        /// The question.
        uq: QuestionId,
        /// Its source.
        source: UqSource,
    },
    /// `answers: uq`, `uq` no longer pending: the entry stays unrouted (A-37).
    NotPending(QuestionId),
    /// `clarifies: r`, `r` unsettled.
    Clarifies(RequestId),
    /// A new request on `node` (the `node` argument, or the session's only live hold), following
    /// the settled request a clarification named, if any (D.14.5).
    Request {
        /// The node.
        node: NodeId,
        /// The settled request it follows.
        follows: Option<RequestId>,
    },
    /// `to: none` (1.7): no action, with the reason.
    NoAction(Text),
    /// No target, and the session holds `holds` nodes, 0 or more than one.
    NoTarget(u32),
}

/// The reason of a routing to none that names none (1.7, §8.2).
const NO_WORK: &str = "no work needed";

/// A `to` resolved: a node, or none.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// A node.
    Node(NodeId),
    /// `none`.
    Nowhere,
}

/// The routing arguments `from_user`, `hello {from_user}`, and `route` share (1.7, §8.2).
#[derive(Clone, Copy)]
struct RouteArgs<'a> {
    answers: Option<QuestionId>,
    clarifies: Option<RequestId>,
    to: Option<Target>,
    reason: Option<&'a Text>,
}

/// The references of the routing arguments (D.8.24's `not_found` step): `answers` is a question
/// (`not_found(Question)`), `clarifies` a request (`not_found(Request)`).
fn route_refs(s: &State, a: &RouteArgs<'_>) -> Result<(), Error> {
    if let Some(q) = a.answers
        && !is_question(s, q)
    {
        return Err(Error::not_found(What::Question));
    }
    if let Some(r) = a.clarifies
        && !s.requests.contains_key(&r)
    {
        return Err(Error::not_found(What::Request));
    }
    Ok(())
}

/// The parameter checks of the routing arguments (D.8.24's `invalid` step): `answers` is a user
/// question (`invalid(NotUserQuestion)`), a `reason` given is not empty
/// (`invalid(EmptyText(Reason))`), and `to` is not reserved (`invalid(ReservedNode)`, A-29).
fn route_params(s: &State, a: &RouteArgs<'_>) -> Result<(), Error> {
    if let Some(q) = a.answers
        && !s.uquestions.contains_key(&q)
    {
        return Err(Error::invalid(Why::NotUserQuestion));
    }
    if a.reason.is_some_and(Text::is_empty) {
        return Err(Error::invalid(Why::EmptyText(Field::Reason)));
    }
    if matches!(a.to, Some(Target::Node(n)) if n.is_reserved()) {
        return Err(Error::invalid(Why::ReservedNode));
    }
    Ok(())
}

/// The routing the first of `answers`, `clarifies`, and `to` gives, on the pre-state (A-99, 1.7);
/// None when none is given.
fn route_of(s: &State, a: &RouteArgs<'_>) -> Option<Route> {
    if let Some((uq, u)) = a.answers.and_then(|q| s.uquestions.get(&q).map(|u| (q, u))) {
        return Some(if u.state == UqState::Pending {
            Route::Answers {
                uq,
                source: u.source,
            }
        } else {
            Route::NotPending(uq)
        });
    }
    if let Some((r, req)) = a.clarifies.and_then(|r| s.requests.get(&r).map(|x| (r, x))) {
        // Settling is final (§9.4): the clarification of a settled request is a new request on
        // the same node.
        return Some(if req.settled.is_some() {
            Route::Request {
                node: req.node,
                follows: Some(r),
            }
        } else {
            Route::Clarifies(r)
        });
    }
    a.to.map(|t| match t {
        Target::Node(node) => Route::Request {
            node,
            follows: None,
        },
        Target::Nowhere => {
            Route::NoAction(a.reason.cloned().unwrap_or_else(|| Text(NO_WORK.into())))
        }
    })
}

/// Resolves a `to` (`not_found(Node)`).
fn target_of(s: &State, to: Option<&RouteTo>) -> Result<Option<Target>, Error> {
    Ok(match to {
        None => None,
        Some(RouteTo::Nowhere) => Some(Target::Nowhere),
        Some(RouteTo::Node(r)) => Some(Target::Node(crate::apply::node_of(s, r)?)),
    })
}

/// The plan of `talk from_user`, and of the `from_user` of `session hello` (D.8.11).
#[derive(Debug)]
pub(crate) struct FromUserPlan {
    /// Whose words.
    principal: PrincipalRef,
    /// The message verbatim, and the client as `TextField::Client`.
    text: EntryText,
    /// The routing.
    route: Route,
    /// The principal is `user`, and the session did not serve `user` before the call: the
    /// `unnamed_user` warning (1.6, 1.9, §8.1).
    unnamed: bool,
    /// The text looks like a client's injected instructions: the `client_context` warning (1.9,
    /// §8.1).
    client_context: bool,
    /// 1.11 (§8.2): the routing is the default's clarification of this request, the newest
    /// unsettled one of the entry's person on the node of the session's only hold, with its node and
    /// requester: the `follow_up` warning.
    follow_up: Option<(RequestId, NodeId, PrincipalId)>,
}

/// The arguments `from_user` shares with `hello {from_user}`.
struct FromUserArgs<'a> {
    text: &'a Text,
    client: &'a Text,
    principal: Option<&'a PrincipalKey>,
    route: RouteArgs<'a>,
}

/// Whether `q` names a question of any kind: a resident people, agent, or user question, or an
/// entry whose metadata says it is a question (an agent question leaves memory with its note,
/// D.4.12, while its metadata may stay).
fn is_question(s: &State, q: QuestionId) -> bool {
    s.uquestions.contains_key(&q)
        || s.pquestions.contains_key(&q)
        || s.aquestions.contains_key(&q)
        || s.entries
            .get(&q)
            .is_some_and(|m| m.ty == EntryType::Question)
}

/// Whether a `from_user`'s text looks like what a client adds to the human's turn (1.9, §8.1): it
/// starts, after any white space, with `# AGENTS.md instructions`, the heading a Codex client gives
/// an instruction file, or carries a client block, `<environment_context>` or `<INSTRUCTIONS>`.
fn client_context(t: &Text) -> bool {
    let s = t.0.as_str();
    s.trim_start().starts_with("# AGENTS.md instructions")
        || s.contains("<environment_context>")
        || s.contains("<INSTRUCTIONS>")
}

/// D.8.11's validation of a `from_user`, after its target node: `answers` is a question
/// (`not_found(Question)`), `clarifies` a request (`not_found(Request)`); then `answers` is a user
/// question (`invalid(NotUserQuestion)`), a `reason` given is not empty, `to` is not reserved
/// (`invalid(ReservedNode)`, A-29), a session that serves no principal names one
/// (`invalid(NoPrincipal)`, 1.9), and the principal resolves (`invalid(AmbiguousPrincipal)`).
/// Every argument given is checked, though only the first of `answers`, `clarifies`, and `to`
/// routes (A-99, 1.7).
fn plan_from_user(cx: &Vcx<'_>, a: &FromUserArgs<'_>) -> Result<FromUserPlan, Error> {
    let s = cx.s;
    route_refs(s, &a.route)?;
    route_params(s, &a.route)?;
    // 1.9 (§8.1): a `from_user` records a person who wrote to the agent directly, whom its client
    // names, or `user`; with neither the call nor the session naming one, it is no person's words.
    if a.principal.is_none() && serves_none(s, cx.sess) {
        return Err(Error::invalid(Why::NoPrincipal));
    }
    let principal = resolve_principal(s, cx.sess, a.principal)?;
    // 1.6, 1.9: filed for `user` in a session that did not serve `user` before this call (one the
    // call opened served nothing before it, whatever `hello`'s principal bound).
    let served_user = cx.ctx.session != SessionCtx::New
        && cx
            .sess
            .and_then(|x| s.sessions.get(&x))
            .is_some_and(|x| x.principals.contains(&USER_PRINCIPAL));
    let unnamed = principal == PrincipalRef::Id(USER_PRINCIPAL) && !served_user;
    let mut follow_up = None;
    let route = route_of(s, &a.route).unwrap_or_else(|| {
        let holds = cx
            .sess
            .map(|x| live_holds_of(s, x, cx.now))
            .unwrap_or_default();
        match holds.as_slice() {
            // 1.11 (§8.2): a follow-up of the person's open request there clarifies it.
            [only] => match crate::helpers::user::follow_up_of(s, only.node, &principal) {
                Some((r, p)) => {
                    follow_up = Some((r, only.node, p));
                    Route::Clarifies(r)
                }
                None => Route::Request {
                    node: only.node,
                    follows: None,
                },
            },
            _ => Route::NoTarget(u32::try_from(holds.len()).unwrap_or(u32::MAX)),
        }
    });
    let mut text = EntryText {
        main: a.text.clone(),
        extra: Vec::new(),
    };
    text.extra.push((TextField::Client, a.client.clone()));
    Ok(FromUserPlan {
        principal,
        client_context: client_context(a.text),
        text,
        route,
        unnamed,
        follow_up,
    })
}

/// Validates `talk from_user` (D.8.11): the node of `to` resolves (the target, `not_found(Node)`),
/// then `plan_from_user`.
pub(crate) fn from_user(cx: &Vcx<'_>, cmd: &Cmd) -> Result<FromUserPlan, Error> {
    let Cmd::Talk(TalkCmd::FromUser {
        text,
        client,
        principal,
        answers,
        to,
        clarifies,
        reason,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    target(cx.s, cmd)?;
    let to = target_of(cx.s, to.as_ref())?;
    plan_from_user(
        cx,
        &FromUserArgs {
            text,
            client,
            principal: principal.as_ref(),
            route: RouteArgs {
                answers: *answers,
                clarifies: *clarifies,
                to,
                reason: reason.as_ref(),
            },
        },
    )
}

/// Validates the `from_user` of `session hello` (D.8.11): its principal is `from_user.principal`,
/// else `hello`'s own `principal`, else the session's (`invalid(AmbiguousPrincipal)`); the node of
/// its `to` resolves (`not_found(Node)`); it routes as `talk from_user` does (1.6, 1.7, §8.2), by
/// the first of `answers`, `clarifies`, and `to`, else to the session's only live hold, if it has
/// exactly one.
pub(crate) fn hello_from_user(
    cx: &Vcx<'_>,
    f: &FromUserArg,
    hello_principal: Option<&PrincipalKey>,
) -> Result<FromUserPlan, Error> {
    let to = target_of(cx.s, f.to.as_ref())?;
    plan_from_user(
        cx,
        &FromUserArgs {
            text: &f.text,
            client: &f.client,
            principal: f.principal.as_ref().or(hello_principal),
            route: RouteArgs {
                answers: f.answers,
                clarifies: f.clarifies,
                to,
                reason: f.reason.as_ref(),
            },
        },
    )
}

/// How the entry `e` stands after a routing to a node: its `Request` routing (on the request's own
/// node, which `route_to` names when the message had a request already), with `follows`.
fn routed_request(s: &State, e: UserEntryId, follows: Option<RequestId>) -> RoutedReply {
    match s.user_entries.get(&e).map(|u| u.routing) {
        Some(Routing::Request { node, request }) => RoutedReply::Request {
            request,
            node,
            follows,
        },
        Some(r) => routed_of(r),
        None => RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 0 }),
    }
}

/// A stored routing as a reply's `routed` (the first routing, for `route {existing}`).
fn routed_of(r: Routing) -> RoutedReply {
    match r {
        Routing::Unrouted => RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 0 }),
        Routing::Request { node, request } => RoutedReply::Request {
            request,
            node,
            follows: None,
        },
        Routing::Clarifies { request } => RoutedReply::Clarifies { request },
        Routing::Answers { question } => RoutedReply::Answers { question },
        Routing::NoAction => RoutedReply::NoAction,
    }
}

/// Executes a `from_user` (D.8.11): `PrincipalSeen` if the principal is new; `Appended(FromUser)` on
/// `@user`, which makes the session serve its principal, with the `unnamed_user` warning when that
/// principal is `user` and the session did not serve it before (1.6, 1.9), and the
/// `client_context` warning when the text looks like a client's injected instructions (1.9); then
/// the routing. Returns the entry's reply.
fn run_from_user(ex: &mut Exec<'_>, plan: FromUserPlan) -> UserEntryReply {
    let p = intern_ref(ex, &plan.principal);
    let by = ex.author();
    let e = ex.append(USER, by, EntryBody::FromUser { principal: p }, plan.text);
    if plan.unnamed {
        ex.warn(Warning {
            code: WarningCode::UnnamedUser,
            id: Some(USER),
            detail: WarningDetail::UnnamedUser { entry: e },
        });
    }
    if plan.client_context {
        ex.warn(Warning {
            code: WarningCode::ClientContext,
            id: Some(USER),
            detail: WarningDetail::ClientContext { entry: e },
        });
    }
    let routed = apply_route(ex, e, plan.route);
    if let Some((request, node, requester)) = plan.follow_up {
        ex.warn(Warning {
            code: WarningCode::FollowUp,
            id: Some(node),
            detail: WarningDetail::FollowUp {
                entry: e,
                request,
                node,
                requester,
            },
        });
    }
    UserEntryReply {
        entry: e,
        key: KeyStr::of_user_entry(e),
        routed,
        principal: p,
    }
}

/// Routes the unrouted entry `e` as validation decided (D.8.11, 1.7): the events of each branch,
/// and the reply's `routed`.
fn apply_route(ex: &mut Exec<'_>, e: UserEntryId, route: Route) -> RoutedReply {
    match route {
        Route::Answers { uq, source } => {
            ex.emit(Ev::EntryRouted {
                entry: e,
                routing: Routing::Answers { question: uq },
                reason: None,
            });
            ex.emit(Ev::UqAnswered { q: uq, by: e });
            match source {
                UqSource::Escalation { question } => answer_pq(ex, question, e),
                // Answering the refusal question clears the node's refusals (A-56).
                UqSource::Refusal { node } => ex.emit(Ev::RefusalsCleared { node, by: e }),
                UqSource::Agent { .. } => {}
            }
            RoutedReply::Answers { question: uq }
        }
        Route::NotPending(uq) => RoutedReply::Unrouted(UnroutedWhy::NotPending { question: uq }),
        Route::Clarifies(r) => {
            ex.emit(Ev::EntryRouted {
                entry: e,
                routing: Routing::Clarifies { request: r },
                reason: None,
            });
            RoutedReply::Clarifies { request: r }
        }
        Route::Request { node, follows } => {
            route_to(ex, e, node);
            routed_request(ex.s(), e, follows)
        }
        Route::NoAction(reason) => {
            ex.emit(Ev::EntryRouted {
                entry: e,
                routing: Routing::NoAction,
                reason: Some(reason),
            });
            RoutedReply::NoAction
        }
        Route::NoTarget(holds) => RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds }),
    }
}

/// Executes `talk from_user`.
pub(crate) fn from_user_exec(ex: &mut Exec<'_>, plan: FromUserPlan) -> Reply {
    let r = run_from_user(ex, plan);
    Reply::FromUser {
        entry: r.entry,
        key: r.key,
        routed: r.routed,
        principal: r.principal,
    }
}

/// Executes the `from_user` of `session hello`, after its `SessionOpened` or `SessionHello` and
/// before its `InboxRead` (D.8.24).
pub(crate) fn hello_from_user_exec(ex: &mut Exec<'_>, plan: FromUserPlan) -> UserEntryReply {
    run_from_user(ex, plan)
}

// ------------------------------------------------------------------------------------------------
// to_user (D.8.11)

/// The plan of `talk to_user` (D.8.11).
#[derive(Debug)]
pub(crate) struct ToUserPlan {
    /// To whom.
    principal: PrincipalRef,
    /// The report.
    text: EntryText,
    /// Its kind.
    kind: ToUserKind,
    /// Whether the text is what the human sees.
    verbatim: bool,
    /// The requests, user entries, and digest entries it reports on.
    re: BTreeSet<Eid>,
    /// 1.8 (§8.1): the requests `at` named, when the call gave no `re`; 1.9: those a `result`
    /// with neither `re` nor `at` named on the nodes the session holds.
    filled: Vec<RequestId>,
    /// 1.8: the node of an `at` that named no request of the addressee's person, ignored.
    at_ignored: Option<NodeId>,
    /// 1.8 (§21): the report goes to `user`, whom nobody will read (`no_human_in_reach`).
    unread: bool,
}

/// The unsettled user requests on `n`, oldest first, with their requesters: at most
/// `query::SCAN_MAX` of the node's unsettled requests are read (§18).
fn user_requests_on(s: &State, n: NodeId) -> Vec<(RequestId, PrincipalId)> {
    let max = usize::try_from(crate::query::SCAN_MAX).unwrap_or(usize::MAX);
    s.ix.items
        .get(&n)
        .into_iter()
        .flat_map(|it| it.unsettled.iter().take(max))
        .filter_map(|r| match s.requests.get(r).map(|q| q.requester) {
            Some(Requester::User(p)) => Some((*r, p)),
            Some(Requester::External) | None => None,
        })
        .collect()
}

/// How many requests a `result` with neither `re` nor `at` names, at most (1.9, §8.1).
pub(crate) const RESULT_FILL_MAX: usize = 5;

/// 1.9 (§8.1): the unsettled user requests of `p`'s person on the nodes session `x` holds, oldest
/// first, at most `RESULT_FILL_MAX`, when `x` serves that person; none otherwise. Each held node's
/// unsettled requests are read as `user_requests_on` reads them (§18).
fn held_requests_of(s: &State, x: Option<SessionId>, p: PrincipalId, now: Time) -> Vec<RequestId> {
    let Some(x) = x else {
        return Vec::new();
    };
    let serves = s
        .sessions
        .get(&x)
        .is_some_and(|y| y.principals.iter().any(|q| same_person(s, *q, p)));
    if !serves {
        return Vec::new();
    }
    let mut out: BTreeSet<RequestId> = BTreeSet::new();
    for t in live_holds_of(s, x, now) {
        out.extend(
            user_requests_on(s, t.node)
                .into_iter()
                .filter(|(_, q)| same_person(s, p, *q))
                .map(|(r, _)| r),
        );
    }
    out.into_iter().take(RESULT_FILL_MAX).collect()
}

/// Validates `talk to_user` (D.8.11): `at` resolves (the target, `not_found(Node)`, 1.8); every
/// `re` id is below `next_eid` (`not_found(Entry)`), then names a request, a `from_user` entry, or a
/// digest entry (`invalid(NotReportable)`; the shell has restored those that left memory, D.4.12),
/// and the principal resolves (`invalid(AmbiguousPrincipal)`): the call's `principal`, else the
/// requester of the oldest unsettled user request on `at` (1.8), else the session's (§8.1). Without
/// `re`, `at` names that person's unsettled user requests there; with none, `at` is ignored. 1.9:
/// a `result` with neither `re` nor `at` names that person's unsettled user requests on the nodes
/// the session holds, when the session serves that person (`held_requests_of`).
pub(crate) fn to_user(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ToUserPlan, Error> {
    let Cmd::Talk(TalkCmd::ToUser {
        text,
        kind,
        verbatim,
        principal,
        re,
        at: _,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let at = target(cx.s, cmd)?;
    let s = cx.s;
    if re.iter().any(|x| x.0 == 0 || *x >= s.counters.next_eid) {
        return Err(Error::not_found(What::Entry));
    }
    let reportable = |x: &Eid| {
        s.requests.contains_key(x) || s.user_entries.contains_key(x) || s.digest.contains_key(x)
    };
    if !re.iter().all(reportable) {
        return Err(Error::invalid(Why::NotReportable));
    }
    let on_at: Vec<(RequestId, PrincipalId)> =
        at.map(|n| user_requests_on(s, n)).unwrap_or_default();
    let principal = match (principal, on_at.first()) {
        (None, Some(&(_, p))) => PrincipalRef::Id(p),
        (given, _) => resolve_principal(s, cx.sess, given.as_ref())?,
    };
    // The requests on `at` of the addressee's person: what `at` names without `re`. A key not
    // interned yet is nobody's requester.
    let theirs: Vec<RequestId> = match &principal {
        PrincipalRef::Id(p) => on_at
            .iter()
            .filter(|(_, q)| same_person(s, *p, *q))
            .map(|(r, _)| *r)
            .collect(),
        PrincipalRef::Key(_) => Vec::new(),
    };
    let at_ignored = at.filter(|_| theirs.is_empty());
    let filled: Vec<RequestId> = match (&principal, at) {
        _ if !re.is_empty() => Vec::new(),
        (_, Some(_)) => theirs,
        (PrincipalRef::Id(p), None) if *kind == ToUserKind::Result => {
            held_requests_of(s, cx.sess, *p, cx.now)
        }
        _ => Vec::new(),
    };
    let mut named = re.clone();
    named.extend(filled.iter().copied());
    let unread = principal == PrincipalRef::Id(USER_PRINCIPAL)
        && serves_none(s, cx.sess)
        && crate::helpers::principals::user_unread(s);
    Ok(ToUserPlan {
        principal,
        text: EntryText {
            main: text.clone(),
            extra: Vec::new(),
        },
        kind: *kind,
        verbatim: *verbatim,
        re: named,
        filled,
        at_ignored,
        unread,
    })
}

/// The requests a `to_user` names in `re`: a request id, or a user entry routed as a request,
/// ascending (D.8.8).
fn named_requests(s: &State, re: &BTreeSet<Eid>) -> BTreeSet<RequestId> {
    re.iter()
        .filter_map(|x| {
            if s.requests.contains_key(x) {
                Some(*x)
            } else {
                match s.user_entries.get(x).map(|u| u.routing) {
                    Some(Routing::Request { request, .. }) => Some(request),
                    Some(
                        Routing::Unrouted
                        | Routing::Clarifies { .. }
                        | Routing::Answers { .. }
                        | Routing::NoAction,
                    )
                    | None => None,
                }
            }
        })
        .collect()
}

/// Executes `talk to_user` (D.8.11): `PrincipalSeen` if the principal is new;
/// `Appended(ToUser)` on `@user`, eid `a`; `DigestShown { d, a }` for each pending digest entry in
/// `re` whose `to` is the principal or an alias of it, whatever the kind (§8.5, CS-5); for kind
/// `result` only (A-23), `RequestAnswered { r, a }` for each request it names whose requester is the
/// same person and that has no answer yet, each followed by `try_settle(r)` (CS-4). A `result` none
/// of whose named requests is that person's (it named none, or each one's requester is another
/// principal or outside the swarm) warns `result_answered_nothing` with the requests it named (1.4,
/// §21's three cases), unless it names a `from_user` entry of that person, to which it replies
/// (1.5, §21: rule 5's reply to an entry that needs no request). 1.9 (§8.2): it routes none of the
/// `from_user` entries it names, which 1.7 routed to none: only the agent knows whether the report
/// answered all of their words. 1.8 (§8.1, §21): an `at` that named no request of
/// the addressee's person warns `ignored_arguments`, and a report to `user` that nobody will read
/// warns `no_human_in_reach`.
pub(crate) fn to_user_exec(ex: &mut Exec<'_>, plan: ToUserPlan) -> Reply {
    if let Some(node) = plan.at_ignored {
        ex.warn(Warning {
            code: WarningCode::IgnoredArguments,
            id: Some(node),
            detail: WarningDetail::AtIgnored { node },
        });
    }
    if plan.unread {
        ex.warn(Warning {
            code: WarningCode::NoHumanInReach,
            id: None,
            detail: WarningDetail::NoHumanInReach {
                due: None,
                default: false,
            },
        });
    }
    let p = intern_ref(ex, &plan.principal);
    let by = ex.author();
    let a = ex.append(
        USER,
        by,
        EntryBody::ToUser {
            kind: plan.kind,
            verbatim: plan.verbatim,
            re: plan.re.clone(),
            principal: p,
        },
        plan.text,
    );
    let shown: Vec<DigestId> =
        plan.re
            .iter()
            .copied()
            .filter(|d| {
                ex.s().digest.get(d).is_some_and(|x| {
                    x.state == DigestState::Pending && same_person(ex.s(), x.to, p)
                })
            })
            .collect();
    for &d in &shown {
        ex.emit(Ev::DigestShown { entry: d, by: a });
    }
    let mut settled: Vec<RequestId> = Vec::new();
    if plan.kind == ToUserKind::Result {
        let named = named_requests(ex.s(), &plan.re);
        // Whether a named request is the principal's own (its requester the same person), answered
        // now or by an earlier report.
        let mut own = false;
        for &r in &named {
            let (mine, open) = ex.s().requests.get(&r).map_or((false, false), |x| {
                (
                    matches!(x.requester, Requester::User(q) if same_person(ex.s(), p, q)),
                    x.answered_by.is_none() && r < a,
                )
            });
            own |= mine;
            if !(mine && open) {
                continue;
            }
            ex.emit(Ev::RequestAnswered { request: r, by: a });
            if try_settle(ex, r) {
                settled.push(r);
            }
        }
        // §21's three cases: no request named, or each named request's requester is another
        // principal or outside the swarm. A later result about a request of the same person that
        // an earlier report answered is a report like any other, and does not warn; nor does one
        // that names an entry of the same person, which it replies to (1.5).
        let replies = plan.re.iter().any(|x| {
            ex.s()
                .user_entries
                .get(x)
                .is_some_and(|u| same_person(ex.s(), u.principal, p))
        });
        if !own && !replies {
            ex.warn(Warning {
                code: WarningCode::ResultAnsweredNothing,
                id: None,
                detail: WarningDetail::ResultAnsweredNothing {
                    requests: named.into_iter().collect(),
                },
            });
        }
    }
    Reply::ToUser {
        entry: a,
        principal: p,
        settled,
        shown_digest: shown,
        re: plan.filled,
    }
}

// ------------------------------------------------------------------------------------------------
// route (D.8.11)

/// The plan of `talk route` (D.8.11).
#[derive(Debug)]
pub(crate) enum RoutePlan {
    /// The entry is routed already: its first routing, and nothing else (§8.2).
    Existing {
        /// The entry.
        entry: UserEntryId,
        /// Its first routing.
        routing: Routing,
    },
    /// The routing of an unrouted entry (1.7): by the first of `answers`, `clarifies`, and `to`.
    Route {
        /// The entry.
        entry: UserEntryId,
        /// How.
        route: Route,
    },
}

/// Validates `talk route` (D.8.11, 1.7): the node of `to` resolves (the target,
/// `not_found(Node)`); `entry` exists (`not_found(UserEntry)`), `answers` is a question and
/// `clarifies` a request (`not_found`); `entry` is a `from_user` entry (`invalid(NotUserEntry)`;
/// the shell has restored one that left memory, D.4.12); `answers` is a user question, a `reason`
/// given is not empty, and `to` is not reserved (`invalid`, A-29); one of `answers`, `clarifies`,
/// and `to` is given (`invalid(BadParams)`: the shell refuses its absence first). An entry already
/// routed keeps its first routing (§8.2).
pub(crate) fn route(cx: &Vcx<'_>, cmd: &Cmd) -> Result<RoutePlan, Error> {
    let Cmd::Talk(TalkCmd::Route {
        entry,
        to,
        answers,
        clarifies,
        reason,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    target(cx.s, cmd)?;
    let s = cx.s;
    let to = target_of(s, to.as_ref())?;
    if entry.0 == 0 || *entry >= s.counters.next_eid {
        return Err(Error::not_found(What::UserEntry));
    }
    let args = RouteArgs {
        answers: *answers,
        clarifies: *clarifies,
        to,
        reason: reason.as_ref(),
    };
    route_refs(s, &args)?;
    let Some(u) = s.user_entries.get(entry) else {
        return Err(Error::invalid(Why::NotUserEntry));
    };
    route_params(s, &args)?;
    let Some(route) = route_of(s, &args) else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(match u.routing {
        Routing::Unrouted => RoutePlan::Route {
            entry: *entry,
            route,
        },
        routing @ (Routing::Request { .. }
        | Routing::Clarifies { .. }
        | Routing::Answers { .. }
        | Routing::NoAction) => RoutePlan::Existing {
            entry: *entry,
            routing,
        },
    })
}

/// Executes `talk route` (D.8.11, 1.4 R3, 1.7).
pub(crate) fn route_exec(ex: &mut Exec<'_>, plan: RoutePlan) -> Reply {
    match plan {
        RoutePlan::Existing { entry, routing } => Reply::Routed {
            entry,
            routed: routed_of(routing),
            existing: true,
        },
        RoutePlan::Route { entry, route } => Reply::Routed {
            entry,
            routed: apply_route(ex, entry, route),
            existing: false,
        },
    }
}

// ------------------------------------------------------------------------------------------------
// ask to a human (D.8.12)

/// The plan of `talk ask` to a human the swarm serves (D.8.12).
#[derive(Debug)]
pub(crate) enum AskUserPlan {
    /// The same question is pending about `at` for the same first addressee (`{existing}`).
    Existing(QuestionId),
    /// A new question, and the `user_saturated` warning's principal and count, if any.
    New {
        /// The question to create.
        new: NewUq,
        /// The first addressee and its pending count, past `user_question.max_pending`.
        saturated: Option<(PrincipalId, u32)>,
        /// 1.8 (§8.4, §21): `to: user` with a chain of `user` alone, from a session that serves
        /// no principal, while nobody reads `user` (`no_human_in_reach`).
        unread: bool,
    },
}

/// 1.12 (§8.4): the chain of question for the user `q` when none of its stages is a person some
/// session serves (`served`); None when one is, or when `q` is not resident.
fn unserved_chain(s: &State, q: QuestionId, now: Time) -> Option<Vec<PrincipalId>> {
    let chain = &s.uquestions.get(&q)?.to;
    (!chain.is_empty()
        && !chain
            .iter()
            .any(|&p| crate::helpers::principals::served(s, p, now)))
    .then(|| chain.clone())
}

/// The pending questions addressed now to `p`'s person, under any alias (CS-33).
fn pending_for(s: &State, p: PrincipalId) -> usize {
    let mut qs: BTreeSet<QuestionId> = BTreeSet::new();
    for a in aliases(s, p) {
        qs.extend(
            s.ix.pending_uq_by
                .get(&a)
                .into_iter()
                .flatten()
                .map(|r| r.q),
        );
    }
    qs.len()
}

/// Validates `talk ask {to: user | principal:<p>}` (D.8.12, A-16): `at` resolves (the target,
/// `not_found(Node)`). Unless `again`, a pending `Agent` question about `at` with the same digest
/// and the same first addressee (the same person) is returned as `{existing}`; otherwise a new
/// question, with `user_saturated` when the first addressee already has more than
/// `user_question.max_pending` pending questions (counted before this one, CS-33). The human form
/// carries no `blocking` and no `where` (1.4, §20.7, R27), and `options` may be empty: an open
/// question.
pub(crate) fn ask_user(cx: &Vcx<'_>, cmd: &Cmd) -> Result<AskUserPlan, Error> {
    let Cmd::Talk(TalkCmd::Ask {
        text,
        to: AskTo::User(ask),
        again,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let at = target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let session = cx.sess.ok_or(Error::invalid(Why::BadParams))?;
    let s = cx.s;
    let explicit = ask.to.as_ref().map(|k| principal_ref(s, k));
    // A key not interned yet is a principal of its own, the chain's first stage: no pending
    // question has it as its first addressee, and it has none pending.
    let first = match &explicit {
        Some(PrincipalRef::Key(_)) => None,
        Some(PrincipalRef::Id(p)) => chain(s, Some(at), Some(*p), false).first().copied(),
        None => chain(s, Some(at), None, false).first().copied(),
    };
    // 1.8: a chain of `user` alone has `user` as its first addressee (U1: `user` is always last).
    let unread = explicit.is_none()
        && first == Some(crate::types::USER_PRINCIPAL)
        && crate::helpers::principals::serves_none(s, cx.sess)
        && crate::helpers::principals::user_unread(s);
    if !again
        && let Some(f) = first
        && let Some(q) = s
            .ix
            .items
            .get(&at)
            .into_iter()
            .flat_map(|i| i.uquestions.iter())
            .copied()
            .find(|q| {
                s.uquestions.get(q).is_some_and(|u| {
                    u.state == UqState::Pending
                        && matches!(u.source, UqSource::Agent { digest, .. } if digest == ask.digest)
                        && u.to.first().is_some_and(|&t| same_person(s, t, f))
                })
            })
    {
        return Ok(AskUserPlan::Existing(q));
    }
    let saturated = first.and_then(|f| {
        let pending = pending_for(s, f);
        let max = usize::try_from(s.params.user_question_max_pending).unwrap_or(usize::MAX);
        (pending > max).then(|| (f, u32::try_from(pending).unwrap_or(u32::MAX)))
    });
    let mut extra: Vec<(TextField, Text)> = ask
        .options
        .iter()
        .map(|o| (TextField::Option, o.clone()))
        .collect();
    extra.extend(
        ask.recommendation
            .iter()
            .map(|r| (TextField::Recommendation, r.clone())),
    );
    extra.extend(ask.default.iter().map(|d| (TextField::Default, d.clone())));
    Ok(AskUserPlan::New {
        new: NewUq {
            source: UqSource::Agent {
                session,
                digest: ask.digest,
            },
            about: Some(at),
            has_default: ask.default.is_some(),
            explicit_to: ask.to.clone(),
            explicit_due: ask.due,
            text: EntryText {
                main: text.clone(),
                extra,
            },
        },
        saturated,
        unread,
    })
}

/// Executes `talk ask` to a human the swarm serves (D.8.12): `create_uq` (with `PrincipalSeen` for
/// a new explicit principal), and the `user_saturated` warning; or nothing for `{existing}`.
pub(crate) fn ask_user_exec(ex: &mut Exec<'_>, plan: AskUserPlan) -> Reply {
    match plan {
        AskUserPlan::Existing(question) => Reply::Asked {
            question,
            said: None,
            node: USER_DECISIONS,
            existing: true,
            keyed: None,
        },
        AskUserPlan::New {
            new,
            saturated,
            unread,
        } => {
            let question = create_uq(ex, new);
            if let Some((principal, pending)) = saturated {
                ex.warn(Warning {
                    code: WarningCode::UserSaturated,
                    id: None,
                    detail: WarningDetail::UserSaturated { principal, pending },
                });
            }
            if unread {
                let q = ex
                    .s()
                    .uquestions
                    .get(&question)
                    .map(|u| (u.due, u.has_default));
                ex.warn(Warning {
                    code: WarningCode::NoHumanInReach,
                    id: None,
                    detail: WarningDetail::NoHumanInReach {
                        due: q.map(|(due, _)| due),
                        default: q.is_some_and(|(_, d)| d),
                    },
                });
            } else if let Some(chain) = unserved_chain(ex.s(), question, ex.now()) {
                // 1.12 (§8.4): no stage of the chain is a person some session serves, so nobody
                // will put the question to them. In the lab's seventh round (it6 l1), a question
                // for a commit's author, whom no session served, was filed with no warning.
                ex.warn(Warning {
                    code: WarningCode::NoHumanInReach,
                    id: None,
                    detail: WarningDetail::NobodyServes { question, chain },
                });
            }
            Reply::Asked {
                question,
                said: None,
                node: USER_DECISIONS,
                existing: false,
                keyed: None,
            }
        }
    }
}

#[cfg(test)]
#[path = "talk_user_tests.rs"]
mod tests;
