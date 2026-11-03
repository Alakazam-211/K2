#!/usr/bin/env bash
# The Arch release asset never changes bytes (omarchy listing O2): first upload,
# same-bytes re-run, different-bytes refusal, and gh failures, all with exact
# exit codes and text. gh is a stub over a folder per release.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/arch-upload-pkg.sh"
WF="$ROOT/.github/workflows/arch-package.yml"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
npass=0 nfail=0
pass() { echo "  ok: $1"; npass=$((npass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || echo "        $2" >&2; nfail=$((nfail + 1)); }

mkdir -p "$tmp/bin" "$tmp/rel/v9.9.9"
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$STUB_LOG"
[ -z "${STUB_FAIL:-}" ] || { echo "gh: stub failure" >&2; exit 1; }
case "$1 $2" in
    "release view")
        [ -d "$STUB_REL/$3" ] || { echo "release not found" >&2; exit 1; }
        ls "$STUB_REL/$3"
        ;;
    "release upload")
        case " $* " in *" --clobber "*) echo "stub: --clobber must never be used" >&2; exit 99 ;; esac
        [ ! -e "$STUB_REL/$3/$(basename "$4")" ] || { echo "stub: asset exists" >&2; exit 1; }
        cp "$4" "$STUB_REL/$3/"
        ;;
    "release download")
        tag="$3"; shift 3
        while [ $# -gt 0 ]; do
            case "$1" in
                --pattern) pat="$2"; shift 2 ;;
                --dir) dir="$2"; shift 2 ;;
                *) shift ;;
            esac
        done
        cp "$STUB_REL/$tag/$pat" "$dir/"
        ;;
    *) echo "stub: unexpected gh $*" >&2; exit 98 ;;
esac
STUB
chmod +x "$tmp/bin/gh"
export PATH="$tmp/bin:$PATH" STUB_REL="$tmp/rel" STUB_LOG="$tmp/gh.log" K2_RELEASE_REPO=stub/K2
unset STUB_FAIL || true
: > "$tmp/gh.log"

NAME=k2-9.9.9-x86_64.pkg.tar.zst
printf 'first build\n' > "$tmp/$NAME"
mkdir -p "$tmp/other" "$tmp/same"
printf 'second build\n' > "$tmp/other/$NAME"
printf 'first build\n' > "$tmp/same/$NAME"

# run <want rc> <want text> <args...>
run() {
    local want="$1" text="$2" rc=0
    shift 2
    bash "$SCRIPT" "$@" > "$tmp/out" 2>&1 || rc=$?
    [ "$rc" = "$want" ] || { echo "exit $rc, wanted $want: $(cat "$tmp/out")"; return 1; }
    grep -qF -- "$text" "$tmp/out" || { echo "no '$text' in: $(cat "$tmp/out")"; return 1; }
}
check() {
    local name="$1" r
    shift
    if r="$(run "$@")"; then pass "$name"; else fail "$name" "$r"; fi
}

echo "== the Arch asset never changes bytes =="
check "wrong arguments: exit 2" 2 "Usage:" v9.9.9
check "a missing package: exit 1" 1 "is missing or empty" v9.9.9 "$tmp/no-such.pkg"
if [ ! -s "$tmp/gh.log" ]; then pass "gh was not called for a missing package"; else fail "gh called" "$(cat "$tmp/gh.log")"; fi

export STUB_FAIL=1
check "gh can't list the release: exit 1" 1 "stub failure" v9.9.9 "$tmp/$NAME"
unset STUB_FAIL
if [ ! -e "$tmp/rel/v9.9.9/$NAME" ]; then pass "nothing uploaded when gh can't list"; else fail "uploaded anyway"; fi

check "first upload" 0 "Uploaded $NAME to v9.9.9" v9.9.9 "$tmp/$NAME"
if cmp -s "$tmp/$NAME" "$tmp/rel/v9.9.9/$NAME"; then pass "the release holds the built bytes"; else fail "bytes differ after upload"; fi

: > "$tmp/gh.log"
check "same bytes again: exit 0" 0 "already has these exact $NAME bytes" v9.9.9 "$tmp/same/$NAME"
if grep -q 'release upload' "$tmp/gh.log"; then fail "a same-bytes re-run uploaded" "$(cat "$tmp/gh.log")"; else pass "a same-bytes re-run uploads nothing"; fi

: > "$tmp/gh.log"
check "different bytes: exit 1" 1 "already has a DIFFERENT $NAME" v9.9.9 "$tmp/other/$NAME"
if grep -q 'release upload' "$tmp/gh.log"; then fail "different bytes uploaded" "$(cat "$tmp/gh.log")"; else pass "different bytes upload nothing"; fi
if cmp -s "$tmp/$NAME" "$tmp/rel/v9.9.9/$NAME"; then pass "the published file is unchanged"; else fail "published file changed"; fi
if grep -qF "scripts/linux-sums.sh merge v9.9.9 arch.sums" "$tmp/out"; then pass "the refusal says how to recover"; else fail "no recovery line" "$(cat "$tmp/out")"; fi
pub="$(shasum -a 256 "$tmp/$NAME" | awk '{print $1}')"
new="$(shasum -a 256 "$tmp/other/$NAME" | awk '{print $1}')"
if grep -qF "Published: $pub" "$tmp/out" && grep -qF "This build: $new" "$tmp/out"; then pass "the refusal names both sha256s"; else fail "shas missing" "$(cat "$tmp/out")"; fi

check "a release that does not exist: exit 1" 1 "release not found" v0.0.0 "$tmp/$NAME"

echo "== the workflow uses it =="
if python3 -I - "$WF" <<'PY'
import sys
s = open(sys.argv[1]).read()
step = s[s.index("- name: Upload to release"):s.index("- name: SHA256SUMS line for the package")]
assert 'scripts/arch-upload-pkg.sh "$IN_TAG" "$PKG_PATH"' in step, step
assert "--clobber" not in step.replace("Never --clobber", ""), step
assert "gh release upload" not in step, step
assert "K2_RELEASE_REPO: ${{ github.repository }}" in step, step
PY
then pass "arch-package.yml uploads the package only through arch-upload-pkg.sh"; else fail "workflow upload" "see above"; fi

echo
echo "arch-upload-pkg: $npass passed, $nfail failed"
[ "$nfail" -eq 0 ]
