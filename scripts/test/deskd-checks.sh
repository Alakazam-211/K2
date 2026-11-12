#!/usr/bin/env bash
# deskd-checks.sh — the remote desktop helper's checks (prd-remote-desktop-tab-v1
# D3/D4), one mode per run, for a compute node:
#
#   k2 compute run --sha <c> --needs rust@1.99.1,cmake@3,cargo-nextest --mem 28G \
#       --env CARGO_BUILD_JOBS=4 -- bash scripts/test/deskd-checks.sh linux
#   k2 compute run --os macos --sha <c> --needs rust@1.99.1 --mem 6G \
#       -- bash scripts/test/deskd-checks.sh macos
#
#   linux   check + nextest k2-deskd-proto, k2-deskd and the daemon's desktop
#           module; then the Xvfb integration tests if Xvfb is there
#   macos   check + test k2-deskd-proto and k2-deskd (no TCC, no capture:
#           nothing here prompts), and check the daemon's desktop module
#   xvfb    only the Xvfb integration tests (Linux)
#   x11     kessel-video's X11 capture against Xvfb (each test starts its
#           own; xrandr for the size change) and the crate's other lib
#           tests, Linux virtual desktop L1
#   live    L2 / RD D8b through the daemon: builds k2-deskd and its
#           x11_animate example (release), then the daemon's live desktop
#           stream tests one at a time (VP8, JPEG-only, 1080p15 on one
#           core, a just-thawed display); the banner test runs twice, with
#           a session bus that has no notification server
#           (dbus-run-session) and with no bus at all
#   deny    cargo deny --all-features (licences + bans), Steward R27 and
#           R32 (kessel-video's x11rb)
#
# Tests run with every K2_* variable unset and a temp HOME (CLAUDE.md); cargo
# keeps its own CARGO_HOME / RUSTUP_HOME. Every step prints its exit code and
# the script exits non-zero if any step failed.
set -uo pipefail

mode="${1:-}"
fail=0
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
TEST_HOME="$(mktemp -d)"

# The environment a test process gets: no K2_*, a throwaway HOME.
test_env() {
    local args=() v
    while IFS='=' read -r v _; do
        case "$v" in K2_*) args+=(-u "$v") ;; esac
    done < <(env)
    env "${args[@]}" HOME="$TEST_HOME" "$@"
}

step() {
    local name="$1"
    shift
    echo ""
    echo "== $name"
    "$@"
    local rc=$?
    echo "== $name: exit $rc"
    [ "$rc" -eq 0 ] || fail=1
}

# The packages Cargo.lock gains against DESKD_LOCK_BASE (default HEAD's own
# lock): only k2-deskd and k2-deskd-proto.
lock_adds_only_ours() {
    cargo metadata --format-version 1 >/dev/null || return 1
    local base added
    # DESKD_LOCK_BASE: the main commit these crates were added on.
    base="${DESKD_LOCK_BASE:-HEAD}"
    added="$(diff <(git -c safe.directory="*" show "$base:Cargo.lock" | sed -n 's/^name = "\(.*\)"/\1/p' | sort -u) \
                  <(sed -n 's/^name = "\(.*\)"/\1/p' Cargo.lock | sort -u) | sed -n 's/^> //p')"
    echo "packages new to Cargo.lock since $base: ${added:-none}"
    for p in $added; do
        case "$p" in k2-deskd|k2-deskd-proto) ;; *) echo "FAIL: $p is a new third-party crate"; return 1 ;; esac
    done
}

# "name version" for every package in Cargo.lock.
lock_versions() {
    awk '/^name = /{gsub(/"/,"",$3); n=$3} /^version = /{gsub(/"/,"",$3); if (n!="") print n, $3; n=""}' Cargo.lock | sort
}

xvfb_tests() {
    if ! command -v Xvfb >/dev/null 2>&1; then
        echo "Xvfb not installed: skipping the X11 integration tests (FAIL: they are required on Linux)"
        fail=1
        return
    fi
    local disp=":$((90 + RANDOM % 9))"
    Xvfb "$disp" -screen 0 1920x1080x24 -nolisten tcp >/tmp/deskd-xvfb.log 2>&1 &
    local xpid=$!
    sleep 1
    step "Xvfb integration (k2-deskd --features xvfb-tests)" \
        test_env DISPLAY="$disp" XAUTHORITY= cargo nextest run -p k2-deskd --features xvfb-tests --test xvfb --no-fail-fast -j 1
    kill "$xpid" 2>/dev/null
    wait "$xpid" 2>/dev/null
}

case "$mode" in
    linux)
        step "check deskd crates" cargo check -p k2-deskd-proto -p k2-deskd --all-targets
        step "nextest deskd crates" test_env cargo nextest run -p k2-deskd-proto -p k2-deskd --no-fail-fast
        step "nextest daemon desktop module" test_env cargo nextest run -p k2-daemon --lib --no-fail-fast -E 'test(/desktop::/)'
        step "nextest bundle layout (K2 Screen beside K2 Daemon)" test_env cargo nextest run -p k2-daemon --test it_misc --no-fail-fast -E 'test(/macos_daemon_helper_app_layout::/)'
        xvfb_tests
        ;;
    linux-unit)
        # linux without the Xvfb tests (for nodes that don't offer Xvfb).
        step "check deskd crates" cargo check -p k2-deskd-proto -p k2-deskd --all-targets
        step "nextest deskd crates" test_env cargo nextest run -p k2-deskd-proto -p k2-deskd --no-fail-fast
        step "nextest daemon desktop module" test_env cargo nextest run -p k2-daemon --lib --no-fail-fast -E 'test(/desktop::/)'
        step "nextest bundle layout (K2 Screen beside K2 Daemon)" test_env cargo nextest run -p k2-daemon --test it_misc --no-fail-fast -E 'test(/macos_daemon_helper_app_layout::/)'
        ;;
    macos)
        step "check deskd crates" cargo check -p k2-deskd-proto -p k2-deskd --all-targets
        step "test deskd crates" test_env cargo test -p k2-deskd-proto -p k2-deskd --no-fail-fast
        ;;
    mac-live)
        # D5's live encode test: VideoToolbox through deskd's capture engine,
        # units on a real pipe, decoded again. No GUI session or Screen
        # Recording needed. The ScreenCaptureKit half is
        # scripts/test/deskd-sck-live.sh (a logged-in session).
        step "live encode (k2-deskd --features mac-live)" \
            test_env cargo test --release -p k2-deskd --features mac-live --test mac_live_encode -- --nocapture --test-threads=1
        ;;
    macos-daemon)
        # A daemon test link needs more than an 8 GB mini has: check the
        # macOS cfg branches here; the module's tests run in linux mode.
        step "check daemon lib + tests (macos)" cargo check -p k2-daemon --lib --tests
        ;;
    xvfb)
        xvfb_tests
        ;;
    x11)
        step "kessel-video lib tests" test_env cargo test -p kessel-video --lib --no-fail-fast
        step "kessel-video X11 capture (Xvfb, the ignored ones)" \
            test_env cargo test -p kessel-video --lib --no-fail-fast -- --include-ignored linux::x11::
        ;;
    live)
        step "build k2-deskd + x11_animate (release)" cargo build --release -p k2-deskd --bin k2-deskd --example x11_animate
        # Cargo's target folder (compute nodes set their own).
        tdir="$(cargo metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
        tdir="${tdir:-$PWD/target}"
        live_env() {
            test_env DESKD_LIVE_BIN="$tdir/release/k2-deskd" DESKD_LIVE_ANIMATE="$tdir/release/examples/x11_animate" "$@"
        }
        live_filter='test(/desktop::stream::tests::live::/)'
        if command -v dbus-run-session >/dev/null 2>&1; then
            step "live desktop stream (session bus, no notification server)" \
                live_env dbus-run-session -- cargo nextest run -p k2-daemon --lib --run-ignored only --no-fail-fast -j 1 --no-capture -E "$live_filter"
        else
            echo "dbus-run-session not installed: the bus case is not run"
        fi
        step "live desktop stream (no session bus)" \
            live_env env -u DBUS_SESSION_BUS_ADDRESS cargo nextest run -p k2-daemon --lib --run-ignored only --no-fail-fast -j 1 --no-capture -E "$live_filter"
        ;;
    deny)
        # One run covers macOS and Linux: crates/k2-deskd/deny.toml lists both
        # targets. Then the lockfile: the only new packages may be our own.
        # (cargo-deny 0.20 takes --config before the subcommand.)
        step "cargo deny k2-deskd --all-features (macOS + Linux targets)" \
            cargo deny --manifest-path crates/k2-deskd/Cargo.toml --all-features --config "$PWD/crates/k2-deskd/deny.toml" check licenses bans advisories
        step "cargo deny k2-daemon's new wire crate --all-features" \
            cargo deny --manifest-path crates/k2-deskd-proto/Cargo.toml --all-features --config "$PWD/crates/k2-deskd/deny.toml" check licenses bans
        step "cargo deny kessel-video --all-features (every target; R32: x11rb shm/damage/xfixes only)" \
            cargo deny --manifest-path crates/kessel-video/Cargo.toml --all-features --config "$PWD/crates/kessel-video/deny.toml" check licenses bans advisories
        step "Cargo.lock adds no third-party crate" lock_adds_only_ours
        ;;
    lock)
        # Steward (R27 follow-up): fix the two zbus-tree advisories in the
        # lock, no ignores. Prints every package whose version changed.
        before="$(lock_versions)"
        step "cargo metadata (lock gains k2-deskd, k2-deskd-proto)" cargo metadata --format-version 1 >/dev/null
        step "event-listener 5.4.2 (RUSTSEC-2026-0221)" cargo update -p event-listener --precise 5.4.2
        step "async-executor 1.14.1 (1.14.0 is yanked)" cargo update -p async-executor --precise 1.14.1
        after="$(lock_versions)"
        echo "Cargo.lock package changes (< before, > after):"
        diff <(echo "$before") <(echo "$after")
        ;;
    *)
        echo "usage: $0 linux|linux-unit|macos|macos-daemon|xvfb|x11|live|deny|lock" >&2
        exit 2
        ;;
esac

rm -rf "$TEST_HOME"
echo ""
echo "deskd-checks $mode: $([ "$fail" -eq 0 ] && echo PASS || echo FAIL)"
exit "$fail"
