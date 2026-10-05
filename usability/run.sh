#!/usr/bin/env bash
# The live-agent usability test of WP-U (design D.13.2). It runs only when called, and
# `scripts/verify.sh --usability` calls it (WP-F3). It spends model tokens: model API keys and
# logins come from the environment and the CLIs' own logins (`claude`, `codex`), never from this
# repository.
#
# Usage: usability/run.sh <command> [options]; usability/run.sh --help lists them. Examples:
#   usability/run.sh list
#   usability/run.sh probe --model claude:claude-sonnet-5-5
#   usability/run.sh run --scenario s0-smoke --model claude:claude-sonnet-5-5 --arm g,nog --runs 1
#   usability/run.sh campaign --model claude:claude-sonnet-5-5 --model claude:claude-opus-5-5 \
#     --model codex:gpt-6.1-sol --model codex:gpt-6-astra --allow-unisolated
#   usability/run.sh report --out usability/out
#
# It builds forum-server with the test-only feature `usability` (a debug build: the feature does
# not compile in release), then hands over to the harness (Python 3.10 or later, standard library
# only). Runs land in usability/out/ unless --out says otherwise; that directory is not tracked.

set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)

if [ "$#" -eq 0 ]; then
  sed -n '2,17p' "$0"
  exit 2
fi

python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)' || {
  echo "usability/run.sh: Python 3.10 or later is needed" >&2
  exit 2
}

case "$1" in
  list | report | recount | interview | -h | --help) ;;
  *)
    cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p forum-server --features usability \
      --bin forum-server
    ;;
esac

cd "$HERE"
exec python3 -m harness "$@"
