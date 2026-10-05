#!/bin/sh
# Runs Apalache or TLC on a Quint module through files, for models too large for `quint verify`.
#
# Quint 0.33.0 hands the compiled model to the Apalache server as one JSON string, which the server
# refuses beyond 20 MB, and `quint compile --target json` cannot write this model at all (its output
# passes Node's largest string); `quint verify --backend=tlc` takes the same route to get its TLA+.
# This script compiles the module to a file with `compile-pruned.js`, which keeps only what Apalache
# reads, and runs the tools on the file (model/README.md, "Apalache and TLC on a model this size").
#
# Usage, from model/:
#   scripts/verify-file.sh apalache|tlc <spec.qnt> [--main <module>] [--init <action>]
#       [--step <action>] (--invariant <names> | --temporal <names>) [--length <n>] [--out <dir>]
#       [--reuse] [--compile-only]
#
#   --main       the module to check (default: the file name without .qnt)
#   --init       the init action (default: init); for TLC, its assignments must be written in it,
#                not in an action it calls, because this script turns them into equalities
#   --step       the step action (default: step)
#   --invariant  comma-separated invariant names, definitions of the main module (a slice's
#                `inv_<x>`) or of a module it imports
#   --temporal   comma-separated temporal property names (TLC only; Apalache has no fairness, §28)
#   --length     Apalache's depth (default: 12, D.10.7)
#   --out        where the files go (default: traces/<module>; traces/ is git-ignored)
#   --reuse      use the compiled module (and, for TLC, its TLA+) that an earlier run with the same
#                --out, --main, --init, and --step left, instead of compiling again
#   --compile-only  compile (and, for TLC, write the TLA+), then stop
#
# The compiled file holds `q::init`, `q::step`, and every definition of the main module with what
# it reads, so one compilation serves every invariant and property of a slice: Apalache and TLC are
# given the names themselves.
#
# Exit status: 0 when the property holds within the bounds, non-zero on a violation or an error,
# and 128 + the signal's number when a signal stopped it (142 for the alarm of README's "Long runs").
#
# Signals. Every long step (the compiler, Apalache, TLC) runs in the background while this script
# waits for it, and a SIGALRM, SIGTERM, SIGINT, or SIGHUP sent to this script is passed on to it as
# SIGTERM, so that no JVM outlives a run stopped by a time limit.

set -eu

usage() { sed -n '2,36p' "$0"; exit 2; }

# The running step, and what a stopping signal does with it.
pid=
stop() {
  if [ -n "$pid" ]; then
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  if [ -n "${STATES:-}" ]; then rm -rf "$STATES" "$CFG"; fi
  echo "verify-file.sh: stopped by $1" >&2
  exit "$2"
}
trap 'stop SIGALRM 142' ALRM
trap 'stop SIGTERM 143' TERM
trap 'stop SIGINT 130' INT
trap 'stop SIGHUP 129' HUP

# Runs "$@" in the background and waits for it; returns its exit status.
run() {
  "$@" &
  pid=$!
  st=0
  wait "$pid" || st=$?
  pid=
  return "$st"
}

[ $# -ge 2 ] || usage
BACKEND=$1
SPEC=$2
shift 2

MAIN=$(basename "$SPEC" .qnt)
INIT=init
STEP=step
INV=
TEMPORAL=
LENGTH=12
OUT=
REUSE=
ONLY=

while [ $# -gt 0 ]; do
  case "$1" in
    --main) MAIN=$2; shift 2 ;;
    --init) INIT=$2; shift 2 ;;
    --step) STEP=$2; shift 2 ;;
    --invariant) INV=$2; shift 2 ;;
    --temporal) TEMPORAL=$2; shift 2 ;;
    --length) LENGTH=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --reuse) REUSE=1; shift ;;
    --compile-only) ONLY=1; shift ;;
    *) echo "verify-file.sh: unknown argument: $1" >&2; usage ;;
  esac
done

case "$BACKEND" in apalache | tlc) ;; *) usage ;; esac
if [ -z "$INV" ] && [ -z "$TEMPORAL" ]; then
  echo "verify-file.sh: give --invariant or --temporal" >&2
  exit 2
fi
if [ "$BACKEND" = apalache ] && [ -n "$TEMPORAL" ]; then
  echo "verify-file.sh: temporal properties go to TLC (§28)" >&2
  exit 2
fi

MODEL=$(cd "$(dirname "$0")/.." && pwd)
APALACHE_HOME=${APALACHE_HOME:-$HOME/.quint/apalache-dist-0.62.1/apalache}
[ -x "$APALACHE_HOME/bin/apalache-mc" ] || {
  echo "verify-file.sh: no Apalache at $APALACHE_HOME (run any 'npx quint verify' once to download it)" >&2
  exit 2
}
OUT=${OUT:-traces/$MAIN}
JVM_ARGS=${JVM_ARGS:--Xmx8g}   # apalache-mc reads it
export JVM_ARGS
mkdir -p "$OUT"
JSON="$OUT/$MAIN.qnt.json"
STAMP="$OUT/$MAIN.compiled"    # init and step of the compiled file

# 1. Compile, with q::init and q::step added by Quint. `quint compile --target json` cannot write
# this model (its output passes Node's largest string), so `compile-pruned.js` runs Quint's own
# stages and writes only what Apalache reads (the script's header).
if [ -n "$REUSE" ] && [ -f "$JSON" ] && [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$INIT $STEP" ]; then
  echo "reusing $JSON"
else
  rm -f "$STAMP" "$OUT/$MAIN.tla"
  echo "compiling $SPEC ($MAIN) to $JSON"
  set -- --main "$MAIN" --init "$INIT" --step "$STEP"
  if [ -n "$INV" ]; then set -- "$@" --invariant "$INV"; fi
  if [ -n "$TEMPORAL" ]; then set -- "$@" --temporal "$TEMPORAL"; fi
  NODE_OPTIONS=${NODE_OPTIONS:---max-old-space-size=14000} run node "$MODEL/scripts/compile-pruned.js" "$SPEC" \
    "$@" --out "$JSON"
  echo "$INIT $STEP" > "$STAMP"
fi

if [ "$BACKEND" = apalache ] && [ -n "$ONLY" ]; then exit 0; fi
if [ "$BACKEND" = apalache ]; then
  # 2a. Apalache reads Quint's JSON IR from the file (the .qnt.json suffix names the format).
  echo "apalache: --inv=$INV --length=$LENGTH, output in $OUT/apalache"
  status=0
  run "$APALACHE_HOME/bin/apalache-mc" check --out-dir="$OUT/apalache" \
    --init=q::init --next=q::step --inv="$INV" --length="$LENGTH" --no-deadlock "$JSON" || status=$?
  cex=$(find "$OUT/apalache" -name 'violation1.itf.json' 2>/dev/null | sort | tail -n 1)
  if [ -n "$cex" ]; then echo "counterexample: $cex"; fi
  exit $status
fi

# 2b. TLC: Apalache writes the TLA+, init becomes a predicate (what `quint verify --backend=tlc`
# does before it asks for TLA+), and TLC runs from the Apalache JAR, without deadlock checking
# (D.10.7: a run that reaches the bounds stutters there).
TLA="$OUT/$MAIN.tla"
CFG="$OUT/$MAIN.$$.cfg"   # one per run: two runs on one module may overlap (check.sh)
if [ -n "$REUSE" ] && [ -f "$TLA" ]; then
  echo "reusing $TLA"
else
  run "$APALACHE_HOME/bin/apalache-mc" parse --out-dir="$OUT/apalache" \
    --output="$TLA" "$JSON" > "$OUT/parse.log" 2>&1 || { cat "$OUT/parse.log"; exit 1; }
  # The init action is the operator that `q_init` names: in a module that instantiates another one
  # (`import forum(…).*`, the slices and instances of D.10.1), Apalache prefixes its name.
  INIT="$INIT" perl -0pi -e '
    my $name = /\nq_init == (\w+)\n/ ? $1 : $ENV{INIT};
    my $init = quotemeta($name);
    my $n = s{(\n$init ==.*?)(?=\n\n)}{ my $b = $1; $b =~ s/(\w+)\x27 :=/$1 =/g; $b }se;
    die "verify-file.sh: no definition of $name in the TLA+\n" unless $n;
  ' "$TLA"
fi
if [ -n "$ONLY" ]; then exit 0; fi
# A definition of the main module keeps its name in the TLA+; one of an instantiated module gets
# the instance's prefix (`smoke_tlc_settle_smoke_inv_tlc`), which this finds when it is unique.
# A name the compiled module does not hold (an imported definition that an earlier compilation
# with --reuse did not keep) stops the run: compile again without --reuse.
tla_name() {
  if grep -q "^$1 ==" "$TLA"; then echo "$1"; return; fi
  found=$(grep -oE "^[A-Za-z0-9_]+_$1 ==" "$TLA" | sed 's/ ==$//')
  if [ -n "$found" ] && [ "$(echo "$found" | grep -c .)" -eq 1 ]; then echo "$found"; fi
}
{
  echo "INIT q_init"
  echo "NEXT q_step"
  for i in $(echo "$INV" | tr ',' ' '); do echo "INVARIANT $(tla_name "$i")"; done
  for p in $(echo "$TEMPORAL" | tr ',' ' '); do echo "PROPERTY $(tla_name "$p")"; done
} > "$CFG"
if grep -qE '^(INVARIANT|PROPERTY) *$' "$CFG"; then
  echo "verify-file.sh: a name of --invariant or --temporal has no single definition in $TLA" >&2
  exit 2
fi
echo "tlc: $TLA with $CFG"
STATES="$OUT/tlc-states-$$"
status=0
run java ${TLC_JVM_ARGS:--Xmx8g -Xss16m -XX:+UseParallelGC} -cp "$APALACHE_HOME/lib/apalache.jar" tlc2.TLC -deadlock \
  -workers ${TLC_WORKERS:-auto} -metadir "$STATES" -config "$CFG" "$TLA" || status=$?
rm -rf "$STATES" "$CFG"
exit $status
