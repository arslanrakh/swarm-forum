#!/usr/bin/env bash
# G-11, group commit (design D.12.8, CS-13): a crash between the commit and the replies, and the
# live race test of D.12.7 with groups on. Pass: retries with an op_id return identical bytes;
# exactly one holder per slot. It reuses WP-F2's tests, crates/forum-server/tests/race.rs (and
# crash.rs when it exists), run in release, and says "not yet" while they do not exist.
#
# Usage: scripts/bench/g11-group-commit.sh [--smoke | --full]
# Exit: 0 pass, 1 fail, 2 error, 3 not yet.

# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
parse_args "$@"
TITLE="group commit (the race test of D.12.7)"
RACE=crates/forum-server/tests/race.rs
CRASH=crates/forum-server/tests/crash.rs
if [ ! -f "$RACE" ]; then
  verdict G-11 "$TITLE" not-yet "$RACE (WP-F2) does not exist"
  exit 3
fi
tests=(--test race)
[ -f "$CRASH" ] && tests+=(--test crash)
echo "running ${tests[*]} of forum-server in release (FORUM_RACE_SCALE=$SCALE)"
if FORUM_RACE_SCALE=$SCALE cargo test --release -p forum-server "${tests[@]}" -- --nocapture; then
  status=pass
else
  status=fail
fi
note=""
[ -f "$CRASH" ] || note="the crash test (crash.rs, WP-F2) does not exist yet: the race test only"
verdict G-11 "$TITLE" "$status" "$note"
