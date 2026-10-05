//! Wait and dependency cycles (design D.8.4, T5): `reaches`, shared by `hold release`, `node
//! update`, and `node merge`. Its search is `search`, generic over its successor function, the
//! store of its marks, and its stack, so that the Kani harness `reaches_terminates` checks the very
//! search `reaches` runs, with fixed arrays where `reaches` uses a `BTreeMap` and a `Vec` (D.12.5:
//! the `BTreeMap`, and a growing `Vec` under a symbolic graph, are beyond CBMC's budget).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::state::{State, WaitOn};
use crate::types::NodeId;

/// The marks of a search: which nodes it has visited, each with the node it was reached from.
pub trait Marks {
    /// Marks `x` visited, reached from `parent`; false if it was visited already.
    fn visit(&mut self, x: NodeId, parent: Option<NodeId>) -> bool;
    /// The node `x` was reached from, if it was visited and is not the start.
    fn parent(&self, x: NodeId) -> Option<NodeId>;
}

impl Marks for BTreeMap<NodeId, Option<NodeId>> {
    fn visit(&mut self, x: NodeId, parent: Option<NodeId>) -> bool {
        if self.contains_key(&x) {
            return false;
        }
        self.insert(x, parent);
        true
    }

    fn parent(&self, x: NodeId) -> Option<NodeId> {
        self.get(&x).copied().flatten()
    }
}

/// The stack of a search.
pub trait Stack {
    /// Pushes `x`.
    fn push(&mut self, x: NodeId);
    /// Pops the last node pushed, if any.
    fn pop(&mut self) -> Option<NodeId>;
}

impl Stack for Vec<NodeId> {
    fn push(&mut self, x: NodeId) {
        Vec::push(self, x);
    }

    fn pop(&mut self) -> Option<NodeId> {
        Vec::pop(self)
    }
}

/// Iterative depth-first search from `from` for `to` over `succ`, with the marks in `marks` and the
/// stack in `stack` (empty at the start), at most `bound` pops. Returns whether `to` was reached
/// within them; `path` then reads the path from the marks.
pub fn search<F, I, M, S>(
    from: NodeId,
    to: NodeId,
    succ: F,
    marks: &mut M,
    stack: &mut S,
    bound: usize,
) -> bool
where
    F: Fn(NodeId) -> I,
    I: IntoIterator<Item = NodeId>,
    M: Marks,
    S: Stack,
{
    marks.visit(from, None);
    stack.push(from);
    let mut pops = 0_usize;
    while let Some(x) = stack.pop() {
        if pops >= bound {
            return false;
        }
        pops = pops.saturating_add(1);
        if x == to {
            return true;
        }
        for y in succ(x) {
            if marks.visit(y, Some(x)) {
                stack.push(y);
            }
        }
    }
    false
}

/// The path from a search's start to `to`, both included, by the parents in `marks`: at most
/// `bound + 1` nodes.
pub fn path<M: Marks>(marks: &M, to: NodeId, bound: usize) -> Vec<NodeId> {
    let mut path = alloc::vec![to];
    let mut cur = to;
    while let Some(p) = marks.parent(cur) {
        if path.len() > bound {
            break;
        }
        path.push(p);
        cur = p;
    }
    path.reverse();
    path
}

/// The edges of T5 from `x` in `s`: its `{node}` wait, resolved or not (a reopened target
/// unresolves it, A-55), and its `depends_on` targets.
fn edges(s: &State, x: NodeId) -> Vec<NodeId> {
    let Some(node) = s.nodes.get(&x) else {
        return Vec::new();
    };
    let mut out: Vec<NodeId> = node.depends_on.iter().copied().collect();
    if let Some(w) = &node.wait
        && let WaitOn::Node(t) = w.on
    {
        out.push(t);
    }
    out
}

/// `reaches(s, from, to, extra, extra_removed)` (D.8.4): whether `to` is reachable from `from`
/// over the T5 edges, with the edges the same command is about to add (`extra`) and drop
/// (`extra_removed`); at most `nodes.len()` pops (plus the extra edges' nodes). Returns the path
/// for the hint.
#[must_use]
pub fn reaches(
    s: &State,
    from: NodeId,
    to: NodeId,
    extra: &[(NodeId, NodeId)],
    extra_removed: &[(NodeId, NodeId)],
) -> Option<Vec<NodeId>> {
    let succ = |x: NodeId| {
        let mut out: Vec<NodeId> = edges(s, x)
            .into_iter()
            .filter(|y| !extra_removed.contains(&(x, *y)))
            .collect();
        out.extend(extra.iter().filter(|(a, _)| *a == x).map(|(_, b)| *b));
        out
    };
    let bound = s
        .nodes
        .len()
        .saturating_add(extra.len().saturating_mul(2))
        .saturating_add(1);
    let mut marks: BTreeMap<NodeId, Option<NodeId>> = BTreeMap::new();
    let mut stack: Vec<NodeId> = Vec::new();
    search(from, to, succ, &mut marks, &mut stack, bound).then(|| path(&marks, to, bound))
}
