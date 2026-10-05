#!/usr/bin/env bash
# G-1, latency (design D.12.8, D.15.4: 10,000 agents and a company): 10,000 simulated agents with
# the base mix for 1 h, then the heavy mix with 3x bursts for 15 min; within it a `talk say {to:
# holders:/**}` every minute, and a pause of / after which every agent calls `control health` once
# within a minute. One session in ten serves one of 1,000 principals, a third park in `talk wait`,
# and half vanish mid-task. Pass: p99 of writes <= 50 ms, reads <= 20 ms, `hold next` <= 30 ms,
# `hello` with search <= 100 ms; p99.9 <= 250 ms; no unexpected error codes; the health herd costs
# one computation per HEALTH_REFRESH, and its p99 holds. A call counts in the phase of its
# intended time; answers of commands that are still stubs are in no latency, and make the verdict
# provisional. Writes G-2's verdict too (the same run). Smoke: 1,000 agents on a clock 10 times as
# fast (full's call rate), 2 min base, 1 min heavy.
#
# Usage: scripts/bench/g01-latency.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g1
