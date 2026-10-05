//! What the shell showed each session (1.7, §20.1): the health level and reason of its last status
//! line, the agreement texts its `agreement` warnings carried (§11.2), and (1.8, §20.9) the
//! standing next calls of its last responses, which it folds once shown on 3 in a row.
//!
//! This is not forum state: no event records it, the core never reads it, and a repeat answered by
//! a stored `op_id` record changes none of it. The writer keeps it beside its other per-session
//! maps and forgets a session at its silence or retirement (CS-17), and everything at a restart, so
//! a session is then shown the reason and the texts again. In the lab's second round, a standing
//! red reason came on all 61 responses of one run and all 56 of another, and the agreement warning
//! fired 14 times without its text.

use std::collections::BTreeMap;

use forum_core::health::Level;
use forum_core::{NodeId, SessionId};

/// The health a status line showed (1.7, §20.1): its level, and the identity of its first reason,
/// its kind and its first item, which `status::reason_key` gives (None while green).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HealthShown {
    /// The level shown.
    pub level: Level,
    /// The first reason's kind and first item, without its counts.
    pub reason: Option<String>,
}

/// What the shell showed one session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The health of its last status line.
    pub health: Option<HealthShown>,
    /// The agreement texts its `agreement` warnings carried: each source, with the FNV-1a-64 of
    /// the text shown, so that a changed text is shown again.
    pub agreements: BTreeMap<NodeId, u64>,
    /// 1.8 (§20.9): the standing next calls of its last response, each with how many responses
    /// in a row showed it, at most `next_calls::FOLD_AFTER`. In the lab's third round, a lead was
    /// shown the same two calls on 16 and 17 consecutive responses.
    pub standing: super::next_calls::Standing,
}

/// [`Seen`] for every session that has been shown something since it was last silent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeenBySession {
    map: BTreeMap<SessionId, Seen>,
}

impl SeenBySession {
    /// Nothing shown yet.
    #[must_use]
    pub fn new() -> SeenBySession {
        SeenBySession::default()
    }

    /// What `session` was shown.
    #[must_use]
    pub fn get(&self, session: SessionId) -> Option<&Seen> {
        self.map.get(&session)
    }

    /// Records what `session` was shown, after a response to it.
    pub fn set(&mut self, session: SessionId, seen: Seen) {
        self.map.insert(session, seen);
    }

    /// Forgets `session`: at its silence or retirement (CS-17).
    pub fn drop_session(&mut self, session: SessionId) {
        self.map.remove(&session);
    }

    /// How many sessions it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
