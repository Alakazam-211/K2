#!/usr/bin/env bash
# Tests for scripts/lib/build-paths.sh: the release path maps (rustc flags in
# cargo's encoded form, C/C++ wrappers) and the gate that refuses a release
# file holding the builder's home or user name. Fixture files and a fake
# compiler only; nothing compiles, signs or touches ~/.k2. Runs on macOS and
# Linux. No skips.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LIB="$ROOT/scripts/lib/build-paths.sh"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
me="$(id -un)"

# gate <HOME> <USER> <path...>: the gate's exit code; its output in $work/out.
gate() {
    local h="$1" u="$2"
    shift 2
    set +e
    env HOME="$h" USER="$u" bash -c 'source "$1"; shift; k2_assert_no_build_paths "$@"' _ "$LIB" "$@" > "$work/out" 2>&1
    local rc=$?
    set -e
    return "$rc"
}
fixture() { printf '%b' "$2" > "$work/$1"; printf '%s' "$work/$1"; }

echo "the gate refuses the builder's paths:"
f="$(fixture home.bin 'x\0panicked at /Users/acmebuilder/.cargo/registry/src/foo-1.0/src/lib.rs:3:9\0y')"
if gate /Users/acmebuilder acmebuilder "$f"; then
    fail "a file with \$HOME is refused" "passed: $(cat "$work/out")"
else
    grep -qF "FATAL: $f holds the builder's path '/Users/acmebuilder' (1 times)" "$work/out" \
        && grep -qF "/Users/acmebuilder/.cargo/registry/src/foo-1.0/src/lib.rs" "$work/out" \
        && pass "a file with \$HOME is refused, naming the file and the string" || fail "home message" "$(cat "$work/out")"
fi
f="$(fixture user-linux.bin '/home/acmebuilder/src/k2/crates/a.rs')"
if gate /srv/builds/job1 acmebuilder "$f"; then
    fail "/home/<user> is refused (HOME elsewhere)" "passed: $(cat "$work/out")"
else
    grep -qF "'/home/acmebuilder'" "$work/out" && pass "/home/<user> is refused even when \$HOME is elsewhere" || fail "linux user form" "$(cat "$work/out")"
fi
f="$(fixture user-mac.bin 'at /Users/acmebuilder')"
if gate /srv/builds/job1 acmebuilder "$f"; then
    fail "/Users/<user> at the end of a string is refused" "passed: $(cat "$work/out")"
else
    pass "/Users/<user> is refused, also at the end of a string"
fi
f="$(fixture id-user.bin "/home/$me/.cargo/git/checkouts/x/src/lib.rs")"
if gate /srv/builds/job1 nobody-else "$f"; then
    fail "the id -un user is checked, not only \$USER" "passed: $(cat "$work/out")"
else
    pass "the user from id -un is checked, not only \$USER"
fi
mkdir -p "$work/App.app/Contents/Helpers/Some Helper.app/Contents/MacOS" "$work/App.app/Contents/MacOS"
printf 'clean' > "$work/App.app/Contents/MacOS/main"
printf '/Users/acmebuilder/DevProjects/Acme Labs/K2/target/release/build/x/out/y.c' > "$work/App.app/Contents/Helpers/Some Helper.app/Contents/MacOS/new-sidecar"
if gate /Users/acmebuilder acmebuilder "$work/App.app"; then
    fail "every file under an app dir is checked" "passed: $(cat "$work/out")"
else
    grep -qF "Some Helper.app/Contents/MacOS/new-sidecar holds" "$work/out" && ! grep -qF "MacOS/main holds" "$work/out" \
        && pass "every file under an app is checked (a new sidecar needs no list), only the bad one is named" \
        || fail "dir scan" "$(cat "$work/out")"
fi
f="$(fixture cargohome.bin '/srv/acme-cache/cargo-home/registry/src/a-1.0/src/lib.rs')"
if CARGO_HOME=/srv/acme-cache/cargo-home gate /srv/builds/job1 acmebuilder "$f"; then
    fail "a cargo home outside \$HOME is refused" "passed: $(cat "$work/out")"
else
    grep -qF "'/srv/acme-cache/cargo-home'" "$work/out" && pass "a cargo home outside \$HOME is refused (a compute node's cache)" || fail "cargo home" "$(cat "$work/out")"
fi
mkdir -p "$work/jobsrc"
f="$(fixture checkout.bin "$(cd "$work/jobsrc" && pwd -P)/target/release/build/x/out/y.c")"
if (cd "$work/jobsrc" && gate /srv/builds/job1 acmebuilder "$f"); then
    fail "the checkout (current dir) is refused" "passed: $(cat "$work/out")"
else
    pass "the checkout (the current dir) is refused, also outside \$HOME"
fi
mkdir -p "$work/realhome"
ln -s "$work/realhome" "$work/linkhome"
phys="$(cd "$work/realhome" && pwd -P)"
f="$(fixture phys.bin "$phys/.cargo/registry/src/a.rs")"
if gate "$work/linkhome" acmebuilder "$f"; then
    fail "a symlinked \$HOME is checked as its physical path too" "passed: $(cat "$work/out")"
else
    pass "a symlinked \$HOME is checked as its physical path too"
fi

echo "the gate passes clean files and has no false positives:"
f="$(fixture clean.bin 'panicked at /cargo/registry/src/foo-1.0/src/lib.rs\0/k2/crates/a.rs\0/home/.rustup/x')"
gate /Users/acmebuilder acmebuilder "$f" && grep -q "no builder paths in 1 file(s)" "$work/out" \
    && pass "a mapped binary passes" || fail "clean" "$(cat "$work/out")"
f="$(fixture bare.bin 'messages read [from acmebuilder]; e.g. https://acmebuilder.app.example.com')"
gate /Users/acmebuilder acmebuilder "$f" && pass "the bare user name (shipped text) is not a hit" || fail "bare name" "$(cat "$work/out")"
f="$(fixture longer.bin '/Users/acmebuilder2/x /home/acmebuilder.old/y /Users/acmebuilder_z')"
gate /Users/acmebuilder acmebuilder "$f" && pass "a longer user name is not a hit (/Users/al never matches /Users/alice)" || fail "boundary" "$(cat "$work/out")"
f="$(fixture short.bin '/al/x /home/al /Users/al')"
gate /home/al al "$f" && fail "a short user name in a home path is still a hit" "passed" || pass "a short user name is a hit only in a home path"
f="$(fixture short2.bin 'Your albums and /alpha/beta')"
gate /home/al al "$f" && pass "a short user name elsewhere is not a hit" || fail "short elsewhere" "$(cat "$work/out")"
f="$(fixture root.bin '/root/.config and /root')"
gate /root root "$f" && pass "a one-part \$HOME (/root) is not a needle" || fail "one-part home" "$(cat "$work/out")"
gate / acmebuilder "$f" && pass "HOME=/ is not a needle" || fail "home slash" "$(cat "$work/out")"
: > "$work/empty.bin"
gate /Users/acmebuilder acmebuilder "$work/empty.bin" && pass "an empty file passes" || fail "empty" "$(cat "$work/out")"
if gate /Users/acmebuilder acmebuilder "$work/nope.bin"; then fail "a missing file fails" "passed"; else pass "a missing file fails"; fi
if gate /Users/acmebuilder acmebuilder; then fail "no files fails" "passed"; else pass "no files given fails"; fi

echo "the maps:"
src="$work/Acme Labs/K2"
mkdir -p "$src" "$work/bin" "$work/cargo"
# A fake compiler: prints each argument on its own line.
printf '#!/bin/sh\nfor a in "$@"; do printf "ARG:%%s\\n" "$a"; done\n' > "$work/bin/cc"
chmod +x "$work/bin/cc"
maps() {
    # maps <out> <env...>: run k2_release_path_maps in $src with a fake HOME.
    local out="$1"
    shift
    (cd "$src" && env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS HOME="$work" CARGO_HOME="$work/cargo" PATH="$work/bin:$PATH" TMPDIR="$work/tmp" "$@" \
        bash -c 'set -euo pipefail; source "$1"; k2_release_path_maps "$PWD" >/dev/null
            printf "%s" "$K2_RELEASE_RUSTFLAGS" | tr "\037" "\n"; echo
            echo "CCDIR=$K2_RELEASE_CC_DIR"
            k2_path_mapped cc -c "x y.c"
            k2_path_mapped sh -c "printf \"ENC=%s\n\" \"\$CARGO_ENCODED_RUSTFLAGS\" | tr \"\037\" \"|\""
            echo "AFTER=${CARGO_ENCODED_RUSTFLAGS:-unset}"' _ "$LIB") > "$out" 2>&1
}
mkdir -p "$work/tmp"
wp="$(cd "$work" && pwd -P)"
sp="$(cd "$src" && pwd -P)"
maps "$work/m1" || fail "maps run" "$(cat "$work/m1")"
python3 - "$work/m1" "$work" "$wp" "$src" "$sp" <<'PY' && pass "rustc maps: \$HOME first, then the checkout, then the cargo home (last match wins); a path with a space is one flag" || fail "rustc maps" "$(cat "$work/m1")"
import sys
lines = open(sys.argv[1]).read().split("\n")
home, home_p, src, src_p = sys.argv[2:6]
flags = lines[:next(i for i, l in enumerate(lines) if l.startswith("CCDIR="))]
want = [f"--remap-path-prefix={home}=/home"]
if home_p != home:
    want.append(f"--remap-path-prefix={home_p}=/home")
want.append(f"--remap-path-prefix={src}=/k2")
if src_p != src:
    want.append(f"--remap-path-prefix={src_p}=/k2")
want.append(f"--remap-path-prefix={home}/cargo=/cargo")
if home_p != home:
    want.append(f"--remap-path-prefix={home_p}/cargo=/cargo")
assert flags == want, (flags, want)
PY
grep -qxF "ARG:-ffile-prefix-map=$src=/k2" "$work/m1" && grep -qxF "ARG:-ffile-prefix-map=$work=/home" "$work/m1" \
    && grep -qxF "ARG:x y.c" "$work/m1" && [ "$(grep -c '^ARG:' "$work/m1")" -ge 5 ] \
    && pass "the cc wrapper passes each map as one argument (the space survives), then the caller's" || fail "cc wrapper" "$(cat "$work/m1")"
python3 - "$work/m1" <<'PY' && pass "the wrapper's maps come in the same order (\$HOME first) and before the caller's arguments" || fail "cc order" "$(cat "$work/m1")"
import sys
a = [l[4:] for l in open(sys.argv[1]).read().split("\n") if l.startswith("ARG:")]
m = [x for x in a if x.startswith("-ffile-prefix-map=")]
assert m[0].endswith("=/home") and m[-1].endswith("=/cargo"), m
assert a[-2:] == ["-c", "x y.c"], a
PY
grep -q "^CCDIR=$work/tmp/k2-path-maps-[0-9]*$" "$work/m1" && pass "the wrappers live outside the checkout, in TMPDIR" || fail "cc dir" "$(cat "$work/m1")"
grep -q "^ENC=--remap-path-prefix=$work=/home|" "$work/m1" && grep -qx "AFTER=unset" "$work/m1" \
    && pass "k2_path_mapped sets CARGO_ENCODED_RUSTFLAGS for its command only" || fail "scope" "$(cat "$work/m1")"
d1="$(sed -n 's/^CCDIR=//p' "$work/m1")"
maps "$work/m1b" || true
[ "$(sed -n 's/^CCDIR=//p' "$work/m1b")" = "$d1" ] && pass "the same maps give the same wrapper dir (stable compiler path)" || fail "stable dir" "$(cat "$work/m1b")"
maps "$work/m2" RUSTFLAGS="-C keep  --cfg acme" || true
[ "$(sed -n '1,3p' "$work/m2" | tr '\n' ' ')" = "-C keep --cfg " ] && sed -n 4p "$work/m2" | grep -qx acme \
    && sed -n 5p "$work/m2" | grep -q "^--remap-path-prefix=$work=/home$" \
    && pass "the caller's RUSTFLAGS are kept, first" || fail "RUSTFLAGS kept" "$(cat "$work/m2")"
maps "$work/m3" CARGO_ENCODED_RUSTFLAGS="$(printf -- '-C\037link-arg=a b')" RUSTFLAGS="-C ignored" || true
[ "$(sed -n 1p "$work/m3")" = "-C" ] && [ "$(sed -n 2p "$work/m3")" = "link-arg=a b" ] && ! grep -q ignored "$work/m3" \
    && pass "a caller's CARGO_ENCODED_RUSTFLAGS is kept as is (and wins over RUSTFLAGS, as in cargo)" || fail "encoded kept" "$(cat "$work/m3")"
if (env -u K2_RELEASE_RUSTFLAGS -u K2_RELEASE_CC_DIR bash -c 'source "$1"; k2_path_mapped true' _ "$LIB" >/dev/null 2>&1); then
    fail "k2_path_mapped without maps fails" "passed"
else
    pass "k2_path_mapped without maps fails"
fi

echo "nothing is dropped, and every release build uses them:"
# CARGO_ENCODED_RUSTFLAGS replaces config rustflags: the repo must have none.
if grep -nE '^[[:space:]]*(rustflags|\[build\]|\[target)' "$ROOT/.cargo/config.toml"; then
    fail ".cargo/config.toml sets no rustflags" "the release build's CARGO_ENCODED_RUSTFLAGS would drop them; add them in build-paths.sh"
else
    pass ".cargo/config.toml sets no [build]/[target] rustflags (the encoded flags would drop them)"
fi
grep -qx '    k2_release_path_maps "$PWD" || return 1' "$ROOT/scripts/lib/build-binaries.sh" \
    && grep -qx '    k2_assert_no_build_paths "$app" || return 1' "$ROOT/scripts/lib/build-binaries.sh" \
    && pass "macOS Step 2 maps its builds and checks the whole app" || fail "build-binaries.sh" "no maps or no check"
grep -q 'k2_path_mapped cargo build --release "${K2_MAC_SIDECAR_CARGO_ARGS\[@\]}"' "$ROOT/scripts/macos-daemon-helper-app.sh" \
    && grep -q 'k2_assert_no_build_paths "${CARGO_TARGET_DIR:-target}/release/k2-daemon" || exit 1' "$ROOT/scripts/macos-daemon-helper-app.sh" \
    && pass "the Step 8.5 standalone daemon is mapped and checked" || fail "standalone" "no maps or no check"
grep -qx 'k2_assert_no_build_paths "$OUT/$DAEMON" "$OUT/$NODE" "$OUT/$HELPER" "$OUT/$ENGINE"' "$ROOT/scripts/ci/build-linux-bins.sh" \
    && grep -qx 'k2_assert_no_build_paths "$TARGET_DIR/release/k2"' "$ROOT/scripts/ci/build-linux-app.sh" \
    && pass "the Linux release builds check every binary they ship" || fail "linux" "no check"
builds="$(sed -E 's/^[[:space:]]+//' "$ROOT/scripts/lib/build-binaries.sh" | grep -vE '^(#|echo )' | grep -E 'cargo build --release|bun run tauri build')"
n="$(printf '%s\n' "$builds" | grep -c .)"
m="$(printf '%s\n' "$builds" | grep -cE 'k2_path_mapped (cargo build --release|bun run tauri build)')"
[ "$n" = "$m" ] && [ "$n" = 3 ] && pass "every cargo and tauri build in build-binaries.sh runs mapped ($m of $n)" || fail "unmapped build" "$m of $n mapped"
bash -n "$LIB" && pass "bash -n build-paths.sh" || fail "bash -n" "syntax"

echo ""
echo "build-paths: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
