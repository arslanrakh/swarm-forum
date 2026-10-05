//! Unit tests of WP-D4's keys (design D.12.1): `node link`, `node key`, the join of
//! create-or-join, and the helpers of conversations, requests, and signals (D.8.8, D.8.9, D.8.11,
//! D.8.15, D.8.19, D.8.20). Every call runs through `World::call_ctx`, which checks `check_step`,
//! the dirty set, `decide == apply`, the fold of the events, the firings, and every state
//! invariant with R1 after the call. The helpers below are shared with `restructure_tests.rs`.

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

use crate::apply::tests::{class, create_with, created, entry, label, ok, refused, text, world};
use crate::cmd::*;
use crate::error::{Error, What, Why};
use crate::event::{EntryBody, Ev, QuestionBody};
use crate::helpers::Exec;
use crate::helpers::conversation::{
    ConvNode, answer_pq, conv_key, conv_node, record_request, route_to, try_settle,
};
use crate::helpers::restructure::open_signal;
use crate::reply::Reply;
use crate::state::{Author, PqState, Role, Routing, SignalKey, UqSource, UqState};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Helpers, shared with the restructuring tests

pub(crate) fn nf(what: What) -> Error {
    Error::not_found(what)
}

pub(crate) fn inv(why: Why) -> Error {
    Error::invalid(why)
}

pub(crate) fn key(k: &str) -> Key {
    Key {
        key: KeyStr::new(k).unwrap(),
        nv: NormVer(1),
    }
}

pub(crate) fn ks(k: &str) -> KeyStr {
    KeyStr::new(k).unwrap()
}

pub(crate) fn id(n: NodeId) -> NodeRef {
    NodeRef::Id(n)
}

pub(crate) fn key_add(n: NodeId, k: &str) -> Cmd {
    Cmd::Node(NodeCmd::Key {
        at: id(n),
        change: KeyChange::Add(key(k)),
    })
}

pub(crate) fn key_remove(n: NodeId, k: &str) -> Cmd {
    Cmd::Node(NodeCmd::Key {
        at: id(n),
        change: KeyChange::Remove(ks(k)),
    })
}

pub(crate) fn link(a: NodeId, rel: LinkRel, b: NodeId) -> Cmd {
    Cmd::Node(NodeCmd::Link {
        at: id(a),
        rel,
        to: id(b),
    })
}

/// A world with one session of class `worker`.
pub(crate) fn one() -> (World, ClassName, SessionId) {
    let mut w = world();
    let c = class("worker");
    let s = w.hello(&c);
    (w, c, s)
}

/// Creates `p` without a hold, with `keys`.
pub(crate) fn node(w: &mut World, c: &ClassName, s: SessionId, p: &str, keys: &[&str]) -> NodeId {
    let d = ok(w.call(
        c,
        s,
        create_with(p, |a| {
            a.keys = keys.iter().map(|k| key(k)).collect();
            a.take = Some(false);
        }),
    ));
    created(&d).0
}

pub(crate) fn close_cmd(n: NodeId, outcome: Outcome) -> Cmd {
    crate::apply::tests::close(n, outcome)
}

/// A `from_user` entry of `principal` recorded by `s`, by its event alone (WP-D5a's command is a
/// stub in this layer).
pub(crate) fn user_entry(w: &mut World, s: SessionId, principal: PrincipalId) -> Eid {
    entry(
        w,
        USER,
        Author::Session(s),
        EntryBody::FromUser { principal },
    )
}

/// A `said` of `kind` on `n` in conversation `conv` by `s`, by its event alone (WP-D5b's).
pub(crate) fn said(w: &mut World, s: SessionId, n: NodeId, conv: &str, kind: SaidKind) -> Eid {
    entry(
        w,
        n,
        Author::Session(s),
        EntryBody::Said {
            conv: ks(conv),
            kind,
            corrects: None,
            voice: false,
        },
    )
}

/// A request on `n` from the message `origin` of conversation `conv`, by its event alone.
pub(crate) fn request(w: &mut World, s: SessionId, n: NodeId, origin: &str, conv: &str) -> Eid {
    entry(
        w,
        n,
        Author::Session(s),
        EntryBody::Request {
            origin: key(origin),
            conv: ks(conv),
            from_user: None,
            requester: Requester::External,
        },
    )
}

/// A decision on `n` that settles `settles`, by its event alone.
pub(crate) fn decision(w: &mut World, s: SessionId, n: NodeId, settles: &[Eid]) -> Eid {
    entry(
        w,
        n,
        Author::Session(s),
        EntryBody::Decision {
            settles: settles.iter().copied().collect(),
            supersedes: None,
            assumes: BTreeSet::new(),
            evidence: None,
            basis: Vec::new(),
        },
    )
}

/// A people question on `n` in `conv`, with its `said`, by events alone; returns the question.
pub(crate) fn people_question(
    w: &mut World,
    s: SessionId,
    n: NodeId,
    conv: &str,
    blocking: bool,
) -> Eid {
    let a = said(w, s, n, conv, SaidKind::Question);
    let at = w.clock.plus(Dur(3_600_000));
    entry(
        w,
        n,
        Author::Session(s),
        EntryBody::Question(QuestionBody::People {
            conv: ks(conv),
            blocking,
            said: a,
            supersedes: None,
            recheck_at: at,
        }),
    )
}

/// The outcome `w.call` gave, with the events of the call.
pub(crate) fn evs(w: &World) -> Vec<Ev> {
    w.last_events.iter().map(|e| e.ev.clone()).collect()
}

/// The signal of a pair, if one is open.
pub(crate) fn signal(w: &World, kind: SignalKind, x: NodeId, y: NodeId) -> Option<NodeId> {
    w.s.signals
        .get(&SignalKey {
            kind,
            a: x.min(y),
            b: x.max(y),
        })
        .copied()
}

// ------------------------------------------------------------------------------------------------
// node link (D.8.20)

#[test]
fn link_refusals_in_order() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let ghost = NodeId(999);
    refused(
        &mut w,
        &c,
        s,
        link(ghost, LinkRel::Continues, a),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        link(a, LinkRel::Continues, ghost),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        link(a, LinkRel::Duplicate, a),
        inv(Why::SelfLink),
    );
}

/// A link bumps `version` once; an identical one changes nothing; `duplicate` and
/// `conflicts_with` open one signal per kind and pair; `superseded_by` resolves both (D.8.19).
#[test]
fn links_open_and_resolve_signals() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    let v = w.s.nodes[&a].version;
    let d = ok(w.call(&c, s, link(a, LinkRel::DerivedFrom, b)));
    assert_eq!(
        d.reply,
        Reply::Linked {
            existing: false,
            signal: None
        }
    );
    assert_eq!(w.s.nodes[&a].version, Version(v.0 + 1));
    let d = ok(w.call(&c, s, link(a, LinkRel::DerivedFrom, b)));
    assert_eq!(
        d.reply,
        Reply::Linked {
            existing: true,
            signal: None
        }
    );
    assert!(w.last_events.is_empty());
    // A duplicate signal: a server-made node under /sys/signals, offered, linked to both.
    let d = ok(w.call(&c, s, link(b, LinkRel::Duplicate, a)));
    let Reply::Linked {
        signal: Some(sig), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    let sn = &w.s.nodes[&sig];
    assert_eq!(sn.parent(), Some(SIGNALS));
    assert_eq!(sn.origin, crate::state::Origin::Signal);
    assert!(sn.tags.contains(&label("offer")));
    assert_eq!(sn.capacity, 1);
    assert_eq!(
        sn.links,
        [(LinkRel::Duplicate, a), (LinkRel::Duplicate, b)]
            .into_iter()
            .collect()
    );
    assert_eq!(
        sn.role,
        Role::Signal(SignalKey {
            kind: SignalKind::Duplicate,
            a,
            b
        })
    );
    assert_eq!(
        sn.title.0,
        alloc::format!("duplicate: #{} and #{}", a.0, b.0)
    );
    // The other direction finds the same signal (S1); a conflict is a signal of its own.
    let d = ok(w.call(&c, s, link(a, LinkRel::Duplicate, b)));
    assert_eq!(
        d.reply,
        Reply::Linked {
            existing: false,
            signal: Some(sig)
        }
    );
    let d = ok(w.call(&c, s, link(a, LinkRel::ConflictsWith, b)));
    let Reply::Linked {
        signal: Some(conflict),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_ne!(conflict, sig);
    assert_eq!(w.s.signals.len(), 2);
    // `superseded_by` resolves both: the server closes each signal node as done.
    let d = ok(w.call(&c, s, link(b, LinkRel::SupersededBy, a)));
    assert_eq!(
        d.reply,
        Reply::Linked {
            existing: false,
            signal: None
        }
    );
    assert!(w.s.signals.is_empty());
    for n in [sig, conflict] {
        assert!(!w.s.nodes[&n].is_open());
        assert!(matches!(
            &w.s.nodes[&n].life,
            crate::state::Life::Closed(cl) if cl.outcome == Outcome::Done
        ));
    }
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::Appended {
            by: Author::Server,
            body: EntryBody::System(crate::event::SystemNote::Closed { .. }),
            ..
        }
    )));
}

// ------------------------------------------------------------------------------------------------
// node key (D.8.9)

/// D.8.24's row: the node → `invalid(ReservedKey)` → `invalid(ReservedNode)` → `not_found(UserEntry)`
/// (an addition) or `not_found(Key)` (a removal); nothing is applied.
#[test]
fn key_refusals_in_order() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    refused(
        &mut w,
        &c,
        s,
        key_add(NodeId(999), "repeat:1:x"),
        nf(What::Node),
    );
    refused(
        &mut w,
        &c,
        s,
        key_add(PEOPLE, "repeat:1:x"),
        inv(Why::ReservedKey),
    );
    refused(
        &mut w,
        &c,
        s,
        key_remove(a, "repeat:1:x"),
        inv(Why::ReservedKey),
    );
    for r in [
        ROOT,
        USER,
        USER_DECISIONS,
        USER_DIGEST,
        SYS,
        SIGNALS,
        PEOPLE,
        TEAMS,
    ] {
        refused(
            &mut w,
            &c,
            s,
            key_add(r, "user:e999"),
            inv(Why::ReservedNode),
        );
        refused(
            &mut w,
            &c,
            s,
            key_remove(r, "chat:x/1"),
            inv(Why::ReservedNode),
        );
    }
    refused(&mut w, &c, s, key_add(a, "user:e999"), nf(What::UserEntry));
    refused(&mut w, &c, s, key_remove(a, "chat:x/1"), nf(What::Key));
}

#[test]
fn key_add_and_remove() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    let v = w.s.nodes[&a].version;
    let d = ok(w.call(&c, s, key_add(a, "chat:x/1")));
    assert_eq!(
        d.reply,
        Reply::KeyAdded {
            already: false,
            user_request: None
        }
    );
    assert_eq!(w.s.nodes[&a].version, Version(v.0 + 1));
    assert_eq!(w.s.ix.keys_open[&ks("chat:x/1")], a);
    let d = ok(w.call(&c, s, key_add(a, "chat:x/1")));
    assert_eq!(
        d.reply,
        Reply::KeyAdded {
            already: true,
            user_request: None
        }
    );
    assert!(w.last_events.is_empty());
    // Another open node's key is not added, and opens a duplicate signal (§7).
    let d = ok(w.call(&c, s, key_add(b, "chat:x/1")));
    let sig = signal(&w, SignalKind::Duplicate, a, b);
    assert!(sig.is_some());
    assert_eq!(
        d.reply,
        Reply::KeyExisting {
            existing: a,
            signal: sig
        }
    );
    assert!(!w.s.nodes[&b].keys.contains_key(&ks("chat:x/1")));
    // Once the carrier closed, the key may go on another open node (K1 counts open carriers).
    ok(w.call(&c, s, close_cmd(a, Outcome::Done)));
    let d = ok(w.call(&c, s, key_add(b, "chat:x/1")));
    assert_eq!(
        d.reply,
        Reply::KeyAdded {
            already: false,
            user_request: None
        }
    );
    assert_eq!(w.s.ix.keys_open[&ks("chat:x/1")], b);
    let d = ok(w.call(&c, s, key_remove(b, "chat:x/1")));
    assert_eq!(d.reply, Reply::KeyRemoved);
    assert!(!w.s.nodes[&b].keys.contains_key(&ks("chat:x/1")));
    assert!(!w.s.ix.keys_open.contains_key(&ks("chat:x/1")));
    assert_eq!(w.replay(), w.s);
}

/// A `person:` key interns its principal first (D.2.2); a `user:e<n>` key routes its unrouted
/// entry to the node, as a request of the entry's principal (CS-4), without adding the key twice,
/// also to a closed node that carries it already (D.14.5).
#[test]
fn key_add_interns_and_routes() {
    let (mut w, c, s) = one();
    let p = node(&mut w, &c, s, "/@people/bob", &[]);
    let d = ok(w.call(&c, s, key_add(p, "person:mail:bob@x")));
    assert_eq!(
        d.reply,
        Reply::KeyAdded {
            already: false,
            user_request: None
        }
    );
    let bob = PrincipalKey::new("mail:bob@x").unwrap();
    let pid = w.s.principal_ids[&bob];
    assert!(matches!(
        evs(&w).as_slice(),
        [Ev::PrincipalSeen { .. }, Ev::KeyAdded { .. }, ..]
            | [
                Ev::Touched { .. },
                Ev::PrincipalSeen { .. },
                Ev::KeyAdded { .. },
                ..
            ]
    ));
    assert_eq!(w.s.ix.person_of.get(&pid), Some(&p));
    // A user key routes its entry.
    let a = node(&mut w, &c, s, "/a", &[]);
    let e = user_entry(&mut w, s, pid);
    let k = alloc::format!("user:e{}", e.0);
    let d = ok(w.call(&c, s, key_add(a, &k)));
    let routed = w.s.user_entries[&e].routing;
    let Routing::Request { node: rn, request } = routed else {
        panic!("{routed:?}");
    };
    // 1.4 (R3): the reply names the request the routing recorded.
    assert_eq!(
        d.reply,
        Reply::KeyAdded {
            already: false,
            user_request: Some(request)
        }
    );
    assert_eq!(rn, a);
    let r = &w.s.requests[&request];
    assert_eq!(r.node, a);
    assert_eq!(r.from_user, Some(e));
    assert_eq!(r.requester, Requester::User(pid));
    assert_eq!(r.conv, KeyStr::of_user_entry(e));
    assert_eq!(
        evs(&w)
            .iter()
            .filter(|x| matches!(x, Ev::KeyAdded { .. }))
            .count(),
        1
    );
    // A closed node gets a user key and its routing too, with the key added once (D.14.5).
    let b = node(&mut w, &c, s, "/b", &[]);
    ok(w.call(&c, s, close_cmd(b, Outcome::Done)));
    let e2 = user_entry(&mut w, s, USER_PRINCIPAL);
    let k2 = alloc::format!("user:e{}", e2.0);
    ok(w.call(&c, s, key_add(b, &k2)));
    assert!(matches!(
        w.s.user_entries[&e2].routing,
        Routing::Request { node, .. } if node == b
    ));
    assert_eq!(w.s.anomalies, 0);
    // An entry already routed is not routed again.
    let d2 = node(&mut w, &c, s, "/d", &[]);
    ok(w.call(&c, s, key_add(d2, &k)));
    assert!(matches!(
        w.s.user_entries[&e].routing,
        Routing::Request { node, .. } if node == a
    ));
}

// ------------------------------------------------------------------------------------------------
// create-or-join (D.8.9)

/// A create whose key an open node carries joins it: the keys no open node carries are added, the
/// other fields are ignored (an empty title included), no hold is issued, and no node is created.
#[test]
fn create_joins_an_open_carrier() {
    let (mut w, c, s) = one();
    let (a, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/a", |x| x.keys = vec![key("chat:x/1")]),
    )));
    let nodes = w.s.nodes.len();
    let d = ok(w.call(
        &c,
        s,
        create_with("/b", |x| {
            x.keys = vec![key("issue:p#1"), key("chat:x/1"), key("issue:p#1")];
            x.title = text("");
        }),
    ));
    assert_eq!(
        d.reply,
        Reply::Joined {
            existing: a,
            holders: vec![s],
            open_question: None,
            added_keys: vec![ks("issue:p#1")],
            request: None,
            user_requests: Vec::new(),
        }
    );
    assert_eq!(w.s.nodes.len(), nodes);
    assert!(w.s.nodes[&a].keys.contains_key(&ks("issue:p#1")));
    assert!(!evs(&w).iter().any(|e| matches!(e, Ev::HoldIssued { .. })));
    assert!(crate::query::resolve(&w.s, &crate::apply::tests::at("/b")).is_none());
}

/// A join with two open carriers joins the smaller id and opens a duplicate signal with the other;
/// its request lies on the conversation node of its `where` (§9.1); its `person:` keys intern
/// their principals; the oldest open people question is returned.
#[test]
fn a_join_records_its_request_and_signals_other_carriers() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1"]);
    let b = node(&mut w, &c, s, "/b", &["issue:p#1"]);
    let q = people_question(&mut w, s, a, "chat:x/1", false);
    let d = ok(w.call(
        &c,
        s,
        create_with("/z", |x| {
            x.keys = vec![key("issue:p#1"), key("chat:x/1"), key("person:mail:al@x")];
            x.request = Some(RequestArg {
                text: text("please"),
                origin: Some(key("chat:x/1/7")),
                where_: None,
                requester: text("al"),
            });
        }),
    ));
    let Reply::Joined {
        existing,
        open_question,
        added_keys,
        request: Some(r),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(existing, a);
    assert_eq!(open_question, Some(q));
    assert_eq!(added_keys, vec![ks("person:mail:al@x")]);
    assert!(signal(&w, SignalKind::Duplicate, a, b).is_some());
    assert!(
        w.s.principal_ids
            .contains_key(&PrincipalKey::new("mail:al@x").unwrap())
    );
    // `where` defaults to the first conversation key among the keys, `issue:p#1`, carried by b.
    let req = &w.s.requests[&r];
    assert_eq!(req.node, b);
    assert_eq!(req.conv, ks("issue:p#1"));
    assert_eq!(req.origin.key, ks("chat:x/1/7"));
    // A repeat of the same message is the same request (Q1): none recorded.
    let d = ok(w.call(
        &c,
        s,
        create_with("/z", |x| {
            x.keys = vec![key("chat:x/1")];
            x.request = Some(RequestArg {
                text: text("please"),
                origin: Some(key("chat:x/1/7")),
                where_: Some(key("chat:x/1")),
                requester: text("al"),
            });
        }),
    ));
    assert!(matches!(d.reply, Reply::Joined { request: None, .. }));
    // A `where` no node carries is added to the carrier first (case 3).
    let d = ok(w.call(
        &c,
        s,
        create_with("/z", |x| {
            x.keys = vec![key("chat:x/1")];
            x.request = Some(RequestArg {
                text: text("more"),
                origin: Some(key("mail:m1")),
                where_: Some(key("mail:thread")),
                requester: text("al"),
            });
        }),
    ));
    let Reply::Joined {
        request: Some(r2), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(w.s.requests[&r2].node, a);
    assert!(w.s.nodes[&a].keys.contains_key(&ks("mail:thread")));
}

/// A join routes the unrouted entry of a user key it adds, to the carrier.
#[test]
fn a_join_routes_an_added_user_key() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1"]);
    let e = user_entry(&mut w, s, USER_PRINCIPAL);
    let k = alloc::format!("user:e{}", e.0);
    let d = ok(w.call(
        &c,
        s,
        create_with("/b", |x| {
            x.keys = vec![
                key("chat:x/1"),
                Key {
                    key: ks(&k),
                    nv: NormVer(0),
                },
            ]
        }),
    ));
    assert!(matches!(d.reply, Reply::Joined { existing, .. } if existing == a));
    assert!(matches!(
        w.s.user_entries[&e].routing,
        Routing::Request { node, .. } if node == a
    ));
    assert!(w.s.nodes[&a].keys.contains_key(&ks(&k)));
}

/// The `user:e<n>` key of a user entry, with the normaliser version `route_to` gives it.
fn user_key(e: Eid) -> Key {
    Key {
        key: ks(&alloc::format!("user:e{}", e.0)),
        nv: NormVer(0),
    }
}

/// A create's request whose origin is the user's message `user:e<n>`.
fn user_request(e: Eid) -> RequestArg {
    RequestArg {
        text: text("please do it"),
        origin: Some(user_key(e)),
        where_: None,
        requester: text("alice"),
    }
}

/// Review finding (D4 R1): a request whose origin is a user's message is the user's request in
/// both branches of `node create` (§9.4: "A user's message is `user:<entry>`", and "the requester
/// of a user request is its entry's principal"; C1: it has no conversation): `from_user` is the
/// entry, the requester its principal, and the unrouted entry is routed to the request's node, so
/// that a later `node key {add}` of its key names no other node.
#[test]
fn a_create_request_with_a_user_origin_is_the_entrys() {
    let (mut w, c, s) = one();
    // The join: the request lies on the carrier, found by `where`'s default `chat:x/1`.
    let carrier = node(&mut w, &c, s, "/c", &["chat:x/1"]);
    let e = user_entry(&mut w, s, USER_PRINCIPAL);
    let d = ok(w.call(
        &c,
        s,
        create_with("/new", |a| {
            a.keys = vec![key("chat:x/1")];
            a.request = Some(user_request(e));
            a.take = Some(false);
        }),
    ));
    let Reply::Joined {
        request: Some(r),
        existing,
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(existing, carrier);
    let req = &w.s.requests[&r];
    assert_eq!(req.node, carrier);
    assert_eq!(req.from_user, Some(e));
    assert_eq!(req.requester, Requester::User(USER_PRINCIPAL));
    assert_eq!(
        w.s.user_entries[&e].routing,
        Routing::Request {
            node: carrier,
            request: r
        }
    );
    // A later key of the entry on another node routes nothing: the first routing wins (§8.2).
    let other = node(&mut w, &c, s, "/other", &[]);
    let k = alloc::format!("user:e{}", e.0);
    ok(w.call(&c, s, key_add(other, &k)));
    assert_eq!(
        w.s.user_entries[&e].routing,
        Routing::Request {
            node: carrier,
            request: r
        }
    );
    assert_eq!(w.s.requests[&r].node, carrier);
    // The create: the same on the new node.
    let e2 = user_entry(&mut w, s, USER_PRINCIPAL);
    let d = ok(w.call(
        &c,
        s,
        create_with("/fresh", |a| {
            a.keys = vec![key("chat:y/1")];
            a.request = Some(user_request(e2));
            a.take = Some(false);
        }),
    ));
    let Reply::Created {
        id: n,
        request: Some(r2),
        ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    let req = &w.s.requests[&r2];
    assert_eq!(
        (req.node, req.from_user, req.requester),
        (n, Some(e2), Requester::User(USER_PRINCIPAL))
    );
    assert_eq!(
        w.s.user_entries[&e2].routing,
        Routing::Request {
            node: n,
            request: r2
        }
    );
    // An origin `user:e<n>` that names no user entry (here a node's own entry) stays a request
    // from outside: there is no entry to take a principal from.
    let not_entry = said(&mut w, s, n, "chat:y/1", SaidKind::Ack);
    let d = ok(w.call(
        &c,
        s,
        create_with("/third", |a| {
            a.keys = vec![key("chat:z/1")];
            a.request = Some(user_request(not_entry));
            a.take = Some(false);
        }),
    ));
    let Reply::Created {
        request: Some(r3), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    assert_eq!(
        (w.s.requests[&r3].from_user, w.s.requests[&r3].requester),
        (None, Requester::External)
    );
}

/// Review finding (D4 R1): when a request of the message `user:e<n>` was recorded before the entry
/// existed (Q1 keeps it), routing the entry names that request on its own node, never the node the
/// routing was asked for.
#[test]
fn a_routing_names_the_node_of_an_existing_request() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    // The request takes the next eid, and the entry the one after it.
    let m = Eid(w.s.counters.next_eid.0 + 1);
    let r = entry(
        &mut w,
        a,
        Author::Session(s),
        EntryBody::Request {
            origin: user_key(m),
            conv: user_key(m).key,
            from_user: None,
            requester: Requester::External,
        },
    );
    let e = user_entry(&mut w, s, USER_PRINCIPAL);
    assert_eq!(e, m);
    let d = ok(w.call(&c, s, key_add(b, &alloc::format!("user:e{}", e.0))));
    assert_eq!(
        d.reply,
        Reply::KeyAdded {
            already: false,
            user_request: None
        }
    );
    assert_eq!(
        w.s.user_entries[&e].routing,
        Routing::Request {
            node: a,
            request: r
        }
    );
}

/// `continues` links the newest closed carrier, the largest id, also after an older carrier is
/// reopened and closed again; `view find {key}` agrees (A-117, D.8.9).
#[test]
fn continues_names_the_carrier_with_the_largest_id() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["lean:ab"]);
    ok(w.call(&c, s, close_cmd(a, Outcome::Failed)));
    let b = node(&mut w, &c, s, "/b", &["lean:ab"]);
    ok(w.call(&c, s, close_cmd(b, Outcome::Failed)));
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Reopen {
            at: id(a),
            reason: text("again"),
        }),
    ));
    ok(w.call(&c, s, close_cmd(a, Outcome::Failed)));
    let d = ok(w.call(
        &c,
        s,
        create_with("/n", |x| {
            x.keys = vec![key("lean:ab")];
            x.take = Some(false);
        }),
    ));
    assert!(matches!(&d.reply, Reply::Created { continues, .. } if continues == &vec![b]));
    let f = crate::query::find_key(&w.s, &ks("lean:ab"));
    let (n, _) = created(&d);
    assert_eq!(f.open, Some(n));
    assert_eq!(f.closed, vec![b, a]);
}

// ------------------------------------------------------------------------------------------------
// The helpers (D.8.8, D.8.11, D.8.15, D.8.19)

/// `conv_key`: `where` if given; else the one conversation key of the nearest node that carries
/// any (object keys such as `lean:` ignored, CS-36); several there are ambiguous, none at all is
/// `NoConversationKey` (A-30).
#[test]
fn conv_key_finds_the_nearest_conversation_key() {
    let (mut w, c, s) = one();
    let p = node(&mut w, &c, s, "/p", &["lean:ab", "chat:x/1"]);
    let q = node(&mut w, &c, s, "/p/q", &["arxiv:1"]);
    let r = node(&mut w, &c, s, "/r", &["chat:x/2", "mail:m"]);
    let lone = node(&mut w, &c, s, "/lone", &[]);
    assert_eq!(conv_key(&w.s, q, None), Ok(key("chat:x/1")));
    assert_eq!(conv_key(&w.s, p, None), Ok(key("chat:x/1")));
    assert_eq!(
        conv_key(&w.s, r, None),
        Err(inv(Why::AmbiguousConversationKey))
    );
    assert_eq!(conv_key(&w.s, lone, None), Err(inv(Why::NoConversationKey)));
    assert_eq!(
        conv_key(&w.s, lone, Some(&key("mail:z"))),
        Ok(key("mail:z"))
    );
    // A closed node's keys count too (§9.1 reads keys, open or closed).
    ok(w.call(&c, s, close_cmd(p, Outcome::Done)));
    assert_eq!(conv_key(&w.s, q, None), Ok(key("chat:x/1")));
}

/// `conv_node`: the nearest carrier from `at` up, open or closed; else the open carrier; else `at`
/// with `add_key`; a reserved result is `invalid(ReservedNode)` (A-29); and it equals
/// `spec::conv_node` in every case.
#[test]
fn conv_node_has_three_cases() {
    let (mut w, c, s) = one();
    let p = node(&mut w, &c, s, "/p", &["chat:x/1"]);
    let q = node(&mut w, &c, s, "/p/q", &[]);
    let o = node(&mut w, &c, s, "/o", &["chat:x/2"]);
    let cases = [
        (q, "chat:x/1", Ok((p, false))),
        (q, "chat:x/2", Ok((o, false))),
        (q, "chat:x/3", Ok((q, true))),
        (USER, "chat:x/3", Err(inv(Why::ReservedNode))),
        (USER, "chat:x/2", Ok((o, false))),
    ];
    for (at, k, want) in cases {
        let got = conv_node(&w.s, at, &ks(k)).map(|x| (x.node, x.add_key));
        assert_eq!(got, want, "{at:?} {k}");
        if let Ok((n, _)) = got {
            assert_eq!(crate::spec::conv_node(&w.s, at, &ks(k)), n);
        }
    }
    // A closed ancestor that carries the key is still case 1.
    ok(w.call(&c, s, close_cmd(p, Outcome::Done)));
    assert_eq!(
        conv_node(&w.s, q, &ks("chat:x/1")),
        Ok(ConvNode {
            node: p,
            add_key: false
        })
    );
}

/// `record_request` keeps one request per message (Q1) and takes the requester from the entry
/// (CS-4); `try_settle` needs an answer and a decision that is not retracted, in either order, and
/// never runs twice (A-11).
#[test]
fn record_request_and_try_settle() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1"]);
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    let (r, existed) = record_request(
        &mut ex,
        a,
        key("chat:x/1/1"),
        ks("chat:x/1"),
        None,
        crate::event::EntryText::default(),
    );
    assert!(!existed);
    let (r2, existed) = record_request(
        &mut ex,
        a,
        key("chat:x/1/1"),
        ks("chat:x/1"),
        None,
        crate::event::EntryText::default(),
    );
    assert!(existed);
    assert_eq!(r2, r);
    assert!(!try_settle(&mut ex, r), "neither answer nor decision");
    let (events, _) = ex.finish();
    assert_eq!(events.len(), 1);
    assert_eq!(w.s.requests[&r].requester, Requester::External);
    // A decision first, then the answer: settles by the decision.
    let d = decision(&mut w, s, a, &[r]);
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    assert!(!try_settle(&mut ex, r), "no answer yet");
    drop(ex);
    let x = said(&mut w, s, a, "chat:x/1", SaidKind::Reply);
    w.emit(Ev::RequestAnswered { request: r, by: x });
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    assert!(try_settle(&mut ex, r));
    assert!(!try_settle(&mut ex, r), "settling is final");
    drop(ex);
    let st = w.s.requests[&r].settled.unwrap();
    assert_eq!((st.decision, st.answer), (d, x));
    // A request of a user entry has its principal as its requester.
    let e = user_entry(&mut w, s, USER_PRINCIPAL);
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    let r3 = route_to(&mut ex, e, a).unwrap();
    assert_eq!(
        route_to(&mut ex, e, a),
        None,
        "a routed entry stays routed, and a second routing records nothing"
    );
    drop(ex);
    assert_eq!(w.s.requests[&r3].requester, Requester::User(USER_PRINCIPAL));
}

/// `answer_pq`: `PqAnswered` and the question's marks off; an escalated question's pending user
/// question is answered (A-60); a lapsed one's assume marks go (CS-2); an answered question is left
/// alone.
#[test]
fn answer_pq_answers_and_clears() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1"]);
    let b = node(&mut w, &c, s, "/b", &[]);
    // An open question with a recheck mark on b.
    let q = people_question(&mut w, s, a, "chat:x/1", false);
    w.emit(Ev::MarkSet {
        node: b,
        mark: Mark::Recheck,
        question: q,
        on: true,
    });
    let x = said(&mut w, s, a, "chat:x/1", SaidKind::Ack);
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    answer_pq(&mut ex, q, x);
    let n_events = ex.events().len();
    answer_pq(&mut ex, q, x);
    assert_eq!(
        ex.events().len(),
        n_events,
        "an answered question is left alone"
    );
    drop(ex);
    assert_eq!(w.s.pquestions[&q].state, PqState::Answered { by: x });
    assert!(w.s.nodes[&b].recheck_marks.is_empty());
    // An escalated blocking question: its pending user question is answered.
    let q2 = people_question(&mut w, s, a, "chat:x/1", true);
    let due = w.clock.plus(Dur(3_600_000));
    let uq = entry(
        &mut w,
        USER_DECISIONS,
        Author::Server,
        EntryBody::Question(QuestionBody::User {
            source: UqSource::Escalation { question: q2 },
            about: Some(a),
            to: vec![USER_PRINCIPAL],
            due,
            has_default: false,
        }),
    );
    w.emit(Ev::PqEscalated { q: q2, uq });
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    answer_pq(&mut ex, q2, x);
    drop(ex);
    assert_eq!(w.s.uquestions[&uq].state, UqState::Answered { by: x });
    // A lapsed user question: the assume marks of its lapse go.
    let q3 = people_question(&mut w, s, a, "chat:x/1", true);
    let due = w.clock.plus(Dur(3_600_000));
    let uq3 = entry(
        &mut w,
        USER_DECISIONS,
        Author::Server,
        EntryBody::Question(QuestionBody::User {
            source: UqSource::Escalation { question: q3 },
            about: Some(a),
            to: vec![USER_PRINCIPAL],
            due,
            has_default: false,
        }),
    );
    w.emit(Ev::PqEscalated { q: q3, uq: uq3 });
    w.emit(Ev::UqLapsed { q: uq3 });
    w.emit(Ev::MarkSet {
        node: b,
        mark: Mark::Assume,
        question: uq3,
        on: true,
    });
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    answer_pq(&mut ex, q3, x);
    drop(ex);
    assert!(w.s.nodes[&b].assume_marks.is_empty());
    assert_eq!(w.s.uquestions[&uq3].state, UqState::Lapsed);
    assert_eq!(
        crate::invariants::r1_differences(&w.s, w.clock),
        Vec::<&str>::new()
    );
}

/// `open_signal`: none for a node and itself; one per kind and unordered pair (S1).
#[test]
fn open_signal_is_one_per_kind_and_pair() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    let now = w.clock;
    let mut ex = Exec::new(&mut w.s, now, Some(s));
    assert_eq!(open_signal(&mut ex, SignalKind::Duplicate, a, a), None);
    let x = open_signal(&mut ex, SignalKind::Duplicate, a, b);
    assert!(x.is_some());
    assert_eq!(open_signal(&mut ex, SignalKind::Duplicate, b, a), x);
    let y = open_signal(&mut ex, SignalKind::Conflict, b, a);
    assert!(y.is_some() && y != x);
    drop(ex);
    assert_eq!(w.s.signals.len(), 2);
    assert_eq!(
        crate::invariants::r1_differences(&w.s, w.clock),
        Vec::<&str>::new()
    );
}

/// A name an agent took under `sys/signals` does not stop a signal: it gets a suffix (D.8.19).
#[test]
fn a_signal_name_taken_by_hand_gets_a_suffix() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    let next = NodeId(w.s.counters.next_node.0 + 1);
    let taken = alloc::format!("/sys/signals/duplicate-{}-{}-{}", a.0, b.0, next.0);
    node(&mut w, &c, s, &taken, &[]);
    let d = ok(w.call(&c, s, link(a, LinkRel::Duplicate, b)));
    let Reply::Linked {
        signal: Some(sig), ..
    } = d.reply
    else {
        panic!("{:?}", d.reply);
    };
    let name = w.s.nodes[&sig].place.as_ref().unwrap().name.clone();
    assert_eq!(
        name.as_str(),
        alloc::format!("duplicate-{}-{}-{}-1", a.0, b.0, sig.0)
    );
}

/// Item 8 of D.12.3 for `conv_node`, which is not public: on random trees built by creates with
/// keys, key changes, closes, reopens, moves, and merges, `conv_node` equals `spec::conv_node` for
/// every node and key, and refuses exactly where that node is reserved.
#[test]
fn conv_node_agrees_with_spec_on_random_trees() {
    const POOL: [&str; 4] = ["chat:x/1", "chat:x/2", "issue:p#1", "mail:m"];
    const PATHS: [&str; 6] = ["/a", "/b", "/a/c", "/b/d", "/@people/e", "/a/c/f"];
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move |n: u64| {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545_f491_4f6c_dd1d) % n
    };
    for _ in 0..24 {
        let (mut w, c, s) = one();
        for _ in 0..40 {
            let nodes: Vec<NodeId> = w.s.nodes.keys().copied().collect();
            let pick = |i: u64| nodes[usize::try_from(i).unwrap() % nodes.len()];
            let k = POOL[usize::try_from(next(4)).unwrap()];
            let p = PATHS[usize::try_from(next(6)).unwrap()];
            let cmd = match next(7) {
                0 | 1 => create_with(p, |x| {
                    x.keys = vec![key(k)];
                    x.take = Some(false);
                }),
                2 => key_add(pick(next(64)), k),
                3 => close_cmd(pick(next(64)), Outcome::Done),
                4 => Cmd::Node(NodeCmd::Reopen {
                    at: id(pick(next(64))),
                    reason: text("r"),
                }),
                5 => Cmd::Node(NodeCmd::Move {
                    at: id(pick(next(64))),
                    to: crate::apply::tests::path(p),
                }),
                _ => Cmd::Node(NodeCmd::Merge {
                    from: id(pick(next(64))),
                    into: id(pick(next(64))),
                }),
            };
            let _ = w.call(&c, s, cmd);
            for &at in w.s.nodes.keys() {
                for k in POOL.iter().chain(["chat:new"].iter()) {
                    let want = crate::spec::conv_node(&w.s, at, &ks(k));
                    let got = conv_node(&w.s, at, &ks(k));
                    if want.is_reserved() {
                        assert_eq!(got, Err(inv(Why::ReservedNode)));
                    } else {
                        // Case 3, and only it, adds the key: no node from `at` up carries it,
                        // and no open node does.
                        let carried = crate::spec::ancestors_or_self(&w.s, at)
                            .iter()
                            .any(|m| w.s.nodes[m].keys.contains_key(&ks(k)))
                            || w.s
                                .nodes
                                .values()
                                .any(|x| x.is_open() && x.keys.contains_key(&ks(k)));
                        assert_eq!(
                            got,
                            Ok(ConvNode {
                                node: want,
                                add_key: !carried
                            }),
                            "{at:?} {k}"
                        );
                    }
                }
            }
        }
    }
}
