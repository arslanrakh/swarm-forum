//! Unit tests of WP-D6's recurring work (design D.8.18, D.12.1; §12, K6, R2): a series' first
//! occurrence and its coverage warning at create, the OccurrenceSlot firing and its copy (R10),
//! missed occurrences and the next slot, a checkpoint or an open intent that keeps an occurrence
//! open, the consequences of `update {repeat}` on open and closed nodes, a schedule that ends,
//! L1's occurrence depth (D.14.5), the refusal of a series merged into its own occurrence (1.5),
//! and `coverage`'s `first_start` (R18). Every call runs through `World::call_ctx` and every firing
//! through `World::settle`, which check `check_step`, `check_fire` (the occurrence rows among
//! them), the dirty set, `decide == apply`, and every state invariant (K6 and R2 among them).

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
use alloc::string::String;
use alloc::vec::Vec;

use crate::apply::tests::{
    checkpoint, class, close, create_with, created, label, ok, refused, take, text,
};
use crate::cmd::*;
use crate::error::{Error, LimitKind, Why};
use crate::event::{Ev, Event};
use crate::helpers::recurring::first_usual;
use crate::params::Params;
use crate::reply::{Reply, WarningCode, WarningDetail};
use crate::state::{Life, OccPhase, Role, State};
use crate::testkit::World;
use crate::types::*;

/// 2026-10-01T09:00Z.
const START: Time = Time(1_790_845_200_000);
/// 2026-10-02T07:00Z: the first slot of `0 7 * * *` after `START`.
const SLOT1: Time = Time(1_790_924_400_000);
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;

fn utc() -> forum_cron::TzTable {
    forum_cron::TzTable {
        name: String::from("UTC"),
        tzdb_version: String::new(),
        initial: 0,
        transitions: Vec::new(),
    }
}

/// Europe/Berlin in 2026: CET, CEST from 2026-03-29T01:00Z, CET from 2026-10-25T01:00Z.
fn berlin() -> forum_cron::TzTable {
    forum_cron::TzTable {
        name: String::from("Europe/Berlin"),
        tzdb_version: String::new(),
        initial: 3_600,
        transitions: alloc::vec![
            forum_cron::Transition {
                at: 1_774_746_000,
                offset: 7_200,
            },
            forum_cron::Transition {
                at: 1_792_890_000,
                offset: 3_600,
            },
        ],
    }
}

fn rep(cron: &str, within: u64) -> RepeatArg {
    RepeatArg {
        cron_text: text(cron),
        cron: forum_cron::parse_cron(cron).unwrap(),
        tz: utc(),
        run_within: Dur(within),
    }
}

/// A world at `START` with a worker session.
fn start() -> (World, ClassName, SessionId) {
    let mut w = World::new(Params::default(), START);
    let c = class("worker");
    let s = w.hello(&c);
    (w, c, s)
}

/// Creates the series `p` with `repeat`, without a hold, with a body, `est` 20, and needs `{x}`.
fn series(
    w: &mut World,
    c: &ClassName,
    s: SessionId,
    p: &str,
    r: RepeatArg,
) -> (NodeId, crate::reply::Done) {
    let d = ok(w.call(
        c,
        s,
        create_with(p, |a| {
            a.repeat = Some(r);
            a.take = Some(false);
            a.body = Some(text("post to #daily"));
            a.est = Some(20);
            a.needs = Some([label("x")].into());
        }),
    ));
    (created(&d).0, d)
}

/// The open occurrence of series `n` (K6: exactly one while `n` is open).
fn occ(s: &State, n: NodeId) -> NodeId {
    let open = s.ix.series_open.get(&n).cloned().unwrap_or_default();
    assert_eq!(open.len(), 1, "K6: {open:?}");
    *open.iter().next().unwrap()
}

fn occurrence(s: &State, o: NodeId) -> crate::state::Occurrence {
    match &s.nodes[&o].role {
        Role::Occurrence(x) => x.clone(),
        other => panic!("not an occurrence: {other:?}"),
    }
}

fn update_cmd(s: &State, n: NodeId, f: impl FnOnce(&mut Patch)) -> Cmd {
    let mut patch = Patch::default();
    f(&mut patch);
    Cmd::Node(NodeCmd::Update {
        at: NodeRef::Id(n),
        version: s.nodes[&n].version,
        digest_version: None,
        token: None,
        patch,
    })
}

fn flags(s: &State, n: NodeId) -> FlagSet {
    s.ix.node[&n].flags
}

fn outcome(s: &State, n: NodeId) -> Option<Outcome> {
    match &s.nodes[&n].life {
        Life::Closed(c) => Some(c.outcome.clone()),
        Life::Open => None,
    }
}

// ------------------------------------------------------------------------------------------------
// create

/// A create with `repeat` makes a series and its first occurrence, a waiting child named after its
/// UTC slot with the key `repeat:<id>:<slot>`, the series' title, body, needs, and est, capacity 1,
/// and `due` at the end of its window; with no session history the response warns
/// `uncovered_slot`, with no first likely start (D.8.9 step 3, D.8.18).
#[test]
fn a_series_opens_its_first_occurrence_and_warns_uncovered() {
    let (mut w, c, s) = start();
    let d = ok(w.call(
        &c,
        s,
        create_with("/digest/daily", |a| {
            a.repeat = Some(rep("0 7 * * *", 2 * HOUR_MS));
            a.take = Some(false);
            a.title = text("Daily digest");
            a.est = Some(20);
        }),
    ));
    let (n, _) = created(&d);
    assert!(matches!(w.s.nodes[&n].role, Role::Series(_)));
    let o = occ(&w.s, n);
    let node = &w.s.nodes[&o];
    assert_eq!(node.parent(), Some(n));
    assert_eq!(
        w.s.names.get(&(n, Name::new("20261002T0700Z").unwrap())),
        Some(&o)
    );
    assert_eq!(node.title, text("Daily digest"));
    assert_eq!(node.est, Some(20));
    assert_eq!(node.capacity, 1);
    assert_eq!(node.due, Some(SLOT1.plus(Dur(2 * HOUR_MS))));
    assert_eq!(
        node.keys.get(&KeyStr::of_occurrence(n, SLOT1)),
        Some(&NormVer(0))
    );
    let x = occurrence(&w.s, o);
    assert_eq!(
        (x.slot, x.phase, x.window_fired),
        (SLOT1, OccPhase::Waiting, false)
    );
    // Not a candidate before its slot.
    assert!(!w.s.ix.next.of.contains_key(&crate::state::Cand::Node(o)));
    let warning = d
        .warnings
        .iter()
        .find(|x| x.code == WarningCode::UncoveredSlot)
        .expect("uncovered_slot");
    assert_eq!(warning.id, Some(n));
    match &warning.detail {
        WarningDetail::UncoveredSlot {
            covered_days,
            days,
            usual_hours,
            first_start,
        } => {
            assert_eq!((*covered_days, *days), (0, 28));
            // The only start is the worker's own, at the create's own instant, outside
            // `[now − coverage.window, now)`: no usual hour, so no first likely start (§32 step 1).
            assert!(usual_hours.is_empty());
            assert_eq!(*first_start, None);
        }
        other => panic!("{other:?}"),
    }
}

/// Session starts on most days within the windows cover the slot: no warning. Uncovered windows
/// warn with the usual hours, and name the first likely start inside the next window when a usual
/// hour falls there, or none when it falls after the window's end (R18).
#[test]
fn coverage_from_the_minute_counters() {
    let mut w = World::new(Params::default(), Time(SLOT1.0 - 22 * DAY_MS));
    let c = class("worker");
    // A session starts at 07:30 on each of 20 days.
    let mut t = Time(SLOT1.0 - 21 * DAY_MS + HOUR_MS / 2);
    for _ in 0..20 {
        w.at(t);
        w.hello(&c);
        t = t.plus(Dur(DAY_MS));
    }
    w.at(START);
    let s = w.hello(&c);
    let (_, d) = series(&mut w, &c, s, "/covered", rep("0 7 * * *", 2 * HOUR_MS));
    // No coverage warning; the series' needs `{x}`, which no session here has, warn `unmet_needs`
    // (1.9).
    let codes: Vec<WarningCode> = d.warnings.iter().map(|x| x.code).collect();
    assert_eq!(codes, [WarningCode::UnmetNeeds], "{:?}", d.warnings);
    let coverage = |w: &World, cron: &str, within: u64| {
        crate::helpers::recurring::coverage(
            &w.s,
            &crate::recurring::repeat_of(&rep(cron, within)),
            w.clock,
        )
    };
    assert_eq!(coverage(&w, "0 7 * * *", 2 * HOUR_MS), None);
    // 05:00 to 06:00 misses every start; the usual hour 7 lies after the next window.
    match coverage(&w, "0 5 * * *", HOUR_MS) {
        Some(WarningDetail::UncoveredSlot {
            covered_days,
            days,
            usual_hours,
            first_start,
        }) => {
            assert_eq!((covered_days, days), (0, 28));
            // The session that creates the series started at `now`, outside `[from, now)`.
            assert_eq!(usual_hours, alloc::vec![7]);
            assert_eq!(first_start, None);
        }
        other => panic!("{other:?}"),
    }
    // 06:00 to 07:30 misses the 07:30 starts, but its next window holds 07:00.
    match coverage(&w, "0 6 * * *", 90 * 60_000) {
        Some(WarningDetail::UncoveredSlot { first_start, .. }) => {
            assert_eq!(first_start, Some(SLOT1));
        }
        other => panic!("{other:?}"),
    }
}

/// `first_usual` (R18): the slot itself when its hour is usual; a later hour the same day, or the
/// next day, inside the window; none past the window's end; a DST gap skips its hour; none with no
/// usual hour.
#[test]
fn the_first_usual_start_inside_the_next_window() {
    let tz = utc();
    let end = |h: u64| SLOT1.plus(Dur(h * HOUR_MS));
    assert_eq!(first_usual(&tz, SLOT1, end(2), &[7, 22]), Some(SLOT1));
    assert_eq!(
        first_usual(&tz, SLOT1, end(4), &[9, 22]),
        Some(SLOT1.plus(Dur(2 * HOUR_MS)))
    );
    assert_eq!(
        first_usual(&tz, SLOT1, end(30), &[3]),
        Some(SLOT1.plus(Dur(20 * HOUR_MS)))
    );
    assert_eq!(first_usual(&tz, SLOT1, end(2), &[9]), None);
    assert_eq!(first_usual(&tz, SLOT1, end(2), &[]), None);
    // Berlin, 2026-03-29: local 01:00 CET is 00:00Z; local 02:00 does not exist; local 03:00 CEST
    // is 01:00Z.
    let tz = berlin();
    let next = Time(1_774_742_400_000);
    assert_eq!(
        first_usual(&tz, next, next.plus(Dur(4 * HOUR_MS)), &[2, 3]),
        Some(Time(1_774_746_000_000))
    );
}

/// L1 (D.14.5): a series' occurrences lie one level below it, so a create with `repeat` one level
/// above the limit is `limit(Depth)`, and one two levels above it succeeds; `update {repeat}`
/// counts the same on an open and on a closed node.
#[test]
fn a_series_must_leave_room_for_its_occurrences() {
    let (mut w, c, s) = start();
    let max = usize::try_from(w.s.params.tree_max_depth).unwrap();
    let deep: String = (0..max).map(|i| alloc::format!("/z{i}")).collect();
    refused(
        &mut w,
        &c,
        s,
        create_with(&deep, |a| a.repeat = Some(rep("0 7 * * *", HOUR_MS))),
        Error::limit(LimitKind::Depth),
    );
    let ok_path: String = (0..max - 1).map(|i| alloc::format!("/y{i}")).collect();
    let (n, _) = series(&mut w, &c, s, &ok_path, rep("0 7 * * *", HOUR_MS));
    assert_eq!(
        usize::try_from(w.s.ix.node[&occ(&w.s, n)].depth).unwrap(),
        max
    );
    // A plain node at the limit cannot get `repeat`, open or closed.
    let (plain, _) = created(&ok(w.call(
        &c,
        s,
        create_with(&deep, |a| a.take = Some(false)),
    )));
    let cmd = update_cmd(&w.s, plain, |p| {
        p.repeat = Change::Set(rep("0 7 * * *", HOUR_MS))
    });
    refused(&mut w, &c, s, cmd, Error::limit(LimitKind::Depth));
    ok(w.call(&c, s, close(plain, Outcome::Done)));
    let cmd = update_cmd(&w.s, plain, |p| {
        p.repeat = Change::Set(rep("0 7 * * *", HOUR_MS))
    });
    refused(&mut w, &c, s, cmd, Error::limit(LimitKind::Depth));
}

// ------------------------------------------------------------------------------------------------
// The slot, the window, and missed occurrences

/// At its slot an occurrence copies its series' title, body, needs, and est as they are then,
/// replacing an edit made to the waiting occurrence itself, with one `NodeUpdated` whose title and
/// body bump `version` once; then it is available and a candidate in the partition of its new
/// needs and est (R10).
#[test]
fn the_slot_copies_the_series_and_makes_the_occurrence_available() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    let v0 = w.s.nodes[&o].version;
    let cmd = update_cmd(&w.s, n, |p| {
        p.title = Some(text("Daily digest, new channel"));
        p.body = Some(text("post to #digest"));
        p.needs_add = [label("y")].into();
        p.est = Change::Set(40);
    });
    ok(w.call(&c, s, cmd));
    let cmd = update_cmd(&w.s, o, |p| {
        p.title = Some(text("edited on the occurrence"))
    });
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.nodes[&o].title, text("edited on the occurrence"));
    let v1 = w.s.nodes[&o].version;
    w.at(SLOT1);
    let evs = w.settle();
    let updates: Vec<&Ev> = evs
        .iter()
        .map(|e| &e.ev)
        .filter(|e| matches!(e, Ev::NodeUpdated { node, .. } if *node == o))
        .collect();
    assert_eq!(updates.len(), 1);
    let node = &w.s.nodes[&o];
    assert_eq!(node.title, text("Daily digest, new channel"));
    assert_eq!(node.body, text("post to #digest"));
    assert_eq!(node.needs, [label("x"), label("y")].into());
    assert_eq!(node.est, Some(40));
    assert_eq!(node.version.0, v1.0 + 1);
    assert!(v1.0 > v0.0);
    assert_eq!(occurrence(&w.s, o).phase, OccPhase::Available);
    let at = &w.s.ix.next.of[&crate::state::Cand::Node(o)];
    assert_eq!(at.part.needs, [label("x"), label("y")].into());
    assert_eq!(at.part.est_bucket, crate::index::est_bucket(Some(40)));
}

/// With nothing edited, the slot emits no `NodeUpdated`: `Fired`, then `OccurrenceAvailable`.
#[test]
fn an_unchanged_series_copies_nothing_at_the_slot() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    let v = w.s.nodes[&o].version;
    w.at(SLOT1);
    let evs: Vec<Ev> = w.settle().into_iter().map(|e| e.ev).collect();
    let i = evs
        .iter()
        .position(|e| {
            matches!(
                e,
                Ev::Fired {
                    kind: crate::state::DeadlineKind::OccurrenceSlot,
                    ..
                }
            )
        })
        .unwrap();
    assert!(matches!(evs[i + 1], Ev::OccurrenceAvailable { node } if node == o));
    assert_eq!(w.s.nodes[&o].version, v);
}

/// Nobody takes it within its window: at the window's end it closes as `missed`, the series
/// carries `missed`, and the next occurrence opens at the next slot after the close; a later
/// occurrence closed otherwise clears the flag (§12).
#[test]
fn an_untaken_occurrence_is_missed_and_the_next_one_opens() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    w.at(SLOT1.plus(Dur(2 * HOUR_MS)));
    w.settle();
    assert_eq!(outcome(&w.s, o), Some(Outcome::Missed));
    assert!(flags(&w.s, n).contains(Flag::Missed));
    let o2 = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o2).slot, SLOT1.plus(Dur(DAY_MS)));
    // The next one is dropped by hand (skip one run): the flag clears, a third opens.
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, close(o2, Outcome::Dropped)));
    assert!(!flags(&w.s, n).contains(Flag::Missed));
    let o3 = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o3).slot, SLOT1.plus(Dur(2 * DAY_MS)));
}

/// An occurrence with a checkpoint is never missed (R2): its lease expires after its window, and
/// it stays open, `overdue`, until someone finishes it; one held through the window's end without
/// a checkpoint is missed at the lease's expiry (A-80); and one with an open intent stays open with
/// `open_intent`.
#[test]
fn a_checkpoint_or_an_intent_keeps_an_occurrence_open() {
    for case in 0..3 {
        let (mut w, c, s) = start();
        let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
        let o = occ(&w.s, n);
        // Taken ten minutes before the window ends.
        w.at(SLOT1.plus(Dur(110 * 60_000)));
        let worker = w.hello(&class("x-worker"));
        let token = match ok(w.call(&class("x-worker"), worker, take(o))).reply {
            Reply::Taken { token, .. } => token,
            other => panic!("{other:?}"),
        };
        match case {
            0 => {
                ok(w.call(&class("x-worker"), worker, checkpoint(token)));
            }
            1 => {}
            _ => {
                let cmd = Cmd::Hold(HoldCmd::Checkpoint {
                    token,
                    done: text("posted? not sure"),
                    next_step: text("check the channel"),
                    intent: Some(text("post the digest")),
                    intent_done: None,
                    files: Vec::new(),
                });
                ok(w.call(&class("x-worker"), worker, cmd));
            }
        }
        // The window ends while it is held, then the lease expires.
        w.advance(Dur(DAY_MS / 2));
        let _ = s;
        match case {
            0 => {
                assert_eq!(outcome(&w.s, o), None, "a started occurrence stays open");
                assert!(flags(&w.s, o).contains(Flag::Overdue));
            }
            1 => assert_eq!(outcome(&w.s, o), Some(Outcome::Missed)),
            _ => {
                assert_eq!(outcome(&w.s, o), None);
                assert!(flags(&w.s, o).contains(Flag::OpenIntent));
            }
        }
    }
}

/// A waiting occurrence cannot be taken, closed other than as dropped, or merged from (§12,
/// A-91); a series cannot be merged into its own open occurrence (1.5), which comes after
/// `OccurrenceNotYetAvailable` and before `cycle` (D.8.24).
#[test]
fn a_series_cannot_be_merged_into_its_own_occurrence() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    let merge = |from: NodeId, into: NodeId| {
        Cmd::Node(NodeCmd::Merge {
            from: NodeRef::Id(from),
            into: NodeRef::Id(into),
        })
    };
    refused(
        &mut w,
        &c,
        s,
        take(o),
        Error::invalid(Why::OccurrenceNotYetAvailable),
    );
    refused(
        &mut w,
        &c,
        s,
        close(o, Outcome::Done),
        Error::invalid(Why::OccurrenceNotYetAvailable),
    );
    refused(
        &mut w,
        &c,
        s,
        merge(o, n),
        Error::invalid(Why::OccurrenceNotYetAvailable),
    );
    refused(
        &mut w,
        &c,
        s,
        merge(n, o),
        Error::invalid(Why::MergeIntoOwnOccurrence),
    );
    // A merge that would also close a cycle is refused for the occurrence first.
    let (x, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/x", |a| a.take = Some(false)),
    )));
    let cmd = update_cmd(&w.s, x, |p| {
        p.depends_on = Some(alloc::vec![NodeRef::Id(n)])
    });
    ok(w.call(&c, s, cmd));
    let cmd = update_cmd(&w.s, o, |p| {
        p.depends_on = Some(alloc::vec![NodeRef::Id(x)])
    });
    ok(w.call(&c, s, cmd));
    refused(
        &mut w,
        &c,
        s,
        merge(n, o),
        Error::invalid(Why::MergeIntoOwnOccurrence),
    );
    // Available, the occurrence still cannot be the target.
    w.at(SLOT1);
    w.settle();
    let s2 = w.hello(&c);
    refused(
        &mut w,
        &c,
        s2,
        merge(n, o),
        Error::invalid(Why::MergeIntoOwnOccurrence),
    );
}

/// A schedule with no further slot within 3,000 days ends the series: `ScheduleEnded` on it, and it
/// closes as `failed` (A-51). `0 0 29 2 */7` is 29 February on a Sunday: 2032, then 2060.
#[test]
fn a_schedule_with_no_further_slot_closes_the_series_as_failed() {
    let start = Time(1_953_849_600_000); // 2031-12-01T00:00Z
    let mut w = World::new(Params::default(), start);
    let c = class("worker");
    let s = w.hello(&c);
    let (n, _) = series(&mut w, &c, s, "/leap", rep("0 0 29 2 */7", HOUR_MS));
    let o = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o).slot, Time(1_961_625_600_000));
    w.at(Time(1_961_625_600_000 + HOUR_MS));
    let evs = w.settle();
    assert_eq!(outcome(&w.s, o), Some(Outcome::Missed));
    assert_eq!(outcome(&w.s, n), Some(Outcome::Failed));
    assert!(evs.iter().any(|e| matches!(&e.ev, Ev::Appended {
        node,
        body: crate::event::EntryBody::System(crate::event::SystemNote::ScheduleEnded),
        ..
    } if *node == n)));
    // A create of it now is refused: no slot within 3,000 days.
    let s2 = w.hello(&c);
    refused(
        &mut w,
        &c,
        s2,
        create_with("/leap2", |a| a.repeat = Some(rep("0 0 29 2 */7", HOUR_MS))),
        Error::invalid(Why::ScheduleNeverFires),
    );
}

// ------------------------------------------------------------------------------------------------
// update {repeat}

/// On an open series, a new schedule drops a waiting occurrence and starts the new one from now; a
/// schedule re-sent unchanged changes nothing (A-75); an available occurrence is kept, and the new
/// schedule applies from its close.
#[test]
fn a_new_schedule_on_an_open_series() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    // Unchanged: no event.
    let cmd = update_cmd(&w.s, n, |p| {
        p.repeat = Change::Set(rep("0 7 * * *", 2 * HOUR_MS))
    });
    ok(w.call(&c, s, cmd));
    assert!(
        w.last_events
            .iter()
            .all(|e| !matches!(e.ev, Ev::NodeUpdated { .. }))
    );
    assert_eq!(occ(&w.s, n), o);
    // 08:00 instead: the waiting occurrence is dropped, the next is at 08:00 the next day.
    let cmd = update_cmd(&w.s, n, |p| {
        p.repeat = Change::Set(rep("0 8 * * *", 2 * HOUR_MS))
    });
    let d = ok(w.call(&c, s, cmd));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::UncoveredSlot)
    );
    assert_eq!(outcome(&w.s, o), Some(Outcome::Dropped));
    let o2 = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o2).slot, SLOT1.plus(Dur(HOUR_MS)));
    // Available at its slot, it is kept by a further change; its close follows the new schedule.
    w.at(SLOT1.plus(Dur(HOUR_MS)));
    w.settle();
    let s2 = w.hello(&c);
    let cmd = update_cmd(&w.s, n, |p| {
        p.repeat = Change::Set(rep("30 9 * * *", 2 * HOUR_MS))
    });
    ok(w.call(&c, s2, cmd));
    assert_eq!(occ(&w.s, n), o2);
    ok(w.call(&c, s2, close(o2, Outcome::Dropped)));
    let o3 = occ(&w.s, n);
    assert_eq!(
        occurrence(&w.s, o3).slot,
        SLOT1.plus(Dur(2 * HOUR_MS + HOUR_MS / 2))
    );
}

/// Removing `repeat` drops a waiting occurrence (no next one opens), or detaches an available one,
/// which becomes an ordinary node that K6 no longer counts; adding it to a plain node opens its
/// first occurrence.
#[test]
fn removing_and_adding_repeat() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    let cmd = update_cmd(&w.s, n, |p| p.repeat = Change::Clear);
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.nodes[&n].role, Role::Plain);
    assert_eq!(outcome(&w.s, o), Some(Outcome::Dropped));
    assert!(w.s.ix.series_open.get(&n).is_none_or(BTreeSet::is_empty));
    // Added again: the first occurrence opens.
    let cmd = update_cmd(&w.s, n, |p| {
        p.repeat = Change::Set(rep("0 7 * * *", 2 * HOUR_MS))
    });
    ok(w.call(&c, s, cmd));
    let o2 = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o2).slot, SLOT1);
    // Available, then removed: detached.
    w.at(SLOT1);
    w.settle();
    let s2 = w.hello(&c);
    let cmd = update_cmd(&w.s, n, |p| p.repeat = Change::Clear);
    ok(w.call(&c, s2, cmd));
    assert_eq!(w.s.nodes[&o2].role, Role::Plain);
    assert_eq!(outcome(&w.s, o2), None);
    assert!(w.s.ix.series_open.get(&n).is_none_or(BTreeSet::is_empty));
}

/// On a closed node a change of `repeat` changes only the role (K6: no occurrence opens); the
/// first occurrence opens when the node is reopened. The coverage check runs on it too (§12: "on
/// every update of `repeat`"), against the schedule's first slot after `now`.
#[test]
fn repeat_on_a_closed_node_waits_for_its_reopen() {
    let (mut w, c, s) = start();
    let (n, _) = created(&ok(w.call(
        &c,
        s,
        create_with("/later", |a| a.take = Some(false)),
    )));
    ok(w.call(&c, s, close(n, Outcome::Done)));
    let cmd = update_cmd(&w.s, n, |p| {
        p.repeat = Change::Set(rep("0 7 * * *", 2 * HOUR_MS))
    });
    let d = ok(w.call(&c, s, cmd));
    assert!(matches!(w.s.nodes[&n].role, Role::Series(_)));
    assert!(w.s.ix.series_open.get(&n).is_none_or(BTreeSet::is_empty));
    // No session started within `coverage.window`: the check warns, with no usual hour.
    match &d.warnings[..] {
        [x] => {
            assert_eq!((x.code, x.id), (WarningCode::UncoveredSlot, Some(n)));
            assert!(matches!(
                x.detail,
                WarningDetail::UncoveredSlot {
                    covered_days: 0,
                    days: 28,
                    first_start: None,
                    ..
                }
            ));
        }
        other => panic!("{other:?}"),
    }
    assert!(
        w.last_events
            .iter()
            .all(|e| matches!(e.ev, Ev::NodeUpdated { .. } | Ev::Touched { .. })),
        "{:?}",
        w.last_events
    );
    ok(w.call(
        &c,
        s,
        Cmd::Node(NodeCmd::Reopen {
            at: NodeRef::Id(n),
            reason: text("again"),
        }),
    ));
    let o = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o).slot, SLOT1);
    // Closing the series drops it, with `series_stopped`.
    let d = ok(w.call(&c, s, close(n, Outcome::Done)));
    assert!(matches!(d.reply, Reply::Closed { dropped: Some(x), .. } if x == o));
    assert!(
        d.warnings
            .iter()
            .any(|x| x.code == WarningCode::SeriesStopped)
    );
    assert_eq!(outcome(&w.s, o), Some(Outcome::Dropped));
    // Removing `repeat` from the closed series makes it plain.
    let cmd = update_cmd(&w.s, n, |p| p.repeat = Change::Clear);
    ok(w.call(&c, s, cmd));
    assert_eq!(w.s.nodes[&n].role, Role::Plain);
}

/// A slot whose name is taken under the series by an agent's own child gets `-<id>` (D.8.18).
#[test]
fn an_occurrence_name_taken_by_a_child_gets_its_id() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    ok(w.call(&c, s, close(o, Outcome::Dropped)));
    let o2 = occ(&w.s, n);
    let name = |w: &World, x: NodeId| {
        w.s.names
            .iter()
            .find(|(_, v)| **v == x)
            .map(|((_, k), _)| k.as_str().to_owned())
            .unwrap()
    };
    assert_eq!(name(&w, o2), "20261003T0700Z");
    ok(w.call(
        &c,
        s,
        create_with("/daily/20261004T0700Z", |a| a.take = Some(false)),
    ));
    ok(w.call(&c, s, close(o2, Outcome::Dropped)));
    let o3 = occ(&w.s, n);
    assert_eq!(name(&w, o3), alloc::format!("20261004T0700Z-{}", o3.0));
}

// ------------------------------------------------------------------------------------------------
// check_fire: the occurrence rows of D.8.2's table

/// `check_fire` holds the OccurrenceSlot and OccurrenceWindow firings to their rows: a copy that
/// leaves out a field that differs, or no `OccurrenceAvailable`, and a window firing that closes an
/// occurrence a session still holds, are violations (T4's "exactly the table's events").
#[test]
fn check_fire_holds_the_occurrence_rows() {
    use crate::invariants::{Inv, check_fire};
    use crate::settle::settle;
    use crate::state::{DeadlineKey, DeadlineKind, Owner};
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    let cmd = update_cmd(&w.s, n, |p| p.title = Some(text("a new title")));
    ok(w.call(&c, s, cmd));
    w.at(SLOT1);
    let key = DeadlineKey {
        at: SLOT1,
        kind: DeadlineKind::OccurrenceSlot,
        owner: Owner::Node(o),
    };
    while w.s.ix.deadlines.first() != Some(&key) {
        let k = w.s.ix.deadlines.first().cloned().unwrap();
        assert!(k.at <= SLOT1);
        let (evs, _) = settle(&mut w.s, k.at, 1);
        w.log.extend(evs);
    }
    let before = w.s.clone();
    let mut after = w.s.clone();
    let (evs, _) = settle(&mut after, SLOT1, 1);
    assert!(check_fire(&before, &key, &evs, &after).is_ok());
    // The copy left out, or availability left out.
    let no_copy: Vec<_> = evs
        .iter()
        .filter(|e| !matches!(e.ev, Ev::NodeUpdated { .. }))
        .cloned()
        .collect();
    let bad = check_fire(&before, &key, &no_copy, &after).unwrap_err();
    assert!(bad.iter().any(|v| v.inv == Inv::T4));
    let short: Vec<_> = evs[..evs.len() - 1].to_vec();
    assert!(check_fire(&before, &key, &short, &after).is_err());
    // The window: an occurrence held through its window's end is not missed; a close in that
    // firing is not the row.
    w.s = after;
    w.log.extend(evs);
    w.at(SLOT1.plus(Dur(110 * 60_000)));
    let worker = w.hello(&class("x-worker"));
    let t = match ok(w.call(&class("x-worker"), worker, take(o))).reply {
        Reply::Taken { token, .. } => token,
        other => panic!("{other:?}"),
    };
    ok(w.call(&class("x-worker"), worker, checkpoint(t)));
    let key = DeadlineKey {
        at: SLOT1.plus(Dur(2 * HOUR_MS)),
        kind: DeadlineKind::OccurrenceWindow,
        owner: Owner::Node(o),
    };
    while w.s.ix.deadlines.first() != Some(&key) {
        let k = w.s.ix.deadlines.first().cloned().unwrap();
        assert!(k.at <= key.at);
        let (evs, _) = settle(&mut w.s, k.at, 1);
        w.log.extend(evs);
    }
    let before = w.s.clone();
    let mut after = w.s.clone();
    let (evs, _) = settle(&mut after, key.at, 1);
    assert_eq!(evs.len(), 1, "held: only Fired");
    assert!(check_fire(&before, &key, &evs, &after).is_ok());
    let mut closed = evs.clone();
    closed.push(Event {
        at: key.at,
        ev: Ev::NodeClosed {
            node: o,
            outcome: Outcome::Missed,
            merged_into: None,
            by: None,
        },
    });
    let bad = check_fire(&before, &key, &closed, &after).unwrap_err();
    assert!(bad.iter().any(|v| v.inv == Inv::T4));
}

// ------------------------------------------------------------------------------------------------
// The review's regressions (WP-D6 fix round)

/// The occurrence of `series` taken at `at` by a new `x-worker` session, with no checkpoint.
fn taken_at(w: &mut World, o: NodeId, at: Time) -> (SessionId, Token) {
    w.at(at);
    let worker = w.hello(&class("x-worker"));
    match ok(w.call(&class("x-worker"), worker, take(o))).reply {
        Reply::Taken { token, .. } => (worker, token),
        other => panic!("{other:?}"),
    }
}

/// A `node move` by another session ends the hold on an occurrence whose window has passed, and
/// `maybe_missed` closes it as missed in the same step (A-80; §12: "after any later hold end
/// (expiry, release, or move)"). R2's step check accepts the step, since the hold ended earlier in
/// it, and still refuses a missed close that leaves a hold live.
#[test]
fn a_move_by_another_session_after_the_window_closes_the_occurrence_as_missed() {
    use crate::invariants::{Inv, check_step};
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    // Taken ten minutes before the window ends, with no checkpoint; the window ends while the
    // lease (15 minutes) is still live.
    let (_, token) = taken_at(&mut w, o, SLOT1.plus(Dur(110 * 60_000)));
    w.at(SLOT1.plus(Dur(121 * 60_000)));
    w.settle();
    assert_eq!(outcome(&w.s, o), None);
    assert!(occurrence(&w.s, o).window_fired);
    // Another session moves it: the hold ends, and the occurrence is missed (World::call runs
    // check_step, R2 among it).
    let mover = w.hello(&c);
    let mv = Cmd::Node(NodeCmd::Move {
        at: NodeRef::Id(o),
        to: crate::apply::tests::path("/elsewhere"),
    });
    let before = w.s.clone();
    let ctx = w.ctx(&c, SessionCtx::Existing(mover), None);
    let out = w.call(&c, mover, mv.clone());
    assert!(out.is_ok(), "{out:?}");
    let evs = w.last_events.clone();
    let ended = evs
        .iter()
        .position(|e| matches!(&e.ev, Ev::HoldEnded { token: t, .. } if *t == token))
        .expect("the move ends the hold");
    let closed = evs
        .iter()
        .position(|e| {
            matches!(&e.ev, Ev::NodeClosed { node, outcome: Outcome::Missed, .. } if *node == o)
        })
        .expect("the occurrence is missed");
    assert!(ended < closed);
    assert_eq!(outcome(&w.s, o), Some(Outcome::Missed));
    assert!(flags(&w.s, n).contains(Flag::Missed));
    assert_ne!(occ(&w.s, n), o, "the next occurrence opens");
    // The same step without the hold's end: the missed close leaves a live hold, which R2
    // refuses.
    let without: Vec<Event> = evs
        .iter()
        .filter(|e| !matches!(&e.ev, Ev::HoldEnded { .. }))
        .cloned()
        .collect();
    let bad = check_step(&before, &ctx, &mv, &out, &without, &w.s).unwrap_err();
    assert!(bad.iter().any(|v| v.inv == Inv::R2), "{bad:?}");
}

/// An occurrence with an open intent and no checkpoint (a merge into it brought the intent, N5)
/// is not missed when its window passes: it stays open with `open_intent` (§12, D.8.18).
#[test]
fn an_intent_merged_into_an_occurrence_keeps_it_open_past_its_window() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    let (x, tok) = created(&ok(w.call(&c, s, create_with("/x", |_| {}))));
    let cmd = Cmd::Hold(HoldCmd::Checkpoint {
        token: tok.unwrap(),
        done: text("started"),
        next_step: text("post"),
        intent: Some(text("post the digest")),
        intent_done: None,
        files: Vec::new(),
    });
    ok(w.call(&c, s, cmd));
    let s2 = w.hello(&c);
    ok(w.call(
        &c,
        s2,
        Cmd::Node(NodeCmd::Merge {
            from: NodeRef::Id(x),
            into: NodeRef::Id(o),
        }),
    ));
    assert!(w.s.nodes[&o].open_intent.is_some());
    assert_eq!(w.s.nodes[&o].checkpoints, 0);
    w.at(SLOT1.plus(Dur(2 * HOUR_MS + 60_000)));
    w.settle();
    assert_eq!(outcome(&w.s, o), None);
    assert!(flags(&w.s, o).contains(Flag::OpenIntent));
    assert!(occurrence(&w.s, o).window_fired);
}

/// A new schedule starts from now (§12, D.8.18: `reslot = Some(now)`): at 09:00, `0 7` becomes
/// `0 12`, and the next occurrence is at 12:00 the same day, not after the dropped 07:00 slot of
/// the next day.
#[test]
fn a_new_schedule_starts_from_now() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let cmd = update_cmd(&w.s, n, |p| {
        p.repeat = Change::Set(rep("0 12 * * *", 2 * HOUR_MS))
    });
    ok(w.call(&c, s, cmd));
    let o2 = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o2).slot, START.plus(Dur(3 * HOUR_MS)));
}

/// A world at `START` whose `stats.starts` holds `count` starts at each of `minutes` (minute
/// numbers since the epoch); nothing else has started.
fn starts_at(minutes: &[(u64, u32)]) -> World {
    let mut w = World::new(Params::default(), START);
    w.s.stats.starts = minutes.iter().copied().collect();
    w
}

/// The minute number of `t`.
fn minute(t: Time) -> u64 {
    t.0 / 60_000
}

/// The coverage of `cron` with `run_within` `within` (UTC) at the clock.
fn coverage_of(w: &World, cron: &str, within: u64) -> Option<WarningDetail> {
    crate::helpers::recurring::coverage(
        &w.s,
        &crate::recurring::repeat_of(&rep(cron, within)),
        w.clock,
    )
}

/// "On most days no session started" (A-47): exactly half of the days covered is not most, so no
/// warning; one day fewer warns.
#[test]
fn coverage_at_exactly_half_the_days() {
    // `0 7` within `coverage.window` (28 days) before 2026-10-01T09:00Z: 28 slots on 28 days, the
    // last at 07:00 today. A start at 07:30 on the last `k` of them.
    let today_slot = START.minus(Dur(2 * HOUR_MS));
    let covered = |k: u64| -> alloc::vec::Vec<(u64, u32)> {
        (0..k)
            .map(|i| (minute(today_slot.minus(Dur(i * DAY_MS))) + 30, 1))
            .collect()
    };
    let w = starts_at(&covered(14));
    assert_eq!(coverage_of(&w, "0 7 * * *", HOUR_MS), None);
    let w = starts_at(&covered(13));
    match coverage_of(&w, "0 7 * * *", HOUR_MS) {
        Some(WarningDetail::UncoveredSlot {
            covered_days, days, ..
        }) => assert_eq!((covered_days, days), (13, 28)),
        other => panic!("{other:?}"),
    }
}

/// The usual hours are the three most frequent local start hours, by count descending and then
/// hour ascending (D.8.18): ties go to the earlier hour, and a fourth hour is left out.
#[test]
fn the_usual_hours_are_three_by_count_then_hour() {
    // Starts outside a 05:00 window: five at 10:00 and at 08:00, three at 09:00, one at 11:00,
    // yesterday.
    let day = minute(START.minus(Dur(DAY_MS + 9 * HOUR_MS)));
    let w = starts_at(&[
        (day + 8 * 60, 5),
        (day + 9 * 60, 3),
        (day + 10 * 60, 5),
        (day + 11 * 60, 1),
    ]);
    match coverage_of(&w, "0 5 * * *", HOUR_MS) {
        Some(WarningDetail::UncoveredSlot { usual_hours, .. }) => {
            assert_eq!(usual_hours, alloc::vec![8, 10, 9]);
        }
        other => panic!("{other:?}"),
    }
}

/// A slot covers the starts of the minutes `m` with `slot ≤ m × 60,000 < slot + run_within`
/// (D.8.18): a session that starts at `slot − 30 s` is counted in the minute before the slot, and
/// does not cover it; one in the slot's own minute does, and one in the window's last minute does,
/// but not one in the minute at its end. (A zone's offsets are whole minutes, `valid_table`, so a
/// slot is always a whole minute, and rounding the slot up or down to a minute agree.)
#[test]
fn the_minutes_that_cover_a_slot() {
    let today_slot = START.minus(Dur(2 * HOUR_MS));
    // Starts at `slot + d` (d in minutes, possibly negative) on each of the 28 days.
    let days = |d: i64| -> alloc::vec::Vec<(u64, u32)> {
        (0..28)
            .map(|i| {
                let m = i64::try_from(minute(today_slot.minus(Dur(i * DAY_MS)))).unwrap() + d;
                (u64::try_from(m).unwrap(), 1)
            })
            .collect()
    };
    let uncovered = |w: &World| match coverage_of(w, "0 7 * * *", HOUR_MS) {
        Some(WarningDetail::UncoveredSlot { covered_days, .. }) => covered_days == 0,
        other => panic!("{other:?}"),
    };
    // slot − 30 s lies in the minute before the slot.
    assert_eq!(
        minute(today_slot.minus(Dur(30_000))),
        minute(today_slot) - 1
    );
    assert!(uncovered(&starts_at(&days(-1))));
    assert_eq!(
        coverage_of(&starts_at(&days(0)), "0 7 * * *", HOUR_MS),
        None
    );
    assert_eq!(
        coverage_of(&starts_at(&days(59)), "0 7 * * *", HOUR_MS),
        None
    );
    assert!(uncovered(&starts_at(&days(60))));
}

/// An occurrence moved out of its series and closed with a flag stays out of `next` while its
/// series is open (`occ_locked`), and becomes a candidate when the series closes, although it is
/// no longer the series' child: `refresh_locked` finds it among the flagged nodes (the full-
/// universe proptest's seed 30). `World::call` compares every index with its recount.
#[test]
fn a_moved_flagged_occurrence_is_released_when_its_series_closes() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    let o = occ(&w.s, n);
    // Available; a worker takes it and records an intent; another session closes it, so the
    // intent stays open on the closed occurrence.
    let (worker, token) = taken_at(&mut w, o, SLOT1.plus(Dur(10 * 60_000)));
    let cmd = Cmd::Hold(HoldCmd::Checkpoint {
        token,
        done: text("started"),
        next_step: text("post"),
        intent: Some(text("post the digest")),
        intent_done: None,
        files: Vec::new(),
    });
    ok(w.call(&class("x-worker"), worker, cmd));
    let s2 = w.hello(&c);
    ok(w.call(&c, s2, close(o, Outcome::Done)));
    assert!(flags(&w.s, o).contains(Flag::OpenIntent));
    assert!(!w.s.ix.next.of.contains_key(&crate::state::Cand::Node(o)));
    // Moved out of the series: still locked.
    ok(w.call(
        &c,
        s2,
        Cmd::Node(NodeCmd::Move {
            at: NodeRef::Id(o),
            to: crate::apply::tests::path("/elsewhere"),
        }),
    ));
    assert_ne!(w.s.nodes[&o].parent(), Some(n));
    assert!(!w.s.ix.next.of.contains_key(&crate::state::Cand::Node(o)));
    // The series closes: the occurrence is no longer locked, and is a candidate.
    ok(w.call(&c, s2, close(n, Outcome::Done)));
    assert!(w.s.ix.next.of.contains_key(&crate::state::Cand::Node(o)));
}

/// 1.11 (§10.4, §12; it5 l2 F4): a series whose open occurrence waits for its slot is held back
/// from `next` for every caller, its `missed` flag and all, and an empty `next` counts it as
/// `before_slot`; at the slot its occurrence comes first, in tier 0 within its window, and the
/// series, flagged, is offered after it. In the lab, an idle worker was sent to take a series whose
/// next run was six hours away, which no take could advance.
#[test]
fn a_series_is_not_offered_while_its_occurrence_waits_for_its_slot() {
    let (mut w, c, s) = start();
    let (n, _) = series(&mut w, &c, s, "/daily", rep("0 7 * * *", 2 * HOUR_MS));
    w.at(SLOT1.plus(Dur(2 * HOUR_MS)));
    w.settle();
    assert!(flags(&w.s, n).contains(Flag::Missed));
    let o2 = occ(&w.s, n);
    assert_eq!(occurrence(&w.s, o2).phase, OccPhase::Waiting);
    // A worker that could do the run.
    let ctx = w.ctx(&c, SessionCtx::New, None);
    let x = ok(w.call_ctx(
        &ctx,
        &Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: Some([label("x")].into()),
            horizon: None,
            principal: None,
            from_user: None,
        }),
    ))
    .opened
    .unwrap();
    assert!(w.s.ix.next.held[&crate::state::Cand::Node(n)].before_slot);
    let (got, why) = crate::query::ranked(
        &w.s,
        x,
        w.clock,
        &NextQuery::default(),
        5,
        &mut crate::query::NoMeter,
    );
    assert!(got.is_empty(), "{got:?}");
    assert_eq!(why.before_slot, 1);
    assert!(crate::spec::ranked(&w.s, x, w.clock, &NextQuery::default()).is_empty());
    assert!(crate::query::leave_better(&w.s, None, Some(x)).is_empty());
    // At the slot: the occurrence first, then the series.
    w.at(occurrence(&w.s, o2).slot);
    w.settle();
    assert_eq!(occurrence(&w.s, o2).phase, OccPhase::Available);
    assert!(!w.s.ix.next.held.contains_key(&crate::state::Cand::Node(n)));
    let (got, _) = crate::query::ranked(
        &w.s,
        x,
        w.clock,
        &NextQuery::default(),
        5,
        &mut crate::query::NoMeter,
    );
    let got: Vec<crate::state::Cand> = got.into_iter().map(|r| r.cand).collect();
    assert_eq!(
        got,
        alloc::vec![crate::state::Cand::Node(o2), crate::state::Cand::Node(n)]
    );
    assert_eq!(
        got,
        crate::spec::ranked(&w.s, x, w.clock, &NextQuery::default())
    );
}
