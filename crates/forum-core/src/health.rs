//! Health and capacity (design D.8.22, §17): the thresholds, which are constants because the
//! specification fixes them (A-41, CS-9, CS-31), the types `control health` and the status line
//! read, and their computation (WP-D6): `compute`, the body of `query::health`, and `gaps`, the
//! body of `query::m1b_gaps` (M1b, D.9.3).
//!
//! Every reason costs O(pauses + held candidates), O(partitions × a label list + the candidates
//! of partitions that straddle a horizon + held candidates), O(window days) and, for
//! `Overloaded`'s oldest candidate, O(partitions × tiers) range queries and the review items (1.10:
//! they rank by the ring, not by age, so each partition's are read whole), or, for `NoMover`,
//! O(flagged nodes and the waiting chains); none reads the sessions or the history (CS-21).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::index::{day_of, median_done};
use crate::query::PauseView;
use crate::state::{Cand, NextKey, Node, OccPhase, PartKey, Role, State, WaitKey, WaitOn};
use crate::types::{ClassName, Dur, FlagSet, Label, NodeId, ROOT, SessionId, Time, USER};

/// "Idle share is low": below 20 %.
pub const IDLE_LOW_PERMILLE: u32 = 200;
/// "Idle share is high": above 50 %.
pub const IDLE_HIGH_PERMILLE: u32 = 500;
/// `Underused` needs at least this many `next` calls in `capacity.window` (1.7, §17, §22).
pub const UNDERUSED_NEXT_CALLS: u32 = 20;
/// `Underused` needs `next` calls from at least this many sessions in `capacity.window` (1.7, §17,
/// §22): the length of `DayStats::next_callers`.
pub const UNDERUSED_SESSIONS: usize = 3;
/// The demand estimate with no `est` and no completed work.
pub const FALLBACK_EST_MIN: u64 = 30;
/// Completed nodes kept for the median (D.4.9).
pub const DONE_RING: usize = 101;
/// Items listed per reason, besides its count (CS-9).
pub const HEALTH_TOP: usize = 10;
/// A counted reason is red from 1 % of its population.
pub const RED_SHARE_PERMILLE: u32 = 10;
/// A pause is red at ROOT or over 10 % of the candidates.
pub const PAUSE_RED_PERMILLE: u32 = 100;
/// `FlagsPilingUp`: growth over `capacity.window` above max(50, 10 % of the start), and growth on
/// each of the last 3 days (CS-31).
pub const FLAGS_GROWTH_MIN: u32 = 50;
/// See `FLAGS_GROWTH_MIN`.
pub const FLAGS_GROWTH_PERMILLE: u32 = 100;
/// See `FLAGS_GROWTH_MIN`.
pub const FLAGS_GROWTH_DAYS: u64 = 3;

/// The health of the forum.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Health {
    /// The highest level among the reasons (`HealthReason::level`), green with none.
    pub level: Level,
    /// Every reason that holds, each with its own level (`HealthReason::level`), sorted red first
    /// and then in declaration order: the status line shows the first (CS-9). A reason about many
    /// objects carries their count and at most `HEALTH_TOP` of them, never one item per object
    /// (CS-9, FS-8).
    pub reasons: Vec<HealthReason>,
    /// The capacity report.
    pub capacity: CapacityReport,
}

/// A health level.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Level {
    /// Nothing to report.
    #[default]
    Green,
    /// Worth a look.
    Yellow,
    /// Fix it first (constitution line 9).
    Red,
}

/// One reason `health` is not green, with its level (§17: "each reason that holds, with its level,
/// its count, and its first 10 items"). A reason whose level depends on more than it carries
/// (`Paused` on the backlog, `NoMover` on its population and the age of its oldest item,
/// `Unreachable` and `Overloaded` on the age of their oldest item, D.8.22) stores the level
/// `query::health` computed; the others have the fixed level §17 gives them. `HealthReason::level`
/// reads either.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HealthReason {
    /// Active pauses, those that exclude the most candidates first (CS-9, FQ-3).
    Paused {
        /// Red if a pause is at ROOT or excludes at least `PAUSE_RED_PERMILLE` of the backlog,
        /// else yellow.
        level: Level,
        /// How many.
        count: u32,
        /// The first `HEALTH_TOP`.
        top: Vec<PauseExclusion>,
    },
    /// Objects whose mover is unreachable (M1b, D.9.3, A-54).
    NoMover {
        /// `share_level` of D.8.22 over `|ix.flagged|` and the oldest item's time.
        level: Level,
        /// How many.
        count: u32,
        /// The first `HEALTH_TOP`.
        top: Vec<MoverGap>,
    },
    /// Candidates no session profile seen within `capacity.window` could take (CS-9, RS-18).
    Unreachable {
        /// `age_level` of D.8.22 over the oldest `since` (1.5: no share, so that one unreachable
        /// node in a small forum is yellow, not red).
        level: Level,
        /// How many.
        count: u32,
        /// By their needs.
        by_needs: u32,
        /// By their estimate.
        by_horizon: u32,
        /// The first `HEALTH_TOP`.
        top: Vec<(NodeId, UnreachableBy)>,
    },
    /// Flagged nodes that are not waiting keep growing (CS-31): the count at the window's start,
    /// and now.
    FlagsPilingUp {
        /// At the window's start.
        start: u32,
        /// Now.
        now: u32,
    },
    /// Demand exceeds supply and the swarm is rarely idle.
    Overloaded {
        /// `age_level` of D.8.22 over the oldest candidate's `since` (1.5: yellow until then, so
        /// that a young forum's first `next` that finds work is not red).
        level: Level,
        /// Demand in minutes.
        demand_min: u64,
        /// Supply in minutes.
        supply_min: u64,
        /// The idle share.
        idle_permille: u32,
        /// The oldest candidate for `next` that nothing holds back, the one `level` reads, ties
        /// by `Cand`; None when every candidate is held back (1.6: the status line names it).
        oldest: Option<Cand>,
    },
    /// Added by the shell, which alone counts writes per class (CS-29, D.11.2).
    WriteSurge {
        /// The class.
        class: ClassName,
        /// Its writes in the last hour.
        last_hour: u32,
        /// Its mean hourly writes over the 24 hours before.
        baseline: u32,
    },
    /// The swarm is mostly idle with no backlog.
    Underused {
        /// The idle share.
        idle_permille: u32,
    },
}

impl HealthReason {
    /// The reason's own level (§17, D.8.22): the stored one, or the fixed one of §17.
    #[must_use]
    pub const fn level(&self) -> Level {
        match self {
            HealthReason::Paused { level, .. }
            | HealthReason::NoMover { level, .. }
            | HealthReason::Unreachable { level, .. }
            | HealthReason::Overloaded { level, .. } => *level,
            HealthReason::FlagsPilingUp { .. } => Level::Red,
            HealthReason::WriteSurge { .. } | HealthReason::Underused { .. } => Level::Yellow,
        }
    }
}

/// A pause and how many candidates it excludes.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PauseExclusion {
    /// The pause.
    pub pause: PauseView,
    /// The candidates it excludes.
    pub excluded: u32,
}

/// Why a candidate is unreachable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum UnreachableBy {
    /// No profile meets its needs.
    Needs,
    /// No profile's horizon covers its estimate.
    Horizon,
    /// 1.12 (§17): as shown to a session whose caps meet its needs and whose horizon, this one,
    /// its estimate exceeds (`counting`): "est 60 min exceeds your horizon (45) and every other
    /// session's". In the lab's seventh round, a Lean prover with a 45-minute horizon was told
    /// that no session had lean.
    Yours {
        /// The session's horizon, in minutes.
        horizon: u32,
    },
}

/// The capacity report of §17.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CapacityReport {
    /// Demand in minutes over `capacity.window`.
    pub demand_min: u64,
    /// Supply in minutes.
    pub supply_min: u64,
    /// Time held by holds that expired without a checkpoint.
    pub lost_min: u64,
    /// `next` calls.
    pub next_calls: u32,
    /// True empties.
    pub next_empty: u32,
    /// Empties for the caller only.
    pub next_empty_for_you: u32,
    /// `1000 × next_empty / next_calls`; `None`, unknown, with no `next` call in the window, when
    /// neither `Overloaded` nor `Underused` fires (§17, 1.5).
    pub idle_permille: Option<u32>,
    /// Node and status candidates.
    pub backlog: u32,
    /// `|ix.flagged_active|`.
    pub flagged_active: u32,
    /// The median of completed work.
    pub median_done_min: u64,
}

/// An object with no mover that the server cannot prevent (M1b, D.9.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MoverGap {
    /// A `{node}` wait whose target has no mover.
    NodeWait {
        /// The waiting node.
        node: NodeId,
        /// Its target.
        target: NodeId,
    },
    /// The same for `depends_on`.
    Dependency {
        /// The dependent node.
        node: NodeId,
        /// Its target.
        target: NodeId,
    },
    /// A mover flag on a node no one can take.
    FlaggedUnreachable {
        /// The node.
        node: NodeId,
        /// Its flags.
        flags: FlagSet,
    },
}

// ------------------------------------------------------------------------------------------------
// The computation (WP-D6)

/// `age_level` of D.8.22: red once the reason's oldest item has been in it for `capacity.window`,
/// and yellow before, or when no item carries a time (`Unreachable` and `Overloaded`, 1.5).
#[must_use]
pub fn age_level(oldest: Option<Time>, now: Time, window: Dur) -> Level {
    if oldest.is_some_and(|t| t <= now.minus(window)) {
        Level::Red
    } else {
        Level::Yellow
    }
}

/// `share_level` of D.8.22 (`NoMover`): red when the count reaches `RED_SHARE_PERMILLE` of the
/// population, or by `age_level`; yellow from the first item; None without one.
#[must_use]
pub fn share_level(
    count: u32,
    population: u32,
    oldest: Option<Time>,
    now: Time,
    window: Dur,
) -> Option<Level> {
    if count == 0 {
        return None;
    }
    let share = u64::from(count).saturating_mul(1_000)
        >= u64::from(RED_SHARE_PERMILLE).saturating_mul(u64::from(population));
    Some(if share {
        Level::Red
    } else {
        age_level(oldest, now, window)
    })
}

/// The order of a reason within its level: its declaration order (§17).
const fn rank(r: &HealthReason) -> u8 {
    match r {
        HealthReason::Paused { .. } => 0,
        HealthReason::NoMover { .. } => 1,
        HealthReason::Unreachable { .. } => 2,
        HealthReason::FlagsPilingUp { .. } => 3,
        HealthReason::Overloaded { .. } => 4,
        HealthReason::WriteSurge { .. } => 5,
        HealthReason::Underused { .. } => 6,
    }
}

/// Sorts `reasons` red first, then in declaration order, and sets the level to the worst of them,
/// green with none (§17, CS-9).
#[must_use]
pub fn finish(mut reasons: Vec<HealthReason>, capacity: CapacityReport) -> Health {
    reasons.sort_by_key(|r| (r.level() != Level::Red, rank(r)));
    let level = reasons
        .iter()
        .map(HealthReason::level)
        .max()
        .unwrap_or(Level::Green);
    Health {
        level,
        reasons,
        capacity,
    }
}

/// 1.11 (§17): `h` as a session with `caps` and `horizon` is shown it. The health is a cached
/// result, which may predate the session's arrival or its last change of caps or horizon, and
/// `Unreachable` counts the profiles of every session seen within `capacity.window`, the caller's
/// among them: so each listed item of `Unreachable` whose needs `caps` meet and whose `est`
/// `horizon` covers is left out, with one from its count; the reason goes once none of its listed
/// items is left, and the level is the worst of the reasons left, green with none. The order of
/// the reasons is kept. O(`HEALTH_TOP`): the next refresh counts the session for everyone. In the
/// lab's fourth to sixth rounds, `hello` told the only sessions with a11y, and a Lean prover, that
/// no session had their caps. 1.12: an item whose needs `caps` meet, and whose `est` `horizon`
/// does not cover, is computed for that session: `Yours { horizon }`, "est 60 min exceeds your
/// horizon (45) and every other session's", never "no session has (lean)" when it has.
#[must_use]
pub fn counting(h: &Health, s: &State, caps: &BTreeSet<Label>, horizon: Option<u32>) -> Health {
    let reach = Reach::of(horizon);
    let reaches = |n: NodeId| {
        s.nodes
            .get(&n)
            .is_some_and(|x| x.needs.is_subset(caps) && reach.fits(x.est))
    };
    // 1.12: the caller's caps meet the needs, and only its horizon keeps it off.
    let yours = |n: NodeId| {
        horizon.filter(|_| {
            s.nodes
                .get(&n)
                .is_some_and(|x| x.needs.is_subset(caps) && !reach.fits(x.est))
        })
    };
    let mut out = h.clone();
    out.reasons.retain_mut(|r| match r {
        HealthReason::Unreachable {
            count,
            by_needs,
            by_horizon,
            top,
            ..
        } => {
            top.retain_mut(|(n, by)| {
                if !reaches(*n) {
                    if let Some(h) = yours(*n) {
                        if *by == UnreachableBy::Needs {
                            *by_needs = by_needs.saturating_sub(1);
                            *by_horizon = by_horizon.saturating_add(1);
                        }
                        *by = UnreachableBy::Yours { horizon: h };
                    }
                    return true;
                }
                *count = count.saturating_sub(1);
                match by {
                    UnreachableBy::Needs => *by_needs = by_needs.saturating_sub(1),
                    UnreachableBy::Horizon | UnreachableBy::Yours { .. } => {
                        *by_horizon = by_horizon.saturating_sub(1);
                    }
                }
                false
            });
            !top.is_empty()
        }
        HealthReason::Paused { .. }
        | HealthReason::NoMover { .. }
        | HealthReason::FlagsPilingUp { .. }
        | HealthReason::Overloaded { .. }
        | HealthReason::WriteSurge { .. }
        | HealthReason::Underused { .. } => true,
    });
    out.level = out
        .reasons
        .iter()
        .map(HealthReason::level)
        .max()
        .unwrap_or(Level::Green);
    out
}

/// `query::health(s, now)` (D.8.22): every reason that holds over the days of `stats.days` within
/// `capacity.window` before `now`, and the capacity report.
#[must_use]
pub fn compute(s: &State, now: Time) -> Health {
    let window = s.params.capacity_window;
    let start = now.minus(window);
    let backlog = s.ix.next.nodes;
    let mut reasons = Vec::new();
    if let Some(r) = paused(s, now, backlog) {
        reasons.push(r);
    }
    let gaps = gaps_with_age(s, now);
    let oldest = gaps.iter().filter_map(|(_, t)| *t).min();
    let flagged = u32::try_from(s.ix.flagged.len()).unwrap_or(u32::MAX);
    let count = u32::try_from(gaps.len()).unwrap_or(u32::MAX);
    if let Some(level) = share_level(count, flagged, oldest, now, window) {
        reasons.push(HealthReason::NoMover {
            level,
            count,
            top: gaps.iter().take(HEALTH_TOP).map(|(g, _)| *g).collect(),
        });
    }
    if let Some(r) = unreachable(s, now) {
        reasons.push(r);
    }
    // The window's days, and the capacity report (A-41, CS-31).
    let first_day = day_of(start);
    let days: Vec<(u64, crate::state::DayStats)> = s
        .stats
        .days
        .range(first_day..=day_of(now))
        .map(|(d, x)| (*d, *x))
        .collect();
    let sum = |f: fn(&crate::state::DayStats) -> u64| {
        days.iter().fold(0_u64, |a, (_, x)| a.saturating_add(f(x)))
    };
    let live_ms = s
        .holds
        .values()
        .filter(|h| now < h.lease_until)
        .fold(0_u64, |a, h| {
            a.saturating_add(now.since(h.taken_at.max(start)).0)
        });
    let next_calls = u32::try_from(sum(|x| u64::from(x.next_calls))).unwrap_or(u32::MAX);
    let next_empty = u32::try_from(sum(|x| u64::from(x.next_empty))).unwrap_or(u32::MAX);
    // Unknown with no `next` call in the window (§17, 1.5): a swarm whose agents take nodes
    // directly is neither idle nor busy by this measure.
    let idle_permille = u64::from(next_empty)
        .saturating_mul(1_000)
        .checked_div(u64::from(next_calls))
        .map(|x| u32::try_from(x).unwrap_or(u32::MAX));
    let capacity = CapacityReport {
        demand_min: sum(|x| x.demand_min),
        supply_min: sum(|x| x.supply_ms)
            .saturating_add(live_ms)
            .checked_div(60_000)
            .unwrap_or(0),
        lost_min: sum(|x| x.lost_ms).checked_div(60_000).unwrap_or(0),
        next_calls,
        next_empty,
        next_empty_for_you: u32::try_from(sum(|x| u64::from(x.next_empty_for_you)))
            .unwrap_or(u32::MAX),
        idle_permille,
        backlog,
        flagged_active: u32::try_from(s.ix.flagged_active.len()).unwrap_or(u32::MAX),
        median_done_min: median_done(&s.stats.done_minutes).unwrap_or(0),
    };
    if let Some(r) = piling_up(&days, now, first_day, capacity.flagged_active) {
        reasons.push(r);
    }
    if let Some(idle_permille) = idle_permille {
        if capacity.demand_min > capacity.supply_min && idle_permille < IDLE_LOW_PERMILLE {
            // Its items are the candidates for `next` that nothing holds back (1.5); the oldest,
            // ties by `Cand`, is named (1.6).
            let oldest =
                s.ix.next
                    .parts
                    .iter()
                    .filter_map(|(p, k)| oldest_in(s, k, p.needs.len(), true))
                    .min();
            reasons.push(HealthReason::Overloaded {
                level: age_level(oldest.map(|(t, _)| t), now, window),
                demand_min: capacity.demand_min,
                supply_min: capacity.supply_min,
                idle_permille,
                oldest: oldest.map(|(_, c)| c),
            });
        }
        // 1.7 (§17): one idle worker's empty `next` says nothing about the swarm; `Underused`
        // needs UNDERUSED_NEXT_CALLS `next` calls from UNDERUSED_SESSIONS sessions in the window.
        let callers: BTreeSet<SessionId> = days
            .iter()
            .flat_map(|(_, x)| x.next_callers.iter().flatten().copied())
            .collect();
        if idle_permille > IDLE_HIGH_PERMILLE
            && backlog == 0
            && next_calls >= UNDERUSED_NEXT_CALLS
            && callers.len() >= UNDERUSED_SESSIONS
        {
            reasons.push(HealthReason::Underused { idle_permille });
        }
    }
    finish(reasons, capacity)
}

/// `Paused` (CS-9, FQ-3): every active pause with the candidates `ix.next.held` attributes to its
/// scope, those that exclude the most first; red if one is at ROOT, or excludes at least one
/// candidate and `PAUSE_RED_PERMILLE` of the backlog.
fn paused(s: &State, now: Time, backlog: u32) -> Option<HealthReason> {
    let mut excluded: BTreeMap<NodeId, u32> = BTreeMap::new();
    for h in s.ix.next.held.values() {
        if let Some(p) = h.paused_by {
            let e = excluded.entry(p).or_insert(0);
            *e = e.saturating_add(1);
        }
    }
    let mut all: Vec<PauseExclusion> = s
        .pauses
        .iter()
        .filter(|(_, p)| p.until > now)
        .map(|(&scope, p)| PauseExclusion {
            pause: PauseView {
                scope,
                by: p.by,
                class: p.class.clone(),
                until: p.until,
            },
            excluded: excluded.get(&scope).copied().unwrap_or(0),
        })
        .collect();
    if all.is_empty() {
        return None;
    }
    all.sort_by(|a, b| {
        b.excluded
            .cmp(&a.excluded)
            .then(a.pause.scope.cmp(&b.pause.scope))
    });
    let red = all.iter().any(|x| {
        x.pause.scope == ROOT
            || (x.excluded >= 1
                && u64::from(x.excluded).saturating_mul(1_000)
                    >= u64::from(PAUSE_RED_PERMILLE).saturating_mul(u64::from(backlog)))
    });
    let count = u32::try_from(all.len()).unwrap_or(u32::MAX);
    all.truncate(HEALTH_TOP);
    Some(HealthReason::Paused {
        level: if red { Level::Red } else { Level::Yellow },
        count,
        top: all,
    })
}

/// How far a caps set's profiles reach: no horizon (any estimate), or up to a horizon in minutes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reach {
    /// Some profile has no horizon.
    Any,
    /// The largest horizon seen.
    Upto(u32),
}

impl Reach {
    fn of(h: Option<u32>) -> Reach {
        h.map_or(Reach::Any, Reach::Upto)
    }

    fn join(self, other: Reach) -> Reach {
        match (self, other) {
            (Reach::Upto(a), Reach::Upto(b)) => Reach::Upto(a.max(b)),
            (Reach::Any, _) | (_, Reach::Any) => Reach::Any,
        }
    }

    fn fits(self, est: Option<u32>) -> bool {
        match (self, est) {
            (Reach::Any, _) | (_, None) => true,
            (Reach::Upto(h), Some(e)) => e <= h,
        }
    }
}

/// 1.12 (§10.4): how far the session profiles seen within `capacity.window` reach, for `next`'s
/// work longer than every session's horizon: built once per read, O(profiles).
pub(crate) struct Horizons<'a>(Profiles<'a>);

impl<'a> Horizons<'a> {
    /// The profiles seen within `capacity.window` before `now`.
    pub(crate) fn of(s: &'a State, now: Time) -> Horizons<'a> {
        Horizons(Profiles::of(s, now))
    }

    /// The longest horizon of the profiles whose caps meet `needs`: None when one of them has no
    /// horizon, 0 when none meets them.
    pub(crate) fn longest(&self, needs: &BTreeSet<Label>) -> Option<u32> {
        match self.0.reach(needs) {
            None => Some(0),
            Some(Reach::Upto(h)) => Some(h),
            Some(Reach::Any) => None,
        }
    }
}

/// The session profiles seen within `capacity.window`, reduced to one per caps set with the
/// largest horizon seen for it, and indexed by label (D.8.22).
struct Profiles<'a> {
    by_caps: BTreeMap<&'a BTreeSet<Label>, Reach>,
    by_label: BTreeMap<&'a Label, Vec<&'a BTreeSet<Label>>>,
}

impl<'a> Profiles<'a> {
    fn of(s: &'a State, now: Time) -> Profiles<'a> {
        let since = now.minus(s.params.capacity_window);
        let mut by_caps: BTreeMap<&BTreeSet<Label>, Reach> = BTreeMap::new();
        for ((caps, _), p) in &s.stats.profiles {
            if p.last_seen < since {
                continue;
            }
            let r = Reach::of(p.max_horizon);
            by_caps
                .entry(caps)
                .and_modify(|x| *x = x.join(r))
                .or_insert(r);
        }
        let mut by_label: BTreeMap<&Label, Vec<&BTreeSet<Label>>> = BTreeMap::new();
        for caps in by_caps.keys() {
            for l in *caps {
                by_label.entry(l).or_default().push(caps);
            }
        }
        Profiles { by_caps, by_label }
    }

    /// The reach of the caps sets that meet `needs`, through the shortest label list; None when
    /// no profile meets them.
    fn reach(&self, needs: &BTreeSet<Label>) -> Option<Reach> {
        let fold = |it: &mut dyn Iterator<Item = &&BTreeSet<Label>>| {
            it.filter(|caps| needs.is_subset(caps))
                .filter_map(|caps| self.by_caps.get(*caps).copied())
                .reduce(Reach::join)
        };
        if needs.is_empty() {
            return fold(&mut self.by_caps.keys());
        }
        let shortest = needs
            .iter()
            .map(|l| self.by_label.get(l).map_or(0, Vec::len))
            .zip(needs.iter())
            .min_by_key(|(n, _)| *n)
            .map(|(_, l)| l)?;
        fold(&mut self.by_label.get(shortest)?.iter())
    }
}

/// The candidate of a partition key.
fn cand_of(k: &NextKey) -> Option<Cand> {
    match k.kind {
        0 => Some(Cand::Node(NodeId(k.id))),
        1 => Some(Cand::Status(NodeId(k.id))),
        _ => None,
    }
}

/// The oldest `since` among a partition's candidates, up to priority, with its candidate (ties by
/// `Cand`, 1.6): the first entry of each tier, since a partition is ordered by tier, then hour
/// group (D.8.5), so that a young candidate of a higher tier hides no older one (1.5). With
/// `routes` false, routing items are skipped, as they are not node work (`Unreachable`); the walk
/// past them is the one `unreachable` already makes. With `routes` true (`Overloaded`), each band
/// costs one range query.
///
/// 1.7: a recheck-due candidate's group lies `RECHECK_SHIFT` below the others of its tier (§10.4),
/// and 1.8: a stale candidate's `STALE_SHIFT` above them, so each tier is read from the first
/// entry of each of its bands. 1.10: the review items lie in a band of their own, `REVIEW_SHIFT`
/// below the plain ones, ordered by priority and the ring rather than by age, so that band is read
/// whole: its items are the closed nodes with an open objection, few beside the backlog. 1.11: a
/// candidate without a request lies `NO_REQUEST_SHIFT` above, and the needs of the partition,
/// `needs` labels, put all of its candidates at one offset (`index::band`), so each tier has eight
/// bands, (stale, request, recheck), each with its review band below its plain one.
fn oldest_in(
    s: &State,
    keys: &BTreeSet<NextKey>,
    needs: usize,
    routes: bool,
) -> Option<(Time, Cand)> {
    let rv = crate::index::REVIEW_SHIFT;
    let since = |c: Cand| s.ix.next.of.get(&c).map(|a| (a.since, c));
    let mut bands: Vec<(i64, bool)> = Vec::new();
    for stale in [false, true] {
        for request in [false, true] {
            for recheck in [false, true] {
                let b = crate::index::band(stale, request, recheck, needs);
                // The review band, below the plain band of the same bands.
                bands.push((b.saturating_sub(rv.saturating_mul(2)), true));
                bands.push((b.saturating_sub(rv / 2), false));
            }
        }
    }
    (0..=2_u8)
        .flat_map(|tier| bands.iter().map(move |&(g, review)| (tier, g, review)))
        .filter_map(|(tier, group, review)| {
            let from = NextKey {
                tier,
                group,
                fnv: 0,
                id: 0,
                kind: 0,
            };
            let mut it = keys.range(from..).take_while(|k| k.tier == tier);
            if review {
                // 1.10: the whole review band, its oldest by `since`.
                let end = group.saturating_add(rv.saturating_mul(2).saturating_sub(rv / 2));
                return it
                    .take_while(|k| k.group < end)
                    .filter_map(cand_of)
                    .filter_map(since)
                    .min();
            }
            if routes {
                it.next()
                    .map(|k| cand_of(k).unwrap_or(Cand::Route(crate::types::Eid(k.id))))
            } else {
                it.find_map(cand_of)
            }
            .and_then(since)
        })
        .min()
}

/// The node of a node or status candidate.
fn node_of(c: Cand) -> Option<NodeId> {
    match c {
        Cand::Node(n) | Cand::Status(n) => Some(n),
        Cand::Route(_) => None,
    }
}

/// Why a candidate of partition `part` is unreachable for profiles that reach `reach` (None: no
/// profile meets its needs), given its node's `est`.
fn why_unreachable(reach: Option<Reach>, est: Option<u32>) -> Option<UnreachableBy> {
    match reach {
        None => Some(UnreachableBy::Needs),
        Some(r) if !r.fits(est) => Some(UnreachableBy::Horizon),
        Some(_) => None,
    }
}

/// Whether a whole `est` bucket fits a reach: Some(true) every estimate of the bucket fits,
/// Some(false) none does, None it straddles the horizon (bucket `b` holds `[2^(b−1), 2^b − 1]`,
/// and bucket 0 has no estimate).
fn bucket_fits(reach: Reach, bucket: u8) -> Option<bool> {
    let Reach::Upto(h) = reach else {
        return Some(true);
    };
    if bucket == 0 {
        return Some(true);
    }
    let low = 1_u64
        .checked_shl(u32::from(bucket).saturating_sub(1))
        .unwrap_or(u64::MAX);
    let high = 1_u64
        .checked_shl(u32::from(bucket))
        .map_or(u64::MAX, |x| x.saturating_sub(1));
    if high <= u64::from(h) {
        Some(true)
    } else if low > u64::from(h) {
        Some(false)
    } else {
        None
    }
}

/// `Unreachable` (CS-9, RS-18): the node and status candidates that no session profile seen within
/// `capacity.window` could take, by needs or by horizon, per partition of `ix.next` and per
/// candidate only where a partition's `est` bucket straddles the largest fitting horizon; the held
/// candidates one by one. Its level is `age_level` over the oldest `since` (for a whole partition,
/// `oldest_in`); 1.5 dropped the share, which made one unreachable node of a small forum red.
fn unreachable(s: &State, now: Time) -> Option<HealthReason> {
    /// What the reason accumulates.
    #[derive(Default)]
    struct Acc {
        count: u32,
        by_needs: u32,
        by_horizon: u32,
        top: Vec<(NodeId, UnreachableBy)>,
        oldest: Option<Time>,
    }
    impl Acc {
        /// `k` unreachable candidates, of which `n` is the first, waiting since `since`.
        fn add(&mut self, n: NodeId, by: UnreachableBy, since: Option<Time>, k: u32) {
            self.count = self.count.saturating_add(k);
            match by {
                UnreachableBy::Needs => self.by_needs = self.by_needs.saturating_add(k),
                UnreachableBy::Horizon | UnreachableBy::Yours { .. } => {
                    self.by_horizon = self.by_horizon.saturating_add(k);
                }
            }
            self.listed(n, by);
            if let Some(t) = since {
                self.oldest = Some(self.oldest.map_or(t, |o| o.min(t)));
            }
        }
        /// One more item for the top list, if there is room.
        fn listed(&mut self, n: NodeId, by: UnreachableBy) {
            if self.top.len() < HEALTH_TOP {
                self.top.push((n, by));
            }
        }
    }
    let profiles = Profiles::of(s, now);
    let mut acc = Acc::default();
    // The routing items in the partitions: every candidate that is not a node or status item, less
    // those held back (D.8.5: they all sit in the routing partition, `index::route_part`; `Cand`
    // orders routes last, so the held ones are a range).
    let held_routes =
        s.ix.next
            .held
            .range(Cand::Route(crate::types::Eid(0))..)
            .count();
    let routes =
        s.ix.next
            .of
            .len()
            .saturating_sub(usize::try_from(s.ix.next.nodes).unwrap_or(usize::MAX))
            .saturating_sub(held_routes);
    let route_part = crate::index::route_part();
    let routes_in = |part: &PartKey| if *part == route_part { routes } else { 0 };
    let since_of = |c: Cand| s.ix.next.of.get(&c).map(|a| a.since);
    let est_of = |n: NodeId| s.nodes.get(&n).and_then(|x| x.est);
    let mut reaches: BTreeMap<&PartKey, Option<Reach>> = BTreeMap::new();
    for (part, keys) in &s.ix.next.parts {
        let reach = profiles.reach(&part.needs);
        reaches.insert(part, reach);
        let whole = match reach {
            None => Some(UnreachableBy::Needs),
            Some(r) => match bucket_fits(r, part.est_bucket) {
                Some(true) => None,
                Some(false) => Some(UnreachableBy::Horizon),
                None => {
                    for k in keys {
                        let Some(c) = cand_of(k) else { continue };
                        let Some(n) = node_of(c) else { continue };
                        if let Some(by) = why_unreachable(reach, est_of(n)) {
                            acc.add(n, by, since_of(c), 1);
                        }
                    }
                    None
                }
            },
        };
        let Some(by) = whole else { continue };
        // Routing items sit in the partition with no needs and bucket 0: not node work. The
        // partition is counted, not walked: its size less its routes; only its first node items
        // are listed.
        let n_nodes = u32::try_from(keys.len().saturating_sub(routes_in(part))).unwrap_or(u32::MAX);
        let mut first = true;
        for c in keys.iter().filter_map(cand_of) {
            let Some(n) = node_of(c) else { continue };
            if first {
                acc.add(
                    n,
                    by,
                    oldest_in(s, keys, part.needs.len(), false).map(|(t, _)| t),
                    n_nodes,
                );
                first = false;
            } else if acc.top.len() < HEALTH_TOP {
                acc.listed(n, by);
            } else {
                break;
            }
        }
    }
    for c in s.ix.next.held.keys() {
        let Some(n) = node_of(*c) else { continue };
        let Some(at) = s.ix.next.of.get(c) else {
            continue;
        };
        let reach = match reaches.get(&at.part) {
            Some(r) => *r,
            None => profiles.reach(&at.part.needs),
        };
        if let Some(by) = why_unreachable(reach, est_of(n)) {
            acc.add(n, by, Some(at.since), 1);
        }
    }
    if acc.count == 0 {
        return None;
    }
    let level = age_level(acc.oldest, now, s.params.capacity_window);
    Some(HealthReason::Unreachable {
        level,
        count: acc.count,
        by_needs: acc.by_needs,
        by_horizon: acc.by_horizon,
        top: acc.top,
    })
}

/// `FlagsPilingUp` (CS-31): with `f(d)` the `flagged_end` of day `d`, carried forward over days
/// without an entry (a day's entry holds the count it began with when no flag changed that day,
/// `index::day_stats`, so a recorded 0 is a value), the earliest day kept standing for the
/// window's start, and `f(now) = |ix.flagged_active|`: red when `f(now) > f(start) +
/// max(FLAGS_GROWTH_MIN, f(start) × FLAGS_GROWTH_PERMILLE / 1000)` and `f` grew on each of the
/// last `FLAGS_GROWTH_DAYS` days, today included.
fn piling_up(
    days: &[(u64, crate::state::DayStats)],
    now: Time,
    first_day: u64,
    current: u32,
) -> Option<HealthReason> {
    let today = day_of(now);
    // f over the days from the window's first to today, carried forward.
    let start_value = days.first().map_or(0, |(_, x)| x.flagged_end);
    let recorded: BTreeMap<u64, u32> = days.iter().map(|(d, x)| (*d, x.flagged_end)).collect();
    let f = |d: u64| -> u32 {
        if d >= today {
            return current;
        }
        recorded
            .range(first_day..=d)
            .next_back()
            .map_or(start_value, |(_, v)| *v)
    };
    let margin = FLAGS_GROWTH_MIN.max(
        u32::try_from(
            u64::from(start_value)
                .saturating_mul(u64::from(FLAGS_GROWTH_PERMILLE))
                .checked_div(1_000)
                .unwrap_or(0),
        )
        .unwrap_or(u32::MAX),
    );
    if current <= start_value.saturating_add(margin) {
        return None;
    }
    let grew = (0..FLAGS_GROWTH_DAYS).all(|i| {
        let d = today.saturating_sub(i);
        d > first_day && f(d) > f(d.saturating_sub(1))
    });
    grew.then_some(HealthReason::FlagsPilingUp {
        start: start_value,
        now: current,
    })
}

/// A node's unresolved wait, if any.
fn wait_on(s: &State, node: &Node, now: Time) -> Option<WaitOn> {
    node.wait
        .as_ref()
        .filter(|w| !crate::flags::resolved(s, &w.on, now))
        .map(|w| w.on)
}

/// Whether open node `n`, which is not a candidate, has a Due deadline queued that will make it one
/// (D.9.3: "a deadline-based mover"): its `due` lies after `now`, so the deadline is queued (D.4.8),
/// and at it `overdue` turns on (D.8.2); a node with a free slot and a flag is a candidate (D.8.5),
/// unless it is reserved, has no slot, or waits, which the caller has ruled out but for the first
/// two.
fn due_mover(s: &State, n: NodeId, node: &Node, now: Time) -> bool {
    node.origin != crate::state::Origin::Reserved
        && !n.is_reserved()
        && node.capacity > 0
        && s.ix
            .deadline_at
            .get(&(
                crate::state::DeadlineKind::Due,
                crate::state::Owner::Node(n),
            ))
            .is_some_and(|t| *t > now)
}

/// Whether the chain of unresolved `{node}` waits and `depends_on` edges from `t` ends, on every
/// branch, at a node that is held, a candidate (not held back by a dependency), closed, or moved
/// by a deadline (M1b, D.9.3): a time or external wait, a waiting occurrence's slot, or a queued
/// Due deadline (`due_mover`). Iterative, with a memo; a node met again on the current path (a
/// cycle, which T5 excludes) ends no chain.
fn chain_ok(s: &State, t: NodeId, now: Time, memo: &mut BTreeMap<NodeId, bool>) -> bool {
    // Each frame: a node and the targets still to visit.
    let mut stack: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
    let mut on_path: BTreeSet<NodeId> = BTreeSet::new();
    let start = |n: NodeId, memo: &BTreeMap<NodeId, bool>| -> Result<bool, Vec<NodeId>> {
        if let Some(&v) = memo.get(&n) {
            return Ok(v);
        }
        let Some(node) = s.nodes.get(&n) else {
            return Ok(true);
        };
        if !node.is_open() || s.ix.node.get(&n).is_some_and(|x| x.live > 0) {
            return Ok(true);
        }
        if let Some(w) = wait_on(s, node, now) {
            return match w {
                WaitOn::Node(t2) => Err(alloc::vec![t2]),
                WaitOn::Question { .. }
                | WaitOn::UserQuestion(_)
                | WaitOn::Time(_)
                | WaitOn::External(_) => Ok(true),
            };
        }
        if matches!(node.role, Role::Occurrence(ref o) if o.phase == OccPhase::Waiting) {
            return Ok(true);
        }
        let open_deps = || -> Vec<NodeId> {
            node.depends_on
                .iter()
                .copied()
                .filter(|d| s.nodes.get(d).is_some_and(Node::is_open))
                .collect()
        };
        let cand = [Cand::Node(n), Cand::Status(n)]
            .into_iter()
            .find(|c| s.ix.next.of.contains_key(c));
        match cand {
            // A queued Due deadline moves it: once due it carries `overdue` and is a candidate,
            // held back only by an open dependency, through which the chain then goes on.
            None if due_mover(s, n, node, now) => {
                let deps = open_deps();
                if deps.is_empty() { Ok(true) } else { Err(deps) }
            }
            None => Ok(false),
            Some(c) if s.ix.next.held.get(&c).is_some_and(|h| h.depends) => Err(open_deps()),
            Some(_) => Ok(true),
        }
    };
    match start(t, memo) {
        Ok(v) => return v,
        Err(next) => {
            on_path.insert(t);
            stack.push((t, next));
        }
    }
    let mut result = true;
    while let Some((n, mut todo)) = stack.pop() {
        let Some(x) = todo.pop() else {
            // Every target of `n` ends well.
            memo.insert(n, true);
            on_path.remove(&n);
            continue;
        };
        stack.push((n, todo));
        if on_path.contains(&x) {
            result = false;
            break;
        }
        match start(x, memo) {
            Ok(true) => {}
            Ok(false) => {
                memo.insert(x, false);
                result = false;
                break;
            }
            Err(next) => {
                on_path.insert(x);
                stack.push((x, next));
            }
        }
    }
    if !result {
        for (n, _) in &stack {
            memo.insert(*n, false);
        }
    }
    memo.get(&t).copied().unwrap_or(result)
}

/// M1b's gaps (D.9.3), each with the time its item has waited since, if it carries one: a flagged
/// node that is neither a candidate nor held nor waiting, by id; then each open node whose `{node}`
/// wait, or whose open `depends_on` target, starts a chain that ends at a node nothing moves, by
/// id (its wait first, then its dependencies by target).
///
/// The chains start from the `{node}` entries of `ix.waiters` and from the open targets of
/// `ix.dependents`, so the cost is O(flagged nodes and waiting chains), never a walk of every node
/// (D.8.22); a closed node has no wait (`NodeClosed` clears it), and a closed dependent is skipped.
fn gaps_with_age(s: &State, now: Time) -> Vec<(MoverGap, Option<Time>)> {
    let mut out = Vec::new();
    // `Cand` orders routes last: whether one exists is a range query, not a walk of `ix.next.of`.
    let routes =
        s.ix.next
            .of
            .range(Cand::Route(crate::types::Eid(0))..)
            .next()
            .is_some();
    for &n in &s.ix.flagged {
        let Some(node) = s.nodes.get(&n) else {
            continue;
        };
        let flags = s.ix.node.get(&n).map_or(FlagSet::EMPTY, |x| x.flags);
        let held = s.ix.node.get(&n).is_some_and(|x| x.live > 0);
        let cand = s.ix.next.of.contains_key(&Cand::Node(n))
            || s.ix.next.of.contains_key(&Cand::Status(n))
            || (n == USER && routes);
        let waiting = node.is_open() && wait_on(s, node, now).is_some();
        if !held && !cand && !waiting {
            out.push((MoverGap::FlaggedUnreachable { node: n, flags }, None));
        }
    }
    let mut memo: BTreeMap<NodeId, bool> = BTreeMap::new();
    // (node, 0 for its wait or 1 for a dependency, target): the order of a walk by node id.
    let mut chains: BTreeMap<(NodeId, u8, NodeId), (MoverGap, Option<Time>)> = BTreeMap::new();
    let node_waits =
        s.ix.waiters
            .range(WaitKey::Node(NodeId(0))..)
            .take_while(|(k, _)| matches!(k, WaitKey::Node(_)));
    for (k, waiters) in node_waits {
        let WaitKey::Node(t) = *k else { continue };
        for &n in waiters {
            let Some(node) = s.nodes.get(&n) else {
                continue;
            };
            if node.is_open()
                && wait_on(s, node, now) == Some(WaitOn::Node(t))
                && !chain_ok(s, t, now, &mut memo)
            {
                let since = node.wait.as_ref().map(|w| w.set_at);
                chains.insert(
                    (n, 0, t),
                    (MoverGap::NodeWait { node: n, target: t }, since),
                );
            }
        }
    }
    for (&t, dependents) in &s.ix.dependents {
        if !s.nodes.get(&t).is_some_and(Node::is_open) {
            continue;
        }
        for &n in dependents {
            if s.nodes.get(&n).is_some_and(Node::is_open) && !chain_ok(s, t, now, &mut memo) {
                let since = [Cand::Node(n), Cand::Status(n)]
                    .iter()
                    .find_map(|c| s.ix.next.of.get(c).map(|a| a.since));
                chains.insert(
                    (n, 1, t),
                    (MoverGap::Dependency { node: n, target: t }, since),
                );
            }
        }
    }
    out.extend(chains.into_values());
    out
}

/// `query::m1b_gaps(s, now)` (D.9.3): the objects with no mover that `health` reports as
/// `NoMover`.
#[must_use]
pub fn gaps(s: &State, now: Time) -> Vec<MoverGap> {
    gaps_with_age(s, now).into_iter().map(|(g, _)| g).collect()
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod compute_tests;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;
    use alloc::vec;

    fn counted(level: Level) -> [HealthReason; 4] {
        [
            HealthReason::Paused {
                level,
                count: 1,
                top: Vec::new(),
            },
            HealthReason::NoMover {
                level,
                count: 1,
                top: Vec::new(),
            },
            HealthReason::Unreachable {
                level,
                count: 1,
                by_needs: 1,
                by_horizon: 0,
                top: vec![(NodeId(9), UnreachableBy::Needs)],
            },
            HealthReason::Overloaded {
                level,
                demand_min: 2,
                supply_min: 1,
                idle_permille: 0,
                oldest: Some(Cand::Node(NodeId(9))),
            },
        ]
    }

    #[test]
    fn each_reason_has_its_level() {
        // §17: Paused, NoMover, Unreachable, and (1.5) Overloaded are yellow or red by what they
        // count or by the age of their oldest item.
        for level in [Level::Yellow, Level::Red] {
            for reason in counted(level) {
                assert_eq!(reason.level(), level, "{reason:?}");
            }
        }
        // The fixed ones.
        let start = HealthReason::FlagsPilingUp { start: 1, now: 60 };
        let surge = HealthReason::WriteSurge {
            class: ClassName::new("worker").unwrap(),
            last_hour: 3_000,
            baseline: 1,
        };
        let under = HealthReason::Underused { idle_permille: 900 };
        assert_eq!(start.level(), Level::Red);
        assert_eq!(surge.level(), Level::Yellow);
        assert_eq!(under.level(), Level::Yellow);
        assert!(Level::Green < Level::Yellow && Level::Yellow < Level::Red);
    }

    #[test]
    fn health_tells_reasons_of_different_levels_apart() {
        // The review's two states: one unreachable candidate of 1,000, recent (yellow), and the
        // same candidate older than `capacity.window` (red). The counts and items are equal; the
        // per-reason level is what differs, and `Health` must carry it (§17).
        let health = |unreachable: Level| Health {
            level: Level::Red,
            reasons: vec![
                HealthReason::NoMover {
                    level: Level::Red,
                    count: 1,
                    top: Vec::new(),
                },
                HealthReason::Unreachable {
                    level: unreachable,
                    count: 1,
                    by_needs: 1,
                    by_horizon: 0,
                    top: vec![(NodeId(9), UnreachableBy::Needs)],
                },
            ],
            capacity: CapacityReport {
                backlog: 1_000,
                ..CapacityReport::default()
            },
        };
        assert_ne!(health(Level::Yellow), health(Level::Red));
        let levels = |h: &Health| {
            h.reasons
                .iter()
                .map(HealthReason::level)
                .collect::<Vec<_>>()
        };
        assert_eq!(levels(&health(Level::Yellow)), [Level::Red, Level::Yellow]);
        assert_eq!(levels(&health(Level::Red)), [Level::Red, Level::Red]);
    }
}
