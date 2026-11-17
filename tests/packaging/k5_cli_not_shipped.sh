#!/usr/bin/env bash
# The k5-cli binary is built by the workspace but not shipped yet (K2's
# terms for R1): no release, bundle, packaging or CLI-install script names
# it. This fails, naming the file and line, if one does.
#
# Scanned: scripts/release.sh, scripts/package-*.sh, scripts/install-*.sh,
# scripts/provision-k2-server.sh, scripts/arch-release-asset.sh,
# scripts/sign-*.sh, scripts/windows-sign-after-release.sh, every file
# under packaging/, the Tauri bundle configs (src-tauri/tauri*.json), and
# the code that installs or stages the CLI (k2-client's cli_install and
# win_cli, k2-core's cli_stage, the root helper's cli_refresh, the app's
# Install CLI in src-tauri's settings). A listed file or glob that matches
# nothing fails too: a rename must update this list.
#
# It proves it bites first: on a scratch copy of those files it must pass,
# then fail once a copy names k5-cli, then fail once a listed file is gone.
# No cargo, no daemon, no network. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# One path or glob per line, from the repo root. A trailing /** means
# every file under that folder.
LISTED="scripts/release.sh
scripts/package-*.sh
scripts/install-*.sh
scripts/provision-k2-server.sh
scripts/arch-release-asset.sh
scripts/sign-*.sh
scripts/windows-sign-after-release.sh
packaging/**
src-tauri/tauri*.json
crates/k2-client/src/cli_install.rs
crates/k2-client/src/win_cli.rs
crates/k2-core/src/cli_stage.rs
crates/k2-daemon/src/root_helper/cli_refresh.rs
src-tauri/src/commands/settings.rs"

# scan <root>: prints each file:line that names k5-cli, and each listed
# entry that matches nothing; exit 1 when there is any.
scan() {
    local root="$1" entry f bad=0 matched hits
    local -a files
    while IFS= read -r entry; do
        files=()
        case "$entry" in
            */\*\*)
                if [ -d "$root/${entry%/\*\*}" ]; then
                    while IFS= read -r f; do files+=("$f"); done < <(cd "$root" && find "${entry%/\*\*}" -type f | LC_ALL=C sort)
                fi
                ;;
            *)
                matched="$(cd "$root" && compgen -G "$entry" || true)"
                if [ -n "$matched" ]; then
                    while IFS= read -r f; do files+=("$f"); done <<<"$matched"
                fi
                ;;
        esac
        if [ "${#files[@]}" -eq 0 ]; then
            echo "MISSING: $entry matches no file (update the list in tests/packaging/k5_cli_not_shipped.sh)"
            bad=1
            continue
        fi
        for f in "${files[@]}"; do
            if hits="$(grep -n -F -- "k5-cli" "$root/$f")"; then
                while IFS= read -r line; do echo "NAMES k5-cli: $f:$line"; done <<<"$hits"
                bad=1
            fi
        done
    done <<<"$LISTED"
    return "$bad"
}

pass=0
die() { echo "FAIL: $*" >&2; exit 1; }

# ── The check bites: a scratch copy of the listed files ────────────────
T="$(mktemp -d -t k5-not-shipped-XXXXXX)"
trap 'rm -rf "$T"' EXIT
copied=0
while IFS= read -r entry; do
    case "$entry" in
        */\*\*) list="$(cd "$ROOT" && find "${entry%/\*\*}" -type f)" ;;
        *) list="$(cd "$ROOT" && compgen -G "$entry" || true)" ;;
    esac
    [ -n "$list" ] || die "the repo has nothing for $entry (update the list)"
    while IFS= read -r f; do
        mkdir -p "$T/$(dirname "$f")"
        cp "$ROOT/$f" "$T/$f"
        copied=$((copied + 1))
    done <<<"$list"
done <<<"$LISTED"
[ "$copied" -ge 20 ] || die "only $copied files copied: the globs no longer match what they did"

out="$(scan "$T")" || die "the scratch copy fails before any change: $out"
pass=$((pass + 1))

echo 'cargo build --release -p k5-cli' >>"$T/scripts/release.sh"
want_line="$(wc -l <"$T/scripts/release.sh" | tr -d ' ')"
if out="$(scan "$T")"; then die "a release.sh that builds k5-cli passed"; fi
grep -qxF "NAMES k5-cli: scripts/release.sh:$want_line:cargo build --release -p k5-cli" <<<"$out" \
    || die "the failure doesn't name scripts/release.sh:$want_line: $out"
pass=$((pass + 1))
cp "$ROOT/scripts/release.sh" "$T/scripts/release.sh"

first_pkg="$(cd "$T" && find packaging -type f | LC_ALL=C sort | head -1)"
printf 'install -m755 target/release/k5-cli "$pkgdir/usr/bin/k5"\n' >>"$T/$first_pkg"
if out="$(scan "$T")"; then die "a packaging file that installs k5-cli passed"; fi
grep -q "^NAMES k5-cli: $first_pkg:" <<<"$out" || die "the failure doesn't name $first_pkg: $out"
pass=$((pass + 1))
cp "$ROOT/$first_pkg" "$T/$first_pkg"

printf '{"bundle": {"externalBin": ["binaries/k5-cli"]}}\n' >"$T/src-tauri/tauri.conf.json"
if out="$(scan "$T")"; then die "a Tauri config that bundles k5-cli passed"; fi
grep -q "^NAMES k5-cli: src-tauri/tauri.conf.json:1:" <<<"$out" || die "the failure doesn't name tauri.conf.json: $out"
pass=$((pass + 1))
cp "$ROOT/src-tauri/tauri.conf.json" "$T/src-tauri/tauri.conf.json"

rm "$T/crates/k2-core/src/cli_stage.rs"
if out="$(scan "$T")"; then die "a missing cli_stage.rs passed"; fi
grep -q "^MISSING: crates/k2-core/src/cli_stage.rs " <<<"$out" || die "the failure doesn't name the missing file: $out"
pass=$((pass + 1))

rm "$T"/scripts/sign-*.sh
if out="$(scan "$T")"; then die "no scripts/sign-*.sh passed"; fi
grep -q "^MISSING: scripts/sign-\*.sh " <<<"$out" || die "the failure doesn't name the empty glob: $out"
pass=$((pass + 1))

# ── The repo ───────────────────────────────────────────────────────────
if ! out="$(scan "$ROOT")"; then
    echo "$out" >&2
    die "k5-cli is not shipped yet: the files above must not name it"
fi
pass=$((pass + 1))

echo "k5_cli_not_shipped: $pass passed, 0 failed ($copied files scanned)"
