#!/usr/bin/env bash
# G-12, connections (design D.12.8, CS-17): 10,000 MCP sessions, one connection each, 5,000 of
# them in `talk wait`, at the base rate per agent on a clock at real time. Pass: no
# file-descriptor errors (the driver's, and accept or open-file errors in the server's log), and no
# transport errors; server memory <= 2 GB; the shell's maps shrink after presence.silence, which
# comes by a jump of the server's clock. Smoke: 2,000 sessions, 1,000 waiting, live for 60 s.
#
# Usage: scripts/bench/g12-connections.sh [--smoke | --full] [swarm-sim gate options]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
gate g12
