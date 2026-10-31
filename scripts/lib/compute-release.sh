#!/usr/bin/env bash
# compute-release.sh — Linux release builds on K2 Compute, the parts
# release.sh (and scripts/ci/shadow-linux-builds.sh) share
# (plan-ci-release-on-k2-compute-v1 §2, stage 0 "shadow").
#
# Sourced; sets no shell options. Every function returns non-zero on
# failure and prints why; the caller decides whether that stops anything
# (in shadow mode nothing does: GitHub stays the source of truth).
#
#   k2cr_init <version> <commit> <state-dir> <files-dir>
#   k2cr_submit_release_set      L1 x86 bins, L3 app, [V1 verify], [L2 arm]
#   k2cr_submit <label> <node> <kind> <arch> <needs> -- <argv…>
#   k2cr_wait <minutes>          every submitted job ends (or time is up)
#   k2cr_collect                 fetch + verify each job (verdicts in state dir)
#   k2cr_compare_github <tag>    shadow report vs the GitHub-built assets
#   k2cr_cancel_open             best effort, for an aborted release
#
# Standalone (a dry run of the shadow builds for any commit; no bump, no
# tag, no signing, no publishing, no GitHub compare; only git reads here
# and `k2 compute` client calls):
#   bash scripts/lib/compute-release.sh --shadow-only <sha> [--wait-min N]
# Run it from the release checkout (the repo that has <sha>). The version
# is crates/k2-daemon/Cargo.toml at <sha>. Audit:
# ${K2_RELEASE_AUDIT_DIR:-~/k2-release-audit}/shadow-<sha12>/{compute,files}.
# Exit 0 = every job built, came back with a receipt, was fetched and
# verified (and V1 matched L1 when V1 ran); 1 = not; 2 = usage.
#
# State dir (the audit archive, G13: ~/k2-release-audit/<v>/compute):
#   jobs.tsv, <label>/{get,receipt,fetch}.json, <label>.verdict.json,
#   shadow-report.json, summary.txt. Files dir holds the fetched bytes
#   (big; target/compute-dist/<v>), and <files-dir>-github the GitHub
#   assets the shadow report compares them with.
#
# Settings (env). Every job is sent with --gate (= --tier ci: next free
# slot; the workspace needs a grant with --ci on each node).
#   K2_COMPUTE_NODE          k2n9-vin    the default node for L1 and L3.
#                                        0.46.7 vendors the private Yrs fork
#                                        (vendor/k2-mpc-base/vendor/yrs), so
#                                        its jobs may run only on k2n9-vin or
#                                        k2-arch until Rosson lifts that. One
#                                        node = the jobs run one after another
#                                        (a grant runs 1 job at a time).
#   K2_COMPUTE_X86_NODE      $K2_COMPUTE_NODE  L1: daemon, node, root helper, engine
#   K2_COMPUTE_APP_NODE      $K2_COMPUTE_NODE  L3: deb, rpm, AppImage
#   K2_COMPUTE_VERIFY_NODE   (unset)     V1: second x86 build of the root-key
#                                        assets on another node (G12 compare)
#   K2_COMPUTE_ARM_NODE      (unset)     L2: aarch64, natively. Unset until the
#                                        ARM Linux VM exists (G10); GitHub still
#                                        builds aarch64 then.
#   K2_COMPUTE_RELEASE_NODES the build pool: k2n7-vin … k2n12-vin, k2-arch
#                                        (+ the ARM node): a receipt from any
#                                        other node fails. k2n4/5/6 are gone.
#   K2_COMPUTE_QUEUE_TIMEOUT 2h          --queue-timeout per job (on one node
#                                        the last job queues behind the others)
#   K2_COMPUTE_RUST          stable      rust@<this> (pin it for G12)
#   K2_COMPUTE_WORKSPACE     (unset)     --workspace for k2 compute run (z3mbpZ's
#                                        release checkout: pod-leader-4)
#   K2_COMPUTE_CPUS          (unset)     --cpus per job (unset: the node's default,
#                                        4 on the Ryzens; z3mbpZ's share allows 8)
#   K2_COMPUTE_OAUTH         placeholder placeholder | env: pass this Mac's
#                                        Gmail/MS OAuth values with --env
#                                        (stored in the job plan; Q4) |
#                                        secret: vault secrets via --secret
#                                        (sealed to the node, names only in
#                                        the plan). Only env/secret bake the
#                                        daemon the way GitHub does, so only
#                                        they can match its bytes. Every
#                                        linux-bins job (L1, V1, L2) gets the
#                                        same mode.
#   K2_COMPUTE_OAUTH_SECRETS "K2_GMAIL_CLIENT_ID,K2_GMAIL_CLIENT_SECRET"
#                                        vault names for secret mode: Gmail id,
#                                        Gmail secret[, Microsoft id] (the MS
#                                        one only if GitHub bakes it too). A
#                                        person grants each: k2 vault grant
#                                        NAME --to compute:<ws>/<node> --as env
#   K2_BIN                   k2          the CLI (tests stub it)

K2CR_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K2CR_PY="$K2CR_LIB_DIR/../ci/compute_release.py"

k2cr_k2() { "${K2_BIN:-k2}" "$@"; }

k2cr_init() {
    K2CR_VERSION="$1"
    K2CR_COMMIT="$2"
    K2CR_STATE="$3"
    K2CR_FILES="$4"
    K2CR_TREE="$(git rev-parse "${K2CR_COMMIT}^{tree}")" || return 1
    K2CR_COLLECTED=0
    mkdir -p "$K2CR_STATE" "$K2CR_FILES" || return 1
    K2CR_STATE="$(cd "$K2CR_STATE" && pwd)"
    K2CR_FILES="$(cd "$K2CR_FILES" && pwd)"
    : > "$K2CR_STATE/jobs.tsv"
    {
        echo "version $K2CR_VERSION"
        echo "commit  $K2CR_COMMIT"
        echo "tree    $K2CR_TREE"
    } > "$K2CR_STATE/summary.txt"
}

k2cr_release_nodes() {
    local n="${K2_COMPUTE_RELEASE_NODES:-k2n7-vin,k2n8-vin,k2n9-vin,k2n10-vin,k2n11-vin,k2n12-vin,k2-arch}"
    [ -n "${K2_COMPUTE_ARM_NODE:-}" ] && n="$n,$K2_COMPUTE_ARM_NODE"
    printf '%s' "$n"
}

# k2cr_submit <label> <node> <kind> <arch> <needs> -- <argv…>
# Detached, --gate, at exactly K2CR_COMMIT, keeping dist/* (the node's
# walker matches no file with a trailing `**`). Appends
# "label<TAB>job<TAB>node<TAB>kind<TAB>arch<TAB>argv-json" to jobs.tsv.
k2cr_submit() {
    local label="$1" node="$2" kind="$3" arch="$4" needs="$5"
    shift 5
    [ "${1:-}" = "--" ] && shift
    local -a args=(compute run "$node" --sha "$K2CR_COMMIT" --detach --gate --json
        --artifact 'dist/*' --needs "$needs" --mem 16G --queue-timeout "${K2_COMPUTE_QUEUE_TIMEOUT:-2h}" --retries 1
        --env "BUILD_COMMIT=$K2CR_COMMIT" --env "BUILD_TREE=$K2CR_TREE")
    [ -n "${K2_COMPUTE_WORKSPACE:-}" ] && args+=(--workspace "$K2_COMPUTE_WORKSPACE")
    [ -n "${K2_COMPUTE_CPUS:-}" ] && args+=(--cpus "$K2_COMPUTE_CPUS")
    if [ "$kind" = "linux-bins" ]; then
        if [ "${K2_COMPUTE_OAUTH:-placeholder}" = "env" ]; then
            [ -n "${K2_GMAIL_CLIENT_ID:-}" ] && [ -n "${K2_GMAIL_CLIENT_SECRET:-}" ] \
                || { echo "  ✗ $label: K2_COMPUTE_OAUTH=env but this Mac has no Gmail OAuth values" >&2; return 1; }
            # K2_* names are refused in --env; the build script maps BUILD_*.
            args+=(--env "BUILD_GMAIL_CLIENT_ID=$K2_GMAIL_CLIENT_ID" --env "BUILD_GMAIL_CLIENT_SECRET=$K2_GMAIL_CLIENT_SECRET")
            [ -n "${K2_MICROSOFT_CLIENT_ID:-}" ] && args+=(--env "BUILD_MICROSOFT_CLIENT_ID=$K2_MICROSOFT_CLIENT_ID")
        elif [ "${K2_COMPUTE_OAUTH:-placeholder}" = "secret" ]; then
            # Values never in the plan or argv: the node fetches them sealed
            # at spawn, as the BUILD_* names the build script maps.
            local -a names
            IFS=, read -r -a names <<< "${K2_COMPUTE_OAUTH_SECRETS:-K2_GMAIL_CLIENT_ID,K2_GMAIL_CLIENT_SECRET}"
            [ "${#names[@]}" -ge 2 ] && [ -n "${names[0]}" ] && [ -n "${names[1]}" ] \
                || { echo "  ✗ $label: K2_COMPUTE_OAUTH_SECRETS needs the Gmail id and secret names" >&2; return 1; }
            args+=(--secret "${names[0]}=BUILD_GMAIL_CLIENT_ID" --secret "${names[1]}=BUILD_GMAIL_CLIENT_SECRET")
            [ -n "${names[2]:-}" ] && args+=(--secret "${names[2]}=BUILD_MICROSOFT_CLIENT_ID")
        elif [ "${K2_COMPUTE_OAUTH:-placeholder}" = "placeholder" ]; then
            args+=(--env BUILD_OAUTH=placeholder)
        else
            echo "  ✗ $label: K2_COMPUTE_OAUTH must be placeholder, env or secret (got $K2_COMPUTE_OAUTH)" >&2
            return 1
        fi
        # Path remap is always on in build-linux-bins.sh (repro-build-env.sh).
    fi
    local out job argv_json
    out="$(k2cr_k2 "${args[@]}" -- "$@")" || { echo "  ✗ $label: submit to $node failed" >&2; return 1; }
    job="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job"])')" \
        || { echo "  ✗ $label: no job id in: $out" >&2; return 1; }
    argv_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "$@")"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$job" "$node" "$kind" "$arch" "$argv_json" >> "$K2CR_STATE/jobs.tsv"
    echo "  $label → job ${job:0:8} on $node ($kind $arch)"
}

k2cr_submit_release_set() {
    local rust="rust@${K2_COMPUTE_RUST:-stable}" rc=0
    local bins_needs="$rust,cmake@3,sys:pkg-config,sys:zlib1g-dev"
    local app_needs="$rust,cmake@3,bun@1.2,sys:pkg-config,sys:libwebkit2gtk-4.1-dev,sys:libgtk-3-dev,sys:libayatana-appindicator3-dev,sys:librsvg2-dev,sys:patchelf,sys:libxdo-dev,sys:file,sys:xdg-utils,sys:libssl-dev"
    local node="${K2_COMPUTE_NODE:-k2n9-vin}"
    k2cr_submit L1 "${K2_COMPUTE_X86_NODE:-$node}" linux-bins x86_64 "$bins_needs" -- bash scripts/ci/build-linux-bins.sh x86_64 || rc=1
    k2cr_submit L3 "${K2_COMPUTE_APP_NODE:-$node}" linux-app x86_64 "$app_needs" -- bash scripts/ci/build-linux-app.sh || rc=1
    if [ -n "${K2_COMPUTE_VERIFY_NODE:-}" ]; then
        k2cr_submit V1 "$K2_COMPUTE_VERIFY_NODE" linux-bins x86_64 "$bins_needs" -- bash scripts/ci/build-linux-bins.sh x86_64 || rc=1
    fi
    if [ -n "${K2_COMPUTE_ARM_NODE:-}" ]; then
        k2cr_submit L2 "$K2_COMPUTE_ARM_NODE" linux-bins aarch64 "$bins_needs" -- bash scripts/ci/build-linux-bins.sh aarch64 || rc=1
    else
        echo "  L2 aarch64: no ARM node yet (K2_COMPUTE_ARM_NODE unset, plan G10); GitHub builds aarch64." | tee -a "$K2CR_STATE/summary.txt"
    fi
    return "$rc"
}

k2cr_jobs() { cut -f2 "$K2CR_STATE/jobs.tsv"; }

# Block until every job ended, at most <minutes>. 0 = all ended.
k2cr_wait() {
    local minutes="${1:-60}" ids
    ids="$(k2cr_jobs | tr '\n' ' ')"
    [ -n "${ids// /}" ] || { echo "  (no compute jobs to wait for)"; return 1; }
    local deadline=$(( $(date +%s) + minutes * 60 )) rc
    while :; do
        rc=0
        # shellcheck disable=SC2086
        k2cr_k2 compute wait $ids --max-wait 9m --quiet >/dev/null 2>"$K2CR_STATE/wait.err" || rc=$?
        if ! grep -q still_running "$K2CR_STATE/wait.err" 2>/dev/null; then
            return 0 # ended (pass or fail: collect reads each job)
        fi
        if [ "$(date +%s)" -ge "$deadline" ]; then
            echo "  ✗ compute jobs still running after ${minutes} min" >&2
            return 1
        fi
        echo "  …compute jobs still running (rc=$rc)"
    done
}

# Fetch and verify every job. 0 = every job verified.
k2cr_collect() {
    local label job node kind arch argv_json bad=0 d require=""
    [ "${K2_COMPUTE_REQUIRE_BAKED_OAUTH:-0}" = "1" ] && require="--require-oauth baked"
    while IFS=$'\t' read -r label job node kind arch argv_json; do
        [ -n "$label" ] || continue
        d="$K2CR_STATE/$label"
        mkdir -p "$d"
        if ! k2cr_k2 compute get "$job" --json > "$d/get.json" 2>"$d/get.err"; then
            # A failed job still has a result object; get exits with the job's code.
            [ -s "$d/get.json" ] || { echo "  ✗ $label: k2 compute get failed: $(head -c 300 "$d/get.err")" >&2; bad=1; continue; }
        fi
        if ! k2cr_k2 compute receipt "$job" --json > "$d/receipt.json" 2>"$d/receipt.err"; then
            echo "  ✗ $label: no receipt: $(head -c 300 "$d/receipt.err")" >&2; bad=1; continue
        fi
        rm -rf "${K2CR_FILES:?}/$label"
        if ! k2cr_k2 compute fetch "$job" --out "$K2CR_FILES/$label" --over-relay --json > "$d/fetch.json" 2>"$d/fetch.err"; then
            echo "  ✗ $label: fetch failed: $(tail -c 300 "$d/fetch.err")" >&2; bad=1; continue
        fi
        # shellcheck disable=SC2086
        if python3 "$K2CR_PY" check-job --label "$label" --get "$d/get.json" --receipt "$d/receipt.json" \
            --fetched "$d/fetch.json" --commit "$K2CR_COMMIT" --tree "$K2CR_TREE" --argv-json "$argv_json" \
            --nodes "$(k2cr_release_nodes)" --version "$K2CR_VERSION" --kind "$kind" --arch "$arch" $require \
            > "$K2CR_STATE/$label.verdict.json"; then
            echo "  ✓ $label verified (job ${job:0:8} on $node)" | tee -a "$K2CR_STATE/summary.txt"
        else
            echo "  ✗ $label NOT verified (job ${job:0:8} on $node):" | tee -a "$K2CR_STATE/summary.txt" >&2
            python3 -c 'import json,sys; [print("      - " + p) for p in json.load(open(sys.argv[1]))["problems"]]' \
                "$K2CR_STATE/$label.verdict.json" | tee -a "$K2CR_STATE/summary.txt" >&2
            bad=1
        fi
    done < "$K2CR_STATE/jobs.tsv"
    # G12: the verify build must equal L1 for the root-key assets.
    if [ -s "$K2CR_STATE/V1.verdict.json" ] && [ -s "$K2CR_STATE/L1.verdict.json" ]; then
        if python3 "$K2CR_PY" same-files --a "$K2CR_FILES/L1/dist" --b "$K2CR_FILES/V1/dist" \
            --names k2-root-helper-linux-x86_64,k2-node-linux-x86_64,k2-app-engine-linux-x86_64 \
            | tee -a "$K2CR_STATE/summary.txt"; then :; else bad=1; fi
    fi
    K2CR_COLLECTED=1
    return "$bad"
}

# Shadow report: compute-built files vs this tag's GitHub assets.
k2cr_compare_github() {
    local tag="$1" gh_dir="${K2CR_FILES%/}-github" n
    local repo="${K2_RELEASE_REPO:-Alakazam-211/K2}"
    rm -rf "$gh_dir"
    mkdir -p "$gh_dir"
    # Exactly the names compute built (never a glob that could pick up
    # an air-gap or other private asset).
    for n in $(python3 -c '
import json, os, sys
names = set()
for f in os.listdir(sys.argv[1]):
    if f.endswith(".verdict.json"):
        for k in json.load(open(os.path.join(sys.argv[1], f))).get("files") or {}:
            b = k.rsplit("/", 1)[-1]
            if b != "build-info.json" and not b.endswith(".sha256") and b != "app-linux.sums":
                names.add(b)
print("\n".join(sorted(names)))' "$K2CR_STATE"); do
        gh release download "$tag" --repo "$repo" --pattern "$n" --dir "$gh_dir" --clobber >/dev/null 2>&1 \
            || echo "  (no GitHub asset $n on $tag yet)"
    done
    python3 "$K2CR_PY" compare --compute "$K2CR_FILES" --github "$gh_dir" --out "$K2CR_STATE/shadow-report.json" \
        | tee -a "$K2CR_STATE/summary.txt"
}

# Release aborted before collect: stop jobs nobody will read.
k2cr_cancel_open() {
    [ -n "${K2CR_STATE:-}" ] && [ -s "$K2CR_STATE/jobs.tsv" ] && [ "${K2CR_COLLECTED:-0}" = "0" ] || return 0
    local job
    for job in $(k2cr_jobs); do
        k2cr_k2 compute cancel "$job" --force >/dev/null 2>&1 || true
    done
    echo "  compute jobs cancelled (release stopped before they were collected)." >&2
}

# One line per job: where, how long it queued and ran, how it ended.
k2cr_job_times() {
    local label job node kind arch argv_json
    while IFS=$'\t' read -r label job node kind arch argv_json; do
        [ -n "$label" ] || continue
        python3 - "$K2CR_STATE/$label/get.json" "$label" "$job" "$node" <<'PY' 2>/dev/null || echo "  $label: job ${job:0:8} on $node (no result)"
import json, sys
path, label, job, node = sys.argv[1:5]
g = json.load(open(path))
q, d = g.get("queueSecs"), g.get("durationSecs")
fmt = lambda s: "?" if s is None else f"{s / 60:.1f} min"
print(f"  {label}: job {job[:8]} on {node}: {g.get('state')} exit {g.get('exit')}, queued {fmt(q)}, ran {fmt(d)}")
PY
    done < "$K2CR_STATE/jobs.tsv"
}

# ── Standalone: --shadow-only <sha> (see the header) ──
k2cr_shadow_only_main() {
    local usage="usage: bash scripts/lib/compute-release.sh --shadow-only <sha> [--wait-min N]"
    local sha="" wait_min=90
    while [ $# -gt 0 ]; do
        case "$1" in
            --shadow-only) [ $# -ge 2 ] || { echo "$usage" >&2; return 2; }; sha="$2"; shift 2 ;;
            --wait-min) [ $# -ge 2 ] || { echo "$usage" >&2; return 2; }; wait_min="$2"; shift 2 ;;
            *) echo "$usage" >&2; return 2 ;;
        esac
    done
    [ -n "$sha" ] || { echo "$usage" >&2; return 2; }
    case "$wait_min" in ''|*[!0-9]*) echo "$usage" >&2; return 2 ;; esac
    local commit version short base rc=0
    commit="$(git rev-parse --verify --quiet "${sha}^{commit}")" \
        || { echo "FATAL: $sha is not a commit in $(pwd) (run from the release checkout)" >&2; return 2; }
    version="$(git show "$commit:crates/k2-daemon/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)"
    [ -n "$version" ] || { echo "FATAL: no version in crates/k2-daemon/Cargo.toml at ${commit:0:12}" >&2; return 1; }
    short="${commit:0:12}"
    base="${K2_RELEASE_AUDIT_DIR:-$HOME/k2-release-audit}/shadow-$short"
    echo "Shadow-only: K2 Compute Linux builds for ${short} (version ${version}). No tag, no signing, no publishing."
    k2cr_init "$version" "$commit" "$base/compute" "$base/files" || return 1
    echo "  Audit: $K2CR_STATE"
    k2cr_submit_release_set || rc=1
    if [ ! -s "$K2CR_STATE/jobs.tsv" ]; then
        echo "  ✗ no job was submitted" >&2
        return 1
    fi
    k2cr_wait "$wait_min" || rc=1
    k2cr_collect || rc=1
    k2cr_job_times | tee -a "$K2CR_STATE/summary.txt"
    echo "Audit: $K2CR_STATE (summary.txt, <label>.verdict.json); files: $K2CR_FILES"
    return "$rc"
}

if [ "${BASH_SOURCE[0]}" = "${0:-}" ]; then
    set -uo pipefail
    k2cr_shadow_only_main "$@"
    exit $?
fi
