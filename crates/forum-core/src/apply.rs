//! `apply` (design D.8.1, D.6.1): one command, end to end, after the shell's settle.
//!
//! 1. **Clock and text size.** `now := max(ctx.now, s.last_now)` (T1); a command whose longest
//!    `Text` is over `text.max` returns `limit(TextTooLong)` and appends nothing, before O1 and
//!    before a session opens (L1, A-114).
//! 2. **Retry (O1).** A live record of `(class, op_id)` answers: the same digest from no session or
//!    from the record's session is `Replay`, anything else `op_id_conflict`; neither appends.
//! 3. **Session.** An existing session must exist (`not_found(Session)`) and be of the call's
//!    class (`invalid(ClassMismatch)`); `System` runs `Admin` only, and `Admin` runs only under it.
//! 4. **Validate** (`dispatch::validate`, D.8.24's order), on the state after the new session's
//!    `SessionOpened`, which a first call emits whatever the validation result (A-38).
//! 5. **Execute.** `Touched` first when presence is due, the session is silent, or the command
//!    issues a hold (CS-14, CS-8); then the command's events. A `stale_token` appends the `late`
//!    entry on the target (H4); any other error appends nothing.
//! 6. **Record (O1).** `OpRecorded` last, with the outcome, when an `op_id` is given and an event
//!    was emitted (A-106).
//!
//! Every event is emitted through `Exec`, which applies it with `evolve` at once, so the state
//! afterwards is the fold of the events over the state before.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::cmd::{
    AdminCmd, Cmd, ControlCmd, Ctx, HideArg, HoldCmd, NodeCmd, RetractArg, SessionCmd, SessionCtx,
    TalkCmd, ViewCmd,
};
use crate::dispatch::{self, Plan, Vcx};
use crate::error::{Error, ErrorCode, Hint, LimitKind, What, Why};
use crate::event::{EntryBody, EntryText, Ev, Event};
use crate::facts::Tracked;
use crate::helpers::Exec;
use crate::helpers::principals::intern;
use crate::query;
use crate::reply::{Done, Reply, Warning, WarningCode, WarningDetail};
use crate::state::{Author, State};
use crate::types::{NodeId, SessionId, Time, Token};

/// Production entry point (D.6.1): validates against `s`; on success executes by emitting events
/// and applying each with `evolve_mut`, so afterwards `s == fold(evolve, s_before, events)`.
pub fn apply(s: &mut State, ctx: &Ctx, cmd: &Cmd) -> (Vec<Event>, Result<Done, Error>) {
    run(s, ctx, cmd, None, false)
}

/// `apply`, adding to `dirty` every fact its events change (D.6.1, FQ-2).
pub fn apply_tracked(
    s: &mut State,
    ctx: &Ctx,
    cmd: &Cmd,
    dirty: &mut Tracked,
) -> (Vec<Event>, Result<Done, Error>) {
    run(s, ctx, cmd, Some(dirty), false)
}

/// §24's pure form: `decide(s, ctx, cmd) == { let mut t = s.clone(); apply(&mut t, ctx, cmd) }`.
pub fn decide(s: &State, ctx: &Ctx, cmd: &Cmd) -> (Vec<Event>, Result<Done, Error>) {
    let mut t = s.clone();
    apply(&mut t, ctx, cmd)
}

/// Only with the test-only feature `conformance` (D.6.1, D.10.8): `apply` with the tie-break of
/// `next` chosen by the caller. `TieBreak::Id` ranks with the naive order and breaks the ring
/// component by id, as the model's slices do with `USE_FNV = false` (D.8.5, D.10.6).
#[cfg(feature = "conformance")]
pub fn apply_with(
    s: &mut State,
    ctx: &Ctx,
    cmd: &Cmd,
    tie: crate::reply::TieBreak,
) -> (Vec<Event>, Result<Done, Error>) {
    run(s, ctx, cmd, None, tie == crate::reply::TieBreak::Id)
}

/// The caller of a command, after step 3 of D.8.1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Caller {
    /// An existing session, and whether it was silent before the call (§3).
    Existing {
        /// The session.
        sess: SessionId,
        /// It was silent.
        stale: bool,
    },
    /// A session this call opens, with the id its `SessionOpened` carries.
    New(SessionId),
    /// The server itself, at startup (`Cmd::Admin` only).
    System,
}

/// Step 3 of D.8.1: the session, before the command. `query::precheck` shares it (D.8.25).
///
/// # Errors
///
/// `not_found(Session)` for a session that does not exist (a retired one included, CS-8),
/// `invalid(ClassMismatch)` for one of another class, and `invalid(BadParams)` for `Admin` outside
/// `SessionCtx::System` or another command under it (D.5.1).
pub(crate) fn session_check(s: &State, ctx: &Ctx, cmd: &Cmd) -> Result<Caller, Error> {
    let admin = matches!(cmd, Cmd::Admin(_));
    match ctx.session {
        SessionCtx::Existing(sess) => {
            let x = s
                .sessions
                .get(&sess)
                .ok_or(Error::not_found(What::Session))?;
            if x.class != ctx.class {
                return Err(Error::invalid(Why::ClassMismatch));
            }
            if admin {
                return Err(Error::invalid(Why::BadParams));
            }
            Ok(Caller::Existing {
                sess,
                stale: x.silent,
            })
        }
        SessionCtx::New => {
            if admin {
                return Err(Error::invalid(Why::BadParams));
            }
            Ok(Caller::New(s.counters.next_session))
        }
        SessionCtx::System => {
            if admin {
                Ok(Caller::System)
            } else {
                Err(Error::invalid(Why::BadParams))
            }
        }
    }
}

/// Resolves a node reference, or `not_found(Node)`.
pub(crate) fn node_of(s: &State, r: &crate::types::NodeRef) -> Result<NodeId, Error> {
    query::resolve(s, r).ok_or(Error::not_found(What::Node))
}

/// Step 2 of D.8.24: the command's target node, which must exist (`not_found(Node)`): the node a
/// command acts on (`at`, `on`, `target`, `scope`, the `under` of `next`'s query and of `list`,
/// the `for` of `wait`, the `to` of `route`, every node of `view node`), or, for `checkpoint`,
/// `renew`, and `release`, the token's node. `None` for a command without one. Every validation
/// of D.8.24 starts with it, and `query::precheck` reads it, so the two cannot disagree (D.8.25).
///
/// # Errors
///
/// `not_found(Node)`.
pub(crate) fn target(s: &State, cmd: &Cmd) -> Result<Option<NodeId>, Error> {
    let one = |r: &crate::types::NodeRef| node_of(s, r).map(Some);
    let opt = |r: &Option<crate::types::NodeRef>| match r {
        Some(r) => node_of(s, r).map(Some),
        None => Ok(None),
    };
    let token = |t: &Token| {
        if s.nodes.contains_key(&t.node) {
            Ok(Some(t.node))
        } else {
            Err(Error::not_found(What::Node))
        }
    };
    match cmd {
        Cmd::Session(SessionCmd::Hello { .. } | SessionCmd::Status { .. })
        | Cmd::View(
            ViewCmd::Find { .. }
            | ViewCmd::Requests { .. }
            | ViewCmd::UserLog { .. }
            | ViewCmd::Constitution,
        )
        | Cmd::Node(NodeCmd::Create(_))
        | Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry { .. },
            ..
        })
        | Cmd::Talk(
            TalkCmd::SaidConfirm { .. }
            | TalkCmd::Answer { .. }
            | TalkCmd::Resolve { .. }
            | TalkCmd::Retract {
                target: RetractArg::Entry { .. },
                ..
            },
        )
        | Cmd::Control(ControlCmd::Health | ControlCmd::Constitution | ControlCmd::Export { .. })
        | Cmd::Admin(AdminCmd::SetParams(_)) => Ok(None),
        Cmd::View(ViewCmd::Node { targets, .. }) => {
            let mut first = None;
            for r in targets {
                let n = node_of(s, r)?;
                first = first.or(Some(n));
            }
            Ok(first)
        }
        Cmd::View(ViewCmd::Feed { at, .. } | ViewCmd::History { at, .. })
        | Cmd::Node(
            NodeCmd::Update { at, .. }
            | NodeCmd::Link { at, .. }
            | NodeCmd::Key { at, .. }
            | NodeCmd::Close { at, .. }
            | NodeCmd::Reopen { at, .. }
            | NodeCmd::Move { at, .. }
            | NodeCmd::Merge { from: at, .. }
            | NodeCmd::Hide {
                target: HideArg::File { at, .. },
                ..
            },
        )
        | Cmd::Hold(HoldCmd::Take { target: at, .. } | HoldCmd::Check { target: at, .. })
        | Cmd::Talk(
            TalkCmd::Request { at, .. }
            | TalkCmd::Said { at, .. }
            | TalkCmd::Heard { at, .. }
            | TalkCmd::Ask { at, .. }
            | TalkCmd::Say { at, .. }
            | TalkCmd::Decide { at, .. }
            | TalkCmd::Approve { on: at, .. }
            | TalkCmd::Object { on: at, .. }
            | TalkCmd::Retract {
                target: RetractArg::Node(at),
                ..
            },
        )
        | Cmd::Control(ControlCmd::Pause { scope: at, .. } | ControlCmd::Unpause { scope: at }) => {
            one(at)
        }
        Cmd::View(ViewCmd::List { under, .. } | ViewCmd::Agreements { under, .. }) => opt(under),
        // 1.8 (§8.1): the node whose work the report is about, if it names one.
        Cmd::Talk(TalkCmd::ToUser { at, .. }) => opt(at),
        Cmd::Hold(HoldCmd::Next { query, .. }) => opt(&query.under),
        Cmd::Hold(
            HoldCmd::Checkpoint { token: t, .. }
            | HoldCmd::Renew { token: t, .. }
            | HoldCmd::Release { token: t, .. },
        ) => token(t),
        Cmd::Talk(TalkCmd::FromUser { to, .. } | TalkCmd::Route { to, .. }) => {
            match to.as_ref().and_then(crate::cmd::RouteTo::node) {
                Some(r) => one(r),
                None => Ok(None),
            }
        }
        Cmd::Talk(TalkCmd::Wait { for_, .. }) => opt(for_),
        Cmd::Talk(TalkCmd::Who { at }) => opt(at),
    }
}

/// The token a write carries, and the node it must hold (D.8.24 step 3): `checkpoint`, `renew`,
/// and `release` on the token's node; `update {token}` and `close {token}` on their `at`.
fn write_token(s: &State, cmd: &Cmd) -> Option<(Token, NodeId)> {
    match cmd {
        Cmd::Hold(
            HoldCmd::Checkpoint { token, .. }
            | HoldCmd::Renew { token, .. }
            | HoldCmd::Release { token, .. },
        ) => Some((*token, token.node)),
        Cmd::Node(
            NodeCmd::Update {
                at,
                token: Some(token),
                ..
            }
            | NodeCmd::Close {
                at,
                token: Some(token),
                ..
            },
        ) => query::resolve(s, at).map(|n| (*token, n)),
        Cmd::Session(_)
        | Cmd::View(_)
        | Cmd::Node(_)
        | Cmd::Hold(_)
        | Cmd::Talk(_)
        | Cmd::Control(_)
        | Cmd::Admin(_) => None,
    }
}

/// Whether `token` is a live hold on `n` at `now` (H4).
pub(crate) fn token_live(s: &State, token: Token, n: NodeId, now: Time) -> bool {
    live_on(s.holds.get(&token), token, n, now)
}

/// H4's predicate over the token's hold, if any: the token names `n`, its hold exists, and its
/// lease has not ended (§26 `live(h, now)`). Pure, for the Kani harness `stale_token_only_late`.
#[must_use]
pub(crate) fn live_on(h: Option<&crate::state::Hold>, token: Token, n: NodeId, now: Time) -> bool {
    token.node == n && h.is_some_and(|h| now < h.lease_until)
}

/// What steps 1 and 2 of D.8.1 decide before the session is read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Gate {
    /// A text over `text.max`: `limit(TextTooLong)`, nothing applied (L1, A-114).
    TextTooLong,
    /// A live record of the same digest from no session or its own: `Replay` (O1).
    Replay,
    /// A live record with another digest, or from another session: `op_id_conflict` (O1, CS-8).
    Conflict,
    /// No answer yet: the call goes on to the session.
    Proceed,
}

/// Steps 1 and 2 of D.8.1 as a pure function: the text size first, whatever the record (A-114);
/// then the live record of `(class, op_id)`, if any (`expires_at > now`, CS-8). The Kani harnesses
/// `op_retry_no_effect` and `stale_token_only_late` check it (D.12.5).
#[must_use]
pub(crate) fn gate(
    longest: u64,
    text_max: u32,
    record: Option<&crate::state::OpRecord>,
    digest: Option<crate::types::ReqDigest>,
    session: SessionCtx,
    now: Time,
) -> Gate {
    if longest > u64::from(text_max) {
        return Gate::TextTooLong;
    }
    let (Some(r), Some(d)) = (record.filter(|r| r.expires_at > now), digest) else {
        return Gate::Proceed;
    };
    let same_session = match session {
        SessionCtx::New => true,
        SessionCtx::Existing(x) => x == r.session,
        SessionCtx::System => false,
    };
    if r.digest == d && same_session {
        Gate::Replay
    } else {
        Gate::Conflict
    }
}

/// T1's clamp (§26 T1, D.8.1 step 1): a command runs at `max(ctx.now, last_now)`, so its events
/// never carry a time before an applied event's. Pure, for the Kani harness `event_stamps_t1`.
#[must_use]
pub(crate) fn clamp_now(ctx_now: Time, last_now: Time) -> Time {
    ctx_now.max(last_now)
}

/// A touch's `retire_at` (CS-8): `max(retire_at, now + session.retain)`, never decreasing.
#[must_use]
pub(crate) fn retire_after(old: Time, now: Time, retain: crate::types::Dur) -> Time {
    old.max(now.plus(retain))
}

/// Step 3 of D.8.24 for a write that carries a token: `stale_token` when it is not a live hold on
/// the target, with the eid the `late` entry gets (H4). Nothing before the late entry allocates an
/// eid, so the hint names it exactly.
///
/// # Errors
///
/// `stale_token`.
pub(crate) fn check_token(cx: &Vcx<'_>, token: Token, n: NodeId) -> Result<(), Error> {
    if token_live(cx.s, token, n, cx.now) {
        Ok(())
    } else {
        Err(Error::of(Hint::StaleToken {
            token,
            late: cx.s.counters.next_eid,
        }))
    }
}

/// Whether the plan issues a hold, which touches the session whatever its presence (CS-8: every
/// `HoldIssued` in a command step is preceded by a `Touched` or its session's `SessionOpened`).
fn issues_hold(plan: &Plan) -> bool {
    if let Plan::HoldTake(p) = plan {
        p.issues_hold()
    } else if let Plan::HoldNext(p) = plan {
        p.issues_hold()
    } else if let Plan::NodeCreate(p) = plan {
        p.issues_hold()
    } else {
        false
    }
}

/// `SessionOpened` for a first call, after `PrincipalSeen` for a new principal key (D.8.1 step 5,
/// D.8.23): `hello`'s arguments, its caps added to the token's default caps (CS-26, 1.6), and its
/// principal.
fn open_session(ex: &mut Exec<'_>, ctx: &Ctx, cmd: &Cmd, sess: SessionId) {
    let (caps, horizon, intent, principal) = match cmd {
        Cmd::Session(SessionCmd::Hello {
            intent,
            caps,
            horizon,
            principal,
            from_user: _,
        }) => (
            crate::cmd::hello_caps(caps.as_ref(), ctx.caps.as_ref()),
            *horizon,
            intent.clone(),
            principal.clone(),
        ),
        Cmd::Session(SessionCmd::Status { .. })
        | Cmd::View(_)
        | Cmd::Node(_)
        | Cmd::Hold(_)
        | Cmd::Talk(_)
        | Cmd::Control(_)
        | Cmd::Admin(_) => (ctx.caps.clone(), None, None, None),
    };
    let principal = principal.map(|k| intern(ex, &k));
    let now = ex.now();
    let params = &ex.s().params;
    let (silence, retain) = (params.presence_silence, params.session_retain);
    ex.emit(Ev::SessionOpened {
        session: sess,
        class: ctx.class.clone(),
        caps: caps.unwrap_or_default(),
        horizon,
        intent,
        silence_at: now.plus(silence),
        retire_at: now.plus(retain),
        principal,
    });
}

/// `Touched` (D.8.23, CS-14): when the session is silent, when presence is due
/// (`now >= last_seen + presence.resolution`), or when the command issues a hold.
fn touch(ex: &mut Exec<'_>, sess: SessionId, hold: bool) {
    let now = ex.now();
    let s = ex.s();
    let Some(x) = s.sessions.get(&sess) else {
        return;
    };
    let due = now >= x.last_seen.plus(s.params.presence_resolution);
    if !(x.silent || due || hold) {
        return;
    }
    let silence_at = now.plus(s.params.presence_silence);
    let retire_at = retire_after(x.retire_at, now, s.params.session_retain);
    ex.emit(Ev::Touched {
        session: sess,
        silence_at,
        retire_at,
    });
}

fn validate(cx: &Vcx<'_>, cmd: &Cmd, by_id: bool) -> Result<Plan, Error> {
    #[cfg(feature = "conformance")]
    if by_id {
        return dispatch::validate_by_id(cx, cmd);
    }
    let _ = by_id;
    dispatch::validate(cx, cmd)
}

/// D.8.1, steps 1 to 7.
fn run(
    s: &mut State,
    ctx: &Ctx,
    cmd: &Cmd,
    dirty: Option<&mut Tracked>,
    by_id: bool,
) -> (Vec<Event>, Result<Done, Error>) {
    // Step 1: the clock (T1) and the text size (L1, A-114). `apply` runs on a state settled at
    // `now` (D.8.1); the core does not panic on one that is not (D.1.3): T3's and T4's checks
    // report it.
    let now = clamp_now(ctx.now, s.last_now);
    // Step 2: the retry store (O1, CS-8). Never under `System` (D.5.1).
    let op = match ctx.session {
        SessionCtx::System => None,
        SessionCtx::New | SessionCtx::Existing(_) => ctx.op.as_ref(),
    };
    let record = op.and_then(|o| query::op_lookup(s, &ctx.class, &o.id, now));
    match gate(
        cmd.longest_text(),
        s.params.text_max,
        record,
        op.map(|o| o.digest),
        ctx.session,
        now,
    ) {
        Gate::TextTooLong => return (Vec::new(), Err(Error::limit(LimitKind::TextTooLong))),
        Gate::Replay => {
            let out = Ok(Done {
                reply: Reply::Replay,
                warnings: Vec::new(),
                opened: None,
                stale_session: false,
                delivered: Vec::new(),
            });
            return (Vec::new(), out);
        }
        Gate::Conflict => return (Vec::new(), Err(Error::op_id_conflict())),
        Gate::Proceed => {}
    }
    // Step 3: the session.
    let caller = match session_check(s, ctx, cmd) {
        Ok(c) => c,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let (sess, stale, new) = match caller {
        Caller::Existing { sess, stale } => (Some(sess), stale, false),
        Caller::New(sess) => (Some(sess), false, true),
        Caller::System => (None, false, false),
    };
    let mut ex = Exec::tracked(s, now, sess, dirty);
    if new && let Some(x) = sess {
        open_session(&mut ex, ctx, cmd, x);
    }
    // Step 4: validate, on the state the call's own `SessionOpened` left.
    let validated = {
        let cx = Vcx {
            s: ex.s(),
            now,
            ctx,
            sess,
        };
        validate(&cx, cmd, by_id)
    };
    // Step 5: execute.
    let result = match validated {
        Ok(plan) => {
            if !new && let Some(x) = sess {
                touch(&mut ex, x, issues_hold(&plan));
            }
            let first = ex.s().counters.next_eid;
            let reply = dispatch::execute(&mut ex, plan);
            // 1.7 (§13): notes ride on writes. The response to a write by an existing session
            // delivers up to WRITE_NOTES_MAX of its unread notes, oldest first, and marks only
            // those read, in the same step; `hello` and `talk wait`, reads, deliver their own. A
            // note the call itself recorded, to its own session, waits for the next call.
            let delivered = match sess {
                Some(x) if !new && !cmd.is_read() => {
                    let mut notes = crate::helpers::agents::inbox(ex.s(), x, now);
                    notes.retain(|e| *e < first);
                    notes.truncate(query::WRITE_NOTES_MAX);
                    if !notes.is_empty() {
                        ex.emit(Ev::InboxRead {
                            session: x,
                            notes: notes.iter().copied().collect(),
                        });
                    }
                    notes
                }
                _ => Vec::new(),
            };
            let mut warnings = ex.warnings().to_vec();
            let opened = if new { sess } else { None };
            if let Some(x) = opened
                && !matches!(cmd, Cmd::Session(SessionCmd::Hello { .. }))
            {
                warnings.push(Warning {
                    code: WarningCode::SessionOpened,
                    id: None,
                    detail: WarningDetail::SessionOpened { session: x },
                });
            }
            Ok(Done {
                reply,
                warnings,
                opened,
                stale_session: stale,
                delivered,
            })
        }
        Err(e) => {
            if e.code == ErrorCode::StaleToken
                && let Some(x) = sess
                && let Some((token, node)) = write_token(ex.s(), cmd)
            {
                ex.append(
                    node,
                    Author::Session(x),
                    EntryBody::Late {
                        token,
                        attempted: Box::new(cmd.clone()),
                    },
                    EntryText::default(),
                );
            }
            Err(e)
        }
    };
    // Step 6: the retry record (O1), last, when the call emitted anything (A-106).
    if let Some(op) = op
        && let Some(x) = sess
        && !ex.events().is_empty()
    {
        let expires_at = now.plus(ex.s().params.op_retain);
        ex.emit(Ev::OpRecorded {
            class: ctx.class.clone(),
            op: op.id.clone(),
            digest: op.digest,
            session: x,
            expires_at,
            outcome: result.clone(),
        });
    }
    let (events, _) = ex.finish();
    (events, result)
}

#[cfg(test)]
#[path = "apply_tests.rs"]
pub(crate) mod tests;
