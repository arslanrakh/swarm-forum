//! The status line's core part (design D.8.25, §20.1) and `checkpoint_by` (1.4, R8).
//!
//! The user side is read only through `ix.pending_uq_by[a]` for every alias `a` of the session's
//! principals, so the cost depends neither on the number of principals in the forum nor on how much
//! is pending for others (CS-3, §18).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::guide::{CHECKPOINT_EVERY, LEASE_MARGIN};
use super::{HoldStatus, PauseView, StatusLine};
use crate::state::{Hold, State};
use crate::types::{ClassName, NodeId, ROOT, SessionId, Time};

/// When a hold's checkpoint is due (§20.9 "due"; 1.4: shown on the status line, §20.1, R8):
/// `max(h.last_extended, min(h.last_extended + CHECKPOINT_EVERY, h.lease_until − LEASE_MARGIN))`.
/// Rule 3 of the next calls and the status line both call it, so the two never disagree (D.8.26).
#[must_use]
pub fn checkpoint_by(h: &Hold) -> Time {
    h.last_extended.max(
        h.last_extended
            .plus(CHECKPOINT_EVERY)
            .min(h.lease_until.minus(LEASE_MARGIN)),
    )
}

/// A pause as the status line shows it (CS-9).
fn pause_view(s: &State, scope: NodeId, now: Time) -> Option<PauseView> {
    let p = s.pauses.get(&scope).filter(|p| p.until > now)?;
    Some(PauseView {
        scope,
        by: p.by,
        class: p.class.clone(),
        until: p.until,
    })
}

/// The number of pending user questions addressed now to the persons of the session's principals,
/// under any alias (CS-3); None for a session that serves none.
pub(crate) fn pending_questions(s: &State, sess: SessionId) -> Option<u32> {
    let x = s.sessions.get(&sess)?;
    if x.principals.is_empty() {
        return None;
    }
    let served = super::reads::served(s, sess);
    let count = served
        .iter()
        .map(|a| s.ix.pending_uq_by.get(a).map_or(0, BTreeSet::len))
        .fold(0_usize, usize::saturating_add);
    Some(u32::try_from(count).unwrap_or(u32::MAX))
}

/// The status line's core part (§20.1): ROOT's pause, the other pauses that cover the session's
/// holds (nearest first per hold, each scope once), the time of the response (1.4, R8), the class,
/// the session, the live holds with their lease ends and the times their checkpoints are due (1.4,
/// R8), the unread notes, and the user decisions pending for its principals.
#[must_use]
pub fn status_line(s: &State, sess: SessionId, now: Time) -> StatusLine {
    let x = s.sessions.get(&sess);
    let holds: Vec<HoldStatus> = crate::helpers::holds::live_holds_of(s, sess, now)
        .into_iter()
        .filter_map(|t| {
            s.holds.get(&t).map(|h| HoldStatus {
                node: t.node,
                lease_until: h.lease_until,
                checkpoint_by: checkpoint_by(h),
            })
        })
        .collect();
    let mut hold_pauses = Vec::new();
    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    for h in &holds {
        let n = h.node;
        let mut chain = alloc::vec![n];
        chain.extend(crate::index::ancestors(s, n));
        for m in chain {
            if m != ROOT
                && seen.insert(m)
                && let Some(v) = pause_view(s, m, now)
            {
                hold_pauses.push(v);
            }
        }
    }
    StatusLine {
        root_pause: pause_view(s, ROOT, now),
        hold_pauses,
        now,
        class: x.map_or_else(ClassName::unknown, |x| x.class.clone()),
        session: sess,
        holds,
        inbox: u32::try_from(crate::helpers::agents::inbox(s, sess, now).len()).unwrap_or(u32::MAX),
        user_decisions: pending_questions(s, sess),
    }
}
