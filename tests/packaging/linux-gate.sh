#!/usr/bin/env bash
# release.sh Step 0.6 / build-app.sh Step 0: scripts/linux-build-gate.sh,
# with a stub `k2` on PATH and a scratch git checkout. The gate runs as a
# K2 Compute job by default (no GitHub runner, no ssh box); this checks the
# job it sends. Never runs cargo, ssh, rsync or a real k2. No skips.
#
# Run with: bash tests/packaging/linux-gate.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# A scratch checkout holding the gate and its libs, pinned to a version
# unlike the repo's own (the pin is read at HEAD).
co="$tmp/co"
mkdir -p "$co/scripts/lib"
cp "$ROOT/scripts/linux-build-gate.sh" "$co/scripts/"
cp "$ROOT/scripts/lib/rust-pin.sh" "$ROOT/scripts/lib/compute-tier.sh" "$co/scripts/lib/"
printf '[toolchain]\nchannel = "1.98.7"\n' > "$co/rust-toolchain.toml"
git -C "$co" init -q
git -C "$co" add -A
git -C "$co" -c user.email=t@t -c user.name=t commit -q -m gate
HEAD_SHA="$(git -C "$co" rev-parse HEAD)"

mkdir -p "$tmp/bin"
cat > "$tmp/bin/k2" <<'STUB'
#!/usr/bin/env bash
echo "k2 $*" >> "$STUB_CALLS"
case "$1 $2" in
    "compute --help")
        echo "k2 compute — help"; echo "  [--tier standard|priority|batch|ci]"
        [ "${STUB_AVOID:-0}" = 1 ] && echo "  [--avoid <node>]..."
        exit 0
        ;;
    "compute nodes")
        python3 -c 'import json,sys; print(json.dumps({"nodes": [{"name": n} for n in sys.argv[1].split(",") if n]}))' "${STUB_NODES:-k2n7-vin}"
        exit 0
        ;;
    "compute run") exit "${STUB_RC:-0}" ;;
esac
exit 2
STUB
chmod +x "$tmp/bin/k2"

gate() { # <name> [env…]: runs the gate; echoes rc
    local name="$1"; shift
    export STUB_CALLS="$tmp/calls-$name"
    : > "$STUB_CALLS"
    local rc=0
    env PATH="$tmp/bin:$PATH" "$@" bash "$co/scripts/linux-build-gate.sh" > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}
runline() { grep '^k2 compute run ' "$tmp/calls-$1" || true; }

echo "compute (the default):"
rc="$(gate ok)"
line="$(runline ok)"
[ "$rc" = 0 ] && grep -q "Linux build gate passed" "$tmp/out-ok" && pass "exit 0 when the job passes" || fail "ok rc=$rc" "$(cat "$tmp/out-ok")"
for want in "compute run --any --os linux --arch x86_64 --sha $HEAD_SHA" "--tier ci" "--needs rust@1.98.7,cmake@3," \
    "sys:libasound2-dev" "sys:libwebkit2gtk-4.1-dev" "sys:libgtk-3-dev" "--mem 20G" "--wait --failures" \
    "-- sh -c ./scripts/fetch-frpc.sh && RUSTFLAGS='-D warnings' cargo check --workspace --all-targets"; do
    [[ "$line" == *"$want"* ]] && pass "job has: $want" || fail "job lacks: $want" "$line"
done
! [[ "$line" == *--dirty* ]] && ! [[ "$line" == *--avoid* ]] && ! [[ "$line" == *rust@stable* ]] \
    && pass "a clean checkout: no --dirty; old CLI: no --avoid; never rust@stable" || fail "clean extras" "$line"

echo "--dirty ships uncommitted changes:"
printf 'x\n' > "$co/new-file.txt"
rc="$(gate dirty)"
[ "$rc" = 0 ] && [[ "$(runline dirty)" == *" --dirty "* ]] && grep -q "uncommitted changes" "$tmp/out-dirty" \
    && pass "a dirty checkout: --dirty, and it says so" || fail "dirty" "rc=$rc $(runline dirty)"
rm -f "$co/new-file.txt"

echo "placement:"
rc="$(gate avoid STUB_AVOID=1 STUB_NODES=k2n7-vin,k2-arch)"
[ "$rc" = 0 ] && [[ "$(runline avoid)" == *"--arch x86_64 --sha $HEAD_SHA --tier ci --avoid k2-arch "* ]] \
    && pass "k2-arch granted: --avoid k2-arch (Arch has no Debian sys: packages)" || fail "avoid" "rc=$rc $(runline avoid)"
rc="$(gate avoid-ungranted STUB_AVOID=1 STUB_NODES=k2n7-vin)"
[ "$rc" = 0 ] && ! [[ "$(runline avoid-ungranted)" == *--avoid* ]] \
    && pass "k2-arch not granted: no --avoid" || fail "avoid ungranted" "$(runline avoid-ungranted)"
rc="$(gate ws K2_LINUX_GATE_WORKSPACE=pod-leader K2_LINUX_GATE_TIER=standard)"
[ "$rc" = 0 ] && [[ "$(runline ws)" == *"--tier standard"* ]] && [[ "$(runline ws)" == *"--workspace pod-leader"* ]] \
    && pass "K2_LINUX_GATE_WORKSPACE and K2_LINUX_GATE_TIER reach the job" || fail "ws/tier" "$(runline ws)"

echo "failures are loud:"
rc="$(gate fail STUB_RC=101)"
[ "$rc" = 101 ] && grep -q "FATAL: Linux build gate failed on K2 Compute (exit 101)" "$tmp/out-fail" && grep -q "K2_LINUX_GATE=ssh" "$tmp/out-fail" \
    && ! grep -q "gate passed" "$tmp/out-fail" && pass "a failed job: its exit code, and the ways out" || fail "fail" "rc=$rc $(cat "$tmp/out-fail")"
rc="$(gate infra STUB_RC=3)"
[ "$rc" = 3 ] && pass "an infra refusal (exit 3) stops the gate too" || fail "infra" "rc=$rc"
rc="$(gate badmode K2_LINUX_GATE=github)"
[ "$rc" = 2 ] && grep -q "must be compute or ssh" "$tmp/out-badmode" && pass "an unknown K2_LINUX_GATE is refused" || fail "bad mode" "rc=$rc"
rc="$(gate skip K2_SKIP_LINUX_GATE=1)"
[ "$rc" = 0 ] && grep -q "SKIPPED" "$tmp/out-skip" && [ ! -s "$tmp/calls-skip" ] && pass "K2_SKIP_LINUX_GATE=1: loud, nothing sent" || fail "skip" "rc=$rc"

echo "callers:"
grep -q '^"$PROJECT_DIR/scripts/linux-build-gate.sh"$' "$ROOT/scripts/release.sh" && pass "release.sh Step 0.6 calls the gate" || fail "release.sh" "no call"
grep -q 'scripts/linux-build-gate.sh" || { echo "FATAL: Linux build gate failed"' "$ROOT/scripts/build-app.sh" && pass "build-app.sh Step 0 calls the gate" || fail "build-app.sh" "no call"
bash -n "$ROOT/scripts/linux-build-gate.sh" && pass "bash -n linux-build-gate.sh" || fail "bash -n" "gate"

echo ""
echo "linux-gate: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
