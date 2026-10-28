#!/usr/bin/env bash
# scripts/sew-pins-from-release.sh over a synthetic release (never Sew
# code): the core entry plus the 0.5.0-on App kit row and a provenance line; a pre-release entry
# pins like any release; a kit hash that disagrees with SHA256SUMS or the bytes,
# or a kit without kit.json, refuses (exit 1, nothing on stdout).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PIN="$PROJECT_ROOT/scripts/sew-pins-from-release.sh"
KIT_FIXTURE="$PROJECT_ROOT/crates/k2-root-proto/testdata/sew/sew-app-kit-0.0.2.tar.gz"
[ -f "$KIT_FIXTURE" ] || { echo "FAIL: missing $KIT_FIXTURE" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-sew-pins-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# make <dir> <mode>: a 0.0.2 release with one x86_64 core tarball and the
# fixture kit. mode: ok | prerelease | kit-sums-wrong | kit-bytes-wrong | kit-no-json
make() {
	python3 -I - "$1" "$2" "$KIT_FIXTURE" <<'PY'
import gzip, hashlib, io, json, os, shutil, sys, tarfile
d, mode, fixture = sys.argv[1:4]
os.makedirs(d)
sha = lambda b: hashlib.sha256(b).hexdigest()
def tgz(members):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as t:
        for name, data in members:
            ti = tarfile.TarInfo(name); ti.size = len(data); ti.mode = 0o755
            t.addfile(ti, io.BytesIO(data))
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode="wb", mtime=0) as g:
        g.write(raw.getvalue())
    return out.getvalue()
binary = b"\x7fELF fake sew"
core_name = "sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz"
core = tgz([("sew", binary), ("LICENSE", b"Apache-2.0")])
open(os.path.join(d, core_name), "wb").write(core)
kit_name = "sew-app-kit-0.0.2.tar.gz"
kit = open(fixture, "rb").read()
if mode == "kit-no-json":
    kit = tgz([("sew-app-kit-0.0.2/README", b"no kit.json")])
kit_sha = sha(kit)
open(os.path.join(d, kit_name), "wb").write(kit if mode != "kit-bytes-wrong" else kit + b"x")
rel = {"version": "0.0.2",
       "artifacts": [{"arch": "x86_64", "triple": "x86_64-unknown-linux-gnu", "file": core_name,
                      "sha256": sha(core), "binary_sha256": sha(binary)}],
       "app_kit": {"file": kit_name, "sha256": kit_sha}}
if mode == "prerelease":
    rel["prerelease"] = True
json.dump(rel, open(os.path.join(d, "sew-0.0.2-release.json"), "w"))
sums_kit = kit_sha if mode != "kit-sums-wrong" else "0" * 64
open(os.path.join(d, "SHA256SUMS"), "w").write(
    f"{sha(core)}  {core_name}\n{sha(binary)}  {core_name[:-7]}/sew\n{sums_kit}  {kit_name}\n")
PY
}

kit_sha="$(python3 -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$KIT_FIXTURE")"

make "$WORK/ok" ok
out="$(bash "$PIN" "$WORK/ok" 2>"$WORK/err")" && rc=0 || rc=$?
if [ "$rc" = 0 ] && printf '%s' "$out" | grep -Fq 'version: "0.0.2"' && printf '%s' "$out" | grep -Fq "SewAppKit { version: \"0.0.2\", sha256: \"$kit_sha\" },"; then
	ok "a release with a kit prints the entry and the kit row"
else
	bad "ok release (rc=$rc): $out $(cat "$WORK/err")"
fi
sums_sha="$(python3 -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$WORK/ok/SHA256SUMS")"
if printf '%s' "$out" | grep -Fq "// provenance: sew-releases tag v0.0.2, SHA256SUMS sha256 $sums_sha, sew-0.0.2-release.json sha256 "; then
	ok "the provenance line names the tag and the SHA256SUMS hash"
else
	bad "provenance: $out"
fi

make "$WORK/pre" prerelease
out="$(bash "$PIN" "$WORK/pre" 2>"$WORK/err")" && rc=0 || rc=$?
if [ "$rc" = 0 ] && printf '%s' "$out" | grep -Fq "SewAppKit" && printf '%s' "$out" | grep -Fq "tag v0.0.2, published as a pre-release,"; then
	ok "a pre-release pins like any release, recorded as one"
else
	bad "pre-release (rc=$rc): $out $(cat "$WORK/err")"
fi

for mode in kit-sums-wrong kit-bytes-wrong kit-no-json; do
	make "$WORK/$mode" "$mode"
	out="$(bash "$PIN" "$WORK/$mode" 2>"$WORK/err")" && rc=0 || rc=$?
	if [ "$rc" = 1 ] && [ -z "$out" ] && grep -Fq "sew-app-kit-0.0.2.tar.gz" "$WORK/err"; then
		ok "$mode refuses"
	else
		bad "$mode (rc=$rc): $out $(cat "$WORK/err")"
	fi
done

echo "sew-pins-from-release: $pass passed, $fail failed"
[ "$fail" = 0 ]
