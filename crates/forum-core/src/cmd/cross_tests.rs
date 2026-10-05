//! Cross-cutting tests of WP-D6 (design D.13.2): flows that need both D5 packages and memory
//! (D.4.12), with E1, D1, Settled, and M1 checked after every call and firing by `World`. The
//! first is the regression of the full-universe proptest's finding and of the year simulation's
//! Y-1: a people question answered after its escalation lapsed leaves memory, and so, after it,
//! does the lapsed user question, which the late answer finished (D.4.12); E1 (f) and M1 hold
//! after the people question leaves ("answered after all").

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm,
    reason = "test code: the deny list guards the crate's own code"
)]

use super::keys::tests::{node, one};
use super::talk_people::tests::{ask_cmd, heard_cmd};
use crate::apply::tests::{class, close, ok, release, take};
use crate::cmd::*;
use crate::reply::Reply;
use crate::state::{EntityRef, FINISHED_RETAIN, PqState, UqState, WaitKey, WaitOn};
use crate::types::*;

/// A blocking question to a person escalates at its last recheck; the user question lapses with
/// no answer and marks the waiting node; the person then answers after all, which removes the
/// mark. The people question is finished and leaves memory FINISHED_RETAIN later; the lapsed user
/// question, finished by the late answer and held back while its people question is resident,
/// leaves FINISHED_RETAIN after that (D.4.12, Y-1). E1 (f) and M1 hold throughout, in particular
/// once the people question has left (`World` checks every invariant after every call and
/// firing), and neither question stays for good.
#[test]
fn an_answer_after_a_lapse_lets_both_questions_leave_memory() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/support", &["chat:a/1"]);
    let work = node(&mut w, &c, s, "/support/work", &[]);
    let d = ok(w.call(&c, s, ask_cmd(n, None, true, false)));
    let q = match d.reply {
        Reply::Asked { question, .. } => question,
        other => panic!("{other:?}"),
    };
    // The rechecks before the last one; then the work waits for the answer, and the last recheck
    // escalates it to the user, moving the wait there (A-93).
    for _ in 1..w.s.params.question_max_rechecks {
        let at = w.s.pquestions[&q].recheck_at;
        w.at(at);
        w.settle();
    }
    let t = match ok(w.call(&c, s, take(work))).reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    };
    let mut rel = release(t, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut rel {
        *waits_for = Some(WaitArg::Question(q));
    }
    ok(w.call(&c, s, rel));
    let at = w.s.pquestions[&q].recheck_at;
    w.at(at);
    w.settle();
    let PqState::Escalated { uq } = w.s.pquestions[&q].state else {
        panic!("{:?}", w.s.pquestions[&q].state)
    };
    assert_eq!(
        w.s.nodes[&work].wait.as_ref().map(|x| x.on),
        Some(WaitOn::UserQuestion(uq))
    );
    while w.s.uquestions[&uq].state == UqState::Pending {
        let due = w.s.uquestions[&uq].due;
        w.at(due);
        w.settle();
    }
    assert_eq!(w.s.uquestions[&uq].state, UqState::Lapsed);
    assert!(w.s.nodes[&work].assume_marks.contains(&uq));
    // The person answers after all.
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, heard_cmd(n, None, Some(q))));
    assert!(matches!(w.s.pquestions[&q].state, PqState::Answered { .. }));
    assert!(!w.s.nodes[&work].assume_marks.contains(&uq));
    // The work closes, which clears its wait (resolved since the lapse, it still named the user
    // question and held it back, D.4.12).
    ok(w.call(&c, s2, close(work, Outcome::Done)));
    assert!(
        w.s.ix
            .waiters
            .get(&WaitKey::Question(uq))
            .is_none_or(|x| x.is_empty())
    );
    // Both are finished; the user question is held back while its people question is resident.
    let answered_at = w.clock;
    assert_eq!(
        w.s.leaving.get(&EntityRef::PQuestion(q)),
        Some(&answered_at.plus(FINISHED_RETAIN))
    );
    assert_eq!(w.s.uquestions[&uq].state, UqState::Lapsed);
    assert!(!w.s.leaving.contains_key(&EntityRef::UQuestion(uq)));
    // The people question leaves at its time (the next event prunes it); the user question then
    // gets its own leaving time, and leaves in turn.
    w.advance(Dur(FINISHED_RETAIN.0 + DAY.0));
    let _ = w.hello(&class("other"));
    assert!(!w.s.pquestions.contains_key(&q));
    assert!(w.s.uquestions.contains_key(&uq));
    assert_eq!(
        w.s.leaving.get(&EntityRef::UQuestion(uq)),
        Some(&w.clock.plus(FINISHED_RETAIN))
    );
    w.advance(Dur(FINISHED_RETAIN.0 + DAY.0));
    let _ = w.hello(&class("other"));
    assert!(!w.s.uquestions.contains_key(&uq));
    assert!(!w.s.leaving.contains_key(&EntityRef::UQuestion(uq)));
    w.check().unwrap();
}
