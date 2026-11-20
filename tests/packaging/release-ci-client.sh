#!/usr/bin/env bash
# release.sh K2_CI_PATH=client (prd-compute-build-attestations-v1 S1),
# with stand-ins. Never runs release.sh, cargo, gh, k2 or k2-signer: the
# functions are cut out of release.sh as written, run with the real
# scripts/lib/ci-client-release.sh, and the `k2` CLI (K2_BIN) and k2-signer
# (K2_SIGNER_CMD) are Python stand-ins that answer per $STUB_*.
#
#  - the pre-flight: k2-signer's preflight lists the node keys a person
#    accepted, and `k2 ci release --plan` gets exactly those (--only-keys);
#    a signer that isn't ready, no accepted key, a plan that can't place, or
#    K2_SIGNER=keychain sends all of Linux to GitHub before any build
#    (K2_COMPUTE_FALLBACK=0: stops); other aarch64 cross hashes than
#    cross-pins.sh send only aarch64;
#  - 1.6c → 7.9c → 8.5: two k2 ci release runs, their evidence merged into
#    one bundle that k2-signer rehearses before the tag; a refused aarch64
#    part falls back alone; a refused x86_64 set sends all of Linux; staging
#    copies only files that are their receipt's artifact;
#  - shadow mode only warns; 10.5 hands the bundle to sign-daemon.sh and
#    --verdict to the three root-helper-key scripts (static).
#
# Run with: bash tests/packaging/release-ci-client.sh
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

python3 - "$REL" > "$tmp/fns.sh" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
names = ["k2_ci_client", "k2_ci_client_preflight", "k2_compute_signer_check", "k2_linux_builds_want_var", "k2_linux_builds_get_var",
         "k2_linux_builds_var_is", "k2_linux_builds_can_set_var", "k2_compute_ready", "k2_compute_fallback", "k2_linux_builds_preflight",
         "k2_linux_builds_set_var", "k2_linux_builds_github_part", "k2_shadow_warn", "k2_compute_fatal", "k2_compute_problem",
         "k2_compute_on", "k2_shadow_start", "k2_ci_client_collect", "k2_shadow_collect", "k2_compute_unstage", "k2_compute_stage",
         "k2_shadow_report"]
for n in names:
    m = re.search(r"^%s\(\) \{\n.*?^\}\n" % re.escape(n), s, re.S | re.M) or re.search(r"^%s\(\) \{ .* \}\n" % re.escape(n), s, re.M)
    if not m:
        sys.exit("release.sh has no function " + n)
    print(m.group(0))
PY
# The step timer those functions call (scripts/lib/release-timer.sh; it
# does nothing here: no _rt_init).
cat "$ROOT/scripts/lib/release-timer.sh" >> "$tmp/fns.sh"

mkdir -p "$tmp/bin" "$tmp/scripts/ci"
cp "$ROOT/scripts/ci/cross-pins.sh" "$tmp/scripts/ci/cross-pins.sh"
ZIGP="$(sed -n 's/^K2_CROSS_ZIG_SHA256_X86_64="\(.*\)"$/\1/p' "$ROOT/scripts/ci/cross-pins.sh")"
ZBP="$(sed -n 's/^K2_CROSS_ZIGBUILD_CRATE_SHA256="\(.*\)"$/\1/p' "$ROOT/scripts/ci/cross-pins.sh")"
export ZIGP ZBP
K1="$(printf 'a%.0s' $(seq 64))"; K2K="$(printf 'b%.0s' $(seq 64))"
export K1 K2K

# gh: the repo variable (as release-compute-mode.sh).
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
echo "gh $*" >> "$STUB_CALLS"
if [ "$1 $2 $3" = "variable get LINUX_BUILDS" ]; then v="$(cat "$STUB_VAR_FILE" 2>/dev/null || true)"; [ -n "$v" ] || exit 1; echo "$v"; exit 0; fi
if [ "$1 $2 $3" = "variable set LINUX_BUILDS" ]; then printf '%s' "$5" > "$STUB_VAR_FILE"; exit 0; fi
if [ "$1" = api ]; then echo true; exit 0; fi
exit 2
STUB

# k2 (K2_BIN): ci release / status / evidence, compute cancel.
cat > "$tmp/bin/k2-stub" <<'STUB'
#!/usr/bin/env python3
import hashlib, json, os, sys
a = sys.argv[1:]
open(os.environ["STUB_CALLS"], "a").write("k2 " + " ".join(a) + "\n")
def flag(n):
    return a[a.index(n) + 1] if n in a else None
if a[:2] == ["ci", "release"]:
    builds = flag("--build") or ""
    kind = "arm" if "aarch64" in builds else "x86"
    if "--plan" in a:
        rc = int(os.environ.get("STUB_PLAN_RC_" + kind.upper(), "0"))
        if rc:
            print("seat_degraded: build %s is a dual build and needs two seat nodes that can take it; 1 can" % builds, file=sys.stderr)
            sys.exit(rc)
        print("release %s of k2 at c0ffee on owner.example's seat seat-1 (lane ci-standard); nothing submitted (--plan):" % a[2])
        sys.exit(0)
    if os.environ.get("STUB_SUBMIT_FAIL_" + kind.upper()) == "1":
        print("seat_degraded: no", file=sys.stderr)
        sys.exit(3)
    labels = ["L2", "V2"] if kind == "arm" else ["L1", "V1", "L3"]
    print(json.dumps({"run": {"id": "run-" + kind + "-0000000000000000", "short": "run-" + kind, "state": "running", "detail": None},
                      "placed": [{"label": l, "node": "n-" + l, "why": "starts now"} for l in labels]}))
    sys.exit(0)
if a[:2] == ["ci", "status"]:
    kind = "arm" if "arm" in a[2] else "x86"
    print(json.dumps({"run": {"state": os.environ.get("STUB_STATE_" + kind.upper(), "passed"),
                              "checks": [{"job": "job-" + kind, "state": "running"}]}}))
    sys.exit(0)
if a[:2] == ["ci", "evidence"]:
    run, out = a[2], flag("--out")
    kind = "arm" if "arm" in run else "x86"
    if os.environ.get("STUB_STATE_" + kind.upper(), "passed") != "passed":
        print("ci_release_not_done: release run is failed", file=sys.stderr)
        sys.exit(3)
    arch = "aarch64" if kind == "arm" else "x86_64"
    labels = {"arm": [("L2", "linux-aarch64", "build"), ("V2", "linux-aarch64", "verify")],
              "x86": [("L1", "linux-x86_64", "build"), ("V1", "linux-x86_64", "verify"), ("L3", "linux-app", "build")]}[kind]
    rec, rep = {}, {}
    for label, build, role in labels:
        d = os.path.join(out, label, "dist")
        os.makedirs(d, exist_ok=True)
        if build == "linux-app":
            names = ["K2_9.9.9_amd64.deb", "K2-9.9.9-1.x86_64.rpm", "K2_9.9.9_amd64.AppImage"]
        else:
            names = ["%s-linux-%s" % (b, arch) for b in ("k2-daemon", "k2-node", "k2-root-helper", "k2-app-engine", "k5-cli")] + ["build-info.json"]
        arts = []
        sums = []
        for n in names:
            data = ("%s built by %s" % (n, label if role == "verify" else "the build")).encode()
            if n == "build-info.json":
                data = json.dumps({"cross": {"zig_tarball_sha256": os.environ["ZIGP"], "cargo_zigbuild_crate_sha256": os.environ["ZBP"]}}).encode()
            open(os.path.join(d, n), "wb").write(data)
            h = hashlib.sha256(data).hexdigest()
            if os.environ.get("STUB_LIE_ABOUT") == n and role == "build":
                h = "0" * 64
            arts.append({"name": "dist/" + n, "sha256": h, "bytes": len(data)})
            if n != "build-info.json" and build != "linux-app":
                open(os.path.join(d, n + ".sha256"), "w").write("%s  %s\n" % (hashlib.sha256(data).hexdigest(), n))
            if build == "linux-app":
                sums.append("%s  %s\n" % (hashlib.sha256(data).hexdigest(), n))
        if sums:
            open(os.path.join(d, "app-linux.sums"), "w").write("".join(sums))
        rec[label] = {"build": build, "role": role, "body": json.dumps({"artifacts": arts}), "sig": "", "plan": {}}
        if "--reported" in a and os.path.isfile(os.path.join(d, "build-info.json")):
            rep[label] = {"dist/build-info.json": "eA=="}
    b = {"v": 3, "kind": "k2-ci-evidence", "project": "k2", "version": "9.9.9", "run_id": run, "controller_fp": "c" * 64,
         "created_at": 5 if kind == "arm" else 4, "source": {"commit": "1" * 40, "tree": "2" * 40, "commit_object": None},
         "receipts": rec, "statements": [{"s": "same"}, {"s": kind}], "rotations": [], "attestations": [],
         "rosters": [{"roster": 7}], "seats": [{"seat": 1}], "pairing_pins": {},
         "provenance": {l: {"payloadType": "application/vnd.in-toto+json", "payload": "e30=", "signatures": []} for l, _, _ in labels}}
    if rep:
        b["reported"] = rep
    p = os.path.join(out, "evidence-k2-9.9.9.json")
    json.dump(b, open(p, "w"))
    print(json.dumps({"path": p, "short": run[:8]}))
    sys.exit(0)
if a[:2] == ["compute", "cancel"]:
    sys.exit(0)
print("k2 stand-in: unexpected " + " ".join(a), file=sys.stderr)
sys.exit(2)
STUB

# k2-signer (K2_SIGNER_CMD): preflight, rehearse.
cat > "$tmp/bin/k2-signer-stub" <<'STUB'
#!/usr/bin/env python3
import json, os, sys
a = sys.argv[1:]
open(os.environ["STUB_CALLS"], "a").write("k2-signer " + " ".join(a) + "\n")
if a[:2] == ["preflight", "k2"]:
    if os.environ.get("STUB_SIGNER_REFUSE"):
        print("\x1b[1;31mk2-signer: refused: preflight for k2: owner 0000… in the policy is not pinned\x1b[0m", file=sys.stderr)
        sys.exit(3)
    keys = [k for k in os.environ.get("STUB_KEYS", os.environ["K1"] + "," + os.environ["K2K"]).split(",") if k]
    eq = {"/cross/zig_tarball_sha256": os.environ.get("STUB_ZIG", os.environ["ZIGP"]), "/cross/cargo_zigbuild_crate_sha256": os.environ["ZBP"]}
    print(json.dumps({"ok": True, "project": "k2", "roster_changes": "ask", "accepted_keys": keys,
                      "accepted": [{"name": "node-%d" % i, "fp": k, "how": "person"} for i, k in enumerate(keys)],
                      "builds": {"linux-aarch64": {"reported": {"file": "dist/build-info.json", "equals": eq}}}, "notes": []}))
    sys.exit(0)
if a[:2] == ["rehearse", "k2"]:
    b = json.load(sys.stdin)
    labels = sorted(b["receipts"])
    open(os.environ["STUB_CALLS"], "a").write("rehearsed " + ",".join(labels) + " rosters=%d statements=%d provenance=%s\n" % (len(b["rosters"]), len(b["statements"]), ",".join(sorted(b.get("provenance") or {}))))
    bad = os.environ.get("STUB_REHEARSE_REFUSE", "")
    if bad and bad in labels:
        print("k2-signer: refused: %s: the job's rustc was 1.98.0, the commit pins 1.99.1" % bad, file=sys.stderr)
        sys.exit(3)
    print(json.dumps({"ok": True, "rehearsal": True}))
    sys.exit(0)
print("k2-signer stand-in: unexpected " + " ".join(a), file=sys.stderr)
sys.exit(2)
STUB
chmod +x "$tmp/bin/"*

cat > "$tmp/stubs.sh" <<'LIB'
k2cr_init() { echo "k2cr_init $*" >> "$STUB_CALLS"; K2CR_VERSION="$1"; K2CR_COMMIT="$2"; K2CR_TREE="t1"; K2CR_STATE="$3"; K2CR_FILES="$4"; K2CR_COLLECTED=0; mkdir -p "$3" "$4"; : > "$3/jobs.tsv"; : > "$3/summary.txt"; }
k2cr_submit_release_set() { echo "SCRIPTS-PATH submit" >> "$STUB_CALLS"; return 1; }
k2u_compute_pins() { echo "SCRIPTS-PATH pins" >> "$STUB_CALLS"; return 1; }
k2_release_preview_commit() { echo "c0ffee00"; }
cargo() { return 0; }
git() { if [ "$1 $2" = "rev-parse HEAD" ]; then echo "head0000"; else command git "$@"; fi; }
LIB

run() {
    local name="$1" snippet="$2" rc=0
    export STUB_CALLS="$tmp/calls-$name" STUB_VAR_FILE="$tmp/var-$name"
    : > "$STUB_CALLS"; : > "$STUB_VAR_FILE"
    PATH="$tmp/bin:$PATH" RELEASE_REPO="Alakazam-211/K2" VERSION="9.9.9" PROJECT_DIR="$tmp" K2_SIGNER="${STUB_SIGNER:-k2sign}" \
        K2_RELEASE_AUDIT_DIR="$tmp/audit-$name" DIST_DIR="$tmp/dist-$name" K2_BIN="$tmp/bin/k2-stub" K2_SIGNER_CMD="$tmp/bin/k2-signer-stub" \
        K2_CI_PATH=client K2_CI_WAIT_SECS=0 \
        bash -c 'set -euo pipefail; K2_SHADOW_STATE=""; K2_SHADOW_NOTE=""; K2_COMPUTE_FELL_BACK=""; K2_LINUX_BUILDS="${K2_LINUX_BUILDS:-compute}"; K2_COMPUTE_ARM="${K2_COMPUTE_ARM:-compute}"
            mode() { echo "MODE $K2_LINUX_BUILDS $K2_COMPUTE_ARM"; }
            source "$1/scripts/lib/compute-release.sh"; source "$1/scripts/lib/ci-client-release.sh"; source "$2"; source "$3"; eval "$4"' \
        _ "$ROOT" "$tmp/stubs.sh" "$tmp/fns.sh" "$snippet" > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}
has() { grep -q -- "$2" "$tmp/out-$1"; }
called() { grep -q -- "$2" "$tmp/calls-$1"; }

echo "pre-flight:"
rc="$(run ok 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has ok "MODE compute compute" && called ok "k2 ci release 9.9.9 --sha head0000 --build linux-x86_64,linux-app --only-keys $K1,$K2K --plan" \
    && called ok "k2 ci release 9.9.9 --sha head0000 --build linux-aarch64 --only-keys $K1,$K2K --plan" && has ok "node keys accepted: node-0" \
    && ! called ok "SCRIPTS-PATH" && ! has ok "FALL BACK" \
    && pass "ready: the plan gets exactly the accepted keys, for both sets; no k2sign pins asked" || fail "ready" "rc=$rc $(cat "$tmp/out-ok" "$tmp/calls-ok")"
rc="$(STUB_SIGNER_REFUSE=1 run refuse 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has refuse "MODE github github" && has refuse "FALL BACK TO GITHUB" && has refuse "is not pinned" && has refuse "apply-setup" \
    && ! called refuse "ci release" && pass "signer not ready: all of Linux to GitHub, before any plan, with the setup step named" \
    || fail "signer refuses" "rc=$rc $(cat "$tmp/out-refuse")"
rc="$(STUB_SIGNER_REFUSE=1 K2_COMPUTE_FALLBACK=0 run refuse0 'k2_linux_builds_preflight; mode')"
[ "$rc" = 1 ] && has refuse0 "FATAL" && ! has refuse0 "MODE" && pass "K2_COMPUTE_FALLBACK=0: stops instead" || fail "fallback 0" "rc=$rc $(cat "$tmp/out-refuse0")"
rc="$(STUB_KEYS= run nokeys 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has nokeys "MODE github github" && has nokeys "accepted no node key" && ! called nokeys "ci release" \
    && pass "no node key accepted yet: GitHub, and no --only-keys with an empty list" || fail "no keys" "rc=$rc $(cat "$tmp/out-nokeys")"
rc="$(STUB_KEYS="$K1" STUB_PLAN_RC_X86=3 run degraded 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has degraded "MODE github github" && has degraded "seat_degraded" && called degraded "--only-keys $K1 --plan" \
    && pass "one accepted node: the plan can't place the dual build (exit 3), so GitHub, before any build" || fail "degraded" "rc=$rc $(cat "$tmp/out-degraded")"
rc="$(STUB_ZIG=ffff run zig 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has zig "MODE compute github" && has zig "another zig tarball" && has zig "aarch64 FALL BACK" \
    && pass "the signer pins other cross hashes: only aarch64 to GitHub" || fail "zig" "rc=$rc $(cat "$tmp/out-zig")"
rc="$(STUB_PLAN_RC_ARM=3 run armplan 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has armplan "MODE compute github" && pass "the aarch64 set can't be placed: only aarch64 to GitHub" || fail "arm plan" "rc=$rc $(cat "$tmp/out-armplan")"
rc="$(STUB_SIGNER=keychain run keychain 'k2_linux_builds_preflight; mode')"
[ "$rc" = 0 ] && has keychain "MODE github github" && has keychain "needs K2_SIGNER=k2sign" && pass "K2_SIGNER=keychain: the client path can't sign, so GitHub" \
    || fail "keychain" "rc=$rc $(cat "$tmp/out-keychain")"

FLOW='k2_linux_builds_preflight; k2_shadow_start; k2_shadow_collect; k2_compute_stage; mode; echo "BUNDLE $(k2ccr_bundle)"; { cat "$K2_SHADOW_STATE/staged.txt" 2>/dev/null || true; } | sed "s/^/STAGED /"'
echo "1.6c → 7.9c → 8.5:"
rc="$(run flow "$FLOW")"
[ "$rc" = 0 ] && has flow "MODE compute compute" && called flow "k2 ci release 9.9.9 --sha c0ffee00 --build linux-x86_64,linux-app --only-keys $K1,$K2K --json" \
    && called flow "k2 ci release 9.9.9 --sha c0ffee00 --build linux-aarch64 --only-keys $K1,$K2K --json" \
    && called flow "--reported dist/build-info.json" && called flow "rehearsed L1,L2,L3,V1,V2 rosters=1 statements=3 provenance=L1,L2,L3,V1,V2" \
    && has flow "STAGED k2-daemon-linux-aarch64" && has flow "STAGED K2_9.9.9_amd64.deb" && has flow "STAGED SHA256SUMS-linux-x86_64.txt" \
    && [ -s "$tmp/dist-flow/k2-root-helper-linux-x86_64" ] \
    && pass "two runs on the accepted keys; one merged bundle (rosters and statements once) rehearsed; both arches staged" \
    || fail "flow" "rc=$rc $(cat "$tmp/out-flow" "$tmp/calls-flow")"
rc="$(STUB_REHEARSE_REFUSE=L2 run armrefused "$FLOW")"
[ "$rc" = 0 ] && has armrefused "MODE compute github" && called armrefused "rehearsed L1,L3,V1 rosters=1 statements=2 provenance=L1,L3,V1" && ! has armrefused "STAGED k2-daemon-linux-aarch64" \
    && has armrefused "STAGED k2-daemon-linux-x86_64" && has armrefused "rustc was 1.98.0" \
    && pass "the rehearsal refuses the aarch64 part: x86_64 rehearsed alone and staged; aarch64 to GitHub" || fail "arm refused" "rc=$rc $(cat "$tmp/out-armrefused")"
rc="$(STUB_STATE_ARM=failed run armfailed "$FLOW")"
[ "$rc" = 0 ] && has armfailed "MODE compute github" && has armfailed "STAGED k2-daemon-linux-x86_64" \
    && pass "a failed aarch64 run: x86_64 goes on alone" || fail "arm failed" "rc=$rc $(cat "$tmp/out-armfailed")"
rc="$(STUB_REHEARSE_REFUSE=V1 run x86refused "$FLOW")"
[ "$rc" = 0 ] && has x86refused "MODE github github" && has x86refused "did not pass k2-signer's rehearsal" && ! has x86refused "STAGED" \
    && pass "the x86_64 set refused: all of Linux to GitHub before the tag, nothing staged" || fail "x86 refused" "rc=$rc $(cat "$tmp/out-x86refused")"
rc="$(STUB_SUBMIT_FAIL_X86=1 run submitfail "$FLOW")"
[ "$rc" = 0 ] && has submitfail "MODE github github" && pass "k2 ci release refuses the x86_64 set: GitHub" || fail "submit fail" "rc=$rc $(cat "$tmp/out-submitfail")"
rc="$(STUB_LIE_ABOUT=k2-node-linux-x86_64 run lie "$FLOW")"
[ "$rc" = 0 ] && has lie "MODE github github" && has lie "not L1's artifact" \
    && pass "a fetched file that isn't its receipt's artifact is never staged: GitHub" || fail "lie" "rc=$rc $(cat "$tmp/out-lie")"
rc="$(K2_LINUX_BUILDS=shadow STUB_REHEARSE_REFUSE=V1 run shadow "$FLOW; k2_shadow_report")"
[ "$rc" = 0 ] && has shadow "MODE shadow" && has shadow "shadow (K2 Compute)" && has shadow "client path" && ! has shadow "FALL BACK" \
    && pass "shadow: a refused rehearsal only warns" || fail "shadow" "rc=$rc $(cat "$tmp/out-shadow")"

echo "sign-daemon.sh --ci-evidence (argument rules; nothing signed):"
mkdir -p "$tmp/sd-dist" && echo '{}' > "$tmp/sd-bundle.json"
sd() { env -u K2_SIGNER_CMD PATH="$tmp/bin:$PATH" K2_SIGNER="${SD_SIGNER:-k2sign}" bash "$ROOT/scripts/sign-daemon.sh" 9.9.9 "$@" > "$tmp/sd.out" 2>&1; }
sd --from-dir "$tmp/sd-dist" --ci-evidence "$tmp/sd-bundle.json" --compute-verdict "$tmp" && fail "sd two paths" "accepted" \
    || { grep -q "two paths" "$tmp/sd.out" && pass "--ci-evidence and --compute-verdict together: refused" || fail "sd two paths" "$(cat "$tmp/sd.out")"; }
sd --ci-evidence "$tmp/sd-bundle.json" && fail "sd no dir" "accepted" \
    || { grep -q "needs --from-dir" "$tmp/sd.out" && pass "--ci-evidence without --from-dir: refused" || fail "sd no dir" "$(cat "$tmp/sd.out")"; }
sd --from-dir "$tmp/sd-dist" --ci-evidence "$tmp/none.json" && fail "sd no bundle" "accepted" \
    || { grep -q "no bundle there" "$tmp/sd.out" && pass "--ci-evidence with no bundle: refused" || fail "sd no bundle" "$(cat "$tmp/sd.out")"; }
sd --from-dir "$tmp/sd-dist" --ci-evidence "$tmp/sd-bundle.json" --dry --manifest "$tmp/sd-bundle.json" --out "$tmp/sd-out" && fail "sd dry" "accepted" \
    || { grep -q "can't be --dry" "$tmp/sd.out" && pass "--ci-evidence --dry: refused (k2-signer registers for the real tag)" || fail "sd dry" "$(cat "$tmp/sd.out")"; }
SD_SIGNER=keychain sd --from-dir "$tmp/sd-dist" --ci-evidence "$tmp/sd-bundle.json" && fail "sd keychain" "accepted" \
    || { grep -q "needs K2_SIGNER=k2sign" "$tmp/sd.out" && pass "--ci-evidence under K2_SIGNER=keychain: refused" || fail "sd keychain" "$(cat "$tmp/sd.out")"; }

echo "Step 10.5 wiring (static):"
grep -q -- '--ci-evidence "$(k2ccr_bundle)"' "$REL" && pass "sign-daemon.sh gets the rehearsed bundle on the client path" || fail "ci-evidence" "release.sh"
for s in sign-root-helper.sh sign-k2-node.sh sign-app-engine.sh; do
    grep -q "scripts/$s\" \"\${VERSION}\" \${ROOT_KEY_SIGN_ARGS\[@\]+" "$REL" || { fail "$s" "not passed ROOT_KEY_SIGN_ARGS"; continue; }
    grep -q 'k2s_verdict "$A" "$VERSION" "$TMP/$A"' "$ROOT/scripts/$s" || { fail "$s" "no verdict gate"; continue; }
    pass "$s: --verdict on the client path, gated on k2-signer's verdict"
done
grep -q 'K2_SIGNER_CMD' <(sed -n '/^for _k2_seam in/p' "$REL") && pass "release.sh refuses the K2_SIGNER_CMD test seam" || fail "seam" "K2_SIGNER_CMD not refused"
grep -q '^K2_CI_PATH="${K2_CI_PATH:-scripts}"' "$REL" && pass "the scripts path stays the default until the client path is proven" || fail "default" "K2_CI_PATH default"

echo ""
echo "release-ci-client: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
