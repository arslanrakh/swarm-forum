//! Signals and reopening (design D.8.19, D.8.20). Bodies: WP-D4.
//!
//! Besides the shared helpers, this file holds the pure decisions about keys that `node create`,
//! `node key`, and `reopen` share (D.8.9, D.8.20), and those of `node move` and `node merge` about
//! the tree and the edges of T5, so that the Kani harnesses `keys_open_unique`, `move_keeps_tree`,
//! and `merge_redirects` check the very functions the commands call (D.12.5).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use super::Exec;
use super::close::{Closing, close_node};
use super::recurring::open_next_occurrence;
use super::user::e1_check;
use crate::event::{EndCause, EntryBody, EntryText, Ev, NodeInit, SystemNote};
use crate::state::{Author, Origin, Role, SignalKey, State};
use crate::types::{
    KeyStr, Label, LinkRel, Name, NodeId, OFFER, Outcome, SIGNALS, SignalKind, Text,
};

// ------------------------------------------------------------------------------------------------
// The decisions about keys (K1, K7)

/// What `node key {add}` does with a key on `n` (D.8.9), given the key's open carrier, if any, and
/// whether `n` carries it already.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum KeyAdd {
    /// Another open node carries it: not added; a `duplicate` signal (§7).
    Existing(NodeId),
    /// `n` carries it already: nothing.
    Already,
    /// `KeyAdded`.
    Add,
}

/// The choice of `node key {add}` (D.8.9): another open carrier first, then `n`'s own key.
#[must_use]
pub(crate) fn key_add(open_carrier: Option<NodeId>, n: NodeId, carries: bool) -> KeyAdd {
    match open_carrier {
        Some(o) if o != n => KeyAdd::Existing(o),
        Some(_) | None if carries => KeyAdd::Already,
        Some(_) | None => KeyAdd::Add,
    }
}

/// Whether a join gives its carrier a key of the create (D.8.9 step 1): only one that no open node
/// carries (K1).
#[must_use]
pub(crate) fn join_adds(open_carrier: Option<NodeId>) -> bool {
    open_carrier.is_none()
}

/// Whether a reopen of `n` keeps a key it carries (K7): no other open node carries it.
#[must_use]
pub(crate) fn reopen_keeps(open_carrier: Option<NodeId>, n: NodeId) -> bool {
    open_carrier.is_none_or(|o| o == n)
}

// ------------------------------------------------------------------------------------------------
// The decisions about the tree and the edges (K4, L1, T5, N5)

/// Whether `p` is `n` or lies below `n`: the walk up from `p` along `parent` meets `n` within
/// `bound` steps (D.8.4, K4: "walk up from the new parent"). A move of `n` under such a `p` is a
/// `cycle`.
#[must_use]
pub(crate) fn within<F: Fn(NodeId) -> Option<NodeId>>(
    n: NodeId,
    p: NodeId,
    parent: F,
    bound: usize,
) -> bool {
    let mut cur = Some(p);
    let mut steps = 0_usize;
    while let Some(x) = cur {
        if x == n {
            return true;
        }
        if steps >= bound {
            return false;
        }
        steps = steps.saturating_add(1);
        cur = parent(x);
    }
    false
}

/// What `node move` does with `n` at the new place it names (D.8.20, in D.8.24's order): `parent`
/// is the existing node the place hangs from, `missing` whether steps of the path below it are
/// missing (they become ancestors), `here` whether `n` already sits at `(parent, name)`, and
/// `live` whether a live name sits there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MovePlace {
    /// `n`'s own place: a no-op success.
    Noop,
    /// The new parent is `n` or lies below it (`cycle`, K4).
    Cycle,
    /// A live name at the new place (`invalid(NameTaken)`, K3).
    NameTaken,
    /// The move goes ahead.
    Move,
}

/// The place decision of `node move` (D.8.20, K3, K4), which `move_node` takes and the Kani
/// harness `move_keeps_tree` checks: its own place is a no-op; a new parent within `n`'s subtree
/// (`within`, over at most `bound` steps up) is a cycle; a live name at a place with no missing
/// step is taken (with a missing step the place is a new node's, where no name lives yet).
#[must_use]
pub(crate) fn move_place<F: Fn(NodeId) -> Option<NodeId>>(
    n: NodeId,
    parent: NodeId,
    missing: bool,
    here: bool,
    live: bool,
    parent_of: F,
    bound: usize,
) -> MovePlace {
    if !missing && here {
        return MovePlace::Noop;
    }
    if within(n, parent, parent_of, bound) {
        return MovePlace::Cycle;
    }
    if !missing && live {
        return MovePlace::NameTaken;
    }
    MovePlace::Move
}

/// The refusal of `node merge` that its two sides decide, in D.8.24's order, or `None` (D.8.20,
/// §16, K5, N5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MergeSides {
    /// `from = into` (`invalid(SelfMerge)`).
    SelfMerge,
    /// Either side is reserved (`invalid(ReservedNode)`, K8).
    Reserved,
    /// `into` is closed, a merged node included (`closed`, K5).
    IntoClosed,
    /// `from` is closed (`invalid(MergeFromClosed)`, A-33).
    FromClosed,
}

/// The side checks of `node merge` (D.8.20), which `merge` takes and the Kani harness
/// `merge_redirects` checks: a merge only ever closes an open `from` into a different open `into`,
/// so `merged_into` never points at a merged node and stays acyclic (K5).
#[must_use]
pub(crate) fn merge_sides(
    from: NodeId,
    into: NodeId,
    reserved: bool,
    from_open: bool,
    into_open: bool,
) -> Option<MergeSides> {
    if from == into {
        return Some(MergeSides::SelfMerge);
    }
    if reserved {
        return Some(MergeSides::Reserved);
    }
    if !into_open {
        return Some(MergeSides::IntoClosed);
    }
    if !from_open {
        return Some(MergeSides::FromClosed);
    }
    None
}

/// `height(n)` of D.8.4 for a move (D.14.5): the longest downward path from `n` in its subtree, a
/// series counting one level more for the occurrence it opens below itself, so that a move
/// leaves room for every series' occurrences within `tree.max_depth` (L1). `levels` lists, for
/// each node of the subtree, its depth below `n` and whether it is a series.
#[must_use]
pub(crate) fn move_height(levels: impl IntoIterator<Item = (u32, bool)>) -> u32 {
    levels
        .into_iter()
        .map(|(below, series)| below.saturating_add(u32::from(series)))
        .max()
        .unwrap_or(0)
}

/// Edges of T5, `(x, y)` for `x → y`.
pub(crate) type Edges = Vec<(NodeId, NodeId)>;

/// Where an edge `x → t` of T5 points after a merge of `from` into `into` (D.8.20, N5): an edge to
/// `from` goes to `into`, and an edge that would then be a self-edge is dropped (`None`).
#[must_use]
pub(crate) fn redirect(x: NodeId, t: NodeId, from: NodeId, into: NodeId) -> Option<NodeId> {
    let t = if t == from { into } else { t };
    (t != x).then_some(t)
}

/// The edges of T5 that a merge of `from` into `into` changes, given the nodes `preds` with an
/// edge to `from`: `(extra, removed)`, where each edge `w → from` leaves (`removed`), and its
/// `redirect` comes in its place (`extra`): `w → into`, unless `w = into`, whose edge becomes a
/// self-edge and is dropped.
#[must_use]
pub(crate) fn merge_edges(preds: &[NodeId], from: NodeId, into: NodeId) -> (Edges, Edges) {
    let removed: Edges = preds.iter().map(|&w| (w, from)).collect();
    let extra: Edges = preds
        .iter()
        .filter_map(|&w| redirect(w, from, from, into).map(|t| (w, t)))
        .collect();
    (extra, removed)
}

// ------------------------------------------------------------------------------------------------
// Signals (D.8.19)

/// The name of a signal node: `<kind>-<a>-<b>-<id>`, which contains the new node's own id, so it
/// never collides with another signal's; a live name an agent took under `sys/signals` gets a
/// suffix `-<i>`, so that creating a signal never fails (D.8.19). Each taken name is another
/// node's, so the search ends within `nodes.len()` suffixes.
fn signal_name(s: &State, kind: SignalKind, a: NodeId, b: NodeId, id: NodeId) -> Option<Name> {
    let base = alloc::format!("{}-{}-{}-{}", kind_text(kind), a.0, b.0, id.0);
    let bound = u64::try_from(s.nodes.len()).unwrap_or(u64::MAX);
    let mut i: u64 = 0;
    loop {
        let candidate = if i == 0 {
            base.clone()
        } else {
            alloc::format!("{base}-{i}")
        };
        let name = Name::new(&candidate)?;
        if i >= bound || !s.names.contains_key(&(SIGNALS, name.clone())) {
            return Some(name);
        }
        i = i.saturating_add(1);
    }
}

/// `duplicate` or `conflict`.
fn kind_text(kind: SignalKind) -> &'static str {
    match kind {
        SignalKind::Duplicate => "duplicate",
        SignalKind::Conflict => "conflict",
    }
}

/// The link relation of a signal kind.
fn kind_rel(kind: SignalKind) -> LinkRel {
    match kind {
        SignalKind::Duplicate => LinkRel::Duplicate,
        SignalKind::Conflict => LinkRel::ConflictsWith,
    }
}

/// `open_signal(kind, x, y)` (D.8.19, S1): `None` for `x = y`; the open signal of the pair, if
/// any; otherwise a new signal node under `/sys/signals` (server-made: tagged `offer`, capacity 1,
/// linked to both nodes, exempt from `tree.max_children`) and `SignalOpened`.
pub fn open_signal(ex: &mut Exec<'_>, kind: SignalKind, x: NodeId, y: NodeId) -> Option<NodeId> {
    if x == y {
        return None;
    }
    let (a, b) = (x.min(y), x.max(y));
    let key = SignalKey { kind, a, b };
    if let Some(&id) = ex.s().signals.get(&key) {
        return Some(id);
    }
    let id = ex.s().counters.next_node;
    let rel = kind_rel(kind);
    let name = signal_name(ex.s(), kind, a, b, id)?;
    let title = alloc::format!("{}: #{} and #{}", kind_text(kind), a.0, b.0);
    let mut tags = BTreeSet::new();
    tags.extend(Label::new(OFFER));
    ex.emit(Ev::NodeCreated {
        id,
        parent: SIGNALS,
        name,
        origin: Origin::Signal,
        init: NodeInit {
            title: Text(title),
            body: Text::default(),
            fields: alloc::collections::BTreeMap::new(),
            status: Text::default(),
            digest: Text::default(),
            agreements: Text::default(),
            kind: None,
            tags,
            capacity: 1,
            priority: 0,
            est: None,
            needs: BTreeSet::new(),
            due: None,
            depends_on: BTreeSet::new(),
            owners: BTreeSet::new(),
            keys: alloc::collections::BTreeMap::new(),
            links: [(rel, a), (rel, b)].into_iter().collect(),
            role: Role::Signal(key),
        },
    });
    ex.emit(Ev::SignalOpened { key, node: id });
    Some(id)
}

/// Resolves the open signals of the pair `{x, y}`, both kinds (D.8.19: a merge of one into the
/// other, or a `superseded_by` link between them): the server closes each signal node as `done`,
/// and `close_node` resolves its signal. `why` is the text of its `System(Closed)` entry.
pub(crate) fn resolve_pair(ex: &mut Exec<'_>, x: NodeId, y: NodeId, why: &str) {
    if x == y {
        return;
    }
    let (a, b) = (x.min(y), x.max(y));
    for kind in [SignalKind::Duplicate, SignalKind::Conflict] {
        if let Some(&node) = ex.s().signals.get(&SignalKey { kind, a, b }) {
            close_node(
                ex,
                node,
                Closing {
                    outcome: Outcome::Done,
                    by: Author::Server,
                    merged_into: None,
                    cause: EndCause::Close,
                    text: EntryText {
                        main: Text(String::from(why)),
                        extra: Vec::new(),
                    },
                    reslot: None,
                },
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Reopening (D.8.20)

/// `reopen(n, reason)` (D.8.20), shared by `node reopen` and by `take` and `next` of a flagged
/// closed node (reason "taken"):
/// 1. `KeyRemoved` for each key of `n` that another open node carries (K7);
/// 2. while `n` is still closed, `OccurrenceDetached` for an occurrence (A-13), or `SignalDetached`
///    for a signal node whose pair has another open signal (A-89);
/// 3. `NodeReopened` (open, `merged_into` gone, version + 1; the parent may pass
///    `tree.max_children`, CS-10);
/// 4. `SignalOpened` for a signal node whose pair is free (S1);
/// 5. `open_signal(Duplicate, n, o)` for each dropped key, then `System(KeysDropped)` if any;
/// 6. `System(Reopened)` on `n`, with `reason` as its text;
/// 7. a series' next occurrence;
/// 8. `e1_check(n)`.
///
/// Returns the dropped keys with their open carriers; the caller adds the `duplicate_key` warning
/// that lists them (step 9: `take` and `next` add it themselves). A node that is not closed is
/// left alone.
pub fn reopen(ex: &mut Exec<'_>, n: NodeId, reason: EntryText) -> Vec<(KeyStr, NodeId)> {
    let s = ex.s();
    let Some(node) = s.nodes.get(&n) else {
        return Vec::new();
    };
    if node.is_open() {
        return Vec::new();
    }
    let dropped: Vec<(KeyStr, NodeId)> = node
        .keys
        .keys()
        .filter_map(|k| match s.ix.keys_open.get(k) {
            Some(&o) if !reopen_keeps(Some(o), n) => Some((k.clone(), o)),
            Some(_) | None => None,
        })
        .collect();
    let detach = match &node.role {
        Role::Occurrence(_) => Some(Ev::OccurrenceDetached { node: n }),
        Role::Signal(key) if s.signals.get(key).is_some_and(|&o| o != n) => {
            Some(Ev::SignalDetached { node: n })
        }
        Role::Signal(_) | Role::Plain | Role::Series(_) => None,
    };
    // 1.
    for (k, _) in &dropped {
        ex.emit(Ev::KeyRemoved {
            node: n,
            key: k.clone(),
        });
    }
    // 2.
    if let Some(ev) = detach {
        ex.emit(ev);
    }
    // 3.
    ex.emit(Ev::NodeReopened { node: n });
    // 4.
    if let Some(Role::Signal(key)) = ex.s().nodes.get(&n).map(|x| x.role.clone())
        && !ex.s().signals.contains_key(&key)
    {
        ex.emit(Ev::SignalOpened { key, node: n });
    }
    // 5.
    for &(_, o) in &dropped {
        open_signal(ex, SignalKind::Duplicate, n, o);
    }
    let by = ex.author();
    if !dropped.is_empty() {
        ex.append(
            n,
            by,
            EntryBody::System(SystemNote::KeysDropped {
                keys: dropped.clone(),
            }),
            EntryText::default(),
        );
    }
    // 6.
    ex.append(n, by, EntryBody::System(SystemNote::Reopened), reason);
    // 7.
    if ex
        .s()
        .nodes
        .get(&n)
        .is_some_and(|x| matches!(x.role, Role::Series(_)))
    {
        let now = ex.now();
        open_next_occurrence(ex, n, now);
    }
    // 8.
    e1_check(ex, n);
    dropped
}
