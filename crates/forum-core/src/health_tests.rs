//! Unit tests of `query::health` and `query::m1b_gaps` (design D.8.22, D.9.3, D.12.1; §17, CS-9,
//! CS-31): every `HealthReason` on a built state, the shares and ages that make a counted reason
//! red, the order of the reasons, the accounting of supply, lost time, and idleness, and M1b's
//! gaps. States are built through the commands where they can be (`World`, with every check after
//! every call), and the statistics' days by hand where a reason reads only them.

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
use alloc::vec::Vec;

use super::*;
use crate::apply::tests::{class, create_with, created, label, ok, take, text};
use crate::cmd::{
    Cmd, ControlCmd, HoldCmd, NextQuery, NodeCmd, Patch, SessionCmd, SessionCtx, WaitArg,
};
use crate::event::{Ev, NodeInit};
use crate::params::Params;
use crate::query;
use crate::reply::Reply;
use crate::state::{DayStats, Origin, State};
use crate::testkit::World;
use crate::types::{
    Change, DAY, MINUTE, Name, NodeRef, Outcome, ROOT, ReleaseReason, SessionId, Text, Time, Token,
};

const START: Time = Time(1_790_845_200_000);

fn world() -> World {
    World::new(Params::default(), START)
}

/// Opens a session with `caps` and `horizon` through `hello`.
fn hello(w: &mut World, caps: &[&str], horizon: Option<u32>) -> SessionId {
    let ctx = w.ctx(&class("worker"), SessionCtx::New, None);
    let cmd = Cmd::Session(SessionCmd::Hello {
        intent: None,
        caps: Some(caps.iter().map(|l| label(l)).collect()),
        horizon,
        principal: None,
        from_user: None,
    });
    ok(w.call_ctx(&ctx, &cmd)).opened.unwrap()
}

/// `count` offered candidates under ROOT named `<prefix><i>`, with `needs` and `est`, by their
/// events alone and unchecked (health is a function of the state).
fn offered(w: &mut World, prefix: &str, count: usize, needs: &[&str], est: Option<u32>) {
    offered_at(w, prefix, count, needs, est, 0);
}

/// `offered` with a priority.
fn offered_at(
    w: &mut World,
    prefix: &str,
    count: usize,
    needs: &[&str],
    est: Option<u32>,
    priority: i32,
) {
    for i in 0..count {
        let id = w.s.counters.next_node;
        w.emit_raw(Ev::NodeCreated {
            id,
            parent: ROOT,
            name: Name::new(&alloc::format!("{prefix}{i}")).unwrap(),
            origin: Origin::Agent(SessionId(0)),
            init: NodeInit {
                title: Text(String::from("w")),
                body: Text::default(),
                fields: BTreeMap::new(),
                status: Text::default(),
                digest: Text::default(),
                agreements: Text::default(),
                kind: None,
                tags: [label("offer")].into(),
                capacity: 1,
                priority,
                est,
                needs: needs.iter().map(|l| label(l)).collect(),
                due: None,
                depends_on: BTreeSet::new(),
                owners: BTreeSet::new(),
                keys: BTreeMap::new(),
                links: BTreeSet::new(),
                role: crate::state::Role::Plain,
            },
        });
    }
}

fn reason(h: &Health, f: impl Fn(&HealthReason) -> bool) -> Option<&HealthReason> {
    h.reasons.iter().find(|r| f(r))
}

fn unreachable_of(h: &Health) -> Option<(Level, u32, u32, u32)> {
    h.reasons.iter().find_map(|r| match r {
        HealthReason::Unreachable {
            level,
            count,
            by_needs,
            by_horizon,
            ..
        } => Some((*level, *count, *by_needs, *by_horizon)),
        _ => None,
    })
}

// ------------------------------------------------------------------------------------------------
// Paused

/// A pause of ROOT is red; a pause of a small scope is yellow, and lists what it excludes, those
/// that exclude the most first (CS-9).
#[test]
fn a_root_pause_is_red_and_a_small_one_yellow() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    offered(&mut w, "w", 30, &[], None);
    let (small, _) = created(&ok(w.call(
        &class("worker"),
        s,
        create_with("/small/a", |a| {
            a.take = Some(false);
            a.tags = [label("offer")].into();
        }),
    )));
    let pause = |scope: NodeRef| {
        Cmd::Control(ControlCmd::Pause {
            scope,
            reason: text("checking"),
        })
    };
    let parent = w.s.nodes[&small].parent().unwrap();
    ok(w.call(&class("worker"), s, pause(NodeRef::Id(parent))));
    let h = query::health(&w.s, w.clock);
    match reason(&h, |r| matches!(r, HealthReason::Paused { .. })) {
        Some(HealthReason::Paused { level, count, top }) => {
            assert_eq!((*level, *count), (Level::Yellow, 1));
            assert_eq!(top[0].pause.scope, parent);
            assert_eq!(top[0].excluded, 1);
        }
        other => panic!("{other:?}"),
    }
    // With no `next` call the idle share is unknown, so the backlog with no supply is not
    // `Overloaded` (§17, 1.5): the small pause alone makes the level yellow.
    assert_eq!(h.reasons.len(), 1, "{:?}", h.reasons);
    assert_eq!(h.level, Level::Yellow);
    ok(w.call(&class("worker"), s, pause(NodeRef::Id(ROOT))));
    let h = query::health(&w.s, w.clock);
    match &h.reasons[0] {
        HealthReason::Paused { level, count, top } => {
            assert_eq!((*level, *count), (Level::Red, 2));
            // ROOT holds back the 30 others; the small pause, nearer, keeps its own.
            assert_eq!((top[0].pause.scope, top[0].excluded), (ROOT, 30));
            assert_eq!((top[1].pause.scope, top[1].excluded), (parent, 1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(h.level, Level::Red);
}

// ------------------------------------------------------------------------------------------------
// Unreachable

/// One candidate no profile can take, by needs, is yellow at 1 in 10 and at 1 in 1,000, since only
/// its age makes it red (1.5: at 1 in 10 a small forum read red at once); by horizon the same; with
/// no session profile within `capacity.window` at all, every candidate is unreachable (CS-9,
/// RS-18).
#[test]
fn unreachable_by_needs_and_by_horizon_at_two_shares() {
    for (others, want) in [(9, Level::Yellow), (999, Level::Yellow)] {
        let mut w = world();
        let _ = hello(&mut w, &[], Some(20));
        offered(&mut w, "ok", others, &[], Some(10));
        offered(&mut w, "x", 1, &["x"], None);
        let h = query::health(&w.s, w.clock);
        assert_eq!(usize::try_from(h.capacity.backlog).unwrap(), others + 1);
        assert_eq!(
            unreachable_of(&h),
            Some((want, 1, 1, 0)),
            "by needs, {others}"
        );
        let mut w = world();
        let _ = hello(&mut w, &[], Some(20));
        offered(&mut w, "ok", others, &[], Some(10));
        offered(&mut w, "long", 1, &[], Some(64));
        let h = query::health(&w.s, w.clock);
        assert_eq!(
            unreachable_of(&h),
            Some((want, 1, 0, 1)),
            "by horizon, {others}"
        );
    }
    // A bucket that straddles the horizon is read per candidate: 30 does not fit 20, 18 does.
    let mut w = world();
    let _ = hello(&mut w, &[], Some(20));
    offered(&mut w, "a", 1, &[], Some(30));
    offered(&mut w, "b", 1, &[], Some(18));
    let h = query::health(&w.s, w.clock);
    assert_eq!(unreachable_of(&h).map(|x| (x.1, x.3)), Some((1, 1)));
    // A session with no horizon, or with the caps, reaches them all.
    let _ = hello(&mut w, &["x"], None);
    offered(&mut w, "c", 1, &["x"], Some(500));
    let h = query::health(&w.s, w.clock);
    assert_eq!(unreachable_of(&h), None);
    // No profile within the window: every candidate is unreachable, and yellow while young.
    let mut w = world();
    offered(&mut w, "d", 3, &[], None);
    let h = query::health(&w.s, w.clock);
    assert_eq!(unreachable_of(&h), Some((Level::Yellow, 3, 3, 0)));
    assert_eq!(h.level, Level::Yellow);
}

/// 1.11 (§17): the health a session is shown counts its caps and horizon, which the cached result
/// may predate: the listed `Unreachable` items it reaches are left out, the reason once none is
/// left, and the level is the worst of the rest; what is left equals the next computation's. In
/// the lab's sixth round, `hello` told the only sessions with a11y, and a Lean prover, that no
/// session had their caps.
#[test]
fn a_session_is_shown_the_health_with_its_own_caps_and_horizon_counted() {
    let mut w = world();
    let _ = hello(&mut w, &[], Some(20));
    offered(&mut w, "x", 1, &["x"], None);
    offered(&mut w, "long", 1, &[], Some(64));
    let h = query::health(&w.s, w.clock);
    assert_eq!(unreachable_of(&h), Some((Level::Yellow, 2, 1, 1)));
    let caps = |xs: &[&str]| -> BTreeSet<Label> { xs.iter().map(|l| label(l)).collect() };
    // Caps that meet `x`'s needs, and a horizon that does not cover `long`: `x` is left out.
    let shown = counting(&h, &w.s, &caps(&["x"]), Some(20));
    assert_eq!(unreachable_of(&shown), Some((Level::Yellow, 1, 0, 1)));
    assert_eq!(shown.level, Level::Yellow);
    // Without a horizon too, nothing is left: the reason goes, and the level is green.
    let shown = counting(&h, &w.s, &caps(&["x"]), None);
    assert_eq!(unreachable_of(&shown), None);
    assert_eq!(shown.level, Level::Green);
    // Caps that meet only what needs nothing: as cached, but for `long`, which needs nothing and
    // is shown for the session's horizon (1.12).
    let shown_y = counting(&h, &w.s, &caps(&["y"]), Some(20));
    assert_eq!(unreachable_of(&shown_y), unreachable_of(&h));
    assert!(shown_y.reasons.iter().any(|r| matches!(
        r,
        HealthReason::Unreachable { top, .. }
            if top.iter().any(|(_, by)| *by == UnreachableBy::Yours { horizon: 20 })
                && top.iter().any(|(_, by)| *by == UnreachableBy::Needs)
    )));
    // 1.12: `long`, whose needs the caps meet, is shown for the session's horizon.
    let shown = counting(&h, &w.s, &caps(&["x"]), Some(20));
    assert!(shown.reasons.iter().any(|r| matches!(
        r,
        HealthReason::Unreachable { top, .. }
            if top.iter().any(|(_, by)| *by == UnreachableBy::Yours { horizon: 20 })
    )));
    // What is left is what the next computation finds once the session has arrived, but for the
    // words of what its own horizon keeps it from (1.12).
    let _ = hello(&mut w, &["x"], Some(20));
    let next = query::health(&w.s, w.clock);
    let mut left = counting(&h, &w.s, &caps(&["x"]), Some(20)).reasons;
    for r in &mut left {
        if let HealthReason::Unreachable { top, .. } = r {
            for (_, by) in top.iter_mut() {
                if matches!(by, UnreachableBy::Yours { .. }) {
                    *by = UnreachableBy::Horizon;
                }
            }
        }
    }
    assert_eq!(left, next.reasons);
}

/// 1.12 (§17; it6 l5 F4): a session whose caps meet a node's needs, and whose horizon does not
/// cover its `est`, is shown that its own horizon and every other session's keep it from the
/// node, never that no session has the caps, which the cached result may say: a Lean prover with
/// a 45-minute horizon was told that no session had lean.
#[test]
fn the_unreachable_reason_is_computed_for_the_session_shown_it() {
    let mut w = world();
    let _ = hello(&mut w, &["numerics"], None);
    offered(&mut w, "lemma-01", 1, &["lean"], Some(60));
    let h = query::health(&w.s, w.clock);
    assert_eq!(unreachable_of(&h), Some((Level::Yellow, 1, 1, 0)));
    let caps: BTreeSet<Label> = [label("lean"), label("shell")].into_iter().collect();
    let shown = counting(&h, &w.s, &caps, Some(45));
    assert_eq!(unreachable_of(&shown), Some((Level::Yellow, 1, 0, 1)));
    let HealthReason::Unreachable { top, .. } = &shown.reasons[0] else {
        panic!("{shown:?}");
    };
    assert_eq!(top[0].1, UnreachableBy::Yours { horizon: 45 });
    // Without the caps, as cached; with a horizon that covers it, nothing is left.
    assert_eq!(counting(&h, &w.s, &BTreeSet::new(), Some(45)), h);
    assert_eq!(unreachable_of(&counting(&h, &w.s, &caps, Some(60))), None);
}

/// An unreachable candidate turns red once its `since` is `capacity.window` old, at any share, and
/// not a millisecond before (D.8.22: red by age).
#[test]
fn an_old_unreachable_candidate_is_red_at_a_small_share() {
    let mut w = world();
    offered(&mut w, "x", 1, &["x"], None);
    w.advance(Dur(7 * DAY.0 - 1));
    let _ = hello(&mut w, &[], None);
    offered(&mut w, "ok", 999, &[], None);
    let h = query::health(&w.s, w.clock);
    assert_eq!(
        unreachable_of(&h).map(|x| (x.0, x.1)),
        Some((Level::Yellow, 1))
    );
    w.advance(Dur(1));
    let h = query::health(&w.s, w.clock);
    assert_eq!(
        unreachable_of(&h).map(|x| (x.0, x.1)),
        Some((Level::Red, 1))
    );
}

/// The oldest item of a partition is found past a younger one of a higher tier: a partition is
/// ordered by tier before age (D.8.5), so its first entry alone would hide an old ordinary
/// candidate behind a young prioritised one (1.5, D.8.22).
#[test]
fn the_oldest_item_is_found_past_a_younger_one_of_a_higher_tier() {
    let mut w = world();
    offered(&mut w, "old", 1, &["x"], None);
    w.advance(Dur(8 * DAY.0));
    let _ = hello(&mut w, &[], None);
    offered_at(&mut w, "young", 1, &["x"], None, 5);
    let young = w.s.names[&(ROOT, Name::new("young0").unwrap())];
    let part = &w.s.ix.next.of[&Cand::Node(young)].part;
    let first = w.s.ix.next.parts[part].iter().next().unwrap();
    assert_eq!((first.id, first.tier), (young.0, 1));
    let h = query::health(&w.s, w.clock);
    assert_eq!(
        unreachable_of(&h).map(|x| (x.0, x.1)),
        Some((Level::Red, 2))
    );
}

// ------------------------------------------------------------------------------------------------
// NoMover (M1b)

/// M1b's gaps: flags on a node of capacity 0 (nobody can take it); a `{node}` wait on a node
/// nothing offers; a dependency on such a node; and none for a chain that ends at a held node, a
/// candidate, or a node a deadline moves (D.9.3).
#[test]
fn no_mover_gaps() {
    let mut w = world();
    let c = class("worker");
    let s = hello(&mut w, &[], None);
    let mk = |w: &mut World, p: &str, f: &dyn Fn(&mut crate::cmd::CreateArgs)| {
        created(&ok(w.call(
            &c,
            s,
            create_with(p, |a| {
                a.take = Some(false);
                f(a);
            }),
        )))
        .0
    };
    assert!(query::m1b_gaps(&w.s, w.clock).is_empty());
    // A target nobody offers, and a waiter on it.
    let idle = mk(&mut w, "/idle", &|_| {});
    let waiter = mk(&mut w, "/waiter", &|_| {});
    let t = match ok(w.call(&c, s, take(waiter))).reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    };
    let release_on = |t: Token, n: NodeId| {
        Cmd::Hold(HoldCmd::Release {
            token: t,
            done: text("half"),
            next_step: text("wait"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Node(NodeRef::Id(n))),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        })
    };
    ok(w.call(&c, s, release_on(t, idle)));
    // A dependent of the same target, offered.
    let dep = mk(&mut w, "/dependent", &|a| {
        a.tags = [label("offer")].into();
        a.depends_on = alloc::vec![NodeRef::Id(idle)];
    });
    let gaps = query::m1b_gaps(&w.s, w.clock);
    assert!(gaps.contains(&MoverGap::NodeWait {
        node: waiter,
        target: idle
    }));
    assert!(gaps.contains(&MoverGap::Dependency {
        node: dep,
        target: idle
    }));
    let h = query::health(&w.s, w.clock);
    match reason(&h, |r| matches!(r, HealthReason::NoMover { .. })) {
        Some(HealthReason::NoMover { count, .. }) => assert_eq!(*count, 2),
        other => panic!("{other:?}"),
    }
    // Offering the target ends both chains at a candidate.
    let v = w.s.nodes[&idle].version;
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Update {
            at: NodeRef::Id(idle),
            version: v,
            digest_version: None,
            token: None,
            patch: Patch {
                tags_add: [label("offer")].into(),
                ..Patch::default()
            },
        }),
    ));
    assert!(query::m1b_gaps(&w.s, w.clock).is_empty());
    // Flags on a node of capacity 0: nobody can take it.
    let zero = mk(&mut w, "/zero", &|a| {
        a.capacity = Some(0);
        a.due = Some(crate::types::When::In(Dur(MINUTE.0)));
    });
    w.advance(Dur(2 * MINUTE.0));
    assert!(!w.s.ix.node[&zero].flags.is_empty());
    let gaps = query::m1b_gaps(&w.s, w.clock);
    assert!(
        gaps.iter()
            .any(|g| matches!(g, MoverGap::FlaggedUnreachable { node, .. } if *node == zero)),
        "{gaps:?}"
    );
    // A chain through a waiting node ends at a deadline: a waiter on a node that waits for a time.
    let timed = mk(&mut w, "/timed", &|_| {});
    let s2 = hello(&mut w, &[], None);
    let t = match ok(w.call(&c, s2, take(timed))).reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    };
    ok(w.call(
        &c,
        s2,
        Cmd::Hold(HoldCmd::Release {
            token: t,
            done: text("half"),
            next_step: text("wait"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Time(crate::types::When::In(Dur(DAY.0)))),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
    let w2 = mk(&mut w, "/second-waiter", &|_| {});
    let t = match ok(w.call(&c, s2, take(w2))).reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    };
    ok(w.call(&c, s2, release_on(t, timed)));
    let gaps = query::m1b_gaps(&w.s, w.clock);
    assert!(
        !gaps
            .iter()
            .any(|g| matches!(g, MoverGap::NodeWait { node, .. } if *node == w2)),
        "{gaps:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// The statistics' reasons

/// A state whose `stats.days` are `days` (by offset from today, `flagged_end`), with
/// `flagged_active` set to `now` nodes, for `FlagsPilingUp`, which reads only these.
fn piling(days: &[(u64, u32)], now: u32) -> Option<HealthReason> {
    let mut w = world();
    let today = crate::index::day_of(w.clock);
    for &(back, f) in days {
        w.s.stats.days.insert(
            today - back,
            DayStats {
                flagged_end: f,
                ..DayStats::default()
            },
        );
    }
    let mut s: State = w.s.clone();
    s.ix.flagged_active = (0..u64::from(now)).map(NodeId).collect();
    compute(&s, w.clock)
        .reasons
        .into_iter()
        .find(|r| matches!(r, HealthReason::FlagsPilingUp { .. }))
}

/// Flagged nodes that keep growing over the window and on each of the last three days are red;
/// growth below the margin, or a day without growth, is not (CS-31).
#[test]
fn flags_piling_up() {
    // 10 at the window's start, then growth on each of the last three days to 100.
    let up = piling(&[(6, 10), (2, 40), (1, 70)], 100);
    assert_eq!(
        up,
        Some(HealthReason::FlagsPilingUp {
            start: 10,
            now: 100
        })
    );
    // Below max(50, 10 %) over the window.
    assert_eq!(piling(&[(6, 10), (2, 20), (1, 40)], 55), None);
    // A day without growth (the count carried forward over day −2).
    assert_eq!(piling(&[(6, 10), (3, 40), (1, 70)], 100), None);
    // Today without growth.
    assert_eq!(piling(&[(6, 10), (2, 40), (1, 100)], 100), None);
}

/// The capacity reasons: demand above supply while rarely idle is `Overloaded` (yellow with no
/// candidate that has waited `capacity.window`, 1.5); idle above half with no backlog is
/// `Underused` (yellow); and the report sums the window's days.
#[test]
fn overloaded_and_underused() {
    let mut w = world();
    let today = crate::index::day_of(w.clock);
    w.s.stats.days.insert(
        today,
        DayStats {
            demand_min: 600,
            supply_ms: 120 * 60_000,
            next_calls: 10,
            next_empty: 1,
            ..DayStats::default()
        },
    );
    // A day outside the window is not read.
    w.s.stats.days.insert(
        today - 8,
        DayStats {
            demand_min: 1_000_000,
            ..DayStats::default()
        },
    );
    let h = compute(&w.s, w.clock);
    assert_eq!(
        reason(&h, |r| matches!(r, HealthReason::Overloaded { .. })),
        Some(&HealthReason::Overloaded {
            level: Level::Yellow,
            demand_min: 600,
            supply_min: 120,
            idle_permille: 100,
            oldest: None,
        })
    );
    assert_eq!(h.level, Level::Yellow);
    let mut w = world();
    let three = [Some(SessionId(1)), Some(SessionId(2)), Some(SessionId(3))];
    w.s.stats.days.insert(
        today,
        DayStats {
            next_calls: 20,
            next_empty: 12,
            next_callers: three,
            ..DayStats::default()
        },
    );
    let h = compute(&w.s, w.clock);
    assert_eq!(
        h.reasons,
        alloc::vec![HealthReason::Underused { idle_permille: 600 }]
    );
    assert_eq!(h.level, Level::Yellow);
    // 1.7 (§17): fewer than 20 `next` calls, or calls from fewer than 3 sessions in the window,
    // are no `Underused`: one idle worker's empty `next` says nothing about the swarm.
    for (calls, empty, callers) in [
        (19, 12, three),
        (20, 12, [Some(SessionId(1)), Some(SessionId(2)), None]),
        (1, 1, [Some(SessionId(1)), None, None]),
    ] {
        w.s.stats.days.insert(
            today,
            DayStats {
                next_calls: calls,
                next_empty: empty,
                next_callers: callers,
                ..DayStats::default()
            },
        );
        assert!(compute(&w.s, w.clock).reasons.is_empty());
    }
    // Callers count across the window's days, each once.
    w.s.stats.days.insert(
        today,
        DayStats {
            next_calls: 10,
            next_empty: 6,
            next_callers: [Some(SessionId(1)), Some(SessionId(2)), None],
            ..DayStats::default()
        },
    );
    w.s.stats.days.insert(
        today - 1,
        DayStats {
            next_calls: 10,
            next_empty: 6,
            next_callers: [Some(SessionId(2)), Some(SessionId(3)), None],
            ..DayStats::default()
        },
    );
    assert_eq!(
        compute(&w.s, w.clock).reasons,
        alloc::vec![HealthReason::Underused { idle_permille: 600 }]
    );
    w.s.stats.days.remove(&(today - 1));
    // An empty forum with no calls is green.
    let w = world();
    assert_eq!(query::health(&w.s, w.clock).level, Level::Green);
    // With no `next` call in the window, the idle share is unknown, and neither capacity reason
    // fires, though demand exceeds supply: agents that take nodes directly are not overloaded by
    // this measure (§17, 1.5; §32 over HTTP read red from its two-hour wait on).
    let mut w = world();
    w.s.stats.days.insert(
        today,
        DayStats {
            demand_min: 600,
            supply_ms: 120 * 60_000,
            ..DayStats::default()
        },
    );
    let h = compute(&w.s, w.clock);
    assert_eq!(h.capacity.idle_permille, None);
    assert_eq!(h.capacity.next_calls, 0);
    assert!(h.reasons.is_empty(), "{:?}", h.reasons);
    assert_eq!(h.level, Level::Green);
    // One `next` call that found work is a known idle share of 0: overloaded again, and yellow,
    // as a young forum's first `next` is (1.5: it read red at once).
    w.s.stats.days.get_mut(&today).unwrap().next_calls = 1;
    let h = compute(&w.s, w.clock);
    assert_eq!(h.capacity.idle_permille, Some(0));
    assert_eq!(overloaded_level(&h), Some(Level::Yellow));
    assert_eq!(h.level, Level::Yellow);
}

fn overloaded_level(h: &Health) -> Option<Level> {
    h.reasons.iter().find_map(|r| match r {
        HealthReason::Overloaded { level, .. } => Some(*level),
        _ => None,
    })
}

/// `Overloaded` turns red once its oldest candidate has waited `capacity.window`, and not a
/// millisecond before; a candidate held back for every caller is not its item (1.5, D.8.22).
#[test]
fn overloaded_is_red_once_its_oldest_candidate_has_waited_the_window() {
    let busy = DayStats {
        demand_min: 600,
        supply_ms: 120 * 60_000,
        next_calls: 10,
        next_empty: 1,
        ..DayStats::default()
    };
    let with_busy_day = |w: &World| {
        let mut s = w.s.clone();
        s.stats.days.insert(crate::index::day_of(w.clock), busy);
        compute(&s, w.clock)
    };
    let oldest = |h: &Health| {
        h.reasons.iter().find_map(|r| match r {
            HealthReason::Overloaded { oldest, .. } => Some(*oldest),
            _ => None,
        })
    };
    let mut w = world();
    let first = w.s.counters.next_node;
    offered(&mut w, "w", 1, &[], None);
    assert_eq!(overloaded_level(&with_busy_day(&w)), Some(Level::Yellow));
    // 1.6: the reason names the candidate whose age makes its level.
    assert_eq!(oldest(&with_busy_day(&w)), Some(Some(Cand::Node(first))));
    w.advance(Dur(7 * DAY.0 - 1));
    // A younger candidate is not the oldest.
    offered(&mut w, "y", 1, &[], None);
    assert_eq!(overloaded_level(&with_busy_day(&w)), Some(Level::Yellow));
    assert_eq!(oldest(&with_busy_day(&w)), Some(Some(Cand::Node(first))));
    w.advance(Dur(1));
    let h = with_busy_day(&w);
    assert_eq!(overloaded_level(&h), Some(Level::Red));
    assert_eq!(h.level, Level::Red);
    // Paused, the old candidate is held back: it is the pause's item, not the overload's.
    let s = hello(&mut w, &[], None);
    ok(w.call(&class("worker"), s, pause(ROOT)));
    assert_eq!(overloaded_level(&with_busy_day(&w)), Some(Level::Yellow));
    assert_eq!(oldest(&with_busy_day(&w)), Some(None));
}

/// 1.10 (§10.4): review items rank by priority and the ring, not by age, so the first review of a
/// partition is not its oldest: `Overloaded` reads the whole review band, and names the old review
/// behind a young one of a higher priority.
#[test]
fn the_oldest_review_is_found_behind_a_younger_one_of_a_higher_priority() {
    let busy = DayStats {
        demand_min: 600,
        supply_ms: 120 * 60_000,
        next_calls: 10,
        next_empty: 1,
        ..DayStats::default()
    };
    let oldest = |w: &World| {
        let mut s = w.s.clone();
        s.stats.days.insert(crate::index::day_of(w.clock), busy);
        compute(&s, w.clock).reasons.iter().find_map(|r| match r {
            HealthReason::Overloaded { oldest, .. } => Some(*oldest),
            _ => None,
        })
    };
    let mut w = world();
    let c = class("worker");
    let s = hello(&mut w, &[], None);
    let review = |w: &mut World, s: SessionId, p: &str, priority: i32| {
        let n = created(&ok(w.call(
            &c,
            s,
            create_with(p, |a| {
                a.take = Some(false);
                a.priority = Some(priority);
            }),
        )))
        .0;
        ok(w.call(
            &c,
            s,
            Cmd::Node(NodeCmd::Close {
                at: NodeRef::Id(n),
                outcome: Outcome::Done,
                summary: text("done"),
                verified: None,
                settles: None,
                learnings: None,
                token: None,
                unsent: BTreeSet::new(),
            }),
        ));
        ok(w.call(
            &c,
            s,
            Cmd::Talk(crate::cmd::TalkCmd::Object {
                on: NodeRef::Id(n),
                note: text("it does not hold"),
            }),
        ));
        n
    };
    let old = review(&mut w, s, "/old", 0);
    w.advance(Dur(2 * DAY.0));
    let s2 = hello(&mut w, &[], None);
    let young = review(&mut w, s2, "/young", 5);
    let part = &w.s.ix.next.of[&Cand::Status(young)].part;
    let first = w.s.ix.next.parts[part].iter().next().unwrap();
    assert_eq!(first.id, young.0, "the young review leads its band");
    assert_eq!(oldest(&w), Some(Some(Cand::Status(old))));
}

/// The accounting (CS-31): a hold that made progress counts as supply, and one that expired
/// without a checkpoint as lost time, apart; a `next` that found work only for others is an empty
/// for the caller, not idleness; a live hold counts its time so far.
#[test]
fn supply_lost_time_and_idleness() {
    let mut w = world();
    let c = class("worker");
    let a = hello(&mut w, &[], None);
    let (n1, t1) = created(&ok(w.call(&c, a, create_with("/one", |_| {}))));
    w.advance(Dur(10 * MINUTE.0));
    ok(w.call(
        &c,
        a,
        Cmd::Hold(HoldCmd::Release {
            token: t1.unwrap(),
            done: text("done"),
            next_step: text("none"),
            reason: ReleaseReason::Handoff,
            waits_for: None,
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
    let _ = n1;
    let b = hello(&mut w, &[], None);
    let (_, t2) = created(&ok(w.call(&c, b, create_with("/two", |_| {}))));
    assert!(t2.is_some());
    w.advance(Dur(16 * MINUTE.0));
    // The lease of `/two` expired at 15 minutes with no checkpoint: lost.
    let h = query::health(&w.s, w.clock);
    assert_eq!(h.capacity.supply_min, 10);
    assert_eq!(h.capacity.lost_min, 15);
    // Two `next` calls: one finds nothing at all, one only work its caller cannot take.
    let x = hello(&mut w, &[], None);
    for p in ["one", "two"] {
        let n = w.s.names[&(ROOT, Name::new(p).unwrap())];
        ok(w.call(&c, x, crate::apply::tests::close(n, Outcome::Done)));
    }
    let next = Cmd::Hold(HoldCmd::Next {
        query: NextQuery::default(),
        lease: None,
    });
    ok(w.call(&c, x, next.clone()));
    ok(w.call(
        &c,
        x,
        create_with("/needs-y", |a| {
            a.take = Some(false);
            a.tags = [label("offer")].into();
            a.needs = Some([label("y")].into());
        }),
    ));
    ok(w.call(&c, x, next));
    let h = query::health(&w.s, w.clock);
    assert_eq!(h.capacity.next_calls, 2);
    assert_eq!(
        (h.capacity.next_empty, h.capacity.next_empty_for_you),
        (1, 1)
    );
    assert_eq!(h.capacity.idle_permille, Some(500));
    // A live hold counts its time so far.
    let (_, t) = created(&ok(w.call(&c, x, create_with("/three", |_| {}))));
    assert!(t.is_some());
    w.advance(Dur(5 * MINUTE.0));
    let h2 = query::health(&w.s, w.clock);
    assert_eq!(h2.capacity.supply_min, h.capacity.supply_min + 5);
}

/// The reasons come red first, then in their declaration order, and the level is the worst
/// (§17, CS-9).
#[test]
fn reasons_red_first_then_in_order() {
    let h = finish(
        alloc::vec![
            HealthReason::Underused { idle_permille: 900 },
            HealthReason::Unreachable {
                level: Level::Yellow,
                count: 1,
                by_needs: 1,
                by_horizon: 0,
                top: Vec::new(),
            },
            HealthReason::FlagsPilingUp { start: 1, now: 99 },
            HealthReason::NoMover {
                level: Level::Red,
                count: 1,
                top: Vec::new(),
            },
        ],
        CapacityReport::default(),
    );
    let order: Vec<u8> = h.reasons.iter().map(rank).collect();
    assert_eq!(order, alloc::vec![1, 3, 2, 6]);
    assert_eq!(h.level, Level::Red);
    assert_eq!(
        finish(Vec::new(), CapacityReport::default()).level,
        Level::Green
    );
}

/// `share_level`: none without an item; yellow below 1 % of the population; red from 1 %, or by
/// age.
#[test]
fn share_level_by_share_and_by_age() {
    let now = START;
    let window = Dur(7 * DAY.0);
    assert_eq!(share_level(0, 10, None, now, window), None);
    assert_eq!(share_level(1, 100, None, now, window), Some(Level::Red));
    assert_eq!(share_level(1, 101, None, now, window), Some(Level::Yellow));
    assert_eq!(
        share_level(1, 1_000, Some(now.minus(window)), now, window),
        Some(Level::Red)
    );
    assert_eq!(
        share_level(1, 1_000, Some(now.minus(Dur(window.0 - 1))), now, window),
        Some(Level::Yellow)
    );
    // `age_level` alone (1.5: `Unreachable` and `Overloaded`): no share, and no time is yellow.
    assert_eq!(age_level(None, now, window), Level::Yellow);
    assert_eq!(age_level(Some(now.minus(window)), now, window), Level::Red);
    assert_eq!(
        age_level(Some(now.minus(Dur(window.0 - 1))), now, window),
        Level::Yellow
    );
    let _ = Change::<u32>::Keep;
}

// ------------------------------------------------------------------------------------------------
// The review's regressions and boundaries (WP-D6 fix round)

/// A node created by `s` without a hold, with `f` applied to its arguments.
fn node(
    w: &mut World,
    s: SessionId,
    p: &str,
    f: impl FnOnce(&mut crate::cmd::CreateArgs),
) -> NodeId {
    created(&ok(w.call(
        &class("worker"),
        s,
        create_with(p, |a| {
            a.take = Some(false);
            f(a);
        }),
    )))
    .0
}

/// Takes `n` in `s` and releases it, blocked, waiting for `wait`.
fn wait_on(w: &mut World, s: SessionId, n: NodeId, wait: WaitArg) {
    let token = match ok(w.call(&class("worker"), s, take(n))).reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    };
    ok(w.call(
        &class("worker"),
        s,
        Cmd::Hold(HoldCmd::Release {
            token,
            done: text("half"),
            next_step: text("wait"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(wait),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    ));
}

fn pause(scope: NodeId) -> Cmd {
    Cmd::Control(ControlCmd::Pause {
        scope: NodeRef::Id(scope),
        reason: text("checking"),
    })
}

fn paused_level(h: &Health) -> Option<Level> {
    h.reasons.iter().find_map(|r| match r {
        HealthReason::Paused { level, .. } => Some(*level),
        _ => None,
    })
}

fn no_mover_level(h: &Health) -> Option<(Level, u32)> {
    h.reasons.iter().find_map(|r| match r {
        HealthReason::NoMover { level, count, .. } => Some((*level, *count)),
        _ => None,
    })
}

/// D.9.3: a chain ends well at a node that has a deadline-based mover. A target nobody offers, but
/// whose `due` deadline is queued, becomes `overdue` and a candidate at its due, so a `{node}` wait
/// on it is no gap; once due, it is a candidate. A target that an open dependency will hold back
/// even then is followed through that dependency.
#[test]
fn a_wait_on_a_target_with_a_queued_due_has_a_mover() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    let target = node(&mut w, s, "/target", |a| {
        a.due = Some(crate::types::When::In(Dur(DAY.0)));
    });
    let waiter = node(&mut w, s, "/waiter", |_| {});
    wait_on(&mut w, s, waiter, WaitArg::Node(NodeRef::Id(target)));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(target)));
    assert_eq!(query::m1b_gaps(&w.s, w.clock), Vec::new());
    assert_eq!(no_mover_level(&query::health(&w.s, w.clock)), None);
    // Due, but held back by a dependency on a node nothing moves: the chain goes on to it.
    let idle = node(&mut w, s, "/idle", |_| {});
    let held = node(&mut w, s, "/held", |a| {
        a.due = Some(crate::types::When::In(Dur(DAY.0)));
        a.depends_on = alloc::vec![NodeRef::Id(idle)];
    });
    let second = node(&mut w, s, "/second", |_| {});
    wait_on(&mut w, s, second, WaitArg::Node(NodeRef::Id(held)));
    let gaps = query::m1b_gaps(&w.s, w.clock);
    assert_eq!(
        gaps,
        alloc::vec![
            MoverGap::Dependency {
                node: held,
                target: idle
            },
            MoverGap::NodeWait {
                node: second,
                target: held
            },
        ]
    );
    // Past the due, the first target is an `overdue` candidate.
    w.advance(Dur(2 * DAY.0));
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(target)));
    let gaps = query::m1b_gaps(&w.s, w.clock);
    assert!(
        !gaps
            .iter()
            .any(|g| matches!(g, MoverGap::NodeWait { node, .. } if *node == waiter)),
        "{gaps:?}"
    );
}

/// A chain through a dependency (M1b, D.9.3): a waiter's target is an offered candidate held back
/// by an open dependency on a node nothing moves, so the wait has no mover; a chain that ends at a
/// waiting occurrence (its slot is its deadline) has one.
#[test]
fn chains_through_a_dependency_and_to_a_waiting_occurrence() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    let idle = node(&mut w, s, "/idle", |_| {});
    let target = node(&mut w, s, "/target", |a| {
        a.tags = [label("offer")].into();
        a.depends_on = alloc::vec![NodeRef::Id(idle)];
    });
    assert!(
        w.s.ix
            .next
            .held
            .get(&Cand::Node(target))
            .is_some_and(|h| h.depends)
    );
    let waiter = node(&mut w, s, "/waiter", |_| {});
    wait_on(&mut w, s, waiter, WaitArg::Node(NodeRef::Id(target)));
    let gaps = query::m1b_gaps(&w.s, w.clock);
    assert!(
        gaps.contains(&MoverGap::NodeWait {
            node: waiter,
            target
        }),
        "{gaps:?}"
    );
    // A series' waiting occurrence moves by its slot.
    let series = created(&ok(w.call(
        &class("worker"),
        s,
        create_with("/daily", |a| {
            a.take = Some(false);
            a.repeat = Some(crate::cmd::RepeatArg {
                cron_text: text("0 7 * * *"),
                cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
                tz: forum_cron::TzTable {
                    name: String::from("UTC"),
                    tzdb_version: String::new(),
                    initial: 0,
                    transitions: Vec::new(),
                },
                run_within: Dur(2 * 3_600_000),
            });
        }),
    )))
    .0;
    let o = *w.s.ix.series_open[&series].iter().next().unwrap();
    let second = node(&mut w, s, "/second", |_| {});
    wait_on(&mut w, s, second, WaitArg::Node(NodeRef::Id(o)));
    let gaps = query::m1b_gaps(&w.s, w.clock);
    assert!(
        !gaps
            .iter()
            .any(|g| matches!(g, MoverGap::NodeWait { node, .. } if *node == second)),
        "{gaps:?}"
    );
}

/// A flagged node that waits is moved by its wait, not a gap (M1b: "neither a candidate nor held
/// nor waiting").
#[test]
fn a_waiting_flagged_node_is_no_gap() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    let n = node(&mut w, s, "/late", |a| {
        a.due = Some(crate::types::When::In(Dur(MINUTE.0)));
    });
    wait_on(
        &mut w,
        s,
        n,
        WaitArg::Time(crate::types::When::In(Dur(DAY.0))),
    );
    w.advance(Dur(2 * MINUTE.0));
    assert!(w.s.ix.flagged.contains(&n));
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(n)));
    assert_eq!(query::m1b_gaps(&w.s, w.clock), Vec::new());
}

/// `NoMover`'s share is over `|ix.flagged|`, not the backlog (D.8.22): one gap among one flagged
/// node is red, however many unflagged candidates wait.
#[test]
fn no_mover_counts_its_share_of_the_flagged_nodes() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    offered(&mut w, "ok", 200, &[], None);
    let zero = node(&mut w, s, "/zero", |a| {
        a.capacity = Some(0);
        a.due = Some(crate::types::When::In(Dur(MINUTE.0)));
    });
    w.advance(Dur(2 * MINUTE.0));
    assert_eq!(
        w.s.ix.flagged.iter().copied().collect::<Vec<_>>(),
        alloc::vec![zero]
    );
    let h = query::health(&w.s, w.clock);
    assert!(h.capacity.backlog >= 200);
    assert_eq!(no_mover_level(&h), Some((Level::Red, 1)));
}

/// `Paused` (D.8.22): red from exactly `PAUSE_RED_PERMILLE` of the backlog (1 of 10), yellow below
/// (1 of 11); a pause that excludes no candidate is yellow, even with an empty backlog.
#[test]
fn a_pause_is_red_from_a_tenth_of_the_backlog() {
    for (others, want) in [(9, Level::Red), (10, Level::Yellow)] {
        let mut w = world();
        let s = hello(&mut w, &[], None);
        offered(&mut w, "w", others, &[], None);
        let a = node(&mut w, s, "/p/a", |a| a.tags = [label("offer")].into());
        let p = w.s.nodes[&a].parent().unwrap();
        ok(w.call(&class("worker"), s, pause(p)));
        let h = query::health(&w.s, w.clock);
        assert_eq!(usize::try_from(h.capacity.backlog).unwrap(), others + 1);
        assert_eq!(paused_level(&h), Some(want), "1 of {}", others + 1);
    }
    let mut w = world();
    let s = hello(&mut w, &[], None);
    let q = node(&mut w, s, "/q", |_| {});
    ok(w.call(&class("worker"), s, pause(q)));
    let h = query::health(&w.s, w.clock);
    assert_eq!(h.capacity.backlog, 0);
    assert_eq!(paused_level(&h), Some(Level::Yellow));
}

/// `Unreachable` (D.8.22): a candidate held back by a pause counts too; and an `est` equal to the
/// largest horizon, at the low end of its bucket, fits it, while one minute more does not.
#[test]
fn unreachable_counts_held_candidates_and_fits_an_equal_horizon() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    let a = node(&mut w, s, "/p/a", |a| {
        a.tags = [label("offer")].into();
        a.needs = Some([label("x")].into());
    });
    let p = w.s.nodes[&a].parent().unwrap();
    ok(w.call(&class("worker"), s, pause(p)));
    assert!(w.s.ix.next.held.contains_key(&Cand::Node(a)));
    assert_eq!(
        unreachable_of(&query::health(&w.s, w.clock)).map(|x| (x.1, x.2)),
        Some((1, 1))
    );
    // Bucket 5 holds the estimates 16 to 31.
    for (est, want) in [(16, None), (17, Some((1, 0, 1)))] {
        let mut w = world();
        let _ = hello(&mut w, &[], Some(16));
        offered(&mut w, "e", 1, &[], Some(est));
        assert_eq!(
            unreachable_of(&query::health(&w.s, w.clock)).map(|x| (x.1, x.2, x.3)),
            want,
            "est {est}"
        );
    }
}

/// `Overloaded` needs an idle share below 200 ‰, and `Underused` one above 500 ‰ with an empty
/// backlog: exactly 200 ‰ is not overloaded, exactly 500 ‰ not underused, and a backlog keeps an
/// idle swarm from `Underused`.
#[test]
fn the_idle_share_boundaries() {
    let day = |calls: u32, empty: u32, demand: u64| DayStats {
        demand_min: demand,
        next_calls: calls,
        next_empty: empty,
        next_callers: [Some(SessionId(1)), Some(SessionId(2)), Some(SessionId(3))],
        ..DayStats::default()
    };
    let reasons = |w: &World, d: DayStats| {
        let mut s = w.s.clone();
        s.stats.days.insert(crate::index::day_of(w.clock), d);
        compute(&s, w.clock).reasons
    };
    let w = world();
    let overloaded = |r: &[HealthReason]| {
        r.iter()
            .any(|x| matches!(x, HealthReason::Overloaded { .. }))
    };
    let underused = |r: &[HealthReason]| {
        r.iter()
            .any(|x| matches!(x, HealthReason::Underused { .. }))
    };
    assert!(overloaded(&reasons(&w, day(10, 1, 600))));
    assert!(!overloaded(&reasons(&w, day(10, 2, 600))));
    assert!(!underused(&reasons(&w, day(20, 10, 0))));
    assert!(underused(&reasons(&w, day(20, 11, 0))));
    let mut w = world();
    offered(&mut w, "w", 1, &[], None);
    assert!(!underused(&reasons(&w, day(20, 11, 0))));
}

/// `FlagsPilingUp` (CS-31, D.8.22): the margin is `max(50, 10 %)` above the start, so exactly the
/// margin is not red and one more is; a recorded 0 is a start like any other (a real drop to 0);
/// and with a window shorter than three days the growth cannot be seen on three days in it.
#[test]
fn flags_piling_up_boundaries() {
    assert_eq!(piling(&[(6, 10), (2, 30), (1, 50)], 60), None);
    assert_eq!(
        piling(&[(6, 10), (2, 30), (1, 50)], 61),
        Some(HealthReason::FlagsPilingUp { start: 10, now: 61 })
    );
    assert_eq!(
        piling(&[(6, 0), (2, 10), (1, 30)], 55),
        Some(HealthReason::FlagsPilingUp { start: 0, now: 55 })
    );
    // A two-day window: the window's first day has no day before it in the window.
    let mut w = World::new(
        Params {
            capacity_window: Dur(2 * DAY.0),
            ..Params::default()
        },
        START,
    );
    let today = crate::index::day_of(w.clock);
    for (back, f) in [(2, 10), (1, 40)] {
        w.s.stats.days.insert(
            today - back,
            DayStats {
                flagged_end: f,
                ..DayStats::default()
            },
        );
    }
    w.s.ix.flagged_active = (0..100).map(NodeId).collect();
    assert!(
        !compute(&w.s, w.clock)
            .reasons
            .iter()
            .any(|r| matches!(r, HealthReason::FlagsPilingUp { .. }))
    );
}

/// A day's statistics that another counter creates carry the flagged count forward (D.8.22,
/// `index::day_stats`): a day on which no flag changed holds the count it began with, not 0.
#[test]
fn a_day_without_a_flag_change_carries_the_flagged_count() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    let n = node(&mut w, s, "/late", |a| {
        a.needs = Some([label("x")].into());
        a.due = Some(crate::types::When::In(Dur(MINUTE.0)));
    });
    w.advance(Dur(2 * MINUTE.0));
    assert!(w.s.ix.flagged_active.contains(&n));
    let today = crate::index::day_of(w.clock);
    assert_eq!(w.s.stats.days[&today].flagged_end, 1);
    // The next day, a `next` that finds only work for others makes the day's entry.
    w.advance(Dur(DAY.0));
    let tomorrow = crate::index::day_of(w.clock);
    assert!(!w.s.stats.days.contains_key(&tomorrow));
    let s2 = hello(&mut w, &[], None);
    ok(w.call(
        &class("worker"),
        s2,
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery::default(),
            lease: None,
        }),
    ));
    let d = w.s.stats.days[&tomorrow];
    assert_eq!((d.next_calls, d.next_empty_for_you), (1, 1));
    assert_eq!(d.flagged_end, 1);
}

/// `Unreachable` counts a whole partition by its size less its routing items (D.8.5: unrouted user
/// entries sit in the partition with no needs and no estimate): with no session profile within
/// `capacity.window`, three offered nodes are unreachable, and an unrouted entry beside them is not
/// node work, so its age does not make them red either (1.5).
#[test]
fn an_unreachable_partition_does_not_count_its_routes() {
    let mut w = world();
    let s = hello(&mut w, &[], None);
    ok(w.call(
        &class("worker"),
        s,
        Cmd::Talk(crate::cmd::TalkCmd::FromUser {
            text: text("anything new?"),
            client: text("cli"),
            // 1.9 (§8.1): a session that serves none names its principal.
            principal: crate::types::PrincipalKey::new("user"),
            answers: None,
            to: None,
            clarifies: None,
            reason: None,
        }),
    ));
    assert!(!w.s.ix.unrouted.is_empty());
    // The session's profile leaves the window.
    w.advance(Dur(8 * DAY.0));
    offered(&mut w, "d", 3, &[], None);
    let h = query::health(&w.s, w.clock);
    assert_eq!(h.capacity.backlog, 3);
    assert_eq!(unreachable_of(&h), Some((Level::Yellow, 3, 3, 0)));
}

/// 1.7 (§17): a day keeps its first three distinct `next` callers, which `Underused` counts across
/// the window; a session's repeat counts once.
#[test]
fn a_day_keeps_its_first_three_next_callers() {
    let mut w = world();
    let s: Vec<SessionId> = (0..4).map(|_| hello(&mut w, &[], None)).collect();
    for x in [s[0], s[0], s[1], s[2], s[3]] {
        ok(w.call(
            &class("worker"),
            x,
            Cmd::Hold(HoldCmd::Next {
                query: NextQuery::default(),
                lease: None,
            }),
        ));
    }
    let d = w.s.stats.days[&crate::index::day_of(w.clock)];
    assert_eq!(d.next_calls, 5);
    assert_eq!(d.next_callers, [Some(s[0]), Some(s[1]), Some(s[2])]);
}
