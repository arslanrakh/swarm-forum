#!/usr/bin/env bash
# Runs every check of design D.12.9 and prints, for each, whether it passed, failed, or was
# skipped, and why. Exits non-zero if any check that ran failed.
#
# Usage: scripts/verify.sh [--no-model] [--no-kani] [--bench] [--usability] [-- <run.sh arguments>]
#   --no-model   skips the Quint model's checks and the conformance sweep (steps 6 and 7), for a
#                change that leaves model/ untouched
#   --no-kani    skips the Kani harnesses (step 5), for an intermediate package; the release's run
#                keeps them
#   --bench      also runs the scale gates of D.12.8, once each, through scripts/bench/run-all.sh
#                (step 8); BENCH_SCALE=smoke runs them at the laptop scale (default: full, the
#                reference hardware's)
#   --usability  also runs the live-agent usability test of WP-U through usability/run.sh (step 9),
#                which spends model tokens and needs the vendors' CLIs and logins; the arguments
#                after -- go to run.sh (default: campaign)
#
# Environment: MODEL_CHECK_SECONDS, the time limit of each TLC run (model/scripts/check.sh,
# default 1800); VERIFY_APALACHE=1 also runs Apalache on the key slices, which today does not
# reach its first step (docs/verification.md); RUSTFLAGS is extended, not replaced, for the
# bench-gates build.

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT" || exit 2

BENCH=0
MODEL=1
KANI=1
USABILITY=0
USABILITY_ARGS=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --bench) BENCH=1 ;;
    --no-model) MODEL=0 ;;
    --no-kani) KANI=0 ;;
    --usability) USABILITY=1 ;;
    --) shift; USABILITY_ARGS=("$@"); break ;;
    -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
    *) echo "verify.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ "${#USABILITY_ARGS[@]}" -gt 0 ] && [ "$USABILITY" -ne 1 ]; then
  echo "verify.sh: arguments after -- are for --usability" >&2
  exit 2
fi
[ "${#USABILITY_ARGS[@]}" -gt 0 ] || USABILITY_ARGS=(campaign)

# The model's own Quint is on PATH for the model checks and the conformance tests (D.10.8).
export PATH="$ROOT/model/node_modules/.bin:$PATH"

# D.0.3: the model checks of the key invariants run on these slices (the holds, keys, and time
# parts; the time checks are in every slice).
KEY_SLICES="holds keys"

PASSED=()
FAILED=()
SKIPPED=()

# Under GitHub Actions each check's output is a collapsible group, and a failure an annotation.
GHA=${GITHUB_ACTIONS:-false}
IN_GROUP=0
section() {
  if [ "$GHA" = true ]; then
    if [ "$IN_GROUP" -eq 1 ]; then echo "::endgroup::"; fi
    echo "::group::$1"
    IN_GROUP=1
  else
    printf '\n==> %s\n' "$1"
  fi
}
failed() {
  FAILED+=("$1")
  if [ "$GHA" = true ]; then echo "::error title=verify.sh::$1"; fi
}

# run NAME COMMAND...: runs one check and records whether it passed, and how long it took.
run() {
  local name=$1 start status
  shift
  section "$name"
  start=$SECONDS
  "$@"
  status=$?
  if [ "$status" -eq 0 ]; then
    PASSED+=("$name ($((SECONDS - start)) s)")
  else
    failed "$name (exit $status, $((SECONDS - start)) s)"
  fi
}

# skip NAME REASON: records a check that did not run, and why.
skip() {
  section "$1"
  echo "skipped: $2"
  SKIPPED+=("$1: $2")
}

# --- Guard rails that lints cannot express (D.1.3) -------------------------------------------------

CORE_ALLOWED_DEPS="forum-core forum-cron serde serde_core serde_derive proc-macro2 quote syn unicode-ident"

# forum-core, with serde, depends on nothing outside the allow-list.
guard_dependencies() {
  local tree names name bad=""
  tree=$(cargo tree -p forum-core -e normal --prefix none --features serde) || return 1
  names=$(printf '%s\n' "$tree" | awk 'NF { print $1 }' | sort -u)
  echo "forum-core (feature serde) depends on:" $names
  for name in $names; do
    case " $CORE_ALLOWED_DEPS " in
      *" $name "*) ;;
      *) bad="$bad $name" ;;
    esac
  done
  if [ -n "$bad" ]; then
    echo "not in the allow-list:$bad"
    return 1
  fi
}

# The verified crates never name a floating-point type.
guard_floats() {
  local status
  if command -v rg >/dev/null 2>&1; then
    rg -n '\bf(32|64)\b' crates/forum-core/src crates/forum-cron/src
  else
    grep -rnwE 'f(32|64)' crates/forum-core/src crates/forum-cron/src
  fi
  status=$?
  case $status in
    0) echo "floating-point types found (above)"; return 1 ;;
    1) echo "none found" ;;
    *) echo "the search failed"; return 1 ;;
  esac
}

# A copy of forum-core with a test module that uses HashMap must fail clippy: proof that the
# lints and clippy.toml catch it.
guard_hashmap_probe() {
  local probe out status f
  probe=$(mktemp -d "${TMPDIR:-/tmp}/forum-hashmap-probe.XXXXXX") || return 1
  for f in Cargo.toml Cargo.lock clippy.toml rust-toolchain.toml; do
    if [ -f "$f" ]; then cp "$f" "$probe/"; fi
  done
  cp -R crates "$probe/crates" || { rm -rf "$probe"; return 1; }
  cat >>"$probe/crates/forum-core/src/lib.rs" <<'EOF'

#[cfg(test)]
mod hashmap_probe {
    #[test]
    fn uses_a_hash_map() {
        let mut map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
        map.insert(1, 2);
        assert_eq!(map.len(), 1);
    }
}
EOF
  out=$(cd "$probe/crates/forum-core" &&
    CARGO_TARGET_DIR="$ROOT/target/hashmap-probe" cargo clippy --all-targets -- -D warnings 2>&1)
  status=$?
  rm -rf "$probe"
  if [ "$status" -eq 0 ]; then
    echo "clippy passed a test module that uses HashMap: the lints do not catch it"
    return 1
  fi
  if printf '%s\n' "$out" | grep -q 'disallowed type `std::collections::HashMap`'; then
    echo "clippy rejected the probe's HashMap, as it should"
    return 0
  fi
  printf '%s\n' "$out" | tail -n 30
  echo "clippy failed on the probe, but not because of HashMap"
  return 1
}

# --- The feature builds of forum-server (WP-U, WP-F4) ------------------------------------------------

# The scale gates' build: the bench feature, only under --cfg forum_bench and the bench-gates
# profile, whose binaries land in target/bench-gates (D.12.8).
bench_gates() { RUSTFLAGS="${RUSTFLAGS:-} --cfg forum_bench" "$@"; }

# The usability harness's own tests; their end-to-end ones run the usability build of the server
# from target/debug, so it is built first, as usability/run.sh does.
harness_tests() {
  cargo build -p forum-server --features usability --bin forum-server || return 1
  python3 -m unittest discover -s usability/tests
}

# --- Quint model (D.10.1, D.10.7) and the conformance sweep (D.10.8) ---------------------------------

model_script() { (cd "$ROOT/model" && npm run --silent "$1"); }

# model_check CHECK SLICE MODULE: one TLC, liveness, or Apalache run of model/scripts/check.sh. A
# run its time limit stopped passes there with an INCOMPLETE line; it is reported as bounded here.
model_check() {
  local name="model: $1 $3" out start status line
  section "$name"
  out=$(mktemp "${TMPDIR:-/tmp}/forum-model-check.XXXXXX") || return 1
  start=$SECONDS
  (cd "$ROOT/model" && scripts/check.sh "$1" "$2" "$3") 2>&1 | tee "$out"
  status=${PIPESTATUS[0]}
  line=$(grep -E '^INCOMPLETE ' "$out" | tail -n 1)
  rm -f "$out"
  if [ "$status" -ne 0 ]; then
    failed "$name (exit $status, $((SECONDS - start)) s)"
  elif [ -n "$line" ]; then
    PASSED+=("$name, bounded: ${line#INCOMPLETE } ($((SECONDS - start)) s)")
  else
    PASSED+=("$name ($((SECONDS - start)) s)")
  fi
}

model_checks() {
  local scripts s slice module
  if [ -z "$(find model -name '*.qnt' -not -path '*/node_modules/*' 2>/dev/null | head -n 1)" ]; then
    skip "Quint model checks" "no model files"
    return
  fi
  if [ "$HAVE_QUINT" -ne 1 ]; then
    skip "Quint model checks" "npx quint is missing (run npm ci in model/)"
    return
  fi
  scripts=$(cd model && node -e 'console.log(Object.keys(require("./package.json").scripts || {}).join("\n"))')
  for s in typecheck test simulate traces; do
    if printf '%s\n' "$scripts" | grep -qx "$s"; then
      run "model: npm run $s" model_script "$s"
    else
      skip "model: npm run $s" "no such script"
    fi
  done
  # D.0.3: TLC on the key slices, in both class configurations, and the liveness of holds; the
  # other slices' runs are not part of the verification scope.
  for slice in $KEY_SLICES; do
    for module in $(printf '%s\n' "$scripts" | sed -n "s/^tlc:\(${slice}_[a-z]*\)$/\1/p"); do
      if [ "$HAVE_JAVA" -ne 1 ]; then
        skip "model: tlc $module" "java is missing"
      else
        model_check tlc "$slice" "$module"
      fi
    done
    for module in $(printf '%s\n' "$scripts" | sed -n "s/^liveness:\(${slice}_[a-z]*\)$/\1/p"); do
      if [ "$HAVE_JAVA" -ne 1 ]; then
        skip "model: liveness $module" "java is missing"
      else
        model_check liveness "$slice" "$module"
      fi
    done
    for module in $(printf '%s\n' "$scripts" | sed -n "s/^verify:\(${slice}_[a-z]*\)$/\1/p"); do
      if [ "${VERIFY_APALACHE:-0}" != 1 ]; then
        skip "model: apalache $module" \
          "Apalache does not reach its first step on the slices (docs/verification.md); VERIFY_APALACHE=1 runs it"
      elif [ "$HAVE_JAVA" -ne 1 ]; then
        skip "model: apalache $module" "java is missing"
      else
        model_check apalache "$slice" "$module"
      fi
    done
  done
}

# The sweep of D.10.8: hundreds of model traces of the key slices, generated with fixed seeds,
# each group replayed against the core. A trace that stops at a command of a package not built yet
# is a gap, not a divergence.
conformance_sweep() {
  local group log status=0
  crates/forum-conformance/scripts/traces.sh --sweep || return 1
  log=$(mktemp "${TMPDIR:-/tmp}/forum-sweep.XXXXXX") || return 1
  for group in model/traces/conformance/*/; do
    [ -d "$group" ] || continue
    echo "-- replaying ${group%/}"
    if ! FORUM_CONFORMANCE_TRACES="$ROOT/${group%/}" \
      cargo test -p forum-conformance --test sweep -- --ignored --nocapture >"$log" 2>&1; then
      status=1
      tail -n 40 "$log"
    fi
    grep -E '^[0-9]+ traces, ' "$log"
  done
  rm -f "$log"
  return "$status"
}

# --- Tools -------------------------------------------------------------------------------------------

section "tools"
echo "rustc: $(rustc --version 2>&1 | head -n 1)"
if cargo kani --version >/dev/null 2>&1; then
  HAVE_KANI=1
  echo "cargo kani: $(cargo kani --version 2>/dev/null | head -n 1)"
else
  HAVE_KANI=0
  echo "cargo kani: missing"
fi
if java -version >/dev/null 2>&1; then
  HAVE_JAVA=1
  echo "java: $(java -version 2>&1 | head -n 1)"
else
  HAVE_JAVA=0
  echo "java: missing"
fi
if [ -d model ] && (cd model && npx --no -- quint --version) >/dev/null 2>&1; then
  HAVE_QUINT=1
  echo "npx quint: $(cd model && npx --no -- quint --version 2>/dev/null | tail -n 1)"
else
  HAVE_QUINT=0
  echo "npx quint: missing"
fi
if python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)' >/dev/null 2>&1; then
  HAVE_PYTHON=1
  echo "python3: $(python3 --version 2>&1)"
else
  HAVE_PYTHON=0
  echo "python3 3.10 or later: missing"
fi

# --- The checks of D.12.9, in order -------------------------------------------------------------------

# 1
run "cargo fmt --check" cargo fmt --check

# 2: clippy on every build that exists: the default one, the usability test's, and the scale
# gates' (each feature compiles code the others do not)
run "cargo clippy --workspace --all-targets -- -D warnings" \
  cargo clippy --workspace --all-targets -- -D warnings
run "cargo clippy -p forum-server --features usability --all-targets -- -D warnings" \
  cargo clippy -p forum-server --features usability --all-targets -- -D warnings
run "cargo clippy --profile bench-gates -p forum-server --features bench --all-targets -- -D warnings (cfg forum_bench)" \
  bench_gates cargo clippy --profile bench-gates -p forum-server --features bench --all-targets -- -D warnings

# 3
run "guard rail: forum-core dependency allow-list" guard_dependencies
run "guard rail: no floating-point types in forum-core and forum-cron" guard_floats
run "guard rail: the HashMap probe fails clippy" guard_hashmap_probe

# 4: unit, scenario, property, cost, registry, conformance (the committed fixtures, with
# model/node_modules/.bin on PATH: step 7), and shell tests; the feature builds' own tests; the
# generated tables of docs/verification.md; and the usability harness's own tests, which spend no
# model tokens
run "cargo test --workspace" cargo test --workspace
run "cargo test -p forum-server --features usability" cargo test -p forum-server --features usability
run "cargo test --profile bench-gates -p forum-server --features bench --lib --bins (cfg forum_bench)" \
  bench_gates cargo test --profile bench-gates -p forum-server --features bench --lib --bins
if [ "$HAVE_PYTHON" -ne 1 ]; then
  skip "docs/verification.md: the generated tables are current" "python3 3.10 or later is missing"
  skip "usability harness: python3 -m unittest discover -s usability/tests" "python3 3.10 or later is missing"
else
  run "docs/verification.md: the generated tables are current" \
    python3 scripts/gen-verification-table.py --check
  run "usability harness: python3 -m unittest discover -s usability/tests" harness_tests
fi

# 5
for crate in forum-cron forum-core; do
  if [ "$HAVE_KANI" -ne 1 ]; then
    skip "cargo kani -p $crate" "cargo kani is missing"
  elif [ "$KANI" -eq 0 ]; then
    skip "cargo kani -p $crate" "--no-kani"
  elif ! grep -rqs 'kani::proof' "crates/$crate/src"; then
    skip "cargo kani -p $crate" "no harnesses"
  else
    run "cargo kani -p $crate" cargo kani -p "$crate"
  fi
done

# 6
if [ "$MODEL" -eq 1 ]; then
  model_checks
else
  skip "Quint model checks" "--no-model"
fi

# 7
section "conformance tests"
echo "the committed fixtures ran in step 4 (cargo test --workspace) with model/node_modules/.bin on PATH"
if [ "$MODEL" -ne 1 ]; then
  skip "conformance sweep" "--no-model"
elif [ "$HAVE_QUINT" -ne 1 ]; then
  skip "conformance sweep" "npx quint is missing (run npm ci in model/)"
else
  run "conformance sweep (traces.sh --sweep, then each group replayed)" conformance_sweep
fi

# 8
if [ "$BENCH" -ne 1 ]; then
  skip "scale gates" "run with --bench"
elif [ ! -f scripts/bench/run-all.sh ]; then
  skip "scale gates" "scripts/bench/run-all.sh is missing"
else
  run "scale gates: scripts/bench/run-all.sh (${BENCH_SCALE:-full})" bash scripts/bench/run-all.sh
fi

# 9
if [ "$USABILITY" -ne 1 ]; then
  skip "usability test with live agents" "run with --usability (it spends model tokens)"
elif [ "$HAVE_PYTHON" -ne 1 ]; then
  skip "usability test with live agents" "python3 3.10 or later is missing"
else
  run "usability test with live agents: usability/run.sh ${USABILITY_ARGS[*]}" \
    usability/run.sh "${USABILITY_ARGS[@]}"
fi

# --- Summary -----------------------------------------------------------------------------------------

summary() {
  local c
  for c in ${PASSED[@]+"${PASSED[@]}"}; do echo "passed:  $c"; done
  for c in ${FAILED[@]+"${FAILED[@]}"}; do echo "FAILED:  $c"; done
  for c in ${SKIPPED[@]+"${SKIPPED[@]}"}; do echo "skipped: $c"; done
  echo "${#PASSED[@]} passed, ${#FAILED[@]} failed, ${#SKIPPED[@]} skipped"
}

if [ "$GHA" = true ] && [ "$IN_GROUP" -eq 1 ]; then echo "::endgroup::"; fi
if [ "$GHA" = true ] && [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  { echo '### scripts/verify.sh'; echo; echo '```'; summary; echo '```'; } >>"$GITHUB_STEP_SUMMARY"
fi
printf '\n==> %s\n' "summary"
summary
[ "${#FAILED[@]}" -eq 0 ]
