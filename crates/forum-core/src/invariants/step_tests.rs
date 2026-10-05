//! The step checks reject hand-built violating steps (design D.9.2): each test takes a real step
//! of `apply` or of one firing and breaks one clause.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm,
    clippy::needless_range_loop,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::vec::Vec;

use super::*;
use crate::apply::tests::{class, create, created, ok, release, text, world};
use crate::cmd::{Cmd, ControlCmd, OpCtx, SessionCtx, ViewCmd};
use crate::event::Event;
use crate::reply::Done;
use crate::state::State;
use crate::types::{Dur, Fence, OpId, ReleaseReason, ReqDigest, SessionId};

/// One step: the state before, the context, the command, the outcome, the events, and after.
struct Step {
    before: State,
    ctx: Ctx,
    cmd: Cmd,
    out: Result<Done, Error>,
    events: Vec<Event>,
    after: State,
}

fn step(w: &mut crate::testkit::World, ctx: &Ctx, cmd: Cmd) -> Step {
    w.settle();
    let before = w.s.clone();
    let mut after = before.clone();
    let mut ctx = ctx.clone();
    ctx.now = w.clock;
    let (events, out) = crate::apply::apply(&mut after, &ctx, &cmd);
    Step {
        before,
        ctx,
        cmd,
        out,
        events,
        after,
    }
}

fn violated(st: &Step) -> Vec<Inv> {
    match check_step(&st.before, &st.ctx, &st.cmd, &st.out, &st.events, &st.after) {
        Ok(()) => Vec::new(),
        Err(v) => v.into_iter().map(|x| x.inv).collect(),
    }
}

/// Replays `events` over `before` into `after`, as if the core had emitted them.
fn refold(st: &mut Step) {
    let mut s = st.before.clone();
    for e in &st.events {
        crate::evolve::evolve_mut(&mut s, e);
    }
    st.after = s;
}

fn setup() -> (crate::testkit::World, Ctx, SessionId) {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let ctx = w.ctx(&c, SessionCtx::Existing(s), None);
    (w, ctx, s)
}

#[test]
fn a_real_step_passes() {
    let (mut w, ctx, _) = setup();
    let st = step(&mut w, &ctx, create("/a"));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
}

/// H2: a hold issued in a command step without a `Touched` of its session before it.
#[test]
fn h2_rejects_a_hold_without_touched() {
    let (mut w, ctx, _) = setup();
    w.advance(Dur(1_000));
    let mut st = step(&mut w, &ctx, create("/a"));
    st.events
        .retain(|e| !matches!(e.ev, crate::event::Ev::Touched { .. }));
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::H2));
}

/// H3 and O2: a `HoldIssued` that skips a fence.
#[test]
fn h3_rejects_a_skipped_fence() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, create("/a"));
    for e in &mut st.events {
        if let crate::event::Ev::HoldIssued { token, .. } = &mut e.ev {
            token.fence = Fence(2);
        }
    }
    let v = violated(&st);
    assert!(v.contains(&Inv::H3), "{v:?}");
}

/// H4: a stale write whose outcome is not `stale_token`, or that changes the base state.
#[test]
fn h4_rejects_a_stale_write_that_applies() {
    let (mut w, ctx, s) = setup();
    let (_, t) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    let t = t.unwrap();
    ok(w.call(&class("worker"), s, release(t, ReleaseReason::Handoff)));
    let mut st = step(&mut w, &ctx, crate::apply::tests::checkpoint(t));
    assert!(violated(&st).is_empty());
    st.out = Err(Error::invalid(crate::error::Why::BadParams));
    assert!(violated(&st).contains(&Inv::H4));
    let mut st = step(&mut w, &ctx, crate::apply::tests::checkpoint(t));
    st.after.nodes.get_mut(&t.node).unwrap().title = text("changed");
    assert!(violated(&st).contains(&Inv::H4));
}

/// H5 and P2: a hold with terms other than the params', and one under a pause.
#[test]
fn h5_and_p2_reject_bad_holds() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, create("/a"));
    for e in &mut st.events {
        if let crate::event::Ev::HoldIssued { terms, .. } = &mut e.ev {
            terms.renew_max = 99;
        }
    }
    assert!(violated(&st).contains(&Inv::H5));
    ok(w.call(
        &class("worker"),
        SessionId(1),
        Cmd::Control(ControlCmd::Pause {
            scope: crate::types::NodeRef::Path(crate::apply::tests::path("/")),
            reason: text("stop"),
        }),
    ));
    let mut st = step(&mut w, &ctx, create("/b"));
    assert!(violated(&st).is_empty());
    // Forge a hold into the paused tree.
    let n = crate::query::resolve(&st.after, &crate::apply::tests::at("/b")).unwrap();
    let terms = crate::helpers::holds::terms_of(&st.after.params);
    let at = st.ctx.now;
    st.events.insert(
        st.events.len().saturating_sub(1).max(1),
        Event {
            at,
            ev: crate::event::Ev::Touched {
                session: SessionId(1),
                silence_at: at.plus(Dur(900_000)),
                retire_at: st.before.sessions[&SessionId(1)].retire_at,
            },
        },
    );
    st.events.push(Event {
        at,
        ev: crate::event::Ev::HoldIssued {
            token: Token {
                node: n,
                fence: Fence(1),
            },
            session: SessionId(1),
            lease_until: at.plus(terms.first),
            terms,
            l: terms.default,
            reopened: None,
        },
    });
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::P2));
}

/// O1: a call with an `op_id` whose `OpRecorded` is missing, or carries another outcome.
#[test]
fn o1_rejects_a_missing_or_wrong_record() {
    let (mut w, mut ctx, _) = setup();
    ctx.op = Some(OpCtx {
        id: OpId::new("o").unwrap(),
        digest: ReqDigest([3; 32]),
    });
    let mut st = step(&mut w, &ctx, create("/a"));
    assert!(violated(&st).is_empty());
    let last = st.events.pop();
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::O1));
    st.events.extend(last);
    st.out = Err(Error::op_id_conflict());
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::O1));
}

/// T1 and T4: an event at another time, and a new key before `now`.
#[test]
fn t1_and_t4_reject_bad_times() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, create("/a"));
    if let Some(e) = st.events.last_mut() {
        e.at = e.at.plus(Dur(1));
    }
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::T1));
    let mut st = step(&mut w, &ctx, create("/b"));
    let past = st.ctx.now.minus(Dur(10));
    st.after.ix.deadlines.insert(crate::state::DeadlineKey {
        at: past,
        kind: crate::state::DeadlineKind::Due,
        owner: crate::state::Owner::Node(crate::types::ROOT),
    });
    assert!(violated(&st).contains(&Inv::T4));
}

/// L1 (texts): a text over `text.max` that is not refused.
#[test]
fn l1_rejects_an_unrefused_long_text() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.before.params.text_max = 0;
    st.after.params.text_max = 0;
    st.cmd = create("/a");
    assert!(
        st.events.is_empty(),
        "a read within presence.resolution appends nothing"
    );
    assert!(violated(&st).contains(&Inv::L1));
}

/// O2: a counter that moved off its rule; O3: a changed entry.
#[test]
fn o2_and_o3_reject_counters_and_entries() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, create("/a"));
    let n = crate::query::resolve(&st.after, &crate::apply::tests::at("/a")).unwrap();
    st.after.nodes.get_mut(&n).unwrap().version = crate::types::Version(7);
    assert!(violated(&st).contains(&Inv::O2));
    let mut st = step(&mut w, &ctx, create("/b"));
    st.after.counters.events += 1;
    assert!(violated(&st).contains(&Inv::O3));
}

/// `check_fire`: a key that is not the queue's minimum, events that are not the table's, and a new
/// key at the firing's own time.
#[test]
fn check_fire_rejects_bad_firings() {
    let (mut w, _, s) = setup();
    let (_, t) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    let t = t.unwrap();
    w.at(w.s.holds[&t].lease_until);
    let before = w.s.clone();
    let key = before.ix.deadlines.first().cloned().unwrap();
    let mut after = before.clone();
    let (events, _) = crate::settle::settle(&mut after, key.at, 1);
    assert!(check_fire(&before, &key, &events, &after).is_ok());
    // Not the minimum.
    let mut other = key.clone();
    other.at = other.at.plus(Dur(1));
    assert!(check_fire(&before, &other, &events, &after).is_err());
    // The lease arm without its HoldEnded.
    let short: Vec<Event> = events.iter().take(1).cloned().collect();
    assert!(check_fire(&before, &key, &short, &after).is_err());
    // A new key at the firing's own time.
    let mut bad = after.clone();
    bad.ix.deadlines.insert(crate::state::DeadlineKey {
        at: key.at,
        kind: crate::state::DeadlineKind::Due,
        owner: crate::state::Owner::Node(crate::types::ROOT),
    });
    assert!(check_fire(&before, &key, &events, &bad).is_err());
}

/// R2: a `Missed` close of a node with a checkpoint.
#[test]
fn r2_rejects_a_missed_close_after_a_checkpoint() {
    let (mut w, ctx, s) = setup();
    let (n, t) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    ok(w.call(
        &class("worker"),
        s,
        crate::apply::tests::checkpoint(t.unwrap()),
    ));
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.events.push(Event {
        at: st.ctx.now,
        ev: crate::event::Ev::NodeClosed {
            node: n,
            outcome: crate::types::Outcome::Missed,
            merged_into: None,
            by: None,
        },
    });
    assert!(violated(&st).contains(&Inv::R2));
}

/// `check_fire`: a firing of a key that is not the queue's minimum is rejected, even when its own
/// events are the table's (T4).
#[test]
fn check_fire_rejects_a_key_out_of_order() {
    let (mut w, _, s) = setup();
    let (n, _) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    ok(w.call(
        &class("worker"),
        s,
        Cmd::Control(ControlCmd::Pause {
            scope: crate::types::NodeRef::Id(n),
            reason: text("r"),
        }),
    ));
    let before = w.s.clone();
    let mut keys = before.ix.deadlines.iter().cloned();
    let first = keys.next().unwrap();
    let later = keys
        .find(|k| k.kind == crate::state::DeadlineKind::Pause)
        .unwrap();
    assert!(first < later);
    let mut after = before.clone();
    let mut ex = crate::helpers::Exec::new(&mut after, later.at, None);
    ex.emit(crate::event::Ev::Fired {
        kind: later.kind,
        owner: later.owner.clone(),
    });
    crate::settle::fire(&mut ex, &later);
    let (events, _) = ex.finish();
    let v = check_fire(&before, &later, &events, &after).unwrap_err();
    assert!(
        v.contains(&Violation {
            inv: Inv::T4,
            detail: ViolationDetail::Deadline(later),
        }),
        "{v:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// One violating step per clause (the review of WP-D3): each clause, disabled, fails its test.

/// The violations of a firing.
fn fire_violated(before: &State, key: &DeadlineKey, events: &[Event], after: &State) -> Vec<Inv> {
    match check_fire(before, key, events, after) {
        Ok(()) => Vec::new(),
        Err(v) => v.into_iter().map(|x| x.inv).collect(),
    }
}

/// Settles `w` to just before its first queued key of `kind`, and fires that key alone: the state
/// before, the key, its events, and the state after. The real firing passes `check_fire`.
fn fire_first(
    w: &mut crate::testkit::World,
    kind: DeadlineKind,
) -> (State, DeadlineKey, Vec<Event>, State) {
    let key =
        w.s.ix
            .deadlines
            .iter()
            .find(|k| k.kind == kind)
            .cloned()
            .unwrap();
    w.at(Time(key.at.0 - 1));
    w.settle();
    assert_eq!(w.s.ix.deadlines.first(), Some(&key));
    let before = w.s.clone();
    let mut after = before.clone();
    let (events, _) = crate::settle::settle(&mut after, key.at, 1);
    assert!(
        fire_violated(&before, &key, &events, &after).is_empty(),
        "{:?}",
        check_fire(&before, &key, &events, &after)
    );
    (before, key, events, after)
}

/// A stale `checkpoint` step: `/a` created, its hold released, then a checkpoint with the token.
fn stale_step() -> (crate::testkit::World, Step, SessionId, Token) {
    let (mut w, ctx, s) = setup();
    let (_, t) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    let t = t.unwrap();
    ok(w.call(&class("worker"), s, release(t, ReleaseReason::Handoff)));
    let st = step(&mut w, &ctx, crate::apply::tests::checkpoint(t));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    (w, st, s, t)
}

/// A user request on `node` from principal `p`, settled or not.
fn user_request(node: NodeId, p: PrincipalId, settled: bool) -> crate::state::Request {
    crate::state::Request {
        node,
        origin: crate::types::Key {
            key: crate::types::KeyStr::of_user_entry(Eid(1)),
            nv: crate::types::NormVer(0),
        },
        conv: crate::types::KeyStr::of_user_entry(Eid(1)),
        from_user: Some(Eid(1)),
        requester: crate::types::Requester::User(p),
        answered_by: settled.then_some(Eid(3)),
        settled: settled.then_some(crate::state::Settlement {
            decision: Eid(2),
            answer: Eid(3),
        }),
        moved_from: None,
    }
}

/// T1 (`check_fire`): an event of a firing at another time than the key's.
#[test]
fn check_fire_rejects_an_event_at_another_time() {
    let (mut w, _, s) = setup();
    ok(w.call(&class("worker"), s, create("/a")));
    let (before, key, mut events, after) = fire_first(&mut w, DeadlineKind::Lease);
    events[1].at = key.at.plus(Dur(1));
    assert!(fire_violated(&before, &key, &events, &after).contains(&Inv::T1));
}

/// H2 (`check_fire`): a session that retires while it holds a hold.
#[test]
fn check_fire_rejects_a_retirement_with_a_hold() {
    let (mut w, _, s) = setup();
    ok(w.call(&class("worker"), s, create("/a")));
    let before = w.s.clone();
    assert!(before.holds.values().any(|h| h.session == s));
    let key = DeadlineKey {
        at: before.last_now,
        kind: DeadlineKind::Retire,
        owner: Owner::Session(s),
    };
    let events = alloc::vec![
        Event {
            at: key.at,
            ev: Ev::Fired {
                kind: key.kind,
                owner: key.owner.clone(),
            },
        },
        Event {
            at: key.at,
            ev: Ev::SessionRetired { session: s },
        },
    ];
    let v = check_fire(&before, &key, &events, &before).unwrap_err();
    assert!(v.contains(&Violation {
        inv: Inv::H2,
        detail: ViolationDetail::Session(s),
    }));
}

/// The table of D.8.2 (`check_fire`, T4): a refusal's expiry that names another class.
#[test]
fn check_fire_rejects_a_wrong_refusal_expiry() {
    let (mut w, _, s) = setup();
    let (_, t) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    ok(w.call(
        &class("worker"),
        s,
        release(t.unwrap(), ReleaseReason::Cannot),
    ));
    let (before, key, mut events, after) = fire_first(&mut w, DeadlineKind::RefusalExpiry);
    for e in &mut events {
        if let Ev::RefusalExpired { class: c, .. } = &mut e.ev {
            *c = class("other");
        }
    }
    assert!(fire_violated(&before, &key, &events, &after).contains(&Inv::T4));
}

/// The table of D.8.2 (`check_fire`, T4): a pause's expiry that names another scope.
#[test]
fn check_fire_rejects_a_wrong_pause_expiry() {
    let (mut w, _, s) = setup();
    let (n, _) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    ok(w.call(
        &class("worker"),
        s,
        Cmd::Control(ControlCmd::Pause {
            scope: crate::types::NodeRef::Id(n),
            reason: text("r"),
        }),
    ));
    let (before, key, mut events, after) = fire_first(&mut w, DeadlineKind::Pause);
    for e in &mut events {
        if let Ev::PauseExpired { scope } = &mut e.ev {
            *scope = crate::types::ROOT;
        }
    }
    assert!(fire_violated(&before, &key, &events, &after).contains(&Inv::T4));
}

/// The table of D.8.2 (`check_fire`, T4): a session falling silent unconfirms exactly its pending
/// `said` entries, and here it has none.
#[test]
fn check_fire_rejects_a_wrong_silence() {
    let (mut w, _, _) = setup();
    let (before, key, mut events, after) = fire_first(&mut w, DeadlineKind::Silence);
    events.push(Event {
        at: key.at,
        ev: Ev::SaidUnconfirmed { said: Eid(77) },
    });
    assert!(fire_violated(&before, &key, &events, &after).contains(&Inv::T4));
}

/// The table of D.8.2 (`check_fire`, T4): a retirement of another session than the key's.
#[test]
fn check_fire_rejects_a_wrong_retirement() {
    let (mut w, _, _) = setup();
    let (before, key, mut events, after) = fire_first(&mut w, DeadlineKind::Retire);
    for e in &mut events {
        if let Ev::SessionRetired { session } = &mut e.ev {
            *session = SessionId(99);
        }
    }
    assert!(fire_violated(&before, &key, &events, &after).contains(&Inv::T4));
}

/// O2 (`check_fire`): a firing whose state moved a node's counters off its events (D.9.2: O2 is
/// checked per event in every step).
#[test]
fn check_fire_rejects_counters_off_the_events() {
    let (mut w, _, s) = setup();
    let (n, _) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    let (before, key, events, after) = fire_first(&mut w, DeadlineKind::Lease);
    for f in [
        |x: &mut crate::state::Node| x.version = crate::types::Version(99),
        |x: &mut crate::state::Node| x.seq = crate::types::Seq(42),
        |x: &mut crate::state::Node| x.fence = Fence(42),
    ] {
        let mut bad = after.clone();
        f(bad.nodes.get_mut(&n).unwrap());
        assert!(fire_violated(&before, &key, &events, &bad).contains(&Inv::O2));
    }
}

/// O1: a live record answers with no events and `after == before`: a replay that claims a
/// conflict, or appends.
#[test]
fn o1_rejects_a_wrong_answer_from_a_live_record() {
    let (mut w, mut ctx, _) = setup();
    ctx.op = Some(OpCtx {
        id: OpId::new("o").unwrap(),
        digest: ReqDigest([3; 32]),
    });
    let first = step(&mut w, &ctx, create("/a"));
    w.s = first.after.clone();
    let mut st = step(&mut w, &ctx, create("/a"));
    assert_eq!(
        st.out.as_ref().map(|d| &d.reply),
        Ok(&crate::reply::Reply::Replay)
    );
    assert!(violated(&st).is_empty());
    st.out = Err(Error::op_id_conflict());
    assert!(violated(&st).contains(&Inv::O1));
    let mut st = step(&mut w, &ctx, create("/a"));
    st.events.push(Event {
        at: st.ctx.now,
        ev: Ev::Touched {
            session: SessionId(1),
            silence_at: st.ctx.now.plus(Dur(900_000)),
            retire_at: st.before.sessions[&SessionId(1)].retire_at,
        },
    });
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::O1));
}

/// O1: a call that appended nothing records nothing; a record stores `now + op.retain`.
#[test]
fn o1_rejects_a_record_without_events_or_with_another_expiry() {
    let (mut w, mut ctx, _) = setup();
    let op = OpCtx {
        id: OpId::new("o").unwrap(),
        digest: ReqDigest([3; 32]),
    };
    ctx.op = Some(op.clone());
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    assert!(st.events.is_empty() && violated(&st).is_empty());
    st.after.ops.insert(
        (ctx.class.clone(), op.id.clone()),
        crate::state::OpRecord {
            digest: op.digest,
            session: SessionId(1),
            expires_at: st.ctx.now.plus(st.before.params.op_retain),
        },
    );
    assert!(violated(&st).contains(&Inv::O1));
    let mut st = step(&mut w, &ctx, create("/a"));
    assert!(violated(&st).is_empty());
    let r = st
        .after
        .ops
        .get_mut(&(ctx.class.clone(), op.id.clone()))
        .unwrap();
    r.expires_at = r.expires_at.plus(Dur(1));
    assert!(violated(&st).contains(&Inv::O1));
}

/// H4: a stale write with two late entries, or with another event.
#[test]
fn h4_rejects_other_events_of_a_stale_write() {
    let (_, st, s, _) = stale_step();
    let mut two = Step {
        events: st.events.clone(),
        after: st.after.clone(),
        before: st.before.clone(),
        ctx: st.ctx.clone(),
        cmd: st.cmd.clone(),
        out: st.out.clone(),
    };
    let late = two.events[0].clone();
    two.events.push(late);
    assert!(violated(&two).contains(&Inv::H4));
    let mut other = st;
    other.events.push(Event {
        at: other.ctx.now,
        ev: Ev::Touched {
            session: s,
            silence_at: other.ctx.now.plus(Dur(900_000)),
            retire_at: other.before.sessions[&s].retire_at,
        },
    });
    assert!(violated(&other).contains(&Inv::H4));
}

/// H4: a stale write that changes a hold, a pause, a session, or another base map (the base state
/// of `after` equals `before`'s but the late entry, the prunes, and a first call's session).
#[test]
fn h4_rejects_changes_to_the_base_state() {
    let (_, st, s, t) = stale_step();
    let p = PrincipalId(77);
    let note = crate::state::Note {
        node: t.node,
        to: crate::types::Addressee::Session(SessionId(1)),
        until: st.ctx.now.plus(Dur(60_000)),
        recipients: alloc::collections::BTreeSet::new(),
        pending_holder: false,
        read_by: alloc::collections::BTreeSet::new(),
    };
    let meta = crate::state::EntryMeta {
        node: t.node,
        seq: crate::types::Seq(1),
        at: st.ctx.now,
        by: crate::state::Author::Session(SessionId(1)),
        ty: EntryType::Object,
        principal: None,
    };
    let forge: [&dyn Fn(&mut State); 12] = [
        &|a| {
            a.notes.insert(Eid(900), note.clone());
        },
        &|a| {
            a.aquestions.insert(
                Eid(901),
                crate::state::AQuestion {
                    node: t.node,
                    asker: SessionId(1),
                    state: crate::state::AqState::Open,
                },
            );
        },
        &|a| {
            a.entries.insert(Eid(902), meta);
        },
        &|a| {
            a.holds.insert(
                Token {
                    node: t.node,
                    fence: Fence(9),
                },
                a.holds
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or(crate::state::Hold {
                        session: SessionId(1),
                        taken_at: a.last_now,
                        lease_until: a.last_now.plus(Dur(60_000)),
                        last_extended: a.last_now,
                        terms: crate::helpers::holds::terms_of(&a.params),
                        l: Dur(60_000),
                        renews: 0,
                        checkpointed: false,
                        reopened: None,
                    }),
            );
        },
        &|a| {
            a.pauses.insert(
                t.node,
                crate::state::Pause {
                    until: a.last_now.plus(Dur(60_000)),
                    last_renewed: a.last_now,
                    ttl: a.params.pause_ttl,
                    by: SessionId(1),
                    class: class("worker"),
                },
            );
        },
        &|a| {
            a.sessions
                .get_mut(&SessionId(1))
                .unwrap()
                .caps
                .insert(crate::types::Label::new("z").unwrap());
        },
        &|a| {
            a.principal_ids
                .insert(crate::types::PrincipalKey::new("test:z").unwrap(), p);
        },
        &|a| {
            a.digest_left.insert(p, 5);
        },
        &|a| {
            a.user_msgs.insert(p, alloc::vec![a.last_now]);
        },
        &|a| a.params.renew_max += 1,
        &|a| {
            a.nodes.get_mut(&t.node).unwrap().people_left = crate::types::Seq(9);
        },
        &|a| a.nodes.get_mut(&t.node).unwrap().title = text("changed"),
    ];
    for (i, f) in forge.iter().enumerate() {
        let mut bad = Step {
            events: st.events.clone(),
            after: st.after.clone(),
            before: st.before.clone(),
            ctx: st.ctx.clone(),
            cmd: st.cmd.clone(),
            out: st.out.clone(),
        };
        f(&mut bad.after);
        assert!(violated(&bad).contains(&Inv::H4), "forgery {i}");
    }
    let _ = s;
}

/// H4: the prune of D.4.12 that a stale write's event runs raises `people_left` on the node of a
/// people entry whose metadata leaves memory, and the check allows it (a false positive before the
/// review of WP-D3).
#[test]
fn h4_allows_the_people_left_of_the_prune() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create("/a"))));
    let t = t.unwrap();
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    let e = crate::apply::tests::entry(
        &mut w,
        n,
        crate::state::Author::Session(s),
        crate::event::EntryBody::Heard {
            conv: crate::types::KeyStr::new("chat:x/1").unwrap(),
            answers: None,
        },
    );
    let leave = w.s.leaving[&crate::state::EntityRef::Meta(e)];
    w.advance(Dur(leave.0 - w.clock.0 + 1));
    assert!(
        w.s.entries.contains_key(&e),
        "still resident: no event since"
    );
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let st = step(&mut w, &ctx, crate::apply::tests::checkpoint(t));
    assert!(matches!(&st.out, Err(x) if x.code == ErrorCode::StaleToken));
    assert!(!st.after.entries.contains_key(&e), "pruned by the step");
    assert_ne!(
        st.before.nodes[&n].people_left,
        st.after.nodes[&n].people_left
    );
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
}

/// O2: a digest change in an `update` without `digest_version`; a `stale_version` outcome with an
/// event of the command.
#[test]
fn o2_rejects_digest_and_stale_version_steps() {
    let (mut w, ctx, s) = setup();
    let (n, _) = created(&ok(w.call(
        &class("worker"),
        s,
        crate::apply::tests::create_with("/a", |a| a.take = Some(false)),
    )));
    let node = &w.s.nodes[&n];
    let (v, dv) = (node.version, node.digest_version);
    let mut st = step(
        &mut w,
        &ctx,
        Cmd::Node(NodeCmd::Update {
            at: crate::types::NodeRef::Id(n),
            version: v,
            digest_version: Some(dv),
            token: None,
            patch: crate::cmd::Patch {
                digest: Some(text("d")),
                ..crate::cmd::Patch::default()
            },
        }),
    );
    assert!(st.out.is_ok() && violated(&st).is_empty(), "{:?}", st.out);
    if let Cmd::Node(NodeCmd::Update { digest_version, .. }) = &mut st.cmd {
        *digest_version = None;
    }
    assert!(violated(&st).contains(&Inv::O2));
    let mut st = step(
        &mut w,
        &ctx,
        Cmd::Node(NodeCmd::Update {
            at: crate::types::NodeRef::Id(n),
            version: crate::types::Version(v.0 + 5),
            digest_version: None,
            token: None,
            patch: crate::cmd::Patch {
                title: Some(text("x")),
                ..crate::cmd::Patch::default()
            },
        }),
    );
    assert!(matches!(&st.out, Err(e) if e.code == ErrorCode::StaleVersion));
    assert!(violated(&st).is_empty());
    st.events.push(Event {
        at: st.ctx.now,
        ev: Ev::NodeUpdated {
            node: n,
            patch: NodePatch {
                tags: Some([crate::types::Label::new("z").unwrap()].into()),
                ..NodePatch::default()
            },
        },
    });
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::O2));
}

/// O2 (§26: "an edit with a stale `version` or a stale `digest_version` changes nothing"): a
/// successful `update` whose command named a stale version, or a stale digest version.
#[test]
fn o2_rejects_a_successful_update_with_a_stale_version() {
    let (mut w, ctx, s) = setup();
    let (n, _) = created(&ok(w.call(
        &class("worker"),
        s,
        crate::apply::tests::create_with("/a", |a| a.take = Some(false)),
    )));
    let v = w.s.nodes[&n].version;
    let mut st = step(
        &mut w,
        &ctx,
        Cmd::Node(NodeCmd::Update {
            at: crate::types::NodeRef::Id(n),
            version: v,
            digest_version: None,
            token: None,
            patch: crate::cmd::Patch {
                title: Some(text("new title")),
                ..crate::cmd::Patch::default()
            },
        }),
    );
    assert!(st.out.is_ok() && violated(&st).is_empty());
    let good = st.cmd.clone();
    if let Cmd::Node(NodeCmd::Update { version, .. }) = &mut st.cmd {
        *version = crate::types::Version(version.0 + 7);
    }
    assert!(violated(&st).contains(&Inv::O2));
    st.cmd = good;
    if let Cmd::Node(NodeCmd::Update { digest_version, .. }) = &mut st.cmd {
        *digest_version = Some(crate::types::Version(42));
    }
    assert!(violated(&st).contains(&Inv::O2));
}

/// O3: a resident entry whose metadata changed; `retracted` that shrinks.
#[test]
fn o3_rejects_a_changed_entry_and_a_lost_retraction() {
    let (mut w, ctx, s) = setup();
    let (n, _) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    let e = crate::apply::tests::entry(
        &mut w,
        n,
        crate::state::Author::Session(s),
        crate::event::EntryBody::Object,
    );
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    assert!(violated(&st).is_empty());
    st.after.entries.get_mut(&e).unwrap().at = Time(1);
    assert!(violated(&st).contains(&Inv::O3));
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.before
        .retracted
        .insert(crate::state::BasisTarget::Node(n));
    assert!(violated(&st).contains(&Inv::O3));
}

/// O3: the metadata of an entry leaves memory only when it is due (D.4.12): a question, a note,
/// or an answer that vanishes before its time is reported; a note whose `until` came may go.
#[test]
fn o3_rejects_metadata_that_was_not_due_to_leave() {
    let (mut w, ctx, _) = setup();
    let st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    assert!(violated(&st).is_empty());
    let meta = crate::state::EntryMeta {
        node: crate::types::ROOT,
        seq: crate::types::Seq(1),
        at: st.ctx.now,
        by: crate::state::Author::Server,
        ty: EntryType::Question,
        principal: None,
    };
    for ty in [
        EntryType::Question,
        EntryType::Note,
        EntryType::Answer,
        EntryType::Checkpoint,
    ] {
        let mut bad = Step {
            events: st.events.clone(),
            after: st.after.clone(),
            before: st.before.clone(),
            ctx: st.ctx.clone(),
            cmd: st.cmd.clone(),
            out: st.out.clone(),
        };
        bad.before
            .entries
            .insert(Eid(12_345), crate::state::EntryMeta { ty, ..meta });
        assert!(violated(&bad).contains(&Inv::O3), "{ty:?}");
    }
    // A note whose `until` came leaves with its metadata.
    let mut fine = st;
    fine.before.entries.insert(
        Eid(12_345),
        crate::state::EntryMeta {
            ty: EntryType::Note,
            ..meta
        },
    );
    fine.before.notes.insert(
        Eid(12_345),
        crate::state::Note {
            node: crate::types::ROOT,
            to: crate::types::Addressee::Session(SessionId(1)),
            until: fine.ctx.now,
            recipients: alloc::collections::BTreeSet::new(),
            pending_holder: false,
            read_by: alloc::collections::BTreeSet::new(),
        },
    );
    assert!(!violated(&fine).contains(&Inv::O3), "{:?}", violated(&fine));
}

/// Settled: a `settled` that clears; a `to_user` of kind `ack` that answers a user request.
#[test]
fn settled_rejects_a_cleared_settlement_and_an_ack_that_answers() {
    let (mut w, ctx, _) = setup();
    let st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    let r = Eid(500);
    let p = crate::types::USER_PRINCIPAL;
    let mut bad = Step {
        events: st.events.clone(),
        after: st.after.clone(),
        before: st.before.clone(),
        ctx: st.ctx.clone(),
        cmd: st.cmd.clone(),
        out: st.out.clone(),
    };
    bad.before
        .requests
        .insert(r, user_request(crate::types::ROOT, p, true));
    bad.after
        .requests
        .insert(r, user_request(crate::types::ROOT, p, false));
    assert!(violated(&bad).contains(&Inv::Settled));
    let mut ack = st;
    ack.before
        .requests
        .insert(r, user_request(crate::types::ROOT, p, false));
    let a = Eid(501);
    ack.events = alloc::vec![
        Event {
            at: ack.ctx.now,
            ev: Ev::Appended {
                eid: a,
                node: crate::types::USER,
                seq: crate::types::Seq(1),
                by: crate::state::Author::Session(SessionId(1)),
                body: EntryBody::ToUser {
                    kind: ToUserKind::Ack,
                    verbatim: true,
                    re: [r].into(),
                    principal: p,
                },
                text: crate::event::EntryText::default(),
            },
        },
        Event {
            at: ack.ctx.now,
            ev: Ev::RequestAnswered { request: r, by: a },
        },
    ];
    assert!(violated(&ack).contains(&Inv::Settled));
}

/// D1: an expiry of a people question with no digest entry for its audience (never empty).
#[test]
fn d1_rejects_an_expiry_without_its_digest_entries() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.events.push(Event {
        at: st.ctx.now,
        ev: Ev::PqExpired { q: Eid(600) },
    });
    assert!(violated(&st).contains(&Inv::D1));
}

/// U1: a user question created with a chain other than `spec::chain`'s.
#[test]
fn u1_rejects_a_wrong_chain() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.events.push(Event {
        at: st.ctx.now,
        ev: Ev::Appended {
            eid: Eid(700),
            node: crate::types::ROOT,
            seq: crate::types::Seq(1),
            by: crate::state::Author::Session(SessionId(1)),
            body: EntryBody::Question(QuestionBody::User {
                source: UqSource::Agent {
                    session: SessionId(1),
                    digest: ReqDigest([1; 32]),
                },
                about: None,
                to: alloc::vec![PrincipalId(42)],
                due: st.ctx.now.plus(Dur(60_000)),
                has_default: false,
            }),
            text: crate::event::EntryText::default(),
        },
    });
    assert!(violated(&st).contains(&Inv::U1));
}

/// L1 (children): a create that puts a parent past `tree.max_children`.
#[test]
fn l1_rejects_a_child_past_the_limit() {
    let (mut w, ctx, s) = setup();
    ok(w.call(&class("worker"), s, create("/p/a")));
    let mut st = step(&mut w, &ctx, create("/p/b"));
    assert!(st.out.is_ok() && violated(&st).is_empty());
    st.after.params.tree_max_children = 1;
    assert!(violated(&st).contains(&Inv::L1));
}

/// R2 (§26: "only the server closes a node as `missed`"): a session's close rewritten to `missed`,
/// and a command `close {outcome: missed}` that succeeds.
#[test]
fn r2_rejects_a_missed_close_by_a_session() {
    let (mut w, ctx, s) = setup();
    let (n, _) = created(&ok(w.call(
        &class("worker"),
        s,
        crate::apply::tests::create_with("/a", |a| a.take = Some(false)),
    )));
    let mut st = step(&mut w, &ctx, crate::apply::tests::close(n, Outcome::Done));
    assert!(st.out.is_ok() && violated(&st).is_empty());
    let good = st.events.clone();
    for e in &mut st.events {
        match &mut e.ev {
            Ev::NodeClosed { outcome, .. } => *outcome = Outcome::Missed,
            Ev::Appended {
                body: EntryBody::System(SystemNote::Closed { outcome }),
                by,
                ..
            } => {
                assert_eq!(*by, crate::state::Author::Session(s));
                *outcome = Outcome::Missed;
            }
            _ => {}
        }
    }
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::R2));
    st.events = good;
    refold(&mut st);
    st.cmd = crate::apply::tests::close(n, Outcome::Missed);
    assert!(violated(&st).contains(&Inv::R2));
}

/// T4 (command): a new key at exactly `now` only for a clamped user question's due, or for the
/// Silence key of a session whose last hold the command ended: not for a session that holds one,
/// nor for another kind.
#[test]
fn t4_rejects_keys_at_now() {
    let (mut w, ctx, s) = setup();
    let st = step(&mut w, &ctx, create("/a"));
    assert!(st.after.holds.values().any(|h| h.session == s));
    for kind in [DeadlineKind::Silence, DeadlineKind::UserQuestion] {
        let mut bad = Step {
            events: st.events.clone(),
            after: st.after.clone(),
            before: st.before.clone(),
            ctx: st.ctx.clone(),
            cmd: st.cmd.clone(),
            out: st.out.clone(),
        };
        bad.after.ix.deadlines.insert(DeadlineKey {
            at: st.ctx.now,
            kind,
            owner: if kind == DeadlineKind::Silence {
                Owner::Session(s)
            } else {
                Owner::Entry(Eid(800))
            },
        });
        assert!(violated(&bad).contains(&Inv::T4), "{kind:?}");
    }
}

/// T4 (command, 1.4): a close by one session that ends another's hold after that session's
/// `silence_at` moves its Silence key to `now`, which the step check allows and the post-settle
/// fires (D.4.8). `World::call` runs `check_step`.
#[test]
fn t4_allows_the_silence_key_of_a_hold_ended_past_silence_at() {
    let mut w = world();
    let c = class("worker");
    let a = w.hello(&c);
    let b = w.hello(&c);
    let (n, t) = created(&ok(w.call(
        &c,
        a,
        crate::apply::tests::create_with("/a", |x| x.capacity = Some(2)),
    )));
    let t = t.unwrap();
    w.advance(Dur(10_000));
    ok(w.call(&c, a, crate::apply::tests::checkpoint(t)));
    let silence_at = w.s.sessions[&a].silence_at;
    assert!(w.s.holds[&t].lease_until > silence_at);
    w.at(Time(silence_at.0 + 60_000));
    w.settle();
    assert!(!w.s.sessions[&a].silent);
    ok(w.call(&c, b, crate::apply::tests::close(n, Outcome::Done)));
    assert!(w.s.sessions[&a].silent);
}

/// T4 and T1 (`check_fire`): the fired key still queued afterwards; a key before `last_now`.
#[test]
fn check_fire_rejects_a_key_left_queued_or_before_last_now() {
    let (mut w, _, s) = setup();
    ok(w.call(&class("worker"), s, create("/a")));
    let (before, key, events, after) = fire_first(&mut w, DeadlineKind::Lease);
    let mut kept = after.clone();
    kept.ix.deadlines.insert(key.clone());
    assert!(fire_violated(&before, &key, &events, &kept).contains(&Inv::T4));
    let mut late = before.clone();
    late.last_now = key.at.plus(Dur(1));
    let mut late_after = after.clone();
    late_after.last_now = late.last_now;
    assert!(fire_violated(&late, &key, &events, &late_after).contains(&Inv::T1));
    // `last_now` afterwards other than the key's time; a first event other than its `Fired`.
    let mut moved = after;
    moved.last_now = key.at.plus(Dur(1));
    assert!(fire_violated(&before, &key, &events, &moved).contains(&Inv::T1));
    let mut unfired = events.clone();
    unfired[0].ev = Ev::Fired {
        kind: DeadlineKind::Due,
        owner: key.owner.clone(),
    };
    let mut refolded = before.clone();
    for e in &unfired {
        crate::evolve::evolve_mut(&mut refolded, e);
    }
    assert!(fire_violated(&before, &key, &unfired, &refolded).contains(&Inv::T4));
}

/// H4 on a first call: a stale write opens its session and nothing else, so a second new session,
/// or a principal no `PrincipalSeen` of the step names, is reported.
#[test]
fn h4_rejects_more_than_the_first_calls_session() {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    let (_, t) = created(&ok(w.call(&c, s, create("/a"))));
    let t = t.unwrap();
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let st = step(&mut w, &ctx, crate::apply::tests::checkpoint(t));
    assert!(matches!(&st.out, Err(x) if x.code == ErrorCode::StaleToken));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    let mut two = Step {
        events: st.events.clone(),
        after: st.after.clone(),
        before: st.before.clone(),
        ctx: st.ctx.clone(),
        cmd: st.cmd.clone(),
        out: st.out.clone(),
    };
    let extra = two.after.sessions[&s].clone();
    two.after.sessions.insert(SessionId(77), extra);
    assert!(violated(&two).contains(&Inv::H4));
    let mut named = st;
    named.after.principal_ids.insert(
        crate::types::PrincipalKey::new("test:z").unwrap(),
        PrincipalId(77),
    );
    assert!(violated(&named).contains(&Inv::H4));
}

/// T1 (command), each clause alone: an event at another time with `last_now` where the events
/// should leave it; `last_now` moved with the events at the right time.
#[test]
fn t1_rejects_each_clause_alone() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, create("/a"));
    assert!(violated(&st).is_empty());
    let mut off = Step {
        events: st.events.clone(),
        after: st.after.clone(),
        before: st.before.clone(),
        ctx: st.ctx.clone(),
        cmd: st.cmd.clone(),
        out: st.out.clone(),
    };
    off.events[0].at = off.ctx.now.plus(Dur(1));
    assert!(violated(&off).contains(&Inv::T1));
    st.after.last_now = st.ctx.now.plus(Dur(1));
    assert!(violated(&st).contains(&Inv::T1));
}

/// O1: an `OpRecorded` in a call without an `op_id`, and in a call under `SessionCtx::System`.
#[test]
fn o1_rejects_a_record_without_an_op_id_or_under_system() {
    let (mut w, ctx, s) = setup();
    let recorded = |at: Time| Event {
        at,
        ev: Ev::OpRecorded {
            class: class("worker"),
            op: OpId::new("o").unwrap(),
            digest: ReqDigest([3; 32]),
            session: s,
            expires_at: at.plus(Dur(86_400_000)),
            outcome: Err(Error::invalid(crate::error::Why::BadParams)),
        },
    };
    let mut st = step(&mut w, &ctx, create("/a"));
    st.events.push(recorded(st.ctx.now));
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::O1));
    let sys = w.ctx(&class("worker"), SessionCtx::System, None);
    let mut st = step(
        &mut w,
        &sys,
        Cmd::Admin(crate::cmd::AdminCmd::SetParams(
            crate::params::Params::default(),
        )),
    );
    assert!(st.out.is_ok() && violated(&st).is_empty(), "{:?}", st.out);
    st.events.push(recorded(st.ctx.now));
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::O1));
}

/// H2 (command): no command retires a session; retirement is the Retire firing's (CS-8).
#[test]
fn h2_rejects_a_retirement_in_a_command() {
    let (mut w, ctx, _) = setup();
    let other = w.hello(&class("worker"));
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.events.push(Event {
        at: st.ctx.now,
        ev: Ev::SessionRetired { session: other },
    });
    refold(&mut st);
    let v = check_step(&st.before, &st.ctx, &st.cmd, &st.out, &st.events, &st.after).unwrap_err();
    assert!(v.contains(&Violation {
        inv: Inv::H2,
        detail: ViolationDetail::Session(other),
    }));
}

/// H4's preconditions: no late entry for a missing target or an unknown session; a live token is
/// never answered `stale_token`.
#[test]
fn h4_rejects_a_late_entry_before_its_preconditions_and_a_live_token_called_stale() {
    let (mut w, ctx, s) = setup();
    let (n, t) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    let t = t.unwrap();
    let late = |at: Time, node: NodeId, token: Token| Event {
        at,
        ev: Ev::Appended {
            eid: Eid(950),
            node,
            seq: crate::types::Seq(99),
            by: crate::state::Author::Session(s),
            body: EntryBody::Late {
                token,
                attempted: alloc::boxed::Box::new(crate::apply::tests::checkpoint(token)),
            },
            text: crate::event::EntryText::default(),
        },
    };
    // A missing target: `not_found(Node)`, no late entry.
    let gone = Token {
        node: NodeId(9_999),
        fence: Fence(1),
    };
    let mut st = step(&mut w, &ctx, crate::apply::tests::checkpoint(gone));
    assert!(st.out.is_err() && violated(&st).is_empty());
    st.events.push(late(st.ctx.now, n, gone));
    assert!(violated(&st).contains(&Inv::H4));
    // An unknown session: `not_found(Session)`, no late entry.
    let mut nobody = ctx.clone();
    nobody.session = SessionCtx::Existing(SessionId(99));
    let mut st = step(&mut w, &nobody, crate::apply::tests::checkpoint(t));
    assert!(st.out.is_err() && violated(&st).is_empty());
    st.events.push(late(st.ctx.now, n, t));
    assert!(violated(&st).contains(&Inv::H4));
    // A live token answered `stale_token`.
    let mut st = step(&mut w, &ctx, crate::apply::tests::checkpoint(t));
    assert!(st.out.is_ok() && violated(&st).is_empty());
    st.out = Err(Error::of(crate::error::Hint::StaleToken {
        token: t,
        late: Eid(1),
    }));
    assert!(violated(&st).contains(&Inv::H4));
}

/// O3: an entry in `after` that the step neither appended nor restored.
#[test]
fn o3_rejects_an_entry_from_nowhere() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.after.entries.insert(
        Eid(990),
        crate::state::EntryMeta {
            node: crate::types::ROOT,
            seq: crate::types::Seq(1),
            at: st.ctx.now,
            by: crate::state::Author::Server,
            ty: EntryType::Object,
            principal: None,
        },
    );
    assert!(violated(&st).contains(&Inv::O3));
}

/// R2: a `missed` close by the server, with its own `System(Closed)` entry, of a node with a
/// checkpoint.
#[test]
fn r2_rejects_a_server_missed_close_after_a_checkpoint() {
    let (mut w, ctx, s) = setup();
    let (n, t) = created(&ok(w.call(&class("worker"), s, create("/a"))));
    ok(w.call(
        &class("worker"),
        s,
        crate::apply::tests::checkpoint(t.unwrap()),
    ));
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    let at = st.ctx.now;
    st.events.push(Event {
        at,
        ev: Ev::Appended {
            eid: st.before.counters.next_eid,
            node: n,
            seq: crate::types::Seq(st.before.nodes[&n].seq.0 + 1),
            by: crate::state::Author::Server,
            body: EntryBody::System(SystemNote::Closed {
                outcome: Outcome::Missed,
            }),
            text: crate::event::EntryText::default(),
        },
    });
    st.events.push(Event {
        at,
        ev: Ev::NodeClosed {
            node: n,
            outcome: Outcome::Missed,
            merged_into: None,
            by: None,
        },
    });
    let v = check_step(&st.before, &st.ctx, &st.cmd, &st.out, &st.events, &st.after).unwrap_err();
    assert_eq!(
        v.iter()
            .filter(|x| x.inv == Inv::R2 && x.detail == ViolationDetail::Node(n))
            .count(),
        1,
        "{v:?}"
    );
}

/// D1: a `DigestShown` with no `to_user` of the step behind it; a digest entry with no expiry,
/// default, drop, or lapse in the step.
#[test]
fn d1_rejects_a_shown_entry_without_its_report_and_an_entry_without_its_cause() {
    let (mut w, ctx, _) = setup();
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.events.push(Event {
        at: st.ctx.now,
        ev: Ev::DigestShown {
            entry: Eid(980),
            by: Eid(981),
        },
    });
    assert!(violated(&st).contains(&Inv::D1));
    let mut st = step(&mut w, &ctx, Cmd::View(ViewCmd::Constitution));
    st.events.push(Event {
        at: st.ctx.now,
        ev: Ev::Appended {
            eid: Eid(982),
            node: crate::types::USER_DIGEST,
            seq: crate::types::Seq(1),
            by: crate::state::Author::Server,
            body: EntryBody::System(SystemNote::Digest {
                item: DigestItem::Assumption { question: Eid(983) },
                to: crate::types::USER_PRINCIPAL,
            }),
            text: crate::event::EntryText::default(),
        },
    });
    assert!(violated(&st).contains(&Inv::D1));
}

/// N1 [step] (1.11, §8.2): a `from_user` with no target, from a session whose only hold carries an
/// unsettled request of its person, must clarify the newest; one routed otherwise is rejected.
#[test]
fn n1_rejects_a_follow_up_not_routed_as_a_clarification() {
    use crate::helpers::user::tests::from_user_cmd;
    use crate::state::Routing;
    let (mut w, ctx, s) = setup();
    let c = class("worker");
    ok(w.call(&c, s, create("/refunds")));
    let first = ok(w.call(
        &c,
        s,
        from_user_cmd("retry refunds", Some("user"), None, None, None),
    ));
    let Reply::FromUser {
        routed: crate::reply::RoutedReply::Request { request, .. },
        ..
    } = first.reply
    else {
        panic!("a request: {:?}", first.reply)
    };
    let mut st = step(
        &mut w,
        &ctx,
        from_user_cmd("yes, 3 retries", None, None, None, None),
    );
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    let Ok(Done {
        reply: Reply::FromUser { entry, .. },
        ..
    }) = &st.out
    else {
        panic!("a from_user: {:?}", st.out)
    };
    assert_eq!(
        st.after.user_entries[entry].routing,
        Routing::Clarifies { request }
    );
    if let Some(u) = st.after.user_entries.get_mut(entry) {
        u.routing = Routing::NoAction;
    }
    assert!(violated(&st).contains(&Inv::N1));
}

/// S1 [step] (1.11, §14): the close of the second node of a duplicate pair must resolve the
/// pair's signal; one left open is rejected.
#[test]
fn s1_rejects_a_duplicate_signal_left_open_after_both_close() {
    let (mut w, ctx, s) = setup();
    let c = class("worker");
    let mk = |w: &mut crate::testkit::World, p: &str| {
        created(&ok(w.call(
            &c,
            s,
            crate::apply::tests::create_with(p, |a| a.take = Some(false)),
        )))
        .0
    };
    let a = mk(&mut w, "/a");
    let b = mk(&mut w, "/b");
    let d = ok(w.call(
        &c,
        s,
        Cmd::Node(crate::cmd::NodeCmd::Link {
            at: crate::types::NodeRef::Id(b),
            rel: crate::types::LinkRel::Duplicate,
            to: crate::types::NodeRef::Id(a),
        }),
    ));
    let Reply::Linked {
        signal: Some(sig), ..
    } = d.reply
    else {
        panic!("a signal: {:?}", d.reply)
    };
    ok(w.call(&c, s, crate::apply::tests::close(a, Outcome::Done)));
    let mut st = step(&mut w, &ctx, crate::apply::tests::close(b, Outcome::Done));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    assert!(!st.after.nodes[&sig].is_open());
    let key = match &st.after.nodes[&sig].role {
        crate::state::Role::Signal(k) => *k,
        other => panic!("a signal node: {other:?}"),
    };
    if let Some(x) = st.after.nodes.get_mut(&sig) {
        x.life = crate::state::Life::Open;
    }
    st.after.signals.insert(key, sig);
    assert!(violated(&st).contains(&Inv::S1));
}

/// A key, normalised, for the tests below.
fn conv_key(k: &str) -> crate::types::Key {
    crate::types::Key {
        key: crate::types::KeyStr::new(k).unwrap(),
        nv: crate::types::NormVer(1),
    }
}

/// `talk request` of message `origin` in conversation `conv` on `n`.
fn request_in(n: NodeId, origin: &str, conv: &str) -> Cmd {
    Cmd::Talk(crate::cmd::TalkCmd::Request {
        at: crate::types::NodeRef::Id(n),
        text: text("why did the summary miss two tickets?"),
        origin: conv_key(origin),
        where_: Some(conv_key(conv)),
        requester: text("Bob"),
    })
}

/// Settled [step] (1.12, §10.3): a release with `settles` must record exactly one decision on its
/// node, after its checkpoint, listing them; one whose decision lists other requests, or a release
/// without `settles` that records one, is rejected.
#[test]
fn settled_rejects_a_release_whose_decision_is_not_its_settles() {
    let (mut w, ctx, s) = setup();
    let c = class("worker");
    let n = created(&ok(w.call(
        &c,
        s,
        crate::apply::tests::create_with("/fix", |a| {
            a.keys = alloc::vec![conv_key("chat:eng/T1")];
        }),
    )));
    let (n, t) = (n.0, n.1.unwrap());
    let r = match ok(w.call(&c, s, request_in(n, "chat:eng/T1/M1", "chat:eng/T1"))).reply {
        Reply::Request { request, .. } => request,
        other => panic!("a request: {other:?}"),
    };
    ok(w.call(
        &c,
        s,
        Cmd::Talk(crate::cmd::TalkCmd::Said {
            at: crate::types::NodeRef::Id(n),
            where_: Some(conv_key("chat:eng/T1")),
            text: text("fixed in PR #412"),
            kind: crate::types::SaidKind::Result,
            corrects: None,
            seen: None,
        }),
    ));
    let settling = || {
        let mut x = release(t, ReleaseReason::Blocked);
        if let Cmd::Hold(crate::cmd::HoldCmd::Release { settles, .. }) = &mut x {
            settles.insert(r);
        }
        x
    };
    let mut st = step(&mut w, &ctx, settling());
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    let decision: Vec<Event> = st
        .events
        .iter()
        .filter(|e| {
            matches!(
                e.ev,
                crate::event::Ev::Appended {
                    body: crate::event::EntryBody::Decision { .. },
                    ..
                }
            )
        })
        .cloned()
        .collect();
    assert_eq!(decision.len(), 1);
    for e in &mut st.events {
        if let crate::event::Ev::Appended {
            body: crate::event::EntryBody::Decision { settles, .. },
            ..
        } = &mut e.ev
        {
            settles.insert(crate::types::Eid(9_999));
        }
    }
    assert!(violated(&st).contains(&Inv::Settled));
    // The same release without `settles`, whose step records the decision all the same.
    let mut st = step(&mut w, &ctx, release(t, ReleaseReason::Blocked));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    st.events.extend(decision);
    assert!(violated(&st).contains(&Inv::Settled));
}

/// E1 [step] (1.12, §9.3): a question to a person becomes withdrawn only by a retract that names
/// it, while it is open; one withdrawn by another step is rejected.
#[test]
fn e1_rejects_a_question_withdrawn_without_its_retract() {
    let (mut w, ctx, s) = setup();
    let c = class("worker");
    let n = created(&ok(w.call(
        &c,
        s,
        crate::apply::tests::create_with("/support", |a| {
            a.keys = alloc::vec![conv_key("chat:a/1")];
            a.take = Some(false);
        }),
    )))
    .0;
    let ask = Cmd::Talk(crate::cmd::TalkCmd::Ask {
        at: crate::types::NodeRef::Id(n),
        text: text("which tickets?"),
        to: crate::cmd::AskTo::Person {
            where_: None,
            blocking: false,
        },
        again: false,
    });
    let q = match ok(w.call(&c, s, ask)).reply {
        Reply::Asked { question, .. } => question,
        other => panic!("asked: {other:?}"),
    };
    let mut st = step(&mut w, &ctx, create("/other"));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    if let Some(pq) = st.after.pquestions.get_mut(&q) {
        pq.state = crate::state::PqState::Withdrawn {
            by: crate::types::Eid(1),
        };
    }
    assert!(violated(&st).contains(&Inv::E1));
    // The retract that names it, while it is open, passes.
    let st = step(
        &mut w,
        &ctx,
        Cmd::Talk(crate::cmd::TalkCmd::Retract {
            target: crate::cmd::RetractArg::Entry { eid: q, node: n },
            text: text("wrong person"),
            reason: text("not the author"),
        }),
    );
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    assert!(matches!(
        st.after.pquestions[&q].state,
        crate::state::PqState::Withdrawn { .. }
    ));
}
