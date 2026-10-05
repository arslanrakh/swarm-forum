#!/usr/bin/env bash
# G-3, scale independence (design D.12.8, §18): the core's cost test,
# crates/forum-core/tests/cost.rs (owned by the core track), timed per tool with its large-forum
# additions. Pass: visits bounded by constants, apart from the recorded paths (a hub's citers, the
# boundary bucket); a time ratio large/small <= 1.5 for every tool. It runs the test in release if
# it exists, passing FORUM_COST_SCALE=smoke|full, and records the per-tool visits and times of its
# output in the verdict. It says "not yet" (exit 3) while the test is not D.12.8's: while it runs
# its own sizes (it does not read FORUM_COST_SCALE) or lacks query::guide or query::health. A
# failed test fails the gate either way.
#
# Usage: scripts/bench/g03-cost.sh [--smoke | --full]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
TITLE="scale independence (the core's cost test)"
TEST=crates/forum-core/tests/cost.rs
if [ ! -f "$TEST" ]; then
  verdict G-3 "$TITLE" not-yet "$TEST (the core track) does not exist"
  exit 3
fi
notes=()
ready=1
if ! grep -q FORUM_COST_SCALE "$TEST"; then
  notes+=("the test runs its own sizes (CI's), not D.12.8's small and large forums (10^4 and 10^6 nodes), at either scale")
  ready=0
fi
missing=""
for q in guide health; do
  grep -q "query::$q(" "$TEST" || missing="$missing query::$q"
done
if [ -n "$missing" ]; then
  notes+=("not in the test yet:$missing")
  ready=0
fi
LOG="$WORK/g3-$SCALE.log"
echo "running $TEST in release (FORUM_COST_SCALE=$SCALE)"
FORUM_COST_SCALE=$SCALE cargo test --release -p forum-core --test cost -- --nocapture 2>&1 | tee "$LOG"
tested=${PIPESTATUS[0]}
if [ "$tested" -ne 0 ]; then
  status=fail
elif [ "$ready" = 1 ]; then
  status=pass
else
  status=not-yet
fi
note=""
if [ "${#notes[@]}" -gt 0 ]; then
  note=$(printf '%s; ' "${notes[@]}")
  note=${note%; }
fi
verdict G-3 "$TITLE" "$status" "$note" --log "$LOG"
code=$?
[ "$code" -eq 2 ] && exit 2
case "$status" in
  pass) exit 0 ;;
  fail) exit 1 ;;
  *) exit 3 ;;
esac
