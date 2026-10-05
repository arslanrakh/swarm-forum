#!/usr/bin/env bash
# G-9, search (design D.12.8, CS-19): `hello` search over 10^8 indexed documents. Pass: the
# server's search for a worst-case `hello` intent, p99 <= 50 ms with the budget (measured on the
# server, apart from the call); the writer's busy time unchanged (within 10 %) under concurrent
# worst-case searches, against cheap searches that find as many hits, in alternating phases. Full:
# fills 10^8 documents over HTTP first (hours). Smoke: 50,000 documents.
#
# Usage: scripts/bench/g09-search.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g9
