# The plants part's harness (tools/tests/full.sh, PART=plants), sourced by its arm. A plant is
# a test-only fault a feature or an environment variable switches on; the part runs each plant
# alone, over a suite, after a control of that suite with no plant, and names the rows it must
# fail. Under cargo-nextest only (its report names each failed row).
#
#   control LABEL -- CMD...
#       CMD with no plant: GREEN when it exits 0 with a Summary and no failed row.
#   plant NAME 'ROWS' AT 'MSG' [VAR=VALUE] -- CMD...
#       CMD with VAR=VALUE (if given) in its environment. ROWS: the rows the plant must fail,
#       space-separated, each BINARY/TEST as nextest names it (`k2-mpc-base/engine::x::tests::y`
#       for a lib row, `k2-mpc-base::roster/y` for an integration test's). CAUGHT only when the
#       failed rows are exactly ROWS and each failed at its own assertion: the row's last
#       failure is at a location holding AT (a path: a dependency's may print in full) with a
#       message holding MSG (any message when empty). Anything else is a FINDING, printed with each failed row's location and
#       message; a plant whose ROWS are empty is always one (no row is named to catch it).
#   plants_summary
#       the part's result line.
# Each prints `=== RESULT ...` lines and returns non-zero unless GREEN or CAUGHT, so the job
# fails on a red control or a finding.
PL_T=$(mktemp -d)
PL_CAUGHT=0; PL_FINDINGS=0; PL_GREEN=0; PL_RED=0
# the line that starts a test thread's failure report, and its location
PL_FAIL_RX="thread '[^']*' ([(][0-9]+[)] )?panicked at "
if [ "${NEXTEST:-1}" = 0 ]; then
  echo "plants: cargo-nextest is required (its report names each failed row)"; RC=1
fi

# pl_failed LOG: the failed rows, BINARY/TEST, one per line
pl_failed() {
  grep -E '^ +(FAIL|SIGSEGV|SIGABRT|SIGKILL|SIGBUS|TIMEOUT|ABORT|LEAK-FAIL) \[' "$1" \
    | sed -E 's/^[^]]*\] +//; s/^\( *[0-9]+\/[0-9]+\) +//; s/ +$//; s/ /\//' | sort -u || true
}
pl_summaries() { grep -cE '^ +Summary \[' "$1" || true; }

control() {
  pl_label=$1; shift; [ "$1" = -- ] && shift
  pl_log=$PL_T/control-$pl_label.log
  "$@" > "$pl_log" 2>&1; pl_rc=$?
  pl_got=$(pl_failed "$pl_log")
  printf '%s\n' "$pl_got" | sed '/^$/d; s/^/[control '"$pl_label"'] failed: /'
  pl_sum=$(grep -E '^ +Summary \[' "$pl_log" | tail -1 | sed 's/^ *//')
  if [ "$pl_rc" = 0 ] && [ "$(pl_summaries "$pl_log")" -ge 1 ] && [ -z "$pl_got" ]; then
    PL_GREEN=$((PL_GREEN + 1)); echo "=== RESULT control $pl_label: GREEN, ${pl_sum:-no Summary}"; return 0
  fi
  tail -30 "$pl_log"
  PL_RED=$((PL_RED + 1)); echo "=== RESULT control $pl_label: RED (exit $pl_rc), ${pl_sum:-no Summary}"; return 1
}

plant() {
  pl_name=$1; pl_rows=$2; pl_at=$3; pl_msg=$4; shift 4
  pl_env=""
  if [ "$1" != -- ]; then pl_env=$1; shift; fi
  [ "$1" = -- ] && shift
  pl_log=$PL_T/plant-$pl_name.log
  echo "[$pl_name] ${pl_env:-(no variable)}: $*"
  # a subshell exports it: CMD may be a shell function (full.sh's `tests`), which `env` can't run
  if [ -n "$pl_env" ]; then (export "$pl_env"; "$@") > "$pl_log" 2>&1; else "$@" > "$pl_log" 2>&1; fi
  pl_rc=$?
  printf '%s\n' $pl_rows | sed '/^$/d' | sort -u > "$PL_T/want"
  pl_failed "$pl_log" > "$PL_T/got"
  pl_bad=""
  while IFS= read -r pl_row; do
    [ -n "$pl_row" ] || continue
    pl_th=${pl_row#*/}
    pl_loc=$(grep -E "$PL_FAIL_RX" "$pl_log" | grep -F "thread '$pl_th' " | tail -1 | sed -E "s/.*$PL_FAIL_RX//; s/:\$//")
    pl_m=$(awk -v t="thread '$pl_th' " -v rx="$PL_FAIL_RX" 'index($0, t) && $0 ~ rx { getline m; last = m } END { print last }' "$pl_log" | sed 's/^ *//')
    pl_ok=yes
    case $pl_loc in *"$pl_at"*) ;; *) pl_ok=no ;; esac
    if [ -n "$pl_msg" ]; then case $pl_m in *"$pl_msg"*) ;; *) pl_ok=no ;; esac; fi
    [ "$pl_ok" = yes ] || pl_bad="$pl_bad $pl_row"
    echo "[$pl_name] failed: $pl_row | at ${pl_loc:-(no location)} | ${pl_m:-(no message)} | its own assertion: $pl_ok" | cut -c1-400
  done < "$PL_T/got"
  pl_extra=$(comm -13 "$PL_T/want" "$PL_T/got" | tr '\n' ' ' | sed 's/ $//')
  pl_missing=$(comm -23 "$PL_T/want" "$PL_T/got" | tr '\n' ' ' | sed 's/ $//')
  pl_sum=$(grep -E '^ +Summary \[' "$pl_log" | tail -1 | sed 's/^ *//')
  pl_n=$(wc -l < "$PL_T/want" | tr -d ' '); pl_f=$(wc -l < "$PL_T/got" | tr -d ' ')
  if [ "$pl_n" -gt 0 ] && [ -z "$pl_extra$pl_missing$pl_bad" ] && [ "$(pl_summaries "$pl_log")" -ge 1 ]; then
    PL_CAUGHT=$((PL_CAUGHT + 1))
    echo "=== RESULT plant $pl_name: CAUGHT; named $pl_n, failed $pl_f; ${pl_sum:-no Summary}"; return 0
  fi
  [ "$(pl_summaries "$pl_log")" -ge 1 ] || tail -30 "$pl_log"
  PL_FINDINGS=$((PL_FINDINGS + 1))
  echo "=== RESULT plant $pl_name: FINDING (extra: ${pl_extra:-none}; missing: ${pl_missing:-none}; not at its own assertion:${pl_bad:- none}); named $pl_n, failed $pl_f, exit $pl_rc; ${pl_sum:-no Summary}"
  return 1
}

plants_summary() {
  if [ "$PL_RED" = 0 ] && [ "$PL_FINDINGS" = 0 ]; then pl_v=GREEN; else pl_v=RED; fi
  echo "=== RESULT plants: $pl_v; controls $PL_GREEN green, $PL_RED red; plants $PL_CAUGHT caught, $PL_FINDINGS findings"
  rm -rf "$PL_T"
}
