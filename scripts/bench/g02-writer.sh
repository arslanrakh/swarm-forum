#!/usr/bin/env bash
# G-2, writer utilisation (design D.12.8, CS-13, CS-14): the writer's busy share during G-1's run,
# from the server's own counters (the bench feature). Pass: <= 30 % on average at the base mix, <=
# 60 % in a heavy burst. It runs G-1's scenario; under run-all.sh (BENCH_REUSE=1) it reuses the G-2
# verdict that G-1's run wrote just before.
#
# Usage: scripts/bench/g02-writer.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
if [ "${BENCH_REUSE:-0}" = 1 ] && [ -f "$RESULTS/g2-$SCALE.json" ]; then
  gate g2 --reuse "$RESULTS/g2-$SCALE.json"
else
  gate g2
fi
