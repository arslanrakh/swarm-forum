#!/usr/bin/env bash
# G-6, memory (design D.12.8, CS-8, CS-17, D-5): 30 days of forum time at the base profile on a
# compressed clock. Pass: resident memory <= 10 GB, flat after FINISHED_RETAIN (its growth a day
# after day 7 at most a fifth of its growth before); sessions, op records, notes, agent questions,
# digest entries, and the shell's maps (the bench feature's counts) stop growing alike. Full:
# 10,000 agents at 8 times real time (90 h; `--days` shortens it). Smoke: 30 agents, 15 forum days
# at 1,500 times real time (about 15 min), so that a tail as long as the retention follows it.
#
# Usage: scripts/bench/g06-memory.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g6
