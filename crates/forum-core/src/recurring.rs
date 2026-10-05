//! The consequences of `repeat` in `node create` and `node update` (design D.8.9 step 3, D.8.18,
//! D.8.20; §12, K6): a series' first occurrence and the coverage warning at create, and, at an
//! update that changes `repeat` on an open node, the drop of a waiting occurrence, the detachment
//! of an available or held one, or the first occurrence of a node that becomes a series. WP-D6.
//!
//! The commands (`cmd/node_basic.rs`, WP-D3's) validate; this module executes their `repeat`
//! part through `Exec`, after the `NodeCreated` or `NodeUpdated` that sets the role.

use alloc::string::String;
use alloc::vec::Vec;

use crate::cmd::RepeatArg;
use crate::event::{EndCause, EntryText, Ev};
use crate::helpers::Exec;
use crate::helpers::close::{Closing, close_node};
use crate::helpers::recurring::{coverage_at, open_next_occurrence};
use crate::reply::{Warning, WarningCode};
use crate::state::{OccPhase, Repeat, Role, Series, State};
use crate::types::{NodeId, Outcome, Text, Time};

/// The stored schedule of a `repeat` argument.
#[must_use]
pub(crate) fn repeat_of(r: &RepeatArg) -> Repeat {
    Repeat {
        cron_text: r.cron_text.clone(),
        cron: r.cron.clone(),
        tz: r.tz.clone(),
        run_within: r.run_within,
    }
}

/// The role a `create` with `repeat` gives its node.
#[must_use]
pub(crate) fn series_role(r: &RepeatArg) -> Role {
    Role::Series(Series {
        repeat: repeat_of(r),
        last_missed: false,
    })
}

/// What an `update` does to a node's schedule (A-75), decided at validation on the state before.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RepeatChange {
    /// A plain node becomes a series.
    Added,
    /// A series gets a new schedule.
    Changed,
    /// A series becomes a plain node.
    Removed,
}

/// The open occurrence of series `n`, with its phase (K6: at most one).
fn open_occurrence(s: &State, n: NodeId) -> Option<(NodeId, OccPhase)> {
    let o = s.ix.series_open.get(&n)?.iter().next().copied()?;
    match &s.nodes.get(&o)?.role {
        Role::Occurrence(x) => Some((o, x.phase)),
        Role::Plain | Role::Series(_) | Role::Signal(_) => None,
    }
}

fn entry_text(s: &str) -> EntryText {
    EntryText {
        main: Text(String::from(s)),
        extra: Vec::new(),
    }
}

/// The coverage warning of series `n` (D.8.18, A-47, R18), against the slot of its next
/// occurrence: the waiting occurrence's slot, else the schedule's first slot after `now` (an
/// available or held occurrence of the old schedule is kept, and the new schedule applies from
/// its close).
fn coverage_warning(ex: &mut Exec<'_>, n: NodeId) {
    let s = ex.s();
    let Some(Role::Series(x)) = s.nodes.get(&n).map(|node| &node.role) else {
        return;
    };
    let now = ex.now();
    let waiting = open_occurrence(s, n).and_then(|(o, phase)| {
        if phase != OccPhase::Waiting {
            return None;
        }
        match &s.nodes.get(&o)?.role {
            Role::Occurrence(occ) => Some(occ.slot),
            Role::Plain | Role::Series(_) | Role::Signal(_) => None,
        }
    });
    let Some(next) =
        waiting.or_else(|| forum_cron::next_slot(&x.repeat.cron, &x.repeat.tz, now.0).map(Time))
    else {
        return;
    };
    if let Some(detail) = coverage_at(s, &x.repeat, now, next) {
        ex.warn(Warning {
            code: WarningCode::UncoveredSlot,
            id: Some(n),
            detail,
        });
    }
}

/// D.8.9's create step 3: a node created with `repeat` opens its first occurrence after `now`, and
/// the response carries `uncovered_slot` when no session usually starts within its windows.
pub(crate) fn series_created(ex: &mut Exec<'_>, n: NodeId) {
    let now = ex.now();
    open_next_occurrence(ex, n, now);
    coverage_warning(ex, n);
}

/// D.8.18's "Changing `repeat`" on an open node, after the `NodeUpdated` that changed the role or
/// the schedule (A-75); on a closed node a change of `repeat` changes only the role, and the first
/// occurrence opens when the node is reopened (K6):
/// - a new schedule on a series: a waiting occurrence is dropped with `reslot = now`, so the next
///   one follows the new schedule from now; an available or held one is kept, and the new
///   schedule applies from its close;
/// - `repeat` removed: a waiting occurrence is dropped (no next one opens: the node is no longer a
///   series), and an available or held one is detached and becomes an ordinary node;
/// - `repeat` added to a plain node: its first occurrence opens after `now`.
///
/// A changed schedule (added or changed) then gets the coverage check, on an open node and on a
/// closed one alike (§12: "on every update of `repeat`"); a closed series has no occurrence, so
/// its next slot is the schedule's first after `now`. The check emits nothing: it only warns.
pub(crate) fn repeat_updated(ex: &mut Exec<'_>, n: NodeId, change: RepeatChange) {
    if ex
        .s()
        .nodes
        .get(&n)
        .is_some_and(crate::state::Node::is_open)
    {
        occurrence_follows(ex, n, change);
    }
    if change != RepeatChange::Removed {
        coverage_warning(ex, n);
    }
}

/// The occurrence half of `repeat_updated`, on an open node.
fn occurrence_follows(ex: &mut Exec<'_>, n: NodeId, change: RepeatChange) {
    let now = ex.now();
    let by = ex.author();
    let occ = open_occurrence(ex.s(), n);
    match change {
        RepeatChange::Added => open_next_occurrence(ex, n, now),
        RepeatChange::Changed => {
            if let Some((o, OccPhase::Waiting)) = occ {
                close_node(
                    ex,
                    o,
                    Closing {
                        outcome: Outcome::Dropped,
                        by,
                        merged_into: None,
                        cause: EndCause::Close,
                        text: entry_text("dropped: the series' schedule changed"),
                        reslot: Some(now),
                    },
                );
            }
        }
        RepeatChange::Removed => match occ {
            Some((o, OccPhase::Waiting)) => {
                close_node(
                    ex,
                    o,
                    Closing {
                        outcome: Outcome::Dropped,
                        by,
                        merged_into: None,
                        cause: EndCause::Close,
                        text: entry_text("dropped: the series no longer repeats"),
                        reslot: None,
                    },
                );
            }
            Some((o, OccPhase::Available)) => ex.emit(Ev::OccurrenceDetached { node: o }),
            None => {}
        },
    }
}

/// Whether `n` is a series whose open occurrence a merge into it would drop (1.5, D.8.20): `into`
/// is the open occurrence of the series `from`.
#[must_use]
pub(crate) fn into_own_occurrence(s: &State, from: NodeId, into: NodeId) -> bool {
    matches!(s.nodes.get(&from).map(|x| &x.role), Some(Role::Series(_)))
        && s.ix
            .series_open
            .get(&from)
            .is_some_and(|o| o.contains(&into))
}
