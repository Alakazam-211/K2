#!/usr/bin/env bash
# cli/k2 is built from parts: cli/src/<part>.sh, joined in the order
# cli/src/ORDER lists them (one file name per line). The join is plain
# concatenation; nothing is added between parts.
#
#   scripts/build-cli.sh           write cli/k2 from the parts
#   scripts/build-cli.sh --check   change nothing; exit 1 if cli/k2 is not the join
#
# One folder is allowed beside the parts: cli/src/schema/<noun>.entries,
# the command entries `k2 --schema` prints, a run per noun. ORDER lists
# them as schema/<noun>.entries between schema-head.sh and schema-tail.sh.
# Each one is checked on its own before any join (see check_entries).
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
    case "$name" in
        schema/*.entries)
            case "${name#schema/}" in */*|.*) echo "build-cli: ORDER names a part outside cli/src: $name" >&2; exit 2 ;; esac ;;
        */*|.*) echo "build-cli: ORDER names a part outside cli/src: $name" >&2; exit 2 ;;
    esac
    [ -f "$SRC/$name" ] || { echo "build-cli: ORDER lists $name, but cli/src/$name does not exist" >&2; exit 2; }
    for seen in ${parts[@]+"${parts[@]}"}; do
        [ "$seen" != "$name" ] || { echo "build-cli: ORDER lists $name twice" >&2; exit 2; }
    done
    parts+=("$name")
done < "$SRC/ORDER"
[ "${#parts[@]}" -gt 0 ] || { echo "build-cli: ORDER lists no parts" >&2; exit 2; }
# A part nobody listed would silently drop out of the CLI.
for f in "$SRC"/*.sh "$SRC"/schema/*.entries; do
    [ -e "$f" ] || continue
    base="${f#"$SRC"/}"
    found=0
    for name in "${parts[@]}"; do [ "$name" != "$base" ] || found=1; done
    [ "$found" = 1 ] || { echo "build-cli: cli/src/$base is not in ORDER: add it where it belongs, or remove the file" >&2; exit 2; }
done

# The schema entries: each file is a run of JSON objects, every one closed
# by `},`, so adding a command never touches another entry's comma.
check_entries() {
    command -v python3 >/dev/null || { echo "build-cli: python3 is required to check cli/src/schema" >&2; return 2; }
    python3 - "$SRC" "$@" <<'PY'
import json, os, re, sys
src, seen, bad = sys.argv[1], {}, []
# The last entries stay in the shell part that closes the manifest.
tail = src + "/schema-tail.sh"
if os.path.isfile(tail):
    with open(tail, encoding="utf-8") as f:
        for name in re.findall(r'^ {4}\{\n {6}"name": "([^"]*)",$', f.read(), re.M):
            seen[name] = "schema-tail.sh"
for rel in sys.argv[2:]:
    with open(src + "/" + rel, encoding="utf-8", newline="") as f:
        text = f.read()
    def no(why):
        bad.append("build-cli: cli/src/%s: %s" % (rel, why))
    if any(line == "EOF" for line in text.split("\n")):
        no("a line is the heredoc word EOF alone; it would end the manifest early")
        continue
    if not text.endswith("},\n"):
        no("the last entry must end with `},` and a new line")
        continue
    try:
        entries = json.loads("[" + text[:-2] + "]")
    except ValueError as e:
        no("not a run of JSON entries: %s (the line is counted in this file)" % e)
        continue
    for n, e in enumerate(entries, 1):
        if not isinstance(e, dict):
            no("entry %d is not an object" % n)
            continue
        name = e.get("name")
        what = "entry %d (%s)" % (n, name if isinstance(name, str) else "no name")
        for key in ("name", "usage", "description"):
            if not isinstance(e.get(key), str):
                no("%s: \"%s\" must be a string" % (what, key))
        for key in ("args", "flags"):
            if key in e and not isinstance(e[key], list):
                no("%s: \"%s\" must be a list" % (what, key))
        if isinstance(name, str):
            if name in seen:
                no("%s is already in cli/src/%s" % (what, seen[name]))
            seen[name] = rel
for line in bad:
    print(line, file=sys.stderr)
sys.exit(1 if bad else 0)
PY
}
entries=()
for name in "${parts[@]}"; do
    case "$name" in schema/*) entries+=("$name") ;; esac
done
if [ "${#entries[@]}" -gt 0 ]; then
    check_entries "${entries[@]}" || exit 1
fi

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
