//! Tests of `evolve`, the indices, the recount, and the state checks (design D.12.1, D.13.2 WP-D2):
//! hand-built event sequences through `World`, which after every event requires no anomaly, a dirty
//! set that covers every changed fact, and indices equal to their recount; the full state checks at
//! the points where the sequence is a settled state; and a hand-built violation of every state
//! check. The child module `checks` (`evolve_check_tests.rs`) breaks each clause of the state
//! checks on its own, and holds the regression tests of the package's review.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use super::*;
use crate::deadlines::{deadline_of, recount_deadlines};
use crate::event::{EntryText, Extension, IntentEnd, NextOutcome, NodeInit, NodePatch};
use crate::invariants::{Inv, check_invariants, check_keys, check_startup};
use crate::params::Params;
use crate::recount::recount;
use crate::state::{
    BasisRef, Cand, DeadlineKey, DeadlineKind, DigestItem, LeaseTerms, Owner, Role, Settlement,
    WaitOn,
};
use crate::testkit::World;
use crate::types::{
    Addressee, ClassName, DAY, Flag, FlagSet, HOUR, Key, KeyStr, MINUTE, Name, NodeId, NormVer,
    OpId, Outcome, PrincipalKey, ReqDigest, Requester, SaidKind, ToUserKind, Token,
};

const T0: Time = Time(1_790_000_000_000);

fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}

fn class(s: &str) -> ClassName {
    ClassName::new(s).unwrap()
}

fn key(s: &str) -> Key {
    Key {
        key: KeyStr::new(s).unwrap(),
        nv: NormVer(1),
    }
}

fn init(title: &str) -> NodeInit {
    NodeInit {
        title: Text(title.into()),
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
        role: Role::Plain,
    }
}

fn terms(p: &Params) -> LeaseTerms {
    LeaseTerms {
        first: p.lease_first,
        default: p.lease_default,
        max: p.lease_max,
        ceiling: p.lease_ceiling,
        renew_max: p.renew_max,
    }
}

/// Builders over `World` that emit one checked event each.
trait Build {
    fn session(&mut self, class_name: &str) -> SessionId;
    fn create(&mut self, parent: NodeId, n: &str, init: NodeInit) -> NodeId;
    fn node(&mut self, parent: NodeId, n: &str) -> NodeId;
    fn take(&mut self, n: NodeId, s: SessionId) -> Token;
    fn append(&mut self, node: NodeId, by: Author, body: EntryBody) -> Eid;
    fn principal(&mut self, k: &str) -> PrincipalId;
    fn touch(&mut self, s: SessionId);
}

impl Build for World {
    fn touch(&mut self, s: SessionId) {
        let p = self.s.params.clone();
        let retire = self.s.sessions[&s]
            .retire_at
            .max(self.clock.plus(p.session_retain));
        self.emit(Ev::Touched {
            session: s,
            silence_at: self.clock.plus(p.presence_silence),
            retire_at: retire,
        });
    }

    fn session(&mut self, class_name: &str) -> SessionId {
        let s = self.s.counters.next_session;
        let p = self.s.params.clone();
        self.emit(Ev::SessionOpened {
            session: s,
            class: class(class_name),
            caps: BTreeSet::new(),
            horizon: None,
            intent: None,
            silence_at: self.clock.plus(p.presence_silence),
            retire_at: self.clock.plus(p.session_retain),
            principal: None,
        });
        self.sessions.push((class(class_name), s));
        s
    }

    fn create(&mut self, parent: NodeId, n: &str, init: NodeInit) -> NodeId {
        let id = self.s.counters.next_node;
        self.emit(Ev::NodeCreated {
            id,
            parent,
            name: name(n),
            origin: Origin::Agent(SessionId(1)),
            init,
        });
        id
    }

    fn node(&mut self, parent: NodeId, n: &str) -> NodeId {
        self.create(parent, n, init(n))
    }

    fn take(&mut self, n: NodeId, s: SessionId) -> Token {
        let fence = Fence(self.s.nodes[&n].fence.0 + 1);
        let token = Token { node: n, fence };
        let p = self.s.params.clone();
        self.emit(Ev::HoldIssued {
            token,
            session: s,
            lease_until: self.clock.plus(p.lease_first),
            terms: terms(&p),
            l: p.lease_default,
            reopened: None,
        });
        token
    }

    fn append(&mut self, node: NodeId, by: Author, body: EntryBody) -> Eid {
        let eid = self.s.counters.next_eid;
        let seq = Seq(self.s.nodes[&node].seq.0 + 1);
        self.emit(Ev::Appended {
            eid,
            node,
            seq,
            by,
            body,
            text: EntryText::default(),
        });
        eid
    }

    fn principal(&mut self, k: &str) -> PrincipalId {
        let id = self.s.counters.next_principal;
        self.emit(Ev::PrincipalSeen {
            id,
            key: PrincipalKey::new(k).unwrap(),
        });
        id
    }
}

fn world() -> World {
    World::new(Params::default(), T0)
}

fn ok(w: &World) {
    if let Err(v) = w.check() {
        panic!("invariants at {:?}: {v:?}", w.clock);
    }
}

fn violates(s: &State, now: Time, inv: Inv) -> bool {
    check_invariants(s, now).is_err_and(|v| v.iter().any(|x| x.inv == inv))
}

// ------------------------------------------------------------------------------------------------
// Genesis and the reserved nodes

#[test]
fn genesis_makes_the_reserved_nodes_and_the_user_principal() {
    let w = world();
    assert_eq!(w.s.nodes.len(), 8);
    assert_eq!(w.s.names.len(), 7);
    assert_eq!(w.s.principal_ids.len(), 1);
    assert_eq!(w.s.counters.next_principal, PrincipalId(1));
    assert_eq!(w.s.ix.node[&USER_DECISIONS].depth, 2);
    assert_eq!(w.s.ix.node[&ROOT].sub.open, 7);
    assert_eq!(w.s.nodes[&USER].title.0, "@user");
    ok(&w);
    check_startup(&w.s, w.clock).unwrap();
    assert_eq!(w.replay(), w.s);
}

#[test]
fn a_second_genesis_is_an_anomaly() {
    let mut w = world();
    let before = w.s.clone();
    w.emit_raw(Ev::Genesis {
        params: Params::default(),
    });
    assert_eq!(w.s.anomalies, 1);
    assert_eq!(w.s.nodes, before.nodes);
}

// ------------------------------------------------------------------------------------------------
// Sessions, nodes, holds, and the 1.4 Silence key (R27)

#[test]
fn a_session_a_node_and_a_hold() {
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    // An unflagged, unoffered node is no candidate.
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(a)));
    w.emit(Ev::OfferSet { node: a, on: true });
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(a)));
    assert_eq!(w.s.since[&Cand::Node(a)], T0);
    ok(&w);
    let t = w.take(a, s1);
    assert_eq!(w.s.ix.node[&a].live, 1);
    assert_eq!(w.s.ix.node[&ROOT].subtree_live, 1);
    assert!(w.s.ix.attended_children[&ROOT].contains(&a));
    // Capacity 1 and one live hold: no longer a candidate.
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(a)));
    ok(&w);
    w.at(T0.plus(MINUTE));
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::Released,
    });
    assert_eq!(w.s.nodes[&a].held_ms, MINUTE.0);
    assert!(!w.s.ix.attended_children.contains_key(&ROOT));
    assert_eq!(w.s.stats.days[&day_of(w.clock)].supply_ms, MINUTE.0);
    ok(&w);
    assert_eq!(w.replay(), w.s);
}

#[test]
fn the_silence_key_waits_for_the_last_lease() {
    // 1.4 (§3, R27, D.4.8): a holder's Silence key is the later of `silence_at` and its last
    // lease's end.
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    let silence_at = w.s.sessions[&s1].silence_at;
    let owner = Owner::Session(s1);
    assert_eq!(
        deadline_of(&w.s, DeadlineKind::Silence, &owner),
        Some(silence_at)
    );
    let t = w.take(a, s1);
    // The first lease (15 min) ends with the presence (15 min): equal.
    assert_eq!(
        deadline_of(&w.s, DeadlineKind::Silence, &owner),
        Some(silence_at)
    );
    // A checkpoint within presence.resolution extends the lease past silence_at, and the key
    // follows it.
    w.at(T0.plus(MINUTE));
    w.append(
        a,
        Author::Session(s1),
        EntryBody::Checkpoint {
            token: t,
            release: None,
            pr_named: None,
            handed_off_until: None,
        },
    );
    let lease = w.clock.plus(w.s.params.lease_default);
    w.emit(Ev::HoldExtended {
        token: t,
        lease_until: lease,
        by: Extension::Checkpoint,
    });
    assert!(lease > silence_at);
    assert_eq!(
        deadline_of(&w.s, DeadlineKind::Silence, &owner),
        Some(lease)
    );
    assert!(w.s.ix.deadlines.contains(&DeadlineKey {
        at: lease,
        kind: DeadlineKind::Silence,
        owner: owner.clone(),
    }));
    ok(&w);
    // Another session's command ends the hold after silence_at: the key lies at that `now`.
    let later = silence_at.plus(MINUTE);
    w.at(later);
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::EndedByClose(crate::event::EndCause::Move),
    });
    assert_eq!(
        deadline_of(&w.s, DeadlineKind::Silence, &owner),
        Some(later)
    );
    // Firing it makes the session silent, holding nothing.
    w.emit(Ev::Fired {
        kind: DeadlineKind::Silence,
        owner: owner.clone(),
    });
    assert!(w.s.sessions[&s1].silent);
    assert!(!w.s.ix.live_sessions.contains(&s1));
    assert_eq!(deadline_of(&w.s, DeadlineKind::Silence, &owner), None);
}

#[test]
fn a_silence_key_in_the_past_is_floored_at_the_next_event() {
    // On a state that is not settled, a later event moves the overdue Silence key to its own time
    // (the floor at last_now), so the queue still equals its recount (T4).
    let mut w = world();
    let s1 = w.session("w");
    let silence_at = w.s.sessions[&s1].silence_at;
    w.at(silence_at.plus(HOUR));
    w.emit(Ev::SessionStatus {
        session: s1,
        text: Text("x".into()),
    });
    assert_eq!(
        deadline_of(&w.s, DeadlineKind::Silence, &Owner::Session(s1)),
        Some(w.clock)
    );
}

#[test]
fn retirement_needs_silence_and_no_hold() {
    let mut w = world();
    let s1 = w.session("w");
    let retire = w.s.sessions[&s1].retire_at;
    w.at(retire);
    // Not silent yet: an anomaly.
    let before = w.s.clone();
    w.emit_raw(Ev::SessionRetired { session: s1 });
    assert_eq!(w.s.anomalies, before.anomalies + 1);
    assert!(w.s.sessions.contains_key(&s1));
    w.emit(Ev::Fired {
        kind: DeadlineKind::Silence,
        owner: Owner::Session(s1),
    });
    w.emit(Ev::SessionRetired { session: s1 });
    assert!(!w.s.sessions.contains_key(&s1));
    assert!(w.s.ix.live_by_class.is_empty());
}

// ------------------------------------------------------------------------------------------------
// The tree: moves, closes, reopens, and the rollups (CS-32)

#[test]
fn subtree_rollups_follow_create_close_reopen_and_move() {
    let mut w = world();
    let a = w.node(ROOT, "a");
    let b = w.node(ROOT, "b");
    let a1 = w.node(a, "a1");
    let a2 = w.node(a1, "a2");
    assert_eq!(w.s.ix.node[&a].sub.open, 2);
    w.emit(Ev::NodeClosed {
        node: a2,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    assert_eq!(w.s.ix.node[&a].sub.done, 1);
    // a1 has one child, closed, but was never held: a folder, not ready to close (1.6, §14).
    assert!(!w.s.ix.node[&a1].flags.contains(Flag::ReadyToClose));
    assert_eq!(w.s.ix.node[&a].sub.flagged, 0);
    // Held once and released, it is work: ready_to_close.
    let s1 = w.session("w");
    let t = w.take(a1, s1);
    assert!(w.s.ix.node[&a1].flags.contains(Flag::ReadyToClose));
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::Released,
    });
    assert!(w.s.ix.node[&a1].flags.contains(Flag::ReadyToClose));
    assert_eq!(w.s.ix.node[&a].sub.flagged, 1);
    ok(&w);
    w.emit(Ev::NodeClosed {
        node: a1,
        outcome: Outcome::Failed,
        merged_into: None,
        by: None,
    });
    assert_eq!(w.s.ix.node[&a].sub.failed, 1);
    w.emit(Ev::NodeReopened { node: a1 });
    // Move the 2-node subtree under b.
    w.emit(Ev::NodeMoved {
        node: a1,
        parent: b,
        name: name("moved"),
    });
    assert_eq!(w.s.ix.node[&a].sub, crate::state::Subtree::default());
    assert_eq!(w.s.ix.node[&b].sub.open, 1);
    assert_eq!(w.s.ix.node[&b].sub.done, 1);
    assert_eq!(w.s.ix.node[&a2].depth, 3);
    assert_eq!(w.s.aliases[&(a, name("a1"))], a1);
    ok(&w);
    // A move into the node's own subtree is an anomaly (K4).
    let n = w.s.anomalies;
    w.emit_raw(Ev::NodeMoved {
        node: b,
        parent: a2,
        name: name("loop"),
    });
    assert_eq!(w.s.anomalies, n + 1);
}

#[test]
fn a_held_node_moved_carries_its_attendance() {
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    let b = w.node(ROOT, "b");
    let x = w.node(a, "x");
    w.take(x, s1);
    assert!(w.s.ix.attended_children[&a].contains(&x));
    w.emit(Ev::NodeMoved {
        node: x,
        parent: b,
        name: name("x"),
    });
    assert!(!w.s.ix.attended_children.contains_key(&a));
    assert!(w.s.ix.attended_children[&b].contains(&x));
    assert_eq!(w.s.ix.node[&b].subtree_live, 1);
    assert_eq!(w.s.ix.node[&a].subtree_live, 0);
    ok(&w);
}

// ------------------------------------------------------------------------------------------------
// The directory (A-113)

#[test]
fn a_person_is_a_container_and_follows_its_moves() {
    let mut w = world();
    let s1 = w.session("w");
    let bob = w.principal("mail:bob@x");
    let mut i = init("bob");
    i.keys
        .insert(KeyStr::new("person:mail:bob@x").unwrap(), NormVer(1));
    let p = w.create(PEOPLE, "bob", i);
    assert_eq!(w.s.ix.person_of[&bob], p);
    // A hold that ends gives a person no offer tag.
    let t = w.take(p, s1);
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::Released,
    });
    assert!(w.s.nodes[&p].tags.is_empty());
    // Tagged by hand, it is still no candidate.
    w.emit(Ev::OfferSet { node: p, on: true });
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(p)));
    // Moved out of @people, it is an ordinary offered node, and no longer a person.
    w.emit(Ev::NodeMoved {
        node: p,
        parent: ROOT,
        name: name("bob"),
    });
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(p)));
    assert!(!w.s.ix.person_of.contains_key(&bob));
    ok(&w);
    // Back in, a person again.
    w.emit(Ev::NodeMoved {
        node: p,
        parent: PEOPLE,
        name: name("bob2"),
    });
    assert_eq!(w.s.ix.person_of[&bob], p);
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(p)));
    // A principal interned after the key is an alias too.
    w.emit_raw(Ev::KeyAdded {
        node: p,
        key: Key {
            key: KeyStr::new("person:slack:t1/u2").unwrap(),
            nv: NormVer(1),
        },
    });
    assert_eq!(
        w.s.anomalies, 1,
        "the person: key's principal is not interned"
    );
    w.s.anomalies = 0;
    let slack = w.principal("slack:t1/u2");
    w.emit(Ev::KeyAdded {
        node: p,
        key: Key {
            key: KeyStr::new("person:slack:t1/u2").unwrap(),
            nv: NormVer(1),
        },
    });
    assert_eq!(w.s.ix.person_of[&slack], p);
    assert_eq!(w.s.ix.aliases_of[&p].len(), 2);
}

// ------------------------------------------------------------------------------------------------
// Requests, said, decisions, settling (§9.4, Settled)

fn conversation(w: &mut World) -> (SessionId, NodeId, KeyStr) {
    let s1 = w.session("w");
    let mut i = init("thread");
    i.keys.insert(KeyStr::new("chat:c/1").unwrap(), NormVer(1));
    i.keys.insert(KeyStr::new("mail:m@x").unwrap(), NormVer(1));
    let n = w.create(ROOT, "thread", i);
    (s1, n, KeyStr::new("chat:c/1").unwrap())
}

#[test]
fn a_request_answered_decided_and_settled() {
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let mail = KeyStr::new("mail:m@x").unwrap();
    let r = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/1/7"),
            conv: chat.clone(),
            from_user: None,
            requester: Requester::External,
        },
    );
    let r2 = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("mail:m@x/9"),
            conv: mail.clone(),
            from_user: None,
            requester: Requester::External,
        },
    );
    assert!(w.s.ix.node[&n].flags.contains(Flag::UnansweredRequest));
    assert!(w.s.ix.items[&n].unanswered[&chat].contains(&r));
    assert!(w.s.ix.items[&n].unanswered[&mail].contains(&r2));
    ok(&w);
    let d = w.append(
        n,
        Author::Session(s1),
        EntryBody::Decision {
            settles: [r].into(),
            supersedes: None,
            assumes: BTreeSet::new(),
            evidence: None,
            basis: Vec::new(),
        },
    );
    assert!(w.s.ix.items[&n].owed_reply.contains(&r));
    let a = w.append(
        n,
        Author::Session(s1),
        EntryBody::Said {
            conv: chat.clone(),
            kind: SaidKind::Reply,
            corrects: None,
            voice: false,
        },
    );
    w.emit(Ev::RequestAnswered { request: r, by: a });
    assert!(w.s.ix.items[&n].owed_decision.contains(&r));
    w.emit(Ev::RequestSettled {
        request: r,
        decision: d,
        answer: a,
    });
    assert_eq!(
        w.s.requests[&r].settled,
        Some(Settlement {
            decision: d,
            answer: a
        })
    );
    assert!(w.s.leaving.contains_key(&EntityRef::Request(r)));
    // The other conversation's request is untouched.
    assert!(w.s.ix.items[&n].unanswered[&mail].contains(&r2));
    ok(&w);
    // The answering said, once confirmed, stays: it is the answer of a resident request.
    w.emit(Ev::SaidConfirmed {
        said: a,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    assert!(!w.s.leaving.contains_key(&EntityRef::Said(a)));
    ok(&w);
    // FINISHED_RETAIN later, the request leaves memory, and then the said.
    w.at(w.clock.plus(FINISHED_RETAIN));
    w.emit(Ev::SessionStatus {
        session: s1,
        text: Text("x".into()),
    });
    assert!(!w.s.requests.contains_key(&r));
    assert!(w.s.leaving.contains_key(&EntityRef::Said(a)));
    assert_eq!(w.s.nodes[&n].left, 1);
    // Restore the request, and it is indexed again.
    let meta = w.log.iter().find_map(|e| {
        if let Ev::Appended {
            eid,
            node,
            seq,
            by,
            body,
            ..
        } = &e.ev
            && *eid == r
        {
            Some(EntryMeta {
                node: *node,
                seq: *seq,
                at: e.at,
                by: *by,
                ty: body.entry_type(),
                principal: None,
            })
        } else {
            None
        }
    });
    let row = Request {
        node: n,
        origin: key("chat:c/1/7"),
        conv: chat,
        from_user: None,
        requester: Requester::External,
        answered_by: Some(a),
        settled: Some(Settlement {
            decision: d,
            answer: a,
        }),
        moved_from: None,
    };
    let before = w.s.clone();
    let mut dirty = crate::facts::Tracked::new();
    let evs = restore_tracked(
        &mut w.s,
        w.clock,
        alloc::vec![Restored::Request(r, row, meta.unwrap())],
        &mut dirty,
    );
    assert_eq!(evs.len(), 1);
    w.log.extend(evs);
    w.consistent(&before, &dirty).unwrap();
    assert!(w.s.requests.contains_key(&r));
    assert_eq!(w.s.nodes[&n].left, 0);
    assert!(!w.s.leaving.contains_key(&EntityRef::Said(a)));
    // A second restore of a resident item emits nothing.
    let none = restore_tracked(&mut w.s, w.clock, Vec::new(), &mut dirty);
    assert!(none.is_empty());
    assert_eq!(w.replay(), w.s);
}

// ------------------------------------------------------------------------------------------------
// Waits, questions, commitments, and flags (D.8.3, W1)

#[test]
fn a_wait_suppresses_the_request_flag_until_it_resolves() {
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/1/1"),
            conv: chat,
            from_user: None,
            requester: Requester::External,
        },
    );
    assert!(w.s.ix.node[&n].flags.contains(Flag::UnansweredRequest));
    let until = w.clock.plus(HOUR);
    w.emit(Ev::WaitSet {
        node: n,
        wait: Some(WaitOn::Time(until)),
    });
    assert!(!w.s.ix.node[&n].flags.contains(Flag::UnansweredRequest));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(n)));
    ok(&w);
    w.at(until);
    w.emit(Ev::Fired {
        kind: DeadlineKind::Wait,
        owner: Owner::Node(n),
    });
    assert!(w.s.ix.node[&n].flags.contains(Flag::UnansweredRequest));
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(n)));
    // A time at or before `at` sets the marker at once (A-68).
    w.emit(Ev::WaitSet {
        node: n,
        wait: Some(WaitOn::Time(w.clock)),
    });
    assert!(w.s.nodes[&n].wait.as_ref().unwrap().fired);
    assert_eq!(deadline_of(&w.s, DeadlineKind::Wait, &Owner::Node(n)), None);
}

#[test]
fn a_commitment_past_its_followup_on_a_waiting_node_is_a_status_item() {
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let ack = w.append(
        n,
        Author::Session(s1),
        EntryBody::Said {
            conv: chat.clone(),
            kind: SaidKind::Ack,
            corrects: None,
            voice: false,
        },
    );
    let followup = w.clock.plus(w.s.params.commitment_followup);
    w.emit(Ev::CommitmentOpened {
        c: ack,
        node: n,
        conv: chat,
        followup_at: followup,
    });
    w.emit(Ev::SaidConfirmed {
        said: ack,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    w.emit(Ev::WaitSet {
        node: n,
        wait: Some(WaitOn::Time(followup.plus(DAY))),
    });
    w.at(followup);
    w.emit(Ev::Fired {
        kind: DeadlineKind::Commitment,
        owner: Owner::Entry(ack),
    });
    w.touch(s1);
    assert!(w.s.ix.node[&n].flags.contains(Flag::OpenCommitment));
    assert!(w.s.ix.next.of.contains_key(&Cand::Status(n)));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(n)));
    ok(&w);
    // Fulfilled by a reply in its conversation.
    let reply = w.append(
        n,
        Author::Session(s1),
        EntryBody::Said {
            conv: KeyStr::new("chat:c/1").unwrap(),
            kind: SaidKind::Reply,
            corrects: None,
            voice: false,
        },
    );
    w.emit(Ev::CommitmentFulfilled { c: ack, by: reply });
    assert!(!w.s.ix.next.of.contains_key(&Cand::Status(n)));
    assert!(w.s.leaving.contains_key(&EntityRef::Commitment(ack)));
}

// ------------------------------------------------------------------------------------------------
// The basis (D-4, CS-22)

#[test]
fn a_refuted_lemma_flags_its_citer_and_a_merge_refiles() {
    let mut w = world();
    let s1 = w.session("w");
    let lemma = w.node(ROOT, "lemma");
    let dup = w.node(ROOT, "dup");
    let thm = w.node(ROOT, "thm");
    let d = w.append(
        thm,
        Author::Session(s1),
        EntryBody::Decision {
            settles: BTreeSet::new(),
            supersedes: None,
            assumes: BTreeSet::new(),
            evidence: None,
            basis: alloc::vec![BasisRef::Node {
                id: dup,
                version: None,
                life_at: None,
            }],
        },
    );
    assert!(w.s.ix.cited_by[&BasisTarget::Node(dup)].contains(&d));
    // dup merged into lemma: refiled, not stale.
    w.emit(Ev::NodeClosed {
        node: dup,
        outcome: Outcome::Superseded,
        merged_into: Some(lemma),
        by: None,
    });
    assert!(w.s.ix.cited_by[&BasisTarget::Node(lemma)].contains(&d));
    assert!(!w.s.ix.cited_by.contains_key(&BasisTarget::Node(dup)));
    assert!(!w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    ok(&w);
    // The lemma closes failed: the citer's node is flagged.
    w.emit(Ev::NodeClosed {
        node: lemma,
        outcome: Outcome::Failed,
        merged_into: None,
        by: None,
    });
    assert!(w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    assert!(w.s.ix.items[&thm].stale_basis.contains(&d));
    // A reopen of the lemma clears the staleness (it is under re-examination).
    w.emit(Ev::NodeReopened { node: lemma });
    assert!(!w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    // Reopening dup files the citer back under dup.
    w.emit(Ev::NodeReopened { node: dup });
    assert!(w.s.ix.cited_by[&BasisTarget::Node(dup)].contains(&d));
    assert!(!w.s.ix.cited_by.contains_key(&BasisTarget::Node(lemma)));
    // A retraction of dup makes the reference stale.
    w.append(
        dup,
        Author::Session(s1),
        EntryBody::Retract {
            target: BasisTarget::Node(dup),
        },
    );
    assert!(w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    ok(&w);
    // Superseding the citing decision clears it.
    let d2 = w.append(
        thm,
        Author::Session(s1),
        EntryBody::Decision {
            settles: BTreeSet::new(),
            supersedes: Some(d),
            assumes: BTreeSet::new(),
            evidence: None,
            basis: Vec::new(),
        },
    );
    w.emit(Ev::DecisionSuperseded { d, by: d2 });
    assert!(!w.s.ix.node[&thm].flags.contains(Flag::BasisChanged));
    assert!(w.s.leaving.contains_key(&EntityRef::Decision(d)));
    ok(&w);
}

// ------------------------------------------------------------------------------------------------
// The user side: entries, questions, digest (§8)

#[test]
fn user_entries_questions_and_the_digest() {
    let mut w = world();
    let s1 = w.session("w");
    let e = w.append(
        USER,
        Author::Session(s1),
        EntryBody::FromUser {
            principal: USER_PRINCIPAL,
        },
    );
    assert!(w.s.ix.node[&USER].flags.contains(Flag::Unrouted));
    assert!(w.s.ix.next.of.contains_key(&Cand::Route(e)));
    assert!(w.s.ix.unrouted_by[&s1].contains(&e));
    ok(&w);
    w.emit(Ev::EntryRouted {
        entry: e,
        routing: Routing::NoAction,
        reason: None,
    });
    assert!(!w.s.ix.next.of.contains_key(&Cand::Route(e)));
    assert!(w.s.leaving.contains_key(&EntityRef::UserEntry(e)));
    // A question for the user, defaulted, and a digest entry shown by a to_user.
    let a = w.node(ROOT, "a");
    let q = w.append(
        USER_DECISIONS,
        Author::Session(s1),
        EntryBody::Question(QuestionBody::User {
            source: UqSource::Agent {
                session: s1,
                digest: ReqDigest([0; 32]),
            },
            about: Some(a),
            to: alloc::vec![USER_PRINCIPAL],
            due: w.clock.plus(HOUR),
            has_default: true,
        }),
    );
    assert!(
        w.s.ix.pending_uq_by[&USER_PRINCIPAL]
            .iter()
            .any(|r| r.q == q)
    );
    // The rank follows the about node's priority.
    let patch = NodePatch {
        priority: Some(5),
        ..NodePatch::default()
    };
    w.emit(Ev::NodeUpdated { node: a, patch });
    assert!(
        w.s.ix.pending_uq_by[&USER_PRINCIPAL]
            .iter()
            .any(|r| r.q == q && r.neg_priority == -5)
    );
    ok(&w);
    w.at(w.clock.plus(HOUR));
    w.emit(Ev::Fired {
        kind: DeadlineKind::UserQuestion,
        owner: Owner::Entry(q),
    });
    w.emit(Ev::UqDefaulted { q });
    w.touch(s1);
    assert!(w.s.ix.pending_uq_by.is_empty());
    let dg = w.append(
        USER_DIGEST,
        Author::Server,
        EntryBody::System(SystemNote::Digest {
            item: DigestItem::DefaultApplied { question: q },
            to: USER_PRINCIPAL,
        }),
    );
    assert_eq!(
        w.s.leaving[&EntityRef::Digest(dg)],
        w.clock.plus(DIGEST_RETAIN)
    );
    ok(&w);
    let tu = w.append(
        USER,
        Author::Session(s1),
        EntryBody::ToUser {
            kind: ToUserKind::Result,
            verbatim: true,
            re: [dg].into(),
            principal: USER_PRINCIPAL,
        },
    );
    w.emit(Ev::DigestShown { entry: dg, by: tu });
    assert_eq!(
        w.s.leaving[&EntityRef::Digest(dg)],
        w.clock.plus(FINISHED_RETAIN)
    );
    ok(&w);
    // A digest entry left unshown past DIGEST_RETAIN is counted in digest_left.
    let dg2 = w.append(
        USER_DIGEST,
        Author::Server,
        EntryBody::System(SystemNote::Digest {
            item: DigestItem::DefaultApplied { question: q },
            to: USER_PRINCIPAL,
        }),
    );
    w.at(w.clock.plus(DIGEST_RETAIN));
    w.emit(Ev::SessionStatus {
        session: s1,
        text: Text("x".into()),
    });
    assert!(!w.s.digest.contains_key(&dg2));
    assert_eq!(w.s.digest_left[&USER_PRINCIPAL], 1);
    assert_eq!(w.replay(), w.s);
}

// ------------------------------------------------------------------------------------------------
// Notes and retries (CS-8, CS-17)

#[test]
fn notes_and_op_records_leave_at_their_time() {
    let mut w = world();
    let s1 = w.session("w");
    let s2 = w.session("w");
    let a = w.node(ROOT, "a");
    let until = w.clock.plus(HOUR);
    let x = w.append(
        a,
        Author::Session(s1),
        EntryBody::Note {
            to: Some(Addressee::Session(s2)),
            until: Some(until),
            recipients: [s2].into(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        },
    );
    assert!(w.s.ix.inbox[&s2].contains(&x));
    w.emit(Ev::OpRecorded {
        class: class("w"),
        op: OpId::new("op-1").unwrap(),
        digest: ReqDigest([1; 32]),
        session: s1,
        expires_at: w.clock.plus(MINUTE),
        outcome: Err(crate::error::Error {
            code: crate::error::ErrorCode::OpIdConflict,
            hint: crate::error::Hint::OpIdConflict,
        }),
    });
    ok(&w);
    w.at(until);
    w.emit(Ev::SessionStatus {
        session: s1,
        text: Text("x".into()),
    });
    assert!(w.s.notes.is_empty());
    assert!(w.s.ops.is_empty());
    assert!(!w.s.entries.contains_key(&x));
}

// ------------------------------------------------------------------------------------------------
// Anomalies (D.7.5)

#[test]
fn an_event_naming_a_missing_object_is_an_anomaly_and_changes_nothing_else() {
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    let missing = NodeId(99);
    let bad = [
        Ev::NodeReopened { node: a },
        Ev::NodeClosed {
            node: missing,
            outcome: Outcome::Done,
            merged_into: None,
            by: None,
        },
        Ev::OfferSet { node: a, on: false },
        Ev::HoldEnded {
            token: Token {
                node: a,
                fence: Fence(7),
            },
            end: HoldEnd::Released,
        },
        Ev::RequestAnswered {
            request: Eid(99),
            by: Eid(1),
        },
        Ev::IntentClosed {
            node: a,
            end: IntentEnd::ResolvedByClose,
        },
        Ev::Touched {
            session: SessionId(9),
            silence_at: T0,
            retire_at: T0,
        },
        Ev::NextCalled {
            session: s1,
            outcome: NextOutcome::Item,
            open: true,
            class_window_end: T0,
        },
    ];
    for ev in bad {
        let before = w.s.clone();
        w.emit_raw(ev.clone());
        assert_eq!(w.s.anomalies, before.anomalies + 1, "{ev:?}");
        let mut after = w.s.clone();
        after.anomalies = before.anomalies;
        after.counters.events = before.counters.events;
        after.last_now = before.last_now;
        assert_eq!(after, before, "{ev:?}");
    }
    // An id the event carries still advances its counter.
    let next = w.s.counters.next_node;
    w.emit_raw(Ev::NodeCreated {
        id: NodeId(next.0 + 5),
        parent: ROOT,
        name: name("far"),
        origin: Origin::Agent(s1),
        init: init("far"),
    });
    assert_eq!(w.s.counters.next_node, NodeId(next.0 + 6));
    assert!(violates(&w.s, w.clock, Inv::Total));
}

// ------------------------------------------------------------------------------------------------
// Pauses and dependencies (CS-16)

#[test]
fn a_pause_and_a_dependency_hold_candidates_back() {
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    let x = w.node(a, "x");
    let y = w.node(ROOT, "y");
    w.emit(Ev::OfferSet { node: x, on: true });
    let part = w.s.ix.next.of[&Cand::Node(x)].part.clone();
    assert!(w.s.ix.next.parts[&part].iter().any(|k| k.id == x.0));
    w.emit(Ev::Paused {
        scope: a,
        until: w.clock.plus(HOUR),
        ttl: w.s.params.pause_ttl,
        by: s1,
        class: class("w"),
        renewed: false,
    });
    assert_eq!(w.s.ix.next.held[&Cand::Node(x)].paused_by, Some(a));
    ok(&w);
    w.emit(Ev::Unpaused { scope: a });
    assert!(!w.s.ix.next.held.contains_key(&Cand::Node(x)));
    let patch = NodePatch {
        depends_on: Some([y].into()),
        ..NodePatch::default()
    };
    w.emit(Ev::NodeUpdated { node: x, patch });
    assert!(w.s.ix.next.held[&Cand::Node(x)].depends);
    w.emit(Ev::NodeClosed {
        node: y,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    assert!(!w.s.ix.next.held.contains_key(&Cand::Node(x)));
    ok(&w);
}

// ------------------------------------------------------------------------------------------------
// The state checks reject a hand-built violation of each (D.13.2 WP-D2)

/// A settled state with one of most things in it.
fn rich() -> World {
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let r = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/1/1"),
            conv: chat.clone(),
            from_user: None,
            requester: Requester::External,
        },
    );
    let _ = r;
    let t = w.take(n, s1);
    let _ = t;
    let a = w.node(ROOT, "a");
    w.emit(Ev::OfferSet { node: a, on: true });
    let q = w.append(
        USER_DECISIONS,
        Author::Session(s1),
        EntryBody::Question(QuestionBody::User {
            source: UqSource::Agent {
                session: s1,
                digest: ReqDigest([0; 32]),
            },
            about: Some(a),
            to: alloc::vec![USER_PRINCIPAL],
            due: w.clock.plus(HOUR),
            has_default: false,
        }),
    );
    let _ = q;
    ok(&w);
    w
}

#[test]
fn every_state_check_rejects_a_violating_state() {
    let w = rich();
    let now = w.clock;
    let n = *w.s.names.get(&(ROOT, name("thread"))).expect("thread");
    let a = *w.s.names.get(&(ROOT, name("a"))).expect("a");
    let (t, _) =
        w.s.holds
            .iter()
            .next()
            .map(|(t, h)| (*t, h.clone()))
            .unwrap();
    let q = *w.s.uquestions.keys().next().unwrap();
    let r = *w.s.requests.keys().next().unwrap();
    let mut cases: Vec<(Inv, State)> = Vec::new();
    let mut s = w.s.clone();
    s.anomalies = 1;
    cases.push((Inv::Total, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&n).unwrap().capacity = 0;
    cases.push((Inv::H1, s));
    let mut s = w.s.clone();
    s.params.hold_per_session = 0;
    cases.push((Inv::H2, s));
    let mut s = w.s.clone();
    s.sessions.values_mut().for_each(|x| x.silent = true);
    cases.push((Inv::H2, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&n).unwrap().fence = Fence(0);
    cases.push((Inv::H3, s));
    let mut s = w.s.clone();
    s.holds.get_mut(&t).unwrap().renews = 99;
    cases.push((Inv::H5, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&n).unwrap().life = Life::Closed(crate::state::Closure {
        at: now,
        outcome: Outcome::Done,
        merged_into: None,
        upheld: None,
        corrected_from: None,
        by: None,
    });
    cases.push((Inv::H6, s));
    let mut s = w.s.clone();
    s.ops.insert(
        (class("w"), OpId::new("o").unwrap()),
        crate::state::OpRecord {
            digest: ReqDigest([0; 32]),
            session: SessionId(1),
            expires_at: s.last_now,
        },
    );
    cases.push((Inv::O1, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&n).unwrap().version = Version(0);
    cases.push((Inv::O2, s));
    let mut s = w.s.clone();
    s.ix.unsettled_requests.clear();
    cases.push((Inv::R1, s));
    let mut s = w.s.clone();
    let k = KeyStr::new("chat:c/1").unwrap();
    s.nodes.get_mut(&a).unwrap().keys.insert(k, NormVer(1));
    cases.push((Inv::K1, s));
    let mut s = w.s.clone();
    s.nodes
        .get_mut(&a)
        .unwrap()
        .keys
        .insert(KeyStr::new("user:e999").unwrap(), NormVer(0));
    cases.push((Inv::K2, s));
    let mut s = w.s.clone();
    s.aliases.insert((ROOT, name("a")), a);
    cases.push((Inv::K3, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&a).unwrap().place = Some(crate::state::Place {
        parent: a,
        name: name("a"),
    });
    cases.push((Inv::K4, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&a).unwrap().life = Life::Closed(crate::state::Closure {
        at: now,
        outcome: Outcome::Done,
        merged_into: Some(n),
        upheld: None,
        corrected_from: None,
        by: None,
    });
    cases.push((Inv::K5, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&a).unwrap().role = Role::Series(crate::state::Series {
        repeat: crate::state::Repeat {
            cron_text: Text::default(),
            cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
            tz: forum_cron::TzTable {
                name: alloc::string::String::from("UTC"),
                tzdb_version: alloc::string::String::new(),
                initial: 0,
                transitions: Vec::new(),
            },
            run_within: HOUR,
        },
        last_missed: false,
    });
    cases.push((Inv::K6, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&SYS).unwrap().capacity = 1;
    cases.push((Inv::K8, s));
    let mut s = w.s.clone();
    let mut r2 = s.requests[&r].clone();
    r2.node = a;
    s.requests.insert(Eid(9_999), r2);
    cases.push((Inv::Q1, s));
    let mut s = w.s.clone();
    s.signals.insert(
        crate::state::SignalKey {
            kind: crate::types::SignalKind::Duplicate,
            a: n,
            b: a,
        },
        a,
    );
    cases.push((Inv::S1, s));
    let mut s = w.s.clone();
    s.user_entries.insert(
        Eid(9_998),
        crate::state::UserEntry {
            session: SessionId(1),
            principal: USER_PRINCIPAL,
            at: now,
            routing: Routing::Unrouted,
        },
    );
    cases.push((Inv::N1, s));
    let mut s = w.s.clone();
    s.requests.get_mut(&r).unwrap().settled = Some(Settlement {
        decision: Eid(1),
        answer: Eid(1),
    });
    cases.push((Inv::Settled, s));
    let mut s = w.s.clone();
    s.ix.node.get_mut(&a).unwrap().flags = FlagSet(Flag::Objection.bit());
    cases.push((Inv::F, s));
    let mut s = w.s.clone();
    s.classes.insert(
        class("z"),
        crate::state::ClassActivity {
            caps_seen: BTreeMap::new(),
        },
    );
    cases.push((Inv::E1, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&a).unwrap().wait = Some(crate::state::Wait {
        on: WaitOn::Node(n),
        set_at: now,
        fired: false,
    });
    cases.push((Inv::W1, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&a).unwrap().life = Life::Closed(crate::state::Closure {
        at: now,
        outcome: Outcome::Missed,
        merged_into: None,
        upheld: None,
        corrected_from: None,
        by: None,
    });
    s.nodes.get_mut(&a).unwrap().checkpoints = 1;
    cases.push((Inv::R2, s));
    let mut s = w.s.clone();
    s.digest.insert(
        Eid(9_997),
        crate::state::DigestEntry {
            item: DigestItem::Lapsed { question: q },
            to: PrincipalId(77),
            state: crate::state::DigestState::Pending,
        },
    );
    cases.push((Inv::D1, s));
    let mut s = w.s.clone();
    s.ix.deadlines
        .retain(|k| k.kind != DeadlineKind::UserQuestion);
    s.ix.deadline_at
        .retain(|k, _| k.0 != DeadlineKind::UserQuestion);
    cases.push((Inv::M1, s));
    let mut s = w.s.clone();
    s.uquestions.get_mut(&q).unwrap().to = alloc::vec![PrincipalId(5)];
    cases.push((Inv::U1, s));
    let mut s = w.s.clone();
    let e = *s.entries.keys().next().unwrap();
    s.entries.get_mut(&e).unwrap().at = Time(u64::MAX);
    cases.push((Inv::T1, s));
    let mut s = w.s.clone();
    s.leaving
        .insert(EntityRef::Request(r), s.last_now.plus(HOUR));
    cases.push((Inv::T3, s));
    let mut s = w.s.clone();
    s.ix.deadlines.clear();
    cases.push((Inv::T4, s));
    let mut s = w.s.clone();
    s.nodes.get_mut(&a).unwrap().depends_on.insert(a);
    cases.push((Inv::T5, s));
    let mut s = w.s.clone();
    s.params.tree_max_depth = 0;
    cases.push((Inv::L1, s));
    let mut s = w.s.clone();
    s.pauses.insert(
        a,
        crate::state::Pause {
            until: now.plus(DAY),
            last_renewed: now,
            ttl: HOUR,
            by: SessionId(1),
            class: class("w"),
        },
    );
    cases.push((Inv::P1, s));
    for (inv, s) in cases {
        let v = check_invariants(&s, now).expect_err("a violation");
        assert!(
            v.iter().any(|x| x.inv == inv),
            "{inv:?} not reported: {v:?}"
        );
    }
}

#[test]
fn check_startup_reports_k8() {
    // A-118: the reserved nodes are checked at every startup.
    let w = world();
    let mut s = w.s.clone();
    s.nodes
        .get_mut(&PEOPLE)
        .unwrap()
        .keys
        .insert(KeyStr::new("chat:x").unwrap(), NormVer(1));
    let v = check_startup(&s, w.clock).expect_err("K8");
    assert!(v.iter().any(|x| x.inv == Inv::K8));
    let mut s = w.s.clone();
    s.nodes.get_mut(&ROOT).unwrap().life = Life::Closed(crate::state::Closure {
        at: T0,
        outcome: Outcome::Done,
        merged_into: None,
        upheld: None,
        corrected_from: None,
        by: None,
    });
    assert!(check_startup(&s, w.clock).is_err_and(|v| v.iter().any(|x| x.inv == Inv::K8)));
}

#[test]
fn check_keys_asks_the_shell_oracle() {
    let w = rich();
    assert!(check_keys(&w.s, &|_, _| true).is_ok());
    let v = check_keys(&w.s, &|k, _| k.as_str() != "chat:c/1").expect_err("K2");
    assert!(v.iter().all(|x| x.inv == Inv::K2));
}

#[test]
fn the_recount_of_a_rich_state_equals_its_indices() {
    let w = rich();
    assert_eq!(recount(&w.s, w.clock).next, w.s.ix.next);
    assert_eq!(recount_deadlines(&w.s), w.s.ix.deadlines);
    assert!(check_startup(&w.s, w.clock).is_ok());
}

#[test]
fn a_team_is_a_container_but_its_flags_bring_it_to_next() {
    // A-113: a child of @teams gets no offer tag at a hold end, and only its flags make it a
    // candidate; moved out of @teams it is an ordinary node.
    let mut w = world();
    let s1 = w.session("w");
    let team = w.node(TEAMS, "payments");
    let t = w.take(team, s1);
    w.at(T0.plus(MINUTE));
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::Expired,
    });
    assert!(w.s.nodes[&team].tags.is_empty());
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(team)));
    // A flag (an objection) makes it a candidate, as a series' flags would.
    w.append(team, Author::Session(s1), EntryBody::Object);
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(team)));
    ok(&w);
    // A hold that ends after it moved out of @teams tags it (judged at its new place, D.8.20).
    let t2 = w.take(team, s1);
    w.emit(Ev::NodeMoved {
        node: team,
        parent: ROOT,
        name: name("payments"),
    });
    w.emit(Ev::HoldEnded {
        token: t2,
        end: HoldEnd::EndedByClose(crate::event::EndCause::Move),
    });
    assert!(w.s.nodes[&team].tags.iter().any(|l| l.as_str() == OFFER));
    assert_eq!(w.s.nodes[&team].attempts, 2);
}

#[test]
fn the_silence_firing_reads_the_pending_saids_of_the_session() {
    // 1.3 (D.4.10): open_saids_by holds the pending and the unconfirmed entries; the Silence arm
    // turns only the pending ones unconfirmed.
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let a = w.append(
        n,
        Author::Session(s1),
        EntryBody::Said {
            conv: chat.clone(),
            kind: SaidKind::Reply,
            corrects: None,
            voice: false,
        },
    );
    let b = w.append(
        n,
        Author::Session(s1),
        EntryBody::Said {
            conv: chat,
            kind: SaidKind::Ack,
            corrects: None,
            voice: false,
        },
    );
    w.emit(Ev::SaidUnconfirmed { said: a });
    assert_eq!(w.s.ix.open_saids_by[&s1], [a, b].into());
    assert_eq!(crate::index::pending_saids(&w.s, s1), alloc::vec![b]);
    assert!(w.s.ix.node[&n].flags.contains(Flag::UnconfirmedSaid));
    w.emit(Ev::SaidConfirmed {
        said: b,
        via: crate::event::ConfirmVia::Release,
        link: None,
    });
    assert!(crate::index::pending_saids(&w.s, s1).is_empty());
    // A retired session leaves its index entry; its unconfirmed entry keeps its flag.
    let silence = w.s.sessions[&s1].silence_at;
    w.at(silence);
    w.emit(Ev::Fired {
        kind: DeadlineKind::Silence,
        owner: Owner::Session(s1),
    });
    let retire = w.s.sessions[&s1].retire_at;
    w.at(retire);
    w.emit(Ev::Fired {
        kind: DeadlineKind::Retire,
        owner: Owner::Session(s1),
    });
    w.emit(Ev::SessionRetired { session: s1 });
    assert!(!w.s.ix.open_saids_by.contains_key(&s1));
    assert!(w.s.ix.node[&n].flags.contains(Flag::UnconfirmedSaid));
}

// ------------------------------------------------------------------------------------------------
// Base effects of D.7.4 that the recount does not see

#[test]
fn hold_ends_count_attempts_supply_and_lost_time() {
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    // An expired hold without a checkpoint is an attempt, and its time is lost (CS-31).
    let t = w.take(a, s1);
    w.at(T0.plus(w.s.params.lease_first));
    w.emit(Ev::Fired {
        kind: DeadlineKind::Lease,
        owner: Owner::Hold(t),
    });
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::Expired,
    });
    let d = day_of(w.clock);
    assert_eq!(w.s.nodes[&a].attempts, 1);
    assert_eq!(w.s.stats.days[&d].lost_ms, w.s.params.lease_first.0);
    assert!(w.s.nodes[&a].tags.iter().any(|l| l.as_str() == OFFER));
    // A checkpointed hold that expires is supply, and the checkpoint resets the attempts.
    w.touch(s1);
    let t2 = w.take(a, s1);
    w.append(
        a,
        Author::Session(s1),
        EntryBody::Checkpoint {
            token: t2,
            release: None,
            pr_named: None,
            handed_off_until: None,
        },
    );
    assert_eq!(w.s.nodes[&a].attempts, 0);
    assert!(w.s.holds[&t2].checkpointed);
    w.at(w.clock.plus(MINUTE));
    w.emit(Ev::HoldEnded {
        token: t2,
        end: HoldEnd::Expired,
    });
    assert_eq!(w.s.nodes[&a].attempts, 0);
    assert_eq!(w.s.stats.days[&d].supply_ms, MINUTE.0);
    // too_big after too_big.attempts expiries without a checkpoint.
    for _ in 0..w.s.params.too_big_attempts {
        let t = w.take(a, s1);
        w.emit(Ev::HoldEnded {
            token: t,
            end: HoldEnd::EndedByClose(crate::event::EndCause::Move),
        });
    }
    assert!(w.s.ix.node[&a].flags.contains(Flag::TooBig));
    w.emit(Ev::NodeClosed {
        node: a,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    // too_big holds on open nodes only (D.8.3); a Done close pushes the held minutes.
    assert!(!w.s.ix.node[&a].flags.contains(Flag::TooBig));
    assert_eq!(
        w.s.stats.done_minutes,
        alloc::vec![w.s.nodes[&a].held_ms / 60_000]
    );
    ok(&w);
}

#[test]
fn the_done_ring_keeps_the_last_101() {
    let mut w = world();
    for i in 0..(crate::health::DONE_RING + 3) {
        let n = w.node(ROOT, &alloc::format!("n{i}"));
        w.emit(Ev::NodeClosed {
            node: n,
            outcome: Outcome::Done,
            merged_into: None,
            by: None,
        });
    }
    assert_eq!(w.s.stats.done_minutes.len(), crate::health::DONE_RING);
}

#[test]
fn sessions_record_profiles_and_starts_and_prune_them() {
    let mut w = world();
    let s1 = w.session("w");
    let key = (BTreeSet::new(), 33_u8);
    assert_eq!(w.s.stats.profiles[&key].last_seen, T0);
    assert_eq!(w.s.stats.starts[&minute_of(T0)], 1);
    w.emit(Ev::SessionHello {
        session: s1,
        caps: None,
        horizon: Some(64),
        intent: None,
        principal: None,
    });
    assert_eq!(
        w.s.stats.profiles[&(BTreeSet::new(), 7)].max_horizon,
        Some(64)
    );
    // A session opened after coverage.window and capacity.window prunes both.
    w.at(T0.plus(w.s.params.coverage_window).plus(MINUTE));
    let s2 = w.session("w");
    assert!(!w.s.stats.starts.contains_key(&minute_of(T0)));
    assert!(!w.s.stats.profiles.contains_key(&(BTreeSet::new(), 7)));
    assert!(w.s.sessions.contains_key(&s2));
}

#[test]
fn an_open_next_makes_its_class_active_until_its_window_ends() {
    let mut w = world();
    let s1 = w.session("w");
    let end = w.clock.plus(w.s.params.refusal_window);
    w.emit(Ev::NextCalled {
        session: s1,
        outcome: NextOutcome::Nothing,
        open: true,
        class_window_end: end,
    });
    assert_eq!(w.s.classes[&class("w")].caps_seen[&BTreeSet::new()], end);
    assert_eq!(w.s.stats.days[&day_of(T0)].next_empty, 1);
    // A queried next records no profile.
    w.emit(Ev::NextCalled {
        session: s1,
        outcome: NextOutcome::NothingForYou,
        open: false,
        class_window_end: end,
    });
    assert_eq!(w.s.stats.days[&day_of(T0)].next_empty_for_you, 1);
    w.at(end);
    w.touch(s1);
    w.emit(Ev::Fired {
        kind: DeadlineKind::RefusalExpiry,
        owner: Owner::Class(class("w")),
    });
    assert!(w.s.classes.is_empty());
}

#[test]
fn refusals_are_one_entry_per_session_and_expire_one_by_one() {
    let mut w = world();
    let s1 = w.session("w");
    let s2 = w.session("w");
    let a = w.node(ROOT, "a");
    let p = w.s.params.clone();
    for (s, d) in [(s1, 0), (s2, 1), (s1, 2)] {
        w.at(T0.plus(Dur(d * MINUTE.0)));
        w.emit(Ev::Refused {
            node: a,
            class: class("w"),
            session: s,
            backoff_until: w.clock.plus(p.refusal_backoff),
            expires_at: w.clock.plus(p.refusal_ttl),
            quorum: 2,
        });
    }
    let r = &w.s.refusals[&(a, class("w"))];
    assert_eq!(r.by.len(), 2);
    assert_eq!(r.quorum, 2);
    assert!(w.s.ix.refused[&a].contains(&class("w")));
    // s2's entry expires first: s1 renewed its own.
    let first = T0.plus(Dur(MINUTE.0)).plus(p.refusal_ttl);
    assert_eq!(
        deadline_of(
            &w.s,
            DeadlineKind::RefusalExpiry,
            &Owner::Refusal(a, class("w"))
        ),
        Some(first)
    );
    w.at(first);
    w.emit(Ev::RefusalExpired {
        node: a,
        class: class("w"),
    });
    assert_eq!(w.s.refusals[&(a, class("w"))].by.len(), 1);
    w.emit(Ev::RefusalsCleared {
        node: a,
        by: Eid(1),
    });
    assert!(w.s.refusals.is_empty());
    assert!(w.s.ix.refused.is_empty());
}

#[test]
fn holder_and_holders_notes() {
    let mut w = world();
    let s1 = w.session("w");
    let s2 = w.session("w");
    let a = w.node(ROOT, "a");
    let until = w.clock.plus(HOUR);
    // A holder: note with nobody holding waits for the next taker (D.8.16).
    let x = w.append(
        a,
        Author::Session(s1),
        EntryBody::Note {
            to: Some(Addressee::Holder(a)),
            until: Some(until),
            recipients: BTreeSet::new(),
            pending_holder: true,
            expects: None,
            reply_to: None,
        },
    );
    assert!(w.s.ix.pending_holder[&a].contains(&x));
    w.take(a, s2);
    w.emit(Ev::NoteBound {
        note: x,
        session: s2,
    });
    assert!(!w.s.ix.pending_holder.contains_key(&a));
    assert!(w.s.ix.inbox[&s2].contains(&x));
    w.emit(Ev::InboxRead {
        session: s2,
        notes: [x].into(),
    });
    assert!(!w.s.ix.inbox.contains_key(&s2));
    // A holders: note records no recipients; reading it moves the scope's cursor (D-6).
    let y = w.append(
        ROOT,
        Author::Session(s1),
        EntryBody::Note {
            to: Some(Addressee::Holders(ROOT)),
            until: Some(until),
            recipients: BTreeSet::new(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        },
    );
    assert!(w.s.ix.branch_notes[&ROOT].contains(&y));
    w.emit(Ev::InboxRead {
        session: s2,
        notes: [y].into(),
    });
    assert_eq!(w.s.sessions[&s2].branch_seen[&ROOT], y);
    assert!(w.s.notes[&y].read_by.is_empty());
    ok(&w);
}

#[test]
fn an_occurrence_waits_for_its_slot_and_its_series_is_flagged_when_missed() {
    let mut w = world();
    let mut si = init("series");
    si.role = Role::Series(crate::state::Series {
        repeat: crate::state::Repeat {
            cron_text: Text("0 7 * * *".into()),
            cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
            tz: forum_cron::TzTable {
                name: alloc::string::String::from("UTC"),
                tzdb_version: alloc::string::String::new(),
                initial: 0,
                transitions: Vec::new(),
            },
            run_within: HOUR,
        },
        last_missed: false,
    });
    let x = w.create(ROOT, "series", si);
    let slot = T0.plus(HOUR);
    let mut oi = init("run");
    oi.due = Some(slot.plus(HOUR));
    oi.role = Role::Occurrence(crate::state::Occurrence {
        series: x,
        slot,
        window_end: slot.plus(HOUR),
        phase: crate::state::OccPhase::Waiting,
        window_fired: false,
    });
    let o = w.create(x, "run", oi);
    assert!(w.s.ix.series_open[&x].contains(&o));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(o)));
    ok(&w);
    w.at(slot);
    w.emit(Ev::Fired {
        kind: DeadlineKind::OccurrenceSlot,
        owner: Owner::Node(o),
    });
    w.emit(Ev::OccurrenceAvailable { node: o });
    // An available occurrence is a candidate, in tier 0 while its window is open.
    assert_eq!(w.s.ix.next.of[&Cand::Node(o)].key.tier, 0);
    assert_eq!(w.s.since[&Cand::Node(o)], slot);
    w.emit(Ev::NodeClosed {
        node: o,
        outcome: Outcome::Missed,
        merged_into: None,
        by: None,
    });
    w.emit(Ev::SeriesOutcome {
        series: x,
        missed: true,
    });
    assert!(w.s.ix.node[&x].flags.contains(Flag::Missed));
    assert!(!w.s.ix.series_open.contains_key(&x));
    // Removing the repeat makes the series a plain node: no longer missed (open-only on series).
    let patch = NodePatch {
        repeat: Change::Clear,
        ..NodePatch::default()
    };
    w.emit(Ev::NodeUpdated { node: x, patch });
    assert!(!w.s.ix.node[&x].flags.contains(Flag::Missed));
    // The closed occurrence of a series that is no longer one is unlocked and can be detached.
    w.emit(Ev::OccurrenceDetached { node: o });
    assert_eq!(w.s.nodes[&o].role, Role::Plain);
}

#[test]
fn approvals_count_per_version_and_principals_intern() {
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    w.append(
        a,
        Author::Session(s1),
        EntryBody::Approve {
            version: Version(1),
        },
    );
    w.append(
        a,
        Author::Session(s1),
        EntryBody::Approve {
            version: Version(1),
        },
    );
    assert_eq!(w.s.approvals[&(a, Version(1))], 2);
    let p = w.principal("mail:x@y");
    assert_eq!(p, PrincipalId(1));
    assert_eq!(w.s.ix.principal_key[&p].as_str(), "mail:x@y");
    // The same key twice is an anomaly.
    w.emit_raw(Ev::PrincipalSeen {
        id: PrincipalId(2),
        key: PrincipalKey::new("mail:x@y").unwrap(),
    });
    assert_eq!(w.s.anomalies, 1);
    assert_eq!(w.s.counters.next_principal, PrincipalId(3));
}

#[test]
fn a_principals_message_times_keep_the_gap_window() {
    // D.4.2: at each message of p, the times before at − user_gap.window are dropped.
    let mut w = world();
    let s1 = w.session("w");
    w.append(
        USER,
        Author::Session(s1),
        EntryBody::FromUser {
            principal: USER_PRINCIPAL,
        },
    );
    let window = w.s.params.user_gap_window;
    w.at(T0.plus(window).plus(MINUTE));
    w.touch(s1);
    w.append(
        USER,
        Author::Session(s1),
        EntryBody::FromUser {
            principal: USER_PRINCIPAL,
        },
    );
    assert_eq!(w.s.user_msgs[&USER_PRINCIPAL], alloc::vec![w.clock]);
}

#[test]
fn a_reopen_sets_due_fired_from_its_own_time() {
    // D.7.4: `due_fired := due <= at` at NodeReopened, so no Due key lies in the past.
    let mut w = world();
    let mut i = init("a");
    i.due = Some(T0.plus(HOUR));
    let a = w.create(ROOT, "a", i);
    w.emit(Ev::NodeClosed {
        node: a,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    w.at(T0.plus(DAY));
    w.emit(Ev::NodeReopened { node: a });
    assert!(w.s.nodes[&a].due_fired);
    assert!(w.s.ix.node[&a].flags.contains(Flag::Overdue));
    ok(&w);
}

#[test]
fn a_node_counts_its_demand_once_per_opening() {
    // A-41: a node's estimate enters the demand when it first becomes a candidate after it was
    // created or reopened, however often it is handed off.
    let mut w = world();
    let s1 = w.session("w");
    let mut i = init("a");
    i.est = Some(40);
    i.tags.insert(Label::new(OFFER).unwrap());
    let a = w.create(ROOT, "a", i);
    let d = day_of(T0);
    assert_eq!(w.s.stats.days[&d].demand_min, 40);
    // Handed off: taken and released, a candidate again, counted once.
    let t = w.take(a, s1);
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::Released,
    });
    assert_eq!(w.s.stats.days[&d].demand_min, 40);
    // Closed, reopened, and offered again: a candidate again after the close, counted again.
    w.emit(Ev::NodeClosed {
        node: a,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    assert!(!w.s.stats.demand_counted.contains(&a));
    w.emit(Ev::NodeReopened { node: a });
    w.emit(Ev::OfferSet { node: a, on: true });
    assert_eq!(w.s.stats.days[&d].demand_min, 80);
    // D.7.4 counts at the candidate's appearance: a node that stays a candidate across its close
    // and reopen (closed with a flag of CLOSED_TAKE) is not counted again (see the report). 1.9:
    // an unanswered request, since an objection makes a closed node a review item instead.
    w.append(
        a,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/9/1"),
            conv: KeyStr::new("chat:c/9").unwrap(),
            from_user: None,
            requester: Requester::External,
        },
    );
    w.emit(Ev::NodeClosed {
        node: a,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    w.emit(Ev::NodeReopened { node: a });
    assert_eq!(w.s.stats.days[&d].demand_min, 80);
}

#[test]
fn o2_counters_move_by_their_rule_for_every_node_event() {
    // §26 O2 (A-3, CS-34): version by 1 exactly for a key or link added or removed, a close, a
    // reopen, and an update of title, body, fields, files, or status; digest_version exactly for
    // the digest; content_version follows version exactly for title, body, fields, or files; seq
    // per entry; fence per issued hold.
    let mut w = world();
    let s1 = w.session("w");
    let a = w.node(ROOT, "a");
    let b = w.node(ROOT, "b");
    let text = |t: &str| Text(t.into());
    let mut fields = BTreeMap::new();
    fields.insert(text("k"), text("v"));
    // (event, version, digest_version, content_version follows, seq, fence)
    let cases: Vec<(Ev, u64, u64, bool, u64, u64)> = alloc::vec![
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    title: Some(text("t")),
                    ..NodePatch::default()
                }
            },
            1,
            0,
            true,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    body: Some(text("b")),
                    ..NodePatch::default()
                }
            },
            1,
            0,
            true,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    fields: Some(fields.clone()),
                    ..NodePatch::default()
                }
            },
            1,
            0,
            true,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    files: Some(fields.clone()),
                    ..NodePatch::default()
                }
            },
            1,
            0,
            true,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    status: Some(text("s")),
                    ..NodePatch::default()
                }
            },
            1,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    digest: Some(text("d")),
                    ..NodePatch::default()
                }
            },
            0,
            1,
            false,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    agreements: Some(text("g")),
                    ..NodePatch::default()
                }
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    kind: Change::Set(Label::new("k").unwrap()),
                    ..NodePatch::default()
                }
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    tags: Some([Label::new("x").unwrap()].into()),
                    ..NodePatch::default()
                }
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    capacity: Some(2),
                    priority: Some(3),
                    est: Change::Set(9),
                    ..NodePatch::default()
                }
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    needs: Some([Label::new("y").unwrap()].into()),
                    due: Change::Set(T0.plus(DAY)),
                    ..NodePatch::default()
                }
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeUpdated {
                node: a,
                patch: NodePatch {
                    depends_on: Some([b].into()),
                    owners: Some([b].into()),
                    ..NodePatch::default()
                }
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::KeyAdded {
                node: a,
                key: key("lean:t")
            },
            1,
            0,
            false,
            0,
            0
        ),
        (
            Ev::KeyRemoved {
                node: a,
                key: KeyStr::new("lean:t").unwrap()
            },
            1,
            0,
            false,
            0,
            0
        ),
        (
            Ev::LinkAdded {
                node: a,
                rel: crate::types::LinkRel::DerivedFrom,
                to: b
            },
            1,
            0,
            false,
            0,
            0
        ),
        (Ev::OfferSet { node: a, on: true }, 0, 0, false, 0, 0),
        (
            Ev::WaitSet {
                node: a,
                wait: Some(WaitOn::Time(T0.plus(HOUR)))
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeMoved {
                node: a,
                parent: b,
                name: name("a")
            },
            0,
            0,
            false,
            0,
            0
        ),
        (
            Ev::NodeClosed {
                node: a,
                outcome: Outcome::Done,
                merged_into: None,
                by: None,
            },
            1,
            0,
            false,
            0,
            0
        ),
        (Ev::NodeReopened { node: a }, 1, 0, false, 0, 0),
    ];
    for (ev, dv, dd, follows, ds, df) in cases {
        let before = w.s.nodes[&a].clone();
        w.emit(ev.clone());
        let after = &w.s.nodes[&a];
        assert_eq!(after.version.0, before.version.0 + dv, "{ev:?}");
        assert_eq!(
            after.digest_version.0,
            before.digest_version.0 + dd,
            "{ev:?}"
        );
        if follows {
            assert_eq!(after.content_version, after.version, "{ev:?}");
        } else {
            assert_eq!(after.content_version, before.content_version, "{ev:?}");
        }
        assert_eq!(after.seq.0, before.seq.0 + ds, "{ev:?}");
        assert_eq!(after.fence.0, before.fence.0 + df, "{ev:?}");
    }
    // An entry moves seq by 1, a hold fence by 1, and neither moves version.
    let before = w.s.nodes[&a].clone();
    w.append(a, Author::Session(s1), EntryBody::Object);
    w.take(a, s1);
    let after = &w.s.nodes[&a];
    assert_eq!(after.seq.0, before.seq.0 + 1);
    assert_eq!(after.fence.0, before.fence.0 + 1);
    assert_eq!(after.version, before.version);
}

#[path = "evolve_check_tests.rs"]
mod checks;
