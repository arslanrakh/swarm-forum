//! `spec::guide` (design D.8.26, D.9.1, §20.9): the next calls written naively from §20.9 over the
//! base maps, the oracle that the proptests compare `query::guide` with (D.12.3, item 8).
//!
//! It scans `saids`, `user_entries`, `requests`, `decisions`, `commitments`, `objections`,
//! `pquestions`, `uquestions`, `digest`, `entries`, and `holds` instead of the indices of D.4.10;
//! it computes flags with `spec::flags`, candidates with `spec::ranked`, a conversation's holder
//! by a preorder walk over every live hold, owed replies and decisions per request as §20.9's
//! terms define them, and whether a take or a merge would pass by running it through `decide` on
//! the state. It shares only the item types, `finish` (the partition, dedupe, and cut, which
//! `check_guide` checks on its own), the fixed values, D.8.26's bound on the requests of a node
//! that rules 6 and 7 read (`request_scan`, the oldest first), and `inbox` (compared with its own
//! naive form elsewhere); and the words of a text, `words::doc_words` (1.9) and
//! `words::names_pull_request` (1.10), which `words`' own tests check. 1.10's `left_unfinished`,
//! the naive form of §20.9's term, is `check_guide`'s too (G1 (d)).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::{ancestors_or_self, flags, live_holds, ranked, resolve, same_person, stale};
use crate::cmd::{
    AskTo, BasisArg, Cmd, Ctx, HoldCmd, NextQuery, NodeCmd, RetractArg, SessionCmd, SessionCtx,
    TalkCmd,
};
use crate::error::{Error, Hint, LimitKind, What, Why};
use crate::query::guide::{
    Because, CHECKPOINT_EVERY, End, Guidance, LEASE_MARGIN, Moment, NEXT_IDS_MAX, NextCall,
    RouteAlt, RouteSk, Rule, Skeleton, finish, request_scan,
};
use crate::query::{MOVED_MAX, SCAN_MAX};
use crate::reply::{Done, NextReply, Reply, RoutedReply, Total, WarningCode};
use crate::state::{
    AqState, Author, CommitmentState, DecisionState, DigestState, Hold, Node, ObjectionState,
    PqState, Role, Routing, SaidState, State, UqSource, UqState, live,
};
use crate::types::{
    ClassName, Eid, EntryType, Flag, KeyStr, NodeId, NodeRef, OFFER, Outcome, PrincipalId,
    RequestId, Requester, SECOND, SaidKind, Seq, SessionId, SignalKind, Time, ToUserKind, Token,
    USER_PRINCIPAL,
};

/// §20.9's next calls of a response, naively (D.8.26's `spec::guide`).
#[must_use]
pub fn guide(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
) -> Guidance {
    guide_with(s, sess, now, cmd, out, None)
}

/// `guide` with the first row of the call's `related` (1.11, §20.9 "Inputs"), naively.
#[must_use]
pub fn guide_with(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
) -> Guidance {
    let mut g = finish(s, cmd, items(s, sess, now, cmd, out, related));
    // 1.7 (§20.9 "End"): with no item, after a `next` that found nothing with an empty
    // `leave_better`, or a `hello` after which `next` would offer the session nothing; 1.10 (rule
    // E): a `hello` refused for a `from_user` that names no principal too.
    let refused_unnamed = matches!(cmd, Cmd::Session(SessionCmd::Hello { .. }))
        && matches!(out, Err(e) if e.hint == Hint::Invalid(Why::NoPrincipal));
    if g.items.is_empty()
        && let Some(x) = sess.filter(|x| s.sessions.contains_key(x))
    {
        g.end = match out.as_ref().ok().map(|d| &d.reply) {
            None if refused_unnamed && ranked(s, x, now, &NextQuery::default()).is_empty() => {
                Some(End::AtHello)
            }
            Some(Reply::Next(NextReply::Empty { leave_better, .. })) if leave_better.is_empty() => {
                Some(End::AfterNext)
            }
            Some(Reply::Hello { .. }) if ranked(s, x, now, &NextQuery::default()).is_empty() => {
                Some(End::AtHello)
            }
            // 1.8 (rule 13): a worker with no live hold, after a release, a resolve, or a reopen.
            Some(
                Reply::Released { .. } | Reply::ObjectionResolved { .. } | Reply::Reopened { .. },
            ) if s.sessions.get(&x).is_some_and(|y| y.principals.is_empty())
                && !s.holds.values().any(|h| h.session == x && live(h, now))
                && ranked(s, x, now, &NextQuery::default()).is_empty() =>
            {
                Some(End::Idle)
            }
            _ => None,
        };
    }
    g
}

/// 1.10 (§20.9 Terms), naively: request `r` is *left unfinished* after the call `cmd` with reply
/// `ok`: its node is open and not a container (reserved, a series, or a person or team), it was
/// answered, and the node's last checkpoint came after its last answer, the latest of the entry
/// that answered it and every `said` of kind `reply`, `result`, or `correction` on its node, in
/// its conversation, that is not withheld; a `to_user` of kind `result` that this call recorded,
/// naming it in `re` (directly or through its user entry) or in the reply's `re`, is later than
/// any checkpoint. `check_guide` reads it for G1 (d).
#[must_use]
pub fn left_unfinished(s: &State, cmd: &Cmd, ok: Option<&Reply>, r: RequestId) -> bool {
    let Some(q) = s.requests.get(&r) else {
        return false;
    };
    let Some(answer) = q.answered_by else {
        return false;
    };
    let Some(node) = s.nodes.get(&q.node) else {
        return false;
    };
    let container = node.place.is_none()
        || matches!(node.origin, crate::state::Origin::Reserved)
        || matches!(node.role, Role::Series(_))
        || node
            .parent()
            .is_some_and(|p| p == crate::types::PEOPLE || p == crate::types::TEAMS);
    if !matches!(node.life, crate::state::Life::Open) || container {
        return false;
    }
    if let Cmd::Talk(TalkCmd::ToUser {
        kind: ToUserKind::Result,
        re,
        ..
    }) = cmd
    {
        let through_entry = |e: &Eid| {
            s.user_entries.get(e).is_some_and(
                |u| matches!(u.routing, Routing::Request { request, .. } if request == r),
            )
        };
        let in_reply = matches!(ok, Some(Reply::ToUser { re, .. }) if re.contains(&r));
        if re.iter().any(|e| *e == r || through_entry(e)) || in_reply {
            return false;
        }
    }
    let last = s
        .saids
        .iter()
        .filter(|(_, x)| {
            x.node == q.node
                && x.conv == q.conv
                && matches!(
                    x.kind,
                    SaidKind::Reply | SaidKind::Result | SaidKind::Correction
                )
                && x.state != SaidState::Withheld
        })
        .map(|(y, _)| *y)
        .fold(answer, Eid::max);
    node.last_checkpoint.is_some_and(|cp| cp > last)
}

fn it(rule: Rule, when: Moment, once: bool, call: Skeleton, because: Because) -> NextCall {
    NextCall {
        rule,
        when,
        once,
        call,
        because,
    }
}

fn first5<T: Copy>(xs: impl IntoIterator<Item = T>) -> Vec<T> {
    xs.into_iter().take(NEXT_IDS_MAX).collect()
}

/// The items in the order of §20.9's rules, before `finish`.
fn items(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
) -> Vec<NextCall> {
    let mut v = Vec::new();
    match out {
        Ok(d) if matches!(d.reply, Reply::Replay) => return v,
        Err(e)
            if matches!(
                e.hint,
                Hint::Limit(LimitKind::TextTooLong | LimitKind::Busy | LimitKind::WritesPerHour)
            ) =>
        {
            return v;
        }
        Err(e)
            if matches!(
                e.hint,
                Hint::NotFound(What::Session) | Hint::Invalid(Why::ClassMismatch)
            ) =>
        {
            v.push(it(
                Rule::Remedy,
                Moment::Now,
                true,
                Skeleton::Hello,
                Because::NoSession,
            ));
            return v;
        }
        Ok(_) | Err(_) => {}
    }
    let Some(x) = sess.filter(|x| s.sessions.contains_key(x)) else {
        return v;
    };
    let n = Naive {
        s,
        x,
        now,
        cmd,
        out,
        ok: out.as_ref().ok().map(|d| &d.reply),
        related,
    };
    if let Err(e) = out {
        v.extend(n.remedy(e));
    }
    let ok = out.is_ok();
    if ok {
        v.extend(n.rule1());
    }
    v.extend(n.rule2());
    // 1.12's review: rule 3's due checkpoint is `later` on a hold whose node rule 8 closes now.
    let closes = n.rule8();
    let closing_now: BTreeSet<Token> = closes
        .iter()
        .filter(|i| i.when == Moment::Now)
        .filter_map(|i| {
            if let Skeleton::Close { token, .. } = &i.call {
                Some(*token)
            } else {
                None
            }
        })
        .collect();
    v.extend(n.rule3(&closing_now));
    v.extend(n.rule4());
    v.extend(n.rule5());
    let closing: BTreeSet<NodeId> = closes
        .iter()
        .filter_map(|i| {
            if let Skeleton::Close { node, .. } = &i.call {
                Some(*node)
            } else {
                None
            }
        })
        .collect();
    v.extend(n.rule6());
    v.extend(n.rule7(&closing));
    v.extend(closes);
    if ok {
        v.extend(n.rule9());
    }
    v.extend(n.rule10());
    v.extend(n.rule11());
    if ok {
        v.extend(n.rule12());
    }
    v.extend(n.rule13());
    v
}

struct Naive<'a> {
    s: &'a State,
    x: SessionId,
    now: Time,
    cmd: &'a Cmd,
    out: &'a Result<Done, Error>,
    ok: Option<&'a Reply>,
    /// 1.11: the first row of the call's `related`.
    related: Option<NodeId>,
}

impl Naive<'_> {
    /// "Your holds": every live hold the session took, in token order.
    fn holds(&self) -> Vec<(Token, &Hold)> {
        self.s
            .holds
            .iter()
            .filter(|(_, h)| h.session == self.x && live(h, self.now))
            .map(|(t, h)| (*t, h))
            .collect()
    }

    fn open(&self, n: NodeId) -> bool {
        self.s.nodes.get(&n).is_some_and(Node::is_open)
    }

    /// 1.8 (§20.9 "Another's node"): some other session holds `n` live, and this one does not.
    fn others(&self, n: NodeId) -> bool {
        let on_n = || {
            self.s
                .holds
                .iter()
                .filter(|(t, h)| t.node == n && live(h, self.now))
        };
        on_n().any(|(_, h)| h.session != self.x) && !on_n().any(|(_, h)| h.session == self.x)
    }

    /// The open nodes the session holds.
    fn held_open(&self) -> BTreeSet<NodeId> {
        self.holds()
            .into_iter()
            .map(|(t, _)| t.node)
            .filter(|n| self.open(*n))
            .collect()
    }

    fn ctx(&self, session: SessionCtx) -> Ctx {
        Ctx {
            now: self.now,
            class: self
                .s
                .sessions
                .get(&self.x)
                .map_or_else(ClassName::unknown, |x| x.class.clone()),
            caps: None,
            session,
            op: None,
        }
    }

    /// A take of `n` succeeds now: `decide` issues a new hold.
    fn take_ok(&self, n: NodeId) -> bool {
        let cmd = Cmd::Hold(HoldCmd::Take {
            target: NodeRef::Id(n),
            lease: None,
        });
        matches!(
            crate::decide(self.s, &self.ctx(SessionCtx::Existing(self.x)), &cmd).1,
            Ok(Done {
                reply: Reply::Taken {
                    existing: false,
                    ..
                },
                ..
            })
        )
    }

    fn take_or_view(&self, n: NodeId) -> Skeleton {
        if self.take_ok(n) {
            Skeleton::Take { target: n }
        } else {
            Skeleton::ViewNode { node: n }
        }
    }

    /// 1.9 (§10.4): the first open objection of `n`, by id, while `n` is closed: what a review of
    /// it decides.
    fn review_objection(&self, n: NodeId) -> Option<crate::types::ObjectionId> {
        if self.open(n) || !self.s.nodes.contains_key(&n) {
            return None;
        }
        self.s
            .objections
            .iter()
            .find(|(_, o)| o.node == n && o.state == ObjectionState::Open)
            .map(|(&o, _)| o)
    }

    fn token_live(&self, t: Token) -> bool {
        self.s.holds.get(&t).is_some_and(|h| live(h, self.now))
    }

    fn serves_several(&self) -> bool {
        let Some(x) = self.s.sessions.get(&self.x) else {
            return false;
        };
        x.principals
            .iter()
            .any(|&p| x.principals.iter().any(|&q| !same_person(self.s, p, q)))
    }

    // E

    fn remedy(&self, e: &Error) -> Option<NextCall> {
        let s = self.s;
        let reread = |n: NodeId| {
            let node = s.nodes.get(&n)?;
            Some((
                Skeleton::ViewNode { node: n },
                Because::Reread {
                    version: node.version,
                    digest_version: node.digest_version,
                },
            ))
        };
        let (call, because) = match &e.hint {
            Hint::StaleToken { token, late } => {
                let n = if let Cmd::Node(NodeCmd::Update { at, .. } | NodeCmd::Close { at, .. }) =
                    self.cmd
                {
                    resolve(s, at)
                } else {
                    Some(token.node)
                }?;
                s.nodes.get(&n)?;
                (self.take_or_view(n), Because::HoldEnded { late: *late })
            }
            Hint::StaleVersion { .. } | Hint::Invalid(Why::MissingDigestVersion) => {
                reread(crate::apply::target(s, self.cmd).ok().flatten()?)?
            }
            Hint::Invalid(Why::VersionAhead) => {
                let n = if let Cmd::Talk(TalkCmd::Approve { on, .. }) = self.cmd {
                    resolve(s, on)
                } else if let Cmd::Talk(TalkCmd::Decide { basis, .. }) = self.cmd {
                    basis.iter().find_map(|b| {
                        if let BasisArg::Node {
                            at,
                            version: Some(v),
                        } = b
                        {
                            resolve(s, at)
                                .filter(|n| s.nodes.get(n).is_some_and(|x| *v > x.version))
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }?;
                reread(n)?
            }
            Hint::Closed {
                merged_into: Some(m),
                ..
            } => {
                s.nodes.get(m)?;
                (self.take_or_view(*m), Because::MergedInto { node: *m })
            }
            // 1.9 (§21): a take refused for an open objection on a closed node: its review.
            Hint::Closed {
                node,
                merged_into: None,
            } if matches!(self.cmd, Cmd::Hold(HoldCmd::Take { .. })) => {
                let o = self.review_objection(*node)?;
                (
                    Skeleton::Resolve { objection: o },
                    Because::Review { objection: o },
                )
            }
            Hint::Invalid(Why::IntentOpen) => {
                let Cmd::Hold(HoldCmd::Checkpoint { token, .. }) = self.cmd else {
                    return None;
                };
                if !self.token_live(*token) {
                    return None;
                }
                let i = s.nodes.get(&token.node)?.open_intent?;
                (
                    Skeleton::CheckpointSwap { token: *token },
                    Because::CloseOpenIntent {
                        by: i.session,
                        entry: i.eid,
                        own: i.session == self.x,
                    },
                )
            }
            Hint::Limit(LimitKind::Renew) => {
                let Cmd::Hold(HoldCmd::Renew { token, .. }) = self.cmd else {
                    return None;
                };
                if !self.token_live(*token) {
                    return None;
                }
                (
                    Skeleton::Checkpoint {
                        token: *token,
                        intent_done: false,
                        intent: false,
                    },
                    Because::RenewLimit,
                )
            }
            Hint::Invalid(
                Why::NotPeopleQuestion | Why::NotAgentQuestion | Why::NotUserQuestion,
            ) => {
                let Cmd::Talk(
                    TalkCmd::Heard {
                        answers: Some(q), ..
                    }
                    | TalkCmd::FromUser {
                        answers: Some(q), ..
                    }
                    | TalkCmd::Answer { question: q, .. },
                ) = self.cmd
                else {
                    return None;
                };
                let q = *q;
                let call = if let Some(pq) = s.pquestions.get(&q) {
                    Skeleton::Heard {
                        at: pq.node,
                        answers: q,
                        where_: pq.conv.clone(),
                    }
                } else if s.aquestions.contains_key(&q) {
                    Skeleton::AnswerAgent { question: q }
                } else if let Some(uq) = s.uquestions.get(&q) {
                    let to = uq
                        .to
                        .get(usize::from(uq.stage))
                        .copied()
                        .unwrap_or(USER_PRINCIPAL);
                    Skeleton::AnswerUser {
                        question: q,
                        principal: self.serves_several().then_some(to),
                    }
                } else {
                    return None;
                };
                (call, Because::AnswerFits { question: q })
            }
            Hint::Full { .. }
            | Hint::HoldsLimit { .. }
            | Hint::Paused { .. }
            | Hint::Cycle { .. }
            | Hint::Closed { .. }
            | Hint::OpIdConflict
            | Hint::Limit(_)
            | Hint::Invalid(_)
            | Hint::NotFound(_) => return None,
        };
        Some(it(Rule::Remedy, Moment::Now, true, call, because))
    }

    // 1 to 5

    fn rule1(&self) -> Option<NextCall> {
        let Cmd::Session(SessionCmd::Hello {
            principal: Some(k),
            from_user: None,
            ..
        }) = self.cmd
        else {
            return None;
        };
        let p = *self.s.principal_ids.get(k)?;
        Some(it(
            Rule::LogFirst,
            Moment::Now,
            true,
            Skeleton::LogFromUser { principal: p },
            Because::LogFirst { principal: p },
        ))
    }

    /// The people entries on `c` (request, said, heard, question, answer), by seq.
    fn people_on(&self, c: NodeId) -> Vec<(Seq, Eid)> {
        let mut out: Vec<(Seq, Eid)> = self
            .s
            .entries
            .iter()
            .filter(|(_, m)| {
                m.node == c
                    && matches!(
                        m.ty,
                        EntryType::Request
                            | EntryType::Said
                            | EntryType::Heard
                            | EntryType::Question
                            | EntryType::Answer
                    )
            })
            .map(|(e, m)| (m.seq, *e))
            .collect();
        out.sort();
        out
    }

    /// The session holding work in `c`'s subtree that the caller is told about (§9.2): None when
    /// the caller holds `c` or a node under it; else, over a preorder walk of `c`'s subtree,
    /// children by id, each node's holds by token: the first holder whose caps include the cap of
    /// `k`'s scheme (1.11, §9.1), else None when the caller has that cap (it is the conversation's
    /// voice), else the first holder.
    fn conversation_holder(&self, c: NodeId, conv: &KeyStr) -> Option<SessionId> {
        let s = self.s;
        let holds: Vec<(Token, &Hold)> = s.holds.iter().map(|(t, h)| (*t, h)).collect();
        let under = |n: NodeId| ancestors_or_self(s, n).contains(&c);
        if holds
            .iter()
            .any(|(t, h)| h.session == self.x && under(t.node))
        {
            return None;
        }
        let cap = conv.scheme();
        let capable = |x: SessionId| {
            s.sessions
                .get(&x)
                .is_some_and(|y| y.caps.iter().any(|l| l.as_str() == cap))
        };
        let mut preorder: Vec<SessionId> = Vec::new();
        let mut stack = alloc::vec![c];
        let mut steps = 0_usize;
        while let Some(n) = stack.pop() {
            steps = steps.saturating_add(1);
            if steps > s.nodes.len().saturating_add(1) {
                break;
            }
            preorder.extend(
                holds
                    .iter()
                    .filter(|(t, _)| t.node == n)
                    .map(|(_, h)| h.session),
            );
            let mut kids: Vec<NodeId> = s
                .nodes
                .iter()
                .filter(|(_, x)| x.parent() == Some(n))
                .map(|(k, _)| *k)
                .filter(|k| {
                    holds
                        .iter()
                        .any(|(t, _)| ancestors_or_self(s, t.node).contains(k))
                })
                .collect();
            kids.sort();
            stack.extend(kids.into_iter().rev());
        }
        let first = preorder.first().copied()?;
        if let Some(h) = preorder.iter().copied().find(|h| capable(*h)) {
            return Some(h);
        }
        if capable(self.x) {
            return None;
        }
        Some(first)
    }

    /// Rule 2 (1.9): the `say` to the holder of the conversation of the `said` this call
    /// recorded; no item confirms a `said` (§9.2).
    fn rule2(&self) -> Vec<NextCall> {
        let s = self.s;
        let recorded = match self.ok {
            Some(
                Reply::Said { said, .. }
                | Reply::Asked {
                    said: Some(said), ..
                },
            ) => Some(*said),
            _ => None,
        };
        let mut v = Vec::new();
        for (&y, said) in &s.saids {
            // 1.9 (§9.2): a `talk said` recorded while another session held its conversation, or
            // an `ask`'s: the `say` to the holder alone (1.10: it counts; 1.9 withheld it).
            // 1.11 (§9.2): one its author recorded holding nothing is confirmed at once.
            if said.session != self.x
                || !matches!(
                    said.state,
                    SaidState::Pending | SaidState::Withheld | SaidState::Confirmed
                )
                || recorded != Some(y)
            {
                continue;
            }
            let c = said.node;
            if let Some(h) = self.conversation_holder(c, &said.conv) {
                v.push(it(
                    Rule::TellHolder,
                    Moment::Now,
                    true,
                    Skeleton::SayTo { at: c, to: h },
                    Because::TellHolder { holder: h },
                ));
            }
        }
        v
    }

    fn issued(&self) -> Option<Token> {
        match self.ok {
            Some(Reply::Created { token, .. }) => *token,
            Some(Reply::Taken {
                token,
                existing: false,
                ..
            }) => Some(*token),
            Some(Reply::Next(NextReply::Hold { token, .. })) => Some(*token),
            _ => None,
        }
    }

    fn rule3(&self, closing_now: &BTreeSet<Token>) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        let uncovered = self.out.as_ref().is_ok_and(|d| {
            d.warnings
                .iter()
                .any(|w| w.code == WarningCode::UncoveredSlot)
        });
        for (t, h) in self.holds() {
            // Due at the earlier of 5 minutes after the last extension and 1 minute before the
            // lease ends, never before that extension (§20.9).
            let five = h.last_extended.plus(CHECKPOINT_EVERY);
            let margin = h.lease_until.minus(LEASE_MARGIN);
            let due = if five < margin { five } else { margin };
            let due = if due < h.last_extended {
                h.last_extended
            } else {
                due
            };
            let intent = s.nodes.get(&t.node).and_then(|n| n.open_intent);
            if self.now >= due {
                let closing = closing_now.contains(&t);
                v.push(it(
                    Rule::Checkpoint,
                    if closing { Moment::Later } else { Moment::Now },
                    false,
                    Skeleton::Checkpoint {
                        token: t,
                        intent_done: false,
                        intent: false,
                    },
                    Because::LeaseDue {
                        lease_until: h.lease_until,
                        at_ceiling: h.lease_until == h.taken_at.plus(h.terms.ceiling),
                        intent: intent.map(|i| i.eid),
                        closing,
                    },
                ));
            } else if let Some(i) = intent {
                let own = i.session == self.x;
                v.push(it(
                    Rule::IntentDone,
                    if own { Moment::Later } else { Moment::Now },
                    false,
                    Skeleton::Checkpoint {
                        token: t,
                        intent_done: true,
                        intent: false,
                    },
                    Because::IntentOpen {
                        by: i.session,
                        entry: i.eid,
                        own,
                    },
                ));
            } else if self.issued() == Some(t) {
                if uncovered {
                    v.push(it(
                        Rule::Checkpoint,
                        Moment::Now,
                        true,
                        Skeleton::Checkpoint {
                            token: t,
                            intent_done: false,
                            intent: true,
                        },
                        Because::UncoveredSlot,
                    ));
                } else {
                    v.push(it(
                        Rule::Checkpoint,
                        Moment::Later,
                        true,
                        Skeleton::Checkpoint {
                            token: t,
                            intent_done: false,
                            intent: false,
                        },
                        Because::CheckpointBy { by: due },
                    ));
                }
            }
        }
        v
    }

    /// Entries by others on `m` past the session's `seen`, after `since` (§20.9 rule 4).
    fn news(&self, m: NodeId, since: Time) -> Option<Total> {
        let s = self.s;
        let seen = s
            .sessions
            .get(&self.x)
            .and_then(|x| x.shown.get(&m))
            .copied()
            .unwrap_or(Seq(0));
        let max = u32::try_from(MOVED_MAX).unwrap_or(u32::MAX);
        let mut count = 0_u32;
        for (visited, (_, e)) in self
            .people_on(m)
            .into_iter()
            .filter(|(q, _)| *q > seen)
            .rev()
            .enumerate()
        {
            if u32::try_from(visited).unwrap_or(u32::MAX) >= SCAN_MAX {
                return (count > 0).then_some(Total::AtLeast(count));
            }
            let Some(meta) = s.entries.get(&e) else {
                continue;
            };
            if meta.at <= since {
                break;
            }
            if meta.by == Author::Session(self.x) {
                continue;
            }
            if count >= max {
                return Some(Total::AtLeast(max));
            }
            count = count.saturating_add(1);
        }
        (count > 0).then_some(Total::Exact(count))
    }

    /// The nearest node from `n` up that carries a conversation key.
    fn conversation_node(&self, n: NodeId) -> Option<NodeId> {
        let schemes = &self.s.params.conversation_schemes;
        ancestors_or_self(self.s, n).into_iter().find(|m| {
            self.s
                .nodes
                .get(m)
                .is_some_and(|x| x.keys.keys().any(|k| k.is_conversation(schemes)))
        })
    }

    fn rule4(&self) -> Vec<NextCall> {
        let mut v = Vec::new();
        for (t, h) in self.holds() {
            let n = t.node;
            if let Some(count) = self.news(n, h.taken_at) {
                v.push(it(
                    Rule::Reread,
                    Moment::Now,
                    false,
                    Skeleton::ViewNode { node: n },
                    Because::News { node: n, count },
                ));
            } else if let Some(c) = self.conversation_node(n)
                && c != n
                && let Some(count) = self.news(c, Time::ZERO)
            {
                v.push(it(
                    Rule::Reread,
                    Moment::Now,
                    false,
                    Skeleton::ViewNode { node: c },
                    Because::News { node: c, count },
                ));
            }
        }
        v
    }

    /// Rule 5 (1.7, 1.9): per unrouted entry of the session, one item, the first of: its route as
    /// the answer to the first question pending for its person (by `uq_rank`); its route to none
    /// after a `to_user` that named it; its route to the session's only open hold, else to that
    /// question's open node, not reserved; its `node create` by key. The second of these, if any,
    /// is the alternative its `why` names.
    fn rule5(&self) -> Vec<NextCall> {
        let s = self.s;
        let named: BTreeSet<Eid> = match (self.ok, self.cmd) {
            (Some(_), Cmd::Talk(TalkCmd::ToUser { re, .. })) => re.clone(),
            _ => BTreeSet::new(),
        };
        let held: Vec<NodeId> = self.held_open().into_iter().collect();
        let only_hold = match held.as_slice() {
            [n] => Some(*n),
            _ => None,
        };
        let mut v = Vec::new();
        for (&e, u) in &s.user_entries {
            if u.session != self.x || u.routing != Routing::Unrouted {
                continue;
            }
            let pending = s
                .uquestions
                .iter()
                .filter(|(_, q)| {
                    q.state == UqState::Pending
                        && q.to
                            .get(usize::from(q.stage))
                            .is_some_and(|&t| same_person(s, t, u.principal))
                })
                .map(|(&q, x)| (crate::index::uq_rank(s, q, x.about, x.due), q))
                .min()
                .map(|(_, q)| q);
            let about = pending
                .and_then(|q| s.uquestions.get(&q))
                .and_then(|q| q.about)
                .filter(|n| !n.is_reserved() && self.open(*n));
            let to = match (only_hold, about) {
                (Some(n), _) => Some((n, true)),
                (None, Some(n)) => Some((n, false)),
                (None, None) => None,
            };
            // 1.11 (§8.2): on the only hold, the newest unsettled request of the entry's person,
            // among the node's newest SCAN_MAX unsettled requests.
            let clarifies = only_hold.and_then(|n| {
                let mut on: Vec<RequestId> = s
                    .requests
                    .iter()
                    .filter(|(_, q)| q.node == n && q.settled.is_none())
                    .map(|(r, _)| *r)
                    .collect();
                on.sort_unstable();
                on.into_iter()
                    .rev()
                    .take(usize::try_from(SCAN_MAX).unwrap_or(usize::MAX))
                    .find(|r| {
                        matches!(s.requests.get(r).map(|q| q.requester),
                            Some(Requester::User(p)) if same_person(s, p, u.principal))
                    })
            });
            // 1.11: the first related row, for the entry this call logged; 1.12: an open work
            // node, which the create's `why` names: not reserved, a signal, a container (a series,
            // a person or team), or under `@people` or `@teams`, and with no open child.
            let logged = matches!(self.ok, Some(Reply::FromUser { entry, .. }) if *entry == e)
                || matches!(self.ok, Some(Reply::Hello { user_entry: Some(x), .. }) if x.entry == e);
            let related = self.related.filter(|r| {
                logged
                    && s.nodes.get(r).is_some_and(|x| {
                        x.is_open()
                            && !r.is_reserved()
                            && x.origin != crate::state::Origin::Reserved
                            && !matches!(x.role, Role::Signal(_) | Role::Series(_))
                    })
                    && !s
                        .nodes
                        .values()
                        .any(|c| c.parent() == Some(*r) && c.is_open())
                    && !super::ancestors(s, *r)
                        .iter()
                        .any(|a| *a == crate::types::PEOPLE || *a == crate::types::TEAMS)
            });
            // The routings that apply, in order: (skeleton, its alternative's form).
            let mut mine: Vec<(Option<RouteSk>, Option<RouteAlt>)> = Vec::new();
            if let Some(q) = pending {
                mine.push((Some(RouteSk::Answers(q)), None));
            }
            if named.contains(&e) {
                mine.push((Some(RouteSk::Nowhere), Some(RouteAlt::Nowhere)));
            }
            if let Some(r) = clarifies {
                mine.push((Some(RouteSk::Clarifies(r)), None));
            }
            if let Some((n, held)) = to {
                mine.push((
                    Some(RouteSk::Node(n)),
                    Some(RouteAlt::Node { node: n, held }),
                ));
            }
            mine.push((None, Some(RouteAlt::Create)));
            let or = mine.get(1).and_then(|(_, alt)| *alt);
            let Some(&(first, _)) = mine.first() else {
                continue;
            };
            let because = match first {
                Some(RouteSk::Answers(q)) => Because::RouteAnswers {
                    entry: e,
                    question: q,
                    or,
                },
                Some(RouteSk::Nowhere) => Because::AnsweredEntry { entry: e, or },
                Some(RouteSk::Clarifies(r)) => Because::RouteClarifies {
                    entry: e,
                    request: r,
                    or,
                },
                Some(RouteSk::Node(n)) => Because::RouteNode {
                    entry: e,
                    node: n,
                    held: to.is_some_and(|(_, h)| h),
                    or,
                },
                Some(RouteSk::Fill) | None => Because::Unrouted { entry: e, related },
            };
            let call = match first {
                Some(to) => Skeleton::Route { entry: e, to },
                None => Skeleton::CreateFor { entry: e },
            };
            v.push(it(Rule::Route, Moment::Now, false, call, because));
        }
        v
    }

    // 6 to 8: requests, per request as §20.9's terms define them

    fn unsettled(&self, r: RequestId) -> bool {
        self.s.requests.get(&r).is_some_and(|q| q.settled.is_none())
    }

    /// Owed a reply: a decision that is not retracted lists it, and no answer has been recorded.
    fn owed_reply(&self, r: RequestId) -> bool {
        self.s.requests.get(&r).is_some_and(|q| {
            q.settled.is_none()
                && q.answered_by.is_none()
                && self.s.decisions.values().any(|d| {
                    !matches!(d.state, DecisionState::Retracted { .. }) && d.settles.contains(&r)
                })
        })
    }

    /// Owed a decision: an answer has been recorded and it is not settled.
    fn owed_decision(&self, r: RequestId) -> bool {
        self.s
            .requests
            .get(&r)
            .is_some_and(|q| q.settled.is_none() && q.answered_by.is_some())
    }

    fn closed_node(&self) -> Option<NodeId> {
        match self.ok {
            Some(Reply::Closed { node, .. }) => Some(*node),
            _ => None,
        }
    }

    fn named(&self) -> BTreeSet<RequestId> {
        let s = self.s;
        let mut out = BTreeSet::new();
        if let Cmd::Talk(TalkCmd::Decide { settles, .. })
        | Cmd::Hold(HoldCmd::Release { settles, .. })
        | Cmd::Node(NodeCmd::Close {
            settles: Some(settles),
            ..
        }) = self.cmd
        {
            out.extend(settles.iter().copied());
        }
        if let Cmd::Talk(TalkCmd::ToUser { re, .. }) = self.cmd {
            for e in re {
                out.insert(*e);
                for (r, q) in &s.requests {
                    if q.from_user == Some(*e)
                        && s.user_entries.get(e).is_some_and(|u| {
                            matches!(u.routing, Routing::Request { request, .. } if request == *r)
                        })
                    {
                        out.insert(*r);
                    }
                }
            }
        }
        // 1.8 (§8.1): the requests `to_user {at}` named in `re`.
        if let Some(Reply::ToUser { re, .. }) = self.ok {
            out.extend(re.iter().copied());
        }
        out.retain(|r| s.requests.contains_key(r));
        out
    }

    fn recorded(&self) -> BTreeSet<RequestId> {
        let mut out = BTreeSet::new();
        let routed = |r: &RoutedReply| {
            if let RoutedReply::Request { request, .. } = r {
                Some(*request)
            } else {
                None
            }
        };
        match self.ok {
            Some(Reply::FromUser { routed: r, .. }) => out.extend(routed(r)),
            Some(Reply::Hello {
                user_entry: Some(u),
                ..
            }) => out.extend(routed(&u.routed)),
            Some(Reply::Routed {
                routed: r,
                existing: false,
                ..
            }) => out.extend(routed(r)),
            Some(Reply::Request {
                request,
                existing: false,
                ..
            }) => {
                out.insert(*request);
            }
            Some(
                Reply::Created {
                    request,
                    user_requests,
                    ..
                }
                | Reply::Joined {
                    request,
                    user_requests,
                    ..
                },
            ) => {
                out.extend(request.iter().copied());
                out.extend(user_requests.iter().copied());
            }
            Some(Reply::KeyAdded {
                user_request: Some(r),
                ..
            }) => {
                out.insert(*r);
            }
            _ => {}
        }
        out.retain(|r| self.s.requests.contains_key(r));
        out
    }

    /// In reach (§20.9): on a node you hold, recorded by the call, named by the call, or on the
    /// node the call closed; never on a node another session holds and you do not (1.8).
    fn in_reach(&self, r: RequestId) -> bool {
        let Some(q) = self.s.requests.get(&r) else {
            return false;
        };
        !self.others(q.node)
            && (self.held_open().contains(&q.node)
                || self.closed_node() == Some(q.node)
                || self.named().contains(&r)
                || self.recorded().contains(&r))
    }

    fn rule6(&self) -> Vec<NextCall> {
        let s = self.s;
        let held = self.held_open();
        // In reach and owed a reply (§20.9), within D.8.26's bounds: on a node whose every
        // request is in reach (one you hold, or the node the call closed), the oldest
        // `request_scan()` of those owed a reply; elsewhere, the ones the call names or recorded.
        let mut whole = held.clone();
        whole.extend(self.closed_node());
        let mut owed: BTreeSet<RequestId> = BTreeSet::new();
        for n in &whole {
            owed.extend(
                s.requests
                    .iter()
                    .filter(|(r, q)| q.node == *n && self.owed_reply(**r))
                    .map(|(r, _)| *r)
                    .take(request_scan()),
            );
        }
        let mut partial = self.named();
        partial.extend(self.recorded());
        owed.extend(
            partial
                .into_iter()
                .filter(|&r| self.owed_reply(r) && self.in_reach(r)),
        );
        let mut mis = BTreeSet::new();
        let mut to = None;
        if let (
            Some(Reply::ToUser { principal, .. }),
            Cmd::Talk(TalkCmd::ToUser {
                kind: ToUserKind::Result,
                ..
            }),
        ) = (self.ok, self.cmd)
        {
            for r in self.named() {
                let Some(q) = s.requests.get(&r) else {
                    continue;
                };
                let other = match q.requester {
                    Requester::External => true,
                    Requester::User(p) => !same_person(s, p, *principal),
                };
                if q.settled.is_none() && q.answered_by.is_none() && other && self.in_reach(r) {
                    mis.insert(r);
                }
            }
            if !mis.is_empty() {
                to = Some(*principal);
            }
        }
        owed.extend(mis.iter().copied());
        let mut v = self.reply_groups(&owed, &held, &mis, to, false, false);
        // 1.7: the unanswered requests no decision lists on a held node that carries
        // `ready_to_close` (now, standing), and those this call recorded on a held node (later).
        let unanswered = |r: &RequestId| {
            s.requests
                .get(r)
                .is_some_and(|q| q.settled.is_none() && q.answered_by.is_none())
        };
        let mut ready: BTreeSet<RequestId> = BTreeSet::new();
        for &n in &held {
            if !flags(s, n, self.now).contains(Flag::ReadyToClose) {
                continue;
            }
            ready.extend(
                s.requests
                    .iter()
                    .filter(|(_, q)| q.node == n && q.settled.is_none())
                    .map(|(r, _)| *r)
                    .take(request_scan())
                    .filter(|r| unanswered(r) && !self.owed_reply(*r)),
            );
        }
        v.extend(self.reply_groups(&ready, &held, &BTreeSet::new(), None, false, true));
        let recorded: BTreeSet<RequestId> = self
            .recorded()
            .into_iter()
            .filter(|r| {
                unanswered(r)
                    && !ready.contains(r)
                    && !self.owed_reply(*r)
                    && s.requests.get(r).is_some_and(|q| held.contains(&q.node))
            })
            .collect();
        v.extend(self.reply_groups(&recorded, &held, &BTreeSet::new(), None, true, false));
        v
    }

    /// Rule 6's items for the requests `owed`: one `to_user` per requester person, one `said` per
    /// conversation; `recorded` (1.7): later and once, else now, standing on a held node.
    fn reply_groups(
        &self,
        owed: &BTreeSet<RequestId>,
        held: &BTreeSet<NodeId>,
        mis: &BTreeSet<RequestId>,
        to: Option<PrincipalId>,
        recorded: bool,
        ready: bool,
    ) -> Vec<NextCall> {
        let s = self.s;
        let mut users: Vec<(PrincipalId, Vec<RequestId>)> = Vec::new();
        let mut convs: Vec<((NodeId, KeyStr), Vec<RequestId>)> = Vec::new();
        for &r in owed {
            let Some(q) = s.requests.get(&r) else {
                continue;
            };
            match q.requester {
                Requester::User(p) => match users.iter_mut().find(|(o, _)| same_person(s, *o, p)) {
                    Some((_, rs)) => rs.push(r),
                    None => users.push((p, alloc::vec![r])),
                },
                Requester::External => {
                    let key = (q.node, q.conv.clone());
                    match convs.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, rs)) => rs.push(r),
                        None => convs.push((key, alloc::vec![r])),
                    }
                }
            }
        }
        let on_held = |rs: &[RequestId]| {
            !recorded
                && rs
                    .iter()
                    .any(|r| s.requests.get(r).is_some_and(|q| held.contains(&q.node)))
        };
        let when = if recorded { Moment::Later } else { Moment::Now };
        let because = |rs: &[RequestId]| Because::ReplyOwed {
            requests: first5(rs.iter().copied()),
            misaddressed: if rs.iter().any(|r| mis.contains(r)) {
                to
            } else {
                None
            },
            recorded,
            ready,
        };
        let mut v = Vec::new();
        for (p, rs) in users {
            v.push(it(
                Rule::Reply,
                when,
                !on_held(&rs),
                Skeleton::ToUser {
                    principal: p,
                    kind: Some(ToUserKind::Result),
                    re: first5(rs.iter().copied()),
                },
                because(&rs),
            ));
        }
        for ((n, k), rs) in convs {
            // 1.9 (§9.2): none where another session holds the conversation: the holder speaks.
            if super::conv_node(s, n, &k) != n || self.conversation_holder(n, &k).is_some() {
                continue;
            }
            let where_ = (self.default_conv(n).as_ref() != Some(&k)).then_some(k);
            v.push(it(
                Rule::Reply,
                when,
                !on_held(&rs),
                Skeleton::Said {
                    at: n,
                    kind: SaidKind::Result,
                    where_,
                    where_fill: false,
                },
                because(&rs),
            ));
        }
        v
    }

    /// The default conversation key of `n` (§9.1): the one key of the nearest node from `n` up
    /// that carries any; none when that node carries several.
    fn default_conv(&self, n: NodeId) -> Option<KeyStr> {
        let schemes = &self.s.params.conversation_schemes;
        let c = self.conversation_node(n)?;
        let keys: Vec<&KeyStr> = self
            .s
            .nodes
            .get(&c)?
            .keys
            .keys()
            .filter(|k| k.is_conversation(schemes))
            .collect();
        match keys.as_slice() {
            [one] => Some((*one).clone()),
            _ => None,
        }
    }

    /// The node this call released (1.7, rule 7).
    fn released_node(&self) -> Option<NodeId> {
        match (self.ok, self.cmd) {
            (Some(Reply::Released { .. }), Cmd::Hold(HoldCmd::Release { token, .. })) => {
                Some(token.node)
            }
            _ => None,
        }
    }

    /// A container (§4): reserved, a series, or a person or team.
    fn container(&self, n: NodeId) -> bool {
        self.s.nodes.get(&n).is_none_or(|node| {
            node.place.is_none()
                || matches!(node.origin, crate::state::Origin::Reserved)
                || matches!(node.role, Role::Series(_))
                || node
                    .parent()
                    .is_some_and(|p| p == crate::types::PEOPLE || p == crate::types::TEAMS)
        })
    }

    fn rule7(&self, closing: &BTreeSet<NodeId>) -> Vec<NextCall> {
        let s = self.s;
        let held = self.held_open();
        let closed = self.closed_node();
        let released = self.released_node();
        let recorded = self.recorded();
        let mut nodes: BTreeSet<NodeId> = BTreeSet::new();
        for (&r, q) in &s.requests {
            if self.in_reach(r) {
                nodes.insert(q.node);
            }
        }
        nodes.extend(held.iter().copied());
        nodes.extend(closed);
        nodes.extend(released.filter(|n| !self.others(*n)));
        let mut v = Vec::new();
        for n in nodes {
            let is_held = held.contains(&n);
            let container = self.container(n);
            // 1.7: on a node the session holds, rule 8's close records the decision when it is
            // suggested; on another held node only answered requests are decided, and a container,
            // which no close finishes, is decided as before.
            if is_held && !container && closing.contains(&n) {
                continue;
            }
            let after_release = released == Some(n) && !is_held;
            let ready = is_held && container && flags(s, n, self.now).contains(Flag::ReadyToClose);
            let whole = ready || closed == Some(n);
            let on_n: Vec<RequestId> = s
                .requests
                .iter()
                .filter(|(_, q)| q.node == n)
                .map(|(r, _)| *r)
                .collect();
            // On a node read whole, among its oldest `request_scan()` unsettled requests (D.8.26's
            // bounds).
            let main: BTreeSet<RequestId> = if whole {
                on_n.iter()
                    .copied()
                    .filter(|&r| self.unsettled(r))
                    .take(request_scan())
                    .filter(|&r| !self.owed_reply(r))
                    .collect()
            } else if is_held || after_release {
                // 1.10: never a request left unfinished, among the oldest `request_scan()` of
                // those owed a decision.
                on_n.iter()
                    .copied()
                    .filter(|&r| self.owed_decision(r))
                    .take(request_scan())
                    .filter(|&r| !left_unfinished(s, self.cmd, self.ok, r))
                    .collect()
            } else {
                on_n.iter()
                    .copied()
                    .filter(|&r| {
                        self.owed_decision(r)
                            && self.in_reach(r)
                            && !left_unfinished(s, self.cmd, self.ok, r)
                    })
                    .collect()
            };
            let nobody = live_holds(s, n, self.now) == 0;
            let rec: BTreeSet<RequestId> = on_n
                .iter()
                .copied()
                .filter(|r| recorded.contains(r) && (nobody || (is_held && container)))
                .collect();
            let d: BTreeSet<RequestId> = main.union(&rec).copied().collect();
            if d.is_empty() {
                continue;
            }
            v.push(it(
                Rule::Decide,
                if is_held && main.is_empty() {
                    Moment::Later
                } else {
                    Moment::Now
                },
                !is_held,
                Skeleton::Decide {
                    at: n,
                    settles: first5(d.iter().copied()),
                    supersedes: None,
                },
                Because::DecisionOwed {
                    requests: first5(d.iter().copied()),
                    after_close: closed == Some(n),
                    ready,
                    recorded: !rec.is_empty(),
                    held: is_held,
                    after_release,
                },
            ));
        }
        v
    }

    /// Whether the unsettled request `r` has a result answer (1.7, §5), naively: for a user
    /// request, its answer; for another, a `said` of kind result or correction after it, on its
    /// node, in its conversation, not unconfirmed.
    fn result_answered(&self, r: RequestId) -> bool {
        let s = self.s;
        let Some(q) = s.requests.get(&r) else {
            return false;
        };
        if q.settled.is_some() || q.answered_by.is_none() {
            return false;
        }
        match q.requester {
            Requester::User(_) => true,
            Requester::External => s.saids.iter().any(|(&y, x)| {
                y > r
                    && x.node == q.node
                    && x.conv == q.conv
                    && matches!(x.kind, SaidKind::Result | SaidKind::Correction)
                    && x.state != SaidState::Unconfirmed
            }),
        }
    }

    fn rule8(&self) -> Vec<NextCall> {
        let s = self.s;
        let settled_out: Vec<RequestId> = match self.ok {
            Some(
                Reply::Said { settled, .. }
                | Reply::ToUser { settled, .. }
                | Reply::Decided { settled, .. }
                | Reply::Closed { settled, .. }
                | Reply::Released { settled, .. },
            ) => settled.clone(),
            _ => Vec::new(),
        };
        let mut v = Vec::new();
        for (t, h) in self.holds() {
            let n = t.node;
            let Some(node) = s.nodes.get(&n).filter(|x| x.is_open()) else {
                continue;
            };
            let u: Vec<RequestId> = s
                .requests
                .iter()
                .filter(|(_, q)| q.node == n && q.settled.is_none())
                .map(|(r, _)| *r)
                .collect();
            let settled_here = settled_out
                .iter()
                .any(|r| s.requests.get(r).is_some_and(|q| q.node == n));
            let series = matches!(node.role, Role::Series(_));
            let container = node.place.is_none()
                || matches!(node.origin, crate::state::Origin::Reserved)
                || series
                || node
                    .parent()
                    .is_some_and(|p| p == crate::types::PEOPLE || p == crate::types::TEAMS);
            if container {
                let when = if settled_here && u.is_empty() {
                    Some(Moment::Now)
                } else if self.issued() == Some(t) {
                    Some(Moment::Later)
                } else {
                    None
                };
                if let Some(when) = when {
                    v.push(it(
                        Rule::ReleaseContainer,
                        when,
                        true,
                        Skeleton::Release {
                            token: t,
                            wait: None,
                        },
                        Because::ContainerStays { series },
                    ));
                }
                continue;
            }
            let open_child = s
                .nodes
                .values()
                .any(|x| x.parent() == Some(n) && x.is_open());
            let commitment = s
                .commitments
                .values()
                .any(|c| c.node == n && c.state == CommitmentState::Open);
            let objection = s
                .objections
                .values()
                .any(|o| o.node == n && o.state == ObjectionState::Open);
            let other_hold = s
                .holds
                .iter()
                .any(|(k, x)| k.node == n && x.session != self.x && live(x, self.now));
            let other_intent = node.open_intent.is_some_and(|i| i.session != self.x);
            let answered = u
                .iter()
                .all(|r| s.requests.get(r).is_some_and(|q| q.answered_by.is_some()));
            let clean = !open_child
                && !commitment
                && !objection
                && !other_hold
                && !other_intent
                && answered;
            if !clean || u.len() > NEXT_IDS_MAX {
                continue;
            }
            // 1.11 (§20.9 rule 8): "all answered" counts no request left unfinished.
            let standing = (!u.is_empty()
                && !u.iter().any(|&r| left_unfinished(s, self.cmd, self.ok, r)))
                || (flags(s, n, self.now).contains(Flag::ReadyToClose) && u.is_empty());
            let decided = u.is_empty()
                && matches!(self.cmd, Cmd::Talk(TalkCmd::Decide { at, .. }) if resolve(s, at) == Some(n));
            let intent_closed = u.is_empty()
                && matches!(
                    self.ok,
                    Some(Reply::Checkpointed {
                        intent_done: Some(_),
                        intent: None,
                        ..
                    })
                )
                && matches!(self.cmd, Cmd::Hold(HoldCmd::Checkpoint { token, .. }) if *token == t);
            let leaves_nothing = !s
                .saids
                .values()
                .any(|y| y.node == n && y.state == SaidState::Unconfirmed)
                && !s.decisions.values().any(|d| {
                    d.node == n
                        && d.state == DecisionState::Active
                        && d.basis.iter().any(|b| stale(s, b))
                });
            let reclosed = h.reopened.is_some() && self.cleared(n) && leaves_nothing;
            // 1.12: on a node whose last `intent_done`, or whose open intent, names a pull
            // request, a decision this call recorded, or the last request it settled, makes the
            // close `later`.
            let pr = node.pr_named || node.open_intent.is_some_and(|i| i.pr_named);
            let reacted = (settled_here && u.is_empty()) || decided;
            let once_now = (reacted && !pr) || reclosed;
            if !(standing || once_now || intent_closed || (reacted && pr)) {
                continue;
            }
            let later = !standing && !once_now;
            let answered_unsettled = u.clone();
            // 1.7 (§5): the close settles by default what a result answered.
            let u = if u.iter().all(|&r| self.result_answered(r)) {
                Vec::new()
            } else {
                u
            };
            v.push(it(
                Rule::Close,
                if later { Moment::Later } else { Moment::Now },
                !standing,
                Skeleton::Close {
                    node: n,
                    token: t,
                    settles: u,
                    outcome: match &h.reopened {
                        Some(Outcome::Missed) | None => None,
                        Some(o) => Some(o.clone()),
                    },
                },
                match &h.reopened {
                    Some(was) => Because::Reclose { was: was.clone() },
                    None if pr => Because::MergeWait {
                        settles: answered_unsettled,
                    },
                    None => Because::Finished,
                },
            ));
        }
        // 1.10: after a `handoff` release of a node that is not a container, whose `intent_done`
        // names a pull request, its take, when a take would succeed.
        if let (
            Some(Reply::Released { .. }),
            Cmd::Hold(HoldCmd::Release {
                token,
                reason: crate::types::ReleaseReason::Handoff,
                waits_for: None,
                intent_done: Some(done),
                ..
            }),
        ) = (self.ok, self.cmd)
            && !self.container(token.node)
            && crate::words::names_pull_request(&done.0)
            && self.take_ok(token.node)
        {
            v.push(it(
                Rule::AwaitMerge,
                Moment::Now,
                true,
                Skeleton::Take { target: token.node },
                Because::AwaitMerge,
            ));
        }
        v
    }

    /// This call removed on `n` an item a closed `n` would be flagged for (D.8.26 `cleared`).
    fn cleared(&self, n: NodeId) -> bool {
        let s = self.s;
        let on = |r: &RequestId| s.requests.get(r).is_some_and(|q| q.node == n);
        match (self.ok, self.cmd) {
            (
                Some(Reply::Said {
                    settled, fulfilled, ..
                }),
                _,
            ) => {
                settled.iter().any(on)
                    || fulfilled
                        .iter()
                        .any(|c| s.commitments.get(c).is_some_and(|x| x.node == n))
            }
            (Some(Reply::ToUser { settled, .. }), _) => settled.iter().any(on),
            (Some(Reply::Decided { decision, .. }), _) => {
                s.decisions.get(decision).is_some_and(|d| d.node == n)
            }
            (
                Some(Reply::ObjectionResolved { entry: Some(_), .. }),
                Cmd::Talk(TalkCmd::Resolve { objection, .. }),
            ) => s.objections.get(objection).is_some_and(|o| o.node == n),
            (
                Some(Reply::HoldCheck {
                    node,
                    intent_done: Some(_),
                    ..
                }),
                _,
            ) => *node == n,
            (
                Some(Reply::SaidConfirmed { already: false }),
                Cmd::Talk(TalkCmd::SaidConfirm { said, .. }),
            ) => s.saids.get(said).is_some_and(|y| y.node == n),
            (
                Some(Reply::Checkpointed {
                    intent_done: Some(_),
                    ..
                }),
                Cmd::Hold(HoldCmd::Checkpoint { token, .. }),
            ) => token.node == n,
            (Some(Reply::Retracted { .. }), Cmd::Talk(TalkCmd::Retract { target, .. })) => {
                match target {
                    RetractArg::Entry { node, .. } => *node == n,
                    RetractArg::Node(r) => resolve(s, r) == Some(n),
                }
            }
            _ => false,
        }
    }

    // 9

    fn rule9(&self) -> Vec<NextCall> {
        let s = self.s;
        let Some(reply) = self.ok else {
            return Vec::new();
        };
        let mut v = Vec::new();
        match (self.cmd, reply) {
            (
                Cmd::Talk(TalkCmd::Ask { at, to, .. }),
                Reply::Asked {
                    question, existing, ..
                },
            ) => match to {
                AskTo::Person { .. } | AskTo::User(_) => {
                    let Some(n) = resolve(s, at) else {
                        return v;
                    };
                    let Some((t, _)) = self.holds().into_iter().find(|(t, _)| t.node == n) else {
                        return v;
                    };
                    let blocking = matches!(to, AskTo::Person { .. })
                        && s.pquestions.get(question).is_some_and(|q| q.blocking);
                    v.push(it(
                        Rule::AskWait,
                        if blocking { Moment::Now } else { Moment::Later },
                        true,
                        Skeleton::Release {
                            token: t,
                            wait: Some(*question),
                        },
                        Because::AskWait {
                            question: *question,
                            blocking,
                            existing: *existing,
                        },
                    ));
                }
                AskTo::Agent(_) => v.push(it(
                    Rule::AgentWait,
                    Moment::Later,
                    true,
                    Skeleton::Wait {
                        timeout: s.params.talk_wait_max,
                    },
                    Because::AgentWait {
                        question: *question,
                    },
                )),
            },
            (
                _,
                Reply::Joined {
                    existing,
                    open_question: Some(q),
                    ..
                },
            ) => {
                if let Some(pq) = s.pquestions.get(q) {
                    v.push(it(
                        Rule::JoinAnswer,
                        Moment::Now,
                        true,
                        Skeleton::Heard {
                            at: *existing,
                            answers: *q,
                            where_: pq.conv.clone(),
                        },
                        Because::JoinAnswer { question: *q },
                    ));
                }
            }
            (
                Cmd::Node(NodeCmd::Create(a)),
                Reply::Created {
                    id, token: None, ..
                },
            ) if a.take == Some(false) => {
                if let Some(node) = s.nodes.get(id)
                    && node.capacity >= 1
                    && !node.tags.iter().any(|t| t.as_str() == OFFER)
                    && flags(s, *id, self.now).is_empty()
                    && !node.is_container()
                    && node.role == Role::Plain
                {
                    v.push(it(
                        Rule::Offer,
                        Moment::Now,
                        true,
                        Skeleton::AddOffer {
                            node: *id,
                            version: node.version,
                        },
                        Because::NotOffered,
                    ));
                }
            }
            (
                Cmd::Talk(
                    TalkCmd::ToUser {
                        kind: ToUserKind::Question,
                        ..
                    }
                    | TalkCmd::FromUser {
                        answers: Some(_), ..
                    },
                ),
                _,
            ) => {
                let Some(x) = s.sessions.get(&self.x) else {
                    return v;
                };
                let serves = |p: PrincipalId| x.principals.iter().any(|&q| same_person(s, q, p));
                let first = s
                    .uquestions
                    .iter()
                    .filter(|(_, u)| u.state == UqState::Pending)
                    .filter_map(|(q, u)| {
                        let to = *u.to.get(usize::from(u.stage))?;
                        serves(to).then(|| (crate::index::uq_rank(s, *q, u.about, u.due), to))
                    })
                    .min();
                if let Some((r, to)) = first {
                    v.push(it(
                        Rule::AnswersNext,
                        Moment::Later,
                        true,
                        Skeleton::AnswerUser {
                            question: r.q,
                            principal: self.serves_several().then_some(to),
                        },
                        Because::AnswersNext {
                            question: r.q,
                            principal: to,
                        },
                    ));
                }
            }
            (_, Reply::Next(NextReply::Status { node: n, .. })) => {
                let n = *n;
                let mut convs: Vec<(Eid, KeyStr)> = Vec::new();
                for (&c, k) in &s.commitments {
                    if k.node != n || k.state != CommitmentState::Open || self.now < k.followup_at {
                        continue;
                    }
                    let asked = s
                        .pquestions
                        .values()
                        .any(|q| q.node == n && q.conv == k.conv && q.state == PqState::Open);
                    if !asked && !convs.iter().any(|(_, x)| *x == k.conv) {
                        convs.push((c, k.conv.clone()));
                    }
                }
                let default = self.default_conv(n);
                for (_, k) in convs {
                    // 1.9 (§9.2): nor where another session holds the conversation.
                    if super::conv_node(s, n, &k) != n || self.conversation_holder(n, &k).is_some()
                    {
                        continue;
                    }
                    let where_ = (default.as_ref() != Some(&k)).then_some(k);
                    v.push(it(
                        Rule::StatusReply,
                        Moment::Now,
                        true,
                        Skeleton::Said {
                            at: n,
                            kind: SaidKind::Reply,
                            where_,
                            where_fill: false,
                        },
                        Because::StatusItem,
                    ));
                }
            }
            // 1.9 (§10.4): a review item asks whether its objection stands.
            (_, Reply::Next(NextReply::Review { objection, .. }))
                if s.objections.contains_key(objection) =>
            {
                v.push(it(
                    Rule::Review,
                    Moment::Now,
                    true,
                    Skeleton::Resolve {
                        objection: *objection,
                    },
                    Because::Review {
                        objection: *objection,
                    },
                ));
            }
            (_, Reply::Next(NextReply::Route { entry, .. })) => v.push(it(
                Rule::RouteItem,
                Moment::Now,
                true,
                Skeleton::Route {
                    entry: *entry,
                    to: RouteSk::Fill,
                },
                Because::RouteItem,
            )),
            // 1.12 (§11.3): a resolve that returned `no_basis`: the reopen of its node, when a
            // reopen of it passes `decide`.
            (Cmd::Talk(TalkCmd::Resolve { .. }), Reply::ObjectionResolved { .. }) => {
                let n = self.out.as_ref().ok().and_then(|d| {
                    d.warnings.iter().find_map(|w| {
                        if let crate::reply::WarningDetail::NoBasis { node, .. } = w.detail
                            && w.code == WarningCode::NoBasis
                        {
                            Some(node)
                        } else {
                            None
                        }
                    })
                });
                if let Some(n) = n
                    && self.reopen_ok(n)
                {
                    v.push(it(
                        Rule::Unproved,
                        Moment::Now,
                        true,
                        Skeleton::Reopen { node: n },
                        Because::Unproved,
                    ));
                }
            }
            _ => {}
        }
        v
    }

    /// 1.12: a reopen of `n` passes `decide` now.
    fn reopen_ok(&self, n: NodeId) -> bool {
        let cmd = Cmd::Node(NodeCmd::Reopen {
            at: NodeRef::Id(n),
            reason: crate::types::Text(alloc::string::String::from("x")),
        });
        crate::decide(self.s, &self.ctx(SessionCtx::Existing(self.x)), &cmd)
            .1
            .is_ok()
    }

    // 10 to 13

    fn rule10(&self) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        for (t, _) in self.holds() {
            let n = t.node;
            let Some(node) = s.nodes.get(&n) else {
                continue;
            };
            let stale_decision = s.decisions.iter().find(|(_, d)| {
                d.node == n
                    && d.state == DecisionState::Active
                    && d.basis.iter().any(|b| stale(s, b))
            });
            let objection = s
                .objections
                .iter()
                .find(|(_, o)| o.node == n && o.state == ObjectionState::Open);
            let depth = ancestors_or_self(s, n).len().saturating_sub(1);
            let agent_children = s
                .nodes
                .values()
                .filter(|x| {
                    x.parent() == Some(n)
                        && x.is_open()
                        && matches!(
                            x.origin,
                            crate::state::Origin::Agent(_) | crate::state::Origin::Ancestor(_)
                        )
                })
                .count();
            let room = u32::try_from(depth).unwrap_or(u32::MAX) < s.params.tree_max_depth
                && u32::try_from(agent_children).unwrap_or(u32::MAX) < s.params.tree_max_children;
            let push = |v: &mut Vec<NextCall>, call: Skeleton, because: Because, when: Moment| {
                v.push(it(Rule::HeldFlag, when, false, call, because));
            };
            // 1.9 (§9.2): no item confirms a `said`, another session's unconfirmed one included.
            if !node.assume_marks.is_empty() {
                let blocking = node.assume_marks.iter().any(|q| {
                    s.uquestions
                        .get(q)
                        .is_some_and(|u| matches!(u.source, UqSource::Escalation { .. }))
                });
                push(
                    &mut v,
                    Skeleton::Decide {
                        at: n,
                        settles: Vec::new(),
                        supersedes: None,
                    },
                    Because::Assumption {
                        questions: first5(node.assume_marks.iter().copied()),
                        blocking,
                    },
                    Moment::Now,
                );
            } else if let Some((&d, _)) = stale_decision {
                push(
                    &mut v,
                    Skeleton::Decide {
                        at: n,
                        settles: Vec::new(),
                        supersedes: Some(d),
                    },
                    Because::StaleBasis { decision: d },
                    Moment::Now,
                );
            } else if let Some(&q) = node.recheck_marks.first() {
                push(
                    &mut v,
                    Skeleton::Release {
                        token: t,
                        wait: Some(q),
                    },
                    Because::Recheck { question: q },
                    Moment::Now,
                );
            } else if node.attempts >= s.params.too_big_attempts && room {
                push(
                    &mut v,
                    Skeleton::Split { parent: n },
                    Because::TooBig {
                        attempts: node.attempts,
                    },
                    Moment::Now,
                );
            } else if let Role::Signal(key) = &node.role {
                let mut merge = None;
                if key.kind == SignalKind::Duplicate {
                    // §16's convention: into the older (`key.a`), unless the newer is held.
                    let newer_held = live_holds(s, key.b, self.now) > 0;
                    let (from, into) = if newer_held {
                        (key.a, key.b)
                    } else {
                        (key.b, key.a)
                    };
                    // 1.8: never a merge that closes a node another session holds.
                    let containers = s.nodes.get(&from).is_none_or(Node::is_container)
                        || s.nodes.get(&into).is_none_or(Node::is_container)
                        || self.others(from);
                    let cmd = Cmd::Node(NodeCmd::Merge {
                        from: NodeRef::Id(from),
                        into: NodeRef::Id(into),
                    });
                    if !containers
                        && crate::decide(s, &self.ctx(SessionCtx::Existing(self.x)), &cmd)
                            .1
                            .is_ok()
                    {
                        merge = Some((from, into));
                    }
                }
                match merge {
                    Some((from, into)) => push(
                        &mut v,
                        Skeleton::Merge { from, into },
                        Because::Duplicate { other: into },
                        Moment::Now,
                    ),
                    None => push(
                        &mut v,
                        Skeleton::Decide {
                            at: n,
                            settles: Vec::new(),
                            supersedes: None,
                        },
                        Because::Signal,
                        Moment::Now,
                    ),
                }
            } else if let Some((&o, _)) = objection {
                // 1.9: the newest retracted decision of a node with no active one.
                let on_n = || s.decisions.iter().filter(|(_, d)| d.node == n);
                let retracted = if on_n().any(|(_, d)| d.state == DecisionState::Active) {
                    None
                } else {
                    on_n()
                        .filter(|(_, d)| matches!(d.state, DecisionState::Retracted { .. }))
                        .map(|(&d, _)| d)
                        .max()
                };
                push(
                    &mut v,
                    Skeleton::Resolve { objection: o },
                    Because::Objection {
                        objection: o,
                        retracted,
                    },
                    Moment::Later,
                );
            }
        }
        // 1.11: after a `node link {rel: duplicate}`, its pair's merge in §16's direction.
        if let (
            Some(Reply::Linked { .. }),
            Cmd::Node(NodeCmd::Link {
                at,
                rel: crate::types::LinkRel::Duplicate,
                to,
            }),
        ) = (self.ok, self.cmd)
            && let (Some(x), Some(y)) = (resolve(s, at), resolve(s, to))
            && x != y
        {
            let (a, b) = (x.min(y), x.max(y));
            let (from, into) = if live_holds(s, b, self.now) > 0 {
                (a, b)
            } else {
                (b, a)
            };
            let containers = s.nodes.get(&from).is_none_or(Node::is_container)
                || s.nodes.get(&into).is_none_or(Node::is_container)
                || self.others(from);
            let cmd = Cmd::Node(NodeCmd::Merge {
                from: NodeRef::Id(from),
                into: NodeRef::Id(into),
            });
            if !containers
                && crate::decide(s, &self.ctx(SessionCtx::Existing(self.x)), &cmd)
                    .1
                    .is_ok()
            {
                v.push(it(
                    Rule::HeldFlag,
                    Moment::Now,
                    true,
                    Skeleton::Merge { from, into },
                    Because::Linked { into },
                ));
            }
        }
        v
    }

    fn rule11(&self) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        let unread = crate::helpers::agents::inbox(s, self.x, self.now).len();
        if unread > 0 {
            v.push(it(
                Rule::Inbox,
                Moment::Now,
                false,
                Skeleton::Wait { timeout: SECOND },
                Because::Inbox {
                    count: u32::try_from(unread).unwrap_or(u32::MAX),
                },
            ));
        }
        let asked: BTreeSet<Eid> = match self.ok {
            Some(Reply::Hello { inbox, .. }) => inbox.iter().copied().collect(),
            Some(Reply::Waited { delivered, .. }) => delivered.iter().copied().collect(),
            _ => BTreeSet::new(),
        };
        // 1.7 (§13): the notes the response to a write delivered; 1.8: answered `later`.
        let on_write: BTreeSet<Eid> = self
            .out
            .as_ref()
            .map(|d| d.delivered.iter().copied().collect())
            .unwrap_or_default();
        for q in asked.union(&on_write).copied() {
            if let Some(aq) = s.aquestions.get(&q)
                && aq.state == AqState::Open
                && aq.asker != self.x
                && let Some(note) = s.notes.get(&q)
            {
                let later = !asked.contains(&q);
                v.push(it(
                    Rule::AnswerAgent,
                    if later { Moment::Later } else { Moment::Now },
                    true,
                    Skeleton::AnswerAgent { question: q },
                    Because::AgentAsked {
                        by: aq.asker,
                        until: note.until,
                        question: q,
                        later,
                    },
                ));
            }
        }
        v
    }

    fn rule12(&self) -> Option<NextCall> {
        let s = self.s;
        let x = s.sessions.get(&self.x)?;
        let mut ps: Vec<(crate::types::PrincipalKey, PrincipalId)> = x
            .principals
            .iter()
            .filter_map(|&p| {
                s.principal_ids
                    .iter()
                    .find(|(_, id)| **id == p)
                    .map(|(k, _)| (k.clone(), p))
            })
            .collect();
        ps.sort();
        for (_, p) in ps {
            if x.told.iter().any(|&t| same_person(s, t, p)) {
                continue;
            }
            let questions = s
                .uquestions
                .values()
                .filter(|u| {
                    u.state == UqState::Pending
                        && u.to
                            .get(usize::from(u.stage))
                            .is_some_and(|&to| same_person(s, to, p))
                })
                .count();
            let digest: Vec<Eid> = s
                .digest
                .iter()
                .filter(|(_, d)| d.state == DigestState::Pending && same_person(s, d.to, p))
                .map(|(e, _)| *e)
                .collect();
            if questions == 0 && digest.is_empty() {
                continue;
            }
            return Some(it(
                Rule::HumanItems,
                Moment::Now,
                false,
                Skeleton::ToUser {
                    principal: p,
                    kind: (questions > 0).then_some(ToUserKind::Question),
                    re: first5(digest.iter().copied()),
                },
                Because::HumanItems {
                    principal: p,
                    questions: u32::try_from(questions).unwrap_or(u32::MAX),
                    digest: u32::try_from(digest.len()).unwrap_or(u32::MAX),
                },
            ));
        }
        None
    }

    /// Rule 13; 1.8: after a release, a resolve, or a reopen (`after_work`), `hold next` too,
    /// after a release's tidy item, while the worker holds nothing and `next` offers it an item.
    /// 1.9: the review of the first `leave_better` node when it is closed with an open objection,
    /// in place of its take; then, where `end` would come, the forum's tidy items.
    fn rule13(&self) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        if !s
            .sessions
            .get(&self.x)
            .is_some_and(|x| x.principals.is_empty())
        {
            return v;
        }
        let hello = matches!(self.cmd, Cmd::Session(SessionCmd::Hello { .. }));
        let mut lb: Vec<NodeId> = Vec::new();
        let mut joined = None;
        let mut empty_next = false;
        let mut after_work = false;
        match (self.ok, self.out.as_ref().err().map(|e| &e.hint)) {
            (Some(Reply::Closed { leave_better, .. }), _) => {
                lb.clone_from(leave_better);
            }
            (Some(Reply::Released { leave_better, .. }), _) => {
                lb.clone_from(leave_better);
                after_work = true;
            }
            (Some(Reply::ObjectionResolved { .. } | Reply::Reopened { .. }), _) => {
                after_work = true;
            }
            (Some(Reply::Next(NextReply::Empty { leave_better, .. })), _) => {
                lb.clone_from(leave_better);
                empty_next = true;
            }
            (Some(Reply::Joined { existing, .. }), _) => joined = Some(*existing),
            (Some(Reply::Hello { .. }), _) if hello => {}
            // 1.10 (rule E): a `hello` refused only for a `from_user` that names no principal.
            (None, Some(Hint::Invalid(Why::NoPrincipal))) if hello => {}
            (
                None,
                Some(
                    Hint::Full { .. }
                    | Hint::Paused { .. }
                    | Hint::Closed {
                        merged_into: None, ..
                    },
                ),
            ) => {}
            // 1.12: any other write but `hold next`: only its `hold next` below.
            (Some(_), None)
                if !self.cmd.is_read() && !matches!(self.cmd, Cmd::Hold(HoldCmd::Next { .. })) => {}
            _ => return v,
        }
        let holds = self.holds().len();
        let per = usize::try_from(s.params.hold_per_session).unwrap_or(usize::MAX);
        // 1.9 (§10.3, §10.4): a closed node with an open objection is tidied by its review.
        if let Some(&m) = lb.first()
            && let Some(o) = self.review_objection(m)
        {
            v.push(it(
                Rule::Tidy,
                Moment::Now,
                true,
                Skeleton::Resolve { objection: o },
                Because::Review { objection: o },
            ));
            if !after_work {
                return v;
            }
        } else if let Some(&m) = lb.first()
            && flags(s, m, self.now) == crate::types::FlagSet::EMPTY.with(Flag::Stale)
        {
            // 1.10 (§16): a stale offer whose only flag is `stale` is tidied by §16's item for it.
            if let Some(item) = self.stale_item(m, false, holds < per) {
                v.push(item);
                if !after_work {
                    return v;
                }
            }
        } else if let Some(&m) = lb.iter().find(|&&m| {
            // 1.12: past reviews and stale offers, the first the session's caps and horizon fit.
            let fit = s.nodes.get(&m).is_some_and(|x| {
                s.sessions.get(&self.x).is_some_and(|y| {
                    x.needs.is_subset(&y.caps)
                        && !matches!((x.est, y.horizon), (Some(e), Some(h)) if e > h)
                })
            });
            self.review_objection(m).is_none()
                && flags(s, m, self.now) != crate::types::FlagSet::EMPTY.with(Flag::Stale)
                && fit
                && self.take_ok(m)
        }) && holds < per
        {
            v.push(it(
                Rule::Tidy,
                Moment::Now,
                true,
                Skeleton::Take { target: m },
                Because::Tidy {
                    node: m,
                    flags: flags(s, m, self.now),
                },
            ));
            if !after_work {
                return v;
            }
        }
        if let Some(j) = joined
            && self.take_ok(j)
        {
            v.push(it(
                Rule::TakeJoined,
                Moment::Now,
                true,
                Skeleton::Take { target: j },
                Because::Joined,
            ));
            return v;
        }
        if holds == 0 {
            if let Some(&first) = ranked(s, self.x, self.now, &NextQuery::default()).first() {
                if !empty_next {
                    // 1.10: after any call but a `hello`, what `next` would give.
                    v.push(it(
                        Rule::MoreWork,
                        Moment::Now,
                        true,
                        Skeleton::Next,
                        if hello {
                            Because::MoreWork { at_hello: true }
                        } else {
                            Because::WorkWaiting { first }
                        },
                    ));
                }
            } else if empty_next || after_work || hello {
                // 1.9: the forum's tidy items, where `end` would come.
                v.extend(self.tidy());
            }
        }
        v
    }

    /// 1.10 (§16): the item of stale offer `n`, naively, by the questions for the user about it:
    /// none while one is pending or lapsed; with one answered or defaulted, the newest, a take
    /// (when `room`, the session below `hold.per_session`, and the take succeeds); with none, the
    /// question for its owners.
    fn stale_item(&self, n: NodeId, cut: bool, room: bool) -> Option<NextCall> {
        match self.stale_step(n)? {
            None => Some(it(
                Rule::Tidy,
                Moment::Now,
                true,
                Skeleton::AskOwners { at: n },
                Because::StaleOffer { node: n, cut },
            )),
            Some(question) => (room && self.take_ok(n)).then(|| {
                it(
                    Rule::Tidy,
                    Moment::Now,
                    true,
                    Skeleton::Take { target: n },
                    Because::StaleAnswered {
                        node: n,
                        question,
                        cut,
                    },
                )
            }),
        }
    }

    /// 1.10 (§16): none while a question for the user about stale offer `n` is pending or lapsed;
    /// else the newest one answered or defaulted, if any.
    fn stale_step(&self, n: NodeId) -> Option<Option<crate::types::QuestionId>> {
        let mut answered = None;
        for (&q, u) in &self.s.uquestions {
            if u.about != Some(n) {
                continue;
            }
            match u.state {
                UqState::Pending | UqState::Lapsed => return None,
                UqState::Answered { .. } | UqState::Defaulted => answered = Some(q),
            }
        }
        Some(answered)
    }

    /// 1.9 (§10.3, §16, rule 13): the forum's tidy items, naively: every stale offer the session
    /// can take, by id, and every pair of open, plain children of a parent of an open, plain
    /// candidate that share at least `ALIKE_MIN` words that at most `RARE_DF` of all those children
    /// carry, with one of them in each title, most shared first; a stale offer and a pair in turn, at most `TIDY_MAX`, `cut` when
    /// some were left out. It has no scan bound: the proptests' states lie far below `TIDY_SCAN`.
    fn tidy(&self) -> Vec<NextCall> {
        use crate::query::leave_better::TIDY_MAX;
        use crate::words::{ALIKE_MIN, RARE_DF, doc_words};
        use alloc::string::String;
        let s = self.s;
        let now = self.now;
        let plain = |n: NodeId| {
            !n.is_reserved()
                && s.nodes.get(&n).is_some_and(|x| {
                    x.is_open() && matches!(x.role, Role::Plain) && !x.is_container()
                })
        };
        let cand = |n: NodeId| super::classify(s, n, now) == Some(crate::state::Cand::Node(n));
        let mut stale: Vec<NodeId> = Vec::new();
        for (&n, node) in &s.nodes {
            let held_back =
                node.depends_on.iter().any(|d| super::open(s, *d)) || super::paused(s, n, now);
            // 1.10: none while its owners are being asked.
            if flags(s, n, now).contains(Flag::Stale)
                && cand(n)
                && !held_back
                && self.take_ok(n)
                && self.stale_step(n).is_some()
            {
                stale.push(n);
            }
        }
        let parents: BTreeSet<NodeId> = s
            .nodes
            .iter()
            .filter(|(n, _)| cand(**n) && plain(**n))
            .filter_map(|(_, x)| x.parent())
            .collect();
        // Every open, plain child of those parents, with its words.
        let read: Vec<(NodeId, NodeId, crate::words::Doc)> = s
            .nodes
            .iter()
            .filter(|(n, x)| x.parent().is_some_and(|p| parents.contains(&p)) && plain(**n))
            .filter_map(|(n, x)| {
                let w = doc_words(&x.title.0, &x.body.0);
                (!w.is_empty()).then(|| (x.parent().unwrap_or(*n), *n, w))
            })
            .collect();
        let rare = |w: &String| {
            u32::try_from(read.iter().filter(|(_, _, d)| d.all.contains(w)).count())
                .is_ok_and(|n| n <= RARE_DF)
        };
        let linked = |a: NodeId, b: NodeId| {
            s.nodes
                .get(&a)
                .is_some_and(|x| x.links.iter().any(|(_, t)| *t == b))
        };
        let mut pairs: Vec<(u32, NodeId, NodeId)> = Vec::new();
        for (i, (pa, a, wa)) in read.iter().enumerate() {
            for (pb, b, wb) in read.iter().skip(i.saturating_add(1)) {
                if pa != pb {
                    continue;
                }
                let (a, b) = ((*a).min(*b), (*a).max(*b));
                let shared =
                    u32::try_from(wa.all.intersection(&wb.all).filter(|w| rare(w)).count())
                        .unwrap_or(u32::MAX);
                let titled = |x: &crate::words::Doc, y: &crate::words::Doc| {
                    x.title.iter().any(|w| rare(w) && y.all.contains(w))
                };
                let signal = s.signals.contains_key(&crate::state::SignalKey {
                    kind: SignalKind::Duplicate,
                    a,
                    b,
                });
                // 1.10: two nodes of one creating session are its plan.
                let creator = |n: NodeId| match s.nodes.get(&n).map(|x| &x.origin) {
                    Some(crate::state::Origin::Agent(x) | crate::state::Origin::Ancestor(x)) => {
                        Some(*x)
                    }
                    _ => None,
                };
                let one_author = creator(a).is_some() && creator(a) == creator(b);
                if shared < ALIKE_MIN
                    || !titled(wa, wb)
                    || !titled(wb, wa)
                    || linked(a, b)
                    || linked(b, a)
                    || signal
                    || one_author
                {
                    continue;
                }
                // §16: the newer into the older, unless only the newer is held.
                let (from, to) = if live_holds(s, b, now) > 0 {
                    (a, b)
                } else {
                    (b, a)
                };
                pairs.push((shared, from, to));
            }
        }
        pairs.sort_by(|(sa, fa, ta), (sb, fb, tb)| {
            sb.cmp(sa)
                .then(crate::fnv::fnv_id(fa.0).cmp(&crate::fnv::fnv_id(fb.0)))
                .then(fa.cmp(fb))
                .then(ta.cmp(tb))
        });
        let total = stale.len().saturating_add(pairs.len());
        let cut = total > TIDY_MAX;
        let mut out = Vec::new();
        let (mut si, mut pi) = (stale.iter(), pairs.iter());
        while out.len() < TIDY_MAX {
            let mut any = false;
            if let Some(&n) = si.next() {
                any = true;
                out.extend(self.stale_item(n, cut, true));
            }
            if out.len() < TIDY_MAX
                && let Some(&(_, from, to)) = pi.next()
            {
                any = true;
                out.push(it(
                    Rule::Tidy,
                    Moment::Now,
                    true,
                    Skeleton::LinkDuplicate { from, to },
                    Because::Alike { from, to, cut },
                ));
            }
            if !any {
                break;
            }
        }
        out
    }
}
