#!/usr/bin/env bash
# cli-parity.sh: the R1 gate, first form. Every CLI test passes on the
# k5-cli binary as it does on the bash CLI.
#
#   bash scripts/cli-parity.sh <path to a built k5-cli> [--shard I/N] [-- test.sh ...]
#
# Makes a temp folder with `k2` and `k5` links to the binary (so its
# argv[0] is a real name) and runs each tests/cli/*.sh twice: plain (the
# bash CLI, cli/k2) and with K5_CLI_UNDER_TEST=<temp>/k2 (the binary;
# tests/cli/_cli_under_test.sh hands it to the test). Skipped: helpers
# (a name starting with `_`) and tests marked `# bash-only` (they read
# cli/k2 as source, which a binary can't stand in for).
#
# Each run gets a fresh temp HOME, no K2_*, K5_* or K2SO_* variable but
# K5_CLI_UNDER_TEST, stdin from /dev/null, and at most
# CLI_PARITY_TIMEOUT seconds (default 900) when `timeout` is there.
#
# A test that passes on bash and fails on the binary fails the gate: its
# name and the tail of its output are printed. One that fails on both is
# listed as "fails on both" and does not fail the gate; nor does one that
# fails on bash only (listed too: it points at a flaky test).
#
# --shard I/N runs every Nth test starting at the Ith (1-based), so the
# list can be split across jobs. Names after `--` run only those tests.
#
# Exit 0 when nothing fails on the binary only, 1 when something does,
# 2 on a usage or setup error.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
usage() { echo "usage: bash scripts/cli-parity.sh <k5-cli> [--shard I/N] [-- test.sh ...]" >&2; exit 2; }

[ $# -ge 1 ] || usage
BIN="$1"; shift
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
[ -f "$BIN" ] && [ -x "$BIN" ] || { echo "cli-parity: not an executable file: $BIN" >&2; exit 2; }

SHARD=1 SHARDS=1
ONLY=()
while [ $# -gt 0 ]; do
    case "$1" in
        --shard)
            [ $# -ge 2 ] || usage
            case "$2" in
                [0-9]*/[0-9]*) SHARD="${2%/*}"; SHARDS="${2#*/}" ;;
                *) usage ;;
            esac
            [ "$SHARDS" -ge 1 ] && [ "$SHARD" -ge 1 ] && [ "$SHARD" -le "$SHARDS" ] || usage
            shift 2
            ;;
        --) shift; ONLY=("$@"); break ;;
        *) usage ;;
    esac
done

TIMEOUT_SECS="${CLI_PARITY_TIMEOUT:-900}"
TIMEOUT=()
if command -v timeout >/dev/null 2>&1; then TIMEOUT=(timeout "$TIMEOUT_SECS"); fi

WORK="$(mktemp -d -t k5-cli-parity-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/out"
ln -s "$BIN" "$WORK/bin/k2"
ln -s "$BIN" "$WORK/bin/k5"

# The tests, in a stable order.
TESTS=()
skipped_bash_only=0
if [ ${#ONLY[@]} -gt 0 ]; then
    for t in "${ONLY[@]}"; do
        t="${t##*/}"
        [ -f "$ROOT/tests/cli/$t" ] || { echo "cli-parity: no test tests/cli/$t" >&2; exit 2; }
        TESTS+=("$t")
    done
else
    i=0
    while IFS= read -r path; do
        t="${path##*/}"
        case "$t" in _*) continue ;; esac
        if grep -q '^# bash-only' "$path"; then
            skipped_bash_only=$((skipped_bash_only + 1))
            continue
        fi
        i=$((i + 1))
        [ $(( (i - 1) % SHARDS + 1 )) -eq "$SHARD" ] || continue
        TESTS+=("$t")
    done < <(find "$ROOT/tests/cli" -maxdepth 1 -name '*.sh' -type f | LC_ALL=C sort)
fi
[ ${#TESTS[@]} -gt 0 ] || { echo "cli-parity: no tests selected" >&2; exit 2; }

# run_one <test> <bash|binary>: the test's exit code; its output goes to
# $WORK/out/<test>.<mode>.
run_one() {
    local t="$1" mode="$2" home rc=0
    home="$(mktemp -d "$WORK/home.XXXXXX")"
    (
        while IFS= read -r v; do
            case "$v" in K2_*|K5_*|K2SO_*) unset "$v" ;; esac
        done < <(compgen -e)
        export HOME="$home"
        if [ "$mode" = binary ]; then export K5_CLI_UNDER_TEST="$WORK/bin/k2"; fi
        cd "$ROOT"
        exec ${TIMEOUT[@]+"${TIMEOUT[@]}"} bash "tests/cli/$t"
    ) >"$WORK/out/$t.$mode" 2>&1 </dev/null || rc=$?
    rm -rf "$home"
    return "$rc"
}

# The links answer as the bash does before any test runs. The bash wants a
# port and a token even for --version (a dead port and a dummy, as
# tests/cli/_hermetic_cli.sh hands it).
hermetic=(env -i PATH="$PATH" HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=test-token)
want_version="$("${hermetic[@]}" bash "$ROOT/cli/k2" --version)"
for name in k2 k5; do
    got="$("${hermetic[@]}" "$WORK/bin/$name" --version)" \
        || { echo "cli-parity: $name --version failed" >&2; exit 2; }
    [ "$got" = "$want_version" ] || { echo "cli-parity: $name --version printed '$got', bash cli/k2 '$want_version'" >&2; exit 2; }
done

echo "cli-parity: ${#TESTS[@]} tests (shard $SHARD/$SHARDS), binary $BIN ($want_version)"
BOTH=() BINARY_ONLY=() FAIL_BOTH=() BASH_ONLY=()
for t in "${TESTS[@]}"; do
    start=$SECONDS
    rb=0; run_one "$t" bash || rb=$?
    rr=0; run_one "$t" binary || rr=$?
    took=$((SECONDS - start))
    if [ "$rb" = 0 ] && [ "$rr" = 0 ]; then
        BOTH+=("$t"); echo "  ok          $t (${took}s)"
    elif [ "$rb" = 0 ]; then
        BINARY_ONLY+=("$t"); echo "  FAIL binary $t (bash 0, binary $rr, ${took}s)"
        echo "    ── tail of its output on the binary ──"
        tail -25 "$WORK/out/$t.binary" | sed 's/^/    | /'
    elif [ "$rr" = 0 ]; then
        BASH_ONLY+=("$t"); echo "  fails bash  $t (bash $rb, binary 0, ${took}s)"
    else
        FAIL_BOTH+=("$t"); echo "  fails both  $t (bash $rb, binary $rr, ${took}s)"
        tail -3 "$WORK/out/$t.bash" | sed 's/^/    bash | /'
    fi
done

echo ""
[ ${#BINARY_ONLY[@]} -eq 0 ] || echo "fail on the binary only: ${BINARY_ONLY[*]}"
[ ${#FAIL_BOTH[@]} -eq 0 ] || echo "fail on both: ${FAIL_BOTH[*]}"
[ ${#BASH_ONLY[@]} -eq 0 ] || echo "fail on bash only: ${BASH_ONLY[*]}"
echo "cli-parity: ${#BOTH[@]} passed on both, ${#BINARY_ONLY[@]} fail on the binary only, ${#FAIL_BOTH[@]} fail on both, ${#BASH_ONLY[@]} fail on bash only ($skipped_bash_only bash-only skipped; shard $SHARD/$SHARDS)"
[ ${#BINARY_ONLY[@]} -eq 0 ]
