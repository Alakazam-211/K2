#!/usr/bin/env bash
# ci-client-release.sh — K2's Linux release on the client path
# (prd-compute-build-attestations-v1 S1; release.sh K2_CI_PATH=client):
# the builds go through `k2 ci release` on this server's seat, and the
# release signer is k2-signer, which trusts the node owner's roster (keys a
# person accepted) instead of one hand pin per node.
#
# Sourced by release.sh after compute-release.sh (it reuses k2cr_k2 and the
# state dir k2cr_init made); sets no shell options. Every function returns
# non-zero on failure and says why; release.sh decides (fall back to
# GitHub before the tag, or warn in shadow mode).
#
#   k2ccr_preflight <version> <arm:0|1>
#                     the signer's preflight (policy, owner, controller,
#                     accepted node keys); the aarch64 cross hashes it
#                     checks equal scripts/ci/cross-pins.sh; then
#                     `k2 ci release --plan --only-keys <accepted>` for the
#                     x86_64 set (and the aarch64 one). 0 = ready; 1 = not
#                     (all of Linux falls back); 2 = only aarch64 is not.
#   k2ccr_submit <version> <commit>   two runs: x86_64 + packages, aarch64
#   k2ccr_wait <minutes>              every run ends (or time is up)
#   k2ccr_collect <version>           evidence of each run (files fetched),
#                     merged into one bundle (one per version, as the
#                     signer registers it), then `k2-signer rehearse k2`
#                     before the tag; sets K2CR_X86_OK / K2CR_ARM_OK
#   k2ccr_stage <dist-dir>            the rehearsed files, where release.sh
#                                     signs and publishes them
#   k2ccr_bundle                      the bundle path (Step 10.5)
#   k2ccr_cancel_open                 best effort, for an aborted release
#
# Settings (env):
#   K2_CI_RELEASE_BUILDS_X86  linux-x86_64,linux-app   [release] builds of run 1
#   K2_CI_RELEASE_BUILDS_ARM  linux-aarch64            [release] builds of run 2
#   K2_COMPUTE_WORKSPACE      --workspace for k2 ci (as compute-release.sh):
#                             the pre-flight and submit refuse a handle whose
#                             folder isn't this checkout
#   K2_SIGNER_CMD             a stand-in for k2-signer (tests; release.sh refuses it)
#
# State (the audit dir, ~/k2-release-audit/<v>/compute): ci-run-x86.txt,
# ci-run-arm.txt, ci/<run kind>/ (each run's evidence and files),
# ci/evidence.json (the merged bundle), signer-preflight.json,
# rehearse.json, summary.txt.

K2CCR_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/k2-signer.sh
source "$K2CCR_LIB_DIR/k2-signer.sh"

K2CCR_KEYS=""

k2ccr_ws_args() {
    [ -n "${K2_COMPUTE_WORKSPACE:-}" ] && printf -- '--workspace\n%s\n' "$K2_COMPUTE_WORKSPACE"
    return 0
}

k2ccr_ci() {
    local -a ws=()
    local a
    while IFS= read -r a; do [ -n "$a" ] && ws+=("$a"); done < <(k2ccr_ws_args)
    k2cr_k2 ci "$@" ${ws[@]+"${ws[@]}"}
}

k2ccr_strip() { sed $'s/\x1b\\[[0-9;]*m//g'; }

k2ccr_preflight() {
    local version="$1" arm="$2" pf rc=0 out wserr
    # K2_COMPUTE_WORKSPACE must be this checkout's own workspace (a handle
    # is per server; compute-release.sh's k2cr_check_workspace).
    if ! wserr="$(k2cr_check_workspace 2>&1)"; then
        echo "ERROR: $(printf '%s\n' "$wserr" | head -1 | sed 's/^ *✗ //')" >&2
        printf '%s\n' "$wserr" | tail -n +2 >&2
        return 1
    fi
    if ! pf="$(k2s_run preflight k2 2>"${TMPDIR:-/tmp}/k2ccr-pf.$$")"; then
        echo "ERROR: the release signer isn't ready for project k2: $(k2ccr_strip < "${TMPDIR:-/tmp}/k2ccr-pf.$$" | tr '\n' ' ')" >&2
        echo "  (the setup: k2 ci setup --plan --out setup.json here, then at the signing Mac: sudo k2-signer-admin apply-setup <file>)" >&2
        rm -f "${TMPDIR:-/tmp}/k2ccr-pf.$$"
        return 1
    fi
    rm -f "${TMPDIR:-/tmp}/k2ccr-pf.$$"
    [ -n "${K2CR_STATE:-}" ] && printf '%s\n' "$pf" > "$K2CR_STATE/signer-preflight.json"
    local zig zb
    zig="$(sed -n 's/^K2_CROSS_ZIG_SHA256_X86_64="\([0-9a-f]*\)"$/\1/p' "${PROJECT_DIR:-.}/scripts/ci/cross-pins.sh" 2>/dev/null)"
    zb="$(sed -n 's/^K2_CROSS_ZIGBUILD_CRATE_SHA256="\([0-9a-f]*\)"$/\1/p' "${PROJECT_DIR:-.}/scripts/ci/cross-pins.sh" 2>/dev/null)"
    out="$(python3 -I - "$pf" "$arm" "$zig" "$zb" <<'PY'
import json, sys
p, arm, zig, zb = json.loads(sys.argv[1]), sys.argv[2] == "1", sys.argv[3], sys.argv[4]
keys = p.get("accepted_keys") or []
for n in p.get("notes") or []:
    print("note\t" + n)
if p.get("roster_changes") != "ask":
    print("note\tthe signer's policy says roster_changes = %s for project k2 (K2's is ask)" % p.get("roster_changes"))
print("keys\t" + ",".join(keys))
print("names\t" + ", ".join("%s (%s…)" % (a.get("name"), a.get("fp", "")[:12]) for a in p.get("accepted") or []))
if arm:
    b = (p.get("builds") or {}).get("linux-aarch64") or {}
    eq = ((b.get("reported") or {}).get("equals")) or {}
    bad = []
    if not eq:
        bad.append("the signer's policy checks no aarch64 cross toolchain hashes (no [build.linux-aarch64.reported])")
    elif not zig or not zb:
        bad.append("this checkout's scripts/ci/cross-pins.sh has no zig / cargo-zigbuild hashes")
    else:
        if eq.get("/cross/zig_tarball_sha256") != zig:
            bad.append("the signer pins another zig tarball (%s…) than scripts/ci/cross-pins.sh (%s…)" % (str(eq.get("/cross/zig_tarball_sha256"))[:12], zig[:12]))
        if eq.get("/cross/cargo_zigbuild_crate_sha256") != zb:
            bad.append("the signer pins another cargo-zigbuild crate than scripts/ci/cross-pins.sh")
    for b in bad:
        print("arm\t" + b)
PY
)" || { echo "ERROR: can't read the signer's preflight answer" >&2; return 1; }
    printf '%s\n' "$out" | sed -n 's/^note\t/  signer: /p' >&2
    K2CCR_KEYS="$(printf '%s\n' "$out" | sed -n 's/^keys\t//p')"
    if [ -z "$K2CCR_KEYS" ]; then
        echo "ERROR: the release signer has accepted no node key for project k2 yet: nothing can be placed (k2 ci setup --plan, then apply-setup at the signing Mac)" >&2
        return 1
    fi
    echo "  Release signer: k2-signer, node keys accepted: $(printf '%s\n' "$out" | sed -n 's/^names\t//p')"
    local x86="${K2_CI_RELEASE_BUILDS_X86:-linux-x86_64,linux-app}" arm_builds="${K2_CI_RELEASE_BUILDS_ARM:-linux-aarch64}" head
    head="$(git rev-parse HEAD)" || return 1
    if ! k2ccr_ci release "$version" --sha "$head" --build "$x86" --only-keys "$K2CCR_KEYS" --plan > "${K2CR_STATE:-${TMPDIR:-/tmp}}/ci-plan-x86.txt" 2>&1; then
        echo "ERROR: k2 ci release --plan can't place the x86_64 set on nodes the signer accepted: $(k2ccr_strip < "${K2CR_STATE:-${TMPDIR:-/tmp}}/ci-plan-x86.txt" | tail -3 | tr '\n' ' ')" >&2
        return 1
    fi
    sed 's/^/  /' "${K2CR_STATE:-${TMPDIR:-/tmp}}/ci-plan-x86.txt"
    if [ "$arm" = 1 ]; then
        if printf '%s\n' "$out" | grep -q '^arm'; then
            printf '%s\n' "$out" | sed -n 's/^arm\t/ERROR: aarch64: /p' >&2
            rc=2
        elif ! k2ccr_ci release "$version" --sha "$head" --build "$arm_builds" --only-keys "$K2CCR_KEYS" --plan > "${K2CR_STATE:-${TMPDIR:-/tmp}}/ci-plan-arm.txt" 2>&1; then
            echo "ERROR: k2 ci release --plan can't place the aarch64 set on nodes the signer accepted: $(k2ccr_strip < "${K2CR_STATE:-${TMPDIR:-/tmp}}/ci-plan-arm.txt" | tail -3 | tr '\n' ' ')" >&2
            rc=2
        else
            sed 's/^/  /' "${K2CR_STATE:-${TMPDIR:-/tmp}}/ci-plan-arm.txt"
        fi
    fi
    return "$rc"
}

# k2ccr_submit_one <kind> <builds> <version> <commit>: one `k2 ci release`
# run; its id in ci-run-<kind>.txt.
k2ccr_submit_one() {
    local kind="$1" builds="$2" version="$3" commit="$4" out id
    out="$(k2ccr_ci release "$version" --sha "$commit" --build "$builds" --only-keys "$K2CCR_KEYS" --json 2>"$K2CR_STATE/ci-submit-$kind.err")" \
        || { echo "  ✗ k2 ci release ($kind: $builds) refused: $(k2ccr_strip < "$K2CR_STATE/ci-submit-$kind.err" | tail -2 | tr '\n' ' ')" >&2; return 1; }
    id="$(printf '%s' "$out" | python3 -I -c '
import json, sys
v = json.load(sys.stdin)
r = v["run"]
if r.get("state") not in ("running", "passed"):
    sys.exit("run %s is %s: %s" % (r.get("short"), r.get("state"), r.get("detail")))
print(r["id"])')" || { echo "  ✗ k2 ci release ($kind): the set was not submitted whole" >&2; return 1; }
    printf '%s\n' "$id" > "$K2CR_STATE/ci-run-$kind.txt"
    printf '%s' "$out" | python3 -I -c '
import json, sys
v = json.load(sys.stdin)
for p in v.get("placed") or []:
    print("  %s → %s (%s)" % (p["label"], p["node"], p["why"]))' | tee -a "$K2CR_STATE/summary.txt"
    echo "  $kind run ${id:0:8} submitted" | tee -a "$K2CR_STATE/summary.txt"
}

k2ccr_submit() {
    local version="$1" commit="$2" rc=0
    [ -n "$K2CCR_KEYS" ] || { echo "  ✗ no accepted node keys (the pre-flight didn't run)" >&2; return 1; }
    k2cr_check_workspace || return 1
    rm -f "$K2CR_STATE"/ci-run-*.txt
    k2ccr_submit_one x86 "${K2_CI_RELEASE_BUILDS_X86:-linux-x86_64,linux-app}" "$version" "$commit" || rc=1
    K2CR_ARM_SUBMIT_OK=""
    if [ "${K2_COMPUTE_ARM:-compute}" = compute ]; then
        if k2ccr_submit_one arm "${K2_CI_RELEASE_BUILDS_ARM:-linux-aarch64}" "$version" "$commit"; then K2CR_ARM_SUBMIT_OK=1; else K2CR_ARM_SUBMIT_OK=0; fi
    fi
    return "$rc"
}

k2ccr_runs() { cat "$K2CR_STATE"/ci-run-*.txt 2>/dev/null; }
k2ccr_run_of() { cat "$K2CR_STATE/ci-run-$1.txt" 2>/dev/null; }

# The state of a run (running, passed, failed, error).
k2ccr_state() {
    k2ccr_ci status "$1" --json 2>/dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["run"]["state"])'
}

k2ccr_wait() {
    local minutes="${1:-60}" deadline id st open
    deadline=$(( $(date +%s) + minutes * 60 ))
    [ -n "$(k2ccr_runs)" ] || { echo "  (no k2 ci release runs to wait for)"; return 1; }
    while :; do
        open=0
        for id in $(k2ccr_runs); do
            st="$(k2ccr_state "$id")" || st=unknown
            [ "$st" = running ] && open=1
        done
        [ "$open" = 0 ] && return 0
        if [ "$(date +%s)" -ge "$deadline" ]; then
            echo "  ✗ k2 ci release runs still running after ${minutes} min" >&2
            return 1
        fi
        sleep "${K2_CI_WAIT_SECS:-30}"
    done
}

# k2ccr_evidence <kind>: the run's evidence (files fetched, build-info.json
# carried); prints the bundle path. Non-zero when the run didn't pass.
k2ccr_evidence() {
    local kind="$1" id dir out
    id="$(k2ccr_run_of "$kind")" || return 1
    [ -n "$id" ] || return 1
    dir="$K2CR_STATE/ci/$kind"
    rm -rf "$dir"
    mkdir -p "$dir" || return 1
    out="$(k2ccr_ci evidence "$id" --out "$dir" --reported dist/build-info.json --json 2>"$dir.err")" \
        || { echo "  ✗ $kind run ${id:0:8}: no evidence: $(k2ccr_strip < "$dir.err" | tail -2 | tr '\n' ' ')" >&2; return 1; }
    printf '%s' "$out" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["path"])'
}

# k2ccr_merge <out> <bundle>…: one bundle of the version from each run's
# (nothing in a bundle is trusted for being there: the signer checks every
# receipt, roster and seat itself, so joining them is only bookkeeping).
k2ccr_merge() {
    python3 -I - "$@" <<'PY'
import json, sys
out, paths = sys.argv[1], sys.argv[2:]
bs = [json.load(open(p)) for p in paths]
m = dict(bs[0])
for k in ("project", "version", "controller_fp", "source", "v", "kind"):
    for b in bs[1:]:
        if b[k] != m[k]:
            sys.exit("the runs' evidence differs in %s: %r vs %r" % (k, m[k], b[k]))
def union(key):
    seen, out_ = set(), []
    for b in bs:
        for x in b.get(key) or []:
            j = json.dumps(x, sort_keys=True)
            if j not in seen:
                seen.add(j)
                out_.append(x)
    return out_
for key in ("statements", "rotations", "attestations", "rosters", "seats"):
    m[key] = union(key)
# Bundle v3: the optional, reserved roster_approvals as a union too.
if any("roster_approvals" in b for b in bs):
    m["roster_approvals"] = union("roster_approvals")
# v3's provenance (receipt label -> build statement) joins by label like the
# receipts; it is always there in v3, so it stays when empty.
for key in ("receipts", "pairing_pins", "reported", "provenance"):
    d = {}
    for b in bs:
        for k, v in (b.get(key) or {}).items():
            if k in d and d[k] != v:
                sys.exit("the runs' evidence has two %s for %s" % (key, k))
            d[k] = v
    if d or key in ("receipts", "pairing_pins") or (key == "provenance" and any(key in b for b in bs)):
        m[key] = d
m["created_at"] = max(b["created_at"] for b in bs)
with open(out, "w") as f:
    json.dump(m, f, indent=1, sort_keys=True)
    f.write("\n")
PY
}

k2ccr_rehearse() {
    local bundle="$1" log="$2"
    if k2s_run rehearse k2 < "$bundle" > "$log" 2> "$log.err"; then
        return 0
    fi
    k2ccr_strip < "$log.err" | sed 's/^/      /' | tee -a "$K2CR_STATE/summary.txt" >&2
    return 1
}

k2ccr_collect() {
    local x86 arm="" merged="$K2CR_STATE/ci/evidence.json"
    K2CR_X86_OK=0 K2CR_ARM_OK=""
    K2CR_COLLECTED=0
    mkdir -p "$K2CR_STATE/ci"
    rm -f "$merged"
    x86="$(k2ccr_evidence x86)" || { echo "  ✗ the x86_64 run did not pass" | tee -a "$K2CR_STATE/summary.txt" >&2; return 1; }
    if [ -n "$(k2ccr_run_of arm)" ]; then
        K2CR_ARM_OK=0
        arm="$(k2ccr_evidence arm)" || { arm=""; echo "  ✗ the aarch64 run did not pass" | tee -a "$K2CR_STATE/summary.txt" >&2; }
    fi
    if [ -n "$arm" ]; then
        k2ccr_merge "$merged" "$x86" "$arm" || return 1
        if k2ccr_rehearse "$merged" "$K2CR_STATE/rehearse.json"; then
            K2CR_X86_OK=1 K2CR_ARM_OK=1 K2CR_COLLECTED=1
            echo "  ✓ k2-signer rehearse k2: L1, V1, L3, L2 and V2 pass every check (nothing registered or signed)" | tee -a "$K2CR_STATE/summary.txt"
            return 0
        fi
        echo "  ✗ the rehearsal refused the full set; trying x86_64 alone" | tee -a "$K2CR_STATE/summary.txt" >&2
    fi
    cp "$x86" "$merged" || return 1
    if k2ccr_rehearse "$merged" "$K2CR_STATE/rehearse.json"; then
        K2CR_X86_OK=1 K2CR_COLLECTED=1
        echo "  ✓ k2-signer rehearse k2: the x86_64 set passes every check (nothing registered or signed)" | tee -a "$K2CR_STATE/summary.txt"
        [ "$K2CR_ARM_OK" = 0 ] && return 1
        return 0
    fi
    echo "  ✗ k2-signer rehearse k2 refused the x86_64 set" | tee -a "$K2CR_STATE/summary.txt" >&2
    return 1
}

k2ccr_bundle() { printf '%s' "$K2CR_STATE/ci/evidence.json"; }

# The files a label's job built, as `k2 ci evidence` fetched them.
k2ccr_files() {
    local label="$1" kind=x86
    case "$label" in L2|V2) kind=arm ;; esac
    printf '%s' "$K2CR_STATE/ci/$kind/$label/dist"
}

# Step 8.5: the rehearsed files go into <dist-dir>; each must be the
# artifact the label's receipt in the bundle lists (the signer checks
# again at Step 10.5, against what it registered).
k2ccr_stage() {
    local dist="$1" f b arch lab arches="x86_64"
    [ "${K2CR_COLLECTED:-0}" = 1 ] && [ "${K2CR_X86_OK:-}" = 1 ] || { echo "  ✗ stage: nothing rehearsed" >&2; return 1; }
    if [ "${K2CR_ARM_OK:-}" = 1 ] && [ "${K2_COMPUTE_ARM:-compute}" = compute ]; then arches="x86_64 aarch64"; fi
    mkdir -p "$dist" || return 1
    : > "$K2CR_STATE/staged.txt"
    for arch in $arches; do
        lab=L1; [ "$arch" = aarch64 ] && lab=L2
        for b in $K2CR_LINUX_BINS; do
            f="$b-linux-$arch"
            [ -s "$(k2ccr_files $lab)/$f" ] && [ -s "$(k2ccr_files $lab)/$f.sha256" ] || { echo "  ✗ stage: $f missing from $lab" >&2; return 1; }
            cp "$(k2ccr_files $lab)/$f" "$(k2ccr_files $lab)/$f.sha256" "$dist/" || return 1
            printf '%s\n%s\n' "$f" "$f.sha256" >> "$K2CR_STATE/staged.txt"
        done
    done
    local app sums
    app="$(k2ccr_files L3)"
    sums="$app/app-linux.sums"
    [ -s "$sums" ] && [ "$(grep -c . "$sums")" = 3 ] || { echo "  ✗ stage: L3's app-linux.sums is not 3 lines" >&2; return 1; }
    while read -r _ f; do
        [ -s "$app/$f" ] || { echo "  ✗ stage: $f missing from L3" >&2; return 1; }
        cp "$app/$f" "$dist/" || return 1
        echo "$f" >> "$K2CR_STATE/staged.txt"
    done < "$sums"
    cp "$sums" "$dist/SHA256SUMS-linux-x86_64.txt" || return 1
    echo "SHA256SUMS-linux-x86_64.txt" >> "$K2CR_STATE/staged.txt"
    # Every staged binary and package is the artifact its job's receipt lists.
    python3 -I - "$(k2ccr_bundle)" "$dist" "$K2CR_STATE/staged.txt" <<'PY' || { echo "  ✗ stage: a staged file is not its receipt's artifact" >&2; return 1; }
import hashlib, json, os, sys
b, dist, staged = json.load(open(sys.argv[1])), sys.argv[2], sys.argv[3]
arts = {}
for label, r in b["receipts"].items():
    for a in json.loads(r["body"]).get("artifacts") or []:
        if r["role"] == "build":
            arts[os.path.basename(a["name"])] = (label, a["sha256"])
bad = []
for f in open(staged).read().split():
    if f.endswith(".sha256") or f == "SHA256SUMS-linux-x86_64.txt":
        continue
    want = arts.get(f)
    got = hashlib.sha256(open(os.path.join(dist, f), "rb").read()).hexdigest()
    if want is None or want[1] != got:
        bad.append("%s (%s)" % (f, "in no build receipt" if want is None else "not %s's artifact" % want[0]))
if bad:
    sys.exit("staged files that are not their receipt's artifact: " + ", ".join(bad))
PY
    echo "  staged $(grep -c . "$K2CR_STATE/staged.txt") rehearsed files in $dist" | tee -a "$K2CR_STATE/summary.txt"
}

# Release stopped before collect: cancel the runs' jobs nobody will read.
k2ccr_cancel_open() {
    [ -n "${K2CR_STATE:-}" ] && [ -n "$(k2ccr_runs)" ] && [ "${K2CR_COLLECTED:-0}" = 0 ] || return 0
    local id job
    for id in $(k2ccr_runs); do
        for job in $(k2ccr_ci status "$id" --json 2>/dev/null | python3 -I -c '
import json, sys
for c in json.load(sys.stdin)["run"].get("checks") or []:
    if c.get("job") and c.get("state") in ("pending", "running"):
        print(c["job"])' 2>/dev/null); do
            k2cr_k2 compute cancel "$job" --force >/dev/null 2>&1 || true
        done
    done
    echo "  k2 ci release jobs cancelled (release stopped before they were collected)." >&2
}
