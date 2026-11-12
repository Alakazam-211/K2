#!/usr/bin/env bash
# Tests for scripts/lib/compute-release.sh + scripts/ci/compute_release.py
# (release.sh K2_LINUX_BUILDS=shadow, plan-ci-release-on-k2-compute-v1)
# with a stub `k2` and `gh` on PATH. The stub k2 "builds" small files with
# the real shapes (ELF headers, build markers, deb/rpm/AppImage magic) and
# answers get/receipt/fetch the way the real CLI prints them, so every
# check runs on real bytes. Nothing here talks to a daemon, a node or
# GitHub. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PY="$ROOT/scripts/ci/compute_release.py"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export PYTHONDONTWRITEBYTECODE=1

VERSION="9.9.9"
# A repo with one commit: the "release commit".
repo="$tmp/repo"
mkdir -p "$repo"
git -C "$repo" init -q
# Pinned to a version unlike the real repo's, so the tests see the pin read
# AT THE RELEASE COMMIT in the repo it lives in.
printf '[toolchain]\nchannel = "1.98.7"\n' > "$repo/rust-toolchain.toml"
git -C "$repo" add rust-toolchain.toml
git -C "$repo" -c user.email=t@t -c user.name=t commit -q -m release
COMMIT="$(git -C "$repo" rev-parse HEAD)"
TREE="$(git -C "$repo" rev-parse 'HEAD^{tree}')"

# ── fixture maker: the files a build job would keep ──
cat > "$tmp/gen.py" <<'PY'
import hashlib, json, os, sys
kind, arch, out, version, commit, tree = sys.argv[1:7]
tamper = os.environ.get("GEN_TAMPER", "")
os.makedirs(os.path.join(out, "dist"), exist_ok=True)
machine = {"x86_64": 62, "aarch64": 183}[arch]
def elf(extra):
    head = bytearray(64)
    head[0:4] = b"\x7fELF"; head[4] = 2; head[5] = 1
    head[18:20] = machine.to_bytes(2, "little")
    return bytes(head) + extra
files = {}
ptyd = hashlib.sha256(b"ptyd").hexdigest()
# The daemon embeds k2-export and k2-deskd too (build-linux-bins.sh records
# their hashes; compute_release.py looks for each one in the daemon).
export = hashlib.sha256(b"export").hexdigest()
deskd = hashlib.sha256(b"deskd").hexdigest()
v = "1.0.0" if tamper == "marker_version" else version
if kind == "linux-bins":
    embeds = {"ptyd": ptyd, "export": export, "deskd": deskd}
    files[f"k2-daemon-linux-{arch}"] = elf(b"daemon " + b" ".join(
        h.encode() for k, h in embeds.items() if tamper != "no_" + k))
    node = b"node K2_NODE_BUILD v=%s proto=3 os=linux arch=%s;" % (v.encode(), arch.encode())
    if tamper == "test_keys":
        node += b" K2_NODE_TEST_KEYS_ENABLED"
    files[f"k2-node-linux-{arch}"] = elf(node)
    files[f"k2-root-helper-linux-{arch}"] = elf(b"helper K2_ROOT_HELPER_BUILD v=%s proto=9 arch=%s;" % (v.encode(), arch.encode()))
    files[f"k2-app-engine-linux-{arch}"] = elf(b"engine K2_APP_ENGINE_BUILD v=%s proto=1 arch=%s;" % (v.encode(), arch.encode()))
    if os.environ.get("GEN_SALT"):
        files[f"k2-node-linux-{arch}"] += os.environ["GEN_SALT"].encode()
    if os.environ.get("GEN_DAEMON_SALT"):
        files[f"k2-daemon-linux-{arch}"] += os.environ["GEN_DAEMON_SALT"].encode()
else:
    files[f"K2_{version}_amd64.deb"] = b"!<arch>\ndebian-binary   " + b"x" * 40
    files[f"K2-{version}-1.x86_64.rpm"] = b"\xed\xab\xee\xdb" + b"r" * 40
    head = bytearray(elf(b"appimage"))
    head[8:11] = b"AI\x02"
    files[f"K2_{version}_amd64.AppImage"] = bytes(head)
meta = {}
sums = []
for n, data in files.items():
    open(os.path.join(out, "dist", n), "wb").write(data)
    meta[n] = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
    # The side files the real build scripts write: <bin>.sha256 (linux-bins),
    # app-linux.sums (linux-app), both in sha256sum format.
    if kind == "linux-bins":
        open(os.path.join(out, "dist", n + ".sha256"), "w").write("%s  %s\n" % (meta[n]["sha256"], n))
    else:
        sums.append("%s  %s\n" % (meta[n]["sha256"], n))
if sums:
    open(os.path.join(out, "dist", "app-linux.sums"), "w").write("".join(sums))
info = {"kind": kind, "version": "0.0.1" if tamper == "info_version" else version, "arch": arch,
        "commit": commit, "tree": tree, "files": meta}
if kind == "linux-bins":
    info.update(oauth="placeholder", ptyd_sha256=ptyd, export_sha256=export, deskd_sha256=deskd)
json.dump(info, open(os.path.join(out, "dist", "build-info.json"), "w"))
PY

# ── stub k2 ──
mkdir -p "$tmp/bin" "$tmp/stub"
cat > "$tmp/bin/k2" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "k2 $*" >> "$STUB_CALLS"
S="$STUB_DIR"
[ "$1" = "compute" ] || { echo "stub k2: only compute" >&2; exit 2; }
shift
verb="$1"; shift
case "$verb" in
    run)
        node="$1"; shift
        sha=""; argv=(); seen_dd=0
        while [ $# -gt 0 ]; do
            if [ "$seen_dd" = 1 ]; then argv+=("$1"); shift; continue; fi
            case "$1" in
                --) seen_dd=1; shift ;;
                --sha|--artifact|--needs|--mem|--queue-timeout|--retries|--env|--secret|--workspace|--tier) shift 2 ;;
                *) shift ;;
            esac
        done
        n=$(( $(cat "$S/counter" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$S/counter"
        job="$(printf 'job%05d%s' "$n" 0123456789abcdef0123456789ab)"
        mkdir -p "$S/$job"
        echo "$node" > "$S/$job/node"
        python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${argv[@]}" > "$S/$job/argv.json"
        kind=linux-app; arch=x86_64
        case "${argv[*]}" in *build-linux-bins.sh*) kind=linux-bins; arch="${argv[${#argv[@]}-1]}" ;; esac
        python3 "$GEN" "$kind" "$arch" "$S/$job/files" "$STUB_VERSION" "$STUB_COMMIT" "$STUB_TREE"
        printf '{"job": "%s", "short": "%s"}\n' "$job" "${job:0:8}"
        ;;
    wait) exit 0 ;;
    get)
        job="$1"
        STUB_JOBDIR="$S/$job" python3 - <<'PY'
import json, os
d = os.environ["STUB_JOBDIR"]
t = os.environ.get("STUB_TAMPER", "")
print(json.dumps({
    "job": os.path.basename(d), "short": os.path.basename(d)[:8],
    "node": open(os.path.join(d, "node")).read().strip(),
    "state": "failed" if t == "failed" else "done", "ok": t != "failed", "exit": 1 if t == "failed" else 0,
    "argv": json.load(open(os.path.join(d, "argv.json"))),
    "commit": os.environ["STUB_COMMIT"], "tree": os.environ["STUB_TREE"],
    "receipt": {"id": "r" * 16, "verified": t != "unverified"},
    "durationSecs": 61,
}))
PY
        ;;
    receipt)
        job="$1"
        STUB_JOBDIR="$S/$job" python3 - "$PYLIB" <<'PY'
import hashlib, json, os, sys
sys.path.insert(0, os.path.dirname(sys.argv[1]))
import compute_release as cr
d = os.environ["STUB_JOBDIR"]
t = os.environ.get("STUB_TAMPER", "")
argv = json.load(open(os.path.join(d, "argv.json")))
arts = []
root = os.path.join(d, "files")
for dp, _, names in os.walk(root):
    for n in sorted(names):
        p = os.path.join(dp, n)
        arts.append({"name": os.path.relpath(p, root), "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest(), "bytes": os.path.getsize(p)})
print(json.dumps({"job": os.path.basename(d)[:8], "verified": t != "unverified", "receipt": {
    "node_fp": os.environ.get("STUB_FP") or "fp-" + open(os.path.join(d, "node")).read().strip(),
    "argv_sha256": cr.argv_sha256(argv + (["x"] if t == "argv" else [])),
    "src": {"commit": os.environ["STUB_COMMIT"], "tree": "0" * 40 if t == "tree" else os.environ["STUB_TREE"],
            "dirty_sha256": "d" * 64 if t == "dirty" else None},
    "state": "done", "exit": {"code": 0, "signal": None}, "tools": {"rust": "1.99.1"},
    "artifacts": arts}}))
PY
        ;;
    fetch)
        job="$1"; shift; out=""
        while [ $# -gt 0 ]; do case "$1" in --out) out="$2"; shift 2 ;; *) shift ;; esac; done
        mkdir -p "$out"
        cp -R "$S/$job/files/." "$out/"
        if [ "${STUB_TAMPER:-}" = "swap" ]; then
            f="$(ls "$out"/dist/k2-daemon-linux-* 2>/dev/null | head -1)"
            [ -n "$f" ] && printf 'EVIL' >> "$f"
        fi
        STUB_OUT="$out" STUB_JOBDIR="$S/$job" python3 - <<'PY'
import hashlib, json, os
out = os.environ["STUB_OUT"]; src = os.path.join(os.environ["STUB_JOBDIR"], "files")
files = []
for dp, _, names in os.walk(src):
    for n in sorted(names):
        rel = os.path.relpath(os.path.join(dp, n), src)
        p = os.path.join(src, rel)
        files.append({"name": rel, "path": os.path.join(out, rel), "bytes": os.path.getsize(p),
                      "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest()})
print(json.dumps({"job": os.path.basename(os.environ["STUB_JOBDIR"]), "files": files}, indent=2))
PY
        ;;
    cancel) exit 0 ;;
    *) echo "stub k2: unknown compute $verb" >&2; exit 2 ;;
esac
STUB
chmod +x "$tmp/bin/k2"

cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "gh $*" >> "$STUB_CALLS"
[ "$1 $2" = "release download" ] || { echo "stub gh: only release download" >&2; exit 2; }
tag="$3"; shift 3; pattern=""; dir="."
while [ $# -gt 0 ]; do case "$1" in --pattern) pattern="$2"; shift 2 ;; --dir) dir="$2"; shift 2 ;; *) shift ;; esac; done
[ -f "$STUB_REL/$tag/$pattern" ] || { echo "no asset $pattern" >&2; exit 1; }
cp "$STUB_REL/$tag/$pattern" "$dir/$pattern"
STUB
chmod +x "$tmp/bin/gh"

export PATH="$tmp/bin:$PATH" GEN="$tmp/gen.py" PYLIB="$PY"
export STUB_VERSION="$VERSION" STUB_COMMIT="$COMMIT" STUB_TREE="$TREE"

# One fresh run: init, submit, wait, collect in a subshell. Echoes the
# collect rc; state in $tmp/<name>.
run_case() {
    local name="$1"
    export STUB_DIR="$tmp/stub-$name" STUB_CALLS="$tmp/calls-$name"
    rm -rf "${STUB_DIR:?}" "${tmp:?}/$name" "${tmp:?}/$name-files"
    mkdir -p "$STUB_DIR"
    : > "$STUB_CALLS"
    (
        cd "$repo"
        # shellcheck source=scripts/lib/compute-release.sh
        source "$ROOT/scripts/lib/compute-release.sh"
        k2cr_init "$VERSION" "$COMMIT" "$tmp/$name" "$tmp/$name-files"
        k2cr_submit_release_set >"$tmp/$name.submit" 2>&1
        k2cr_wait 1 >/dev/null
        rc=0
        k2cr_collect >"$tmp/$name.collect" 2>&1 || rc=$?
        echo "$rc"
    )
}

problems() { python3 -c 'import json,sys; print("\n".join(json.load(open(sys.argv[1]))["problems"]))' "$1"; }

echo "argv hash:"
want="$(printf '%s' '["bash","scripts/ci/build-linux-bins.sh","x86_64"]' | shasum -a 256 | awk '{print $1}')"
got="$(python3 "$PY" argv-sha256 bash scripts/ci/build-linux-bins.sh x86_64)"
[ "$got" = "$want" ] && pass "argv_sha256 is sha256 of the canonical JSON array" || fail "argv_sha256" "$got != $want"

echo "happy path (the defaults: secret OAuth, V1 on, --tier ci, the pin):"
unset STUB_TAMPER K2_COMPUTE_X86_NODE K2_COMPUTE_VERIFY_NODE K2_COMPUTE_ARM_NODE GEN_TAMPER GEN_SALT K2_COMPUTE_OAUTH K2_COMPUTE_OAUTH_SECRETS K2_COMPUTE_RUST K2_COMPUTE_TIER K2_LINUX_BUILDS K2_COMPUTE_CPUS
# The x86_64 sections below keep aarch64 off compute (K2_COMPUTE_ARM=github);
# "aarch64 on compute" further down covers L2 and V2.
export K2_COMPUTE_ARM=github
# This Mac's .env values: they must never reach argv (secret mode).
export K2_GMAIL_CLIENT_ID="gm-id-VALUE-1234.apps.example" K2_GMAIL_CLIENT_SECRET="gm-SECRET-VALUE-5678" K2_MICROSOFT_CLIENT_ID="ms-ID-VALUE-9012"
rc="$(run_case ok)"
[ "$rc" = 0 ] && pass "collect verifies L1, L3 and V1" || fail "collect rc=$rc" "$(cat "$tmp/ok.collect")"
[ "$(cut -f1,3 "$tmp/ok/jobs.tsv" | tr '\t\n' ': ')" = "L1:k2n7-vin L3:k2n8-vin V1:k2n9-vin " ] \
    && pass "three jobs: L1 k2n7, L3 k2n8, V1 k2n9 (the Ryzen nodes)" || fail "jobs.tsv" "$(cat "$tmp/ok/jobs.tsv")"
grep -q 'L2/V2 aarch64: not on compute (K2_COMPUTE_ARM=github' "$tmp/ok.submit" && pass "K2_COMPUTE_ARM=github: no L2/V2, and it says GitHub builds aarch64" \
    || fail "aarch64 note" "$(cat "$tmp/ok.submit")"
run_line="$(grep -m1 'k2 compute run k2n7-vin' "$tmp/calls-ok" || true)"
for want in "--sha $COMMIT" "--tier ci" "--detach" "--artifact dist/*" "--env BUILD_COMMIT=$COMMIT" "--needs rust@1.98.7," "--mem 20G" "--queue-timeout 2h" \
    "--secret K2_GMAIL_CLIENT_ID=BUILD_GMAIL_CLIENT_ID" "--secret K2_GMAIL_CLIENT_SECRET=BUILD_GMAIL_CLIENT_SECRET" \
    "--secret K2_MICROSOFT_CLIENT_ID=BUILD_MICROSOFT_CLIENT_ID" "-- bash scripts/ci/build-linux-bins.sh x86_64"; do
    [[ "$run_line" == *"$want"* ]] && pass "L1 submit has $want" || fail "L1 submit lacks $want" "$run_line"
done
! grep -q -- "--gate\|rust@stable\|BUILD_OAUTH=placeholder" "$tmp/calls-ok" && pass "no --gate, no rust@stable, no placeholder by default" \
    || fail "old defaults" "$(cat "$tmp/calls-ok")"
! grep -q -e "gm-id-VALUE" -e "gm-SECRET-VALUE" -e "ms-ID-VALUE" "$tmp/calls-ok" "$tmp/ok/jobs.tsv" "$tmp/ok/summary.txt" \
    && pass "secret mode: no OAuth value in argv or the audit files (names only)" || fail "value leaked" "$(grep -e VALUE "$tmp/calls-ok")"
grep -q "Rust for every job: rust@1.98.7" "$tmp/ok/summary.txt" && pass "the pin is read at the release commit and recorded" || fail "pin summary" "$(cat "$tmp/ok/summary.txt")"
! grep -qE -- '--env (K2_|K2SO_)' "$tmp/calls-ok" && pass "no K2_* name in --env (nodes refuse them: env_k2_reserved)" \
    || fail "K2_* env" "$(grep -- '--env K2' "$tmp/calls-ok")"
grep -q 'k2 compute run k2n8-vin .*rust@1.98.7,.*bun@1.2.*sys:libwebkit2gtk-4.1-dev.*-- bash scripts/ci/build-linux-app.sh' "$tmp/calls-ok" \
    && pass "L3 goes to k2n8-vin with the pin, bun + GTK packages" || fail "L3 submit" "$(cat "$tmp/calls-ok")"
grep -q 'k2 compute run k2n8-vin .*sys:libasound2-dev --mem 24G .*-- bash scripts/ci/build-linux-app.sh' "$tmp/calls-ok" \
    && pass "L3 asks for ALSA headers and 24G (native-voice release link)" || fail "L3 voice needs" "$(grep build-linux-app "$tmp/calls-ok")"
grep -q 'identical in both builds: .*k2-daemon-linux-x86_64' "$tmp/ok.collect" && ! grep -q 'reported only' "$tmp/ok.collect" \
    && pass "the daemon's dual build is required, not only reported (review H1)" || fail "daemon dual" "$(cat "$tmp/ok.collect")"
grep -q 'L1 on k2n7-vin and V1 on k2n9-vin: two nodes' "$tmp/ok.collect" && pass "collect checks L1 and V1 came from two node keys" \
    || fail "dual nodes line" "$(cat "$tmp/ok.collect")"
for n in k2n7-vin k2n9-vin; do
    line="$(grep -m1 "k2 compute run $n " "$tmp/calls-ok" || true)"
    [[ "$line" == *"--exclusive --why "* ]] && pass "the release build on $n is --exclusive (with a reason)" || fail "$n not exclusive" "$line"
done
! grep 'k2 compute run k2n8-vin ' "$tmp/calls-ok" | grep -q -- '--exclusive' && pass "L3 (no OAuth values) is not exclusive" \
    || fail "L3 exclusive" "$(grep k2n8-vin "$tmp/calls-ok")"
grep -q 'k2 compute fetch .* --over-relay --json' "$tmp/calls-ok" && pass "fetch passes --over-relay (AppImage > 100 MB)" || fail "fetch flags" "$(cat "$tmp/calls-ok")"
python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); assert v["ok"] and v["oauth"]=="placeholder" and v["node"]=="k2n7-vin", v' "$tmp/ok/L1.verdict.json" \
    && pass "L1 verdict: ok, node, oauth mode recorded" || fail "L1 verdict" "$(cat "$tmp/ok/L1.verdict.json")"

echo "compute mode staging (stage 1):"
(
    cd "$repo"
    source "$ROOT/scripts/lib/compute-release.sh"
    K2CR_STATE="$tmp/ok" K2CR_FILES="$tmp/ok-files" K2CR_COLLECTED=1 k2cr_stage "$tmp/dist" > "$tmp/stage.out" 2>&1
) && pass "k2cr_stage copies the verified files" || fail "stage" "$(cat "$tmp/stage.out")"
python3 - "$tmp/dist" "$tmp/ok/staged.txt" <<'PYS' && pass "staged: 4 binaries + .sha256, deb/rpm/AppImage, SHA256SUMS with L3's 3 lines" || fail "staged set" "$(ls "$tmp/dist")"
import os, sys
d, staged = sys.argv[1], open(sys.argv[2]).read().split()
want = {f"{b}-linux-x86_64{x}" for b in ("k2-daemon", "k2-node", "k2-root-helper", "k2-app-engine") for x in ("", ".sha256")}
want |= {"K2_9.9.9_amd64.deb", "K2-9.9.9-1.x86_64.rpm", "K2_9.9.9_amd64.AppImage", "SHA256SUMS-linux-x86_64.txt"}
assert set(os.listdir(d)) == want, sorted(set(os.listdir(d)) ^ want)
assert set(staged) == want, staged
assert len(open(os.path.join(d, "SHA256SUMS-linux-x86_64.txt")).read().splitlines()) == 3
PYS
rc=0; (cd "$repo"; source "$ROOT/scripts/lib/compute-release.sh"; K2CR_STATE="$tmp/ok" K2CR_FILES="$tmp/ok-files" K2CR_COLLECTED=0 k2cr_stage "$tmp/dist2") >/dev/null 2>&1 || rc=$?
[ "$rc" != 0 ] && [ ! -e "$tmp/dist2" ] && pass "stage refuses before collect" || fail "stage before collect" "rc=$rc"

echo "one OAuth mode and path remap for every Linux builder (0.46.6):"
export K2_COMPUTE_OAUTH=placeholder
rc="$(run_case remap)"
! grep -q 'BUILD_REMAP' "$tmp/calls-remap" && pass "no per-job BUILD_REMAP (the build script always remaps)" \
    || fail "BUILD_REMAP still passed" "$(grep BUILD_REMAP "$tmp/calls-remap")"
[ "$(grep -c -- '--env BUILD_OAUTH=placeholder' "$tmp/calls-remap")" = 2 ] && pass "L1 and V1 share the OAuth mode" \
    || fail "oauth per job" "$(cat "$tmp/calls-remap")"
export K2_COMPUTE_OAUTH=secret K2_COMPUTE_OAUTH_SECRETS="gmail-id,gmail-secret"
rc="$(run_case secret)"
l1="$(grep -m1 'k2 compute run k2n7-vin' "$tmp/calls-secret" || true)"
for want in "--secret gmail-id=BUILD_GMAIL_CLIENT_ID" "--secret gmail-secret=BUILD_GMAIL_CLIENT_SECRET"; do
    [[ "$l1" == *"$want"* ]] && pass "secret mode: L1 has $want" || fail "secret mode lacks $want" "$l1"
done
[[ "$l1" != *BUILD_OAUTH=placeholder* ]] && pass "secret mode: no placeholder" || fail "secret mode placeholder" "$l1"
[ "$(grep -c -- '--secret gmail-id=BUILD_GMAIL_CLIENT_ID' "$tmp/calls-secret")" = 2 ] && pass "secret mode: V1 too" \
    || fail "secret mode V1" "$(cat "$tmp/calls-secret")"
! grep -q 'build-linux-app.sh.*--secret\|--secret.*build-linux-app' "$tmp/calls-secret" \
    && ! grep 'k2n8-vin' "$tmp/calls-secret" | grep -q -- '--secret' && pass "secret mode: the app job gets no OAuth" \
    || fail "app job secrets" "$(grep k2n8-vin "$tmp/calls-secret")"
unset K2_COMPUTE_OAUTH_SECRETS K2_MICROSOFT_CLIENT_ID
rc="$(run_case nomss)"
! grep -q "K2_MICROSOFT_CLIENT_ID" "$tmp/calls-nomss" && [ "$(grep -c -- '--secret K2_GMAIL_CLIENT_ID=' "$tmp/calls-nomss")" = 2 ] \
    && pass "no Microsoft id in this Mac's env: only the two Gmail secrets" || fail "ms default" "$(cat "$tmp/calls-nomss")"
export K2_MICROSOFT_CLIENT_ID="ms-ID-VALUE-9012"
export K2_COMPUTE_VERIFY_NODE=none
rc="$(run_case nov1)"
[ "$(wc -l < "$tmp/nov1/jobs.tsv" | tr -d ' ')" = 2 ] && pass "K2_COMPUTE_VERIFY_NODE=none: no V1" || fail "no V1" "$(cat "$tmp/nov1/jobs.tsv")"
unset K2_COMPUTE_VERIFY_NODE
export K2_COMPUTE_RUST=1.97.0 K2_COMPUTE_TIER=standard
rc="$(run_case overrides)"
grep -q -- "--tier standard .*--needs rust@1.97.0," "$tmp/calls-overrides" && pass "K2_COMPUTE_RUST and K2_COMPUTE_TIER override" || fail "overrides" "$(cat "$tmp/calls-overrides")"
unset K2_COMPUTE_RUST K2_COMPUTE_TIER
export K2_COMPUTE_OAUTH=bogus
rc="$(run_case badmode)"
grep -q 'K2_COMPUTE_OAUTH must be placeholder, env or secret' "$tmp/badmode.submit" && pass "an unknown OAuth mode is refused" \
    || fail "bad mode" "$(cat "$tmp/badmode.submit")"
unset K2_COMPUTE_OAUTH K2_COMPUTE_OAUTH_SECRETS K2_COMPUTE_VERIFY_NODE

echo "tampering is caught:"
check_tamper() {
    local name="$1" what="$2" needle="$3" label="${4:-L1}"
    local rc
    rc="$(run_case "$name")"
    if [ "$rc" = 0 ]; then
        fail "$what" "collect passed"
    elif problems "$tmp/$name/$label.verdict.json" | grep -q "$needle"; then
        pass "$what"
    else
        fail "$what" "$(problems "$tmp/$name/$label.verdict.json")"
    fi
}
export STUB_TAMPER=tree;       check_tamper t-tree "receipt tree != release tree" "receipt tree"
export STUB_TAMPER=dirty;      check_tamper t-dirty "uncommitted changes in the receipt" "uncommitted"
export STUB_TAMPER=argv;       check_tamper t-argv "receipt argv hash for another command" "argv_sha256"
export STUB_TAMPER=unverified; check_tamper t-unverified "unverified receipt signature" "not verified"
export STUB_TAMPER=failed;     check_tamper t-failed "a failed job" "job ended failed"
export STUB_TAMPER=swap;       check_tamper t-swap "bytes swapped after the node signed" "don't match the signed receipt"
unset STUB_TAMPER
export GEN_TAMPER=test_keys;      check_tamper t-keys "a test-keys k2-node" "test-keys"
export GEN_TAMPER=marker_version; check_tamper t-marker "build marker of another version" "build marker says"
export GEN_TAMPER=info_version;   check_tamper t-info "build-info of another version" "build-info version"
export GEN_TAMPER=no_ptyd;        check_tamper t-ptyd "a daemon without k2-ptyd" "does not embed k2-ptyd"
export GEN_TAMPER=no_export;      check_tamper t-export "a daemon without k2-export" "does not embed k2-export"
export GEN_TAMPER=no_deskd;       check_tamper t-deskd "a daemon without k2-deskd" "does not embed k2-deskd"
unset GEN_TAMPER
export K2_COMPUTE_X86_NODE=stranger-1; check_tamper t-node "a node off the release list" "not a release node"
unset K2_COMPUTE_X86_NODE
export K2_COMPUTE_REQUIRE_BAKED_OAUTH=1; check_tamper t-oauth "a placeholder-OAuth daemon when baked is required" "oauth 'placeholder'"
unset K2_COMPUTE_REQUIRE_BAKED_OAUTH
export K2_LINUX_BUILDS=compute; check_tamper t-oauth-compute "compute mode always requires baked OAuth" "oauth 'placeholder'"
unset K2_LINUX_BUILDS


echo "aarch64 on compute (L2 + V2, cross-compiled with zig):"
# A release commit with the cross pins (what k2cr_arm_needs reads at it).
repo_arm="$tmp/repo-arm"
mkdir -p "$repo_arm/scripts/ci"
git -C "$repo_arm" init -q
printf '[toolchain]\nchannel = "1.98.7"\ntargets = ["aarch64-unknown-linux-gnu"]\n' > "$repo_arm/rust-toolchain.toml"
cp "$ROOT/scripts/ci/cross-pins.sh" "$repo_arm/scripts/ci/cross-pins.sh"
git -C "$repo_arm" add -A
git -C "$repo_arm" -c user.email=t@t -c user.name=t commit -q -m release
COMMIT_ARM="$(git -C "$repo_arm" rev-parse HEAD)"
TREE_ARM="$(git -C "$repo_arm" rev-parse 'HEAD^{tree}')"
ZIG="$(sed -n 's/^K2_CROSS_ZIG="\(.*\)"$/\1/p' "$ROOT/scripts/ci/cross-pins.sh")"
ZB="$(sed -n 's/^K2_CROSS_ZIGBUILD="\(.*\)"$/\1/p' "$ROOT/scripts/ci/cross-pins.sh")"
# run_arm <name> [setup snippet]: init, submit, wait, collect at COMMIT_ARM;
# echoes "<collect rc> <K2CR_X86_OK> <K2CR_ARM_OK> <K2CR_ARM_SUBMIT_OK>".
run_arm() {
    local name="$1" setup="${2:-}"
    export STUB_DIR="$tmp/stub-$name" STUB_CALLS="$tmp/calls-$name" STUB_COMMIT="$COMMIT_ARM" STUB_TREE="$TREE_ARM"
    rm -rf "${STUB_DIR:?}" "${tmp:?}/$name" "${tmp:?}/$name-files"
    mkdir -p "$STUB_DIR"
    : > "$STUB_CALLS"
    (
        cd "$repo_arm"
        source "$ROOT/scripts/lib/compute-release.sh"
        eval "$setup"
        k2cr_init "$VERSION" "$COMMIT_ARM" "$tmp/$name" "$tmp/$name-files"
        k2cr_submit_release_set >"$tmp/$name.submit" 2>&1 || true
        k2cr_wait 1 >/dev/null
        rc=0
        k2cr_collect >"$tmp/$name.collect" 2>&1 || rc=$?
        k2cr_stage "$tmp/$name-dist" >"$tmp/$name.stage" 2>&1 || true
        echo "$rc ${K2CR_X86_OK:-x} ${K2CR_ARM_OK:-x} ${K2CR_ARM_SUBMIT_OK:-x}"
    )
}
export K2_COMPUTE_ARM=compute
out="$(run_arm arm)"
[ "$out" = "0 1 1 1" ] && pass "five jobs submitted, x86_64 and aarch64 both verified" || fail "arm happy path" "$out $(cat "$tmp/arm.submit" "$tmp/arm.collect")"
[ "$(cut -f1,3,5 "$tmp/arm/jobs.tsv" | tr '\t\n' ': ')" = "L1:k2n7-vin:x86_64 L3:k2n8-vin:x86_64 V1:k2n9-vin:x86_64 L2:k2n10-vin:aarch64 V2:k2n11-vin:aarch64 " ] \
    && pass "L2 on k2n10-vin, V2 on k2n11-vin (the defaults)" || fail "arm jobs.tsv" "$(cat "$tmp/arm/jobs.tsv")"
for lab_node in k2n10-vin k2n11-vin; do
    line="$(grep -m1 "k2 compute run $lab_node " "$tmp/calls-arm" || true)"
    for want in "--needs cmake@3,sys:pkg-config,sys:zlib1g-dev,zig@$ZIG,cargo:cargo-zigbuild@$ZB " "--exclusive --why " \
        "--secret K2_GMAIL_CLIENT_ID=BUILD_GMAIL_CLIENT_ID" "--env BUILD_COMMIT=$COMMIT_ARM" "--cpus 8 " "-- bash scripts/ci/build-linux-bins.sh aarch64"; do
        [[ "$line" == *"$want"* ]] && pass "$lab_node aarch64 submit has $want" || fail "$lab_node aarch64 submit lacks $want" "$line"
    done
    [[ "$line" != *"rust@"* ]] && pass "$lab_node: no rust@ need (the node installs rust-toolchain.toml's channel AND its aarch64 target)" \
        || fail "$lab_node rust need" "$line"
done
grep -q 'L2 on k2n10-vin and V2 on k2n11-vin: two nodes' "$tmp/arm.collect" && grep -q 'identical in both builds: .*k2-daemon-linux-aarch64' "$tmp/arm.collect" \
    && pass "collect: V2 came from another node key and matches L2's daemon, root helper, node and engine" || fail "arm dual" "$(cat "$tmp/arm.collect")"
python3 - "$tmp/arm-dist" "$tmp/arm/staged.txt" <<'PYS' && pass "staged: both arches' binaries + .sha256, and the x86_64 packages" || fail "arm staged set" "$(ls "$tmp/arm-dist")"
import os, sys
d, staged = sys.argv[1], open(sys.argv[2]).read().split()
want = {f"{b}-linux-{a}{x}" for a in ("x86_64", "aarch64") for b in ("k2-daemon", "k2-node", "k2-root-helper", "k2-app-engine") for x in ("", ".sha256")}
want |= {"K2_9.9.9_amd64.deb", "K2-9.9.9-1.x86_64.rpm", "K2_9.9.9_amd64.AppImage", "SHA256SUMS-linux-x86_64.txt"}
assert set(os.listdir(d)) == want, sorted(set(os.listdir(d)) ^ want)
assert set(staged) == want, staged
PYS
# V2 builds other bytes: the stub's 5th run (V2) gets a salted daemon.
cat > "$tmp/salt-v2.sh" <<'SALT'
k2cr_k2() {
    if [ "${1:-} ${2:-} ${3:-}" = "compute run k2n11-vin" ]; then GEN_DAEMON_SALT=other "${K2_BIN:-k2}" "$@"; else "${K2_BIN:-k2}" "$@"; fi
}
SALT
out="$(run_arm arm-v2diff "source '$tmp/salt-v2.sh'")"
[ "$out" = "1 1 0 1" ] && grep -q 'k2-daemon-linux-aarch64: the two builds differ' "$tmp/arm-v2diff.collect" \
    && pass "V2's aarch64 daemon differing from L2's: aarch64 fails, x86_64 still verified" || fail "V2 diff" "$out $(cat "$tmp/arm-v2diff.collect")"
python3 - "$tmp/arm-v2diff-dist" <<'PYS' && pass "…and only x86_64 is staged" || fail "arm fallback staging" "$(ls "$tmp/arm-v2diff-dist")"
import os, sys
names = os.listdir(sys.argv[1])
assert "k2-daemon-linux-x86_64" in names and not any("aarch64" in n for n in names), names
PYS
out="$(K2_COMPUTE_ARM_VERIFY_NODE=k2n10-vin run_arm arm-same)"
[ "${out%% *}" = 0 ] && [ "$(wc -l < "$tmp/arm-same/jobs.tsv" | tr -d ' ')" = 0 ] && grep -q 'L2 and V2 are both on k2n10-vin' "$tmp/arm-same.submit" \
    && pass "L2 and V2 on one node: refused before anything is submitted" || fail "arm same node" "$out $(cat "$tmp/arm-same.submit")"
out="$(K2_COMPUTE_ARM_VERIFY_NODE=none run_arm arm-nov2)"
grep -q 'aarch64 on compute needs V2 on a second node' "$tmp/arm-nov2.submit" && pass "V2 off: refused (no aarch64 compute build without the dual build)" \
    || fail "arm no V2" "$out $(cat "$tmp/arm-nov2.submit")"
out="$(K2_COMPUTE_ARM_NODE=none run_arm arm-none)"
[ "$out" = "0 1 x x" ] && ! grep -q 'build-linux-bins.sh aarch64' "$tmp/calls-arm-none" && pass "K2_COMPUTE_ARM_NODE=none: no aarch64 jobs" \
    || fail "arm none" "$out"
# A release commit without the cross pins: aarch64 isn't submitted, x86_64 goes on.
export STUB_COMMIT="$COMMIT" STUB_TREE="$TREE"
rc="$(K2_COMPUTE_ARM=compute run_case arm-nopins)"
[ "$rc" = 0 ] && grep -q 'no scripts/ci/cross-pins.sh at' "$tmp/arm-nopins.submit" && ! grep -q 'aarch64' "$tmp/arm-nopins/jobs.tsv" \
    && pass "no cross pins at the release commit: no aarch64 jobs, x86_64 unaffected" || fail "arm no pins" "rc=$rc $(cat "$tmp/arm-nopins.submit")"
export K2_COMPUTE_ARM=github

echo "dual build (V1, G12):"
rc="$(run_case v-same)"
[ "$rc" = 0 ] && grep -q 'identical in both builds' "$tmp/v-same.collect" && pass "V1 identical to L1 passes" \
    || fail "V1 same" "rc=$rc $(cat "$tmp/v-same.collect")"
# The second build of k2-node differs (GEN_SALT applies to every job, so
# salt only the V1 job: the stub's 3rd run).
export STUB_DIR="$tmp/stub-v-diff"
rc="$(
    export STUB_CALLS="$tmp/calls-v-diff"; rm -rf "$STUB_DIR" "$tmp/v-diff" "$tmp/v-diff-files"; mkdir -p "$STUB_DIR"
    cd "$repo"
    source "$ROOT/scripts/lib/compute-release.sh"
    k2cr_init "$VERSION" "$COMMIT" "$tmp/v-diff" "$tmp/v-diff-files"
    k2cr_submit L1 k2n7-vin linux-bins x86_64 rust@1.98.7 -- bash scripts/ci/build-linux-bins.sh x86_64 >/dev/null
    GEN_SALT=other k2cr_submit V1 k2n9-vin linux-bins x86_64 rust@1.98.7 -- bash scripts/ci/build-linux-bins.sh x86_64 >/dev/null
    r=0; k2cr_collect >"$tmp/v-diff.collect" 2>&1 || r=$?; echo "$r"
)"
[ "$rc" != 0 ] && grep -q 'k2-node-linux-x86_64: the two builds differ' "$tmp/v-diff.collect" && pass "V1 differing from L1 fails" \
    || fail "V1 diff" "rc=$rc $(cat "$tmp/v-diff.collect")"
unset K2_COMPUTE_VERIFY_NODE
# collect_pair <name> <L1 node> <V1 node> [V1 daemon salt]: L1 and V1 by hand, then collect; echoes rc.
collect_pair() {
    local name="$1" l1="$2" v1="$3" salt="${4:-}"
    export STUB_DIR="$tmp/stub-$name"
    (
        export STUB_CALLS="$tmp/calls-$name"; rm -rf "$STUB_DIR" "${tmp:?}/$name" "${tmp:?}/$name-files"; mkdir -p "$STUB_DIR"
        cd "$repo"
        source "$ROOT/scripts/lib/compute-release.sh"
        k2cr_init "$VERSION" "$COMMIT" "$tmp/$name" "$tmp/$name-files"
        k2cr_submit L1 "$l1" linux-bins x86_64 rust@1.98.7 -- bash scripts/ci/build-linux-bins.sh x86_64 >/dev/null
        GEN_DAEMON_SALT="$salt" k2cr_submit V1 "$v1" linux-bins x86_64 rust@1.98.7 -- bash scripts/ci/build-linux-bins.sh x86_64 >/dev/null
        r=0; k2cr_collect >"$tmp/$name.collect" 2>&1 || r=$?; echo "$r"
    )
}
rc="$(collect_pair v-daemon k2n7-vin k2n9-vin trojan)"
[ "$rc" != 0 ] && grep -q 'k2-daemon-linux-x86_64: the two builds differ' "$tmp/v-daemon.collect" \
    && pass "review H1: V1's daemon differing from L1's fails collect (not only reported)" || fail "V1 daemon diff" "rc=$rc $(cat "$tmp/v-daemon.collect")"
rc="$(collect_pair v-samename k2n7-vin k2n7-vin)"
[ "$rc" != 0 ] && grep -q 'both ran on k2n7-vin' "$tmp/v-samename.collect" && grep -q 'same node key' "$tmp/v-samename.collect" \
    && pass "collect: V1 on L1's node fails (name and node_fp)" || fail "V1 same node" "rc=$rc $(cat "$tmp/v-samename.collect")"
export STUB_FP="fp-one-key-under-two-names"
rc="$(collect_pair v-samefp k2n7-vin k2n9-vin)"
unset STUB_FP
[ "$rc" != 0 ] && grep -q 'same node key' "$tmp/v-samefp.collect" && ! grep -q 'both ran on' "$tmp/v-samefp.collect" \
    && pass "collect: two node names with one node_fp fails (the receipts' keys are compared)" || fail "V1 same fp" "rc=$rc $(cat "$tmp/v-samefp.collect")"
python3 - "$tmp" <<'PYV' && pass "dual-nodes: a verdict without node_fp fails" || fail "dual-nodes no fp" ""
import json, subprocess, sys, os
t = sys.argv[1]
json.dump({"label": "L1", "node": "a", "node_fp": "fa"}, open(os.path.join(t, "dn-a.json"), "w"))
json.dump({"label": "V1", "node": "b", "node_fp": None}, open(os.path.join(t, "dn-b.json"), "w"))
p = subprocess.run([sys.executable, os.environ["PYLIB"], "dual-nodes", "--a", os.path.join(t, "dn-a.json"), "--b", os.path.join(t, "dn-b.json")],
                   stdout=subprocess.PIPE, text=True)
assert p.returncode == 1 and "no node_fp in the V1 receipt" in p.stdout, p
PYV

echo "nodes (0.46.7: the Ryzens, Yrs rule):"
export K2_COMPUTE_NODE=k2n10-vin
rc="$(run_case one-node)"
[ "$rc" = 0 ] && [ "$(grep -c 'k2 compute run k2n10-vin ' "$tmp/calls-one-node")" = 2 ] \
    && pass "K2_COMPUTE_NODE moves L1 and L3 together (k2n10-vin is a release node)" || fail "K2_COMPUTE_NODE" "rc=$rc $(cat "$tmp/calls-one-node")"
export K2_COMPUTE_X86_NODE=k2n9-vin K2_COMPUTE_VERIFY_NODE=k2n11-vin
rc="$(run_case split-node)"
grep -q 'k2 compute run k2n9-vin .*build-linux-bins.sh' "$tmp/calls-split-node" && grep -q 'k2 compute run k2n10-vin .*build-linux-app.sh' "$tmp/calls-split-node" \
    && pass "K2_COMPUTE_X86_NODE beats K2_COMPUTE_NODE for L1 only" || fail "per-job node" "$(cat "$tmp/calls-split-node")"
unset K2_COMPUTE_NODE K2_COMPUTE_X86_NODE K2_COMPUTE_VERIFY_NODE
echo "K2_COMPUTE_EXCLUSIVE (a share without exclusive, before its owner widens it):"
export K2_COMPUTE_EXCLUSIVE=0 K2_LINUX_BUILDS=shadow
rc="$(run_case noexcl-sh)"
[ "$(grep -c 'k2 compute run' "$tmp/calls-noexcl-sh")" = 3 ] && ! grep -q -- '--exclusive' "$tmp/calls-noexcl-sh" \
    && pass "shadow, K2_COMPUTE_EXCLUSIVE=0: the bins jobs go without --exclusive" || fail "noexcl shadow" "$(cat "$tmp/calls-noexcl-sh")"
export K2_LINUX_BUILDS=compute
rc="$(run_case noexcl-c)"
grep -q 'K2_COMPUTE_EXCLUSIVE=0 is for shadow and by-hand runs' "$tmp/noexcl-c.submit" && ! grep -q 'build-linux-bins.sh' "$tmp/calls-noexcl-c" \
    && pass "compute mode, K2_COMPUTE_EXCLUSIVE=0: no bins job is sent" || fail "noexcl compute" "$(cat "$tmp/noexcl-c.submit" "$tmp/calls-noexcl-c")"
export K2_COMPUTE_EXCLUSIVE=maybe K2_LINUX_BUILDS=shadow
rc="$(run_case excl-bogus)"
grep -q 'K2_COMPUTE_EXCLUSIVE must be 1 or 0' "$tmp/excl-bogus.submit" && ! grep -q 'build-linux-bins.sh' "$tmp/calls-excl-bogus" \
    && pass "K2_COMPUTE_EXCLUSIVE must be 1 or 0" || fail "excl bogus" "$(cat "$tmp/excl-bogus.submit")"
unset K2_COMPUTE_EXCLUSIVE K2_LINUX_BUILDS

echo "L1 and V1 never on one node (review: finding 2), in every mode:"
for mode in shadow compute; do
    export K2_LINUX_BUILDS="$mode" K2_COMPUTE_NODE=k2n9-vin
    rc="$(run_case "one-node-v1-$mode")"
    grep -q 'L1 and V1 are both on k2n9-vin: the dual build needs two nodes' "$tmp/one-node-v1-$mode.submit" \
        && ! grep -q 'k2 compute run' "$tmp/calls-one-node-v1-$mode" \
        && pass "$mode: K2_COMPUTE_NODE=k2n9-vin (the default V1 node) is refused before anything is submitted" \
        || fail "$mode one node" "rc=$rc $(cat "$tmp/one-node-v1-$mode.submit") $(cat "$tmp/calls-one-node-v1-$mode")"
    unset K2_LINUX_BUILDS K2_COMPUTE_NODE
done
export K2_COMPUTE_X86_NODE=k2n10-vin K2_COMPUTE_VERIFY_NODE=k2n10-vin
rc="$(run_case same-l1-v1)"
grep -q 'L1 and V1 are both on k2n10-vin' "$tmp/same-l1-v1.submit" && ! grep -q 'k2 compute run' "$tmp/calls-same-l1-v1" \
    && pass "K2_COMPUTE_X86_NODE = K2_COMPUTE_VERIFY_NODE: refused" || fail "same L1 V1" "rc=$rc $(cat "$tmp/same-l1-v1.submit")"
unset K2_COMPUTE_X86_NODE K2_COMPUTE_VERIFY_NODE
export K2_COMPUTE_NODE=k2n9-vin K2_COMPUTE_VERIFY_NODE=none
rc="$(run_case one-node-nov1)"
[ "$rc" = 0 ] && [ "$(grep -c 'k2 compute run k2n9-vin ' "$tmp/calls-one-node-nov1")" = 2 ] \
    && pass "with V1 off, one node for L1 and L3 is fine" || fail "one node, no V1" "rc=$rc $(cat "$tmp/calls-one-node-nov1")"
unset K2_COMPUTE_NODE K2_COMPUTE_VERIFY_NODE
[ "$(grep -c -- '--cpus 8 ' "$tmp/calls-ok")" = 3 ] && pass "every job asks for 8 CPUs by default (z3mbpZ's share; the node default is 4)" \
    || fail "default cpus" "$(cat "$tmp/calls-ok")"
export K2_COMPUTE_CPUS=default
rc="$(run_case cpudef)"
! grep -q -- '--cpus' "$tmp/calls-cpudef" && pass "K2_COMPUTE_CPUS=default: no --cpus (the node's default)" || fail "cpus default" "$(cat "$tmp/calls-cpudef")"
unset K2_COMPUTE_CPUS
export K2_COMPUTE_QUEUE_TIMEOUT=3h K2_COMPUTE_CPUS=8
rc="$(run_case qt)"
[ "$(grep -c -- '--queue-timeout 3h' "$tmp/calls-qt")" = 3 ] && pass "K2_COMPUTE_QUEUE_TIMEOUT reaches every job" || fail "queue timeout" "$(cat "$tmp/calls-qt")"
[ "$(grep -c -- '--cpus 8 ' "$tmp/calls-qt")" = 3 ] && pass "K2_COMPUTE_CPUS reaches every job" || fail "cpus" "$(cat "$tmp/calls-qt")"
unset K2_COMPUTE_QUEUE_TIMEOUT K2_COMPUTE_CPUS
for gone in k2n4-vin k2n5-vin k2n6-hil; do
    export K2_COMPUTE_X86_NODE="$gone"; check_tamper "t-gone-$gone" "$gone (gone) is not a release node" "not a release node"
done
# k2-arch is Arch (pacman, no apt sys: packages) and k2n12 is the fuzz node.
for notrel in k2-arch k2n12-vin; do
    export K2_COMPUTE_X86_NODE="$notrel"; check_tamper "t-notrel-$notrel" "$notrel is not a release node" "not a release node"
done
unset K2_COMPUTE_X86_NODE

echo "--shadow-only <sha> (dry run: submit, fetch, verify; nothing else):"
repo2="$tmp/repo2"
mkdir -p "$repo2/crates/k2-daemon"
git -C "$repo2" init -q
printf '[package]\nname = "k2-daemon"\nversion = "%s"\n' "$VERSION" > "$repo2/crates/k2-daemon/Cargo.toml"
git -C "$repo2" add -A
git -C "$repo2" -c user.email=t@t -c user.name=t commit -q -m candidate
C2="$(git -C "$repo2" rev-parse HEAD)"
T2="$(git -C "$repo2" rev-parse 'HEAD^{tree}')"
export STUB_DIR="$tmp/stub-so" STUB_CALLS="$tmp/calls-so"
rm -rf "$STUB_DIR"; mkdir -p "$STUB_DIR"; : > "$STUB_CALLS"
so_rc=0
(cd "$repo2" && STUB_COMMIT="$C2" STUB_TREE="$T2" K2_RELEASE_AUDIT_DIR="$tmp/audit" \
    bash "$ROOT/scripts/lib/compute-release.sh" --shadow-only "${C2:0:9}" --wait-min 1) > "$tmp/so.out" 2>&1 || so_rc=$?
so_state="$tmp/audit/shadow-${C2:0:12}/compute"
[ "$so_rc" = 0 ] && pass "shadow-only: exit 0 when every job verifies" || fail "shadow-only rc=$so_rc" "$(cat "$tmp/so.out")"
[ "$(wc -l < "$so_state/jobs.tsv" | tr -d ' ')" = 3 ] && grep -q "^version 9.9.9" "$so_state/summary.txt" \
    && pass "shadow-only: the release set, version read from the commit" || fail "shadow-only state" "$(cat "$so_state/summary.txt" "$so_state/jobs.tsv")"
grep -q "commit  $C2" "$so_state/summary.txt" && grep -q -- "--sha $C2" "$STUB_CALLS" \
    && pass "shadow-only: the short sha is resolved to the full commit" || fail "shadow-only sha" "$(cat "$STUB_CALLS")"
python3 -c 'import json,sys; [json.load(open(p))["ok"] or sys.exit(1) for p in sys.argv[1:]]' "$so_state/L1.verdict.json" "$so_state/L3.verdict.json" \
    && pass "shadow-only: receipts fetched and verified (L1, L3)" || fail "shadow-only verdicts" "$(cat "$so_state"/*.verdict.json)"
grep -q 'L1: job .* on k2n7-vin: done exit 0, queued ?, ran 1.0 min' "$tmp/so.out" \
    && pass "shadow-only: prints where and how long each job ran" || fail "shadow-only times" "$(cat "$tmp/so.out")"
! grep -q '^gh ' "$STUB_CALLS" && ! grep -qvE '^k2 compute (run|wait|get|receipt|fetch) |^k2 compute --help$' "$STUB_CALLS" \
    && pass "shadow-only: only k2 compute run/wait/get/receipt/fetch and the --tier probe (no GitHub, no cancel)" || fail "shadow-only calls" "$(cat "$STUB_CALLS")"
[ -z "$(git -C "$repo2" status --porcelain)" ] && [ "$(git -C "$repo2" tag)" = "" ] && [ "$(git -C "$repo2" rev-parse HEAD)" = "$C2" ] \
    && pass "shadow-only: the checkout is untouched (no tag, no commit, no files)" || fail "shadow-only repo" "$(git -C "$repo2" status --porcelain)"
rc=0; (cd "$repo2" && bash "$ROOT/scripts/lib/compute-release.sh" --shadow-only) >/dev/null 2>&1 || rc=$?
[ "$rc" = 2 ] && pass "shadow-only: no sha = usage (exit 2)" || fail "shadow-only usage" "rc=$rc"
rc=0; (cd "$repo2" && bash "$ROOT/scripts/lib/compute-release.sh" --shadow-only deadbeefdead) >/dev/null 2>&1 || rc=$?
[ "$rc" = 2 ] && pass "shadow-only: an unknown sha is refused (exit 2)" || fail "shadow-only bad sha" "rc=$rc"
rc=0; (cd "$repo2" && bash "$ROOT/scripts/lib/compute-release.sh" --tag v1) >/dev/null 2>&1 || rc=$?
[ "$rc" = 2 ] && pass "standalone: only --shadow-only is accepted" || fail "standalone usage" "rc=$rc"
python3 - "$ROOT/scripts/release.sh" <<'PY' && pass "release.sh: a 7.9c wait timeout defers collect to Step 10.7 (never holds the release)" || fail "late collect" "release.sh"
import sys
s = open(sys.argv[1]).read()
c = s[s.index("k2_shadow_collect() {"):s.index("k2_shadow_tree_check() {")]
r = s[s.index("k2_shadow_report() {"):]
assert "K2_SHADOW_LATE=1" in c and c.index("K2_SHADOW_LATE=1") < c.index("k2cr_collect"), "collect"
assert 'if [ "$K2_SHADOW_LATE" = "1" ]' in r and "K2_COMPUTE_LATE_WAIT_MIN" in r, "report"
assert r.index("k2cr_collect") < r.index("k2cr_compare_github"), "order"
PY

echo "shadow report vs GitHub:"
export STUB_REL="$tmp/rel" STUB_DIR="$tmp/stub-ok" STUB_CALLS="$tmp/calls-ok"
mkdir -p "$STUB_REL/v$VERSION"
# GitHub has the same daemon, a different k2-node, and no AppImage.
cp "$tmp/ok-files/L1/dist/k2-daemon-linux-x86_64" "$STUB_REL/v$VERSION/"
printf 'github-built' > "$STUB_REL/v$VERSION/k2-node-linux-x86_64"
(
    cd "$repo"
    source "$ROOT/scripts/lib/compute-release.sh"
    K2CR_STATE="$tmp/ok" K2CR_FILES="$tmp/ok-files" k2cr_compare_github "v$VERSION" > "$tmp/report.out" 2>&1
)
python3 - "$tmp/ok/shadow-report.json" <<'PY' && pass "report: per job (L1, V1, L3): same daemon, differing node, missing assets counted" || fail "report" "$(cat "$tmp/report.out")"
import json, sys
r = json.load(open(sys.argv[1]))
by = {(x["job"], x["name"]): x for x in r["files"]}
for job in ("L1", "V1"):  # V1 (the dual build) is compared too, row by row
    assert by[(job, "k2-daemon-linux-x86_64")]["match"] is True, by
    assert by[(job, "k2-node-linux-x86_64")]["match"] is False and by[(job, "k2-node-linux-x86_64")]["markers_equal"] is False, by
assert by[("L3", "K2_9.9.9_amd64.AppImage")]["match"] is None, by
assert r["summary"]["match"] == 2 and r["summary"]["differ"] == 2, r["summary"]
assert not any("build-info" in n or n.endswith((".sha256", ".sums")) for _, n in by), by
PY
grep -q 'gh release download v9.9.9 .*--pattern k2-daemon-linux-x86_64' "$tmp/calls-ok" && ! grep -q -- '--pattern \*' "$tmp/calls-ok" \
    && pass "GitHub assets fetched by exact name" || fail "gh calls" "$(grep gh "$tmp/calls-ok")"

python3 - "$PY" "$tmp" <<'PYE' && pass "elf-comment reads the toolchain lines of an ELF's .comment" || fail "elf-comment" "see above"
import os, subprocess, sys
py, tmp = sys.argv[1], sys.argv[2]
# A minimal 64-bit LE ELF: header, .comment data, .shstrtab, 3 section headers.
comment = b"GCC: (Ubuntu 13.3.0) 13.3.0\0rustc version 1.99.1 (x 2026-10-13)\0GCC: (Ubuntu 13.3.0) 13.3.0\0"
shstr = b"\0.comment\0.shstrtab\0"
body = comment + shstr
shoff = 64 + len(body)
h = bytearray(64); h[0:4] = b"\x7fELF"; h[4] = 2; h[5] = 1; h[18:20] = (62).to_bytes(2, "little")
h[40:48] = shoff.to_bytes(8, "little"); h[58:60] = (64).to_bytes(2, "little"); h[60:62] = (3).to_bytes(2, "little"); h[62:64] = (2).to_bytes(2, "little")
def sh(name, off, size):
    b = bytearray(64); b[0:4] = name.to_bytes(4, "little"); b[24:32] = off.to_bytes(8, "little"); b[32:40] = size.to_bytes(8, "little"); return bytes(b)
data = bytes(h) + body + bytes(64) + sh(1, 64, len(comment)) + sh(10, 64 + len(comment), len(shstr))
p = os.path.join(tmp, "elf-with-comment"); open(p, "wb").write(data)
out = subprocess.run([sys.executable, py, "elf-comment", p], capture_output=True, text=True, check=True).stdout.splitlines()
assert out == ["GCC: (Ubuntu 13.3.0) 13.3.0", "rustc version 1.99.1 (x 2026-10-13)"], out
PYE

echo "aborted release:"
export STUB_DIR="$tmp/stub-abort" STUB_CALLS="$tmp/calls-abort"
rm -rf "$STUB_DIR"; mkdir -p "$STUB_DIR"; : > "$STUB_CALLS"
(
    cd "$repo"
    source "$ROOT/scripts/lib/compute-release.sh"
    k2cr_init "$VERSION" "$COMMIT" "$tmp/abort" "$tmp/abort-files"
    k2cr_submit_release_set >/dev/null 2>&1
    k2cr_cancel_open 2>/dev/null
)
[ "$(grep -c 'k2 compute cancel .* --force' "$tmp/calls-abort")" = 3 ] && pass "uncollected jobs are cancelled" || fail "cancel" "$(cat "$tmp/calls-abort")"

echo "build scripts:"
for s in scripts/ci/build-linux-bins.sh scripts/ci/build-linux-app.sh scripts/ci/shadow-linux-builds.sh scripts/ci/repro-build-env.sh scripts/lib/compute-release.sh scripts/release.sh; do
    bash -n "$ROOT/$s" && pass "bash -n $s" || fail "bash -n $s" "syntax"
done
# repro-build-env.sh: the same mappings, most specific last; rustc through
# RUSTFLAGS, C/C++ through compiler wrappers (never CFLAGS: vendored OpenSSL
# bakes "compiler: cc <CFLAGS>" into the daemon); SOURCE_DATE_EPOCH from the
# commit.
fake="$tmp/repro"; mkdir -p "$fake/src/target" "$fake/cargo" "$fake/realbin"
git -C "$fake/src" init -q
GIT_COMMITTER_DATE="2026-10-01T00:00:00Z" git -C "$fake/src" -c user.email=t@t -c user.name=t commit -q --allow-empty -m t
CT="$(git -C "$fake/src" log -1 --format=%ct)"
for t in cc c++ gcc g++ aarch64-linux-gnu-gcc; do
    printf '#!/bin/sh\necho "REAL %s $*"\n' "$t" > "$fake/realbin/$t"; chmod +x "$fake/realbin/$t"
done
rf="$(cd "$fake/src" && env -u RUSTFLAGS -u CFLAGS -u CXXFLAGS -u CARGO_TARGET_DIR -u SOURCE_DATE_EPOCH -u CC -u CXX \
    PATH="$fake/realbin:$PATH" CARGO_HOME="$fake/cargo" CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    bash -c 'source "$1"; k2_repro_build_env "$PWD" >/dev/null
        printf "%s\n%s\n%s\n%s\n" "$RUSTFLAGS" "${CFLAGS:-}" "${CXXFLAGS:-}" "$SOURCE_DATE_EPOCH"
        printf "%s\n" "$(command -v cc)"
        cc -c x.c -o x.o; c++ -c y.cc; aarch64-linux-gnu-gcc -c z.c' _ "$ROOT/scripts/ci/repro-build-env.sh")"
src_p="$(cd "$fake/src" && pwd -P)"
python3 - "$rf" "$src_p" "$fake/cargo" "$CT" <<'PY' && pass "repro env: rustc remaps (target last); C/C++ maps come from wrappers, not CFLAGS; SOURCE_DATE_EPOCH = commit time" || fail "repro env" "$rf"
import sys
lines = sys.argv[1].split("\n")
rust, c, cxx, sde, cc_path = lines[:5]
calls = lines[5:8]
src, cargo, ct = sys.argv[2], sys.argv[3], sys.argv[4]
r = rust.split()
assert f"--remap-path-prefix={src}=/k2" in r, r
assert f"--remap-path-prefix={cargo}=/cargo" in r, r
assert f"--remap-path-prefix={src}/target=/target" in r, r
assert r.index(f"--remap-path-prefix={src}/target=/target") > r.index(f"--remap-path-prefix={src}=/k2"), r
assert c == "" and cxx == "", ("CFLAGS/CXXFLAGS must carry no paths (OpenSSL records them)", c, cxx)
assert sde == ct, (sde, ct)
assert "/k2-repro-cc." in cc_path and cc_path.endswith("/cc"), cc_path
for call, tool, tail in zip(calls, ("cc", "c++", "aarch64-linux-gnu-gcc"), ("-c x.c -o x.o", "-c y.cc", "-c z.c")):
    w = call.split()
    assert w[:2] == ["REAL", tool], call
    assert f"-ffile-prefix-map={src}=/k2" in w and f"-ffile-prefix-map={src}/target=/target" in w, call
    assert w.index(f"-ffile-prefix-map={src}/target=/target") > w.index(f"-ffile-prefix-map={src}=/k2"), call
    assert call.endswith(tail), call
PY
sde2="$(cd "$fake/src" && env SOURCE_DATE_EPOCH=12345 PATH="$fake/realbin:$PATH" CARGO_HOME="$fake/cargo" bash -c 'source "$1"; k2_repro_build_env "$PWD" >/dev/null; echo "$SOURCE_DATE_EPOCH"' _ "$ROOT/scripts/ci/repro-build-env.sh")"
[ "$sde2" = 12345 ] && pass "repro env keeps a SOURCE_DATE_EPOCH the caller set" || fail "sde kept" "$sde2"
# V8: k2-node's build commit comes from the checkout (the same for compute
# and GitHub builds), a caller's own is kept, a malformed one is refused.
nc="$(cd "$fake/src" && env -u K2_NODE_BUILD_COMMIT PATH="$fake/realbin:$PATH" CARGO_HOME="$fake/cargo" bash -c 'source "$1"; k2_repro_build_env "$PWD" >/dev/null; echo "$K2_NODE_BUILD_COMMIT"' _ "$ROOT/scripts/ci/repro-build-env.sh")"
[ "$nc" = "$(git -C "$fake/src" rev-parse HEAD)" ] && pass "repro env sets K2_NODE_BUILD_COMMIT from the checkout" || fail "node commit" "$nc"
nc2="$(cd "$fake/src" && env K2_NODE_BUILD_COMMIT=0123456789abcdef0123456789abcdef01234567 PATH="$fake/realbin:$PATH" CARGO_HOME="$fake/cargo" bash -c 'source "$1"; k2_repro_build_env "$PWD" >/dev/null; echo "$K2_NODE_BUILD_COMMIT"' _ "$ROOT/scripts/ci/repro-build-env.sh")"
[ "$nc2" = 0123456789abcdef0123456789abcdef01234567 ] && pass "repro env keeps a K2_NODE_BUILD_COMMIT the caller set" || fail "node commit kept" "$nc2"
if (cd "$fake/src" && env K2_NODE_BUILD_COMMIT=nope CARGO_HOME="$fake/cargo" bash -c 'source "$1"; k2_repro_build_env "$PWD"' _ "$ROOT/scripts/ci/repro-build-env.sh" >/dev/null 2>&1); then
    fail "node commit malformed" "accepted"
else
    pass "repro env refuses a malformed K2_NODE_BUILD_COMMIT"
fi
rf2="$(env RUSTFLAGS="-C keep" CARGO_HOME="$fake/cargo" bash -c 'source "$1"; k2_repro_build_env "$2" >/dev/null; printf "%s" "$RUSTFLAGS"' _ "$ROOT/scripts/ci/repro-build-env.sh" "$fake/src")"
[[ "$rf2" == "-C keep --remap-path-prefix="* ]] && pass "repro env keeps the caller's RUSTFLAGS first" || fail "repro env append" "$rf2"
mkdir -p "$fake/with space"
if (env CARGO_HOME="$fake/cargo" bash -c 'source "$1"; k2_repro_build_env "$2"' _ "$ROOT/scripts/ci/repro-build-env.sh" "$fake/with space" >/dev/null 2>&1); then
    fail "repro env whitespace" "a path with a space was accepted"
else
    pass "repro env refuses a path with whitespace"
fi
for f in scripts/ci/build-linux-bins.sh scripts/ci/build-linux-app.sh .github/workflows/daemon-binaries.yml .github/workflows/app-linux.yml; do
    grep -q 'k2_repro_build_env' "$ROOT/$f" && pass "$f uses the shared path remap" || fail "$f remap" "no k2_repro_build_env"
done
python3 - "$ROOT/scripts/ci/build-linux-app.sh" <<'PY' && pass "build-linux-app sets CI=true before tauri build (k2-node sets CI=1)" || fail "CI=true" "missing or after tauri build"
import sys
s = open(sys.argv[1]).read()
assert "\nexport CI=true\n" in s and s.index("\nexport CI=true\n") < s.index("bunx tauri build"), "CI=true"
PY
out="$(bash "$ROOT/scripts/ci/build-linux-bins.sh" sparc 2>&1 || true)"
[[ "$out" == *usage* ]] && pass "build-linux-bins refuses an unknown arch" || fail "arch usage" "$out"
if [ "$(uname -s)" != "Linux" ]; then
    out="$(bash "$ROOT/scripts/ci/build-linux-bins.sh" x86_64 2>&1 || true)"
    [[ "$out" == *"builds Linux binaries on Linux"* ]] && pass "build-linux-bins refuses a non-Linux host" || fail "host check" "$out"
elif [ "$(uname -m)" = aarch64 ]; then
    out="$(bash "$ROOT/scripts/ci/build-linux-bins.sh" x86_64 2>&1 || true)"
    [[ "$out" == *"Build x86_64 natively"* ]] && pass "build-linux-bins refuses x86_64 on an aarch64 host" || fail "cross check" "$out"
else
    # aarch64 on x86_64 is the zig cross build: it refuses a zig or
    # cargo-zigbuild that isn't cross-pins.sh's, before any cargo runs.
    fake="$tmp/fakezig"; mkdir -p "$fake"
    printf '#!/bin/sh\necho 0.0.1\n' > "$fake/zig"
    printf '#!/bin/sh\necho "cargo-zigbuild 0.0.1"\n' > "$fake/cargo-zigbuild"
    chmod +x "$fake/zig" "$fake/cargo-zigbuild"
    out="$(env -u K2_GMAIL_CLIENT_ID -u K2_GMAIL_CLIENT_SECRET -u K2_MICROSOFT_CLIENT_ID PATH="$fake:$PATH" BUILD_OAUTH=placeholder \
        bash "$ROOT/scripts/ci/build-linux-bins.sh" aarch64 --out "$tmp/xout" 2>&1 || true)"
    [[ "$out" == *"zig is 0.0.1; scripts/ci/cross-pins.sh pins"* ]] && [[ "$out" != *Compiling* ]] \
        && pass "build-linux-bins: the aarch64 cross build refuses an unpinned zig before any cargo" || fail "cross pin check" "$out"
fi
python3 - "$ROOT/scripts/ci/build-linux-bins.sh" <<'PY2' && pass "build-linux-bins: cross mode only for aarch64 on x86_64; cargo build → cargo zigbuild <triple>.<glibc>; C path maps in CFLAGS_<triple>" || fail "cross wiring" ""
import sys
s = open(sys.argv[1]).read()
assert 'if [ "$HOST_ARCH" = "x86_64" ] && [ "$ARCH" = "aarch64" ]; then\n        CROSS=1' in s
assert 'source "$ROOT/scripts/ci/cross-pins.sh"' in s
assert 'a+=(--target "$TRIPLE.$CROSS_GLIBC")' in s and 'command cargo zigbuild "${a[@]}"' in s
assert '$K2_REPRO_CFLAGS' in s
assert s.index('k2_repro_build_env "$ROOT"') < s.index('if [ "$CROSS" = 1 ]; then') < s.index('k2_ptyd_build_for_embed "$TRIPLE"')
PY2

echo ""
echo "compute-release: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
