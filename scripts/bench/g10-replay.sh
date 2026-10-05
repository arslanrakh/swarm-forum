#!/usr/bin/env bash
# G-10, history replay (design D.12.8, FQ-2): the history of one forum day at the base profile (5 x
# 10^7 events) folds through evolve to state equal to the state tables, single-threaded, within 15
# min. Full: 10,000 agents at 8 times real time (3 h), then `forum-server replay-verify`. Smoke: a
# quarter of a forum day with 200 agents; the replay time is projected to 5 x 10^7 events.
#
# Usage: scripts/bench/g10-replay.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g10
