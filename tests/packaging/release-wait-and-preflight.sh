#!/usr/bin/env bash
# release.sh polish (0.46.4), stubbed, no cargo, never runs release.sh:
#
#  (a) Wait loops print ONE "waiting for …" line, then a dot per poll, with a
#      short status line every K2_WAIT_STATUS_SECS — not a line per poll
#      (scripts/lib/wait-progress.sh; used by release.sh, linux-sums.sh and
#      the sign-* scripts). Driven with a fake clock (a `date` stub).
#  (b) release.sh Step 0.5's pre-flight runs with the shared mac build env:
#      SDKROOT from xcrun, CMAKE_POLICY_VERSION_MINIMUM, and half-configured
#      cmake dirs cleared before `cargo check`. The Step 0.5 block is cut out
#      of release.sh as written and run against stubs (xcrun, cargo, bun,
#      bunx) that record what they were given.
#
# Run with: bash tests/packaging/release-wait-and-preflight.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1  [$2]"; fi; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
REAL_DATE="$(command -v date)"

echo "== wait-progress helper, fake clock =="
out="$({
    # shellcheck source=../../scripts/lib/wait-progress.sh
    source "$ROOT/scripts/lib/wait-progress.sh"
    T=1000
    k2_wait_now() { echo "$T"; }
    K2_WAIT_STATUS_SECS=300
    # 26 polls, 30 s apart = 12.5 minutes of waiting.
    for i in $(seq 1 26); do
        k2_wait_tick "[x] waiting for the assets…"
        echo "said=$K2_WAIT_SAID" >> "$tmp/said"
        T=$((T + 30))
    done
    k2_wait_done
    echo "after done"
    { k2_wait_tick "[x] second wait"; k2_wait_done; } 2>&1
    echo "elapsed: $(k2_wait_elapsed 45) $(k2_wait_elapsed 75) $(k2_wait_elapsed 3900)"
    } 2>&1
)"
want="$(printf '%s\n' \
    "[x] waiting for the assets…" \
    "........." \
    "  still waiting (5m00s): [x] waiting for the assets…" \
    "........." \
    "  still waiting (10m00s): [x] waiting for the assets…" \
    "....." \
    "after done" \
    "[x] second wait" \
    "elapsed: 45s 1m15s 1h05m")"
if [ "$out" = "$want" ]; then ok "26 polls → 1 line, dots, a status line every 5 min, newline at done"; else bad "helper output:
--- want
$want
--- got
$out"; fi
check "K2_WAIT_SAID is 1 only on the first and status ticks" '[ "$(grep -c "said=1" "$tmp/said")" = 3 ] && [ "$(grep -c "said=0" "$tmp/said")" = 23 ]'

# A `date` stub: `date +%s` is a fake clock that moves 10 s per call;
# anything else is the real date.
mkdir -p "$tmp/bin"
cat > "$tmp/bin/date" <<EOF
#!/usr/bin/env bash
if [ "\${1:-}" = "+%s" ]; then
    n=\$(cat "\$FAKE_CLOCK" 2>/dev/null || echo 0)
    echo \$((n + 10)) > "\$FAKE_CLOCK"
    echo \$((1700000000 + n))
    exit 0
fi
exec "$REAL_DATE" "\$@"
EOF
# gh stub: `release view` lists the assets; the SUMS file appears only after
# \$GH_READY_AFTER views. `api …/releases/tags/…` lists nothing (incomplete).
cat > "$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "gh $*" >> "$GH_CALLS"
case "$1 $2" in
    "release view")
        n=$(grep -c "^gh release view" "$GH_CALLS")
        echo "K2_1.0.0_amd64.deb"
        if [ "$n" -gt "${GH_READY_AFTER:-999999}" ]; then echo "SHA256SUMS-linux-x86_64.txt"; fi
        ;;
    "api repos/"*) : ;;
    *) exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/date" "$tmp/bin/gh"

echo "== linux-sums wait: one line, dots, then the result =="
export FAKE_CLOCK="$tmp/clock1" GH_CALLS="$tmp/calls1"
: > "$GH_CALLS"
set +e
err="$(PATH="$tmp/bin:$PATH" GH_READY_AFTER=6 K2_SUMS_POLL_SECS=0 K2_WAIT_STATUS_SECS=99999 \
    bash "$ROOT/scripts/linux-sums.sh" wait v1.0.0 30 2>&1 >/dev/null)"
rc=$?
set -e
want="$(printf '%s\n' \
    "[linux-sums] waiting for app-linux.yml to upload SHA256SUMS-linux-x86_64.txt to v1.0.0..." \
    "....." \
    "[linux-sums] v1.0.0 has SHA256SUMS-linux-x86_64.txt")"
check "wait exit 0" '[ "$rc" = 0 ]'
check "polled 7 times" '[ "$(grep -c "^gh release view" "$GH_CALLS")" = 7 ]'
if [ "$err" = "$want" ]; then ok "6 waiting polls → 1 line + 5 dots + the result"; else bad "linux-sums wait output:
--- want
$want
--- got
$err"; fi

echo "== linux-sums check --wait-min: findings only under the first and status lines =="
export FAKE_CLOCK="$tmp/clock2" GH_CALLS="$tmp/calls2"
: > "$GH_CALLS"
set +e
err="$(PATH="$tmp/bin:$PATH" K2_SUMS_POLL_SECS=0 K2_WAIT_STATUS_SECS=60 \
    bash "$ROOT/scripts/linux-sums.sh" check 1.0.0 --wait-min 3 2>&1 >/dev/null)"
rc=$?
set -e
polls="$(grep -c "^gh api" "$GH_CALLS")"
first="$(printf '%s\n' "$err" | grep -c '^\[linux-sums\] waiting for app-linux.yml / arch-package.yml (v1.0.0)...$' || true)"
status="$(printf '%s\n' "$err" | grep -c '^  still waiting (' || true)"
detail="$(printf '%s\n' "$err" | grep -c '^    \[linux-sums\] not on the release yet: SHA256SUMS-linux-x86_64.txt$' || true)"
final_detail="$(printf '%s\n' "$err" | grep -c '^\[linux-sums\] not on the release yet: SHA256SUMS-linux-x86_64.txt$' || true)"
check "gives up with exit 1 (got $rc)" '[ "$rc" = 1 ]'
check "names the give-up" 'printf "%s" "$err" | grep -q "still missing after 3 min"'
check "polled several times ($polls)" '[ "$polls" -ge 5 ]'
check "the waiting line once" '[ "$first" = 1 ]'
check "at least one status line ($status)" '[ "$status" -ge 1 ]'
check "findings under the first + each status line only ($detail = 1 + $status)" '[ "$detail" = $((1 + status)) ]'
check "a dots line between them" 'printf "%s\n" "$err" | grep -qx "\.\.*"'
check "the last pass findings once more before giving up" '[ "$final_detail" = 1 ]'
check "far fewer lines than polls would print" '[ "$(printf "%s\n" "$err" | wc -l | tr -d " ")" -lt $((polls * 6)) ]'

echo "== every release wait loop uses the helper =="
for f in scripts/sign-root-helper.sh scripts/sign-k2-node.sh scripts/sign-app-engine.sh scripts/linux-sums.sh scripts/release.sh; do
    check "$f sources wait-progress.sh" 'grep -q "lib/wait-progress.sh" "$ROOT/$f"'
done
for f in scripts/sign-root-helper.sh scripts/sign-k2-node.sh scripts/sign-app-engine.sh; do
    check "$f ticks instead of logging each poll" 'grep -q "k2_wait_tick" "$ROOT/$f" && ! grep -q "log \"waiting" "$ROOT/$f"'
done
check "linux-sums ticks instead of logging each poll" '! grep -q "log \"waiting" "$ROOT/scripts/linux-sums.sh"'
check "release.sh verify_manifest ticks" 'sed -n "/^verify_manifest()/,/^}/p" "$ROOT/scripts/release.sh" | grep -q k2_wait_tick'

echo "== release.sh Step 0.5 runs with the shared mac build env =="
block="$(awk '/^echo "Step 0\.5: Pre-flight gates/{on=1} on{print} on && /Pre-flight gates passed\./{exit}' "$ROOT/scripts/release.sh")"
check "found the Step 0.5 block" '[ -n "$block" ] && printf "%s" "$block" | grep -q "cargo check"'
proj="$tmp/proj"
stubs="$tmp/stubs"
mkdir -p "$proj" "$stubs"
half="$proj/target/debug/build/llama-cpp-sys-2-aaaa/out/build"
good="$proj/target/debug/build/prost-build-bbbb/out/build"
mkdir -p "$half" "$good"
: > "$half/CMakeCache.txt"
: > "$good/CMakeCache.txt"
: > "$good/Makefile"
cat > "$stubs/xcrun" <<'EOF'
#!/usr/bin/env bash
[ "$*" = "--sdk macosx --show-sdk-path" ] && { echo "/fake/Xcode.app/MacOSX26.5.sdk"; exit 0; }
exit 1
EOF
cat > "$stubs/cargo" <<'EOF'
#!/usr/bin/env bash
{
    echo "cargo $*"
    echo "SDKROOT=${SDKROOT:-}"
    echo "CMAKE_POLICY_VERSION_MINIMUM=${CMAKE_POLICY_VERSION_MINIMUM:-}"
    echo "RUSTFLAGS=${RUSTFLAGS:-}"
    [ -e target/debug/build/llama-cpp-sys-2-aaaa/out/build ] && echo "half=present" || echo "half=cleared"
    [ -e target/debug/build/prost-build-bbbb/out/build/Makefile ] && echo "good=kept" || echo "good=gone"
} >> "$STUB_LOG"
EOF
for t in bun bunx; do
    printf '#!/usr/bin/env bash\necho "%s $*" >> "$STUB_LOG"\n' "$t" > "$stubs/$t"
done
chmod +x "$stubs"/*
printf '%s\n' "$block" > "$tmp/step05.sh"
set +e
out="$(cd "$proj" && env -u SDKROOT -u CMAKE_POLICY_VERSION_MINIMUM PATH="$stubs:$PATH" STUB_LOG="$tmp/step05.log" \
    bash -c 'set -euo pipefail; source "$1/scripts/lib/build-binaries.sh"; source "$2"' _ "$ROOT" "$tmp/step05.sh" 2>&1)"
rc=$?
set -e
log="$(cat "$tmp/step05.log" 2>/dev/null || true)"
check "the block ran (rc $rc: $out)" '[ "$rc" = 0 ]'
check "cargo check ran" 'printf "%s\n" "$log" | grep -qx "cargo check --workspace --all-targets"'
check "cargo saw SDKROOT from xcrun" 'printf "%s\n" "$log" | grep -qx "SDKROOT=/fake/Xcode.app/MacOSX26.5.sdk"'
check "cargo saw the cmake policy floor" 'printf "%s\n" "$log" | grep -qx "CMAKE_POLICY_VERSION_MINIMUM=3.5"'
check "cargo kept -D warnings" 'printf "%s\n" "$log" | grep -qx "RUSTFLAGS=-D warnings"'
check "the half-configured cmake dir was cleared before cargo" 'printf "%s\n" "$log" | grep -qx "half=cleared"'
check "a good cmake dir was kept" 'printf "%s\n" "$log" | grep -qx "good=kept"'
check "the rest of the gates still ran" 'printf "%s\n" "$log" | grep -q "^bun run typecheck:web" && printf "%s\n" "$log" | grep -q "^bunx vitest run" && printf "%s\n" "$log" | grep -q "^bun run vite:build:web"'
check "says which SDK it uses" 'printf "%s" "$out" | grep -q "build env: SDKROOT=/fake/Xcode.app/MacOSX26.5.sdk"'

# An SDKROOT the caller set still wins.
: > "$tmp/step05.log"
(cd "$proj" && SDKROOT=/caller/sdk PATH="$stubs:$PATH" STUB_LOG="$tmp/step05.log" \
    bash -c 'set -euo pipefail; source "$1/scripts/lib/build-binaries.sh"; source "$2"' _ "$ROOT" "$tmp/step05.sh" >/dev/null 2>&1)
check "a caller's SDKROOT wins" 'grep -qx "SDKROOT=/caller/sdk" "$tmp/step05.log"'

# (c) The updater key and its password never ride on argv (0.46.6): the
# tauri signer reads TAURI_SIGNING_PRIVATE_KEY(_PASSWORD) from the env, and
# argv is visible to every process on the build Mac (ps).
for f in scripts/release.sh scripts/windows-patch-latest-json.sh; do
    check "$f: no --private-key / --password on argv" \
        '! grep -nE -- "--private-key|--password[ =]" "$ROOT/$f"'
    check "$f: still signs with the tauri signer" \
        'grep -q "signer sign" "$ROOT/$f"'
done

echo ""
echo "release-wait-and-preflight: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
