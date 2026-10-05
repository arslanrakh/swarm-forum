//! The commands of `talk request`, `said`, its confirmation, `heard`, and `ask` to a person (design
//! D.8.8, D.8.15, §9). Owner: WP-D5b.
//!
//! Each command is a pair: a validation that reads the state and returns the plan or the error,
//! and an execution of the plan that emits the events and cannot fail. Every validation follows
//! D.8.24's row of its command, with `not_found` before `invalid` (D.14.5): the target, the shape,
//! the other references, then what the state decides (the conversation key and its node, C1).
//!
//! The warnings of `said` and `ask` (`moved`, `conversation_held`, `agreement`) are computed on
//! the pre-state, at validation, and carried by the plan (D.8.15).

use alloc::vec::Vec;

use crate::apply::target;
use crate::cmd::{AskTo, Cmd, TalkCmd};
use crate::error::{Error, Field, What, Why};
use crate::event::{ConfirmVia, EntryBody, EntryText, Ev, QuestionBody, TextField};
use crate::helpers::Exec;
use crate::helpers::agents::{Voice, conversation_holder, voice};
use crate::helpers::conversation::{
    ConvNode, answer_pq, conv_key, conv_node, record_request, try_settle,
};
use crate::helpers::people::resolving;
use crate::query::{MOVED_MAX, SCAN_MAX};
use crate::reply::{Reply, Total, Warning, WarningCode, WarningDetail};
use crate::state::{Author, PqState, SaidState, State, WaitOn};
use crate::types::{
    CommitmentId, EntryType, Key, KeyStr, NodeId, QuestionId, RequestId, SaidId, SaidKind, Seq,
    SessionId, Text, Time,
};

use super::Vcx;

/// The calling session, which every `talk` action has (a call that opens one has its id).
fn session(cx: &Vcx<'_>) -> Result<SessionId, Error> {
    cx.sess.ok_or(Error::invalid(Why::BadParams))
}

/// The target `at`: it resolved (`target` returned `Some`).
fn at_of(cx: &Vcx<'_>, cmd: &Cmd) -> Result<NodeId, Error> {
    target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))
}

/// The conversation of a `said`, `heard`, or `ask` on `at` (§9.1, D.8.8): a `repeat:` `where` is
/// `invalid(ReservedKey)` (A-90); else the key `where` names, or the one conversation key of the
/// nearest node from `at` up (`invalid(NoConversationKey | AmbiguousConversationKey)`, A-30); and
/// its conversation node, which must not be reserved (`invalid(ReservedNode)`, A-29, C1).
fn conversation(s: &State, at: NodeId, where_: Option<&Key>) -> Result<(Key, ConvNode), Error> {
    if where_.is_some_and(|k| k.key.is_reserved()) {
        return Err(Error::invalid(Why::ReservedKey));
    }
    let k = conv_key(s, at, where_)?;
    let c = conv_node(s, at, &k.key)?;
    Ok((k, c))
}

/// What kind of question an entry is, as far as the core can tell: a resident question of one of
/// the three kinds, a question whose entity left memory (its metadata still resident), or none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum QuestionKind {
    /// To a person outside the swarm.
    People,
    /// To agents.
    Agent,
    /// To a human the swarm serves.
    User,
    /// A question entry whose entity is not resident.
    Unknown,
}

/// The kind of question `q` names, None when it names no question the core knows: an id at or past
/// `next_eid`, an entry of another type, or one that left memory with its metadata.
pub(super) fn question_kind(s: &State, q: QuestionId) -> Option<QuestionKind> {
    if s.pquestions.contains_key(&q) {
        Some(QuestionKind::People)
    } else if s.aquestions.contains_key(&q) {
        Some(QuestionKind::Agent)
    } else if s.uquestions.contains_key(&q) {
        Some(QuestionKind::User)
    } else if s
        .entries
        .get(&q)
        .is_some_and(|m| m.ty == EntryType::Question)
    {
        Some(QuestionKind::Unknown)
    } else {
        None
    }
}

// ------------------------------------------------------------------------------------------------
// The warnings of `said` and `ask` (D.8.15), on the pre-state

/// `moved` (§9.2, CS-17): the people entries on `c` after `seen` that another session recorded,
/// read from `ix.people` newest first: at most `MOVED_MAX` of them, with their total, exact when
/// the range holds at most `SCAN_MAX` entries and no people entry after `seen` has left memory
/// (`seen >= c.people_left`), `≥k` otherwise. None when it finds none.
fn moved(s: &State, c: NodeId, sess: SessionId, seen: Seq) -> Option<Warning> {
    let lo = (c, Seq(seen.0.saturating_add(1)));
    let hi = (c, Seq(u64::MAX));
    let mut scanned: u32 = 0;
    let mut count: u32 = 0;
    let mut cut = false;
    let mut entries = Vec::new();
    if lo <= hi {
        for (_, &e) in s.ix.people.range(lo..=hi).rev() {
            if scanned >= SCAN_MAX {
                cut = true;
                break;
            }
            scanned = scanned.saturating_add(1);
            if s.entries.get(&e).map(|m| m.by) == Some(Author::Session(sess)) {
                continue;
            }
            count = count.saturating_add(1);
            if entries.len() < MOVED_MAX {
                entries.push(e);
            }
        }
    }
    if count == 0 {
        return None;
    }
    let left = s.nodes.get(&c).map_or(Seq(0), |n| n.people_left);
    let total = if !cut && seen >= left {
        Total::Exact(count)
    } else {
        Total::AtLeast(count)
    };
    Some(Warning {
        code: WarningCode::Moved,
        id: Some(c),
        detail: WarningDetail::Moved { entries, total },
    })
}

/// `conversation_held` (§9.2): another session holds `c` or a descendant, and the caller holds
/// neither; it names the session rule 2 of the next calls would have the caller tell
/// (`conversation_holder`, D.8.26). 1.10: the `said` counts all the same (1.9 withheld it). 1.11:
/// none when no such holder has the cap of `k`'s scheme and the caller has it (§9.1): the caller
/// is the conversation's voice.
fn held(s: &State, c: NodeId, sess: SessionId, k: &KeyStr) -> Option<Warning> {
    conversation_holder(s, c, sess, k).map(|h| Warning {
        code: WarningCode::ConversationHeld,
        id: Some(c),
        detail: WarningDetail::ConversationHeld {
            holders: alloc::vec![h],
        },
    })
}

/// `agreement` of a `said` or an `ask` (§11.2, A-76, CS-34): the non-empty agreements of `at` and
/// its ancestors, nearest first, then those of `c` and its ancestors when `c` is not among them;
/// with the approvals of `at`'s current content.
fn agreement(s: &State, at: NodeId, c: NodeId) -> Option<Warning> {
    let mut sources = crate::query::reads::agreement_sources(s, at);
    if !crate::query::is_under(s, at, c) {
        for m in crate::query::reads::agreement_sources(s, c) {
            if !sources.contains(&m) {
                sources.push(m);
            }
        }
    }
    if sources.is_empty() {
        return None;
    }
    Some(Warning {
        code: WarningCode::Agreement,
        id: Some(at),
        detail: WarningDetail::Agreement {
            sources,
            approvals: crate::query::reads::current_approvals(s, at),
            content_version: s
                .nodes
                .get(&at)
                .map(|x| x.content_version)
                .unwrap_or_default(),
        },
    })
}

/// The warnings of a `said` or an `ask` that records one, in §21's order: `moved`,
/// `conversation_held`, `agreement`. `seen` defaults to what the session was shown of `c`, or 0.
fn said_warnings(
    s: &State,
    at: NodeId,
    c: NodeId,
    sess: SessionId,
    k: &KeyStr,
    seen: Option<Seq>,
) -> Vec<Warning> {
    let seen = seen.unwrap_or_else(|| {
        s.sessions
            .get(&sess)
            .and_then(|x| x.shown.get(&c).copied())
            .unwrap_or(Seq(0))
    });
    [
        moved(s, c, sess, seen),
        held(s, c, sess, k),
        agreement(s, at, c),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// 1.11 (§9.2): whether `sess` holds no live hold at `now`. A `said` it records, by `talk said` or
/// `talk ask`, is then taken as sent at once, with its author's reply (`with_reply`): no release
/// or close of its will come to mark it. In the lab's sixth round (l6), a chat worker that held
/// nothing posted two replies that stayed pending in the holder's brief, and the holder recorded
/// that the support update had not been delivered.
fn holds_nothing(s: &State, sess: SessionId, now: Time) -> bool {
    !s.ix.holds_by_session.get(&sess).is_some_and(|ts| {
        ts.iter()
            .any(|t| s.holds.get(t).is_some_and(|h| now < h.lease_until))
    })
}

/// The `said` entry of a `talk said` or a `talk ask` to a person on `n` (D.8.15), then, 1.11
/// (§9.2), its confirmation `with_reply` when its author holds nothing (`alone`).
fn said_entry(
    ex: &mut Exec<'_>,
    n: NodeId,
    k: &Key,
    kind: SaidKind,
    corrects: Option<SaidId>,
    who: Who,
    text: &Text,
) -> SaidId {
    let by = ex.author();
    let a = ex.append(
        n,
        by,
        EntryBody::Said {
            conv: k.key.clone(),
            kind,
            corrects,
            voice: who.voice,
        },
        main(text),
    );
    if who.alone {
        ex.emit(Ev::SaidConfirmed {
            said: a,
            via: ConfirmVia::Recorded,
            link: None,
        });
    }
    a
}

/// What a `said`'s author is in its conversation, read on the pre-state (1.11, §9.2): the
/// conversation's voice by its cap (`voice`), and a session that holds nothing (`alone`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Who {
    voice: bool,
    alone: bool,
}

impl Who {
    fn of(s: &State, c: NodeId, sess: SessionId, k: &KeyStr, now: Time) -> Who {
        Who {
            voice: voice(s, c, sess, k) == Voice::Caller,
            alone: holds_nothing(s, sess, now),
        }
    }
}

/// `KeyAdded { c, k }` when the conversation node is case 3 of §9.1 (`at`, which gets `k`).
fn add_key(ex: &mut Exec<'_>, c: ConvNode, k: &Key) {
    if c.add_key {
        ex.emit(Ev::KeyAdded {
            node: c.node,
            key: k.clone(),
        });
    }
}

/// A text as an entry's main text.
fn main(t: &Text) -> EntryText {
    EntryText {
        main: t.clone(),
        extra: Vec::new(),
    }
}

// ------------------------------------------------------------------------------------------------
// request

/// The plan of `talk request` (D.8.15).
#[derive(Debug)]
pub(crate) enum RequestPlan {
    /// A request of the same message exists (Q1, A-61): nothing is recorded.
    Existing {
        /// The request.
        request: RequestId,
        /// Its node.
        node: NodeId,
    },
    /// A new request.
    New {
        /// Its conversation node.
        c: ConvNode,
        /// The message.
        origin: Key,
        /// Its conversation key: `where`, else the origin.
        conv: Key,
        /// The request and its requester.
        text: EntryText,
    },
}

/// Validates `talk request {at, text, origin, where?, requester}` (D.8.15, D.8.24's row): `at` →
/// `invalid(EmptyText(Text))` → a `repeat:` origin or `where` is `invalid(ReservedKey)` (A-90) →
/// a known origin is the request it names (Q1) → the conversation node of `where`, else the
/// origin, which must not be reserved (`invalid(ReservedNode)`, C1). Recording needs no hold.
pub(crate) fn request(cx: &Vcx<'_>, cmd: &Cmd) -> Result<RequestPlan, Error> {
    let Cmd::Talk(TalkCmd::Request {
        text,
        origin,
        where_,
        requester,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let at = at_of(cx, cmd)?;
    if text.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::Text)));
    }
    if origin.key.is_reserved() || where_.as_ref().is_some_and(|k| k.key.is_reserved()) {
        return Err(Error::invalid(Why::ReservedKey));
    }
    if let Some(&r) = s.ix.origins.get(&origin.key) {
        let node = s.requests.get(&r).map_or(at, |x| x.node);
        return Ok(RequestPlan::Existing { request: r, node });
    }
    let conv = where_.clone().unwrap_or_else(|| origin.clone());
    let c = conv_node(s, at, &conv.key)?;
    Ok(RequestPlan::New {
        c,
        origin: origin.clone(),
        conv,
        text: EntryText {
            main: text.clone(),
            extra: alloc::vec![(TextField::Requester, requester.clone())],
        },
    })
}

/// Executes `talk request`: `KeyAdded` for case 3, then `record_request` on the conversation node,
/// which routes an unrouted entry of a `user:e<n>` origin there (D.8.8).
pub(crate) fn request_exec(ex: &mut Exec<'_>, plan: RequestPlan) -> Reply {
    match plan {
        RequestPlan::Existing { request, node } => Reply::Request {
            request,
            node,
            existing: true,
        },
        RequestPlan::New {
            c,
            origin,
            conv,
            text,
        } => {
            add_key(ex, c, &conv);
            let (request, existed) = record_request(ex, c.node, origin, conv.key, None, text);
            let node = ex.s().requests.get(&request).map_or(c.node, |r| r.node);
            Reply::Request {
                request,
                node,
                existing: existed,
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// said

/// The plan of `talk said` (D.8.15).
#[derive(Debug)]
pub(crate) struct SaidPlan {
    c: ConvNode,
    k: Key,
    kind: SaidKind,
    corrects: Option<SaidId>,
    text: Text,
    who: Who,
    warnings: Vec<Warning>,
}

/// Validates `talk said {at, where?, text, kind, corrects?, seen?}` (D.8.15, D.8.24's row): `at` →
/// `invalid(SaidKindQuestion)` → `corrects` names a `said` (`not_found(Said)`) → the conversation
/// (`invalid(ReservedKey | NoConversationKey | AmbiguousConversationKey | ReservedNode)`). The
/// warnings `moved`, `conversation_held`, and `agreement` are computed here, on the pre-state.
pub(crate) fn said(cx: &Vcx<'_>, cmd: &Cmd) -> Result<SaidPlan, Error> {
    let Cmd::Talk(TalkCmd::Said {
        where_,
        text,
        kind,
        corrects,
        seen,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let at = at_of(cx, cmd)?;
    let sess = session(cx)?;
    if *kind == SaidKind::Question {
        return Err(Error::invalid(Why::SaidKindQuestion));
    }
    if let Some(x) = corrects
        && !s.saids.contains_key(x)
    {
        return Err(Error::not_found(What::Said));
    }
    let (k, c) = conversation(s, at, where_.as_ref())?;
    Ok(SaidPlan {
        c,
        who: Who::of(s, c.node, sess, &k.key, cx.now),
        warnings: said_warnings(s, at, c.node, sess, &k.key, *seen),
        k,
        kind: *kind,
        corrects: *corrects,
        text: text.clone(),
    })
}

/// 1.12 (§9.2): whether conversation node `c` carries a request of conversation `k` in memory,
/// settled or not. A `said` of kind `reply` or `result` that answers nothing where none is warns
/// `answered_nothing`: in the lab's seventh round (it6 l3), a chat worker answered two questions
/// in a thread with no request recorded for either, and nothing noticed.
fn has_request(s: &State, c: NodeId, k: &KeyStr) -> bool {
    s.ix.items.get(&c).is_some_and(|it| {
        it.requests
            .iter()
            .any(|r| s.requests.get(r).is_some_and(|x| x.conv == *k))
    })
}

/// The open commitments on `c` of conversation `k`, ascending (§9.2: refreshed and fulfilled only
/// in their own conversation).
fn open_commitments(s: &State, c: NodeId, k: &Key) -> Vec<CommitmentId> {
    s.ix.items
        .get(&c)
        .into_iter()
        .flat_map(|it| it.open_commitments.iter().copied())
        .filter(|x| s.commitments.get(x).is_some_and(|y| y.conv == k.key))
        .collect()
}

/// Executes `talk said` (D.8.15): `KeyAdded` for case 3; the `said` entry on the conversation
/// node; then, for an `ack`, the refresh of the open commitments of its conversation there, or a
/// new commitment (A-26); for a `reply`, `result`, or `correction`, the fulfilment of every open
/// commitment of its conversation there (§9.2), then the answer of every unanswered request of its
/// conversation there that is not a user request (§9.4), each followed by `try_settle`. The reply
/// names the requests it answered and settled and the commitments it fulfilled (1.4, R9). 1.10
/// (§9.2): a `said` by a session that holds neither the conversation node nor work inside it,
/// while another session does, does all of that too, with `conversation_held`; 1.9 withheld it,
/// and in the lab's fifth round (l6) the chat worker's answer was withheld while the holder could
/// not post in the chat.
pub(crate) fn said_exec(ex: &mut Exec<'_>, plan: SaidPlan) -> Reply {
    let SaidPlan {
        c,
        k,
        kind,
        corrects,
        text,
        who,
        warnings,
    } = plan;
    let now = ex.now();
    add_key(ex, c, &k);
    let n = c.node;
    let a = said_entry(ex, n, &k, kind, corrects, who, &text);
    let mut answered = Vec::new();
    let mut settled = Vec::new();
    let mut fulfilled = Vec::new();
    let open = open_commitments(ex.s(), n, &k);
    match kind {
        SaidKind::Ack => {
            let followup_at = now.plus(ex.s().params.commitment_followup);
            if open.is_empty() {
                ex.emit(Ev::CommitmentOpened {
                    c: a,
                    node: n,
                    conv: k.key.clone(),
                    followup_at,
                });
            } else {
                for x in open {
                    ex.emit(Ev::CommitmentRefreshed { c: x, followup_at });
                }
            }
        }
        SaidKind::Reply | SaidKind::Result | SaidKind::Correction => {
            for x in open {
                ex.emit(Ev::CommitmentFulfilled { c: x, by: a });
                fulfilled.push(x);
            }
            let s = ex.s();
            let unanswered: Vec<RequestId> =
                s.ix.items
                    .get(&n)
                    .and_then(|it| it.unanswered.get(&k.key))
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|r| {
                        *r < a
                            && s.requests
                                .get(r)
                                .is_some_and(|x| x.from_user.is_none() && x.answered_by.is_none())
                    })
                    .collect();
            for r in unanswered {
                ex.emit(Ev::RequestAnswered { request: r, by: a });
                answered.push(r);
                if try_settle(ex, r) {
                    settled.push(r);
                }
            }
        }
        // Refused at validation: only `talk ask` records a question (§9.2).
        SaidKind::Question => {}
    }
    for w in warnings {
        ex.warn(w);
    }
    if matches!(kind, SaidKind::Reply | SaidKind::Result)
        && answered.is_empty()
        && !has_request(ex.s(), n, &k.key)
    {
        ex.warn(Warning {
            code: WarningCode::AnsweredNothing,
            id: Some(n),
            detail: WarningDetail::AnsweredNothing {
                node: n,
                conv: k.key.clone(),
            },
        });
    }
    Reply::Said {
        said: a,
        node: n,
        answered,
        settled,
        fulfilled,
        keyed: c.add_key.then(|| k.key.clone()),
    }
}

// ------------------------------------------------------------------------------------------------
// said {said_id, confirmed}

/// The plan of `talk said {said_id, confirmed}` (D.8.15).
#[derive(Debug)]
pub(crate) struct SaidConfirmPlan {
    said: SaidId,
    link: Text,
    already: bool,
}

/// Validates `talk said {said_id, confirmed}` (D.8.15): the `said` exists (`not_found(Said)`).
pub(crate) fn said_confirm(cx: &Vcx<'_>, cmd: &Cmd) -> Result<SaidConfirmPlan, Error> {
    let Cmd::Talk(TalkCmd::SaidConfirm { said, confirmed }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let x = cx.s.saids.get(said).ok_or(Error::not_found(What::Said))?;
    Ok(SaidConfirmPlan {
        said: *said,
        link: confirmed.clone(),
        already: x.state == SaidState::Confirmed,
    })
}

/// Executes `talk said {said_id, confirmed}`: pending or unconfirmed → confirmed, with the link
/// (§25); an already confirmed `said` changes nothing.
pub(crate) fn said_confirm_exec(ex: &mut Exec<'_>, plan: SaidConfirmPlan) -> Reply {
    if !plan.already {
        ex.emit(Ev::SaidConfirmed {
            said: plan.said,
            via: ConfirmVia::Explicit,
            link: Some(plan.link),
        });
    }
    Reply::SaidConfirmed {
        already: plan.already,
    }
}

// ------------------------------------------------------------------------------------------------
// heard

/// The plan of `talk heard` (D.8.15).
#[derive(Debug)]
pub(crate) struct HeardPlan {
    c: ConvNode,
    k: Key,
    answers: Option<QuestionId>,
    text: EntryText,
}

/// Validates `talk heard {at, where?, from, text, answers?}` (D.8.15, D.8.24): `at` → `answers`
/// names a question (`not_found(Question)`) → the conversation (`invalid(ReservedKey |
/// NoConversationKey | AmbiguousConversationKey | ReservedNode)`) → `answers` is a people question
/// (`invalid(NotPeopleQuestion)`). D.14.5's one order puts `not_found` before `invalid`.
pub(crate) fn heard(cx: &Vcx<'_>, cmd: &Cmd) -> Result<HeardPlan, Error> {
    let Cmd::Talk(TalkCmd::Heard {
        where_,
        from,
        text,
        answers,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let at = at_of(cx, cmd)?;
    let kind = match answers {
        Some(q) => Some(question_kind(s, *q).ok_or(Error::not_found(What::Question))?),
        None => None,
    };
    let (k, c) = conversation(s, at, where_.as_ref())?;
    if kind.is_some_and(|x| x != QuestionKind::People) {
        return Err(Error::invalid(Why::NotPeopleQuestion));
    }
    Ok(HeardPlan {
        c,
        k,
        answers: *answers,
        text: EntryText {
            main: text.clone(),
            extra: alloc::vec![(TextField::From, from.clone())],
        },
    })
}

/// Executes `talk heard`: `KeyAdded` for case 3; the `heard` entry on the conversation node; with
/// `answers: q`, `answer_pq(q, h)` when `q` is open, expired, or escalated (§9.3). An answer to an
/// answered or superseded question is recorded and changes no state (§25).
pub(crate) fn heard_exec(ex: &mut Exec<'_>, plan: HeardPlan) -> Reply {
    let by = ex.author();
    add_key(ex, plan.c, &plan.k);
    let h = ex.append(
        plan.c.node,
        by,
        EntryBody::Heard {
            conv: plan.k.key.clone(),
            answers: plan.answers,
        },
        plan.text,
    );
    let answered = plan.answers.filter(|q| {
        ex.s().pquestions.get(q).is_some_and(|pq| {
            matches!(
                pq.state,
                PqState::Open | PqState::Expired | PqState::Escalated { .. }
            )
        })
    });
    if let Some(q) = answered {
        answer_pq(ex, q, h);
    }
    Reply::Heard {
        entry: h,
        node: plan.c.node,
        answered,
        keyed: plan.c.add_key.then(|| plan.k.key.clone()),
    }
}

// ------------------------------------------------------------------------------------------------
// ask, to a person

/// The plan of `talk ask` to a person (D.8.15).
#[derive(Debug)]
pub(crate) enum AskPersonPlan {
    /// The conversation has an open question already, and `again` is not set (§9.3, A-25).
    Existing {
        /// The question.
        question: QuestionId,
        /// Its conversation node.
        node: NodeId,
    },
    /// A new question.
    New {
        c: ConvNode,
        k: Key,
        blocking: bool,
        text: Text,
        /// With `again`: the open question it supersedes, and the nodes whose wait on it is still
        /// unresolved, read before the supersession (A-109).
        old: Option<(QuestionId, Vec<NodeId>)>,
        /// 1.11 (§9.2): what its author is in the conversation.
        who: Who,
        warnings: Vec<Warning>,
    },
}

/// Validates `talk ask {at, text, where?, again?, blocking?}` to a person (D.8.15): as `said`, the
/// target and then the conversation. An open question of the conversation on its node is returned
/// unless `again` (§9.3: asking only once); after a merge there may be several, and the first is
/// the one returned, or superseded with `again`.
pub(crate) fn ask_person(cx: &Vcx<'_>, cmd: &Cmd) -> Result<AskPersonPlan, Error> {
    let Cmd::Talk(TalkCmd::Ask {
        text,
        to: AskTo::Person { where_, blocking },
        again,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let at = at_of(cx, cmd)?;
    let sess = session(cx)?;
    let (k, c) = conversation(s, at, where_.as_ref())?;
    let open =
        s.ix.items
            .get(&c.node)
            .and_then(|it| it.open_pq.get(&k.key))
            .and_then(|qs| qs.first().copied());
    if let Some(q) = open
        && !*again
    {
        return Ok(AskPersonPlan::Existing {
            question: q,
            node: c.node,
        });
    }
    let old = open.map(|q| {
        let rechecks = s.pquestions.get(&q).map_or(0, |pq| pq.rechecks);
        (q, resolving(s, q, rechecks))
    });
    Ok(AskPersonPlan::New {
        who: Who::of(s, c.node, sess, &k.key, cx.now),
        warnings: said_warnings(s, at, c.node, sess, &k.key, None),
        c,
        k,
        blocking: *blocking,
        text: text.clone(),
        old,
    })
}

/// Executes `talk ask` to a person (D.8.15): `KeyAdded` for case 3; the `said` of kind question
/// (pending, so the next release or close confirms it); the question, with its first recheck at
/// `now + question.recheck` (A-24); with `again`, the old question superseded and the waits on it
/// that are still unresolved moved to the new one (§9.3, A-109).
pub(crate) fn ask_person_exec(ex: &mut Exec<'_>, plan: AskPersonPlan) -> Reply {
    let (c, k, blocking, text, old, who, warnings) = match plan {
        AskPersonPlan::Existing { question, node } => {
            return Reply::Asked {
                question,
                said: None,
                node,
                existing: true,
                keyed: None,
            };
        }
        AskPersonPlan::New {
            c,
            k,
            blocking,
            text,
            old,
            who,
            warnings,
        } => (c, k, blocking, text, old, who, warnings),
    };
    let now = ex.now();
    let by = ex.author();
    add_key(ex, c, &k);
    let n = c.node;
    let a = said_entry(ex, n, &k, SaidKind::Question, None, who, &text);
    let recheck_at = now.plus(ex.s().params.question_recheck);
    let q = ex.append(
        n,
        by,
        EntryBody::Question(QuestionBody::People {
            conv: k.key.clone(),
            blocking,
            said: a,
            supersedes: old.as_ref().map(|(o, _)| *o),
            recheck_at,
        }),
        main(&text),
    );
    if let Some((o, waiting)) = old {
        ex.emit(Ev::PqSuperseded { q: o, by: q });
        for w in waiting {
            ex.emit(Ev::WaitSet {
                node: w,
                wait: Some(WaitOn::Question {
                    q,
                    rechecks_at_set: 0,
                }),
            });
        }
    }
    for w in warnings {
        ex.warn(w);
    }
    Reply::Asked {
        question: q,
        said: Some(a),
        node: n,
        existing: false,
        keyed: c.add_key.then(|| k.key.clone()),
    }
}

#[cfg(test)]
#[path = "talk_people_tests.rs"]
pub(super) mod tests;
