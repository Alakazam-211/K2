#!/usr/bin/env bash
# `k2 daemon install --version` takes the release with or without its v.
#
# Regression: the manifest URL template already adds the tag's v
# (.../releases/download/v%s/daemon-latest.json), so `--version v0.46.7`
# asked for .../download/vv0.46.7/... and failed. Now 0.46.7 and v0.46.7
# both ask for .../download/v0.46.7/daemon-latest.json and install that
# release; a release older than the first one signed with the update keys
# is refused in both spellings before any request.
#
# Hermetic: env -i, a temp HOME, stubs on PATH for uname (Linux x86_64) and
# curl (logs each call; answers only the v0.46.7 manifest URL, refuses the
# rest). `--dry-run --no-service`: nothing is downloaded or written. Loud.
# Run with: bash tests/cli/daemon_install_version_tag.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
has() {
    case "$2" in
        *"$3"*) ok "$1" ;;
        *) bad "$1 (missing $(printf %q "$3"))"; printf '%s\n' "$2" | sed 's/^/      | /' >&2 ;;
    esac
}

WORK="$(mktemp -d -t k2-install-version-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
STUBS="$WORK/stubs"
H="$WORK/home"
LOG="$WORK/curl.log"
mkdir -p "$STUBS" "$H/.k2"
WANT_URL="https://github.com/Alakazam-211/K2/releases/download/v0.46.7/daemon-latest.json"
cat >"$WORK/manifest.json" <<'EOF'
{
  "version": "0.46.7",
  "platforms": {
    "linux-x86_64": {
      "url": "https://github.com/Alakazam-211/K2/releases/download/v0.46.7/k2-daemon-linux-x86_64",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "sig2": "https://github.com/Alakazam-211/K2/releases/download/v0.46.7/k2-daemon-linux-x86_64.sig2"
    }
  }
}
EOF
REAL_UNAME="$(command -v uname)"
cat >"$STUBS/uname" <<EOF
#!/bin/sh
case "\${1:-}" in
    -s) echo Linux ;;
    -m) echo x86_64 ;;
    *) exec "$REAL_UNAME" "\$@" ;;
esac
EOF
cat >"$STUBS/curl" <<EOF
#!/bin/sh
printf 'curl %s\n' "\$*" >>"\$STUB_LOG"
# The body goes where -o says (a GitHub fetch follows each redirect itself).
out=""; prev=""
for a in "\$@"; do
    [ "\$prev" = "-o" ] && out="\$a"
    prev="\$a"
done
for a in "\$@"; do
    if [ "\$a" = "$WANT_URL" ]; then
        if [ -n "\$out" ]; then cat "$WORK/manifest.json" >"\$out"; else cat "$WORK/manifest.json"; fi
        exit 0
    fi
done
echo "stub curl: refusing \$*" >&2
exit 22
EOF
chmod +x "$STUBS/uname" "$STUBS/curl"

# run <args...>: `k2 daemon install --dry-run --no-service <args>`; sets OUT, RC.
run() {
    : >"$LOG"
    set +e
    OUT="$(env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" STUB_LOG="$LOG" \
        "$K2" daemon install --dry-run --no-service "$@" 2>&1)"
    RC=$?
    set -e
}

for v in 0.46.7 v0.46.7; do
    echo "== --version $v =="
    run --version "$v"
    [ "$RC" -eq 0 ] && ok "$v: exit 0" || bad "$v: exit $RC (want 0)"
    # One request, for that URL, following no redirect on curl's own
    # (tests/cli/update_github_redirects.sh covers the redirects).
    n="$(grep -c . "$LOG" || true)"
    if [ "$n" = 1 ] && grep -q " $WANT_URL\$" "$LOG" && grep -q -- ' --no-location --proto =https ' "$LOG"; then
        ok "$v: asked for exactly $WANT_URL, https only, no curl -L"
    else
        bad "$v: the requests were not exactly $WANT_URL"; sed 's/^/      | /' "$LOG" >&2
    fi
    if grep -q '/download/vv' "$LOG"; then bad "$v: a vv URL was asked for"; else ok "$v: no vv URL"; fi
    has "$v: the dry run's manifest url" "$OUT" "  manifest url:   $WANT_URL"
    has "$v: the manifest's version" "$OUT" "  manifest ver:   0.46.7"
    has "$v: the requested version" "$OUT" "  requested ver:  0.46.7"
done

for v in 0.46.5 v0.46.5; do
    echo "== --version $v (older than the first release signed with the update keys) =="
    run --version "$v"
    [ "$RC" -eq 1 ] && ok "$v: exit 1" || bad "$v: exit $RC (want 1)"
    has "$v: says why" "$OUT" "Versions before 0.46.6 are signed with the retired update key"
    if [ -s "$LOG" ]; then bad "$v: sent a request:"; sed 's/^/      | /' "$LOG" >&2; else ok "$v: sent nothing"; fi
done

echo ""
echo "daemon_install_version_tag: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
