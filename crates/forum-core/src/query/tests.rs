//! Unit tests of WP-D3's queries (design D.12.1): `ranked` from the index against `spec::ranked`
//! (CS-16), `leave_better` (A-63), the brief's sections, the status line and `hello`, `who`, `list`
//! and archiving (AR), what rests on a node (CS-22), and the board (CS-25).

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    clippy::wildcard_enum_match_arm,
    clippy::needless_range_loop,
    reason = "test code: the deny list guards the crate's own code"
)]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::apply::tests::{T0, class, label, text};
use crate::cmd::NextQuery;
use crate::event::{EntryBody, EntryText, Ev, NodeInit};
use crate::params::Params;
use crate::spec;
use crate::state::{Author, Origin, Role};
use crate::testkit::World;
use crate::types::{
    Dur, Fence, HOUR, LinkRel, MINUTE, Name, NodeId, NodeRef, Outcome, ROOT, Seq, SessionId,
};

/// xorshift64*: a small deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

fn init(title: &str) -> NodeInit {
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
        role: Role::Plain,
    }
}

fn mk(w: &mut World, parent: NodeId, name: &str, i: NodeInit) -> NodeId {
    let id = w.s.counters.next_node;
    w.emit(Ev::NodeCreated {
        id,
        parent,
        name: Name::new(name).unwrap(),
        origin: Origin::Agent(SessionId(1)),
        init: i,
    });
    id
}

fn session(w: &mut World, c: &str, caps: &[&str], horizon: Option<u32>) -> SessionId {
    let s = w.s.counters.next_session;
    let p = w.s.params.clone();
    w.emit(Ev::SessionOpened {
        session: s,
        class: class(c),
        caps: caps.iter().map(|x| label(x)).collect(),
        horizon,
        intent: None,
        silence_at: w.clock.plus(p.presence_silence),
        retire_at: w.clock.plus(p.session_retain),
        principal: None,
    });
    w.sessions.push((class(c), s));
    s
}

fn hold(w: &mut World, n: NodeId, s: SessionId) -> Token {
    let fence = Fence(w.s.nodes[&n].fence.0 + 1);
    let t = Token { node: n, fence };
    let terms = crate::helpers::holds::terms_of(&w.s.params);
    let at = w.clock;
    w.emit(Ev::HoldIssued {
        token: t,
        session: s,
        lease_until: at.plus(terms.first),
        terms,
        l: terms.default,
        reopened: None,
    });
    t
}

fn add(w: &mut World, n: NodeId, by: Author, body: EntryBody) -> Eid {
    let eid = w.s.counters.next_eid;
    let seq = Seq(w.s.nodes[&n].seq.0 + 1);
    w.emit(Ev::Appended {
        eid,
        node: n,
        seq,
        by,
        body,
        text: EntryText::default(),
    });
    eid
}

/// Compares the index's ranking with the naive sort for every session and a few queries: equal,
/// unless the query filter capped the scan, where the index's list is a prefix (§18).
fn compare(w: &World, queries: &[NextQuery]) {
    let now = w.clock;
    for &s in w.s.sessions.keys() {
        for q in queries {
            let (got, why) = ranked(&w.s, s, now, q, usize::MAX, &mut NoMeter);
            let got: Vec<Cand> = got.into_iter().map(|r| r.cand).collect();
            let want = spec::ranked(&w.s, s, now, q);
            if why.query_capped {
                assert_eq!(&want[..got.len()], &got[..], "a prefix for {s:?} {q:?}");
            } else {
                assert_eq!(got, want, "session {s:?} query {q:?} why {why:?}");
            }
            // The first k are the first k.
            for k in [1_usize, 3] {
                let (top, _) = ranked(&w.s, s, now, q, k, &mut NoMeter);
                let top: Vec<Cand> = top.into_iter().map(|r| r.cand).collect();
                assert_eq!(top, want.iter().take(k).copied().collect::<Vec<_>>());
            }
        }
    }
}

/// `ranked` from the index equals `spec::ranked` (CS-16): sessions with and without caps and
/// horizons, refusals, own holds, priorities and hour buckets, and across pause, unpause, pause
/// expiry, a `depends_on` target's close and reopen, and a move into and out of a paused scope;
/// 1.10: review items of several priorities and ages, and, under a horizon, nodes without `est`
/// beside the status, review, and routing items of their partition.
#[test]
fn ranked_from_the_index_equals_spec() {
    for seed in 1..=12_u64 {
        let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut w = World::new(
            Params {
                hold_per_session: 3,
                ..Params::default()
            },
            Time(T0.0 + seed * 1_234_567),
        );
        let labels = ["x", "y"];
        let mut sessions = Vec::new();
        for i in 0..4 {
            let caps: Vec<&str> = labels.iter().copied().filter(|_| r.chance(50)).collect();
            let horizon = r
                .chance(50)
                .then(|| [1_u32, 30, 64, 100, 200][r.below(5) as usize]);
            let c = if i % 2 == 0 { "a" } else { "b" };
            sessions.push(session(&mut w, c, &caps, horizon));
        }
        let parent = mk(&mut w, ROOT, "p", init("p"));
        let mut nodes = Vec::new();
        for i in 0..24 {
            let mut x = init("n");
            x.tags = if r.chance(70) {
                [label("offer")].into()
            } else {
                BTreeSet::new()
            };
            if r.chance(30) {
                x.tags.insert(label("t"));
            }
            x.needs = labels
                .iter()
                .filter(|_| r.chance(30))
                .map(|l| label(l))
                .collect();
            x.est = r
                .chance(60)
                .then(|| [10_u32, 40, 64, 65, 100, 127, 128, 300][r.below(8) as usize]);
            x.priority = [0_i32, 0, 1, 2, -1][r.below(5) as usize];
            x.capacity = [1_u32, 1, 2, 0][r.below(4) as usize];
            x.kind = r.chance(30).then(|| label("k"));
            if r.chance(20) && !nodes.is_empty() {
                x.depends_on = [nodes[r.below(nodes.len() as u64) as usize]].into();
            }
            let under = if r.chance(50) { parent } else { ROOT };
            nodes.push(mk(&mut w, under, &alloc::format!("n{i}"), x));
            if r.chance(30) {
                w.at(w.clock.plus(Dur(r.below(2 * HOUR.0))));
            }
        }
        // Flags (objections), holds, and refusals.
        for &n in &nodes {
            if r.chance(15) {
                add(&mut w, n, Author::Server, EntryBody::Object);
            }
            if r.chance(20) {
                let s = sessions[r.below(4) as usize];
                if w.s.nodes[&n].capacity > 0
                    && crate::spec::live_holds(&w.s, n, w.clock) < w.s.nodes[&n].capacity as usize
                    && w.s.ix.holds_by_session.get(&s).is_none_or(|h| h.len() < 3)
                {
                    hold(&mut w, n, s);
                }
            }
            if r.chance(15) {
                let s = sessions[r.below(4) as usize];
                let c = w.s.sessions[&s].class.clone();
                let at = w.clock;
                w.emit(Ev::Refused {
                    node: n,
                    class: c,
                    session: s,
                    backoff_until: at.plus(Dur(HOUR.0)),
                    expires_at: at.plus(Dur(2 * HOUR.0)),
                    quorum: 1 + r.below(2) as u32,
                });
            }
        }
        // 1.10: reviews, closed nodes with an open objection, of several priorities and ages, which
        // rank by the ring before age; and closed nodes with none. Two batches, an hour and more
        // apart; only nodes with no hold at all are closed.
        for batch in 0..2 {
            for &n in &nodes {
                let unheld = !w.s.holds.keys().any(|t| t.node == n);
                if r.chance(15) && unheld && w.s.nodes[&n].is_open() {
                    if r.chance(70) {
                        add(&mut w, n, Author::Server, EntryBody::Object);
                    }
                    w.emit(Ev::NodeClosed {
                        node: n,
                        outcome: Outcome::Done,
                        merged_into: None,
                        by: None,
                    });
                }
            }
            if batch == 0 {
                w.at(w.clock.plus(Dur(HOUR.0 + r.below(2 * HOUR.0))));
                w.settle();
            }
        }
        // A routing item.
        add(
            &mut w,
            crate::types::USER,
            Author::Session(sessions[0]),
            EntryBody::FromUser {
                principal: crate::types::USER_PRINCIPAL,
            },
        );
        let queries = [
            NextQuery::default(),
            NextQuery {
                under: Some(NodeRef::Id(parent)),
                kind: None,
                tags: BTreeSet::new(),
            },
            NextQuery {
                under: None,
                kind: Some(label("k")),
                tags: [label("t")].into(),
            },
        ];
        compare(&w, &queries);
        // A pause of the parent, its expiry; a pause of ROOT, lifted.
        let by = sessions[1];
        let at = w.clock;
        w.emit(Ev::Paused {
            scope: parent,
            until: at.plus(Dur(30 * MINUTE.0)),
            ttl: Dur(30 * MINUTE.0),
            by,
            class: class("b"),
            renewed: false,
        });
        compare(&w, &queries);
        // A move into and out of the paused scope.
        let mover = nodes[(seed as usize) % nodes.len()];
        if w.s.nodes[&mover].parent() == Some(ROOT) {
            w.emit(Ev::NodeMoved {
                node: mover,
                parent,
                name: Name::new("moved").unwrap(),
            });
            compare(&w, &queries);
            w.emit(Ev::NodeMoved {
                node: mover,
                parent: ROOT,
                name: Name::new("back").unwrap(),
            });
            compare(&w, &queries);
        }
        w.at(w.clock.plus(Dur(30 * MINUTE.0)));
        w.settle();
        compare(&w, &queries);
        let at = w.clock;
        w.emit(Ev::Paused {
            scope: ROOT,
            until: at.plus(Dur(HOUR.0)),
            ttl: Dur(HOUR.0),
            by,
            class: class("b"),
            renewed: false,
        });
        compare(&w, &queries);
        w.emit(Ev::Unpaused { scope: ROOT });
        compare(&w, &queries);
        // A depends_on target's close and reopen.
        let targets: Vec<NodeId> = nodes
            .iter()
            .flat_map(|n| w.s.nodes[n].depends_on.iter().copied())
            .collect();
        if let Some(&t) = targets.first()
            && w.s.ix.node[&t].live == 0
            && w.s.nodes[&t].is_open()
        {
            w.emit(Ev::NodeClosed {
                node: t,
                outcome: Outcome::Done,
                merged_into: None,
                by: None,
            });
            compare(&w, &queries);
            w.emit(Ev::NodeReopened { node: t });
            compare(&w, &queries);
        }
        // Later: hour buckets age, backoffs end, refusals expire.
        for _ in 0..3 {
            w.at(w.clock.plus(Dur(40 * MINUTE.0)));
            w.settle();
            compare(&w, &queries);
        }
    }
}

/// A query whose filter rejects `SCAN_MAX` entries stops and says `≥k` (CS-16, §18).
#[test]
fn a_query_that_hits_scan_max_stops() {
    let mut w = World::new(Params::default(), T0);
    let s = session(&mut w, "a", &[], None);
    for i in 0..(SCAN_MAX + 5) {
        let mut x = init("n");
        x.tags = [label("offer")].into();
        // Unchecked events, for speed; the indices are checked once below.
        let id = w.s.counters.next_node;
        w.emit_raw(Ev::NodeCreated {
            id,
            parent: ROOT,
            name: Name::new(&alloc::format!("n{i}")).unwrap(),
            origin: Origin::Agent(SessionId(1)),
            init: x,
        });
    }
    w.indices().unwrap();
    let q = NextQuery {
        under: None,
        kind: Some(label("none")),
        tags: BTreeSet::new(),
    };
    let mut m = CountMeter::default();
    let (got, why) = ranked(&w.s, s, w.clock, &q, 1, &mut m);
    assert!(got.is_empty());
    assert!(why.query_capped);
    assert_eq!(why.query, SCAN_MAX);
    assert!(m.0 <= u64::from(SCAN_MAX) + 10, "visits {}", m.0);
    compare(&w, &[q]);
}

/// The hour buckets of `aged`: a candidate that arrived at 10:59 has aged 1 at 11:00 (D.8.5, CS-12).
#[test]
fn aged_counts_wall_clock_hours() {
    let start = Time(1_700_006_400_000 + 10 * HOUR.0 + 59 * MINUTE.0);
    let mut w = World::new(Params::default(), start);
    let s = session(&mut w, "a", &[], None);
    let mut x = init("n");
    x.tags = [label("offer")].into();
    mk(&mut w, ROOT, "n", x);
    w.at(Time(start.0 + MINUTE.0));
    let (got, _) = ranked(&w.s, s, w.clock, &NextQuery::default(), 1, &mut NoMeter);
    assert_eq!(got[0].why.aged, 1);
    assert_eq!(got[0].why.reason, crate::reply::Reason::Offered);
    assert_eq!(got[0].why.tier, 2);
}

/// `leave_better` (A-63): the anchor's siblings, then the parent, then the parent's siblings, and
/// so on up; a held, waiting, or reserved ancestor is skipped; the anchor is never listed; the
/// empty form only within the caller's eligible partitions.
#[test]
fn leave_better_order() {
    let mut w = World::new(
        Params {
            leave_better_max: 10,
            ..Params::default()
        },
        T0,
    );
    let s = session(&mut w, "a", &[], None);
    let flag = |w: &mut World, n: NodeId| {
        add(w, n, Author::Server, EntryBody::Object);
    };
    let g = mk(&mut w, ROOT, "g", init("g"));
    let uncle = mk(&mut w, ROOT, "uncle", init("u"));
    let p = mk(&mut w, g, "p", init("p"));
    let aunt = mk(&mut w, g, "aunt", init("a"));
    let n = mk(&mut w, p, "n", init("n"));
    let sib2 = mk(&mut w, p, "sib2", init("s2"));
    let sib1 = mk(&mut w, p, "sib1", init("s1"));
    for x in [g, uncle, p, aunt, n, sib1, sib2] {
        flag(&mut w, x);
    }
    let lb = leave_better(&w.s, Some(n), None);
    assert_eq!(lb, vec![sib2, sib1, p, aunt, g, uncle]);
    // A held ancestor is skipped; a waiting one too.
    hold(&mut w, p, s);
    let at = w.clock;
    w.emit(Ev::WaitSet {
        node: g,
        wait: Some(crate::state::WaitOn::Time(at.plus(Dur(HOUR.0)))),
    });
    let lb = leave_better(&w.s, Some(n), None);
    assert_eq!(lb, vec![sib2, sib1, aunt, uncle]);
    // The cap.
    w.s.params.leave_better_max = 2;
    assert_eq!(leave_better(&w.s, Some(n), None), vec![sib2, sib1]);
    // The empty form: by id within the caller's partitions.
    w.s.params.leave_better_max = 10;
    let lb = leave_better(&w.s, None, Some(s));
    let mut want: Vec<NodeId> = vec![uncle, aunt, n, sib2, sib1];
    want.sort();
    assert_eq!(lb, want);
    let needy = {
        let mut x = init("needy");
        x.needs = [label("x")].into();
        mk(&mut w, ROOT, "needy", x)
    };
    flag(&mut w, needy);
    assert!(!leave_better(&w.s, None, Some(s)).contains(&needy));
    let s2 = session(&mut w, "a", &["x"], None);
    assert!(leave_better(&w.s, None, Some(s2)).contains(&needy));
}

/// The brief: nine sections in §10.4 order, with the flags, the open intent and the open `said`
/// entries, the other `said` entries and the open commitments, the decisions with the agreements
/// of the node and its ancestors, the unsettled requests, the objections, the digest, the last
/// checkpoint, and the total of the rest; the header's rollup (CS-32).
#[test]
fn brief_sections() {
    let mut w = World::new(Params::default(), T0);
    let s = session(&mut w, "a", &[], None);
    let mut top = init("top");
    top.agreements = text("review");
    let p = mk(&mut w, ROOT, "p", top);
    let mut x = init("n");
    x.digest = text("sum");
    x.keys = [(
        crate::types::KeyStr::new("chat:x/1").unwrap(),
        crate::types::NormVer(1),
    )]
    .into();
    let other = mk(&mut w, ROOT, "other", init("o"));
    x.links = [(LinkRel::DerivedFrom, other)].into();
    let n = mk(&mut w, p, "n", x);
    let child = mk(&mut w, n, "c", init("c"));
    let by = Author::Session(s);
    let conv = crate::types::KeyStr::new("chat:x/1").unwrap();
    let said = |w: &mut World, kind| {
        add(
            w,
            n,
            by,
            EntryBody::Said {
                conv: conv.clone(),
                kind,
                corrects: None,
                voice: false,
            },
        )
    };
    let ack = said(&mut w, crate::types::SaidKind::Ack);
    w.emit(Ev::CommitmentOpened {
        c: ack,
        node: n,
        conv: conv.clone(),
        followup_at: w.clock.plus(Dur(HOUR.0)),
    });
    w.emit(Ev::SaidConfirmed {
        said: ack,
        via: crate::event::ConfirmVia::Explicit,
        link: None,
    });
    let open = said(&mut w, crate::types::SaidKind::Reply);
    let r = add(
        &mut w,
        n,
        by,
        EntryBody::Request {
            origin: crate::types::Key {
                key: crate::types::KeyStr::new("chat:x/1/7").unwrap(),
                nv: crate::types::NormVer(1),
            },
            conv: conv.clone(),
            from_user: None,
            requester: crate::types::Requester::External,
        },
    );
    let d = add(
        &mut w,
        n,
        by,
        EntryBody::Decision {
            settles: BTreeSet::new(),
            supersedes: None,
            assumes: BTreeSet::new(),
            evidence: None,
            basis: Vec::new(),
        },
    );
    let o = add(&mut w, n, by, EntryBody::Object);
    let t = hold(&mut w, n, s);
    let cp = add(
        &mut w,
        n,
        by,
        EntryBody::Checkpoint {
            token: t,
            release: None,
            pr_named: None,
            handed_off_until: None,
        },
    );
    w.emit(Ev::IntentOpened {
        node: n,
        intent: crate::state::Intent {
            eid: cp,
            session: s,
            pr_named: false,
        },
    });
    let b = brief(&w.s, n, Some(s), w.clock, &mut NoMeter);
    let names: Vec<SectionName> = b.sections.iter().map(|x| x.name).collect();
    assert_eq!(
        names,
        vec![
            SectionName::Flags,
            SectionName::Unfinished,
            SectionName::SaidAndCommitments,
            SectionName::DecisionsAndAgreements,
            SectionName::UnsettledRequests,
            SectionName::Objections,
            SectionName::Digest,
            SectionName::LastCheckpoint,
            SectionName::Everything,
        ]
    );
    let items = |i: usize| b.sections[i].items.clone();
    assert!(items(0).contains(&BriefItem::Flag(Flag::Objection)));
    assert_eq!(
        items(1),
        vec![
            BriefItem::Intent(crate::state::Intent {
                eid: cp,
                session: s,
                pr_named: false,
            }),
            BriefItem::Said(open)
        ]
    );
    assert_eq!(
        items(2),
        vec![BriefItem::Said(ack), BriefItem::Commitment(ack)]
    );
    assert_eq!(
        items(3),
        vec![BriefItem::Decision(d), BriefItem::Agreement { source: p }]
    );
    assert_eq!(items(4), vec![BriefItem::Request(r)]);
    assert_eq!(items(5), vec![BriefItem::Objection(o)]);
    assert_eq!(items(6), vec![BriefItem::Digest]);
    assert_eq!(items(7), vec![BriefItem::Checkpoint(cp)]);
    assert_eq!(b.sections[8].total, w.s.nodes[&n].seq.0 as u32);
    assert_eq!(b.holders, vec![(s, w.s.holds[&t].lease_until)]);
    assert!(
        b.continues.is_empty(),
        "only `continues` links make predecessors"
    );
    assert_eq!(b.rollup.open_children, 1);
    assert_eq!(b.rollup.sub.open, 1);
    let _ = (child, LinkRel::Continues);
}

/// The status line and `hello`: holds, pauses with their sources (CS-9), and, for a session that
/// serves principals, the pending questions and digest entries under any alias (CS-3, U1).
#[test]
fn status_line_and_hello() {
    let mut w = World::new(Params::default(), T0);
    let s = session(&mut w, "a", &[], None);
    let n = mk(&mut w, ROOT, "n", init("n"));
    let t = hold(&mut w, n, s);
    let at = w.clock;
    w.emit(Ev::Paused {
        scope: n,
        until: at.plus(Dur(HOUR.0)),
        ttl: Dur(HOUR.0),
        by: s,
        class: class("a"),
        renewed: false,
    });
    let line = status_line(&w.s, s, w.clock);
    let h = &w.s.holds[&t];
    assert_eq!(
        line.holds,
        vec![crate::query::HoldStatus {
            node: n,
            lease_until: h.lease_until,
            checkpoint_by: crate::query::checkpoint_by(h),
        }]
    );
    assert_eq!(line.now, w.clock);
    assert_eq!(line.hold_pauses.len(), 1);
    assert!(line.root_pause.is_none());
    assert_eq!(line.user_decisions, None);
    assert_eq!(line.class, class("a"));
    let h = hello(&w.s, s, w.clock);
    assert_eq!(h.holds, vec![(t, n, w.s.holds[&t].lease_until)]);
    assert!(h.principals.is_empty());
    assert_eq!(h.user_decisions.count, 0);
    assert_eq!(h.present.total, crate::reply::Total::Exact(1));
    // The user's items match spec for a session that serves `user`.
    w.emit(Ev::SessionHello {
        session: s,
        caps: None,
        horizon: None,
        intent: None,
        principal: Some(crate::types::USER_PRINCIPAL),
    });
    let h = hello(&w.s, s, w.clock);
    let want = spec::hello_items(&w.s, s);
    assert_eq!(h.user_decisions.count as usize, want.questions.len());
    assert_eq!(h.user_digest.count as usize, want.digest.len());
    assert_eq!(h.user_digest.older, Some(want.older));
    assert_eq!(status_line(&w.s, s, w.clock).user_decisions, Some(0));
}

/// `who`: at most `WHO_MAX` sessions with exact counts by class; `who {at}` descends through the
/// attended children (CS-21).
#[test]
fn who_lists_and_counts() {
    let mut w = World::new(Params::default(), T0);
    let mut ss = Vec::new();
    for i in 0..14 {
        ss.push(session(
            &mut w,
            if i % 2 == 0 { "a" } else { "b" },
            &[],
            None,
        ));
    }
    let page = who(&w.s, None, w.clock);
    assert_eq!(page.items.len(), WHO_MAX);
    assert_eq!(page.total, crate::reply::Total::Exact(14));
    assert_eq!(page.by_class, vec![(class("a"), 7), (class("b"), 7)]);
    assert_eq!(page.items[0].session, ss[13]);
    let p = mk(&mut w, ROOT, "p", init("p"));
    for i in 0..12 {
        let mut x = init("c");
        x.capacity = 1;
        let c = mk(&mut w, p, &alloc::format!("c{i}"), x);
        hold(&mut w, c, ss[i]);
    }
    let page = who(&w.s, Some(p), w.clock);
    assert_eq!(page.items.len(), WHO_MAX);
    assert_eq!(page.total, crate::reply::Total::AtLeast(WHO_MAX as u32));
    assert!(page.by_class.is_empty());
    let one = mk(&mut w, ROOT, "one", init("o"));
    hold(&mut w, one, ss[13]);
    let page = who(&w.s, Some(one), w.clock);
    assert_eq!(page.total, crate::reply::Total::Exact(1));
}

/// `list`: by id, with `closed: None` the open nodes and the closed ones not archived (AR), and the
/// exact subtree counts of its `under` (CS-32).
#[test]
fn list_and_archiving() {
    let mut w = World::new(Params::default(), T0);
    let p = mk(&mut w, ROOT, "p", init("p"));
    let a = mk(&mut w, p, "a", init("a"));
    let b = mk(&mut w, p, "b", init("b"));
    w.emit(Ev::NodeClosed {
        node: b,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    let f = |closed| ListFilter {
        under: Some(p),
        kind: None,
        tags: BTreeSet::new(),
        status: None,
        fields: BTreeMap::new(),
        flags: FlagSet::EMPTY,
        closed,
        limit: 50,
    };
    let l = list(&w.s, &f(None), w.clock, &mut NoMeter);
    assert_eq!(l.nodes, vec![p, a, b]);
    assert_eq!(l.under.open, 1);
    assert_eq!(l.under.done, 1);
    let later = w.clock.plus(w.s.params.archive_after);
    assert!(archived(&w.s, b, later));
    assert!(!archived(&w.s, a, later));
    let l = list(&w.s, &f(None), later, &mut NoMeter);
    assert_eq!(l.nodes, vec![p, a]);
    let l = list(&w.s, &f(Some(true)), later, &mut NoMeter);
    assert_eq!(l.nodes, vec![b]);
    let l = list(&w.s, &f(Some(false)), later, &mut NoMeter);
    assert_eq!(l.nodes, vec![p, a]);
    assert_eq!(path_of(&w.s, a).0.len(), 2);
    assert!(is_under(&w.s, a, p));
    assert!(!is_under(&w.s, p, a));
}

/// What rests on a node (CS-22): incoming links by relation, newest source first, dependents, with
/// pages and totals.
#[test]
fn backlinks_pages() {
    let mut w = World::new(Params::default(), T0);
    let n = mk(&mut w, ROOT, "n", init("n"));
    let mut srcs = Vec::new();
    for i in 0..(BACKLINKS_MAX + 5) {
        let mut x = init("s");
        x.links = [(LinkRel::DerivedFrom, n)].into();
        x.depends_on = [n].into();
        srcs.push(mk(&mut w, ROOT, &alloc::format!("s{i}"), x));
    }
    let b = backlinks(&w.s, n, 0, &mut NoMeter);
    assert_eq!(b.links.len(), BACKLINKS_MAX);
    assert_eq!(b.links[0], (LinkRel::DerivedFrom, *srcs.last().unwrap()));
    assert_eq!(
        b.links_total,
        crate::reply::Total::Exact((BACKLINKS_MAX + 5) as u32)
    );
    let b1 = backlinks(&w.s, n, 1, &mut NoMeter);
    assert_eq!(b1.dependents.len(), 5);
    assert_eq!(b1.dependents.last(), Some(&srcs[0]));
    assert!(b.cited_by.is_empty());
    assert_eq!(b.cited_by_total, crate::reply::Total::Exact(0));
}

/// The board (D.8.8, CS-25): unrouted entries and unsettled requests oldest first, the states, and
/// the scope: `all` while `user` is the only principal.
#[test]
fn the_board() {
    let mut w = World::new(Params::default(), T0);
    let s = session(&mut w, "a", &[], None);
    let n = mk(&mut w, ROOT, "n", init("n"));
    let conv = crate::types::KeyStr::new("chat:x/1").unwrap();
    let req = |w: &mut World, m: &str| {
        add(
            w,
            n,
            Author::Session(s),
            EntryBody::Request {
                origin: crate::types::Key {
                    key: crate::types::KeyStr::new(m).unwrap(),
                    nv: crate::types::NormVer(1),
                },
                conv: conv.clone(),
                from_user: None,
                requester: crate::types::Requester::External,
            },
        )
    };
    let r1 = req(&mut w, "chat:x/1/1");
    let e = add(
        &mut w,
        crate::types::USER,
        Author::Session(s),
        EntryBody::FromUser {
            principal: crate::types::USER_PRINCIPAL,
        },
    );
    let r2 = req(&mut w, "chat:x/1/2");
    assert_eq!(board_scope(&w.s, s, None), crate::reply::BoardScope::All);
    let page = board(
        &w.s,
        None,
        &crate::reply::BoardScope::All,
        w.clock,
        10,
        &mut NoMeter,
    );
    let ids: Vec<(Option<Eid>, Option<Eid>)> =
        page.rows.iter().map(|r| (r.request, r.entry)).collect();
    assert_eq!(
        ids,
        vec![(Some(r1), None), (None, Some(e)), (Some(r2), None)]
    );
    assert_eq!(page.total, crate::reply::Total::Exact(3));
    assert_eq!(page.rows[1].state, crate::cmd::BoardState::Unrouted);
    assert_eq!(page.rows[0].state, crate::cmd::BoardState::InProgress);
    let only = board(
        &w.s,
        Some(crate::cmd::BoardState::Unrouted),
        &crate::reply::BoardScope::All,
        w.clock,
        10,
        &mut NoMeter,
    );
    assert_eq!(only.rows.len(), 1);
    let _ = (String::new(), vec![0_u8]);
}

/// §10.4 and §18 (1.12's review): `beyond_horizon`'s count is `≥k` (`capped`) when the scan of the
/// bucket the horizon falls in stopped at `SCAN_MAX` entries, and its first is a node to take, never
/// the review of a closed node, which the count includes.
#[test]
fn beyond_horizon_says_its_count_was_cut_and_names_a_node_to_take() {
    let mut w = World::new(Params::default(), T0);
    let a = session(&mut w, "a", &["lean"], Some(45));
    // A session with lean and no horizon: nothing is longer than every horizon.
    let _b = session(&mut w, "b", &["lean"], None);
    let node = |name: &str, est: u32| {
        let mut x = init(name);
        x.tags = [label("offer")].into();
        x.needs = [label("lean")].into();
        x.est = Some(est);
        x
    };
    // The review of a closed node, and a node to take, both over the horizon, in one bucket.
    let review = mk(&mut w, ROOT, "review", node("review", 120));
    add(&mut w, review, Author::Server, EntryBody::Object);
    w.emit(Ev::NodeClosed {
        node: review,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    let over = mk(&mut w, ROOT, "over", node("over", 100));
    // The review comes first in their bucket's order.
    let bucket =
        w.s.ix
            .next
            .parts
            .values()
            .find(|x| x.iter().any(|k| k.id == over.0))
            .unwrap();
    assert_eq!(
        bucket.iter().map(|k| (k.id, k.kind)).collect::<Vec<_>>(),
        [(review.0, 1), (over.0, 0)]
    );
    let (got, _) = ranked(&w.s, a, w.clock, &NextQuery::default(), 1, &mut NoMeter);
    assert!(got.is_empty());
    let fit = mk(&mut w, ROOT, "fit", node("fit", 40));
    let (got, _) = ranked(&w.s, a, w.clock, &NextQuery::default(), 1, &mut NoMeter);
    assert_eq!(got[0].cand, Cand::Node(fit));
    let b = got[0].why.beyond_horizon.unwrap();
    assert_eq!((b.count, b.capped, b.first, b.est), (2, false, over, 100));
    // More than SCAN_MAX entries in the bucket the horizon falls in (32 to 63): the count is cut.
    for i in 0..SCAN_MAX {
        let id = w.s.counters.next_node;
        w.emit_raw(Ev::NodeCreated {
            id,
            parent: ROOT,
            name: Name::new(&alloc::format!("f{i}")).unwrap(),
            origin: Origin::Agent(SessionId(1)),
            init: node("f", 40),
        });
    }
    w.indices().unwrap();
    let (got, _) = ranked(&w.s, a, w.clock, &NextQuery::default(), 1, &mut NoMeter);
    let b = got[0].why.beyond_horizon.unwrap();
    assert!(b.capped && b.count >= 2 && b.first == over, "{b:?}");
}

/// §10.4 (1.12; it6 l5 F4): work whose `est` exceeds the caller's horizon and that of every session
/// seen whose caps meet its needs is offered after every fitting candidate, marked `longer`; the
/// first candidate names what the caller's horizon excludes (`beyond_horizon`); a session with the
/// caps and no horizon takes the lemmas off that list, for every caller.
#[test]
fn work_longer_than_every_horizon_is_offered_last() {
    let mut w = World::new(Params::default(), T0);
    let a = session(&mut w, "a", &["lean", "shell"], Some(45));
    let _c = session(&mut w, "a", &["lean"], Some(20));
    let lemma = |w: &mut World, name: &str, est: u32| {
        let mut x = init(name);
        x.tags = [label("offer")].into();
        x.needs = [label("lean")].into();
        x.est = Some(est);
        mk(w, ROOT, name, x)
    };
    let l1 = lemma(&mut w, "lemma-01", 60);
    let l2 = lemma(&mut w, "lemma-02", 120);
    let mut x = init("fit");
    x.tags = [label("offer")].into();
    x.est = Some(10);
    let fit = mk(&mut w, ROOT, "fit", x);
    compare(&w, &[NextQuery::default()]);
    let (got, _) = ranked(&w.s, a, w.clock, &NextQuery::default(), 3, &mut NoMeter);
    let cands: Vec<Cand> = got.iter().map(|r| r.cand).collect();
    assert_eq!(cands.first(), Some(&Cand::Node(fit)));
    assert_eq!(
        cands.iter().skip(1).copied().collect::<BTreeSet<_>>(),
        [Cand::Node(l1), Cand::Node(l2)].into_iter().collect()
    );
    assert!(!got[0].why.longer && got[1].why.longer && got[2].why.longer);
    let b = got[0].why.beyond_horizon.unwrap();
    assert_eq!((b.count, b.horizon), (2, 45));
    assert!(
        (b.first == l1 && b.est == 60) || (b.first == l2 && b.est == 120),
        "{b:?}"
    );
    assert!(got[1].why.beyond_horizon.is_none());
    // `next` with the first only gives the fitting node, and the lemmas once it is held.
    let (one, _) = ranked(&w.s, a, w.clock, &NextQuery::default(), 1, &mut NoMeter);
    assert_eq!(one.len(), 1);
    hold(&mut w, fit, a);
    let (one, _) = ranked(&w.s, a, w.clock, &NextQuery::default(), 1, &mut NoMeter);
    assert!(one.first().is_some_and(|r| r.why.longer));
    compare(&w, &[NextQuery::default()]);
    // Without lean, the caller is offered none of them, and told of none.
    let d = session(&mut w, "a", &["shell"], Some(45));
    let (got, _) = ranked(&w.s, d, w.clock, &NextQuery::default(), 3, &mut NoMeter);
    assert!(got.is_empty());
    // A session with lean and no horizon: the lemmas are no longer longer than every horizon, and
    // the 45-minute prover is told of them, not offered them.
    let _e = session(&mut w, "b", &["lean"], None);
    let (got, why) = ranked(&w.s, a, w.clock, &NextQuery::default(), 3, &mut NoMeter);
    assert!(got.is_empty(), "{got:?}");
    assert_eq!(why.horizon, 2);
    compare(&w, &[NextQuery::default()]);
}
