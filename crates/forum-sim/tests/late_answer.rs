//! Finding Y-1 of the year simulation (no-user profile, seed 1): a person answers a blocking people
//! question after the user question it escalated to has lapsed. The answer withdraws the lapse's
//! `proceed_on_assumption` mark (D.8.15), so no assumption is ever recorded; the people question,
//! answered, finishes and leaves memory `FINISHED_RETAIN` (7 days) later. Before the fix, from then
//! on E1 (f) and M1 read a lapsed user question with no assumption, no mark, and no resident people
//! question to excuse it, and `check_invariants` failed for good.
//!
//! The fix (WP-D6, D.4.12's "answered after all"): such a lapsed escalation is finished by the late
//! answer, held back while its people question is resident, and leaves memory after it; E1 (f) and
//! M1 read a people question that left as answered. The core's own regression test is
//! `cmd::cross_tests::an_answer_after_a_lapse_lets_both_questions_leave_memory`; this one drives
//! the year simulation's sequence through the simulation's forum.

use forum_core::{
    ClassName, DAY, HOUR, Params, ReleaseReason, Reply, SessionCtx, UqState, WaitArg,
    check_invariants,
};
use forum_sim::START;
use forum_sim::cmds;
use forum_sim::forum::Forum;
use forum_sim::observe::{Observer, UqKind, UqOutcome};

#[test]
fn a_late_answer_to_an_escalated_question_keeps_the_invariants() {
    let mut f = Forum::new(Params::default(), START, Observer::default());
    let c = ClassName::new("worker").unwrap();
    let open = |f: &mut Forum| {
        f.call(&c, SessionCtx::New, &cmds::hello(None, None))
            .0
            .unwrap()
            .opened
            .unwrap()
    };
    // A scheduler's job, taken at once, asks its reporter a blocking question and waits for it.
    let s1 = open(&mut f);
    let out = f.call_in(
        &c,
        s1,
        &cmds::create(cmds::path(&["j1"]), "a job", |a| {
            a.keys = vec![cmds::key("issue:sim/j1")];
            a.request = Some(cmds::external_request("run it", "scheduler"));
            a.take = Some(true);
        }),
    );
    let Ok(forum_core::Done {
        reply:
            Reply::Created {
                id: n,
                token: Some(token),
                ..
            },
        ..
    }) = out
    else {
        panic!("{out:?}");
    };
    let Ok(forum_core::Done {
        reply: Reply::Asked { question: pq, .. },
        ..
    }) = f.call_in(&c, s1, &cmds::ask_person(n, true))
    else {
        panic!("no question");
    };
    f.call_in(
        &c,
        s1,
        &cmds::release(token, ReleaseReason::Blocked, Some(WaitArg::Question(pq))),
    )
    .unwrap();
    // Nobody answers: the rechecks run out, it escalates to `user`, whom nobody serves, and lapses.
    let mut t = START;
    let uq = loop {
        t = t.plus(HOUR);
        f.advance(t);
        let lapsed = f
            .obs
            .uqs
            .iter()
            .find(|(_, u)| u.kind == UqKind::Escalation && u.outcome == UqOutcome::Lapsed);
        if let Some((q, _)) = lapsed {
            break *q;
        }
        assert!(t < START.plus(DAY), "no lapse within a day");
    };
    // The reporter answers after all; a worker reports the result and closes the job.
    let s2 = open(&mut f);
    f.call_in(&c, s2, &cmds::heard(n, pq)).unwrap();
    let Ok(forum_core::Done {
        reply: Reply::Next(forum_core::NextReply::Hold { token, .. }),
        ..
    }) = f.call_in(&c, s2, &cmds::next())
    else {
        panic!("the job is not offered again");
    };
    f.call_in(&c, s2, &cmds::said_result(n)).unwrap();
    let settles = f.s.ix.items[&n].unsettled.clone();
    f.call_in(&c, s2, &cmds::close(n, Some(token), settles))
        .unwrap();
    // The user question stays lapsed, with no assumption and no mark.
    let u = &f.s.uquestions[&uq];
    assert_eq!(u.state, UqState::Lapsed);
    assert_eq!(u.assumed_by, None);
    // Holds while the answered people question is resident ...
    f.advance(t.plus(DAY.times(6)));
    assert_eq!(check_invariants(&f.s, f.now), Ok(()));
    // ... and after it leaves memory, FINISHED_RETAIN after the answer.
    f.advance(t.plus(DAY.times(8)));
    assert!(
        !f.s.pquestions.contains_key(&pq),
        "the people question is still resident"
    );
    assert_eq!(check_invariants(&f.s, f.now), Ok(()));
    // The late answer finished the lapsed user question: it leaves FINISHED_RETAIN after its
    // people question did (at the first event after that time), and the invariants still hold.
    assert!(
        f.s.leaving
            .contains_key(&forum_core::EntityRef::UQuestion(uq)),
        "the lapsed user question never leaves"
    );
    f.advance(t.plus(DAY.times(16)));
    open(&mut f);
    assert!(
        !f.s.uquestions.contains_key(&uq),
        "the lapsed user question is still resident"
    );
    assert_eq!(check_invariants(&f.s, f.now), Ok(()));
}

trait Times {
    fn times(self, k: u64) -> forum_core::Dur;
}

impl Times for forum_core::Dur {
    fn times(self, k: u64) -> forum_core::Dur {
        forum_core::Dur(self.0 * k)
    }
}
