#!/usr/bin/env bash
# run-checks.sh — run checks.yml's steps for one commit on K2 Compute
# nodes and post GitHub commit statuses (plan-ci-release-on-k2-compute-v1
# §3). The CI poller (k2-ci-poll.sh) calls it for each new push; run it by
# hand for any commit; release.sh Step 0.6 can call it on the release commit.
#
#   scripts/ci/run-checks.sh <sha> [--parts web,rust-check,rust-test,audit] [--wait-min N]
#
# Run from a checkout that is a K2 workspace with compute grants on the
# nodes (z3mbpZ's CI checkout). The commit need not be pushed: K2 Compute
# sends a git bundle.
#
# One job per part, in parallel, each on its own node:
#   k2-compute/web         scripts/ci/checks.sh web          K2_CI_NODE_WEB        (k2n4-vin)
#   k2-compute/rust-check  scripts/ci/checks.sh rust-check   K2_CI_NODE_RUST_CHECK (k2n5-vin)
#   k2-compute/rust-test   scripts/ci/checks.sh rust-test    K2_CI_NODE_RUST_TEST  (k2n6-hil)
#   k2-compute/audit       scripts/ci/checks.sh audit        K2_CI_NODE_AUDIT      (k2n5-vin)
# A node name of "any" means --any --os linux --arch x86_64.
#
# web (bun install) and audit (advisory databases, tool downloads) need
# the internet. The share default on a Linux node, `no-lan`, allows it, so
# no network flag is passed; keep both on Linux nodes (on macOS a team
# tenant's `no-lan` is `none`).
#
# Statuses: pending at submit, then success / failure (the checks failed)
# / error (infra: not submitted, cancelled, node lost, timed out), each
# with "job <id> on <node>, <N>m" and no link. scripts/ci/post-status.sh
# (K2_CI_DRY_RUN=1 prints instead of posting).
#
# Record: $K2_CI_STATE/runs/<sha>.json. Exit 0 = every part passed.
set -euo pipefail

SHA="${1:-}"
[[ "$SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "usage: run-checks.sh <40-hex sha> [--parts a,b] [--wait-min N]" >&2; exit 2; }
shift
PARTS="web,rust-check,rust-test,audit"
WAIT_MIN="${K2_CI_WAIT_MIN:-150}"
while [ $# -gt 0 ]; do
    case "$1" in
        --parts) PARTS="${2:-}"; shift 2 ;;
        --wait-min) WAIT_MIN="${2:-}"; shift 2 ;;
        *) echo "usage: run-checks.sh <sha> [--parts a,b] [--wait-min N]" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CI="$ROOT/scripts/ci"
K2="${K2_BIN:-k2}"
STATE="${K2_CI_STATE:-$HOME/.local/state/k2-ci}"
mkdir -p "$STATE/runs"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-ci-${SHA:0:12}.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

git -C "$ROOT" cat-file -e "${SHA}^{commit}" 2>/dev/null || git -C "$ROOT" fetch -q origin "$SHA" \
    || { echo "FATAL: commit $SHA is not here and can't be fetched" >&2; exit 1; }

# cargo-nextest: rust-test runs nextest (.config/nextest.toml, profile ci).
RUST_NEEDS="${K2_CI_RUST_NEEDS:-rust@stable,cargo-nextest,cmake@3,sys:pkg-config,sys:zlib1g-dev}"
WEB_NEEDS="${K2_CI_WEB_NEEDS:-bun@1.2,node@22,python@3.12}"
AUDIT_NEEDS="${K2_CI_AUDIT_NEEDS:-rust@stable,python@3.12,sys:pkg-config}"

node_for() {
    case "$1" in
        web) echo "${K2_CI_NODE_WEB:-k2n4-vin}" ;;
        rust-check) echo "${K2_CI_NODE_RUST_CHECK:-k2n5-vin}" ;;
        rust-test) echo "${K2_CI_NODE_RUST_TEST:-k2n6-hil}" ;;
        audit) echo "${K2_CI_NODE_AUDIT:-k2n5-vin}" ;;
    esac
}

status() { bash "$CI/post-status.sh" "$SHA" "$1" "k2-compute/$2" "$3" || echo "  (status for $2 not posted)" >&2; }

IFS=',' read -r -a parts <<< "$PARTS"
for p in "${parts[@]}"; do
    case "$p" in web|rust-check|rust-test|audit) ;; *) echo "unknown part $p" >&2; exit 2 ;; esac
done

# Submit every part, pending first.
: > "$WORK/jobs"
for p in "${parts[@]}"; do
    node="$(node_for "$p")"
    needs="$RUST_NEEDS"; [ "$p" = web ] && needs="$WEB_NEEDS"; [ "$p" = audit ] && needs="$AUDIT_NEEDS"
    status pending "$p" "submitting to $node"
    args=(compute run)
    if [ "$node" = any ]; then args+=(--any --os linux --arch x86_64); else args+=("$node"); fi
    args+=(--sha "$SHA" --detach --json --needs "$needs" --mem 16G --timeout 2h --queue-timeout 2h --retries 1)
    [ -n "${K2_CI_WORKSPACE:-}" ] && args+=(--workspace "$K2_CI_WORKSPACE")
    if out="$(cd "$ROOT" && "$K2" "${args[@]}" -- bash scripts/ci/checks.sh "$p" 2>"$WORK/$p.err")" \
        && job="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job"])' 2>/dev/null)"; then
        printf '%s\t%s\n' "$p" "$job" >> "$WORK/jobs"
        status pending "$p" "job ${job:0:8} on $node"
        echo "  $p → job ${job:0:8} on $node"
    else
        code="$(python3 -c 'import json,sys
try: print(json.loads(open(sys.argv[1]).read().strip().splitlines()[-1]).get("error","submit_failed"))
except Exception: print("submit_failed")' "$WORK/$p.err")"
        status error "$p" "not submitted to $node: $code"
        echo "  ✗ $p: not submitted ($code)" >&2
    fi
done

ids="$(cut -f2 "$WORK/jobs" | tr '\n' ' ')"
deadline=$(( $(date +%s) + WAIT_MIN * 60 ))
while [ -n "${ids// /}" ]; do
    # shellcheck disable=SC2086
    "$K2" compute wait $ids --max-wait 9m --quiet >/dev/null 2>"$WORK/wait.err" || true
    grep -q still_running "$WORK/wait.err" 2>/dev/null || break
    if [ "$(date +%s)" -ge "$deadline" ]; then
        echo "  ✗ still running after $WAIT_MIN min" >&2
        break
    fi
done

rc=0
record="[]"
while IFS=$'\t' read -r p job; do
    [ -n "$p" ] || continue
    "$K2" compute get "$job" --json > "$WORK/$p.json" 2>/dev/null || true
    line="$(python3 - "$WORK/$p.json" <<'PY'
import json, sys
try:
    v = json.load(open(sys.argv[1]))
except Exception:
    print("error\tno result for the job\t?"); sys.exit(0)
short, node = v.get("short") or "?", v.get("node") or "?"
mins = max(1, round((v.get("durationSecs") or 0) / 60))
st, ex = v.get("state"), v.get("exit")
if st == "done" and ex == 0:
    print("success\tjob %s on %s, %dm\t%s" % (short, node, mins, st))
elif st == "done":
    print("failure\tjob %s on %s: exit %s after %dm (k2 compute logs %s --failures)\t%s" % (short, node, ex, mins, short, st))
elif st in ("queued", "assigned", "running", "finishing"):
    print("error\tjob %s on %s still %s at the deadline\t%s" % (short, node, st, st))
else:
    print("error\tjob %s on %s: %s (%s)\t%s" % (short, node, st, v.get("reason") or "", st))
PY
)"
    state="${line%%$'\t'*}" rest="${line#*$'\t'}"
    desc="${rest%%$'\t'*}"
    status "$state" "$p" "$desc"
    echo "  $p: $state — $desc"
    [ "$state" = success ] || rc=1
    record="$(python3 -c 'import json,sys; a=json.loads(sys.argv[1]); a.append({"part": sys.argv[2], "job": sys.argv[3], "state": sys.argv[4], "description": sys.argv[5]}); print(json.dumps(a))' "$record" "$p" "$job" "$state" "$desc")"
done < "$WORK/jobs"
# A part that was never submitted already has its error status.
[ "$(wc -l < "$WORK/jobs" | tr -d ' ')" = "${#parts[@]}" ] || rc=1
python3 -c 'import json,sys,time; json.dump({"sha": sys.argv[1], "at": int(time.time()), "parts": json.loads(sys.argv[2]), "ok": sys.argv[3] == "0"}, open(sys.argv[4], "w"), indent=2)' \
    "$SHA" "$record" "$rc" "$STATE/runs/$SHA.json"
exit "$rc"
