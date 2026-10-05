//! Unit tests of `apply` and the commands of WP-D3 (design D.12.1): every call runs through
//! `World::call_ctx`, which checks `check_step`, the dirty set, `decide == apply`, the fold of the
//! events, the firings with `check_fire`, and every state invariant with R1 after the call.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    clippy::needless_range_loop,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::cmd::*;
use crate::error::{Error, ErrorCode, Field, Hint, LimitKind, What, Why};
use crate::event::{EntryBody, Ev};
use crate::params::Params;
use crate::query;
use crate::reply::{Done, NextReply, NoHoldWhy, Reply, WarningCode, WarningDetail};
use crate::state::Cand;
use crate::testkit::World;
use crate::types::*;

pub(crate) const T0: Time = Time(1_700_000_000_000);

pub(crate) fn class(name: &str) -> ClassName {
    ClassName::new(name).unwrap()
}

pub(crate) fn path(p: &str) -> Path {
    Path(
        p.split('/')
            .filter(|x| !x.is_empty())
            .map(|x| Name::new(x).unwrap())
            .collect(),
    )
}

pub(crate) fn at(p: &str) -> NodeRef {
    NodeRef::Path(path(p))
}

pub(crate) fn text(t: &str) -> Text {
    Text(String::from(t))
}

pub(crate) fn label(l: &str) -> Label {
    Label::new(l).unwrap()
}

pub(crate) fn world() -> World {
    World::new(Params::default(), T0)
}

pub(crate) fn create_args(p: &str) -> CreateArgs {
    CreateArgs {
        path: path(p),
        keys: Vec::new(),
        kind: None,
        title: text("t"),
        body: None,
        digest: None,
        fields: BTreeMap::new(),
        tags: BTreeSet::new(),
        capacity: None,
        needs: None,
        est: None,
        priority: None,
        due: None,
        depends_on: Vec::new(),
        owners: Vec::new(),
        repeat: None,
        agreements: None,
        request: None,
        take: None,
    }
}

pub(crate) fn create(p: &str) -> Cmd {
    Cmd::Node(NodeCmd::Create(create_args(p)))
}

pub(crate) fn create_with(p: &str, f: impl FnOnce(&mut CreateArgs)) -> Cmd {
    let mut a = create_args(p);
    f(&mut a);
    Cmd::Node(NodeCmd::Create(a))
}

pub(crate) fn take(n: NodeId) -> Cmd {
    Cmd::Hold(HoldCmd::Take {
        target: NodeRef::Id(n),
        lease: None,
    })
}

pub(crate) fn next() -> Cmd {
    Cmd::Hold(HoldCmd::Next {
        query: NextQuery::default(),
        lease: None,
    })
}

pub(crate) fn checkpoint(t: Token) -> Cmd {
    Cmd::Hold(HoldCmd::Checkpoint {
        token: t,
        done: text("did"),
        next_step: text("next"),
        intent: None,
        intent_done: None,
        files: Vec::new(),
    })
}

pub(crate) fn release(t: Token, reason: ReleaseReason) -> Cmd {
    Cmd::Hold(HoldCmd::Release {
        token: t,
        done: text("did"),
        next_step: text("next"),
        reason,
        waits_for: None,
        intent_done: None,
        learnings: None,
        unsent: BTreeSet::new(),
        settles: BTreeSet::new(),
    })
}

pub(crate) fn close(n: NodeId, outcome: Outcome) -> Cmd {
    Cmd::Node(NodeCmd::Close {
        at: NodeRef::Id(n),
        outcome,
        summary: text("done"),
        verified: None,
        settles: Some(BTreeSet::new()),
        learnings: None,
        token: None,
        unsent: BTreeSet::new(),
    })
}

pub(crate) fn ok(r: Result<Done, Error>) -> Done {
    r.unwrap_or_else(|e| panic!("refused: {e:?}"))
}

/// The id and the token of a create.
pub(crate) fn created(d: &Done) -> (NodeId, Option<Token>) {
    match &d.reply {
        Reply::Created { id, token, .. } => (*id, *token),
        other => panic!("not created: {other:?}"),
    }
}

#[test]
fn a_session_creates_takes_checkpoints_releases_and_closes() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let d = ok(w.call(&c, s, create("/a/b")));
    let (b, token) = created(&d);
    let token = token.expect("a hold");
    assert_eq!(w.s.nodes[&b].fence, Fence(1));
    let d = ok(w.call(&c, s, checkpoint(token)));
    assert!(matches!(d.reply, Reply::Checkpointed { .. }));
    let d = ok(w.call(&c, s, release(token, ReleaseReason::Handoff)));
    assert!(matches!(d.reply, Reply::Released { .. }));
    assert!(w.s.holds.is_empty());
    // The released node is offered, and another session's `next` takes it; 1.12 (§10.4): a
    // handoff keeps it from the session that released it.
    assert!(w.s.nodes[&b].tags.contains(&label("offer")));
    let d = ok(w.call(&c, s, next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Empty { .. })),
        "{:?}",
        d.reply
    );
    let s2 = w.hello(&c);
    let d = ok(w.call(&c, s2, next()));
    let Reply::Next(NextReply::Hold { node, token, .. }) = d.reply else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(node, b);
    let mut cl = close(b, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { token: t, .. }) = &mut cl {
        *t = Some(token);
    }
    let d = ok(w.call(&c, s2, cl));
    assert!(matches!(d.reply, Reply::Closed { .. }));
    assert!(!w.s.nodes[&b].is_open());
    assert!(w.s.holds.is_empty());
    assert_eq!(w.replay(), w.s);
}

// ------------------------------------------------------------------------------------------------
// Helpers for refusals

/// Calls `cmd`, which must be refused with `want`, appending nothing but what a refusal may: the
/// `SessionOpened` of a first call and its `OpRecorded`, or a `late` entry for `stale_token`.
pub(crate) fn refused(w: &mut World, c: &ClassName, s: SessionId, cmd: Cmd, want: Error) {
    let r = w.call(c, s, cmd);
    assert_eq!(r.as_ref().err(), Some(&want), "outcome {r:?}");
    if want.code == ErrorCode::StaleToken {
        assert!(w.last_events.iter().all(|e| matches!(
            e.ev,
            Ev::Appended {
                body: EntryBody::Late { .. },
                ..
            }
        )));
        assert_eq!(w.last_events.len(), 1);
    } else {
        assert!(w.last_events.is_empty(), "{:?}", w.last_events);
    }
}

fn nf(what: What) -> Error {
    Error::not_found(what)
}

fn inv(why: Why) -> Error {
    Error::invalid(why)
}

fn repeat_arg(cron: &str, run_within: Dur) -> RepeatArg {
    RepeatArg {
        cron_text: text(cron),
        cron: forum_cron::parse_cron(cron).unwrap(),
        tz: forum_cron::TzTable {
            name: String::from("UTC"),
            tzdb_version: String::new(),
            initial: 0,
            transitions: Vec::new(),
        },
        run_within,
    }
}

fn update(n: NodeId, version: Version, f: impl FnOnce(&mut Patch)) -> Cmd {
    let mut patch = Patch::default();
    f(&mut patch);
    Cmd::Node(NodeCmd::Update {
        at: NodeRef::Id(n),
        version,
        digest_version: None,
        token: None,
        patch,
    })
}

/// A world with one session of class `worker` and a node `/a` it created and holds.
fn held() -> (World, ClassName, SessionId, NodeId, Token) {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create("/a"))));
    (w, c, s, n, t.unwrap())
}

// ------------------------------------------------------------------------------------------------
// apply: text size, retries, sessions (D.8.1)

/// A command one byte over `text.max` is `limit(TextTooLong)` with no events, before O1 (a live
/// record with the same or another digest), before a session opens, and with a stale token (no
/// late entry) (A-114).
#[test]
fn a_text_over_text_max_reaches_nothing_else() {
    let (mut w, c, s, _, t) = held();
    let max = usize::try_from(w.s.params.text_max).unwrap();
    let long = Text("x".repeat(max + 1));
    let fits = Text("x".repeat(max));
    let cp = |next_step: Text, token: Token| {
        Cmd::Hold(HoldCmd::Checkpoint {
            token,
            done: text("d"),
            next_step,
            intent: None,
            intent_done: None,
            files: Vec::new(),
        })
    };
    // Exactly text.max is fine.
    ok(w.call(&c, s, cp(fits, t)));
    // One over: refused, nothing applied.
    refused(
        &mut w,
        &c,
        s,
        cp(long.clone(), t),
        Error::limit(LimitKind::TextTooLong),
    );
    // With a stale token: still no late entry.
    let stale = Token {
        node: t.node,
        fence: Fence(99),
    };
    refused(
        &mut w,
        &c,
        s,
        cp(long.clone(), stale),
        Error::limit(LimitKind::TextTooLong),
    );
    // Before a session opens.
    let n0 = w.s.sessions.len();
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let r = w.call_ctx(&ctx, &cp(long.clone(), t));
    assert_eq!(r, Err(Error::limit(LimitKind::TextTooLong)));
    assert!(w.last_events.is_empty());
    assert_eq!(w.s.sessions.len(), n0);
    // Before O1: a live record of the same op_id, with the same or another digest.
    let op = OpCtx {
        id: OpId::new("op-1").unwrap(),
        digest: ReqDigest([1; 32]),
    };
    let ctx = w.ctx(&c, SessionCtx::Existing(s), Some(op.clone()));
    ok(w.call_ctx(&ctx, &checkpoint(t)));
    assert!(w.s.ops.contains_key(&(c.clone(), op.id.clone())));
    for digest in [op.digest, ReqDigest([2; 32])] {
        let ctx = w.ctx(
            &c,
            SessionCtx::Existing(s),
            Some(OpCtx {
                id: op.id.clone(),
                digest,
            }),
        );
        let r = w.call_ctx(&ctx, &cp(long.clone(), t));
        assert_eq!(r, Err(Error::limit(LimitKind::TextTooLong)));
        assert!(w.last_events.is_empty());
    }
}

/// O1 (CS-8, A-106): a repeat from the same session or from none is `Replay` with no events; from
/// another session, or with another digest, `op_id_conflict`; after `op.retain` the record is gone
/// and the call runs afresh; an error that appended nothing records nothing.
#[test]
fn retries_replay_conflict_and_expire() {
    let (mut w, c, s, n, t) = held();
    let op = OpCtx {
        id: OpId::new("op-7").unwrap(),
        digest: ReqDigest([7; 32]),
    };
    let ctx = w.ctx(&c, SessionCtx::Existing(s), Some(op.clone()));
    let first = ok(w.call_ctx(&ctx, &checkpoint(t)));
    assert!(matches!(
        w.last_events.last().map(|e| &e.ev),
        Some(Ev::OpRecorded { .. })
    ));
    // The same session, and no session: Replay, no events.
    for sc in [SessionCtx::Existing(s), SessionCtx::New] {
        let ctx = w.ctx(&c, sc, Some(op.clone()));
        let d = ok(w.call_ctx(&ctx, &checkpoint(t)));
        assert_eq!(d.reply, Reply::Replay);
        assert!(w.last_events.is_empty());
    }
    // Another digest, and another session: op_id_conflict, no events.
    let s2 = w.hello(&c);
    let other = OpCtx {
        id: op.id.clone(),
        digest: ReqDigest([8; 32]),
    };
    for (sc, o) in [
        (SessionCtx::Existing(s), other.clone()),
        (SessionCtx::Existing(s2), op.clone()),
    ] {
        let ctx = w.ctx(&c, sc, Some(o));
        assert_eq!(
            w.call_ctx(&ctx, &checkpoint(t)),
            Err(Error::op_id_conflict())
        );
        assert!(w.last_events.is_empty());
    }
    // Another class's record is another key.
    let c2 = class("other");
    let s3 = w.hello(&c2);
    let ctx = w.ctx(&c2, SessionCtx::Existing(s3), Some(op.clone()));
    ok(w.call_ctx(&ctx, &Cmd::View(ViewCmd::Constitution)));
    // An error that appended nothing is not recorded.
    let op2 = OpCtx {
        id: OpId::new("op-8").unwrap(),
        digest: ReqDigest([9; 32]),
    };
    let ctx = w.ctx(&c, SessionCtx::Existing(s), Some(op2.clone()));
    let missing = Cmd::Hold(HoldCmd::Take {
        target: NodeRef::Id(NodeId(999)),
        lease: None,
    });
    assert_eq!(w.call_ctx(&ctx, &missing), Err(nf(What::Node)));
    assert!(!w.s.ops.contains_key(&(c.clone(), op2.id.clone())));
    // After op.retain the record is gone: the same op_id runs afresh.
    let retain = w.s.params.op_retain;
    let lease_first = w.s.params.lease_first;
    let _ = (first, n, lease_first);
    w.advance(retain);
    assert!(!w.s.ops.contains_key(&(c.clone(), op.id.clone())));
    let s4 = w.hello(&c);
    let ctx = w.ctx(&c, SessionCtx::Existing(s4), Some(op.clone()));
    let d = ok(w.call_ctx(&ctx, &Cmd::View(ViewCmd::Constitution)));
    assert_ne!(d.reply, Reply::Replay);
}

/// Step 3 of D.8.1: a session that does not exist, one of another class, `Admin` outside
/// `System`, and another command under `System`.
#[test]
fn the_session_check() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let view = Cmd::View(ViewCmd::Constitution);
    refused(&mut w, &c, SessionId(99), view.clone(), nf(What::Session));
    refused(
        &mut w,
        &class("other"),
        s,
        view.clone(),
        inv(Why::ClassMismatch),
    );
    let admin = Cmd::Admin(AdminCmd::SetParams(Params::default()));
    refused(&mut w, &c, s, admin.clone(), inv(Why::BadParams));
    let ctx = w.ctx(&c, SessionCtx::System, None);
    assert_eq!(w.call_ctx(&ctx, &view), Err(inv(Why::BadParams)));
    assert!(w.last_events.is_empty());
    // Under System the admin command runs, with no session, no Touched, and no OpRecorded.
    let p = Params {
        leave_better_max: 5,
        ..Params::default()
    };
    let ctx = w.ctx(&c, SessionCtx::System, None);
    let d = ok(w.call_ctx(&ctx, &Cmd::Admin(AdminCmd::SetParams(p.clone()))));
    assert_eq!(d.reply, Reply::ParamsSet);
    assert_eq!(w.last_events.len(), 1);
    assert_eq!(w.s.params, p);
    // Params that fail `Params::valid`, or current usage, are refused (D.11.8).
    let bad = Params {
        lease_first: Dur(0),
        ..Params::default()
    };
    assert_eq!(
        w.call_ctx(&ctx, &Cmd::Admin(AdminCmd::SetParams(bad))),
        Err(inv(Why::BadParams))
    );
    ok(w.call(&c, s, create("/a")));
    ok(w.call(&c, s, create("/b")));
    let fewer = Params {
        hold_per_session: 1,
        ..Params::default()
    };
    assert_eq!(
        w.call_ctx(&ctx, &Cmd::Admin(AdminCmd::SetParams(fewer))),
        Err(inv(Why::BadParams))
    );
    ok(w.call(&c, s, create("/c/d/e/f")));
    let shallow = Params {
        tree_max_depth: 3,
        ..Params::default()
    };
    assert_eq!(
        w.call_ctx(&ctx, &Cmd::Admin(AdminCmd::SetParams(shallow))),
        Err(inv(Why::BadParams))
    );
}

/// A first call opens its session even when it fails (A-38), and a call other than `hello` that
/// opens one succeeds with `session_opened` (CS-29); its caps are the token's default caps
/// (CS-26), and `hello`'s caps are added to them (1.6).
#[test]
fn first_calls_open_their_session() {
    let mut w = world();
    let c = class("worker");
    let caps: BTreeSet<Label> = [label("x")].into();
    let mut ctx = w.ctx(&c, SessionCtx::New, None);
    ctx.caps = Some(caps.clone());
    // A failing first call opens its session.
    let missing = Cmd::Hold(HoldCmd::Take {
        target: NodeRef::Id(NodeId(999)),
        lease: None,
    });
    assert_eq!(w.call_ctx(&ctx, &missing), Err(nf(What::Node)));
    assert!(matches!(
        w.last_events.as_slice(),
        [crate::event::Event {
            ev: Ev::SessionOpened { .. },
            ..
        }]
    ));
    let s1 = SessionId(1);
    assert_eq!(w.s.sessions[&s1].caps, caps);
    // A successful non-hello first call warns `session_opened`.
    let d = ok(w.call_ctx(&ctx, &create("/a")));
    let s2 = d.opened.unwrap();
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::SessionOpened
                && x.detail == WarningDetail::SessionOpened { session: s2 })
    );
    // A hold issued by the call that opens its session (H2's step check holds).
    assert!(matches!(d.reply, Reply::Created { token: Some(_), .. }));
    // `hello` with caps adds them to the token's (1.6); without, it keeps the token's; no warning.
    let hello = |caps: Option<BTreeSet<Label>>| {
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps,
            horizon: Some(30),
            principal: Some(PrincipalKey::new("mail:a@b").unwrap()),
            from_user: None,
        })
    };
    let d = ok(w.call_ctx(&ctx, &hello(Some(BTreeSet::new()))));
    let s3 = d.opened.unwrap();
    assert!(d.warnings.is_empty());
    assert_eq!(w.s.sessions[&s3].caps, caps);
    let more: BTreeSet<Label> = [label("y")].into();
    let d = ok(w.call_ctx(&ctx, &hello(Some(more.clone()))));
    let both: BTreeSet<Label> = [label("x"), label("y")].into();
    assert_eq!(w.s.sessions[&d.opened.unwrap()].caps, both);
    assert_eq!(w.s.sessions[&s3].horizon, Some(30));
    assert_eq!(w.s.sessions[&s3].principals.len(), 1);
    let d = ok(w.call_ctx(&ctx, &hello(None)));
    assert_eq!(w.s.sessions[&d.opened.unwrap()].caps, caps);
    // `hello` on an existing session sets what it gives (SessionHello).
    let d = ok(w.call(&c, s3, hello(Some(caps.clone()))));
    assert!(matches!(d.reply, Reply::Hello { session, .. } if session == s3));
    assert_eq!(w.s.sessions[&s3].caps, caps);
    assert!(
        w.last_events
            .iter()
            .any(|e| matches!(e.ev, Ev::SessionHello { .. }))
    );
}

/// `Touched` only when presence is due, the session is silent, or the command issues a hold
/// (CS-14, CS-8); a holder is never silent while its hold is live, even after a checkpoint within
/// `presence.resolution` of the take extended the lease past `presence.silence` (1.4, R27); a
/// silent session's next successful call reports `stale_session` (§3).
#[test]
fn presence_is_refreshed_at_its_resolution() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let touched = |w: &World| {
        w.last_events
            .iter()
            .any(|e| matches!(e.ev, Ev::Touched { .. }))
    };
    w.advance(Dur(60_000));
    ok(w.call(&c, s, create_with("/a", |a| a.take = Some(false))));
    assert!(!touched(&w), "within presence.resolution, no hold");
    w.advance(w.s.params.presence_resolution);
    ok(w.call(&c, s, Cmd::View(ViewCmd::Constitution)));
    assert!(touched(&w), "presence is due");
    let (_, t) = created(&ok(w.call(&c, s, create("/b"))));
    assert!(touched(&w), "a hold-issuing command touches");
    let t = t.unwrap();
    w.advance(Dur(10_000));
    ok(w.call(&c, s, checkpoint(t)));
    assert!(
        !touched(&w),
        "a checkpoint within presence.resolution touches nothing"
    );
    let lease = w.s.holds[&t].lease_until;
    let silence_at = w.s.sessions[&s].silence_at;
    assert!(lease > silence_at, "the lease passes presence.silence");
    w.at(silence_at);
    w.settle();
    assert!(!w.s.sessions[&s].silent, "a holder is never silent");
    w.at(lease);
    w.settle();
    assert!(w.s.holds.is_empty());
    assert!(w.s.sessions[&s].silent, "silent at its last lease's end");
    let d = ok(w.call(&c, s, Cmd::View(ViewCmd::Constitution)));
    assert!(d.stale_session);
    assert!(touched(&w));
    assert!(!w.s.sessions[&s].silent);
}

/// A session retires `session.retain` after its last touch, not before, holding nothing; a call
/// naming it then gets `not_found(Session)` with no events (CS-8). `retire_at` never decreases
/// after a `ParamsChanged` that lowers `session.retain`.
#[test]
fn sessions_retire_and_the_retire_arm() {
    let (mut w, c, s, _, _) = held();
    let retire_at = w.s.sessions[&s].retire_at;
    let retain = w.s.params.session_retain;
    // Lower session.retain: the stored retire_at stays.
    let ctx = w.ctx(&c, SessionCtx::System, None);
    let p = Params {
        session_retain: Dur(retain.0 / 2),
        ..Params::default()
    };
    ok(w.call_ctx(&ctx, &Cmd::Admin(AdminCmd::SetParams(p))));
    assert_eq!(w.s.sessions[&s].retire_at, retire_at);
    // A touch under the lowered parameter keeps the later retire_at (D.12.1): a call once presence
    // is due, and a call that issues a hold.
    w.advance(w.s.params.presence_resolution);
    ok(w.call(&c, s, Cmd::View(ViewCmd::Constitution)));
    assert!(
        w.last_events
            .iter()
            .any(|e| matches!(e.ev, Ev::Touched { session, .. } if session == s)),
        "presence was due: the call touched the session"
    );
    assert_eq!(w.s.sessions[&s].retire_at, retire_at);
    ok(w.call(&c, s, create("/b")));
    assert!(
        w.last_events
            .iter()
            .any(|e| matches!(e.ev, Ev::HoldIssued { session, .. } if session == s))
    );
    assert_eq!(w.s.sessions[&s].retire_at, retire_at);
    w.at(Time(retire_at.0 - 1));
    w.settle();
    assert!(w.s.sessions.contains_key(&s), "not before retire_at");
    assert!(w.s.holds.is_empty(), "every hold ended before retire_at");
    w.at(retire_at);
    let evs = w.settle();
    assert!(
        evs.iter()
            .any(|e| matches!(e.ev, Ev::SessionRetired { session } if session == s))
    );
    assert!(!w.s.sessions.contains_key(&s));
    refused(
        &mut w,
        &c,
        s,
        Cmd::View(ViewCmd::Constitution),
        nf(What::Session),
    );
}

/// T1's clamp (D.8.1 step 1): a command whose `ctx.now` lies before `last_now` runs at `last_now`,
/// so every event it emits carries `last_now` and the recorded time does not move back; one that
/// emits nothing leaves `last_now` alone. `World::call_ctx` never sends an earlier clock, so this
/// calls `apply` directly.
#[test]
fn a_clock_behind_the_log_is_clamped() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    w.advance(Dur(60_000));
    ok(w.call(&c, s, create("/a")));
    w.settle();
    let before = w.s.clone();
    let last = before.last_now;
    let mut ctx = w.ctx(&c, SessionCtx::Existing(s), None);
    ctx.now = Time(last.0 - 30_000);
    for cmd in [create("/b"), Cmd::View(ViewCmd::Constitution)] {
        let mut after = before.clone();
        let (events, out) = crate::apply::apply(&mut after, &ctx, &cmd);
        assert!(out.is_ok(), "{out:?}");
        assert!(events.iter().all(|e| e.at == last), "{events:?}");
        assert_eq!(after.last_now, last);
        if let Err(v) = crate::invariants::check_step(&before, &ctx, &cmd, &out, &events, &after) {
            panic!("{v:?}");
        }
    }
}

/// `settle` returns `Stuck` for a firing whose transition fails its precondition (D.8.2): a
/// session whose Retire deadline comes while it is not yet silent.
#[test]
fn settle_is_stuck_on_a_failed_transition() {
    let mut w = world();
    let s = w.open(&class("worker"));
    let now = w.clock;
    w.emit(Ev::Touched {
        session: s,
        silence_at: Time(now.0 + 10_000),
        retire_at: w.s.sessions[&s].retire_at,
    });
    // Lower retire_at below silence_at by hand: a state no command reaches.
    w.s.sessions.get_mut(&s).unwrap().retire_at = Time(now.0 + 5_000);
    w.s.ix.deadlines = crate::deadlines::recount_deadlines(&w.s);
    w.s.ix.deadline_at =
        w.s.ix
            .deadlines
            .iter()
            .map(|k| ((k.kind, k.owner.clone()), k.at))
            .collect();
    let (_, st) = crate::settle::settle(&mut w.s, Time(now.0 + 6_000), 10);
    assert!(matches!(
        st,
        crate::reply::Settled::Stuck(crate::state::DeadlineKey {
            kind: crate::state::DeadlineKind::Retire,
            ..
        })
    ));
}

/// A write with a stale token is `stale_token` with one `late` entry on the target (H4); for
/// `update` and `close`, on the node they name.
#[test]
fn stale_tokens_save_a_late_entry() {
    let (mut w, c, s, n, t) = held();
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    let late = w.s.counters.next_eid;
    refused(
        &mut w,
        &c,
        s,
        checkpoint(t),
        Error::of(Hint::StaleToken { token: t, late }),
    );
    let other = created(&ok(w.call(&c, s, create("/b")))).0;
    let late = w.s.counters.next_eid;
    let mut cl = close(other, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { token, .. }) = &mut cl {
        *token = Some(t);
    }
    refused(
        &mut w,
        &c,
        s,
        cl,
        Error::of(Hint::StaleToken { token: t, late }),
    );
    assert!(matches!(
        w.last_events.first().map(|e| &e.ev),
        Some(Ev::Appended { node, .. }) if *node == other
    ));
    let _ = n;
}

// ------------------------------------------------------------------------------------------------
// node create (D.8.9)

#[test]
fn create_refusals_in_order() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let key = |k: &str| Key {
        key: KeyStr::new(k).unwrap(),
        nv: NormVer(1),
    };
    // The keys (D.8.9 step 1): a `repeat:` key, then the user entries they name.
    let missing_entry = Key {
        key: KeyStr::of_user_entry(Eid(999)),
        nv: NormVer(0),
    };
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| {
            a.keys = vec![missing_entry.clone(), key("repeat:9:x")];
        }),
        inv(Why::ReservedKey),
    );
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.keys = vec![missing_entry.clone()]),
        nf(What::UserEntry),
    );
    // The request (step 2), after the keys: D.8.24's row puts `not_found(UserEntry)` before
    // `invalid(NoRequestOrigin)` and the request's `invalid(ReservedKey)`.
    let req = |origin: Option<Key>| RequestArg {
        text: text("please"),
        origin,
        where_: None,
        requester: text("bob"),
    };
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| {
            a.keys = vec![missing_entry.clone()];
            a.request = Some(req(Some(key("repeat:1:y"))));
        }),
        nf(What::UserEntry),
    );
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.request = Some(req(None))),
        inv(Why::NoRequestOrigin),
    );
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.request = Some(req(Some(key("repeat:1:y"))))),
        inv(Why::ReservedKey),
    );
    // The create's shape.
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.title = text("")),
        inv(Why::EmptyText(Field::Title)),
    );
    let long = Text("d".repeat(usize::try_from(w.s.params.digest_max).unwrap() + 1));
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.digest = Some(long)),
        inv(Why::DigestTooLong),
    );
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.repeat = Some(repeat_arg("0 7 * * *", Dur(0)))),
        inv(Why::RunWithinZero),
    );
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| {
            let mut r = repeat_arg("0 7 * * *", Dur(60_000));
            r.tz.transitions = vec![
                forum_cron::Transition { at: 10, offset: 0 },
                forum_cron::Transition { at: 5, offset: 0 },
            ];
            a.repeat = Some(r);
        }),
        inv(Why::BadTimeZone),
    );
    // References.
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.depends_on = vec![NodeRef::Id(NodeId(999))]),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        create_with("/a", |a| a.owners = vec![at("/nope")]),
        nf(What::Node),
    );
    // State.
    ok(w.call(&c, s, create_with("/a", |a| a.take = Some(false))));
    refused(&mut w, &c, s, create("/a"), inv(Why::NameTaken));
    refused(&mut w, &c, s, create("/"), inv(Why::NameTaken));
    refused(
        &mut w,
        &c,
        s,
        create_with("/b", |a| {
            a.repeat = Some(repeat_arg("0 0 30 2 *", Dur(60_000)))
        }),
        inv(Why::ScheduleNeverFires),
    );
    // Safety valves (L1).
    let deep: String = (0..17).map(|i| alloc::format!("/d{i}")).collect();
    refused(&mut w, &c, s, create(&deep), Error::limit(LimitKind::Depth));
}

/// The child limit counts open agent-made children only, and never the persons and teams of the
/// directory (L1, CS-10, A-113).
#[test]
fn the_child_limit_spares_people_and_teams() {
    let mut w = World::new(
        Params {
            tree_max_children: 2,
            hold_per_session: 1,
            ..Params::default()
        },
        T0,
    );
    let c = class("worker");
    let s = w.hello(&c);
    for name in ["/p/a", "/p/b"] {
        ok(w.call(&c, s, create_with(name, |a| a.take = Some(false))));
    }
    refused(
        &mut w,
        &c,
        s,
        create_with("/p/c", |a| a.take = Some(false)),
        Error::limit(LimitKind::Children),
    );
    // Closing one frees a slot.
    let b = query::resolve(&w.s, &at("/p/b")).unwrap();
    ok(w.call(&c, s, close(b, Outcome::Done)));
    ok(w.call(&c, s, create_with("/p/c", |a| a.take = Some(false))));
    // @people and @teams take more than tree.max_children.
    for dir in ["@people", "@teams"] {
        for i in 0..4 {
            let p = alloc::format!("/{dir}/x{i}");
            ok(w.call(&c, s, create_with(&p, |a| a.take = Some(false))));
        }
    }
    // A child of a person still counts against that person.
    for i in 0..2 {
        let p = alloc::format!("/@people/x0/c{i}");
        ok(w.call(&c, s, create_with(&p, |a| a.take = Some(false))));
    }
    refused(
        &mut w,
        &c,
        s,
        create_with("/@people/x0/c2", |a| a.take = Some(false)),
        Error::limit(LimitKind::Children),
    );
}

/// Missing ancestors copy their parent's needs, a create without `needs` copies its parent's, and
/// `needs: []` means none (CS-26); the hold, the `offer` tag, and `no_hold` (§7, A-65).
#[test]
fn create_defaults_and_holds() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let x: BTreeSet<Label> = [label("x")].into();
    ok(w.call(
        &c,
        s,
        create_with("/team", |a| {
            a.needs = Some(x.clone());
            a.take = Some(false);
        }),
    ));
    let d = ok(w.call(&c, s, create_with("/team/a/b", |a| a.take = Some(false))));
    let (b, _) = created(&d);
    let a = query::resolve(&w.s, &at("/team/a")).unwrap();
    assert_eq!(w.s.nodes[&a].needs, x);
    assert_eq!(w.s.nodes[&a].origin, crate::state::Origin::Ancestor(s));
    assert_eq!(w.s.nodes[&b].needs, x);
    let (none, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/team/c", |a| {
            a.needs = Some(BTreeSet::new());
            a.take = Some(false);
        }),
    )));
    assert!(w.s.nodes[&none].needs.is_empty());
    assert!(!w.s.nodes[&none].tags.contains(&label("offer")));
    // capacity 0: no hold, no tag, no_hold(NoCapacity).
    let d = ok(w.call(&c, s, create_with("/z", |a| a.capacity = Some(0))));
    assert!(matches!(d.reply, Reply::Created { token: None, .. }));
    assert!(d.warnings.iter().any(|x| x.detail
        == WarningDetail::NoHold {
            why: NoHoldWhy::NoCapacity
        }));
    // At hold.per_session: the offer tag and no_hold(HoldsLimit).
    ok(w.call(&c, s, create("/h1")));
    ok(w.call(&c, s, create("/h2")));
    let d = ok(w.call(&c, s, create("/h3")));
    let (h3, t) = created(&d);
    assert!(t.is_none());
    assert!(w.s.nodes[&h3].tags.contains(&label("offer")));
    assert!(d.warnings.iter().any(|x| x.detail
        == WarningDetail::NoHold {
            why: NoHoldWhy::HoldsLimit
        }));
    // Under a pause: no_hold(Paused).
    let s2 = w.hello(&c);
    ok(w.call(
        &c,
        s2,
        Cmd::Control(ControlCmd::Pause {
            scope: at("/team"),
            reason: text("stop"),
        }),
    ));
    let d = ok(w.call(&c, s2, create("/team/p")));
    assert!(d.warnings.iter().any(|x| x.detail
        == WarningDetail::NoHold {
            why: NoHoldWhy::Paused
        }));
}

/// A create on a key with closed carriers links `continues` to the newest only, and `view find
/// {key}` lists the open carrier, then the closed ones newest (largest id) first (§7, A-117).
#[test]
fn find_lists_carriers_newest_first() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let k = Key {
        key: KeyStr::new("issue:p#1").unwrap(),
        nv: NormVer(1),
    };
    let mut ids = Vec::new();
    for name in ["/a", "/b", "/c"] {
        let d = ok(w.call(
            &c,
            s,
            create_with(name, |a| {
                a.keys = vec![k.clone()];
                a.take = Some(false);
            }),
        ));
        let (n, _) = created(&d);
        if let Some(prev) = ids.last() {
            assert!(
                matches!(&d.reply, Reply::Created { continues, .. } if continues == &vec![*prev])
            );
        }
        ids.push(n);
        if name != "/c" {
            ok(w.call(&c, s, close(n, Outcome::Failed)));
        }
    }
    let f = query::find_key(&w.s, &k.key);
    assert_eq!(f.open, Some(ids[2]));
    assert_eq!(f.closed, vec![ids[1], ids[0]]);
    // The brief walks the chain of `continues`.
    let b = query::brief(&w.s, ids[2], Some(s), w.clock, &mut query::NoMeter);
    assert_eq!(
        b.continues.iter().map(|p| p.node).collect::<Vec<_>>(),
        vec![ids[1], ids[0]]
    );
    let d = ok(w.call(
        &c,
        s,
        Cmd::View(ViewCmd::Find {
            by: FindBy::Key(k.key.clone()),
        }),
    ));
    assert!(matches!(d.reply, Reply::ViewFind { .. }));
}

// ------------------------------------------------------------------------------------------------
// node update (D.8.20)

#[test]
fn update_refusals_in_order() {
    let (mut w, c, s, n, t) = held();
    let v = w.s.nodes[&n].version;
    // The node, then the token.
    refused(
        &mut w,
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(NodeId(999)),
            version: v,
            digest_version: None,
            token: None,
            patch: Patch::default(),
        }),
        nf(What::Node),
    );
    let stale = Token {
        node: n,
        fence: Fence(5),
    };
    let late = w.s.counters.next_eid;
    refused(
        &mut w,
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(n),
            version: v,
            digest_version: None,
            token: Some(stale),
            patch: Patch::default(),
        }),
        Error::of(Hint::StaleToken { token: stale, late }),
    );
    let v = w.s.nodes[&n].version;
    // Shape.
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| p.title = Some(text(""))),
        inv(Why::EmptyText(Field::Title)),
    );
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| p.digest = Some(text("d"))),
        inv(Why::MissingDigestVersion),
    );
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| {
            p.repeat = Change::Set(repeat_arg("0 7 * * *", Dur(0)))
        }),
        inv(Why::RunWithinZero),
    );
    // References, then versions.
    refused(
        &mut w,
        &c,
        s,
        update(n, Version(99), |p| {
            p.depends_on = Some(vec![NodeRef::Id(NodeId(999))])
        }),
        nf(What::Node),
    );
    let dv = w.s.nodes[&n].digest_version;
    refused(
        &mut w,
        &c,
        s,
        update(n, Version(99), |_| {}),
        Error::of(Hint::StaleVersion {
            current: v,
            digest_current: dv,
        }),
    );
    refused(
        &mut w,
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(n),
            version: v,
            digest_version: Some(Version(dv.0 + 1)),
            token: None,
            patch: Patch {
                digest: Some(text("d")),
                ..Patch::default()
            },
        }),
        Error::of(Hint::StaleVersion {
            current: v,
            digest_current: dv,
        }),
    );
    // State: reserved nodes, schedules, cycles.
    let rv = w.s.nodes[&ROOT].version;
    refused(
        &mut w,
        &c,
        s,
        update(ROOT, rv, |p| p.capacity = Some(1)),
        inv(Why::ReservedNode),
    );
    refused(
        &mut w,
        &c,
        s,
        update(ROOT, rv, |p| {
            p.repeat = Change::Set(repeat_arg("0 7 * * *", Dur(60_000)))
        }),
        inv(Why::ReservedNode),
    );
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| {
            p.repeat = Change::Set(repeat_arg("0 0 30 2 *", Dur(60_000)))
        }),
        inv(Why::ScheduleNeverFires),
    );
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| p.depends_on = Some(vec![NodeRef::Id(n)])),
        Error::of(Hint::Cycle { path: vec![n, n] }),
    );
    let (m, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/m", |a| {
            a.depends_on = vec![NodeRef::Id(n)];
            a.take = Some(false);
        }),
    )));
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| p.depends_on = Some(vec![NodeRef::Id(m)])),
        Error::of(Hint::Cycle {
            path: vec![n, m, n],
        }),
    );
    // Resources: a capacity below the live holds.
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| p.capacity = Some(0)),
        Error::of(Hint::Full {
            node: n,
            capacity: 0,
            live: 1,
        }),
    );
    let _ = t;
}

/// An update changes only the fields that differ (O2): tags and needs by additions and removals,
/// which commute (CS-34); an update that changes nothing appends nothing but presence; the digest
/// needs its version (CS-11).
#[test]
fn updates_patch_what_differs() {
    let (mut w, c, s, n, _) = held();
    let v = w.s.nodes[&n].version;
    let d = ok(w.call(
        &c,
        s,
        update(n, v, |p| {
            p.title = Some(text("new"));
            p.status = Some(text("doing"));
            p.tags_add = [label("a"), label("b")].into();
        }),
    ));
    let Reply::Updated { version, .. } = d.reply else {
        panic!()
    };
    assert_eq!(version, Version(v.0 + 1));
    assert_eq!(w.s.nodes[&n].tags, [label("a"), label("b")].into());
    // Two patches from one version, in either order, keep both labels.
    let v = w.s.nodes[&n].version;
    ok(w.call(&c, s, update(n, v, |p| p.tags_remove = [label("a")].into())));
    ok(w.call(&c, s, update(n, v, |p| p.tags_add = [label("c")].into())));
    assert_eq!(w.s.nodes[&n].tags, [label("b"), label("c")].into());
    // Nothing differs: no NodeUpdated.
    ok(w.call(&c, s, update(n, v, |p| p.title = Some(text("new")))));
    assert!(
        !w.last_events
            .iter()
            .any(|e| matches!(e.ev, Ev::NodeUpdated { .. }))
    );
    // The digest with its version.
    let dv = w.s.nodes[&n].digest_version;
    let d = ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(n),
            version: v,
            digest_version: Some(dv),
            token: None,
            patch: Patch {
                digest: Some(text("sum")),
                ..Patch::default()
            },
        }),
    ));
    assert!(
        matches!(d.reply, Reply::Updated { digest_version, .. } if digest_version == Version(dv.0 + 1))
    );
}

// ------------------------------------------------------------------------------------------------
// node close and hide (D.8.20)

#[test]
fn close_refusals_in_order() {
    let (mut w, c, s, n, t) = held();
    refused(
        &mut w,
        &c,
        s,
        close(NodeId(999), Outcome::Done),
        nf(What::Node),
    );
    let stale = Token {
        node: n,
        fence: Fence(9),
    };
    let late = w.s.counters.next_eid;
    let mut cl = close(n, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { token, .. }) = &mut cl {
        *token = Some(stale);
    }
    refused(
        &mut w,
        &c,
        s,
        cl,
        Error::of(Hint::StaleToken { token: stale, late }),
    );
    refused(
        &mut w,
        &c,
        s,
        close(n, Outcome::Missed),
        inv(Why::ServerOutcome),
    );
    let mut cl = close(n, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { summary, .. }) = &mut cl {
        *summary = text("");
    }
    refused(&mut w, &c, s, cl, inv(Why::EmptyText(Field::Summary)));
    let mut cl = close(n, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { settles, .. }) = &mut cl {
        *settles = Some([Eid(999)].into());
    }
    refused(&mut w, &c, s, cl, nf(What::Request));
    let mut cl = close(n, Outcome::Done);
    if let Cmd::Node(NodeCmd::Close { unsent, .. }) = &mut cl {
        *unsent = [Eid(999)].into();
    }
    refused(&mut w, &c, s, cl, nf(What::Said));
    for r in [ROOT, USER, PEOPLE] {
        refused(
            &mut w,
            &c,
            s,
            close(r, Outcome::Done),
            inv(Why::ReservedNode),
        );
    }
    ok(w.call(&c, s, close(n, Outcome::Done)));
    refused(
        &mut w,
        &c,
        s,
        close(n, Outcome::Done),
        inv(Why::AlreadyClosed),
    );
    let _ = t;
}

/// A close ends every hold (H6), with `held_by_other` for other sessions', the `agreement`
/// warning with the approvals of the current content (CS-34), and `leave_better` (§10.3).
#[test]
fn close_ends_holds_and_warns() {
    let mut w = world();
    let c = class("worker");
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    ok(w.call(
        &c,
        s1,
        create_with("/p", |a| {
            a.agreements = Some(text("review first"));
            a.take = Some(false);
        }),
    ));
    let (n, _) = created(&ok(w.call(
        &c,
        s1,
        create_with("/p/x", |a| a.capacity = Some(2)),
    )));
    ok(w.call(&c, s2, take(n)));
    let d = ok(w.call(&c, s1, close(n, Outcome::Done)));
    assert!(w.s.holds.is_empty());
    assert!(d.warnings.iter().any(|x| x.detail
        == WarningDetail::HeldByOther {
            holders: vec![s2],
            left: vec![]
        }));
    let p = query::resolve(&w.s, &at("/p")).unwrap();
    assert!(d.warnings.iter().any(|x| matches!(&x.detail,
        WarningDetail::Agreement { sources, approvals: 0, .. } if sources == &vec![p])));
    assert!(matches!(d.reply, Reply::Closed { node, .. } if node == n));
}

#[test]
fn hide_refusals_and_effect() {
    let (mut w, c, s, n, _) = held();
    let hide = |target: HideArg, reason: &str| {
        Cmd::Node(NodeCmd::Hide {
            target,
            reason: text(reason),
        })
    };
    let entry = HideArg::Entry {
        eid: Eid(1),
        node: n,
    };
    refused(
        &mut w,
        &c,
        s,
        hide(
            HideArg::Entry {
                eid: Eid(9_999),
                node: n,
            },
            "r",
        ),
        nf(What::Entry),
    );
    refused(
        &mut w,
        &c,
        s,
        hide(
            HideArg::File {
                at: NodeRef::Id(n),
                name: text("f"),
            },
            "r",
        ),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        hide(
            HideArg::File {
                at: NodeRef::Id(NodeId(999)),
                name: text("f"),
            },
            "r",
        ),
        nf(What::Node),
    );
    // D.8.24's row: `not_found` before `invalid(EmptyText)`, for an entry and for a file.
    refused(
        &mut w,
        &c,
        s,
        hide(
            HideArg::Entry {
                eid: Eid(9_999),
                node: n,
            },
            "",
        ),
        nf(What::Entry),
    );
    refused(
        &mut w,
        &c,
        s,
        hide(
            HideArg::File {
                at: NodeRef::Id(n),
                name: text("f"),
            },
            "",
        ),
        nf(What::Node),
    );
    w.emit(Ev::Appended {
        eid: w.s.counters.next_eid,
        node: n,
        seq: Seq(w.s.nodes[&n].seq.0 + 1),
        by: crate::state::Author::Session(s),
        body: EntryBody::Object,
        text: crate::event::EntryText::default(),
    });
    refused(
        &mut w,
        &c,
        s,
        hide(entry.clone(), ""),
        inv(Why::EmptyText(Field::Reason)),
    );
    let d = ok(w.call(&c, s, hide(entry, "secret")));
    assert_eq!(d.reply, Reply::Hidden);
}

// ------------------------------------------------------------------------------------------------
// hold (D.8.10, D.8.5)

/// Appends an entry by its event alone.
pub(crate) fn entry(w: &mut World, node: NodeId, by: crate::state::Author, body: EntryBody) -> Eid {
    let eid = w.s.counters.next_eid;
    let seq = Seq(w.s.nodes[&node].seq.0 + 1);
    w.emit(Ev::Appended {
        eid,
        node,
        seq,
        by,
        body,
        text: crate::event::EntryText::default(),
    });
    eid
}

#[test]
fn take_refusals_in_order() {
    let mut w = World::new(
        Params {
            hold_per_session: 1,
            ..Params::default()
        },
        T0,
    );
    let c = class("worker");
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    refused(&mut w, &c, s1, take(NodeId(999)), nf(What::Node));
    let (a, ta) = created(&ok(w.call(&c, s1, create("/a"))));
    // The session's own hold is returned.
    let d = ok(w.call(&c, s1, take(a)));
    assert!(matches!(d.reply, Reply::Taken { existing: true, token, .. } if Some(token) == ta));
    // full: capacity 1, held.
    refused(
        &mut w,
        &c,
        s2,
        take(a),
        Error::of(Hint::Full {
            node: a,
            capacity: 1,
            live: 1,
        }),
    );
    // holds_limit.
    let (b, _) = created(&ok(w.call(
        &c,
        s2,
        create_with("/b", |x| x.take = Some(false)),
    )));
    refused(
        &mut w,
        &c,
        s1,
        take(b),
        Error::of(Hint::HoldsLimit {
            holds: vec![ta.unwrap()],
        }),
    );
    // paused.
    ok(w.call(
        &c,
        s2,
        Cmd::Control(ControlCmd::Pause {
            scope: NodeRef::Id(b),
            reason: text("wait"),
        }),
    ));
    let until = w.s.pauses[&b].until;
    refused(
        &mut w,
        &c,
        s2,
        take(b),
        Error::of(Hint::Paused { scope: b, until }),
    );
    // closed, unflagged; and closed and merged.
    let (z, _) = created(&ok(w.call(
        &c,
        s2,
        create_with("/z", |x| x.take = Some(false)),
    )));
    ok(w.call(&c, s2, close(z, Outcome::Done)));
    refused(
        &mut w,
        &c,
        s2,
        take(z),
        Error::of(Hint::Closed {
            node: z,
            merged_into: None,
        }),
    );
    // A reserved node has capacity 0: never free.
    refused(
        &mut w,
        &c,
        s2,
        take(ROOT),
        Error::of(Hint::Full {
            node: ROOT,
            capacity: 0,
            live: 0,
        }),
    );
    assert!(!crate::helpers::holds::can_take(&w.s, s2, ROOT, w.clock));
    assert!(!crate::helpers::holds::can_take(&w.s, s1, b, w.clock));
}

/// `next` and `hello.offers` never offer a node to take that the session already holds (A-115):
/// a node of capacity 2 held once by the session is offered to another session only; a status
/// item for a waiting node the session holds is still offered.
#[test]
fn next_skips_the_callers_own_holds() {
    let mut w = world();
    let c = class("worker");
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    let (n, _) = created(&ok(w.call(
        &c,
        s1,
        create_with("/n", |a| {
            a.capacity = Some(2);
            a.tags = [label("offer")].into();
        }),
    )));
    let offers = |w: &World, s: SessionId| {
        query::hello(&w.s, s, w.clock)
            .offers
            .into_iter()
            .map(|r| r.cand)
            .collect::<Vec<_>>()
    };
    assert!(offers(&w, s1).is_empty());
    assert_eq!(offers(&w, s2), vec![Cand::Node(n)]);
    let (r, why) = query::ranked(
        &w.s,
        s1,
        w.clock,
        &NextQuery::default(),
        5,
        &mut query::NoMeter,
    );
    assert!(r.is_empty());
    assert_eq!(why.own, 1);
    let d = ok(w.call(&c, s1, next()));
    assert!(matches!(d.reply, Reply::Next(NextReply::Empty { .. })));
    let d = ok(w.call(&c, s2, next()));
    assert!(matches!(d.reply, Reply::Next(NextReply::Hold { node, .. }) if node == n));
    // A status item for a waiting node the session holds is still offered.
    let (m, tm) = created(&ok(w.call(
        &c,
        s1,
        create_with("/conv", |a| {
            a.keys = vec![Key {
                key: KeyStr::new("chat:x/1").unwrap(),
                nv: NormVer(1),
            }];
        }),
    )));
    let tm = tm.unwrap();
    let said = entry(
        &mut w,
        m,
        crate::state::Author::Session(s1),
        EntryBody::Said {
            conv: KeyStr::new("chat:x/1").unwrap(),
            kind: SaidKind::Ack,
            corrects: None,
            voice: false,
        },
    );
    let follow = w.clock.plus(Dur(60_000));
    w.emit(Ev::CommitmentOpened {
        c: said,
        node: m,
        conv: KeyStr::new("chat:x/1").unwrap(),
        followup_at: follow,
    });
    w.emit(Ev::SaidConfirmed {
        said,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    w.emit(Ev::WaitSet {
        node: m,
        wait: Some(crate::state::WaitOn::Time(w.clock.plus(Dur(3_600_000)))),
    });
    w.advance(Dur(60_000));
    // Attended: no open_commitment yet. Released with the wait kept, the commitment's follow-up
    // has passed: a status item, offered to the session that held it too.
    let mut rel = release(tm, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut rel {
        *waits_for = Some(WaitArg::Time(When::At(w.clock.plus(Dur(3_000_000)))));
    }
    ok(w.call(&c, s1, rel));
    assert!(w.s.ix.next.of.contains_key(&Cand::Status(m)));
    let (r, _) = query::ranked(
        &w.s,
        s1,
        w.clock,
        &NextQuery::default(),
        5,
        &mut query::NoMeter,
    );
    assert!(r.iter().any(|x| x.cand == Cand::Status(m)));
}

#[test]
fn next_refusals_and_its_items() {
    let mut w = World::new(
        Params {
            hold_per_session: 1,
            ..Params::default()
        },
        T0,
    );
    let c = class("worker");
    let s = w.hello(&c);
    let q = |under: NodeRef| {
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery {
                under: Some(under),
                kind: None,
                tags: BTreeSet::new(),
            },
            lease: None,
        })
    };
    refused(&mut w, &c, s, q(at("/nope")), nf(What::Node));
    // Empty with no candidate at all: Nothing.
    let d = ok(w.call(&c, s, next()));
    let Reply::Next(NextReply::Empty { why, .. }) = d.reply else {
        panic!()
    };
    assert_eq!(why.candidates, 0);
    assert!(w.last_events.iter().any(|e| matches!(
        e.ev,
        Ev::NextCalled {
            outcome: crate::event::NextOutcome::Nothing,
            open: true,
            ..
        }
    )));
    // A routing item: an unrouted user entry, offered only without a query.
    let e = entry(
        &mut w,
        USER,
        crate::state::Author::Session(s),
        EntryBody::FromUser {
            principal: USER_PRINCIPAL,
        },
    );
    let d = ok(w.call(&c, s, q(at("/"))));
    assert!(matches!(d.reply, Reply::Next(NextReply::Empty { .. })));
    let d = ok(w.call(&c, s, next()));
    assert!(matches!(d.reply, Reply::Next(NextReply::Route { entry, .. }) if entry == e));
    // holds_limit.
    let (_, t) = created(&ok(w.call(&c, s, create("/a"))));
    refused(
        &mut w,
        &c,
        s,
        next(),
        Error::of(Hint::HoldsLimit {
            holds: vec![t.unwrap()],
        }),
    );
}

#[test]
fn checkpoint_renew_release_and_check() {
    let (mut w, c, s, n, t) = held();
    let cp = |intent: Option<&str>, intent_done: Option<&str>, next_step: &str| {
        Cmd::Hold(HoldCmd::Checkpoint {
            token: t,
            done: text("d"),
            next_step: text(next_step),
            intent: intent.map(text),
            intent_done: intent_done.map(text),
            files: vec![text("f.txt")],
        })
    };
    refused(
        &mut w,
        &c,
        s,
        Cmd::Hold(HoldCmd::Checkpoint {
            token: Token {
                node: NodeId(999),
                fence: Fence(1),
            },
            done: text("d"),
            next_step: text("n"),
            intent: None,
            intent_done: None,
            files: Vec::new(),
        }),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        cp(None, None, ""),
        inv(Why::EmptyText(Field::NextStep)),
    );
    refused(&mut w, &c, s, cp(None, Some("x"), "n"), inv(Why::NoIntent));
    let d = ok(w.call(&c, s, cp(Some("post"), None, "n")));
    let Reply::Checkpointed {
        intent: Some(i), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(w.s.nodes[&n].open_intent.map(|x| x.eid), Some(i));
    refused(
        &mut w,
        &c,
        s,
        cp(Some("again"), None, "n"),
        inv(Why::IntentOpen),
    );
    // One checkpoint closes the open intent and opens a new one.
    let d = ok(w.call(&c, s, cp(Some("again"), Some("posted"), "n")));
    let Reply::Checkpointed {
        intent: Some(j), ..
    } = d.reply
    else {
        panic!()
    };
    assert_ne!(i, j);
    // After a checkpoint the lease extends to now + L, within the ceiling.
    let h = &w.s.holds[&t];
    assert_eq!(h.lease_until, w.clock.plus(h.l));
    // renew: renew.max times, then limit(Renew).
    let renew = || {
        Cmd::Hold(HoldCmd::Renew {
            token: t,
            why: text("still"),
        })
    };
    let max = w.s.params.renew_max;
    for k in 0..max {
        let d = ok(w.call(&c, s, renew()));
        assert!(
            matches!(d.reply, Reply::Renewed { renews_left, .. } if renews_left == max - k - 1)
        );
    }
    refused(&mut w, &c, s, renew(), Error::limit(LimitKind::Renew));
    // check: read-only.
    let d = ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Check {
            target: NodeRef::Id(n),
            token: Some(t),
            intent_done: None,
        }),
    ));
    assert_eq!(
        d.reply,
        Reply::HoldCheck {
            node: n,
            token_current: Some(true),
            entry: None,
            intent_done: None,
        }
    );
    // release: the wait's references, and cycles.
    let rel = |waits_for: Option<WaitArg>| {
        Cmd::Hold(HoldCmd::Release {
            token: t,
            done: text("d"),
            next_step: text("n"),
            reason: ReleaseReason::Cannot,
            waits_for,
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        })
    };
    refused(
        &mut w,
        &c,
        s,
        rel(Some(WaitArg::Question(Eid(999)))),
        nf(What::Question),
    );
    refused(
        &mut w,
        &c,
        s,
        rel(Some(WaitArg::Node(at("/nope")))),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        rel(Some(WaitArg::Node(NodeRef::Id(n)))),
        Error::of(Hint::Cycle { path: vec![n, n] }),
    );
    let (m, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/m", |a| {
            a.depends_on = vec![NodeRef::Id(n)];
            a.take = Some(false);
        }),
    )));
    refused(
        &mut w,
        &c,
        s,
        rel(Some(WaitArg::Node(NodeRef::Id(m)))),
        Error::of(Hint::Cycle {
            path: vec![n, m, n],
        }),
    );
    // A wait records `blocked` whatever the reason (A-98): no refusal.
    let d = ok(w.call(
        &c,
        s,
        rel(Some(WaitArg::External {
            what: text("ci"),
            recheck_after: When::In(Dur(600_000)),
        })),
    ));
    assert!(matches!(d.reply, Reply::Released { .. }));
    assert!(w.s.refusals.is_empty());
    assert!(w.last_events.iter().any(|e| matches!(
        e.ev,
        Ev::Appended {
            body: EntryBody::Checkpoint {
                release: Some(ReleaseReason::Blocked),
                ..
            },
            ..
        }
    )));
    assert!(w.s.holds.is_empty());
    assert!(matches!(
        w.s.nodes[&n].wait.as_ref().map(|x| x.on),
        Some(crate::state::WaitOn::External(_))
    ));
    let d = ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Check {
            target: NodeRef::Id(n),
            token: Some(t),
            intent_done: None,
        }),
    ));
    assert_eq!(
        d.reply,
        Reply::HoldCheck {
            node: n,
            token_current: Some(false),
            entry: None,
            intent_done: None,
        }
    );
}

/// A `cannot` release records the session's refusal for its class, backing the class off the
/// node for `refusal.backoff` (CS-7).
#[test]
fn a_cannot_release_backs_the_class_off() {
    let (mut w, c, s, n, t) = held();
    ok(w.call(&c, s, release(t, ReleaseReason::Cannot)));
    let r = &w.s.refusals[&(n, c.clone())];
    assert_eq!(r.by.len(), 1);
    assert!(crate::refusals::backed_off(&w.s, n, &c, w.clock));
    let s2 = w.hello(&c);
    let (got, why) = query::ranked(
        &w.s,
        s2,
        w.clock,
        &NextQuery::default(),
        3,
        &mut query::NoMeter,
    );
    assert!(got.is_empty());
    assert_eq!(why.refused, 1);
    w.advance(w.s.params.refusal_backoff);
    let (got, _) = query::ranked(
        &w.s,
        s2,
        w.clock,
        &NextQuery::default(),
        3,
        &mut query::NoMeter,
    );
    assert_eq!(got.first().map(|r| r.cand), Some(Cand::Node(n)));
}

// ------------------------------------------------------------------------------------------------
// control (D.8.21) and views (D.8.25)

#[test]
fn pause_and_unpause() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let pause = |scope: NodeRef, reason: &str| {
        Cmd::Control(ControlCmd::Pause {
            scope,
            reason: text(reason),
        })
    };
    let unpause = |scope: NodeRef| Cmd::Control(ControlCmd::Unpause { scope });
    refused(&mut w, &c, s, pause(at("/nope"), "r"), nf(What::Node));
    refused(
        &mut w,
        &c,
        s,
        pause(at("/"), ""),
        inv(Why::EmptyText(Field::Reason)),
    );
    refused(&mut w, &c, s, unpause(at("/")), nf(What::Pause));
    let d = ok(w.call(&c, s, pause(at("/"), "stop")));
    assert!(matches!(d.reply, Reply::Paused { renewed: false, .. }));
    let line = query::status_line(&w.s, s, w.clock);
    assert!(line.root_pause.is_some());
    let d = ok(w.call(&c, s, pause(at("/"), "still")));
    assert!(matches!(d.reply, Reply::Paused { renewed: true, .. }));
    ok(w.call(&c, s, unpause(at("/"))));
    assert!(w.s.pauses.is_empty());
    // Expiry is the Pause deadline.
    ok(w.call(&c, s, pause(at("/"), "stop")));
    let evs = w.advance(w.s.params.pause_ttl);
    assert!(evs.iter().any(|e| matches!(e.ev, Ev::PauseExpired { .. })));
    assert!(w.s.pauses.is_empty());
}

#[test]
fn views_record_what_they_showed() {
    let (mut w, c, s, n, _) = held();
    let view = |targets: Vec<NodeRef>| {
        Cmd::View(ViewCmd::Node {
            targets,
            view: None,
            page: None,
        })
    };
    refused(
        &mut w,
        &c,
        s,
        view(vec![NodeRef::Id(n), at("/x")]),
        nf(What::Node),
    );
    // A node with entries: the first view shows them, a re-read appends nothing (CS-14).
    entry(
        &mut w,
        n,
        crate::state::Author::Session(s),
        EntryBody::Object,
    );
    w.advance(Dur(1_000));
    let d = ok(w.call(&c, s, view(vec![NodeRef::Id(n)])));
    assert!(matches!(
        d.reply,
        Reply::ViewNodes {
            view: ViewMode::Brief,
            page: 0,
            ..
        }
    ));
    assert!(
        w.last_events
            .iter()
            .any(|e| matches!(e.ev, Ev::Shown { .. }))
    );
    ok(w.call(&c, s, view(vec![NodeRef::Id(n)])));
    assert!(w.last_events.is_empty());
    // feed: the page, and Shown up to its end.
    let feed = Cmd::View(ViewCmd::Feed {
        at: NodeRef::Id(n),
        since: Some(Seq(0)),
        types: BTreeSet::new(),
        limit: Some(1),
    });
    let d = ok(w.call(&c, s, feed));
    assert!(matches!(
        d.reply,
        Reply::ViewFeed {
            upto: Seq(1),
            limit: 1,
            ..
        }
    ));
    refused(
        &mut w,
        &c,
        s,
        Cmd::View(ViewCmd::History {
            at: at("/x"),
            before: None,
            limit: None,
        }),
        nf(What::Node),
    );
    let d = ok(w.call(
        &c,
        s,
        Cmd::View(ViewCmd::History {
            at: NodeRef::Id(n),
            before: None,
            limit: Some(1_000),
        }),
    ));
    assert!(matches!(d.reply, Reply::ViewHistory { limit: 200, .. }));
    let d = ok(w.call(
        &c,
        s,
        Cmd::View(ViewCmd::List {
            under: None,
            kind: None,
            tags: BTreeSet::new(),
            status: None,
            fields: BTreeMap::new(),
            flags: FlagSet::EMPTY,
            closed: None,
            limit: None,
        }),
    ));
    let Reply::ViewList { nodes, total } = d.reply else {
        panic!()
    };
    assert!(nodes.contains(&n));
    assert_eq!(
        total,
        crate::reply::Total::Exact(u32::try_from(nodes.len()).unwrap())
    );
    let d = ok(w.call(
        &c,
        s,
        Cmd::View(ViewCmd::Requests {
            status: None,
            requester: None,
        }),
    ));
    assert!(matches!(
        d.reply,
        Reply::ViewRequests {
            scope: crate::reply::BoardScope::All,
            ..
        }
    ));
    for cmd in [
        Cmd::View(ViewCmd::UserLog {
            since: None,
            limit: None,
        }),
        Cmd::View(ViewCmd::Constitution),
        Cmd::Control(ControlCmd::Health),
        Cmd::Control(ControlCmd::Constitution),
        Cmd::Control(ControlCmd::Export {
            after: None,
            limit: None,
        }),
    ] {
        ok(w.call(&c, s, cmd));
    }
}

/// `query::precheck` agrees with `apply`'s session and target errors (D.8.25).
#[test]
fn precheck_agrees_with_apply() {
    let (w, c, s, n, t) = held();
    let cmds = vec![
        take(NodeId(999)),
        take(n),
        checkpoint(Token {
            node: NodeId(999),
            fence: Fence(1),
        }),
        checkpoint(t),
        close(NodeId(999), Outcome::Done),
        Cmd::Control(ControlCmd::Pause {
            scope: at("/nope"),
            reason: text("r"),
        }),
        create("/q"),
        Cmd::View(ViewCmd::Node {
            targets: vec![NodeRef::Id(n), at("/nope")],
            view: None,
            page: None,
        }),
        Cmd::View(ViewCmd::Feed {
            at: at("/nope"),
            since: None,
            types: BTreeSet::new(),
            limit: None,
        }),
        Cmd::View(ViewCmd::History {
            at: NodeRef::Id(n),
            before: None,
            limit: None,
        }),
        update(NodeId(999), Version(1), |_| {}),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::File {
                at: at("/nope"),
                name: text("f"),
            },
            reason: text("r"),
        }),
        Cmd::Hold(HoldCmd::Renew {
            token: Token {
                node: NodeId(999),
                fence: Fence(1),
            },
            why: text("w"),
        }),
        release(t, ReleaseReason::Handoff),
        Cmd::Hold(HoldCmd::Check {
            target: at("/nope"),
            token: None,
            intent_done: None,
        }),
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery {
                under: Some(at("/nope")),
                kind: None,
                tags: BTreeSet::new(),
            },
            lease: None,
        }),
        Cmd::Control(ControlCmd::Unpause { scope: at("/") }),
    ];
    for cmd in cmds {
        for (cls, sc) in [
            (c.clone(), SessionCtx::Existing(s)),
            (c.clone(), SessionCtx::Existing(SessionId(77))),
            (class("other"), SessionCtx::Existing(s)),
        ] {
            let ctx = w.ctx(&cls, sc, None);
            let pre = query::precheck(&w.s, &ctx, &cmd);
            let (_, out) = crate::apply::decide(&w.s, &ctx, &cmd);
            match &pre {
                Some(e) => assert_eq!(out.as_ref().err(), Some(e), "{cmd:?}"),
                None => assert!(
                    !matches!(
                        out,
                        Err(Error {
                            code: ErrorCode::NotFound,
                            hint: Hint::NotFound(What::Session | What::Node)
                        })
                    ) || matches!(&cmd, Cmd::Hold(HoldCmd::Checkpoint { .. })),
                    "{cmd:?}: {out:?}"
                ),
            }
        }
    }
}

/// A person or team created with a hold and released gets no `offer` tag and is not a candidate,
/// nor is one tagged `offer` by hand; a flag makes it one (A-113).
#[test]
fn persons_and_teams_are_not_offered() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (bob, t) = created(&ok(w.call(&c, s, create("/@people/bob"))));
    ok(w.call(&c, s, release(t.unwrap(), ReleaseReason::Handoff)));
    assert!(!w.s.nodes[&bob].tags.contains(&label("offer")));
    let (team, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/@teams/core", |a| {
            a.tags = [label("offer")].into();
            a.take = Some(false);
        }),
    )));
    for n in [bob, team] {
        assert!(!w.s.ix.next.of.contains_key(&Cand::Node(n)));
    }
    entry(
        &mut w,
        team,
        crate::state::Author::Session(s),
        EntryBody::Object,
    );
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(team)));
}

/// `apply_with(.., TieBreak::Id)` breaks the ring component by id (D.6.1, D.10.6); `apply` uses
/// the ring (§10.4).
#[cfg(feature = "conformance")]
#[test]
fn the_tie_break_by_id() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let mut ids = Vec::new();
    for name in ["/a", "/b", "/c", "/d", "/e"] {
        let (n, _) = created(&ok(w.call(
            &c,
            s,
            create_with(name, |a| {
                a.tags = [label("offer")].into();
                a.take = Some(false);
            }),
        )));
        ids.push(n);
    }
    let s2 = w.hello(&c);
    let ctx = w.ctx(&c, SessionCtx::Existing(s2), None);
    let mut by_id = w.s.clone();
    let (_, out) = crate::apply::apply_with(&mut by_id, &ctx, &next(), crate::reply::TieBreak::Id);
    let Reply::Next(NextReply::Hold { node, .. }) = ok(out).reply else {
        panic!()
    };
    assert_eq!(node, ids[0]);
    let mut ring = w.s.clone();
    let (_, out) = crate::apply::apply_with(&mut ring, &ctx, &next(), crate::reply::TieBreak::Fnv);
    let Reply::Next(NextReply::Hold {
        node: ring_node, ..
    }) = ok(out).reply
    else {
        panic!()
    };
    let want = crate::spec::ranked(&w.s, s2, w.clock, &NextQuery::default())[0];
    assert_eq!(Cand::Node(ring_node), want);
}

/// `holds_limit` comes before `full` (D.8.10): a session at its limit taking a full node.
#[test]
fn holds_limit_before_full() {
    let mut w = World::new(
        Params {
            hold_per_session: 1,
            ..Params::default()
        },
        T0,
    );
    let c = class("worker");
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    let (a, _) = created(&ok(w.call(&c, s1, create("/a"))));
    let (_, tb) = created(&ok(w.call(&c, s2, create("/b"))));
    refused(
        &mut w,
        &c,
        s2,
        take(a),
        Error::of(Hint::HoldsLimit {
            holds: vec![tb.unwrap()],
        }),
    );
}

/// A create at exactly `tree.max_depth` succeeds; one level deeper is `limit(Depth)` (L1).
#[test]
fn the_depth_limit_is_inclusive() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let max = usize::try_from(w.s.params.tree_max_depth).unwrap();
    let at_max: String = (0..max).map(|i| alloc::format!("/d{i}")).collect();
    ok(w.call(&c, s, create_with(&at_max, |a| a.take = Some(false))));
    let deeper = alloc::format!("{at_max}/x");
    refused(
        &mut w,
        &c,
        s,
        create(&deeper),
        Error::limit(LimitKind::Depth),
    );
}

/// A feed page ends at the node's last seq (D.8.25): `upto = min(after + limit, n.seq)`.
#[test]
fn a_feed_page_ends_at_the_last_seq() {
    let (mut w, c, s, n, _) = held();
    entry(
        &mut w,
        n,
        crate::state::Author::Session(s),
        EntryBody::Object,
    );
    let seq = w.s.nodes[&n].seq;
    let d = ok(w.call(
        &c,
        s,
        Cmd::View(ViewCmd::Feed {
            at: NodeRef::Id(n),
            since: None,
            types: BTreeSet::new(),
            limit: Some(10),
        }),
    ));
    assert!(
        matches!(d.reply, Reply::ViewFeed { after: Seq(0), upto, limit: 10, .. } if upto == seq)
    );
    assert_eq!(w.s.sessions[&s].shown[&n], seq);
}

/// A close warns about what it leaves open: unsettled requests (the flag stays, §5).
#[test]
fn close_warns_about_unsettled_requests() {
    let (mut w, c, s, n, _) = held();
    let r = entry(
        &mut w,
        n,
        crate::state::Author::Session(s),
        EntryBody::Request {
            origin: Key {
                key: KeyStr::new("chat:x/1/1").unwrap(),
                nv: NormVer(1),
            },
            conv: KeyStr::new("chat:x/1").unwrap(),
            from_user: None,
            requester: Requester::External,
        },
    );
    let d = ok(w.call(&c, s, close(n, Outcome::Done)));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.detail == WarningDetail::UnsettledRequests { requests: vec![r] })
    );
    assert!(w.s.ix.node[&n].flags.contains(Flag::UnansweredRequest));
}

/// Within one patch, removals apply first, then additions (CS-34): a label both removed and added
/// stays.
#[test]
fn removals_apply_before_additions() {
    let (mut w, c, s, n, _) = held();
    let v = w.s.nodes[&n].version;
    ok(w.call(
        &c,
        s,
        update(n, v, |p| {
            p.tags_add = [label("x")].into();
            p.tags_remove = [label("x")].into();
        }),
    ));
    assert!(w.s.nodes[&n].tags.contains(&label("x")));
}

/// The status line names ROOT's pause apart, and lists the other pauses over the session's holds
/// once each (CS-9).
#[test]
fn the_status_line_names_pauses() {
    let (mut w, c, s, n, _) = held();
    let pause = |scope: NodeRef| {
        Cmd::Control(ControlCmd::Pause {
            scope,
            reason: text("r"),
        })
    };
    ok(w.call(&c, s, pause(NodeRef::Id(ROOT))));
    ok(w.call(&c, s, pause(NodeRef::Id(n))));
    let line = query::status_line(&w.s, s, w.clock);
    assert_eq!(line.root_pause.map(|p| p.scope), Some(ROOT));
    assert_eq!(
        line.hold_pauses.iter().map(|p| p.scope).collect::<Vec<_>>(),
        vec![n]
    );
}

/// A pending `holder:` note binds to the next taker, but never to its sender (1.4, R12).
#[test]
fn holder_notes_skip_their_sender() {
    let mut w = world();
    let c = class("worker");
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    let (n, _) = created(&ok(w.call(
        &c,
        s1,
        create_with("/n", |a| a.take = Some(false)),
    )));
    let until = w.clock.plus(Dur(3_600_000));
    let note = entry(
        &mut w,
        n,
        crate::state::Author::Session(s1),
        EntryBody::Note {
            to: Some(Addressee::Holder(n)),
            until: Some(until),
            recipients: BTreeSet::new(),
            pending_holder: true,
            expects: None,
            reply_to: None,
        },
    );
    let (_, t) = match ok(w.call(&c, s1, take(n))).reply {
        Reply::Taken { token, .. } => ((), token),
        other => panic!("{other:?}"),
    };
    assert!(
        !w.last_events
            .iter()
            .any(|e| matches!(e.ev, Ev::NoteBound { .. }))
    );
    assert!(w.s.notes[&note].pending_holder);
    ok(w.call(&c, s1, release(t, ReleaseReason::Handoff)));
    ok(w.call(&c, s2, take(n)));
    assert!(w.last_events.iter().any(|e| matches!(
        e.ev,
        Ev::NoteBound { note: x, session } if x == note && session == s2
    )));
}

/// A close resolves the open intent only when it is the closing session's (§5): another
/// session's intent stays, and flags the closed node.
#[test]
fn a_close_resolves_only_its_own_intent() {
    let mut w = world();
    let c = class("worker");
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    for (closer, own) in [(s1, true), (s2, false)] {
        let name = if own { "/own" } else { "/other" };
        let (n, t) = created(&ok(w.call(&c, s1, create(name))));
        let t = t.unwrap();
        ok(w.call(
            &c,
            s1,
            Cmd::Hold(HoldCmd::Checkpoint {
                token: t,
                done: text("d"),
                next_step: text("n"),
                intent: Some(text("post")),
                intent_done: None,
                files: Vec::new(),
            }),
        ));
        ok(w.call(&c, s1, release(t, ReleaseReason::Handoff)));
        ok(w.call(&c, closer, close(n, Outcome::Done)));
        assert_eq!(w.s.nodes[&n].open_intent.is_none(), own, "{name}");
        assert_eq!(w.s.ix.node[&n].flags.contains(Flag::OpenIntent), !own);
    }
}

/// The validation branches of D.8.24 the tests above do not reach: one per branch, each with its
/// exact error and nothing applied.
#[test]
fn the_remaining_validation_branches() {
    let (mut w, c, s, n, t) = held();
    let nope = || at("/nope");
    // view feed and view list: the node.
    refused(
        &mut w,
        &c,
        s,
        Cmd::View(ViewCmd::Feed {
            at: nope(),
            since: None,
            types: BTreeSet::new(),
            limit: None,
        }),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        Cmd::View(ViewCmd::List {
            under: Some(nope()),
            kind: None,
            tags: BTreeSet::new(),
            status: None,
            fields: BTreeMap::new(),
            flags: FlagSet::EMPTY,
            closed: None,
            limit: None,
        }),
        nf(What::Node),
    );
    // node update: the digest's length, the time zone, the owners, and the depth of a schedule.
    let v = w.s.nodes[&n].version;
    let long = Text("d".repeat(usize::try_from(w.s.params.digest_max).unwrap() + 1));
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| p.digest = Some(long)),
        inv(Why::DigestTooLong),
    );
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| {
            let mut r = repeat_arg("0 7 * * *", Dur(60_000));
            r.tz.transitions = vec![
                forum_cron::Transition { at: 9, offset: 0 },
                forum_cron::Transition { at: 3, offset: 0 },
            ];
            p.repeat = Change::Set(r);
        }),
        inv(Why::BadTimeZone),
    );
    refused(
        &mut w,
        &c,
        s,
        update(n, v, |p| p.owners = Some(vec![nope()])),
        nf(What::Node),
    );
    let max = usize::try_from(w.s.params.tree_max_depth).unwrap();
    let deepest: String = (0..max).map(|i| alloc::format!("/z{i}")).collect();
    let (deep, _) = created(&ok(w.call(
        &c,
        s,
        create_with(&deepest, |a| a.take = Some(false)),
    )));
    let dv = w.s.nodes[&deep].version;
    refused(
        &mut w,
        &c,
        s,
        update(deep, dv, |p| {
            p.repeat = Change::Set(repeat_arg("0 7 * * *", Dur(60_000)))
        }),
        Error::limit(LimitKind::Depth),
    );
    // A repeat that changes the schedule makes the node a series with its first occurrence
    // (WP-D6, D.8.18).
    ok(w.call(
        &c,
        s,
        update(n, v, |p| {
            p.repeat = Change::Set(repeat_arg("0 7 * * *", Dur(60_000)))
        }),
    ));
    assert!(matches!(w.s.nodes[&n].role, crate::state::Role::Series(_)));
    assert_eq!(w.s.ix.series_open.get(&n).map(BTreeSet::len), Some(1));
    // node update: `repeat` on a signal node.
    let key = crate::state::SignalKey {
        kind: SignalKind::Duplicate,
        a: n,
        b: deep,
    };
    let sig = w.s.counters.next_node;
    let mut init = crate::event::NodeInit {
        title: text("sig"),
        body: Text::default(),
        fields: BTreeMap::new(),
        status: Text::default(),
        digest: Text::default(),
        agreements: Text::default(),
        kind: None,
        tags: BTreeSet::new(),
        capacity: 1,
        priority: 0,
        est: None,
        needs: BTreeSet::new(),
        due: None,
        depends_on: BTreeSet::new(),
        owners: BTreeSet::new(),
        keys: BTreeMap::new(),
        links: BTreeSet::new(),
        role: crate::state::Role::Signal(key),
    };
    init.tags.insert(label("offer"));
    w.emit(Ev::NodeCreated {
        id: sig,
        parent: SIGNALS,
        name: Name::new("s1").unwrap(),
        origin: crate::state::Origin::Signal,
        init,
    });
    w.emit(Ev::SignalOpened { key, node: sig });
    let sv = w.s.nodes[&sig].version;
    refused(
        &mut w,
        &c,
        s,
        update(sig, sv, |p| {
            p.repeat = Change::Set(repeat_arg("0 7 * * *", Dur(60_000)))
        }),
        inv(Why::RoleConflict),
    );
    // node hide: an entry whose node the shell did not find (its `UNKNOWN_NODE`) is an entry the
    // core cannot find either: `not_found(Entry)` (D.14.6).
    let e = entry(
        &mut w,
        n,
        crate::state::Author::Session(s),
        EntryBody::Object,
    );
    refused(
        &mut w,
        &c,
        s,
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry {
                eid: e,
                node: NodeId(u64::MAX),
            },
            reason: text("r"),
        }),
        nf(What::Entry),
    );
    // hold renew, release, and check: the token's node, and the release's next step.
    let ghost = Token {
        node: NodeId(999),
        fence: Fence(1),
    };
    refused(
        &mut w,
        &c,
        s,
        Cmd::Hold(HoldCmd::Renew {
            token: ghost,
            why: text("w"),
        }),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        release(ghost, ReleaseReason::Handoff),
        nf(What::Node),
    );
    let mut rel = release(t, ReleaseReason::Handoff);
    if let Cmd::Hold(HoldCmd::Release { next_step, .. }) = &mut rel {
        *next_step = text("");
    }
    refused(&mut w, &c, s, rel, inv(Why::EmptyText(Field::NextStep)));
    refused(
        &mut w,
        &c,
        s,
        Cmd::Hold(HoldCmd::Check {
            target: nope(),
            token: None,
            intent_done: None,
        }),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        Cmd::Control(ControlCmd::Unpause { scope: nope() }),
        nf(What::Node),
    );
    // hold take of a merged node: `closed`, naming where it lives on (A-94).
    let (m, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/m", |a| a.take = Some(false)),
    )));
    w.emit(Ev::NodeClosed {
        node: m,
        outcome: Outcome::Superseded,
        merged_into: Some(n),
        by: None,
    });
    entry(
        &mut w,
        m,
        crate::state::Author::Session(s),
        EntryBody::Object,
    );
    refused(
        &mut w,
        &c,
        s,
        take(m),
        Error::of(Hint::Closed {
            node: m,
            merged_into: Some(n),
        }),
    );
}

/// 1.6 (§10.4): an empty `next` names, for each exclusion, its first node and the value that
/// excluded it: the needs the caller's caps lack, an `est` over its horizon, its class's refusal,
/// an open dependency, and a wait. In the lab, agents found what their own horizon excluded by
/// trial.
#[test]
fn an_empty_next_names_the_first_node_of_each_exclusion() {
    let mut w = world();
    let c = class("worker");
    let maker = w.hello(&c);
    let (a, _) = created(&ok(w.call(
        &c,
        maker,
        create_with("/a", |a| {
            a.take = Some(false);
            a.tags = [label("offer")].into();
            a.needs = Some([label("support")].into());
        }),
    )));
    let (b, _) = created(&ok(w.call(
        &c,
        maker,
        create_with("/b", |a| {
            a.take = Some(false);
            a.tags = [label("offer")].into();
            a.est = Some(60);
        }),
    )));
    // /x stays held by its maker, and /d depends on it.
    let (x, _) = created(&ok(w.call(&c, maker, create("/x"))));
    let (d, _) = created(&ok(w.call(
        &c,
        maker,
        create_with("/d", |a| {
            a.take = Some(false);
            a.tags = [label("offer")].into();
            a.depends_on = vec![NodeRef::Id(x)];
        }),
    )));
    // /wait is released with a wait an hour away.
    let (wn, t) = created(&ok(w.call(&c, maker, create("/wait"))));
    let mut rel = release(t.unwrap(), ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut rel {
        *waits_for = Some(WaitArg::Time(When::At(w.clock.plus(Dur(3_600_000)))));
    }
    ok(w.call(&c, maker, rel));
    // The caller, with a horizon of 30 minutes, refuses /r.
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let caller = ok(w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: Some(30),
            principal: None,
            from_user: None,
        }),
    ))
    .opened
    .unwrap();
    let (r, _) = created(&ok(w.call(
        &c,
        maker,
        create_with("/r", |a| {
            a.take = Some(false);
            a.tags = [label("offer")].into();
        }),
    )));
    let Reply::Taken { token, .. } = ok(w.call(&c, caller, take(r))).reply else {
        panic!("not taken")
    };
    ok(w.call(&c, caller, release(token, ReleaseReason::Cannot)));
    let found = ok(w.call(&c, caller, next()));
    let Reply::Next(NextReply::Empty { why, .. }) = found.reply else {
        panic!("not empty: {:?}", found.reply)
    };
    assert_eq!(why.first.needs, Some((a, [label("support")].into())));
    assert_eq!(why.first.horizon, Some((b, 60, 30)));
    assert_eq!(why.first.depends, Some((d, x)));
    assert_eq!(why.first.waiting, Some(wn));
    assert_eq!(why.first.refused, Some(r));
    // A `next` that finds work fills none of them.
    let o = class("other");
    let other = w.hello(&o);
    let found = ok(w.call(&o, other, next()));
    assert!(matches!(found.reply, Reply::Next(NextReply::Hold { .. })));
}

/// 1.6 (§10.1, §21): `take` holds a node that waits, and warns `waiting`, naming the wait and when
/// it was set; the wait stays. A take of a node whose wait has
/// resolved, or that has none, does not warn. In the lab, a take of a node that waited on another
/// session's action in flight said nothing, and the taker redid the work.
#[test]
fn take_of_a_waiting_node_warns_waiting() {
    let mut w = world();
    let c = class("worker");
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    let waiting = |d: &Done| {
        d.warnings
            .iter()
            .filter(|x| x.code == WarningCode::Waiting)
            .map(|x| (x.id, x.detail.clone()))
            .collect::<Vec<_>>()
    };
    let (n, t) = created(&ok(w.call(&c, s1, create("/deploy"))));
    let set_at = w.clock;
    let until = w.clock.plus(Dur(3_600_000));
    let mut rel = release(t.unwrap(), ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut rel {
        *waits_for = Some(WaitArg::External {
            what: text("the deploy's checks"),
            recheck_after: When::At(until),
        });
    }
    ok(w.call(&c, s1, rel));
    let release_entry = w.s.nodes[&n].last_checkpoint;
    assert!(release_entry.is_some());
    let d = ok(w.call(&c, s2, take(n)));
    assert!(matches!(
        d.reply,
        Reply::Taken {
            existing: false,
            ..
        }
    ));
    // 1.10: it names the release that set the wait, which another session wrote.
    assert_eq!(
        waiting(&d),
        vec![(
            Some(n),
            WarningDetail::Waiting {
                on: crate::state::WaitOn::External(until),
                set_at,
                release: release_entry,
            }
        )]
    );
    assert!(w.s.nodes[&n].wait.is_some(), "the wait stays");
    // Once the wait has resolved, a take does not warn.
    let Reply::Taken { token, .. } = d.reply else {
        panic!()
    };
    ok(w.call(&c, s2, release(token, ReleaseReason::Handoff)));
    let (m, t) = created(&ok(w.call(&c, s1, create("/later"))));
    let mut rel = release(t.unwrap(), ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut rel {
        *waits_for = Some(WaitArg::Time(When::At(w.clock.plus(Dur(60_000)))));
    }
    ok(w.call(&c, s1, rel));
    w.advance(Dur(120_000));
    let d = ok(w.call(&c, s2, take(m)));
    assert!(waiting(&d).is_empty());
}

/// 1.9 (§10.1, §20.5, §21): a `create`'s `needs`, or an `update`'s `needs_add`, that name a label
/// no session can meet now are applied, with `unmet_needs` naming those labels; a label is met by
/// the caller's caps, its token's default caps, or a session seen within `capacity.window`. A
/// `take` by a session whose caps lack the node's needs holds it, and warns `unmet_needs` with what
/// they lack. In the lab, needs that said what the work lacked left claims offered to nobody, and a
/// session with caps `mail` took Swift work.
#[test]
fn needs_nobody_can_meet_warn_unmet_needs() {
    let unmet = |d: &Done| {
        d.warnings
            .iter()
            .filter(|x| x.code == WarningCode::UnmetNeeds)
            .map(|x| (x.id, x.detail.clone()))
            .collect::<Vec<_>>()
    };
    let labels = |ls: &[&str]| ls.iter().map(|l| label(l)).collect::<BTreeSet<Label>>();
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    // No session has `exact-statement`: the create applies it, and warns.
    let d = ok(w.call(
        &c,
        s,
        create_with("/lemma", |a| {
            a.needs = Some(labels(&["exact-statement"]));
            a.take = Some(false);
        }),
    ));
    let (lemma, _) = created(&d);
    assert_eq!(w.s.nodes[&lemma].needs, labels(&["exact-statement"]));
    assert_eq!(
        unmet(&d),
        vec![(
            Some(lemma),
            WarningDetail::UnmetNeeds {
                needs: vec![label("exact-statement")],
                take: false,
            }
        )]
    );
    // A session that says hello with caps `lean`: `lean` is met, the other label is not.
    let lc = class("prover");
    let ctx = w.ctx(&lc, SessionCtx::New, None);
    let prover = ok(w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some(labels(&["lean"])),
            horizon: None,
            principal: None,
            from_user: None,
        }),
    ))
    .opened
    .unwrap();
    let d = ok(w.call(
        &c,
        s,
        create_with("/proof", |a| {
            a.needs = Some(labels(&["lean", "proof-artifact"]));
            a.take = Some(false);
        }),
    ));
    let (proof, _) = created(&d);
    assert_eq!(
        unmet(&d),
        vec![(
            Some(proof),
            WarningDetail::UnmetNeeds {
                needs: vec![label("proof-artifact")],
                take: false,
            }
        )]
    );
    // The caller's token's default caps meet a label too; a create with no needs takes its
    // parent's and warns nothing.
    let mut ctx = w.ctx(&c, SessionCtx::Existing(s), None);
    ctx.caps = Some(labels(&["sql"]));
    let d = ok(w.call_ctx(
        &ctx,
        &create_with("/report", |a| {
            a.needs = Some(labels(&["sql"]));
            a.take = Some(false);
        }),
    ));
    assert!(unmet(&d).is_empty(), "{:?}", d.warnings);
    let d = ok(w.call(&c, s, create_with("/lemma/part", |a| a.take = Some(false))));
    assert!(unmet(&d).is_empty(), "{:?}", d.warnings);
    // `update {needs_add}`: the labels it adds that nobody has.
    let v = w.s.nodes[&lemma].version;
    let d = ok(w.call(
        &c,
        s,
        update(lemma, v, |p| {
            p.needs_add = labels(&["lean", "counterexample-witness"])
        }),
    ));
    assert_eq!(
        unmet(&d),
        vec![(
            Some(lemma),
            WarningDetail::UnmetNeeds {
                needs: vec![label("counterexample-witness")],
                take: false,
            }
        )]
    );
    // A take by caps that lack the needs holds the node, and warns; by caps that have them, not.
    let v = w.s.nodes[&proof].version;
    ok(w.call(
        &c,
        s,
        update(proof, v, |p| p.needs_remove = labels(&["proof-artifact"])),
    ));
    let d = ok(w.call(&c, s, take(proof)));
    assert!(matches!(
        d.reply,
        Reply::Taken {
            existing: false,
            ..
        }
    ));
    assert_eq!(
        unmet(&d),
        vec![(
            Some(proof),
            WarningDetail::UnmetNeeds {
                needs: vec![label("lean")],
                take: true,
            }
        )]
    );
    let Reply::Taken { token, .. } = d.reply else {
        panic!()
    };
    ok(w.call(&c, s, release(token, ReleaseReason::Handoff)));
    let d = ok(w.call(&lc, prover, take(proof)));
    assert!(unmet(&d).is_empty(), "{:?}", d.warnings);
    // Past `capacity.window`, a profile no longer meets a label.
    w.advance(Dur(w.s.params.capacity_window.0 + 60_000));
    let s2 = w.hello(&c);
    let d = ok(w.call(
        &c,
        s2,
        create_with("/proof2", |a| {
            a.needs = Some(labels(&["lean"]));
            a.take = Some(false);
        }),
    ));
    assert_eq!(unmet(&d).len(), 1, "{:?}", d.warnings);
}
