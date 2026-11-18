#!/usr/bin/env bash
# `k2 update` / `--app` / `--list` respect air-gap and need no daemon.
#
# Regression: these forms only ask GitHub (the latest release, the release
# list), yet they ignored air-gap and went to GitHub with "airgap": true or
# K2_AIRGAP=1, and they stopped at the connection gate ("Cannot connect to
# K2") on a box with no daemon connection. Now:
#   - under air-gap each refuses before any request, with the line and exit
#     code `k2 daemon install` uses;
#   - with air-gap off they skip the connection gate, as `k2 daemon install`
#     does, and print the versions from GitHub;
#   - `k2 update --cli` reads the local daemon's version, so it keeps the
#     gate; `k2 update --help` still prints under air-gap.
#
# How: a curl stub first on PATH logs every call; it answers GitHub's
# releases/latest and releases?per_page=10 from fixtures and refuses
# everything else. Hermetic: env -i, a temp HOME, no daemon, no network.
# Loud. Run with: bash tests/cli/update_app_airgap.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 required (k2 update --app reads GitHub's JSON with it)" >&2; exit 1; }

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
hasnt() {
    case "$2" in
        *"$3"*) bad "$1 (found $(printf %q "$3"))"; printf '%s\n' "$2" | sed 's/^/      | /' >&2 ;;
        *) ok "$1" ;;
    esac
}

WORK="$(mktemp -d -t k2-update-airgap-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
STUBS="$WORK/stubs"
H="$WORK/home"
LOG="$WORK/curl.log"
mkdir -p "$STUBS" "$H/.k2"
cat >"$STUBS/curl" <<'EOF'
#!/bin/sh
printf 'curl %s\n' "$*" >>"$STUB_LOG"
for a in "$@"; do
    case "$a" in
        https://api.github.com/repos/Alakazam-211/K2/releases/latest)
            printf '{"tag_name":"v9.9.9","published_at":"2026-01-02T00:00:00Z"}'
            exit 0 ;;
        "https://api.github.com/repos/Alakazam-211/K2/releases?per_page=10")
            printf '[{"tag_name":"v9.9.9","published_at":"2026-01-02T00:00:00Z","prerelease":false},'
            printf '{"tag_name":"v9.9.8","published_at":"2026-01-01T00:00:00Z","prerelease":true}]'
            exit 0 ;;
    esac
done
echo "stub curl: refusing $*" >&2
exit 22
EOF
chmod +x "$STUBS/curl"

REFUSE_ON="Error: air-gap is on (K2_AIRGAP=1). This command cannot phone Connect, GitHub, or other hosted services."
NO_CONN="Error: Cannot connect to K2."

# settings <on|off|none>: the persisted air-gap setting.
settings() {
    rm -f "$H/.k2/settings.json"
    case "$1" in
        on) printf '{\n  "airgap": true\n}\n' >"$H/.k2/settings.json" ;;
        off) printf '{\n  "airgap": false\n}\n' >"$H/.k2/settings.json" ;;
        none) ;;
    esac
}

# run <conn|noconn> [VAR=value...] -- <k2 args...>: sets OUT (stdout and
# stderr), RC; clears the curl log first. conn hands the CLI a dead port and
# a dummy token; noconn gives neither.
run() {
    local how="$1"; shift
    local extra=()
    [ "$how" = conn ] && extra=(K2_PORT=1 K2_HOOK_TOKEN=test-token)
    while [ "$1" != "--" ]; do extra+=("$1"); shift; done
    shift
    : >"$LOG"
    set +e
    OUT="$(env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" STUB_LOG="$LOG" ${extra[@]+"${extra[@]}"} \
        "$K2" update "$@" 2>&1)"
    RC=$?
    set -e
}

no_request() {
    if [ -s "$LOG" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$LOG" >&2
    else
        ok "$1: sent nothing"
    fi
}

echo "== air-gap on: every GitHub form refuses before any request =="
for src in setting env; do
    for how in conn noconn; do
        for form in "" "--app" "--list" "--app --list"; do
            if [ "$src" = setting ]; then settings on; envs=(); else settings none; envs=(K2_AIRGAP=1); fi
            # shellcheck disable=SC2086
            run "$how" ${envs[@]+"${envs[@]}"} -- $form
            label="$src, $how: k2 update${form:+ $form}"
            [ "$RC" -eq 1 ] && ok "$label: exit 1" || bad "$label: exit $RC (want 1)"
            has "$label: the air-gap line" "$OUT" "$REFUSE_ON"
            hasnt "$label: not the connection gate" "$OUT" "$NO_CONN"
            no_request "$label"
        done
    done
done

echo "== air-gap on: --help still prints =="
settings on
run noconn -- --help
[ "$RC" -eq 0 ] && ok "--help under air-gap: exit 0" || bad "--help under air-gap: exit $RC (want 0)"
has "--help under air-gap: the usage" "$OUT" "Usage: k2 update [--app|--cli [--check]|--list]"
has "--help says --app and --list are refused under air-gap" "$OUT" "so they are refused while air-gap is on"
no_request "--help under air-gap"

echo "== air-gap off, no daemon connection: the GitHub forms run =="
for src in setting-off none env-off; do
    case "$src" in
        setting-off) settings off; envs=() ;;
        none) settings none; envs=() ;;
        env-off) settings on; envs=(K2_AIRGAP=0) ;;
    esac
    run noconn ${envs[@]+"${envs[@]}"} -- --app
    label="$src: k2 update --app"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    hasnt "$label: no connection gate" "$OUT" "$NO_CONN"
    has "$label: the latest tag" "$OUT" "  Latest:  v9.9.9"
    has "$label: the download page" "$OUT" "https://github.com/Alakazam-211/K2/releases/tag/v9.9.9"
    grep -q 'releases/latest' "$LOG" && ok "$label: asked GitHub" || bad "$label: no GitHub request"

    run noconn ${envs[@]+"${envs[@]}"} --
    label="$src: k2 update"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    has "$label: the latest tag" "$OUT" "  Latest:  v9.9.9"

    run noconn ${envs[@]+"${envs[@]}"} -- --list
    label="$src: k2 update --list"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    hasnt "$label: no connection gate" "$OUT" "$NO_CONN"
    has "$label: a release" "$OUT" "  v9.9.9  2026-01-02"
    has "$label: a pre-release" "$OUT" "  v9.9.8  2026-01-01 (pre-release)"
    grep -q 'releases?per_page=10' "$LOG" && ok "$label: asked GitHub" || bad "$label: no GitHub request"
done

echo "== --cli keeps the connection gate =="
settings none
run noconn -- --cli
[ "$RC" -eq 1 ] && ok "--cli without a connection: exit 1" || bad "--cli without a connection: exit $RC (want 1)"
has "--cli without a connection: the gate's line" "$OUT" "$NO_CONN"
no_request "--cli without a connection"

echo ""
echo "update_app_airgap: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
