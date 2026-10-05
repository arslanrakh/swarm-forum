//! The `hold` actions (design D.8.10, D.8.5). Owner: WP-D3.
//!
//! Each validation follows D.8.24's order: the target node (for `checkpoint`, `renew`, and
//! `release`, the token's node), the token (`stale_token`, H4), the shape, the other references,
//! state conflicts, and resources. `take` shares its checks with `can_take` (D.8.26).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use crate::apply::{check_token, target, token_live};
use crate::cmd::{Cmd, HoldCmd, WaitArg};
use crate::error::{Error, Field, Hint, LimitKind, What, Why};
use crate::event::{
    ConfirmVia, EntryBody, EntryText, Ev, Extension, HoldEnd, IntentEnd, NextOutcome, SystemNote,
    TextField,
};
use crate::helpers::Exec;
use crate::helpers::holds::{
    TakeBlock, bind_holder_notes, extend, issue_hold, live_holds_of, renew_allowed, take_block,
    under_limit,
};
use crate::helpers::recurring::maybe_missed;
use crate::helpers::restructure::reopen;
use crate::helpers::tree::reaches;
use crate::helpers::user::e1_check;
use crate::query::Ranked;
use crate::query::next::{Caller, rank};
use crate::reply::{EmptyWhy, NextReply, Reply};
use crate::state::{Cand, Intent, Life, PqState, Role, SaidState, State, WaitOn};
use crate::types::{
    Dur, Eid, Mark, NodeId, Outcome, QuestionId, ReleaseReason, SaidId, SaidKind, SessionId, Text,
    Token,
};

use super::Vcx;
use super::view::show;

/// The calling session, which every `hold` action has.
fn session(cx: &Vcx<'_>) -> Result<SessionId, Error> {
    cx.sess.ok_or(Error::invalid(Why::BadParams))
}

/// The target a `hold` action names: it resolved (`target` returned `Some`).
fn node_of(cx: &Vcx<'_>, cmd: &Cmd) -> Result<NodeId, Error> {
    target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))
}

/// The text "taken", the reason a take or a `next` records when it reopens a node (D.8.20).
fn taken() -> EntryText {
    EntryText {
        main: Text(String::from("taken")),
        extra: Vec::new(),
    }
}

/// The outcome of `n` while it is closed, None while it is open: the `{was}` of a take that
/// reopens it (1.4, §10.1, R4).
fn closed_outcome(s: &crate::state::State, n: NodeId) -> Option<Outcome> {
    match &s.nodes.get(&n)?.life {
        Life::Closed(c) => Some(c.outcome.clone()),
        Life::Open => None,
    }
}

// ------------------------------------------------------------------------------------------------
// take

/// The plan of `hold take` (D.8.10).
#[derive(Debug)]
pub(crate) struct TakePlan {
    n: NodeId,
    /// The session's live hold on `n`, which `take` returns (§10.1).
    existing: Option<Token>,
    /// The node is closed with this outcome: the take reopens it (`{was}`, 1.4, R4).
    closed: Option<Outcome>,
    /// The node waits: the take holds it all the same, and warns `waiting` (1.6, §10.1).
    waits: Option<crate::reply::WarningDetail>,
    /// 1.9 (§10.1): the node's needs that the taker's caps lack: the take holds it all the same,
    /// and warns `unmet_needs`.
    lacked: Vec<crate::types::Label>,
    lease: Option<Dur>,
}

impl TakePlan {
    /// Whether the take issues a hold.
    pub(crate) fn issues_hold(&self) -> bool {
        self.existing.is_none()
    }
}

/// The error a `TakeBlock` is, for node `n` and session `sess`.
fn take_error(cx: &Vcx<'_>, sess: SessionId, n: NodeId, b: TakeBlock) -> Error {
    match b {
        TakeBlock::Missing => Error::not_found(What::Node),
        TakeBlock::Existing(_) => Error::invalid(Why::BadParams),
        TakeBlock::Closed(merged_into) => Error::of(Hint::Closed {
            node: n,
            merged_into,
        }),
        TakeBlock::OccurrenceReopen => Error::invalid(Why::OccurrenceReopen),
        TakeBlock::NotYetAvailable => Error::invalid(Why::OccurrenceNotYetAvailable),
        TakeBlock::Paused(scope, until) => Error::of(Hint::Paused { scope, until }),
        TakeBlock::HoldsLimit => Error::of(Hint::HoldsLimit {
            holds: live_holds_of(cx.s, sess, cx.now),
        }),
        TakeBlock::Full(capacity, live) => Error::of(Hint::Full {
            node: n,
            capacity,
            live,
        }),
    }
}

/// The `waiting` warning's detail for a take of `n` (1.6, §10.1): the node's unresolved wait, and
/// when it was set; 1.10: the release that set it, the node's last checkpoint, whose author the
/// shell reads. None for a node that does not wait.
fn waiting_detail(cx: &Vcx<'_>, n: NodeId) -> Option<crate::reply::WarningDetail> {
    let node = cx.s.nodes.get(&n)?;
    if !crate::flags::waiting(cx.s, node, cx.now) {
        return None;
    }
    let w = node.wait.as_ref()?;
    Some(crate::reply::WarningDetail::Waiting {
        on: w.on,
        set_at: w.set_at,
        release: node.last_checkpoint,
    })
}

/// Validates `hold take {target, lease?}` (D.8.10): the target resolves → the session's existing
/// hold is returned → `closed` → `invalid(OccurrenceReopen)` → `invalid(OccurrenceNotYetAvailable)`
/// → `paused` → `holds_limit` → `full`. A node that waits is taken all the same (§10.3), with the
/// `waiting` warning (1.6), and so is one whose needs the session's caps lack, with the
/// `unmet_needs` warning (1.9).
pub(crate) fn take(cx: &Vcx<'_>, cmd: &Cmd) -> Result<TakePlan, Error> {
    let Cmd::Hold(HoldCmd::Take { lease, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = node_of(cx, cmd)?;
    let sess = session(cx)?;
    let closed = closed_outcome(cx.s, n);
    match take_block(cx.s, sess, n, cx.now) {
        None => Ok(TakePlan {
            n,
            existing: None,
            closed,
            waits: waiting_detail(cx, n),
            lacked: crate::helpers::holds::needs_lacked(cx.s, sess, n),
            lease: *lease,
        }),
        Some(TakeBlock::Existing(t)) => Ok(TakePlan {
            n,
            existing: Some(t),
            closed: None,
            waits: None,
            lacked: Vec::new(),
            lease: *lease,
        }),
        Some(b) => Err(take_error(cx, sess, n, b)),
    }
}

/// Executes `hold take`: a closed node is reopened first (D.8.20, reason "taken"); then the hold,
/// which keeps the outcome the node had (`{was}`, 1.4, R4), the pending `holder:` notes, and
/// `Shown`.
pub(crate) fn take_exec(ex: &mut Exec<'_>, plan: TakePlan) -> Reply {
    let n = plan.n;
    if let Some(t) = plan.existing {
        show(ex, n);
        let lease_until = ex.s().holds.get(&t).map_or(ex.now(), |h| h.lease_until);
        return Reply::Taken {
            node: n,
            token: t,
            lease_until,
            reopened: None,
            existing: true,
        };
    }
    if plan.closed.is_some() {
        let dropped = reopen(ex, n, taken());
        duplicate_key_warning(ex, n, dropped);
    }
    if let Some(detail) = plan.waits {
        ex.warn(crate::reply::Warning {
            code: crate::reply::WarningCode::Waiting,
            id: Some(n),
            detail,
        });
    }
    if !plan.lacked.is_empty() {
        ex.warn(crate::reply::Warning {
            code: crate::reply::WarningCode::UnmetNeeds,
            id: Some(n),
            detail: crate::reply::WarningDetail::UnmetNeeds {
                needs: plan.lacked,
                take: true,
            },
        });
    }
    let (token, lease_until) = issue_hold(ex, n, plan.lease, plan.closed.clone());
    bind_holder_notes(ex, n);
    show(ex, n);
    Reply::Taken {
        node: n,
        token,
        lease_until,
        reopened: plan.closed,
        existing: false,
    }
}

/// The `duplicate_key` warning of a reopen that dropped keys another open node carries (K7).
fn duplicate_key_warning(
    ex: &mut Exec<'_>,
    n: NodeId,
    dropped: Vec<(crate::types::KeyStr, NodeId)>,
) {
    if dropped.is_empty() {
        return;
    }
    ex.warn(crate::reply::Warning {
        code: crate::reply::WarningCode::DuplicateKey,
        id: Some(n),
        detail: crate::reply::WarningDetail::DuplicateKey { keys: dropped },
    });
}

// ------------------------------------------------------------------------------------------------
// next

/// The plan of `hold next` (D.8.5): the candidate the index ranked first, if any, and why.
#[derive(Debug)]
pub(crate) struct NextPlan {
    pick: Option<Ranked>,
    why: EmptyWhy,
    /// The query is empty: the session asked for any work it can do (CS-7).
    open: bool,
    lease: Option<Dur>,
}

impl NextPlan {
    /// Whether `next` issues a hold: its candidate is a node to take.
    pub(crate) fn issues_hold(&self) -> bool {
        matches!(self.pick.map(|r| r.cand), Some(Cand::Node(_)))
    }
}

/// Validates `hold next {query?, lease?}` (D.8.5): the query's `under` resolves → `holds_limit`
/// (literal, although routing and status items need no hold, A-50). Then the best candidate.
pub(crate) fn next(cx: &Vcx<'_>, cmd: &Cmd) -> Result<NextPlan, Error> {
    next_with(cx, cmd, |caller, q| {
        rank(cx.s, caller, cx.now, q, 1, &mut crate::query::NoMeter)
    })
}

/// `next` with the tie-break by id (`TieBreak::Id`, D.6.1): only with `conformance`.
#[cfg(feature = "conformance")]
pub(crate) fn next_by_id(cx: &Vcx<'_>, cmd: &Cmd) -> Result<NextPlan, Error> {
    next_with(cx, cmd, |caller, q| {
        crate::query::next::rank_by_id(cx.s, caller, cx.now, q, 1)
    })
}

fn next_with<F>(cx: &Vcx<'_>, cmd: &Cmd, ranker: F) -> Result<NextPlan, Error>
where
    F: Fn(&Caller<'_>, &crate::cmd::NextQuery) -> (Vec<Ranked>, EmptyWhy),
{
    let Cmd::Hold(HoldCmd::Next { query, lease }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    target(cx.s, cmd)?;
    let sess = session(cx)?;
    let held = live_holds_of(cx.s, sess, cx.now);
    if !under_limit(
        u32::try_from(held.len()).unwrap_or(u32::MAX),
        cx.s.params.hold_per_session,
    ) {
        return Err(Error::of(Hint::HoldsLimit { holds: held }));
    }
    let caller = Caller::of(cx.s, sess).ok_or(Error::not_found(What::Session))?;
    let (top, why) = ranker(&caller, query);
    let open = query.under.is_none() && query.kind.is_none() && query.tags.is_empty();
    Ok(NextPlan {
        pick: top.first().copied(),
        why,
        open,
        lease: *lease,
    })
}

/// Executes `hold next` (D.8.5): `NextCalled`, then the candidate: nothing, a routing item, a
/// status item, a review item (1.9), or a node, reopened first if closed, then held.
pub(crate) fn next_exec(ex: &mut Exec<'_>, plan: NextPlan) -> Reply {
    let Some(sess) = ex.sess() else {
        return Reply::Next(NextReply::Empty {
            why: plan.why,
            leave_better: Vec::new(),
        });
    };
    let outcome = if plan.pick.is_some() {
        NextOutcome::Item
    } else if plan.why.candidates == 0 {
        NextOutcome::Nothing
    } else {
        NextOutcome::NothingForYou
    };
    let class_window_end = ex.now().plus(ex.s().params.refusal_window);
    ex.emit(Ev::NextCalled {
        session: sess,
        outcome,
        open: plan.open,
        class_window_end,
    });
    let Some(pick) = plan.pick else {
        let leave_better = crate::query::leave_better(ex.s(), None, Some(sess));
        return Reply::Next(NextReply::Empty {
            why: plan.why,
            leave_better,
        });
    };
    match pick.cand {
        Cand::Route(entry) => Reply::Next(NextReply::Route {
            entry,
            why: pick.why,
        }),
        Cand::Status(n) => {
            show(ex, n);
            // 1.9 (§10.4): on a closed node, the item with no hold is the review of its first open
            // objection; the node is not reopened.
            match crate::query::review_objection(ex.s(), n) {
                Some(objection) => Reply::Next(NextReply::Review {
                    node: n,
                    objection,
                    why: pick.why,
                }),
                None => Reply::Next(NextReply::Status {
                    node: n,
                    why: pick.why,
                }),
            }
        }
        Cand::Node(n) => {
            let closed = closed_outcome(ex.s(), n);
            if closed.is_some() {
                let dropped = reopen(ex, n, taken());
                duplicate_key_warning(ex, n, dropped);
            }
            let (token, lease_until) = issue_hold(ex, n, plan.lease, closed.clone());
            bind_holder_notes(ex, n);
            show(ex, n);
            Reply::Next(NextReply::Hold {
                node: n,
                token,
                lease_until,
                why: pick.why,
                reopened: closed,
            })
        }
    }
}

// ------------------------------------------------------------------------------------------------
// checkpoint

/// The plan of `hold checkpoint` (D.8.10).
#[derive(Debug)]
pub(crate) struct CheckpointPlan {
    token: Token,
    text: EntryText,
    intent: bool,
    intent_done: bool,
    /// 1.10 (§10.2): the node waits, and this checkpoint, of a live hold, ends the wait.
    ends_wait: bool,
    /// 1.10 (§10.2, §19): the intent's node is under an active pause, whose scope and end the
    /// `paused` warning names.
    paused: Option<(NodeId, crate::types::Time)>,
}

/// Validates `hold checkpoint {token, done, next_step, intent?, intent_done?, files?}` (D.8.10):
/// the token's node exists → `stale_token` → `invalid(EmptyText(NextStep))` →
/// `invalid(IntentOpen)` for a new intent while one is open and `intent_done` not given →
/// `invalid(NoIntent)` for `intent_done` with none open (A-67). 1.10: whether the node waits, and,
/// for an `intent`, the pause over the node, which refuse nothing.
pub(crate) fn checkpoint(cx: &Vcx<'_>, cmd: &Cmd) -> Result<CheckpointPlan, Error> {
    let Cmd::Hold(HoldCmd::Checkpoint {
        token,
        done,
        next_step,
        intent,
        intent_done,
        files,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = node_of(cx, cmd)?;
    check_token(cx, *token, n)?;
    if next_step.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::NextStep)));
    }
    let open = cx.s.nodes.get(&n).and_then(|x| x.open_intent).is_some();
    if intent.is_some() && open && intent_done.is_none() {
        return Err(Error::invalid(Why::IntentOpen));
    }
    if intent_done.is_some() && !open {
        return Err(Error::invalid(Why::NoIntent));
    }
    let mut extra = alloc::vec![(TextField::NextStep, next_step.clone())];
    if let Some(i) = intent {
        extra.push((TextField::Intent, i.clone()));
    }
    if let Some(d) = intent_done {
        extra.push((TextField::IntentDone, d.clone()));
    }
    extra.extend(files.iter().map(|f| (TextField::File, f.clone())));
    let ends_wait = cx.s.nodes.get(&n).is_some_and(|x| x.wait.is_some());
    let paused = if intent.is_some() {
        crate::helpers::holds::paused_by(cx.s, n, cx.now)
    } else {
        None
    };
    Ok(CheckpointPlan {
        token: *token,
        text: EntryText {
            main: done.clone(),
            extra,
        },
        intent: intent.is_some(),
        intent_done: intent_done.is_some(),
        ends_wait,
        paused,
    })
}

/// 1.12 (§5, §20.9 rule 8): whether the field `f` of a checkpoint's text, if it has one, names a
/// pull request (`words::names_pull_request`, 1.10's detector): the entry carries it, so that a log
/// written before 1.12, whose entries carry none, replays to the state it wrote (D.7.6).
fn names_pr(text: &EntryText, f: TextField) -> Option<bool> {
    text.extra
        .iter()
        .find(|(g, _)| *g == f)
        .map(|(_, t)| crate::words::names_pull_request(&t.0))
}

/// Executes `hold checkpoint`: the checkpoint entry, the intent closed and opened, the node's wait
/// ended (1.10, §10.2: a holder at work is not waiting; `WaitSet { None }`), and the lease
/// extended (D.8.10). The reply names its own entry, the intent it opened, and the intent it
/// closed, whichever session opened that one (1.4, §20.6, R9). 1.10 (§10.2, §19): an intent on a
/// node under a pause warns `paused`. In the lab's fifth round (l1), a worker that took its own
/// waiting node back worked on it while the wait stood, and the node would have been offered to
/// nobody for 3.5 hours once its lease ended.
pub(crate) fn checkpoint_exec(ex: &mut Exec<'_>, plan: CheckpointPlan) -> Reply {
    let n = plan.token.node;
    let by = ex.author();
    let closed_intent = if plan.intent_done {
        ex.s()
            .nodes
            .get(&n)
            .and_then(|x| x.open_intent)
            .map(|i| i.eid)
    } else {
        None
    };
    // 1.12 (§5, §20.9 rule 8): whether the `intent_done` and the intent name a pull request.
    let pr_named = names_pr(&plan.text, TextField::IntentDone);
    let intent_pr = names_pr(&plan.text, TextField::Intent).unwrap_or(false);
    let eid = ex.append(
        n,
        by,
        EntryBody::Checkpoint {
            token: plan.token,
            release: None,
            pr_named,
            handed_off_until: None,
        },
        plan.text,
    );
    if plan.intent_done {
        ex.emit(Ev::IntentClosed {
            node: n,
            end: IntentEnd::Done { by: eid },
        });
    }
    if plan.intent
        && let Some(session) = ex.sess()
    {
        ex.emit(Ev::IntentOpened {
            node: n,
            intent: Intent {
                eid,
                session,
                pr_named: intent_pr,
            },
        });
    }
    if plan.ends_wait {
        ex.emit(Ev::WaitSet {
            node: n,
            wait: None,
        });
    }
    if let Some((scope, until)) = plan.paused {
        ex.warn(crate::reply::Warning {
            code: crate::reply::WarningCode::Paused,
            id: Some(n),
            detail: crate::reply::WarningDetail::Paused { scope, until },
        });
    }
    let lease_until = extend(ex, plan.token, Extension::Checkpoint);
    Reply::Checkpointed {
        entry: eid,
        lease_until,
        intent: plan.intent.then_some(eid),
        intent_done: closed_intent,
    }
}

// ------------------------------------------------------------------------------------------------
// renew

/// The plan of `hold renew` (D.8.10).
#[derive(Debug)]
pub(crate) struct RenewPlan {
    token: Token,
    why: Text,
}

/// Validates `hold renew {token, why}` (D.8.10): the token's node exists → `stale_token` →
/// `limit(Renew)` when `renews + 1 > renew_max` (H5).
pub(crate) fn renew(cx: &Vcx<'_>, cmd: &Cmd) -> Result<RenewPlan, Error> {
    let Cmd::Hold(HoldCmd::Renew { token, why }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = node_of(cx, cmd)?;
    check_token(cx, *token, n)?;
    let over =
        cx.s.holds
            .get(token)
            .is_none_or(|h| !renew_allowed(h.renews, h.terms.renew_max));
    if over {
        return Err(Error::limit(LimitKind::Renew));
    }
    Ok(RenewPlan {
        token: *token,
        why: why.clone(),
    })
}

/// Executes `hold renew`: the renewal's system entry, then the extension (D.8.10).
pub(crate) fn renew_exec(ex: &mut Exec<'_>, plan: RenewPlan) -> Reply {
    let n = plan.token.node;
    let by = ex.author();
    ex.append(
        n,
        by,
        EntryBody::System(SystemNote::Renewed { token: plan.token }),
        EntryText {
            main: plan.why,
            extra: Vec::new(),
        },
    );
    let lease_until = extend(ex, plan.token, Extension::Renew);
    let renews_left = ex
        .s()
        .holds
        .get(&plan.token)
        .map_or(0, |h| h.terms.renew_max.saturating_sub(h.renews));
    Reply::Renewed {
        lease_until,
        renews_left,
    }
}

// ------------------------------------------------------------------------------------------------
// release

/// The plan of `hold release` (D.8.10).
#[derive(Debug)]
pub(crate) struct ReleasePlan {
    token: Token,
    text: EntryText,
    /// The reason recorded: `blocked` whenever there is a wait (A-98).
    reason: ReleaseReason,
    /// The node's wait after the release (§10.3: a release replaces the wait).
    wait: Option<WaitOn>,
    /// A people question waited on, whose recheck the release schedules (A-24).
    recheck: Option<QuestionId>,
    /// 1.4 (§10.3, R15): the release closes the node's open intent.
    intent_done: bool,
    unsent: BTreeSet<SaidId>,
    /// 1.12 (§10.3): the decision `settles` records after the release, as `talk decide` would.
    decision: Option<super::review::DecidePlan>,
}

/// Validates `hold release {token, done, next_step, reason, waits_for?, intent_done?, settles?,
/// learnings?, unsent?}` (D.8.10): the token's node exists → `stale_token` →
/// `invalid(EmptyText(NextStep))` → (1.12, §10.3) each request of `settles` exists
/// (`not_found(Request)`, as for a close) → the wait target (`not_found(Question)` for a question
/// that is not a people or user question, A-31; `not_found(Node)`) → `invalid(NoIntent)` for
/// `intent_done` with no open intent on the node (1.4, §10.3, R15; a state check, so after the
/// references, D.8.24) → `cycle` for a `{node}` wait on the node itself or one that reaches it (T5).
pub(crate) fn release(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ReleasePlan, Error> {
    let Cmd::Hold(HoldCmd::Release {
        token,
        done,
        next_step,
        reason,
        waits_for,
        intent_done,
        learnings,
        unsent,
        settles,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = node_of(cx, cmd)?;
    check_token(cx, *token, n)?;
    if next_step.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::NextStep)));
    }
    if settles.iter().any(|r| !cx.s.requests.contains_key(r)) {
        return Err(Error::not_found(What::Request));
    }
    let mut extra = alloc::vec![(TextField::NextStep, next_step.clone())];
    if let Some(d) = intent_done {
        extra.push((TextField::IntentDone, d.clone()));
    }
    if let Some(l) = learnings {
        extra.push((TextField::Learnings, l.clone()));
    }
    let mut recheck = None;
    let wait = match waits_for {
        None => None,
        Some(WaitArg::Question(q)) => {
            if let Some(pq) = cx.s.pquestions.get(q) {
                if pq.state == PqState::Open {
                    recheck = Some(*q);
                }
                Some(WaitOn::Question {
                    q: *q,
                    rechecks_at_set: pq.rechecks,
                })
            } else if cx.s.uquestions.contains_key(q) {
                Some(WaitOn::UserQuestion(*q))
            } else {
                return Err(Error::not_found(What::Question));
            }
        }
        Some(WaitArg::Node(r)) => {
            let t = crate::apply::node_of(cx.s, r)?;
            Some(WaitOn::Node(t))
        }
        Some(WaitArg::Time(w)) => Some(WaitOn::Time(w.resolve(cx.now))),
        Some(WaitArg::External {
            what,
            recheck_after,
        }) => {
            extra.push((TextField::What, what.clone()));
            Some(WaitOn::External(recheck_after.resolve(cx.now)))
        }
    };
    if intent_done.is_some() && cx.s.nodes.get(&n).and_then(|x| x.open_intent).is_none() {
        return Err(Error::invalid(Why::NoIntent));
    }
    if let Some(WaitOn::Node(t)) = wait {
        if t == n {
            return Err(Error::of(Hint::Cycle {
                path: alloc::vec![n, n],
            }));
        }
        if let Some(path) = reaches(cx.s, t, n, &[], &[]) {
            let mut full = alloc::vec![n];
            full.extend(path);
            return Err(Error::of(Hint::Cycle { path: full }));
        }
    }
    let reason = if waits_for.is_some() {
        ReleaseReason::Blocked
    } else {
        *reason
    };
    Ok(ReleasePlan {
        token: *token,
        text: EntryText {
            main: done.clone(),
            extra,
        },
        reason,
        wait,
        recheck,
        intent_done: intent_done.is_some(),
        unsent: unsent.clone(),
        decision: (!settles.is_empty())
            .then(|| super::review::DecidePlan::of_release(cx.s, n, settles.clone(), done)),
    })
}

/// Executes `hold release` (D.8.10): the checkpoint entry and, with `intent_done`, the intent it
/// closes (1.4, R15), the wait, the recheck, the recheck marks off, the hold's end (with the missed
/// check of an occurrence), a `cannot`'s refusal and E1, and the session's pending `said` entries
/// confirmed but those it did not send, which become unconfirmed (1.5); then, with `settles` (1.12,
/// §10.3), the decision `talk decide {at, text: done, settles}` records (`record_decision`).
/// Warning `open_intent` when the node's open intent is still the session's own after these events
/// (1.4, §10.3): the release is not refused, but the next holder must check whether the action
/// happened.
pub(crate) fn release_exec(ex: &mut Exec<'_>, plan: ReleasePlan) -> Reply {
    let n = plan.token.node;
    let now = ex.now();
    let by = ex.author();
    // 1.
    let closed_intent = if plan.intent_done {
        ex.s()
            .nodes
            .get(&n)
            .and_then(|x| x.open_intent)
            .map(|i| i.eid)
    } else {
        None
    };
    // 1.12: whether the `intent_done` names a pull request (§5), and, for a `handoff` release,
    // until when it keeps the node from its releaser, `lease.first` of the hold's terms (§10.4).
    let pr_named = names_pr(&plan.text, TextField::IntentDone);
    let handed_off_until = if plan.reason == ReleaseReason::Handoff {
        ex.s()
            .holds
            .get(&plan.token)
            .map(|h| now.plus(h.terms.first))
    } else {
        None
    };
    let eid = ex.append(
        n,
        by,
        EntryBody::Checkpoint {
            token: plan.token,
            release: Some(plan.reason),
            pr_named,
            handed_off_until,
        },
        plan.text,
    );
    if plan.intent_done {
        ex.emit(Ev::IntentClosed {
            node: n,
            end: IntentEnd::Done { by: eid },
        });
    }
    // 2.
    let current = ex
        .s()
        .nodes
        .get(&n)
        .and_then(|x| x.wait.as_ref().map(|w| w.on));
    if current != plan.wait {
        ex.emit(Ev::WaitSet {
            node: n,
            wait: plan.wait,
        });
    }
    // 3.
    if let Some(q) = plan.recheck {
        let at = now.plus(ex.s().params.question_recheck);
        ex.emit(Ev::PqRecheckAt { q, at });
    }
    // 4.
    let marks: Vec<QuestionId> = ex
        .s()
        .nodes
        .get(&n)
        .map(|x| x.recheck_marks.iter().copied().collect())
        .unwrap_or_default();
    for q in marks {
        ex.emit(Ev::MarkSet {
            node: n,
            mark: Mark::Recheck,
            question: q,
            on: false,
        });
    }
    // 5.
    ex.emit(Ev::HoldEnded {
        token: plan.token,
        end: HoldEnd::Released,
    });
    if ex
        .s()
        .nodes
        .get(&n)
        .is_some_and(|x| matches!(x.role, Role::Occurrence(_)))
    {
        maybe_missed(ex, n);
    }
    // 6.
    if plan.reason == ReleaseReason::Cannot
        && plan.wait.is_none()
        && let Some(sess) = ex.sess()
        && let Some(class) = ex.s().sessions.get(&sess).map(|x| x.class.clone())
    {
        let p = &ex.s().params;
        let (backoff_until, expires_at, quorum) = (
            now.plus(p.refusal_backoff),
            now.plus(p.refusal_ttl),
            p.refusal_quorum,
        );
        ex.emit(Ev::Refused {
            node: n,
            class,
            session: sess,
            backoff_until,
            expires_at,
            quorum,
        });
        e1_check(ex, n);
    }
    // 7.
    confirm_pending(ex, &plan.unsent, ConfirmVia::Release);
    // 8. 1.12 (§10.3): the decision of `settles`, after the release, as a `decide` would record it.
    // In the lab's seventh round (it6 l7, l1, l4), finished work that waited for its merge took a
    // `decide` beside the release, and workers recorded a pull request's status as a decision.
    let (decision, settled, not_settled) = match &plan.decision {
        Some(d) => {
            let (d, settled, not_settled) = super::review::record_decision(ex, d);
            (Some(d), settled, not_settled)
        }
        None => (None, Vec::new(), Vec::new()),
    };
    if let Some(sess) = ex.sess()
        && let Some(intent) = ex.s().nodes.get(&n).and_then(|x| x.open_intent)
        && intent.session == sess
    {
        ex.warn(crate::reply::Warning {
            code: crate::reply::WarningCode::OpenIntent,
            id: Some(n),
            detail: crate::reply::WarningDetail::OpenIntent { intent: intent.eid },
        });
    }
    Reply::Released {
        intent_done: closed_intent,
        leave_better: crate::query::leave_better(ex.s(), Some(n), ex.sess()),
        decision,
        settled,
        not_settled,
    }
}

/// For each pending `said` of the session, ascending (§9.2): `SaidConfirmed { x, via, link: None }`
/// when it is not in `unsent`, since a release or a close confirms what the session recorded, and
/// `SaidUnconfirmed { x }` when it is (1.5): an entry the session says it did not send is not
/// confirmed as sent, so it leaves rule 2, which reads pending entries, and its node carries
/// `unconfirmed_said` once unattended, for whoever checks the conversation. 1.9 (§9.2): an entry
/// in `unsent` that a later `said` of its conversation replaced (`replaced`) is `SaidWithheld`
/// instead: nobody is to send it, so nobody is sent to check. In the lab's fourth round (l6), a
/// draft replaced by its correction and listed in `unsent` reopened the closed support node for a
/// worker that could not read the chat.
pub(crate) fn confirm_pending(ex: &mut Exec<'_>, unsent: &BTreeSet<SaidId>, via: ConfirmVia) {
    let Some(sess) = ex.sess() else { return };
    let s = ex.s();
    let pending: Vec<SaidId> =
        s.ix.open_saids_by
            .get(&sess)
            .into_iter()
            .flatten()
            .copied()
            .filter(|x| {
                s.saids
                    .get(x)
                    .is_some_and(|y| y.state == SaidState::Pending)
            })
            .collect();
    for said in pending {
        if unsent.contains(&said) {
            if replaced(ex.s(), said) {
                ex.emit(Ev::SaidWithheld { said });
            } else {
                ex.emit(Ev::SaidUnconfirmed { said });
            }
        } else {
            ex.emit(Ev::SaidConfirmed {
                said,
                via,
                link: None,
            });
        }
    }
}

/// Whether a later `said` replaced `x` (1.9, §9.2): one of kind `reply`, `result`, or
/// `correction`, recorded after it in its conversation, on its conversation node, and not itself
/// withheld (a correction of it is one). Read from the node's `said` entries after `x`.
pub(crate) fn replaced(s: &State, x: SaidId) -> bool {
    let Some(sd) = s.saids.get(&x) else {
        return false;
    };
    s.ix.items.get(&sd.node).is_some_and(|it| {
        it.saids.range(Eid(x.0.saturating_add(1))..).any(|y| {
            s.saids.get(y).is_some_and(|z| {
                z.conv == sd.conv
                    && matches!(
                        z.kind,
                        SaidKind::Reply | SaidKind::Result | SaidKind::Correction
                    )
                    && z.state != SaidState::Withheld
            })
        })
    })
}

// ------------------------------------------------------------------------------------------------
// check

/// The plan of `hold check` (D.8.10).
#[derive(Debug)]
pub(crate) struct CheckPlan {
    n: NodeId,
    token: Option<Token>,
    /// 1.8 (§10.2): what became of the node's open intent, and that intent.
    intent_done: Option<(Text, Eid)>,
}

/// Validates `hold check {target, token?, intent_done?}`: the target resolves; with `intent_done`
/// (1.8, §10.2), the node has an open intent (`invalid(NoIntent)`), whoever opened it, whether or
/// not anyone holds the node, open or closed. A token that is not current is not an error here
/// (A-70).
pub(crate) fn check(cx: &Vcx<'_>, cmd: &Cmd) -> Result<CheckPlan, Error> {
    let Cmd::Hold(HoldCmd::Check {
        token, intent_done, ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = node_of(cx, cmd)?;
    let intent_done = match intent_done {
        None => None,
        Some(text) => {
            let open = cx.s.nodes.get(&n).and_then(|x| x.open_intent);
            let Some(i) = open else {
                return Err(Error::invalid(Why::NoIntent));
            };
            Some((text.clone(), i.eid))
        }
    };
    Ok(CheckPlan {
        n,
        token: *token,
        intent_done,
    })
}

/// Executes `hold check`: whether the given token is a live hold on the node. Without
/// `intent_done` it reads only; with it (1.8, §10.2), the `checkpoint` entry by the calling session
/// that records it, and `IntentClosed` with that entry, as a checkpoint's `intent_done` closes it.
pub(crate) fn check_exec(ex: &mut Exec<'_>, plan: CheckPlan) -> Reply {
    let now = ex.now();
    let token_current = plan.token.map(|t| token_live(ex.s(), t, plan.n, now));
    let (entry, intent_done) = match plan.intent_done {
        None => (None, None),
        Some((text, intent)) => {
            let by = ex.author();
            // 1.12 (§5, §20.9 rule 8): whether the `intent_done` names a pull request.
            let pr_named = crate::words::names_pull_request(&text.0);
            let eid = ex.append(
                plan.n,
                by,
                EntryBody::IntentChecked { intent, pr_named },
                EntryText {
                    main: text,
                    extra: Vec::new(),
                },
            );
            ex.emit(Ev::IntentClosed {
                node: plan.n,
                end: IntentEnd::Done { by: eid },
            });
            (Some(eid), Some(intent))
        }
    };
    Reply::HoldCheck {
        node: plan.n,
        token_current,
        entry,
        intent_done,
    }
}
