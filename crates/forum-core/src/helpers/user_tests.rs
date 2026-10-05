//! Unit tests of the D5a helpers (design D.8.12 to D.8.14, D.12.1's user side), and the fixtures
//! the command tests of `cmd/talk_user_tests.rs` share. Every call runs through `World::call_ctx`,
//! and every firing through `World::settle`, which check `check_step` and `check_fire` (U1, D1,
//! Settled, N1, and the UserQuestion firing's shape), the dirty set, `decide == apply`, the fold of
//! the events, and every state invariant with R1 (E1 (a), (e), (f) among them).

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
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::apply::tests::{T0, class, create_with, created, ok, text};
use crate::cmd::*;
use crate::event::{EntryBody, Ev, QuestionBody};
use crate::params::Params;
use crate::reply::{Done, Reply, RoutedReply};
use crate::spec;
use crate::state::{DigestState, UqState};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Fixtures, shared with `cmd/talk_user_tests.rs`

pub(crate) fn pk(k: &str) -> PrincipalKey {
    PrincipalKey::new(k).unwrap()
}

pub(crate) fn key(k: &str) -> Key {
    Key {
        key: KeyStr::new(k).unwrap(),
        nv: NormVer(1),
    }
}

/// A world at `T0` with the default parameters changed by `f`.
pub(crate) fn world_with(f: impl FnOnce(&mut Params)) -> World {
    let mut p = Params::default();
    f(&mut p);
    World::new(p, T0)
}

/// Opens a session of class `c` with `hello {principal}`.
pub(crate) fn open_as(w: &mut World, c: &ClassName, principal: Option<&str>) -> SessionId {
    let ctx = w.ctx(c, SessionCtx::New, None);
    let cmd = Cmd::Session(SessionCmd::Hello {
        intent: None,
        caps: None,
        horizon: None,
        principal: principal.map(pk),
        from_user: None,
    });
    ok(w.call_ctx(&ctx, &cmd)).opened.unwrap()
}

/// Creates `p` without a hold, after `f`.
pub(crate) fn mk(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    p: &str,
    f: impl FnOnce(&mut CreateArgs),
) -> NodeId {
    created(&ok(w.call(
        c,
        s,
        create_with(p, |a| {
            a.take = Some(false);
            f(a);
        }),
    )))
    .0
}

/// A person `/@people/<name>` whose aliases are the principal keys `aliases`.
pub(crate) fn person(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    name: &str,
    aliases: &[&str],
) -> NodeId {
    mk(w, c, s, &format!("/@people/{name}"), |a| {
        a.keys = aliases
            .iter()
            .map(|k| key(&format!("person:{k}")))
            .collect();
    })
}

/// The interned id of principal key `k`.
pub(crate) fn pid(w: &World, k: &str) -> PrincipalId {
    w.s.principal_ids[&pk(k)]
}

/// `node update` of `n` at its current version, with `f` filling the patch.
pub(crate) fn update(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    n: NodeId,
    f: impl FnOnce(&mut Patch),
) {
    let mut patch = Patch::default();
    f(&mut patch);
    let version = w.s.nodes[&n].version;
    ok(w.call(
        c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(n),
            version,
            digest_version: None,
            token: None,
            patch,
        }),
    ));
}

/// Sets the `owners` of `n`.
pub(crate) fn set_owners(w: &mut World, c: &ClassName, s: SessionId, n: NodeId, owners: &[NodeId]) {
    update(w, c, s, n, |p| {
        p.owners = Some(owners.iter().map(|&o| NodeRef::Id(o)).collect());
    });
}

/// Tags a person `gone`.
pub(crate) fn mark_gone(w: &mut World, c: &ClassName, s: SessionId, n: NodeId) {
    update(w, c, s, n, |p| {
        p.tags_add = [Label::new(GONE).unwrap()].into();
    });
}

/// `talk from_user`.
pub(crate) fn from_user_cmd(
    t: &str,
    principal: Option<&str>,
    answers: Option<Eid>,
    node: Option<NodeId>,
    clarifies: Option<Eid>,
) -> Cmd {
    Cmd::Talk(TalkCmd::FromUser {
        text: text(t),
        client: text("chat"),
        principal: principal.map(pk),
        answers,
        to: (node.map(NodeRef::Id)).map(crate::cmd::RouteTo::Node),
        clarifies,
        reason: None,
    })
}

/// The entry and the `routed` of a `from_user` reply.
pub(crate) fn routed(d: &Done) -> (UserEntryId, RoutedReply) {
    match &d.reply {
        Reply::FromUser { entry, routed, .. } => (*entry, *routed),
        other => panic!("not a from_user reply: {other:?}"),
    }
}

/// A user request of `principal` on `n`, recorded by `s` with `from_user {node: n}`.
pub(crate) fn user_request(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    n: NodeId,
    principal: &str,
) -> (UserEntryId, RequestId) {
    let d = ok(w.call(
        c,
        s,
        from_user_cmd("please", Some(principal), None, Some(n), None),
    ));
    match routed(&d) {
        (e, RoutedReply::Request { request, node, .. }) => {
            assert_eq!(node, n);
            (e, request)
        }
        other => panic!("not routed as a request: {other:?}"),
    }
}

/// `talk to_user`.
pub(crate) fn to_user_cmd(kind: ToUserKind, principal: Option<&str>, re: &[Eid]) -> Cmd {
    Cmd::Talk(TalkCmd::ToUser {
        text: text("report"),
        kind,
        verbatim: true,
        principal: principal.map(pk),
        re: re.iter().copied().collect(),
        at: None,
    })
}

/// `talk route`.
pub(crate) fn route_cmd(entry: UserEntryId, to: Option<NodeId>, reason: &str) -> Cmd {
    Cmd::Talk(TalkCmd::Route {
        entry,
        to: Some(
            (to.map(NodeRef::Id)).map_or(crate::cmd::RouteTo::Nowhere, crate::cmd::RouteTo::Node),
        ),
        answers: None,
        clarifies: None,
        reason: Some(text(reason)),
    })
}

/// The arguments of `talk ask` to a human.
#[derive(Clone)]
pub(crate) struct Ask {
    pub(crate) to: Option<&'static str>,
    pub(crate) options: Vec<&'static str>,
    pub(crate) default: Option<&'static str>,
    pub(crate) due: Option<When>,
    pub(crate) digest: u8,
    pub(crate) again: bool,
}

impl Default for Ask {
    fn default() -> Ask {
        Ask {
            to: None,
            options: vec!["yes", "no"],
            default: None,
            due: None,
            digest: 1,
            again: false,
        }
    }
}

/// `talk ask {at, to: user | principal:<k>, ..}`.
pub(crate) fn ask_cmd(at: NodeId, a: &Ask) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: NodeRef::Id(at),
        text: text("which?"),
        to: AskTo::User(UserAsk {
            to: a.to.map(pk),
            options: a.options.iter().map(|o| text(o)).collect(),
            recommendation: a.options.first().map(|o| text(o)),
            default: a.default.map(text),
            due: a.due,
            digest: ReqDigest([a.digest; 32]),
        }),
        again: a.again,
    })
}

/// The question and `existing` of an `ask` reply.
pub(crate) fn asked(d: &Done) -> (QuestionId, bool) {
    match &d.reply {
        Reply::Asked {
            question, existing, ..
        } => (*question, *existing),
        other => panic!("not an ask reply: {other:?}"),
    }
}

/// A decision by `s` on `n` listing `settles`, by its event alone (`talk decide` is WP-D5b's).
pub(crate) fn decision(w: &mut World, s: SessionId, n: NodeId, settles: &[Eid]) -> Eid {
    crate::apply::tests::entry(
        w,
        n,
        crate::state::Author::Session(s),
        EntryBody::Decision {
            settles: settles.iter().copied().collect(),
            supersedes: None,
            assumes: BTreeSet::new(),
            evidence: None,
            basis: Vec::new(),
        },
    )
}

/// `node key {add}`.
pub(crate) fn key_add(n: NodeId, k: &str) -> Cmd {
    Cmd::Node(NodeCmd::Key {
        at: NodeRef::Id(n),
        change: KeyChange::Add(key(k)),
    })
}

/// A request from outside the swarm on `n`, from the message `origin` of its own conversation, by
/// its event alone (`talk request` is WP-D5b's).
pub(crate) fn ext_request(w: &mut World, s: SessionId, n: NodeId, origin: &str) -> Eid {
    crate::apply::tests::entry(
        w,
        n,
        crate::state::Author::Session(s),
        EntryBody::Request {
            origin: key(origin),
            conv: KeyStr::new(origin).unwrap(),
            from_user: None,
            requester: Requester::External,
        },
    )
}

/// A people question on `n` in `conv`, with its `said`, by events alone (WP-D5b's command).
pub(crate) fn people_question(
    w: &mut World,
    s: SessionId,
    n: NodeId,
    conv: &str,
    blocking: bool,
) -> Eid {
    let by = crate::state::Author::Session(s);
    let a = crate::apply::tests::entry(
        w,
        n,
        by,
        EntryBody::Said {
            conv: KeyStr::new(conv).unwrap(),
            kind: SaidKind::Question,
            corrects: None,
            voice: false,
        },
    );
    let at = w.clock.plus(HOUR);
    crate::apply::tests::entry(
        w,
        n,
        by,
        EntryBody::Question(QuestionBody::People {
            conv: KeyStr::new(conv).unwrap(),
            blocking,
            said: a,
            supersedes: None,
            recheck_at: at,
        }),
    )
}

/// The token a reply issued.
pub(crate) fn token_of(d: &Done) -> Token {
    match &d.reply {
        Reply::Created { token: Some(t), .. } | Reply::Taken { token: t, .. } => *t,
        other => panic!("no hold: {other:?}"),
    }
}

/// The events of the last command.
pub(crate) fn evs(w: &World) -> Vec<Ev> {
    w.last_events.iter().map(|e| e.ev.clone()).collect()
}

/// The digest entries for `item`, by principal.
pub(crate) fn digest_to(w: &World, item: DigestItem) -> Vec<PrincipalId> {
    w.s.digest
        .values()
        .filter(|d| d.item == item)
        .map(|d| d.to)
        .collect()
}

/// A world with a worker session and the directory: alice (`mail:alice@x`), bob (`mail:bob@x` and
/// `chat:bob`), carol (`mail:carol@x`), and dave (`mail:dave@x`), each a person node.
pub(crate) struct Dir {
    pub(crate) w: World,
    pub(crate) c: ClassName,
    pub(crate) s: SessionId,
    pub(crate) alice: NodeId,
    pub(crate) bob: NodeId,
    pub(crate) carol: NodeId,
    pub(crate) dave: NodeId,
}

pub(crate) fn dir() -> Dir {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let alice = person(&mut w, &c, s, "alice", &["mail:alice@x"]);
    let bob = person(&mut w, &c, s, "bob", &["mail:bob@x", "chat:bob"]);
    let carol = person(&mut w, &c, s, "carol", &["mail:carol@x"]);
    let dave = person(&mut w, &c, s, "dave", &["mail:dave@x"]);
    Dir {
        w,
        c,
        s,
        alice,
        bob,
        carol,
        dave,
    }
}

// ------------------------------------------------------------------------------------------------
// chain, requesters, owners (D.8.12, U1, CS-2, D-3)

/// `chain` equals `spec::chain`, and keeps U1's shape.
fn same_chain(
    w: &World,
    about: Option<NodeId>,
    explicit: Option<PrincipalId>,
    refusal: bool,
) -> Vec<PrincipalId> {
    let got = chain(&w.s, about, explicit, refusal);
    assert_eq!(got, spec::chain(&w.s, about, explicit, refusal));
    assert!(!got.is_empty() && got.len() <= crate::user::UQ_CHAIN_MAX);
    assert_eq!(got.last(), Some(&USER_PRINCIPAL));
    for (i, &p) in got.iter().enumerate() {
        for &q in got.iter().skip(i + 1) {
            assert!(
                !same_person(&w.s, p, q),
                "two stages of one person: {got:?}"
            );
        }
    }
    got
}

/// With no directory and no user request in reach, the chain is `[user]`.
#[test]
fn a_chain_without_a_directory_is_user_alone() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let n = mk(&mut w, &c, s, "/a", |_| {});
    assert_eq!(same_chain(&w, Some(n), None, false), vec![USER_PRINCIPAL]);
    assert_eq!(same_chain(&w, None, None, true), vec![USER_PRINCIPAL]);
}

/// The explicit addressee, the requesters of the nearest node that has any (oldest first, not an
/// intake node above it), the owners of the nearest owned node and then ROOT's, and `user`; two
/// aliases of one person make one stage, and an alias of `user` takes no stage before the last.
#[test]
fn a_chain_names_the_explicit_addressee_the_requesters_the_owners_and_user() {
    let Dir {
        mut w,
        c,
        s,
        alice,
        bob,
        carol,
        dave,
    } = dir();
    let intake = mk(&mut w, &c, s, "/intake", |_| {});
    let x = mk(&mut w, &c, s, "/intake/x", |_| {});
    let y = mk(&mut w, &c, s, "/intake/x/y", |_| {});
    // Dave's request on the intake node is not reached from below x, which has its own.
    user_request(&mut w, &c, s, intake, "mail:dave@x");
    user_request(&mut w, &c, s, x, "chat:bob");
    user_request(&mut w, &c, s, x, "mail:carol@x");
    user_request(&mut w, &c, s, x, "mail:bob@x"); // bob again, under another alias
    let (bob_chat, carol_p, alice_p, dave_p) = (
        pid(&w, "chat:bob"),
        pid(&w, "mail:carol@x"),
        pid(&w, "mail:alice@x"),
        pid(&w, "mail:dave@x"),
    );
    assert_eq!(requesters(&w.s, Some(y)), vec![bob_chat, carol_p]);
    assert_eq!(requesters(&w.s, Some(intake)), vec![dave_p]);
    // Owners: x's own (alice), then ROOT's (dave).
    set_owners(&mut w, &c, s, x, &[alice]);
    set_owners(&mut w, &c, s, ROOT, &[dave]);
    assert_eq!(owners(&w.s, Some(y)), vec![alice_p, dave_p]);
    assert_eq!(owners(&w.s, None), vec![dave_p]);
    let got = same_chain(&w, Some(y), Some(alice_p), false);
    assert_eq!(
        got,
        vec![alice_p, bob_chat, carol_p, dave_p, USER_PRINCIPAL]
    );
    // An explicit alias of bob takes bob's place: one stage per person.
    let bob_mail = pid(&w, "mail:bob@x");
    let got = same_chain(&w, Some(y), Some(bob_mail), false);
    assert_eq!(
        got,
        vec![bob_mail, carol_p, alice_p, dave_p, USER_PRINCIPAL]
    );
    // `user` as an alias of carol: carol takes no stage before the last.
    ok(w.call(&c, s, key_add(carol, "person:user")));
    let got = same_chain(&w, Some(y), None, false);
    assert_eq!(got, vec![bob_chat, alice_p, dave_p, USER_PRINCIPAL]);
    let _ = bob;
}

/// A team stands for the persons of its own `owners`; a team with none stands for nobody, even
/// under an owned ancestor (A-116); a gone person is skipped, but an explicit addressee tagged gone
/// keeps its stage; owners and requesters stop at their caps.
#[test]
fn owners_expand_teams_skip_the_gone_and_stop_at_their_caps() {
    let Dir {
        mut w,
        c,
        s,
        alice,
        bob,
        carol,
        dave,
    } = dir();
    let pay = mk(&mut w, &c, s, "/@teams/pay", |a| {
        a.owners = vec![NodeRef::Id(bob), NodeRef::Id(carol)];
    });
    let empty = mk(&mut w, &c, s, "/@teams/empty", |_| {});
    let work = mk(&mut w, &c, s, "/work", |_| {});
    let task = mk(&mut w, &c, s, "/work/task", |_| {});
    set_owners(&mut w, &c, s, work, &[pay]);
    let (alice_p, bob_p, carol_p, dave_p) = (
        pid(&w, "mail:alice@x"),
        pid(&w, "mail:bob@x"),
        pid(&w, "mail:carol@x"),
        pid(&w, "mail:dave@x"),
    );
    assert_eq!(owners(&w.s, Some(task)), vec![bob_p, carol_p]);
    // An empty team under an owned ROOT: the team gives nobody, ROOT's owners follow.
    set_owners(&mut w, &c, s, ROOT, &[alice]);
    set_owners(&mut w, &c, s, work, &[empty]);
    assert_eq!(owners(&w.s, Some(task)), vec![alice_p]);
    assert_eq!(
        same_chain(&w, Some(task), None, false),
        vec![alice_p, USER_PRINCIPAL]
    );
    // More owners than UQ_OWNERS_MAX: the first three, the owners in id order (dave's node is
    // older than the team's), a team's members in theirs.
    set_owners(&mut w, &c, s, work, &[pay, dave]);
    assert!(dave < pay);
    assert_eq!(owners(&w.s, Some(task)), vec![dave_p, bob_p, carol_p]);
    // A gone person is skipped; as an explicit addressee it keeps its stage.
    mark_gone(&mut w, &c, s, carol);
    assert_eq!(owners(&w.s, Some(task)), vec![dave_p, bob_p, alice_p]);
    assert_eq!(
        same_chain(&w, Some(task), Some(carol_p), false),
        vec![carol_p, dave_p, bob_p, alice_p, USER_PRINCIPAL]
    );
    // More requesters than UQ_REQUESTERS_MAX: the three oldest, gone ones skipped.
    for k in ["mail:r1@x", "mail:r2@x", "mail:r3@x", "mail:r4@x"] {
        user_request(&mut w, &c, s, task, k);
    }
    let reqs = requesters(&w.s, Some(task));
    assert_eq!(
        reqs,
        vec![
            pid(&w, "mail:r1@x"),
            pid(&w, "mail:r2@x"),
            pid(&w, "mail:r3@x")
        ]
    );
    let got = same_chain(&w, Some(task), Some(carol_p), false);
    assert_eq!(got.len(), crate::user::UQ_CHAIN_MAX);
}

/// A refusal question's requesters are those of its own node only: a refused sub-task without
/// requests under a parent with bob's request goes to the owners, not to bob, and one with its own
/// requests goes to their requesters (§8.4, §10.3).
#[test]
fn a_refusal_chain_reads_the_refused_node_alone() {
    let Dir {
        mut w, c, s, alice, ..
    } = dir();
    let parent = mk(&mut w, &c, s, "/p", |_| {});
    let sub = mk(&mut w, &c, s, "/p/sub", |_| {});
    user_request(&mut w, &c, s, parent, "mail:bob@x");
    set_owners(&mut w, &c, s, ROOT, &[alice]);
    let alice_p = pid(&w, "mail:alice@x");
    let bob_p = pid(&w, "mail:bob@x");
    assert_eq!(
        same_chain(&w, Some(sub), None, true),
        vec![alice_p, USER_PRINCIPAL]
    );
    assert_eq!(
        same_chain(&w, Some(sub), None, false),
        vec![bob_p, alice_p, USER_PRINCIPAL]
    );
    user_request(&mut w, &c, s, sub, "mail:carol@x");
    let carol_p = pid(&w, "mail:carol@x");
    assert_eq!(
        same_chain(&w, Some(sub), None, true),
        vec![carol_p, alice_p, USER_PRINCIPAL]
    );
}

/// A gone requester takes no stage in a chain, and the next requester takes its place, but the
/// gone requester stays in the audience of what the swarm decides about its request (D.8.13).
#[test]
fn a_gone_requester_takes_no_stage_but_stays_in_the_audience() {
    let Dir {
        mut w, c, s, carol, ..
    } = dir();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    user_request(&mut w, &c, s, n, "mail:carol@x");
    user_request(&mut w, &c, s, n, "mail:dave@x");
    let (carol_p, dave_p) = (pid(&w, "mail:carol@x"), pid(&w, "mail:dave@x"));
    assert_eq!(requesters(&w.s, Some(n)), vec![carol_p, dave_p]);
    mark_gone(&mut w, &c, s, carol);
    assert_eq!(requesters(&w.s, Some(n)), vec![dave_p]);
    assert_eq!(requesters_on(&w.s, n, false), vec![carol_p, dave_p]);
    assert_eq!(
        same_chain(&w, Some(n), None, false),
        vec![dave_p, USER_PRINCIPAL]
    );
    // The chain of a question asked now is checked against `spec::chain` (U1's step check).
    let q = asked(&ok(w.call(&c, s, ask_cmd(n, &Ask::default())))).0;
    assert_eq!(w.s.uquestions[&q].to, vec![dave_p, USER_PRINCIPAL]);
    let item = DigestItem::Lapsed { question: q };
    assert_eq!(audience(&w.s, &item), [carol_p, dave_p].into());
}

// ------------------------------------------------------------------------------------------------
// absent, p90_gap, due_for (D.8.12, A-46, CS-2)

/// Records a message of `principal` at the clock (a `from_user` with no target).
fn message(w: &mut World, c: &ClassName, s: SessionId, principal: &str) {
    ok(w.call(c, s, from_user_cmd("hi", Some(principal), None, None, None)));
}

#[test]
fn the_p90_rank_lies_within_the_gaps() {
    for len in 1..=2_000_usize {
        let r = p90_rank(len);
        assert!(r >= 1 && r <= len, "{len}: {r}");
        // ⌈0.9 × len⌉
        assert_eq!(r, (9 * len).div_ceil(10));
    }
    assert_eq!(p90_rank(0), 0);
    assert_eq!(p90_of(&mut []), Dur(0));
    assert_eq!(p90_of(&mut [Dur(5)]), Dur(5));
    let mut gaps: Vec<Dur> = (1..=10).rev().map(Dur).collect();
    assert_eq!(p90_of(&mut gaps), Dur(9));
}

/// `absent` and `p90_gap` per person, at the window's edge: no message, one message, a burst, a
/// person who stops writing, a message under another alias, and a `gone` mark.
#[test]
fn absent_and_p90_are_per_person_and_merge_the_aliases() {
    let Dir {
        mut w, c, s, bob, ..
    } = dir();
    let window = w.s.params.user_gap_window;
    let (bob_mail, bob_chat) = (pid(&w, "mail:bob@x"), pid(&w, "chat:bob"));
    let now = w.clock;
    assert!(absent(&w.s, bob_mail, now));
    assert_eq!(p90_gap(&w.s, bob_mail, now), Dur(0));
    message(&mut w, &c, s, "mail:bob@x");
    let first = w.clock;
    assert!(!absent(&w.s, bob_mail, first));
    assert!(!absent(&w.s, bob_chat, first), "an alias wrote");
    assert_eq!(
        p90_gap(&w.s, bob_mail, first),
        Dur(0),
        "one message, no gap"
    );
    // A burst under both aliases: gaps of 1, 2, ..., 10 minutes, merged across the aliases.
    for i in 1..=10_u64 {
        w.advance(Dur(i * MINUTE.0));
        let k = if i % 2 == 0 { "mail:bob@x" } else { "chat:bob" };
        message(&mut w, &c, s, k);
    }
    let last = w.clock;
    assert_eq!(p90_gap(&w.s, bob_chat, last), Dur(9 * MINUTE.0));
    assert_eq!(p90_gap(&w.s, bob_mail, last), p90_gap(&w.s, bob_chat, last));
    // At the window's edge: the last message is still within it, one millisecond later it is not.
    let edge = last.plus(window);
    assert!(!absent(&w.s, bob_mail, edge));
    assert!(absent(&w.s, bob_mail, edge.plus(Dur(1))));
    assert_eq!(p90_gap(&w.s, bob_mail, edge.plus(Dur(1))), Dur(0));
    // Only the messages within the window count towards the p90.
    let mid = first.plus(window).plus(Dur(3 * MINUTE.0));
    let msgs: Vec<Time> = w.s.user_msgs[&bob_mail]
        .iter()
        .chain(&w.s.user_msgs[&bob_chat])
        .copied()
        .filter(|t| *t >= mid.minus(window))
        .collect();
    assert!(msgs.len() < 11);
    assert!(p90_gap(&w.s, bob_mail, mid) <= Dur(10 * MINUTE.0));
    // A gone person is absent, whatever they wrote.
    mark_gone(&mut w, &c, s, bob);
    assert!(absent(&w.s, bob_mail, w.clock));
}

/// Only stage 0 waits for the person's p90; an absent addressee gets `due_absent`, except on a
/// refusal question, which keeps the long due.
#[test]
fn only_stage_zero_waits_for_the_p90() {
    let Dir { mut w, c, s, .. } = dir();
    let p = w.s.params.clone();
    let bob = pid(&w, "mail:bob@x");
    let agent = UqSource::Agent {
        session: s,
        digest: ReqDigest([0; 32]),
    };
    let refusal = UqSource::Refusal { node: ROOT };
    let now = w.clock;
    assert_eq!(
        due_for(&w.s, bob, 0, &agent, now),
        now.plus(p.user_question_due_absent)
    );
    assert_eq!(
        due_for(&w.s, bob, 0, &refusal, now),
        now.plus(p.user_question_due)
    );
    assert_eq!(
        due_for(&w.s, bob, 2, &refusal, now),
        now.plus(p.user_question_due)
    );
    // A writer whose gaps are 100 hours: stage 0 waits for them, stage 1 does not.
    message(&mut w, &c, s, "mail:bob@x");
    w.advance(Dur(100 * HOUR.0));
    // The worker retired meanwhile.
    let s = w.hello(&c);
    message(&mut w, &c, s, "mail:bob@x");
    let now = w.clock;
    assert_eq!(p90_gap(&w.s, bob, now), Dur(100 * HOUR.0));
    assert_eq!(
        due_for(&w.s, bob, 0, &agent, now),
        now.plus(Dur(100 * HOUR.0))
    );
    assert_eq!(
        due_for(&w.s, bob, 1, &agent, now),
        now.plus(p.user_question_due)
    );
}

/// A person who writes every 10 days holds a question's first stage for `capacity.window` (7 days),
/// not for their p90 (1.5, O-1: the year simulation saw a monthly writer hold a task 27.8 days);
/// one whose p90 lies between `user_question.due` and `capacity.window` keeps it. An `ask` stores
/// the capped due, which `check_step` recomputes (`invariants::step_user`), and the question moves
/// on at it.
#[test]
fn the_first_stage_waits_at_most_the_capacity_window() {
    let Dir { mut w, c, s, .. } = dir();
    let p = w.s.params.clone();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let bob = pid(&w, "mail:bob@x");
    let agent = UqSource::Agent {
        session: s,
        digest: ReqDigest([0; 32]),
    };
    let ten = Dur(10 * DAY.0);
    assert!(ten > p.capacity_window && ten.0 * 2 < p.user_gap_window.0);
    for i in 0..3 {
        if i > 0 {
            w.advance(ten);
        }
        let x = w.hello(&c);
        message(&mut w, &c, x, "mail:bob@x");
    }
    let now = w.clock;
    assert_eq!(p90_gap(&w.s, bob, now), ten);
    assert_eq!(
        due_for(&w.s, bob, 0, &agent, now),
        now.plus(p.capacity_window)
    );
    // A later stage is unchanged.
    assert_eq!(
        due_for(&w.s, bob, 1, &agent, now),
        now.plus(p.user_question_due)
    );
    // Through `ask`: the stored due is the capped one, and the question moves on at it.
    let x = w.hello(&c);
    let now = w.clock;
    let q = asked(&ok(w.call(
        &c,
        x,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:bob@x"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    assert_eq!(
        stage(&w, q),
        (0, now.plus(p.capacity_window), UqState::Pending)
    );
    w.advance(p.capacity_window);
    assert_eq!(stage(&w, q).0, 1, "the question moved on at the capped due");
    // A writer whose p90 is 5 days, between `user_question.due` and `capacity.window`, keeps it.
    let mut v = dir();
    let five = Dur(5 * DAY.0);
    assert!(five > p.user_question_due && five < p.capacity_window);
    for i in 0..3 {
        if i > 0 {
            v.w.advance(five);
        }
        let x = v.w.hello(&v.c);
        message(&mut v.w, &v.c, x, "mail:bob@x");
    }
    let now = v.w.clock;
    let bob = pid(&v.w, "mail:bob@x");
    assert_eq!(due_for(&v.w.s, bob, 0, &agent, now), now.plus(five));
}

/// The p90 counts a message exactly `user_gap.window` old, and not one a millisecond older
/// (D.8.12: the times at or after `now − user_gap.window`).
#[test]
fn the_p90_window_includes_its_edge() {
    let Dir { mut w, c, .. } = dir();
    let window = w.s.params.user_gap_window;
    let bob = pid(&w, "mail:bob@x");
    let x = w.hello(&c);
    message(&mut w, &c, x, "mail:bob@x");
    let first = w.clock;
    w.advance(Dur(5 * HOUR.0));
    let x = w.hello(&c);
    message(&mut w, &c, x, "mail:bob@x");
    w.advance(HOUR);
    let x = w.hello(&c);
    message(&mut w, &c, x, "mail:bob@x");
    // Gaps of 5 h and 1 h: the nearest rank of two is the second, 5 h.
    let edge = first.plus(window);
    assert_eq!(p90_gap(&w.s, bob, edge), Dur(5 * HOUR.0));
    // Without the first message, one gap of 1 h is left.
    assert_eq!(p90_gap(&w.s, bob, edge.plus(Dur(1))), HOUR);
}

// ------------------------------------------------------------------------------------------------
// The UserQuestion firing: re-addressing, lapse, default (D.8.2, D.8.12, CS-2)

/// Only the first stage waits for its addressee's p90 (D.8.12): bob, who writes every 5 days, owns
/// the node; when the question reaches him at stage 1, after carol's absent stage, his due is
/// `user_question.due` from then, not his p90. `check_fire` recomputes the due (D.8.2's table).
#[test]
fn a_later_stage_ignores_its_addressees_p90() {
    let Dir {
        mut w, c, s, bob, ..
    } = dir();
    let p = w.s.params.clone();
    let n = mk(&mut w, &c, s, "/n", |a| a.owners = vec![NodeRef::Id(bob)]);
    let five = Dur(5 * DAY.0);
    for i in 0..3 {
        if i > 0 {
            w.advance(five);
        }
        let x = w.hello(&c);
        message(&mut w, &c, x, "mail:bob@x");
    }
    let bob_p = pid(&w, "mail:bob@x");
    assert_eq!(p90_gap(&w.s, bob_p, w.clock), five);
    assert!(five > p.user_question_due);
    let x = w.hello(&c);
    let q = asked(&ok(w.call(
        &c,
        x,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:carol@x"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    let to = w.s.uquestions[&q].to.clone();
    assert_eq!(to.len(), 3);
    assert_eq!(to[0], pid(&w, "mail:carol@x"));
    assert!(same_person(&w.s, to[1], bob_p));
    // Carol never wrote: her stage is `due_absent`.
    w.advance(p.user_question_due_absent);
    let at1 = w.clock;
    assert_eq!(
        stage(&w, q),
        (1, at1.plus(p.user_question_due), UqState::Pending)
    );
    // At stage 0, bob would have waited for his p90.
    let source = w.s.uquestions[&q].source;
    assert_eq!(due_for(&w.s, to[1], 0, &source, at1), at1.plus(five));
}

/// The pending question's current stage and due.
fn stage(w: &World, q: QuestionId) -> (u8, Time, UqState) {
    let u = &w.s.uquestions[&q];
    (u.stage, u.due, u.state)
}

/// A question without a default is re-addressed through every stage, each with its addressee's
/// own due, and past the last one it lapses: its waiters are marked, the `Lapsed` entries go to
/// the persons it was put to, and a later answer stays unrouted (A-37).
#[test]
fn a_question_moves_along_its_chain_and_lapses_onto_its_waiters() {
    let Dir {
        mut w,
        c,
        s,
        alice,
        bob,
        ..
    } = dir();
    let p = w.s.params.clone();
    set_owners(&mut w, &c, s, ROOT, &[alice, bob]);
    let (alice_p, bob_p) = (pid(&w, "mail:alice@x"), pid(&w, "mail:bob@x"));
    // Alice writes, so her stage has the long due; bob never does, so his has `due_absent`.
    message(&mut w, &c, s, "mail:alice@x");
    let (n, t) = created(&ok(w.call(&c, s, create_with("/w", |_| {}))));
    let q = asked(&ok(w.call(&c, s, ask_cmd(n, &Ask::default())))).0;
    assert_eq!(w.s.uquestions[&q].to, vec![alice_p, bob_p, USER_PRINCIPAL]);
    let start = w.clock;
    assert_eq!(
        stage(&w, q),
        (0, start.plus(p.user_question_due), UqState::Pending)
    );
    // The holder waits on the question.
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Release {
            token: t.unwrap(),
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
    assert!(w.s.nodes[&n].wait.is_some());
    let evs = w.advance(p.user_question_due);
    assert!(
        evs.iter()
            .any(|e| matches!(e.ev, Ev::UqReaddressed { q: x, stage: 1, .. } if x == q))
    );
    let at1 = w.clock;
    assert_eq!(
        stage(&w, q),
        (1, at1.plus(p.user_question_due_absent), UqState::Pending)
    );
    assert!(w.s.ix.pending_uq_by[&bob_p].iter().any(|r| r.q == q));
    w.advance(p.user_question_due_absent);
    let at2 = w.clock;
    // `user` writes nothing in this world: absent.
    assert_eq!(
        stage(&w, q),
        (2, at2.plus(p.user_question_due_absent), UqState::Pending)
    );
    w.advance(p.user_question_due_absent);
    assert_eq!(w.s.uquestions[&q].state, UqState::Lapsed);
    // The waiter is marked to proceed on an assumption; nothing else is.
    assert!(w.s.nodes[&n].assume_marks.contains(&q));
    assert!(
        w.s.nodes[&n].wait.is_some(),
        "the wait resolves by predicate"
    );
    // The persons asked (alice, bob, not `user`) get the lapse in their digest.
    let mut to = digest_to(&w, DigestItem::Lapsed { question: q });
    to.sort_unstable();
    assert_eq!(to, vec![alice_p, bob_p]);
    // A late answer stays unrouted (the worker retired meanwhile: a new session records it).
    let a = open_as(&mut w, &class("assistant"), Some("mail:alice@x"));
    let d = ok(w.call(
        &class("assistant"),
        a,
        from_user_cmd("yes", None, Some(q), None, None),
    ));
    assert_eq!(
        routed(&d).1,
        RoutedReply::Unrouted(crate::reply::UnroutedWhy::NotPending { question: q })
    );
}

/// A lapse with no waiter marks the open `about` node; with a closed `about` node, the server
/// records the assumption at once (`assume_now`), on that node.
#[test]
fn a_lapse_marks_the_open_about_node_or_records_the_assumption() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let p = w.s.params.clone();
    let open = mk(&mut w, &c, s, "/open", |_| {});
    let shut = mk(&mut w, &c, s, "/shut", |_| {});
    let q1 = asked(&ok(w.call(&c, s, ask_cmd(open, &Ask::default())))).0;
    let q2 = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            shut,
            &Ask {
                digest: 2,
                ..Ask::default()
            },
        ),
    )))
    .0;
    ok(w.call(&c, s, crate::apply::tests::close(shut, Outcome::Done)));
    // `[user]`, absent: one stage, due_absent.
    assert_eq!(w.s.uquestions[&q1].to, vec![USER_PRINCIPAL]);
    w.advance(p.user_question_due_absent);
    assert_eq!(w.s.uquestions[&q1].state, UqState::Lapsed);
    assert!(w.s.nodes[&open].assume_marks.contains(&q1));
    assert_eq!(w.s.uquestions[&q1].assumed_by, None);
    let u2 = &w.s.uquestions[&q2];
    assert_eq!(u2.state, UqState::Lapsed);
    let d = u2.assumed_by.expect("recorded at once");
    assert_eq!(w.s.decisions[&d].node, shut);
    assert_eq!(
        digest_to(&w, DigestItem::Lapsed { question: q2 }),
        vec![USER_PRINCIPAL],
        "nobody else was asked"
    );
}

/// A lapse never marks a reserved node: a question about `/` that nobody waits on records the
/// assumption at once, on `@user/decisions` (§8.4, D.8.12).
#[test]
fn a_lapse_about_a_reserved_node_records_the_assumption() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let q = asked(&ok(w.call(&c, s, ask_cmd(ROOT, &Ask::default())))).0;
    assert_eq!(w.s.uquestions[&q].to, vec![USER_PRINCIPAL]);
    let due = w.s.params.user_question_due_absent;
    let evs = w.advance(due);
    let u = &w.s.uquestions[&q];
    assert_eq!(u.state, UqState::Lapsed);
    let d = u.assumed_by.expect("recorded at once");
    assert_eq!(w.s.decisions[&d].node, USER_DECISIONS);
    assert!(!w.s.nodes[&ROOT].assume_marks.contains(&q));
    assert!(!evs.iter().any(|e| matches!(e.ev, Ev::MarkSet { .. })));
}

/// A lapse marks the nodes that waited on the question, and not the node it is about when that
/// one did not wait (§8.4, D.8.12).
#[test]
fn a_lapse_marks_the_waiters_and_not_the_about_node() {
    let mut w = world_with(|_| {});
    let c = class("worker");
    let s = w.hello(&c);
    let about = mk(&mut w, &c, s, "/about", |_| {});
    let q = asked(&ok(w.call(&c, s, ask_cmd(about, &Ask::default())))).0;
    let (m, t) = created(&ok(w.call(&c, s, create_with("/m", |_| {}))));
    ok(w.call(
        &c,
        s,
        Cmd::Hold(HoldCmd::Release {
            token: t.unwrap(),
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
    let due = w.s.params.user_question_due_absent;
    w.advance(due);
    assert_eq!(w.s.uquestions[&q].state, UqState::Lapsed);
    assert!(w.s.nodes[&m].assume_marks.contains(&q));
    assert!(!w.s.nodes[&about].assume_marks.contains(&q));
    assert_eq!(w.s.uquestions[&q].assumed_by, None);
}

/// With a default, the default applies at the first stage's due: a `Defaulted` entry, the
/// question `Defaulted`, and a `DefaultApplied` entry for the requesters on the nearest node.
#[test]
fn a_default_applies_at_the_due_and_tells_the_requesters() {
    let Dir { mut w, c, s, .. } = dir();
    let p = w.s.params.clone();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    user_request(&mut w, &c, s, n, "mail:carol@x");
    let carol = pid(&w, "mail:carol@x");
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                default: Some("no"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    assert_eq!(w.s.uquestions[&q].to, vec![carol, USER_PRINCIPAL]);
    // Carol wrote just now, so her stage waits the long due.
    let evs = w.advance(p.user_question_due);
    assert_eq!(w.s.uquestions[&q].state, UqState::Defaulted);
    assert!(evs.iter().any(|e| matches!(
        &e.ev,
        Ev::Appended {
            node: USER_DECISIONS,
            body: EntryBody::System(crate::event::SystemNote::Defaulted { question }),
            ..
        } if *question == q
    )));
    assert_eq!(
        digest_to(&w, DigestItem::DefaultApplied { question: q }),
        vec![carol]
    );
}

/// A default applies at the first stage, so with no requester in reach its audience is the first
/// addressee alone, not the later stages of the chain, which were never asked (D.8.13).
#[test]
fn a_default_tells_the_first_addressee_alone() {
    let Dir {
        mut w, c, s, alice, ..
    } = dir();
    let n = mk(&mut w, &c, s, "/n", |a| a.owners = vec![NodeRef::Id(alice)]);
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:carol@x"),
                default: Some("no"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    let (carol, alice_p) = (pid(&w, "mail:carol@x"), pid(&w, "mail:alice@x"));
    assert_eq!(w.s.uquestions[&q].to, vec![carol, alice_p, USER_PRINCIPAL]);
    // Carol never wrote: `due_absent`.
    let due = w.s.params.user_question_due_absent;
    w.advance(due);
    assert_eq!(w.s.uquestions[&q].state, UqState::Defaulted);
    assert_eq!(
        digest_to(&w, DigestItem::DefaultApplied { question: q }),
        vec![carol]
    );
}

// ------------------------------------------------------------------------------------------------
// The audience (D.8.13, D1, CS-5)

/// The audience of each item: the nearest node's requesters (gone ones included); else the persons
/// the question was put to, `user` excepted; else `user`. Never empty, and always `spec`'s.
#[test]
fn the_audience_falls_back_from_requesters_to_the_persons_asked_to_user() {
    let Dir {
        mut w, c, s, bob, ..
    } = dir();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let m = mk(&mut w, &c, s, "/m", |_| {});
    let (_, r) = user_request(&mut w, &c, s, n, "mail:bob@x");
    let bob_p = pid(&w, "mail:bob@x");
    let q = asked(&ok(w.call(&c, s, ask_cmd(n, &Ask::default())))).0;
    let q2 = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            m,
            &Ask {
                to: Some("mail:carol@x"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    let q3 = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            m,
            &Ask {
                digest: 9,
                ..Ask::default()
            },
        ),
    )))
    .0;
    let check = |w: &World, item: DigestItem, want: &[PrincipalId]| {
        let got = audience(&w.s, &item);
        assert_eq!(got, spec::audience(&w.s, &item));
        assert_eq!(got.into_iter().collect::<Vec<_>>(), want);
    };
    // A requester, even gone, is the audience.
    mark_gone(&mut w, &c, s, bob);
    check(&w, DigestItem::Lapsed { question: q }, &[bob_p]);
    check(&w, DigestItem::DefaultApplied { question: q }, &[bob_p]);
    check(
        &w,
        DigestItem::RefusalDrop {
            node: n,
            question: q,
        },
        &[bob_p],
    );
    // No requester: the persons asked so far (stage 0 only), never `user` while another was.
    let carol_p = pid(&w, "mail:carol@x");
    check(&w, DigestItem::Lapsed { question: q2 }, &[carol_p]);
    check(
        &w,
        DigestItem::RefusalDrop {
            node: m,
            question: q2,
        },
        &[carol_p],
    );
    // Nobody else: `user`.
    check(&w, DigestItem::Lapsed { question: q3 }, &[USER_PRINCIPAL]);
    // Requests from outside the swarm only: the persons asked, as with none; a gone explicit
    // addressee stays in the audience.
    ext_request(&mut w, s, m, "mail:m1");
    let carol = w.s.ix.person_of[&carol_p];
    mark_gone(&mut w, &c, s, carol);
    check(
        &w,
        DigestItem::RefusalDrop {
            node: m,
            question: q2,
        },
        &[carol_p],
    );
    check(&w, DigestItem::Lapsed { question: q2 }, &[carol_p]);
    let _ = r;
}

/// A chain is fixed at creation (§8.4): when two of its stages become aliases of one person later (a
/// `person:` key added, D-3), the persons asked are still one principal each, the first in chain
/// order, in the core's audience and in `spec`'s, so the lapse tells that person once and D1's
/// `check_fire` accepts it (D.8.13).
#[test]
fn stages_that_become_aliases_after_creation_are_told_once() {
    let Dir {
        mut w, c, s, alice, ..
    } = dir();
    let n = mk(&mut w, &c, s, "/n", |a| a.owners = vec![NodeRef::Id(alice)]);
    let q = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("chat:alice"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    let (chat, mail) = (pid(&w, "chat:alice"), pid(&w, "mail:alice@x"));
    assert_eq!(w.s.uquestions[&q].to, vec![chat, mail, USER_PRINCIPAL]);
    ok(w.call(&c, s, key_add(alice, "person:chat:alice")));
    assert!(same_person(&w.s, chat, mail));
    // Every stage is absent: three stages of `due_absent`.
    let p = w.s.params.clone();
    w.advance(p.user_question_due_absent);
    w.advance(p.user_question_due_absent);
    assert_eq!(w.s.uquestions[&q].stage, 2);
    let item = DigestItem::Lapsed { question: q };
    assert_eq!(audience(&w.s, &item), [chat].into());
    assert_eq!(spec::audience(&w.s, &item), [chat].into());
    w.advance(p.user_question_due_absent);
    assert_eq!(w.s.uquestions[&q].state, UqState::Lapsed);
    assert_eq!(digest_to(&w, item), vec![chat]);
}

/// A digest entry left unshown past DIGEST_RETAIN leaves the pending list, `hello` counts it as
/// older, and a `to_user` that names it, once the shell has restored it, shows it (D.4.12, CS-5).
#[test]
fn an_older_digest_entry_is_shown_once_restored() {
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
    let bob = pid(&w, "mail:bob@x");
    let (d, entry) =
        w.s.digest
            .iter()
            .find(|(_, x)| x.item == DigestItem::DefaultApplied { question: q })
            .map(|(d, x)| (*d, *x))
            .unwrap();
    assert_eq!(entry.to, bob);
    let meta = w.s.entries[&d];
    w.advance(crate::user::DIGEST_RETAIN);
    let a = open_as(&mut w, &class("assistant"), Some("chat:bob"));
    assert!(!w.s.digest.contains_key(&d));
    assert_eq!(w.s.digest_left[&bob], 1);
    let h = crate::query::hello(&w.s, a, w.clock);
    assert_eq!((h.user_digest.count, h.user_digest.older), (0, Some(1)));
    // The shell restores the entry the command names, then the report shows it.
    w.emit(Ev::Restored {
        items: vec![crate::state::Restored::Digest(d, entry, meta)],
    });
    assert_eq!(w.s.digest_left.get(&bob).copied().unwrap_or(0), 0);
    let r = ok(w.call(
        &class("assistant"),
        a,
        crate::helpers::user::tests::to_user_cmd(ToUserKind::Ack, None, &[d]),
    ));
    assert!(matches!(&r.reply, Reply::ToUser { shown_digest, .. } if *shown_digest == vec![d]));
    assert!(matches!(w.s.digest[&d].state, DigestState::Shown { .. }));
}

/// The audience of an assumption after a person did not answer: the requesters of the nearest
/// node, else the first addressee other than `user` that a question about the node would have.
#[test]
fn the_audience_of_an_assumption_is_the_first_stage_that_is_not_user() {
    let Dir {
        mut w, c, s, alice, ..
    } = dir();
    let n = mk(&mut w, &c, s, "/n", |a| a.keys = vec![key("chat:t/1")]);
    let q = people_question(&mut w, s, n, "chat:t/1", false);
    let item = DigestItem::Assumption { question: q };
    assert_eq!(audience(&w.s, &item), [USER_PRINCIPAL].into());
    set_owners(&mut w, &c, s, n, &[alice]);
    let alice_p = pid(&w, "mail:alice@x");
    assert_eq!(audience(&w.s, &item), [alice_p].into());
    user_request(&mut w, &c, s, n, "mail:dave@x");
    let dave_p = pid(&w, "mail:dave@x");
    assert_eq!(audience(&w.s, &item), [dave_p].into());
    assert_eq!(audience(&w.s, &item), spec::audience(&w.s, &item));
}

// ------------------------------------------------------------------------------------------------
// E1 (D.8.14, CS-7)

/// A `cannot` release of `t`.
pub(crate) fn cannot(t: Token) -> Cmd {
    crate::apply::tests::release(t, ReleaseReason::Cannot)
}

/// A world whose classes refuse at a quorum of `q`.
fn e1_world(q: u32) -> (World, ClassName) {
    (world_with(|p| p.refusal_quorum = q), class("worker"))
}

/// With quorum 2, one `cannot` backs the class off and asks nothing, a repeat by the same session
/// counts once, and a second session's makes the class refuse and asks the user once; with no
/// active class, one refusing class is enough (§10.3).
#[test]
fn e1_asks_once_the_quorum_of_sessions_refused() {
    let (mut w, c) = e1_world(2);
    let s1 = w.hello(&c);
    let s2 = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s1, create_with("/x", |_| {}))));
    ok(w.call(&c, s1, cannot(t.unwrap())));
    assert!(!w.s.ix.refusal_question.contains_key(&n));
    assert!(crate::refusals::backed_off(&w.s, n, &c, w.clock));
    let tk = token_of(&ok(w.call(&c, s1, crate::apply::tests::take(n))));
    ok(w.call(&c, s1, cannot(tk)));
    assert!(
        !w.s.ix.refusal_question.contains_key(&n),
        "one session counts once"
    );
    let tk = token_of(&ok(w.call(&c, s2, crate::apply::tests::take(n))));
    ok(w.call(&c, s2, cannot(tk)));
    let q = w.s.ix.refusal_question[&n];
    let u = &w.s.uquestions[&q];
    assert_eq!(u.source, UqSource::Refusal { node: n });
    assert!(u.has_default);
    assert_eq!(u.to, vec![USER_PRINCIPAL]);
    // The long due, though `user` is absent (CS-2).
    assert_eq!(u.due, w.clock.plus(w.s.params.user_question_due));
    // The entry carries the two options, the first recommended and the default.
    let e = w
        .log
        .iter()
        .find_map(|e| match &e.ev {
            Ev::Appended {
                eid,
                text,
                body: EntryBody::Question(QuestionBody::User { .. }),
                ..
            } if *eid == q => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    let opts: Vec<&str> = e
        .extra
        .iter()
        .filter(|(f, _)| *f == crate::event::TextField::Option)
        .map(|(_, t)| t.0.as_str())
        .collect();
    assert_eq!(opts, REFUSAL_OPTIONS.to_vec());
}

/// While a refusal question covers the node, a further refusal asks nothing more: another class's
/// `cannot` makes every class refuse it again, and E1 finds it covered (D.8.14).
#[test]
fn e1_asks_once_while_its_question_covers_the_node() {
    let (mut w, c) = e1_world(1);
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    ok(w.call(&c, s, cannot(t.unwrap())));
    let q = w.s.ix.refusal_question[&n];
    let oc = class("other");
    let o = w.hello(&oc);
    let tk = token_of(&ok(w.call(&oc, o, crate::apply::tests::take(n))));
    ok(w.call(&oc, o, cannot(tk)));
    assert!(crate::refusals::all_refused(&w.s, n, w.clock));
    let refusal: Vec<QuestionId> =
        w.s.uquestions
            .iter()
            .filter(|(_, u)| u.source == UqSource::Refusal { node: n })
            .map(|(q, _)| *q)
            .collect();
    assert_eq!(refusal, vec![q]);
    assert!(!evs(&w).iter().any(|e| matches!(
        e,
        Ev::Appended {
            body: EntryBody::Question(_),
            ..
        }
    )));
}

/// E1's default drops an open, unheld node: `RefusalsCleared`, a server decision listing its
/// unsettled requests, the node closed `dropped`, and a `RefusalDrop` entry for each requester
/// whose request it settles, read before any of them settles; the request settles once its
/// requester is told.
#[test]
fn e1_default_drops_the_node_and_tells_the_requesters() {
    let (mut w, c) = e1_world(1);
    let s = w.hello(&c);
    let a = open_as(&mut w, &class("assistant"), Some("mail:alice@x"));
    let (n, t) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    let (_, r) = user_request(&mut w, &class("assistant"), a, n, "mail:alice@x");
    let ext = ext_request(&mut w, s, n, "mail:m1");
    let alice = pid(&w, "mail:alice@x");
    ok(w.call(&c, s, cannot(t.unwrap())));
    let q = w.s.ix.refusal_question[&n];
    assert_eq!(w.s.uquestions[&q].to, vec![alice, USER_PRINCIPAL]);
    let due = w.s.uquestions[&q].due;
    w.at(due);
    let evs = w.settle();
    assert!(
        evs.iter()
            .any(|e| matches!(e.ev, Ev::RefusalsCleared { node, .. } if node == n))
    );
    assert!(w.s.refusals.keys().all(|(m, _)| *m != n));
    let node = &w.s.nodes[&n];
    assert!(matches!(&node.life, crate::state::Life::Closed(cl) if cl.outcome == Outcome::Dropped));
    let d =
        w.s.decisions
            .iter()
            .find(|(_, x)| x.node == n)
            .map(|(d, x)| (*d, x.settles.clone()))
            .unwrap();
    assert_eq!(d.1, [r, ext].into());
    assert_eq!(
        digest_to(
            &w,
            DigestItem::RefusalDrop {
                node: n,
                question: q
            }
        ),
        vec![alice]
    );
    // Telling alice settles her request; the external one waits for a `said`.
    let a2 = open_as(&mut w, &class("assistant"), Some("mail:alice@x"));
    let done = ok(w.call(
        &class("assistant"),
        a2,
        to_user_cmd(ToUserKind::Result, None, &[r]),
    ));
    assert!(matches!(&done.reply, Reply::ToUser { settled, .. } if *settled == vec![r]));
    assert!(w.s.requests[&r].settled.is_some());
    assert!(w.s.requests[&ext].settled.is_none());
}

/// The drop's audience is read before any of its requests settles (D.8.13, D.8.14): alice's
/// request, answered already by a `result`, settles with the drop's decision, and alice is told
/// all the same, beside bob, whose request waits for its answer.
#[test]
fn a_drop_tells_a_requester_whose_request_its_decision_settles() {
    let (mut w, c) = e1_world(1);
    let s = w.hello(&c);
    let (ac, bc) = (class("assistant"), class("bob-bot"));
    let a = open_as(&mut w, &ac, Some("mail:alice@x"));
    let b = open_as(&mut w, &bc, Some("mail:bob@x"));
    let (n, t) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    let (_, ra) = user_request(&mut w, &ac, a, n, "mail:alice@x");
    let (_, rb) = user_request(&mut w, &bc, b, n, "mail:bob@x");
    ok(w.call(&ac, a, to_user_cmd(ToUserKind::Result, None, &[ra])));
    assert!(w.s.requests[&ra].answered_by.is_some());
    assert!(w.s.requests[&ra].settled.is_none());
    ok(w.call(&c, s, cannot(t.unwrap())));
    let q = w.s.ix.refusal_question[&n];
    let due = w.s.uquestions[&q].due;
    w.at(due);
    w.settle();
    assert!(w.s.requests[&ra].settled.is_some());
    assert!(w.s.requests[&rb].settled.is_none());
    assert_eq!(
        digest_to(
            &w,
            DigestItem::RefusalDrop {
                node: n,
                question: q
            }
        ),
        vec![pid(&w, "mail:alice@x"), pid(&w, "mail:bob@x")]
    );
}

/// A drop tells every requester whose request it settles, past the three of a chain (§8.5,
/// D.8.13).
#[test]
fn a_drop_tells_every_requester_past_three() {
    let (mut w, c) = e1_world(1);
    let s = w.hello(&c);
    let ac = class("assistant");
    let a = w.hello(&ac);
    let (n, t) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    let keys = ["mail:a@x", "mail:b@x", "mail:c@x", "mail:d@x"];
    for k in keys {
        user_request(&mut w, &ac, a, n, k);
    }
    let ps: Vec<PrincipalId> = keys.iter().map(|k| pid(&w, k)).collect();
    ok(w.call(&c, s, cannot(t.unwrap())));
    let q = w.s.ix.refusal_question[&n];
    // The chain stops at three requesters; the drop tells all four.
    assert_eq!(w.s.uquestions[&q].to.len(), 4);
    let due = w.s.uquestions[&q].due;
    w.at(due);
    w.settle();
    let mut to = digest_to(
        &w,
        DigestItem::RefusalDrop {
            node: n,
            question: q,
        },
    );
    to.sort_unstable();
    assert_eq!(to, ps);
}

/// E1's default on a node that is held by then drops nothing: the refusals clear and a
/// `DefaultApplied` entry is added; on a refusal drop of a node whose requests are all external,
/// or that has none, the audience is the persons asked, else `user`.
#[test]
fn e1_default_on_a_held_node_drops_nothing() {
    let mut w = world_with(|p| {
        p.refusal_quorum = 1;
        p.user_question_due = Dur(2 * HOUR.0);
    });
    let c = class("worker");
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    ok(w.call(&c, s, cannot(t.unwrap())));
    let q = w.s.ix.refusal_question[&n];
    // Another class takes it.
    let o = w.hello(&class("other"));
    ok(w.call(&class("other"), o, crate::apply::tests::take(n)));
    let due = w.s.uquestions[&q].due;
    // Keep the hold alive across the due with checkpoints.
    loop {
        let tk = *w.s.ix.holds_by_session[&o].iter().next().unwrap();
        if w.clock.plus(Dur(10 * MINUTE.0)) >= due {
            break;
        }
        w.advance(Dur(10 * MINUTE.0));
        ok(w.call(&class("other"), o, crate::apply::tests::checkpoint(tk)));
    }
    w.at(due);
    w.settle();
    assert_eq!(w.s.uquestions[&q].state, UqState::Defaulted);
    assert!(w.s.nodes[&n].is_open());
    assert_eq!(
        digest_to(&w, DigestItem::DefaultApplied { question: q }),
        vec![USER_PRINCIPAL]
    );
    assert!(w.s.refusals.keys().all(|(m, _)| *m != n));
}

/// Answering E1's question clears the node's refusals (A-56), and the node is offered again.
#[test]
fn answering_the_refusal_question_clears_the_refusals() {
    let (mut w, c) = e1_world(1);
    let s = w.hello(&c);
    let (n, t) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    ok(w.call(&c, s, cannot(t.unwrap())));
    let q = w.s.ix.refusal_question[&n];
    let a = open_as(&mut w, &class("assistant"), None);
    let d = ok(w.call(
        &class("assistant"),
        a,
        from_user_cmd("keep it open", Some("user"), Some(q), None, None),
    ));
    assert_eq!(routed(&d).1, RoutedReply::Answers { question: q });
    assert!(matches!(w.s.uquestions[&q].state, UqState::Answered { .. }));
    assert!(w.s.refusals.keys().all(|(m, _)| *m != n));
    assert!(
        evs(&w)
            .iter()
            .any(|e| matches!(e, Ev::RefusalsCleared { node, .. } if *node == n))
    );
}

/// A class that never called an open `next`, or whose only caps profile misses the node's needs,
/// is not active: the other class's quorum asks (E1, CS-7); an `update {needs_add}` that leaves
/// only refusing classes active asks too.
#[test]
fn a_passive_class_does_not_keep_e1_false() {
    let (mut w, c) = e1_world(1);
    let s = w.hello(&c);
    // The other class is active: it called an open `next`, with no caps.
    let oc = class("other");
    let o = w.hello(&oc);
    ok(w.call(&oc, o, crate::apply::tests::next()));
    let (n, t) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    ok(w.call(&c, s, cannot(t.unwrap())));
    assert!(
        !w.s.ix.refusal_question.contains_key(&n),
        "the other class is active"
    );
    // A need the other class's caps lack: only the refusing class... is not active either.
    let (n2, t2) = created(&ok(w.call(&c, s, create_with("/y", |_| {}))));
    ok(w.call(&c, s, cannot(t2.unwrap())));
    assert!(!w.s.ix.refusal_question.contains_key(&n2));
    update(&mut w, &c, s, n2, |p| {
        p.needs_add = [Label::new("gpu").unwrap()].into();
    });
    assert!(w.s.ix.refusal_question.contains_key(&n2));
    let _ = n;
}

// ------------------------------------------------------------------------------------------------
// hello's user items (U1's query property, CS-3)

/// `hello` lists exactly the pending questions and digest entries addressed to the persons a
/// session serves, every alias included; a session that serves none sees none.
#[test]
fn hello_shows_the_items_of_every_alias_of_the_served_persons() {
    let Dir { mut w, c, s, .. } = dir();
    let n = mk(&mut w, &c, s, "/n", |_| {});
    let q1 = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("mail:bob@x"),
                ..Ask::default()
            },
        ),
    )))
    .0;
    let q2 = asked(&ok(w.call(
        &c,
        s,
        ask_cmd(
            n,
            &Ask {
                to: Some("chat:bob"),
                digest: 2,
                ..Ask::default()
            },
        ),
    )))
    .0;
    let a = open_as(&mut w, &class("assistant"), Some("chat:bob"));
    let h = crate::query::hello(&w.s, a, w.clock);
    assert_eq!(h.user_decisions.count, 2);
    let items: BTreeSet<QuestionId> = h.user_decisions.items.iter().copied().collect();
    assert_eq!(items, [q1, q2].into());
    let worker = crate::query::hello(&w.s, s, w.clock);
    assert_eq!(worker.user_decisions.count, 0);
    let want = spec::hello_items(&w.s, a);
    assert_eq!(want.questions, [q1, q2].into());
    // Bob's question lapses (absent: due_absent) into a digest entry for him.
    let p = w.s.params.clone();
    w.advance(p.user_question_due_absent);
    w.advance(p.user_question_due_absent);
    let h = crate::query::hello(&w.s, a, w.clock);
    assert_eq!(h.user_digest.count, 2);
    assert_eq!(h.user_digest.older, Some(0));
    for d in &h.user_digest.items {
        assert_eq!(w.s.digest[d].state, DigestState::Pending);
    }
    let want = spec::hello_items(&w.s, a);
    assert_eq!(
        h.user_digest.items.iter().copied().collect::<BTreeSet<_>>(),
        want.digest
    );
}

/// E1's question is addressed along the chain at its creation (§8.4, §26 U1): a `node update` that
/// adds a need the active class lacks and sets new owners in one call asks the new owners, since
/// E1 runs after its `NodeUpdated` (D.8.20).
#[test]
fn an_update_that_sets_owners_and_needs_asks_the_new_owners() {
    let mut w = world_with(|p| p.refusal_quorum = 1);
    let c = class("worker");
    let s = w.hello(&c);
    let alice = person(&mut w, &c, s, "alice", &["mail:alice@x"]);
    let oc = class("other");
    let o = w.hello(&oc);
    ok(w.call(&oc, o, crate::apply::tests::next()));
    let (n, t) = created(&ok(w.call(&c, s, create_with("/y", |_| {}))));
    ok(w.call(&c, s, cannot(t.unwrap())));
    assert!(!w.s.ix.refusal_question.contains_key(&n));
    update(&mut w, &c, s, n, |p| {
        p.needs_add = [Label::new("gpu").unwrap()].into();
        p.owners = Some(vec![NodeRef::Id(alice)]);
    });
    let q = w.s.ix.refusal_question[&n];
    assert_eq!(
        w.s.uquestions[&q].to,
        vec![pid(&w, "mail:alice@x"), USER_PRINCIPAL]
    );
}
