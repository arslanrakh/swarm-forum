#!/bin/sh
# Simulates the instances with `inv_all` and reports, for each, how many runs reached each witness
# of `forum.qnt` (model/README.md, "Simulation coverage"). A check over states that simulation
# never reaches passes vacuously, so a simulation's result is its verdict together with these
# counts.
#
# Usage, from model/:
#   scripts/simulate.sh <samples> [instance ...]
#
#   <samples>   runs of 40 steps per instance (`--max-samples`)
#   instance    two_class, one_class, no_user (default: all three)
#
# Environment: SEED (a fixed `--seed`, for a reproducible run; default: random), OUT (a directory
# for one log per instance; default: none, the output goes to the terminal), THREADS (the Rust
# evaluator's `--n-threads`; default: Quint's, one per core).
#
# Exit status: 0 when no instance violated `inv_all` and every witness in REQUIRED (below) was
# reached in at least one run of every instance that can reach it; 1 otherwise.

set -eu

[ $# -ge 1 ] || { sed -n '2,18p' "$0"; exit 2; }
samples=$1
shift
[ $# -ge 1 ] || set -- two_class one_class no_user

# Every witness of forum.qnt.
WITNESSES="w_ok_hello w_ok_create w_ok_update w_ok_link w_ok_key_add w_ok_key_remove w_ok_close
w_ok_reopen w_ok_move w_ok_merge w_ok_hide w_ok_take w_ok_next w_ok_checkpoint w_ok_renew w_ok_release
w_ok_from_user w_ok_to_user w_ok_route w_ok_ask_user w_ok_request w_ok_said w_ok_confirm w_ok_heard
w_ok_ask_person w_ok_ask_agent w_ok_answer w_ok_say w_ok_wait w_ok_decide w_ok_retract w_ok_approve
w_ok_object w_ok_resolve w_ok_check_intent w_ok_pause w_ok_unpause w_ok_view
w_replay w_op_conflict w_stale_token w_post_settle w_request_answered w_request_settled
w_user_request_answered w_moved_request w_commitment_fulfilled w_two_conversations w_conversations_apart w_pq_answered
w_pq_expired w_pq_escalated w_uq_answered w_uq_defaulted w_uq_lapsed w_uq_readdressed
w_wait_on_question w_refusal w_class_refused w_refusal_drop w_digest_shown w_decision_superseded
w_decision_retracted w_basis_two_nodes w_stale_basis w_merged w_signal w_missed w_assume_mark
w_open_intent w_person_and_team w_live_session_late w_upheld w_stale_offer"

# The witnesses that a simulation of 1,000 runs of each instance reaches in more than 0.5 % of its
# runs (README, "Simulation coverage", measured with several seeds): every command but `reopen`
# and `route`, a request answered and settled, a commitment fulfilled, a people question answered,
# and a node carrying two conversation keys. The others are reported and not required: at 1,000
# runs some of them are reached in a handful of runs or none.
REQUIRED="w_ok_hello w_ok_create w_ok_update w_ok_link w_ok_key_add w_ok_key_remove w_ok_close
w_ok_move w_ok_merge w_ok_hide w_ok_take w_ok_next w_ok_checkpoint w_ok_renew w_ok_release
w_ok_from_user w_ok_to_user w_ok_ask_user w_ok_request w_ok_said w_ok_confirm w_ok_heard
w_ok_ask_person w_ok_ask_agent w_ok_answer w_ok_say w_ok_wait w_ok_decide w_ok_retract w_ok_approve
w_ok_object w_ok_resolve w_ok_pause w_ok_unpause w_ok_view
w_request_answered w_request_settled w_commitment_fulfilled w_two_conversations w_pq_answered"
# What the forum with no human cannot reach: it has no `from_user` and no `to_user` action.
NO_USER_SKIPS="w_ok_from_user w_ok_to_user w_user_request_answered"

status=0
for inst in "$@"; do
  log=$(mktemp)
  echo "simulate $inst ($samples runs of 40 steps)"
  # shellcheck disable=SC2086
  if NODE_OPTIONS=--max-old-space-size=12000 npx quint run "instances/$inst.qnt" --max-steps 40 \
       --max-samples "$samples" --invariant inv_all --verbosity 1 ${SEED:+--seed "$SEED"} ${THREADS:+--n-threads "$THREADS"} \
       --witnesses $WITNESSES > "$log" 2>&1; then
    :
  else
    status=1
  fi
  grep -E '^\[(ok|violation)\]|^Use --seed|error' "$log" || true
  grep ' was witnessed in ' "$log" | sed -E 's/ was witnessed in ([0-9]+) trace\(s\) out of ([0-9]+) explored \(([0-9.]+)%\)/: \1 of \2 (\3 %)/'
  for w in $REQUIRED; do
    case "$inst" in no_user) case " $(echo $NO_USER_SKIPS) " in *" $w "*) continue ;; esac ;; esac
    if grep -q "^$w was witnessed in 0 trace" "$log" || ! grep -q "^$w was witnessed" "$log"; then
      echo "simulate.sh: $inst never reached $w" >&2
      status=1
    fi
  done
  if [ -n "${OUT:-}" ]; then mkdir -p "$OUT"; cp "$log" "$OUT/$inst.log"; fi
  rm -f "$log"
done
exit $status
