//! The cost test of design D.12.8 (§18), for WP-D3's queries and commands: a small and a large
//! forum, and the queries `ranked`, `brief`, `board` (first page), `list`, and `backlinks` under a
//! `CountMeter`, whose visits must be bounded by constants (the page size, the open items of the
//! node, the eligible partitions, the skips of D.8.5), so that the large forum costs no more visits
//! than the small one; `status_line`, `hello`, and `who`, which take no meter, and `apply` of
//! `take`, `checkpoint`, and `release`, are timed, with a time ratio large/small of at most 1.5
//! (review TP-19), measured as the minimum of repeated runs with a small absolute slack for the
//! timer's noise.
//!
//! Sizes: this test runs in CI on every `cargo test`, so its forums are smaller than D.12.8's (10⁴
//! and 10⁶ nodes): 2,000 and 60,000 nodes, 100 and 6,000 candidates, 100 and 4,000 live sessions,
//! 10 and 3,000 requests on one conversation node. The scale gates (WP-F4, `verify.sh --bench`)
//! run D.12.8's sizes over HTTP. `query::guide` has its own tests below (WP-D7): a holder of the
//! hot node, and D.12.8's 1.4 caller (a principal with many items pending, a held hub with many
//! requests owed a reply, and a request recorded on an unheld hub); `query::health` and
//! `query::m1b_gaps` (WP-D6's) are timed like `status_line`, since they take no meter.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "test code: the deny list guards the crate's own code, and this test times it from \
              outside"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use forum_core::query::{self, CountMeter, ListFilter, NoMeter};
use forum_core::*;

const START: Time = Time(1_700_000_000_000);

struct Size {
    nodes: usize,
    candidates: usize,
    sessions: usize,
    requests: usize,
}

const SMALL: Size = Size {
    nodes: 2_000,
    candidates: 100,
    sessions: 100,
    requests: 10,
};

const LARGE: Size = Size {
    nodes: 60_000,
    candidates: 6_000,
    sessions: 4_000,
    requests: 3_000,
};

fn label(l: &str) -> Label {
    Label::new(l).unwrap()
}

fn init(title: &str, offer: bool) -> NodeInit {
    NodeInit {
        title: Text(title.to_string()),
        body: Text::default(),
        fields: BTreeMap::new(),
        status: Text::default(),
        digest: Text::default(),
        agreements: Text::default(),
        kind: None,
        tags: if offer {
            [label("offer")].into()
        } else {
            BTreeSet::new()
        },
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

/// A forum built by events, unchecked, and the ids the test needs.
struct Forum {
    s: State,
    worker: SessionId,
    hot: NodeId,
    free: NodeId,
}

fn ev(s: &mut State, ev: Ev) {
    let e = Event {
        at: s.last_now.max(START),
        ev,
    };
    evolve_mut(s, &e);
}

fn build(z: &Size) -> Forum {
    let mut s = State::default();
    evolve_mut(&mut s, &genesis(Params::default(), START));
    let class = ClassName::new("w").unwrap();
    let p = s.params.clone();
    // Sessions.
    let mut sessions = Vec::new();
    for _ in 0..z.sessions {
        let id = s.counters.next_session;
        ev(
            &mut s,
            Ev::SessionOpened {
                session: id,
                class: class.clone(),
                caps: BTreeSet::new(),
                horizon: None,
                intent: None,
                silence_at: START.plus(p.presence_silence),
                retire_at: START.plus(p.session_retain),
                principal: None,
            },
        );
        sessions.push(id);
    }
    // The tree: 100 branches, the nodes spread under them; the first `candidates` are offered.
    let mut branches = Vec::new();
    for b in 0..100 {
        let id = s.counters.next_node;
        ev(
            &mut s,
            Ev::NodeCreated {
                id,
                parent: NodeId(0),
                name: Name::new(&format!("b{b}")).unwrap(),
                origin: Origin::Agent(SessionId(1)),
                init: init("b", false),
            },
        );
        branches.push(id);
    }
    let mut made = 0;
    let mut free = NodeId(0);
    while made < z.nodes {
        let parent = branches[made % branches.len()];
        let id = s.counters.next_node;
        let offer = made < z.candidates;
        ev(
            &mut s,
            Ev::NodeCreated {
                id,
                parent,
                name: Name::new(&format!("n{made}")).unwrap(),
                origin: Origin::Agent(SessionId(1)),
                init: init("n", offer),
            },
        );
        if offer {
            free = id;
        }
        made += 1;
    }
    // A hot conversation node with many requests, and a hold on it.
    let hot = s.counters.next_node;
    let mut hot_init = init("hot", false);
    hot_init.capacity = 2;
    hot_init.keys = [(KeyStr::new("chat:x/1").unwrap(), NormVer(1))].into();
    ev(
        &mut s,
        Ev::NodeCreated {
            id: hot,
            parent: NodeId(0),
            name: Name::new("hot").unwrap(),
            origin: Origin::Agent(SessionId(1)),
            init: hot_init,
        },
    );
    for i in 0..z.requests {
        let eid = s.counters.next_eid;
        let seq = Seq(s.nodes[&hot].seq.0 + 1);
        ev(
            &mut s,
            Ev::Appended {
                eid,
                node: hot,
                seq,
                by: Author::Session(sessions[0]),
                body: EntryBody::Request {
                    origin: Key {
                        key: KeyStr::new(&format!("chat:x/1/{i}")).unwrap(),
                        nv: NormVer(1),
                    },
                    conv: KeyStr::new("chat:x/1").unwrap(),
                    from_user: None,
                    requester: Requester::External,
                },
                text: EntryText::default(),
            },
        );
    }
    // Some live holds: one per tenth session.
    let terms = LeaseTerms {
        first: p.lease_first,
        default: p.lease_default,
        max: p.lease_max,
        ceiling: p.lease_ceiling,
        renew_max: p.renew_max,
    };
    for (i, &x) in sessions.iter().enumerate().skip(1).step_by(10) {
        let n = NodeId(branches[0].0 + 100 + i as u64);
        if !s.nodes.contains_key(&n) || s.nodes[&n].tags.contains(&label("offer")) {
            continue;
        }
        let t = Token {
            node: n,
            fence: Fence(s.nodes[&n].fence.0 + 1),
        };
        ev(
            &mut s,
            Ev::HoldIssued {
                token: t,
                session: x,
                lease_until: START.plus(terms.first),
                terms,
                l: terms.default,
                reopened: None,
            },
        );
    }
    Forum {
        s,
        worker: sessions[0],
        hot,
        free,
    }
}

/// The minimum time of `f` over `n` runs.
fn time<F: FnMut()>(n: usize, mut f: F) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..n {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed());
    }
    best
}

/// `large <= 1.5 × small`, with an absolute slack for the timer.
fn ratio_ok(what: &str, small: Duration, large: Duration) {
    eprintln!("{what}: small {small:?}, large {large:?}");
    let bound = small.mul_f64(1.5) + Duration::from_micros(200);
    assert!(large <= bound, "{what}: {large:?} > 1.5 × {small:?}");
}

#[test]
fn queries_cost_no_more_in_a_larger_forum() {
    let small = build(&SMALL);
    let large = build(&LARGE);
    let now = small.s.last_now.max(large.s.last_now);
    let visits = |f: &Forum| {
        let mut out = Vec::new();
        let mut m = CountMeter::default();
        let (got, _) = query::ranked(&f.s, f.worker, now, &NextQuery::default(), 1, &mut m);
        assert_eq!(got.len(), 1);
        out.push(("ranked k=1", m.0));
        let mut m = CountMeter::default();
        let _ = query::ranked(&f.s, f.worker, now, &NextQuery::default(), 3, &mut m);
        out.push(("ranked k=3", m.0));
        let mut m = CountMeter::default();
        let _ = query::brief(&f.s, f.hot, Some(f.worker), now, &mut m);
        out.push(("brief", m.0));
        let mut m = CountMeter::default();
        let page = query::board(&f.s, None, &BoardScope::All, now, 50, &mut m);
        assert!(page.rows.len() <= 50);
        out.push(("board", m.0));
        let mut m = CountMeter::default();
        let _ = query::backlinks(&f.s, f.hot, 0, &mut m);
        out.push(("backlinks", m.0));
        let mut m = CountMeter::default();
        let filter = ListFilter {
            under: None,
            kind: None,
            tags: BTreeSet::new(),
            status: None,
            fields: BTreeMap::new(),
            flags: FlagSet::EMPTY,
            closed: None,
            limit: 50,
        };
        let _ = query::list(&f.s, &filter, now, &mut m);
        out.push(("list", m.0));
        out
    };
    let vs = visits(&small);
    let vl = visits(&large);
    for ((what, a), (_, b)) in vs.iter().zip(&vl) {
        eprintln!("{what}: small {a} visits, large {b} visits");
        match *what {
            // The board's first page reads the unsettled backlog (D.8.8): bounded by it, not by the
            // forum. The list stops at SCAN_MAX (§18).
            "board" => assert!(*b <= 2 * LARGE.requests as u64 + 64),
            "list" => assert!(*b <= u64::from(query::SCAN_MAX)),
            // At most 100 items per section and 100 predecessors (D.8.6).
            "brief" => assert!(*b <= 10 * 100),
            _ => assert!(
                b <= a || *b <= 32,
                "{what}: {b} visits in the large forum, {a} in the small"
            ),
        }
    }
    // Timed: the queries without a meter, and apply of take, checkpoint, and release.
    for (what, f) in [
        ("status_line", 0_u8),
        ("hello", 1),
        ("who", 2),
        ("who at", 3),
        ("m1b_gaps", 5),
        ("health", 4),
    ] {
        let run = |x: &Forum| {
            time(30, || match f {
                0 => {
                    let _ = query::status_line(&x.s, x.worker, now);
                }
                1 => {
                    let _ = query::hello(&x.s, x.worker, now);
                }
                2 => {
                    let _ = query::who(&x.s, None, now);
                }
                3 => {
                    let _ = query::who(&x.s, Some(NodeId(0)), now);
                }
                // WP-D6: `health` counts an unreachable partition without walking it, and M1b's
                // chains start from the waits and dependencies, never a walk of every node
                // (D.8.22): both forums have one flagged node and no wait.
                4 => {
                    let _ = query::health(&x.s, now);
                }
                _ => {
                    let _ = query::m1b_gaps(&x.s, now);
                }
            })
        };
        ratio_ok(what, run(&small), run(&large));
    }
    let class = ClassName::new("w").unwrap();
    // `apply` on one state, without a clone: take, checkpoint, and release in turn, each timed,
    // the minimum of the cycles kept; and a take of the hot conversation node.
    let apply_cost = |x: &Forum| {
        let ctx = Ctx {
            now,
            class: class.clone(),
            caps: None,
            session: SessionCtx::Existing(x.worker),
            op: None,
        };
        let mut s = x.s.clone();
        let mut best = [Duration::MAX; 4];
        for _ in 0..20 {
            for (i, target) in [(0_usize, x.free), (3, x.hot)] {
                let take = Cmd::Hold(HoldCmd::Take {
                    target: NodeRef::Id(target),
                    lease: None,
                });
                let t = Instant::now();
                let (_, out) = apply(&mut s, &ctx, &take);
                best[i] = best[i].min(t.elapsed());
                let Ok(Done {
                    reply: Reply::Taken { token, .. },
                    ..
                }) = out
                else {
                    panic!("take: {out:?}");
                };
                let cp = Cmd::Hold(HoldCmd::Checkpoint {
                    token,
                    done: Text("d".into()),
                    next_step: Text("n".into()),
                    intent: None,
                    intent_done: None,
                    files: Vec::new(),
                });
                let t = Instant::now();
                let (_, out) = apply(&mut s, &ctx, &cp);
                if i == 0 {
                    best[1] = best[1].min(t.elapsed());
                }
                assert!(out.is_ok(), "{out:?}");
                let rel = Cmd::Hold(HoldCmd::Release {
                    token,
                    done: Text("d".into()),
                    next_step: Text("n".into()),
                    reason: ReleaseReason::Handoff,
                    waits_for: None,
                    intent_done: None,
                    learnings: None,
                    unsent: BTreeSet::new(),
                    settles: BTreeSet::new(),
                });
                let t = Instant::now();
                let (_, out) = apply(&mut s, &ctx, &rel);
                if i == 0 {
                    best[2] = best[2].min(t.elapsed());
                }
                assert!(out.is_ok(), "{out:?}");
            }
        }
        best
    };
    let a = apply_cost(&small);
    let b = apply_cost(&large);
    for (i, what) in [
        "apply take",
        "apply checkpoint",
        "apply release",
        "a hold under the hot node",
    ]
    .iter()
    .enumerate()
    {
        ratio_ok(what, a[i], b[i]);
    }
    let _ = NoMeter;
}

/// `query::guide` (WP-D7, D.8.26's cost note, D.12.8): a session that holds the free node and the
/// hot conversation node, whose 10 or 3,000 requests another session recorded before the take,
/// gets its next calls in visits bounded by constants (its holds, the requests the call names,
/// the entries after its take, `ranked` with `k = 1`), whatever the forum's size, and in time
/// within 1.5 times the small forum's. The guide is computed on the response to a read, to a
/// checkpoint, and to a release.
#[test]
fn guide_costs_no_more_in_a_larger_forum() {
    let small = build(&SMALL);
    let large = build(&LARGE);
    let now = small.s.last_now.max(large.s.last_now);
    let class = ClassName::new("w").unwrap();
    let run = |f: &Forum| {
        // A session that holds nothing yet, and is not the requests' author.
        let x = SessionId(f.worker.0 + 2);
        let ctx = Ctx {
            now,
            class: class.clone(),
            caps: None,
            session: SessionCtx::Existing(x),
            op: None,
        };
        let mut s = f.s.clone();
        let mut tokens = Vec::new();
        for target in [f.free, f.hot] {
            let take = Cmd::Hold(HoldCmd::Take {
                target: NodeRef::Id(target),
                lease: None,
            });
            let (_, out) = apply(&mut s, &ctx, &take);
            let Ok(Done {
                reply: Reply::Taken { token, .. },
                ..
            }) = out
            else {
                panic!("take: {out:?}");
            };
            tokens.push(token);
        }
        let read = Cmd::View(ViewCmd::Constitution);
        let (_, read_out) = apply(&mut s, &ctx, &read);
        let checkpoint = Cmd::Hold(HoldCmd::Checkpoint {
            token: tokens[1],
            done: Text("d".into()),
            next_step: Text("n".into()),
            intent: None,
            intent_done: None,
            files: Vec::new(),
        });
        let mut s2 = s.clone();
        let (_, cp_out) = apply(&mut s2, &ctx, &checkpoint);
        assert!(cp_out.is_ok(), "{cp_out:?}");
        let release = Cmd::Hold(HoldCmd::Release {
            token: tokens[0],
            done: Text("d".into()),
            next_step: Text("n".into()),
            reason: ReleaseReason::Handoff,
            waits_for: None,
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        });
        let mut s3 = s.clone();
        let (_, rel_out) = apply(&mut s3, &ctx, &release);
        assert!(rel_out.is_ok(), "{rel_out:?}");
        let mut visits = Vec::new();
        for (what, st, cmd, out) in [
            ("read", &s, &read, &read_out),
            ("checkpoint", &s2, &checkpoint, &cp_out),
            ("release", &s3, &release, &rel_out),
        ] {
            let mut m = CountMeter::default();
            let g = query::guide(st, Some(x), now, cmd, out, &mut m);
            assert!(g.items.len() <= query::guide::NEXT_CALLS_MAX);
            let t = time(30, || {
                let _ = query::guide(st, Some(x), now, cmd, out, &mut NoMeter);
            });
            visits.push((what, m.0, t));
        }
        visits
    };
    let a = run(&small);
    let b = run(&large);
    for ((what, va, ta), (_, vb, tb)) in a.iter().zip(&b) {
        eprintln!("guide after {what}: small {va} visits, large {vb} visits");
        assert!(
            vb <= va || *vb <= 64,
            "guide after {what}: {vb} visits in the large forum, {va} in the small"
        );
        ratio_ok(&format!("guide after {what}"), *ta, *tb);
    }
}

/// The caller of D.12.8's 1.4 gate for `query::guide` (R3, R13): a session that serves a principal
/// not yet told, with 10⁴ pending questions and 10⁵ pending digest entries in the large forum (10
/// and 10 in the small), holds the hot conversation node, a third of whose requests a decision
/// lists and no answer settled (owed a reply, rule 6), with as many people entries by another
/// session after its take (rule 4); it then records a request on an unheld hub with the same
/// number of others (rule 7, R3). Its next calls after a read and after the request visit no more
/// entries in the large forum than in the small one, up to the caps of D.8.26, and take at most
/// 1.5 times as long.
#[test]
fn guide_for_a_principal_and_a_hub_costs_no_more_in_a_larger_forum() {
    let small = build(&SMALL);
    let large = build(&LARGE);
    let now = small.s.last_now.max(large.s.last_now);
    let later = now.plus(Dur(1_000));
    let workers = ClassName::new("w").unwrap();
    let assistant = ClassName::new("a").unwrap();
    let ctx_at = |at: Time, session: SessionCtx, class: &ClassName| Ctx {
        now: at,
        class: class.clone(),
        caps: None,
        session,
        op: None,
    };
    let key = |k: &str| Key {
        key: KeyStr::new(k).unwrap(),
        nv: NormVer(1),
    };
    let run = |f: &Forum, z: &Size, (questions, digest): (u64, u64)| {
        let mut s = f.s.clone();
        // The caller: `hello {principal}`.
        let alice = PrincipalKey::new("mail:alice@x").unwrap();
        let (_, out) = apply(
            &mut s,
            &ctx_at(now, SessionCtx::New, &assistant),
            &Cmd::Session(SessionCmd::Hello {
                intent: None,
                caps: None,
                horizon: None,
                principal: Some(alice.clone()),
                from_user: None,
            }),
        );
        let x = out.unwrap().opened.unwrap();
        let p = s.principal_ids[&alice];
        // Pending for alice, not yet told: written into the indices that rule 12 and `hello`
        // read (this test measures reads; it checks no invariant).
        let uq = s.ix.pending_uq_by.entry(p).or_default();
        for i in 0..questions {
            uq.insert(UqRank {
                neg_priority: 0,
                due: later,
                q: Eid(1_000_000_000 + i),
            });
        }
        let ds = s.ix.pending_digest_by.entry(p).or_default();
        for i in 0..digest {
            ds.insert(Eid(2_000_000_000 + i));
        }
        // A third of the hot node's requests, decided by another session: owed a reply.
        let owed: BTreeSet<Eid> = s
            .requests
            .iter()
            .filter(|(_, q)| q.node == f.hot)
            .map(|(r, _)| *r)
            .take(z.requests.div_ceil(3))
            .collect();
        let (_, out) = apply(
            &mut s,
            &ctx_at(now, SessionCtx::Existing(f.worker), &workers),
            &Cmd::Talk(TalkCmd::Decide {
                at: NodeRef::Id(f.hot),
                text: Text("decided".into()),
                settles: owed,
                supersedes: None,
                basis: Vec::new(),
                evidence: None,
            }),
        );
        assert!(out.is_ok(), "{out:?}");
        // The caller takes the hot node.
        let (_, out) = apply(
            &mut s,
            &ctx_at(now, SessionCtx::Existing(x), &assistant),
            &Cmd::Hold(HoldCmd::Take {
                target: NodeRef::Id(f.hot),
                lease: None,
            }),
        );
        assert!(out.is_ok(), "{out:?}");
        // An unheld hub with as many requests, by events.
        let hub = s.counters.next_node;
        let mut hub_init = init("hub", false);
        hub_init.keys = [(KeyStr::new("chat:y/1").unwrap(), NormVer(1))].into();
        ev(
            &mut s,
            Ev::NodeCreated {
                id: hub,
                parent: NodeId(0),
                name: Name::new("hub").unwrap(),
                origin: Origin::Agent(SessionId(1)),
                init: hub_init,
            },
        );
        for i in 0..z.requests {
            let eid = s.counters.next_eid;
            let seq = Seq(s.nodes[&hub].seq.0 + 1);
            ev(
                &mut s,
                Ev::Appended {
                    eid,
                    node: hub,
                    seq,
                    by: Author::Session(f.worker),
                    body: EntryBody::Request {
                        origin: key(&format!("chat:y/1/{i}")),
                        conv: KeyStr::new("chat:y/1").unwrap(),
                        from_user: None,
                        requester: Requester::External,
                    },
                    text: EntryText::default(),
                },
            );
        }
        // People entries by another session on the hot node, after the take.
        for i in 0..z.requests {
            let (_, out) = apply(
                &mut s,
                &ctx_at(later, SessionCtx::Existing(f.worker), &workers),
                &Cmd::Talk(TalkCmd::Request {
                    at: NodeRef::Id(f.hot),
                    text: Text("more".into()),
                    origin: key(&format!("chat:x/1/late{i}")),
                    where_: Some(key("chat:x/1")),
                    requester: Text("Bob".into()),
                }),
            );
            assert!(out.is_ok(), "{out:?}");
        }
        let caller = ctx_at(later, SessionCtx::Existing(x), &assistant);
        let read = Cmd::View(ViewCmd::Constitution);
        let mut s1 = s.clone();
        let (_, read_out) = apply(&mut s1, &caller, &read);
        assert!(read_out.is_ok(), "{read_out:?}");
        let request = Cmd::Talk(TalkCmd::Request {
            at: NodeRef::Id(hub),
            text: Text("thanks".into()),
            origin: key("chat:y/1/caller"),
            where_: Some(key("chat:y/1")),
            requester: Text("Bob".into()),
        });
        let mut s2 = s.clone();
        let (_, req_out) = apply(&mut s2, &caller, &request);
        assert!(
            matches!(
                req_out,
                Ok(Done {
                    reply: Reply::Request {
                        existing: false,
                        ..
                    },
                    ..
                })
            ),
            "{req_out:?}"
        );
        let mut visits = Vec::new();
        for (what, st, cmd, out) in [
            ("read", &s1, &read, &read_out),
            ("request on a hub", &s2, &request, &req_out),
        ] {
            let mut m = CountMeter::default();
            let g = query::guide(st, Some(x), later, cmd, out, &mut m);
            assert!(g.items.len() <= query::guide::NEXT_CALLS_MAX);
            let rules: Vec<_> = query::guide::all_items(st, Some(x), later, cmd, out, None)
                .0
                .iter()
                .map(|i| i.rule)
                .collect();
            let t = time(30, || {
                let _ = query::guide(st, Some(x), later, cmd, out, &mut NoMeter);
            });
            visits.push((what, m.0, t, rules));
        }
        visits
    };
    let a = run(&small, &SMALL, (10, 10));
    let b = run(&large, &LARGE, (10_000, 100_000));
    // The caps of D.8.26 that the large forum reaches and the small one does not: the requests
    // owed a reply that rule 6 reads, and the news that rule 4 counts.
    let caps = u64::from(query::guide::NEXT_TOTAL_CAP) + query::MOVED_MAX as u64 + 1;
    for ((what, va, ta, ra), (_, vb, tb, rb)) in a.iter().zip(&b) {
        eprintln!("guide after {what}: small {va} visits {ra:?}, large {vb} visits {rb:?}");
        assert_eq!(
            ra, rb,
            "guide after {what}: the same rules fire in both forums"
        );
        assert!(
            *vb <= va + caps,
            "guide after {what}: {vb} visits in the large forum, {va} in the small"
        );
        ratio_ok(&format!("guide after {what}"), *ta, *tb);
    }
    use query::guide::Rule;
    assert!(
        [Rule::Reread, Rule::Reply, Rule::HumanItems]
            .iter()
            .all(|r| a[0].3.contains(r)),
        "the read reaches rules 4, 6, and 12: {:?}",
        a[0].3
    );
    assert!(
        a[1].3.contains(&Rule::Decide),
        "the request reaches rule 7: {:?}",
        a[1].3
    );
}
