#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP="${1:-}"
EXPECTED_ARCH="${2:-$("$SCRIPT_DIR/macos-native-arch.sh")}"
PROVENANCE="${3:-}"
EXPECTED_COMMIT="${4:-}"

if [ -z "$APP" ]; then
    echo "Usage: verify-macos-bundle.sh <K2.app> [x86_64|aarch64] [provenance.json] [commit]" >&2
    exit 2
fi
if [ ! -d "$APP" ]; then
    echo "verify-macos-bundle: app not found: $APP" >&2
    exit 1
fi
case "$EXPECTED_ARCH" in
    x86_64|aarch64) ;;
    *) echo "verify-macos-bundle: unsupported expected architecture: $EXPECTED_ARCH" >&2; exit 1 ;;
esac

MACOS_DIR="$APP/Contents/MacOS"
for name in k2 k2-daemon frpc; do
    path="$MACOS_DIR/$name"
    if [ ! -x "$path" ]; then
        echo "verify-macos-bundle: missing executable: $path" >&2
        exit 1
    fi
    archs="$(lipo -archs "$path")"
    case " $archs " in
        *" $EXPECTED_ARCH "*) ;;
        *)
            echo "verify-macos-bundle: $name has '$archs', expected '$EXPECTED_ARCH'" >&2
            exit 1
            ;;
    esac
    printf '%s\tarch=%s\tsha256=%s\n' \
        "$name" "$archs" "$(shasum -a 256 "$path" | awk '{print $1}')"
done

plist="$APP/Contents/Info.plist"
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")"
reported="$("$MACOS_DIR/k2" --version)"
if [ "$reported" != "K2 $version" ]; then
    echo "verify-macos-bundle: bundle version '$version' does not match '$reported'" >&2
    exit 1
fi
daemon_probe=--k2-artifact-version-v1
daemon_marker="$APP/Contents/Resources/k2-daemon.artifact-probe-v1.sha256"
if [ ! -f "$daemon_marker" ]; then
    echo "verify-macos-bundle: daemon lacks the non-starting probe marker" >&2
    exit 1
fi
marker_hash="$(tr -d '[:space:]' < "$daemon_marker")"
daemon_hash="$(shasum -a 256 "$MACOS_DIR/k2-daemon" | awk '{print $1}')"
if [ "$marker_hash" != "$daemon_hash" ]; then
    echo "verify-macos-bundle: daemon probe marker does not match the binary" >&2
    exit 1
fi
daemon_reported="$("$MACOS_DIR/k2-daemon" "$daemon_probe")"
if [ "$daemon_reported" != "k2-daemon $version (tokio)" ]; then
    echo "verify-macos-bundle: bundle version '$version' does not match '$daemon_reported'" >&2
    exit 1
fi
printf 'bundle_version=%s\n' "$version"

if [ -n "$PROVENANCE" ]; then
    if [ ! -f "$PROVENANCE" ]; then
        echo "verify-macos-bundle: provenance not found: $PROVENANCE" >&2
        exit 1
    fi
    command -v python3 >/dev/null 2>&1 \
        || { echo "verify-macos-bundle: python3 is required for provenance JSON" >&2; exit 1; }
    provenance_fields="$(python3 - "$PROVENANCE" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    p = json.load(f)

values = [
    p["version"],
    p["architecture"],
    p["source_commit"],
    str(p["source_dirty_before_build"]).lower(),
    str(p["signed"]).lower(),
    str(p["notarized"]).lower(),
    p["artifacts"]["k2"]["sha256"],
    p["artifacts"]["k2-daemon"]["sha256"],
    p["artifacts"]["frpc"]["sha256"],
    p["artifacts"]["dmg"]["name"],
    p["artifacts"]["dmg"]["sha256"],
]
print("|".join(values))
PY
)" || { echo "verify-macos-bundle: invalid provenance JSON" >&2; exit 1; }
    IFS='|' read -r provenance_version provenance_arch provenance_commit \
        provenance_dirty provenance_signed provenance_notarized \
        provenance_k2_hash provenance_daemon_hash provenance_frpc_hash \
        dmg_name provenance_dmg_hash <<< "$provenance_fields"
    [ "$provenance_version" = "$version" ] \
        || { echo "verify-macos-bundle: provenance version mismatch" >&2; exit 1; }
    [ "$provenance_arch" = "$EXPECTED_ARCH" ] \
        || { echo "verify-macos-bundle: provenance architecture mismatch" >&2; exit 1; }
    if [ -n "$EXPECTED_COMMIT" ] && [ "$provenance_commit" != "$EXPECTED_COMMIT" ]; then
        echo "verify-macos-bundle: provenance source commit mismatch" >&2
        exit 1
    fi
    if [ "$provenance_dirty" != false ] && [ "${K2_ALLOW_DIRTY_PROVENANCE:-0}" != 1 ]; then
        echo "verify-macos-bundle: dirty-source provenance is development-only" >&2
        exit 1
    fi
    [ "$provenance_signed" = true ] \
        || { echo "verify-macos-bundle: provenance does not attest signing" >&2; exit 1; }
    [ "$provenance_notarized" = true ] \
        || { echo "verify-macos-bundle: provenance does not attest notarization" >&2; exit 1; }
    for name in k2 k2-daemon frpc; do
        case "$name" in
            k2)        expected_hash="$provenance_k2_hash" ;;
            k2-daemon) expected_hash="$provenance_daemon_hash" ;;
            frpc)      expected_hash="$provenance_frpc_hash" ;;
        esac
        actual_hash="$(shasum -a 256 "$MACOS_DIR/$name" | awk '{print $1}')"
        [ "$actual_hash" = "$expected_hash" ] \
            || { echo "verify-macos-bundle: provenance hash mismatch for $name" >&2; exit 1; }
    done
    dmg_path="$(dirname "$PROVENANCE")/$dmg_name"
    if [ -f "$dmg_path" ]; then
        actual_hash="$(shasum -a 256 "$dmg_path" | awk '{print $1}')"
        [ "$actual_hash" = "$provenance_dmg_hash" ] \
            || { echo "verify-macos-bundle: provenance hash mismatch for $dmg_name" >&2; exit 1; }
    fi
    codesign --verify --deep --strict --verbose=2 "$APP"
    xcrun stapler validate "$APP"
    printf 'provenance=verified\n'
fi
