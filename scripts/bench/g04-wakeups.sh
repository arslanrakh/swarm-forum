#!/usr/bin/env bash
# G-4, talk wait wake-ups (design D.12.8, CS-15): part A, 5,000 sessions parked in `talk wait`
# while writers are added until the commits reach 500 a second, the group commit's ceiling
# (1/COMMIT_LINGER), of which the gate asks 90 %; part B, 5,000 sessions parked on one node that
# gets 10 appends a second. Pass: the parked sessions the wake-ups examine per commit <= the waits
# the commits concern; the commit rate reached; wake latency p99 <= 100 ms (within WAIT_COALESCE
# for the hot node); wake work <= 5 % of the writer; the hot node's woken waits append no event and
# keep the writer within G-2. Smoke: 1,000 parked sessions, 200 commits a second, 30 s per part.
#
# Usage: scripts/bench/g04-wakeups.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g4
