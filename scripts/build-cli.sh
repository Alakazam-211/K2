#!/usr/bin/env bash
# cli/k2 is built from parts: cli/src/<part>.sh, joined in the order
# cli/src/ORDER lists them (one file name per line). The join is plain
# concatenation; nothing is added between parts.
#
#   scripts/build-cli.sh           write cli/k2 from the parts
#   scripts/build-cli.sh --check   change nothing; exit 1 if cli/k2 is not the join
#
# Edit a part, never cli/k2: the next build would undo an edit made there.
# cli/k2 stays committed, because the app bundle, the installers, the
# daemon (include_str!) and the tests all read that one file.
#
# For tests: --root DIR works on another tree.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECK=0
while [ $# -gt 0 ]; do
    case "$1" in
        --check) CHECK=1 ;;
        --root) ROOT="$2"; shift ;;
        *) echo "build-cli: unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done
SRC="$ROOT/cli/src"
OUT="$ROOT/cli/k2"
[ -f "$SRC/ORDER" ] || { echo "build-cli: no $SRC/ORDER" >&2; exit 2; }

parts=()
while IFS= read -r name || [ -n "$name" ]; do
    case "$name" in ''|'#'*) continue ;; esac
    case "$name" in */*|.*) echo "build-cli: ORDER names a part outside cli/src: $name" >&2; exit 2 ;; esac
    [ -f "$SRC/$name" ] || { echo "build-cli: ORDER lists $name, but cli/src/$name does not exist" >&2; exit 2; }
    for seen in ${parts[@]+"${parts[@]}"}; do
        [ "$seen" != "$name" ] || { echo "build-cli: ORDER lists $name twice" >&2; exit 2; }
    done
    parts+=("$name")
done < "$SRC/ORDER"
[ "${#parts[@]}" -gt 0 ] || { echo "build-cli: ORDER lists no parts" >&2; exit 2; }
# A part nobody listed would silently drop out of the CLI.
for f in "$SRC"/*.sh; do
    [ -e "$f" ] || continue
    base="$(basename "$f")"
    found=0
    for name in "${parts[@]}"; do [ "$name" != "$base" ] || found=1; done
    [ "$found" = 1 ] || { echo "build-cli: cli/src/$base is not in ORDER: add it where it belongs, or remove the file" >&2; exit 2; }
done

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
for name in "${parts[@]}"; do cat "$SRC/$name"; done > "$tmp"

if [ "$CHECK" = 1 ]; then
    if [ -f "$OUT" ] && cmp -s "$tmp" "$OUT"; then
        echo "build-cli: cli/k2 is current (${#parts[@]} parts)"
        exit 0
    fi
    echo "build-cli: cli/k2 is not the join of cli/src. Edit the part in cli/src (never cli/k2), then run scripts/build-cli.sh." >&2
    exit 1
fi
if [ -f "$OUT" ] && cmp -s "$tmp" "$OUT"; then
    echo "build-cli: cli/k2 is already current (${#parts[@]} parts)"
    exit 0
fi
# Written in place, so the file keeps its mode (it is executable).
[ -f "$OUT" ] || { : > "$OUT"; chmod +x "$OUT"; }
cat "$tmp" > "$OUT"
echo "build-cli: wrote cli/k2 from ${#parts[@]} parts"
