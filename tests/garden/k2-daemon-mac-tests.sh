#!/usr/bin/env bash
# Tests for scripts/ci/k2-daemon-mac-tests.sh on a throwaway tree, then the
# real list against the real test sources.
# Run: bash tests/garden/k2-daemon-mac-tests.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
B="$HERE/scripts/ci/k2-daemon-mac-tests.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(bash "$B" --root "$T" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL ($*): exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL ($*): output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
C="$T/crates/k2-daemon"
L="$C/tests/mac-unwalled-tests.txt"
mkdir -p "$C/src/mail" "$C/src/spawn" "$C/tests/it_sessions"
# The crate's own tests, in each place a test module can live.
printf '#[cfg(test)]\nmod tests {\n    #[test]\n    fn inline_one() {}\n}\n' > "$C/src/plain.rs"
printf '    #[tokio::test]\n    async fn moved_one() {}\n' > "$C/src/mail/send_tests.rs"
printf '#[test]\nfn dir_one() {}\n' > "$C/src/spawn/tests.rs"
# A grouped test binary, and a binary of one file.
printf '#[tokio::test(flavor = "multi_thread")]\nasync fn shell_one() {}\nfn helper_not_a_test_name_used_below() {}\n' > "$C/tests/it_sessions/shells.rs"
printf '#[test]\nfn single_one() {}\n' > "$C/tests/single_bin.rs"
good() {
    printf '# why\n\nlib  plain::tests::inline_one  a login shell\nlib\tmail::send::tests::moved_one\ta login shell\nlib  spawn::tests::dir_one  a login shell\nit_sessions  shells::shell_one  a login shell\nsingle_bin  single_one  a login shell\n' > "$L"
}
good
run 0 "ok (5 tests run on the Mac node with no wall)" check
# The filters: one term per test, the crate's own tests by kind, the others
# by binary; walled is the `not` of unwalled.
run 0 "(package(k2-daemon) & kind(lib) & test(=plain::tests::inline_one)) | (package(k2-daemon) & kind(lib) & test(=mail::send::tests::moved_one))" filter unwalled
run 0 "(package(k2-daemon) & binary(it_sessions) & test(=shells::shell_one)) | (package(k2-daemon) & binary(single_bin) & test(=single_one))" filter unwalled
[ "$(bash "$B" --root "$T" filter walled)" = "not ($(bash "$B" --root "$T" filter unwalled))" ] || { echo "FAIL: the filters are not complements" >&2; exit 1; }
pass=$((pass + 1))

# A listed test that is gone, renamed, or in another file: it would run nowhere.
printf 'lib  plain::tests::inline_two  x\n' > "$L"
run 1 "lib plain::tests::inline_two is listed, but no such test function is in the file its name points at" check
printf 'it_sessions  shells::moved_one  x\n' > "$L"
run 1 "it_sessions shells::moved_one is listed, but no such test function" check
printf 'it_sessions  gone_file::shell_one  x\n' > "$L"
run 1 "it_sessions gone_file::shell_one is listed" check
printf 'no_such_bin  single_one  x\n' > "$L"
run 1 "no_such_bin single_one is listed" check
# A name that only starts like a function there does not count.
printf 'it_sessions  shells::helper_not_a_test  x\n' > "$L"
run 1 "it_sessions shells::helper_not_a_test is listed" check
# Bad lists.
printf 'lib  plain::tests::inline_one\n' > "$L"
run 2 "a line is '<binary> <test name> <why>'" check
printf 'lib  plain::tests::inline_one  x\nlib  plain::tests::inline_one  y\n' > "$L"
run 2 "lib plain::tests::inline_one is listed twice" check
printf 'it-sessions  shells::shell_one  x\n' > "$L"
run 2 "'it-sessions' is not a test binary name" check
printf 'lib  plain::tests::inline_one)|all()  x\n' > "$L"
run 2 "is not a test name" filter walled
printf 'lib  plain:::inline_one  x\n' > "$L"
run 2 "is not a test name" check
printf '# nothing\n' > "$L"
run 2 "the list names no test" filter walled
rm "$L"
run 2 "no list at" check
good
run 2 "usage:" filter
run 2 "usage:" filter both
run 2 "too many arguments" filter walled extra
run 2 "usage:"

# The real list and the real test sources.
out="$(bash "$B" check)" || { echo "FAIL: the real list: $out" >&2; exit 1; }
pass=$((pass + 1))
echo "k2-daemon-mac-tests tests: $pass passed"
