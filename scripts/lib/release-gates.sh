# shellcheck shell=bash
# release-gates.sh — release.sh's pre-flight gates (Step 0.5, and the Linux
# build gate of Step 0.6) as parallel K2 Compute jobs, and
# `release.sh --gates-only [<commit>]`, which runs only them.
#
# Sourced after rust-pin.sh, compute-tier.sh and linux-gate-spec.sh. Sets no
# shell options; release.sh runs under set -euo pipefail.
#
# The gates (one job each, at one commit, --tier ci, detached):
#   cargo-linux     RUSTFLAGS='-D warnings' cargo check --workspace
#                   --all-targets on a Linux x86 node K2 picks (never
#                   k2-arch), the pinned Rust: the old Step 0.6 job.
#   cargo-macos     the same check on macOS (the code behind cfg(macos)).
#                   In a release it runs HERE, on the release Mac, beside
#                   the jobs (its target/ is warm and Step 2 builds here
#                   anyway); --gates-only sends it to K2_RELEASE_MAC_CHECK_NODE.
#                   K2_RELEASE_MAC_CHECK=local|compute picks.
#   vitest          the full suite on K2_RELEASE_VITEST_NODE (k2n1-m4, the
#                   fast Mac lane): --cpus 7 --mem 8G, 6 workers.
#   typecheck-web   bun run typecheck:web (tsc-rs) on a Linux x86 node.
#   vite-build-web  bun run vite:build:web on a Linux x86 node.
# Each job's command runs under set -euo pipefail, and a gate passes only
# with exit 0 AND its result line in the log (vitest's "Tests … passed",
# cargo's "Finished", vite's "built in", the typecheck's marker line):
# exit 0 without one is a failure, never a pass.
#
# Settings:
#   K2_RELEASE_GATES             compute (default) | local (the old serial
#                                path on this Mac, plus Step 0.6's own job)
#   K2_RELEASE_MAC_CHECK         local | compute (default: local in a
#                                release, compute for --gates-only)
#   K2_RELEASE_MAC_CHECK_NODE    default k2n1-m4
#   K2_RELEASE_VITEST_NODE       default k2n1-m4
#   K2_RELEASE_GATES_TIER        default ci (the release grant has --ci)
#   K2_RELEASE_GATES_WORKSPACE   sends --workspace
#   K2_RELEASE_GATES_CACHE       1 (default): the cargo gates keep their
#                                target/ in a node cache (--cache), so
#                                dependencies stay built between releases
#   K2_RELEASE_GATES_MAX_WAIT_MIN  default 60
#   K2_RELEASE_GATES_LIST        --gates-only: only these gates (comma list)
#   K2_COMPUTE_AVOID             nodes K2 never picks (compute-tier.sh)
#   K2_BIN                       the k2 CLI (tests stub it)
#
#   k2rg_mode                     compute | local
#   k2rg_reachable                0 = compute can take the gates; else a
#                                 reason in K2RG_WHY
#   k2rg_begin <sha> <release|gates-only>
#   k2rg_submit_all               send every gate
#   k2rg_wait_all                 one line per gate as each ends. 0 = every
#                                 gate passed or had a node problem (those
#                                 are in K2RG_INFRA, for the caller to run
#                                 here); 1 = a gate failed (the rest are
#                                 cancelled); 2 = still running at the cap.
#                                 K2RG_LINUX_OK=1 once cargo-linux passed.
#   k2rg_cancel_open              cancel the gate jobs that haven't ended
#   k2rg_local <gate>             run one gate here, the old way
#   k2rg_local_all                the old Step 0.5, serial, here
#   k2rg_gates_only <rev>         release.sh --gates-only

# Sent in this order: vitest first, so on k2n1-m4 (one slot) it queues ahead
# of the macOS check when --gates-only sends both there.
K2RG_GATES_ALL="vitest cargo-linux cargo-macos typecheck-web vite-build-web"
K2RG_STATE=""
K2RG_SHA=""
K2RG_KIND=""
K2RG_INFRA=""
K2RG_LINUX_OK=0
K2RG_LINUX_JOB=""

_k2rg_k2() { "${K2_BIN:-k2}" "$@"; }

k2rg_mode() {
    case "${K2_RELEASE_GATES:-compute}" in
        compute|local) printf '%s\n' "${K2_RELEASE_GATES:-compute}" ;;
        *) echo "FATAL: K2_RELEASE_GATES must be compute or local (got $K2_RELEASE_GATES)" >&2; return 2 ;;
    esac
}

# The mac check's place: K2_RELEASE_MAC_CHECK, else local in a release and
# compute for --gates-only.
_k2rg_mac_where() {
    local d=local
    [ "$K2RG_KIND" = gates-only ] && d=compute
    case "${K2_RELEASE_MAC_CHECK:-$d}" in
        local|compute) printf '%s\n' "${K2_RELEASE_MAC_CHECK:-$d}" ;;
        *) echo "FATAL: K2_RELEASE_MAC_CHECK must be local or compute (got $K2_RELEASE_MAC_CHECK)" >&2; return 2 ;;
    esac
}

# The gates sent to compute, in order.
_k2rg_compute_gates() {
    local g out=""
    for g in $K2RG_GATES_ALL; do
        # --gates-only may rerun some gates: K2_RELEASE_GATES_LIST=vitest,cargo-linux
        if [ "$K2RG_KIND" = gates-only ] && [ -n "${K2_RELEASE_GATES_LIST:-}" ]; then
            case ",$K2_RELEASE_GATES_LIST," in *",$g,"*) ;; *) continue ;; esac
        fi
        if [ "$g" = cargo-macos ] && [ "$(_k2rg_mac_where)" != compute ]; then
            continue
        fi
        out="$out $g"
    done
    printf '%s\n' "${out# }"
}

# 0 = the node list reads, a Linux x86 node other than the avoided ones is
# online, and every node a gate is pinned to is online.
k2rg_reachable() {
    local json pinned
    K2RG_WHY=""
    if ! json="$(_k2rg_k2 compute nodes --json 2>&1 </dev/null)"; then
        K2RG_WHY="k2 compute nodes failed: $(printf '%s' "$json" | head -c 200)"
        return 1
    fi
    pinned="${K2_RELEASE_VITEST_NODE:-k2n1-m4}"
    if [ "$(_k2rg_mac_where)" = compute ]; then
        pinned="$pinned,${K2_RELEASE_MAC_CHECK_NODE:-k2n1-m4}"
    fi
    K2RG_WHY="$(printf '%s' "$json" | python3 -c '
import json, sys
pinned = [p for p in sys.argv[1].split(",") if p]
avoid = [a for a in sys.argv[2].split(",") if a]
try:
    nodes = json.load(sys.stdin)["nodes"]
except Exception as e:
    print("the node list does not read (%s)" % e); sys.exit(0)
up = {n["name"]: n for n in nodes if n.get("online") and n.get("state", "active") == "active"}
if not any(n.get("os") == "linux" and n.get("arch") == "x86_64" and name not in avoid for name, n in up.items()):
    print("no Linux x86 node online"); sys.exit(0)
for p in pinned:
    if p not in up:
        print("%s is offline or not granted" % p); sys.exit(0)
' "$pinned" "${K2_COMPUTE_AVOID-k2-arch}")"
    [ -z "$K2RG_WHY" ]
}

# k2rg_begin <sha> <release|gates-only>
k2rg_begin() {
    K2RG_SHA="${1:?k2rg_begin <sha> <kind>}"
    K2RG_KIND="${2:?k2rg_begin <sha> <kind>}"
    K2RG_STATE="$(mktemp -d -t k2-release-gates.XXXXXX)"
    : > "$K2RG_STATE/jobs.tsv"
    K2RG_INFRA=""
    K2RG_LINUX_OK=0
    K2RG_LINUX_JOB=""
    K2RG_T0="$(date +%s)"
}

# _k2rg_submit <gate> <what> <run args…> -- <shell command>
_k2rg_submit() {
    local gate="$1" what="$2"
    shift 2
    local -a pre=() t
    while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do pre+=("$1"); shift; done
    shift
    local cmd="$1"
    local -a args=(compute run)
    # A pinned node goes first, as `k2 compute run <node>` wants it.
    if [ "${pre[0]:-}" != "" ] && [ "${pre[0]#--}" = "${pre[0]}" ]; then
        args+=("${pre[0]}")
        pre=("${pre[@]:1}")
    fi
    args+=(--sha "$K2RG_SHA" --detach --json)
    while IFS= read -r t; do [ -n "$t" ] && args+=("$t"); done \
        < <(K2_BIN="${K2_BIN:-k2}" k2_compute_tier_args "${K2_RELEASE_GATES_TIER:-ci}")
    if [ "$K2RG_KIND" = release ] && [ -n "$(git status --porcelain --untracked-files=normal 2>/dev/null)" ]; then
        args+=(--dirty)
    fi
    [ -n "${K2_RELEASE_GATES_WORKSPACE:-}" ] && args+=(--workspace "$K2_RELEASE_GATES_WORKSPACE")
    args+=(--queue-timeout 1h --retries 1 ${pre[@]+"${pre[@]}"})
    local out job
    if ! out="$(_k2rg_k2 "${args[@]}" -- bash -c "$cmd" </dev/null 2>"$K2RG_STATE/$gate.submit.err")"; then
        echo "  ✗ $gate: not sent: $(head -c 400 "$K2RG_STATE/$gate.submit.err")" >&2
        return 1
    fi
    job="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job"])' 2>/dev/null)" \
        || { echo "  ✗ $gate: no job id in: $(printf '%s' "$out" | head -c 300)" >&2; return 1; }
    printf '%s\t%s\t%s\n' "$gate" "$job" "$what" >> "$K2RG_STATE/jobs.tsv"
    printf '  → %-15s job %s  %s\n' "$gate" "${job:0:8}" "$what"
}

# The cargo gates' target/ in a node cache, when on.
_k2rg_cache_args() { # <name>
    [ "${K2_RELEASE_GATES_CACHE:-1}" = 1 ] && printf '%s\n%s\n' --cache "$1"
    return 0
}

k2rg_submit_all() {
    local rust repo g rc=0 t
    local -a placement=() cache=()
    repo="$(git rev-parse --show-toplevel)"
    rust="$(K2_RUST_PIN_REPO="$repo" k2_rust_need "$K2RG_SHA")" \
        || { echo "  ✗ can't read the Rust pin at ${K2RG_SHA:0:12}" >&2; return 1; }
    while IFS= read -r t; do [ -n "$t" ] && placement+=("$t"); done < <(K2_BIN="${K2_BIN:-k2}" k2_compute_avoid_args)
    local target='if [ -n "${K2_JOB_CACHE:-}" ]; then export CARGO_TARGET_DIR="$K2_JOB_CACHE/target"; fi'
    echo "  Gates at ${K2RG_SHA:0:12} ($rust), one K2 Compute job each:"
    for g in $(_k2rg_compute_gates); do
        case "$g" in
            cargo-linux)
                cache=()
                while IFS= read -r t; do [ -n "$t" ] && cache+=("$t"); done < <(_k2rg_cache_args k2-release-gate-linux)
                _k2rg_submit cargo-linux "Linux x86, cargo check -D warnings" \
                    --any --os linux --arch x86_64 ${placement[@]+"${placement[@]}"} ${cache[@]+"${cache[@]}"} \
                    --needs "$(k2_linux_gate_needs "$rust")" --mem 24G --timeout 90m \
                    -- "set -euo pipefail; $target; $K2_LINUX_GATE_CMD" || rc=1
                ;;
            cargo-macos)
                cache=()
                while IFS= read -r t; do [ -n "$t" ] && cache+=("$t"); done < <(_k2rg_cache_args k2-release-gate-macos)
                # The k2 crate's build script wants the frpc sidecar staged;
                # a check needs only a file there, so a stand-in is staged.
                _k2rg_submit cargo-macos "macOS, cargo check -D warnings" \
                    "${K2_RELEASE_MAC_CHECK_NODE:-k2n1-m4}" ${cache[@]+"${cache[@]}"} \
                    --needs "$rust,cmake@3" --cpus 7 --mem 8G --env CARGO_BUILD_JOBS="${K2_RELEASE_MAC_CHECK_JOBS:-4}" --timeout 90m \
                    -- "set -euo pipefail; $target; FRPC_SRC=/usr/bin/true ./scripts/fetch-frpc.sh; source scripts/lib/build-binaries.sh; k2_mac_build_env; if [ -n \"\${K2_JOB_CACHE:-}\" ]; then (cd \"\$K2_JOB_CACHE\" && k2_clear_failed_cmake_dirs); else k2_clear_failed_cmake_dirs; fi; RUSTFLAGS='-D warnings' cargo check --workspace --all-targets" || rc=1
                ;;
            vitest)
                _k2rg_submit vitest "full vitest" \
                    "${K2_RELEASE_VITEST_NODE:-k2n1-m4}" --cpus 7 --mem 8G --needs node@22,bun --timeout 45m \
                    -- "set -euo pipefail; bun install --frozen-lockfile >/dev/null; bunx vitest run --reporter=dot --minWorkers=1 --maxWorkers=6" || rc=1
                ;;
            typecheck-web)
                _k2rg_submit typecheck-web "bun run typecheck:web (tsc-rs)" \
                    --any --os linux --arch x86_64 ${placement[@]+"${placement[@]}"} --cpus 4 --mem 6G --needs node@22,bun --timeout 30m \
                    -- "set -euo pipefail; bun install --frozen-lockfile >/dev/null; bun run typecheck:web; echo 'typecheck:web: no errors'" || rc=1
                ;;
            vite-build-web)
                _k2rg_submit vite-build-web "bun run vite:build:web" \
                    --any --os linux --arch x86_64 ${placement[@]+"${placement[@]}"} --cpus 4 --mem 6G --needs node@22,bun --timeout 30m \
                    -- "set -euo pipefail; bun install --frozen-lockfile >/dev/null; bun run vite:build:web" || rc=1
                ;;
        esac
    done
    return "$rc"
}

# The line a gate's log must end with to count as passed (an ERE).
_k2rg_result_re() {
    case "$1" in
        vitest) printf '%s\n' '^ *Tests +[0-9]' ;;
        cargo-*) printf '%s\n' '^ *Finished .* in [0-9]' ;;
        typecheck-web) printf '%s\n' '^typecheck:web: no errors$' ;;
        vite-build-web) printf '%s\n' 'built in [0-9]' ;;
    esac
}

_k2rg_open_jobs() { # the jobs not ended yet
    local gate job what
    while IFS=$'\t' read -r gate job what; do
        [ -n "$job" ] && [ ! -f "$K2RG_STATE/$gate.ended" ] && printf '%s\n' "$job"
    done < "$K2RG_STATE/jobs.tsv"
    return 0
}

# _k2rg_collect <gate> <job>: 0 ended (verdict written), 1 not ended yet.
# Verdict in $K2RG_STATE/<gate>.ended: pass | fail | infra.
_k2rg_collect() {
    local gate="$1" job="$2" d="$K2RG_STATE/$1" info verdict when result
    mkdir -p "$d"
    _k2rg_k2 compute get "$job" --json > "$d/get.json" 2>"$d/get.err" </dev/null || return 1
    info="$(python3 - "$d/get.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1]))
state, ex = g.get("state"), g.get("exit")
if g.get("endedAt") is None or state in ("queued", "assigned", "starting", "running"):
    print("open"); sys.exit(0)
fmt = lambda s: "?" if s is None else "%dm%02ds" % (int(s) // 60, int(s) % 60)
where = "on %s · ran %s (queued %s)" % (g.get("node") or "?", fmt(g.get("durationSecs")), fmt(g.get("queueSecs")))
if state == "done" and isinstance(ex, int):
    print("%s\t%d\t%s\t%s" % ("ok" if ex == 0 else "fail", ex, where, g.get("durationSecs") or 0))
else:
    why = g.get("reason") or state
    detail = (g.get("detail") or "").replace("\t", " ").replace("\n", " ")[:200]
    print("infra\t-\t%s · %s%s\t%s" % (where, why, (": " + detail) if detail else "", g.get("durationSecs") or 0))
PY
)" || return 1
    [ "$info" = open ] && return 1
    local kind ex secs
    IFS=$'\t' read -r kind ex when secs <<< "$info"
    echo "$secs" > "$d/secs"
    _k2rg_k2 compute logs "$job" --tail 120 > "$d/tail.log" 2>&1 </dev/null || true
    result="$(grep -E "$(_k2rg_result_re "$gate")" "$d/tail.log" | tail -n 1 | sed -E 's/^ +//' || true)"
    case "$kind" in
        ok)
            if [ -n "$result" ]; then
                verdict=pass
                printf '  ✓ %-15s job %s %s · %s\n' "$gate" "${job:0:8}" "$when" "$result"
            else
                verdict=fail
                printf '  ✗ %-15s job %s %s · exit 0 but no test result line in its log: not trusted\n' "$gate" "${job:0:8}" "$when"
                sed 's/^/      /' "$d/tail.log" | tail -n 20
            fi
            ;;
        fail)
            verdict=fail
            printf '  ✗ %-15s job %s %s · exit %s%s\n' "$gate" "${job:0:8}" "$when" "$ex" "${result:+ · $result}"
            _k2rg_k2 compute logs "$job" --failures > "$d/failures.log" 2>&1 </dev/null || true
            if [ -s "$d/failures.log" ]; then
                echo "    --- k2 compute logs ${job:0:8} --failures (last ${K2_RELEASE_GATES_FAIL_LINES:-60} lines) ---"
                tail -n "${K2_RELEASE_GATES_FAIL_LINES:-60}" "$d/failures.log" | sed 's/^/    /'
            else
                # No failing tests to list (a compile or type error): the log's end.
                echo "    --- k2 compute logs ${job:0:8} (its end; --failures lists none) ---"
                grep -v '^\[k2-node\]' "$d/tail.log" | tail -n "${K2_RELEASE_GATES_FAIL_LINES:-60}" | sed 's/^/    /' || true
            fi
            ;;
        infra)
            verdict=infra
            printf '  ! %-15s job %s %s · node problem, not a gate result\n' "$gate" "${job:0:8}" "$when"
            ;;
    esac
    echo "$verdict" > "$K2RG_STATE/$gate.ended"
    if [ "$gate" = cargo-linux ] && [ "$verdict" = pass ]; then
        K2RG_LINUX_OK=1
        K2RG_LINUX_JOB="${job:0:8}"
    fi
    [ "$verdict" = infra ] && K2RG_INFRA="${K2RG_INFRA:+$K2RG_INFRA }$gate"
    return 0
}

k2rg_wait_all() {
    local deadline open gate job what failed=0 left mw
    deadline=$(( $(date +%s) + ${K2_RELEASE_GATES_MAX_WAIT_MIN:-60} * 60 ))
    while :; do
        open="$(_k2rg_open_jobs | tr '\n' ' ')"
        [ -n "${open// /}" ] || break
        left=$(( deadline - $(date +%s) ))
        if [ "$left" -le 0 ]; then
            echo "  ✗ gates still running after ${K2_RELEASE_GATES_MAX_WAIT_MIN:-60} min: $(cut -f1 "$K2RG_STATE/jobs.tsv" | while read -r g; do [ -f "$K2RG_STATE/$g.ended" ] || printf '%s ' "$g"; done)" >&2
            k2rg_cancel_open
            return 2
        fi
        mw=$(( left < 540 ? left : 540 ))
        # shellcheck disable=SC2086
        _k2rg_k2 compute wait $open --any --max-wait "${mw}s" --quiet >/dev/null 2>&1 </dev/null || true
        while IFS=$'\t' read -r gate job what; do
            [ -n "$job" ] && [ ! -f "$K2RG_STATE/$gate.ended" ] || continue
            _k2rg_collect "$gate" "$job" || continue
            [ "$(cat "$K2RG_STATE/$gate.ended")" = fail ] && failed=1
        done < "$K2RG_STATE/jobs.tsv"
        if [ "$failed" = 1 ]; then
            k2rg_cancel_open
            return 1
        fi
    done
    return 0
}

k2rg_cancel_open() {
    [ -n "${K2RG_STATE:-}" ] && [ -s "$K2RG_STATE/jobs.tsv" ] || return 0
    local gate job what n=0
    while IFS=$'\t' read -r gate job what; do
        [ -n "$job" ] && [ ! -f "$K2RG_STATE/$gate.ended" ] || continue
        _k2rg_k2 compute cancel "$job" --force >/dev/null 2>&1 </dev/null || true
        echo cancelled > "$K2RG_STATE/$gate.ended"
        n=$((n + 1))
    done < "$K2RG_STATE/jobs.tsv"
    [ "$n" -gt 0 ] && echo "  cancelled $n gate job(s) still open." >&2
    return 0
}

# The wall-clock and the jobs' summed run time, for the record.
k2rg_summary() {
    local n=0 sum=0 s gate job what wall
    while IFS=$'\t' read -r gate job what; do
        [ -n "$gate" ] || continue
        [ "$(cat "$K2RG_STATE/$gate.ended" 2>/dev/null)" = pass ] && n=$((n + 1))
        s="$(cat "$K2RG_STATE/$gate/secs" 2>/dev/null || echo 0)"
        sum=$((sum + ${s%.*}))
    done < "$K2RG_STATE/jobs.tsv"
    wall=$(( $(date +%s) - K2RG_T0 ))
    printf '  Gates: %d passed in %dm%02ds wall-clock (the jobs ran %dm%02ds added up).\n' \
        "$n" $((wall / 60)) $((wall % 60)) $((sum / 60)) $((sum % 60))
}

# One gate here, the old way (release.sh's Step 0.5 / 0.6 commands). Run
# from the repo root, with scripts/lib/build-binaries.sh sourced.
k2rg_local() {
    case "$1" in
        cargo-macos)
            k2_mac_build_env
            k2_clear_failed_cmake_dirs
            RUSTFLAGS="-D warnings" cargo check --workspace --all-targets
            ;;
        typecheck-web) bun run typecheck:web ;;
        vitest) bunx vitest run --reporter=dot ;;
        vite-build-web) bun run vite:build:web ;;
        cargo-linux) "$(git rev-parse --show-toplevel)/scripts/linux-build-gate.sh" ;;
        *) echo "FATAL: no gate $1" >&2; return 2 ;;
    esac
}

# The old Step 0.5, serial, on this Mac (Step 0.6 then runs the Linux gate).
k2rg_local_all() {
    local g
    for g in cargo-macos typecheck-web vitest vite-build-web; do
        k2rg_local "$g"
    done
    echo "  Pre-flight gates passed."
}

# release.sh --gates-only [<rev>]: only the gates, at a commit; no lock, no
# signing, no version. 0 = every gate passed; 1 = a gate failed or had a
# node problem; 3 = compute unreachable (nothing sent).
k2rg_gates_only() {
    local rev="${1:-HEAD}" sha rc=0
    sha="$(git rev-parse --verify "${rev}^{commit}")" || { echo "FATAL: $rev is not a commit here" >&2; return 2; }
    k2rg_begin "$sha" gates-only
    echo "Release gates only, at ${sha:0:12} (no lock, no signing, no version bump):"
    if [ "$(k2rg_mode)" = local ]; then
        if [ "$sha" != "$(git rev-parse HEAD)" ]; then
            echo "FATAL: K2_RELEASE_GATES=local runs the checkout as it is: check out ${sha:0:12} first" >&2
            return 2
        fi
        # shellcheck source=scripts/lib/build-binaries.sh
        source "$(git rev-parse --show-toplevel)/scripts/lib/build-binaries.sh"
        k2rg_local_all && k2rg_local cargo-linux || return 1
        return 0
    fi
    if ! k2rg_reachable; then
        echo "  K2 Compute is unreachable ($K2RG_WHY): nothing sent." >&2
        echo "  Run them here instead: K2_RELEASE_GATES=local scripts/release.sh --gates-only" >&2
        return 3
    fi
    [ -z "$(git status --porcelain --untracked-files=normal 2>/dev/null)" ] \
        || echo "  note: uncommitted changes here are not in the gates (they test the commit)."
    trap 'k2rg_cancel_open' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    k2rg_submit_all || { k2rg_cancel_open; return 1; }
    echo "  waiting for the gates (each prints a line as it ends)..."
    k2rg_wait_all || rc=$?
    if [ "$rc" = 0 ] && [ -n "$K2RG_INFRA" ]; then
        echo "  node problem on: $K2RG_INFRA (a release would run those here; rerun --gates-only)." >&2
        rc=1
    fi
    k2rg_summary
    trap - EXIT
    [ "$rc" = 0 ] && echo "  Release gates passed." || echo "  Release gates FAILED." >&2
    [ "$rc" = 0 ] || return 1
}
