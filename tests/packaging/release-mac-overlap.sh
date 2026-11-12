#!/bin/bash
# Stubbed test for the Mac release steps that now overlap (0.46.13):
#   - Step 8.5's standalone daemon builds in the background from Step 3.6
#     (k2_standalone_macos_daemon_start / _poll / _finish / _kill in
#     scripts/macos-daemon-helper-app.sh);
#   - k2-node is notarized beside the app (scripts/lib/notary.sh:
#     k2_notary_submit_nowait at Step 4, k2_notary_wait_accepted at 8.55);
#   - and release.sh calls them in an order that keeps every signing,
#     notarizing and approval guarantee.
# Stub `cargo`, `codesign` and `xcrun` on PATH: nothing compiles, signs,
# notarizes or touches ~/.k2. Runs on macOS and Linux. No skips.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }
check() { if eval "$1"; then pass "$2"; else fail "$2" "${3:-}"; fi; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stub="$work/stub"
proj="$work/proj"
mkdir -p "$stub" "$proj/scripts" "$proj/target/release"
cp "$ROOT/scripts/linux-ptyd-embed.sh" "$proj/scripts/"

# cargo plays build.rs: the daemon carries the sha256 of the embeds.
# STUB_SLEEP: take that long. STUB_FAIL=1: fail. Its pid goes to $work/cargo.pid.
cat > "$stub/cargo" <<'EOF'
#!/bin/bash
echo $$ > "$STUB_DIR/cargo.pid"
printf '%s\tembed=%s\texport=%s\n' "$*" "${K2_PTYD_EMBED:-}" "${K2_EXPORT_EMBED:-}" >> "$STUB_DIR/cargo.calls"
echo "   Compiling k2-daemon v0.0.0 (stub)"
[ -z "${STUB_SLEEP:-}" ] || sleep "$STUB_SLEEP"
[ "${STUB_FAIL:-0}" = 1 ] && { echo "error: stub cargo failing on purpose"; exit 101; }
out="${CARGO_TARGET_DIR:-target}/release/k2-daemon"
mkdir -p "$(dirname "$out")"
printf 'daemon-bytes ' > "$out"
shasum -a 256 "$K2_PTYD_EMBED" | cut -d' ' -f1 >> "$out"
shasum -a 256 "$K2_EXPORT_EMBED" | cut -d' ' -f1 >> "$out"
chmod +x "$out"
echo "    Finished \`release\` profile [optimized] target(s) in 0.01s"
EOF
cat > "$stub/codesign" <<'EOF'
#!/bin/bash
case "$1" in
    --verify) exit 0 ;;
    -dv) case "$2" in *k2-export) echo "Identifier=dev.k2.app.export" >&2 ;; *) echo "Identifier=dev.k2.app.ptyd" >&2 ;; esac; exit 0 ;;
esac
exit 1
EOF
# xcrun notarytool: submit prints JSON with an id; wait prints a status
# (STUB_STATUS, default Accepted); STUB_WAIT_FAIL=1 makes wait fail; log
# prints a marker. Every call is logged.
cat > "$stub/xcrun" <<'EOF'
#!/bin/bash
echo "$*" >> "$STUB_DIR/xcrun.calls"
[ "$1" = notarytool ] || exit 1
case "$2" in
    submit) [ -f "$3" ] || { echo '{"message":"no file"}'; exit 1; }
            echo '{"id":"0b8e7c52-1f0e-4c43-9d5b-2a7c1e9f3a11","message":"Successfully uploaded file","path":"'"$3"'"}' ;;
    wait)   [ "${STUB_WAIT_FAIL:-0}" = 1 ] && { echo '{"message":"network"}'; exit 1; }
            echo '{"id":"'"$3"'","status":"'"${STUB_STATUS:-Accepted}"'","message":"Processing complete"}' ;;
    log)    echo "APPLE-LOG-FOR-$3" ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$stub/cargo" "$stub/codesign" "$stub/xcrun"
export STUB_DIR="$work"
export PATH="$stub:$PATH"
unset CARGO_TARGET_DIR SDKROOT

# shellcheck source=scripts/macos-daemon-helper-app.sh
. "$ROOT/scripts/macos-daemon-helper-app.sh"
app="$work/app/K2.app"
pdir="$app/Contents/Helpers/$K2_DAEMON_HELPER_APP/Contents/MacOS"
mkdir -p "$pdir"
head -c 8192 /dev/urandom > "$pdir/$K2_DAEMON_HELPER_PTYD"; chmod 0755 "$pdir/$K2_DAEMON_HELPER_PTYD"
head -c 8192 /dev/urandom > "$pdir/$K2_DAEMON_HELPER_EXPORT"; chmod 0755 "$pdir/$K2_DAEMON_HELPER_EXPORT"
sha="$(shasum -a 256 "$pdir/$K2_DAEMON_HELPER_PTYD" | cut -d' ' -f1)"
esha="$(shasum -a 256 "$pdir/$K2_DAEMON_HELPER_EXPORT" | cut -d' ' -f1)"
stage="$work/stage"

echo "Standalone daemon in the background:"
: > "$work/cargo.calls"
t0=$(date +%s)
STUB_SLEEP=2 k2_standalone_macos_daemon_start "$app" "$stage" "$proj" > "$work/start.log" 2>&1
check '[ $(( $(date +%s) - t0 )) -lt 2 ] && [ -n "$K2_STANDALONE_PID" ] && kill -0 "$K2_STANDALONE_PID"' \
    "start returns at once with the build running" "$(cat "$work/start.log")"
check 'k2_standalone_macos_daemon_poll "$stage"' "poll while it runs: 0"
check 'cmp -s "$stage/embed/k2-ptyd" "$pdir/k2-ptyd" && cmp -s "$stage/embed/k2-export" "$pdir/k2-export"' \
    "it embeds COPIES of the signed k2-ptyd and k2-export"
want="build --release -p k2-daemon --bin k2-daemon --bin k2-power-helper -p k2-ptyd --bin k2-ptyd -p k2-export --bin k2-export -p k2-node --bin k2-node -p k2-menubar --bin k2-menubar -p k2-deskd --bin k2-deskd"
k2_standalone_macos_daemon_finish "$app" "$stage" "$work/out-daemon" > "$work/finish.log" 2>&1 \
    && pass "finish waits and succeeds" || fail "finish waits and succeeds" "$(cat "$work/finish.log")"
check '[ "$(cut -f1 "$work/cargo.calls")" = "$want" ] && [ "$(cut -f2 "$work/cargo.calls")" = "embed=$stage/embed/k2-ptyd" ]' \
    "the build is Step 2's package set with the staged embeds" "$(cat "$work/cargo.calls")"
check 'grep -q "$sha" "$work/out-daemon" && grep -q "$esha" "$work/out-daemon"' "the published daemon embeds the shipped ptyd and export"
check 'grep -q "build: Compiling k2-daemon" "$work/finish.log" && grep -q "waiting for the background" "$work/finish.log"' \
    "finish shows what the build compiled" "$(cat "$work/finish.log")"

# The app's k2-ptyd changes after the start (what Step 4's staple must
# never do): finish refuses to publish.
rm -f "$work/out-daemon"
k2_standalone_macos_daemon_start "$app" "$stage" "$proj" > /dev/null 2>&1
cp "$pdir/k2-ptyd" "$work/ptyd.keep"; head -c 8192 /dev/urandom > "$pdir/k2-ptyd"
if k2_standalone_macos_daemon_finish "$app" "$stage" "$work/out-daemon" > "$work/finish2.log" 2>&1; then
    fail "a k2-ptyd that differs from the shipped one is refused" "$(cat "$work/finish2.log")"
else
    check '[ ! -e "$work/out-daemon" ] && grep -q "is not the one in the shipped app" "$work/finish2.log"' \
        "a k2-ptyd that differs from the shipped one is refused" "$(cat "$work/finish2.log")"
fi
cp "$work/ptyd.keep" "$pdir/k2-ptyd"

# A failed build: poll says so once it has ended (the release stops there).
STUB_FAIL=1 k2_standalone_macos_daemon_start "$app" "$stage" "$proj" > /dev/null 2>&1
i=0; while kill -0 "$K2_STANDALONE_PID" 2>/dev/null && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
if k2_standalone_macos_daemon_poll "$stage" > "$work/poll.log" 2>&1; then
    fail "poll fails once the build has failed" "$(cat "$work/poll.log")"
else
    check 'grep -q "background standalone daemon build failed" "$work/poll.log" && grep -q "failing on purpose" "$work/poll.log"' \
        "poll fails once the build has failed, with the build's last lines" "$(cat "$work/poll.log")"
fi
if k2_standalone_macos_daemon_finish "$app" "$stage" "$work/out-daemon" > /dev/null 2>&1; then
    fail "finish refuses after a failed build" "published anyway"
else
    pass "finish refuses after a failed build"
fi

# Step 7.5's wait (before the VM): blocks until the build ends; finish
# then collects without waiting again. A failed build fails the wait.
rm -f "$work/out-daemon"
STUB_SLEEP=1 k2_standalone_macos_daemon_start "$app" "$stage" "$proj" > /dev/null 2>&1
k2_standalone_macos_daemon_wait "$stage" > "$work/wait.log" 2>&1 && pass "wait blocks until a good build ends: 0" \
    || fail "wait blocks until a good build ends: 0" "$(cat "$work/wait.log")"
k2_standalone_macos_daemon_finish "$app" "$stage" "$work/out-daemon" > "$work/finish3.log" 2>&1
check '[ -s "$work/out-daemon" ] && ! grep -q "waiting for the background" "$work/finish3.log"' \
    "finish after the wait collects it without waiting" "$(cat "$work/finish3.log")"
STUB_FAIL=1 k2_standalone_macos_daemon_start "$app" "$stage" "$proj" > /dev/null 2>&1
if k2_standalone_macos_daemon_wait "$stage" > /dev/null 2>&1; then fail "wait fails on a failed build" "returned 0"; else pass "wait fails on a failed build"; fi

# The EXIT trap's kill: stops a running build and everything under it.
STUB_SLEEP=30 k2_standalone_macos_daemon_start "$app" "$stage" "$proj" > /dev/null 2>&1
i=0; while [ ! -s "$work/cargo.pid" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
rm -f "$work/cargo.pid"
i=0; while [ ! -s "$work/cargo.pid" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
cpid="$(cat "$work/cargo.pid")"
bpid="$K2_STANDALONE_PID"
t0=$(date +%s)
k2_standalone_macos_daemon_kill
sleep 0.3
check '[ $(( $(date +%s) - t0 )) -lt 5 ] && [ -z "$K2_STANDALONE_PID" ] && ! kill -0 "$bpid" 2>/dev/null && ! kill -0 "$cpid" 2>/dev/null' \
    "kill stops the build and its cargo at once" "bg $bpid cargo $cpid"
if k2_standalone_macos_daemon_start "$app" "relative/stage" "$proj" > /dev/null 2>&1; then
    fail "a relative stage dir is refused" "started"; k2_standalone_macos_daemon_kill
else
    pass "a relative stage dir is refused"
fi

echo "Notarization overlap (scripts/lib/notary.sh):"
notary_auth_args() { printf '%s\n' --keychain-profile TEST-PROFILE; }
# shellcheck source=scripts/lib/notary.sh
. "$ROOT/scripts/lib/notary.sh"
printf 'zip' > "$work/n.zip"
: > "$work/xcrun.calls"
id="$(k2_notary_submit_nowait "$work/n.zip")"
check '[ "$id" = "0b8e7c52-1f0e-4c43-9d5b-2a7c1e9f3a11" ]' "submit without waiting prints only the id" "got: $id"
check 'grep -qx "notarytool submit $work/n.zip --keychain-profile TEST-PROFILE --output-format json" "$work/xcrun.calls" && ! grep -q -- "--wait" "$work/xcrun.calls"' \
    "submit uses the release's notary auth and does not wait" "$(cat "$work/xcrun.calls")"
if k2_notary_submit_nowait "$work/missing.zip" > /dev/null 2> "$work/sub.err"; then
    fail "a failed submission fails" "returned 0"
else
    pass "a failed submission fails"
fi
k2_notary_wait_accepted "$id" k2-node > "$work/wait.log" 2>&1 \
    && check 'grep -q "k2-node notarization $id: Accepted" "$work/wait.log"' "Accepted passes" "$(cat "$work/wait.log")" \
    || fail "Accepted passes" "$(cat "$work/wait.log")"
for st in Invalid Rejected "In Progress"; do
    if STUB_STATUS="$st" k2_notary_wait_accepted "$id" k2-node > "$work/wait.log" 2>&1; then
        fail "status $st fails" "returned 0"
    else
        check 'grep -q "APPLE-LOG-FOR-$id" "$work/wait.log"' "status '$st' fails and prints Apple's log" "$(cat "$work/wait.log")"
    fi
done
if STUB_WAIT_FAIL=1 k2_notary_wait_accepted "$id" k2-node > /dev/null 2>&1; then
    fail "a failed wait fails" "returned 0"
else
    pass "a failed wait fails"
fi

echo "release.sh order:"
python3 -I - "$ROOT/scripts/release.sh" > "$work/order.log" 2>&1 <<'PY' && pass "release.sh: overlap steps sit where the guarantees need them" || fail "release.sh order" "$(cat "$work/order.log")"
import re, sys
s = open(sys.argv[1]).read()
def at(needle, n=1):
    assert s.count(needle) == n, (needle, s.count(needle))
    return s.index(needle)
approve   = at('k2sa_open_session "$VERSION" "$K2_SIGN_BASE" "${K2_SIGN_ASSETS[@]}" || exit 1')
sign3     = re.search(r'^k2_daemon_helper_sign ', s, re.M).start()
verify3   = at('k2_screen_helper_verify_signed "target/release/bundle/macos/K2.app"')
smoke35   = at('"$SMOKE_BIN" --version >/tmp/k2-smoke.out 2>&1 &')
start     = at('k2_standalone_macos_daemon_start "$PROJECT_DIR/target/release/bundle/macos/K2.app" "$STANDALONE_STAGE" "$PROJECT_DIR" || exit 1')
node_cp   = at('cp "$PROJECT_DIR/target/release/bundle/macos/K2.app/Contents/MacOS/k2-node" "$NODE_NOTARY_COPY"')
node_sig  = at('"$NODE_NOTARY_COPY" \\\n        || { echo "  FATAL: k2-node in K2.app is not signed with K2\'s Developer ID" >&2; exit 1; }')
node_chk  = at('bash "$PROJECT_DIR/scripts/sign-k2-node.sh" --check "$NODE_NOTARY_COPY" macos aarch64 "$VERSION" >/dev/null')
node_sub  = at('NODE_NOTARY_ID="$(k2_notary_submit_nowait "$NODE_ZIP")" || exit 1')
app_sub   = at('xcrun notarytool submit "/tmp/K2_${VERSION}.zip" \\\n    "${NOTARY_AUTH[@]}" --wait')
app_stap  = at('staple_with_retry "K2.app"')
poll      = [m.start() for m in re.finditer(r'^k2_standalone_macos_daemon_poll "\$STANDALONE_STAGE" \|\| exit 1$', s, re.M)]
dmg_sub   = at('xcrun notarytool submit "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg" \\\n    "${NOTARY_AUTH[@]}" --wait')
dmg_stap  = at('staple_with_retry "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"')
vm        = at('"$PROJECT_DIR/scripts/vm-newuser-pair-smoke.sh"')
vm_wait   = at('k2_standalone_macos_daemon_wait "$STANDALONE_STAGE" || exit 1')
vm_step   = at('echo "Step 7.5: Clean-VM new-user pairing smoke..."')
assert vm_step < vm_wait < vm, "the build ends before the 8 GB VM starts (unless K2_STANDALONE_OVERLAP_VM=1)"
finish    = at('k2_standalone_macos_daemon_finish "$PROJECT_DIR/target/release/bundle/macos/K2.app" "$STANDALONE_STAGE" "$DIST_DIR/$MAC_ASSET" || exit 1')
fallback  = re.search(r'^k2_build_standalone_macos_daemon "\$PROJECT_DIR/target/release/bundle/macos/K2.app" "\$DIST_DIR/\$MAC_ASSET" "\$PROJECT_DIR"$', s, re.M).start()
u1_daemon = at('k2u_sign u1 "$DIST_DIR/$MAC_ASSET"')
node_cmp  = at('cmp -s "$NODE_NOTARY_COPY" "$DIST_DIR/$NODE_MAC_ASSET"')
node_wait = at('k2_notary_wait_accepted "$NODE_NOTARY_ID" "k2-node" || exit 1')
node_old  = at('xcrun notarytool submit "$NODE_ZIP" "${NODE_NOTARY_AUTH[@]}" --wait')
node_sum  = at('( cd "$DIST_DIR" && shasum -a 256 "$NODE_MAC_ASSET" > "$NODE_MAC_ASSET.sha256" )')
gh        = re.search(r'^gh release create', s, re.M).start()
# The approval stays before anything is built, signed or notarized.
assert approve < sign3 < verify3 < smoke35 < start, "the background build starts after Step 3 signed (and 3.5 launched) the app"
assert start < app_sub, "and before the app's notarization wait it overlaps"
assert approve < node_cp < node_sig < node_chk < node_sub < app_sub, "k2-node: copied, Developer-ID checked, byte-checked, then submitted, beside the app"
assert len(poll) == 2 and app_stap < poll[0] < dmg_sub and dmg_stap < poll[1] < vm, poll
assert dmg_sub < dmg_stap < vm < finish, "the DMG is still notarized and stapled before the VM smoke"
assert finish < u1_daemon and fallback < u1_daemon, "the standalone daemon is collected before it is minisigned"
assert finish < node_cmp < node_wait < node_sum < gh, "k2-node: compared with what was submitted, Accepted, before the release"
assert node_wait < node_old, "a resumed run still submits k2-node at 8.55 and waits"
# The SKIP_BUILD (resume) path: start, node submission and polls are inside it.
opens = [m.start() for m in re.finditer(r'^if \[ "\$SKIP_BUILD" = 0 \]; then$', s, re.M)]
closes = [m.start() for m in re.finditer(r'^fi # SKIP_BUILD\b', s, re.M)]
o, c = opens[1], closes[1]
for x in (start, node_sub, poll[0], poll[1]):
    assert o < x < c, ("inside the build block", x)
assert not (o < finish < c), "finish is outside it (resumed runs reach 8.5)"
# The EXIT trap stops a build nobody will collect.
m = re.search(r'^k2_arch_gate_on_exit\(\) \{\n    local rc=\$\?\n    k2_standalone_macos_daemon_kill \|\| true\n', s, re.M)
assert m, "the EXIT trap kills the background build"
assert re.search(r'^trap k2_arch_gate_on_exit EXIT$', s, re.M)
assert 'source "$PROJECT_DIR/scripts/lib/notary.sh"' in s
PY

echo ""
echo "release-mac-overlap: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
