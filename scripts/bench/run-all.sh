#!/usr/bin/env bash
# Runs every scale gate of design D.12.8 (G-1 to G-12) at one scale, each once, then prints a
# summary: each gate's pass, fail, not yet, or error, with its result file, and whether its
# numbers are provisional (some commands of the mix still stubs). G-2 reuses the verdict of G-1's
# run. This is the one entry point for the gates together: `scripts/verify.sh --bench` should call
# it rather than each gate script, since a gate script's exit 3 ("not yet") is no failure.
#
# Usage: scripts/bench/run-all.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 when no gate failed or erred ("not yet" is not a failure), else 1.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
export BENCH_SCALE=$SCALE BENCH_REUSE=1

build || exit 2
# Results of an earlier run must not pass for this one's.
rm -f "$RESULTS"/g*-"$SCALE".json

rows=()
failed=0
for script in "$BENCH_ROOT"/scripts/bench/g[0-9][0-9]-*.sh; do
  name=$(basename "$script" .sh)
  num=$((10#${name:1:2}))
  file="$RESULTS/g$num-$SCALE.json"
  echo
  echo "################ $name ($SCALE) ################"
  bash "$script" "--$SCALE" ${EXTRA[@]+"${EXTRA[@]}"}
  case $? in
    0) st=pass ;;
    1) st=FAIL; failed=1 ;;
    3) st="not yet" ;;
    *) st=ERROR; failed=1 ;;
  esac
  # A verdict with reasons in its "provisional" list.
  if [ "$st" != ERROR ] && [ -f "$file" ] && grep -q '"provisional": \[$' "$file"; then
    st="$st (provisional)"
  fi
  rows+=("$(printf '%-22s %-20s %s' "$name" "$st" "$file")")
done

echo
echo "################ summary ($SCALE) ################"
for r in "${rows[@]}"; do echo "$r"; done
exit "$failed"
