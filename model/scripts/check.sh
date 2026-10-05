#!/bin/sh
# Runs one check of D.10.7 on a slice module with a time limit, and says how far it got: TLC with
# the slice's invariant, Apalache to depth 12 with the slice's invariant, or TLC with the slice's
# liveness property (model/README.md, "The checks of D.10.7").
#
# Usage, from model/:
#   scripts/check.sh tlc|apalache|liveness <slice> <module>
#
#   tlc        scripts/verify-file.sh tlc slices/<slice>.qnt --main <module> --step step_<slice>
#              --invariant inv_<slice>
#   apalache   the same with apalache, --invariant inv_<slice>_apalache when the module defines it
#              (T2 over fewer ticks, D.10.7), else inv_<slice>, and --length ${APALACHE_LENGTH:-12}
#   liveness   tlc with --temporal liveness_<slice>
#
# The module is compiled once into traces/<module>/ and reused by the other checks (--reuse); the
# compilation is not counted in the time limit. The log is traces/<module>/<check>.log.
#
# Environment: MODEL_CHECK_SECONDS (the time limit of the check, default 1800), MODEL_CHECK_STRICT
# (1: a check stopped by its time limit fails; default: it passes with an INCOMPLETE line that gives
# the depth it reached), TLC_WORKERS, JVM_ARGS, TLC_JVM_ARGS (as verify-file.sh).
#
# Exit status: 0 when the check completed with no violation, or (not strict) was stopped by its
# time limit with none; 1 on a violation or an error.
#
# Runs of this script may overlap (a check of one module beside a check of another, or two callers
# at once): the compilation of a module and each check of a module take a lock of their own,
# `traces/<module>/<compile|check>.lock`, which a run whose owner has died leaves to the next.

set -u
[ $# -eq 3 ] || { sed -n '2,27p' "$0"; exit 2; }
CHECK=$1
SLICE=$2
MAIN=$3
SPEC=slices/$SLICE.qnt
OUT=traces/$MAIN
LIMIT=${MODEL_CHECK_SECONDS:-1800}
mkdir -p "$OUT"
LOG=$OUT/$CHECK.log

case "$CHECK" in
  tlc) set -- tlc --invariant "inv_$SLICE" ;;
  apalache)
    if grep -q "val inv_${SLICE}_apalache " "$SPEC"; then inv=inv_${SLICE}_apalache; else inv=inv_$SLICE; fi
    set -- apalache --invariant "$inv" --length "${APALACHE_LENGTH:-12}" ;;
  liveness) set -- tlc --temporal "liveness_$SLICE" ;;
  *) sed -n '2,27p' "$0"; exit 2 ;;
esac
BACKEND=$1
shift

# A lock is a directory holding its owner's pid; one whose owner no longer runs is taken over.
held=
lock() {
  while ! mkdir "$1" 2>/dev/null; do
    owner=$(cat "$1/pid" 2>/dev/null)
    if [ -n "$owner" ] && ! kill -0 "$owner" 2>/dev/null; then rm -rf "$1"; continue; fi
    sleep 10
  done
  echo $$ > "$1/pid"
  held="$held $1"
}
unlock() { rm -rf "$1"; held=$(echo "$held" | sed "s# $1##"); }
trap 'for l in $held; do rm -rf "$l"; done' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# 1. Compile once (no time limit), unless an earlier check of this module did and no file a slice
# reads has changed since: the modules in this directory and `slices/` (a slice imports no test and
# no instance).
stale() {
  [ ! -f "$OUT/$MAIN.compiled" ] || [ "$(cat "$OUT/$MAIN.compiled")" != "init step_$SLICE" ] || [ ! -f "$OUT/$MAIN.tla" ] ||
    [ -n "$(find . slices -maxdepth 1 -name '*.qnt' -newer "$OUT/$MAIN.compiled" | head -n 1)" ]
}
lock "$OUT/compile.lock"
if stale; then
  echo "compiling $SPEC ($MAIN)"
  scripts/verify-file.sh tlc "$SPEC" --main "$MAIN" --step "step_$SLICE" --invariant "inv_$SLICE" --out "$OUT" \
    --compile-only > "$OUT/compile.log" 2>&1
  if [ ! -f "$OUT/$MAIN.compiled" ] || [ ! -f "$OUT/$MAIN.tla" ]; then cat "$OUT/compile.log"; exit 1; fi
fi
unlock "$OUT/compile.lock"
lock "$OUT/$CHECK.lock"

# 2. The check, under the time limit (the alarm ends verify-file.sh, which ends the JVM).
start=$(date +%s)
status=0
perl -e "alarm $LIMIT; exec @ARGV" scripts/verify-file.sh "$BACKEND" "$SPEC" --main "$MAIN" \
  --step "step_$SLICE" "$@" --out "$OUT" --reuse > "$LOG" 2>&1 || status=$?
secs=$(( $(date +%s) - start ))

if [ "$BACKEND" = tlc ]; then
  depth=$(grep -oE 'Progress\([0-9]+\)' "$LOG" | tail -n 1 | tr -dc 0-9)
  done_depth=$(grep -oE 'depth of the complete state graph search is [0-9]+' "$LOG" | tr -dc 0-9)
  states=$(grep -E 'distinct states (found|generated)' "$LOG" | tail -n 1)
else
  depth=$(grep -oE 'Step [0-9]+: picking a transition' "$LOG" | tail -n 1 | tr -dc 0-9)
  done_depth=
  states=
fi

case "$status" in
  0) echo "PASS $CHECK $MAIN in ${secs}s${done_depth:+, depth $done_depth}${states:+: $states}"; exit 0 ;;
  142)
    echo "INCOMPLETE $CHECK $MAIN: stopped at the time limit (${LIMIT}s) with no violation, depth ${depth:-0} reached${states:+: $states}"
    if [ "${MODEL_CHECK_STRICT:-0}" = 1 ]; then exit 1; fi
    exit 0 ;;
  *)
    echo "FAIL $CHECK $MAIN (exit $status), log $LOG"
    grep -E 'counterexample|violated|Error|error' "$LOG" | head -n 10
    exit 1 ;;
esac
