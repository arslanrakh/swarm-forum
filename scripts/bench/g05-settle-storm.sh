#!/usr/bin/env bash
# G-5, settle storm (design D.12.8, CS-30): 10,000 Silence and 5,000 Lease deadlines at one
# instant, while probes keep calling. The sessions open and take their holds on a clock at real
# time; then the server's clock jumps past every deadline at once (the bench feature's jump), so
# that all fall due together. Pass: no call waits more than 1 s; p99 over the minute after the
# jump <= 250 ms; every deadline fired. Smoke: 2,000 silences and 1,000 leases, 30 s after the
# jump.
#
# Usage: scripts/bench/g05-settle-storm.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g5
