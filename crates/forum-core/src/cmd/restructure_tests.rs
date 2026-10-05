//! Unit tests of WP-D4's restructuring (design D.12.1): `node reopen`, `move`, and `merge`
//! (D.8.20, §16), the reserved-node refusals of §21 (K8, C1), and the step checks of K7, N5, C1,
//! and L1's half for series, each of which rejects a hand-built violating step (D.9.2).

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

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::super::keys::tests::{
    close_cmd, decision, evs, id, inv, key, key_add, ks, link, nf, node, one, people_question,
    request, said, signal,
};
use crate::apply::tests::{
    at, class, create, create_with, created, entry, label, ok, refused, take, text,
};
use crate::cmd::*;
use crate::error::{Error, Hint, LimitKind, What, Why};
use crate::event::{EntryBody, Ev, Event, NodeInit, SystemNote};
use crate::invariants::{Inv, check_step};
use crate::params::Params;
use crate::reply::{Done, Reply, WarningCode, WarningDetail};
use crate::state::{
    Author, Intent, Life, OccPhase, Occurrence, Origin, Repeat, Role, SaidState, Series, State,
    WaitOn,
};
use crate::testkit::World;
use crate::types::*;

// ------------------------------------------------------------------------------------------------
// Helpers

fn reopen(n: NodeId) -> Cmd {
    Cmd::Node(NodeCmd::Reopen {
        at: id(n),
        reason: text("again"),
    })
}

fn move_to(n: NodeId, p: &str) -> Cmd {
    Cmd::Node(NodeCmd::Move {
        at: id(n),
        to: crate::apply::tests::path(p),
    })
}

pub(crate) fn merge(from: NodeId, into: NodeId) -> Cmd {
    Cmd::Node(NodeCmd::Merge {
        from: id(from),
        into: id(into),
    })
}

fn closed_with(w: &World, n: NodeId) -> Option<(Outcome, Option<NodeId>)> {
    match &w.s.nodes[&n].life {
        Life::Open => None,
        Life::Closed(c) => Some((c.outcome.clone(), c.merged_into)),
    }
}

pub(crate) fn blank_init(title: &str, role: Role) -> NodeInit {
    NodeInit {
        title: text(title),
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
        role,
    }
}

fn series_role() -> Role {
    Role::Series(Series {
        repeat: Repeat {
            cron_text: text("0 7 * * *"),
            cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
            tz: forum_cron::TzTable {
                name: String::from("UTC"),
                tzdb_version: String::new(),
                initial: 0,
                transitions: Vec::new(),
            },
            run_within: Dur(3_600_000),
        },
        last_missed: false,
    })
}

/// A series `name` under `parent`, by its events alone (WP-D6's commands are stubs in this layer),
/// with an open occurrence before its slot, ten days ahead; returns both.
pub(crate) fn series(w: &mut World, s: SessionId, parent: NodeId, name: &str) -> (NodeId, NodeId) {
    let x = w.s.counters.next_node;
    w.emit(Ev::NodeCreated {
        id: x,
        parent,
        name: Name::new(name).unwrap(),
        origin: Origin::Agent(s),
        init: blank_init(name, series_role()),
    });
    let o = occurrence(w, x, "o1");
    (x, o)
}

/// An occurrence of `x` before its slot, by its event alone.
fn occurrence(w: &mut World, x: NodeId, name: &str) -> NodeId {
    let o = w.s.counters.next_node;
    let slot = w.clock.plus(Dur(10 * DAY.0));
    let window_end = slot.plus(Dur(3_600_000));
    let mut init = blank_init(
        name,
        Role::Occurrence(Occurrence {
            series: x,
            slot,
            window_end,
            phase: OccPhase::Waiting,
            window_fired: false,
        }),
    );
    init.due = Some(window_end);
    w.emit(Ev::NodeCreated {
        id: o,
        parent: x,
        name: Name::new(name).unwrap(),
        origin: Origin::Occurrence,
        init,
    });
    o
}

/// A second session of class `worker`.
fn second(w: &mut World) -> SessionId {
    w.hello(&class("worker"))
}

// ------------------------------------------------------------------------------------------------
// node reopen (D.8.20)

/// D.8.24's row: the node → `invalid(NotClosed)` (a reserved node is always open) →
/// `invalid(OccurrenceReopen)` for an occurrence of a series still open and recurring (A-13).
#[test]
fn reopen_refusals_in_order() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    refused(&mut w, &c, s, reopen(NodeId(999)), nf(What::Node));
    refused(&mut w, &c, s, reopen(a), inv(Why::NotClosed));
    refused(&mut w, &c, s, reopen(PEOPLE), inv(Why::NotClosed));
    let (x, o) = series(&mut w, s, ROOT, "daily");
    w.emit(Ev::NodeClosed {
        node: o,
        outcome: Outcome::Dropped,
        merged_into: None,
        by: None,
    });
    occurrence(&mut w, x, "o2");
    refused(&mut w, &c, s, reopen(o), inv(Why::OccurrenceReopen));
}

/// Review finding (D4 R3): `reason` is required (§20.5), and an empty required text is `invalid`
/// with the argument named (§21): after the target, before the state checks (D.8.24).
#[test]
fn reopen_refuses_an_empty_reason() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let empty = |n: NodeId| {
        Cmd::Node(NodeCmd::Reopen {
            at: id(n),
            reason: text(""),
        })
    };
    let want = inv(Why::EmptyText(crate::error::Field::Reason));
    refused(&mut w, &c, s, empty(NodeId(999)), nf(What::Node));
    refused(&mut w, &c, s, empty(a), want.clone());
    ok(w.call(&c, s, close_cmd(a, Outcome::Done)));
    refused(&mut w, &c, s, empty(a), want);
    assert!(!w.s.nodes[&a].is_open());
    ok(w.call(&c, s, reopen(a)));
}

/// K7: a reopen keeps only the keys no other open node carries; the others are dropped with the
/// `duplicate_key` warning, a duplicate signal per carrier, and `System(KeysDropped)`, before
/// `System(Reopened)` with the reason (§16).
#[test]
fn reopen_drops_the_keys_another_open_node_carries() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1", "issue:p#1"]);
    ok(w.call(&c, s, close_cmd(a, Outcome::Done)));
    let b = node(&mut w, &c, s, "/b", &["chat:x/1"]);
    let v = w.s.nodes[&a].version;
    let d = ok(w.call(&c, s, reopen(a)));
    assert_eq!(
        d.reply,
        Reply::Reopened {
            node: a,
            dropped_keys: vec![ks("chat:x/1")],
            offered: true,
        }
    );
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(d.warnings[0].code, WarningCode::DuplicateKey);
    assert_eq!(
        d.warnings[0].detail,
        WarningDetail::DuplicateKey {
            keys: vec![(ks("chat:x/1"), b)]
        }
    );
    assert!(w.s.nodes[&a].is_open());
    assert_eq!(
        w.s.nodes[&a].keys.keys().cloned().collect::<Vec<_>>(),
        vec![ks("issue:p#1")]
    );
    // KeyRemoved and NodeReopened bump the version once each (O2).
    assert_eq!(w.s.nodes[&a].version, Version(v.0 + 2));
    assert!(signal(&w, SignalKind::Duplicate, a, b).is_some());
    let notes: Vec<SystemNote> = evs(&w)
        .into_iter()
        .filter_map(|e| match e {
            Ev::Appended {
                body: EntryBody::System(n),
                ..
            } => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(
        notes,
        vec![
            SystemNote::KeysDropped {
                keys: vec![(ks("chat:x/1"), b)]
            },
            SystemNote::Reopened
        ]
    );
}

/// A reopened merged node loses `merged_into` (§16); `take` of a flagged closed node reopens it
/// with the warning once.
#[test]
fn reopen_of_a_merged_node_and_a_take() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1"]);
    let b = node(&mut w, &c, s, "/b", &[]);
    ok(w.call(&c, s, merge(a, b)));
    assert_eq!(closed_with(&w, a), Some((Outcome::Superseded, Some(b))));
    ok(w.call(&c, s, reopen(a)));
    assert!(w.s.nodes[&a].is_open());
    assert_eq!(w.s.nodes[&a].merged_into(), None);
    // The key moved to b at the merge, so a reopens with none.
    assert!(w.s.nodes[&a].keys.is_empty());
    // A take of a closed node with a flag (an unsettled request) reopens it, dropping a key an
    // open node carries, with one `duplicate_key` warning.
    let x = node(&mut w, &c, s, "/x", &["issue:p#1"]);
    request(&mut w, s, x, "issue:p#1/1", "issue:p#1");
    ok(w.call(&c, s, close_cmd(x, Outcome::Done)));
    ok(w.call(&c, s, key_add(b, "issue:p#1")));
    let d = ok(w.call(&c, s, take(x)));
    assert!(matches!(
        d.reply,
        Reply::Taken {
            reopened: Some(Outcome::Done),
            ..
        }
    ));
    assert_eq!(
        d.warnings
            .iter()
            .filter(|w| w.code == WarningCode::DuplicateKey)
            .count(),
        1
    );
    assert!(w.s.nodes[&x].keys.is_empty());
}

/// A closed signal node reopens as the pair's open signal when the pair has none; otherwise it is
/// detached and reopens as an ordinary node (A-89, S1).
#[test]
fn reopen_of_a_signal_node() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    ok(w.call(&c, s, link(a, LinkRel::Duplicate, b)));
    let sig = signal(&w, SignalKind::Duplicate, a, b).unwrap();
    ok(w.call(&c, s, close_cmd(sig, Outcome::Done)));
    assert!(w.s.signals.is_empty());
    ok(w.call(&c, s, reopen(sig)));
    assert_eq!(signal(&w, SignalKind::Duplicate, a, b), Some(sig));
    assert!(matches!(w.s.nodes[&sig].role, Role::Signal(_)));
    // Closed again, and a new signal for the pair: a reopen now detaches it.
    ok(w.call(&c, s, close_cmd(sig, Outcome::Done)));
    ok(w.call(&c, s, link(b, LinkRel::Duplicate, a)));
    let sig2 = signal(&w, SignalKind::Duplicate, a, b).unwrap();
    assert_ne!(sig2, sig);
    ok(w.call(&c, s, reopen(sig)));
    assert_eq!(w.s.nodes[&sig].role, Role::Plain);
    assert!(w.s.nodes[&sig].is_open());
    assert_eq!(signal(&w, SignalKind::Duplicate, a, b), Some(sig2));
    let e = evs(&w);
    let det = e
        .iter()
        .position(|e| matches!(e, Ev::SignalDetached { .. }))
        .unwrap();
    assert!(matches!(e[det + 1], Ev::NodeReopened { .. }));
    assert!(!e.iter().any(|e| matches!(e, Ev::SignalOpened { .. })));
}

/// An occurrence of a closed series reopens detached, as an ordinary node (A-13, K6).
#[test]
fn reopen_detaches_an_occurrence_of_a_closed_series() {
    let (mut w, c, s) = one();
    let (x, o) = series(&mut w, s, ROOT, "daily");
    ok(w.call(&c, s, close_cmd(x, Outcome::Done)));
    assert_eq!(closed_with(&w, o).map(|x| x.0), Some(Outcome::Dropped));
    ok(w.call(&c, s, reopen(o)));
    assert_eq!(w.s.nodes[&o].role, Role::Plain);
    assert!(w.s.nodes[&o].is_open());
    let e = evs(&w);
    let det = e
        .iter()
        .position(|e| matches!(e, Ev::OccurrenceDetached { .. }))
        .unwrap();
    let re = e
        .iter()
        .position(|e| matches!(e, Ev::NodeReopened { .. }))
        .unwrap();
    assert!(det < re);
}

// ------------------------------------------------------------------------------------------------
// node move (D.8.20)

/// D.8.24's row: the node → `invalid(ReservedNode)` → `cycle` (into itself or its subtree, K4) →
/// `invalid(NameTaken)` (K3) → `limit(Depth)` → `limit(Children)`; nothing is applied.
#[test]
fn move_refusals_in_order() {
    let mut w = World::new(
        Params {
            tree_max_children: 2,
            ..Params::default()
        },
        crate::apply::tests::T0,
    );
    let c = class("worker");
    let s = w.hello(&c);
    let a = node(&mut w, &c, s, "/a", &[]);
    let ab = node(&mut w, &c, s, "/a/b", &[]);
    refused(&mut w, &c, s, move_to(NodeId(999), "/z"), nf(What::Node));
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
        refused(&mut w, &c, s, move_to(r, "/z"), inv(Why::ReservedNode));
    }
    refused(
        &mut w,
        &c,
        s,
        move_to(a, "/a/x"),
        Error::of(Hint::Cycle { path: vec![a] }),
    );
    refused(
        &mut w,
        &c,
        s,
        move_to(a, "/a/b/c/d"),
        Error::of(Hint::Cycle { path: vec![a, ab] }),
    );
    let z = node(&mut w, &c, s, "/z", &[]);
    refused(&mut w, &c, s, move_to(ab, "/z"), inv(Why::NameTaken));
    refused(&mut w, &c, s, move_to(z, "/"), inv(Why::NameTaken));
    // A node of height 0 fits at depth 16, the default `tree.max_depth`, and not at 17.
    let deep: String = (0..17).map(|i| alloc::format!("/d{i}")).collect();
    refused(
        &mut w,
        &c,
        s,
        move_to(z, &deep),
        Error::limit(LimitKind::Depth),
    );
    // ROOT has /a and /z; the child limit is 2.
    refused(
        &mut w,
        &c,
        s,
        move_to(ab, "/y"),
        Error::limit(LimitKind::Children),
    );
    refused(
        &mut w,
        &c,
        s,
        move_to(ab, "/y/new"),
        Error::limit(LimitKind::Children),
    );
    // The node's own place is a no-op success, with no event.
    let d = ok(w.call(&c, s, move_to(ab, "/a/b")));
    assert_eq!(d.reply, Reply::Moved { node: ab });
    assert!(w.last_events.is_empty());
}

/// A move makes the missing ancestors (CS-26: with their parent's needs), keeps the old name as an
/// alias that still resolves, records `System(Moved)`, and changes no counter (A-3).
#[test]
fn move_makes_ancestors_and_aliases() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/a/b", &[]);
    let v = w.s.nodes[&b].version;
    let d = ok(w.call(&c, s, move_to(b, "/c/d/e")));
    assert_eq!(d.reply, Reply::Moved { node: b });
    let cnode = crate::query::resolve(&w.s, &at("/c")).unwrap();
    let dnode = crate::query::resolve(&w.s, &at("/c/d")).unwrap();
    assert_eq!(w.s.nodes[&cnode].origin, Origin::Ancestor(s));
    assert_eq!(w.s.nodes[&dnode].parent(), Some(cnode));
    assert_eq!(w.s.nodes[&b].parent(), Some(dnode));
    assert_eq!(crate::query::resolve(&w.s, &at("/c/d/e")), Some(b));
    assert_eq!(crate::query::resolve(&w.s, &at("/a/b")), Some(b));
    assert_eq!(w.s.nodes[&b].version, v);
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::Appended {
            body: EntryBody::System(SystemNote::Moved { from }),
            ..
        } if from.parent == a
    )));
    // A create onto the alias takes the name (§4).
    let (n, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/a/b", |x| x.take = Some(false)),
    )));
    assert_ne!(n, b);
    assert_eq!(crate::query::resolve(&w.s, &at("/a/b")), Some(n));
}

/// A move ends the holds of other sessions after `NodeMoved`, so the `offer` tag is judged at the
/// new place: a node moved out of `@people` is offered, one moved into it is not (A-113, D.8.20);
/// the mover's own hold stays; `held_by_other` names the others.
#[test]
fn a_move_ends_other_holds_judged_at_the_new_place() {
    let (mut w, c, s) = one();
    let s2 = second(&mut w);
    // A work node created by mistake under @people, held by s2, moved to /work by s.
    let (p, t) = created(&ok(w.call(&c, s2, create("/@people/job"))));
    assert!(t.is_some());
    let d = ok(w.call(&c, s, move_to(p, "/work/job")));
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(
        d.warnings[0].detail,
        WarningDetail::HeldByOther {
            holders: vec![s2],
            left: vec![]
        }
    );
    let e = evs(&w);
    let moved = e
        .iter()
        .position(|e| matches!(e, Ev::NodeMoved { .. }))
        .unwrap();
    let ended = e
        .iter()
        .position(|e| matches!(e, Ev::HoldEnded { .. }))
        .unwrap();
    assert!(moved < ended);
    assert!(w.s.holds.is_empty());
    assert!(w.s.nodes[&p].tags.contains(&label("offer")));
    assert_eq!(w.s.nodes[&p].attempts, 1);
    // A held work node moved into @people gets no tag.
    let (q, _) = created(&ok(w.call(&c, s2, create("/work/q"))));
    ok(w.call(&c, s, move_to(q, "/@people/q")));
    assert!(!w.s.nodes[&q].tags.contains(&label("offer")));
    // The mover's own hold stays.
    let (m, mt) = created(&ok(w.call(&c, s, create("/work/m"))));
    let d = ok(w.call(&c, s, move_to(m, "/work/m2")));
    assert!(d.warnings.is_empty());
    assert!(w.s.holds.contains_key(&mt.unwrap()));
}

/// The child limit counts open agent-made children, never at `@people` or `@teams`, and not a
/// closed node that moves (L1, CS-10, A-113).
#[test]
fn the_child_limit_on_move_spares_people_and_teams() {
    let mut w = World::new(
        Params {
            tree_max_children: 2,
            ..Params::default()
        },
        crate::apply::tests::T0,
    );
    let c = class("worker");
    let s = w.hello(&c);
    let mut people = Vec::new();
    for p in ["/w0/p0", "/w0/p1", "/w1/p2"] {
        people.push(node(&mut w, &c, s, p, &[]));
    }
    for (i, &p) in people.iter().enumerate() {
        ok(w.call(&c, s, move_to(p, &alloc::format!("/@people/p{i}"))));
        let t = node(&mut w, &c, s, &alloc::format!("/@teams/t{i}"), &[]);
        assert_eq!(w.s.nodes[&t].parent(), Some(TEAMS));
    }
    assert_eq!(w.s.ix.node[&PEOPLE].open_agent_children, 3);
    // A person's own children still count against the person.
    let p = people[0];
    node(&mut w, &c, s, "/@people/p0/x", &[]);
    node(&mut w, &c, s, "/@people/p0/y", &[]);
    let z = node(&mut w, &c, s, "/w1/z", &[]);
    refused(
        &mut w,
        &c,
        s,
        move_to(z, "/@people/p0/z"),
        Error::limit(LimitKind::Children),
    );
    // A closed node moves into a full parent.
    ok(w.call(&c, s, close_cmd(z, Outcome::Done)));
    ok(w.call(&c, s, move_to(z, "/@people/p0/z")));
    assert_eq!(w.s.nodes[&z].parent(), Some(p));
}

/// Review finding (D4 R9): the child limit does not count the moved node against its own parent,
/// so a rename inside a full parent goes ahead; and a missing ancestor a move makes is a new open
/// child of the full parent, so a closed node moved below one is refused (L1, CS-10).
#[test]
fn the_child_limit_on_move_counts_what_the_move_adds() {
    let mut w = World::new(
        Params {
            tree_max_children: 2,
            ..Params::default()
        },
        crate::apply::tests::T0,
    );
    let c = class("worker");
    let s = w.hello(&c);
    let a = node(&mut w, &c, s, "/full/a", &[]);
    node(&mut w, &c, s, "/full/b", &[]);
    let full = w.s.nodes[&a].parent().unwrap();
    assert_eq!(w.s.ix.node[&full].open_agent_children, 2);
    ok(w.call(&c, s, move_to(a, "/full/c")));
    assert_eq!(w.s.nodes[&a].parent(), Some(full));
    let z = node(&mut w, &c, s, "/z", &[]);
    ok(w.call(&c, s, close_cmd(z, Outcome::Done)));
    refused(
        &mut w,
        &c,
        s,
        move_to(z, "/full/new/x"),
        Error::limit(LimitKind::Children),
    );
    // The closed node itself adds no open child: it moves into the full parent.
    ok(w.call(&c, s, move_to(z, "/full/z")));
}

/// L1 and series (D.14.5): a move counts a series as at least one level high, so that its
/// occurrences fit within `tree.max_depth`, a closed series and one deep in the moved subtree
/// included.
#[test]
fn a_move_counts_a_series_one_level_high() {
    let (mut w, c, s) = one();
    let max = usize::try_from(w.s.params.tree_max_depth).unwrap();
    // A parent path at depth max − 1: a node moved under it lands at depth max.
    let deep: String = (0..max - 1).map(|i| alloc::format!("/d{i}")).collect();
    node(&mut w, &c, s, &deep, &[]);
    let (x, o) = series(&mut w, s, ROOT, "daily");
    ok(w.call(&c, s, close_cmd(x, Outcome::Done)));
    // The dropped occurrence moves away, so the series has no child: only the rule counts it high.
    ok(w.call(&c, s, move_to(o, "/old/o1")));
    assert!(w.s.ix.children.get(&x).is_none_or(BTreeSet::is_empty));
    let target = alloc::format!("{deep}/daily");
    refused(
        &mut w,
        &c,
        s,
        move_to(x, &target),
        Error::limit(LimitKind::Depth),
    );
    // A plain closed node fits there.
    let plain = node(&mut w, &c, s, "/plain", &[]);
    ok(w.call(&c, s, close_cmd(plain, Outcome::Done)));
    ok(w.call(&c, s, move_to(plain, &alloc::format!("{deep}/plain"))));
    // A series two levels down in the moved subtree counts too.
    let p = node(&mut w, &c, s, "/p", &[]);
    let (y, yo) = series(&mut w, s, p, "weekly");
    ok(w.call(&c, s, close_cmd(y, Outcome::Done)));
    ok(w.call(&c, s, move_to(yo, "/old/o2")));
    let up: String = (0..max - 2).map(|i| alloc::format!("/d{i}")).collect();
    refused(
        &mut w,
        &c,
        s,
        move_to(p, &alloc::format!("{up}/p")),
        Error::limit(LimitKind::Depth),
    );
    let up3: String = (0..max - 3).map(|i| alloc::format!("/d{i}")).collect();
    ok(w.call(&c, s, move_to(p, &alloc::format!("{up3}/p"))));
}

// ------------------------------------------------------------------------------------------------
// node merge (D.8.20, N5)

/// D.8.24's row: both nodes → `invalid(SelfMerge)` → a reserved node on either side
/// (`invalid(ReservedNode)`) → `closed` for a closed or merged `into` (K5) →
/// `invalid(MergeFromClosed)` → `invalid(OccurrenceNotYetAvailable)` → `cycle` (N5, T5).
#[test]
fn merge_refusals_in_order() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let b = node(&mut w, &c, s, "/b", &[]);
    refused(&mut w, &c, s, merge(NodeId(999), b), nf(What::Node));
    refused(&mut w, &c, s, merge(a, NodeId(999)), nf(What::Node));
    refused(&mut w, &c, s, merge(a, a), inv(Why::SelfMerge));
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
        refused(&mut w, &c, s, merge(a, r), inv(Why::ReservedNode));
        refused(&mut w, &c, s, merge(r, a), inv(Why::ReservedNode));
    }
    let z = node(&mut w, &c, s, "/z", &[]);
    ok(w.call(&c, s, close_cmd(z, Outcome::Done)));
    refused(
        &mut w,
        &c,
        s,
        merge(a, z),
        Error::of(Hint::Closed {
            node: z,
            merged_into: None,
        }),
    );
    let m = node(&mut w, &c, s, "/m", &[]);
    ok(w.call(&c, s, merge(m, b)));
    refused(
        &mut w,
        &c,
        s,
        merge(a, m),
        Error::of(Hint::Closed {
            node: m,
            merged_into: Some(b),
        }),
    );
    refused(&mut w, &c, s, merge(z, a), inv(Why::MergeFromClosed));
    let (_, o) = series(&mut w, s, ROOT, "daily");
    refused(
        &mut w,
        &c,
        s,
        merge(o, a),
        inv(Why::OccurrenceNotYetAvailable),
    );
    // a depends on x, and x waits on b: merging b into a closes the cycle a → x → a.
    let x = node(&mut w, &c, s, "/x", &[]);
    w.emit(Ev::NodeUpdated {
        node: a,
        patch: crate::event::NodePatch {
            depends_on: Some([x].into_iter().collect()),
            ..crate::event::NodePatch::default()
        },
    });
    w.emit(Ev::WaitSet {
        node: x,
        wait: Some(WaitOn::Node(b)),
    });
    refused(
        &mut w,
        &c,
        s,
        merge(b, a),
        Error::of(Hint::Cycle {
            path: vec![a, x, a],
        }),
    );
    // Merging a into b instead makes no cycle: x → b stays, a's edges stay with closed a.
    ok(w.call(&c, s, merge(a, b)));
}

/// N5: a merge moves the keys and what is still open, with ids and `moved_from`; leaves settled
/// and closed items; redirects waits and dependencies, dropping self-edges; closes `from` as
/// superseded with `merged_into`, ending its holds; and resolves the pair's signals.
#[test]
fn merge_moves_what_is_open() {
    let (mut w, c, s) = one();
    let s2 = second(&mut w);
    let f = node(&mut w, &c, s, "/f", &["chat:a", "issue:p#1"]);
    let i = node(&mut w, &c, s, "/i", &["mail:m"]);
    let wt = node(&mut w, &c, s, "/w", &[]);
    let dp = node(&mut w, &c, s, "/d", &[]);
    ok(w.call(&c, s, link(f, LinkRel::Duplicate, i)));
    let sig = signal(&w, SignalKind::Duplicate, f, i).unwrap();
    // Requests: one settled (stays), one open (moves).
    let r0 = request(&mut w, s, f, "chat:a/0", "chat:a");
    let x0 = said(&mut w, s, f, "chat:a", SaidKind::Reply);
    w.emit(Ev::RequestAnswered {
        request: r0,
        by: x0,
    });
    let d0 = decision(&mut w, s, f, &[r0]);
    w.emit(Ev::RequestSettled {
        request: r0,
        decision: d0,
        answer: x0,
    });
    w.emit(Ev::SaidConfirmed {
        said: x0,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    let r1 = request(&mut w, s, f, "chat:a/1", "chat:a");
    // People questions: one open (moves, with its pending said), one answered (stays).
    let q1 = people_question(&mut w, s, f, "chat:a", false);
    let q0 = people_question(&mut w, s, f, "chat:a", false);
    let hx = entry(
        &mut w,
        f,
        Author::Session(s),
        EntryBody::Heard {
            conv: ks("chat:a"),
            answers: Some(q0),
        },
    );
    w.emit(Ev::PqAnswered { q: q0, by: hx });
    // An open commitment, an open objection, a recheck mark, and s2's intent.
    let ack = said(&mut w, s, f, "chat:a", SaidKind::Ack);
    let followup_at = w.clock.plus(w.s.params.commitment_followup);
    w.emit(Ev::CommitmentOpened {
        c: ack,
        node: f,
        conv: ks("chat:a"),
        followup_at,
    });
    let obj = entry(&mut w, f, Author::Session(s2), EntryBody::Object);
    w.emit(Ev::MarkSet {
        node: f,
        mark: Mark::Recheck,
        question: q1,
        on: true,
    });
    // An expired question with its assume mark, and an escalated one with its user question:
    // both move (N5: "open questions" are those not yet answered or superseded).
    let q2 = people_question(&mut w, s, f, "chat:a", false);
    w.emit(Ev::PqExpired { q: q2 });
    w.emit(Ev::MarkSet {
        node: f,
        mark: Mark::Assume,
        question: q2,
        on: true,
    });
    let q3 = people_question(&mut w, s, f, "chat:a", true);
    let due = w.clock.plus(Dur(3_600_000));
    let uq = entry(
        &mut w,
        USER_DECISIONS,
        Author::Server,
        EntryBody::Question(crate::event::QuestionBody::User {
            source: crate::state::UqSource::Escalation { question: q3 },
            about: Some(f),
            to: vec![USER_PRINCIPAL],
            due,
            has_default: false,
        }),
    );
    w.emit(Ev::PqEscalated { q: q3, uq });
    let t2 = ok(w.call(&c, s2, take(f)));
    let Reply::Taken { token: t2, .. } = t2.reply else {
        panic!();
    };
    let cp = entry(
        &mut w,
        f,
        Author::Session(s2),
        EntryBody::Checkpoint {
            token: t2,
            release: None,
            pr_named: None,
            handed_off_until: None,
        },
    );
    w.emit(Ev::IntentOpened {
        node: f,
        intent: Intent {
            eid: cp,
            session: s2,
            pr_named: false,
        },
    });
    // A waiter on f, a dependent of f, and `into` depending on f.
    w.emit(Ev::WaitSet {
        node: wt,
        wait: Some(WaitOn::Node(f)),
    });
    for n in [dp, i] {
        w.emit(Ev::NodeUpdated {
            node: n,
            patch: crate::event::NodePatch {
                depends_on: Some([f].into_iter().collect()),
                ..crate::event::NodePatch::default()
            },
        });
    }
    let d = ok(w.call(&c, s, merge(f, i)));
    assert_eq!(d.reply, Reply::Merged { from: f, into: i });
    assert_eq!(
        d.warnings[0].detail,
        WarningDetail::HeldByOther {
            holders: vec![s2],
            left: vec![]
        }
    );
    // from: closed, merged, keyless, markless, holdless.
    assert_eq!(closed_with(&w, f), Some((Outcome::Superseded, Some(i))));
    assert!(w.s.nodes[&f].keys.is_empty());
    assert!(w.s.nodes[&f].recheck_marks.is_empty());
    assert!(w.s.holds.is_empty());
    // into: the keys, the open items, the mark, the intent.
    let ik: Vec<KeyStr> = w.s.nodes[&i].keys.keys().cloned().collect();
    assert_eq!(ik, vec![ks("chat:a"), ks("issue:p#1"), ks("mail:m")]);
    assert_eq!(w.s.requests[&r1].node, i);
    assert_eq!(w.s.requests[&r1].moved_from, Some(f));
    assert_eq!(w.s.requests[&r0].node, f);
    assert_eq!(w.s.pquestions[&q1].node, i);
    assert_eq!(w.s.pquestions[&q0].node, f);
    let qsaid = w.s.pquestions[&q1].said;
    assert_eq!(w.s.saids[&qsaid].node, i);
    assert_eq!(w.s.saids[&qsaid].state, SaidState::Pending);
    assert_eq!(w.s.saids[&x0].node, f);
    assert_eq!(w.s.commitments[&ack].node, i);
    assert_eq!(w.s.objections[&obj].node, i);
    assert!(w.s.nodes[&i].recheck_marks.contains(&q1));
    for q in [q2, q3] {
        assert_eq!(w.s.pquestions[&q].node, i);
        assert_eq!(w.s.pquestions[&q].moved_from, Some(f));
    }
    assert!(w.s.nodes[&i].assume_marks.contains(&q2));
    assert!(w.s.nodes[&f].assume_marks.is_empty());
    assert_eq!(
        w.s.nodes[&i].open_intent,
        Some(Intent {
            eid: cp,
            session: s2,
            pr_named: false,
        })
    );
    // Waits and dependencies: redirected, the self-edge dropped.
    assert_eq!(
        w.s.nodes[&wt].wait.as_ref().map(|x| x.on),
        Some(WaitOn::Node(i))
    );
    assert!(w.s.nodes[&dp].depends_on.contains(&i));
    assert!(w.s.nodes[&i].depends_on.is_empty());
    // The pair's signal resolved; the system entry on into.
    assert!(!w.s.nodes[&sig].is_open());
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::Appended {
            node,
            body: EntryBody::System(SystemNote::MergedFrom { from, .. }),
            ..
        } if *node == i && *from == f
    )));
}

/// A merge credits a moved request only with a `said` already on `into` of the request's own
/// conversation, after it (§9.4, 1.3, A-95); a decided one then settles.
#[test]
fn a_merge_credits_only_a_said_of_the_requests_conversation() {
    let (mut w, c, s) = one();
    let f = node(&mut w, &c, s, "/f", &["chat:a"]);
    let i = node(&mut w, &c, s, "/i", &["chat:b"]);
    let r = request(&mut w, s, f, "chat:a/1", "chat:a");
    let d = decision(&mut w, s, f, &[r]);
    // A user request on f, routed by its key: a `said` never answers it, even in its key's name.
    let e = super::super::keys::tests::user_entry(&mut w, s, USER_PRINCIPAL);
    let uk = alloc::format!("user:e{}", e.0);
    ok(w.call(&c, s, key_add(f, &uk)));
    let crate::state::Routing::Request { request: ur, .. } = w.s.user_entries[&e].routing else {
        panic!();
    };
    said(&mut w, s, i, "chat:b", SaidKind::Reply);
    said(&mut w, s, i, &uk, SaidKind::Result);
    ok(w.call(&c, s, merge(f, i)));
    assert_eq!(w.s.requests[&r].node, i);
    assert_eq!(w.s.requests[&r].answered_by, None);
    assert_eq!(w.s.requests[&ur].node, i);
    assert_eq!(w.s.requests[&ur].answered_by, None);
    // The same with a reply of the request's conversation on into, after it: answered and
    // settled by the decision.
    let f2 = node(&mut w, &c, s, "/f2", &["chat:c"]);
    let i2 = node(&mut w, &c, s, "/i2", &[]);
    let early = said(&mut w, s, i2, "chat:c", SaidKind::Result);
    let r2 = request(&mut w, s, f2, "chat:c/1", "chat:c");
    let d2 = decision(&mut w, s, f2, &[r2]);
    said(&mut w, s, i2, "chat:c", SaidKind::Ack);
    let late = said(&mut w, s, i2, "chat:c", SaidKind::Correction);
    said(&mut w, s, i2, "chat:c", SaidKind::Reply);
    ok(w.call(&c, s, merge(f2, i2)));
    assert_ne!(w.s.requests[&r2].answered_by, Some(early));
    assert_eq!(w.s.requests[&r2].answered_by, Some(late));
    let st = w.s.requests[&r2].settled.unwrap();
    assert_eq!((st.decision, st.answer), (d2, late));
    let _ = d;
}

/// An intent opened on `n` by `sess` through a take and a checkpoint, by events.
fn intent_on(w: &mut World, c: &ClassName, sess: SessionId, n: NodeId) -> Intent {
    let d = ok(w.call(c, sess, take(n)));
    let Reply::Taken { token, .. } = d.reply else {
        panic!("{:?}", d.reply);
    };
    let cp = entry(
        w,
        n,
        Author::Session(sess),
        EntryBody::Checkpoint {
            token,
            release: None,
            pr_named: None,
            handed_off_until: None,
        },
    );
    let intent = Intent {
        eid: cp,
        session: sess,
        pr_named: false,
    };
    w.emit(Ev::IntentOpened { node: n, intent });
    intent
}

/// Review finding (D4 R7): an intent that cannot move stays off `into` (D.8.20 step 2, §16): the
/// merging session's own, which the close of `from` resolves (§5), and another session's when
/// `into` has an intent of its own, which stays on `from`; and N5's step check rejects a merge
/// that moves either.
#[test]
fn a_merge_leaves_an_intent_that_cannot_move() {
    let (mut w, c, s) = one();
    let s2 = second(&mut w);
    // The merger's own intent.
    let f = node(&mut w, &c, s, "/f", &[]);
    let i = node(&mut w, &c, s, "/i", &[]);
    let mine = intent_on(&mut w, &c, s, f);
    let mut st = step(&mut w, &c, s, merge(f, i));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    let at = st.ctx.now;
    let moved = Event {
        at,
        ev: Ev::IntentMoved { from: f, to: i },
    };
    st.events.insert(0, moved.clone());
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::N5));
    ok(w.call(&c, s, merge(f, i)));
    // It stays off `into`, and the close of `from` resolves it as the closer's own (§5).
    assert_eq!(w.s.nodes[&i].open_intent, None);
    assert_eq!(w.s.nodes[&f].open_intent, None);
    assert!(evs(&w).iter().any(|e| matches!(
        e,
        Ev::IntentClosed {
            node,
            end: crate::event::IntentEnd::ResolvedByClose
        } if *node == f
    )));
    let _ = mine;
    // Another session's intent, while `into` has one: both stay.
    let f2 = node(&mut w, &c, s, "/f2", &[]);
    let i2 = node(&mut w, &c, s, "/i2", &[]);
    let theirs = intent_on(&mut w, &c, s2, f2);
    let s3 = second(&mut w);
    let held = intent_on(&mut w, &c, s3, i2);
    let mut st = step(&mut w, &c, s, merge(f2, i2));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    // `evolve` refuses an `IntentMoved` onto a node with an intent, so the bad step replaces
    // `into`'s intent first.
    let at = st.ctx.now;
    for ev in [
        Ev::IntentMoved { from: f2, to: i2 },
        Ev::IntentClosed {
            node: i2,
            end: crate::event::IntentEnd::ResolvedByClose,
        },
    ] {
        st.events.insert(0, Event { at, ev });
    }
    refold(&mut st);
    assert_eq!(st.after.nodes[&i2].open_intent, Some(theirs));
    assert!(violated(&st).contains(&Inv::N5));
    ok(w.call(&c, s, merge(f2, i2)));
    assert_eq!(w.s.nodes[&f2].open_intent, Some(theirs));
    assert_eq!(w.s.nodes[&i2].open_intent, Some(held));
}

/// Review finding (D4 R9): a merge credits only a moved request still unanswered; an answered one
/// keeps its answer, even with a later qualifying `said` on `into` (D.8.20 step 3).
#[test]
fn a_merge_leaves_an_answered_request_alone() {
    let (mut w, c, s) = one();
    let f = node(&mut w, &c, s, "/f", &["chat:a"]);
    let i = node(&mut w, &c, s, "/i", &[]);
    let r = request(&mut w, s, f, "chat:a/1", "chat:a");
    let x1 = said(&mut w, s, f, "chat:a", SaidKind::Reply);
    w.emit(Ev::RequestAnswered { request: r, by: x1 });
    let x2 = said(&mut w, s, i, "chat:a", SaidKind::Reply);
    ok(w.call(&c, s, merge(f, i)));
    assert_eq!(w.s.requests[&r].node, i);
    assert_eq!(w.s.requests[&r].answered_by, Some(x1));
    assert!(
        !evs(&w)
            .iter()
            .any(|e| matches!(e, Ev::RequestAnswered { .. })),
        "{x2:?} answers nothing"
    );
}

/// Review finding (D4 R9): of two decisions that list a request, settling names the smaller
/// (`min ix.settling[r]`, D.8.8), whichever settles it: here a merge's credit.
#[test]
fn settling_names_the_smallest_listing_decision() {
    let (mut w, c, s) = one();
    let f = node(&mut w, &c, s, "/f", &["chat:a"]);
    let i = node(&mut w, &c, s, "/i", &[]);
    let r = request(&mut w, s, f, "chat:a/1", "chat:a");
    let d1 = decision(&mut w, s, f, &[r]);
    let d2 = decision(&mut w, s, f, &[r]);
    assert!(d1 < d2);
    let x = said(&mut w, s, i, "chat:a", SaidKind::Result);
    ok(w.call(&c, s, merge(f, i)));
    let st = w.s.requests[&r].settled.unwrap();
    assert_eq!((st.decision, st.answer), (d1, x));
}

/// A merge moves an open commitment with its conversation and fulfils nothing (§9.2, 1.3).
#[test]
fn a_merge_moves_a_commitment_and_fulfils_nothing() {
    let (mut w, c, s) = one();
    let f = node(&mut w, &c, s, "/f", &["chat:a"]);
    let i = node(&mut w, &c, s, "/i", &["chat:b"]);
    said(&mut w, s, i, "chat:a", SaidKind::Reply);
    let ack = said(&mut w, s, f, "chat:a", SaidKind::Ack);
    let followup_at = w.clock.plus(w.s.params.commitment_followup);
    w.emit(Ev::CommitmentOpened {
        c: ack,
        node: f,
        conv: ks("chat:a"),
        followup_at,
    });
    ok(w.call(&c, s, merge(f, i)));
    let x = &w.s.commitments[&ack];
    assert_eq!((x.node, x.moved_from), (i, Some(f)));
    assert_eq!(x.conv, ks("chat:a"));
    assert_eq!(x.state, crate::state::CommitmentState::Open);
}

// ------------------------------------------------------------------------------------------------
// The reserved nodes (§4, §21: K8 and C1)

/// Every reserved node refuses a move, a merge on either side, a close, a key (also by routing:
/// a `user:` key), and a capacity or `repeat` change, with `invalid(ReservedNode)`, and is no
/// conversation node (C1); children of reserved nodes are created as usual; K8 holds throughout.
#[test]
fn every_reserved_node_refuses_with_reserved_node() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &[]);
    let e = super::super::keys::tests::user_entry(&mut w, s, USER_PRINCIPAL);
    let uk = alloc::format!("user:e{}", e.0);
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
        let want = inv(Why::ReservedNode);
        refused(&mut w, &c, s, move_to(r, "/x"), want.clone());
        refused(&mut w, &c, s, merge(r, a), want.clone());
        refused(&mut w, &c, s, merge(a, r), want.clone());
        refused(&mut w, &c, s, close_cmd(r, Outcome::Done), want.clone());
        refused(&mut w, &c, s, key_add(r, "chat:x/1"), want.clone());
        refused(&mut w, &c, s, key_add(r, &uk), want.clone());
        let v = w.s.nodes[&r].version;
        let update = |patch: Patch| {
            Cmd::Node(NodeCmd::Update {
                at: id(r),
                version: v,
                digest_version: None,
                token: None,
                patch,
            })
        };
        refused(
            &mut w,
            &c,
            s,
            update(Patch {
                capacity: Some(1),
                ..Patch::default()
            }),
            want.clone(),
        );
        refused(
            &mut w,
            &c,
            s,
            update(Patch {
                repeat: Change::Clear,
                ..Patch::default()
            }),
            want.clone(),
        );
        assert_eq!(
            crate::helpers::conversation::conv_node(&w.s, r, &ks("chat:new")),
            Err(want)
        );
    }
    let d = ok(w.call(&c, s, create_with("/@user/child", |x| x.take = Some(false))));
    assert!(matches!(d.reply, Reply::Created { .. }));
    assert!(crate::invariants::check_invariants(&w.s, w.clock).is_ok());
}

// ------------------------------------------------------------------------------------------------
// The step checks of WP-D4 reject hand-built violating steps (D.9.2)

/// One step of `apply`, with what `check_step` reads.
struct Step {
    before: State,
    ctx: Ctx,
    cmd: Cmd,
    out: Result<Done, Error>,
    events: Vec<Event>,
    after: State,
}

fn step(w: &mut World, c: &ClassName, s: SessionId, cmd: Cmd) -> Step {
    w.settle();
    let ctx = w.ctx(c, SessionCtx::Existing(s), None);
    let before = w.s.clone();
    let mut after = before.clone();
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

fn refold(st: &mut Step) {
    let mut s = st.before.clone();
    for e in &st.events {
        crate::evolve::evolve_mut(&mut s, e);
    }
    st.after = s;
}

/// K7: a reopen that drops a key no other open node carries.
#[test]
fn k7_rejects_a_reopen_that_drops_a_free_key() {
    let (mut w, c, s) = one();
    let a = node(&mut w, &c, s, "/a", &["chat:x/1", "issue:p#1"]);
    ok(w.call(&c, s, close_cmd(a, Outcome::Done)));
    let mut st = step(&mut w, &c, s, reopen(a));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    let at = st.ctx.now;
    let pos = st
        .events
        .iter()
        .position(|e| matches!(e.ev, Ev::NodeReopened { .. }))
        .unwrap();
    st.events.insert(
        pos,
        Event {
            at,
            ev: Ev::KeyRemoved {
                node: a,
                key: ks("issue:p#1"),
            },
        },
    );
    refold(&mut st);
    assert!(violated(&st).contains(&Inv::K7));
}

/// N5: a merge that leaves behind anything N5 moves (a request, a people question, a commitment, an
/// objection, a pending `said`, a mark, a key, another session's intent) or redirects (a wait, a
/// dependency), or that leaves `into` waiting on itself; and a refused merge that applies
/// something.
#[test]
fn n5_rejects_a_merge_that_leaves_things_behind() {
    let (mut w, c, s) = one();
    let s2 = second(&mut w);
    let f = node(&mut w, &c, s, "/f", &["chat:a"]);
    let i = node(&mut w, &c, s, "/i", &[]);
    let x = node(&mut w, &c, s, "/x", &[]);
    let dp = node(&mut w, &c, s, "/d", &[]);
    request(&mut w, s, f, "chat:a/1", "chat:a");
    for n in [x, i] {
        w.emit(Ev::WaitSet {
            node: n,
            wait: Some(WaitOn::Node(f)),
        });
    }
    w.emit(Ev::NodeUpdated {
        node: dp,
        patch: crate::event::NodePatch {
            depends_on: Some([f].into_iter().collect()),
            ..crate::event::NodePatch::default()
        },
    });
    // A people question with its pending `said` and a recheck mark.
    let q = people_question(&mut w, s, f, "chat:a", false);
    w.emit(Ev::MarkSet {
        node: f,
        mark: Mark::Recheck,
        question: q,
        on: true,
    });
    // An open commitment, an open objection, and another session's intent.
    let ack = said(&mut w, s, f, "chat:a", SaidKind::Ack);
    let followup_at = w.clock.plus(w.s.params.commitment_followup);
    w.emit(Ev::CommitmentOpened {
        c: ack,
        node: f,
        conv: ks("chat:a"),
        followup_at,
    });
    entry(&mut w, f, Author::Session(s2), EntryBody::Object);
    intent_on(&mut w, &c, s2, f);
    let st = step(&mut w, &c, s, merge(f, i));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    let evs_of = |st: &Step, p: fn(&Ev) -> bool| st.events.iter().filter(|e| p(&e.ev)).count();
    let drops: [fn(&Ev) -> bool; 11] = [
        |e| matches!(e, Ev::RequestMoved { .. }),
        |e| matches!(e, Ev::WaitSet { .. }),
        |e| matches!(e, Ev::KeyAdded { .. }),
        |e| matches!(e, Ev::PqMoved { .. }),
        |e| matches!(e, Ev::MarkSet { on: true, .. }),
        |e| matches!(e, Ev::MarkSet { on: false, .. }),
        |e| matches!(e, Ev::CommitmentMoved { .. }),
        |e| matches!(e, Ev::ObjectionMoved { .. }),
        |e| matches!(e, Ev::SaidMoved { .. }),
        |e| matches!(e, Ev::IntentMoved { .. }),
        |e| matches!(e, Ev::DependsRedirected { .. }),
    ];
    for drop in drops {
        assert!(evs_of(&st, drop) > 0, "the merge emits each kind");
        let mut st2 = step(&mut w, &c, s, merge(f, i));
        st2.events.retain(|e| !drop(&e.ev));
        refold(&mut st2);
        assert!(violated(&st2).contains(&Inv::N5));
    }
    // `into`'s wait on `from`, redirected to itself instead of cleared.
    let mut st4 = step(&mut w, &c, s, merge(f, i));
    for e in &mut st4.events {
        if let Ev::WaitSet { node, wait } = &mut e.ev
            && *node == i
        {
            *wait = Some(WaitOn::Node(i));
        }
    }
    refold(&mut st4);
    assert!(violated(&st4).contains(&Inv::N5));
    // A refused merge with an event.
    let mut st3 = step(&mut w, &c, s, merge(f, f));
    assert!(st3.out.is_err());
    st3.events = st.events.clone();
    refold(&mut st3);
    assert!(violated(&st3).contains(&Inv::N5));
}

/// Review finding (D4 R13): Settled's merge clause rejects a merge step that does not credit a
/// moved request with the first qualifying `said` on `into` after it, credits a later one, one of
/// another conversation, or an answered request.
#[test]
fn settled_rejects_a_wrong_merge_credit() {
    let (mut w, c, s) = one();
    let f = node(&mut w, &c, s, "/f", &["chat:a"]);
    let i = node(&mut w, &c, s, "/i", &["chat:b"]);
    let r = request(&mut w, s, f, "chat:a/1", "chat:a");
    let answered = request(&mut w, s, f, "chat:a/2", "chat:a");
    // The answer of `answered`, confirmed, so it stays on `from` (N5).
    let x0 = said(&mut w, s, f, "chat:a", SaidKind::Reply);
    w.emit(Ev::RequestAnswered {
        request: answered,
        by: x0,
    });
    w.emit(Ev::SaidConfirmed {
        said: x0,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    let other = said(&mut w, s, i, "chat:b", SaidKind::Reply);
    let first = said(&mut w, s, i, "chat:a", SaidKind::Reply);
    let later = said(&mut w, s, i, "chat:a", SaidKind::Result);
    let st = step(&mut w, &c, s, merge(f, i));
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    assert!(st.events.iter().any(|e| matches!(
        e.ev,
        Ev::RequestAnswered { request, by } if request == r && by == first
    )));
    let mut rewrite = |by: Option<Eid>, extra: Option<Ev>| {
        let mut st2 = step(&mut w, &c, s, merge(f, i));
        match by {
            Some(a) => {
                for e in &mut st2.events {
                    if let Ev::RequestAnswered { by, .. } = &mut e.ev {
                        *by = a;
                    }
                }
            }
            None if extra.is_none() => st2
                .events
                .retain(|e| !matches!(e.ev, Ev::RequestAnswered { .. })),
            None => {}
        }
        if let Some(ev) = extra {
            let at = st2.ctx.now;
            st2.events.push(Event { at, ev });
        }
        refold(&mut st2);
        violated(&st2)
    };
    assert!(rewrite(None, None).contains(&Inv::Settled), "not credited");
    assert!(rewrite(Some(later), None).contains(&Inv::Settled), "later");
    assert!(
        rewrite(Some(other), None).contains(&Inv::Settled),
        "conversation"
    );
    let wrong = Ev::RequestAnswered {
        request: answered,
        by: later,
    };
    assert!(
        rewrite(None, Some(wrong)).contains(&Inv::Settled),
        "answered"
    );
}

/// C1: a create's request on another node than its conversation node, or on a reserved node.
#[test]
fn c1_rejects_a_request_off_its_conversation_node() {
    let (mut w, c, s) = one();
    let other = node(&mut w, &c, s, "/other", &[]);
    let cmd = create_with("/a", |x| {
        x.keys = vec![key("chat:x/1")];
        x.take = Some(false);
        x.request = Some(RequestArg {
            text: text("please"),
            origin: Some(key("chat:x/1/1")),
            where_: None,
            requester: text("bob"),
        });
    });
    let st = step(&mut w, &c, s, cmd.clone());
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    for to in [other, USER] {
        let mut st2 = step(&mut w, &c, s, cmd.clone());
        let seq = Seq(st2.before.nodes[&to].seq.0 + 1);
        for e in &mut st2.events {
            if let Ev::Appended {
                node,
                seq: sq,
                body: EntryBody::Request { .. },
                ..
            } = &mut e.ev
            {
                *node = to;
                *sq = seq;
            }
        }
        refold(&mut st2);
        assert!(violated(&st2).contains(&Inv::C1), "{to:?}");
    }
}

/// Review finding (D4 R8): C1's reserved clause alone rejects a request on a reserved node that is
/// its own conversation node (case 3 of §9.1 with `at = @user`): no other clause of C1 fires.
#[test]
fn c1_rejects_a_request_on_a_reserved_conversation_node() {
    let (mut w, c, s) = one();
    let cmd = Cmd::Talk(TalkCmd::Request {
        at: id(USER),
        text: text("please"),
        origin: key("chat:new/1"),
        where_: Some(key("chat:new")),
        requester: text("bob"),
    });
    let mut st = step(&mut w, &c, s, cmd);
    let eid = st.before.counters.next_eid;
    st.events = vec![Event {
        at: st.ctx.now,
        ev: Ev::Appended {
            eid,
            node: USER,
            seq: Seq(st.before.nodes[&USER].seq.0 + 1),
            by: Author::Session(s),
            body: EntryBody::Request {
                origin: key("chat:new/1"),
                conv: ks("chat:new"),
                from_user: None,
                requester: Requester::External,
            },
            text: crate::event::EntryText::default(),
        },
    }];
    refold(&mut st);
    assert_eq!(
        crate::spec::conv_node(&st.before, USER, &ks("chat:new")),
        USER
    );
    assert!(violated(&st).contains(&Inv::C1));
}

/// C1 (1.3, §9.2): a reply that fulfils a commitment of another conversation, or leaves one of
/// its own conversation open.
#[test]
fn c1_rejects_a_fulfilment_across_conversations() {
    let (mut w, c, s) = one();
    let n = node(&mut w, &c, s, "/n", &["chat:a", "mail:m"]);
    let mut open = BTreeMap::new();
    for conv in ["chat:a", "mail:m"] {
        let ack = said(&mut w, s, n, conv, SaidKind::Ack);
        let followup_at = w.clock.plus(w.s.params.commitment_followup);
        w.emit(Ev::CommitmentOpened {
            c: ack,
            node: n,
            conv: ks(conv),
            followup_at,
        });
        open.insert(conv, ack);
    }
    // A real step (a view) with a reply in `chat:a` added: it must fulfil chat:a's commitment.
    let base = step(&mut w, &c, s, crate::apply::tests::create("/v"));
    let reply_at = |st: &Step| {
        let eid = st.before.counters.next_eid;
        Event {
            at: st.ctx.now,
            ev: Ev::Appended {
                eid,
                node: n,
                seq: Seq(st.before.nodes[&n].seq.0 + 1),
                by: Author::Session(s),
                body: EntryBody::Said {
                    conv: ks("chat:a"),
                    kind: SaidKind::Reply,
                    corrects: None,
                    voice: false,
                },
                text: crate::event::EntryText::default(),
            },
        }
    };
    // The reply with chat:a's fulfilment: passes.
    let mut good = step(&mut w, &c, s, crate::apply::tests::create("/v"));
    let r = reply_at(&good);
    let eid = good.before.counters.next_eid;
    good.events = vec![
        r.clone(),
        Event {
            at: good.ctx.now,
            ev: Ev::CommitmentFulfilled {
                c: open["chat:a"],
                by: eid,
            },
        },
    ];
    good.out = base.out.clone();
    refold(&mut good);
    let v = violated(&good);
    assert!(!v.contains(&Inv::C1), "{v:?}");
    // The reply leaves chat:a's open.
    let mut leaves = step(&mut w, &c, s, crate::apply::tests::create("/v"));
    leaves.events = vec![reply_at(&leaves)];
    refold(&mut leaves);
    assert!(violated(&leaves).contains(&Inv::C1));
    // The reply fulfils mail:m's too.
    let mut across = step(&mut w, &c, s, crate::apply::tests::create("/v"));
    let eid = across.before.counters.next_eid;
    across.events = vec![
        reply_at(&across),
        Event {
            at: across.ctx.now,
            ev: Ev::CommitmentFulfilled {
                c: open["chat:a"],
                by: eid,
            },
        },
        Event {
            at: across.ctx.now,
            ev: Ev::CommitmentFulfilled {
                c: open["mail:m"],
                by: eid,
            },
        },
    ];
    refold(&mut across);
    assert!(violated(&across).contains(&Inv::C1));
}

/// L1's half for series (D.14.5): a move that puts a series at `tree.max_depth`.
#[test]
fn l1_rejects_a_series_moved_to_the_depth_limit() {
    let (mut w, c, s) = one();
    let max = usize::try_from(w.s.params.tree_max_depth).unwrap();
    let deep: String = (0..max - 1).map(|i| alloc::format!("/d{i}")).collect();
    let parent = node(&mut w, &c, s, &deep, &[]);
    let (x, _) = series(&mut w, s, ROOT, "daily");
    ok(w.call(&c, s, close_cmd(x, Outcome::Done)));
    let plain = node(&mut w, &c, s, "/plain", &[]);
    let mut st = step(
        &mut w,
        &c,
        s,
        move_to(plain, &alloc::format!("{deep}/plain")),
    );
    assert!(violated(&st).is_empty(), "{:?}", violated(&st));
    for e in &mut st.events {
        if let Ev::NodeMoved { node, name, .. } = &mut e.ev {
            *node = x;
            *name = Name::new("daily").unwrap();
        }
        if let Ev::Appended { node, .. } = &mut e.ev
            && *node == plain
        {
            *node = x;
        }
    }
    for e in &mut st.events {
        if let Ev::Appended { seq, node, .. } = &mut e.ev
            && *node == x
        {
            *seq = Seq(st.before.nodes[&x].seq.0 + 1);
        }
    }
    refold(&mut st);
    assert_eq!(st.after.nodes[&x].parent(), Some(parent));
    assert!(violated(&st).contains(&Inv::L1));
}

/// `merge_edges` lists exactly the edges a merge changes: every edge to `from` leaves, and its
/// `redirect` to `into` comes in, but for the one from `into`, a self-edge, which is dropped (N5).
#[test]
fn merge_edges_lists_the_changed_edges() {
    use crate::helpers::restructure::{merge_edges, redirect};
    let n = NodeId;
    let (extra, removed) = merge_edges(&[n(1), n(3), n(5)], n(9), n(3));
    assert_eq!(removed, vec![(n(1), n(9)), (n(3), n(9)), (n(5), n(9))]);
    assert_eq!(extra, vec![(n(1), n(3)), (n(5), n(3))]);
    assert_eq!(redirect(n(1), n(9), n(9), n(3)), Some(n(3)));
    assert_eq!(redirect(n(3), n(9), n(9), n(3)), None);
    assert_eq!(redirect(n(1), n(4), n(9), n(3)), Some(n(4)));
    assert_eq!(redirect(n(4), n(4), n(9), n(3)), None);
}

/// A wait of `into` on `from` would be a self-edge after the merge: it is cleared, not redirected
/// (N5, T5); another node's wait on `from` waits on `into`.
#[test]
fn a_merge_drops_the_self_wait_of_into() {
    let (mut w, c, s) = one();
    let f = node(&mut w, &c, s, "/f", &[]);
    let i = node(&mut w, &c, s, "/i", &[]);
    let x = node(&mut w, &c, s, "/x", &[]);
    for n in [i, x] {
        w.emit(Ev::WaitSet {
            node: n,
            wait: Some(WaitOn::Node(f)),
        });
    }
    ok(w.call(&c, s, merge(f, i)));
    assert!(w.s.nodes[&i].wait.is_none());
    assert_eq!(
        w.s.nodes[&x].wait.as_ref().map(|x| x.on),
        Some(WaitOn::Node(i))
    );
}
