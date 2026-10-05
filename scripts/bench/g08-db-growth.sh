#!/usr/bin/env bash
# G-8, growth of the database file (design D.12.8, FQ-2): bytes per served call on the base mix
# (waits included, as D.15.2's rates count them; stubs left out), the whole file after a clean
# shutdown, and from it the months until SQLite's 17.59 TB at the low, base, and high profiles;
# bytes written per append (the WAL frames the commits write) on a node with a 64 KiB body and
# 10^4 keys within 1.5 times those on a fresh node. Smoke: 300 agents on a clock 10 times as fast
# for 90 s.
#
# Usage: scripts/bench/g08-db-growth.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g8
