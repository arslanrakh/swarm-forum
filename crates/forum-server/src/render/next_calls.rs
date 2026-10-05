//! Next calls (§20.9, design D.8.26, D.11.7): the last block of every response the core computed.
//!
//! `query::guide` gives each item as a `Skeleton` (the call with the ids the server knows) and a
//! `Because` (what its `why` reads); this module renders them:
//! - `args`, exactly as D.11.5 decodes the call, with `session` in every item but `session hello`;
//!   a path longer than 64 bytes as its id, except a split's parent path, which `create` needs
//!   whole; principals and `where` by key, never cut; durations in a unit the decoder reads;
//! - `fill`, the arguments the agent writes, a fixed function of the skeleton's variant; an
//!   optional one ends in `?` (`intent?`, `basis?`), the agent writing it only when it applies;
//! - `why`, one template per `Because` variant, at most 100 bytes cut with "…", its condition first;
//!   each template fits within 100 bytes with every id at 10 digits, so a cut falls only on a name
//!   it quotes (a principal's key), which comes last.
//!
//! The text form is `next_calls (<k> of <total>):` and one numbered line per item, `[later:
//! ]<tool> <action> {<args>, <fill>: ?} — <why>`, or `next_calls: none`; `structuredContent`
//! carries `{items: [{tool, args, fill, when, why, rule}], total}`. In the text, `<fill>: ?` marks
//! an argument for the agent to write, not one the server failed to give, and `<fill>?: ?` one it
//! may leave out (1.4, R20, D.8.26): the remedy of `invalid(IntentOpen)` leaves `intent` optional
//! (`CheckpointSwap`, §21), and a decision its `basis` (`Decide`).
//!
//! 1.4's `Checkpoint.intent` (R18), `Close.outcome` (R4), `Reclose` (R4), `UncoveredSlot` (R18),
//! `DecisionOwed.recorded` (R3) and `held` (R28), and the data of `CloseOpenIntent` (R6) are
//! rendered here; rule 4's `STALE SESSION` branch is gone (R27). 1.6's `why` texts (the lab's
//! first round) are only texts: rule 2's confirm no longer asks for a release before a reply,
//! rule 3's intent is for an outside write, and rules 8 and 10 give an objection that stands its
//! way out. 1.7's (the lab's second round): rule 5's routes (`RouteSk`, `RouteAnswers`,
//! `RouteNode`), rule 6's reply before the close (`ReplyOwed.recorded`, `ready`), rule 7 after a
//! release, the confirm once the message is out, and `end`, rendered `next_calls: none — end:
//! <why>`. 1.8's (the lab's third round): rule 3's intent, which a thread post needs not, rule
//! 11's answer `later` on a write, with its addressee, `end` after a worker's release, resolve,
//! or reopen, and the folding of a standing item shown on each of the session's last 3 responses
//! (`block_seen`), which the shell's per-session memory (`Seen`) makes possible. 1.9's (the lab's
//! fourth round): no confirm of a `said` (rule 2's and rule 10's are gone), rule 5's one item with
//! its alternative in the `why` (`RouteAlt`), the review of a closed node's objection (`Review`),
//! with `upheld` to write, and rule 10's objection `why` that names `upheld` and a retraction
//! already made. 1.10's (the lab's fifth round): the verdict is `stands`, `upheld` before. 1.11's
//! (the sixth round): rule 4's item reads the feed from what the session was shown
//! (`item_call`), and rule 8's alternative decides before a release that waits for a merge; rule
//! 5's clarification of the person's open request (`RouteClarifies`) and its route to the first
//! related row, and rule 10's merge after a duplicate link (`Linked`). 1.12's (the seventh
//! round): rule 5's create names the first related row instead of routing there (`Unrouted`'s
//! `related`), rule 8's `why` by the node's state (`Finished`, `MergeWait`, never "decide now"),
//! rule 3's own intent closed in the release, rule 9's reopen of a claim nothing proves
//! (`Unproved`, `Reopen`) and its verdict's three answers, and rule 13's `why` that names the node
//! and the default to take the next (`WorkWaiting`).

use forum_core::query::guide::{
    Because, End, Guidance, Moment, NextCall, RouteAlt, RouteSk, Skeleton,
};
use forum_core::{Eid, Flag, FlagSet, NodeId, PrincipalId, SessionId, State, Total};

use super::entries::addressee;
use super::format::{
    cut, dur_arg, flag, node_ref, path, principal, said_kind, time, to_user_kind, total,
};
use super::json::Json;

/// A `why` is at most this many bytes (§20.9).
pub const WHY_MAX: usize = 100;

/// The call part of an item is at most this many bytes, each key and a split's parent path
/// counted at 64 bytes at most (§20.9).
pub const CALL_MAX: usize = 256;

/// One rendered next call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The tool.
    pub tool: &'static str,
    /// The action.
    pub action: &'static str,
    /// The arguments, `action` included, as D.11.5 decodes them.
    pub args: Json,
    /// The arguments the agent writes; an optional one ends in `?`.
    pub fill: Vec<&'static str>,
}

/// The argument a fill names: the fill without the `?` of an optional one.
#[must_use]
pub fn fill_arg(f: &str) -> &str {
    f.strip_suffix('?').unwrap_or(f)
}

fn id_list<T: Copy>(xs: &[T], f: impl Fn(T) -> String) -> Json {
    Json::strs(xs.iter().copied().map(f))
}

/// The call of a skeleton, for session `sess`.
#[must_use]
pub fn call(s: &State, sess: Option<SessionId>, sk: &Skeleton) -> Call {
    let node = |n: NodeId| Json::S(node_ref(s, n));
    let base = |action: &'static str| {
        let mut j = Json::obj().with("action", Json::s(action));
        if let Some(x) = sess {
            j.put("session", Json::S(x.render()));
        }
        j
    };
    let (tool, action, args, fill): (&'static str, &'static str, Json, Vec<&'static str>) = match sk
    {
        Skeleton::Hello => (
            "session",
            "hello",
            Json::obj().with("action", Json::s("hello")),
            vec![],
        ),
        Skeleton::LogFromUser { principal: p } => (
            "talk",
            "from_user",
            base("from_user").with("principal", Json::S(principal(s, *p))),
            vec!["text", "client"],
        ),
        Skeleton::ViewNode { node: n } => {
            ("view", "node", base("node").with("path", node(*n)), vec![])
        }
        Skeleton::Checkpoint {
            token,
            intent_done,
            intent,
        } => (
            "hold",
            "checkpoint",
            base("checkpoint").with("token", Json::S(token.render())),
            match (*intent_done, *intent) {
                (true, true) => vec!["intent_done", "intent", "done", "next_step"],
                (true, false) => vec!["intent_done", "done", "next_step"],
                (false, true) => vec!["intent", "done", "next_step"],
                (false, false) => vec!["done", "next_step"],
            },
        ),
        Skeleton::CheckpointSwap { token } => (
            "hold",
            "checkpoint",
            base("checkpoint").with("token", Json::S(token.render())),
            vec!["intent_done", "intent?", "done", "next_step"],
        ),
        Skeleton::Release { token, wait } => {
            let mut j = base("release").with("token", Json::S(token.render()));
            match wait {
                Some(q) => {
                    j.put("reason", Json::s("blocked"));
                    j.put(
                        "waits_for",
                        Json::obj().with("question", Json::S(q.render())),
                    );
                }
                None => j.put("reason", Json::s("handoff")),
            }
            ("hold", "release", j, vec!["done", "next_step"])
        }
        Skeleton::Take { target } => (
            "hold",
            "take",
            base("take").with("target", node(*target)),
            vec![],
        ),
        Skeleton::Next => ("hold", "next", base("next"), vec![]),
        Skeleton::CreateFor { entry } => (
            "node",
            "create",
            base("create").with(
                "keys",
                Json::strs([forum_core::KeyStr::of_user_entry(*entry).to_string()]),
            ),
            vec!["path", "title"],
        ),
        Skeleton::Split { parent } => {
            let p = path(s, *parent);
            let at = if p == "/" {
                "/?".to_string()
            } else {
                format!("{p}/?")
            };
            (
                "node",
                "create",
                base("create")
                    .with("path", Json::S(at))
                    .with("take", Json::Bool(false))
                    .with("tags", Json::strs(["offer"])),
                vec!["name", "title"],
            )
        }
        Skeleton::AddOffer { node: n, version } => (
            "node",
            "update",
            base("update")
                .with("path", node(*n))
                .with("version", Json::U(version.0))
                .with("tags_add", Json::strs(["offer"])),
            vec![],
        ),
        Skeleton::Close {
            node: n,
            token,
            settles,
            outcome,
        } => {
            let mut j = base("close")
                .with("path", node(*n))
                .with("token", Json::S(token.render()));
            if !settles.is_empty() {
                j.put("settles", id_list(settles, |r| r.render()));
            }
            match outcome {
                // 1.4 (rule 8, R4): the outcome the node had when the session's take reopened it.
                Some(o) => {
                    j.put("outcome", Json::S(super::format::outcome(o)));
                    ("node", "close", j, vec!["summary"])
                }
                None => ("node", "close", j, vec!["outcome", "summary"]),
            }
        }
        Skeleton::SayTo { at, to } => (
            "talk",
            "say",
            base("say")
                .with("at", node(*at))
                .with("to", Json::S(format!("session:{}", to.render()))),
            vec!["text"],
        ),
        Skeleton::ToUser {
            principal: p,
            kind,
            re,
        } => {
            let mut j = base("to_user").with("principal", Json::S(principal(s, *p)));
            if let Some(k) = kind {
                j.put("kind", Json::s(to_user_kind(*k)));
            }
            if !re.is_empty() {
                j.put("re", id_list(re, |e| e.render()));
            }
            // `verbatim` is optional since 1.6 (§8.1): true only when the text is what they see.
            let fill = if kind.is_some() {
                vec!["text", "verbatim?"]
            } else {
                vec!["kind", "text", "verbatim?"]
            };
            ("talk", "to_user", j, fill)
        }
        Skeleton::Said {
            at,
            kind,
            where_,
            where_fill,
        } => {
            let mut j = base("said").with("at", node(*at));
            if let Some(w) = where_ {
                j.put("where", Json::S(w.to_string()));
            }
            j.put("kind", Json::s(said_kind(*kind)));
            let fill = if *where_fill && where_.is_none() {
                vec!["where", "text"]
            } else {
                vec!["text"]
            };
            ("talk", "said", j, fill)
        }
        Skeleton::Heard {
            at,
            answers,
            where_,
        } => (
            "talk",
            "heard",
            base("heard")
                .with("at", node(*at))
                .with("where", Json::S(where_.to_string()))
                .with("answers", Json::S(answers.render())),
            vec!["from", "text"],
        ),
        Skeleton::Decide {
            at,
            settles,
            supersedes,
        } => {
            let mut j = base("decide").with("at", node(*at));
            if !settles.is_empty() {
                j.put("settles", id_list(settles, |r| r.render()));
            }
            if let Some(d) = supersedes {
                j.put("supersedes", Json::S(d.render()));
            }
            // D.8.26: "fill text, basis?": a decision may cite its basis, and one that supersedes
            // a decision whose basis changed should cite a fresh one.
            ("talk", "decide", j, vec!["text", "basis?"])
        }
        // 1.7 (§8.2): `to` a node or none, or `answers`; `reason` only for none, and optional.
        Skeleton::Route { entry, to } => {
            let mut j = base("route").with("entry", Json::S(entry.render()));
            let fill = match to {
                RouteSk::Fill => vec!["to", "reason?"],
                RouteSk::Nowhere => {
                    j.put("to", Json::s("none"));
                    vec!["reason?"]
                }
                RouteSk::Node(n) => {
                    j.put("to", node(*n));
                    vec![]
                }
                RouteSk::Answers(q) => {
                    j.put("answers", Json::S(q.render()));
                    vec![]
                }
                // 1.11 (§8.2): the default's clarification.
                RouteSk::Clarifies(r) => {
                    j.put("clarifies", Json::S(r.render()));
                    vec![]
                }
            };
            ("talk", "route", j, fill)
        }
        Skeleton::AnswerUser {
            question,
            principal: p,
        } => {
            let mut j = base("from_user").with("answers", Json::S(question.render()));
            if let Some(p) = p {
                j.put("principal", Json::S(principal(s, *p)));
            }
            ("talk", "from_user", j, vec!["text", "client"])
        }
        Skeleton::AnswerAgent { question } => (
            "talk",
            "answer",
            base("answer").with("question", Json::S(question.render())),
            vec!["text"],
        ),
        // 1.9 (§11.3): the agent's verdict, true if the objection stands; 1.10: named `stands`,
        // `upheld` before, which a reviewer read as "the claim is upheld" (it4 l5).
        Skeleton::Resolve { objection } => (
            "talk",
            "resolve",
            base("resolve").with("objection", Json::S(objection.render())),
            vec!["text", "stands"],
        ),
        Skeleton::Merge { from, into } => (
            "node",
            "merge",
            base("merge")
                .with("from", node(*from))
                .with("into", node(*into)),
            vec![],
        ),
        // 1.10 (§16, rule 13): a stale offer's question for its requesters and owners (§8.4), in
        // the human form; its default drops it.
        Skeleton::AskOwners { at } => (
            "talk",
            "ask",
            base("ask")
                .with("at", node(*at))
                .with("to", Json::s("user"))
                .with(
                    "options",
                    Json::strs(forum_core::query::guide::STALE_OPTIONS.iter().copied()),
                )
                .with("default", Json::s(forum_core::query::guide::STALE_DEFAULT)),
            vec!["text"],
        ),
        // 1.9 (§16, rule 13): possible duplicates, linked so that a duplicate signal asks for the
        // merge.
        Skeleton::LinkDuplicate { from, to } => (
            "node",
            "link",
            base("link")
                .with("from", node(*from))
                .with("rel", Json::s("duplicate"))
                .with("to", node(*to)),
            vec![],
        ),
        Skeleton::Wait { timeout } => (
            "talk",
            "wait",
            base("wait").with("timeout", dur_arg(*timeout)),
            vec![],
        ),
        // 1.12 (§11.3, rule 9): the reopen of a claim nothing proves.
        Skeleton::Reopen { node: n } => (
            "node",
            "reopen",
            base("reopen").with("path", node(*n)),
            vec!["reason"],
        ),
    };
    Call {
        tool,
        action,
        args,
        fill,
    }
}

/// A value of `args` as the text form writes it: strings bare, lists in `[…]`, objects in `{…}`.
fn arg_text(v: &Json) -> String {
    match v {
        Json::Null => "null".into(),
        Json::Bool(b) => b.to_string(),
        Json::U(n) => n.to_string(),
        Json::I(n) => n.to_string(),
        Json::S(s) => s.clone(),
        Json::A(xs) => format!(
            "[{}]",
            xs.iter().map(arg_text).collect::<Vec<_>>().join(", ")
        ),
        Json::O(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(k, x)| format!("{k}: {}", arg_text(x)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The text of a call: `<tool> <action> {<args>, <fill>: ?}`, `session` first. A split's name is
/// the `?` that ends its path, so `name` is not written as an argument of its own.
#[must_use]
pub fn call_text(c: &Call) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Json::O(fields) = &c.args {
        if let Some((_, v)) = fields.iter().find(|(k, _)| k == "session") {
            parts.push(format!("session: {}", arg_text(v)));
        }
        for (k, v) in fields {
            if k == "action" || k == "session" {
                continue;
            }
            parts.push(format!("{k}: {}", arg_text(v)));
        }
    }
    for f in &c.fill {
        if *f == "name" && c.action == "create" {
            continue;
        }
        parts.push(format!("{f}: ?"));
    }
    if parts.is_empty() {
        format!("{} {} {{}}", c.tool, c.action)
    } else {
        format!("{} {} {{{}}}", c.tool, c.action, parts.join(", "))
    }
}

fn q(e: Eid) -> String {
    e.render()
}

fn sx(x: SessionId) -> String {
    x.render()
}

fn n_or_more(t: Total) -> String {
    total(t)
}

/// A rule-5 item's alternative (1.9, §20.9): "; else …", or nothing.
fn alt(or: Option<RouteAlt>) -> String {
    match or {
        None => String::new(),
        Some(RouteAlt::Nowhere) => "; else to none if nothing in it needs work".into(),
        Some(RouteAlt::Node { node, .. }) => {
            format!("; else to {} if it lives there", node.render())
        }
        Some(RouteAlt::Create) => "; else node create by key".into(),
    }
}

/// 1.12 (rule 8): the requests a blocked release would settle, `[e2, e5]`, as many whole ids as
/// fit in `room` bytes, the rest as "…": the cut falls on the names the `why` quotes last.
fn settles_list(xs: &[forum_core::RequestId], room: usize) -> String {
    let all: Vec<String> = xs.iter().map(|r| r.render()).collect();
    let mut shown: Vec<&str> = Vec::new();
    for (i, id) in all.iter().enumerate() {
        let rest = i.saturating_add(1) < all.len();
        let mut try_ = shown.clone();
        try_.push(id);
        let text = format!("[{}{}]", try_.join(", "), if rest { ", …" } else { "" });
        if text.len() > room {
            break;
        }
        shown = try_;
    }
    if shown.len() == all.len() {
        format!("[{}]", shown.join(", "))
    } else if shown.is_empty() {
        "[…]".to_string()
    } else {
        format!("[{}, …]", shown.join(", "))
    }
}

/// The `why` of an item: one template per `Because` variant, at most `WHY_MAX` bytes.
#[must_use]
pub fn why(s: &State, b: &Because) -> String {
    let p = |x: PrincipalId| principal(s, x);
    let text = match b {
        // E: the remedies of §21.
        Because::HoldEnded { late } => format!(
            "your hold ended; your write is saved as late {}: re-read before you redo it",
            q(*late)
        ),
        Because::Reread {
            version,
            digest_version,
        } => format!(
            "the node changed: re-read it, then retry with version {} and digest_version {}",
            version.0, digest_version.0
        ),
        Because::MergedInto { node } => {
            format!(
                "it was merged into {}: work on that node instead",
                node.render()
            )
        }
        // 1.4 (§21, R6): for another session's intent, the check first.
        Because::CloseOpenIntent { by, entry, own } => {
            if *own {
                format!(
                    "your {} is open: close it with intent_done; intent only if your next action is \
                     still to do",
                    q(*entry)
                )
            } else {
                format!(
                    "check first if {}'s {} happened, then intent_done; intent only if yours is to do",
                    sx(*by),
                    q(*entry)
                )
            }
        }
        Because::RenewLimit => {
            "renew.max reached: a checkpoint extends the lease and resets the count".into()
        }
        Because::AnswerFits { question } => format!(
            "{} is answered with this call: the one you made fits another kind of question",
            q(*question)
        ),
        Because::NoSession => {
            "your session is unknown, retired, or of another class: open a new one".into()
        }
        // 1 to 5.
        Because::LogFirst { principal: x } => format!(
            "if they wrote to you: log their words verbatim before acting on them; {}",
            p(*x)
        ),
        Because::TellHolder { holder } => format!(
            "{} holds this conversation: tell them what you recorded, so one voice speaks",
            sx(*holder)
        ),
        Because::LeaseDue {
            lease_until,
            at_ceiling,
            intent,
            closing,
        } => match (at_ceiling, intent, closing) {
            (true, _, _) => format!(
                "lease ends {} at lease.ceiling: for a longer step, release with waits_for \
                 external",
                time(*lease_until)
            ),
            // 1.12's review: `later`, beside rule 8's close of the node, for work that goes on.
            (false, Some(e), true) => format!(
                "if not done: lease ends {}; checkpoint by then, with intent_done of {}",
                time(*lease_until),
                q(*e)
            ),
            (false, None, true) => format!(
                "if not done: your lease ends {}; checkpoint by then to keep it",
                time(*lease_until)
            ),
            (false, Some(e), false) => format!(
                "lease ends {}: checkpoint now; intent_done says whether {} happened",
                time(*lease_until),
                q(*e)
            ),
            (false, None, false) => format!(
                "your lease ends {}: checkpoint now to keep it",
                time(*lease_until)
            ),
        },
        Because::IntentOpen { by, entry, own } => {
            if *own {
                // 1.12 (the seventh round): half the workers made a checkpoint for intent_done
                // alone and then released; a release takes it, and a close resolves the intent.
                format!(
                    "after the action announced in {}; pass intent_done in the release if you \
                     release next",
                    q(*entry)
                )
            } else {
                format!(
                    "once you checked if {}'s {} happened: record what you found; do not repeat it",
                    sx(*by),
                    q(*entry)
                )
            }
        }
        // 1.6 (the lab's first round): an intent is for an outside write, not a read. 1.8 (the
        // third round): nor for a post in a thread, whose said is its record (§10.2): 9 of the
        // round's 17 intents needed none. 1.9 (the fourth round): nor for local edits and tests,
        // which nobody else sees (8 of the round's 15 intents); 99 bytes with the time.
        Because::CheckpointBy { by } => format!(
            "by {}; intent before a push, PR, deploy, job, mail; not local edits, tests, posts",
            time(*by)
        ),
        // 1.4 (§12, R18).
        Because::UncoveredSlot => {
            "if you set up an outside trigger: record its intent first".into()
        }
        Because::News { count, .. } => format!(
            "{} entries by others you have not read: read them before you reply or close",
            n_or_more(*count)
        ),
        // 1.9 (§8.2): a result no longer routes the entry; create is for new work, and words that
        // need none are routed to none. 1.12 (the seventh round): the call's first related row, an
        // open work node, is named beside the create, where 1.11's route to it sent a newcomer's
        // question to a team's area as a request.
        Because::Unrouted {
            entry,
            related: None,
        } => format!(
            "if {} asks for work: create its node by key; if it needs none, route it to none",
            q(*entry)
        ),
        Because::Unrouted {
            related: Some(n), ..
        } => format!(
            "if new work: create by key; if only {}'s work: route there; if answered: to_user, to \
             none",
            n.render()
        ),
        // 1.9 (§20.9 rule 5): one item, its alternative in the `why`.
        Because::AnsweredEntry { entry, or } => format!(
            "if nothing else in {} needs work: route it to none{}",
            q(*entry),
            alt(*or)
        ),
        Because::RouteAnswers {
            entry,
            question,
            or,
        } => match or {
            None => format!(
                "if {}'s words answer {}: route them as its answer, not as new work",
                q(*entry),
                q(*question)
            ),
            Some(a) => format!(
                "if {}'s words answer {}{}",
                q(*entry),
                q(*question),
                alt(Some(*a))
            ),
        },
        Because::RouteNode {
            entry,
            node,
            held,
            or,
        } => {
            if *held {
                format!(
                    "if {}'s work lives in the node you hold: route it there{}",
                    q(*entry),
                    alt(*or)
                )
            } else {
                format!(
                    "if {}'s work is in {}, the question's node: route there{}",
                    q(*entry),
                    node.render(),
                    alt(*or)
                )
            }
        }
        // 1.11 (§8.2, rule 5): a follow-up clarifies the person's open request on the only hold.
        Because::RouteClarifies { entry, request, or } => format!(
            "if {} follows up {}: clarify it{}",
            q(*entry),
            q(*request),
            alt(*or)
        ),
        // 6 to 8.
        Because::ReplyOwed {
            misaddressed,
            recorded,
            ready,
            ..
        } => match misaddressed {
            // 1.7 (§5): the reply, then the close, which settles what it answered.
            None if *recorded => {
                "once its work is done: reply with the result, then close, which settles it".into()
            }
            None if *ready => {
                "the parts are closed: reply with the result, then close, which settles it".into()
            }
            None => "decided, not yet answered: report the result, which settles them".into(),
            Some(x) => format!(
                "your result went to someone else: report to the requester; it went to {}",
                p(*x)
            ),
        },
        Because::DecisionOwed {
            after_close,
            ready,
            recorded,
            held,
            after_release,
            ..
        } => {
            if *after_release {
                // 1.7 (§9.4): a release records no decision.
                "you released it with answered requests undecided: decide them now, while you know"
                    .into()
            } else if *recorded && !*held {
                // 1.4 (R3): a request this call recorded on a node nobody holds.
                "if it needs no further work: decide, then reply; nobody holds this node".into()
            } else if *recorded && !*ready {
                // R28 (M-9), 1.7: a request this call recorded on a container the caller holds,
                // which no close finishes.
                "once the work it asks for is done: reply, then decide; it stays open".into()
            } else if *ready {
                "the parts are closed: decide, then reply, then close".into()
            } else if *after_close {
                "you closed it with requests unsettled: decide them, then reply".into()
            } else {
                "answered, not yet decided: record the outcome, which settles them".into()
            }
        }
        // 1.10 (the lab's fifth round): the outside wait a worker meets is a pull request's review
        // or merge, which 1.4's "someone outside" did not name. 1.11 (the sixth round) put "decide
        // now" into every close; 1.12 (the seventh round): the alternative by the node's state, and
        // never "decide now", which made workers file a pull request's status as a decision.
        Because::Finished => "if done; else release handoff with the rest in next_step".into(),
        Because::MergeWait { settles } if settles.is_empty() => {
            "if it is merged; waiting for its merge: release blocked, waits_for {external: 'PR #N \
             merged'}"
                .into()
        }
        Because::MergeWait { settles } => {
            let head = "if done; awaiting merge: release blocked, waits_for {external: 'PR #N merged'}, \
                 settles: ";
            format!(
                "{head}{}",
                settles_list(settles, WHY_MAX.saturating_sub(head.len()))
            )
        }
        // 1.10 (rule 8): a `handoff` release whose `intent_done` names a pull request.
        Because::AwaitMerge => {
            "waiting for its merge: take it, release blocked, waits_for {external: 'PR #N merged'}"
                .into()
        }
        // 1.4 (§10.1, R4); 1.6 (the lab's first round): a claim whose objection stood closes as
        // failed, not with the outcome it had.
        Because::Reclose { was } => format!(
            "if {} still stands, else failed if an objection stood: left open, it is offered as \
             work",
            super::format::outcome(was)
        ),
        Because::ContainerStays { series } => {
            if *series {
                "a series stays open: release it; closing stops it".into()
            } else {
                "a record stays open: release it; never close it".into()
            }
        }
        // 9.
        Because::AskWait {
            question,
            blocking,
            existing,
        } => {
            if *existing {
                format!(
                    "already asked: {} is open; if you cannot go on without it, release and wait",
                    q(*question)
                )
            } else if *blocking {
                format!(
                    "the work cannot go on without {}'s answer: release; the answer brings it back",
                    q(*question)
                )
            } else {
                "if you cannot go on without the answer: the answer or the recheck brings it back"
                    .into()
            }
        }
        Because::AgentWait { question } => format!(
            "when you need the answer to {}: agents answer into your inbox",
            q(*question)
        ),
        Because::JoinAnswer { question } => format!(
            "if the new message answers {}: record it as its answer, not as a new request",
            q(*question)
        ),
        Because::NotOffered => {
            "nothing offers this node: tag it offer, or it waits for nobody".into()
        }
        Because::AnswersNext {
            question,
            principal: x,
        } => format!(
            "when the answer to {} comes: log it with answers; it is for {}",
            q(*question),
            p(*x)
        ),
        Because::StatusItem => {
            "commitments here are past their follow-up: send a status in each conversation".into()
        }
        // 1.9 (§10.4, §11.3): a closed node's objection, offered as a review, not as work to take.
        // 1.10: the verdict `stands`, and what each value means for the claim. 1.12 (the seventh
        // round): "false: it holds" asserted the claim reviewers denied; false says only that the
        // objection fails, and a reviewer that cannot tell defers.
        Because::Review { .. } => {
            "true: the claim is false (refuted or failed); false: the objection fails; cannot tell \
             yet: defer"
                .into()
        }
        // 1.12 (§11.3, rule 9): a resolve that returned `no_basis`.
        Because::Unproved => "if nothing proves it: reopen; it reads unproved, not refuted; a \
                              decide with its basis keeps it done"
            .into(),
        Because::RouteItem => {
            "route this message to its work, or to none, so it is not left unrouted".into()
        }
        // 10.
        Because::Assumption { blocking, .. } => {
            if *blocking {
                "a blocking question lapsed: record the assumption, and take no irreversible step"
                    .into()
            } else {
                "a question went unanswered: record the assumption you proceed on".into()
            }
        }
        Because::StaleBasis { decision } => format!(
            "{}'s basis changed: supersede it with a fresh basis, or retract it",
            q(*decision)
        ),
        Because::Recheck { question } => format!(
            "{} is due a recheck: wait for it, or record its answer with heard",
            q(*question)
        ),
        Because::TooBig { attempts } => format!(
            "{attempts} attempts did not finish it: split off smaller work that next offers"
        ),
        Because::Duplicate { other } => format!(
            "a duplicate of {}: merge the two so their work is done once",
            other.render()
        ),
        // 1.11 (rule 10): after a duplicate link, its merge, once.
        Because::Linked { into } => format!(
            "if they are one piece of work: merge into {}; its requests and keys move, the signal \
             closes",
            into.render()
        ),
        Because::Signal => "this signal needs a decision: record which side holds".into(),
        // 1.6 (the lab's first round): an objection that stands is recorded, resolved as upheld,
        // and its node closed as failed; "once its problem is fixed" alone read as "repair it".
        // 1.9 (the fourth round): the `why` names `upheld`, which no call had passed, and a
        // decision already retracted, for which "decide or retract" was done. 1.10: `stands`.
        Because::Objection {
            retracted: Some(d), ..
        } => format!(
            "{} is retracted: if the objection stands, stands true, close refuted or failed; else \
             false",
            q(*d)
        ),
        Because::Objection {
            retracted: None, ..
        } => "if fixed: stands false; if it stands: decide or retract, stands true, then close \
             refuted or failed"
            .into(),
        // 11 to 13.
        // 1.7 (§13): a write delivers 2 notes; the rest wait for `wait` or `hello`.
        Because::Inbox { count } => {
            format!("{count} more unread notes: talk wait delivers them")
        }
        // 1.8 (§13, the lab's third round): a question delivered on a write is answered once the
        // agent knows; its addressee may be many sessions, not only you.
        Because::AgentAsked {
            by,
            until,
            question,
            later,
        } => {
            let to = s
                .notes
                .get(question)
                .map_or_else(|| "you".to_string(), |n| addressee(s, &n.to));
            if *later {
                format!(
                    "once you know the answer, by {}: {} asked {to}",
                    time(*until),
                    sx(*by)
                )
            } else {
                format!(
                    "answer by {} if you know: {} asked {to}",
                    time(*until),
                    sx(*by)
                )
            }
        }
        Because::HumanItems {
            principal: x,
            questions,
            digest,
        } => format!(
            "ask and summarise: {questions} questions and {digest} digest entries pending for {}",
            p(*x)
        ),
        Because::Tidy { flags: f, .. } if *f == FlagSet::EMPTY.with(Flag::Stale) => {
            // 1.8 (§10.3): an offer nobody has touched for `offer.stale`; 1.10 gives such a node
            // `StaleOffer` or `StaleAnswered` instead, never a take to refresh it.
            "stale offer: close it as dropped if you know it is not wanted".into()
        }
        Because::Tidy { flags: f, .. } => {
            let all: Vec<&str> = f.iter().map(flag).collect();
            let named = match all.as_slice() {
                [] => "flagged".to_string(),
                [one] => format!("flagged {one}"),
                [first, rest @ ..] => format!("flagged {first} and {} more", rest.len()),
            };
            format!("{named}: take it, and leave the place tidier")
        }
        // 1.9 (§10.3, rule 13): the forum's stale offers, for an idle worker; 1.10: asked of its
        // owners, where 1.9's refresh let a worker refresh offers with no news.
        Because::StaleOffer { .. } => "stale offer: ask its owners if it is still wanted; if you \
                                       know it is not, close it as dropped"
            .into(),
        // 1.10 (§16): its owners' answer, or the default, is the news a take acts on.
        Because::StaleAnswered { question, .. } => format!(
            "{} has its answer for this stale offer: take it; close it, or record why it stays",
            q(*question)
        ),
        // 1.9 (§16, rule 13): two open nodes under one parent that share rare words.
        Because::Alike { from, to, .. } => format!(
            "possible duplicates {} and {}: compare, then link as duplicates or merge",
            from.render(),
            to.render()
        ),
        Because::Joined => "you joined it: take it if you will do its work".into(),
        Because::MoreWork { at_hello } => {
            if *at_hello {
                "unless you came for a message or an event".into()
            } else {
                "if you have time for more".into()
            }
        }
        // 1.10 (rule 13): what `next` would give, where "if you have time for more" let workers end
        // while work that fitted their caps waited. 1.12 (the seventh round): the node, its title
        // and estimate, and the default, where both Claude workers ended after one task.
        Because::WorkWaiting { first } => match first {
            forum_core::Cand::Node(n) => take_next(s, *n),
            forum_core::Cand::Status(n) => format!(
                "work that fits your caps is waiting: {}; a worker takes the next until next \
                 offers nothing",
                n.render()
            ),
            forum_core::Cand::Route(e) => {
                format!("a message waits for its routing: {}", e.render())
            }
        },
    };
    cut(&text, WHY_MAX)
}

/// 1.12 (rule 13): "take it: #16 "<title>" (est 30); a worker takes the next until next offers
/// nothing", the title cut to the room the rest leaves within `WHY_MAX`, and left out when that is
/// under 8 bytes: the cut falls on the name it quotes. `next` offers only what fits the caller,
/// so 1.10's "fits your caps" gives its room to the title.
fn take_next(s: &State, n: NodeId) -> String {
    let node = s.nodes.get(&n);
    let est = node
        .and_then(|x| x.est)
        .map(|e| format!(" (est {e})"))
        .unwrap_or_default();
    let head = format!("take it: {}", n.render());
    let tail = format!("{est}; a worker takes the next until next offers nothing");
    let room = WHY_MAX.saturating_sub(head.len() + tail.len() + " \"\"".len());
    let title = node.map(|x| x.title.0.as_str()).unwrap_or("");
    if room < 8 || title.is_empty() {
        format!("{head}{tail}")
    } else {
        format!("{head} \"{}\"{tail}", cut(title, room))
    }
}

/// `now` or `later`.
#[must_use]
pub fn when(m: Moment) -> &'static str {
    match m {
        Moment::Now => "now",
        Moment::Later => "later",
    }
}

/// The call of an item (1.11, §20.9 rule 4): the skeleton's, but rule 4's `view node` is `view
/// feed {at, since}` from the last seq the session was shown of that node (its `seen`, §9.2),
/// which reads only what is new; with none shown, `view node`. In the lab's sixth round (it5 l6),
/// a lead re-read a 5.8 KB brief twice for one new entry each: a third of its response bytes.
#[must_use]
pub fn item_call(s: &State, sess: Option<SessionId>, n: &NextCall) -> Call {
    if let (Skeleton::ViewNode { node }, Because::News { node: news, .. }) = (&n.call, &n.because)
        && node == news
        && let Some(seen) = sess
            .and_then(|x| s.sessions.get(&x))
            .and_then(|x| x.shown.get(node))
            .filter(|q| q.0 > 0)
    {
        let mut args = Json::obj().with("action", Json::s("feed"));
        if let Some(x) = sess {
            args.put("session", Json::S(x.render()));
        }
        return Call {
            tool: "view",
            action: "feed",
            args: args
                .with("at", Json::S(node_ref(s, *node)))
                .with("since", Json::from(seen.0)),
            fill: vec![],
        };
    }
    call(s, sess, &n.call)
}

/// One item: its text line (without its number) and its JSON.
#[must_use]
pub fn item(s: &State, sess: Option<SessionId>, n: &NextCall) -> (String, Json) {
    let c = item_call(s, sess, n);
    let w = why(s, &n.because);
    let mut line = String::new();
    if n.when == Moment::Later {
        line.push_str("later: ");
    }
    line.push_str(&call_text(&c));
    line.push_str(" — ");
    line.push_str(&w);
    let j = Json::obj()
        .with("tool", Json::s(c.tool))
        .with("args", c.args.clone())
        .with("fill", Json::strs(c.fill.iter().copied()))
        .with("when", Json::s(when(n.when)))
        .with("why", Json::S(w))
        .with("rule", Json::s(n.rule.name()));
    (line, j)
}

/// The text of `end` (1.7, §20.9); 1.8: after a release, a resolve, or a reopen, `next`'s. 1.12
/// (the lab's seventh round): it says that the session needs no closing (it6 l1: an idle worker
/// was unsure whether ending its run was enough).
#[must_use]
pub fn end_text(e: End) -> &'static str {
    match e {
        End::AfterNext | End::Idle => {
            "nothing is offered to you: end your run; your session needs no closing, and waiting \
             nodes come back by themselves"
        }
        End::AtHello => {
            "nothing is offered to you; unless you came for a message or an event, end your run; \
             your session needs no closing"
        }
    }
}

/// What `End::AtHello`'s text adds for a session that serves no principal (1.12, §20.9): how a
/// thread's message is recorded. In the lab's seventh round (it6 l3), a chat worker answered a
/// thread's two questions without recording either, since nothing it read named the record.
pub const END_THREAD: &str = "; a thread's message: node create {path, title, keys: [<thread>], \
     request: {origin, requester, text}}, or talk request on its node";

/// The text of `end` for session `sess` (1.12): [`end_text`], and, after a `hello` of a session
/// that serves no principal, [`END_THREAD`].
#[must_use]
pub fn end_text_for(s: &State, sess: Option<SessionId>, e: End) -> String {
    let serves_none = sess
        .and_then(|x| s.sessions.get(&x))
        .is_some_and(|x| x.principals.is_empty());
    match e {
        End::AtHello if serves_none => format!("{}{END_THREAD}", end_text(e)),
        _ => end_text(e).to_string(),
    }
}

/// The block of a response: its text, and `{items, total, end?}` for `structuredContent`, every
/// item printed (a session's first response, or a test's).
#[must_use]
pub fn block(s: &State, sess: Option<SessionId>, g: &Guidance) -> (String, Json) {
    let (text, json, _) = block_seen(s, sess, g, &Standing::new());
    (text, json)
}

/// How many responses in a row the shell remembers a standing item on, and after how many it
/// folds it (1.8, §20.9 "Folding").
pub const FOLD_AFTER: u8 = 3;

/// The standing items of a session's last responses, each with how many responses in a row
/// showed it, at most `FOLD_AFTER` (1.8, §20.9): the shell's memory, which `Seen` keeps.
pub type Standing = std::collections::BTreeMap<NextCall, u8>;

/// `block`, folding each standing item that `before` says was shown on each of the session's
/// last `FOLD_AFTER` responses into one line, `+ <f> standing items, shown before`, carried as
/// `folded` (1.8, §20.9); and the memory after this response. In the lab's third round, a lead
/// was shown the same two items on 16 and 17 consecutive responses.
#[must_use]
pub fn block_seen(
    s: &State,
    sess: Option<SessionId>,
    g: &Guidance,
    before: &Standing,
) -> (String, Json, Standing) {
    let mut after = Standing::new();
    let mut folded: u32 = 0;
    let mut rendered: Vec<(String, Json)> = Vec::new();
    for n in &g.items {
        if !n.once {
            let k = before.get(n).copied().unwrap_or(0);
            after.insert(n.clone(), k.saturating_add(1).min(FOLD_AFTER));
            if k >= FOLD_AFTER {
                folded = folded.saturating_add(1);
                continue;
            }
        }
        rendered.push(item(s, sess, n));
    }
    let text = if g.items.is_empty() {
        match g.end {
            Some(e) => format!("next_calls: none — end: {}", end_text_for(s, sess, e)),
            None => "next_calls: none".to_string(),
        }
    } else {
        let mut t = format!("next_calls ({} of {}):", rendered.len(), total(g.total));
        for (i, (line, _)) in rendered.iter().enumerate() {
            t.push_str(&format!("\n{}. {line}", i + 1));
        }
        if folded > 0 {
            t.push_str(&format!("\n+ {}", folded_text(folded)));
        }
        t
    };
    let total_j = match g.total {
        Total::Exact(n) => Json::from(n),
        Total::AtLeast(n) => Json::S(format!("≥{n}")),
    };
    let json = Json::obj()
        .with(
            "items",
            Json::A(rendered.into_iter().map(|(_, j)| j).collect()),
        )
        .with_opt("folded", (folded > 0).then(|| Json::from(folded)))
        .with("total", total_j)
        .with_opt("end", g.end.map(|e| Json::S(end_text_for(s, sess, e))));
    (text, json, after)
}

/// The line of the folded items (1.8, §20.9).
#[must_use]
pub fn folded_text(f: u32) -> String {
    if f == 1 {
        "1 standing item, shown before".to_string()
    } else {
        format!("{f} standing items, shown before")
    }
}
