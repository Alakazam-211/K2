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
git -C "$repo" -c user.email=t@t -c user.name=t commit -q --allow-empty -m release
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
v = "1.0.0" if tamper == "marker_version" else version
if kind == "linux-bins":
    files[f"k2-daemon-linux-{arch}"] = elf(b"daemon " + ptyd.encode())
    node = b"node K2_NODE_BUILD v=%s proto=3 os=linux arch=%s;" % (v.encode(), arch.encode())
    if tamper == "test_keys":
        node += b" K2_NODE_TEST_KEYS_ENABLED"
    files[f"k2-node-linux-{arch}"] = elf(node)
    files[f"k2-root-helper-linux-{arch}"] = elf(b"helper K2_ROOT_HELPER_BUILD v=%s proto=9 arch=%s;" % (v.encode(), arch.encode()))
    files[f"k2-app-engine-linux-{arch}"] = elf(b"engine K2_APP_ENGINE_BUILD v=%s proto=1 arch=%s;" % (v.encode(), arch.encode()))
    if os.environ.get("GEN_SALT"):
        files[f"k2-node-linux-{arch}"] += os.environ["GEN_SALT"].encode()
else:
    files[f"K2_{version}_amd64.deb"] = b"!<arch>\ndebian-binary   " + b"x" * 40
    files[f"K2-{version}-1.x86_64.rpm"] = b"\xed\xab\xee\xdb" + b"r" * 40
    head = bytearray(elf(b"appimage"))
    head[8:11] = b"AI\x02"
    files[f"K2_{version}_amd64.AppImage"] = bytes(head)
meta = {}
for n, data in files.items():
    open(os.path.join(out, "dist", n), "wb").write(data)
    meta[n] = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
info = {"kind": kind, "version": "0.0.1" if tamper == "info_version" else version, "arch": arch,
        "commit": commit, "tree": tree, "files": meta}
if kind == "linux-bins":
    info.update(oauth="placeholder", ptyd_sha256=ptyd)
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
                --sha|--artifact|--needs|--mem|--queue-timeout|--retries|--env|--workspace) shift 2 ;;
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
    "node_fp": "fp-" + open(os.path.join(d, "node")).read().strip(),
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
    rm -rf "$STUB_DIR" "$tmp/$name" "$tmp/$name-files"
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

echo "happy path:"
unset STUB_TAMPER K2_COMPUTE_X86_NODE K2_COMPUTE_VERIFY_NODE K2_COMPUTE_ARM_NODE GEN_TAMPER GEN_SALT
rc="$(run_case ok)"
[ "$rc" = 0 ] && pass "collect verifies L1 and L3" || fail "collect rc=$rc" "$(cat "$tmp/ok.collect")"
[ "$(wc -l < "$tmp/ok/jobs.tsv" | tr -d ' ')" = 2 ] && pass "two jobs (L1 x86 bins, L3 app)" || fail "jobs.tsv" "$(cat "$tmp/ok/jobs.tsv")"
grep -q 'L2 aarch64: no ARM node yet' "$tmp/ok.submit" && pass "aarch64 seam is named" || fail "aarch64 note" "$(cat "$tmp/ok.submit")"
run_line="$(grep -m1 'k2 compute run k2n4-vin' "$tmp/calls-ok" || true)"
for want in "--sha $COMMIT" "--gate" "--detach" "--artifact dist/**" "--env BUILD_OAUTH=placeholder" "-- bash scripts/ci/build-linux-bins.sh x86_64"; do
    [[ "$run_line" == *"$want"* ]] && pass "L1 submit has $want" || fail "L1 submit lacks $want" "$run_line"
done
! grep -qE -- '--env (K2_|K2SO_)' "$tmp/calls-ok" && pass "no K2_* name in --env (nodes refuse them: env_k2_reserved)" \
    || fail "K2_* env" "$(grep -- '--env K2' "$tmp/calls-ok")"
grep -q 'k2 compute run k2n5-vin .*bun@1.2.*sys:libwebkit2gtk-4.1-dev.*-- bash scripts/ci/build-linux-app.sh' "$tmp/calls-ok" \
    && pass "L3 goes to k2n5-vin with bun + GTK packages" || fail "L3 submit" "$(cat "$tmp/calls-ok")"
grep -q 'k2 compute fetch .* --over-relay --json' "$tmp/calls-ok" && pass "fetch passes --over-relay (AppImage > 100 MB)" || fail "fetch flags" "$(cat "$tmp/calls-ok")"
python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); assert v["ok"] and v["oauth"]=="placeholder" and v["node"]=="k2n4-vin", v' "$tmp/ok/L1.verdict.json" \
    && pass "L1 verdict: ok, node, oauth mode recorded" || fail "L1 verdict" "$(cat "$tmp/ok/L1.verdict.json")"

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
unset GEN_TAMPER
export K2_COMPUTE_X86_NODE=stranger-1; check_tamper t-node "a node off the release list" "not a release node"
unset K2_COMPUTE_X86_NODE
export K2_COMPUTE_REQUIRE_BAKED_OAUTH=1; check_tamper t-oauth "a placeholder-OAuth daemon when baked is required" "oauth 'placeholder'"
unset K2_COMPUTE_REQUIRE_BAKED_OAUTH

echo "dual build (V1, G12):"
export K2_COMPUTE_VERIFY_NODE=k2n6-hil
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
    k2cr_submit L1 k2n4-vin linux-bins x86_64 rust@stable -- bash scripts/ci/build-linux-bins.sh x86_64 >/dev/null
    GEN_SALT=other k2cr_submit V1 k2n6-hil linux-bins x86_64 rust@stable -- bash scripts/ci/build-linux-bins.sh x86_64 >/dev/null
    r=0; k2cr_collect >"$tmp/v-diff.collect" 2>&1 || r=$?; echo "$r"
)"
[ "$rc" != 0 ] && grep -q 'k2-node-linux-x86_64: the two builds differ' "$tmp/v-diff.collect" && pass "V1 differing from L1 fails" \
    || fail "V1 diff" "rc=$rc $(cat "$tmp/v-diff.collect")"
unset K2_COMPUTE_VERIFY_NODE

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
python3 - "$tmp/ok/shadow-report.json" <<'PY' && pass "report: same daemon, differing node, missing assets counted" || fail "report" "$(cat "$tmp/report.out")"
import json, sys
r = json.load(open(sys.argv[1]))
by = {x["name"]: x for x in r["files"]}
assert by["k2-daemon-linux-x86_64"]["match"] is True, by
assert by["k2-node-linux-x86_64"]["match"] is False and by["k2-node-linux-x86_64"]["markers_equal"] is False, by
assert by["K2_9.9.9_amd64.AppImage"]["match"] is None, by
assert r["summary"]["match"] == 1 and r["summary"]["differ"] == 1, r["summary"]
assert not any("build-info" in n or n.endswith(".sha256") for n in by), by
PY
grep -q 'gh release download v9.9.9 .*--pattern k2-daemon-linux-x86_64' "$tmp/calls-ok" && ! grep -q -- '--pattern \*' "$tmp/calls-ok" \
    && pass "GitHub assets fetched by exact name" || fail "gh calls" "$(grep gh "$tmp/calls-ok")"

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
[ "$(grep -c 'k2 compute cancel .* --force' "$tmp/calls-abort")" = 2 ] && pass "uncollected jobs are cancelled" || fail "cancel" "$(cat "$tmp/calls-abort")"

echo "build scripts:"
for s in scripts/ci/build-linux-bins.sh scripts/ci/build-linux-app.sh scripts/ci/shadow-linux-builds.sh scripts/lib/compute-release.sh scripts/release.sh; do
    bash -n "$ROOT/$s" && pass "bash -n $s" || fail "bash -n $s" "syntax"
done
out="$(bash "$ROOT/scripts/ci/build-linux-bins.sh" sparc 2>&1 || true)"
[[ "$out" == *usage* ]] && pass "build-linux-bins refuses an unknown arch" || fail "arch usage" "$out"
if [ "$(uname -s)" != "Linux" ]; then
    out="$(bash "$ROOT/scripts/ci/build-linux-bins.sh" x86_64 2>&1 || true)"
    [[ "$out" == *"builds Linux binaries on Linux"* ]] && pass "build-linux-bins refuses a non-Linux host" || fail "host check" "$out"
else
    other=aarch64; [ "$(uname -m)" = aarch64 ] && other=x86_64
    out="$(bash "$ROOT/scripts/ci/build-linux-bins.sh" "$other" 2>&1 || true)"
    [[ "$out" == *"Build each arch natively"* ]] && pass "build-linux-bins refuses cross builds" || fail "cross check" "$out"
fi

echo ""
echo "compute-release: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
