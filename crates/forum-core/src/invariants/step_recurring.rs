//! The occurrence rows of D.8.2's table for `check_fire` (D.9.1: "the events are exactly the
//! table's"): WP-D6's OccurrenceSlot and OccurrenceWindow arms, and the missed check after a
//! lease of an occurrence expires (§12, R2, R10), recomputed naively from the state before the
//! firing. No code of `helpers::recurring` is shared.

use crate::event::{EntryBody, Ev, HoldEnd, NodePatch, SystemNote};
use crate::state::{Author, OccPhase, Role, State};
use crate::types::{Change, NodeId, Outcome, Token};

/// `rest` (the events after `Fired`) of the OccurrenceSlot row for `o`: a `NodeUpdated { o }` whose
/// patch holds exactly the series' title, body, needs, and est where they differ from the
/// occurrence's (no such event when none differs), then `OccurrenceAvailable { o }` (1.4, R10).
pub(super) fn slot(before: &State, o: NodeId, rest: &[&Ev]) -> bool {
    let Some(node) = before.nodes.get(&o) else {
        return rest.is_empty();
    };
    let Role::Occurrence(x) = &node.role else {
        return rest.is_empty();
    };
    if !node.is_open() || x.phase != OccPhase::Waiting {
        return rest.is_empty();
    }
    let want = before.nodes.get(&x.series).map(|series| NodePatch {
        title: (series.title != node.title).then(|| series.title.clone()),
        body: (series.body != node.body).then(|| series.body.clone()),
        needs: (series.needs != node.needs).then(|| series.needs.clone()),
        est: if series.est == node.est {
            Change::Keep
        } else {
            series.est.map_or(Change::Clear, Change::Set)
        },
        ..NodePatch::default()
    });
    let available =
        |e: Option<&&Ev>| matches!(e, Some(Ev::OccurrenceAvailable { node }) if *node == o);
    match want.filter(|p| *p != NodePatch::default()) {
        Some(p) => {
            rest.len() == 2
                && matches!(rest.first(), Some(Ev::NodeUpdated { node, patch }) if *node == o && *patch == p)
                && available(rest.get(1))
        }
        None => rest.len() == 1 && available(rest.first()),
    }
}

/// Whether `o` closes as missed once its window has passed (§12, R2): an open occurrence with no
/// hold, no open intent, and no checkpoint; `window_passed` is its window marker as the firing
/// leaves it, and `except` a hold the firing has just ended. The holds counted are those in the
/// map: at one instant every Lease key fires before the occurrence's (the kind order of §17), and
/// a hold whose lease ends at that instant is still in the map until its own key fires (D.8.2).
fn missed_due(before: &State, o: NodeId, window_passed: bool, except: Option<Token>) -> bool {
    let held = before
        .holds
        .keys()
        .any(|t| t.node == o && Some(*t) != except);
    before.nodes.get(&o).is_some_and(|node| {
        node.is_open()
            && matches!(node.role, Role::Occurrence(_))
            && window_passed
            && node.open_intent.is_none()
            && node.checkpoints == 0
            && !held
    })
}

/// The missed close of `o` in `rest`: when due, the server's `System(Closed { missed })` on `o`
/// and its `NodeClosed { missed }` (with whatever `close_node` emits around them); otherwise
/// nothing.
fn missed_shape(due: bool, o: NodeId, rest: &[&Ev]) -> bool {
    if !due {
        return rest.is_empty();
    }
    let entry = rest.iter().any(|e| {
        matches!(e, Ev::Appended {
            node,
            by: Author::Server,
            body: EntryBody::System(SystemNote::Closed { outcome: Outcome::Missed }),
            ..
        } if *node == o)
    });
    let closed = rest.iter().any(|e| {
        matches!(e, Ev::NodeClosed { node, outcome: Outcome::Missed, merged_into: None, by: None } if *node == o)
    });
    entry && closed
}

/// `rest` of the OccurrenceWindow row for `o`: `maybe_missed(o)` with `window_fired` set by
/// `Fired`.
pub(super) fn window(before: &State, o: NodeId, rest: &[&Ev]) -> bool {
    missed_shape(missed_due(before, o, true, None), o, rest)
}

/// `rest` of the Lease row for `t`: `HoldEnded { t, Expired }`, then, on an occurrence,
/// `maybe_missed(t.node)` (A-80); nothing else.
pub(super) fn lease(before: &State, t: Token, rest: &[&Ev]) -> bool {
    if !matches!(rest.first(), Some(Ev::HoldEnded { token, end: HoldEnd::Expired }) if *token == t)
    {
        return false;
    }
    let tail = rest.get(1..).unwrap_or_default();
    let fired = before.nodes.get(&t.node).and_then(|n| match &n.role {
        Role::Occurrence(x) => Some(x.window_fired),
        Role::Plain | Role::Series(_) | Role::Signal(_) => None,
    });
    match fired {
        Some(w) => missed_shape(missed_due(before, t.node, w, Some(t)), t.node, tail),
        None => tail.is_empty(),
    }
}
