//! The harnesses of WP-D4 (design D.12.5) for the key invariants it owns (D.0.3): K1 (one open
//! carrier per key), K3 and K4 (names and the tree), K5 (`merged_into`), and T5 for merges (no
//! cycle of waits and dependencies). The harnesses D.12.5 lists for K7, K8, C1, Q1, S1, Settled,
//! and L1's move cases (`reopen_keys_k7`, `reserved_k8`, `conv_node_c1`, `record_request_q1`,
//! `open_signal_s1`, `try_settle_monotone`, `limits_l1`) are not built: D.0.3 leaves those
//! invariants to their executable checks in tests.
//!
//! **Narrowed** (D.12.5), and recorded for `docs/verification.md`. As for WP-D2's and WP-D3's
//! harnesses, a command on a symbolic `State` does not leave CBMC's symbolic execution within the
//! CI budget (every `BTreeMap` operation is expensive), so each harness checks the pure decisions
//! the commands take, over all their inputs, on a model of the few facts they change, written in
//! the harness; the end-to-end form is the keys universe of `tests/prop.rs` (every step checked with
//! `check_step` and `check_invariants`, K1, K3, K4, K5, and T5's state checks among them) and the
//! unit tests of `cmd/keys_tests.rs` and `cmd/restructure_tests.rs`:
//! - `keys_open_unique` (K1): `join_adds`, `key_add`, and `reopen_keeps`, which `node create`'s
//!   join, `node key {add}`, and `reopen` call, over three steps of create-or-join, key add,
//!   reopen, and close on three nodes and three keys, one of them a `repeat:` key that a command
//!   refuses (A-90; that refusal is a test, `KeyStr::is_reserved` being a string scan);
//! - `move_keeps_tree` (K3, K4): `move_place`, the place decision `node move` takes (its own place
//!   a no-op, a new parent within its subtree a `cycle` by `within`, a live name `NameTaken`), over
//!   every tree of four nodes and every target, with the facts it reads (whether the node is at
//!   that place, whether a live name is) and `NodeMoved`'s bookkeeping (the old place becomes an
//!   alias, an alias at the new place goes; `evolve`'s) modelled on arrays: K4 and K3 hold after
//!   every accepted move. `move_place` is given the bound 2 where `move_node` gives `nodes.len()`:
//!   in a tree of four nodes a node other than the root lies at most two steps above any node;
//! - `merge_redirects` (T5, K5): `redirect`, the rule by which `merge_edges` changes the edges
//!   `node merge` redirects, and `search`, the search `reaches` runs, against a brute-force cycle
//!   check of the redirected edges over four nodes with symbolic edges, acyclic before; the merge
//!   refuses exactly when the edges afterwards have a cycle, and leaves no edge to `from` and no
//!   self-edge (N5's redirects); and `merged_into` (modelled) stays acyclic, on closed nodes only,
//!   with every chain ending at a node that was not merged, under a merge that `merge_sides`, the
//!   side checks `node merge` takes, accepts for a symbolic pair (equal ones included), and a
//!   reopen. The check runs one `search` from `into`, where `node merge` runs `reaches` once per
//!   node with an edge to `from` (see the harness).
//!
//! What stays a model: the facts the decisions read (the arrays), and `evolve`'s bookkeeping of
//! `NodeMoved` (the aliases) and `NodeClosed` (`merged_into`). The end-to-end form of each is the
//! keys universe and the unit tests above.
//!
//! **Unwind bounds** (D.12.5): 4 for `keys_open_unique` and `move_keeps_tree`, 5 for
//! `merge_redirects`, whose closure grows four times over four nodes; unwinding assertions are on,
//! so a loop that needed more would fail the harness.

use crate::helpers::restructure::{
    KeyAdd, MovePlace, join_adds, key_add, merge_sides, move_place, reopen_keeps,
};
use crate::helpers::tree::{Marks, Stack, search};
use crate::types::NodeId;

// ------------------------------------------------------------------------------------------------
// K1

/// The open carrier of key `k` among three nodes, the smallest index (K1 makes it unique).
fn open_carrier(open: &[bool; 3], carries: &[[bool; 3]; 3], k: usize) -> Option<NodeId> {
    for n in 0..3 {
        if open[n] && carries[n][k] {
            return Some(NodeId(n as u64));
        }
    }
    None
}

/// K1: every key has at most one open carrier.
fn k1(open: &[bool; 3], carries: &[[bool; 3]; 3]) -> bool {
    for k in 0..3 {
        let mut c = 0;
        for n in 0..3 {
            if open[n] && carries[n][k] {
                c += 1;
            }
        }
        if c > 1 {
            return false;
        }
    }
    true
}

/// K1 under create-or-join, `node key {add}`, reopen, and close, on three nodes and three keys
/// (A-90: key 2 is a `repeat:` key, which a command refuses; the server's occurrences may carry
/// it). Two keys a command may name let a create join one carrier while another carries its other
/// key.
#[kani::proof]
#[kani::unwind(4)]
fn keys_open_unique() {
    let mut exists: [bool; 3] = [true, true, kani::any()];
    let mut open: [bool; 3] = kani::any();
    let mut carries: [[bool; 3]; 3] = kani::any();
    for n in 0..3 {
        if !exists[n] {
            kani::assume(!open[n] && !carries[n][0] && !carries[n][1] && !carries[n][2]);
        }
    }
    kani::assume(k1(&open, &carries));
    for _ in 0..3 {
        let op: u8 = kani::any();
        let n: usize = kani::any();
        kani::assume(n < 3);
        let keys: [bool; 3] = kani::any();
        let k: usize = kani::any();
        kani::assume(k < 3);
        match op % 4 {
            // `node create {keys}`: a `repeat:` key is refused; a key an open node carries makes
            // the call a join of the smallest carrier, which gets the keys no open node carries;
            // otherwise a new node with the keys.
            0 => {
                if keys[2] {
                    continue;
                }
                let mut carrier: Option<usize> = None;
                for j in 0..3 {
                    if keys[j]
                        && let Some(c) = open_carrier(&open, &carries, j)
                    {
                        let c = c.0 as usize;
                        carrier = Some(carrier.map_or(c, |x: usize| x.min(c)));
                    }
                }
                match carrier {
                    Some(c) => {
                        // Every key's carrier before the join: the join adds keys one by one.
                        let before = [
                            open_carrier(&open, &carries, 0),
                            open_carrier(&open, &carries, 1),
                            open_carrier(&open, &carries, 2),
                        ];
                        for j in 0..3 {
                            if keys[j] && join_adds(before[j]) {
                                carries[c][j] = true;
                            }
                        }
                    }
                    None if !exists[2] => {
                        exists[2] = true;
                        open[2] = true;
                        carries[2] = keys;
                    }
                    None => {}
                }
            }
            // `node key {add}` of key k on node n.
            1 => {
                if k == 2 || !exists[n] {
                    continue;
                }
                let choice = key_add(
                    open_carrier(&open, &carries, k),
                    NodeId(n as u64),
                    carries[n][k],
                );
                if choice == KeyAdd::Add {
                    carries[n][k] = true;
                }
            }
            // `reopen` of a closed node n: the keys another open node carries are dropped (K7).
            2 => {
                if !exists[n] || open[n] {
                    continue;
                }
                for j in 0..3 {
                    if carries[n][j]
                        && !reopen_keeps(open_carrier(&open, &carries, j), NodeId(n as u64))
                    {
                        carries[n][j] = false;
                    }
                }
                open[n] = true;
            }
            // A close.
            _ => {
                if exists[n] {
                    open[n] = false;
                }
            }
        }
        assert!(k1(&open, &carries), "at most one open carrier per key");
    }
}

// ------------------------------------------------------------------------------------------------
// K3 and K4

/// The model's node `i` (0 is ROOT): ids above the reserved ones, as `node move` sees them.
fn id_of(i: usize) -> NodeId {
    if i == 0 {
        crate::types::ROOT
    } else {
        NodeId(7 + i as u64)
    }
}

/// K4: from every node, the parent walk reaches the root within three steps.
fn k4(parent: &[usize; 4]) -> bool {
    for i in 1..4 {
        let mut cur = i;
        let mut steps = 0;
        while cur != 0 && steps < 3 {
            cur = parent[cur];
            steps += 1;
        }
        if cur != 0 {
            return false;
        }
    }
    true
}

/// K3: live places `(parent, name)` are unique, and no alias sits at a live place.
fn k3(parent: &[usize; 4], name: &[u8; 4], alias: &[Option<(usize, u8)>; 2]) -> bool {
    for i in 1..4 {
        for j in (i + 1)..4 {
            if parent[i] == parent[j] && name[i] == name[j] {
                return false;
            }
        }
    }
    for a in alias.iter().flatten() {
        for i in 1..4 {
            if (parent[i], name[i]) == *a {
                return false;
            }
        }
    }
    true
}

/// K3 and K4 under any `node move` of a four-node tree: the move validation refuses a reserved
/// node, and `move_place` a target in the node's own subtree (K4) and a live name (K3), and makes
/// the node's own place a no-op; an accepted move keeps the tree a tree and the names unique.
#[kani::proof]
#[kani::unwind(4)]
fn move_keeps_tree() {
    let mut parent: [usize; 4] = [0, kani::any(), kani::any(), kani::any()];
    let mut name: [u8; 4] = [0, kani::any(), kani::any(), kani::any()];
    for i in 1..4 {
        kani::assume(parent[i] < 4 && name[i] < 2);
    }
    let mut alias: [Option<(usize, u8)>; 2] = [kani::any(), None];
    if let Some((p, nm)) = alias[0] {
        kani::assume(p < 4 && nm < 2);
    }
    kani::assume(k4(&parent) && k3(&parent, &name, &alias));
    let n: usize = kani::any();
    let p: usize = kani::any();
    let nm: u8 = kani::any();
    kani::assume(n < 4 && p < 4 && nm < 2);
    // The validation of `node move`, in D.8.24's order.
    if id_of(n).is_reserved() {
        assert!(n == 0);
        return;
    }
    let parent_of = |x: NodeId| -> Option<NodeId> {
        (0..4)
            .find(|&i| id_of(i) == x)
            .filter(|&i| i != 0)
            .map(|i| id_of(parent[i]))
    };
    // The facts `move_node` reads: whether `n` is at the place, and whether a live name is.
    let here = parent[n] == p && name[n] == nm;
    let live = (1..4).any(|i| parent[i] == p && name[i] == nm);
    match move_place(id_of(n), id_of(p), false, here, live, parent_of, 2) {
        MovePlace::Noop | MovePlace::Cycle | MovePlace::NameTaken => return,
        MovePlace::Move => {}
    }
    // `NodeMoved`: the old place becomes an alias; an alias at the new place goes.
    let old = (parent[n], name[n]);
    for a in alias.iter_mut() {
        if *a == Some((p, nm)) {
            *a = None;
        }
    }
    alias[1] = Some(old);
    parent[n] = p;
    name[n] = nm;
    assert!(k4(&parent), "K4: every node reaches the root");
    assert!(k3(&parent, &name, &alias), "K3: one node per place");
}

// ------------------------------------------------------------------------------------------------
// T5 and K5

/// The marks of a search over four nodes, in a fixed array.
struct ArrayMarks([Option<Option<NodeId>>; 4]);

impl Marks for ArrayMarks {
    fn visit(&mut self, x: NodeId, parent: Option<NodeId>) -> bool {
        let i = (x.0 & 3) as usize;
        if self.0[i].is_some() {
            return false;
        }
        self.0[i] = Some(parent);
        true
    }
    fn parent(&self, x: NodeId) -> Option<NodeId> {
        self.0[(x.0 & 3) as usize].flatten()
    }
}

/// The stack of a search over four nodes: each node is pushed once at most.
struct ArrayStack {
    buf: [NodeId; 4],
    len: usize,
}

impl Stack for ArrayStack {
    fn push(&mut self, x: NodeId) {
        assert!(self.len < 4, "each node is pushed once at most");
        self.buf[self.len] = x;
        self.len += 1;
    }
    fn pop(&mut self) -> Option<NodeId> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.buf[self.len])
    }
}

/// The transitive closure of four nodes' edges, as bit sets per node, grown four times.
fn closure(e: &[[bool; 4]; 4]) -> [u8; 4] {
    let mut reach = [0_u8; 4];
    for (i, row) in e.iter().enumerate() {
        for (j, &x) in row.iter().enumerate() {
            if x {
                reach[i] |= 1 << j;
            }
        }
    }
    for _ in 0..4 {
        for i in 0..4 {
            let mut r = reach[i];
            for j in 0..4 {
                if reach[i] & (1 << j) != 0 {
                    r |= reach[j];
                }
            }
            reach[i] = r;
        }
    }
    reach
}

fn acyclic(e: &[[bool; 4]; 4]) -> bool {
    let r = closure(e);
    (0..4).all(|i| r[i] & (1 << i) == 0)
}

/// T5 and N5 for a merge: with the edges after the merge given by `redirect` (each edge to `from`
/// goes to `into`, self-edges dropped), the merge's cycle check, "`into` reaches a node `w ≠ into`
/// with an edge to `from`", run with `search` from `into`, refuses exactly when those edges have a
/// cycle; and they have no edge to `from` and no self-edge. K5: with `merged_into` acyclic on closed
/// nodes before, an accepted merge of open nodes, and a reopen, keep it so.
///
/// Narrowed further than the other harnesses: the form that called `merge_edges` and ran `search`
/// once per node with an edge to `from`, as `node merge` runs `reaches`, made CBMC run out of
/// memory (its vectors of symbolic length). So the harness takes the edges after the merge from
/// `redirect`, which `merge_edges` applies to each edge it changes, and checks reachability from
/// `into` with one `search` towards a node outside the graph, reading the visited nodes from the
/// marks: the same predicate as `merge_cycle`'s `reaches` calls. That `merge_edges` lists exactly
/// the changed edges is the unit test `merge_edges_lists_the_changed_edges`.
#[kani::proof]
#[kani::unwind(5)]
fn merge_redirects() {
    let edges: [[bool; 4]; 4] = kani::any();
    kani::assume(acyclic(&edges));
    let from: usize = kani::any();
    let into: usize = kani::any();
    kani::assume(from < 4 && into < 4 && from != into);
    let id = |i: usize| NodeId(i as u64);
    // The edges after the merge, by `redirect`.
    let mut after = [[false; 4]; 4];
    for x in 0..4 {
        for t in 0..4 {
            if edges[x][t]
                && let Some(y) =
                    crate::helpers::restructure::redirect(id(x), id(t), id(from), id(into))
            {
                after[x][(y.0 & 3) as usize] = true;
            }
        }
    }
    for x in 0..4 {
        assert!(!after[x][from], "no edge to `from` is left");
        assert!(!after[x][x], "no self-edge");
    }
    // The cycle check of `node merge`: the nodes `into` reaches over the edges after the merge.
    let succ = |x: NodeId| {
        let row = after[(x.0 & 3) as usize];
        [
            row[0].then_some(NodeId(0)),
            row[1].then_some(NodeId(1)),
            row[2].then_some(NodeId(2)),
            row[3].then_some(NodeId(3)),
        ]
        .into_iter()
        .flatten()
    };
    let mut marks = ArrayMarks([None; 4]);
    let mut stack = ArrayStack {
        buf: [NodeId(0); 4],
        len: 0,
    };
    let outside = NodeId(64);
    assert!(!search(id(into), outside, succ, &mut marks, &mut stack, 4));
    let mut refused = false;
    for w in 0..4 {
        if w != into && edges[w][from] && marks.0[w].is_some() {
            refused = true;
        }
    }
    assert!(
        refused == !acyclic(&after),
        "the merge refuses exactly a cycle (T5)"
    );
    // K5 over `merged_into`: symbolic, acyclic, on closed nodes only, before.
    let mut open: [bool; 4] = kani::any();
    let mut merged: [Option<usize>; 4] = kani::any();
    for i in 0..4 {
        if let Some(m) = merged[i] {
            kani::assume(m < 4 && !open[i]);
        }
    }
    let k5 = |open: &[bool; 4], merged: &[Option<usize>; 4]| {
        (0..4).all(|i| {
            let mut cur = i;
            let mut steps = 0;
            while let Some(m) = merged[cur] {
                if steps >= 4 {
                    return false;
                }
                cur = m;
                steps += 1;
            }
            merged[i].is_none() || !open[i]
        })
    };
    kani::assume(k5(&open, &merged));
    // `node merge` of a symbolic pair, equal ones included: what `merge_sides` accepts (both open
    // and different) closes `from` as merged into `into`.
    let (mf, mi): (usize, usize) = (kani::any(), kani::any());
    kani::assume(mf < 4 && mi < 4);
    let reserved: bool = kani::any();
    if merge_sides(id(mf), id(mi), reserved, open[mf], open[mi]).is_none() {
        open[mf] = false;
        merged[mf] = Some(mi);
        assert!(k5(&open, &merged), "K5 after a merge");
    }
    // `node reopen` of a closed node clears its `merged_into`.
    let r: usize = kani::any();
    kani::assume(r < 4);
    if !open[r] {
        open[r] = true;
        merged[r] = None;
        assert!(k5(&open, &merged), "K5 after a reopen");
    }
}
