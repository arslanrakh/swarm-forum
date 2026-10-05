//! The versions the shell showed each session of each node (1.7, §20.5): what `node update`
//! takes when it leaves out its `version`, or its `digest_version` beside a `digest`.
//!
//! The mark of a session and a node is the node's `version` and `digest_version` as the last
//! response that showed its brief to that session rendered them: `take`, `next` with a node,
//! `create` of a new node, and `view node` (brief or full). A write of the session itself moves a
//! mark only when the mark was current just before the write, so that a mark never covers another
//! session's change: then the edit that leaves its versions out is refused as stale, as one that
//! gives them is (O2 unchanged). An update whose versions were both checked, given or filled,
//! marks the values it left.
//!
//! It is shell memory, like `wait::NewsSeen`: no state, event, model, or projection carries it. A
//! session forgets its marks at its silence or retirement, and every session forgets them at a
//! restart; an update that leaves its versions out is then refused, its hint naming the node's
//! current values and the same update with them filled.

use std::collections::BTreeMap;

use forum_core::{NodeId, SessionId, State, Version};

/// One mark: the versions shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The node's `version`.
    pub version: Version,
    /// The node's `digest_version`.
    pub digest_version: Version,
}

impl Shown {
    /// The node's versions in `s` now, if it exists.
    #[must_use]
    pub fn of(s: &State, n: NodeId) -> Option<Shown> {
        s.nodes.get(&n).map(|x| Shown {
            version: x.version,
            digest_version: x.digest_version,
        })
    }
}

/// The marks, by session and node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShownVersions {
    map: BTreeMap<(SessionId, NodeId), Shown>,
}

impl ShownVersions {
    /// No mark yet.
    #[must_use]
    pub fn new() -> ShownVersions {
        ShownVersions::default()
    }

    /// The versions of `node` last shown to `session`.
    #[must_use]
    pub fn get(&self, session: SessionId, node: NodeId) -> Option<Shown> {
        self.map.get(&(session, node)).copied()
    }

    /// Records that `session` was shown `node` as it stands in `s`.
    pub fn show(&mut self, s: &State, session: SessionId, node: NodeId) {
        if let Some(v) = Shown::of(s, node) {
            self.map.insert((session, node), v);
        }
    }

    /// The marks of `session` that are current in `s`, taken just before a write of the session:
    /// those its own write may move.
    #[must_use]
    pub fn current(&self, s: &State, session: SessionId) -> Vec<(NodeId, Shown)> {
        self.map
            .range((session, NodeId(0))..=(session, NodeId(u64::MAX)))
            .filter(|((_, n), v)| Shown::of(s, *n) == Some(**v))
            .map(|((_, n), v)| (*n, *v))
            .collect()
    }

    /// After a write of `session`: each mark that was current before it (`before`, from
    /// [`ShownVersions::current`]) follows the node's versions in `s`, which only the write
    /// changed.
    pub fn follow(&mut self, s: &State, session: SessionId, before: &[(NodeId, Shown)]) {
        for (n, _) in before {
            self.show(s, session, *n);
        }
    }

    /// Forgets `session`: at its silence or retirement (CS-17).
    pub fn drop_session(&mut self, session: SessionId) {
        let keys: Vec<_> = self
            .map
            .range((session, NodeId(0))..=(session, NodeId(u64::MAX)))
            .map(|(k, _)| *k)
            .collect();
        for k in keys {
            self.map.remove(&k);
        }
    }

    /// How many marks it holds.
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
