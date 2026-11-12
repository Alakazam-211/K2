#!/usr/bin/env bash
# release.sh Step 0.5 on K2 Compute (scripts/lib/release-gates.sh) and
# `release.sh --gates-only`, with a stub `k2` on PATH. Never runs cargo,
# bun, vitest or a real k2. No skips.
#
#  - every gate is its own detached job at the given commit, --tier ci,
#    with set -o pipefail in its command;
#  - one pass/fail line per gate with its job id, read from the job's
#    result AND its test-result line (exit 0 without one is a failure);
#  - a failed gate prints the job's --failures summary and fails;
#  - a node problem (no exit code) is told apart from a gate failure;
#  - compute unreachable is detected before anything is sent;
#  - release.sh --gates-only runs only the gates (no lock, no signing).
#
# Run with: bash tests/packaging/release-gates.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# A scratch checkout with the libs and a Rust pin unlike the repo's.
co="$tmp/co"
mkdir -p "$co/scripts/lib"
cp "$ROOT/scripts/release.sh" "$co/scripts/"
for f in release-gates.sh rust-pin.sh compute-tier.sh linux-gate-spec.sh; do
    [ -f "$ROOT/scripts/lib/$f" ] && cp "$ROOT/scripts/lib/$f" "$co/scripts/lib/"
done
printf '[toolchain]\nchannel = "1.98.7"\n' > "$co/rust-toolchain.toml"
git -C "$co" init -q
git -C "$co" add -A
git -C "$co" -c user.email=t@t -c user.name=t commit -q -m gates
SHA="$(git -C "$co" rev-parse HEAD)"

# The stub k2. A job's id is its gate's name padded to 32 hex-ish chars;
# STUB_FAIL / STUB_INFRA / STUB_NOLINE name gates that fail, lose their
# node, or end 0 without a result line. Every call is logged.
mkdir -p "$tmp/bin"
cat > "$tmp/bin/k2" <<'STUB'
#!/usr/bin/env bash
echo "k2 $*" >> "$STUB_CALLS"
gate_of() { # the gate a run line is for, from its command
    case "$*" in
        *ratchet.sh*) echo ratchet ;;
        *vitest*) echo vitest ;;
        *typecheck:web*) echo typecheck-web ;;
        *vite:build:web*) echo vite-build-web ;;
        *FRPC_SRC=*) echo cargo-macos ;;
        *"cargo check"*) echo cargo-linux ;;
        *) echo unknown ;;
    esac
}
id_of() { printf '%s%0*d' "$1" $((32 - ${#1})) 0; }
gate_of_id() { echo "${1%%0000*}"; }
has() { case ",${2:-}," in *",$1,"*) return 0 ;; esac; return 1; }
case "$1 $2" in
    "compute --help") echo "k2 compute — help"; echo "  [--tier standard|priority|batch|ci]"; echo "  [--avoid <node>]..."; exit 0 ;;
    "compute nodes")
        [ "${STUB_DOWN:-0}" = 1 ] && { echo '{"error":"unreachable"}' >&2; exit 3; }
        echo '{"nodes":[{"name":"k2n7-vin","os":"linux","arch":"x86_64","online":true,"state":"active"},{"name":"k2n1-m4","os":"macos","arch":"arm64","online":'"${STUB_M4_ONLINE:-true}"',"state":"active"},{"name":"k2-arch","os":"linux","arch":"x86_64","online":true,"state":"active"}]}'
        exit 0 ;;
    "compute run")
        g="$(gate_of "$@")"
        case ",${STUB_REFUSE:-}," in *",$g,"*) echo '{"error":"ci_not_allowed"}' >&2; exit 3 ;; esac
        echo "RUN $g $*" >> "$STUB_CALLS"
        printf '{"job":"%s","node":"stub"}\n' "$(id_of "$g")"
        exit 0 ;;
    "compute wait") exit 0 ;;
    "compute cancel") exit 0 ;;
    "compute get")
        g="$(gate_of_id "$3")"
        if has "$g" "${STUB_INFRA:-}"; then
            printf '{"job":"%s","state":"failed","exit":null,"reason":"mem_cap","node":"k2n7-vin","queueSecs":5,"durationSecs":60,"endedAt":1}\n' "$3"
        elif has "$g" "${STUB_FAIL:-}"; then
            printf '{"job":"%s","state":"done","exit":1,"reason":null,"node":"k2n7-vin","queueSecs":5,"durationSecs":120,"endedAt":1}\n' "$3"
        else
            printf '{"job":"%s","state":"done","exit":0,"reason":null,"node":"k2n7-vin","queueSecs":5,"durationSecs":240,"endedAt":1}\n' "$3"
        fi
        exit 0 ;;
    "compute logs")
        g="$(gate_of_id "$3")"
        if [ "${4:-}" = "--failures" ]; then [ "${STUB_NO_FAILURES:-0}" = 1 ] || echo "FAILURE-SUMMARY-FOR-$g"; exit 0; fi
        has "$g" "${STUB_NOLINE:-}" && { echo "some output"; exit 0; }
        if has "$g" "${STUB_FAIL:-}"; then
            case "$g" in vitest) echo "      Tests  1 failed | 12254 passed (12255)" ;; *) echo "error: could not compile" ;; esac
            exit 0
        fi
        case "$g" in
            vitest) echo " Test Files  954 passed (954)"; echo "      Tests  12255 passed | 2 skipped (12257)" ;;
            cargo-*) echo "    Finished \`dev\` profile [optimized + debuginfo] target(s) in 2m 01s" ;;
            typecheck-web) echo "typecheck:web: no errors" ;;
            vite-build-web) echo "✓ built in 1.92s" ;;
            ratchet) echo "ratchet: ok." ;;
        esac
        exit 0 ;;
esac
echo "stub: unexpected k2 $*" >&2
exit 2
STUB
chmod +x "$tmp/bin/k2"

gates_only() { # <name> [env…] — release.sh --gates-only <SHA>; echoes rc
    local name="$1"; shift
    export STUB_CALLS="$tmp/calls-$name"
    : > "$STUB_CALLS"
    local rc=0
    (cd "$co" && env PATH="$tmp/bin:$PATH" "$@" bash scripts/release.sh --gates-only "$SHA") > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}
runs() { grep "^RUN $2 " "$tmp/calls-$1" || true; }

echo "all gates pass (--gates-only):"
rc="$(gates_only ok)"
out="$(cat "$tmp/out-ok")"
[ "$rc" = 0 ] && pass "exit 0" || fail "ok rc=$rc" "$out"
for g in cargo-linux cargo-macos vitest typecheck-web vite-build-web ratchet; do
    line="$(runs ok "$g")"
    [ -n "$line" ] && pass "$g: one job sent" || { fail "$g: no job" "$(cat "$tmp/calls-ok")"; continue; }
    for want in "--sha $SHA" "--detach" "--json" "--tier ci" "set -euo pipefail"; do
        [[ "$line" == *"$want"* ]] || fail "$g job lacks $want" "$line"
    done
    [[ "$line" == *--dirty* ]] && fail "$g: --gates-only tests the commit, never --dirty" "$line"
    printf '%s\n' "$out" | grep -qE "^  ✓ $g +job $(printf "%s00000000" "$g" | cut -c1-8) " && pass "$g: a pass line with its job id" || fail "$g pass line" "$out"
done
[[ "$(runs ok vitest)" == *"compute run k2n1-m4 "* && "$(runs ok vitest)" == *"--cpus 7"* && "$(runs ok vitest)" == *"--mem 8G"* \
    && "$(runs ok vitest)" == *"--needs node@22,bun"* ]] && pass "vitest on k2n1-m4, --cpus 7 --mem 8G" || fail "vitest placement" "$(runs ok vitest)"
[[ "$(runs ok cargo-linux)" == *"--os linux --arch x86_64"* && "$(runs ok cargo-linux)" == *"--needs rust@1.98.7,cmake@3,"* \
    && "$(runs ok cargo-linux)" == *"RUSTFLAGS='-D warnings' cargo check --workspace --all-targets"* \
    && "$(runs ok cargo-linux)" == *"--avoid k2-arch"* ]] && pass "cargo-linux: a Linux x86 node, the pinned Rust, -D warnings, never k2-arch" || fail "cargo-linux" "$(runs ok cargo-linux)"
[[ "$(runs ok cargo-macos)" == *"--needs rust@1.98.7,cmake@3"* && "$(runs ok cargo-macos)" == *"RUSTFLAGS='-D warnings' cargo check --workspace --all-targets"* \
    && "$(runs ok cargo-macos)" == *"k2_mac_build_env"* && "$(runs ok cargo-macos)" == *"--mem 8G --env CARGO_BUILD_JOBS=1 "* ]] && pass "cargo-macos: the pinned Rust, the mac build env, -D warnings, one crate at a time in 8 GB" || fail "cargo-macos" "$(runs ok cargo-macos)"
printf '%s\n' "$out" | grep -q "Tests  12255 passed | 2 skipped (12257)" && pass "vitest's line shows its test result" || fail "vitest result" "$out"
printf '%s\n' "$out" | grep -qE "Gates: 6 passed in [0-9]+m[0-9]{2}s wall-clock" && pass "a wall-clock summary" || fail "summary" "$out"
grep -qE "^k2 (compute run|compute nodes|compute get|compute logs|compute wait|compute --help)" "$tmp/calls-ok" && ! grep -qE "^k2 (msg|thread|release)" "$tmp/calls-ok" \
    && pass "only compute calls" || fail "other k2 calls" "$(cat "$tmp/calls-ok")"
! grep -qiE "release lock|Signing approval|Bumping version" "$tmp/out-ok" && pass "no lock, no signing approval, no bump" || fail "did more than the gates" "$out"

echo "a gate fails:"
rc="$(gates_only vfail STUB_FAIL=vitest)"
out="$(cat "$tmp/out-vfail")"
[ "$rc" = 1 ] && pass "exit 1" || fail "vfail rc=$rc" "$out"
printf '%s\n' "$out" | grep -qE "^  ✗ vitest +job vitest00 .*exit 1" && pass "a fail line with job id and exit" || fail "fail line" "$out"
printf '%s\n' "$out" | grep -q "FAILURE-SUMMARY-FOR-vitest" && pass "the job's --failures summary is printed" || fail "no failures summary" "$out"
printf '%s\n' "$out" | grep -q "1 failed | 12254 passed" && pass "its test result line is printed" || fail "no result line" "$out"

echo "a compile or type error (no failing tests for --failures to list):"
rc="$(gates_only cfail STUB_FAIL=cargo-linux STUB_NO_FAILURES=1)"
out="$(cat "$tmp/out-cfail")"
[ "$rc" = 1 ] && printf '%s\n' "$out" | grep -q "its end; --failures lists none" && printf '%s\n' "$out" | grep -q "    error: could not compile" \
    && pass "the end of the job's log is printed instead" || fail "cfail rc=$rc" "$out"

echo "exit 0 but no result line is not trusted:"
rc="$(gates_only noline STUB_NOLINE=vitest)"
out="$(cat "$tmp/out-noline")"
[ "$rc" = 1 ] && printf '%s\n' "$out" | grep -qE "^  ✗ vitest .*no test result line" && pass "exit 1, says why" || fail "noline rc=$rc" "$out"

echo "a node problem is not a gate failure:"
rc="$(gates_only infra STUB_INFRA=cargo-linux)"
out="$(cat "$tmp/out-infra")"
[ "$rc" = 1 ] && printf '%s\n' "$out" | grep -qE "^  ! cargo-linux +job cargo-li .*mem_cap.*node problem" && pass "--gates-only: exit 1, names the node problem" || fail "infra rc=$rc" "$out"

echo "compute unreachable:"
rc="$(gates_only down STUB_DOWN=1)"
out="$(cat "$tmp/out-down")"
[ "$rc" = 3 ] && printf '%s\n' "$out" | grep -q "K2 Compute is unreachable" && [ -z "$(grep '^RUN' "$tmp/calls-down" || true)" ] \
    && pass "--gates-only: exit 3, nothing sent" || fail "down rc=$rc" "$out"
rc="$(gates_only m4down STUB_M4_ONLINE=false)"
out="$(cat "$tmp/out-m4down")"
[ "$rc" = 3 ] && printf '%s\n' "$out" | grep -q "k2n1-m4" && pass "the pinned Mac node offline counts as unreachable" || fail "m4down rc=$rc" "$out"

echo "the lib in a release (sourced, as release.sh Step 0.5 uses it):"
lib_run() { # <name> <script> [env…]
    local name="$1" script="$2"; shift 2
    export STUB_CALLS="$tmp/calls-$name"
    : > "$STUB_CALLS"
    local rc=0
    (cd "$co" && env PATH="$tmp/bin:$PATH" "$@" bash -c "set -euo pipefail; source scripts/lib/rust-pin.sh; source scripts/lib/compute-tier.sh; source scripts/lib/linux-gate-spec.sh; source scripts/lib/release-gates.sh; $script") > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}
rc="$(lib_run rel 'k2rg_begin "$(git rev-parse HEAD)" release; k2rg_submit_all; k2rg_wait_all; echo "INFRA=[$K2RG_INFRA] LINUX=$K2RG_LINUX_OK"')"
out="$(cat "$tmp/out-rel")"
[ "$rc" = 0 ] && [ -z "$(runs rel cargo-macos)" ] && printf '%s\n' "$out" | grep -q "LINUX=1" \
    && pass "release: the mac check stays on this Mac by default; Linux gate recorded as run" || fail "release default" "rc=$rc $out"
rc="$(lib_run relinfra 'k2rg_begin "$(git rev-parse HEAD)" release; k2rg_submit_all; rc=0; k2rg_wait_all || rc=$?; echo "RC=$rc INFRA=[$K2RG_INFRA]"' STUB_INFRA=typecheck-web)"
out="$(cat "$tmp/out-relinfra")"
printf '%s\n' "$out" | grep -q "RC=0 INFRA=\[typecheck-web\]" && pass "release: a node problem leaves the gate to run here (K2RG_INFRA)" || fail "relinfra" "$out"
rc="$(lib_run reldirty 'printf x > untracked.txt; k2rg_begin "$(git rev-parse HEAD)" release; k2rg_submit_all; rm -f untracked.txt')"
[ "$(grep -c '^RUN .* --dirty' "$tmp/calls-reldirty")" = 5 ] && pass "release: a dirty checkout ships --dirty on every job" || fail "reldirty" "$(cat "$tmp/calls-reldirty")"
rc="$(lib_run relmac 'k2rg_begin "$(git rev-parse HEAD)" release; k2rg_submit_all' K2_RELEASE_MAC_CHECK=compute)"
[ -n "$(runs relmac cargo-macos)" ] && pass "K2_RELEASE_MAC_CHECK=compute sends the mac check too" || fail "relmac" "$(cat "$tmp/calls-relmac")"
rc="$(lib_run cancel 'k2rg_begin "$(git rev-parse HEAD)" release; k2rg_submit_all; k2rg_cancel_open')"
[ "$(grep -c '^k2 compute cancel .* --force' "$tmp/calls-cancel")" = 5 ] && pass "k2rg_cancel_open cancels every open gate job" || fail "cancel" "$(cat "$tmp/calls-cancel")"

echo "release.sh Step 0.5 as written (k2rg_local stubbed: it records the gates run here):"
awk '/^echo "Step 0\.5: Pre-flight gates/{on=1} /^# ── Step 0\.55/{exit} on{print}' "$ROOT/scripts/release.sh" > "$tmp/step05.sh"
grep -q k2rg_submit_all "$tmp/step05.sh" && pass "found the Step 0.5 block" || fail "no Step 0.5 block" "$(head -3 "$tmp/step05.sh")"
step_run() { # <name> [env…]; echoes rc
    local name="$1"; shift
    export STUB_CALLS="$tmp/calls-$name"
    : > "$STUB_CALLS"
    local rc=0
    (cd "$co" && env PATH="$tmp/bin:$PATH" "$@" bash -c 'set -euo pipefail
        source scripts/lib/rust-pin.sh; source scripts/lib/compute-tier.sh; source scripts/lib/linux-gate-spec.sh; source scripts/lib/release-gates.sh
        k2rg_local() { echo "LOCAL $1" >> "$STUB_CALLS"; }
        source "$1"
        echo "LINUX_OK=$K2RG_LINUX_OK"' _ "$tmp/step05.sh") > "$tmp/out-$name" 2>&1 || rc=$?
    echo "$rc"
}
locals() { grep '^LOCAL ' "$tmp/calls-$1" | cut -d' ' -f2 | tr '\n' ' ' | sed 's/ $//' || true; }
rc="$(step_run s-ok)"
[ "$rc" = 0 ] && [ "$(locals s-ok)" = "cargo-macos" ] && grep -q "LINUX_OK=1" "$tmp/out-s-ok" && grep -q "Pre-flight gates passed." "$tmp/out-s-ok" \
    && pass "compute: 5 jobs, the mac check here, Step 0.6 told the Linux gate ran" || fail "s-ok rc=$rc locals=$(locals s-ok)" "$(cat "$tmp/out-s-ok")"
rc="$(step_run s-down STUB_DOWN=1)"
[ "$rc" = 0 ] && [ "$(locals s-down)" = "ratchet cargo-macos typecheck-web vitest vite-build-web" ] && ! grep -q '^RUN' "$tmp/calls-s-down" \
    && grep -q "K2 Compute is unreachable" "$tmp/out-s-down" && grep -q "LINUX_OK=0" "$tmp/out-s-down" \
    && pass "unreachable: says so, the old local gates, Step 0.6 runs its own job" || fail "s-down rc=$rc locals=$(locals s-down)" "$(cat "$tmp/out-s-down")"
rc="$(step_run s-refuse STUB_REFUSE=vitest)"
[ "$rc" = 0 ] && [ "$(locals s-refuse)" = "ratchet cargo-macos typecheck-web vitest vite-build-web" ] && grep -q "refused a gate job" "$tmp/out-s-refuse" \
    && [ "$(grep -c '^k2 compute cancel' "$tmp/calls-s-refuse")" = 4 ] && grep -q "LINUX_OK=0" "$tmp/out-s-refuse" \
    && pass "a refused job: the sent ones cancelled, the old local gates" || fail "s-refuse rc=$rc locals=$(locals s-refuse)" "$(cat "$tmp/out-s-refuse" "$tmp/calls-s-refuse")"
rc="$(step_run s-infra STUB_INFRA=cargo-linux)"
[ "$rc" = 0 ] && [ "$(locals s-infra)" = "cargo-macos cargo-linux" ] && grep -q "LINUX_OK=1" "$tmp/out-s-infra" \
    && pass "a node problem: that gate runs here" || fail "s-infra rc=$rc locals=$(locals s-infra)" "$(cat "$tmp/out-s-infra")"
rc="$(step_run s-fail STUB_FAIL=typecheck-web)"
[ "$rc" = 1 ] && grep -q "FATAL: a pre-flight gate failed" "$tmp/out-s-fail" && [ "$(locals s-fail)" = "cargo-macos" ] \
    && pass "a failed gate stops the release, no local rerun" || fail "s-fail rc=$rc" "$(cat "$tmp/out-s-fail")"
rc="$(step_run s-local K2_RELEASE_GATES=local)"
[ "$rc" = 0 ] && [ "$(locals s-local)" = "ratchet cargo-macos typecheck-web vitest vite-build-web" ] && ! grep -q '^k2 ' "$tmp/calls-s-local" \
    && pass "K2_RELEASE_GATES=local: the old path, no k2 call" || fail "s-local rc=$rc" "$(cat "$tmp/out-s-local")"

echo "release.sh wiring:"
REL="$ROOT/scripts/release.sh"
python3 - "$REL" <<'PY' && pass "--gates-only is handled before the version, the lock and .env" || fail "gates-only order" "see release.sh top"
import sys
s = open(sys.argv[1]).read()
g = s.find('"--gates-only"')
assert g != -1, "no --gates-only"
for later in ('VERSION="${1:-}"', "k2_release_lock_acquire", 'source "$PROJECT_DIR/.env"', "k2sa_open_session"):
    assert g < s.find(later), later
PY
grep -q 'K2_RELEASE_GATES' "$REL" && grep -q 'k2rg_local_all' "$REL" && pass "K2_RELEASE_GATES=local keeps the old path" || fail "local switch" "release.sh"
sed -n '/^k2_arch_gate_on_exit() {/,/^}/p' "$REL" | grep -q k2rg_cancel_open && pass "the EXIT trap cancels open gate jobs" || fail "exit trap" "release.sh"
grep -q 'K2RG_LINUX_OK' "$REL" && pass "Step 0.6 knows when the Linux gate already ran" || fail "step 0.6" "release.sh"
bash -n "$REL" && bash -n "$ROOT/scripts/lib/release-gates.sh" && pass "bash -n" || fail "bash -n" "release.sh / release-gates.sh"

echo ""
echo "release-gates: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
