//! The restore of an entity whose leaving time has passed with no event since (D.4.12, the fix of
//! WP-D6): such an entity is still resident when a command names it, and the command's first event
//! would prune it, so a later event of the command that names it would fail its precondition (an
//! anomaly). The shell's plan treats it as departed and restores it from its row, which is
//! current; `restore_tracked` keeps the item, the `Restored` event's step 1 prunes it, and its arm
//! re-inserts it with a new leaving time. Here the row is the resident value, as the shell's row
//! of a resident entity is.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::vec;
use alloc::vec::Vec;

use crate::apply::tests::{class, ok, text};
use crate::cmd::{Cmd, SessionCtx, TalkCmd};
use crate::event::Ev;
use crate::evolve::{leaving_due, restore_tracked};
use crate::facts::Tracked;
use crate::helpers::user::tests::{Ask, Dir, ask_cmd, asked, dir, mk, to_user_cmd};
use crate::reply::Reply;
use crate::state::{DigestItem, DigestState, EntityRef, FINISHED_RETAIN, Restored, State};
use crate::testkit::World;
use crate::types::{NodeRef, SaidKind, ToUserKind};
use crate::user::DIGEST_RETAIN;

/// The item the shell's plan builds for `x` from its row: here the resident value, which is what
/// the row of a resident entity holds.
fn resident_item(s: &State, x: EntityRef) -> Restored {
    match x {
        EntityRef::Said(a) => Restored::Said(a, s.saids[&a].clone(), s.entries[&a]),
        EntityRef::Digest(d) => Restored::Digest(d, s.digest[&d], s.entries[&d]),
        other => panic!("not built here: {other:?}"),
    }
}

/// The shell's restore step before a command at the clock: the named entities that are due are
/// restored, with the checks of `World::consistent`.
fn restore_due(w: &mut World, named: &[EntityRef]) -> usize {
    let items: Vec<Restored> = named
        .iter()
        .filter(|x| leaving_due(&w.s, **x, w.clock))
        .map(|x| resident_item(&w.s, *x))
        .collect();
    let before = w.s.clone();
    let mut dirty = Tracked::new();
    let evs = restore_tracked(&mut w.s, w.clock, items, &mut dirty);
    let n = evs.len();
    w.log.extend(evs);
    w.consistent(&before, &dirty).unwrap();
    n
}

/// A confirmed `said` finishes and is due to leave FINISHED_RETAIN later. The forum stays idle past
/// that time with no event (its sessions retired before), so it is still resident; a correction of
/// it from a new session names it. The restore brings it back first, and the command leaves it
/// resident, with a new leaving time, instead of pruning it in its own first event.
#[test]
fn a_due_said_named_by_corrects_is_restored_before_the_command() {
    let (mut w, c, s) = super::keys::tests::one();
    let n = super::keys::tests::node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let said = ok(w.call(
        &c,
        s,
        super::talk_people::tests::said_cmd(n, None, SaidKind::Reply),
    ));
    let Reply::Said { said: a, .. } = said.reply else {
        panic!("not a said")
    };
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::SaidConfirm {
            said: a,
            confirmed: text("https://chat.example/m/1"),
        }),
    ));
    let leaves = w.s.leaving[&EntityRef::Said(a)];
    assert_eq!(leaves, w.clock.plus(FINISHED_RETAIN));
    // Idle past the leaving time: only the session's own deadlines fire, before it.
    w.advance(FINISHED_RETAIN);
    w.advance(crate::types::Dur(60_000));
    assert!(w.s.last_now < leaves, "no event since the leaving time");
    assert!(w.s.saids.contains_key(&a), "still resident");
    assert!(leaving_due(&w.s, EntityRef::Said(a), w.clock));
    // The shell's restore step, then the command.
    assert_eq!(restore_due(&mut w, &[EntityRef::Said(a)]), 1);
    assert!(matches!(
        w.log.last().map(|e| &e.ev),
        Some(Ev::Restored { .. })
    ));
    assert!(!leaving_due(&w.s, EntityRef::Said(a), w.clock));
    let ctx = w.ctx(&class("assistant"), SessionCtx::New, None);
    ok(w.call_ctx(
        &ctx,
        &Cmd::Talk(TalkCmd::Said {
            at: NodeRef::Id(n),
            where_: None,
            text: text("a correction"),
            kind: SaidKind::Correction,
            corrects: Some(a),
            seen: None,
        }),
    ));
    assert!(w.s.saids.contains_key(&a), "the command did not prune it");
    assert_eq!(
        w.s.leaving[&EntityRef::Said(a)],
        w.s.last_now.plus(FINISHED_RETAIN)
    );
    assert_eq!(w.replay(), w.s);
}

/// A pending digest entry leaves DIGEST_RETAIN after its creation. Idle past that time with no
/// event, it is still resident when a report from a new session names it; without the restore, the
/// report's first event (`SessionOpened`) would prune it and its `DigestShown` would be an anomaly.
/// Restored first, it is shown.
#[test]
fn a_due_digest_entry_named_by_a_report_is_restored_and_shown() {
    let Dir { mut w, c, s, .. } = dir();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:bob@x"),
                default: Some("no"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    w.advance(w.s.params.user_question_due_absent);
    let d =
        *w.s.digest
            .iter()
            .find(|(_, x)| x.item == DigestItem::DefaultApplied { question: q })
            .map(|(d, _)| d)
            .unwrap();
    let leaves = w.s.leaving[&EntityRef::Digest(d)];
    w.advance(DIGEST_RETAIN);
    assert!(w.s.last_now < leaves, "no event since the leaving time");
    assert!(leaving_due(&w.s, EntityRef::Digest(d), w.clock));
    assert_eq!(
        restore_due(&mut w, &[EntityRef::Digest(d), EntityRef::Request(d)]),
        1
    );
    let ctx = w.ctx(&class("assistant"), SessionCtx::New, None);
    let r = ok(w.call_ctx(
        &ctx,
        &to_user_cmd(ToUserKind::Ack, Some("mail:bob@x"), &[d]),
    ));
    assert!(matches!(&r.reply, Reply::ToUser { shown_digest, .. } if *shown_digest == vec![d]));
    assert!(matches!(w.s.digest[&d].state, DigestState::Shown { .. }));
    assert_eq!(w.replay(), w.s);
}
