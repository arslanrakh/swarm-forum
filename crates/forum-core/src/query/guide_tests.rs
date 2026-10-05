//! Unit tests of the next calls (design D.12.1, D.8.26, §20.9): one or more per rule, sub-row, and
//! remedy, each with its negative, the 1.4 rows (R3, R4, R6, R11, R12, R13, R18, R19), R28, the
//! ordering, dedupe, cut, and `total` of `finish`, and the isolation of `guide` from the commands.
//! Every call runs through `World::call_ctx`, which also runs `query::guide` and `check_guide` (G1)
//! on the response, so every other test of the crate checks G1 too.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::apply::tests::{T0, class, create_with, created, ok, release, take, text};
use crate::cmd::{AgentAddressee, CreateArgs, UserAsk};
use crate::error::ErrorCode;
use crate::helpers::user::tests::{
    from_user_cmd, key, open_as, person, pid, pk, to_user_cmd, user_request, world_with,
};
use crate::params::Params;
use crate::query::{NoMeter, status_line};
use crate::reply::Done;
use crate::testkit::World;
use crate::types::{HOUR, MINUTE, Outcome, ReqDigest, When};

// ------------------------------------------------------------------------------------------------
// Fixtures

fn id(n: NodeId) -> NodeRef {
    NodeRef::Id(n)
}

/// The calls of the last response, with their rule and moment.
fn calls(w: &World) -> Vec<(Rule, Moment, Skeleton)> {
    w.last_guidance
        .items
        .iter()
        .map(|i| (i.rule, i.when, i.call.clone()))
        .collect()
}

fn rules(w: &World) -> Vec<Rule> {
    w.last_guidance.items.iter().map(|i| i.rule).collect()
}

fn has(w: &World, sk: &Skeleton) -> bool {
    w.last_guidance.items.iter().any(|i| i.call == *sk)
}

fn item_of(w: &World, rule: Rule) -> Option<NextCall> {
    w.last_guidance
        .items
        .iter()
        .find(|i| i.rule == rule)
        .cloned()
}

/// A call by `s` of class `c`, and every item before the cut (`all_items`).
fn go(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    cmd: Cmd,
) -> (Result<Done, Error>, Vec<NextCall>) {
    let out = w.call(c, s, cmd.clone());
    let (all, _) = all_items(&w.s, Some(s), w.clock, &cmd, &out, None);
    (out, all)
}

fn worker() -> (World, ClassName, SessionId) {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    (w, c, s)
}

fn mk(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    p: &str,
    f: impl FnOnce(&mut CreateArgs),
) -> (NodeId, Option<Token>) {
    created(&ok(w.call(c, s, create_with(p, f))))
}

fn held(w: &mut World, c: &ClassName, s: SessionId, p: &str) -> (NodeId, Token) {
    let (n, t) = mk(w, c, s, p, |_| {});
    (n, t.unwrap())
}

fn free(w: &mut World, c: &ClassName, s: SessionId, p: &str) -> NodeId {
    mk(w, c, s, p, |a| a.take = Some(false)).0
}

fn view(n: NodeId) -> Cmd {
    Cmd::View(ViewCmd::Node {
        targets: vec![id(n)],
        view: None,
        page: None,
    })
}

fn view_other() -> Cmd {
    Cmd::View(ViewCmd::Constitution)
}

fn checkpoint_with(t: Token, intent: Option<&str>, intent_done: Option<&str>) -> Cmd {
    Cmd::Hold(HoldCmd::Checkpoint {
        token: t,
        done: text("did"),
        next_step: text("next"),
        intent: intent.map(text),
        intent_done: intent_done.map(text),
        files: Vec::new(),
    })
}

fn close_with(n: NodeId, t: Token, outcome: Outcome, settles: &[Eid]) -> Cmd {
    Cmd::Node(NodeCmd::Close {
        at: id(n),
        outcome,
        summary: text("done"),
        verified: None,
        settles: Some(settles.iter().copied().collect()),
        learnings: None,
        token: Some(t),
        unsent: BTreeSet::new(),
    })
}

/// A close without `settles`: the default of §5 (1.7).
fn close_default(n: NodeId, t: Token) -> Cmd {
    Cmd::Node(NodeCmd::Close {
        at: id(n),
        outcome: Outcome::Done,
        summary: text("done"),
        verified: None,
        settles: None,
        learnings: None,
        token: Some(t),
        unsent: BTreeSet::new(),
    })
}

fn request_cmd(n: NodeId, origin: &str, where_: Option<&str>) -> Cmd {
    Cmd::Talk(TalkCmd::Request {
        at: id(n),
        text: text("why?"),
        origin: key(origin),
        where_: where_.map(key),
        requester: text("Bob"),
    })
}

fn said_cmd(n: NodeId, where_: Option<&str>, kind: SaidKind) -> Cmd {
    Cmd::Talk(TalkCmd::Said {
        at: id(n),
        where_: where_.map(key),
        text: text("on it"),
        kind,
        corrects: None,
        seen: None,
    })
}

fn confirm(a: Eid) -> Cmd {
    Cmd::Talk(TalkCmd::SaidConfirm {
        said: a,
        confirmed: text("https://chat.example/m/1"),
    })
}

fn decide_cmd(n: NodeId, settles: &[Eid]) -> Cmd {
    Cmd::Talk(TalkCmd::Decide {
        at: id(n),
        text: text("decided"),
        settles: settles.iter().copied().collect(),
        supersedes: None,
        basis: Vec::new(),
        evidence: None,
    })
}

fn ask_person(n: NodeId, where_: Option<&str>, blocking: bool) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: id(n),
        text: text("which?"),
        to: AskTo::Person {
            where_: where_.map(key),
            blocking,
        },
        again: false,
    })
}

fn ask_user(n: NodeId, to: Option<&str>, digest: u8) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: id(n),
        text: text("which?"),
        to: AskTo::User(UserAsk {
            to: to.map(pk),
            options: vec![text("a"), text("b")],
            recommendation: None,
            default: None,
            due: None,
            digest: ReqDigest([digest; 32]),
        }),
        again: false,
    })
}

fn ask_agent(n: NodeId, to: AgentAddressee) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: id(n),
        text: text("which?"),
        to: AskTo::Agent(to),
        again: false,
    })
}

fn wait_cmd() -> Cmd {
    Cmd::Talk(TalkCmd::Wait {
        timeout: SECOND,
        for_: None,
        since: None,
    })
}

fn said_of(d: &Done) -> Eid {
    match &d.reply {
        Reply::Said { said, .. } => *said,
        Reply::Asked { said: Some(a), .. } => *a,
        other => panic!("no said: {other:?}"),
    }
}

fn question_of(d: &Done) -> Eid {
    match &d.reply {
        Reply::Asked { question, .. } => *question,
        other => panic!("no question: {other:?}"),
    }
}

fn request_of(d: &Done) -> Eid {
    match &d.reply {
        Reply::Request { request, .. } => *request,
        other => panic!("no request: {other:?}"),
    }
}

fn token_of(d: &Done) -> Token {
    match &d.reply {
        Reply::Created { token: Some(t), .. }
        | Reply::Taken { token: t, .. }
        | Reply::Next(NextReply::Hold { token: t, .. }) => *t,
        other => panic!("no hold: {other:?}"),
    }
}

// ------------------------------------------------------------------------------------------------
// The isolation of guide (D.8.26) and its purity

#[test]
fn no_command_nor_apply_settle_or_evolve_names_guide() {
    let sources = [
        include_str!("../apply.rs"),
        include_str!("../settle.rs"),
        include_str!("../evolve.rs"),
        include_str!("../cmd/mod.rs"),
        include_str!("../cmd/control.rs"),
        include_str!("../cmd/hold.rs"),
        include_str!("../cmd/keys.rs"),
        include_str!("../cmd/node_basic.rs"),
        include_str!("../cmd/restructure.rs"),
        include_str!("../cmd/review.rs"),
        include_str!("../cmd/session.rs"),
        include_str!("../cmd/talk_agents.rs"),
        include_str!("../cmd/talk_people.rs"),
        include_str!("../cmd/talk_user.rs"),
        include_str!("../cmd/view.rs"),
    ];
    for src in sources {
        assert!(!src.contains("guide"), "a command names guide");
    }
}

#[test]
fn guide_changes_nothing_and_is_a_function() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let cmd = view(a);
    let out = w.call(&c, s, cmd.clone());
    let before = w.s.clone();
    let g1 = guide(&w.s, Some(s), w.clock, &cmd, &out, &mut NoMeter);
    let g2 = guide(&w.s, Some(s), w.clock, &cmd, &out, &mut NoMeter);
    assert_eq!(g1, g2);
    assert_eq!(w.s, before);
    assert_eq!(g1, w.last_guidance);
    assert_eq!(g1, crate::spec::guide(&w.s, Some(s), w.clock, &cmd, &out));
}

// ------------------------------------------------------------------------------------------------
// E: the remedies of §21

#[test]
fn remedy_of_a_stale_token_is_a_take_when_it_would_pass_else_a_view() {
    let (mut w, c, s) = worker();
    let (a, t) = held(&mut w, &c, s, "/a");
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    let _ = w.call(&c, s, checkpoint_with(t, None, None));
    let first = &w.last_guidance.items[0];
    assert_eq!(first.rule, Rule::Remedy);
    assert_eq!(first.call, Skeleton::Take { target: a });
    assert!(matches!(first.because, Because::HoldEnded { .. }));
    // Another session holds it now: the remedy is to read it.
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(a)));
    let _ = w.call(&c, s, checkpoint_with(t, None, None));
    assert_eq!(
        w.last_guidance.items[0].call,
        Skeleton::ViewNode { node: a }
    );
}

#[test]
fn remedy_of_a_stale_version_and_a_missing_digest_version_is_a_reread() {
    let (mut w, c, s) = worker();
    let a = free(&mut w, &c, s, "/a");
    let stale = Cmd::Node(NodeCmd::Update {
        at: id(a),
        version: Version(0),
        digest_version: None,
        token: None,
        patch: Patch {
            title: Some(text("new")),
            ..Patch::default()
        },
    });
    let _ = w.call(&c, s, stale);
    let first = w.last_guidance.items[0].clone();
    assert_eq!(first.call, Skeleton::ViewNode { node: a });
    assert!(matches!(first.because, Because::Reread { .. }));
    let version = w.s.nodes[&a].version;
    let no_digest_version = Cmd::Node(NodeCmd::Update {
        at: id(a),
        version,
        digest_version: None,
        token: None,
        patch: Patch {
            digest: Some(text("d")),
            ..Patch::default()
        },
    });
    let out = w.call(&c, s, no_digest_version);
    assert_eq!(
        out.err().map(|e| e.hint),
        Some(Hint::Invalid(Why::MissingDigestVersion))
    );
    assert_eq!(
        w.last_guidance.items[0].call,
        Skeleton::ViewNode { node: a }
    );
}

#[test]
fn remedy_of_a_merged_node_is_a_take_of_where_it_lives_on() {
    let (mut w, c, s) = worker();
    let a = free(&mut w, &c, s, "/a");
    let b = free(&mut w, &c, s, "/b");
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Merge {
            from: id(a),
            into: id(b),
        }),
    ));
    let out = w.call(&c, s, take(a));
    assert!(matches!(
        out.err().map(|e| e.hint),
        Some(Hint::Closed {
            merged_into: Some(_),
            ..
        })
    ));
    let first = w.last_guidance.items[0].clone();
    assert_eq!(first.call, Skeleton::Take { target: b });
    assert_eq!(first.because, Because::MergedInto { node: b });
}

#[test]
fn remedy_of_a_second_intent_closes_the_open_one_and_checks_another_sessions_first() {
    let (mut w, c, s1) = worker();
    let (a, t1) = held(&mut w, &c, s1, "/a");
    ok(w.call(&c, s1, checkpoint_with(t1, Some("send the mail"), None)));
    // The session's own intent: the swap.
    let _ = w.call(&c, s1, checkpoint_with(t1, Some("another"), None));
    let first = w.last_guidance.items[0].clone();
    assert_eq!(first.rule, Rule::Remedy);
    assert_eq!(first.call, Skeleton::CheckpointSwap { token: t1 });
    assert!(matches!(
        first.because,
        Because::CloseOpenIntent { own: true, .. }
    ));
    // Another session's intent (R6): the check first.
    ok(w.call(&c, s1, release(t1, ReleaseReason::Handoff)));
    let s2 = w.hello(&c);
    let t2 = token_of(&ok(w.call(&c, s2, take(a))));
    let _ = w.call(&c, s2, checkpoint_with(t2, Some("mine"), None));
    let first = w.last_guidance.items[0].clone();
    assert_eq!(first.call, Skeleton::CheckpointSwap { token: t2 });
    assert!(matches!(
        first.because,
        Because::CloseOpenIntent { by, own: false, .. } if by == s1
    ));
    // The standard fill leaves `intent` out, and the swap passes (the remedy is optional there).
    let (sc, cmd) = standard_fill(&w.s, Some(s2), &first.call).unwrap();
    assert_eq!(sc, SessionCtx::Existing(s2));
    assert!(matches!(
        cmd,
        Cmd::Hold(HoldCmd::Checkpoint {
            intent: None,
            intent_done: Some(_),
            ..
        })
    ));
}

#[test]
fn remedy_of_too_many_renews_is_a_checkpoint() {
    let (mut w, c, s) = worker();
    let (_, t) = held(&mut w, &c, s, "/a");
    let renew = Cmd::Hold(HoldCmd::Renew {
        token: t,
        why: text("more"),
    });
    for _ in 0..w.s.params.renew_max {
        ok(w.call(&c, s, renew.clone()));
    }
    let out = w.call(&c, s, renew);
    assert_eq!(
        out.err().map(|e| e.hint),
        Some(Hint::Limit(LimitKind::Renew))
    );
    let first = w.last_guidance.items[0].clone();
    assert_eq!(
        first.call,
        Skeleton::Checkpoint {
            token: t,
            intent_done: false,
            intent: false
        }
    );
    assert_eq!(first.because, Because::RenewLimit);
}

#[test]
fn remedy_of_an_answer_of_the_wrong_kind_is_the_answer_that_fits() {
    let (mut w, c, s) = worker();
    let (a, _) = mk(&mut w, &c, s, "/a", |x| {
        x.keys = vec![key("chat:x/1")];
    });
    let q = question_of(&ok(w.call(&c, s, ask_person(a, None, false))));
    let wrong = Cmd::Talk(TalkCmd::Answer {
        question: q,
        node: a,
        asker: Some(s),
        text: text("x"),
    });
    let out = w.call(&c, s, wrong);
    assert_eq!(
        out.err().map(|e| e.hint),
        Some(Hint::Invalid(Why::NotAgentQuestion))
    );
    let first = w.last_guidance.items[0].clone();
    assert_eq!(
        first.call,
        Skeleton::Heard {
            at: a,
            answers: q,
            where_: KeyStr::new("chat:x/1").unwrap()
        }
    );
}

#[test]
fn remedy_of_an_unknown_session_is_hello_alone_and_other_codes_have_none() {
    let (mut w, c, s) = worker();
    let ctx = w.ctx(&c, SessionCtx::Existing(SessionId(999)), None);
    let _ = w.call_ctx(&ctx, &view_other());
    assert_eq!(
        calls(&w),
        vec![(Rule::Remedy, Moment::Now, Skeleton::Hello)]
    );
    // A session of another class.
    let other = class("other");
    let ctx = w.ctx(&other, SessionCtx::Existing(s), None);
    let _ = w.call_ctx(&ctx, &view_other());
    assert_eq!(
        calls(&w),
        vec![(Rule::Remedy, Moment::Now, Skeleton::Hello)]
    );
    // `not_found` of an addressee has no remedy (R12).
    let a = free(&mut w, &c, s, "/a");
    let say = Cmd::Talk(TalkCmd::Say {
        at: id(a),
        text: text("hi"),
        to: Some(AddresseeArg::Session(SessionId(999))),
        expects: None,
        reply_to: None,
        until: None,
    });
    let out = w.call(&c, s, say);
    assert_eq!(
        out.err().map(|e| e.hint),
        Some(Hint::NotFound(What::Addressee))
    );
    assert!(!rules(&w).contains(&Rule::Remedy));
    // `holds_limit` has none.
    held(&mut w, &c, s, "/b");
    held(&mut w, &c, s, "/c");
    let out = w.call(&c, s, take(a));
    assert_eq!(out.err().map(|e| e.code), Some(ErrorCode::HoldsLimit));
    assert!(!rules(&w).contains(&Rule::Remedy));
}

#[test]
fn after_a_refused_decide_the_decision_and_reply_owed_stay_in_view() {
    // R19: rules 6 and 7 run after a refusal, the requests the refused call named included.
    let (mut w, c, s) = worker();
    let (a, _) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    ok(w.call(&c, s, said_cmd(a, None, SaidKind::Result)));
    // `r` is answered, not decided; the refused decide names it with an unknown supersedes.
    let bad = Cmd::Talk(TalkCmd::Decide {
        at: id(a),
        text: text("decided"),
        settles: [r].into(),
        supersedes: Some(Eid(9_999)),
        basis: Vec::new(),
        evidence: None,
    });
    let out = w.call(&c, s, bad);
    assert!(out.is_err());
    // 1.7: on a node the session holds, rule 8's close records the decision, and settles `r` by
    // default, since a result answered it (§5); rule E keeps rule 8.
    let t = w.s.ix.holds_by_session[&s]
        .iter()
        .copied()
        .find(|t| t.node == a)
        .unwrap();
    assert!(has(
        &w,
        &Skeleton::Close {
            node: a,
            token: t,
            settles: vec![],
            outcome: None
        }
    ));
    assert!(!rules(&w).contains(&Rule::Decide));
}

// ------------------------------------------------------------------------------------------------
// 1: log first

#[test]
fn rule_1_logs_the_human_s_words_after_hello_with_a_principal_only() {
    let mut w = world_with(|_| {});
    let c = class("assistant");
    let _ = open_as(&mut w, &c, Some("mail:alice@x"));
    let p = pid(&w, "mail:alice@x");
    assert_eq!(
        calls(&w)[0],
        (
            Rule::LogFirst,
            Moment::Now,
            Skeleton::LogFromUser { principal: p }
        )
    );
    // Without a principal, or with a from_user, no item.
    let _ = w.hello(&c);
    assert!(!rules(&w).contains(&Rule::LogFirst));
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let _ = w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: Some(pk("mail:alice@x")),
            from_user: Some(crate::cmd::FromUserArg {
                text: text("hi"),
                client: text("chat"),
                principal: None,
                answers: None,
                to: None,
                clarifies: None,
                reason: None,
            }),
        }),
    );
    assert!(!rules(&w).contains(&Rule::LogFirst));
}

// ------------------------------------------------------------------------------------------------
// 2: said entries

/// Whether an item of the last response is a confirm of a `said` (`talk said {said_id}`).
fn confirms(w: &World, s: SessionId) -> bool {
    w.last_guidance.items.iter().any(|i| {
        matches!(
            standard_fill(&w.s, Some(s), &i.call),
            Some((_, Cmd::Talk(TalkCmd::SaidConfirm { .. })))
        )
    })
}

/// 1.9 (§9.2, §20.9 rule 2): no next call asks to confirm a `said`. In the lab's fourth round a
/// worker confirmed a reply it never sent on 1.8's `later` confirm, and another spent 3 of its 26
/// calls on confirms. The entry stays pending, taken as sent, and the close marks it sent with its
/// author's last reply; one listed in `unsent`, or left by a session that fell silent, becomes
/// unconfirmed, and is not suggested either.
#[test]
fn rule_2_never_asks_to_confirm_a_said() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let y = said_of(&ok(w.call(&c, s, said_cmd(a, None, SaidKind::Ack))));
    assert!(!confirms(&w, s), "{:?}", calls(&w));
    assert!(!rules(&w).contains(&Rule::TellHolder), "{:?}", calls(&w));
    ok(w.call(&c, s, view_other()));
    assert!(!confirms(&w, s), "{:?}", calls(&w));
    assert_eq!(w.s.saids[&y].state, crate::state::SaidState::Pending);
    // A confirm stays available, for a link.
    ok(w.call(&c, s, confirm(y)));
    assert_eq!(w.s.saids[&y].state, crate::state::SaidState::Confirmed);
    let z = said_of(&ok(w.call(&c, s, said_cmd(a, None, SaidKind::Result))));
    assert!(!confirms(&w, s), "{:?}", calls(&w));
    ok(w.call(&c, s, close_with(a, t, Outcome::Done, &[])));
    assert_eq!(w.s.saids[&z].state, crate::state::SaidState::Confirmed);
    assert!(w.s.saids[&z].with_reply);
    // Unsent, and silent: unconfirmed, and still no confirm suggested.
    let (b, tb) = mk(&mut w, &c, s, "/b", |x| x.keys = vec![key("chat:x/2")]);
    let u = said_of(&ok(w.call(&c, s, said_cmd(b, None, SaidKind::Result))));
    let mut cmd = release(tb.unwrap(), ReleaseReason::Handoff);
    if let Cmd::Hold(HoldCmd::Release { unsent, .. }) = &mut cmd {
        *unsent = [u].into();
    }
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.saids[&u].state, crate::state::SaidState::Unconfirmed);
    assert!(!confirms(&w, s), "{:?}", calls(&w));
    let (d, _) = mk(&mut w, &c, s, "/d", |x| {
        x.keys = vec![key("chat:x/3")];
        x.take = Some(false);
    });
    // 1.11 (§9.2): s holds work, so the said stays pending until its lease ends and it falls
    // silent; one recorded holding nothing is taken as sent at once.
    held(&mut w, &c, s, "/busy");
    let v = said_of(&ok(w.call(&c, s, said_cmd(d, None, SaidKind::Ack))));
    w.advance(Dur(w.s.params.presence_silence.0 + MINUTE.0));
    assert_eq!(w.s.saids[&v].state, crate::state::SaidState::Unconfirmed);
    ok(w.call(&c, s, view_other()));
    assert!(!confirms(&w, s), "{:?}", calls(&w));
}

/// 1.9 (§9.2): rule 2 suggests only the `say` to the holder of a conversation another session
/// holds, and no confirm. 1.10: the `said` counts (pending), where 1.9 withheld it. 1.11: s2 holds
/// nothing, so its `said` is taken as sent at once, with its reply, and rule 2 tells the holder all
/// the same.
#[test]
fn rule_2_tells_the_holder_of_a_withheld_said_and_confirms_nothing() {
    let (mut w, c, s1) = worker();
    let (a, _) = mk(&mut w, &c, s1, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let s2 = w.hello(&c);
    let y = said_of(&ok(w.call(&c, s2, said_cmd(a, None, SaidKind::Reply))));
    assert_eq!(w.s.saids[&y].state, crate::state::SaidState::Confirmed);
    assert!(w.s.saids[&y].with_reply);
    assert!(has(&w, &Skeleton::SayTo { at: a, to: s1 }));
    assert!(!confirms(&w, s2), "{:?}", calls(&w));
    ok(w.call(&c, s2, view(a)));
    assert!(!rules(&w).contains(&Rule::TellHolder));
    assert!(!confirms(&w, s2), "{:?}", calls(&w));
}

/// An `ask`'s `said` is not withheld (1.9): it stays pending, and rule 2 tells the holder, once,
/// and confirms nothing. 1.11: s2 holds other work, so the `said` stays pending.
#[test]
fn rule_2_tells_the_holder_of_a_held_conversation_once() {
    let (mut w, c, s1) = worker();
    let (a, _) = mk(&mut w, &c, s1, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let s2 = w.hello(&c);
    held(&mut w, &c, s2, "/busy");
    let y = match ok(w.call(&c, s2, ask_person(a, None, false))).reply {
        Reply::Asked { said: Some(y), .. } => y,
        other => panic!("{other:?}"),
    };
    assert_eq!(w.s.saids[&y].state, crate::state::SaidState::Pending);
    let it = item_of(&w, Rule::TellHolder).unwrap();
    assert_eq!(
        (it.when, it.call, it.once),
        (Moment::Now, Skeleton::SayTo { at: a, to: s1 }, true)
    );
    assert!(!confirms(&w, s2), "{:?}", calls(&w));
    // The next response: no say, and no confirm.
    ok(w.call(&c, s2, view(a)));
    assert!(!rules(&w).contains(&Rule::TellHolder));
    assert!(!confirms(&w, s2), "{:?}", calls(&w));
}

// ------------------------------------------------------------------------------------------------
// 3: checkpoints and intents

#[test]
fn rule_3_suggests_the_checkpoint_at_issue_later_and_when_due_now() {
    let (mut w, c, s) = worker();
    let (_, t) = held(&mut w, &c, s, "/a");
    let it = item_of(&w, Rule::Checkpoint).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert!(it.once);
    assert!(matches!(it.because, Because::CheckpointBy { .. }));
    // At 4:59 after the take, nothing; at 5:00, now.
    w.advance(Dur(5 * MINUTE.0 - 1_000));
    ok(w.call(&c, s, view_other()));
    assert!(!rules(&w).contains(&Rule::Checkpoint));
    w.advance(SECOND);
    ok(w.call(&c, s, view_other()));
    let it = item_of(&w, Rule::Checkpoint).unwrap();
    assert_eq!(it.when, Moment::Now);
    assert!(!it.once);
    assert_eq!(
        it.call,
        Skeleton::Checkpoint {
            token: t,
            intent_done: false,
            intent: false
        }
    );
    // The status line shows the same time (R8).
    let line = status_line(&w.s, s, w.clock);
    assert_eq!(line.holds[0].checkpoint_by, w.clock);
}

#[test]
fn rule_3_with_a_two_minute_lease_is_due_a_minute_before_it_ends() {
    let (mut w, c, s) = worker();
    let a = free(&mut w, &c, s, "/a");
    let t = token_of(&ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Take {
            target: id(a),
            lease: Some(Dur(2 * MINUTE.0)),
        }),
    )));
    // A checkpoint at 13 minutes keeps the first lease's end, 15 minutes (a lease never
    // shrinks); its next checkpoint is due a minute before it, at 14.
    w.advance(Dur(13 * MINUTE.0));
    ok(w.call(&c, s, checkpoint_with(t, None, None)));
    assert_eq!(w.s.holds[&t].lease_until, w.clock.plus(Dur(2 * MINUTE.0)));
    w.advance(Dur(MINUTE.0 - 1_000));
    ok(w.call(&c, s, view_other()));
    assert!(!rules(&w).contains(&Rule::Checkpoint));
    w.advance(SECOND);
    ok(w.call(&c, s, view_other()));
    assert_eq!(item_of(&w, Rule::Checkpoint).unwrap().when, Moment::Now);
}

#[test]
fn rule_3_at_the_lease_ceiling_says_so() {
    let mut w = world_with(|p| {
        p.lease_default = p.lease_first;
        p.lease_max = p.lease_first;
        p.lease_ceiling = p.lease_first;
    });
    let c = class("worker");
    let s = w.hello(&c);
    held(&mut w, &c, s, "/a");
    w.advance(Dur(5 * MINUTE.0));
    ok(w.call(&c, s, view_other()));
    assert!(matches!(
        item_of(&w, Rule::Checkpoint).unwrap().because,
        Because::LeaseDue {
            at_ceiling: true,
            ..
        }
    ));
}

/// R18's branch of rule 3 on the outcome alone: a `create` that issued a hold and returned
/// `uncovered_slot` gets a `now` checkpoint that records an intent, one-shot; without the warning,
/// the `later` checkpoint by its due time. The warning comes from `create {repeat}` (WP-D6), so
/// the reply here is the real one with the warning added; §32 step 1 runs the whole call
/// (`section_32_step_1_next_calls`).
#[test]
fn rule_3_after_uncovered_slot_records_the_trigger_s_intent_now() {
    use crate::reply::{Warning, WarningDetail};
    let (mut w, c, s) = worker();
    let cmd = create_with("/digest", |_| {});
    let mut out = w.call(&c, s, cmd.clone());
    let d = out.as_ref().unwrap();
    let (a, t) = created(d);
    let t = t.unwrap();
    let plain = guide(&w.s, Some(s), w.clock, &cmd, &out, &mut NoMeter);
    let it = plain
        .items
        .iter()
        .find(|i| i.rule == Rule::Checkpoint)
        .unwrap();
    assert_eq!(
        (it.when, it.once, &it.call),
        (
            Moment::Later,
            true,
            &Skeleton::Checkpoint {
                token: t,
                intent_done: false,
                intent: false
            }
        )
    );
    if let Ok(d) = out.as_mut() {
        d.warnings.push(Warning {
            code: WarningCode::UncoveredSlot,
            id: Some(a),
            detail: WarningDetail::UncoveredSlot {
                covered_days: 0,
                days: 7,
                usual_hours: Vec::new(),
                first_start: None,
            },
        });
    }
    let g = guide(&w.s, Some(s), w.clock, &cmd, &out, &mut NoMeter);
    assert_eq!(g, crate::spec::guide(&w.s, Some(s), w.clock, &cmd, &out));
    assert_eq!(
        crate::check_guide(&w.s, Some(s), w.clock, &cmd, &out, &g),
        Ok(())
    );
    let it = g.items.first().unwrap();
    assert_eq!(it.rule, Rule::Checkpoint);
    assert_eq!(it.when, Moment::Now);
    assert!(it.once);
    assert_eq!(
        it.call,
        Skeleton::Checkpoint {
            token: t,
            intent_done: false,
            intent: true
        }
    );
    assert_eq!(it.because, Because::UncoveredSlot);
}

#[test]
fn rule_3_closes_an_own_intent_later_and_another_session_s_now() {
    let (mut w, c, s1) = worker();
    let (a, t1) = held(&mut w, &c, s1, "/a");
    ok(w.call(&c, s1, checkpoint_with(t1, Some("deploy"), None)));
    let it = item_of(&w, Rule::IntentDone).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert!(matches!(it.because, Because::IntentOpen { own: true, .. }));
    // One item per hold, never skipped while the intent is open: at the due time, the checkpoint
    // names the intent in its why.
    w.advance(Dur(5 * MINUTE.0));
    ok(w.call(&c, s1, view_other()));
    assert!(matches!(
        item_of(&w, Rule::Checkpoint).unwrap().because,
        Because::LeaseDue {
            intent: Some(_),
            ..
        }
    ));
    // Inherited: the check first, now (R6).
    ok(w.call(&c, s1, release(t1, ReleaseReason::Handoff)));
    let s2 = w.hello(&c);
    let t2 = token_of(&ok(w.call(&c, s2, take(a))));
    let it = item_of(&w, Rule::IntentDone).unwrap();
    assert_eq!(it.when, Moment::Now);
    assert_eq!(
        it.call,
        Skeleton::Checkpoint {
            token: t2,
            intent_done: true,
            intent: false
        }
    );
    assert!(matches!(it.because, Because::IntentOpen { by, own: false, .. } if by == s1));
}

// ------------------------------------------------------------------------------------------------
// 4: news

#[test]
fn rule_4_rereads_a_held_node_with_entries_by_others_since_the_take() {
    let (mut w, c, s1) = worker();
    let (a, _) = mk(&mut w, &c, s1, "/a", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    // An entry before the take: not news for the taker.
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, request_cmd(a, "chat:x/1/1", Some("chat:x/1"))));
    w.advance(SECOND);
    ok(w.call(&c, s1, take(a)));
    assert!(!rules(&w).contains(&Rule::Reread));
    w.advance(SECOND);
    ok(w.call(&c, s2, request_cmd(a, "chat:x/1/2", Some("chat:x/1"))));
    ok(w.call(&c, s1, view_other()));
    assert!(has(&w, &Skeleton::ViewNode { node: a }));
    // Read: gone.
    ok(w.call(&c, s1, view(a)));
    ok(w.call(&c, s1, view_other()));
    assert!(!rules(&w).contains(&Rule::Reread));
}

#[test]
fn rule_4_rereads_the_conversation_node_of_a_held_child() {
    // §32 step 4: the take of audit suggests the conversation, summary-gap.
    let (mut w, c, s1) = worker();
    let (gap, _) = mk(&mut w, &c, s1, "/gap", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, request_cmd(gap, "chat:x/1/1", Some("chat:x/1"))));
    let audit = free(&mut w, &c, s1, "/gap/audit");
    let s3 = w.hello(&c);
    ok(w.call(&c, s3, take(audit)));
    assert!(has(&w, &Skeleton::ViewNode { node: gap }));
}

// ------------------------------------------------------------------------------------------------
// 5: unrouted entries

#[test]
fn rule_5_routes_the_session_s_entries() {
    let mut w = world_with(|_| {});
    let c = class("assistant");
    let s = w.hello(&c);
    let d = ok(w.call(
        &c,
        s,
        from_user_cmd("hello there", Some("user"), None, None, None),
    ));
    let Reply::FromUser { entry: e, .. } = d.reply else {
        panic!()
    };
    assert_eq!(
        calls(&w),
        vec![(Rule::Route, Moment::Now, Skeleton::CreateFor { entry: e })]
    );
    // 1.9 (§8.2): a result that names it leaves it unrouted (1.7 routed it to none), and rule 5
    // suggests the route to none, if nothing else in it needs work, with create as the
    // alternative.
    ok(w.call(&c, s, to_user_cmd(ToUserKind::Result, None, &[e])));
    assert_eq!(w.s.user_entries[&e].routing, Routing::Unrouted);
    let it = item_of(&w, Rule::Route).unwrap();
    assert_eq!(
        (it.call, it.because),
        (
            Skeleton::Route {
                entry: e,
                to: RouteSk::Nowhere
            },
            Because::AnsweredEntry {
                entry: e,
                or: Some(RouteAlt::Create)
            }
        )
    );
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Route {
            entry: e,
            to: Some(crate::cmd::RouteTo::Nowhere),
            answers: None,
            clarifies: None,
            reason: None,
        }),
    ));
    assert_eq!(w.s.user_entries[&e].routing, Routing::NoAction);
    assert!(!rules(&w).contains(&Rule::Route));
    // With a question for the user pending for the person: one item (1.9), its answer, with the
    // node the question is about as the alternative; two holds route nothing by themselves.
    let (n, _) = held(&mut w, &c, s, "/work");
    held(&mut w, &c, s, "/other");
    let q = question_of(&ok(w.call(&c, s, ask_user(n, None, 1))));
    let d = ok(w.call(&c, s, from_user_cmd("b, please", None, None, None, None)));
    let Reply::FromUser { entry: e2, .. } = d.reply else {
        panic!()
    };
    let routes = |w: &World| {
        w.last_guidance
            .items
            .iter()
            .filter(|i| i.rule == Rule::Route)
            .map(|i| i.call.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        routes(&w),
        vec![Skeleton::Route {
            entry: e2,
            to: RouteSk::Answers(q)
        }]
    );
    assert_eq!(
        item_of(&w, Rule::Route).unwrap().because,
        Because::RouteAnswers {
            entry: e2,
            question: q,
            or: Some(RouteAlt::Node {
                node: n,
                held: false
            })
        }
    );
    // A result that names it leaves it unrouted, and the route to none is the alternative of the
    // answer route.
    ok(w.call(&c, s, to_user_cmd(ToUserKind::Result, None, &[e2])));
    assert_eq!(w.s.user_entries[&e2].routing, Routing::Unrouted);
    assert_eq!(
        routes(&w),
        vec![Skeleton::Route {
            entry: e2,
            to: RouteSk::Answers(q)
        }]
    );
    assert_eq!(
        item_of(&w, Rule::Route).unwrap().because,
        Because::RouteAnswers {
            entry: e2,
            question: q,
            or: Some(RouteAlt::Nowhere)
        }
    );
    // `route {answers}` answers the question with the words already logged (1.7).
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Route {
            entry: e2,
            to: None,
            answers: Some(q),
            clarifies: None,
            reason: None,
        }),
    ));
    assert_eq!(
        d.reply,
        Reply::Routed {
            entry: e2,
            routed: RoutedReply::Answers { question: q },
            existing: false
        }
    );
    assert!(matches!(
        w.s.uquestions[&q].state,
        crate::state::UqState::Answered { by } if by == e2
    ));
    assert!(!rules(&w).contains(&Rule::Route));
    // An entry logged before the session took its only hold: the route to that hold, with create
    // as the alternative.
    let (mut w, c, s) = {
        let mut w = world_with(|_| {});
        let c = class("assistant");
        let s = w.hello(&c);
        (w, c, s)
    };
    let d = ok(w.call(
        &c,
        s,
        from_user_cmd("one more", Some("user"), None, None, None),
    ));
    let Reply::FromUser { entry: e4, .. } = d.reply else {
        panic!()
    };
    let (m, _) = held(&mut w, &c, s, "/one");
    assert_eq!(
        routes(&w),
        vec![Skeleton::Route {
            entry: e4,
            to: RouteSk::Node(m)
        }]
    );
    assert_eq!(
        item_of(&w, Rule::Route).unwrap().because,
        Because::RouteNode {
            entry: e4,
            node: m,
            held: true,
            or: Some(RouteAlt::Create)
        }
    );
    // `from_user {to: none}` logs words that need no work and routes them in one call.
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::FromUser {
            text: text("thanks"),
            client: text("chat"),
            principal: None,
            answers: None,
            to: Some(crate::cmd::RouteTo::Nowhere),
            clarifies: None,
            reason: None,
        }),
    ));
    let Reply::FromUser {
        entry: e3, routed, ..
    } = d.reply
    else {
        panic!()
    };
    assert_eq!(routed, RoutedReply::NoAction);
    assert_eq!(w.s.user_entries[&e3].routing, Routing::NoAction);
    assert!(
        !routes(&w)
            .iter()
            .any(|k| matches!(k, Skeleton::Route { entry, .. } if *entry == e3))
    );
}

// ------------------------------------------------------------------------------------------------
// 6 and 7: replies and decisions

#[test]
fn rule_6_reports_a_decided_user_request_to_its_requester() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let asst = class("assistant");
    let h = w.hello(&asst);
    let (_, r) = user_request(&mut w, &asst, h, a, "mail:bob@x");
    ok(w.call(&c, s, decide_cmd(a, &[r])));
    let bob = pid(&w, "mail:bob@x");
    let it = item_of(&w, Rule::Reply).unwrap();
    assert_eq!(
        it.call,
        Skeleton::ToUser {
            principal: bob,
            kind: Some(ToUserKind::Result),
            re: vec![r]
        }
    );
    assert!(!it.once, "standing on a held node");
    // A result to Alice names Bob's request: an item to Bob, misaddressed.
    ok(w.call(
        &c,
        s,
        to_user_cmd(ToUserKind::Result, Some("mail:alice@x"), &[r]),
    ));
    let it = item_of(&w, Rule::Reply).unwrap();
    assert!(matches!(it.call, Skeleton::ToUser { principal, .. } if principal == bob));
    assert!(matches!(
        it.because,
        Because::ReplyOwed {
            misaddressed: Some(_),
            ..
        }
    ));
    // An ack to Bob answers nothing: still owed.
    ok(w.call(
        &c,
        s,
        to_user_cmd(ToUserKind::Ack, Some("mail:bob@x"), &[r]),
    ));
    assert!(rules(&w).contains(&Rule::Reply));
    // The result to Bob settles it.
    ok(w.call(
        &c,
        s,
        to_user_cmd(ToUserKind::Result, Some("mail:bob@x"), &[r]),
    ));
    assert!(!rules(&w).contains(&Rule::Reply));
}

#[test]
fn rule_6_gives_one_said_per_conversation_with_its_where() {
    let (mut w, c, s) = worker();
    let (a, _) = mk(&mut w, &c, s, "/a", |x| {
        x.keys = vec![key("chat:a/1"), key("mail:m@x")]
    });
    let r1 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:a/1/1", Some("chat:a/1")),
    )));
    let r2 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:a/1/2", Some("chat:a/1")),
    )));
    let r3 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "mail:m@x/9", Some("mail:m@x")),
    )));
    ok(w.call(&c, s, decide_cmd(a, &[r1, r2, r3])));
    let saids: Vec<Skeleton> = w
        .last_guidance
        .items
        .iter()
        .filter(|i| i.rule == Rule::Reply)
        .map(|i| i.call.clone())
        .collect();
    assert_eq!(
        saids,
        vec![
            Skeleton::Said {
                at: a,
                kind: SaidKind::Result,
                where_: Some(KeyStr::new("chat:a/1").unwrap()),
                where_fill: false
            },
            Skeleton::Said {
                at: a,
                kind: SaidKind::Result,
                where_: Some(KeyStr::new("mail:m@x").unwrap()),
                where_fill: false
            },
        ]
    );
    // After the first conversation's said, only the other's.
    ok(w.call(&c, s, said_cmd(a, Some("chat:a/1"), SaidKind::Result)));
    let saids: Vec<Skeleton> = w
        .last_guidance
        .items
        .iter()
        .filter(|i| i.rule == Rule::Reply)
        .map(|i| i.call.clone())
        .collect();
    assert_eq!(
        saids,
        vec![Skeleton::Said {
            at: a,
            kind: SaidKind::Result,
            where_: Some(KeyStr::new("mail:m@x").unwrap()),
            where_fill: false
        }]
    );
}

#[test]
fn rule_6_reports_the_requests_of_two_aliases_of_one_person_in_one_item() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    person(&mut w, &c, s, "alice", &["mail:alice@x", "chat:alice"]);
    let (a, _) = held(&mut w, &c, s, "/a");
    let asst = class("assistant");
    let h = w.hello(&asst);
    let (_, r1) = user_request(&mut w, &asst, h, a, "mail:alice@x");
    let (_, r2) = user_request(&mut w, &asst, h, a, "chat:alice");
    let (_, r3) = user_request(&mut w, &asst, h, a, "mail:bob@x");
    ok(w.call(&c, s, decide_cmd(a, &[r1, r2, r3])));
    let reports: Vec<Skeleton> = w
        .last_guidance
        .items
        .iter()
        .filter(|i| i.rule == Rule::Reply)
        .map(|i| i.call.clone())
        .collect();
    assert_eq!(
        reports,
        vec![
            Skeleton::ToUser {
                principal: pid(&w, "mail:alice@x"),
                kind: Some(ToUserKind::Result),
                re: vec![r1, r2]
            },
            Skeleton::ToUser {
                principal: pid(&w, "mail:bob@x"),
                kind: Some(ToUserKind::Result),
                re: vec![r3]
            },
        ]
    );
}

/// `/a` carries `chat:x/1` and holds request `r`, decided and unanswered; `s` closes it; `/b` is
/// created with `chat:x/1` (K1 counts open carriers only); `s3` takes `/a`, which reopens it and
/// drops the key that `/b` now carries (`duplicate_key`). Returns the world, the class, `s3`,
/// `/a`, `/b`, and `r`.
fn stranded_request() -> (World, ClassName, SessionId, NodeId, NodeId, RequestId) {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    ok(w.call(&c, s, decide_cmd(a, &[r])));
    ok(w.call(&c, s, close_with(a, t, Outcome::Done, &[])));
    let (b, _) = mk(&mut w, &c, s, "/b", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    let s3 = w.hello(&c);
    let d = ok(w.call(&c, s3, take(a)));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::DuplicateKey),
        "{:?}",
        d.warnings
    );
    (w, c, s3, a, b, r)
}

#[test]
fn rule_6_suggests_no_said_where_the_conversation_left_the_requests_node() {
    // The literal item, `talk said {at: /a, where: chat:x/1}`, would be recorded on `/b`, the
    // open carrier of `chat:x/1` (§9.1 step 2), and would answer nothing: a `said` answers only
    // the requests of its own node (§9.4). Until it is settled what rule 6 should suggest here
    // (see the package report), it suggests nothing for such a request.
    let (mut w, c, s3, a, b, r) = stranded_request();
    assert!(w.s.nodes[&a].keys.is_empty());
    assert!(
        w.s.nodes[&b]
            .keys
            .contains_key(&KeyStr::new("chat:x/1").unwrap())
    );
    assert!(w.s.ix.items[&a].owed_reply.contains(&r));
    ok(w.call(&c, s3, view_other()));
    assert!(!rules(&w).contains(&Rule::Reply), "{:?}", w.last_guidance);
    // The negative: while `/a` keeps its key, the reply is suggested.
    let (mut w, c, s) = worker();
    let (a, _) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    ok(w.call(&c, s, decide_cmd(a, &[r])));
    assert_eq!(
        item_of(&w, Rule::Reply).unwrap().call,
        Skeleton::Said {
            at: a,
            kind: SaidKind::Result,
            where_: None,
            where_fill: false
        }
    );
}

/// 1.9 (§9.2): no `said` is suggested where another session holds the conversation node or work
/// inside it and the caller holds neither: it would be withheld and answer nothing, and the holder
/// speaks. The guided proptest found it (seed 145) once 1.9's groups met.
#[test]
fn rule_6_suggests_no_said_that_would_be_withheld() {
    let world = |other_holds: bool| {
        let (mut w, c, s) = worker();
        let (a, _) = mk(&mut w, &c, s, "/a", |x| {
            x.keys = vec![key("chat:x/1")];
            x.take = Some(false);
        });
        let r = request_of(&ok(w.call(
            &c,
            s,
            request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
        )));
        if other_holds {
            let s2 = w.hello(&c);
            let (_, t) = mk(&mut w, &c, s2, "/a/b", |_| {});
            assert!(t.is_some());
        }
        ok(w.call(&c, s, decide_cmd(a, &[r])));
        assert!(w.s.ix.items[&a].owed_reply.contains(&r));
        (w, a)
    };
    let (w, _) = world(true);
    assert!(!rules(&w).contains(&Rule::Reply), "{:?}", w.last_guidance);
    // The negative: with no other holder, the reply is suggested.
    let (w, a) = world(false);
    assert_eq!(
        item_of(&w, Rule::Reply).unwrap().call,
        Skeleton::Said {
            at: a,
            kind: SaidKind::Result,
            where_: None,
            where_fill: false
        }
    );
}

#[test]
fn rules_6_and_7_reach_only_the_requests_named_on_an_unheld_node() {
    let (mut w, c, s) = worker();
    let (hub, _) = mk(&mut w, &c, s, "/hub", |x| {
        x.keys = vec![key("chat:h/1")];
        x.take = Some(false);
    });
    let r1 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(hub, "chat:h/1/1", Some("chat:h/1")),
    )));
    let r2 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(hub, "chat:h/1/2", Some("chat:h/1")),
    )));
    // Both decided by an earlier decision: owed a reply.
    ok(w.call(&c, s, decide_cmd(hub, &[r1, r2])));
    let it = item_of(&w, Rule::Reply).unwrap();
    assert!(
        matches!(&it.because, Because::ReplyOwed { requests, .. } if *requests == vec![r1, r2])
    );
    // A decide naming r1 alone reaches r1 alone.
    ok(w.call(&c, s, decide_cmd(hub, &[r1])));
    let it = item_of(&w, Rule::Reply).unwrap();
    assert!(matches!(&it.because, Because::ReplyOwed { requests, .. } if *requests == vec![r1]));
    assert!(it.once);
}

#[test]
fn rule_7_decides_a_request_recorded_on_an_unheld_node_once_and_rule_6_replies_later_on_a_held_one()
{
    // R3: from_user {node} to a node nobody holds.
    let (mut w, c, s) = worker();
    let n = free(&mut w, &c, s, "/n");
    let asst = class("assistant");
    let h = w.hello(&asst);
    let (_, r) = user_request(&mut w, &asst, h, n, "mail:bob@x");
    let it = item_of(&w, Rule::Decide).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Decide {
            at: n,
            settles: vec![r],
            supersedes: None
        }
    );
    assert_eq!(it.when, Moment::Now);
    assert!(it.once);
    assert!(matches!(
        it.because,
        Because::DecisionOwed {
            recorded: true,
            held: false,
            ..
        }
    ));
    ok(w.call(&asst, h, view_other()));
    assert!(!rules(&w).contains(&Rule::Decide));
    // 1.7: on a node the caller holds, no decision: rule 6's reply, later and once, then rule 8's
    // close, which records the decision (§5).
    let (m, _) = held(&mut w, &c, s, "/m");
    let d = ok(w.call(&c, s, request_cmd(m, "chat:m/1/1", None)));
    let r = request_of(&d);
    assert!(!rules(&w).contains(&Rule::Decide), "{:?}", calls(&w));
    let it = item_of(&w, Rule::Reply).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert!(it.once);
    assert!(matches!(
        &it.because,
        Because::ReplyOwed {
            recorded: true,
            ready: false,
            requests,
            ..
        } if *requests == vec![r]
    ));
    assert!(matches!(
        it.call,
        Skeleton::Said {
            at,
            kind: SaidKind::Result,
            ..
        } if at == m
    ));
    // On a node another session holds: nothing.
    let other = w.hello(&c);
    let (o, _) = held(&mut w, &c, other, "/o");
    ok(w.call(&c, s, request_cmd(o, "chat:o/1/1", None)));
    assert!(!rules(&w).contains(&Rule::Decide));
}

#[test]
fn rule_7_gives_way_to_rule_8_s_close() {
    // A held node whose requests are answered and not decided: rule 8 closes it, settling them by
    // default (1.7, §5), and rule 7 suggests no separate decision there.
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    ok(w.call(&c, s, said_cmd(a, None, SaidKind::Result)));
    assert!(w.s.ix.items[&a].owed_decision.contains(&r));
    assert!(has(
        &w,
        &Skeleton::Close {
            node: a,
            token: t,
            settles: vec![],
            outcome: None
        }
    ));
    assert!(!rules(&w).contains(&Rule::Decide), "{:?}", w.last_guidance);
    // With an open child the close is unclean, so rule 7 suggests the decision.
    free(&mut w, &c, s, "/a/child");
    ok(w.call(&c, s, view_other()));
    assert!(!rules(&w).contains(&Rule::Close));
    assert_eq!(
        item_of(&w, Rule::Decide).unwrap().call,
        Skeleton::Decide {
            at: a,
            settles: vec![r],
            supersedes: None
        }
    );
    // 1.7 brought the decision now after a release that left it undecided; 1.10: the release came
    // after the answer, so the answer was a partial report, and the request is left unfinished:
    // no decision, which the close, or a decide once the work is done, makes.
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    assert!(!rules(&w).contains(&Rule::Decide), "{:?}", w.last_guidance);
}

/// 1.7 (§5): a close without `settles` settles every unsettled request of its node that a result
/// answered: a user request by a `to_user` of kind `result`, a person's by a `said` of kind
/// `result`; a `said` the close lists in `unsent` answers nothing by default, and `settles: []`
/// settles none. The close confirms the session's pending `said` entries `with_reply`, and an
/// explicit confirm is no `with_reply`.
#[test]
fn a_close_settles_what_a_result_answered() {
    let (mut w, c, s) = worker();
    // A user request, answered by a result.
    let (a, ta) = mk(&mut w, &c, s, "/a", |_| {});
    let asst = class("assistant");
    let h = w.hello(&asst);
    let (_, r) = user_request(&mut w, &asst, h, a, "mail:bob@x");
    ok(w.call(
        &asst,
        h,
        to_user_cmd(ToUserKind::Result, Some("mail:bob@x"), &[r]),
    ));
    let d = ok(w.call(&c, s, close_default(a, ta.unwrap())));
    assert!(
        matches!(&d.reply, Reply::Closed { settled, not_settled, decision: Some(_), .. }
        if *settled == vec![r] && not_settled.is_empty())
    );
    assert!(w.s.requests[&r].settled.is_some());
    // A person's request, answered by a result the close lists in `unsent`: not settled, and why.
    let (b, tb) = mk(&mut w, &c, s, "/b", |x| x.keys = vec![key("chat:x/2")]);
    let r2 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(b, "chat:x/2/1", Some("chat:x/2")),
    )));
    let y = said_of(&ok(w.call(&c, s, said_cmd(b, None, SaidKind::Result))));
    let mut cl = close_default(b, tb.unwrap());
    if let Cmd::Node(NodeCmd::Close { unsent, .. }) = &mut cl {
        *unsent = [y].into();
    }
    let d = ok(w.call(&c, s, cl));
    assert!(
        matches!(&d.reply, Reply::Closed { settled, decision: None, not_settled, .. }
        if settled.is_empty() && *not_settled == vec![crate::reply::NotSettled {
            request: r2,
            why: crate::reply::NotSettledWhy::AwaitsDecision,
        }])
    );
    // A result the close confirms settles it, `with_reply`; `settles: []` settles none.
    let (m, tm) = mk(&mut w, &c, s, "/m", |x| x.keys = vec![key("chat:x/3")]);
    let r3 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(m, "chat:x/3/1", Some("chat:x/3")),
    )));
    let y3 = said_of(&ok(w.call(&c, s, said_cmd(m, None, SaidKind::Result))));
    let d = ok(w.call(&c, s, close_with(m, tm.unwrap(), Outcome::Done, &[])));
    assert!(matches!(&d.reply, Reply::Closed { settled, .. } if settled.is_empty()));
    assert!(w.s.requests[&r3].settled.is_none());
    assert!(w.s.saids[&y3].with_reply);
    // An explicit confirm is no `with_reply`.
    let (n, _) = mk(&mut w, &c, s, "/n", |x| x.keys = vec![key("chat:x/4")]);
    let y4 = said_of(&ok(w.call(&c, s, said_cmd(n, None, SaidKind::Ack))));
    ok(w.call(&c, s, confirm(y4)));
    assert_eq!(w.s.saids[&y4].state, crate::state::SaidState::Confirmed);
    assert!(!w.s.saids[&y4].with_reply);
}

/// 1.7 (§5): a request answered by a `said` of kind `reply` alone is not settled by a close's
/// default, so rule 8's close names it in `settles`.
#[test]
fn rule_8_names_a_request_a_reply_answered() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    ok(w.call(&c, s, said_cmd(a, None, SaidKind::Reply)));
    assert!(has(
        &w,
        &Skeleton::Close {
            node: a,
            token: t,
            settles: vec![r],
            outcome: None
        }
    ));
    // The default alone would leave it, and say why.
    let d = ok(w.call(&c, s, close_default(a, t)));
    let Reply::Closed {
        settled,
        not_settled,
        ..
    } = &d.reply
    else {
        panic!()
    };
    assert!(settled.is_empty());
    assert_eq!(
        not_settled,
        &vec![crate::reply::NotSettled {
            request: r,
            why: crate::reply::NotSettledWhy::AwaitsDecision
        }]
    );
}

#[test]
fn rule_7_after_a_close_that_left_requests_without_a_decision() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    let out = w.call(&c, s, close_with(a, t, Outcome::Done, &[]));
    assert!(out.is_ok());
    let it = item_of(&w, Rule::Decide).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Decide {
            at: a,
            settles: vec![r],
            supersedes: None
        }
    );
    assert!(matches!(
        it.because,
        Because::DecisionOwed {
            after_close: true,
            ..
        }
    ));
}

#[test]
fn rule_6_on_ready_to_close_of_a_held_node_and_nothing_on_an_unheld_one() {
    let (mut w, c, s) = worker();
    let (p, _) = mk(&mut w, &c, s, "/p", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(p, "chat:x/1/1", Some("chat:x/1")),
    )));
    let (k, kt) = held(&mut w, &c, s, "/p/k");
    ok(w.call(&c, s, close_with(k, kt, Outcome::Done, &[])));
    // `ready_to_close` on the unheld parent: nothing.
    assert!(!rules(&w).contains(&Rule::Decide));
    assert!(!rules(&w).contains(&Rule::Reply));
    // 1.7: once held, the reply, now, and no decision: the close records it (§5).
    ok(w.call(&c, s, take(p)));
    assert!(!rules(&w).contains(&Rule::Decide));
    let it = item_of(&w, Rule::Reply).unwrap();
    assert_eq!(it.when, Moment::Now);
    assert!(!it.once);
    assert!(matches!(
        &it.because,
        Because::ReplyOwed { ready: true, requests, .. } if *requests == vec![r]
    ));
}

// ------------------------------------------------------------------------------------------------
// 8: closing and releasing

#[test]
fn rule_8_closes_after_the_last_request_settles_and_never_when_unclean() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    ok(w.call(&c, s, decide_cmd(a, &[r])));
    ok(w.call(&c, s, said_cmd(a, None, SaidKind::Result)));
    let it = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Close {
            node: a,
            token: t,
            settles: vec![],
            outcome: None
        }
    );
    assert_eq!(it.because, Because::Finished);
    assert!(it.once);
    // An open child makes it unclean.
    let (mut w, c, s) = worker();
    let (a, _) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    free(&mut w, &c, s, "/a/child");
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/1/1", Some("chat:x/1")),
    )));
    ok(w.call(&c, s, decide_cmd(a, &[r])));
    ok(w.call(&c, s, said_cmd(a, None, SaidKind::Result)));
    assert!(!rules(&w).contains(&Rule::Close));
}

#[test]
fn rule_8_never_closes_with_an_open_commitment_or_another_holder_or_intent() {
    // A commitment (an ack) left open.
    let (mut w, c, s) = worker();
    let (a, _) = mk(&mut w, &c, s, "/a", |x| {
        x.keys = vec![key("chat:x/1"), key("mail:m@x")];
    });
    ok(w.call(&c, s, said_cmd(a, Some("mail:m@x"), SaidKind::Ack)));
    ok(w.call(&c, s, decide_cmd(a, &[])));
    assert!(!rules(&w).contains(&Rule::Close));
    // A second holder.
    let (mut w, c, s) = worker();
    let (b, _) = mk(&mut w, &c, s, "/b", |x| x.capacity = Some(2));
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(b)));
    ok(w.call(&c, s, decide_cmd(b, &[])));
    assert!(!rules(&w).contains(&Rule::Close));
    // Another session's open intent.
    let (mut w, c, s1) = worker();
    let (n, t1) = held(&mut w, &c, s1, "/n");
    ok(w.call(&c, s1, checkpoint_with(t1, Some("deploy"), None)));
    ok(w.call(&c, s1, release(t1, ReleaseReason::Handoff)));
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(n)));
    ok(w.call(&c, s2, decide_cmd(n, &[])));
    assert!(!rules(&w).contains(&Rule::Close));
}

#[test]
fn rule_8_releases_a_person_and_never_closes_it() {
    let (mut w, c, s) = worker();
    let (bob, t) = mk(&mut w, &c, s, "/@people/bob", |x| {
        x.keys = vec![key("person:mail:bob@x")]
    });
    let t = t.unwrap();
    let it = item_of(&w, Rule::ReleaseContainer).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert_eq!(
        it.call,
        Skeleton::Release {
            token: t,
            wait: None
        }
    );
    assert_eq!(it.because, Because::ContainerStays { series: false });
    ok(w.call(&c, s, decide_cmd(bob, &[])));
    assert!(!rules(&w).contains(&Rule::Close));
}

#[test]
fn rule_8_releases_a_container_once_its_last_request_is_settled() {
    // A person with two requests: settling the first leaves one, so no release; settling the
    // second releases it, at once and once.
    let (mut w, c, s) = worker();
    let (bob, t) = mk(&mut w, &c, s, "/@people/bob", |x| {
        x.keys = vec![key("person:mail:bob@x"), key("chat:bob/1")]
    });
    let t = t.unwrap();
    let r1 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(bob, "chat:bob/1/1", Some("chat:bob/1")),
    )));
    let r2 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(bob, "chat:bob/1/2", Some("chat:bob/1")),
    )));
    // The decision of r1, then a result that answers both: r1 settles, r2 still needs its
    // decision.
    ok(w.call(&c, s, decide_cmd(bob, &[r1])));
    let d = ok(w.call(&c, s, said_cmd(bob, Some("chat:bob/1"), SaidKind::Result)));
    assert!(matches!(&d.reply, Reply::Said { settled, .. } if *settled == vec![r1]));
    assert!(!rules(&w).contains(&Rule::ReleaseContainer));
    let d = ok(w.call(&c, s, decide_cmd(bob, &[r2])));
    assert!(matches!(&d.reply, Reply::Decided { settled, .. } if *settled == vec![r2]));
    let it = item_of(&w, Rule::ReleaseContainer).unwrap();
    assert_eq!(it.when, Moment::Now);
    assert!(it.once);
    assert_eq!(
        it.call,
        Skeleton::Release {
            token: t,
            wait: None
        }
    );
    assert!(!rules(&w).contains(&Rule::Close));
}

#[test]
fn rule_8_closes_later_after_an_intent_done_that_opens_none() {
    // R11.
    let (mut w, c, s) = worker();
    let (a, t) = held(&mut w, &c, s, "/a");
    ok(w.call(&c, s, checkpoint_with(t, Some("deploy"), None)));
    ok(w.call(&c, s, checkpoint_with(t, None, Some("deployed"))));
    let it = item_of(&w, Rule::Close).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert!(it.once);
    assert_eq!(
        it.call,
        Skeleton::Close {
            node: a,
            token: t,
            settles: vec![],
            outcome: None
        }
    );
    assert_eq!(it.because, Because::Finished);
    // A swap (done and a new intent) suggests no close.
    ok(w.call(&c, s, checkpoint_with(t, Some("again"), None)));
    ok(w.call(&c, s, checkpoint_with(t, Some("third"), Some("done"))));
    assert!(!rules(&w).contains(&Rule::Close));
}

#[test]
fn rule_8_recloses_a_node_a_take_reopened_once_its_flag_is_cleared() {
    // R4: a lemma closed failed, reopened by a take for its `basis_changed`, closed again with
    // `failed` once a superseding decision clears the flag.
    let (mut w, c, s) = worker();
    let (lemma, lt) = held(&mut w, &c, s, "/lemma");
    let (thm, tt) = held(&mut w, &c, s, "/thm");
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(thm),
            text: text("by the lemma"),
            settles: BTreeSet::new(),
            supersedes: None,
            basis: vec![BasisArg::Node {
                at: id(lemma),
                version: None,
            }],
            evidence: None,
        }),
    ));
    let Reply::Decided { decision, .. } = d.reply else {
        panic!()
    };
    ok(w.call(&c, s, close_with(thm, tt, Outcome::Failed, &[])));
    // Retract the lemma's support: retract the lemma node itself.
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Node(id(lemma)),
            text: text("wrong"),
            reason: text("counterexample"),
        }),
    ));
    ok(w.call(&c, s, close_with(lemma, lt, Outcome::Failed, &[])));
    assert!(
        w.s.ix.node[&thm].flags.contains(Flag::BasisChanged),
        "the theorem is flagged"
    );
    let d = ok(w.call(&c, s, take(thm)));
    let Reply::Taken {
        reopened: Some(Outcome::Failed),
        token: t2,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert!(!rules(&w).contains(&Rule::Close));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(thm),
            text: text("refuted"),
            settles: BTreeSet::new(),
            supersedes: Some(decision),
            basis: Vec::new(),
            evidence: None,
        }),
    ));
    let it = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Close {
            node: thm,
            token: t2,
            settles: vec![],
            outcome: Some(Outcome::Failed)
        }
    );
    assert_eq!(
        it.because,
        Because::Reclose {
            was: Outcome::Failed
        }
    );
}

/// A free node `/a` carrying `chat:x/1`, with `n` `said` entries of kind `reply` recorded by `s1`,
/// which then falls silent, so they are unconfirmed; `s2` takes `/a` and closes it as done, which
/// flags it `unconfirmed_said`; `s3` takes it again, which reopens it. Returns the world, the
/// class, `s3`, the node, `s3`'s token, and the said entries.
fn reopened_with_unconfirmed_saids(
    n: usize,
) -> (World, ClassName, SessionId, NodeId, Token, Vec<Eid>) {
    let (mut w, c, s1) = worker();
    let (a, _) = mk(&mut w, &c, s1, "/a", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    // 1.11: s1 holds other work, whose lease then ends, so its said entries stay pending.
    held(&mut w, &c, s1, "/busy");
    let ys: Vec<Eid> = (0..n)
        .map(|_| said_of(&ok(w.call(&c, s1, said_cmd(a, None, SaidKind::Reply)))))
        .collect();
    w.advance(Dur(w.s.params.presence_silence.0 + MINUTE.0));
    for y in &ys {
        assert_eq!(w.s.saids[y].state, crate::state::SaidState::Unconfirmed);
    }
    let s2 = w.hello(&c);
    let t2 = token_of(&ok(w.call(&c, s2, take(a))));
    ok(w.call(&c, s2, close_with(a, t2, Outcome::Done, &[])));
    assert!(w.s.ix.node[&a].flags.contains(Flag::UnconfirmedSaid));
    let s3 = w.hello(&c);
    let d = ok(w.call(&c, s3, take(a)));
    assert!(
        matches!(
            d.reply,
            Reply::Taken {
                reopened: Some(Outcome::Done),
                ..
            }
        ),
        "{:?}",
        d.reply
    );
    let t3 = token_of(&d);
    (w, c, s3, a, t3, ys)
}

#[test]
fn rule_8_recloses_once_a_confirm_alone_clears_the_last_flag() {
    // R4's own branch: no decision, no settled request; only the confirmation clears the flag.
    let (mut w, c, s3, a, t3, ys) = reopened_with_unconfirmed_saids(1);
    assert!(
        !rules(&w).contains(&Rule::Close),
        "not while the flag stands"
    );
    ok(w.call(&c, s3, confirm(ys[0])));
    let it = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Close {
            node: a,
            token: t3,
            settles: vec![],
            outcome: Some(Outcome::Done)
        }
    );
    assert_eq!(it.when, Moment::Now);
    assert!(it.once);
    assert_eq!(it.because, Because::Reclose { was: Outcome::Done });
    // One-shot: a later read does not repeat it.
    ok(w.call(&c, s3, view_other()));
    assert!(!rules(&w).contains(&Rule::Close));
}

#[test]
fn rule_8_does_not_reclose_while_another_flag_a_close_would_leave_remains() {
    // Two unconfirmed saids of another session: confirming one leaves the other, which a closed
    // node would still be flagged for (`leaves(n) ≠ ∅`).
    let (mut w, c, s3, _, _, ys) = reopened_with_unconfirmed_saids(2);
    ok(w.call(&c, s3, confirm(ys[0])));
    assert!(!rules(&w).contains(&Rule::Close));
    ok(w.call(&c, s3, confirm(ys[1])));
    assert!(rules(&w).contains(&Rule::Close));
}

/// A take that reopened a node, an objection raised on it while held, then resolved: the close
/// comes back with the outcome the node had. 1.9: an objection on the closed node itself would make
/// it a review, which no take reopens (`a_closed_node_with_an_objection_is_reviewed_not_taken`).
#[test]
fn rule_8_recloses_after_a_resolve_clears_the_objection() {
    let (mut w, c, s3, a, t3, ys) = reopened_with_unconfirmed_saids(1);
    let d = ok(w.call(
        &c,
        s3,
        Cmd::Talk(TalkCmd::Object {
            on: id(a),
            note: text("wrong"),
        }),
    ));
    let Reply::Objected { objection } = d.reply else {
        panic!()
    };
    ok(w.call(&c, s3, confirm(ys[0])));
    assert!(!rules(&w).contains(&Rule::Close), "not while it stands");
    ok(w.call(
        &c,
        s3,
        Cmd::Talk(TalkCmd::Resolve {
            objection,
            note: text("fixed"),
            upheld: false,
            defer: false,
        }),
    ));
    let it = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Close {
            node: a,
            token: t3,
            settles: vec![],
            outcome: Some(Outcome::Done)
        }
    );
    assert!(it.once);
    assert_eq!(it.because, Because::Reclose { was: Outcome::Done });
}

/// 1.9 (§10.1, §10.4, §11.3; §20.9 rules E, 9, and 13): a closed node with an open objection is
/// reviewed, not taken. A take is `closed`, whose remedy is the resolve; `next` gives it as a
/// review item with no hold, and rule 9 the resolve, whose `upheld` the agent writes; a worker's
/// tidy item for it is that resolve, never a take. In the lab's fourth round, `next` and `take`
/// reopened four refuted claims and no objection was upheld.
#[test]
fn a_closed_node_with_an_objection_is_reviewed_not_taken() {
    let (mut w, c, s1) = worker();
    let (x, tx) = held(&mut w, &c, s1, "/p/x");
    ok(w.call(&c, s1, close_with(x, tx, Outcome::Done, &[])));
    // 1.12 (§10.4): another session objects; its own objection is never its review.
    let s0 = w.hello(&c);
    let d = ok(w.call(
        &c,
        s0,
        Cmd::Talk(TalkCmd::Object {
            on: id(x),
            note: text("a counterexample"),
        }),
    ));
    let Reply::Objected { objection } = d.reply else {
        panic!()
    };
    let d = ok(w.call(&c, s0, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Empty { .. })),
        "{:?}",
        d.reply
    );
    // Rule 13: the tidy item of a sibling's close is the review.
    let (y, ty) = held(&mut w, &c, s1, "/p/y");
    let d = ok(w.call(&c, s1, close_with(y, ty, Outcome::Done, &[])));
    assert!(
        matches!(&d.reply, Reply::Closed { leave_better, .. } if leave_better.first() == Some(&x))
    );
    let it = item_of(&w, Rule::Tidy).unwrap();
    assert_eq!(
        (it.call, it.because),
        (
            Skeleton::Resolve { objection },
            Because::Review { objection }
        )
    );
    // E: a take is refused with `closed`, and the review is the remedy.
    let s2 = w.hello(&c);
    let r = w.call(&c, s2, take(x));
    assert_eq!(r.err().map(|e| e.code), Some(ErrorCode::Closed));
    let it = item_of(&w, Rule::Remedy).unwrap();
    assert_eq!(
        (it.call, it.because),
        (
            Skeleton::Resolve { objection },
            Because::Review { objection }
        )
    );
    assert!(!w.s.nodes[&x].is_open());
    // `next`: the review, with no hold and nothing reopened, and rule 9's resolve.
    let d = ok(w.call(&c, s2, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Review { node, objection: o, .. })
            if node == x && o == objection),
        "{:?}",
        d.reply
    );
    assert!(!w.s.nodes[&x].is_open());
    assert!(w.s.holds.keys().all(|t| t.node != x));
    let it = item_of(&w, Rule::Review).unwrap();
    assert_eq!(
        (it.when, it.call.clone(), it.once),
        (Moment::Now, Skeleton::Resolve { objection }, true)
    );
    // Upheld: the claim is corrected and stays closed; nothing offers it any more.
    let d = follow_upheld(&mut w, &c, s2, objection);
    assert!(matches!(
        d.reply,
        Reply::ObjectionResolved {
            corrected: Some(_),
            ..
        }
    ));
    assert!(!w.s.nodes[&x].is_open());
    assert!(crate::query::review_objection(&w.s, x).is_none());
}

/// A resolve of `objection` with `upheld: true`, by `s`.
fn follow_upheld(w: &mut World, c: &ClassName, s: SessionId, objection: Eid) -> Done {
    ok(w.call(
        c,
        s,
        Cmd::Talk(TalkCmd::Resolve {
            objection,
            note: text("it stands"),
            upheld: true,
            defer: false,
        }),
    ))
}

#[test]
fn rule_8_recloses_after_a_retraction_clears_the_stale_basis() {
    // A theorem closed `failed` on a lemma's support; the lemma is retracted, so the theorem's
    // decision has a stale basis (`basis_changed`); a take reopens it, and retracting that
    // decision clears the flag.
    let (mut w, c, s) = worker();
    let (lemma, lt) = held(&mut w, &c, s, "/lemma");
    let (thm, tt) = held(&mut w, &c, s, "/thm");
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(thm),
            text: text("by the lemma"),
            settles: BTreeSet::new(),
            supersedes: None,
            basis: vec![BasisArg::Node {
                at: id(lemma),
                version: None,
            }],
            evidence: None,
        }),
    ));
    let Reply::Decided { decision, .. } = d.reply else {
        panic!()
    };
    ok(w.call(&c, s, close_with(thm, tt, Outcome::Failed, &[])));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Node(id(lemma)),
            text: text("wrong"),
            reason: text("counterexample"),
        }),
    ));
    ok(w.call(&c, s, close_with(lemma, lt, Outcome::Failed, &[])));
    assert!(w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    let t2 = token_of(&ok(w.call(&c, s, take(thm))));
    assert!(!rules(&w).contains(&Rule::Close));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: decision,
                node: thm,
            },
            text: text("withdrawn"),
            reason: text("its support is gone"),
        }),
    ));
    let it = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Close {
            node: thm,
            token: t2,
            settles: vec![],
            outcome: Some(Outcome::Failed)
        }
    );
    assert!(it.once);
}

// ------------------------------------------------------------------------------------------------
// 9: what the call made due

#[test]
fn rule_9_after_asking_a_person_releases_now_if_blocking_later_otherwise() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let q = question_of(&ok(w.call(&c, s, ask_person(a, None, true))));
    let it = item_of(&w, Rule::AskWait).unwrap();
    assert_eq!(it.when, Moment::Now);
    assert_eq!(
        it.call,
        Skeleton::Release {
            token: t,
            wait: Some(q)
        }
    );
    let (mut w, c, s) = worker();
    let (a, _) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    ok(w.call(&c, s, ask_person(a, None, false)));
    assert_eq!(item_of(&w, Rule::AskWait).unwrap().when, Moment::Later);
    // Asked again: "already asked".
    ok(w.call(&c, s, ask_person(a, None, false)));
    assert!(matches!(
        item_of(&w, Rule::AskWait).unwrap().because,
        Because::AskWait { existing: true, .. }
    ));
}

#[test]
fn rule_9_after_asking_agents_waits_later() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let s2 = w.hello(&c);
    ok(w.call(&c, s, ask_agent(a, AgentAddressee::Session(s2))));
    let it = item_of(&w, Rule::AgentWait).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert_eq!(
        it.call,
        Skeleton::Wait {
            timeout: w.s.params.talk_wait_max
        }
    );
    // The addressee's next delivery suggests the answer (rule 11), never to the asker.
    let d = ok(w.call(&c, s2, wait_cmd()));
    let Reply::Waited { delivered, .. } = d.reply else {
        panic!()
    };
    assert_eq!(delivered.len(), 1);
    assert!(rules(&w).contains(&Rule::AnswerAgent));
    let s3 = w.hello(&c);
    ok(w.call(&c, s, ask_agent(a, AgentAddressee::Session(s))));
    ok(w.call(&c, s, wait_cmd()));
    assert!(
        !rules(&w).contains(&Rule::AnswerAgent),
        "never one's own question"
    );
    let _ = s3;
}

#[test]
fn rule_9_after_a_join_with_an_open_question_records_the_heard() {
    let (mut w, c, s1) = worker();
    let (a, _) = mk(&mut w, &c, s1, "/a", |x| {
        x.keys = vec![key("chat:x/1"), key("mail:m@x")];
    });
    let q = question_of(&ok(w.call(&c, s1, ask_person(a, Some("mail:m@x"), false))));
    let s2 = w.hello(&c);
    ok(w.call(
        &c,
        s2,
        create_with("/b", |x| x.keys = vec![key("mail:m@x")]),
    ));
    let it = item_of(&w, Rule::JoinAnswer).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Heard {
            at: a,
            answers: q,
            where_: KeyStr::new("mail:m@x").unwrap()
        }
    );
}

#[test]
fn rule_9_offers_split_off_work_nothing_offers() {
    let (mut w, c, s) = worker();
    let d = ok(w.call(&c, s, create_with("/a", |x| x.take = Some(false))));
    let (a, _) = created(&d);
    assert!(has(
        &w,
        &Skeleton::AddOffer {
            node: a,
            version: w.s.nodes[&a].version
        }
    ));
    // Never with the tag, or capacity 0.
    ok(w.call(
        &c,
        s,
        create_with("/b", |x| {
            x.take = Some(false);
            x.tags = [crate::apply::tests::label("offer")].into();
        }),
    ));
    assert!(!rules(&w).contains(&Rule::Offer));
    ok(w.call(
        &c,
        s,
        create_with("/z", |x| {
            x.take = Some(false);
            x.capacity = Some(0);
        }),
    ));
    assert!(!rules(&w).contains(&Rule::Offer));
}

#[test]
fn rule_9_status_replies_one_per_conversation_whose_commitment_is_due() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| {
        x.keys = vec![key("chat:a/1"), key("mail:m@x"), key("issue:p#1")];
    });
    let t = t.unwrap();
    for k in ["chat:a/1", "mail:m@x"] {
        let y = said_of(&ok(w.call(&c, s, said_cmd(a, Some(k), SaidKind::Ack))));
        ok(w.call(&c, s, confirm(y)));
    }
    // The node waits on a time, so its due commitments make it a status item (D.8.5).
    let far = w.clock.plus(Dur(30 * 24 * HOUR.0));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Release {
            token: t,
            done: text("asked"),
            next_step: text("wait"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Time(When::At(far))),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
    w.advance(Dur(w.s.params.commitment_followup.0 + MINUTE.0));
    // A third conversation's commitment, not yet due.
    let y = said_of(&ok(w.call(
        &c,
        s,
        said_cmd(a, Some("issue:p#1"), SaidKind::Ack),
    )));
    ok(w.call(&c, s, confirm(y)));
    let s2 = w.hello(&c);
    let d = ok(w.call(&c, s2, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Status { node, .. }) if node == a),
        "{:?}",
        d.reply
    );
    let saids: Vec<Skeleton> = w
        .last_guidance
        .items
        .iter()
        .filter(|i| i.rule == Rule::StatusReply)
        .map(|i| i.call.clone())
        .collect();
    assert_eq!(
        saids,
        vec![
            Skeleton::Said {
                at: a,
                kind: SaidKind::Reply,
                where_: Some(KeyStr::new("chat:a/1").unwrap()),
                where_fill: false
            },
            Skeleton::Said {
                at: a,
                kind: SaidKind::Reply,
                where_: Some(KeyStr::new("mail:m@x").unwrap()),
                where_fill: false
            },
        ]
    );
}

#[test]
fn rule_9_suggests_no_status_reply_where_the_conversation_left_the_node() {
    // As rule 6 (`rule_6_suggests_no_said_where_the_conversation_left_the_requests_node`): a
    // reply at `/a` in `chat:x/1` would be recorded on `/b`, which now carries the key, and would
    // fulfil nothing on `/a` (§9.2); the question is open (see the package report).
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let y = said_of(&ok(w.call(&c, s, said_cmd(a, None, SaidKind::Ack))));
    ok(w.call(&c, s, confirm(y)));
    ok(w.call(&c, s, close_with(a, t, Outcome::Done, &[])));
    mk(&mut w, &c, s, "/b", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    let s3 = w.hello(&c);
    let t3 = token_of(&ok(w.call(&c, s3, take(a))));
    assert!(w.s.nodes[&a].keys.is_empty());
    let far = w.clock.plus(Dur(30 * 24 * HOUR.0));
    ok(w.call(
        &c,
        s3,
        Cmd::Hold(HoldCmd::Release {
            token: t3,
            done: text("checked"),
            next_step: text("wait"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Time(When::At(far))),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
    w.advance(Dur(w.s.params.commitment_followup.0 + MINUTE.0));
    let s4 = w.hello(&c);
    let d = ok(w.call(&c, s4, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Status { node, .. }) if node == a),
        "{:?}",
        d.reply
    );
    assert!(
        !rules(&w).contains(&Rule::StatusReply),
        "{:?}",
        w.last_guidance
    );
}

#[test]
fn rule_9_routes_a_routing_item_that_next_returned() {
    let mut w = world_with(|_| {});
    let asst = class("assistant");
    let h = open_as(&mut w, &asst, Some("mail:alice@x"));
    let d = ok(w.call(&asst, h, from_user_cmd("hello", None, None, None, None)));
    let Reply::FromUser { entry, .. } = d.reply else {
        panic!()
    };
    let c = class("worker");
    let s = w.hello(&c);
    let d = ok(w.call(&c, s, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Route { entry: e, .. }) if e == entry),
        "{:?}",
        d.reply
    );
    let it = item_of(&w, Rule::RouteItem).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Route {
            entry,
            to: RouteSk::Fill
        }
    );
    assert_eq!(it.when, Moment::Now);
    assert!(it.once);
    assert_eq!(it.because, Because::RouteItem);
    // The negative: once the entry is routed, a `next` that returns a hold suggests no route.
    ok(w.call(
        &asst,
        h,
        Cmd::Talk(TalkCmd::Route {
            entry,
            to: Some(crate::cmd::RouteTo::Nowhere),
            answers: None,
            clarifies: None,
            reason: Some(text("a greeting")),
        }),
    ));
    ok(w.call(
        &c,
        s,
        create_with("/x", |a| {
            a.take = Some(false);
            a.tags = [crate::apply::tests::label("offer")].into();
        }),
    ));
    let s2 = w.hello(&c);
    let d = ok(w.call(&c, s2, crate::apply::tests::next()));
    assert!(
        matches!(d.reply, Reply::Next(NextReply::Hold { .. })),
        "{:?}",
        d.reply
    );
    assert!(!rules(&w).contains(&Rule::RouteItem));
}

#[test]
fn rule_9_after_a_question_to_the_user_waits_for_the_answer() {
    let mut w = world_with(|_| {});
    let c = class("assistant");
    let s = open_as(&mut w, &c, Some("mail:alice@x"));
    let (a, _) = held(&mut w, &c, s, "/a");
    let q = question_of(&ok(w.call(&c, s, ask_user(a, Some("mail:alice@x"), 1))));
    ok(w.call(
        &c,
        s,
        to_user_cmd(ToUserKind::Question, Some("mail:alice@x"), &[]),
    ));
    let it = item_of(&w, Rule::AnswersNext).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert_eq!(
        it.call,
        Skeleton::AnswerUser {
            question: q,
            principal: None
        }
    );
}

// ------------------------------------------------------------------------------------------------
// 10: flags on held nodes

/// 1.9 (§9.2): another session's unconfirmed `said` gets no item; its brief says to check the
/// conversation. 1.8 suggested confirming it.
#[test]
fn rule_10_never_confirms_another_session_s_unconfirmed_said() {
    let (mut w, c, s1) = worker();
    let (a, _) = mk(&mut w, &c, s1, "/a", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    // 1.11: s1 holds other work, whose lease then ends, so its said stays pending.
    held(&mut w, &c, s1, "/busy");
    let y = said_of(&ok(w.call(&c, s1, said_cmd(a, None, SaidKind::Ack))));
    // s1 falls silent: its said becomes unconfirmed.
    w.advance(Dur(w.s.params.presence_silence.0 + MINUTE.0));
    assert_eq!(w.s.saids[&y].state, crate::state::SaidState::Unconfirmed);
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(a)));
    assert!(!rules(&w).contains(&Rule::HeldFlag), "{:?}", calls(&w));
    assert!(!confirms(&w, s2), "{:?}", calls(&w));
    assert_eq!(w.s.saids[&y].state, crate::state::SaidState::Unconfirmed);
}

#[test]
fn rule_10_resolves_an_objection_later_and_splits_only_where_a_child_fits() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Object {
            on: id(a),
            note: text("wrong"),
        }),
    ));
    let Reply::Objected { objection } = d.reply else {
        panic!()
    };
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert_eq!(it.call, Skeleton::Resolve { objection });
    assert_eq!(
        it.because,
        Because::Objection {
            objection,
            retracted: None
        }
    );
    // 1.9: a decision already retracted, with none active, is named in the `why`, which no longer
    // asks to decide or retract.
    let d = ok(w.call(&c, s, decide_cmd(a, &[])));
    let Reply::Decided { decision, .. } = d.reply else {
        panic!()
    };
    assert_eq!(
        item_of(&w, Rule::HeldFlag).unwrap().because,
        Because::Objection {
            objection,
            retracted: None
        }
    );
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: decision,
                node: a,
            },
            text: text("refuted"),
            reason: text("counterexample"),
        }),
    ));
    assert_eq!(
        item_of(&w, Rule::HeldFlag).unwrap().because,
        Because::Objection {
            objection,
            retracted: Some(decision)
        }
    );
    // too_big: three expired holds without a checkpoint; a child fits.
    let mut w = world_with(|p| p.too_big_attempts = 1);
    let c = class("worker");
    let s = w.hello(&c);
    let (b, _) = held(&mut w, &c, s, "/b");
    w.advance(Dur(w.s.params.lease_first.0 + MINUTE.0));
    assert!(w.s.nodes[&b].attempts >= 1);
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(b)));
    assert!(has(&w, &Skeleton::Split { parent: b }));
    // No room under it: no split.
    let mut w = world_with(|p| {
        p.too_big_attempts = 1;
        p.tree_max_depth = 3;
    });
    let s = w.hello(&c);
    let (deep, _) = held(&mut w, &c, s, "/x/y/z");
    w.advance(Dur(w.s.params.lease_first.0 + MINUTE.0));
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(deep)));
    assert!(!rules(&w).contains(&Rule::HeldFlag));
}

#[test]
fn rule_10_records_the_recheck_of_a_question() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let q = question_of(&ok(w.call(&c, s, ask_person(a, None, false))));
    let wait = Cmd::Hold(HoldCmd::Release {
        token: t,
        done: text("asked"),
        next_step: text("wait"),
        reason: ReleaseReason::Blocked,
        waits_for: Some(WaitArg::Question(q)),
        intent_done: None,
        learnings: None,
        unsent: BTreeSet::new(),
        settles: BTreeSet::new(),
    });
    ok(w.call(&c, s, wait));
    w.advance(Dur(w.s.params.question_recheck.0 + MINUTE.0));
    let s2 = w.hello(&c);
    let d = ok(w.call(&c, s2, take(a)));
    let t2 = token_of(&d);
    assert!(
        w.s.nodes[&a].recheck_marks.contains(&q),
        "the recheck marks the node whose wait it resolved"
    );
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Release {
            token: t2,
            wait: Some(q)
        }
    );
    assert_eq!(it.because, Because::Recheck { question: q });
    assert_eq!(it.when, Moment::Now);
    // The answer clears the mark: no item after it.
    ok(w.call(&c, s2, heard_answer(a, q)));
    assert!(w.s.nodes[&a].recheck_marks.is_empty());
    assert!(!rules(&w).contains(&Rule::HeldFlag));
}

/// A node `/a` with `chat:x/1`, held by `s`, that asks its person a question (`blocking` as
/// given), releases waiting on it, and lets every recheck pass (`question.max_rechecks` is 1):
/// the question expires (non-blocking) or is escalated (blocking).
fn waited_out(blocking: bool) -> (World, ClassName, NodeId, QuestionId) {
    let mut w = world_with(|p| p.question_max_rechecks = 1);
    let c = class("worker");
    let s = w.hello(&c);
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    let q = question_of(&ok(w.call(&c, s, ask_person(a, None, blocking))));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Release {
            token: t,
            done: text("asked"),
            next_step: text("wait"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Question(q)),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
    w.advance(Dur(w.s.params.question_recheck.0 + MINUTE.0));
    (w, c, a, q)
}

#[test]
fn rule_10_records_the_assumption_of_an_expired_question() {
    let (mut w, c, a, q) = waited_out(false);
    assert!(
        w.s.nodes[&a].assume_marks.contains(&q),
        "the expiry marks the node"
    );
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(a)));
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Decide {
            at: a,
            settles: vec![],
            supersedes: None
        }
    );
    assert_eq!(
        it.because,
        Because::Assumption {
            questions: vec![q],
            blocking: false
        }
    );
    assert!(!it.once);
    // The decision records the assumption and clears the mark: no item after it.
    ok(w.call(&c, s2, decide_cmd(a, &[])));
    assert!(w.s.nodes[&a].assume_marks.is_empty());
    assert!(!rules(&w).contains(&Rule::HeldFlag));
}

#[test]
fn rule_10_records_the_assumption_of_a_lapsed_escalation_as_blocking() {
    let (mut w, c, a, q) = waited_out(true);
    assert!(
        w.s.nodes[&a].assume_marks.is_empty(),
        "escalated, not yet lapsed"
    );
    // The question for the user lapses unanswered (§10.3): its nodes proceed on the assumption.
    w.advance(Dur(60 * 24 * HOUR.0));
    let marks: Vec<QuestionId> = w.s.nodes[&a].assume_marks.iter().copied().collect();
    assert!(!marks.is_empty(), "the lapse marks the node");
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(a)));
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Decide {
            at: a,
            settles: vec![],
            supersedes: None
        }
    );
    assert_eq!(
        it.because,
        Because::Assumption {
            questions: marks,
            blocking: true
        }
    );
    let _ = q;
}

#[test]
fn rule_10_supersedes_a_decision_whose_basis_was_superseded() {
    let (mut w, c, s) = worker();
    let (lemma, _) = held(&mut w, &c, s, "/lemma");
    let (thm, _) = held(&mut w, &c, s, "/thm");
    let cited = match ok(w.call(&c, s, decide_cmd(lemma, &[]))).reply {
        Reply::Decided { decision, .. } => decision,
        other => panic!("{other:?}"),
    };
    let d = match ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(thm),
            text: text("by the lemma's decision"),
            settles: BTreeSet::new(),
            supersedes: None,
            basis: vec![BasisArg::Entry(cited)],
            evidence: None,
        }),
    ))
    .reply
    {
        Reply::Decided { decision, .. } => decision,
        other => panic!("{other:?}"),
    };
    // Nothing stale yet: no item.
    assert!(!rules(&w).contains(&Rule::HeldFlag));
    // The cited decision is superseded: the theorem's decision has a stale basis.
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Decide {
            at: id(lemma),
            text: text("revised"),
            settles: BTreeSet::new(),
            supersedes: Some(cited),
            basis: Vec::new(),
            evidence: None,
        }),
    ));
    assert!(w.s.ix.items[&thm].stale_basis.contains(&d));
    let it = w
        .last_guidance
        .items
        .iter()
        .find(|i| {
            i.rule == Rule::HeldFlag && matches!(i.call, Skeleton::Decide { at, .. } if at == thm)
        })
        .cloned()
        .unwrap();
    assert_eq!(
        it.call,
        Skeleton::Decide {
            at: thm,
            settles: vec![],
            supersedes: Some(d)
        }
    );
    assert_eq!(it.because, Because::StaleBasis { decision: d });
}

/// Two plain nodes `/p` (older) and `/q` (newer) linked as duplicates by `s`; returns the world,
/// the class, the two nodes, and the signal node.
fn duplicates(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    p: &str,
    q: &str,
) -> (NodeId, NodeId, NodeId) {
    let older = free(w, c, s, p);
    let newer = free(w, c, s, q);
    let d = ok(w.call(
        c,
        s,
        Cmd::Node(NodeCmd::Link {
            at: id(newer),
            rel: crate::types::LinkRel::Duplicate,
            to: id(older),
        }),
    ));
    let Reply::Linked {
        signal: Some(g), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    (older, newer, g)
}

#[test]
fn rule_10_merges_the_newer_duplicate_into_the_older_one() {
    let (mut w, c, s) = worker();
    let (older, newer, g) = duplicates(&mut w, &c, s, "/p", "/q");
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(g)));
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Merge {
            from: newer,
            into: older
        }
    );
    assert_eq!(it.because, Because::Duplicate { other: older });
    // A held older node changes nothing: it is the one that survives (§16).
    let s3 = w.hello(&c);
    ok(w.call(&c, s3, take(older)));
    ok(w.call(&c, s2, view_other()));
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Merge {
            from: newer,
            into: older
        }
    );
}

#[test]
fn rule_10_merges_the_older_duplicate_into_the_newer_one_when_the_newer_is_held() {
    // §16's convention: into the older node, unless the newer one is held, so that a held node
    // survives; with both held, the newer survives.
    let (mut w, c, s) = worker();
    let (older, newer, g) = duplicates(&mut w, &c, s, "/p", "/q");
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(g)));
    let s3 = w.hello(&c);
    ok(w.call(&c, s3, take(newer)));
    ok(w.call(&c, s2, view_other()));
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Merge {
            from: older,
            into: newer
        }
    );
    assert_eq!(it.because, Because::Duplicate { other: newer });
    // 1.8 (§20.9 "Another's node"): with both held, the merge would close the older node another
    // session holds, so the signal is decided instead.
    let s4 = w.hello(&c);
    ok(w.call(&c, s4, take(older)));
    ok(w.call(&c, s2, view_other()));
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(
        it.call,
        Skeleton::Decide {
            at: g,
            settles: Vec::new(),
            supersedes: None
        }
    );
    assert_eq!(it.because, Because::Signal);
}

#[test]
fn rule_10_decides_a_duplicate_of_containers_instead_of_merging() {
    for (p, q) in [
        ("/@people/ann", "/@people/anne"),
        ("/@teams/core", "/@teams/kernel"),
    ] {
        let (mut w, c, s) = worker();
        let (_, _, g) = duplicates(&mut w, &c, s, p, q);
        let s2 = w.hello(&c);
        ok(w.call(&c, s2, take(g)));
        let it = item_of(&w, Rule::HeldFlag).unwrap();
        assert_eq!(
            it.call,
            Skeleton::Decide {
                at: g,
                settles: vec![],
                supersedes: None
            },
            "{p} and {q}"
        );
        assert_eq!(it.because, Because::Signal);
    }
}

#[test]
fn rule_10_decides_a_duplicate_whose_merge_would_be_refused() {
    // `/p` depends on `/x`, which depends on `/q`: merging `/q` into `/p` would turn `/x → /q`
    // into `/x → /p` and close a cycle of dependencies (T5), so `validate` refuses the merge that
    // rule 10 would suggest, and the item is a decision; once the newer one is held, the merge
    // goes the other way (§16), which closes no cycle, and is suggested again.
    let (mut w, c, s) = worker();
    let (older, newer, g) = duplicates(&mut w, &c, s, "/p", "/q");
    let x = mk(&mut w, &c, s, "/x", |a| {
        a.take = Some(false);
        a.depends_on = vec![id(newer)];
    })
    .0;
    crate::helpers::user::tests::update(&mut w, &c, s, older, |p| {
        p.depends_on = Some(vec![id(x)]);
    });
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, take(g)));
    let decide = Skeleton::Decide {
        at: g,
        settles: vec![],
        supersedes: None,
    };
    let it = item_of(&w, Rule::HeldFlag).unwrap();
    assert_eq!(it.call, decide);
    assert_eq!(it.because, Because::Signal);
    let s3 = w.hello(&c);
    ok(w.call(&c, s3, take(newer)));
    ok(w.call(&c, s2, view_other()));
    assert_eq!(
        item_of(&w, Rule::HeldFlag).unwrap().call,
        Skeleton::Merge {
            from: older,
            into: newer
        }
    );
}

// ------------------------------------------------------------------------------------------------
// 11 and 12: the inbox and the human's items

#[test]
fn rule_11_waits_for_unread_notes() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let s2 = w.hello(&c);
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Say {
            at: id(a),
            text: text("hi"),
            to: Some(AddresseeArg::Session(s2)),
            expects: None,
            reply_to: None,
            until: None,
        }),
    ));
    ok(w.call(&c, s2, view_other()));
    assert!(has(&w, &Skeleton::Wait { timeout: SECOND }));
    ok(w.call(&c, s2, wait_cmd()));
    assert!(!rules(&w).contains(&Rule::Inbox));
}

/// 1.7 (§13): a write delivers up to two unread notes, oldest first, and marks only those read;
/// rule 11 suggests `talk wait` only while more remain, and a read delivers nothing.
#[test]
fn notes_ride_on_writes_two_at_a_time() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let s2 = w.hello(&c);
    let say = |t: &str| {
        Cmd::Talk(TalkCmd::Say {
            at: id(a),
            text: text(t),
            to: Some(AddresseeArg::Session(s2)),
            expects: None,
            reply_to: None,
            until: None,
        })
    };
    let entry = |d: &Done| match d.reply {
        Reply::Say { entry } => entry,
        ref other => panic!("{other:?}"),
    };
    let notes: Vec<Eid> = ["one", "two", "three"]
        .iter()
        .map(|t| entry(&ok(w.call(&c, s, say(t)))))
        .collect();
    // A read delivers nothing.
    let d = ok(w.call(&c, s2, view_other()));
    assert!(d.delivered.is_empty());
    assert!(matches!(
        item_of(&w, Rule::Inbox).unwrap().because,
        Because::Inbox { count: 3 }
    ));
    // A write delivers the two oldest; the third waits, and rule 11 still names it.
    let d = ok(w.call(&c, s2, free_cmd("/b")));
    assert_eq!(d.delivered, notes[..2].to_vec());
    assert_eq!(
        crate::helpers::agents::inbox(&w.s, s2, w.clock),
        vec![notes[2]]
    );
    assert!(matches!(
        item_of(&w, Rule::Inbox).unwrap().because,
        Because::Inbox { count: 1 }
    ));
    // The next write delivers the last; then nothing is suggested.
    let d = ok(w.call(&c, s2, free_cmd("/c")));
    assert_eq!(d.delivered, vec![notes[2]]);
    assert!(!rules(&w).contains(&Rule::Inbox));
    // A refused write delivers nothing.
    ok(w.call(&c, s, say("four")));
    let out = w.call(&c, s2, free_cmd("/c"));
    assert!(out.is_err());
    assert_eq!(crate::helpers::agents::inbox(&w.s, s2, w.clock).len(), 1);
}

/// A `create` that issues no hold, at `p`.
fn free_cmd(p: &str) -> Cmd {
    create_with(p, |a| a.take = Some(false))
}

/// 1.7 (§20.9 "End"): after an empty `next` with nothing in `leave_better`, and at a `hello` with
/// no offer, the next calls end with `end`; never with an item, and not while something is owed.
#[test]
fn end_after_an_empty_next_and_at_a_hello_with_no_offer() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let d = ok(w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: None,
            from_user: None,
        }),
    ));
    let s = d.opened.unwrap();
    assert!(w.last_guidance.items.is_empty());
    assert_eq!(w.last_guidance.end, Some(End::AtHello));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery::default(),
            lease: None,
        }),
    ));
    assert!(w.last_guidance.items.is_empty());
    assert_eq!(w.last_guidance.end, Some(End::AfterNext));
    // A read with nothing to suggest says none, without `end`.
    ok(w.call(&c, s, view_other()));
    assert!(w.last_guidance.items.is_empty());
    assert_eq!(w.last_guidance.end, None);
    // With an offered node, a hello offers it: no `end`.
    mk(&mut w, &c, s, "/offered", |a| {
        a.take = Some(false);
        a.tags = [crate::apply::tests::label("offer")].into();
    });
    w.hello(&c);
    assert!(rules(&w).contains(&Rule::MoreWork));
    assert_eq!(w.last_guidance.end, None);
}

/// 1.7 (§10.4): a node whose `{external}` wait has resolved at its recheck comes before the other
/// candidates of its tier.
#[test]
fn a_resolved_outside_wait_comes_first_in_its_tier() {
    let (mut w, c, s) = worker();
    let a = free(&mut w, &c, s, "/a");
    let (b, tb) = held(&mut w, &c, s, "/b");
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: id(a),
            version: w.s.nodes[&a].version,
            digest_version: None,
            token: None,
            patch: Patch {
                tags_add: [crate::apply::tests::label("offer")].into(),
                ..Patch::default()
            },
        }),
    ));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Release {
            token: tb,
            done: text("started the job"),
            next_step: text("check the job"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::External {
                what: text("cluster job 9362"),
                recheck_after: When::In(Dur(HOUR.0)),
            }),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
    // Waiting, `/b` is no candidate; once its recheck has come, it is first, ahead of `/a`, which
    // has waited longer in the same tier, whatever the ring says.
    w.advance(Dur(HOUR.0 + MINUTE.0));
    for k in 0..4 {
        let x = w.hello(&c);
        let (first, _) =
            crate::query::ranked(&w.s, x, w.clock, &NextQuery::default(), 2, &mut NoMeter);
        assert_eq!(
            first.iter().map(|r| r.cand).collect::<Vec<_>>(),
            vec![crate::state::Cand::Node(b), crate::state::Cand::Node(a)],
            "session {k}"
        );
    }
    assert!(crate::index::recheck_due(&w.s.nodes[&b]));
}

#[test]
fn rule_12_stands_until_the_first_to_user_to_that_person_under_any_alias() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    person(&mut w, &c, s, "alice", &["mail:alice@x", "chat:alice"]);
    let (a, _) = held(&mut w, &c, s, "/a");
    ok(w.call(&c, s, ask_user(a, Some("mail:alice@x"), 7)));
    // A worker gets no rule 12.
    assert!(!rules(&w).contains(&Rule::HumanItems));
    let asst = class("assistant");
    let h = open_as(&mut w, &asst, Some("chat:alice"));
    let p = pid(&w, "chat:alice");
    let it = item_of(&w, Rule::HumanItems).unwrap();
    assert_eq!(
        it.call,
        Skeleton::ToUser {
            principal: p,
            kind: Some(ToUserKind::Question),
            re: vec![]
        }
    );
    // Standing: a later view still names it.
    ok(w.call(&asst, h, view_other()));
    assert!(rules(&w).contains(&Rule::HumanItems));
    // A to_user to the other alias tells the person: gone for good.
    ok(w.call(
        &asst,
        h,
        to_user_cmd(ToUserKind::Ack, Some("mail:alice@x"), &[]),
    ));
    ok(w.call(&asst, h, view_other()));
    assert!(!rules(&w).contains(&Rule::HumanItems));
}

#[test]
fn rule_12_leaves_the_kind_to_the_agent_with_only_digest_entries_pending() {
    // A question with a default for an absent person defaults after `due_absent`, with a digest
    // entry (§8.4, §8.5): nothing is left to ask, only something to show.
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let (a, _) = held(&mut w, &c, s, "/a");
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Ask {
            at: id(a),
            text: text("which?"),
            to: AskTo::User(UserAsk {
                to: Some(pk("mail:alice@x")),
                options: vec![text("a"), text("b")],
                recommendation: None,
                default: Some(text("a")),
                due: None,
                digest: ReqDigest([3; 32]),
            }),
            again: false,
        }),
    ));
    w.advance(Dur(w.s.params.user_question_due_absent.0 + MINUTE.0));
    let p = pid(&w, "mail:alice@x");
    assert!(w.s.ix.pending_uq_by.get(&p).is_none_or(BTreeSet::is_empty));
    let digest: Vec<Eid> = w.s.ix.pending_digest_by[&p].iter().copied().collect();
    assert!(!digest.is_empty());
    let asst = class("assistant");
    let h = open_as(&mut w, &asst, Some("mail:alice@x"));
    let it = item_of(&w, Rule::HumanItems).unwrap();
    assert_eq!(
        it.call,
        Skeleton::ToUser {
            principal: p,
            kind: None,
            re: digest.clone()
        }
    );
    assert_eq!(
        it.because,
        Because::HumanItems {
            principal: p,
            questions: 0,
            digest: u32::try_from(digest.len()).unwrap()
        }
    );
    // Any kind shows the entries and tells the person: gone after it.
    ok(w.call(
        &asst,
        h,
        to_user_cmd(ToUserKind::Ack, Some("mail:alice@x"), &digest),
    ));
    assert!(!rules(&w).contains(&Rule::HumanItems));
}

// ------------------------------------------------------------------------------------------------
// 13: more work for a worker

#[test]
fn rule_13_offers_next_to_a_worker_and_never_to_a_session_with_a_principal() {
    let (mut w, c, s) = worker();
    ok(w.call(
        &c,
        s,
        create_with("/a", |x| {
            x.take = Some(false);
            x.tags = [crate::apply::tests::label("offer")].into();
        }),
    ));
    let _ = w.hello(&c);
    let it = item_of(&w, Rule::MoreWork).unwrap();
    assert_eq!(it.call, Skeleton::Next);
    assert_eq!(it.because, Because::MoreWork { at_hello: true });
    let asst = class("assistant");
    let _ = open_as(&mut w, &asst, Some("mail:alice@x"));
    assert!(!rules(&w).contains(&Rule::MoreWork));
}

#[test]
fn rule_13_takes_leave_better_s_node_below_the_hold_limit() {
    let (mut w, c, s) = worker();
    // 1.12: another session hands `p` off; a node it handed off itself is kept from it.
    let s2 = w.hello(&c);
    let (p, pt) = held(&mut w, &c, s2, "/p");
    let (k, kt) = held(&mut w, &c, s, "/p/k");
    ok(w.call(&c, s2, release(pt, ReleaseReason::Handoff)));
    ok(w.call(&c, s, close_with(k, kt, Outcome::Done, &[])));
    // `p` is ready_to_close, unheld: leave_better names it.
    assert!(has(&w, &Skeleton::Take { target: p }));
    assert_eq!(
        item_of(&w, Rule::Tidy).unwrap().call,
        Skeleton::Take { target: p }
    );
}

#[test]
fn rule_13_takes_the_joined_node_only_below_the_hold_limit() {
    // `hold.per_session` is 1. A worker that holds nothing and joins `/m` is told to take it;
    // one that holds its one slot is not. (Every other trigger leaves a slot free: a close or a
    // release frees one, and `next` is refused at the limit, so `can_take`'s own check of the
    // limit decides there too.)
    for holding in [false, true] {
        let mut w = world_with(|p| p.hold_per_session = 1);
        let c = class("worker");
        let s = w.hello(&c);
        let (m, _) = mk(&mut w, &c, s, "/m", |x| {
            x.keys = vec![key("chat:m/1")];
            x.take = Some(false);
        });
        let x = w.hello(&c);
        if holding {
            held(&mut w, &c, x, "/a");
        }
        let d = ok(w.call(&c, x, create_with("/n", |a| a.keys = vec![key("chat:m/1")])));
        assert!(matches!(d.reply, Reply::Joined { existing, .. } if existing == m));
        assert_eq!(
            has(&w, &Skeleton::Take { target: m }),
            !holding,
            "holding: {holding}"
        );
    }
}

#[test]
fn rule_13_after_a_full_paused_or_closed_refusal_offers_next() {
    let refusals: [fn(&mut World, &ClassName, SessionId) -> NodeId; 3] = [
        // Full: another session holds the one slot.
        |w, c, s| {
            let (f, _) = held(w, c, s, "/f");
            f
        },
        // Paused.
        |w, c, s| {
            let p = free(w, c, s, "/p");
            ok(w.call(
                c,
                s,
                Cmd::Control(crate::cmd::ControlCmd::Pause {
                    scope: id(p),
                    reason: text("hold on"),
                }),
            ));
            p
        },
        // Closed, unflagged, never merged.
        |w, c, s| {
            let (k, t) = held(w, c, s, "/k");
            ok(w.call(c, s, close_with(k, t, Outcome::Done, &[])));
            k
        },
    ];
    for (i, refuse) in refusals.iter().enumerate() {
        let (mut w, c, s) = worker();
        ok(w.call(
            &c,
            s,
            create_with("/offered", |a| {
                a.take = Some(false);
                a.tags = [crate::apply::tests::label("offer")].into();
            }),
        ));
        let n = refuse(&mut w, &c, s);
        let x = w.hello(&c);
        let out = w.call(&c, x, take(n));
        assert!(out.is_err(), "case {i}: {out:?}");
        let it = item_of(&w, Rule::MoreWork)
            .unwrap_or_else(|| panic!("case {i}: {:?}", w.last_guidance));
        assert_eq!(it.call, Skeleton::Next);
        // 1.10: the why names what `next` would give, the offered node.
        assert!(
            matches!(it.because, Because::WorkWaiting { first: crate::state::Cand::Node(o) }
                if w.s.nodes[&o].tags.iter().any(|t| t.as_str() == "offer")),
            "case {i}: {:?}",
            it.because
        );
    }
    // The negatives: a refusal of another code (an unknown node), and `closed` with
    // `merged_into`, whose remedy is the take of where the node lives on.
    let (mut w, c, s) = worker();
    ok(w.call(
        &c,
        s,
        create_with("/offered", |a| {
            a.take = Some(false);
            a.tags = [crate::apply::tests::label("offer")].into();
        }),
    ));
    let x = w.hello(&c);
    assert!(w.call(&c, x, take(NodeId(999))).is_err());
    assert!(!rules(&w).contains(&Rule::MoreWork));
    let from = free(&mut w, &c, s, "/from");
    let into = free(&mut w, &c, s, "/into");
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Merge {
            from: id(from),
            into: id(into),
        }),
    ));
    assert!(w.call(&c, x, take(from)).is_err());
    assert!(!rules(&w).contains(&Rule::MoreWork));
    assert_eq!(
        item_of(&w, Rule::Remedy).unwrap().call,
        Skeleton::Take { target: into }
    );
}

// ------------------------------------------------------------------------------------------------
// finish: order, dedupe, cut, total

#[test]
fn finish_partitions_dedupes_cuts_and_keeps_a_one_shot_item() {
    // Three own unrouted entries (standing, now), and rule 9's release of a blocking question
    // (one-shot, now); 1.9: no confirm of the ask's own said. The release takes the third place,
    // and the displaced entry returns in the next response.
    let mut w = world_with(|_| {});
    let c = class("assistant");
    let s = w.hello(&c);
    let (a, t) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/1")]);
    let t = t.unwrap();
    held(&mut w, &c, s, "/b");
    let mut entries = Vec::new();
    for m in ["one", "two", "three"] {
        let d = ok(w.call(&c, s, from_user_cmd(m, Some("user"), None, None, None)));
        let Reply::FromUser { entry, .. } = d.reply else {
            panic!()
        };
        entries.push(entry);
    }
    let (out, all) = go(&mut w, &c, s, ask_person(a, None, true));
    let d = ok(out);
    let q = question_of(&d);
    let release_item = Skeleton::Release {
        token: t,
        wait: Some(q),
    };
    assert_eq!(
        all.iter().map(|i| i.call.clone()).collect::<Vec<_>>(),
        vec![
            Skeleton::CreateFor { entry: entries[0] },
            Skeleton::CreateFor { entry: entries[1] },
            Skeleton::CreateFor { entry: entries[2] },
            release_item.clone(),
        ]
    );
    assert_eq!(
        calls(&w).into_iter().map(|(_, _, k)| k).collect::<Vec<_>>(),
        vec![
            Skeleton::CreateFor { entry: entries[0] },
            Skeleton::CreateFor { entry: entries[1] },
            release_item,
        ]
    );
    assert_eq!(w.last_guidance.total, Total::Exact(4));
    ok(w.call(&c, s, view_other()));
    assert!(has(&w, &Skeleton::CreateFor { entry: entries[2] }));
}

#[test]
fn finish_drops_the_call_just_made_and_counts_at_least_twenty() {
    // A view of the node rule 4 would suggest drops it.
    let (mut w, c, s1) = worker();
    let (a, _) = mk(&mut w, &c, s1, "/a", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
    });
    ok(w.call(&c, s1, take(a)));
    let s2 = w.hello(&c);
    w.advance(SECOND);
    ok(w.call(&c, s2, request_cmd(a, "chat:x/1/1", Some("chat:x/1"))));
    let (_, all) = go(&mut w, &c, s1, view(a));
    assert!(!all.iter().any(|i| i.call == Skeleton::ViewNode { node: a }));
    // Twenty-one own unrouted entries: total `≥20` (1.9: no confirm items any more).
    let mut w = world_with(|_| {});
    let c = class("assistant");
    let s = w.hello(&c);
    for i in 0..21 {
        let m = alloc::format!("message {i}");
        ok(w.call(&c, s, from_user_cmd(&m, Some("user"), None, None, None)));
    }
    assert_eq!(w.last_guidance.total, Total::AtLeast(NEXT_TOTAL_CAP));
    assert_eq!(w.last_guidance.items.len(), NEXT_CALLS_MAX);
}

#[test]
fn same_call_and_canon() {
    let t = Token {
        node: NodeId(9),
        fence: crate::types::Fence(1),
    };
    assert!(same_skeleton(
        &Skeleton::CheckpointSwap { token: t },
        &Skeleton::Checkpoint {
            token: t,
            intent_done: true,
            intent: false
        }
    ));
    assert!(!same_skeleton(
        &Skeleton::CheckpointSwap { token: t },
        &Skeleton::Checkpoint {
            token: t,
            intent_done: false,
            intent: false
        }
    ));
    let (mut w, c, s0) = worker();
    let n9 = free(&mut w, &c, s0, "/n9");
    let t = Token {
        node: n9,
        fence: crate::types::Fence(1),
    };
    let s = w.s.clone();
    let _ = (Params::default(), T0);
    assert!(same_call(
        &s,
        &Skeleton::Checkpoint {
            token: t,
            intent_done: false,
            intent: false
        },
        &checkpoint_with(t, None, None)
    ));
    assert!(!same_call(
        &s,
        &Skeleton::Checkpoint {
            token: t,
            intent_done: false,
            intent: false
        },
        &checkpoint_with(t, Some("x"), None)
    ));
    // A close whose outcome is left to the agent matches any outcome.
    let mut dropped = close_default(n9, t);
    if let Cmd::Node(NodeCmd::Close { outcome, .. }) = &mut dropped {
        *outcome = Outcome::Dropped;
    }
    assert!(same_call(
        &s,
        &Skeleton::Close {
            node: n9,
            token: t,
            settles: vec![],
            outcome: None
        },
        &dropped
    ));
    // 1.7: an item without `settles` is the default of §5, not `settles: []`, which settles none.
    assert!(!same_call(
        &s,
        &Skeleton::Close {
            node: n9,
            token: t,
            settles: vec![],
            outcome: None
        },
        &close_with(n9, t, Outcome::Dropped, &[])
    ));
    assert!(!same_call(&s, &Skeleton::Hello, &view_other()));
}

// ------------------------------------------------------------------------------------------------
// §32 with next calls (D.16, D.12.2), at core level

/// A first call of class `c`: it opens its session.
fn first(w: &mut World, c: &ClassName, cmd: Cmd) -> (SessionId, Done) {
    let ctx = w.ctx(c, SessionCtx::New, None);
    let d = ok(w.call_ctx(&ctx, &cmd));
    (d.opened.unwrap(), d)
}

fn skeletons(w: &World) -> Vec<(Moment, Skeleton)> {
    w.last_guidance
        .items
        .iter()
        .map(|i| (i.when, i.call.clone()))
        .collect()
}

fn heard_answer(n: NodeId, q: Eid) -> Cmd {
    Cmd::Talk(TalkCmd::Heard {
        at: id(n),
        where_: None,
        from: text("alex"),
        text: text("yes, both after the cutoff"),
        answers: Some(q),
    })
}

/// `hello {principal: user, from_user}`: §32's clients serve the user without naming them (1.9,
/// §8.1).
fn hello_from_user(t: &str, client: &str) -> Cmd {
    Cmd::Session(SessionCmd::Hello {
        intent: None,
        caps: None,
        horizon: None,
        principal: crate::types::PrincipalKey::new("user"),
        from_user: Some(crate::cmd::FromUserArg {
            text: text(t),
            client: text(client),
            principal: None,
            answers: None,
            to: None,
            clarifies: None,
            reason: None,
        }),
    })
}

/// 2026-10-01T09:00Z, §32's start (D.12.2).
const START_32: Time = Time(1_790_845_200_000);

/// Steps 2 to 7 of §32 and their next calls, against D.16. Step 1 needs a series (`node create
/// {repeat}`, WP-D6) and is `section_32_step_1_next_calls`; step 8 is below. 1.7's rule 6 adds a
/// `later` reply to `s-1`'s create in step 2 (1.5's rule 7 had a `later` decision there, R28),
/// which D.16 does not list, and step 7 replies, then closes, with no decision of its own.
#[test]
fn section_32_steps_2_to_7_next_calls() {
    let _ = section_32_plain();
}

/// The plain run of steps 2 to 7, with D.16's assertions; returns the world at its end.
fn section_32_plain() -> World {
    let mut w = World::new(Params::default(), START_32);
    let worker = class("chat-worker");
    let thread = "chat:team/thread-1";
    let create_gap = || {
        create_with("/summary-gap", |a| {
            a.keys = vec![key(thread)];
            a.request = Some(crate::cmd::RequestArg {
                text: text("why did yesterday's summary miss two tickets?"),
                origin: Some(key("chat:team/thread-1/1001")),
                where_: None,
                requester: text("alex"),
            });
        })
    };
    // Step 2. s-1 creates summary-gap with r2; s-2 joins it.
    let (s1, d) = first(&mut w, &worker, create_gap());
    let Reply::Created {
        id: gap,
        token: Some(t11),
        request: Some(r2),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply)
    };
    assert_eq!(
        skeletons(&w),
        vec![
            (
                Moment::Later,
                Skeleton::Checkpoint {
                    token: t11,
                    intent_done: false,
                    intent: false
                }
            ),
            // 1.7: the reply, later, on the node `s-1` holds, where 1.6 had the decision.
            (
                Moment::Later,
                Skeleton::Said {
                    at: gap,
                    kind: SaidKind::Result,
                    where_: None,
                    where_fill: false
                }
            ),
        ]
    );
    let (_s2, d) = first(&mut w, &worker, create_gap());
    assert!(matches!(d.reply, Reply::Joined { existing, .. } if existing == gap));
    assert_eq!(skeletons(&w), vec![]);
    // Step 3. s-1 acknowledges, confirms, splits off two offered children, and releases. 1.9:
    // no next call asks for the confirm (§9.2); it confirms with its link all the same.
    let e5 = said_of(&ok(w.call(&worker, s1, said_cmd(gap, None, SaidKind::Ack))));
    assert_eq!(skeletons(&w), vec![]);
    ok(w.call(&worker, s1, confirm(e5)));
    assert_eq!(skeletons(&w), vec![]);
    let offer = || [crate::apply::tests::label("offer")].into();
    let (audit, _) = mk(&mut w, &worker, s1, "/summary-gap/audit", |a| {
        a.take = Some(false);
        a.tags = offer();
    });
    assert_eq!(skeletons(&w), vec![]);
    let (rerun, _) = mk(&mut w, &worker, s1, "/summary-gap/rerun-test", |a| {
        a.take = Some(false);
        a.tags = offer();
    });
    assert_eq!(skeletons(&w), vec![]);
    ok(w.call(&worker, s1, release(t11, ReleaseReason::Handoff)));
    assert_eq!(skeletons(&w), vec![(Moment::Now, Skeleton::Next)]);
    // Step 4. s-3 takes audit: read the conversation; later, the checkpoint.
    let (s3, d) = first(&mut w, &worker, take(audit));
    let t12 = token_of(&d);
    assert_eq!(
        skeletons(&w),
        vec![
            (Moment::Now, Skeleton::ViewNode { node: gap }),
            (
                Moment::Later,
                Skeleton::Checkpoint {
                    token: t12,
                    intent_done: false,
                    intent: false
                }
            ),
        ]
    );
    assert!(matches!(
        item_of(&w, Rule::Reread).unwrap().because,
        Because::News {
            count: Total::Exact(2),
            ..
        }
    ));
    let d = ok(w.call(&worker, s3, ask_person(audit, None, false)));
    let q1 = question_of(&d);
    assert_eq!(
        skeletons(&w),
        vec![
            (Moment::Now, Skeleton::ViewNode { node: gap }),
            (
                Moment::Later,
                Skeleton::Release {
                    token: t12,
                    wait: Some(q1)
                }
            ),
        ]
    );
    let d = ok(w.call(
        &worker,
        s3,
        Cmd::Hold(HoldCmd::Release {
            token: t12,
            done: text("asked"),
            next_step: text("wait"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Question(q1)),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
    assert!(matches!(&d.reply, Reply::Released { leave_better, .. } if *leave_better == vec![gap]));
    // 1.8 (rule 13): after a release, `hold next` too, while the worker holds nothing.
    assert_eq!(
        skeletons(&w),
        vec![
            (Moment::Now, Skeleton::Take { target: gap }),
            (Moment::Now, Skeleton::Next)
        ]
    );
    w.advance(Dur(2 * HOUR.0));
    // Step 5. s-4 joins: record the heard; take the node.
    let (s4, d) = first(
        &mut w,
        &worker,
        create_with("/summary-gap", |a| a.keys = vec![key(thread)]),
    );
    assert!(matches!(d.reply, Reply::Joined { open_question: Some(q), .. } if q == q1));
    assert_eq!(
        skeletons(&w),
        vec![
            (
                Moment::Now,
                Skeleton::Heard {
                    at: gap,
                    answers: q1,
                    where_: KeyStr::new(thread).unwrap()
                }
            ),
            (Moment::Now, Skeleton::Take { target: gap }),
        ]
    );
    ok(w.call(&worker, s4, heard_answer(gap, q1)));
    // Step 6. s-5 takes rerun-test and announces an intent, then vanishes.
    let (s5, d) = first(&mut w, &worker, take(rerun));
    let t13 = token_of(&d);
    let d = ok(w.call(
        &worker,
        s5,
        checkpoint_with(
            t13,
            Some("re-run yesterday's digest in a test environment"),
            None,
        ),
    ));
    let Reply::Checkpointed { entry: e10, .. } = d.reply else {
        panic!()
    };
    assert_eq!(
        skeletons(&w),
        vec![
            (Moment::Now, Skeleton::ViewNode { node: gap }),
            (
                Moment::Later,
                Skeleton::Checkpoint {
                    token: t13,
                    intent_done: true,
                    intent: false
                }
            ),
        ]
    );
    w.advance(Dur(21 * MINUTE.0));
    let (s6, d) = first(&mut w, &worker, take(rerun));
    let t132 = token_of(&d);
    assert_eq!(
        skeletons(&w),
        vec![
            (
                Moment::Now,
                Skeleton::Checkpoint {
                    token: t132,
                    intent_done: true,
                    intent: false
                }
            ),
            (Moment::Now, Skeleton::ViewNode { node: gap }),
        ]
    );
    assert!(matches!(
        item_of(&w, Rule::IntentDone).unwrap().because,
        Because::IntentOpen { by, entry, own: false } if by == s5 && entry == e10
    ));
    ok(w.call(
        &worker,
        s6,
        checkpoint_with(t132, None, Some("it never ran")),
    ));
    assert_eq!(
        skeletons(&w),
        vec![
            (Moment::Now, Skeleton::ViewNode { node: gap }),
            (
                Moment::Later,
                Skeleton::Close {
                    node: rerun,
                    token: t132,
                    settles: vec![],
                    outcome: None
                }
            ),
        ]
    );
    let d = ok(w.call(&worker, s6, close_with(rerun, t132, Outcome::Done, &[])));
    assert!(matches!(&d.reply, Reply::Closed { leave_better, .. } if *leave_better == vec![gap]));
    assert_eq!(
        skeletons(&w),
        vec![(Moment::Now, Skeleton::Take { target: gap })]
    );
    // Step 7. s-6 takes audit, decides, and closes it; s-7 finishes summary-gap.
    let t122 = token_of(&ok(w.call(&worker, s6, take(audit))));
    ok(w.call(&worker, s6, decide_cmd(audit, &[])));
    assert_eq!(
        skeletons(&w),
        vec![
            (Moment::Now, Skeleton::ViewNode { node: gap }),
            (
                Moment::Now,
                Skeleton::Close {
                    node: audit,
                    token: t122,
                    settles: vec![],
                    outcome: None
                }
            ),
        ]
    );
    let d = ok(w.call(&worker, s6, close_with(audit, t122, Outcome::Done, &[])));
    assert!(matches!(&d.reply, Reply::Closed { leave_better, .. } if *leave_better == vec![gap]));
    assert_eq!(
        skeletons(&w),
        vec![(Moment::Now, Skeleton::Take { target: gap })]
    );
    let (s7, d) = first(&mut w, &worker, take(gap));
    let t112 = token_of(&d);
    // 1.7: the reply first, now, on the held node that carries `ready_to_close`; no decision.
    assert_eq!(
        skeletons(&w),
        vec![
            (
                Moment::Now,
                Skeleton::Said {
                    at: gap,
                    kind: SaidKind::Result,
                    where_: None,
                    where_fill: false
                }
            ),
            (
                Moment::Later,
                Skeleton::Checkpoint {
                    token: t112,
                    intent_done: false,
                    intent: false
                }
            ),
        ]
    );
    assert!(matches!(
        item_of(&w, Rule::Reply).unwrap().because,
        Because::ReplyOwed { ready: true, .. }
    ));
    let e20 = said_of(&ok(w.call(
        &worker,
        s7,
        said_cmd(gap, None, SaidKind::Result),
    )));
    // Then the close, which settles `r2` by default and marks the reply sent `with_reply`; 1.9:
    // no confirm is suggested.
    let close = Skeleton::Close {
        node: gap,
        token: t112,
        settles: vec![],
        outcome: None,
    };
    assert_eq!(skeletons(&w), vec![(Moment::Now, close)]);
    let d = ok(w.call(&worker, s7, close_default(gap, t112)));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert!(matches!(&d.reply, Reply::Closed { settled, .. } if *settled == vec![r2]));
    assert!(w.s.requests[&r2].settled.is_some());
    assert_eq!(w.s.saids[&e20].state, crate::state::SaidState::Confirmed);
    assert!(w.s.saids[&e20].with_reply);
    assert!(!rules(&w).contains(&Rule::Close));
    w
}

/// Makes the call of the last response's item with `rule`, with G1's standard fill.
fn follow(w: &mut World, c: &ClassName, s: SessionId, rule: Rule) -> Done {
    let it = item_of(w, rule).unwrap_or_else(|| panic!("no {rule:?} item: {:?}", w.last_guidance));
    let (sc, cmd) = standard_fill(&w.s, Some(s), &it.call).unwrap();
    let ctx = w.ctx(c, sc, None);
    ok(w.call_ctx(&ctx, &cmd))
}

/// The entity states the guided run must share with the plain one (D.12.2): every request's
/// settlement, every `said`'s state, every node's life, and every node's flags.
type States = (
    Vec<(Eid, bool, bool)>,
    Vec<(Eid, crate::state::SaidState)>,
    Vec<(NodeId, Option<Outcome>, FlagSet)>,
);

fn states(w: &World) -> States {
    (
        w.s.requests
            .iter()
            .map(|(r, q)| (*r, q.settled.is_some(), q.answered_by.is_some()))
            .collect(),
        w.s.saids.iter().map(|(y, x)| (*y, x.state)).collect(),
        w.s.nodes
            .iter()
            .map(|(n, x)| {
                let life = match &x.life {
                    crate::state::Life::Open => None,
                    crate::state::Life::Closed(c) => Some(c.outcome.clone()),
                };
                (
                    *n,
                    life,
                    w.s.ix.node.get(n).map_or(FlagSet::EMPTY, |i| i.flags),
                )
            })
            .collect(),
    )
}

/// The guided §32 (D.12.2) over steps 2 to 7: every call D.16 marks `→` is made from the skeleton
/// of its next call with the standard fill, `s-3` reads summary-gap before its ask (`→
/// (guided)`), and the run ends with the plain run's entity states: the same requests settled,
/// `said` entries confirmed, and nodes closed with their outcomes, and no flag the plain run lacks.
#[test]
fn section_32_guided_ends_as_the_plain_run() {
    let plain = section_32_plain();
    let mut w = World::new(Params::default(), START_32);
    let worker = class("chat-worker");
    let thread = "chat:team/thread-1";
    let create_gap = || {
        create_with("/summary-gap", |a| {
            a.keys = vec![key(thread)];
            a.request = Some(crate::cmd::RequestArg {
                text: text("why did yesterday's summary miss two tickets?"),
                origin: Some(key("chat:team/thread-1/1001")),
                where_: None,
                requester: text("alex"),
            });
        })
    };
    // Step 2.
    let (s1, d) = first(&mut w, &worker, create_gap());
    let Reply::Created {
        id: gap,
        token: Some(t11),
        ..
    } = d.reply
    else {
        panic!()
    };
    first(&mut w, &worker, create_gap());
    // Step 3. 1.9: no confirm is suggested; the release below marks the ack sent `with_reply`.
    ok(w.call(&worker, s1, said_cmd(gap, None, SaidKind::Ack)));
    let offer = || [crate::apply::tests::label("offer")].into();
    let (audit, _) = mk(&mut w, &worker, s1, "/summary-gap/audit", |a| {
        a.take = Some(false);
        a.tags = offer();
    });
    let (rerun, _) = mk(&mut w, &worker, s1, "/summary-gap/rerun-test", |a| {
        a.take = Some(false);
        a.tags = offer();
    });
    ok(w.call(&worker, s1, release(t11, ReleaseReason::Handoff)));
    // Step 4: the guided s-3 reads the conversation first, so its ask meets no `moved`.
    let (s3, _) = first(&mut w, &worker, take(audit));
    follow(&mut w, &worker, s3, Rule::Reread);
    let d = ok(w.call(&worker, s3, ask_person(audit, None, false)));
    assert!(d.warnings.iter().all(|x| x.code != WarningCode::Moved));
    follow(&mut w, &worker, s3, Rule::AskWait);
    w.advance(Dur(2 * HOUR.0));
    // Step 5.
    let (s4, _) = first(
        &mut w,
        &worker,
        create_with("/summary-gap", |a| a.keys = vec![key(thread)]),
    );
    follow(&mut w, &worker, s4, Rule::JoinAnswer);
    // Step 6.
    let (s5, d) = first(&mut w, &worker, take(rerun));
    let t13 = token_of(&d);
    ok(w.call(&worker, s5, checkpoint_with(t13, Some("re-run it"), None)));
    w.advance(Dur(21 * MINUTE.0));
    let (s6, _) = first(&mut w, &worker, take(rerun));
    follow(&mut w, &worker, s6, Rule::IntentDone);
    follow(&mut w, &worker, s6, Rule::Close);
    // Step 7.
    ok(w.call(&worker, s6, take(audit)));
    ok(w.call(&worker, s6, decide_cmd(audit, &[])));
    follow(&mut w, &worker, s6, Rule::Close);
    // 1.7: the reply, then the close, which settles the request and confirms the reply.
    let (s7, _) = first(&mut w, &worker, take(gap));
    follow(&mut w, &worker, s7, Rule::Reply);
    let d = follow(&mut w, &worker, s7, Rule::Close);
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(states(&w), states(&plain));
}

/// §32 step 8 (D.16): the coding agent logs the user's words first, and reports; 1.9 (§8.2): the
/// report, of kind `result`, leaves the entry unrouted, and rule 5 suggests the route to none.
#[test]
fn section_32_step_8_next_calls() {
    let mut w = World::new(Params::default(), START_32);
    let c = class("coding-agent");
    let (s, d) = first(&mut w, &c, hello_from_user("what about my tasks?", "ide"));
    let Reply::Hello {
        user_entry: Some(u),
        ..
    } = d.reply
    else {
        panic!()
    };
    let e = u.entry;
    assert_eq!(
        skeletons(&w),
        vec![(Moment::Now, Skeleton::CreateFor { entry: e })]
    );
    ok(w.call(&c, s, to_user_cmd(ToUserKind::Result, None, &[e])));
    assert_eq!(w.s.user_entries[&e].routing, Routing::Unrouted);
    assert_eq!(
        skeletons(&w),
        vec![(
            Moment::Now,
            Skeleton::Route {
                entry: e,
                to: RouteSk::Nowhere
            }
        )]
    );
    follow(&mut w, &c, s, Rule::Route);
    assert_eq!(w.s.user_entries[&e].routing, Routing::NoAction);
    assert_eq!(skeletons(&w), vec![]);
}

/// §32 step 1 (D.16 and its 1.4 row, R18): the series' create returns `uncovered_slot`, so rule 3
/// suggests the checkpoint with an intent now; for `r1`, which the create recorded on the series it
/// holds, 1.7's rule 6 suggests the report, and rule 7 the decision, both `later`: a series stays
/// open, so no close records it (§5). Rule 8's release of the series, one-shot and `later`, is cut
/// at three items, and comes back, now, once `r1` settles. D.16 lists neither the report nor the
/// decision here.
#[test]
fn section_32_step_1_next_calls() {
    let mut w = World::new(Params::default(), START_32);
    let c = class("chat-assistant");
    let (s, d) = first(
        &mut w,
        &c,
        hello_from_user("summarise new support tickets every morning", "chat"),
    );
    let Reply::Hello {
        user_entry: Some(u),
        ..
    } = d.reply
    else {
        panic!()
    };
    let e1 = u.entry;
    assert_eq!(
        skeletons(&w),
        vec![(Moment::Now, Skeleton::CreateFor { entry: e1 })]
    );
    let d = ok(w.call(
        &c,
        s,
        create_with("/digest/daily-tickets", |a| {
            a.keys = vec![Key {
                key: KeyStr::of_user_entry(e1),
                nv: crate::types::NormVer(0),
            }];
            a.repeat = Some(crate::cmd::RepeatArg {
                cron_text: text("0 7 * * *"),
                cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
                tz: forum_cron::TzTable {
                    name: alloc::string::String::from("UTC"),
                    tzdb_version: alloc::string::String::new(),
                    initial: 0,
                    transitions: Vec::new(),
                },
                run_within: Dur(2 * HOUR.0),
            });
        }),
    ));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::UncoveredSlot)
    );
    let Reply::Created {
        id: series,
        token: Some(t),
        user_requests,
        ..
    } = d.reply
    else {
        panic!()
    };
    let r1 = user_requests[0];
    assert_eq!(
        skeletons(&w),
        vec![
            (
                Moment::Now,
                Skeleton::Checkpoint {
                    token: t,
                    intent_done: false,
                    intent: true
                }
            ),
            (
                Moment::Later,
                Skeleton::ToUser {
                    principal: crate::types::USER_PRINCIPAL,
                    kind: Some(ToUserKind::Result),
                    re: vec![r1]
                }
            ),
            (
                Moment::Later,
                Skeleton::Decide {
                    at: series,
                    settles: vec![r1],
                    supersedes: None
                }
            ),
        ]
    );
    // The report, then the decision, now, then the series' release.
    ok(w.call(&c, s, to_user_cmd(ToUserKind::Result, None, &[r1])));
    let it = item_of(&w, Rule::Decide).unwrap();
    assert_eq!(it.when, Moment::Now);
    ok(w.call(&c, s, decide_cmd(series, &[r1])));
    assert!(w.s.requests[&r1].settled.is_some());
    assert!(has(
        &w,
        &Skeleton::Release {
            token: t,
            wait: None
        }
    ));
}

// ------------------------------------------------------------------------------------------------
// G1's check rejects what it must (D.9.2)

/// `check_guide` accepts `guide`'s next calls and rejects each clause's violation: an item that
/// names an object that does not exist (b), a `now` item after a `later` one (a), a `total` that
/// does not count the items (a), and next calls other than `guide`'s (a). Every violation is G1's.
#[test]
fn check_guide_rejects_each_clause_s_violation() {
    let (mut w, c, s) = worker();
    // A create that issues a hold: rule 3's checkpoint by its due time, at least.
    let cmd = create_with("/a", |_| {});
    let out = w.call(&c, s, cmd.clone());
    let g = guide(&w.s, Some(s), w.clock, &cmd, &out, &mut NoMeter);
    assert_eq!(
        crate::check_guide(&w.s, Some(s), w.clock, &cmd, &out, &g),
        Ok(())
    );
    assert!(!g.items.is_empty(), "{g:?}");
    let rejects = |g: &Guidance, what: &str| {
        let v = crate::check_guide(&w.s, Some(s), w.clock, &cmd, &out, g).unwrap_err();
        assert!(v.iter().all(|x| x.inv == crate::Inv::G1), "{v:?}");
        assert!(
            v.iter().any(|x| matches!(
                x.detail,
                crate::ViolationDetail::Text(t) if t.contains(what)
            )),
            "{what}: {v:?}"
        );
    };
    // (b): a node that does not exist.
    let mut bad = g.clone();
    bad.items.push(NextCall {
        rule: Rule::Reread,
        when: Moment::Now,
        once: true,
        call: Skeleton::ViewNode {
            node: NodeId(9_999),
        },
        because: Because::Reread {
            version: crate::types::Version(1),
            digest_version: crate::types::Version(1),
        },
    });
    rejects(&bad, "names an object that does not exist");
    rejects(&bad, "not guide's");
    // (a): a `now` item after a `later` one.
    let mut bad = g.clone();
    let mut first = bad.items[0].clone();
    first.when = Moment::Later;
    let mut second = bad.items[0].clone();
    second.when = Moment::Now;
    second.call = Skeleton::Next;
    bad.items = vec![first, second];
    rejects(&bad, "a now item after a later one");
    // (a): a total that does not count the items.
    let mut bad = g.clone();
    bad.total = Total::Exact(u32::try_from(bad.items.len()).unwrap() + 7);
    rejects(&bad, "total does not count every item");
}

// ------------------------------------------------------------------------------------------------
// 1.8: flows and guidance (the lab's third round)

/// 1.8 (§5, l6 F2): a close of a node that another session holds, by a session that does not,
/// settles nothing by default; `held_by_other` names what it left. With nobody holding, the default
/// applies as in 1.7.
#[test]
fn a_close_of_another_s_node_settles_nothing_by_default() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let asst = class("assistant");
    let h = w.hello(&asst);
    let (_, r) = user_request(&mut w, &asst, h, a, "mail:bob@x");
    ok(w.call(
        &asst,
        h,
        to_user_cmd(ToUserKind::Result, Some("mail:bob@x"), &[r]),
    ));
    // The assistant's result named a request on s's node: no decide is suggested to it, since the
    // holder owes that decision (§20.9 "Another's node").
    assert!(!rules(&w).contains(&Rule::Decide));
    let s2 = w.hello(&c);
    let mut cl = close_default(
        a,
        Token {
            node: a,
            fence: Fence(0),
        },
    );
    if let Cmd::Node(NodeCmd::Close { token, .. }) = &mut cl {
        *token = None;
    }
    let d = ok(w.call(&c, s2, cl));
    assert!(
        matches!(&d.reply, Reply::Closed { settled, decision: None, .. } if settled.is_empty())
    );
    assert!(w.s.requests[&r].settled.is_none());
    assert!(d.warnings.iter().any(|x| x.detail
        == crate::reply::WarningDetail::HeldByOther {
            holders: vec![s],
            left: vec![r],
        }));
    // Reopened and unheld, the same close settles it by default.
    ok(w.call(
        &c,
        s2,
        Cmd::Node(NodeCmd::Reopen {
            at: id(a),
            reason: text("by mistake"),
        }),
    ));
    let mut cl = close_default(
        a,
        Token {
            node: a,
            fence: Fence(0),
        },
    );
    if let Cmd::Node(NodeCmd::Close { token, .. }) = &mut cl {
        *token = None;
    }
    let d = ok(w.call(&c, s2, cl));
    assert!(matches!(&d.reply, Reply::Closed { settled, .. } if *settled == vec![r]));
}

/// 1.8 (§20.9 rule 13, l2 F6, l5 F4): after a release, a resolve, or a reopen, a worker that holds
/// nothing gets `hold next` when `next` would offer it an item, and `end` when it would not.
#[test]
fn rule_13_after_a_release_a_resolve_or_a_reopen() {
    let (mut w, c, s) = worker();
    let (a, t) = held(&mut w, &c, s, "/a");
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Object {
            on: id(a),
            note: text("wrong"),
        }),
    ));
    let Reply::Objected { objection } = d.reply else {
        panic!()
    };
    // The released node carries the objection and the `offer` tag, so `next` offers it: `hold
    // next`, which 1.7 gave only without a tidy item. 1.12 (§10.4): not to the session that
    // handed it off, until `lease.first` has passed.
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    assert!(!has(&w, &Skeleton::Next));
    w.advance(Dur(w.s.params.lease_first.0 + SECOND.0));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Resolve {
            objection,
            note: text("fixed"),
            upheld: false,
            defer: false,
        }),
    ));
    // `/a` is offered still (the `offer` tag a release puts on it): `hold next`.
    assert_eq!(
        item_of(&w, Rule::MoreWork).map(|i| i.call),
        Some(Skeleton::Next)
    );
    assert_eq!(w.last_guidance.end, None);
    // Closed, it is offered to nobody: a reopen of another closed node with nothing to offer
    // ends the run. 1.12 (§16): a reopen tags its node offer, so `/b` needs a cap the worker
    // lacks, and `next` offers it nothing.
    let (b, tb) = mk(&mut w, &c, s, "/b", |a| {
        a.needs = Some(BTreeSet::from([Label::new("lean").unwrap()]));
    });
    let tb = tb.unwrap();
    ok(w.call(&c, s, close_with(b, tb, Outcome::Done, &[])));
    let s2 = w.hello(&c);
    let t2 = token_of(&ok(w.call(&c, s2, take(a))));
    ok(w.call(&c, s2, close_with(a, t2, Outcome::Done, &[])));
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Reopen {
            at: id(b),
            reason: text("again"),
        }),
    ));
    // The reopened node carries the `offer` tag, and needs what the worker lacks.
    assert!(
        w.s.nodes[&b]
            .tags
            .iter()
            .any(|t| t.as_str() == crate::types::OFFER)
    );
    assert!(
        crate::query::ranked(&w.s, s, w.clock, &NextQuery::default(), 1, &mut NoMeter)
            .0
            .is_empty()
    );
    assert!(w.last_guidance.items.is_empty());
    assert_eq!(w.last_guidance.end, Some(End::Idle));
    // A session that serves a principal gets neither.
    let asst = class("assistant");
    let h = open_as(&mut w, &asst, Some("mail:alice@x"));
    ok(w.call(
        &asst,
        h,
        Cmd::Node(NodeCmd::Reopen {
            at: id(a),
            reason: text("again"),
        }),
    ));
    assert!(!rules(&w).contains(&Rule::MoreWork));
    assert_eq!(w.last_guidance.end, None);
}

/// 1.8 (§13, §20.9 rule 11, l4 F8): an agent question delivered on a write is answered `later`,
/// never before the calls to make now; at `hello` and `talk wait`, now.
#[test]
fn rule_11_answers_a_question_delivered_on_a_write_later() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let s2 = w.hello(&c);
    ok(w.call(&c, s, ask_agent(a, AgentAddressee::Session(s2))));
    // s2's next write delivers the question.
    let (b, _) = held(&mut w, &c, s2, "/b");
    let it = item_of(&w, Rule::AnswerAgent).unwrap();
    assert_eq!(it.when, Moment::Later);
    assert!(matches!(
        it.because,
        Because::AgentAsked { later: true, .. }
    ));
    assert!(
        w.last_guidance
            .items
            .iter()
            .take_while(|i| i.when == Moment::Now)
            .all(|i| i.rule != Rule::AnswerAgent)
    );
    let _ = b;
    // Delivered by `talk wait`: now.
    let s3 = w.hello(&c);
    ok(w.call(&c, s, ask_agent(a, AgentAddressee::Session(s3))));
    ok(w.call(&c, s3, wait_cmd()));
    let it = item_of(&w, Rule::AnswerAgent).unwrap();
    assert_eq!(it.when, Moment::Now);
    assert!(matches!(
        it.because,
        Because::AgentAsked { later: false, .. }
    ));
}

/// 1.8 (G1 (d), (e)): `check_guide` rejects a decide or a report on the requests of a node that
/// another session holds, a merge that closes such a node, and an `end` after a release by a
/// session that still holds a node.
#[test]
fn check_guide_rejects_another_s_node_and_an_idle_end_while_holding() {
    let (mut w, c, s) = worker();
    let (a, _) = held(&mut w, &c, s, "/a");
    let b = free(&mut w, &c, s, "/b");
    let asst = class("assistant");
    let h = w.hello(&asst);
    let (_, r) = user_request(&mut w, &asst, h, a, "mail:bob@x");
    let cmd = view_other();
    let out = w.call(&asst, h, cmd.clone());
    let g = guide(&w.s, Some(h), w.clock, &cmd, &out, &mut NoMeter);
    let g1 = |g: &Guidance| crate::check_guide(&w.s, Some(h), w.clock, &cmd, &out, g);
    assert_eq!(g1(&g), Ok(()));
    let with = |call: Skeleton| {
        let mut bad = g.clone();
        bad.items = vec![NextCall {
            rule: Rule::Decide,
            when: Moment::Now,
            once: true,
            call,
            because: Because::Signal,
        }];
        bad.total = Total::Exact(1);
        bad
    };
    let named = |e: &Result<(), Vec<crate::invariants::Violation>>| {
        e.as_ref().is_err_and(|v| {
            v.iter().any(|x| {
                x.detail == crate::invariants::ViolationDetail::Node(a)
                    && x.inv == crate::invariants::Inv::G1
            })
        })
    };
    assert!(named(&g1(&with(Skeleton::Decide {
        at: a,
        settles: vec![r],
        supersedes: None,
    }))));
    assert!(named(&g1(&with(Skeleton::ToUser {
        principal: pid(&w, "mail:bob@x"),
        kind: Some(ToUserKind::Result),
        re: vec![r],
    }))));
    assert!(named(&g1(&with(Skeleton::Merge { from: a, into: b }))));
    // (e): `Idle` after a release by a session that still holds a node.
    let (_, t1) = held(&mut w, &c, s, "/c");
    let rel = release(t1, ReleaseReason::Handoff);
    let out = w.call(&c, s, rel.clone());
    let mut bad = guide(&w.s, Some(s), w.clock, &rel, &out, &mut NoMeter);
    bad.items.clear();
    bad.total = Total::Exact(0);
    bad.end = Some(End::Idle);
    let e = crate::check_guide(&w.s, Some(s), w.clock, &rel, &out, &bad).unwrap_err();
    assert!(
        e.iter().any(|x| x.detail
            == crate::invariants::ViolationDetail::Text(
                "G1 (e): end comes with an item, or after another call"
            )),
        "{e:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// 1.10: the lab's fifth round

/// A release of `t` with `reason`, `intent_done`, and `waits_for`.
fn release_with(
    t: Token,
    reason: ReleaseReason,
    intent_done: Option<&str>,
    waits_for: Option<WaitArg>,
) -> Cmd {
    Cmd::Hold(HoldCmd::Release {
        token: t,
        done: text("did"),
        next_step: text("next"),
        reason,
        waits_for,
        intent_done: intent_done.map(text),
        learnings: None,
        unsent: BTreeSet::new(),
        settles: BTreeSet::new(),
    })
}

/// 1.10 (§20.9 rule 7 and Terms, G1 (d); it4 l2 F1): a request whose node has a checkpoint after
/// its last answer, a release among them, is left unfinished, and rule 7 never decides it: the
/// worker whose result said the work was incomplete released the node blocked, and 1.7's rule 7
/// settled Sam's request at once. The next holder is not told to decide it either, until its own
/// result names it; for a person's request, a later `said` in its conversation does the same.
#[test]
fn rule_7_never_decides_a_request_left_unfinished() {
    let (mut w, c, s) = worker();
    let (a, t) = held(&mut w, &c, s, "/comments");
    let asst = class("assistant");
    let h = open_as(&mut w, &asst, Some("mail:sam@x"));
    let (_, r) = user_request(&mut w, &asst, h, a, "mail:sam@x");
    // A partial report to Sam answers r; an open child keeps the close unclean, so rule 7 decides
    // it on the held node, as before.
    ok(w.call(
        &c,
        s,
        to_user_cmd(ToUserKind::Result, Some("mail:sam@x"), &[r]),
    ));
    assert!(w.s.requests[&r].answered_by.is_some());
    free(&mut w, &c, s, "/comments/child");
    ok(w.call(&c, s, view_other()));
    assert!(has(
        &w,
        &Skeleton::Decide {
            at: a,
            settles: vec![r],
            supersedes: None
        }
    ));
    // The blocked release: 1.7 asked for the decision now; 1.10 asks for none.
    let cmd = release_with(t, ReleaseReason::Blocked, None, None);
    let out = w.call(&c, s, cmd.clone());
    assert!(out.is_ok());
    assert!(!rules(&w).contains(&Rule::Decide), "{:?}", w.last_guidance);
    assert!(crate::spec::left_unfinished(
        &w.s,
        &cmd,
        out.as_ref().ok().map(|d| &d.reply),
        r
    ));
    // G1 (d) rejects a decide of it.
    let mut bad = guide(&w.s, Some(s), w.clock, &cmd, &out, &mut NoMeter);
    bad.items = vec![NextCall {
        rule: Rule::Decide,
        when: Moment::Now,
        once: true,
        call: Skeleton::Decide {
            at: a,
            settles: vec![r],
            supersedes: None,
        },
        because: Because::DecisionOwed {
            requests: vec![r],
            after_close: false,
            ready: false,
            recorded: false,
            held: false,
            after_release: true,
        },
    }];
    bad.total = Total::Exact(1);
    bad.end = None;
    let e = crate::check_guide(&w.s, Some(s), w.clock, &cmd, &out, &bad).unwrap_err();
    assert!(
        e.iter()
            .any(|x| x.detail == crate::invariants::ViolationDetail::Node(a)),
        "{e:?}"
    );
    // The next holder: the answer predates its hold, and the close is unclean: no decide.
    let s2 = w.hello(&c);
    let t2 = token_of(&ok(w.call(&c, s2, take(a))));
    ok(w.call(&c, s2, view_other()));
    assert!(!rules(&w).contains(&Rule::Decide), "{:?}", w.last_guidance);
    // Its own result, naming r, is r's last answer: the decision comes now.
    ok(w.call(
        &c,
        s2,
        to_user_cmd(ToUserKind::Result, Some("mail:sam@x"), &[r]),
    ));
    assert_eq!(
        item_of(&w, Rule::Decide).map(|i| i.call),
        Some(Skeleton::Decide {
            at: a,
            settles: vec![r],
            supersedes: None
        })
    );
    // A checkpoint after it: the work went on, so it is left unfinished again.
    ok(w.call(&c, s2, checkpoint_with(t2, None, None)));
    assert!(!rules(&w).contains(&Rule::Decide));
    // A person's request: a `said` of kind result answers it; after a checkpoint, no decide; a
    // later `said` in its conversation is its last answer, and the decide comes back.
    let (b, tb) = mk(&mut w, &c, s2, "/b", |x| x.keys = vec![key("chat:b/1")]);
    let tb = tb.unwrap();
    let rb = request_of(&ok(w.call(
        &c,
        s2,
        request_cmd(b, "chat:b/1/1", Some("chat:b/1")),
    )));
    ok(w.call(&c, s2, said_cmd(b, None, SaidKind::Result)));
    free(&mut w, &c, s2, "/b/child");
    ok(w.call(&c, s2, checkpoint_with(tb, None, None)));
    assert!(
        !w.last_guidance
            .items
            .iter()
            .any(|i| matches!(&i.call, Skeleton::Decide { at, .. } if *at == b)),
        "{:?}",
        w.last_guidance
    );
    ok(w.call(&c, s2, said_cmd(b, None, SaidKind::Result)));
    assert!(has(
        &w,
        &Skeleton::Decide {
            at: b,
            settles: vec![rb],
            supersedes: None
        }
    ));
}

/// 1.10 (§20.9 rule 8; it4 l7 F1): after a `handoff` release whose `intent_done` names a pull
/// request, rule 8 suggests taking the node back, to release it blocked with an outside wait for
/// the merge, which keeps it from the next worker; not after a release that names none, nor after
/// one with a wait. Rule 8's close names the review or merge as its alternative.
#[test]
fn rule_8_takes_back_a_handoff_released_after_a_pull_request() {
    let (mut w, c, s) = worker();
    let (a, t) = held(&mut w, &c, s, "/storybook");
    ok(w.call(
        &c,
        s,
        checkpoint_with(t, Some("push v2/storybook and open a PR"), None),
    ));
    ok(w.call(
        &c,
        s,
        release_with(
            t,
            ReleaseReason::Handoff,
            Some("Done: pushed v2/storybook and opened PR #215"),
            None,
        ),
    ));
    let first = &w.last_guidance.items[0];
    assert_eq!(first.rule, Rule::AwaitMerge);
    assert_eq!(first.rule.name(), "await_merge");
    assert_eq!(first.rule.number(), 8);
    assert_eq!(first.call, Skeleton::Take { target: a });
    assert_eq!(first.because, Because::AwaitMerge);
    assert_eq!(first.when, Moment::Now);
    // Followed: the take, and a blocked release with an outside wait; nothing offers it now.
    let t2 = token_of(&ok(w.call(&c, s, take(a))));
    ok(w.call(
        &c,
        s,
        release_with(
            t2,
            ReleaseReason::Blocked,
            None,
            Some(WaitArg::External {
                what: text("PR #215 merged"),
                recheck_after: When::In(Dur(HOUR.0)),
            }),
        ),
    ));
    assert!(!rules(&w).contains(&Rule::AwaitMerge));
    let s2 = w.hello(&c);
    assert!(
        crate::query::ranked(&w.s, s2, w.clock, &NextQuery::default(), 5, &mut NoMeter)
            .0
            .iter()
            .all(|x| x.cand != crate::state::Cand::Node(a))
    );
    // A handoff whose intent_done names no pull request gets no such item.
    let (b, tb) = held(&mut w, &c, s2, "/b");
    ok(w.call(&c, s2, checkpoint_with(tb, Some("push the branch"), None)));
    ok(w.call(
        &c,
        s2,
        release_with(tb, ReleaseReason::Handoff, Some("pushed the branch"), None),
    ));
    assert!(!rules(&w).contains(&Rule::AwaitMerge), "{b:?}");
    // Rule 8's close: the alternative is a review or merge to wait for; 1.12: by the node's
    // state, its last intent_done naming a pull request and no request to settle.
    let (d, td) = held(&mut w, &c, s2, "/d");
    ok(w.call(&c, s2, checkpoint_with(td, Some("open the PR"), None)));
    ok(w.call(&c, s2, checkpoint_with(td, None, Some("opened PR #9"))));
    assert!(w.s.nodes[&d].pr_named);
    let close = item_of(&w, Rule::Close).unwrap();
    assert!(matches!(close.call, Skeleton::Close { node, .. } if node == d));
    assert_eq!(
        close.because,
        Because::MergeWait {
            settles: Vec::new()
        }
    );
}

/// 1.10 (§20.9 rule 13; it4 l3 F5, l7 F4): after a close or a release, a worker's `hold next`
/// names what `next` would give, a node or a routing item, where 1.8's "if you have time for
/// more" let workers end while work waited; at `hello`, "unless you came for a message or an
/// event" stays.
#[test]
fn rule_13_names_the_work_that_waits() {
    let (mut w, c, s) = worker();
    let offered = mk(&mut w, &c, s, "/offered", |x| {
        x.take = Some(false);
        x.tags = [crate::apply::tests::label("offer")].into();
    })
    .0;
    let (a, t) = held(&mut w, &c, s, "/a");
    ok(w.call(&c, s, close_with(a, t, Outcome::Done, &[])));
    let it = item_of(&w, Rule::MoreWork).unwrap();
    assert_eq!(it.call, Skeleton::Next);
    assert_eq!(
        it.because,
        Because::WorkWaiting {
            first: crate::state::Cand::Node(offered)
        }
    );
    // At `hello`, the text of 1.7.
    let _ = w.hello(&c);
    assert_eq!(
        item_of(&w, Rule::MoreWork).unwrap().because,
        Because::MoreWork { at_hello: true }
    );
}

// ------------------------------------------------------------------------------------------------
// 1.11 (the lab's sixth round)

fn link_cmd(a: NodeId, rel: crate::types::LinkRel, b: NodeId) -> Cmd {
    Cmd::Node(NodeCmd::Link {
        at: id(a),
        rel,
        to: id(b),
    })
}

/// The guide of a response with the first row of its `related`, compared with the oracle and G1.
fn guided(
    w: &World,
    s: SessionId,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
) -> Guidance {
    let g = guide_with(&w.s, Some(s), w.clock, cmd, out, related, &mut NoMeter);
    assert_eq!(
        g,
        crate::spec::guide_with(&w.s, Some(s), w.clock, cmd, out, related)
    );
    assert_eq!(
        crate::check_guide_with(&w.s, Some(s), w.clock, cmd, out, related, &g),
        Ok(())
    );
    g
}

/// Rule 5 (1.12; it6 l4 F1, l2 F1): the item for an unrouted entry is the create by key, as in
/// 1.10; when the call that logged it has, as the first row of its `related`, an open work node
/// (not reserved, not a container, with no open child, and not under `@people` or `@teams`), the
/// create's `why` names it beside the route to none. 1.11 made the route there the item, and a
/// newcomer's question went to a team's area as a request. A person, a closed node, an area with
/// an open child, or no row names nothing; and a later call, which has no `related` of that entry,
/// names nothing either.
#[test]
fn rule_5_names_the_first_related_work_node_beside_the_create() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let inc = free(&mut w, &c, s, "/incidents/inc-311");
    let done = free(&mut w, &c, s, "/incidents/inc-300");
    ok(w.call(&c, s, crate::apply::tests::close(done, Outcome::Done)));
    let area = free(&mut w, &c, s, "/data");
    free(&mut w, &c, s, "/data/rollups");
    let noor = person(&mut w, &c, s, "noor", &["mail:noor@x"]);
    let ac = class("lead");
    let ctx = w.ctx(&ac, SessionCtx::New, None);
    let cmd = Cmd::Session(SessionCmd::Hello {
        intent: None,
        caps: None,
        horizon: None,
        principal: Some(pk("mail:noor@x")),
        from_user: Some(crate::cmd::FromUserArg {
            text: text("I just got paged: EU checkout is throwing 500s"),
            client: text("ide"),
            principal: None,
            answers: None,
            to: None,
            clarifies: None,
            reason: None,
        }),
    });
    let out = w.call_ctx(&ctx, &cmd);
    let d = out.as_ref().unwrap();
    let a = d.opened.unwrap();
    let Reply::Hello {
        user_entry: Some(u),
        ..
    } = &d.reply
    else {
        panic!("a user entry: {:?}", d.reply)
    };
    let e = u.entry;
    let g = guided(&w, a, &cmd, &out, Some(inc));
    let route = g.items.iter().find(|i| i.rule == Rule::Route).unwrap();
    assert_eq!(route.call, Skeleton::CreateFor { entry: e });
    assert_eq!(
        route.because,
        Because::Unrouted {
            entry: e,
            related: Some(inc)
        }
    );
    // A person, a closed node, an area with an open child, or nothing: the create names none.
    for row in [Some(noor), Some(done), Some(area), None] {
        let g = guided(&w, a, &cmd, &out, row);
        let route = g.items.iter().find(|i| i.rule == Rule::Route).unwrap();
        assert_eq!(route.call, Skeleton::CreateFor { entry: e }, "{row:?}");
        assert_eq!(
            route.because,
            Because::Unrouted {
                entry: e,
                related: None
            },
            "{row:?}"
        );
    }
    // A later call has no related row of this entry.
    let later = view(inc);
    let out2 = w.call(&ac, a, later.clone());
    let g = guided(&w, a, &later, &out2, Some(inc));
    let route = g.items.iter().find(|i| i.rule == Rule::Route).unwrap();
    assert_eq!(route.call, Skeleton::CreateFor { entry: e });
    assert_eq!(
        route.because,
        Because::Unrouted {
            entry: e,
            related: None
        }
    );
}

/// Rule 8 (1.11; it5 l1 F1): an "I'll look" reply of kind `reply` answers the request, and the
/// close comes now; after the checkpoint that follows it, the request is left unfinished and no
/// close comes; the result that ends the work makes the close come again.
#[test]
fn rule_8_counts_a_request_left_unfinished_as_not_answered() {
    let (mut w, c, s) = worker();
    let (a, t) = mk(&mut w, &c, s, "/deploy/slowdown", |x| {
        x.keys = vec![key("chat:eng/T2001")]
    });
    let t = t.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:eng/T2001/M1", Some("chat:eng/T2001")),
    )));
    let close = Skeleton::Close {
        node: a,
        token: t,
        settles: vec![r],
        outcome: None,
    };
    ok(w.call(&c, s, said_cmd(a, None, SaidKind::Reply)));
    assert!(has(&w, &close));
    let cp = crate::apply::tests::checkpoint(t);
    let out = w.call(&c, s, cp.clone());
    ok(out.clone());
    assert!(
        !w.last_guidance
            .items
            .iter()
            .any(|i| matches!(i.call, Skeleton::Close { node, .. } if node == a)),
        "{:?}",
        w.last_guidance
    );
    assert_eq!(
        w.last_guidance,
        crate::spec::guide(&w.s, Some(s), w.clock, &cp, &out)
    );
    ok(w.call(&c, s, said_cmd(a, None, SaidKind::Result)));
    assert!(has(
        &w,
        &Skeleton::Close {
            node: a,
            token: t,
            settles: vec![],
            outcome: None
        }
    ));
}

/// Rule 10 (1.11; it5 l4 F2): after a `node link {rel: duplicate}`, the merge of the pair in
/// §16's direction, now and once, "if they are one piece of work"; into the held node when the
/// newer one is held; none when the merge would close a node another session holds.
#[test]
fn rule_10_suggests_the_merge_after_a_duplicate_link() {
    let (mut w, c, s) = worker();
    let other = w.hello(&c);
    let older = free(&mut w, &c, other, "/data/warehouse-cost-review");
    let (newer, _) = held(&mut w, &c, s, "/data/review-warehouse-spend");
    let cmd = link_cmd(newer, crate::types::LinkRel::Duplicate, older);
    let out = w.call(&c, s, cmd.clone());
    ok(out.clone());
    let g = guided(&w, s, &cmd, &out, None);
    let merge = g.items.iter().find(|i| i.rule == Rule::HeldFlag).unwrap();
    // The newer one is held: the older merges into it.
    assert_eq!(
        merge.call,
        Skeleton::Merge {
            from: older,
            into: newer
        }
    );
    assert_eq!(merge.because, Because::Linked { into: newer });
    assert_eq!((merge.when, merge.once), (Moment::Now, true));
    // Neither held: the newer into the older.
    let a = free(&mut w, &c, other, "/data/a-review");
    let b = free(&mut w, &c, other, "/data/b-review");
    let cmd = link_cmd(b, crate::types::LinkRel::Duplicate, a);
    let out = w.call(&c, s, cmd.clone());
    let g = guided(&w, s, &cmd, &out, None);
    assert!(
        g.items
            .iter()
            .any(|i| i.call == Skeleton::Merge { from: b, into: a })
    );
    // Both held, the older by another session: the merge would close its node, so none.
    let (x, _) = held(&mut w, &c, other, "/data/x-review");
    let (y, _) = held(&mut w, &c, s, "/data/y-review");
    let cmd = link_cmd(y, crate::types::LinkRel::Duplicate, x);
    let out = w.call(&c, s, cmd.clone());
    let g = guided(&w, s, &cmd, &out, None);
    assert!(
        !g.items
            .iter()
            .any(|i| matches!(i.call, Skeleton::Merge { .. })),
        "{g:?}"
    );
    // A conflict link suggests no merge.
    let cmd = link_cmd(b, crate::types::LinkRel::ConflictsWith, a);
    let out = w.call(&c, s, cmd.clone());
    let g = guided(&w, s, &cmd, &out, None);
    assert!(
        !g.items
            .iter()
            .any(|i| i.because == Because::Linked { into: a })
    );
}

// ------------------------------------------------------------------------------------------------
// 1.12 (the lab's seventh round, it6)

/// A session that says `hello` with `caps` and a horizon, as a worker.
fn worker_with(w: &mut World, c: &ClassName, caps: &[&str], horizon: Option<u32>) -> SessionId {
    let ctx = w.ctx(c, SessionCtx::New, None);
    let d = ok(w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some(caps.iter().map(|l| crate::apply::tests::label(l)).collect()),
            horizon,
            principal: None,
            from_user: None,
        }),
    ));
    d.opened.unwrap()
}

/// Rule 8 (1.12; it6 l7 F1, l2 F2, l1 F3, l6 F5): the close's `why` names its alternative by the
/// node's state, never "decide now". With no pull request named, the rest is handed off; once the
/// node's last `intent_done` names one, the merge is waited for, with the answered requests the
/// blocked release would settle; and on such a node a decision makes the close `later`, never `now`.
#[test]
fn rule_8_names_its_alternative_by_the_node_s_state() {
    let (mut w, c, s) = worker();
    // No pull request: "if done; else release handoff".
    let (a, ta) = held(&mut w, &c, s, "/a");
    ok(w.call(&c, s, checkpoint_with(ta, Some("push the branch"), None)));
    ok(w.call(&c, s, checkpoint_with(ta, None, Some("pushed the branch"))));
    assert!(!w.s.nodes[&a].pr_named);
    let close = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        (close.when, close.because),
        (Moment::Later, Because::Finished)
    );
    // A pull request and no request: "if it is merged; …", later; a decision keeps it later.
    let (b, tb) = held(&mut w, &c, s, "/b");
    ok(w.call(&c, s, checkpoint_with(tb, Some("open the PR"), None)));
    ok(w.call(&c, s, checkpoint_with(tb, None, Some("opened PR #415"))));
    assert!(w.s.nodes[&b].pr_named);
    let merge = Because::MergeWait {
        settles: Vec::new(),
    };
    let close = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        (close.when, close.because.clone()),
        (Moment::Later, merge.clone())
    );
    ok(w.call(&c, s, decide_cmd(b, &[])));
    let close = item_of(&w, Rule::Close).unwrap();
    assert!(matches!(close.call, Skeleton::Close { node, .. } if node == b));
    assert_eq!((close.when, close.because), (Moment::Later, merge));
    // The same decision on a node with no pull request named makes it `now`, as before.
    ok(w.call(&c, s, decide_cmd(a, &[])));
    let close = item_of(&w, Rule::Close).unwrap();
    assert!(matches!(close.call, Skeleton::Close { node, .. } if node == a));
    assert_eq!(
        (close.when, close.because),
        (Moment::Now, Because::Finished)
    );
    // A pull request and an answered request: the release would settle it.
    let s = w.hello(&c);
    let (m, tm) = mk(&mut w, &c, s, "/m", |x| x.keys = vec![key("chat:x/9")]);
    let tm = tm.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(m, "chat:x/9/1", Some("chat:x/9")),
    )));
    ok(w.call(&c, s, checkpoint_with(tm, Some("open the PR"), None)));
    ok(w.call(&c, s, checkpoint_with(tm, None, Some("opened PR #12"))));
    ok(w.call(&c, s, said_cmd(m, None, SaidKind::Result)));
    let close = item_of(&w, Rule::Close).unwrap();
    assert!(matches!(close.call, Skeleton::Close { node, .. } if node == m));
    assert_eq!(
        (close.when, close.because),
        (Moment::Now, Because::MergeWait { settles: vec![r] })
    );
}

/// Rule 9 (1.12; it6 l5 F1): a resolve that returns `no_basis`, the objection failing on a claim
/// nothing proves, gets the reopen now, before `hold next`; a resolve that warns nothing gets none.
#[test]
fn rule_9_reopens_a_claim_nothing_proves() {
    let (mut w, c, s) = worker();
    let claim = |x: &mut CreateArgs| x.kind = Some(crate::apply::tests::label("claim"));
    let (lemma, t) = mk(&mut w, &c, s, "/claims/lemma-11", claim);
    let Reply::Decided { decision, .. } = ok(w.call(&c, s, decide_cmd(lemma, &[]))).reply else {
        panic!()
    };
    ok(w.call(&c, s, close_with(lemma, t.unwrap(), Outcome::Done, &[])));
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: decision,
                node: lemma,
            },
            text: text("the check double-counted"),
            reason: text("duplicate rows"),
        }),
    ));
    let s2 = w.hello(&c);
    let Reply::Objected { objection } = ok(w.call(
        &c,
        s2,
        Cmd::Talk(TalkCmd::Object {
            on: id(lemma),
            note: text("the counterexample"),
        }),
    ))
    .reply
    else {
        panic!()
    };
    let d = ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Resolve {
            objection,
            note: text("the counterexample is invalid; nothing proves the lemma either"),
            upheld: false,
            defer: false,
        }),
    ));
    assert!(d.warnings.iter().any(|x| x.code == WarningCode::NoBasis));
    let first = w.last_guidance.items.first().cloned().unwrap();
    assert_eq!(
        (first.rule, first.when, first.call, first.because),
        (
            Rule::Unproved,
            Moment::Now,
            Skeleton::Reopen { node: lemma },
            Because::Unproved
        )
    );
    assert_eq!(Rule::Unproved.name(), "unproved");
    assert_eq!(Rule::Unproved.number(), 9);
    // Followed, the claim is open work again, unproved, not refuted.
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Reopen {
            at: id(lemma),
            reason: text("nothing proves it"),
        }),
    ));
    assert!(w.s.nodes[&lemma].is_open());
    // A resolve that keeps a node with an active decision warns nothing, and gets no reopen.
    let (task, tt) = held(&mut w, &c, s, "/tasks/fix");
    ok(w.call(&c, s, decide_cmd(task, &[])));
    ok(w.call(&c, s, close_with(task, tt, Outcome::Done, &[])));
    let Reply::Objected { objection } = ok(w.call(
        &c,
        s2,
        Cmd::Talk(TalkCmd::Object {
            on: id(task),
            note: text("wrong"),
        }),
    ))
    .reply
    else {
        panic!()
    };
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Resolve {
            objection,
            note: text("fine"),
            upheld: false,
            defer: false,
        }),
    ));
    assert!(!rules(&w).contains(&Rule::Unproved));
}

/// Rule 13 (1.12; it6 l5 F8, l2 F5): a worker that holds nothing gets `hold next` after any write
/// while `next` would offer it an item, naming the node; and the tidy take is of the first node of
/// `leave_better` that the worker's caps and horizon fit.
#[test]
fn rule_13_follows_any_write_and_takes_only_what_fits() {
    let (mut w, c, s) = worker();
    let o = mk(&mut w, &c, s, "/o", |x| {
        x.take = Some(false);
        x.tags = [crate::apply::tests::label("offer")].into();
    })
    .0;
    // A note at a node by a worker that holds nothing: `hold next`, which names the node.
    ok(w.call(
        &c,
        s,
        Cmd::Talk(TalkCmd::Say {
            at: id(o),
            text: text("progress so far"),
            to: None,
            expects: None,
            reply_to: None,
            until: None,
        }),
    ));
    let it = item_of(&w, Rule::MoreWork).unwrap();
    assert_eq!(
        (it.call, it.because),
        (
            Skeleton::Next,
            Because::WorkWaiting {
                first: crate::state::Cand::Node(o)
            }
        )
    );
    // A read gets nothing of the kind.
    ok(w.call(&c, s, view(o)));
    assert!(!rules(&w).contains(&Rule::MoreWork));
    // The tidy take: siblings flagged by an unanswered request, the first over a 30-minute
    // horizon, the second within it.
    let h = worker_with(&mut w, &c, &[], Some(30));
    let long = mk(&mut w, &c, s, "/q/long", |x| {
        x.take = Some(false);
        x.est = Some(60);
        x.keys = vec![key("chat:q/1")];
    })
    .0;
    let short = mk(&mut w, &c, s, "/q/short", |x| {
        x.take = Some(false);
        x.est = Some(10);
        x.keys = vec![key("chat:q/2")];
    })
    .0;
    ok(w.call(&c, s, request_cmd(long, "chat:q/1/1", Some("chat:q/1"))));
    ok(w.call(&c, s, request_cmd(short, "chat:q/2/1", Some("chat:q/2"))));
    let (z, tz) = held(&mut w, &c, h, "/q/z");
    let d = ok(w.call(&c, h, close_with(z, tz, Outcome::Done, &[])));
    assert!(
        matches!(&d.reply, Reply::Closed { leave_better, .. }
            if leave_better.first() == Some(&long) && leave_better.contains(&short)),
        "{:?}",
        d.reply
    );
    let it = item_of(&w, Rule::Tidy).unwrap();
    assert_eq!(it.call, Skeleton::Take { target: short });
}

/// §10.4 (1.12; it6 l5 F2): `next`, `leave_better`, and so rule 13 keep from a session what it
/// put down: the node it released as `handoff`, until another hold of it or `lease.first`, while
/// another session gets it at once; a blocked release keeps nothing.
#[test]
fn next_keeps_from_a_session_the_node_it_handed_off() {
    let (mut w, c, s) = worker();
    let (a, t) = held(&mut w, &c, s, "/a");
    ok(w.call(&c, s, release(t, ReleaseReason::Handoff)));
    assert!(crate::state::handed_back(&w.s, s, a, w.clock));
    assert!(!rules(&w).contains(&Rule::MoreWork));
    let d = ok(w.call(&c, s, crate::apply::tests::next()));
    assert!(
        matches!(&d.reply, Reply::Next(NextReply::Empty { why, .. }) if why.own == 1),
        "{:?}",
        d.reply
    );
    let s2 = w.hello(&c);
    let first = |w: &World, x: SessionId| {
        crate::query::ranked(&w.s, x, w.clock, &NextQuery::default(), 1, &mut NoMeter)
            .0
            .first()
            .map(|r| r.cand)
    };
    assert_eq!(first(&w, s2), Some(crate::state::Cand::Node(a)));
    // Another hold of it ends the keeping: s2 takes it and hands it off in turn.
    let t2 = token_of(&ok(w.call(&c, s2, take(a))));
    ok(w.call(&c, s2, release(t2, ReleaseReason::Handoff)));
    assert!(!crate::state::handed_back(&w.s, s, a, w.clock));
    assert_eq!(first(&w, s), Some(crate::state::Cand::Node(a)));
    assert_eq!(first(&w, s2), None);
    // `lease.first` later, it comes back to s2 too.
    w.advance(Dur(w.s.params.lease_first.0 + SECOND.0));
    assert_eq!(first(&w, s2), Some(crate::state::Cand::Node(a)));
    // A blocked release keeps nothing.
    let t3 = token_of(&ok(w.call(&c, s, take(a))));
    let mut blocked = release(t3, ReleaseReason::Blocked);
    if let Cmd::Hold(HoldCmd::Release { waits_for, .. }) = &mut blocked {
        *waits_for = Some(WaitArg::Time(When::In(SECOND)));
    }
    ok(w.call(&c, s, blocked));
    w.advance(Dur(2 * SECOND.0));
    assert!(!crate::state::handed_back(&w.s, s, a, w.clock));
    assert_eq!(first(&w, s), Some(crate::state::Cand::Node(a)));
}

// ------------------------------------------------------------------------------------------------
// 1.12's review

/// The external wait of a merge, as rule 8's `why` names it.
fn merge_wait() -> Option<WaitArg> {
    Some(WaitArg::External {
        what: text("PR #12 merged"),
        recheck_after: When::In(Dur(2 * HOUR.0)),
    })
}

/// Rule 8 (1.12's review): the node's open intent that names a pull request makes its close wait
/// for the merge, as its last `intent_done` does. So the flow rule 3 asks for, an intent before the
/// pull request and its `intent_done` passed in the release, is told at the result the blocked
/// release that settles the request, where it was told "else release handoff"; then one release
/// closes the intent, settles the request, and records the wait.
#[test]
fn rule_8_waits_for_the_merge_an_open_intent_names() {
    let (mut w, c, s) = worker();
    let (m, tm) = mk(&mut w, &c, s, "/m", |x| x.keys = vec![key("chat:x/9")]);
    let tm = tm.unwrap();
    let r = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(m, "chat:x/9/1", Some("chat:x/9")),
    )));
    ok(w.call(
        &c,
        s,
        checkpoint_with(tm, Some("push the fix and open a PR"), None),
    ));
    let node = &w.s.nodes[&m];
    assert!(node.open_intent.is_some_and(|i| i.pr_named) && !node.pr_named && node.merge_wait());
    // The push and the pull request happen outside the forum; the result answers the request.
    let cmd = said_cmd(m, None, SaidKind::Result);
    let out = w.call(&c, s, cmd.clone());
    assert_eq!(guided(&w, s, &cmd, &out, None), w.last_guidance);
    let close = item_of(&w, Rule::Close).unwrap();
    assert!(matches!(close.call, Skeleton::Close { node, .. } if node == m));
    assert_eq!(
        (close.when, close.because),
        (Moment::Now, Because::MergeWait { settles: vec![r] })
    );
    let intent = item_of(&w, Rule::IntentDone).unwrap();
    assert_eq!(intent.when, Moment::Later);
    // The one release the `why` names, with the intent's `intent_done`.
    let mut cmd = release_with(
        tm,
        ReleaseReason::Blocked,
        Some("pushed and opened PR #12"),
        merge_wait(),
    );
    if let Cmd::Hold(HoldCmd::Release { settles, .. }) = &mut cmd {
        *settles = [r].into();
    }
    let d = ok(w.call(&c, s, cmd));
    assert!(
        matches!(&d.reply, Reply::Released { settled, intent_done: Some(_), .. }
            if *settled == vec![r]),
        "{:?}",
        d.reply
    );
    assert!(w.s.requests[&r].settled.is_some());
    let node = &w.s.nodes[&m];
    assert!(node.open_intent.is_none() && node.pr_named && node.is_open());
    // An intent that names no pull request keeps "else release handoff", and a decision makes the
    // close `now`; on a node whose open intent names one, the same decision makes it `later`.
    let (a, ta) = held(&mut w, &c, s, "/a");
    ok(w.call(&c, s, checkpoint_with(ta, Some("push the branch"), None)));
    ok(w.call(&c, s, decide_cmd(a, &[])));
    let close = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        (close.when, close.because),
        (Moment::Now, Because::Finished)
    );
    let (b, tb) = held(&mut w, &c, s, "/b");
    ok(w.call(
        &c,
        s,
        checkpoint_with(tb, Some("open a merge request"), None),
    ));
    ok(w.call(&c, s, decide_cmd(b, &[])));
    let close = item_of(&w, Rule::Close).unwrap();
    assert!(matches!(close.call, Skeleton::Close { node, .. } if node == b));
    assert_eq!(
        (close.when, close.because),
        (
            Moment::Later,
            Because::MergeWait {
                settles: Vec::new()
            }
        )
    );
}

/// §5 (1.12's review): a reopen clears the node's `pr_named`, so a pull request named in an
/// earlier round of its work is not this round's merge wait.
#[test]
fn a_reopen_forgets_the_pull_request_of_the_last_round() {
    let (mut w, c, s) = worker();
    let (d, td) = held(&mut w, &c, s, "/d");
    ok(w.call(&c, s, checkpoint_with(td, Some("open the PR"), None)));
    ok(w.call(&c, s, checkpoint_with(td, None, Some("opened PR #9"))));
    ok(w.call(&c, s, close_with(d, td, Outcome::Done, &[])));
    assert!(w.s.nodes[&d].pr_named);
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Reopen {
            at: id(d),
            reason: text("the fix regressed"),
        }),
    ));
    assert!(!w.s.nodes[&d].pr_named);
    // A plain take and a decision: the close is `now`, with no merge to wait for.
    let s2 = w.hello(&c);
    token_of(&ok(w.call(&c, s2, take(d))));
    ok(w.call(&c, s2, decide_cmd(d, &[])));
    let close = item_of(&w, Rule::Close).unwrap();
    assert_eq!(
        (close.when, close.because),
        (Moment::Now, Because::Finished)
    );
}

/// Rule 3 (1.12's review; the guided property test's seed 9): a due checkpoint is `later` while
/// rule 8 closes the hold's node now. Made first, it left the request the result had just answered
/// unfinished, and nothing suggested the close again; without the close, it is `now`, as before.
#[test]
fn rule_3_s_due_checkpoint_comes_after_rule_8_s_close() {
    let (mut w, c, s) = worker();
    let (m, tm) = mk(&mut w, &c, s, "/m", |x| {
        x.keys = vec![key("chat:x/9"), key("chat:x/8")];
    });
    let tm = tm.unwrap();
    ok(w.call(&c, s, request_cmd(m, "chat:x/9/1", Some("chat:x/9"))));
    ok(w.call(&c, s, said_cmd(m, Some("chat:x/9"), SaidKind::Result)));
    w.advance(Dur(5 * MINUTE.0));
    let out = w.call(&c, s, view_other());
    assert_eq!(guided(&w, s, &view_other(), &out, None), w.last_guidance);
    let first = &w.last_guidance.items[0];
    assert_eq!((first.rule, first.when), (Rule::Close, Moment::Now));
    let cp = item_of(&w, Rule::Checkpoint).unwrap();
    assert_eq!(cp.when, Moment::Later);
    assert!(
        matches!(cp.because, Because::LeaseDue { closing: true, .. }),
        "{cp:?}"
    );
    // Followed, the close is clean.
    ok(w.call(&c, s, close_default(m, tm)));
    assert!(!w.s.nodes[&m].is_open());
    // With a request unanswered, no close: the due checkpoint is `now`.
    let (n, _) = mk(&mut w, &c, s, "/n", |x| x.keys = vec![key("chat:x/7")]);
    ok(w.call(&c, s, request_cmd(n, "chat:x/7/1", Some("chat:x/7"))));
    w.advance(Dur(5 * MINUTE.0));
    ok(w.call(&c, s, view_other()));
    assert!(item_of(&w, Rule::Close).is_none());
    let cp = item_of(&w, Rule::Checkpoint).unwrap();
    assert_eq!(cp.when, Moment::Now);
    assert!(matches!(
        cp.because,
        Because::LeaseDue { closing: false, .. }
    ));
}

/// §20.9 Terms (1.12's review): a release's `settles` puts its requests in reach, as a close's and
/// a decide's do. An unanswered one, which the release's decision lists and so is owed a reply,
/// gets rule 6's reply; and a release that settles the last unsettled request of another node the
/// session holds brings that node's close now, as a decide that settles it would.
#[test]
fn a_release_s_settles_are_in_reach() {
    let (mut w, c, s) = worker();
    let (m, tm) = mk(&mut w, &c, s, "/m", |x| {
        x.keys = vec![key("chat:x/9"), key("chat:x/8")];
    });
    let tm = tm.unwrap();
    let r1 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(m, "chat:x/9/1", Some("chat:x/9")),
    )));
    let r2 = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(m, "chat:x/8/1", Some("chat:x/8")),
    )));
    ok(w.call(&c, s, said_cmd(m, Some("chat:x/9"), SaidKind::Result)));
    assert!(w.s.requests[&r2].answered_by.is_none());
    let mut cmd = release_with(tm, ReleaseReason::Blocked, None, merge_wait());
    if let Cmd::Hold(HoldCmd::Release { settles, .. }) = &mut cmd {
        *settles = [r1, r2].into();
    }
    let out = w.call(&c, s, cmd.clone());
    assert_eq!(guided(&w, s, &cmd, &out, None), w.last_guidance);
    let d = ok(out);
    assert!(
        matches!(&d.reply, Reply::Released { settled, not_settled, .. }
            if *settled == vec![r1] && not_settled.iter().any(|x| x.request == r2)),
        "{:?}",
        d.reply
    );
    let reply = item_of(&w, Rule::Reply).unwrap();
    assert!(
        matches!(reply.call, Skeleton::Said { at, .. } if at == m),
        "{reply:?}"
    );
    // Another held node's last request, settled by a release.
    let s = w.hello(&c);
    let (a, _) = mk(&mut w, &c, s, "/a", |x| x.keys = vec![key("chat:x/6")]);
    let ra = request_of(&ok(w.call(
        &c,
        s,
        request_cmd(a, "chat:x/6/1", Some("chat:x/6")),
    )));
    ok(w.call(&c, s, said_cmd(a, Some("chat:x/6"), SaidKind::Result)));
    let (_, tb) = held(&mut w, &c, s, "/b");
    let mut cmd = release_with(tb, ReleaseReason::Blocked, None, merge_wait());
    if let Cmd::Hold(HoldCmd::Release { settles, .. }) = &mut cmd {
        *settles = [ra].into();
    }
    let out = w.call(&c, s, cmd.clone());
    assert_eq!(guided(&w, s, &cmd, &out, None), w.last_guidance);
    assert!(w.s.requests[&ra].settled.is_some());
    let close = item_of(&w, Rule::Close).unwrap();
    assert!(matches!(close.call, Skeleton::Close { node, .. } if node == a));
    assert_eq!(close.when, Moment::Now);
}

/// G1 (d) and (f) (1.12): each clause rejects its own violation, in forged next calls: a `now` close
/// of a node waiting for its merge after a decision, a due checkpoint `now` beside the `now` close
/// of its node (1.12's review), a tidy take of the node the session handed off, a tidy take over
/// the session's horizon, and a tidy review of the session's own objection.
#[test]
fn check_guide_rejects_each_1_12_clause_s_violation() {
    use crate::invariants::ViolationDetail;
    fn forge(
        w: &World,
        s: SessionId,
        cmd: &Cmd,
        out: &Result<Done, Error>,
        it: NextCall,
    ) -> Vec<crate::invariants::Violation> {
        let mut bad = guide(&w.s, Some(s), w.clock, cmd, out, &mut NoMeter);
        bad.items = vec![it];
        bad.total = Total::Exact(1);
        bad.end = None;
        crate::check_guide(&w.s, Some(s), w.clock, cmd, out, &bad).unwrap_err()
    }
    let tidy = |call, because| NextCall {
        rule: Rule::Tidy,
        when: Moment::Now,
        once: true,
        call,
        because,
    };
    let (mut w, c, s) = worker();
    // (d): a decision on a node whose open intent names a pull request: the close is `later`.
    let (b, tb) = held(&mut w, &c, s, "/b");
    ok(w.call(&c, s, checkpoint_with(tb, Some("open the PR"), None)));
    let cmd = decide_cmd(b, &[]);
    let out = w.call(&c, s, cmd.clone());
    assert_eq!(
        item_of(&w, Rule::Close).map(|i| i.when),
        Some(Moment::Later)
    );
    let e = forge(
        &w,
        s,
        &cmd,
        &out,
        NextCall {
            rule: Rule::Close,
            when: Moment::Now,
            once: true,
            call: Skeleton::Close {
                node: b,
                token: tb,
                settles: Vec::new(),
                outcome: None,
            },
            because: Because::MergeWait {
                settles: Vec::new(),
            },
        },
    );
    assert!(
        e.iter().any(|x| x.detail == ViolationDetail::Node(b)),
        "{e:?}"
    );
    // (d), 1.12's review: a due checkpoint `now` beside the `now` close of its node.
    let (m, tm) = mk(&mut w, &c, s, "/m", |x| x.keys = vec![key("chat:x/9")]);
    let tm = tm.unwrap();
    ok(w.call(&c, s, request_cmd(m, "chat:x/9/1", Some("chat:x/9"))));
    ok(w.call(&c, s, said_cmd(m, None, SaidKind::Result)));
    w.advance(Dur(5 * MINUTE.0));
    let cmd = view_other();
    let out = w.call(&c, s, cmd.clone());
    let mut bad = guide(&w.s, Some(s), w.clock, &cmd, &out, &mut NoMeter);
    let due = Skeleton::Checkpoint {
        token: tm,
        intent_done: false,
        intent: false,
    };
    let cp = bad.items.iter().position(|i| i.call == due).unwrap();
    let mut it = bad.items.remove(cp);
    it.when = Moment::Now;
    bad.items.insert(0, it);
    let e = crate::check_guide(&w.s, Some(s), w.clock, &cmd, &out, &bad).unwrap_err();
    assert!(
        e.iter().any(|x| x.detail == ViolationDetail::Node(m)),
        "{e:?}"
    );
    ok(w.call(&c, s, close_default(m, tm)));
    // (f): a tidy take of the node the session handed off.
    let (a, ta) = held(&mut w, &c, s, "/a");
    let cmd = release(ta, ReleaseReason::Handoff);
    let out = w.call(&c, s, cmd.clone());
    assert!(crate::state::handed_back(&w.s, s, a, w.clock));
    let e = forge(
        &w,
        s,
        &cmd,
        &out,
        tidy(
            Skeleton::Take { target: a },
            Because::Tidy {
                node: a,
                flags: FlagSet::default(),
            },
        ),
    );
    assert!(
        e.iter().any(|x| x.detail == ViolationDetail::Node(a)),
        "{e:?}"
    );
    // (f): a tidy take of a node longer than the session's horizon.
    let h = worker_with(&mut w, &c, &[], Some(30));
    let long = mk(&mut w, &c, s, "/q/long", |x| {
        x.take = Some(false);
        x.est = Some(60);
    })
    .0;
    let (z, tz) = held(&mut w, &c, h, "/q/z");
    let cmd = close_with(z, tz, Outcome::Done, &[]);
    let out = w.call(&c, h, cmd.clone());
    let e = forge(
        &w,
        h,
        &cmd,
        &out,
        tidy(
            Skeleton::Take { target: long },
            Because::Tidy {
                node: long,
                flags: FlagSet::default(),
            },
        ),
    );
    assert!(
        e.iter().any(|x| x.detail == ViolationDetail::Node(long)),
        "{e:?}"
    );
    // (f): the review of the session's own objection.
    let (x, tx) = held(&mut w, &c, h, "/p/x");
    ok(w.call(&c, h, close_with(x, tx, Outcome::Done, &[])));
    let Reply::Objected { objection } = ok(w.call(
        &c,
        h,
        Cmd::Talk(TalkCmd::Object {
            on: id(x),
            note: text("a counterexample"),
        }),
    ))
    .reply
    else {
        panic!()
    };
    let (y, ty) = held(&mut w, &c, h, "/p/y");
    let cmd = close_with(y, ty, Outcome::Done, &[]);
    let out = w.call(&c, h, cmd.clone());
    let e = forge(
        &w,
        h,
        &cmd,
        &out,
        tidy(
            Skeleton::Resolve { objection },
            Because::Review { objection },
        ),
    );
    assert!(
        e.iter().any(|v| v.detail
            == ViolationDetail::Text(
                "G1 (f): rule 13 gives a session the review of its own objection"
            )),
        "{e:?}"
    );
}
