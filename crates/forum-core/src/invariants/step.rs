//! The step invariants of §26 (design D.9.2, the rows of kind St): `check_step` for a command and
//! `check_fire` for one firing. K7, N5, C1, and L1's half for series are WP-D4's, in
//! `step_keys.rs`; the Recheck row of `check_fire`'s table is WP-D5b's, in `step_people.rs`.
//!
//! Like the state checks, they compute from the base state and the events, through the naive
//! definitions of `spec` (D.9.1) and the few predicates below, written from the specification
//! (`version_bumps`, `agent_made`, `people_type`): no code of `apply`, `settle`, or `evolve`'s
//! incremental maintenance is shared.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use super::{Inv, Violation, ViolationDetail};
use crate::cmd::{AskTo, Cmd, Ctx, HoldCmd, NodeCmd, RetractArg, SessionCtx, TalkCmd, UserAsk};
use crate::error::{Error, ErrorCode, Hint, LimitKind};
use crate::event::{EntryBody, Ev, Event, NodePatch, QuestionBody, SystemNote};
use crate::reply::{Done, Reply};
use crate::spec;
use crate::state::{
    Author, DeadlineKey, DeadlineKind, DigestItem, DigestState, EntityRef, LeaseTerms, Life,
    Origin, Owner, PqState, SaidState, State, UqSource,
};
use crate::types::{
    Eid, EntryType, NodeId, Outcome, PEOPLE, PrincipalId, RequestId, Seq, SessionId, TEAMS, Time,
    ToUserKind, Token, Version,
};

/// Collected violations.
struct Out(Vec<Violation>);

impl Out {
    fn push(&mut self, inv: Inv, detail: ViolationDetail) {
        self.0.push(Violation { inv, detail });
    }

    fn check(&mut self, ok: bool, inv: Inv, detail: ViolationDetail) {
        if !ok {
            self.push(inv, detail);
        }
    }

    fn done(self) -> Result<(), Vec<Violation>> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(self.0)
        }
    }
}

fn text(t: &'static str) -> ViolationDetail {
    ViolationDetail::Text(t)
}

/// The step invariants of a command: H2, H3, H4, H5, O1, O2, O3, K7, C1, Settled, N5, R2, D1, U1,
/// T1, T4, L1, P2, and (1.12) E1's withdrawn questions (D.9.2), on `(before, ctx, cmd, out,
/// events, after)`.
///
/// # Errors
///
/// Every violation found.
pub fn check_step(
    before: &State,
    ctx: &Ctx,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    events: &[Event],
    after: &State,
) -> Result<(), Vec<Violation>> {
    let mut o = Out(Vec::new());
    let now = ctx.now.max(before.last_now);
    // L1, texts (A-114): answered before anything else, with no event and no change.
    if cmd.longest_text() > u64::from(before.params.text_max) {
        let refused = matches!(out, Err(e) if e.hint == Hint::Limit(LimitKind::TextTooLong));
        o.check(
            refused && events.is_empty() && after == before,
            Inv::L1,
            text("a text over text.max must be refused with nothing applied"),
        );
        return o.done();
    }
    t1_command(before, now, events, after, &mut o);
    o1(before, ctx, out, events, after, now, &mut o);
    h2_command(before, ctx, events, &mut o);
    h3(before, events, &mut o);
    h4(before, ctx, cmd, out, events, after, now, &mut o);
    h5(before, events, &mut o);
    p2(before, events, after, now, &mut o);
    o2(before, Some((cmd, out)), events, after, &mut o);
    o3(before, events, after, &mut o);
    settled(before, events, after, &mut o);
    release_decision(cmd, out, events, &mut o);
    e1_withdrawn(before, Some(cmd), after, &mut o);
    r2(before, Some((cmd, out)), events, now, &mut o);
    d1(before, events, &mut o);
    u1(before, Some(cmd), events, after, &mut o);
    // N1 [step] (1.11): a `from_user` with no target clarifies its person's request by default.
    o.check(
        user::n1(before, cmd, out, after, now),
        Inv::N1,
        text(
            "a from_user with no target must clarify its person's newest request on the only hold",
        ),
    );
    keys::s1_step(before, events, after, &mut o);
    t4_command(before, cmd, after, now, &mut o);
    l1_children(before, events, after, &mut o);
    keys::k7(before, events, after, &mut o);
    keys::n5(before, ctx, cmd, out, events, after, &mut o);
    keys::c1(before, Some((cmd, out)), events, after, &mut o);
    keys::l1_series(events, after, &mut o);
    o.done()
}

/// The step invariants of one firing of `key`: T1, T4, H2, O2, O3, K7, C1, Settled, R2, D1, U1,
/// and (1.12) E1's withdrawn questions, and that its events are those of the table of D.8.2 for
/// the arms that exist (D.9.1).
///
/// # Errors
///
/// Every violation found.
pub fn check_fire(
    before: &State,
    key: &DeadlineKey,
    events: &[Event],
    after: &State,
) -> Result<(), Vec<Violation>> {
    let mut o = Out(Vec::new());
    let d = ViolationDetail::Deadline(key.clone());
    // T4: the key was the queue's minimum, it is gone afterwards, and no new key lies at or before
    // it.
    o.check(before.ix.deadlines.first() == Some(key), Inv::T4, d.clone());
    o.check(!after.ix.deadlines.contains(key), Inv::T4, d.clone());
    for k in after.ix.deadlines.difference(&before.ix.deadlines) {
        o.check(k.at > key.at, Inv::T4, ViolationDetail::Deadline(k.clone()));
    }
    // T1: every event at the key's own time, the first its `Fired`.
    o.check(key.at >= before.last_now, Inv::T1, d.clone());
    o.check(events.iter().all(|e| e.at == key.at), Inv::T1, d.clone());
    o.check(
        after.last_now == before.last_now.max(key.at),
        Inv::T1,
        d.clone(),
    );
    let fired = matches!(events.first(), Some(Event { ev: Ev::Fired { kind, owner }, .. })
        if *kind == key.kind && *owner == key.owner);
    o.check(fired, Inv::T4, d.clone());
    fire_shape(before, key, events, &mut o);
    // H2: a session retires holding nothing.
    for e in events {
        if let Ev::SessionRetired { session } = &e.ev {
            let holds = before.holds.values().any(|h| h.session == *session);
            o.check(!holds, Inv::H2, ViolationDetail::Session(*session));
        }
    }
    o2(before, None, events, after, &mut o);
    o3(before, events, after, &mut o);
    settled(before, events, after, &mut o);
    e1_withdrawn(before, None, after, &mut o);
    r2(before, None, events, key.at, &mut o);
    d1(before, events, &mut o);
    u1(before, None, events, after, &mut o);
    keys::k7(before, events, after, &mut o);
    keys::c1(before, None, events, after, &mut o);
    keys::s1_step(before, events, after, &mut o);
    o.done()
}

/// The events of the table of D.8.2 for WP-D3's arms, WP-D5a's UserQuestion arm, WP-D5b's Recheck
/// arm, and WP-D6's occurrence arms: `Fired`, then exactly these.
fn fire_shape(before: &State, key: &DeadlineKey, events: &[Event], o: &mut Out) {
    let rest: Vec<&Ev> = events.iter().skip(1).map(|e| &e.ev).collect();
    let d = ViolationDetail::Deadline(key.clone());
    let ok = match (key.kind, &key.owner) {
        // The missed check after an occurrence's lease is WP-D6's (A-80).
        (DeadlineKind::Lease, Owner::Hold(t)) => recurring::lease(before, *t, &rest),
        (DeadlineKind::Wait | DeadlineKind::Due, Owner::Node(_))
        | (DeadlineKind::Commitment, Owner::Entry(_)) => rest.is_empty(),
        // WP-D5a's arm (CS-2).
        (DeadlineKind::UserQuestion, Owner::Entry(q)) => {
            user::fire_shape(before, *q, key.at, &rest)
        }
        (DeadlineKind::RefusalExpiry, Owner::Refusal(n, c)) => {
            rest.len() == 1
                && matches!(rest.first(), Some(Ev::RefusalExpired { node, class }) if node == n && class == c)
        }
        (DeadlineKind::Pause, Owner::Node(p)) => {
            rest.len() == 2
                && matches!(rest.first(), Some(Ev::PauseExpired { scope }) if scope == p)
                && matches!(rest.get(1), Some(Ev::Appended { node, body: EntryBody::System(SystemNote::PauseExpired), .. }) if node == p)
        }
        (DeadlineKind::Silence, Owner::Session(x)) => {
            let pending: Vec<Eid> = before
                .saids
                .iter()
                .filter(|(_, a)| a.session == *x && a.state == SaidState::Pending)
                .map(|(id, _)| *id)
                .collect();
            let got: Vec<Eid> = rest
                .iter()
                .filter_map(|e| {
                    if let Ev::SaidUnconfirmed { said } = e {
                        Some(*said)
                    } else {
                        None
                    }
                })
                .collect();
            got == pending && got.len() == rest.len()
        }
        (DeadlineKind::Retire, Owner::Session(x)) => {
            rest.len() == 1
                && matches!(rest.first(), Some(Ev::SessionRetired { session }) if session == x)
        }
        (DeadlineKind::Recheck, Owner::Entry(q)) => people::recheck(before, *q, key.at, &rest),
        // WP-D6's arms (1.4, R10; R2).
        (DeadlineKind::OccurrenceSlot, Owner::Node(o)) => recurring::slot(before, *o, &rest),
        (DeadlineKind::OccurrenceWindow, Owner::Node(o)) => recurring::window(before, *o, &rest),
        // The class profiles' expiry, whose `e1_check` is WP-D5a's.
        _ => true,
    };
    o.check(ok, Inv::T4, d);
}

// ------------------------------------------------------------------------------------------------
// Time

/// T1 for a command: every event at `max(ctx.now, before.last_now)`, and `last_now` after at their
/// maximum.
fn t1_command(before: &State, now: Time, events: &[Event], after: &State, o: &mut Out) {
    o.check(
        events.iter().all(|e| e.at == now),
        Inv::T1,
        text("a command's events carry its clamped now"),
    );
    let want = if events.is_empty() {
        before.last_now
    } else {
        before.last_now.max(now)
    };
    o.check(
        after.last_now == want,
        Inv::T1,
        text("last_now ends at the events' maximum"),
    );
}

/// T4 for a command: no new key before `now`; a key at exactly `now` only for a user question
/// whose explicit `due` was clamped (A-68), or for the Silence key of a session whose last hold the
/// command ended after its `silence_at` (1.4, D.4.8): the post-settle fires it.
fn t4_command(before: &State, cmd: &Cmd, after: &State, now: Time, o: &mut Out) {
    for k in after.ix.deadlines.difference(&before.ix.deadlines) {
        let at_now = k.at == now
            && match (k.kind, &k.owner) {
                (DeadlineKind::UserQuestion, _) => clamped_due(cmd, now),
                (DeadlineKind::Silence, Owner::Session(x)) => {
                    last_hold_ended(before, after, *x, now)
                }
                _ => false,
            };
        o.check(
            k.at > now || at_now,
            Inv::T4,
            ViolationDetail::Deadline(k.clone()),
        );
    }
}

/// The command asks the user with an explicit `due` at or before `now`, which A-68 clamps to it.
fn clamped_due(cmd: &Cmd, now: Time) -> bool {
    matches!(cmd, Cmd::Talk(TalkCmd::Ask {
        to: AskTo::User(UserAsk { due: Some(w), .. }),
        ..
    }) if w.resolve(now) <= now)
}

/// Session `x` held a hold before the step and none after it, is not silent, and its `silence_at`
/// has passed: its Silence key lies at `now` (D.4.8's floor at `last_now`).
fn last_hold_ended(before: &State, after: &State, x: SessionId, now: Time) -> bool {
    after
        .sessions
        .get(&x)
        .is_some_and(|s| !s.silent && s.silence_at <= now)
        && before.holds.values().any(|h| h.session == x)
        && !after.holds.values().any(|h| h.session == x)
}

// ------------------------------------------------------------------------------------------------
// Retries (O1)

fn o1(
    before: &State,
    ctx: &Ctx,
    out: &Result<Done, Error>,
    events: &[Event],
    after: &State,
    now: Time,
    o: &mut Out,
) {
    let system = ctx.session == SessionCtx::System;
    if system {
        o.check(
            !events.iter().any(|e| matches!(e.ev, Ev::OpRecorded { .. })),
            Inv::O1,
            text("no OpRecorded under System"),
        );
        return;
    }
    let Some(op) = &ctx.op else {
        o.check(
            !events.iter().any(|e| matches!(e.ev, Ev::OpRecorded { .. })),
            Inv::O1,
            text("no OpRecorded without an op_id"),
        );
        return;
    };
    let key = (ctx.class.clone(), op.id.clone());
    if let Some(r) = before.ops.get(&key).filter(|r| r.expires_at > now) {
        let same = match ctx.session {
            SessionCtx::New => true,
            SessionCtx::Existing(x) => x == r.session,
            SessionCtx::System => false,
        };
        let ok = if r.digest == op.digest && same {
            matches!(out, Ok(d) if d.reply == Reply::Replay)
        } else {
            matches!(out, Err(e) if e.code == ErrorCode::OpIdConflict)
        };
        o.check(
            ok && events.is_empty() && after == before,
            Inv::O1,
            text("a live record answers with no events"),
        );
        return;
    }
    if events.is_empty() {
        o.check(
            after.ops == before.ops,
            Inv::O1,
            text("a call that appended nothing records nothing"),
        );
        return;
    }
    let last = events.last().map(|e| &e.ev);
    let recorded = matches!(last, Some(Ev::OpRecorded { class, op: id, digest, outcome, .. })
        if *class == ctx.class && *id == op.id && *digest == op.digest && outcome == out);
    o.check(recorded, Inv::O1, text("OpRecorded is the last event"));
    // The call's session: the one it names, or the one its `SessionOpened` opened.
    let session = match ctx.session {
        SessionCtx::Existing(x) => Some(x),
        SessionCtx::New => events.iter().find_map(|e| {
            if let Ev::SessionOpened { session, .. } = &e.ev {
                Some(*session)
            } else {
                None
            }
        }),
        SessionCtx::System => None,
    };
    let rec = after.ops.get(&key);
    o.check(
        rec.is_some_and(|r| {
            r.digest == op.digest
                && Some(r.session) == session
                && r.expires_at == now.plus(before.params.op_retain)
        }),
        Inv::O1,
        text("the record stores the digest, the session, and the expiry"),
    );
}

// ------------------------------------------------------------------------------------------------
// Holds

/// H2 for a command: every `HoldIssued` is preceded in the step by a `Touched` or the
/// `SessionOpened` of its session (CS-8).
fn h2_command(_before: &State, _ctx: &Ctx, events: &[Event], o: &mut Out) {
    let mut touched: BTreeSet<crate::types::SessionId> = BTreeSet::new();
    for e in events {
        if let Ev::Touched { session, .. } | Ev::SessionOpened { session, .. } = &e.ev {
            touched.insert(*session);
        }
        if let Ev::HoldIssued { session, token, .. } = &e.ev {
            o.check(
                touched.contains(session),
                Inv::H2,
                ViolationDetail::Hold(*token),
            );
        }
        if let Ev::SessionRetired { session } = &e.ev {
            o.push(Inv::H2, ViolationDetail::Session(*session));
        }
    }
}

/// H3: every `HoldIssued` has its node's fence plus one, counted through the step.
fn h3(before: &State, events: &[Event], o: &mut Out) {
    let mut fences: BTreeMap<NodeId, u64> = BTreeMap::new();
    for e in events {
        if let Ev::HoldIssued { token, .. } = &e.ev {
            let cur = fences
                .get(&token.node)
                .copied()
                .unwrap_or_else(|| before.nodes.get(&token.node).map_or(0, |x| x.fence.0));
            o.check(
                token.fence.0 == cur.saturating_add(1),
                Inv::H3,
                ViolationDetail::Hold(*token),
            );
            fences.insert(token.node, token.fence.0);
        }
    }
}

/// H5: every `HoldIssued` carries the lease terms of the params in force (A-102).
fn h5(before: &State, events: &[Event], o: &mut Out) {
    let p = &before.params;
    let want = LeaseTerms {
        first: p.lease_first,
        default: p.lease_default,
        max: p.lease_max,
        ceiling: p.lease_ceiling,
        renew_max: p.renew_max,
    };
    for e in events {
        if let Ev::HoldIssued { token, terms, .. } = &e.ev {
            o.check(*terms == want, Inv::H5, ViolationDetail::Hold(*token));
        }
    }
}

/// P2: no hold is issued under a pause: one of `before` over the node or an ancestor, its ancestry
/// read in `after`, so that a node the step created under a paused scope is covered too.
fn p2(before: &State, events: &[Event], after: &State, now: Time, o: &mut Out) {
    for e in events {
        if let Ev::HoldIssued { token, .. } = &e.ev {
            let paused = spec::ancestors_or_self(after, token.node)
                .iter()
                .any(|a| before.pauses.get(a).is_some_and(|p| p.until > now));
            o.check(!paused, Inv::P2, ViolationDetail::Hold(*token));
        }
    }
}

/// The token a write carries, and the node it must hold (D.8.24 step 3).
fn write_token(before: &State, cmd: &Cmd) -> Option<(Token, Option<NodeId>)> {
    if let Cmd::Hold(
        HoldCmd::Checkpoint { token, .. }
        | HoldCmd::Renew { token, .. }
        | HoldCmd::Release { token, .. },
    ) = cmd
    {
        return Some((*token, Some(token.node)));
    }
    if let Cmd::Node(
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
    ) = cmd
    {
        return Some((*token, spec::resolve(before, at)));
    }
    None
}

/// Whether the session of the call resolves in `before` (or the call opens one).
fn session_ok(before: &State, ctx: &Ctx) -> bool {
    match ctx.session {
        SessionCtx::New => true,
        SessionCtx::Existing(x) => before
            .sessions
            .get(&x)
            .is_some_and(|s| s.class == ctx.class),
        SessionCtx::System => false,
    }
}

/// The resident entities whose leaving time has come by `now`: they may leave the maps in any
/// event of the step (D.4.12, CS-17).
fn leavers(before: &State, now: Time) -> BTreeSet<EntityRef> {
    before
        .leaving
        .iter()
        .filter(|(_, t)| **t <= now)
        .map(|(x, _)| *x)
        .collect()
}

/// `a ⊆ b` with equal values, and every key of `b` missing in `a` allowed to leave.
fn equal_but_left<K: Ord + Copy, V: PartialEq>(
    before: &BTreeMap<K, V>,
    after: &BTreeMap<K, V>,
    left: impl Fn(K) -> bool,
) -> bool {
    after
        .iter()
        .all(|(k, v)| before.get(k).is_some_and(|w| w == v))
        && before.keys().all(|k| after.contains_key(k) || left(*k))
}

/// H4: a write with a token that is not a live hold on its target, past the text check, O1, the
/// session check, and the target's existence, is `stale_token` with exactly one `late` entry on the
/// target and no other change of the base state (A-105, A-114).
#[allow(clippy::too_many_arguments, reason = "the arguments of check_step")]
fn h4(
    before: &State,
    ctx: &Ctx,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    events: &[Event],
    after: &State,
    now: Time,
    o: &mut Out,
) {
    let Some((token, target)) = write_token(before, cmd) else {
        return;
    };
    let live_record = ctx.session != SessionCtx::System
        && ctx.op.as_ref().is_some_and(|op| {
            before
                .ops
                .get(&(ctx.class.clone(), op.id.clone()))
                .is_some_and(|r| r.expires_at > now)
        });
    let Some(target) = target.filter(|t| before.nodes.contains_key(t)) else {
        o.check(
            !events.iter().any(|e| {
                matches!(
                    &e.ev,
                    Ev::Appended {
                        body: EntryBody::Late { .. },
                        ..
                    }
                )
            }),
            Inv::H4,
            text("no late entry for a missing target"),
        );
        return;
    };
    if live_record || !session_ok(before, ctx) {
        o.check(
            !events.iter().any(|e| {
                matches!(
                    &e.ev,
                    Ev::Appended {
                        body: EntryBody::Late { .. },
                        ..
                    }
                )
            }),
            Inv::H4,
            text("no late entry before O1 and the session"),
        );
        return;
    }
    let live = token.node == target
        && before
            .holds
            .get(&token)
            .is_some_and(|h| now < h.lease_until);
    if live {
        o.check(
            !matches!(out, Err(e) if e.code == ErrorCode::StaleToken),
            Inv::H4,
            ViolationDetail::Hold(token),
        );
        return;
    }
    let d = ViolationDetail::Hold(token);
    o.check(
        matches!(out, Err(e) if e.code == ErrorCode::StaleToken),
        Inv::H4,
        d.clone(),
    );
    let new = ctx.session == SessionCtx::New;
    let lates: Vec<&Event> = events
        .iter()
        .filter(|e| {
            matches!(
                &e.ev,
                Ev::Appended {
                    body: EntryBody::Late { .. },
                    ..
                }
            )
        })
        .collect();
    let late_ok = lates.len() == 1
        && matches!(&lates.first().map(|e| &e.ev), Some(Ev::Appended { node, body: EntryBody::Late { token: t, .. }, .. }) if *node == target && *t == token);
    o.check(late_ok, Inv::H4, d.clone());
    let others_ok = events.iter().all(|e| {
        matches!(
            e.ev,
            Ev::Appended {
                body: EntryBody::Late { .. },
                ..
            } | Ev::PrincipalSeen { .. }
                | Ev::SessionOpened { .. }
                | Ev::OpRecorded { .. }
        )
    });
    o.check(others_ok, Inv::H4, d.clone());
    // The base state: equal but the late entry's effects, the prunes of D.4.12 and CS-8 (what left
    // memory, and the notes and agent questions whose `until` came), and a first call's session.
    let left = leavers(before, now);
    // The highest seq of each node's people entries whose metadata the step's prune dropped: the
    // node's `people_left` rises to it (D.4.12). O3 checks that each of them was due to leave.
    let mut people_gone: BTreeMap<NodeId, Seq> = BTreeMap::new();
    for (e, m) in &before.entries {
        if !after.entries.contains_key(e) && people_type(m.ty) {
            let s = people_gone.entry(m.node).or_insert(m.seq);
            *s = (*s).max(m.seq);
        }
    }
    let mut nodes_ok = true;
    for (n, x) in &after.nodes {
        let Some(y) = before.nodes.get(n) else {
            nodes_ok = false;
            continue;
        };
        let mut y = y.clone();
        if *n == target {
            y.seq = x.seq;
        }
        y.left = x.left;
        if let Some(&s) = people_gone.get(n) {
            y.people_left = y.people_left.max(s);
        }
        nodes_ok &= *x == y;
    }
    nodes_ok &= before.nodes.len() == after.nodes.len();
    o.check(nodes_ok, Inv::H4, d.clone());
    let until_came = |e: Eid| before.notes.get(&e).is_some_and(|x| x.until <= now);
    // `digest_left` rises by the pending digest entries that left memory (CS-5, D.4.12).
    let mut digest_left = before.digest_left.clone();
    for (x, de) in &before.digest {
        if de.state == DigestState::Pending && !after.digest.contains_key(x) {
            let c = digest_left.entry(de.to).or_insert(0);
            *c = c.saturating_add(1);
        }
    }
    // A first call interns the principals its `PrincipalSeen` events name, and nothing else.
    let mut principal_ids = before.principal_ids.clone();
    if new {
        for e in events {
            if let Ev::PrincipalSeen { id, key } = &e.ev {
                principal_ids.insert(key.clone(), *id);
            }
        }
    }
    let maps_ok = after.holds == before.holds
        && after.params == before.params
        && after.principal_ids == principal_ids
        && after.digest_left == digest_left
        && after.user_msgs == before.user_msgs
        && equal_but_left(&before.entries, &after.entries, |e| {
            may_leave(before, &left, e, now)
        })
        && equal_but_left(&before.notes, &after.notes, until_came)
        && equal_but_left(&before.aquestions, &after.aquestions, until_came)
        && equal_but_left(&before.requests, &after.requests, |k| {
            left.contains(&EntityRef::Request(k))
        })
        && equal_but_left(&before.pquestions, &after.pquestions, |k| {
            left.contains(&EntityRef::PQuestion(k))
        })
        && equal_but_left(&before.uquestions, &after.uquestions, |k| {
            left.contains(&EntityRef::UQuestion(k))
        })
        && equal_but_left(&before.commitments, &after.commitments, |k| {
            left.contains(&EntityRef::Commitment(k))
        })
        && equal_but_left(&before.saids, &after.saids, |k| {
            left.contains(&EntityRef::Said(k))
        })
        && equal_but_left(&before.decisions, &after.decisions, |k| {
            left.contains(&EntityRef::Decision(k))
        })
        && equal_but_left(&before.objections, &after.objections, |k| {
            left.contains(&EntityRef::Objection(k))
        })
        && equal_but_left(&before.user_entries, &after.user_entries, |k| {
            left.contains(&EntityRef::UserEntry(k))
        })
        && equal_but_left(&before.digest, &after.digest, |k| {
            left.contains(&EntityRef::Digest(k))
        })
        && after.approvals == before.approvals
        && after.retracted == before.retracted
        && after.refusals == before.refusals
        && after.classes == before.classes
        && after.signals == before.signals
        && after.pauses == before.pauses
        && after.names == before.names
        && after.aliases == before.aliases;
    o.check(maps_ok, Inv::H4, d.clone());
    let sessions_ok = if new {
        let opened = events
            .iter()
            .filter(|e| matches!(e.ev, Ev::SessionOpened { .. }))
            .count();
        before
            .sessions
            .iter()
            .all(|(k, v)| after.sessions.get(k) == Some(v))
            && after.sessions.len() == before.sessions.len().saturating_add(opened)
            && opened <= 1
    } else {
        after.sessions == before.sessions
    };
    o.check(sessions_ok, Inv::H4, d);
}

// ------------------------------------------------------------------------------------------------
// O2 and O3

/// Which counters a `NodeUpdated` moves, read off §26 O2 and §5 (A-3, CS-34): `version` when the
/// title, body, fields, files, or status change; `content_version` (to the new `version`) when the
/// title, body, fields, or files change; `digest_version` when the digest changes. A patch names
/// only the fields whose value differs (D.7.2), so a field present in it changed. Written here, not
/// shared with `evolve` (D.9.1).
fn version_bumps(p: &NodePatch) -> (bool, bool, bool) {
    let content = p.title.is_some() || p.body.is_some() || p.fields.is_some() || p.files.is_some();
    (content || p.status.is_some(), content, p.digest.is_some())
}

/// The counters' rule of O2 (§26), per node and per event, folded through the step, for a command
/// or a firing. For a command, also the clauses on its own `version` and `digest_version`: a
/// digest changes only in an `update` whose `digest_version` is the node's in `before`, a
/// successful `update` (not a replay of a stored answer, O1) named the node's `version` and, if it
/// gave one, its `digest_version` ("an edit with a stale `version` or a stale `digest_version`
/// changes nothing"), and a `stale_version` outcome appends nothing of the command.
fn o2(
    before: &State,
    cmd: Option<(&Cmd, &Result<Done, Error>)>,
    events: &[Event],
    after: &State,
    o: &mut Out,
) {
    // (version, digest_version, seq, fence, content_version) per node, from `before` or creation.
    let mut c: BTreeMap<NodeId, (u64, u64, u64, u64, u64)> = BTreeMap::new();
    let get = |c: &BTreeMap<NodeId, (u64, u64, u64, u64, u64)>, n: NodeId| {
        c.get(&n).copied().or_else(|| {
            before.nodes.get(&n).map(|x| {
                (
                    x.version.0,
                    x.digest_version.0,
                    x.seq.0,
                    x.fence.0,
                    x.content_version.0,
                )
            })
        })
    };
    for e in events {
        if let Ev::NodeCreated { id, .. } = &e.ev {
            c.insert(*id, (1, 1, 0, 0, 1));
        }
        if let Ev::KeyAdded { node, .. }
        | Ev::KeyRemoved { node, .. }
        | Ev::LinkAdded { node, .. }
        | Ev::NodeClosed { node, .. }
        | Ev::NodeReopened { node } = &e.ev
            && let Some(mut v) = get(&c, *node)
        {
            v.0 = v.0.saturating_add(1);
            c.insert(*node, v);
        }
        if let Ev::NodeUpdated { node, patch } = &e.ev
            && let Some(mut v) = get(&c, *node)
        {
            let (version, content, digest) = version_bumps(patch);
            if version {
                v.0 = v.0.saturating_add(1);
            }
            if digest {
                v.1 = v.1.saturating_add(1);
                let dv = if let Some((Cmd::Node(NodeCmd::Update { digest_version, .. }), _)) = cmd {
                    *digest_version
                } else {
                    None
                };
                o.check(
                    dv.is_some() && dv == before.nodes.get(node).map(|x| x.digest_version),
                    Inv::O2,
                    ViolationDetail::Node(*node),
                );
            }
            if content {
                v.4 = v.0;
            }
            c.insert(*node, v);
        }
        if let Ev::Appended { node, .. } = &e.ev
            && let Some(mut v) = get(&c, *node)
        {
            v.2 = v.2.saturating_add(1);
            c.insert(*node, v);
        }
        if let Ev::HoldIssued { token, .. } = &e.ev
            && let Some(mut v) = get(&c, token.node)
        {
            v.3 = v.3.saturating_add(1);
            c.insert(token.node, v);
        }
    }
    for (n, x) in &after.nodes {
        let Some(v) = get(&c, *n) else { continue };
        let ok = x.version == Version(v.0)
            && x.digest_version == Version(v.1)
            && x.seq.0 == v.2
            && x.fence.0 == v.3
            && x.content_version == Version(v.4);
        o.check(ok, Inv::O2, ViolationDetail::Node(*n));
    }
    let Some((cmd, out)) = cmd else {
        return;
    };
    if matches!(out, Err(e) if e.code == ErrorCode::StaleVersion) {
        let ok = events.iter().all(|e| {
            matches!(
                e.ev,
                Ev::PrincipalSeen { .. } | Ev::SessionOpened { .. } | Ev::OpRecorded { .. }
            )
        });
        o.check(ok, Inv::O2, text("stale_version appends nothing else"));
    }
    if let (
        Cmd::Node(NodeCmd::Update {
            at,
            version,
            digest_version,
            ..
        }),
        Ok(d),
    ) = (cmd, out)
        && d.reply != Reply::Replay
    {
        let n = spec::resolve(before, at).and_then(|n| before.nodes.get(&n));
        o.check(
            n.is_some_and(|x| {
                *version == x.version && digest_version.is_none_or(|d| d == x.digest_version)
            }),
            Inv::O2,
            text("an update with a stale version or digest_version changes nothing"),
        );
    }
}

/// O3: entries are immutable once inserted, except metadata that left memory by D.4.12's prune;
/// every new entry was appended or restored by the step; `retracted` only grows; the event count
/// rises by the step's events.
fn o3(before: &State, events: &[Event], after: &State, o: &mut Out) {
    let last = events.iter().map(|e| e.at).max().unwrap_or(before.last_now);
    let left = leavers(before, last);
    for (e, m) in &before.entries {
        match after.entries.get(e) {
            Some(n) => o.check(n == m, Inv::O3, ViolationDetail::Entry(*e)),
            None => o.check(
                may_leave(before, &left, *e, last),
                Inv::O3,
                ViolationDetail::Entry(*e),
            ),
        }
    }
    let mut made: BTreeSet<Eid> = BTreeSet::new();
    for e in events {
        if let Ev::Appended { eid, .. } = &e.ev {
            made.insert(*eid);
        }
        if let Ev::Restored { .. } = &e.ev {
            made.extend(after.entries.keys().copied());
        }
    }
    for e in after.entries.keys() {
        o.check(
            before.entries.contains_key(e) || made.contains(e),
            Inv::O3,
            ViolationDetail::Entry(*e),
        );
    }
    o.check(
        before.retracted.is_subset(&after.retracted),
        Inv::O3,
        text("retracted only grows"),
    );
    let n = u64::try_from(events.len()).unwrap_or(u64::MAX);
    o.check(
        after.counters.events == before.counters.events.saturating_add(n),
        Inv::O3,
        text("the event count rises by the step's events"),
    );
}

/// Whether the metadata of entry `e` may leave memory in a step whose events end at `last`
/// (D.4.12, CS-17): its own leaving time, or that of the entity born from it, has come (`left`,
/// the leavers by `last`), or it is the entry of a note, or of an agent question, which leaves with
/// its note, whose `until` has come.
fn may_leave(before: &State, left: &BTreeSet<EntityRef>, e: Eid, last: Time) -> bool {
    [
        EntityRef::Meta(e),
        EntityRef::Request(e),
        EntityRef::PQuestion(e),
        EntityRef::UQuestion(e),
        EntityRef::Commitment(e),
        EntityRef::Said(e),
        EntityRef::Decision(e),
        EntityRef::Objection(e),
        EntityRef::UserEntry(e),
        EntityRef::Digest(e),
    ]
    .iter()
    .any(|x| left.contains(x))
        || before.notes.get(&e).is_some_and(|x| x.until <= last)
}

/// The entry types whose metadata counts toward a node's `people_left` (D.4.10, D.4.12): the
/// entries of people and agents talking, request, `said`, `heard`, question, and answer (§9.2's
/// `moved` scans them).
fn people_type(ty: EntryType) -> bool {
    matches!(
        ty,
        EntryType::Request
            | EntryType::Said
            | EntryType::Heard
            | EntryType::Question
            | EntryType::Answer
    )
}

/// The origins of the nodes the child limit counts: made by an agent, directly or as a missing
/// ancestor (CS-10).
fn agent_made(o: Origin) -> bool {
    matches!(o, Origin::Agent(_) | Origin::Ancestor(_))
}

// ------------------------------------------------------------------------------------------------
// Requests, missed occurrences, the digest, user questions

/// Settled (step half): no `settled` becomes `None`; a `RequestAnswered` by a `to_user` follows the
/// step's `Appended(ToUser)` of kind `result` to the requester's person (CS-4, A-23).
fn settled(before: &State, events: &[Event], after: &State, o: &mut Out) {
    for (r, x) in &before.requests {
        if x.settled.is_some()
            && let Some(y) = after.requests.get(r)
        {
            o.check(
                y.settled.is_some(),
                Inv::Settled,
                ViolationDetail::Entry(*r),
            );
        }
    }
    let mut to_users: BTreeMap<Eid, (ToUserKind, PrincipalId)> = BTreeMap::new();
    for e in events {
        if let Ev::Appended {
            eid,
            body: EntryBody::ToUser {
                kind, principal, ..
            },
            ..
        } = &e.ev
        {
            to_users.insert(*eid, (*kind, *principal));
        }
        if let Ev::RequestAnswered { request, by } = &e.ev {
            let is_to_user = before
                .entries
                .get(by)
                .is_some_and(|m| m.ty == EntryType::ToUser)
                || to_users.contains_key(by);
            if !is_to_user {
                continue;
            }
            let ok = to_users.get(by).is_some_and(|(kind, p)| {
                *kind == ToUserKind::Result
                    && before.requests.get(request).is_some_and(|r| {
                        matches!(r.requester, crate::types::Requester::User(q)
                            if spec::same_person(before, *p, q))
                    })
            });
            o.check(ok, Inv::Settled, ViolationDetail::Entry(*request));
        }
    }
}

/// Settled, 1.12's step clause (§10.3, §26): a `hold release` records a decision that lists
/// requests only when its `settles` is non-empty, and then exactly one, on its node, after its
/// `checkpoint` entry, listing exactly its `settles`: without it, a release settles nothing. A step
/// with no release entry (a refusal, or a stored `op_id` answer, O1) is not one.
fn release_decision(cmd: &Cmd, out: &Result<Done, Error>, events: &[Event], o: &mut Out) {
    let Cmd::Hold(HoldCmd::Release { token, settles, .. }) = cmd else {
        return;
    };
    if out.is_err() {
        return;
    }
    let Some(checkpoint) = events.iter().find_map(|e| {
        if let Ev::Appended {
            eid,
            body:
                EntryBody::Checkpoint {
                    token: t,
                    release: Some(_),
                    ..
                },
            ..
        } = &e.ev
            && t == token
        {
            Some(*eid)
        } else {
            None
        }
    }) else {
        return;
    };
    let decisions: Vec<(Eid, NodeId, &BTreeSet<RequestId>)> = events
        .iter()
        .filter_map(|e| {
            if let Ev::Appended {
                eid,
                node,
                body: EntryBody::Decision { settles: s, .. },
                ..
            } = &e.ev
                && !s.is_empty()
            {
                Some((*eid, *node, s))
            } else {
                None
            }
        })
        .collect();
    let ok = if settles.is_empty() {
        decisions.is_empty()
    } else {
        matches!(decisions.as_slice(), [(d, n, s)]
            if *d > checkpoint && *n == token.node && *s == settles)
    };
    o.check(ok, Inv::Settled, ViolationDetail::Node(token.node));
}

/// E1, 1.12's step clause (§9.3, §26): a question to a person becomes withdrawn only in a step of
/// a `talk retract` that names it, and only while it is open; a firing (`cmd` None) withdraws none.
/// A question that was not resident before the step is not judged: a command that names one that
/// left memory restores it as it was (D.4.12).
fn e1_withdrawn(before: &State, cmd: Option<&Cmd>, after: &State, o: &mut Out) {
    for (&q, pq) in &after.pquestions {
        if !matches!(pq.state, PqState::Withdrawn { .. }) {
            continue;
        }
        let Some(was) = before.pquestions.get(&q).map(|x| x.state) else {
            continue;
        };
        if matches!(was, PqState::Withdrawn { .. }) {
            continue;
        }
        let named = matches!(cmd, Some(Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry { eid, .. },
            ..
        })) if *eid == q);
        o.check(
            named && was == PqState::Open,
            Inv::E1,
            ViolationDetail::Entry(q),
        );
    }
}

/// R2 (step half): a `Missed` close only on a node with no checkpoint and no open intent in
/// `before`, and with no live hold left when it closes, and only by the server (§26 R2): its
/// `System(Closed)` entry, which the step appends before the `NodeClosed`, is the server's, and no
/// `node close {outcome: missed}` succeeds, other than as the replay of a stored answer (O1; §21
/// maps `invalid(ServerOutcome)` to R2).
///
/// "No live hold left": every hold on the node that is live in `before`, or issued earlier in the
/// step, has a `HoldEnded` earlier in the step. A `node move` by another session ends the holds on
/// an occurrence whose window has passed, and `maybe_missed` then closes it in the same step
/// (A-80, §12: "after any later hold end (expiry, release, or move)"), so "no live hold in
/// `before`" would refuse a legitimate step; the model's `step_R2` excludes the holds such a move
/// ends in the same way.
fn r2(
    before: &State,
    cmd: Option<(&Cmd, &Result<Done, Error>)>,
    events: &[Event],
    now: Time,
    o: &mut Out,
) {
    for (i, e) in events.iter().enumerate() {
        if let Ev::NodeClosed {
            node,
            outcome: Outcome::Missed,
            ..
        } = &e.ev
        {
            let ok = before
                .nodes
                .get(node)
                .is_some_and(|x| x.checkpoints == 0 && x.open_intent.is_none())
                && live_holds_at(before, *node, now, events.get(..i).unwrap_or_default()) == 0;
            o.check(ok, Inv::R2, ViolationDetail::Node(*node));
            let by_server = events.iter().take(i).any(|p| {
                matches!(&p.ev, Ev::Appended {
                    node: m,
                    by: Author::Server,
                    body: EntryBody::System(SystemNote::Closed { outcome: Outcome::Missed }),
                    ..
                } if m == node)
            });
            o.check(by_server, Inv::R2, ViolationDetail::Node(*node));
        }
    }
    if let Some((
        Cmd::Node(NodeCmd::Close {
            outcome: Outcome::Missed,
            ..
        }),
        Ok(d),
    )) = cmd
        && d.reply != Reply::Replay
    {
        o.push(Inv::R2, text("only the server closes a node as missed"));
    }
}

/// The live holds on `n` after the events `prefix` of a step from `before` at `now`: those live in
/// `before` and those the prefix issues, less those it ends (R2's "no live hold left").
fn live_holds_at(before: &State, n: NodeId, now: Time, prefix: &[Event]) -> usize {
    let mut live: BTreeSet<Token> = before
        .holds
        .iter()
        .filter(|(t, h)| t.node == n && now < h.lease_until)
        .map(|(t, _)| *t)
        .collect();
    for e in prefix {
        if let Ev::HoldIssued { token, .. } = &e.ev
            && token.node == n
        {
            live.insert(*token);
        } else if let Ev::HoldEnded { token, .. } = &e.ev {
            live.remove(token);
        }
    }
    live.len()
}

/// D1 (step half): every expiry, applied default, lapse, and refusal-driven drop of the step has
/// exactly one digest entry per principal of `spec::audience(before, item)`, and no other entry for
/// that item; every `DigestShown` is by a `to_user` of the step to the entry's principal or an
/// alias of it.
fn d1(before: &State, events: &[Event], o: &mut Out) {
    let dropped: BTreeSet<NodeId> = events
        .iter()
        .filter_map(|e| {
            if let Ev::NodeClosed {
                node,
                outcome: Outcome::Dropped,
                ..
            } = &e.ev
            {
                Some(*node)
            } else {
                None
            }
        })
        .collect();
    let mut items: Vec<DigestItem> = Vec::new();
    for e in events {
        if let Ev::PqExpired { q } = &e.ev {
            items.push(DigestItem::Assumption { question: *q });
        }
        if let Ev::UqLapsed { q } = &e.ev {
            items.push(DigestItem::Lapsed { question: *q });
        }
        if let Ev::UqDefaulted { q } = &e.ev {
            let drop = before.uquestions.get(q).and_then(|u| match u.source {
                UqSource::Refusal { node } if dropped.contains(&node) => Some(node),
                UqSource::Refusal { .. } | UqSource::Agent { .. } | UqSource::Escalation { .. } => {
                    None
                }
            });
            items.push(match drop {
                Some(node) => DigestItem::RefusalDrop { node, question: *q },
                None => DigestItem::DefaultApplied { question: *q },
            });
        }
    }
    let mut got: BTreeMap<DigestItem, Vec<PrincipalId>> = BTreeMap::new();
    let mut to_users: BTreeMap<Eid, PrincipalId> = BTreeMap::new();
    for e in events {
        if let Ev::Appended {
            body: EntryBody::System(SystemNote::Digest { item, to }),
            ..
        } = &e.ev
        {
            got.entry(*item).or_default().push(*to);
        }
        if let Ev::Appended {
            eid,
            body: EntryBody::ToUser { principal, .. },
            ..
        } = &e.ev
        {
            to_users.insert(*eid, *principal);
        }
        if let Ev::DigestShown { entry, by } = &e.ev {
            let ok = to_users.get(by).is_some_and(|p| {
                before
                    .digest
                    .get(entry)
                    .is_some_and(|d| spec::same_person(before, d.to, *p))
            });
            o.check(ok, Inv::D1, ViolationDetail::Entry(*entry));
        }
    }
    for item in &items {
        let want: Vec<PrincipalId> = spec::audience(before, item).into_iter().collect();
        let mut have = got.remove(item).unwrap_or_default();
        have.sort_unstable();
        o.check(
            have == want,
            Inv::D1,
            text("a digest entry per audience principal"),
        );
    }
    for (item, _) in got {
        o.check(
            false,
            Inv::D1,
            ViolationDetail::Text(match item {
                DigestItem::Assumption { .. } => "a digest entry without its expiry",
                DigestItem::DefaultApplied { .. } => "a digest entry without its default",
                DigestItem::RefusalDrop { .. } => "a digest entry without its drop",
                DigestItem::Lapsed { .. } => "a digest entry without its lapse",
            }),
        );
    }
}

/// U1 (step half): a user question created in the step has the chain `spec::chain` gives at its
/// creation (§26 U1, CS-2, D-3): on `before`, with the step's earlier events applied
/// (`user::state_at`), since a `node update` that changes `owners` and adds `needs` asks E1's
/// question after its `NodeUpdated` (D.8.20).
fn u1(before: &State, cmd: Option<&Cmd>, events: &[Event], after: &State, o: &mut Out) {
    for (i, e) in events.iter().enumerate() {
        if let Ev::Appended {
            eid,
            body:
                EntryBody::Question(QuestionBody::User {
                    source, about, to, ..
                }),
            ..
        } = &e.ev
        {
            let explicit: Option<PrincipalId> = match cmd {
                Some(Cmd::Talk(TalkCmd::Ask {
                    to: AskTo::User(UserAsk { to: Some(k), .. }),
                    ..
                })) => after.principal_ids.get(k).copied(),
                _ => None,
            };
            let refusal = matches!(source, UqSource::Refusal { .. });
            let want = spec::chain(
                &user::state_at(before, events, i),
                *about,
                explicit,
                refusal,
            );
            o.check(*to == want, Inv::U1, ViolationDetail::Entry(*eid));
        }
    }
}

/// L1 (step half, children): every parent other than `@people` and `@teams` whose open agent-made
/// children a `NodeCreated` or `NodeMoved` of the step raised has at most `tree.max_children` of
/// them afterwards, counted over `nodes` (CS-10, A-113).
fn l1_children(before: &State, events: &[Event], after: &State, o: &mut Out) {
    let count = |s: &State, p: NodeId| {
        s.nodes
            .values()
            .filter(|x| {
                x.parent() == Some(p) && matches!(x.life, Life::Open) && agent_made(x.origin)
            })
            .count()
    };
    let mut parents: BTreeSet<NodeId> = BTreeSet::new();
    for e in events {
        if let Ev::NodeCreated { parent, .. } | Ev::NodeMoved { parent, .. } = &e.ev {
            parents.insert(*parent);
        }
    }
    for p in parents {
        if p == PEOPLE || p == TEAMS {
            continue;
        }
        let (b, a) = (count(before, p), count(after, p));
        if a > b {
            o.check(
                a <= usize::try_from(after.params.tree_max_children).unwrap_or(usize::MAX),
                Inv::L1,
                ViolationDetail::Node(p),
            );
        }
    }
}

#[path = "step_keys.rs"]
mod keys;

#[path = "step_user.rs"]
mod user;

#[path = "step_people.rs"]
mod people;

#[path = "step_recurring.rs"]
mod recurring;

#[cfg(test)]
#[path = "step_tests.rs"]
mod tests;
