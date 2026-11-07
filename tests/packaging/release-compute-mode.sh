#!/usr/bin/env bash
# release.sh K2_LINUX_BUILDS=compute (stage 1, prd-ci-release-on-compute-
# stage1-v1 §7), stubbed. Never runs release.sh, cargo, gh or k2: the
# functions are cut out of release.sh as written and run against stubs.
#
#  - the pre-flight: the repo variable LINUX_BUILDS must agree with the
#    mode (both ways), compute needs secret/env OAuth;
#  - a compute problem is fatal in compute mode (with the fallback lines),
#    a warning in shadow;
#  - Step 7.9c on a resume run submits again in compute mode;
#  - the tree check runs BEFORE the push, and stops a compute release;
#  - staging: an earlier compute run's files are removed in every mode;
#  - Step 8.5 / 9 wiring (static), and the GitHub workflows' switches.
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
names = ["k2_compute_signer_check", "k2_linux_builds_preflight", "k2_shadow_warn", "k2_compute_problem", "k2_compute_on",
         "k2_shadow_start", "k2_shadow_collect", "k2_shadow_tree_check", "k2_compute_stage", "k2_shadow_report"]
for n in names:
    m = re.search(r"^%s\(\) \{\n.*?^\}\n" % re.escape(n), s, re.S | re.M) \
        or re.search(r"^%s\(\) \{ .* \}\n" % re.escape(n), s, re.M)
    if not m:
        sys.exit("release.sh has no function " + n)
    print(m.group(0))
PY

# Stubs: gh variable get (from $STUB_VAR), and the compute-release lib's
# entry points (recorded, each answers per $STUB_*).
mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
echo "gh $*" >> "$STUB_CALLS"
if [ "$1 $2 $3" = "variable get LINUX_BUILDS" ]; then
    [ -n "${STUB_VAR:-}" ] || { echo "variable LINUX_BUILDS was not found" >&2; exit 1; }
    echo "$STUB_VAR"; exit 0
fi
exit 2
STUB
chmod +x "$tmp/bin/gh"
cat > "$tmp/lib.sh" <<'LIB'
k2cr_verify_node() { printf '%s' "${STUB_VERIFY-k2n9-vin}"; }
k2cr_x86_node() { printf '%s' "${STUB_X86-k2n7-vin}"; }
k2cr_check_dual_nodes() { echo "dual-nodes" >> "$STUB_CALLS"; [ -z "$(k2cr_verify_node)" ] || [ "$(k2cr_x86_node)" != "$(k2cr_verify_node)" ] || { echo "  ✗ L1 and V1 are both on $(k2cr_x86_node)" >&2; return 1; }; }
k2cr_init() { echo "k2cr_init $*" >> "$STUB_CALLS"; K2CR_TREE="${STUB_TREE:-t1}"; mkdir -p "$3"; : > "$3/jobs.tsv"; echo "L1	j" > "$3/jobs.tsv"; }
k2cr_submit_release_set() { echo "submit" >> "$STUB_CALLS"; [ "${STUB_SUBMIT_FAIL:-0}" = 0 ]; }
k2cr_wait() { echo "wait" >> "$STUB_CALLS"; return 0; }
k2cr_collect() { echo "collect" >> "$STUB_CALLS"; [ "${STUB_COLLECT_FAIL:-0}" = 0 ] || return 1; [ "${STUB_NO_V1:-0}" = 1 ] || echo '{"ok":true}' > "$K2_SHADOW_STATE/V1.verdict.json"; }
k2cr_stage() { echo "k2cr_stage $*" >> "$STUB_CALLS"; mkdir -p "$1"; printf 'k2-node-linux-x86_64\nK2_9.9.9_amd64.deb\n' > "$K2_SHADOW_STATE/staged.txt"; : > "$1/k2-node-linux-x86_64"; : > "$1/K2_9.9.9_amd64.deb"; }
k2_release_preview_commit() { echo "c0ffee"; }
# The signing user's compute pins (k2sign compute-pins), per $STUB_PINS.
k2u_compute_pins() { echo "pins" >> "$STUB_CALLS"; [ "${STUB_PINS_FAIL:-0}" = 0 ] || return 1; if [ -n "${STUB_PINS:-}" ]; then printf '%s\n' "$STUB_PINS"; else echo '{"controller": null, "nodes": []}'; fi; }
cargo() { return 0; }
LIB

# run <name> <snippet>: a bash with the functions + stubs; echoes rc.
run() {
    local name="$1" snippet="$2" rc=0
    export STUB_CALLS="$tmp/calls-$name"
    : > "$STUB_CALLS"
    PATH="$tmp/bin:$PATH" RELEASE_REPO="Alakazam-211/K2" VERSION="9.9.9" PROJECT_DIR="$tmp" K2_SIGNER="${STUB_SIGNER:-keychain}" \
        K2_RELEASE_AUDIT_DIR="$tmp/audit-$name" DIST_DIR="$tmp/dist-$name" \
        bash -c 'set -euo pipefail; K2_SHADOW_STATE=""; K2_SHADOW_NOTE=""; source "$1"; source "$2"; eval "$3"' \
        _ "$tmp/lib.sh" "$tmp/fns.sh" "$snippet" > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}

echo "pre-flight: the repo variable must agree with the mode:"
for row in "compute compute 0" "compute github 1" "compute - 1" "github compute 1" "shadow compute 1" "github - 0" "shadow github 0"; do
    set -- $row
    mode="$1" var="$2" want="$3"
    [ "$var" = - ] && var=""
    rc="$(STUB_VAR="$var" K2_LINUX_BUILDS="$mode" run "pf-$mode-${var:-unset}" k2_linux_builds_preflight)"
    if [ "$rc" = "$want" ]; then
        pass "mode $mode, variable ${var:-unset} → exit $rc"
    else
        fail "mode $mode, variable ${var:-unset}" "rc=$rc want $want: $(cat "$tmp/out-pf-$mode-${var:-unset}")"
    fi
done
grep -q "gh variable set LINUX_BUILDS -b compute --repo Alakazam-211/K2" "$tmp/out-pf-compute-github" \
    && pass "compute without the variable: says the exact gh command" || fail "compute hint" "$(cat "$tmp/out-pf-compute-github")"
grep -q "will NOT build x86 Linux" "$tmp/out-pf-github-compute" && grep -q "gh variable set LINUX_BUILDS -b github" "$tmp/out-pf-github-compute" \
    && pass "github mode with the variable on compute: refused, with both ways out" || fail "github hint" "$(cat "$tmp/out-pf-github-compute")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute K2_COMPUTE_OAUTH=placeholder run pf-placeholder k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "must never ship" "$tmp/out-pf-placeholder" && pass "compute refuses placeholder OAuth" || fail "placeholder" "rc=$rc $(cat "$tmp/out-pf-placeholder")"
echo "pre-flight under the signing user (K2_SIGNER=k2sign, S3b): both release nodes and the controller pinned, V1 on:"
PINS_OK='{"controller": "c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7", "nodes": [{"name": "k2n7-vin", "fp": "aa"}, {"name": "k2n9-vin", "fp": "bb"}]}'
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign STUB_PINS="$PINS_OK" run pf-k2sign k2_linux_builds_preflight)"
[ "$rc" = 0 ] && grep -q "Signing user: compute builds from L1 k2n7-vin and V1 k2n9-vin" "$tmp/out-pf-k2sign" \
    && ! grep -q "not signable\|S3b, signed receipts" "$tmp/out-pf-k2sign" && grep -q pins "$tmp/calls-pf-k2sign" \
    && pass "compute under k2sign: the pre-flight passes once the signer pinned both nodes and the controller" \
    || fail "k2sign pre-flight" "rc=$rc $(cat "$tmp/out-pf-k2sign")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign run pf-k2sign-none k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "no controller pinned" "$tmp/out-pf-k2sign-none" && grep -q "L1's node k2n7-vin is not pinned" "$tmp/out-pf-k2sign-none" \
    && grep -q "V1's node k2n9-vin is not pinned" "$tmp/out-pf-k2sign-none" \
    && pass "compute under k2sign, nothing pinned: refused before any build, with each pin command" \
    || fail "k2sign no pins" "rc=$rc $(cat "$tmp/out-pf-k2sign-none")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign STUB_PINS='{"controller": "c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7", "nodes": [{"name": "k2n7-vin", "fp": "aa"}]}' run pf-k2sign-v1 k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "V1's node k2n9-vin is not pinned" "$tmp/out-pf-k2sign-v1" && ! grep -q "L1's node" "$tmp/out-pf-k2sign-v1" \
    && pass "compute under k2sign, the V1 node not pinned: refused" || fail "k2sign V1 pin" "rc=$rc $(cat "$tmp/out-pf-k2sign-v1")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign STUB_PINS="$PINS_OK" STUB_X86=k2n9-vin run pf-k2sign-same k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "L1 and V1 are both on k2n9-vin" "$tmp/out-pf-k2sign-same" && ! grep -q pins "$tmp/calls-pf-k2sign-same" \
    && pass "compute under k2sign with L1 on V1's node: refused before any build" || fail "k2sign same node" "rc=$rc $(cat "$tmp/out-pf-k2sign-same")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_X86=k2n9-vin run pf-same k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "L1 and V1 are both on k2n9-vin" "$tmp/out-pf-same" \
    && pass "compute with L1 on V1's node: refused whoever signs (keychain too)" || fail "same node keychain" "rc=$rc $(cat "$tmp/out-pf-same")"
grep -q dual-nodes "$tmp/calls-pf-k2sign" && pass "the pre-flight checks L1 and V1 are two nodes" || fail "dual-nodes not called" "$(cat "$tmp/calls-pf-k2sign")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign STUB_PINS="$PINS_OK" STUB_VERIFY='' run pf-k2sign-nov1 k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "signs a compute-built" "$tmp/out-pf-k2sign-nov1" && ! grep -q pins "$tmp/calls-pf-k2sign-nov1" \
    && pass "compute under k2sign without V1 (K2_COMPUTE_VERIFY_NODE=none): refused (the signer needs the dual build)" \
    || fail "k2sign no V1" "rc=$rc $(cat "$tmp/out-pf-k2sign-nov1")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign STUB_PINS="$PINS_OK" K2_COMPUTE_OAUTH=env run pf-k2sign-env k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "only from" "$tmp/out-pf-k2sign-env" && grep -q "secret jobs" "$tmp/out-pf-k2sign-env" \
    && pass "compute under k2sign with K2_COMPUTE_OAUTH=env: refused (the signer takes only secret jobs)" || fail "k2sign env mode" "rc=$rc $(cat "$tmp/out-pf-k2sign-env")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign STUB_PINS_FAIL=1 run pf-k2sign-old k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "can't list its K2 Compute pins" "$tmp/out-pf-k2sign-old" \
    && pass "compute under an older signer (no compute-pins verb): refused, re-install from this commit" || fail "k2sign old signer" "rc=$rc $(cat "$tmp/out-pf-k2sign-old")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_SIGNER=k2sign STUB_PINS="$PINS_OK" STUB_COLLECT_FAIL=1 \
    run pf-k2sign-bad 'k2_linux_builds_preflight; k2_shadow_start; k2_shadow_collect; echo AFTER')"
[ "$rc" = 1 ] && grep -q "Signing user: compute builds" "$tmp/out-pf-k2sign-bad" \
    && grep -q "FATAL (K2 Compute, stage 1): not every compute build verified" "$tmp/out-pf-k2sign-bad" && ! grep -q AFTER "$tmp/out-pf-k2sign-bad" \
    && pass "compute under k2sign: through the pre-flight, then stopped at a bad verdict (Step 7.9c, before the tag)" \
    || fail "k2sign bad verdict" "rc=$rc $(cat "$tmp/out-pf-k2sign-bad")"
rc="$(STUB_VAR=github K2_LINUX_BUILDS=shadow STUB_SIGNER=k2sign run pf-k2sign-sh k2_linux_builds_preflight)"
[ "$rc" = 0 ] && pass "shadow under the signing user: fine (nothing from compute is signed)" || fail "k2sign shadow" "rc=$rc $(cat "$tmp/out-pf-k2sign-sh")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute STUB_VERIFY='' run pf-nov1 k2_linux_builds_preflight)"
[ "$rc" = 0 ] && grep -q "NO dual build" "$tmp/out-pf-nov1" && pass "compute without V1: loud warning" || fail "no V1 warning" "rc=$rc $(cat "$tmp/out-pf-nov1")"
python3 - "$REL" <<'PY' && pass "the pre-flight runs right after it is defined (before any build)" || fail "pre-flight call" "not called or called late"
import sys
s = open(sys.argv[1]).read()
d = s.index("k2_linux_builds_preflight() {")
c = s.index("\nk2_linux_builds_preflight\n")
assert d < c < s.index("# ── Step 0.4"), "order"
PY

echo "a compute problem: fatal in compute mode, a warning in shadow:"
rc="$(K2_LINUX_BUILDS=compute run prob-c 'k2_compute_problem "boom"; echo AFTER')"
[ "$rc" = 1 ] && grep -q "FATAL (K2 Compute, stage 1): boom" "$tmp/out-prob-c" && grep -q "nothing was pushed, tagged or published" "$tmp/out-prob-c" \
    && grep -q "K2_LINUX_BUILDS=github ./scripts/release.sh 9.9.9" "$tmp/out-prob-c" && ! grep -q "RESUME" "$tmp/out-prob-c" \
    && ! grep -q AFTER "$tmp/out-prob-c" && pass "compute, before the Mac build: exit 1, fall back with a plain re-run" || fail "compute fatal" "rc=$rc $(cat "$tmp/out-prob-c")"
mkdir -p "$tmp/target/release/bundle/dmg"; : > "$tmp/target/release/bundle/dmg/K2_9.9.9_aarch64.dmg"
rc="$(K2_LINUX_BUILDS=compute run prob-c2 'k2_compute_problem "boom"')"
[ "$rc" = 1 ] && grep -q "K2_LINUX_BUILDS=github K2_RELEASE_RESUME=8 ./scripts/release.sh 9.9.9" "$tmp/out-prob-c2" \
    && pass "compute, after the Mac build: the fallback reuses it (K2_RELEASE_RESUME=8)" || fail "compute fatal resume" "rc=$rc $(cat "$tmp/out-prob-c2")"
rm -rf "$tmp/target"
rc="$(K2_LINUX_BUILDS=shadow run prob-s 'k2_compute_problem "boom"; echo "AFTER $K2_SHADOW_NOTE"')"
[ "$rc" = 0 ] && grep -q "AFTER boom" "$tmp/out-prob-s" && pass "shadow: a warning, the release goes on" || fail "shadow warn" "rc=$rc $(cat "$tmp/out-prob-s")"

echo "Steps 1.6c and 7.9c:"
rc="$(K2_LINUX_BUILDS=github run s-gh 'k2_shadow_start; k2_shadow_collect; echo DONE')"
[ "$rc" = 0 ] && [ ! -s "$tmp/calls-s-gh" ] && pass "github mode: no compute call at all" || fail "github" "$(cat "$tmp/calls-s-gh")"
rc="$(K2_LINUX_BUILDS=compute STUB_SUBMIT_FAIL=1 run s-subfail 'k2_shadow_start; echo AFTER')"
[ "$rc" = 1 ] && ! grep -q AFTER "$tmp/out-s-subfail" && pass "compute: a failed submit stops the release at 1.6c" || fail "submit fail" "rc=$rc $(cat "$tmp/out-s-subfail")"
rc="$(K2_LINUX_BUILDS=shadow STUB_SUBMIT_FAIL=1 run s-subfail-sh 'k2_shadow_start; echo AFTER')"
[ "$rc" = 0 ] && grep -q AFTER "$tmp/out-s-subfail-sh" && pass "shadow: a failed submit only warns" || fail "shadow submit" "rc=$rc"
rc="$(K2_LINUX_BUILDS=compute STUB_COLLECT_FAIL=1 run s-colfail 'k2_shadow_start; k2_shadow_collect; echo AFTER')"
[ "$rc" = 1 ] && ! grep -q AFTER "$tmp/out-s-colfail" && pass "compute: an unverified build stops the release at 7.9c (before the tag)" || fail "collect fail" "rc=$rc"
rc="$(K2_LINUX_BUILDS=compute STUB_NO_V1=1 run s-nov1 'k2_shadow_start; k2_shadow_collect; echo AFTER')"
[ "$rc" = 1 ] && grep -q "dual build didn't run" "$tmp/out-s-nov1" && pass "compute: no V1 verdict stops the release" || fail "no V1" "rc=$rc $(cat "$tmp/out-s-nov1")"
rc="$(K2_LINUX_BUILDS=compute run s-resume 'k2_shadow_collect; echo AFTER')"
[ "$rc" = 0 ] && grep -q "^submit$" "$tmp/calls-s-resume" && grep -q "^collect$" "$tmp/calls-s-resume" && grep -q "submitting them now" "$tmp/out-s-resume" \
    && pass "compute resume (no jobs yet): 7.9c submits, then waits and collects" || fail "resume" "rc=$rc $(cat "$tmp/out-s-resume" "$tmp/calls-s-resume")"
rc="$(K2_LINUX_BUILDS=shadow run s-resume-sh 'k2_shadow_collect; echo AFTER')"
[ "$rc" = 0 ] && ! grep -q "^submit$" "$tmp/calls-s-resume-sh" && pass "shadow resume: warns, submits nothing" || fail "shadow resume" "$(cat "$tmp/calls-s-resume-sh")"

echo "the tree check (before the push):"
repo="$tmp/repo"; git init -q "$repo"; git -C "$repo" -c user.email=t@t -c user.name=t commit -q --allow-empty -m r
T="$(git -C "$repo" rev-parse 'HEAD^{tree}')"
rc="$(cd "$repo" && K2_LINUX_BUILDS=compute STUB_TREE="$T" run t-ok 'k2_shadow_start; k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 0 ] && grep -q "is the tree K2 Compute built" "$tmp/out-t-ok" && pass "same tree: passes" || fail "tree ok" "rc=$rc $(cat "$tmp/out-t-ok")"
rc="$(cd "$repo" && K2_LINUX_BUILDS=compute STUB_TREE=other run t-bad 'k2_shadow_start; k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 1 ] && ! grep -q AFTER "$tmp/out-t-bad" && pass "compute, another tree: stops" || fail "tree bad" "rc=$rc $(cat "$tmp/out-t-bad")"
[ "$(git -C "$repo" rev-list --count HEAD)" = 1 ] && pass "the tree check never rewrites git history" || fail "history" "commits changed"
rc="$(cd "$repo" && K2_LINUX_BUILDS=shadow STUB_TREE=other run t-bad-sh 'k2_shadow_start; k2_shadow_tree_check; echo AFTER')"
[ "$rc" = 0 ] && grep -q AFTER "$tmp/out-t-bad-sh" && pass "shadow, another tree: a warning" || fail "tree shadow" "rc=$rc"
python3 - "$REL" <<'PY' && pass "release.sh checks the tree after the commit and BEFORE pushing main and the tag" || fail "tree order" "see release.sh Step 8.9"
import sys
s = open(sys.argv[1]).read()
commit = s.index('git commit -m "chore(release): ${TAG}"')
check = s.index("\nk2_shadow_tree_check\n")
push = s.index("git push origin HEAD:main")
assert commit < check < push, (commit, check, push)
assert s.count("\nk2_shadow_tree_check\n") == 1
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
rc="$(K2_LINUX_BUILDS=compute run st-c 'k2_shadow_start; k2_shadow_collect; k2_compute_stage')"
[ "$rc" = 0 ] && grep -q "k2cr_stage $tmp/dist-st-c" "$tmp/calls-st-c" && grep -q K2_9.9.9_amd64.deb "$tmp/dist-st-c/.compute-staged" \
    && pass "compute mode: stages into DIST_DIR and records what it staged" || fail "stage" "rc=$rc $(cat "$tmp/out-st-c")"

echo "Steps 8.5 and 9 (release.sh as written):"
python3 - "$REL" > "$tmp/static" <<'PY' && pass "stage → manifest (no Linux key yet) → Step 9 attaches the staged set but the daemon → 10.5 signs it from the stage" || fail "8.5/9/10.5 wiring" "$(cat "$tmp/static")"
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
assert "k2-daemon-linux-x86_64|k2-daemon-linux-x86_64.sha256) continue" in blk, "the daemon goes up with its signatures at 10.5 (sign-daemon.sh), not at Step 9"
# SS1: a Linux daemon is signed only by sign-daemon.sh (with provenance), never at 8.5.
assert "k2_sign_daemon_raw" not in s and 'k2u_sign u1 "$DIST_DIR/k2-daemon-linux' not in s, "no Linux daemon signing at 8.5"
assert 'merge-daemon-manifest.py" "$DIST_DIR/daemon-latest.json"' not in s, "Linux manifest keys come from sign-daemon.sh at 10.5"
PY
python3 -c '
import sys; s=open(sys.argv[1]).read(); assert "if k2_compute_on; then\n        k2cr_cancel_open || true" in s' "$REL" \
    && pass "an aborted compute or shadow release cancels its uncollected jobs" || fail "cancel on exit" "exit trap"

echo "GitHub workflows follow the variable:"
python3 - "$ROOT/.github/workflows/daemon-binaries.yml" <<'PY' && pass "daemon-binaries: compute tag push → aarch64 only; otherwise and on dispatch → both" || fail "daemon-binaries matrix" "see the include expression"
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
PY
grep -q "^    if: github.event_name != 'push' || vars.LINUX_BUILDS != 'compute'$" "$ROOT/.github/workflows/app-linux.yml" \
    && pass "app-linux: skips a compute tag push, builds on dispatch" || fail "app-linux if" "missing"
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
    export STUB_CALLS="$tmp/calls-$name" STUB_N="$tmp/n-$name"
    : > "$STUB_CALLS"; rm -f "$STUB_N"
    (cd "$rgit" && PATH="$tmp/bin:$PATH" K2_BIN="$tmp/bin-real/k2" RELEASE_REPO="Alakazam-211/K2" VERSION="9.9.9" TAG="v9.9.9" PROJECT_DIR="$tmp/proj-$name" \
        K2_SIGNER="${STUB_SIGNER:-keychain}" K2_RELEASE_AUDIT_DIR="$tmp/audit-$name" DIST_DIR="$tmp/dist-$name" K2_COMPUTE_RUST=1.99.1 K2_COMPUTE_WAIT_MIN=1 \
        bash -c 'set -euo pipefail; K2_SHADOW_STATE=""; K2_SHADOW_NOTE=""; K2_SHADOW_LATE=""; source "$1"; source "$2"; eval "$3"' \
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
rc="$(K2_LINUX_BUILDS=shadow K2_COMPUTE_EXCLUSIVE=0 run_real real-sh-noexcl "$FLOW")"
[ "$rc" = 0 ] && grep -q AFTER "$tmp/out-real-sh-noexcl" && [ "$(grep -c 'k2 compute run' "$tmp/calls-real-sh-noexcl")" = 3 ] \
    && ! grep -q -- '--exclusive' "$tmp/calls-real-sh-noexcl" \
    && grep -q "shadow (K2 Compute): not every compute build verified" "$tmp/out-real-sh-noexcl" \
    && pass "shadow, K2_COMPUTE_EXCLUSIVE=0: all three submitted without --exclusive; a failed collect only warns" \
    || fail "shadow no exclusive" "rc=$rc $(cat "$tmp/out-real-sh-noexcl" "$tmp/calls-real-sh-noexcl")"
rc="$(STUB_VAR=compute K2_LINUX_BUILDS=compute K2_COMPUTE_EXCLUSIVE=0 run pf-noexcl k2_linux_builds_preflight)"
[ "$rc" = 1 ] && grep -q "needs the release builds --exclusive" "$tmp/out-pf-noexcl" \
    && pass "compute with K2_COMPUTE_EXCLUSIVE=0: refused at the pre-flight" || fail "compute no exclusive" "rc=$rc $(cat "$tmp/out-pf-noexcl")"

echo "syntax:"
bash -n "$REL" && pass "bash -n release.sh" || fail "bash -n" "release.sh"

echo ""
echo "release-compute-mode: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
