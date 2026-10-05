#!/bin/sh
# Regenerates the conformance fixtures of crates/forum-conformance/tests/traces/ from the Quint
# model (design D.10.8, D.0.3, WP-F1).
#
# Usage, from anywhere in the repository:
#   crates/forum-conformance/scripts/traces.sh           the committed fixtures
#   crates/forum-conformance/scripts/traces.sh --sweep   200 traces of each module, every one kept,
#                                                        into model/traces/conformance/ (git-ignored),
#                                                        for tests/sweep.rs
#
# Needs Node.js and Quint 0.33.0 in model/node_modules (`npm ci` in model/). Every run uses a fixed
# seed and one thread, so a regeneration writes the same files.
#
# What it writes (each trace starts at the seed of D.10.7 and records `last_action`):
#   holds/      `quint run --mbt` of slices/holds.qnt, modules holds_two and holds_one, and of the
#               same slice with its commands picked more often than time (quint/conformance.qnt:
#               holds_busy_two, holds_busy_one), whose runs call more before their sessions retire;
#               of 60 runs of each, the richest (scripts/compact.js)
#   keys/       the keys slice restricted to the commands the core implements
#               (quint/conformance.qnt: keys_core_two, keys_core_one)
#   keys_full/  the whole keys slice (keys_two, keys_one): its key, link, move, merge, reopen, and
#               talk commands wait for WP-D4 and WP-D5b, so tests/keys.rs replays them to their end
#               only in an ignored test
#   cex/        a counterexample of the holds slice for a deliberately false invariant, "no node is
#               ever held twice" (`fence <= 1`), which the core must reproduce (D.10.8)
#   findings/   run 49 of 200 of holds_two (seed 0x5eed01; since 1.8 run 50 of quint/conformance.qnt's
#               holds_silence_two, holds_two without 1.8's intent check), on which the model and the core
#               disagreed until the model got 1.4's Silence key (R27; tests/findings.rs); and the
#               first run of post_settle_two (quint/conformance.qnt, seed 0x5eed0a) that ends with
#               a release, with a `{time}` wait at its `now`, whose post-settle fires (D.8.1), on
#               which they disagreed until the model's commands post-settled
#   post_settle/ with --sweep only: 200 runs of post_settle_two
#   said_held/  with --sweep only: 200 runs of said_held_two (1.9), whose `said` entries are
#               recorded while another session may hold their conversation node (1.9 withheld
#               them; since 1.10 they count)
#   review/     with --sweep only: 200 runs of review_two (1.10): objections, resolves with each
#               verdict (second verdicts and withdrawn corrections), notes without `to`, and
#               checkpoints that end a wait; 1.11: deferrals of reviews, and notes at closed nodes
#               for the session that closed them
#   finish/     with --sweep only: 200 runs of finish_two (1.12): releases that settle requests,
#               retracts that withdraw questions to people, notes to holder: of closed nodes, and
#               reopens that tag their node offer

set -eu

# Quint holds every trace of a run in memory, and each state carries `s`, `prev`, and `mid`: 200 runs
# of 40 steps pass Node's default heap, as they do for the model's `npm run traces`.
export NODE_OPTIONS="${NODE_OPTIONS:---max-old-space-size=12000}"

here=$(cd "$(dirname "$0")/.." && pwd)
model=$(cd "$here/../../model" && pwd)
conformance_qnt="$here/quint/conformance.qnt"
compact="$here/scripts/compact.js"

sweep=false
[ "${1:-}" = "--sweep" ] && sweep=true

if $sweep; then
  out="$model/traces/conformance"
  n=200
else
  out="$here/tests/traces"
  n=60
fi
work="$model/traces/conformance-work"
rm -rf "$work"
mkdir -p "$work"

cd "$model"

# The files t0 … t<count - 1> of a directory, in that order.
numbered() {
  i=0
  while [ "$i" -lt "$2" ]; do
    printf '%s\n' "$1/t$i.itf.json"
    i=$((i + 1))
  done
}

# run <dir> <file> <main> <step> <steps> <traces> <samples> <seed> [quint options …]
run() {
  dir=$1 file=$2 main=$3 step=$4 steps=$5 traces=$6 samples=$7 seed=$8
  shift 8
  mkdir -p "$dir"
  npx quint run "$file" --main "$main" --step "$step" --mbt \
    --max-steps "$steps" --n-traces "$traces" --max-samples "$samples" \
    --seed "$seed" --n-threads 1 --verbosity 0 --out-itf "$dir/t{seq}.itf.json" "$@"
}

# gen <group> <prefix> <file> <main> <step> <steps> <keep> <seed>
gen() {
  group=$1 prefix=$2 file=$3 main=$4 step=$5 steps=$6 keep=$7 seed=$8
  $sweep && keep=0
  echo "== $prefix: $n runs of $steps steps of $main, seed $seed"
  run "$work/$prefix" "$file" "$main" "$step" "$steps" "$n" "$n" "$seed"
  mkdir -p "$out/$group"
  rm -f "$out/$group/$prefix"-*.itf.json
  # shellcheck disable=SC2046 # one argument per file
  node "$compact" "$keep" "$out/$group" "$prefix" $(numbered "$work/$prefix" "$n")
}

gen holds holds_two slices/holds.qnt holds_two step_holds 30 2 0x5eed01
gen holds holds_one slices/holds.qnt holds_one step_holds 30 1 0x5eed02
gen holds holds_busy_two "$conformance_qnt" holds_busy_two step_holds_busy 40 3 0x5eed08
gen holds holds_busy_one "$conformance_qnt" holds_busy_one step_holds_busy 40 2 0x5eed09
gen keys keys_core_two "$conformance_qnt" keys_core_two step_keys_core 30 2 0x5eed03
gen keys keys_core_one "$conformance_qnt" keys_core_one step_keys_core 30 2 0x5eed04
gen keys_full keys_two slices/keys.qnt keys_two step_keys 20 2 0x5eed05
gen keys_full keys_one slices/keys.qnt keys_one step_keys 20 1 0x5eed06
if $sweep; then
  gen post_settle post_settle_two "$conformance_qnt" post_settle_two step_post_settle 40 0 0x5eed0a
  gen said_held said_held_two "$conformance_qnt" said_held_two step_said_held 30 0 0x5eed0b
  gen review review_two "$conformance_qnt" review_two step_review 30 0 0x5eed0c
  gen finish finish_two "$conformance_qnt" finish_two step_finish 30 0 0x5eed0d
fi

# The counterexample: `quint run` stops at the first violation and writes that run (status
# "violation"); its exit status is then 1.
echo "== cex: holds_two, invariant fence <= 1"
if run "$work/cex" slices/holds.qnt holds_two step_holds 30 1 5000 0x5eed07 \
  --invariant 's.nodes.keys().forall(n => s.nodes.get(n).fence <= 1)'; then
  echo "no counterexample found" >&2
  exit 1
fi
mkdir -p "$out/cex"
rm -f "$out/cex/"*.itf.json
node "$compact" 0 "$out/cex" holds_two_fence "$work/cex/t0.itf.json"

# The finding (until 1.8, the sweep had the same run among its holds_two traces).
if ! $sweep; then
  echo "== findings: holds_silence_two run 50 of 200, seed 0x5eed01"
  run "$work/findings" "$conformance_qnt" holds_silence_two step_holds_silence 30 200 200 0x5eed01
  mkdir -p "$out/findings"
  rm -f "$out/findings/"*.itf.json
  node "$compact" 0 "$out/findings" holds_two_silence "$work/findings/t50.itf.json"

  # `quint run` stops at the first state that breaks the invariant and writes that run (status
  # "violation"); its exit status is then 1.
  echo "== findings: post_settle_two, the first release with a wait at now whose post-settle fires"
  if run "$work/post_settle" "$conformance_qnt" post_settle_two step_post_settle 40 1 5000 0x5eed0a \
    --invariant 'not(w_release_wait_now)'; then
    echo "no release whose post-settle fires" >&2
    exit 1
  fi
  node "$compact" 0 "$out/findings" post_settle_release "$work/post_settle/t0.itf.json"
fi

rm -rf "$work"
