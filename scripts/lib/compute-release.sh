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
#   k2cr_submit_release_set      L1 x86 bins, L3 app, V1 verify, [L2 arm]
#   k2cr_submit <label> <node> <kind> <arch> <needs> -- <argv…>
#   k2cr_wait <minutes>          every submitted job ends (or time is up)
#   k2cr_collect                 fetch + verify each job (verdicts in state dir)
#   k2cr_compare_github <tag>    shadow report vs the GitHub-built assets
#   k2cr_stage <dist-dir>        compute mode: copy the verified x86 files
#                                where release.sh signs and publishes them
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
# Settings (env; the defaults are the Ryzen build nodes). Every job is sent
# with --tier ci (the same as --gate on a pre-0.46.5 CLI): it runs first in
# the node's queue, and the workspace needs a grant with --ci on each node.
#   K2_COMPUTE_NODE          (unset)     one node for both L1 and L3 (by-hand
#                                        runs; the per-job names below win).
#                                        Never V1's node: k2n9-vin here
#                                        needs another K2_COMPUTE_VERIFY_NODE
#   K2_COMPUTE_X86_NODE      k2n7-vin    L1: daemon, node, root helper, engine
#   K2_COMPUTE_APP_NODE      k2n8-vin    L3: deb, rpm, AppImage
#   K2_COMPUTE_VERIFY_NODE   k2n9-vin    V1: a second x86 build of the same
#                                        set on another node; the daemon, root
#                                        helper, node and engine must be byte-
#                                        identical to L1's (G12). Never L1's
#                                        node: submit refuses it (every mode)
#                                        and collect checks the receipts'
#                                        node_fp. "none" = no V1 (loud in
#                                        compute mode)
#   K2_COMPUTE_ARM_NODE      (unset)     L2: aarch64, natively. Unset until the
#                                        ARM Linux VM exists (G10); GitHub still
#                                        builds aarch64 then.
#   K2_COMPUTE_RELEASE_NODES "k2n7-vin,k2n8-vin,k2n9-vin,k2n10-vin,k2n11-vin"
#                                        (+ the ARM node): a receipt from any
#                                        other node fails. Debian-based Ryzens
#                                        only: k2-arch is Arch (no apt sys:
#                                        packages) and k2n12 is the fuzz node.
#   K2_COMPUTE_RUST          the pin     rust@<this>. Default: rust-toolchain.toml
#                                        AT THE RELEASE COMMIT (scripts/lib/
#                                        rust-pin.sh); an explicit rust@ need
#                                        overrides the job checkout's file
#   K2_COMPUTE_TIER          ci          --tier for every job (ci runs first;
#                                        needs a grant with --ci). standard for
#                                        a by-hand run from a workspace without
#                                        one.
#   K2_COMPUTE_QUEUE_TIMEOUT 2h          --queue-timeout per job
#   K2_COMPUTE_BINS_MEM      20G         --mem for L1, V1 and L2
#   K2_COMPUTE_APP_MEM       24G         --mem for L3: the app links the native
#                                        voice stack (native-voice, on by default
#                                        since 0.46.9); its release link needs
#                                        at least 20G (job 96d5148c, 11-03)
#   K2_COMPUTE_WORKSPACE     (unset)     --workspace for k2 compute run (z3mbpZ's
#                                        release checkout: pod-leader)
#   K2_COMPUTE_CPUS          (unset)     --cpus per job (unset: the node's default,
#                                        4 on the Ryzens; z3mbpZ's share allows 8)
#   K2_COMPUTE_OAUTH         secret      secret: the Gmail/MS OAuth values come
#                                        from the vault via --secret, sealed to
#                                        the node, names only in the plan
#                                        (Rosson 2026-10-29) | placeholder: built
#                                        without them (by-hand shadow only; such
#                                        a daemon never ships) | env: this Mac's
#                                        values with --env (stored in the job
#                                        plan; tests only). Only secret/env bake
#                                        the daemon the way GitHub does. Every
#                                        linux-bins job (L1, V1, L2) gets the
#                                        same mode, and is --exclusive
#                                        (alone on its node: no other job,
#                                        its own workspace's included, runs
#                                        there while it holds the values;
#                                        needs a grant with --ci, and on a
#                                        shared node a share with exclusive)
#   K2_COMPUTE_EXCLUSIVE     1           0: the bins jobs go without
#                                        --exclusive (shadow and by-hand runs
#                                        while the share lacks exclusive;
#                                        such builds can't be signed). Refused
#                                        in compute mode.
#   K2_COMPUTE_OAUTH_SECRETS vault names for secret mode: Gmail id, Gmail
#                                        secret[, Microsoft id]. Default
#                                        K2_GMAIL_CLIENT_ID,K2_GMAIL_CLIENT_SECRET
#                                        plus K2_MICROSOFT_CLIENT_ID when this
#                                        Mac's env sets a real one (so compute
#                                        bakes Microsoft exactly when the Mac
#                                        build does). A person grants each, per
#                                        node: k2 vault grant NAME --to
#                                        compute:<ws>/<node> --as env
#                                        --env-name BUILD_<…> (a K2_* env
#                                        name is refused; the job's --secret
#                                        NAME=BUILD_* names it anyway)
#   K2_BIN                   k2          the CLI (tests stub it)

K2CR_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K2CR_PY="$K2CR_LIB_DIR/../ci/compute_release.py"
# shellcheck source=scripts/lib/rust-pin.sh
source "$K2CR_LIB_DIR/rust-pin.sh"
# shellcheck source=scripts/lib/compute-tier.sh
source "$K2CR_LIB_DIR/compute-tier.sh"

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
    local n="${K2_COMPUTE_RELEASE_NODES:-k2n7-vin,k2n8-vin,k2n9-vin,k2n10-vin,k2n11-vin}"
    [ -n "${K2_COMPUTE_ARM_NODE:-}" ] && n="$n,$K2_COMPUTE_ARM_NODE"
    printf '%s' "$n"
}

# k2cr_submit <label> <node> <kind> <arch> <needs> -- <argv…>
# Detached, --tier ci, at exactly K2CR_COMMIT, keeping dist/* (the node's
# walker matches no file with a trailing `**`). Appends
# "label<TAB>job<TAB>node<TAB>kind<TAB>arch<TAB>argv-json" to jobs.tsv.
k2cr_submit() {
    local label="$1" node="$2" kind="$3" arch="$4" needs="$5"
    shift 5
    [ "${1:-}" = "--" ] && shift
    local -a tier_args=()
    local t
    while IFS= read -r t; do [ -n "$t" ] && tier_args+=("$t"); done < <(k2_compute_tier_args "${K2_COMPUTE_TIER:-ci}") \
        || { echo "  ✗ $label: no tier flags" >&2; return 1; }
    local mem="${K2_COMPUTE_BINS_MEM:-20G}"
    [ "$kind" = "linux-app" ] && mem="${K2_COMPUTE_APP_MEM:-24G}"
    local -a args=(compute run "$node" --sha "$K2CR_COMMIT" --detach ${tier_args[@]+"${tier_args[@]}"} --json
        --artifact 'dist/*' --needs "$needs" --mem "$mem" --queue-timeout "${K2_COMPUTE_QUEUE_TIMEOUT:-2h}" --retries 1
        --env "BUILD_COMMIT=$K2CR_COMMIT" --env "BUILD_TREE=$K2CR_TREE")
    [ -n "${K2_COMPUTE_WORKSPACE:-}" ] && args+=(--workspace "$K2_COMPUTE_WORKSPACE")
    [ -n "${K2_COMPUTE_CPUS:-}" ] && args+=(--cpus "$K2_COMPUTE_CPUS")
    if [ "$kind" = "linux-bins" ]; then
        if [ "${K2_COMPUTE_OAUTH:-secret}" = "env" ]; then
            [ -n "${K2_GMAIL_CLIENT_ID:-}" ] && [ -n "${K2_GMAIL_CLIENT_SECRET:-}" ] \
                || { echo "  ✗ $label: K2_COMPUTE_OAUTH=env but this Mac has no Gmail OAuth values" >&2; return 1; }
            # K2_* names are refused in --env; the build script maps BUILD_*.
            args+=(--env "BUILD_GMAIL_CLIENT_ID=$K2_GMAIL_CLIENT_ID" --env "BUILD_GMAIL_CLIENT_SECRET=$K2_GMAIL_CLIENT_SECRET")
            [ -n "${K2_MICROSOFT_CLIENT_ID:-}" ] && args+=(--env "BUILD_MICROSOFT_CLIENT_ID=$K2_MICROSOFT_CLIENT_ID")
        elif [ "${K2_COMPUTE_OAUTH:-secret}" = "secret" ]; then
            # Values never in the plan or argv: the node fetches them sealed
            # at spawn, as the BUILD_* names the build script maps.
            local -a names
            IFS=, read -r -a names <<< "$(k2cr_oauth_secret_names)"
            [ "${#names[@]}" -ge 2 ] && [ -n "${names[0]}" ] && [ -n "${names[1]}" ] \
                || { echo "  ✗ $label: K2_COMPUTE_OAUTH_SECRETS needs the Gmail id and secret names" >&2; return 1; }
            args+=(--secret "${names[0]}=BUILD_GMAIL_CLIENT_ID" --secret "${names[1]}=BUILD_GMAIL_CLIENT_SECRET")
            [ -n "${names[2]:-}" ] && args+=(--secret "${names[2]}=BUILD_MICROSOFT_CLIENT_ID")
        elif [ "${K2_COMPUTE_OAUTH:-secret}" = "placeholder" ]; then
            args+=(--env BUILD_OAUTH=placeholder)
        else
            echo "  ✗ $label: K2_COMPUTE_OAUTH must be placeholder, env or secret (got $K2_COMPUTE_OAUTH)" >&2
            return 1
        fi
        # Alone on the node: a job of any workspace (its own too) beside it
        # would share its uid and could read the OAuth values in /proc or
        # ptrace cargo. The signer refuses a plan that isn't exclusive.
        case "${K2_COMPUTE_EXCLUSIVE:-1}" in
            1) args+=(--exclusive --why "K2 release build: alone on the node while it holds the OAuth values") ;;
            0)
                if [ "${K2_LINUX_BUILDS:-}" = "compute" ]; then
                    echo "  ✗ $label: K2_COMPUTE_EXCLUSIVE=0 is for shadow and by-hand runs: the signing user refuses a release build that wasn't exclusive" >&2
                    return 1
                fi
                ;;
            *) echo "  ✗ $label: K2_COMPUTE_EXCLUSIVE must be 1 or 0 (got $K2_COMPUTE_EXCLUSIVE)" >&2; return 1 ;;
        esac
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

# The vault names secret mode sends (see K2_COMPUTE_OAUTH_SECRETS).
k2cr_oauth_secret_names() {
    if [ -n "${K2_COMPUTE_OAUTH_SECRETS:-}" ]; then
        printf '%s' "$K2_COMPUTE_OAUTH_SECRETS"
        return 0
    fi
    local n="K2_GMAIL_CLIENT_ID,K2_GMAIL_CLIENT_SECRET"
    case "${K2_MICROSOFT_CLIENT_ID:-}" in
        ""|REPLACE_ME*) ;;
        *) n="$n,K2_MICROSOFT_CLIENT_ID" ;;
    esac
    printf '%s' "$n"
}

k2cr_verify_node() {
    local v="${K2_COMPUTE_VERIFY_NODE-k2n9-vin}"
    [ "$v" = "none" ] && v=""
    printf '%s' "$v"
}

# L1's node (the x86 binaries).
k2cr_x86_node() { printf '%s' "${K2_COMPUTE_X86_NODE:-${K2_COMPUTE_NODE:-k2n7-vin}}"; }

# V1 is a second build only on a second node. 0 = V1 is off, or on another
# node than L1.
k2cr_check_dual_nodes() {
    local l1 v1
    l1="$(k2cr_x86_node)"
    v1="$(k2cr_verify_node)"
    [ -n "$v1" ] || return 0
    if [ "$l1" = "$v1" ]; then
        echo "  ✗ L1 and V1 are both on $l1: the dual build needs two nodes. Set K2_COMPUTE_VERIFY_NODE to another" >&2
        echo "    release node (or move K2_COMPUTE_X86_NODE / K2_COMPUTE_NODE off $v1). Nothing was submitted." >&2
        return 1
    fi
}

k2cr_submit_release_set() {
    local rust rc=0 verify
    if [ -n "${K2_COMPUTE_RUST:-}" ]; then
        rust="rust@$K2_COMPUTE_RUST"
    else
        # The repo the release commit lives in (release.sh / shadow run from it).
        rust="$(K2_RUST_PIN_REPO="$(git rev-parse --show-toplevel)" k2_rust_need "$K2CR_COMMIT")" \
            || { echo "  ✗ can't read the Rust pin at ${K2CR_COMMIT:0:12}" >&2; return 1; }
    fi
    echo "  Rust for every job: $rust" | tee -a "$K2CR_STATE/summary.txt"
    # Before anything is submitted, in every mode (shadow too).
    k2cr_check_dual_nodes || return 1
    local bins_needs="$rust,cmake@3,sys:pkg-config,sys:zlib1g-dev"
    local app_needs="$rust,cmake@3,bun@1.2,sys:pkg-config,sys:libwebkit2gtk-4.1-dev,sys:libgtk-3-dev,sys:libayatana-appindicator3-dev,sys:librsvg2-dev,sys:patchelf,sys:libxdo-dev,sys:file,sys:xdg-utils,sys:libssl-dev,sys:libasound2-dev"
    k2cr_submit L1 "$(k2cr_x86_node)" linux-bins x86_64 "$bins_needs" -- bash scripts/ci/build-linux-bins.sh x86_64 || rc=1
    k2cr_submit L3 "${K2_COMPUTE_APP_NODE:-${K2_COMPUTE_NODE:-k2n8-vin}}" linux-app x86_64 "$app_needs" -- bash scripts/ci/build-linux-app.sh || rc=1
    verify="$(k2cr_verify_node)"
    if [ -n "$verify" ]; then
        k2cr_submit V1 "$verify" linux-bins x86_64 "$bins_needs" -- bash scripts/ci/build-linux-bins.sh x86_64 || rc=1
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
    [ "${K2_LINUX_BUILDS:-}" = "compute" ] && require="--require-oauth baked"
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
    # G12: the verify build must come from another node than L1 and equal
    # L1 for every dual-built file (the daemon and the root-key assets).
    if [ -s "$K2CR_STATE/V1.verdict.json" ] && [ -s "$K2CR_STATE/L1.verdict.json" ]; then
        # PIPESTATUS, not the pipeline's status: this lib sets no shell
        # options, and without pipefail tee's 0 would hide a failure.
        if python3 "$K2CR_PY" dual-nodes --a "$K2CR_STATE/L1.verdict.json" --b "$K2CR_STATE/V1.verdict.json" \
            | tee -a "$K2CR_STATE/summary.txt"; [ "${PIPESTATUS[0]}" = 0 ]; then :; else bad=1; fi
        if python3 "$K2CR_PY" same-files --a "$K2CR_FILES/L1/dist" --b "$K2CR_FILES/V1/dist" \
            --names "$(k2cr_dual_names)" \
            | tee -a "$K2CR_STATE/summary.txt"; [ "${PIPESTATUS[0]}" = 0 ]; then :; else bad=1; fi
    fi
    K2CR_COLLECTED=1
    return "$bad"
}

# The files V1 must reproduce byte for byte: the root-key assets (they
# run as root on customer boxes) and the daemon, the file the signing user
# signs from compute (it refuses one V1 didn't reproduce: policy
# compute_dual_names, which a test keeps equal to this list). The daemons
# reproduce since 0.46.9's shadow run.
k2cr_dual_names() {
    printf '%s' "k2-root-helper-linux-x86_64,k2-node-linux-x86_64,k2-app-engine-linux-x86_64,k2-daemon-linux-x86_64"
}

# Compute mode (stage 1): copy every verified x86 file into <dist-dir>,
# where release.sh signs and publishes it. Writes <dist-dir>/
# SHA256SUMS-linux-x86_64.txt from L3's app-linux.sums (the Arch package's
# line is merged in after the tag by arch-package.yml). Refuses unless
# collect verified everything. Prints the staged names, one per line, to
# <state>/staged.txt.
k2cr_stage() {
    local dist="$1" f
    [ "${K2CR_COLLECTED:-0}" = "1" ] || { echo "  ✗ stage: nothing collected" >&2; return 1; }
    for f in L1 L3; do
        python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))["ok"] else 1)' "$K2CR_STATE/$f.verdict.json" 2>/dev/null \
            || { echo "  ✗ stage: $f is not verified" >&2; return 1; }
    done
    mkdir -p "$dist" || return 1
    : > "$K2CR_STATE/staged.txt"
    local bins="$K2CR_FILES/L1/dist" app="$K2CR_FILES/L3/dist" b
    for b in k2-daemon k2-node k2-root-helper k2-app-engine; do
        f="$b-linux-x86_64"
        [ -s "$bins/$f" ] && [ -s "$bins/$f.sha256" ] || { echo "  ✗ stage: $f missing from L1" >&2; return 1; }
        cp "$bins/$f" "$bins/$f.sha256" "$dist/" || return 1
        printf '%s\n%s\n' "$f" "$f.sha256" >> "$K2CR_STATE/staged.txt"
    done
    local sums="$app/app-linux.sums"
    [ -s "$sums" ] && [ "$(grep -c . "$sums")" = 3 ] || { echo "  ✗ stage: L3's app-linux.sums is not 3 lines" >&2; return 1; }
    while read -r _ f; do
        [ -s "$app/$f" ] || { echo "  ✗ stage: $f missing from L3" >&2; return 1; }
        cp "$app/$f" "$dist/" || return 1
        echo "$f" >> "$K2CR_STATE/staged.txt"
    done < "$sums"
    cp "$sums" "$dist/SHA256SUMS-linux-x86_64.txt" || return 1
    echo "SHA256SUMS-linux-x86_64.txt" >> "$K2CR_STATE/staged.txt"
    # The copies must still be the verified bytes.
    (cd "$dist" && sha256sum -c --quiet SHA256SUMS-linux-x86_64.txt 2>/dev/null || shasum -a 256 -c --quiet SHA256SUMS-linux-x86_64.txt) \
        || { echo "  ✗ stage: staged packages don't match L3's sums" >&2; return 1; }
    for b in k2-daemon k2-node k2-root-helper k2-app-engine; do
        f="$b-linux-x86_64"
        (cd "$dist" && { sha256sum -c --quiet "$f.sha256" 2>/dev/null || shasum -a 256 -c --quiet "$f.sha256"; }) \
            || { echo "  ✗ stage: $f doesn't match its .sha256" >&2; return 1; }
    done
    echo "  staged $(grep -c . "$K2CR_STATE/staged.txt") compute-built files in $dist" | tee -a "$K2CR_STATE/summary.txt"
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
