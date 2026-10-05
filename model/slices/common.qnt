// -*- mode: Bluespec; -*-
/**
 * What the slices of D.10.7 share (WP-A3): the small sets their commands pick from, the six checks in
 * every slice, and the state predicates of the liveness properties (§27).
 *
 * A slice (`slices/<x>.qnt`) restricts `step` to the actions its invariants need, so that Apalache
 * stays tractable and TLC has a chance to terminate (D.10.7). Each slice file has a module of pure
 * definitions, `<x>` (the commands its actions pick from and the conjunction of its invariants), and
 * one module per class configuration (`<x>_two`, `<x>_one`; `user_none` besides), which instantiates
 * `forum.qnt` and defines `step_<x>`, `inv_<x>`, and, for `holds`, `recurring`, and `user`,
 * `liveness_<x>`. Quint 0.33 does not pass a module's constants on to an instance it makes, so the
 * class configuration cannot be a constant of `<x>`, and each configuration module repeats the short
 * list of the slice's actions.
 *
 * A command action of a slice is `slice_call(name, cmds, op_long)` of `forum.qnt`: it picks a live
 * session and a whole command from a small set, and records the command in `last_action` under D.10.4's
 * action name. Outside `holds`, every call has no `op` and no long text: `long` is fixed at `false`
 * outside `holds` (D.10.4), and O1, the only invariant that reads `op`, is checked in `holds` (README,
 * "Deviations").
 */
module common {
  import types.* from "../types"
  import params.* from "../params"
  import basics.* from "../basics"
  import flags.* from "../flags"
  import deadlines.* from "../deadlines"
  import commands.* from "../commands"
  import invariants.* from "../invariants"

  // ------------------------------------------------------------------ picks

  /// The seed's nodes in every configuration (D.10.7): `/w1` (key `Conv("c1")`) and `/w2` (key
  /// `Conv("c2")`); the `recurring` seed adds the series `/s` and its first occurrence.
  pure val SEED_W1 = 8
  pure val SEED_W2 = 9
  pure val SEED_SERIES_NODE = 10

  /// No `op` and no long text.
  pure val PLAIN: Set[(str, bool)] = Set(("", false))

  /// The nodes a slice's commands name: every node an agent or the server made, the reserved nodes
  /// `reserved` (so that K8's refusals are exercised, D.10.7), and the next id, which names none.
  pure def slice_nodes(st: State, reserved: Set[int]): Set[int] =
    st.nodes.keys().filter(n => n >= FIRST_FREE_NODE).union(reserved).union(Set(st.next_node))

  /// The nodes an agent or the server made.
  pure def made_nodes(st: State): Set[int] = st.nodes.keys().filter(n => n >= FIRST_FREE_NODE)

  /// The tokens a write names: every hold's, and one spent token (H4), the latest fence of the
  /// lowest node whose latest hold has ended, or a token of ROOT that was never issued (as
  /// `forum.qnt`'s `token_picks`).
  pure def slice_tokens(st: State): Set[(int, int)] = {
    val spent = st.nodes.keys().filter(n => st.nodes.get(n).fence > 0 and not(st.holds.keys().contains((n, st.nodes.get(n).fence))))
    val m = set_min(spent, -1)
    st.holds.keys().union(Set(if (m >= 0) (m, st.nodes.get(m).fence) else (ROOT, 1)))
  }

  /// The nodes that carry a conversation key, where `request`, `said`, `heard`, and `ask` to a
  /// person succeed, and ROOT, which carries none (`invalid(NoConversationKey)`).
  pure def conv_nodes(st: State): Set[int] =
    st.nodes.keys().filter(n => st.nodes.get(n).keys.exists(k => is_conversation(k))).union(Set(ROOT))

  pure def create_cmd(parent: int, name: Name, ks: Set[Key], take: bool, capacity: int, owners: Set[int], rep: RepeatChange): Cmd =
    CCreate({ parent: parent, name: name, keys: ks, take: take, capacity: capacity, owners: owners, repeat: rep, tags: Set() })

  /// An update of node `n` at its version that changes only what the arguments give.
  pure def update_cmd(st: State, n: int, f: { dver: int, digest: int, capacity: int, needs_add: Set[str],
                      needs_remove: Set[str], tags_add: Set[str], tags_remove: Set[str], depends: IdsChange,
                      owners: IdsChange, repeat: RepeatChange }): Cmd = {
    val v = if (st.nodes.keys().contains(n)) st.nodes.get(n).version else 1
    CUpdate({
      node: n, ver: v, dver: f.dver, content: -1, status: -1, digest: f.digest, capacity: f.capacity,
      needs_add: f.needs_add, needs_remove: f.needs_remove, tags_add: f.tags_add, tags_remove: f.tags_remove,
      due: DueKeep, depends: f.depends, owners: f.owners, repeat: f.repeat,
    })
  }

  pure val NO_UPDATE: { dver: int, digest: int, capacity: int, needs_add: Set[str], needs_remove: Set[str],
                        tags_add: Set[str], tags_remove: Set[str], depends: IdsChange, owners: IdsChange,
                        repeat: RepeatChange } = {
    dver: -1, digest: -1, capacity: -1, needs_add: Set(), needs_remove: Set(), tags_add: Set(), tags_remove: Set(),
    depends: IdsKeep, owners: IdsKeep, repeat: RepKeep,
  }

  // ------------------------------------------------------------------ the checks in every slice (D.10.7)

  /// `step_O3`, `step_T4` (on each step of the action, `steps_of`), `inv_T3`, `inv_T4`, `inv_K8`,
  /// and `inv_settle_bound`.
  pure def common_invariants(cs: Steps, post: State, clk: int, la: Action): bool = and {
    each_step(cs, c => step_O3(c) and step_T4(c)),
    inv_T3(post, clk), inv_T4(post, clk, la), inv_K8(post), inv_settle_bound(la),
  }
}
