#!/usr/bin/env bash
# G-7, startup (design D.12.8, FQ-2, D-5, D.11.8): startup after 365 days of forum time at the high
# profile, and the same after a `shell_version` bump. Pass: serving within 5 minutes in both,
# loading only the resident rows and reading no history: the statements startup runs on the
# history tables (the bench feature's trace) take at most a few bounded lookups' steps. Full:
# 10,000 agents at 4 times real time, which over HTTP takes about 91 days of real time (`--days`
# shortens it). Smoke: 30 agents, 2 forum days at 600 times real time (about 5 min).
#
# Usage: scripts/bench/g07-startup.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g7
