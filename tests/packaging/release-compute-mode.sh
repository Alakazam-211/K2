#!/usr/bin/env bash
# release.sh K2_LINUX_BUILDS=compute (stage 1, prd-ci-release-on-compute-
# stage1-v1 §7), stubbed. Never runs release.sh, cargo, gh or k2: the
# functions are cut out of release.sh as written and run against stubs.
#
#  - the pre-flight: compute is the default; when compute isn't ready
#    (OAuth, nodes, the signing user's pins) it falls back to GitHub with a
#    banner (K2_COMPUTE_FALLBACK=0: refused, as before), all of Linux for
#    x86_64, only aarch64 for an aarch64 problem; the repo variable must
#    be settable (repo admin) or already right;
#  - a compute problem falls back to GitHub in compute mode (fatal with
#    K2_COMPUTE_FALLBACK=0, with the re-run lines), a warning in shadow;
#  - Step 7.9c on a resume run submits again in compute mode;
#  - the tree check runs BEFORE the push, then the repo variable is set to
#    what the run did (compute-all, compute or github);
#  - staging: an earlier compute run's files are removed in every mode;
#  - Step 8.5 / 9 / 10.5 / 10.6 wiring (static), and the GitHub workflows'
#    switches.
#
# Run with: bash tests/packaging/release-compute-mode.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
REL="$ROOT/scripts/release.sh"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Every function release.sh defines for compute mode, as written.
python3 - "$REL" > "$tmp/fns.sh" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
names = ["k2_compute_signer_check", "k2_linux_builds_want_var", "k2_linux_builds_get_var", "k2_linux_builds_var_is",
         "k2_linux_builds_can_set_var", "k2_compute_ready", "k2_compute_fallback", "k2_linux_builds_preflight",
         "k2_linux_builds_set_var", "k2_linux_builds_github_part", "k2_shadow_warn", "k2_compute_fatal",
         "k2_compute_problem", "k2_compute_on", "k2_shadow_start", "k2_shadow_collect", "k2_shadow_tree_check",
         "k2_compute_unstage", "k2_compute_stage", "k2_shadow_report"]
for n in names:
    m = re.search(r"^%s\(\) \{\n.*?^\}\n" % re.escape(n), s, re.S | re.M) \
        or re.search(r"^%s\(\) \{ .* \}\n" % re.escape(n), s, re.M)
    if not m:
        sys.exit("release.sh has no function " + n)
    print(m.group(0))
PY

# Stubs: gh variable get/set (the value lives in $STUB_VAR_FILE, seeded from
# $STUB_VAR), gh api repos/... (admin from $STUB_ADMIN, default true), and
# the compute-release lib's entry points (recorded, each answers per $STUB_*).
mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
echo "gh $*" >> "$STUB_CALLS"
if [ "$1 $2 $3" = "variable get LINUX_BUILDS" ]; then
    v="$(cat "$STUB_VAR_FILE" 2>/dev/null || true)"
    [ -n "$v" ] || { echo "variable LINUX_BUILDS was not found" >&2; exit 1; }
    echo "$v"; exit 0
fi
if [ "$1 $2 $3" = "variable set LINUX_BUILDS" ]; then
    [ "${STUB_ADMIN:-true}" = true ] || { echo "HTTP 403" >&2; exit 1; }
    printf '%s' "$5" > "$STUB_VAR_FILE"; exit 0
fi
if [ "$1" = api ] && [ "$2" = "repos/Alakazam-211/K2" ]; then echo "${STUB_ADMIN:-true}"; exit 0; fi
exit 2
STUB
chmod +x "$tmp/bin/gh"
cat > "$tmp/lib.sh" <<'LIB'
k2cr_verify_node() { printf '%s' "${STUB_VERIFY-k2n9-vin}"; }
k2cr_x86_node() { printf '%s' "${STUB_X86-k2n7-vin}"; }
k2cr_arm_on() { [ "${K2_COMPUTE_ARM:-compute}" = compute ] && [ -n "${STUB_ARM-k2n10-vin}" ]; }
k2cr_arm_node() { k2cr_arm_on && printf '%s' "${STUB_ARM-k2n10-vin}"; return 0; }
k2cr_arm_verify_node() { k2cr_arm_on && printf '%s' "${STUB_ARMV-k2n11-vin}"; return 0; }
k2cr_check_arm_nodes() { echo "arm-nodes" >> "$STUB_CALLS"; [ "$(k2cr_arm_node)" != "$(k2cr_arm_verify_node)" ] || { echo "  ✗ L2 and V2 are both on $(k2cr_arm_node)" >&2; return 1; }; }
k2cr_init() { echo "k2cr_init $*" >> "$STUB_CALLS"; K2CR_TREE="${STUB_TREE:-t1}"; K2CR_STATE="$3"; mkdir -p "$3"; : > "$3/jobs.tsv"; echo "L1	j" > "$3/jobs.tsv"; }
k2cr_submit_release_set() { echo "submit arm=${K2_COMPUTE_ARM:-}" >> "$STUB_CALLS"; K2CR_ARM_SUBMIT_OK="${STUB_ARM_SUBMIT_OK:-1}"; k2cr_arm_on || K2CR_ARM_SUBMIT_OK=""; [ "${STUB_SUBMIT_FAIL:-0}" = 0 ]; }
k2cr_wait() { echo "wait" >> "$STUB_CALLS"; [ "${STUB_WAIT_FAIL:-0}" = 0 ]; }
k2cr_collect() {
    echo "collect" >> "$STUB_CALLS"
    K2CR_COLLECTED=1
    K2CR_X86_OK=1; [ "${STUB_COLLECT_FAIL:-0}" = 0 ] || K2CR_X86_OK=0
    K2CR_ARM_OK=""; k2cr_arm_on && K2CR_ARM_OK="${STUB_ARM_OK:-1}"
    [ "${STUB_NO_V1:-0}" = 1 ] || echo '{"ok":true}' > "$K2_SHADOW_STATE/V1.verdict.json"
    [ "$K2CR_X86_OK" = 1 ] && [ "${K2CR_ARM_OK:-1}" = 1 ]
}
k2cr_stage() { echo "k2cr_stage $* arm=${K2_COMPUTE_ARM:-}" >> "$STUB_CALLS"; mkdir -p "$1"; printf 'k2-node-linux-x86_64\nK2_9.9.9_amd64.deb\n' > "$K2_SHADOW_STATE/staged.txt"; : > "$1/k2-node-linux-x86_64"; : > "$1/K2_9.9.9_amd64.deb"; }
k2cr_cancel_open() { echo "cancel-open" >> "$STUB_CALLS"; }
k2cr_cancel_arm() { echo "cancel-arm" >> "$STUB_CALLS"; }
k2cr_job_times() { :; }
k2_release_preview_commit() { echo "c0ffee"; }
# The signing user's compute pins (k2sign compute-pins), per $STUB_PINS.
k2u_compute_pins() { echo "pins" >> "$STUB_CALLS"; [ "${STUB_PINS_FAIL:-0}" = 0 ] || return 1; if [ -n "${STUB_PINS:-}" ]; then printf '%s\n' "$STUB_PINS"; else echo '{"controller": null, "nodes": []}'; fi; }
cargo() { return 0; }
LIB

# run <name> <snippet>: a bash with the functions + stubs; echoes rc. The
# snippet's last word on the mode is printed as "MODE <linux> <arm>" by
# `mode` (defined below, inside the run).
run() {
    local name="$1" snippet="$2" rc=0
    export STUB_CALLS="$tmp/calls-$name" STUB_VAR_FILE="$tmp/var-$name"
    : > "$STUB_CALLS"
    printf '%s' "${STUB_VAR:-}" > "$STUB_VAR_FILE"
    PATH="$tmp/bin:$PATH" RELEASE_REPO="Alakazam-211/K2" VERSION="9.9.9" PROJECT_DIR="$tmp" K2_SIGNER="${STUB_SIGNER:-keychain}" \
        K2_RELEASE_AUDIT_DIR="$tmp/audit-$name" DIST_DIR="$tmp/dist-$name" \
        bash -c 'set -euo pipefail; K2_SHADOW_STATE=""; K2_SHADOW_NOTE=""; K2_COMPUTE_FELL_BACK=""; K2_LINUX_BUILDS="${K2_LINUX_BUILDS:-compute}"; K2_COMPUTE_ARM="${K2_COMPUTE_ARM:-compute}"
            mode() { echo "MODE $K2_LINUX_BUILDS $K2_COMPUTE_ARM"; }
            source "$1"; source "$2"; eval "$3"' \
        _ "$tmp/lib.sh" "$tmp/fns.sh" "$snippet" > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}
has() { grep -q -- "$2" "$tmp/out-$1"; }
PINS_ALL='{"controller": "c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7", "nodes": [{"name": "k2n7-vin", "fp": "aa"}, {"name": "k2n9-vin", "fp": "bb"}, {"name": "k2n10-vin", "fp": "cc"}, {"name": "k2n11-vin", "fp": "dd"}]}'
PINS_X86='{"controller": "c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7", "nodes": [{"name": "k2n7-vin", "fp": "aa"}, {"name": "k2n9-vin", "fp": "bb"}]}'

echo "pre-flight: compute is the default; GitHub is the fallback, said loudly:"
rc="$(run pf-default 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-default "MODE compute compute" && has pf-default "Linux x86_64 and aarch64: K2 Compute" && ! has pf-default "FALL BACK" \
    && pass "no K2_LINUX_BUILDS: compute for x86_64 and aarch64 (keychain signer, admin gh)" || fail "default" "rc=$rc $(cat "$tmp/out-pf-default")"
python3 - "$REL" <<'PY' && pass "release.sh: K2_LINUX_BUILDS defaults to compute, K2_COMPUTE_ARM to compute" || fail "defaults" "see release.sh"
import sys
s = open(sys.argv[1]).read()
assert 'K2_LINUX_BUILDS="${K2_LINUX_BUILDS:-compute}"' in s
assert 'K2_COMPUTE_ARM="${K2_COMPUTE_ARM:-compute}"' in s
PY
rc="$(K2_COMPUTE_OAUTH=placeholder run pf-placeholder 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-placeholder "MODE github github" && has pf-placeholder "K2 COMPUTE FAILED: Linux x86_64 AND aarch64 FALL BACK TO GITHUB" \
    && has pf-placeholder "must never ship" && has pf-placeholder "Linux: GitHub" \
    && pass "placeholder OAuth: all of Linux falls back to GitHub, with the banner and the reason" || fail "placeholder fallback" "rc=$rc $(cat "$tmp/out-pf-placeholder")"
rc="$(K2_COMPUTE_OAUTH=placeholder K2_COMPUTE_FALLBACK=0 run pf-placeholder0 'k2_linux_builds_preflight; mode')"
[ "$rc" = 1 ] && has pf-placeholder0 "must never ship" && has pf-placeholder0 "K2_COMPUTE_FALLBACK=0" && ! has pf-placeholder0 MODE \
    && pass "K2_COMPUTE_FALLBACK=0: refused instead" || fail "placeholder no fallback" "rc=$rc $(cat "$tmp/out-pf-placeholder0")"
rc="$(K2_LINUX_BUILDS=github run pf-gh 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-gh "MODE github" && has pf-gh "Linux: GitHub (K2_LINUX_BUILDS=github)" && ! grep -q pins "$tmp/calls-pf-gh" \
    && pass "K2_LINUX_BUILDS=github: GitHub on purpose, no compute checks" || fail "github mode" "rc=$rc $(cat "$tmp/out-pf-gh")"

echo "pre-flight: the repo variable (set before the tag push; needs repo admin or the right value already):"
rc="$(STUB_ADMIN=false STUB_VAR=compute-all run pf-var-ok 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-var-ok "MODE compute compute" && pass "no admin, but the variable already says compute-all: fine" || fail "var already right" "rc=$rc $(cat "$tmp/out-pf-var-ok")"
rc="$(STUB_ADMIN=false STUB_VAR=github run pf-var-fb 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-var-fb "MODE github github" && has pf-var-fb "can't set the repo variable LINUX_BUILDS to compute-all" \
    && pass "no admin, the variable says github: falls back to GitHub (which needs no change)" || fail "var fallback" "rc=$rc $(cat "$tmp/out-pf-var-fb")"
rc="$(STUB_ADMIN=false STUB_VAR=compute-all K2_LINUX_BUILDS=github run pf-var-stuck 'k2_linux_builds_preflight; mode')"
[ "$rc" = 1 ] && has pf-var-stuck "gh variable set LINUX_BUILDS -b github --repo Alakazam-211/K2" && ! has pf-var-stuck MODE \
    && pass "github mode, the variable on compute-all and no admin: refused (GitHub would build nothing), with the command" \
    || fail "var stuck" "rc=$rc $(cat "$tmp/out-pf-var-stuck")"
rc="$(STUB_VAR=compute-all K2_LINUX_BUILDS=shadow run pf-var-sh 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && ! grep -q "variable set" "$tmp/calls-pf-var-sh" && pass "the pre-flight never sets the variable (only checks it can)" \
    || fail "pre-flight set var" "$(cat "$tmp/calls-pf-var-sh")"

echo "pre-flight under the signing user (K2_SIGNER=k2sign, S3b): every release node and the controller pinned:"
rc="$(STUB_SIGNER=k2sign STUB_PINS="$PINS_ALL" run pf-k2sign 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-k2sign "MODE compute compute" && has pf-k2sign "Signing user: compute builds from L1 k2n7-vin and V1 k2n9-vin" \
    && has pf-k2sign "Signing user: compute builds from L2 k2n10-vin and V2 k2n11-vin" \
    && pass "compute under k2sign: passes once the signer pinned all four nodes and the controller" || fail "k2sign pre-flight" "rc=$rc $(cat "$tmp/out-pf-k2sign")"
rc="$(STUB_SIGNER=k2sign STUB_PINS="$PINS_X86" run pf-k2sign-x86 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-k2sign-x86 "MODE compute github" && has pf-k2sign-x86 "K2 COMPUTE FAILED: Linux aarch64 FALL BACK TO GITHUB" \
    && has pf-k2sign-x86 "L2's node k2n10-vin is not pinned" && has pf-k2sign-x86 "Linux x86_64: K2 Compute; aarch64 and Arch: GitHub" \
    && pass "the aarch64 nodes not pinned: only aarch64 falls back; x86_64 stays on compute" || fail "k2sign arm pins" "rc=$rc $(cat "$tmp/out-pf-k2sign-x86")"
rc="$(STUB_SIGNER=k2sign run pf-k2sign-none 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-k2sign-none "MODE github github" && has pf-k2sign-none "no controller pinned" && has pf-k2sign-none "FALL BACK TO GITHUB" \
    && pass "nothing pinned: all of Linux falls back, with each pin command shown" || fail "k2sign none fallback" "rc=$rc $(cat "$tmp/out-pf-k2sign-none")"
export K2_COMPUTE_FALLBACK=0
rc="$(STUB_SIGNER=k2sign run pf-k2sign-none0 k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-k2sign-none0 "no controller pinned" && has pf-k2sign-none0 "L1's node k2n7-vin is not pinned" \
    && has pf-k2sign-none0 "V1's node k2n9-vin is not pinned" \
    && pass "K2_COMPUTE_FALLBACK=0, nothing pinned: refused before any build, with each pin command" \
    || fail "k2sign no pins" "rc=$rc $(cat "$tmp/out-pf-k2sign-none0")"
rc="$(STUB_SIGNER=k2sign STUB_PINS='{"controller": "c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7", "nodes": [{"name": "k2n7-vin", "fp": "aa"}]}' run pf-k2sign-v1 k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-k2sign-v1 "V1's node k2n9-vin is not pinned" && ! has pf-k2sign-v1 "L1's node" \
    && pass "K2_COMPUTE_FALLBACK=0, the V1 node not pinned: refused" || fail "k2sign V1 pin" "rc=$rc $(cat "$tmp/out-pf-k2sign-v1")"
rc="$(STUB_SIGNER=k2sign STUB_PINS="$PINS_ALL" STUB_X86=k2n9-vin run pf-k2sign-same k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-k2sign-same "L1 and V1 are both on k2n9-vin" && ! grep -q pins "$tmp/calls-pf-k2sign-same" \
    && pass "K2_COMPUTE_FALLBACK=0, L1 on V1's node: refused before any build" || fail "k2sign same node" "rc=$rc $(cat "$tmp/out-pf-k2sign-same")"
rc="$(STUB_X86=k2n9-vin run pf-same k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-same "L1 and V1 are both on k2n9-vin" \
    && pass "K2_COMPUTE_FALLBACK=0, L1 on V1's node: refused whoever signs (keychain too)" || fail "same node keychain" "rc=$rc $(cat "$tmp/out-pf-same")"
rc="$(STUB_SIGNER=k2sign STUB_PINS="$PINS_ALL" STUB_ARMV=k2n10-vin run pf-armsame k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-armsame "L2 and V2 are both on k2n10-vin" && grep -q arm-nodes "$tmp/calls-pf-armsame" \
    && pass "K2_COMPUTE_FALLBACK=0, L2 on V2's node: refused" || fail "arm same node" "rc=$rc $(cat "$tmp/out-pf-armsame")"
rc="$(STUB_SIGNER=k2sign STUB_PINS="$PINS_ALL" STUB_VERIFY='' run pf-k2sign-nov1 k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-k2sign-nov1 "signs a compute-built" && ! grep -q pins "$tmp/calls-pf-k2sign-nov1" \
    && pass "K2_COMPUTE_FALLBACK=0, k2sign without V1: refused (the signer needs the dual build)" \
    || fail "k2sign no V1" "rc=$rc $(cat "$tmp/out-pf-k2sign-nov1")"
rc="$(STUB_SIGNER=k2sign STUB_PINS="$PINS_ALL" K2_COMPUTE_OAUTH=env run pf-k2sign-env k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-k2sign-env "only from" && has pf-k2sign-env "secret jobs" \
    && pass "K2_COMPUTE_FALLBACK=0, k2sign with K2_COMPUTE_OAUTH=env: refused (the signer takes only secret jobs)" || fail "k2sign env mode" "rc=$rc $(cat "$tmp/out-pf-k2sign-env")"
rc="$(STUB_SIGNER=k2sign STUB_PINS_FAIL=1 run pf-k2sign-old k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-k2sign-old "can't list its K2 Compute pins" \
    && pass "K2_COMPUTE_FALLBACK=0, an older signer (no compute-pins verb): refused, re-install from this commit" || fail "k2sign old signer" "rc=$rc $(cat "$tmp/out-pf-k2sign-old")"
rc="$(K2_COMPUTE_EXCLUSIVE=0 run pf-noexcl k2_linux_builds_preflight)"
[ "$rc" = 1 ] && has pf-noexcl "needs the release builds --exclusive" \
    && pass "K2_COMPUTE_FALLBACK=0, K2_COMPUTE_EXCLUSIVE=0: refused at the pre-flight" || fail "compute no exclusive" "rc=$rc $(cat "$tmp/out-pf-noexcl")"
rc="$(STUB_SIGNER=k2sign STUB_PINS="$PINS_ALL" STUB_COLLECT_FAIL=1 \
    run pf-k2sign-bad 'k2_linux_builds_preflight; k2_shadow_start; k2_shadow_collect; echo AFTER')"
[ "$rc" = 1 ] && has pf-k2sign-bad "Signing user: compute builds" \
    && has pf-k2sign-bad "FATAL (K2 Compute, stage 1): the x86_64 builds did not all verify" && ! has pf-k2sign-bad AFTER \
    && pass "K2_COMPUTE_FALLBACK=0: through the pre-flight, then stopped at a bad verdict (Step 7.9c, before the tag)" \
    || fail "k2sign bad verdict" "rc=$rc $(cat "$tmp/out-pf-k2sign-bad")"
unset K2_COMPUTE_FALLBACK
rc="$(K2_LINUX_BUILDS=shadow STUB_SIGNER=k2sign run pf-k2sign-sh k2_linux_builds_preflight)"
[ "$rc" = 0 ] && pass "shadow under the signing user: fine (nothing from compute is signed)" || fail "k2sign shadow" "rc=$rc $(cat "$tmp/out-pf-k2sign-sh")"
rc="$(STUB_VERIFY='' run pf-nov1 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has pf-nov1 "NO dual build" && has pf-nov1 "MODE compute" && pass "compute (keychain) without V1: loud warning" || fail "no V1 warning" "rc=$rc $(cat "$tmp/out-pf-nov1")"
python3 - "$REL" <<'PY' && pass "the pre-flight runs right after it is defined (before any build)" || fail "pre-flight call" "not called or called late"
import sys
s = open(sys.argv[1]).read()
d = s.index("k2_linux_builds_preflight() {")
c = s.index("\nk2_linux_builds_preflight\n")
assert d < c < s.index("# ── Step 0.4"), "order"
PY

echo "a compute problem: GitHub instead in compute mode (fatal with K2_COMPUTE_FALLBACK=0), a warning in shadow:"
rc="$(run prob-fb 'k2_compute_problem "boom"; echo AFTER; mode')"
[ "$rc" = 0 ] && has prob-fb "FALL BACK TO GITHUB" && has prob-fb "why: boom" && has prob-fb AFTER && has prob-fb "MODE github github" \
    && grep -q cancel-open "$tmp/calls-prob-fb" && pass "compute: GitHub builds all of Linux, open jobs cancelled, the release goes on" \
    || fail "compute fallback" "rc=$rc $(cat "$tmp/out-prob-fb")"
rc="$(K2_COMPUTE_FALLBACK=0 run prob-c 'k2_compute_problem "boom"; echo AFTER')"
[ "$rc" = 1 ] && has prob-c "FATAL (K2 Compute, stage 1): boom" && has prob-c "nothing was pushed, tagged or published" \
    && has prob-c "K2_LINUX_BUILDS=github ./scripts/release.sh 9.9.9" && ! has prob-c "RESUME" \
    && ! has prob-c AFTER && pass "K2_COMPUTE_FALLBACK=0, before the Mac build: exit 1, with a plain re-run" || fail "compute fatal" "rc=$rc $(cat "$tmp/out-prob-c")"
mkdir -p "$tmp/target/release/bundle/dmg"; : > "$tmp/target/release/bundle/dmg/K2_9.9.9_aarch64.dmg"
rc="$(K2_COMPUTE_FALLBACK=0 run prob-c2 'k2_compute_problem "boom"')"
[ "$rc" = 1 ] && has prob-c2 "K2_LINUX_BUILDS=github K2_RELEASE_RESUME=8 ./scripts/release.sh 9.9.9" \
    && pass "K2_COMPUTE_FALLBACK=0, after the Mac build: the re-run reuses it (K2_RELEASE_RESUME=8)" || fail "compute fatal resume" "rc=$rc $(cat "$tmp/out-prob-c2")"
rm -rf "$tmp/target"
rc="$(K2_LINUX_BUILDS=shadow run prob-s 'k2_compute_problem "boom"; echo "AFTER $K2_SHADOW_NOTE"')"
[ "$rc" = 0 ] && has prob-s "AFTER boom" && pass "shadow: a warning, the release goes on" || fail "shadow warn" "rc=$rc $(cat "$tmp/out-prob-s")"
rc="$(run prob-arm 'k2_compute_fallback arm "arm boom"; mode; echo "NOTE $K2_COMPUTE_FELL_BACK"')"
[ "$rc" = 0 ] && has prob-arm "MODE compute github" && has prob-arm "Linux aarch64 FALL BACK TO GITHUB" && has prob-arm "NOTE Linux aarch64: arm boom" \
    && grep -q cancel-arm "$tmp/calls-prob-arm" && ! grep -q cancel-open "$tmp/calls-prob-arm" \
    && pass "an aarch64 problem: only aarch64 goes to GitHub (its jobs cancelled), recorded for the summary" || fail "arm fallback" "rc=$rc $(cat "$tmp/out-prob-arm")"

echo "Steps 1.6c and 7.9c:"
rc="$(K2_LINUX_BUILDS=github run s-gh 'k2_shadow_start; k2_shadow_collect; echo DONE')"
[ "$rc" = 0 ] && [ ! -s "$tmp/calls-s-gh" ] && pass "github mode: no compute call at all" || fail "github" "$(cat "$tmp/calls-s-gh")"
rc="$(STUB_SUBMIT_FAIL=1 run s-subfail 'k2_shadow_start; echo AFTER; mode')"
[ "$rc" = 0 ] && has s-subfail AFTER && has s-subfail "MODE github github" && has s-subfail "not every compute job was submitted" \
    && pass "compute: a failed submit sends Linux to GitHub at 1.6c, the release goes on" || fail "submit fail" "rc=$rc $(cat "$tmp/out-s-subfail")"
rc="$(K2_COMPUTE_FALLBACK=0 STUB_SUBMIT_FAIL=1 run s-subfail0 'k2_shadow_start; echo AFTER')"
[ "$rc" = 1 ] && ! has s-subfail0 AFTER && pass "K2_COMPUTE_FALLBACK=0: a failed submit stops the release at 1.6c" || fail "submit fail stop" "rc=$rc $(cat "$tmp/out-s-subfail0")"
rc="$(STUB_ARM_SUBMIT_OK=0 run s-armsub 'k2_shadow_start; echo AFTER; mode')"
[ "$rc" = 0 ] && has s-armsub "MODE compute github" && has s-armsub "aarch64 jobs (L2, V2) were not submitted" \
    && pass "compute: the aarch64 jobs not submitted: only aarch64 goes to GitHub" || fail "arm submit" "rc=$rc $(cat "$tmp/out-s-armsub")"
rc="$(K2_LINUX_BUILDS=shadow STUB_SUBMIT_FAIL=1 run s-subfail-sh 'k2_shadow_start; echo AFTER')"
[ "$rc" = 0 ] && has s-subfail-sh AFTER && pass "shadow: a failed submit only warns" || fail "shadow submit" "rc=$rc"
rc="$(STUB_COLLECT_FAIL=1 run s-colfail 'k2_shadow_start; k2_shadow_collect; echo AFTER; mode')"
[ "$rc" = 0 ] && has s-colfail AFTER && has s-colfail "MODE github github" && has s-colfail "x86_64 builds did not all verify" \
    && pass "compute: an unverified x86_64 build sends Linux to GitHub at 7.9c (before the tag)" || fail "collect fail" "rc=$rc $(cat "$tmp/out-s-colfail")"
rc="$(STUB_ARM_OK=0 run s-armfail 'k2_shadow_start; k2_shadow_collect; echo AFTER; mode')"
[ "$rc" = 0 ] && has s-armfail "MODE compute github" && has s-armfail "aarch64 builds (L2, V2) did not all verify" \
    && pass "compute: an unverified aarch64 build: only aarch64 goes to GitHub, x86_64 stays" || fail "arm collect fail" "rc=$rc $(cat "$tmp/out-s-armfail")"
rc="$(STUB_WAIT_FAIL=1 run s-waitfail 'k2_shadow_start; k2_shadow_collect; echo AFTER; mode')"
[ "$rc" = 0 ] && has s-waitfail "MODE github github" && has s-waitfail "jobs not done in 60 min" \
    && pass "compute: jobs not done in K2_COMPUTE_WAIT_MIN: GitHub instead" || fail "wait fail" "rc=$rc $(cat "$tmp/out-s-waitfail")"
rc="$(STUB_NO_V1=1 run s-nov1 'k2_shadow_start; k2_shadow_collect; echo AFTER; mode')"
[ "$rc" = 0 ] && has s-nov1 "dual build didn't run" && has s-nov1 "MODE github" && pass "compute: no V1 verdict sends Linux to GitHub" || fail "no V1" "rc=$rc $(cat "$tmp/out-s-nov1")"
rc="$(run s-resume 'k2_shadow_collect; echo AFTER')"
[ "$rc" = 0 ] && grep -q "^submit" "$tmp/calls-s-resume" && grep -q "^collect$" "$tmp/calls-s-resume" && has s-resume "submitting them now" \
    && pass "compute resume (no jobs yet): 7.9c submits, then waits and collects" || fail "resume" "rc=$rc $(cat "$tmp/out-s-resume" "$tmp/calls-s-resume")"
rc="$(K2_LINUX_BUILDS=shadow run s-resume-sh 'k2_shadow_collect; echo AFTER')"
[ "$rc" = 0 ] && ! grep -q "^submit" "$tmp/calls-s-resume-sh" && pass "shadow resume: warns, submits nothing" || fail "shadow resume" "$(cat "$tmp/calls-s-resume-sh")"

echo "the tree check and the repo variable (before the push):"
repo="$tmp/repo"; git init -q "$repo"; git -C "$repo" -c user.email=t@t -c user.name=t commit -q --allow-empty -m r
T="$(git -C "$repo" rev-parse 'HEAD^{tree}')"
rc="$(cd "$repo" && STUB_VAR=github STUB_TREE="$T" run t-ok 'k2_shadow_start; k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 0 ] && has t-ok "is the tree K2 Compute built" && [ "$(cat "$tmp/var-t-ok")" = compute-all ] && has t-ok "LINUX_BUILDS set to compute-all" \
    && pass "same tree: passes, and the variable becomes compute-all (GitHub builds no Linux binaries)" || fail "tree ok" "rc=$rc $(cat "$tmp/out-t-ok")"
rc="$(cd "$repo" && STUB_VAR=github STUB_TREE=other run t-bad 'k2_shadow_start; k2_shadow_tree_check; echo AFTER; mode')"
[ "$rc" = 0 ] && has t-bad AFTER && has t-bad "MODE github github" && [ "$(cat "$tmp/var-t-bad")" = github ] && ! grep -q "variable set" "$tmp/calls-t-bad" \
    && pass "compute, another tree: GitHub builds Linux, the variable stays github" || fail "tree bad" "rc=$rc $(cat "$tmp/out-t-bad")"
rc="$(cd "$repo" && K2_COMPUTE_FALLBACK=0 STUB_TREE=other run t-bad0 'k2_shadow_start; k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 1 ] && ! has t-bad0 AFTER && pass "K2_COMPUTE_FALLBACK=0, another tree: stops" || fail "tree bad stop" "rc=$rc $(cat "$tmp/out-t-bad0")"
[ "$(git -C "$repo" rev-list --count HEAD)" = 1 ] && pass "the tree check never rewrites git history" || fail "history" "commits changed"
rc="$(cd "$repo" && STUB_VAR=compute-all STUB_TREE="$T" run t-armfb 'k2_shadow_start; k2_compute_fallback arm x; k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 0 ] && [ "$(cat "$tmp/var-t-armfb")" = compute ] && has t-armfb "GitHub builds Linux aarch64 and Arch" \
    && pass "aarch64 fell back: the variable becomes compute (GitHub builds aarch64)" || fail "var compute" "rc=$rc $(cat "$tmp/out-t-armfb")"
rc="$(cd "$repo" && STUB_VAR=compute-all K2_LINUX_BUILDS=github run t-gh 'k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 0 ] && [ "$(cat "$tmp/var-t-gh")" = github ] && pass "github mode with a leftover compute-all: the variable is put back to github" \
    || fail "var github" "rc=$rc $(cat "$tmp/out-t-gh")"
rc="$(cd "$repo" && STUB_VAR=compute-all STUB_ADMIN=false K2_LINUX_BUILDS=github run t-noadmin 'k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 1 ] && ! has t-noadmin AFTER && has t-noadmin "nothing was pushed or tagged" && has t-noadmin "gh variable set LINUX_BUILDS -b github" \
    && pass "the variable can't be set: stop before the push, with the command" || fail "var set fails" "rc=$rc $(cat "$tmp/out-t-noadmin")"
rc="$(cd "$repo" && K2_LINUX_BUILDS=shadow STUB_TREE=other run t-bad-sh 'k2_shadow_start; k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 0 ] && has t-bad-sh AFTER && pass "shadow, another tree: a warning" || fail "tree shadow" "rc=$rc"
python3 - "$REL" <<'PY' && pass "release.sh checks the tree (and sets the variable) after the commit and BEFORE pushing main and the tag" || fail "tree order" "see release.sh Step 8.9"
import sys
s = open(sys.argv[1]).read()
commit = s.index('git commit -m "chore(release): ${TAG}"')
check = s.index("\nk2_shadow_tree_check\n")
push = s.index("git push origin HEAD:main")
assert commit < check < push, (commit, check, push)
assert s.count("\nk2_shadow_tree_check\n") == 1
body = s[s.index("k2_shadow_tree_check() {"):]
body = body[:body.index("\n}\n")]
assert "k2_linux_builds_set_var" in body
PY

echo "staging (Step 8.5):"
mkdir -p "$tmp/dist-st-gh"
printf 'k2-node-linux-x86_64\nK2_9.9.9_amd64.deb\n../escape\n' > "$tmp/dist-st-gh/.compute-staged"
: > "$tmp/dist-st-gh/k2-node-linux-x86_64"; : > "$tmp/dist-st-gh/K2_9.9.9_amd64.deb"; : > "$tmp/dist-st-gh/k2-daemon-macos-aarch64"; : > "$tmp/escape"
rc="$(K2_LINUX_BUILDS=github run st-gh 'k2_compute_stage')"
[ "$rc" = 0 ] && [ ! -e "$tmp/dist-st-gh/k2-node-linux-x86_64" ] && [ ! -e "$tmp/dist-st-gh/.compute-staged" ] \
    && [ -e "$tmp/dist-st-gh/k2-daemon-macos-aarch64" ] && [ -e "$tmp/escape" ] && ! grep -q k2cr_stage "$tmp/calls-st-gh" \
    && pass "github mode: an earlier compute run's files go (only those, never outside DIST_DIR); nothing staged" \
    || fail "stale cleanup" "rc=$rc $(ls -A "$tmp/dist-st-gh")"
rc="$(run st-c 'k2_shadow_start; k2_shadow_collect; k2_compute_stage')"
[ "$rc" = 0 ] && grep -q "k2cr_stage $tmp/dist-st-c" "$tmp/calls-st-c" && grep -q K2_9.9.9_amd64.deb "$tmp/dist-st-c/.compute-staged" \
    && pass "compute mode: stages into DIST_DIR and records what it staged" || fail "stage" "rc=$rc $(cat "$tmp/out-st-c")"
rc="$(run st-fb 'k2_shadow_start; k2_shadow_collect; k2_compute_stage; k2_compute_problem late; ls -A "$DIST_DIR"')"
[ "$rc" = 0 ] && [ ! -e "$tmp/dist-st-fb/.compute-staged" ] && [ ! -e "$tmp/dist-st-fb/K2_9.9.9_amd64.deb" ] \
    && pass "a fallback after staging takes the staged compute files back out (Step 9 won't upload them)" || fail "unstage on fallback" "rc=$rc $(ls -A "$tmp/dist-st-fb")"

echo "Steps 8.5, 9, 10.5 and 10.6 (release.sh as written):"
python3 - "$REL" > "$tmp/static" <<'PY' && pass "stage → manifest (no Linux key yet) → Step 9 attaches the staged set but both daemons → 10.5 signs them from the stage" || fail "8.5/9/10.5 wiring" "$(cat "$tmp/static")"
import sys
s = open(sys.argv[1]).read()
i_stage = s.index("\nk2_compute_stage\n")
i_manifest = s.index("k2_daemon_manifest \"${VERSION}\"")
i_assets = s.index('done < "$DIST_DIR/.compute-staged"')
i_create = s.index('gh release create "$TAG" "${ASSETS[@]}"')
i_args = s.index('DAEMON_SIGN_ARGS+=(--from-dir "$DIST_DIR" --compute-verdict "$K2_SHADOW_STATE")')
i_sign = s.index('bash "$PROJECT_DIR/scripts/sign-daemon.sh" "${DAEMON_SIGN_ARGS[@]}"')
assert i_stage < i_manifest < i_assets < i_create < i_args < i_sign, "order"
blk = s[s.index('if [ "$K2_LINUX_BUILDS" = "compute" ]; then\n    while IFS= read -r f; do'):i_assets]
assert "k2-daemon-linux-x86_64|k2-daemon-linux-x86_64.sha256|k2-daemon-linux-aarch64|k2-daemon-linux-aarch64.sha256) continue" in blk, \
    "the daemons go up with their signatures at 10.5 (sign-daemon.sh), not at Step 9"
# SS1: a Linux daemon is signed only by sign-daemon.sh (with provenance), never at 8.5.
assert "k2_sign_daemon_raw" not in s and 'k2u_sign u1 "$DIST_DIR/k2-daemon-linux' not in s, "no Linux daemon signing at 8.5"
assert 'merge-daemon-manifest.py" "$DIST_DIR/daemon-latest.json"' not in s, "Linux manifest keys come from sign-daemon.sh at 10.5"
PY
python3 - "$REL" <<'PY' && pass "10.6 in compute mode: checked once, the Arch package not waited for; github mode waits as before" || fail "10.6 wiring" ""
import sys
s = open(sys.argv[1]).read()
i = s.index("# ── Step 10.6")
blk = s[i:s.index("k2_shadow_report\n", i)]
assert 'k2_linux_sums_step "${VERSION}" 0 arch-later' in blk
assert 'k2_linux_sums_step "${VERSION}" "${K2_LINUX_SUMS_WAIT_MIN:-120}"' in blk
assert blk.index('if [ "$K2_LINUX_BUILDS" = compute ]; then') < blk.index("arch-later")
PY
python3 - "$REL" <<'PY' && pass "the final summary names a fallback to GitHub and a pending Arch package" || fail "summary lines" ""
import sys
s = open(sys.argv[1]).read()
tail = s[s.index('echo "  Release ${TAG} complete!"'):]
assert '[ -n "$K2_COMPUTE_FELL_BACK" ] && echo "  ⚠  K2 COMPUTE FELL BACK TO GITHUB' in tail
assert '[ -n "${ARCH_PKG_PENDING:-}" ]' in tail
PY
python3 -c '
import sys; s=open(sys.argv[1]).read(); assert "if k2_compute_on; then\n        k2cr_cancel_open || true" in s' "$REL" \
    && pass "an aborted compute or shadow release cancels its uncollected jobs" || fail "cancel on exit" "exit trap"

echo "GitHub workflows follow the variable:"
python3 - "$ROOT/.github/workflows/daemon-binaries.yml" <<'PY' && pass "daemon-binaries: compute-all tag push → no build job; compute → aarch64 only; otherwise and on dispatch → both" || fail "daemon-binaries matrix" "see the include expression"
import json, re, sys
s = open(sys.argv[1]).read()
m = re.search(r"include: >-\n\s+\$\{\{ fromJSON\(\n\s+\(github\.event_name == 'push' && vars\.LINUX_BUILDS == 'compute'\)\n\s+&& '(\[.*?\])'\n\s+\|\| '(\[.*?\])'\n\s+\) \}\}", s)
assert m, "expression shape"
compute, both = json.loads(m.group(1)), json.loads(m.group(2))
assert [x["target"] for x in compute] == ["aarch64-unknown-linux-gnu"], compute
assert [x["target"] for x in both] == ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"], both
for x in both:
    a = x["target"].split("-")[0]
    assert x == {"target": x["target"], "asset": "k2-daemon-linux-" + a, "root_asset": "k2-root-helper-linux-" + a,
                 "node_asset": "k2-node-linux-" + a, "engine_asset": "k2-app-engine-linux-" + a, "cross": a == "aarch64"}, x
assert compute[0] == both[1]
job = s[s.index("  build-linux-daemon:"):s.index("    strategy:")]
assert "    if: github.event_name != 'push' || vars.LINUX_BUILDS != 'compute-all'\n" in job, job
inst = s[s.index("  upload-mail-helper-installer:"):]
assert "LINUX_BUILDS" not in inst[:inst.index("    steps:")], "the installer job runs in every mode"
PY
grep -q "^    if: github.event_name != 'push' || (vars.LINUX_BUILDS != 'compute' && vars.LINUX_BUILDS != 'compute-all')$" "$ROOT/.github/workflows/app-linux.yml" \
    && pass "app-linux: skips a compute or compute-all tag push, builds on dispatch" || fail "app-linux if" "missing"
python3 - "$ROOT/.github/workflows/checks.yml" <<'PY' && pass "checks.yml: manual dispatch only (the K2 Compute poller runs its steps)" || fail "checks.yml on:" "not workflow_dispatch only"
import re, sys
s = open(sys.argv[1]).read()
m = re.search(r"^on:\n((?:[ #].*\n|\n)*?)(?=^\S)", s, re.M)
assert m, "no on: block"
keys = re.findall(r"^  ([a-z_]+):", m.group(1), re.M)
assert keys == ["workflow_dispatch"], keys
PY

echo "github and shadow with the REAL compute-release.sh, a controller that refuses --exclusive (a share without it, as today):"
# Every new check (two nodes, node_fp, the daemon match, --exclusive) may
# only warn outside compute mode: github and shadow must be as safe as on
# 0.46.9. A stub k2 refuses every --exclusive submit and has no receipts.
mkdir -p "$tmp/bin-real"
cat > "$tmp/bin-real/k2" <<'STUB'
#!/usr/bin/env bash
echo "k2 $*" >> "$STUB_CALLS"
[ "$1" = compute ] || exit 2
case "$2" in
    run)
        case " $* " in *" --exclusive "*) echo "exclusive_not_allowed: this server's share doesn't allow exclusive jobs" >&2; exit 3 ;; esac
        n=$(( $(cat "$STUB_N" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$STUB_N"
        printf '{"job": "job%04d"}\n' "$n" ;;
    wait) exit 0 ;;
    *) echo "stub k2: no compute $2 here" >&2; exit 1 ;;
esac
STUB
chmod +x "$tmp/bin-real/k2"
cat > "$tmp/lib-real.sh" <<LIB
source "$ROOT/scripts/lib/compute-release.sh"
k2_release_preview_commit() { git rev-parse HEAD; }
k2u_compute_pins() { echo "pins" >> "\$STUB_CALLS"; echo '{"controller": null, "nodes": []}'; }
cargo() { return 0; }
LIB
rgit="$tmp/rgit"; git init -q "$rgit"; git -C "$rgit" -c user.email=t@t -c user.name=t commit -q --allow-empty -m r
# run_real <name> <snippet>: like run, with the real lib, in a git repo; echoes rc.
run_real() {
    local name="$1" snippet="$2" rc=0
    export STUB_CALLS="$tmp/calls-$name" STUB_N="$tmp/n-$name" STUB_VAR_FILE="$tmp/var-$name"
    : > "$STUB_CALLS"; rm -f "$STUB_N"; : > "$STUB_VAR_FILE"
    (cd "$rgit" && PATH="$tmp/bin:$PATH" K2_BIN="$tmp/bin-real/k2" RELEASE_REPO="Alakazam-211/K2" VERSION="9.9.9" TAG="v9.9.9" PROJECT_DIR="$tmp/proj-$name" \
        K2_SIGNER="${STUB_SIGNER:-keychain}" K2_RELEASE_AUDIT_DIR="$tmp/audit-$name" DIST_DIR="$tmp/dist-$name" K2_COMPUTE_RUST=1.99.1 K2_COMPUTE_WAIT_MIN=1 \
        bash -c 'set -euo pipefail; K2_SHADOW_STATE=""; K2_SHADOW_NOTE=""; K2_SHADOW_LATE=""; K2_COMPUTE_FELL_BACK=""; K2_LINUX_BUILDS="${K2_LINUX_BUILDS:-compute}"; K2_COMPUTE_ARM="${K2_COMPUTE_ARM:-compute}"; source "$1"; source "$2"; eval "$3"' \
        _ "$tmp/lib-real.sh" "$tmp/fns.sh" "$snippet") > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}
FLOW='k2_linux_builds_preflight; k2_shadow_start; k2_shadow_collect; k2_compute_stage; k2_shadow_report; echo AFTER'
for sg in keychain k2sign; do
    rc="$(STUB_SIGNER="$sg" K2_LINUX_BUILDS=github K2_COMPUTE_NODE=k2n9-vin run_real "real-gh-$sg" "$FLOW")"
    [ "$rc" = 0 ] && grep -q AFTER "$tmp/out-real-gh-$sg" && ! grep -q '^k2 ' "$tmp/calls-real-gh-$sg" && ! grep -q '^pins' "$tmp/calls-real-gh-$sg" \
        && pass "github mode ($sg), even with L1 = V1 set: no compute call, no new check, the release goes on" \
        || fail "github real ($sg)" "rc=$rc $(cat "$tmp/out-real-gh-$sg" "$tmp/calls-real-gh-$sg")"
done
rc="$(K2_LINUX_BUILDS=shadow run_real real-sh-excl "$FLOW")"
[ "$rc" = 0 ] && grep -q AFTER "$tmp/out-real-sh-excl" && grep -q "shadow (K2 Compute): not every compute job was submitted" "$tmp/out-real-sh-excl" \
    && grep -q "k2 compute run k2n8-vin" "$tmp/calls-real-sh-excl" \
    && pass "shadow: --exclusive refused by the share (L1, V1): a warning, L3 still runs, the release goes on" \
    || fail "shadow exclusive refused" "rc=$rc $(cat "$tmp/out-real-sh-excl")"
rc="$(K2_LINUX_BUILDS=shadow K2_COMPUTE_NODE=k2n9-vin run_real real-sh-same "$FLOW")"
[ "$rc" = 0 ] && grep -q AFTER "$tmp/out-real-sh-same" && grep -q "L1 and V1 are both on k2n9-vin" "$tmp/out-real-sh-same" \
    && ! grep -q "compute run" "$tmp/calls-real-sh-same" \
    && pass "shadow: L1 = V1 refused at submit, only a warning; the release goes on" || fail "shadow same node" "rc=$rc $(cat "$tmp/out-real-sh-same")"
rc="$(K2_LINUX_BUILDS=shadow K2_COMPUTE_EXCLUSIVE=0 K2_COMPUTE_ARM=github run_real real-sh-noexcl "$FLOW")"
[ "$rc" = 0 ] && grep -q AFTER "$tmp/out-real-sh-noexcl" && [ "$(grep -c 'k2 compute run' "$tmp/calls-real-sh-noexcl")" = 3 ] \
    && ! grep -q -- '--exclusive' "$tmp/calls-real-sh-noexcl" \
    && grep -q "shadow (K2 Compute): not every compute build verified" "$tmp/out-real-sh-noexcl" \
    && pass "shadow, K2_COMPUTE_EXCLUSIVE=0: all three submitted without --exclusive; a failed collect only warns" \
    || fail "shadow no exclusive" "rc=$rc $(cat "$tmp/out-real-sh-noexcl" "$tmp/calls-real-sh-noexcl")"
rc="$(K2_COMPUTE_EXCLUSIVE=0 run_real real-c-noexcl "$FLOW")"
[ "$rc" = 0 ] && grep -q AFTER "$tmp/out-real-c-noexcl" && grep -q "FALL BACK TO GITHUB" "$tmp/out-real-c-noexcl" && ! grep -q 'k2 compute run' "$tmp/calls-real-c-noexcl" \
    && pass "compute with K2_COMPUTE_EXCLUSIVE=0 (real lib): GitHub instead, nothing submitted" || fail "compute noexcl real" "rc=$rc $(cat "$tmp/out-real-c-noexcl")"

echo "syntax:"
bash -n "$REL" && pass "bash -n release.sh" || fail "bash -n" "release.sh"

echo ""
echo "release-compute-mode: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
