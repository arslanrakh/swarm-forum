# Shared by the scale-gate scripts of scripts/bench (design D.12.8, D.15.4; WP-F4). Sourced, not
# run.
#
# Every gate script takes a scale:
#   --full   the gate as D.12.8 specifies it, for the reference hardware (16 vCPU, 64 GB, NVMe with
#            a durable fsync of at most 1 ms); the default, as `scripts/verify.sh --bench` runs it;
#   --smoke  the same scenario scaled down, to finish in minutes on a laptop.
# BENCH_SCALE=smoke|full sets the default. Other arguments go to `swarm-sim gate` (such as
# `--agents 500` or `--stubs require`).
#
# Results go to $BENCH_DIR/results/<gate>-<scale>.json (BENCH_DIR defaults to target/bench), each
# with the machine it ran on and its measured fsync; logs and the server's counters to
# $BENCH_DIR/work, where a gate's databases are removed afterwards unless BENCH_KEEP=1.
#
# The server is built with its `bench` feature (a compressed clock that a gate can make jump, the
# writer's counters and the instruments of crates/forum-server/src/bench.rs, a shell_version bump,
# checkpoint, replay-verify, and commit-probe), which the crate accepts only with `--cfg
# forum_bench`, under the `bench-gates` profile: its binary lands in target/bench-gates, never in
# target/release. Its numbers are those of a bench build, not of a release build (D.12.8 says the
# latter): the counters cost a little writer time.

set -u

BENCH_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
cd "$BENCH_ROOT" || exit 2

BENCH_DIR=${BENCH_DIR:-$BENCH_ROOT/target/bench}
RESULTS="$BENCH_DIR/results"
WORK="$BENCH_DIR/work"
SERVER_BIN="$BENCH_ROOT/target/bench-gates/forum-server"
SIM_BIN="$BENCH_ROOT/target/release/swarm-sim"
SCALE=${BENCH_SCALE:-full}
EXTRA=()

# parse_args "$@": --smoke or --full, and the rest for swarm-sim.
parse_args() {
  for arg in "$@"; do
    case "$arg" in
      --smoke) SCALE=smoke ;;
      --full) SCALE=full ;;
      -h | --help)
        sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
        sed -n '4,13p' "$BENCH_ROOT/scripts/bench/lib/common.sh" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
      *) EXTRA+=("$arg") ;;
    esac
  done
  case "$SCALE" in
    smoke | full) ;;
    *) echo "unknown scale: $SCALE (smoke or full)" >&2; exit 2 ;;
  esac
  mkdir -p "$RESULTS" "$WORK"
}

# build: the server with the bench feature, and the driver; once per run-all.
build() {
  if [ "${BENCH_BUILT:-0}" = 1 ]; then return 0; fi
  echo "building forum-server (feature bench, profile bench-gates) and swarm-sim (release)"
  RUSTFLAGS="${RUSTFLAGS:-} --cfg forum_bench" \
    cargo build --quiet --profile bench-gates -p forum-server --features bench || return 2
  cargo build --quiet --release -p swarm-sim || return 2
  export BENCH_BUILT=1
}

# limits: room for many connections, and a word about the disk.
limits() {
  local want=65536 hard
  hard=$(ulimit -Hn)
  if [ "$hard" = unlimited ] || [ "$hard" -ge "$want" ]; then
    ulimit -n "$want" 2>/dev/null || true
  else
    ulimit -n "$hard" 2>/dev/null || true
  fi
  echo "open files allowed: $(ulimit -n)"
  local free_kb need_gb
  free_kb=$(df -k "$BENCH_ROOT" | awk 'NR == 2 { print $4 }')
  need_gb=$([ "$SCALE" = full ] && echo 200 || echo 5)
  if [ -n "$free_kb" ] && [ "$free_kb" -lt $((need_gb * 1024 * 1024)) ]; then
    echo "warning: $((free_kb / 1024 / 1024)) GB free; the $SCALE gates may need about $need_gb GB"
  fi
}

# gate <g1|g2|g4|…|g12>: runs a gate in swarm-sim and returns its exit code (0 pass, 1 fail,
# 2 error, 3 not yet).
gate() {
  local g=$1
  shift
  build || return 2
  limits
  "$SIM_BIN" gate "$g" --scale "$SCALE" --bin "$SERVER_BIN" --work "$WORK" \
    --results "$RESULTS" --keep "${BENCH_KEEP:-0}" "$@" ${EXTRA[@]+"${EXTRA[@]}"}
}

# verdict <G-n> <title> <pass|fail|not-yet> <note> [swarm-sim verdict options, such as --log
# <file>]: records a gate a script checked itself, with the machine (and the server's commit when
# the bench build of the server exists). Returns 0 pass, 1 fail, 3 not yet, 2 error.
verdict() {
  cargo build --quiet --release -p swarm-sim || return 2
  local bin=()
  [ -x "$SERVER_BIN" ] && bin=(--bin "$SERVER_BIN")
  "$SIM_BIN" verdict --gate "$1" --title "$2" --scale "$SCALE" --status "$3" --note "$4" \
    --results "$RESULTS" --work "$WORK" ${bin[@]+"${bin[@]}"} "${@:5}"
}
