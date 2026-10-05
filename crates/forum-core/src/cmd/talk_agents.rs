//! The commands of `talk say`, `ask` to agents, `answer`, `wait`, `who`, and `session status`
//! (design D.8.16, §13). Owner: WP-D5b.
//!
//! Each command is a pair: a validation that reads the state and returns the plan or the error,
//! and an execution of the plan that emits the events and cannot fail. Every validation follows
//! D.8.24's row of its command.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::apply::{node_of, target};
use crate::cmd::{AddresseeArg, AgentAddressee, AskTo, Cmd, SessionCmd, TalkCmd};
use crate::error::{Error, What, Why};
use crate::event::{EntryBody, EntryText, Ev, QuestionBody};
use crate::helpers::Exec;
use crate::helpers::agents::{closer, deliverable, recipients};
use crate::helpers::principals::{PrincipalRef, intern_ref, principal_ref};
use crate::reply::{Reply, Warning, WarningCode, WarningDetail};
use crate::state::{AqState, State};
use crate::types::{
    Addressee, Dur, Eid, Expects, NodeId, NoteId, QuestionId, Seq, SessionId, Text, Time,
    USER_PRINCIPAL,
};

use super::Vcx;
use super::talk_people::{QuestionKind, question_kind};

/// The calling session, which every `talk` action has (a call that opens one has its id).
fn session(cx: &Vcx<'_>) -> Result<SessionId, Error> {
    cx.sess.ok_or(Error::invalid(Why::BadParams))
}

/// A text as an entry's main text.
fn main(t: &Text) -> EntryText {
    EntryText {
        main: t.clone(),
        extra: Vec::new(),
    }
}

// ------------------------------------------------------------------------------------------------
// session status

/// The plan of `session status` (D.8.16).
#[derive(Debug)]
pub(crate) struct StatusPlan {
    sess: SessionId,
    text: Text,
}

/// Validates `session status {text}` (D.8.24's row: no error of its own).
pub(crate) fn status(cx: &Vcx<'_>, cmd: &Cmd) -> Result<StatusPlan, Error> {
    let Cmd::Session(SessionCmd::Status { text }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(StatusPlan {
        sess: session(cx)?,
        text: text.clone(),
    })
}

/// Executes `session status`: `SessionStatus`, the text `who` shows.
pub(crate) fn status_exec(ex: &mut Exec<'_>, plan: StatusPlan) -> Reply {
    ex.emit(Ev::SessionStatus {
        session: plan.sess,
        text: plan.text,
    });
    Reply::Status
}

// ------------------------------------------------------------------------------------------------
// Addressees (§13, D.8.16)

/// An addressee as validation resolves it: ready, or a `principal:<key>` that execution interns
/// first (CS-1).
#[derive(Clone, Debug)]
enum To {
    /// Resolved.
    Ready(Addressee),
    /// `principal:<key>`, interned by execution.
    Principal(PrincipalRef),
}

/// A `session:` addressee: it exists (1.4, §21, R12: `not_found(Addressee)`, whose remedy is not a
/// new session for the caller, unlike `not_found(Session)`).
fn session_to(s: &State, x: SessionId) -> Result<Addressee, Error> {
    if s.sessions.contains_key(&x) {
        Ok(Addressee::Session(x))
    } else {
        Err(Error::not_found(What::Addressee))
    }
}

/// The addressee of `say` (D.8.16): `session:` exists (`not_found(Addressee)`); `holder:` and
/// `holders:` paths resolve (`not_found(Node)`); a bare `user` is the first stage of `at`'s chain,
/// fixed now (CS-3, A-49); `principal:<key>` is interned by execution.
fn say_to(s: &State, at: NodeId, to: &AddresseeArg) -> Result<To, Error> {
    Ok(match to {
        AddresseeArg::Session(x) => To::Ready(session_to(s, *x)?),
        AddresseeArg::Holder(r) => To::Ready(Addressee::Holder(node_of(s, r)?)),
        AddresseeArg::Holders(r) => To::Ready(Addressee::Holders(node_of(s, r)?)),
        AddresseeArg::User => {
            let first = crate::helpers::user::chain(s, Some(at), None, false)
                .first()
                .copied()
                .unwrap_or(USER_PRINCIPAL);
            To::Ready(Addressee::Principal(first))
        }
        AddresseeArg::Principal(k) => To::Principal(principal_ref(s, k)),
    })
}

/// 1.10 (§13): nobody holds `n` live at `now`, and it can be held (open, not reserved, capacity
/// above 0): a `say` at `n` without `to` is addressed `holder:<n>`, which its next taker receives.
/// 1.11: a closed node is never taken, so its next holder never comes (`closer` instead).
fn next_holder(s: &State, n: NodeId, now: Time) -> bool {
    s.nodes.get(&n).is_some_and(|x| {
        x.is_open()
            && !n.is_reserved()
            && x.origin != crate::state::Origin::Reserved
            && x.capacity > 0
    }) && crate::query::holders(s, n, now).is_empty()
}

/// Whether a session other than `sender` holds `n` live at `now` (1.9, §13): a `say` at `n`
/// without `to` is then addressed `holder:<n>`.
fn others_hold(s: &State, n: NodeId, sender: Option<SessionId>, now: Time) -> bool {
    crate::query::holders(s, n, now)
        .iter()
        .any(|(x, _)| Some(*x) != sender)
}

/// 1.12 (§13, §21): the warnings of a note or, with `question`, an agent question on `at` to `to`
/// whose recipients, fixed now, are `reached`: `no_recipient` when it is by `holder:` of a closed
/// node, or a note without `to` at a closed `at`, that reached nobody; `addressee_gone` when it is to
/// `session:` of a silent session, which reads nothing until it calls again. In the lab's seventh
/// round (it6 l6), two notes to the holder of a closed incident and one to a session silent for
/// 36 minutes each reported delivery.
fn delivery_warnings(
    s: &State,
    at: NodeId,
    to: Option<&Addressee>,
    reached: &BTreeSet<SessionId>,
    question: bool,
) -> Vec<Warning> {
    let mut out = Vec::new();
    let closed = |n: NodeId| s.nodes.get(&n).is_some_and(|x| !x.is_open());
    let node = match to {
        Some(Addressee::Holder(n)) => Some(*n),
        None => Some(at),
        Some(Addressee::Session(_) | Addressee::Holders(_) | Addressee::Principal(_)) => None,
    };
    if let Some(n) = node
        && closed(n)
        && reached.is_empty()
    {
        out.push(Warning {
            code: WarningCode::NoRecipient,
            id: Some(n),
            detail: WarningDetail::NoRecipient { node: n, question },
        });
    }
    if let Some(Addressee::Session(x)) = to
        && let Some(y) = s.sessions.get(x)
        && y.silent
    {
        out.push(Warning {
            code: WarningCode::AddresseeGone,
            id: None,
            detail: WarningDetail::AddresseeGone {
                session: *x,
                since: y.last_seen,
            },
        });
    }
    out
}

/// The addressee of `ask` to agents, validated like `say`'s (D.8.16).
fn agent_to(s: &State, to: &AgentAddressee) -> Result<Addressee, Error> {
    match to {
        AgentAddressee::Session(x) => session_to(s, *x),
        AgentAddressee::Holder(r) => Ok(Addressee::Holder(node_of(s, r)?)),
        AgentAddressee::Holders(r) => Ok(Addressee::Holders(node_of(s, r)?)),
    }
}

/// The addressee, interning a new principal key (`PrincipalSeen`, CS-1).
fn resolve_to(ex: &mut Exec<'_>, to: &To) -> Addressee {
    match to {
        To::Ready(a) => *a,
        To::Principal(p) => Addressee::Principal(intern_ref(ex, p)),
    }
}

// ------------------------------------------------------------------------------------------------
// ask, to agents

/// The plan of `talk ask` to agents (D.8.16).
#[derive(Debug)]
pub(crate) struct AskAgentPlan {
    at: NodeId,
    to: Addressee,
    text: Text,
}

/// Validates `talk ask {at, text, to: agent}` (D.8.16, D.8.24's row): `at`, then `to` as `say`'s
/// (`not_found(Addressee)` for a retired or unknown `session:`, 1.4, R12; `not_found(Node)` for a
/// `holder:` or `holders:` path). `again` and `blocking` change nothing for this form (D.14.4).
pub(crate) fn ask_agent(cx: &Vcx<'_>, cmd: &Cmd) -> Result<AskAgentPlan, Error> {
    let Cmd::Talk(TalkCmd::Ask {
        text,
        to: AskTo::Agent(to),
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let at = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let to = agent_to(s, to)?;
    Ok(AskAgentPlan {
        at,
        to,
        text: text.clone(),
    })
}

/// Executes `talk ask` to agents: the question on `at`, delivered until `now + deliver.window` to
/// its recipients, fixed now, the sender never a `holder:` recipient (1.4, R12).
pub(crate) fn ask_agent_exec(ex: &mut Exec<'_>, plan: AskAgentPlan) -> Reply {
    let now = ex.now();
    let by = ex.author();
    let until = now.plus(ex.s().params.deliver_window);
    let (recipients, pending_holder) = recipients(ex.s(), &plan.to, ex.sess(), now);
    let warnings = delivery_warnings(ex.s(), plan.at, Some(&plan.to), &recipients, true);
    let q = ex.append(
        plan.at,
        by,
        EntryBody::Question(QuestionBody::Agent {
            to: plan.to,
            until,
            recipients,
            pending_holder,
        }),
        main(&plan.text),
    );
    for w in warnings {
        ex.warn(w);
    }
    Reply::Asked {
        question: q,
        said: None,
        node: plan.at,
        existing: false,
        keyed: None,
    }
}

// ------------------------------------------------------------------------------------------------
// answer

/// The plan of `talk answer` (D.8.16).
#[derive(Debug)]
pub(crate) struct AnswerPlan {
    question: QuestionId,
    node: NodeId,
    recipient: SessionId,
    text: Text,
}

/// Validates `talk answer {question, text}` (D.8.16, D.8.24's row, D.14.6): the question's entry,
/// whose node and author the shell added, exists (`not_found(Question)`: an id at or past
/// `next_eid`, or the shell's `UNKNOWN_NODE` for an entry its table lacks, refused here after O1
/// and the session check) → it is an agent question (`invalid(NotAgentQuestion)`, A-36): a
/// resident agent question; a resident people or user question, or a resident entry of another
/// type, is not; an entry the core no longer knows is taken for an agent question that left
/// memory at its `until` when the shell names an asking session (D.4.12), and is not without one.
pub(crate) fn answer(cx: &Vcx<'_>, cmd: &Cmd) -> Result<AnswerPlan, Error> {
    let Cmd::Talk(TalkCmd::Answer {
        question,
        node,
        asker,
        text,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    if *question >= s.counters.next_eid || !s.nodes.contains_key(node) {
        return Err(Error::not_found(What::Question));
    }
    let not_agent = Error::invalid(Why::NotAgentQuestion);
    let recipient = match question_kind(s, *question) {
        Some(QuestionKind::Agent) => s.aquestions.get(question).map(|q| q.asker),
        Some(QuestionKind::People | QuestionKind::User | QuestionKind::Unknown) => {
            return Err(not_agent);
        }
        None => {
            if s.entries.contains_key(question) {
                return Err(not_agent);
            }
            *asker
        }
    }
    .ok_or(not_agent)?;
    Ok(AnswerPlan {
        question: *question,
        node: *node,
        recipient,
        text: text.clone(),
    })
}

/// Executes `talk answer`: the answer on the question's node, delivered to the asker until `now +
/// deliver.window`; `AqAnswered` when the question is resident and still open. An answer after the
/// question left memory is recorded and delivered like any other, and changes no state (D.4.12).
pub(crate) fn answer_exec(ex: &mut Exec<'_>, plan: AnswerPlan) -> Reply {
    let now = ex.now();
    let by = ex.author();
    let until = now.plus(ex.s().params.deliver_window);
    let entry = ex.append(
        plan.node,
        by,
        EntryBody::Answer {
            question: plan.question,
            recipient: plan.recipient,
            until,
        },
        main(&plan.text),
    );
    if ex
        .s()
        .aquestions
        .get(&plan.question)
        .is_some_and(|q| q.state == AqState::Open)
    {
        ex.emit(Ev::AqAnswered {
            q: plan.question,
            by: entry,
        });
    }
    Reply::Answered { entry }
}

// ------------------------------------------------------------------------------------------------
// say

/// The plan of `talk say` (D.8.16).
#[derive(Debug)]
pub(crate) struct SayPlan {
    at: NodeId,
    to: Option<To>,
    until: Time,
    expects: Option<Expects>,
    reply_to: Option<Eid>,
    text: Text,
}

/// The note's `until`: as given, absolute or from now, else `now + deliver.window` (§13). A time at
/// or before `now` is moved to the first instant after it, so that the note is born live and
/// expires at once (T3: no live note has `until <= now`); §13 does not say what such a `say` does.
fn until_of(s: &State, until: Option<crate::types::When>, now: Time) -> Time {
    let t = until.map_or_else(|| now.plus(s.params.deliver_window), |w| w.resolve(now));
    t.max(now.plus(Dur(1)))
}

/// Validates `talk say {at, text, to?, expects?, reply_to?, until?}` (D.8.16, D.8.24's row): `at`
/// → `to` (`not_found(Addressee)` for an unknown or retired `session:`, 1.4, R12;
/// `not_found(Node)` for a `holder:` or `holders:` path; 1.9: without `to`, `holder:<at>` when a
/// session other than the caller holds `at`) → `reply_to` exists, `reply_to < next_eid`
/// (`not_found(Entry)`, D.4.5).
pub(crate) fn say(cx: &Vcx<'_>, cmd: &Cmd) -> Result<SayPlan, Error> {
    let Cmd::Talk(TalkCmd::Say {
        text,
        to,
        expects,
        reply_to,
        until,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let at = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let to = match to {
        Some(a) => Some(say_to(s, at, a)?),
        // 1.9 (§13): a note at a node that another session holds reaches its holders, as
        // `holder:<at>` would. In the lab's fourth round (l6), five notes to the lead's node
        // reached nobody. 1.10: so does a note at a node nobody holds that can be held, which its
        // next holder receives; in the fifth round (l7), 16 of 18 notes reached nobody. 1.11: a
        // note at a closed node reaches the session that closed it, while it is live. A note at a
        // node only the sender holds, or one nobody can hold, stays a plain note.
        None if others_hold(s, at, cx.sess, cx.now) || next_holder(s, at, cx.now) => {
            Some(To::Ready(Addressee::Holder(at)))
        }
        None => closer(s, at, cx.sess).map(|x| To::Ready(Addressee::Session(x))),
    };
    if reply_to.is_some_and(|e| e >= s.counters.next_eid) {
        return Err(Error::not_found(What::Entry));
    }
    Ok(SayPlan {
        at,
        to,
        until: until_of(s, *until, cx.now),
        expects: *expects,
        reply_to: *reply_to,
        text: text.clone(),
    })
}

/// Executes `talk say`: the note on `at`, with its addressee (a principal interned first), its
/// recipients fixed now (the sender never a `holder:` recipient, 1.4, R12), or a plain note without
/// `to` at a node only the sender holds, or one nobody can hold (A-17, 1.9, 1.10);
/// `expects_ignored` for `expects` of `ack` or `reply`, which is stored and nothing waits for (1.4,
/// §13, R17).
pub(crate) fn say_exec(ex: &mut Exec<'_>, plan: SayPlan) -> Reply {
    let now = ex.now();
    let by = ex.author();
    let to = plan.to.as_ref().map(|t| resolve_to(ex, t));
    let (recipients, pending_holder) = match &to {
        Some(a) => recipients(ex.s(), a, ex.sess(), now),
        None => (BTreeSet::new(), false),
    };
    let warnings = delivery_warnings(ex.s(), plan.at, to.as_ref(), &recipients, false);
    let entry = ex.append(
        plan.at,
        by,
        EntryBody::Note {
            to,
            until: Some(plan.until),
            recipients,
            pending_holder,
            expects: plan.expects,
            reply_to: plan.reply_to,
        },
        main(&plan.text),
    );
    if let Some(e @ (Expects::Ack | Expects::Reply)) = plan.expects {
        ex.warn(Warning {
            code: WarningCode::ExpectsIgnored,
            id: Some(plan.at),
            detail: WarningDetail::ExpectsIgnored { expects: e },
        });
    }
    for w in warnings {
        ex.warn(w);
    }
    Reply::Say { entry }
}

// ------------------------------------------------------------------------------------------------
// wait

/// The plan of `talk wait` (D.8.16).
#[derive(Debug)]
pub(crate) struct WaitPlan {
    sess: SessionId,
    delivered: Vec<NoteId>,
    news: Option<(NodeId, Seq)>,
}

/// Validates `talk wait {timeout, for?}` (D.8.16, D.11.11): `for` resolves (`not_found(Node)`);
/// the notes it delivers are `first(inbox)` (1.5); its news is `Some((n, n.seq))` when `n.seq`
/// passed `since`, the last seq the shell reported to this session, else what the session was
/// shown of `n`. The long poll is the shell's.
pub(crate) fn wait(cx: &Vcx<'_>, cmd: &Cmd) -> Result<WaitPlan, Error> {
    let Cmd::Talk(TalkCmd::Wait { since, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let for_ = target(s, cmd)?;
    let sess = session(cx)?;
    let delivered = deliverable(s, sess, cx.now);
    let news = for_.and_then(|n| {
        let seen = since.unwrap_or_else(|| {
            s.sessions
                .get(&sess)
                .and_then(|x| x.shown.get(&n).copied())
                .unwrap_or(Seq(0))
        });
        let seq = s.nodes.get(&n)?.seq;
        (seq > seen).then_some((n, seq))
    });
    Ok(WaitPlan {
        sess,
        delivered,
        news,
    })
}

/// Executes `talk wait`: `InboxRead` for the notes it delivers; news moves no `seen` (A-85, CS-14).
pub(crate) fn wait_exec(ex: &mut Exec<'_>, plan: WaitPlan) -> Reply {
    if !plan.delivered.is_empty() {
        ex.emit(Ev::InboxRead {
            session: plan.sess,
            notes: plan.delivered.iter().copied().collect(),
        });
    }
    Reply::Waited {
        delivered: plan.delivered,
        news: plan.news,
    }
}

// ------------------------------------------------------------------------------------------------
// who

/// The plan of `talk who` (D.8.16).
#[derive(Debug)]
pub(crate) struct WhoPlan {
    at: Option<NodeId>,
}

/// Validates `talk who {at?}`: `at` resolves (`not_found(Node)`).
pub(crate) fn who(cx: &Vcx<'_>, cmd: &Cmd) -> Result<WhoPlan, Error> {
    if !matches!(cmd, Cmd::Talk(TalkCmd::Who { .. })) {
        return Err(Error::invalid(Why::BadParams));
    }
    Ok(WhoPlan {
        at: target(cx.s, cmd)?,
    })
}

/// Executes `talk who`: a read; the shell renders `query::who` (D.8.25, CS-21).
pub(crate) fn who_exec(_ex: &mut Exec<'_>, plan: WhoPlan) -> Reply {
    Reply::Who { at: plan.at }
}

#[cfg(test)]
#[path = "talk_agents_tests.rs"]
mod tests;
