//! The body of a response: one `Reply` as compact JSON (§20.3 to §20.8, design D.6.2, D.11.7).
//!
//! A reply carries ids and decided values; the body adds what the agent reads next from the
//! post-state (a brief, a list) and from the shell's tables (texts). Where §20 gives a reply's
//! shape (`take → {token, lease_until, brief}`, `next → {target, token, brief, why_this}`, …) the
//! body has those fields in that order; the other replies name their `Reply` fields.
//!
//! The 1.4 fields of D.6.2 come from the replies (WP-D1.4): `routed` (§8.2, R3), with the reason of
//! a routing to none from the call or its event; `reopened: {was: <outcome>}` (§20.6, R4); a
//! checkpoint's `entry` and the intents it opened and closed, and a release's `intent_done`
//! (§20.6, R9, R15); `Said.answered` and `fulfilled`, `Decided.not_settled`, and `Closed.dropped`
//! (§20.5, §20.7, R9); `user_requests` and `user_request` (R3); and `view agreements` (D.14.5).

use forum_core::health::{Health, HealthReason, MoverGap, UnreachableBy};
use forum_core::query::holders;
use forum_core::{
    Addressee, AgreementItem, Cmd, KeyStr, NextReply, NodeId, NotSettledWhy, Outcome, Params,
    Reply, RequestId, SessionId, State, TalkCmd, Time, Total,
};

use super::brief::{render_brief, view_node_after};
use super::format::{
    cut, dur, flags, node_ref, outcome, path, principal, shown_if_cut, time, total_json,
};
use super::hello::{
    HelloInput, RouteCall, SearchHits, empty_why, inbox_item, render_hello, routed, some_hits,
    user_decisions, user_digest, why_this_at,
};
use super::json::Json;
use super::lists::{self, LIST_TITLE_MAX};
use super::source::Source;
use super::status::{PAUSE_REASON_MAX, cand_words, level, needs_of, reason_words};

/// What a body reads beside the reply.
pub struct BodyCtx<'a> {
    /// The state after the call and its post-settle.
    pub state: &'a State,
    /// The call's clamped `now`.
    pub now: Time,
    /// The call's session.
    pub session: Option<SessionId>,
    /// The call.
    pub cmd: &'a Cmd,
    /// The writer's health cache, and when it was computed (D.11.2).
    pub health: &'a Health,
    /// 1.11 (§17): the health as the call's session is shown it, its caps and horizon counted
    /// (`health::counting`): in an empty `why`, and in `hello`'s.
    pub shown_health: &'a Health,
    /// When the health cache was computed.
    pub health_at: Time,
    /// The search hits the handler found.
    pub hits: &'a SearchHits,
    /// Texts and pages.
    pub src: &'a dyn Source,
    /// The call opened its session: a `hello` shows the constitution only then (1.8, §20.3).
    pub opened: bool,
}

/// 1.10 (§11.3, §20.7): `verdict`, `by`, and `upheld` of the resolve that decided an objection,
/// added to `j`; `by` when the entry names a session.
fn verdict_json(c: &BodyCtx<'_>, j: Json, v: forum_core::Verdict) -> Json {
    j.with("verdict", Json::S(v.verdict.render()))
        .with_opt("by", verdict_by(c, v).map(|x| Json::S(x.render())))
        .with("upheld", Json::Bool(v.upheld))
}

/// The session that wrote the resolve of `v`, if the entry names one.
fn verdict_by(c: &BodyCtx<'_>, v: forum_core::Verdict) -> Option<SessionId> {
    match c.src.entry(v.verdict)?.by {
        forum_core::Author::Session(x) => Some(x),
        forum_core::Author::Server => None,
    }
}

/// 1.10 (§11.3, §20.7): a second, opposite verdict, recorded as a new objection, with what it
/// means: in the lab's fifth round (l5), the later of two opposite verdicts was dropped with a bare
/// `{already: true}`.
fn disputed_json(c: &BodyCtx<'_>, d: &forum_core::Disputed, withdrawn: bool) -> Json {
    let s = c.state;
    let by = verdict_by(c, d.earlier)
        .map(|x| format!(" ({})", x.render()))
        .unwrap_or_default();
    let how = if d.earlier.upheld {
        "upheld"
    } else {
        "resolved as not standing"
    };
    let mut why = format!(
        "{} was {how} by {}{by}; your opposite verdict is recorded as the objection {} on {}, \
         offered for review again",
        d.disputes.render(),
        d.earlier.verdict.render(),
        d.objection.render(),
        node_ref(s, d.node)
    );
    if withdrawn {
        why.push_str(&format!(
            "; the correction {} made is withdrawn until a review decides",
            d.earlier.verdict.render()
        ));
    }
    let j = Json::obj()
        .with("objection", Json::S(d.objection.render()))
        .with("node", Json::S(node_ref(s, d.node)))
        .with("disputes", Json::S(d.disputes.render()));
    verdict_json(c, j, d.earlier).with("why", Json::S(why))
}

fn eids<T: Copy>(xs: impl IntoIterator<Item = T>, f: impl Fn(T) -> String) -> Json {
    Json::strs(xs.into_iter().map(f))
}

fn nodes(s: &State, xs: &[NodeId]) -> Json {
    Json::strs(xs.iter().map(|n| node_ref(s, *n)))
}

/// 1.10 (§9.2, §20.7): what the response to a recorded `said` that is not withheld says of sending
/// it: a `said` is taken as sent, and the release or close before the reply that sends it marks it.
pub const SAID_SEND: &str = "post it in this reply; your release or close marks it sent";

/// 1.11 (§9.2, §20.7): the same for a `said` its author recorded holding nothing, confirmed at once
/// with its reply, since no release or close of that session will come. In the lab's sixth round
/// (l6), such a reply stayed pending in the holder's brief for good.
pub const SAID_SENT_NOW: &str = "post it in this reply; you hold nothing, so it counts as sent now";

/// 1.11 (§9.2): whether session `x` holds no live hold at `now`, as when its `said` was confirmed
/// at once.
fn holds_nothing(s: &State, x: SessionId, now: Time) -> bool {
    !s.holds
        .values()
        .any(|h| h.session == x && now < h.lease_until)
}

/// 1.11 (§9.2, §20.7): what a `said` that answered requests says of them, after "e4 now reads as
/// answered": a post that only says the work is under way is an `ack`, which answers nothing. In
/// the lab's sixth round (it5 l1, as it3 l1), "I'll compare …" recorded as a `reply` answered its
/// request at once, and a close was suggested while the work went on.
pub const SAID_ONLY_ON_IT: &str = "if this was only 'on it', such posts are kind ack";

/// 1.12 (§9.1, §20.7): `conversation_node` of a `said`, `heard`, or `ask` of a person that gave
/// `node` the conversation key `k` (case 3): "#44 now carries chat:x/T1: the thread's later
/// messages join #44". In the lab's seventh round (it6 l3), a chat worker's first `said` made a
/// node the thread's conversation node, and the response did not say so.
fn conversation_node(s: &State, node: NodeId, k: Option<&KeyStr>) -> Option<Json> {
    let k = k?;
    let what = match k.scheme() {
        "chat" => "the thread's",
        "mail" => "the mail's",
        "issue" => "the issue's",
        _ => "the conversation's",
    };
    let n = node_ref(s, node);
    Some(Json::S(format!(
        "{n} now carries {k}: {what} later messages join {n}"
    )))
}

/// 1.11 (§9.2, §20.7): `answered_note` of a `said` that answered requests it did not settle: "e4
/// now reads as answered: your close settles it; if this was only 'on it', such posts are kind
/// ack", the close named only when the caller holds every such request's node; None when it
/// answered none, or settled every one it answered.
fn answered_note(c: &BodyCtx<'_>, answered: &[RequestId], settled: &[RequestId]) -> Option<String> {
    let s = c.state;
    let open: Vec<RequestId> = answered
        .iter()
        .copied()
        .filter(|r| !settled.contains(r))
        .collect();
    if open.is_empty() {
        return None;
    }
    let held = c.session.is_some_and(|x| {
        open.iter().all(|r| {
            s.requests.get(r).is_some_and(|q| {
                holders(s, q.node, c.now)
                    .iter()
                    .any(|(holder, _)| *holder == x)
            })
        })
    });
    let ids = open
        .iter()
        .map(|r| r.render())
        .collect::<Vec<_>>()
        .join(", ");
    let (reads, it) = if open.len() == 1 {
        ("reads", "it")
    } else {
        ("read", "them")
    };
    let close = if held {
        format!(": your close settles {it}")
    } else {
        String::new()
    };
    Some(format!(
        "{ids} now {reads} as answered{close}; {SAID_ONLY_ON_IT}"
    ))
}

/// The bytes of an outside wait's text that a release's `waiting` quotes (1.12, §20.6).
pub const WAIT_TEXT_MAX: usize = 80;

/// 1.12 (§10.3, §20.6): what a release with `waits_for` says of the wait it recorded, and when the
/// node is offered again: "for 'PR #413 merged', outside the forum: offered again at its recheck,
/// 2026-10-05T08:27:00Z, to whoever takes it then"; None for a release without `waits_for`, or a
/// node that does not wait. In the lab's seventh round (it6 l7), eight blocked releases answered
/// only `{leave_better: []}`, and a worker whose `recheck_after` was a bare `2h` never saw the
/// time it meant.
fn released_wait(c: &BodyCtx<'_>) -> Option<String> {
    let Cmd::Hold(forum_core::HoldCmd::Release {
        token,
        waits_for: Some(arg),
        ..
    }) = c.cmd
    else {
        return None;
    };
    let s = c.state;
    let w = s.nodes.get(&token.node)?.wait.as_ref()?;
    Some(match w.on {
        forum_core::WaitOn::External(t) => {
            let what = match arg {
                forum_core::WaitArg::External { what, .. } => {
                    format!("'{}'", cut(&what.0, WAIT_TEXT_MAX))
                }
                _ => "something".to_string(),
            };
            format!(
                "for {what}, outside the forum: offered again at its recheck, {}, to whoever \
                 takes it then",
                time(t)
            )
        }
        forum_core::WaitOn::Time(t) => {
            format!("until {}: offered again then, to whoever takes it", time(t))
        }
        forum_core::WaitOn::Node(n) => format!(
            "for {} to close: offered again when it closes",
            node_ref(s, n)
        ),
        forum_core::WaitOn::Question { q, .. } => format!(
            "for an answer to {}: offered again when it is answered, or at its recheck",
            q.render()
        ),
        forum_core::WaitOn::UserQuestion(q) => format!(
            "for the human's answer to {}: offered again once it is answered, takes its default, \
             or lapses",
            q.render()
        ),
    })
}

/// 1.12 (§9.4, §20.7): the `not_settled` of a `to_user` that answered requests without settling
/// them, as a decision's: each such request, ascending, with why, "answered; your close of #N
/// settles it, or release blocked with settles if it waits for its merge" when the caller holds its
/// node; "answered; #N is closed, so a decide that lists it in settles, settles it" when its node is
/// closed, since closing a closed node is `invalid` (§5); else "answered; a close of #N, or a
/// decide that lists it in settles, settles it". The requests read are those the call named in
/// `re`, a request or a user entry routed as one, and those `at` or the session's holds filled in
/// (`filled`): a request whose answer is `entry` and that has not settled. In the lab's seventh
/// round (it6 l1, l4), the empty `settled` of a result that answered its request read as the
/// report having failed, and nothing said a close or a decision settles it.
fn answered_unsettled(c: &BodyCtx<'_>, entry: forum_core::Eid, filled: &[RequestId]) -> Vec<Json> {
    let s = c.state;
    let named: Vec<forum_core::Eid> = match c.cmd {
        Cmd::Talk(TalkCmd::ToUser { re, .. }) => re.iter().copied().collect(),
        _ => Vec::new(),
    };
    let mut requests: std::collections::BTreeSet<RequestId> = filled.iter().copied().collect();
    for x in named {
        if s.requests.contains_key(&x) {
            requests.insert(x);
        } else if let Some(forum_core::Routing::Request { request, .. }) =
            s.user_entries.get(&x).map(|u| u.routing)
        {
            requests.insert(request);
        }
    }
    requests
        .into_iter()
        .filter_map(|r| {
            let q = s.requests.get(&r)?;
            if q.answered_by != Some(entry) || q.settled.is_some() {
                return None;
            }
            let held = c.session.is_some_and(|x| {
                holders(s, q.node, c.now)
                    .iter()
                    .any(|(holder, _)| *holder == x)
            });
            let why = if held {
                format!(
                    "answered; your close of {} settles it, or release blocked with settles if it \
                     waits for its merge",
                    node_ref(s, q.node)
                )
            } else if !s.nodes.get(&q.node).is_some_and(forum_core::Node::is_open) {
                format!(
                    "answered; {} is closed, so a decide that lists it in settles, settles it",
                    node_ref(s, q.node)
                )
            } else {
                format!(
                    "answered; a close of {}, or a decide that lists it in settles, settles it",
                    node_ref(s, q.node)
                )
            };
            Some(
                Json::obj()
                    .with("request", Json::S(r.render()))
                    .with("why", Json::S(why)),
            )
        })
        .collect()
}

/// 1.11 (§11.3, §20.7): what a deferral says, with the end of the class's backoff from the review
/// of a closed node, or, on an open node, that its holder decides it.
fn deferred_json(d: forum_core::Deferred) -> Json {
    match d.until {
        Some(u) => Json::obj().with("until", Json::S(time(u))).with(
            "why",
            Json::S(format!(
                "recorded: the objection stays open, and its review is not offered to your class \
                 until {} or another verdict",
                time(u)
            )),
        ),
        None => Json::obj().with(
            "why",
            Json::s("recorded: the objection stays open on its open node, whose holder decides it"),
        ),
    }
}

/// 1.8 (§10.3): what `leave_better` asks of the stale offers it lists, one line each, beside the
/// list, which stays a list of nodes; None when it lists none. 1.10: its owners are asked, where
/// 1.8's "or refresh it with a checkpoint" let a worker refresh offers with no news.
pub const STALE_ASKS: &str = "stale offer: ask its owners if it is still wanted (talk ask {to: \
                              user}), or close it as dropped if you know it is not";

fn stale_of(s: &State, xs: &[NodeId]) -> Option<Json> {
    let lines: Vec<String> = xs
        .iter()
        .filter(|n| {
            s.ix.node
                .get(n)
                .is_some_and(|x| x.flags.contains(forum_core::Flag::Stale))
        })
        .map(|n| format!("{}: {STALE_ASKS}", node_ref(s, *n)))
        .collect();
    (!lines.is_empty()).then(|| Json::strs(lines))
}

/// The parameters as §22 names them.
#[must_use]
pub fn params(s: &State) -> Json {
    params_of(&s.params)
}

/// The parameters `p` as §22 names them (`view constitution`, and the events that carry them).
#[must_use]
pub fn params_of(p: &Params) -> Json {
    let d = |x: forum_core::Dur| Json::S(dur(x));
    Json::obj()
        .with("lease.first", d(p.lease_first))
        .with("lease.default", d(p.lease_default))
        .with("lease.max", d(p.lease_max))
        .with("lease.ceiling", d(p.lease_ceiling))
        .with("renew.max", Json::from(p.renew_max))
        .with("hold.per_session", Json::from(p.hold_per_session))
        .with("presence.silence", d(p.presence_silence))
        .with("presence.resolution", d(p.presence_resolution))
        .with("session.retain", d(p.session_retain))
        .with("op.retain", d(p.op_retain))
        .with(
            "conversation.schemes",
            Json::strs(p.conversation_schemes.iter().map(ToString::to_string)),
        )
        .with("commitment.followup", d(p.commitment_followup))
        .with("question.recheck", d(p.question_recheck))
        .with("question.max_rechecks", Json::from(p.question_max_rechecks))
        .with("user_question.due", d(p.user_question_due))
        .with("user_question.due_absent", d(p.user_question_due_absent))
        .with(
            "user_question.max_pending",
            Json::from(p.user_question_max_pending),
        )
        .with("user_gap.window", d(p.user_gap_window))
        .with("deliver.window", d(p.deliver_window))
        .with("talk.wait_max", d(p.talk_wait_max))
        .with("hello.budget", Json::from(p.hello_budget))
        .with("brief.budget", Json::from(p.brief_budget))
        .with("digest.max", Json::from(p.digest_max))
        .with("leave_better.max", Json::from(p.leave_better_max))
        .with_opt("offer.stale", p.offer_stale.map(d))
        .with("refusal.ttl", d(p.refusal_ttl))
        .with("refusal.window", d(p.refusal_window))
        .with("refusal.backoff", d(p.refusal_backoff))
        .with("refusal.quorum", Json::from(p.refusal_quorum))
        .with("too_big.attempts", Json::from(p.too_big_attempts))
        .with("pause.ttl", d(p.pause_ttl))
        .with("archive.after", d(p.archive_after))
        .with("coverage.window", d(p.coverage_window))
        .with("capacity.window", d(p.capacity_window))
        .with("write.per_hour", Json::from(p.write_per_hour))
        .with("queue.max_wait", d(p.queue_max_wait))
        .with("text.max", Json::from(p.text_max))
        .with("tree.max_depth", Json::from(p.tree_max_depth))
        .with("tree.max_children", Json::from(p.tree_max_children))
        .with("settle.batch", Json::from(p.settle_batch))
        .with("instructions.max", Json::from(p.instructions_max))
}

/// The constitution and the parameters (`view constitution`, `control constitution`).
#[must_use]
pub fn constitution(s: &State) -> Json {
    Json::obj()
        .with("constitution", Json::strs(super::hello::CONSTITUTION))
        .with("params", params(s))
}

/// `control health`: the whole cache, with when it was computed (§17, D.8.22). Each reason that
/// holds comes with its level, a few words, its count, and its first items (at most `HEALTH_TOP`)
/// in the shell's formats (§17, D.11.7).
#[must_use]
pub fn health(s: &State, h: &Health, at: Time, src: &dyn Source) -> Json {
    let reasons = h.reasons.iter().map(|r| health_reason(s, r, src)).collect();
    let c = &h.capacity;
    Json::obj()
        .with("level", Json::s(level(h.level)))
        .with("reasons", Json::A(reasons))
        .with(
            "capacity",
            Json::obj()
                .with("demand_min", Json::U(c.demand_min))
                .with("supply_min", Json::U(c.supply_min))
                .with("lost_min", Json::U(c.lost_min))
                .with("next_calls", Json::from(c.next_calls))
                .with("next_empty", Json::from(c.next_empty))
                .with("next_empty_for_you", Json::from(c.next_empty_for_you))
                .with_opt("idle_permille", c.idle_permille.map(Json::from))
                .with("backlog", Json::from(c.backlog))
                .with("flagged_active", Json::from(c.flagged_active))
                .with("median_done_min", Json::U(c.median_done_min)),
        )
        .with("computed_at", Json::S(time(at)))
}

/// One reason of `control health`: `{level, reason, count?, items?, …}`, its items in the shell's
/// formats: paths or `#id`, `s-N`, RFC 3339 times, `<class>/s-N`, and `excludes k`.
fn health_reason(s: &State, r: &HealthReason, src: &dyn Source) -> Json {
    let j = Json::obj()
        .with("level", Json::s(level(r.level())))
        .with("reason", Json::S(reason_words(s, r)));
    let node = |n: NodeId| Json::S(node_ref(s, n));
    match r {
        HealthReason::Paused { count, top, .. } => {
            let items = top
                .iter()
                .map(|p| {
                    let reason = src
                        .pause_reason(p.pause.scope)
                        .map(|t| cut(&t.0, PAUSE_REASON_MAX))
                        .unwrap_or_default();
                    Json::obj()
                        .with("scope", node(p.pause.scope))
                        .with(
                            "by",
                            Json::S(format!("{}/{}", p.pause.class, p.pause.by.render())),
                        )
                        .with("until", Json::S(time(p.pause.until)))
                        .with("reason", Json::S(reason))
                        .with("excludes", Json::from(p.excluded))
                })
                .collect();
            j.with("count", Json::from(*count))
                .with("items", Json::A(items))
        }
        HealthReason::NoMover { count, top, .. } => {
            let items = top
                .iter()
                .map(|g| match g {
                    MoverGap::NodeWait { node: n, target } => Json::obj()
                        .with("node", node(*n))
                        .with("waits_for", node(*target)),
                    MoverGap::Dependency { node: n, target } => Json::obj()
                        .with("node", node(*n))
                        .with("depends_on", node(*target)),
                    MoverGap::FlaggedUnreachable { node: n, flags: f } => {
                        Json::obj().with("node", node(*n)).with("flags", flags(*f))
                    }
                })
                .collect();
            j.with("count", Json::from(*count))
                .with("items", Json::A(items))
        }
        HealthReason::Unreachable {
            count,
            by_needs,
            by_horizon,
            top,
            ..
        } => {
            // 1.6 (the lab's first round): each item names the value that keeps every session
            // from it, its needs or its `est`.
            let items = top
                .iter()
                .map(|(n, by)| {
                    let item = Json::obj().with("node", node(*n));
                    match by {
                        UnreachableBy::Needs => item
                            .with("by", Json::s("needs"))
                            .with("needs", Json::strs(needs_of(s, *n))),
                        UnreachableBy::Horizon => item
                            .with("by", Json::s("horizon"))
                            .with_opt("est", s.nodes.get(n).and_then(|x| x.est).map(Json::from)),
                        // 1.12 (§17): the caller's horizon.
                        UnreachableBy::Yours { horizon } => item
                            .with("by", Json::s("your_horizon"))
                            .with_opt("est", s.nodes.get(n).and_then(|x| x.est).map(Json::from))
                            .with("horizon", Json::from(*horizon)),
                    }
                })
                .collect();
            j.with("count", Json::from(*count))
                .with("by_needs", Json::from(*by_needs))
                .with("by_horizon", Json::from(*by_horizon))
                .with("items", Json::A(items))
        }
        HealthReason::FlagsPilingUp { start, now } => j
            .with("count", Json::from(*now))
            .with("start", Json::from(*start)),
        HealthReason::Overloaded {
            demand_min,
            supply_min,
            idle_permille,
            oldest,
            ..
        } => j
            .with("demand_min", Json::U(*demand_min))
            .with("supply_min", Json::U(*supply_min))
            .with("idle_permille", Json::from(*idle_permille))
            .with_opt("oldest", oldest.map(|c| Json::S(cand_words(s, c)))),
        HealthReason::WriteSurge {
            class,
            last_hour,
            baseline,
        } => j
            .with("class", Json::S(class.to_string()))
            .with("count", Json::from(*last_hour))
            .with("baseline", Json::from(*baseline)),
        HealthReason::Underused { idle_permille } => {
            j.with("idle_permille", Json::from(*idle_permille))
        }
    }
}

/// `reopened: {was: <outcome>}` of a take that reopened a closed node (1.4, §20.6, R4).
fn reopened_was(was: &Outcome) -> Json {
    Json::obj().with("was", Json::S(outcome(was)))
}

/// A list of requests a call recorded (1.4, R3), or None when it recorded none.
fn requests_opt(rs: &[RequestId]) -> Option<Json> {
    (!rs.is_empty()).then(|| eids(rs.iter().copied(), |r| r.render()))
}

/// Why a request a decision lists did not settle (1.4, §20.7, R9): settled before, or the answer it
/// still awaits, named by key.
fn not_settled_why(s: &State, w: &NotSettledWhy) -> String {
    match w {
        NotSettledWhy::SettledBefore => "settled before".into(),
        NotSettledWhy::AwaitsResult { requester } => format!(
            "awaits a to_user of kind result to its requester, {}",
            principal(s, *requester)
        ),
        NotSettledWhy::AwaitsSaid { node, conv } => format!(
            "awaits a said of kind reply, result, or correction in its conversation {conv} on {}",
            node_ref(s, *node)
        ),
        // 1.7 (§5): answered, but only a result answer settles by default.
        NotSettledWhy::AwaitsDecision => {
            "answered, awaits a decision: name it in settles, or talk decide".into()
        }
    }
}

/// The `not_settled` list of a decision or a close (1.4, R9; 1.7, §5).
fn not_settled_list(s: &State, not_settled: &[forum_core::NotSettled]) -> Json {
    Json::A(
        not_settled
            .iter()
            .map(|x| {
                Json::obj()
                    .with("request", Json::S(x.request.render()))
                    .with("why", Json::S(not_settled_why(s, &x.why)))
            })
            .collect(),
    )
}

/// `view agreements` (1.4, §20.4, D.14.5): `{items: [{path, text, approvals?, changed?}], shown?,
/// total?}`, each item a node with agreements, its path, the agreements' text, the approvals of its
/// current content when there are any, and when its agreements last changed when that is known;
/// `shown` and `total` only for a list cut short (1.7, §20.1).
fn agreements(s: &State, items: &[AgreementItem], total: Total) -> Json {
    let list: Vec<Json> = items
        .iter()
        .map(|x| {
            let node = s.nodes.get(&x.node);
            Json::obj()
                .with("path", Json::S(path(s, x.node)))
                .with(
                    "text",
                    Json::S(node.map(|n| n.agreements.0.clone()).unwrap_or_default()),
                )
                .with_opt(
                    "approvals",
                    (x.approvals > 0).then(|| Json::from(x.approvals)),
                )
                .with_opt("changed", x.changed.map(|t| Json::S(time(t))))
        })
        .collect();
    let n = list.len();
    let j = Json::obj().with("items", Json::A(list));
    if shown_if_cut(n, total).is_some() {
        j.with("shown", Json::U(u64::try_from(n).unwrap_or(u64::MAX)))
            .with("total", total_json(total))
    } else {
        j
    }
}

/// The holders of a node, without tokens.
fn holders_json(s: &State, n: NodeId, now: Time) -> Json {
    Json::A(
        holders(s, n, now)
            .iter()
            .map(|(x, until)| {
                Json::obj()
                    .with("session", Json::S(x.render()))
                    .with("until", Json::S(time(*until)))
            })
            .collect(),
    )
}

/// The brief of `n` for the caller.
fn brief_of(c: &BodyCtx<'_>, n: NodeId) -> Json {
    render_brief(c.state, n, c.session, c.now, c.src).0
}

/// The body of `reply` (§20).
#[must_use]
pub fn body(c: &BodyCtx<'_>, reply: &Reply) -> Json {
    let s = c.state;
    let now = c.now;
    match reply {
        Reply::Hello {
            session,
            user_entry,
            inbox,
        } => render_hello(
            s,
            *session,
            now,
            c.shown_health,
            &HelloInput {
                delivered: Some(inbox),
                user_entry: user_entry.as_ref(),
                hits: c.hits,
                route: &RouteCall::of(c.cmd, false),
                first: c.opened,
            },
            c.src,
        ),
        Reply::Status => Json::obj().with("recorded", Json::Bool(true)),
        Reply::ViewNodes {
            nodes: ns,
            view,
            page,
        } => {
            // 1.7 (§10.4): each agreement text once in the response.
            let mut printed = std::collections::BTreeSet::new();
            let views: Vec<Json> = ns
                .iter()
                .map(|n| view_node_after(s, *n, *view, *page, c.session, now, c.src, &mut printed))
                .collect();
            match views.as_slice() {
                [one] => one.clone(),
                _ => Json::obj().with("nodes", Json::A(views)),
            }
        }
        Reply::ViewFind { by } => lists::find(s, by, c.hits, c.session, c.src),
        Reply::ViewFeed {
            node,
            after,
            upto,
            types,
            limit,
        } => lists::feed(s, *node, *after, *upto, types, *limit, c.src),
        Reply::ViewList { nodes: ns, total } => lists::list(
            s,
            ns,
            *total,
            lists::list_under(s, c.cmd),
            now,
            c.session,
            c.src,
        ),
        Reply::ViewHistory {
            node,
            before,
            limit,
        } => lists::history(s, *node, *before, *limit, c.src),
        Reply::ViewRequests { status, scope } => lists::requests(s, *status, scope, now, c.src),
        Reply::ViewUserLog { since, limit } => lists::user_log(s, *since, *limit, c.src),
        Reply::ViewConstitution | Reply::Constitution => constitution(s),
        Reply::Agreements { items, total } => agreements(s, items, *total),
        Reply::Created {
            id,
            token,
            lease_until,
            continues,
            request,
            user_requests,
        } => Json::obj()
            .with("id", Json::S(id.render()))
            .with("path", Json::S(path(s, *id)))
            .with_opt("token", token.map(|t| Json::S(t.render())))
            .with_opt("lease_until", lease_until.map(|t| Json::S(time(t))))
            .with("brief", brief_of(c, *id))
            .with_opt(
                "related",
                some_hits(
                    s,
                    &c.hits.related,
                    &c.hits.matched,
                    LIST_TITLE_MAX,
                    c.session,
                    c.src,
                ),
            )
            .with_opt(
                "continues",
                (!continues.is_empty()).then(|| nodes(s, continues)),
            )
            .with_opt("request", request.map(|r| Json::S(r.render())))
            .with_opt("user_requests", requests_opt(user_requests)),
        Reply::Joined {
            existing,
            holders: hs,
            open_question,
            added_keys,
            request,
            user_requests,
        } => Json::obj()
            .with("existing", Json::S(node_ref(s, *existing)))
            .with("holder", eids(hs.iter().copied(), |x| x.render()))
            .with_opt("open_question", open_question.map(|q| Json::S(q.render())))
            .with(
                "added_keys",
                Json::strs(added_keys.iter().map(ToString::to_string)),
            )
            .with_opt("request", request.map(|r| Json::S(r.render())))
            .with_opt("user_requests", requests_opt(user_requests)),
        Reply::Updated {
            version,
            digest_version,
        } => Json::obj()
            .with("version", Json::U(version.0))
            .with("digest_version", Json::U(digest_version.0)),
        Reply::Linked { existing, signal } => Json::obj()
            .with("existing", Json::Bool(*existing))
            .with_opt("signal", signal.map(|n| Json::S(node_ref(s, n)))),
        Reply::KeyAdded {
            already,
            user_request,
        } => Json::obj()
            .with("already", Json::Bool(*already))
            .with_opt("user_request", user_request.map(|r| Json::S(r.render()))),
        Reply::KeyExisting { existing, signal } => Json::obj()
            .with("existing", Json::S(node_ref(s, *existing)))
            .with_opt("signal", signal.map(|n| Json::S(node_ref(s, n)))),
        Reply::KeyRemoved => Json::obj().with("removed", Json::Bool(true)),
        Reply::Closed {
            node,
            decision,
            settled,
            not_settled,
            dropped,
            leave_better,
        } => Json::obj()
            .with("closed", Json::S(node_ref(s, *node)))
            .with_opt("decision", decision.map(|d| Json::S(d.render())))
            .with("settled", eids(settled.iter().copied(), |r| r.render()))
            .with_opt(
                "not_settled",
                (!not_settled.is_empty()).then(|| not_settled_list(s, not_settled)),
            )
            .with_opt("dropped", dropped.map(|o| Json::S(node_ref(s, o))))
            .with("leave_better", nodes(s, leave_better))
            .with_opt("stale", stale_of(s, leave_better)),
        // 1.12 (§16): a reopen tags the node offer, and says so; in the lab's seventh round (it6
        // l5), four reopened claims were offered to nobody.
        Reply::Reopened {
            node,
            dropped_keys,
            offered,
        } => Json::obj()
            .with("reopened", Json::S(node_ref(s, *node)))
            .with(
                "dropped_keys",
                Json::strs(dropped_keys.iter().map(ToString::to_string)),
            )
            .with_opt(
                "offered",
                offered
                    .then(|| Json::S(format!("tagged offer: next offers {}", node_ref(s, *node)))),
            ),
        Reply::Moved { node } => Json::obj().with("moved", Json::S(path(s, *node))),
        Reply::Merged { from, into } => Json::obj()
            .with("merged", Json::S(from.render()))
            .with("into", Json::S(node_ref(s, *into))),
        Reply::Hidden => Json::obj().with("hidden", Json::Bool(true)),
        Reply::Taken {
            node,
            token,
            lease_until,
            reopened,
            existing,
        } => {
            let mut j = Json::obj()
                .with("token", Json::S(token.render()))
                .with("lease_until", Json::S(time(*lease_until)))
                .with("brief", brief_of(c, *node));
            if let Some(was) = reopened {
                j.put("reopened", reopened_was(was));
            }
            if *existing {
                j.put("existing", Json::Bool(true));
            }
            j
        }
        Reply::Next(n) => next(c, n),
        Reply::Checkpointed {
            entry,
            lease_until,
            intent,
            intent_done,
        } => Json::obj()
            .with("entry", Json::S(entry.render()))
            .with("lease_until", Json::S(time(*lease_until)))
            .with_opt("intent", intent.map(|e| Json::S(e.render())))
            .with_opt("intent_done", intent_done.map(|e| Json::S(e.render()))),
        Reply::Renewed {
            lease_until,
            renews_left,
        } => Json::obj()
            .with("lease_until", Json::S(time(*lease_until)))
            .with("renews_left", Json::from(*renews_left)),
        // 1.12 (§10.3, §20.6): a release with `settles` returns its decision, as `decide` does.
        Reply::Released {
            intent_done,
            leave_better,
            decision,
            settled,
            not_settled,
        } => Json::obj()
            .with_opt("intent_done", intent_done.map(|e| Json::S(e.render())))
            .with_opt("waiting", released_wait(c).map(Json::S))
            .with_opt("decision", decision.map(|d| Json::S(d.render())))
            .with_opt(
                "settled",
                decision.map(|_| eids(settled.iter().copied(), |r| r.render())),
            )
            .with_opt(
                "not_settled",
                (!not_settled.is_empty()).then(|| not_settled_list(s, not_settled)),
            )
            .with("leave_better", nodes(s, leave_better))
            .with_opt("stale", stale_of(s, leave_better)),
        Reply::HoldCheck {
            node,
            token_current,
            entry,
            intent_done,
        } => Json::obj()
            .with("node", Json::S(node_ref(s, *node)))
            .with("holders", holders_json(s, *node, now))
            .with_opt("token_current", token_current.map(Json::Bool))
            .with_opt("entry", entry.map(|e| Json::S(e.render())))
            .with_opt("intent_done", intent_done.map(|e| Json::S(e.render()))),
        Reply::FromUser {
            entry,
            key,
            routed: r,
            principal: p,
        } => {
            let call = RouteCall::of(c.cmd, false);
            let mut j = Json::obj()
                .with("entry", Json::S(entry.render()))
                .with("key", Json::S(key.to_string()))
                .with("routed", routed(s, r, *entry, &call, c.src))
                .with("principal", Json::S(principal(s, *p)))
                .with_opt(
                    "related",
                    some_hits(
                        s,
                        &c.hits.related,
                        &c.hits.matched,
                        LIST_TITLE_MAX,
                        c.session,
                        c.src,
                    ),
                );
            if let Some(x) = c.session
                && super::hello::serves_principal(s, x)
            {
                // 1.7 (§20.1): as `hello` shows them, only when there are any.
                let h = forum_core::query::hello(s, x, now);
                let max = usize::try_from(s.params.hello_budget / 16).unwrap_or(256);
                if h.user_decisions.count > 0 {
                    j.put("user_decisions", user_decisions(s, &h, c.src, max));
                }
                if h.user_digest.count > 0 || h.user_digest.older.unwrap_or(0) > 0 {
                    j.put("user_digest", user_digest(s, &h, c.src, max));
                }
            }
            j
        }
        // 1.12 (§20.7): `settled` only when it settled some, and `not_settled` for each request it
        // answered without settling it, as `decide` gives them (`answered_unsettled`).
        Reply::ToUser {
            entry,
            principal: p,
            settled,
            shown_digest,
            re,
        } => {
            let open = answered_unsettled(c, *entry, re);
            Json::obj()
                .with("entry", Json::S(entry.render()))
                .with("principal", Json::S(principal(s, *p)))
                // 1.8 (§8.1): the requests `at` named; 1.9: those a result with neither `re` nor
                // `at` named on the nodes the session holds.
                .with_opt(
                    "re",
                    (!re.is_empty()).then(|| eids(re.iter().copied(), |r| r.render())),
                )
                .with_opt("settled", requests_opt(settled))
                .with_opt("not_settled", (!open.is_empty()).then_some(Json::A(open)))
                .with(
                    "shown_digest",
                    eids(shown_digest.iter().copied(), |d| d.render()),
                )
        }
        Reply::Routed {
            entry,
            routed: r,
            existing,
        } => {
            let mut j = Json::obj();
            if *existing {
                j.put("existing", Json::Bool(true));
            }
            let call = RouteCall::of(c.cmd, *existing);
            j.with("entry", Json::S(entry.render()))
                .with("routed", routed(s, r, *entry, &call, c.src))
        }
        Reply::Request {
            request,
            node,
            existing,
        } => {
            let mut j = Json::obj();
            if *existing {
                j.put("existing", Json::Bool(true));
            }
            j.with("request", Json::S(request.render()))
                .with("node", Json::S(node_ref(s, *node)))
        }
        // 1.10 (§9.2, §20.7): a recorded `said` that is not withheld says when it counts as sent.
        // In the lab's fifth round (it4 l1), an agent listed in `unsent` a post it sent in the
        // reply after its release, and the post became unconfirmed. 1.11: one that answered
        // requests it did not settle says that they now read as answered (`answered_note`).
        Reply::Said {
            said,
            node,
            answered,
            settled,
            fulfilled,
            keyed,
        } => Json::obj()
            .with("said_id", Json::S(said.render()))
            .with("answered", eids(answered.iter().copied(), |r| r.render()))
            .with("settled", eids(settled.iter().copied(), |r| r.render()))
            .with("fulfilled", eids(fulfilled.iter().copied(), |r| r.render()))
            .with_opt(
                "answered_note",
                answered_note(c, answered, settled).map(Json::S),
            )
            .with_opt(
                "send",
                s.saids.get(said).and_then(|x| match x.state {
                    forum_core::SaidState::Pending => Some(Json::s(SAID_SEND)),
                    // 1.11: confirmed as it was recorded, its author holding nothing.
                    forum_core::SaidState::Confirmed
                        if x.with_reply && holds_nothing(s, x.session, now) =>
                    {
                        Some(Json::s(SAID_SENT_NOW))
                    }
                    _ => None,
                }),
            )
            .with_opt(
                "conversation_node",
                conversation_node(s, *node, keyed.as_ref()),
            ),
        Reply::SaidConfirmed { already } => {
            let said = match c.cmd {
                Cmd::Talk(TalkCmd::SaidConfirm { said, .. }) => Some(*said),
                _ => None,
            };
            let mut j = Json::obj().with_opt("said_id", said.map(|e| Json::S(e.render())));
            j.put("confirmed", Json::Bool(true));
            if *already {
                j.put("already", Json::Bool(true));
            }
            j
        }
        Reply::Heard {
            entry,
            node,
            answered,
            keyed,
        } => Json::obj()
            .with("entry", Json::S(entry.render()))
            .with("node", Json::S(node_ref(s, *node)))
            .with_opt("answered", answered.map(|q| Json::S(q.render())))
            .with_opt(
                "conversation_node",
                conversation_node(s, *node, keyed.as_ref()),
            ),
        Reply::Asked {
            question,
            said,
            node,
            existing,
            keyed,
        } => {
            let mut j = Json::obj();
            if *existing {
                j.put("existing", Json::Bool(true));
            }
            j.with("question", Json::S(question.render()))
                .with_opt("said_id", said.map(|e| Json::S(e.render())))
                .with("node", Json::S(node_ref(s, *node)))
                .with_opt(
                    "conversation_node",
                    conversation_node(s, *node, keyed.as_ref()),
                )
        }
        Reply::Answered { entry } => Json::obj().with("entry", Json::S(entry.render())),
        // 1.12 (§9.3): a retract of an open question to a person withdraws it.
        Reply::Retracted { entry, withdrawn } => Json::obj()
            .with("entry", Json::S(entry.render()))
            .with_opt("withdrawn", withdrawn.map(|q| Json::S(q.render()))),
        // 1.9 (§13): a note says whom it reaches, and a note without `to` at a node another
        // session holds was addressed to its holders, so the sender sees that it reached them.
        // 1.10: one at a node nobody holds reaches its next holder, and a plain note says that it
        // reached nobody; in the lab's fifth round (l7), 16 of 18 notes reached nobody unsaid.
        // 1.11 (§13): a closed node is never taken: the core delivers a note there to the session
        // that closed it while that session is live, and one that reached no session says so,
        // and names that session while it is silent (`nobody_at_closed`).
        // 1.12 (§13, §20.7): `delivered_to` is always a list, `[]` for none, with `pending` for a
        // note to an open node's next holder; a note at a closed node that reached nobody says so
        // in the `no_recipient` warning. In the lab's seventh round (it6 l6), a string there, "the
        // next holder of …", read as delivery.
        Reply::Say { entry } => {
            let note = s.notes.get(entry);
            // A `principal:` or `holders:` note finds its readers when they read.
            let fixed =
                note.is_none_or(|n| matches!(n.to, Addressee::Session(_) | Addressee::Holder(_)));
            let delivered = fixed.then(|| {
                Json::strs(
                    note.into_iter()
                        .flat_map(|n| n.recipients.iter().map(|x| x.render())),
                )
            });
            let pending = note
                .filter(|n| n.pending_holder)
                .map(|n| Json::S(format!("the next holder of {}", node_ref(s, n.node))));
            Json::obj()
                .with("entry", Json::S(entry.render()))
                .with_opt(
                    "to",
                    note.map(|n| Json::S(super::entries::addressee(s, &n.to))),
                )
                .with_opt("delivered_to", delivered)
                .with_opt("pending", pending)
        }
        Reply::Decided {
            decision,
            settled,
            not_settled,
        } => Json::obj()
            .with("decision", Json::S(decision.render()))
            .with("settled", eids(settled.iter().copied(), |r| r.render()))
            .with("not_settled", not_settled_list(s, not_settled)),
        Reply::Approved { entry, approvals } => Json::obj()
            .with("entry", Json::S(entry.render()))
            .with("approvals", Json::from(*approvals)),
        Reply::Objected { objection } => Json::obj().with("objection", Json::S(objection.render())),
        Reply::ObjectionResolved {
            entry,
            corrected,
            already,
            disputed,
            withdrawn,
            deferred,
        } => {
            let correction = |x: &forum_core::Corrected| {
                Json::obj()
                    .with("node", Json::S(node_ref(s, x.node)))
                    .with("was", Json::S(outcome(&x.was)))
                    .with("outcome", Json::S(outcome(&x.outcome)))
            };
            match entry {
                Some(e) => Json::obj()
                    .with("entry", Json::S(e.render()))
                    .with_opt("corrected", corrected.as_ref().map(correction))
                    .with_opt(
                        "disputed",
                        disputed
                            .as_ref()
                            .map(|d| disputed_json(c, d, withdrawn.is_some())),
                    )
                    .with_opt("withdrawn", withdrawn.as_ref().map(correction))
                    .with_opt("deferred", deferred.map(deferred_json)),
                // 1.10 (§11.3): the verdict that decided it, the same as the caller's.
                None => {
                    let j = Json::obj().with("already", Json::Bool(true));
                    match already {
                        Some(v) => verdict_json(c, j, *v),
                        None => j,
                    }
                }
            }
        }
        Reply::Waited { delivered, news } => {
            if delivered.is_empty() && news.is_none() {
                return Json::obj().with("timeout", Json::Bool(true));
            }
            Json::obj()
                .with(
                    "delivered",
                    Json::A(
                        delivered
                            .iter()
                            .map(|e| inbox_item(s, *e, c.src, None))
                            .collect(),
                    ),
                )
                .with_opt(
                    "news",
                    news.map(|(n, q)| {
                        Json::obj()
                            .with("node", Json::S(node_ref(s, n)))
                            .with("seq", Json::U(q.0))
                    }),
                )
        }
        Reply::Who { at } => lists::who_list(s, *at, now),
        Reply::Paused {
            scope,
            until,
            renewed,
        } => {
            let mut j = Json::obj()
                .with("paused", Json::S(node_ref(s, *scope)))
                .with("until", Json::S(time(*until)));
            if *renewed {
                j.put("renewed", Json::Bool(true));
            }
            j
        }
        Reply::Unpaused { scope } => Json::obj().with("unpaused", Json::S(node_ref(s, *scope))),
        Reply::Health => health(s, c.health, c.health_at, c.src),
        Reply::Export { after, limit } => Json::obj()
            .with_opt("after", after.map(Json::U))
            .with("limit", Json::from(*limit)),
        Reply::ParamsSet => Json::obj().with("params", params(s)),
        Reply::Replay => Json::obj(),
    }
}

/// `hold next` (§20.6): `{target, token, brief, why_this}`, `{status: node, brief, why_this}`,
/// 1.9's `{review: node, objection, brief, why_this}`, `{route: entry, why_this}`, or `{empty, why,
/// leave_better}`.
fn next(c: &BodyCtx<'_>, n: &NextReply) -> Json {
    let s = c.state;
    match n {
        NextReply::Hold {
            node,
            token,
            lease_until,
            why,
            reopened,
        } => {
            let mut j = Json::obj()
                .with("target", Json::S(node_ref(s, *node)))
                .with("token", Json::S(token.render()))
                .with("lease_until", Json::S(time(*lease_until)))
                .with("brief", brief_of(c, *node))
                // 1.12 (§10.4): with `beyond_horizon`, the work the caller's horizon excludes.
                .with("why_this", why_this_at(s, Some(*node), why));
            if let Some(was) = reopened {
                j.put("reopened", reopened_was(was));
            }
            j
        }
        NextReply::Route { entry, why } => {
            let text = c
                .src
                .entry(*entry)
                .map(|v| super::entries::text_of(&v, None));
            Json::obj()
                .with("route", Json::S(entry.render()))
                .with_opt("text", text.map(Json::S))
                .with("why_this", why_this_at(s, None, why))
        }
        NextReply::Status { node, why } => Json::obj()
            .with("status", Json::S(node_ref(s, *node)))
            .with("brief", brief_of(c, *node))
            .with("why_this", why_this_at(s, Some(*node), why)),
        // 1.9 (§10.4): a closed node's objection to decide, with no hold.
        NextReply::Review {
            node,
            objection,
            why,
        } => Json::obj()
            .with("review", Json::S(node_ref(s, *node)))
            .with("objection", Json::S(objection.render()))
            .with("brief", brief_of(c, *node))
            .with("why_this", why_this_at(s, Some(*node), why)),
        NextReply::Empty { why, leave_better } => Json::obj()
            .with("empty", Json::Bool(true))
            .with("why", empty_why(s, why, c.shown_health, c.src))
            .with("leave_better", nodes(s, leave_better))
            .with_opt("stale", stale_of(s, leave_better)),
    }
}
