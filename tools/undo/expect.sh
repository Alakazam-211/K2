# Row verdicts for the undo model check's jobs (sourced by tools/undo/job.sh; POSIX sh, sed,
# awk). A job's rows are its runs: each TLC run (`tlc`, tools/undo/tlc.sh), each enumerator run
# (`E`) and each ITF replay (`R`). A commands file declares, before it runs anything, every row
# it must end with and that row's verdict:
#
#   expect KEY VERDICT [RULES]   VERDICT: COMPLETE (TLC, `bfs`, `dfs`, a replay: the whole
#                                bound, no violation, no mismatch), SAMPLED (walks: every walk
#                                asked for ran, no violation, no failed oracle assertion) or VIOLATION at one of
#                                RULES (`R1|R2`: either). A TLC row's rule is the invariant
#                                TLC names (`..._inv_RULE is violated`); an enumerator row's,
#                                one of the stop rules in its SUMMARY's violations=[...].
#   expect_complete KEY...       each KEY COMPLETE
#   expect_mutants tlc|enum|walk [-MUTANT...]
#                                every mutant of UNDO_MUTANT_RULES below but those named with
#                                `-`, VIOLATION at its rule (`walk`: enumerator walks, where
#                                mutant (iv) may end first at the oracle's own shared-edge
#                                assertion, the enumerator's pseudo-rule for a failed one)
#
# A row's KEY: `TOOL@MUTANT` for a run with a mutant (TOOL tlc or enum; the mutant read from the
# instance's MUTANT, or from the enumerator's header line), `TOOL@MUTANT/RULE` when the run
# checks that one rule only (`tlc NAME "inv_RULE"`, `--stop RULE`), else `tlc:INSTANCE`, `enum:RUN` or
# `itf:RUN` (RUN: the run's name in its RUNREC line). Every row is checked as it ends, and the
# job fails (expect_done, at the end and at any exit) when a declared row didn't run or ran
# twice, a row ended with another verdict or rule, a row isn't declared, a tool printed no
# RUNREC (a usage error, a tool that died), a TLC compile failed, or a `timed` step that ran no row
# exited non-zero. A commands file that declares nothing (an ad hoc job) gets every check but
# the verdicts: a row whose status is ERROR, NO_RESULT or UNKNOWN still fails it.
# Sourced once a job (tools/undo/job.sh does): sourced again, it would empty expect-rows/ and
# lose every row declared and checked so far. A second source is refused as one of the job's
# problems (the job ends RED, exit 1), and the rows are kept.
if [ -n "${_EXPECT_SOURCED:-}" ]; then
  _expect_fail "tools/undo/expect.sh sourced a second time in this job (tools/undo/job.sh sources it): refused, the declared rows kept; a commands file must not source it"
  return 0
fi
_EXPECT_SOURCED=1
EXPECT_DIR=$ROOT/expect-rows
rm -rf "$EXPECT_DIR"; mkdir -p "$EXPECT_DIR"
: > "$EXPECT_DIR/declared"; : > "$EXPECT_DIR/seen"; : > "$EXPECT_DIR/fail"; : > "$EXPECT_DIR/rows"; : > "$EXPECT_DIR/steps"
# Each mutant of the model check (crates/spec-undo/src/model/mutant.rs, Mutant::ALL; MUTANT in
# spec/quint/undo.qnt) and the rule it must violate (docs/MODEL-CHECK.md; tests/mutants.rs). A
# mutant row not listed here fails the job as undeclared: add the new mutant with its rule.
UNDO_MUTANT_RULES="i-no-taint:U9 iii-partial-hides-taint:U15 iv-redo-retries-skipped:U17 vi-o5-every-window-end:O5 vii-o5-region-base:O5 skip-u19:U19 f3-rediff:own_only restart-single-taint:RT x-provenance-ignored-in-o4:I1 xi-no-o4a:I1 u13-refused-revert-applies:U13 r3-redo-rewrites-window:own_only r3-revert-rewrites-window:own_only i1-per-request:I1"

# settings NAME[=DEFAULT]...: one `=== settings:` line with each variable's value in this job, its
# default when unset (a job's record shows env names, not values: the log must)
settings() { _st=""; for _sk in "$@"; do _sn=${_sk%%=*}; _sd=""; case $_sk in *=*) _sd=${_sk#*=} ;; esac; eval "_sv=\${$_sn:-}"; _st="$_st $_sn=${_sv:-${_sd:-(unset)}}"; done; echo "=== settings:$_st"; }
_expect_fail() { printf '%s\n' "$*" >> "$EXPECT_DIR/fail"; echo "=== EXPECT FAILED: $*"; }

expect() {
  _xk=${1:?expect KEY VERDICT [RULES]}; _xv=${2:?expect KEY VERDICT [RULES]}; _xr=${3:--}
  case $_xv in
    COMPLETE|SAMPLED) [ "$_xr" = - ] || { _expect_fail "expect $_xk $_xv takes no rule ($_xr)"; return 0; } ;;
    VIOLATION) [ "$_xr" != - ] || { _expect_fail "expect $_xk VIOLATION needs its rule"; return 0; } ;;
    *) _expect_fail "expect $_xk: verdict $_xv (COMPLETE, SAMPLED or VIOLATION)"; return 0 ;;
  esac
  _xd=$(awk -v k="$_xk" '$1 == k' "$EXPECT_DIR/declared")
  if [ -n "$_xd" ]; then
    # the same declaration twice (a sourced file declares its own rows too) is one
    [ "$_xd" = "$_xk $_xv $_xr" ] || _expect_fail "row $_xk declared twice: '$_xd', then '$_xk $_xv $_xr'"
    return 0
  fi
  echo "$_xk $_xv $_xr" >> "$EXPECT_DIR/declared"
}
expect_complete() { for _xc in "$@"; do expect "$_xc" COMPLETE; done; }
# expect_mutants VARIANT [-MUTANT...]: VARIANT names the rows declared (tlc, enum or walk); it
# prints its own `=== expect: variant=` line, and an unknown VARIANT, or an argument that isn't
# `-` and a mutant of UNDO_MUTANT_RULES, is refused (a problem of the job) and declares nothing
expect_mutants() {
  _xvar=${1:-}
  case $_xvar in
    tlc|enum|walk) ;;
    *) _expect_fail "expect_mutants: unknown variant '$_xvar' (tlc, enum or walk): refused, no mutant row declared"; return 0 ;;
  esac
  shift
  for _xa in "$@"; do
    case " $UNDO_MUTANT_RULES " in *" ${_xa#-}:"*) case $_xa in -*) continue ;; esac ;; esac
    _expect_fail "expect_mutants $_xvar: '$_xa' isn't -MUTANT for a mutant of UNDO_MUTANT_RULES: refused, no mutant row declared"; return 0
  done
  echo "=== expect: variant=$_xvar (expect_mutants${*:+, without $*})"
  for _xp in $UNDO_MUTANT_RULES; do
    _xm=${_xp%%:*}; _xmr=${_xp#*:}; _xskip=""
    for _xa in "$@"; do if [ "$_xa" = "-$_xm" ]; then _xskip=1; fi; done
    [ -z "$_xskip" ] || continue
    case $_xvar in
      tlc) expect "tlc@$_xm" VIOLATION "$_xmr" ;;
      enum) expect "enum@$_xm" VIOLATION "$_xmr" ;;
      walk) if [ "$_xm" = iv-redo-retries-skipped ]; then _xmr="$_xmr|panic"; fi; expect "enum@$_xm" VIOLATION "$_xmr" ;;
    esac
  done
}

# a string / number field of a RUNREC line
_xs() { printf '%s\n' "$2" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"; }
_xn() { printf '%s\n' "$2" | sed -n "s/.*\"$1\":\([0-9][0-9]*\).*/\1/p"; }

# _expect_check KEY RUNREC RULES_SEEN: the row against its declaration
_expect_check() {
  _xk=$1; _xrec=$2; _xseen=$3
  echo "$_xk" >> "$EXPECT_DIR/seen"
  _xst=$(_xs status "$_xrec")
  _xd=$(awk -v k="$_xk" '$1 == k' "$EXPECT_DIR/declared")
  if [ -z "$_xd" ]; then
    if [ -s "$EXPECT_DIR/declared" ]; then
      _expect_fail "row $_xk ($_xst${_xseen:+ $_xseen}) isn't declared: expect it with its verdict"
    else
      case $_xst in ERROR|NO_RESULT|UNKNOWN|"") _expect_fail "row $_xk: status ${_xst:-none}: the tool failed" ;; esac
    fi
    return 0
  fi
  _xv=$(echo "$_xd" | awk '{print $2}'); _xr=$(echo "$_xd" | awk '{print $3}')
  _xexp=$_xv; if [ "$_xr" != - ]; then _xexp="$_xv $_xr"; fi
  _xwhy=""
  if [ "$_xst" != "$_xv" ]; then
    _xwhy="status ${_xst:-none}${_xseen:+ [$_xseen]}"
  elif [ "$_xv" = VIOLATION ]; then
    _xok=""
    for _xw in $(echo "$_xr" | tr '|' ' '); do
      for _xg in $_xseen; do if [ "$_xg" = "$_xw" ]; then _xok=1; fi; done
    done
    [ -n "$_xok" ] || _xwhy="violated [${_xseen:-nothing named}], not $_xr"
  else
    [ -z "$(_xs error "$_xrec")" ] || _xwhy="an oracle assertion failed: $(_xs error "$_xrec")"
    if [ "$_xv" = SAMPLED ] && [ "$(_xn walks "$_xrec")" != "$(_xn walks_asked "$_xrec")" ]; then
      _xwhy="$(_xn walks "$_xrec") of $(_xn walks_asked "$_xrec") walks ran"
    fi
  fi
  if [ -n "$_xwhy" ]; then
    _expect_fail "row $_xk: expected $_xexp, got $_xwhy"
  else
    echo "=== row $_xk: $_xst${_xseen:+ [$_xseen]} as expected"
  fi
}

# _expect_tool KIND BIN ARGS...: run an enumerator (enum) or replay (itf) binary, its stdout
# shown and kept, and check its row from its last RUNREC line; returns the tool's exit
_expect_tool() {
  _xkind=$1; shift
  echo "$_xkind" >> "$EXPECT_DIR/rows"
  { "$@"; echo $? > "$EXPECT_DIR/last.rc"; } | tee "$EXPECT_DIR/last.out"
  _xrc=$(cat "$EXPECT_DIR/last.rc" 2>/dev/null || echo 1)
  _xrec=$(grep '^RUNREC ' "$EXPECT_DIR/last.out" | tail -1)
  if [ -z "$_xrec" ]; then
    _expect_fail "$_xkind row: $(basename "$1") exited $_xrc with no RUNREC (the tool failed): $*"
    [ "$_xrc" != 0 ] || _xrc=1
    return "$_xrc"
  fi
  _xmu=$(sed -n '1s/.* mutant=\([^ ]*\).*/\1/p' "$EXPECT_DIR/last.out")
  # the stop set as the header prints it, without its last entry (the always-on pseudo-rule
  # for a failed assertion, which the enumerator adds to every set): one rule left is a
  # one-rule run
  _xone=$(sed -n '1s/.* stop=\([^ ]*\) .*/\1/p' "$EXPECT_DIR/last.out" | sed 's/,[^,]*$//')
  case $_xone in *,*) _xone="" ;; esac
  if [ -n "$_xmu" ] && [ "$_xmu" != none ]; then _xkey="$_xkind@$_xmu${_xone:+/$_xone}"; else _xkey="$_xkind:$(_xs run "$_xrec")"; fi
  _xrules=""
  if [ "$_xkind" = enum ]; then
    _xrules=$(grep '^SUMMARY:' "$EXPECT_DIR/last.out" | tail -1 | sed -n 's/.* violations=\[\([^]]*\)\].*/\1/p' | tr ',' '\n' | sed 's/@.*//' | tr '\n' ' ')
  fi
  _expect_check "$_xkey" "$_xrec" "$(echo $_xrules)"
  return "$_xrc"
}

# _expect_tlc INSTANCE RUNREC "INVS": a TLC row (tools/undo/tlc.sh, after tlc_record), in spec/quint
_expect_tlc() {
  echo tlc >> "$EXPECT_DIR/rows"
  _xmu=$(sed -n 's/.*MUTANT = "\([^"]*\)".*/\1/p' "$1.qnt" 2>/dev/null | head -1)
  _xone=""; case ${3:-} in *" "*|"") ;; inv_*) _xone=/${3#inv_} ;; esac
  if [ -n "$_xmu" ] && [ "$_xmu" != none ]; then _xkey="tlc@$_xmu$_xone"; else _xkey="tlc:$1"; fi
  _expect_check "$_xkey" "$2" "$(_xs violation "$2" | sed -n 's/.*inv_\(.*\) is violated.*/\1/p')"
}

# _expect_step LABEL RC ROWS_BEFORE: a `timed` step that ran no row must exit 0
_expect_step() {
  echo "$1" >> "$EXPECT_DIR/steps"
  if [ "$2" != 0 ] && [ "$(_expect_rows)" -le "$3" ]; then _expect_fail "step '$1': exit $2"; fi
}
_expect_rows() { wc -l < "$EXPECT_DIR/rows" | tr -d ' '; }

# expect_done: every declared row ran once with its verdict, and nothing failed; else exit 1
expect_done() {
  [ -f "$EXPECT_DIR/done" ] && return 0
  : > "$EXPECT_DIR/done"
  while read -r _xk _xv _xr; do
    _xc=$(awk -v k="$_xk" '$1 == k' "$EXPECT_DIR/seen" | wc -l | tr -d ' ')
    if [ "$_xc" != 1 ]; then _xexp=$_xv; if [ "$_xr" != - ]; then _xexp="$_xv $_xr"; fi; _expect_fail "row $_xk (expected $_xexp) ran $_xc times"; fi
  done < "$EXPECT_DIR/declared"
  _xnd=$(wc -l < "$EXPECT_DIR/declared" | tr -d ' ')
  _xns=$(wc -l < "$EXPECT_DIR/seen" | tr -d ' ')
  _xnf=$(wc -l < "$EXPECT_DIR/fail" | tr -d ' ')
  if [ "$_xnf" = 0 ]; then
    if [ "$_xnd" = 0 ] && [ "$_xns" = 0 ] && [ ! -s "$EXPECT_DIR/steps" ]; then
      echo "=== EXPECT: RED: no row and no step ran: nothing checked"; return 1
    elif [ "$_xnd" = 0 ]; then echo "=== EXPECT: no rows declared ($_xns ran): verdicts not checked, no tool failed"
    else echo "=== EXPECT: GREEN: all $_xnd declared rows ran, each with its expected verdict"; fi
    return 0
  fi
  echo "=== EXPECT: RED: $_xnf problem(s) ($_xnd rows declared, $_xns ran):"
  sed 's/^/  /' "$EXPECT_DIR/fail"
  return 1
}
# at any exit (set -e, a stopped job): the check if it hasn't run, then the epilogue
_expect_exit() {
  _xe=$1
  if [ ! -f "$EXPECT_DIR/done" ]; then
    [ "$_xe" = 0 ] || echo "=== EXPECT: the job stopped early (exit $_xe)"
    expect_done || [ "$_xe" != 0 ] || _xe=1
  fi
  if command -v job_epilogue >/dev/null 2>&1; then job_epilogue; fi
  exit "$_xe"
}
