#!/usr/bin/env bash
# run-checks.sh — run checks.yml's steps for one commit on K2 Compute
# nodes and post GitHub commit statuses (plan-ci-release-on-k2-compute-v1
# §3; prd-ci-release-on-compute-stage1-v1 §4).
#
#   scripts/ci/run-checks.sh <sha> [--parts web,rust-check,rust-test,audit] [--ref REF] [--wait-min N]
#       Submit every part, then wait and collect until each job ended (or
#       N minutes, default 150). By hand, or release.sh.
#   scripts/ci/run-checks.sh <sha> --submit-only [--parts …] [--ref REF] [--wait-min N]
#       Submit and return at once: the run is "in flight". The poller
#       (k2-ci-poll.sh) does this, so no run of it ever waits for a job.
#   scripts/ci/run-checks.sh --reap <sha>
#       Look at an in-flight run's jobs once. Every job ended (or the
#       deadline passed): post the final statuses, write the record, exit
#       0 (all passed) or 1. Still running: exit 10, nothing posted.
#
# Run from a checkout that is a K2 workspace with compute grants on the
# nodes (z3mbpZ's CI checkout, workspace k2-ci). The commit need not be
# pushed: K2 Compute sends a git bundle.
#
# One job per part, each `--tier ci` (CI goes first in every node's queue;
# the workspace's grants need --ci):
#   k2-compute/web         scripts/ci/checks.sh web
#   k2-compute/rust-check  scripts/ci/checks.sh rust-check
#   k2-compute/rust-test   scripts/ci/checks.sh rust-test
#   k2-compute/audit       scripts/ci/checks.sh audit
# Where: K2 picks the node (--any --os linux --arch x86_64) among the
# nodes this workspace is granted. The grants are the fence: k2-ci is
# granted k2n7/k2n8/k2n9/k2n10 only, never k2n12. K2_CI_NODE_WEB,
# K2_CI_NODE_RUST_CHECK, K2_CI_NODE_RUST_TEST, K2_CI_NODE_AUDIT pin a part
# to a node instead ("any" = the default).
#
# Rust: rust@<rust-toolchain.toml's channel AT <sha>> (scripts/lib/
# rust-pin.sh). An explicit rust@ need overrides the job checkout's file,
# so it must be the commit's own pin, never rust@stable.
#
# --ref REF: the branch the sha is the tip of. Each job then carries
# --supersede k2-ci/<REF>/<part>: a newer push to that branch cancels this
# run's jobs that haven't started yet (never a running one); they end
# "error … superseded".
#
# web (bun install) and audit (advisory databases, tool downloads) need
# the internet: Linux nodes give a team or own-trust server that by
# default. Keep both on Linux nodes (on macOS a team tenant gets none).
#
# Statuses: pending at submit, then success / failure (the checks failed)
# / error (infra: not submitted, cancelled, superseded, node lost, timed
# out, still running at the deadline), each "job <id> on <node>, <N>m",
# no link. scripts/ci/post-status.sh (K2_CI_DRY_RUN=1 prints instead).
#
# State in $K2_CI_STATE (default ~/.local/state/k2-ci):
#   inflight/<sha>.tsv  a run in flight: "#deadline <epoch>" then
#                       "<part>\t<job>" lines
#   runs/<sha>.json     a finished run's record. Never reads ~/.k2.
#
# Env: K2_CI_TIER (default ci; standard for a by-hand run from a workspace
# whose grants lack --ci), K2_CI_WORKSPACE (--workspace), K2_CI_MEM
# (default 24G), K2_CI_RUST_NEEDS / K2_CI_WEB_NEEDS / K2_CI_AUDIT_NEEDS.
set -euo pipefail

usage() {
    echo "usage: run-checks.sh <40-hex sha> [--parts a,b] [--ref REF] [--wait-min N] [--submit-only]" >&2
    echo "       run-checks.sh --reap <40-hex sha>" >&2
    exit 2
}

MODE=run
SHA=""
if [ "${1:-}" = "--reap" ]; then
    MODE=reap
    SHA="${2:-}"
    [ $# -eq 2 ] || usage
    shift 2
else
    SHA="${1:-}"
    [ $# -ge 1 ] && shift
fi
[[ "$SHA" =~ ^[0-9a-f]{40}$ ]] || usage
PARTS="web,rust-check,rust-test,audit"
WAIT_MIN="${K2_CI_WAIT_MIN:-150}"
REF=""
while [ $# -gt 0 ]; do
    case "$1" in
        --parts) PARTS="${2:-}"; shift 2 ;;
        --wait-min) WAIT_MIN="${2:-}"; shift 2 ;;
        --ref) REF="${2:-}"; shift 2 ;;
        --submit-only) MODE=submit; shift ;;
        *) usage ;;
    esac
done
case "$WAIT_MIN" in ''|*[!0-9]*) usage ;; esac

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CI="$ROOT/scripts/ci"
K2="${K2_BIN:-k2}"
STATE="${K2_CI_STATE:-$HOME/.local/state/k2-ci}"
INFLIGHT="$STATE/inflight/$SHA.tsv"
mkdir -p "$STATE/runs" "$STATE/inflight"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-ci-${SHA:0:12}.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

status() { bash "$CI/post-status.sh" "$SHA" "$1" "k2-compute/$2" "$3" || echo "  (status for $2 not posted)" >&2; }

node_for() {
    case "$1" in
        web) echo "${K2_CI_NODE_WEB:-any}" ;;
        rust-check) echo "${K2_CI_NODE_RUST_CHECK:-any}" ;;
        rust-test) echo "${K2_CI_NODE_RUST_TEST:-any}" ;;
        audit) echo "${K2_CI_NODE_AUDIT:-any}" ;;
    esac
}

submit() {
    git -C "$ROOT" cat-file -e "${SHA}^{commit}" 2>/dev/null || git -C "$ROOT" fetch -q origin "$SHA" \
        || { echo "FATAL: commit $SHA is not here and can't be fetched" >&2; exit 1; }
    if [ -s "$INFLIGHT" ]; then
        echo "FATAL: a run of ${SHA:0:12} is already in flight ($INFLIGHT); reap it first: run-checks.sh --reap $SHA" >&2
        exit 1
    fi
    # shellcheck source=scripts/lib/rust-pin.sh
    source "$ROOT/scripts/lib/rust-pin.sh"
    # shellcheck source=scripts/lib/compute-tier.sh
    source "$ROOT/scripts/lib/compute-tier.sh"
    local rust t
    rust="$(k2_rust_need "$SHA")"
    local -a tier_args=()
    while IFS= read -r t; do [ -n "$t" ] && tier_args+=("$t"); done < <(K2_BIN="$K2" k2_compute_tier_args "${K2_CI_TIER:-ci}")
    # cargo-nextest: rust-test runs nextest (.config/nextest.toml, profile ci).
    local rust_needs="${K2_CI_RUST_NEEDS:-$rust,cargo-nextest,cmake@3,sys:pkg-config,sys:zlib1g-dev}"
    local web_needs="${K2_CI_WEB_NEEDS:-bun@1.2,node@22,python@3.12}"
    local audit_needs="${K2_CI_AUDIT_NEEDS:-$rust,python@3.12,sys:pkg-config}"
    local -a parts
    IFS=',' read -r -a parts <<< "$PARTS"
    local p
    for p in "${parts[@]}"; do
        case "$p" in web|rust-check|rust-test|audit) ;; *) echo "unknown part $p" >&2; exit 2 ;; esac
    done
    printf '#deadline\t%s\n' "$(( $(date +%s) + WAIT_MIN * 60 ))" > "$INFLIGHT.new"
    local node needs out job code
    local -a args
    for p in "${parts[@]}"; do
        node="$(node_for "$p")"
        needs="$rust_needs"; [ "$p" = web ] && needs="$web_needs"; [ "$p" = audit ] && needs="$audit_needs"
        status pending "$p" "submitting ($([ "$node" = any ] && echo "K2 picks the node" || echo "$node"))"
        args=(compute run)
        if [ "$node" = any ]; then args+=(--any --os linux --arch x86_64); else args+=("$node"); fi
        args+=(--sha "$SHA" ${tier_args[@]+"${tier_args[@]}"} --detach --json --needs "$needs" --mem "${K2_CI_MEM:-24G}"
            --timeout 2h --queue-timeout 2h --retries 1)
        [ -n "$REF" ] && args+=(--supersede "k2-ci/${REF#refs/heads/}/$p")
        [ -n "${K2_CI_WORKSPACE:-}" ] && args+=(--workspace "$K2_CI_WORKSPACE")
        if out="$(cd "$ROOT" && "$K2" "${args[@]}" -- bash scripts/ci/checks.sh "$p" 2>"$WORK/$p.err")" \
            && job="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job"])' 2>/dev/null)"; then
            printf '%s\t%s\n' "$p" "$job" >> "$INFLIGHT.new"
            status pending "$p" "job ${job:0:8} queued ($rust)"
            echo "  $p → job ${job:0:8}"
        else
            code="$(python3 -c 'import json,sys
try: print(json.loads(open(sys.argv[1]).read().strip().splitlines()[-1]).get("error","submit_failed"))
except Exception: print("submit_failed")' "$WORK/$p.err")"
            printf '%s\t-\t%s\n' "$p" "$code" >> "$INFLIGHT.new"
            status error "$p" "not submitted: $code"
            echo "  ✗ $p: not submitted ($code)" >&2
        fi
    done
    mv "$INFLIGHT.new" "$INFLIGHT"
}

# One look at every job of the in-flight run. Exit codes: 0 all passed,
# 1 done with a failure or error, 10 still running (before the deadline).
reap() {
    [ -s "$INFLIGHT" ] || { echo "FATAL: no run of ${SHA:0:12} in flight ($INFLIGHT)" >&2; exit 1; }
    local deadline now late=0
    deadline="$(awk -F'\t' '$1=="#deadline" {print $2}' "$INFLIGHT")"
    now="$(date +%s)"
    [ -n "$deadline" ] && [ "$now" -ge "$deadline" ] && late=1
    local p job extra pending=0
    : > "$WORK/lines"
    while IFS=$'\t' read -r p job extra; do
        case "$p" in ''|'#'*) continue ;; esac
        if [ "$job" = "-" ]; then
            printf '%s\t-\terror\tnot submitted: %s\tnot-submitted\n' "$p" "$extra" >> "$WORK/lines"
            continue
        fi
        "$K2" compute get "$job" --json > "$WORK/$p.json" 2>/dev/null < /dev/null || true
        python3 - "$WORK/$p.json" "$p" "$job" "$late" >> "$WORK/lines" <<'PY'
import json, sys
path, part, job, late = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
try:
    v = json.load(open(path))
except Exception:
    v = None
def out(state, desc, raw):
    print("%s\t%s\t%s\t%s\t%s" % (part, job, state, desc.replace("\t", " "), raw))
if not v:
    out("error" if late else "running", "job %s: no result from K2" % job[:8], "unknown")
    sys.exit(0)
short, node = v.get("short") or job[:8], v.get("node") or "?"
mins = max(1, round((v.get("durationSecs") or 0) / 60))
st, ex, reason = v.get("state"), v.get("exit"), v.get("reason") or ""
if st == "done" and ex == 0:
    out("success", "job %s on %s, %dm" % (short, node, mins), st)
elif st == "done":
    out("failure", "job %s on %s: exit %s after %dm (k2 compute logs %s --failures)" % (short, node, ex, mins, short), st)
elif st in ("queued", "assigned", "running", "finishing"):
    if late:
        out("error", "job %s on %s still %s at the deadline" % (short, node, st), st)
    else:
        out("running", "job %s on %s %s" % (short, node, st), st)
elif "supersed" in reason:
    out("error", "job %s superseded by a newer push" % short, st)
else:
    out("error", "job %s on %s: %s (%s)" % (short, node, st, reason), st)
PY
    done < "$INFLIGHT"
    pending="$(awk -F'\t' '$3=="running"' "$WORK/lines" | wc -l | tr -d ' ')"
    if [ "$pending" != 0 ]; then
        echo "  ${SHA:0:12}: $pending part(s) still running"
        return 10
    fi
    local rc=0 state desc
    while IFS=$'\t' read -r p job state desc _; do
        status "$state" "$p" "$desc"
        echo "  $p: $state — $desc"
        [ "$state" = success ] || rc=1
    done < "$WORK/lines"
    python3 - "$WORK/lines" "$SHA" "$rc" "$STATE/runs/$SHA.json" <<'PY'
import json, sys, time
parts = []
for line in open(sys.argv[1]):
    p, job, state, desc, raw = line.rstrip("\n").split("\t")
    parts.append({"part": p, "job": job, "state": state, "description": desc})
json.dump({"sha": sys.argv[2], "at": int(time.time()), "parts": parts, "ok": sys.argv[3] == "0"},
          open(sys.argv[4], "w"), indent=2)
PY
    rm -f "$INFLIGHT"
    return "$rc"
}

case "$MODE" in
    submit) submit ;;
    reap) rc=0; reap || rc=$?; exit "$rc" ;;
    run)
        submit
        while :; do
            ids="$(awk -F'\t' '$1 !~ /^#/ && $2 != "-" {print $2}' "$INFLIGHT" | tr '\n' ' ')"
            if [ -n "${ids// /}" ]; then
                # shellcheck disable=SC2086
                "$K2" compute wait $ids --max-wait 9m --quiet >/dev/null 2>"$WORK/wait.err" || true
            fi
            rc=0
            reap || rc=$?
            [ "$rc" = 10 ] || exit "$rc"
            # wait normally blocks up to 9 min; if it came back at once (daemon
            # restarting), don't spin.
            sleep "${K2_CI_REAP_PAUSE:-5}"
        done
        ;;
esac
