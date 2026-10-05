// -*- mode: Bluespec; -*-
/**
 * The flag predicates of D.8.3 (§14), one to one, over a settled state at `now`. The model stores no
 * flags (D.10.3): a flag is this predicate, evaluated where it is read. Every quantifier ranges over
 * the entities of the base state, as Rust's `spec::flags` does (D.9.1).
 */
module flags {
  import types.* from "./types"
  import params.* from "./params"
  import basics.* from "./basics"

  // ------------------------------------------------------------------ the open subsets of `ix.items` (CS-20)

  /// `items(n).open_pq[k] ≠ ∅`: an `Open` people question on `n` with conversation key `k`.
  pure def open_pq(s: State, n: int, k: Key): bool =
    s.pquestions.keys().exists(q => {
      val pq = s.pquestions.get(q)
      pq.node == n and pq.conv == k and pq.state == PqOpen
    })

  /// `items(n).open_commitments`.
  pure def open_commitments_on(s: State, n: int): Set[int] =
    s.commitments.keys().filter(c => s.commitments.get(c).node == n and s.commitments.get(c).state == CmOpen)

  /// `items(n).unconfirmed`.
  pure def unconfirmed_on(s: State, n: int): Set[int] =
    s.saids.keys().filter(x => s.saids.get(x).node == n and s.saids.get(x).state == SdUnconfirmed)

  /// `items(n).open_objections`.
  pure def open_objections_on(s: State, n: int): Set[int] =
    s.objections.keys().filter(o => s.objections.get(o).node == n and s.objections.get(o).state == ObOpen)

  /// `items(n).stale_basis`: the active decisions on `n` with a stale reference (D-4).
  pure def stale_basis_on(s: State, n: int): Set[int] =
    s.decisions.keys().filter(d => s.decisions.get(d).node == n and stale_decision(s, d))

  // ------------------------------------------------------------------ the fourteen flags

  /// `unrouted`: on USER, some `from_user` entry is unrouted.
  pure def f_unrouted(s: State, n: int, now: int): bool =
    n == USER and s.user_entries.keys().exists(e => s.user_entries.get(e).routing == Unrouted)

  /// `unanswered_request`: an unsettled request, and the node is closed, or unattended and not
  /// waiting.
  pure def f_unanswered_request(s: State, n: int, now: int): bool =
    unsettled_on(s, n) != Set()
    and (is_closed(s, n) or (not(attended(s, n, now)) and not(waiting(s, n, now))))

  /// `open_commitment`: an open commitment, and the node is closed, or it is unattended, the
  /// follow-up has passed, and no open question to that person is on the node.
  pure def f_open_commitment(s: State, n: int, now: int): bool =
    open_commitments_on(s, n).exists(c => {
      val cm = s.commitments.get(c)
      is_closed(s, n)
      or (not(attended(s, n, now)) and now >= cm.followup_at and not(open_pq(s, n, cm.conv)))
    })

  /// `unconfirmed_said`: an unconfirmed `said`, and the node is unattended.
  pure def f_unconfirmed_said(s: State, n: int, now: int): bool =
    unconfirmed_on(s, n) != Set() and not(attended(s, n, now))

  /// `open_intent`: an open intent, no live hold on the node itself, and the node is not waiting.
  pure def f_open_intent(s: State, n: int, now: int): bool =
    s.nodes.get(n).open_intent.eid >= 0 and not(held_self(s, n, now)) and not(waiting(s, n, now))

  /// `question_recheck`: a recheck mark.
  pure def f_question_recheck(s: State, n: int, now: int): bool = s.nodes.get(n).recheck_marks != Set()

  /// `proceed_on_assumption`: an assume mark.
  pure def f_proceed_on_assumption(s: State, n: int, now: int): bool = s.nodes.get(n).assume_marks != Set()

  /// `objection`: an open objection.
  pure def f_objection(s: State, n: int, now: int): bool = open_objections_on(s, n) != Set()

  /// `basis_changed`: an active decision with a stale reference, on a node that is not merged (D-4).
  pure def f_basis_changed(s: State, n: int, now: int): bool =
    stale_basis_on(s, n) != Set() and not(merged(s, n))

  /// `too_big`: open, and the attempts reached `too_big.attempts` (open nodes only, D.8.3).
  pure def f_too_big(s: State, n: int, now: int): bool =
    is_open(s, n) and s.nodes.get(n).attempts >= TOO_BIG_ATTEMPTS

  /// `overdue`: open, and the due has passed (an occurrence's due is its window end).
  pure def f_overdue(s: State, n: int, now: int): bool =
    is_open(s, n) and s.nodes.get(n).due >= 0 and now >= s.nodes.get(n).due

  /// `ready_to_close`: open, not reserved, held at least once (its fence is past 0), at least one
  /// child, and every child closed (1.6: a node never held is a folder, which its children's closing
  /// does not finish).
  pure def f_ready_to_close(s: State, n: int, now: int): bool =
    is_open(s, n) and s.nodes.get(n).origin != OReserved and s.nodes.get(n).fence > 0
    and children(s, n) != Set() and open_children(s, n) == Set()

  /// `missed`: an open series whose last closed occurrence closed as `missed` (open nodes only).
  pure def f_missed(s: State, n: int, now: int): bool =
    is_open(s, n) and match s.nodes.get(n).role { | Series(x) => x.last_missed | _ => false }

  /// `stale` (1.8, §14): an open node that carries `offer`, has capacity, and is not a container,
  /// with no live hold on itself and no unresolved wait, untouched for `offer.stale` (open nodes
  /// only).
  pure def f_stale(s: State, n: int, now: int): bool = {
    val nd = s.nodes.get(n)
    and {
      is_open(s, n),
      nd.tags.contains(OFFER),
      nd.capacity > 0,
      not(is_container(s, n)),
      not(held_self(s, n, now)),
      not(waiting(s, n, now)),
      nd.touched >= 0,
      now >= nd.touched + OFFER_STALE,
    }
  }

  /// `flags_of(s, n, now)`: one flag per predicate above.
  pure def flags_of(s: State, n: int, now: int): Set[Flag] =
    if (not(has_node(s, n))) Set()
    else Set(
      (FUnrouted, f_unrouted(s, n, now)),
      (FUnansweredRequest, f_unanswered_request(s, n, now)),
      (FOpenCommitment, f_open_commitment(s, n, now)),
      (FUnconfirmedSaid, f_unconfirmed_said(s, n, now)),
      (FOpenIntent, f_open_intent(s, n, now)),
      (FQuestionRecheck, f_question_recheck(s, n, now)),
      (FProceedOnAssumption, f_proceed_on_assumption(s, n, now)),
      (FObjection, f_objection(s, n, now)),
      (FBasisChanged, f_basis_changed(s, n, now)),
      (FTooBig, f_too_big(s, n, now)),
      (FOverdue, f_overdue(s, n, now)),
      (FReadyToClose, f_ready_to_close(s, n, now)),
      (FMissed, f_missed(s, n, now)),
      (FStale, f_stale(s, n, now)),
    ).filter(p => p._2).map(p => p._1)

  pure def flagged(s: State, n: int, now: int): bool = flags_of(s, n, now) != Set()

  // ------------------------------------------------------------------ flag sets that `next` reads (D.8.5)

  /// The flags that make a closed node a candidate for `next` (A-10, D-4): every flag a closed node
  /// that is not merged can carry.
  pure val CLOSED_TAKE: Set[Flag] = Set(
    FUnansweredRequest, FOpenCommitment, FObjection, FOpenIntent, FUnconfirmedSaid, FBasisChanged
  )

  /// The flags of tier 0 (§10.4).
  pure val T0: Set[Flag] = Set(
    FUnrouted, FUnansweredRequest, FOpenCommitment, FUnconfirmedSaid, FOpenIntent, FQuestionRecheck,
    FProceedOnAssumption, FBasisChanged
  )
}
