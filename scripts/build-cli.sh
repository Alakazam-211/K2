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
# An entry may also say how the Rust CLI answers it: `impl` (bash, native
# or spec) and, for a spec verb, `parse`, `call`, `output`, `exits` and
# `auth`. contract-gen reads them into crates/k5-cli/src/spec/table.rs.
# They come last in the entry, each key on its own line at six spaces,
# and the join leaves them out: `k2 --schema` prints the entry without
# them (see strip_spec_keys).
#
# Text the CLI prints (the `k2 study` pages and the static help) lives in
# cli/text/, one file per page, so the Rust CLI can read the same files. The part keeps the
# page's quoted heredoc with one line in it, `%include cli/text/<path>.txt`,
# and the join puts the file's bytes in that line's place. Every file under
# cli/text is included exactly once (see join_parts).
#
# A page or an entry names the company, the sign-in service, the contact
# address and the source repo by slot, {company}, {login}, {contact_email}
# and {repo} (scripts/brand/page_slots.py). The join fills them with ours
# (brand/k2: bash is built in the default brand only), so cli/k2 never
# holds a slot; the k5 binary's build fills them with its own brand's
# (crates/k5-cli/build.rs).
#
# Edit a part, never cli/k2: the next build would undo an edit made there.
# cli/k2 stays committed, because the app bundle, the installers, the
# daemon (include_str!) and the tests all read that one file.
#
# For tests: --root DIR works on another tree.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# The slots' filler and our brand file are this script's own, for any --root.
SLOTS_PY="$ROOT/scripts/brand/page_slots.py"
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
BASE_KEYS = ["name", "usage", "description", "args", "flags"]
SPEC_KEYS = ["impl", "parse", "call", "output", "exits", "auth"]
# The words a spec entry's fields take, as far as the Rust runner answers
# them today. contract-gen checks every field (crates/k2-core/src/contract/
# cli_spec.rs, which reads this line); this catches a wrong word before cargo.
SPEC_LISTS = {"impl": ["bash", "native", "spec"], "auth": ["session"], "via": ["compute"], "method": ["GET"], "style": ["none", "for"], "print": ["body", "lines"]}
def spec_problems(e):
    out = []
    keys = list(e)
    for k in keys:
        if k not in BASE_KEYS and k not in SPEC_KEYS:
            out.append('"%s" is not an entry key' % k)
    spec_at = [i for i, k in enumerate(keys) if k in SPEC_KEYS]
    if spec_at and any(k in BASE_KEYS for k in keys[spec_at[0]:]):
        out.append("the keys %s come after %s" % ("/".join(SPEC_KEYS), "/".join(BASE_KEYS)))
    if not spec_at:
        return out
    impl = e.get("impl")
    if impl not in SPEC_LISTS["impl"]:
        out.append('"impl" must be one of %s' % ", ".join(SPEC_LISTS["impl"]))
        return out
    rest = [k for k in SPEC_KEYS[1:] if k in e]
    if impl != "spec":
        if rest:
            out.append('%s only go with "impl": "spec"' % ", ".join(rest))
        return out
    for k in ("parse", "call", "output", "auth"):
        if k not in e:
            out.append('a spec entry needs "%s"' % k)
    def word(where, value, lst):
        if value not in SPEC_LISTS[lst]:
            out.append('%s must be one of %s, not %s' % (where, ", ".join(SPEC_LISTS[lst]), json.dumps(value)))
    if "auth" in e:
        word('"auth"', e["auth"], "auth")
    p, c, o = e.get("parse"), e.get("call"), e.get("output")
    if "parse" in e:
        word('"parse" "style"', p.get("style") if isinstance(p, dict) else None, "style")
    if "call" in e:
        c = c if isinstance(c, dict) else {}
        word('"call" "via"', c.get("via"), "via")
        word('"call" "method"', c.get("method"), "method")
    if "output" in e:
        o = o if isinstance(o, dict) else {}
        word('"output" "text"', o.get("text"), "print")
        if "json" in o:
            word('"output" "json"', o["json"], "print")
    return out
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
        for why in spec_problems(e):
            no("%s: %s" % (what, why))
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

# The join: the parts in ORDER, byte for byte, with each `%include` line
# replaced by the text file it names. The line must be the only line of a
# quoted heredoc (`cat <<'WORD'`, maybe after a case pattern on the same
# line, maybe sent to stderr with `>&2` before or after the word, or piped
# on, then the line, then WORD), so the file prints exactly as written:
# nothing in it expands.
# A schema part goes in without its entries' spec keys (strip_spec_keys).
# Both have their slots filled with our names (page_slots.py).
# Exit 2 for a broken include or a file nobody includes, 1 for text that
# can't sit in its heredoc or spec keys the join can't leave out. Writes
# the join to $1.
join_parts() {
    command -v python3 >/dev/null || { echo "build-cli: python3 is required to join cli/src" >&2; return 2; }
    python3 - "$ROOT" "$1" "$SLOTS_PY" "${parts[@]}" <<'PY'
import importlib.util, json, os, re, sys
root, out, slots_py, names = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location("page_slots", slots_py)
slots = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(slots)
ours = slots.names()
def filled(data):
    return slots.fill(data.decode("utf-8"), ours).encode("utf-8")
INCLUDE = re.compile(r"%include (cli/text/[a-z0-9_-]+(?:/[a-z0-9_-]+)*\.txt)")
OPEN = re.compile(r"\s*(?:[^\s()][^()]*\) )?cat(?: >&2)? <<'([A-Za-z_][A-Za-z_0-9]*)'(?: >&2| \| .*)?")
SPEC_KEYS = ("impl", "parse", "call", "output", "exits", "auth")
SPEC_LINE = re.compile(r'      "(?:%s)": ' % "|".join(SPEC_KEYS))
used, pieces, broken, bad_text = {}, [], [], []
# strip_spec_keys: an entry's lines from its first spec key up to the
# entry's closing `    },`, and the comma before them, go. The rest must
# parse as the same entries without those keys, or the file is refused.
def strip_spec_keys(text):
    lines, kept, i = text.split("\n"), [], 0
    while i < len(lines):
        if not SPEC_LINE.match(lines[i]):
            kept.append(lines[i])
            i += 1
            continue
        j = i
        while j < len(lines) and lines[j] != "    },":
            j += 1
        if j == len(lines) or not kept or not kept[-1].endswith(","):
            return None
        kept[-1] = kept[-1][:-1]
        i = j
    stripped = "\n".join(kept)
    try:
        want = [{k: v for k, v in e.items() if k not in SPEC_KEYS} for e in json.loads("[" + text[:-2] + "]")]
        got = json.loads("[" + stripped[:-2] + "]")
    except ValueError:
        return None
    return stripped if got == want else None
for name in names:
    with open(root + "/cli/src/" + name, "rb") as f:
        raw = f.read()
    if name.startswith("schema/"):
        text = raw.decode("utf-8")
        try:
            entries = json.loads("[" + text[:-2] + "]")
        except ValueError:
            entries = []
        if any(isinstance(e, dict) and any(k in e for k in SPEC_KEYS) for e in entries):
            stripped = strip_spec_keys(text)
            if stripped is None:
                bad_text.append("cli/src/%s: an entry's spec keys must come last, each on its own line at six spaces, the entry closed by `    },`" % name)
                continue
            raw = stripped.encode("utf-8")
        raw = filled(raw)
    lines = raw.split(b"\n")
    for i, raw in enumerate(lines):
        if not raw.startswith(b"%include"):
            continue
        line = raw.decode("utf-8", "replace")
        where = "cli/src/%s:%d" % (name, i + 1)
        m = INCLUDE.fullmatch(line)
        if not m:
            broken.append("%s: an include is `%%include cli/text/<path>.txt` (a-z, 0-9, _ and -), not %r" % (where, line))
            continue
        rel = m.group(1)
        opener = OPEN.fullmatch(lines[i - 1].decode("utf-8", "replace")) if i > 0 else None
        if not opener or i + 1 >= len(lines) or lines[i + 1] != opener.group(1).encode():
            broken.append("%s: %s must be the only line of a quoted heredoc (cat <<'WORD' above it, WORD below)" % (where, rel))
            continue
        if rel in used:
            broken.append("%s: %s is already included at %s; a text file is included once" % (where, rel, used[rel]))
            continue
        used[rel] = where
        if not os.path.isfile(root + "/" + rel):
            broken.append("%s: %s does not exist" % (where, rel))
            continue
        with open(root + "/" + rel, "rb") as f:
            body = f.read()
        word = opener.group(1)
        try:
            body.decode("utf-8")
        except UnicodeDecodeError as e:
            bad_text.append("%s: not UTF-8 (%s)" % (rel, e))
            continue
        if not body.endswith(b"\n"):
            bad_text.append("%s: must end with a new line (the heredoc prints one after the last line)" % rel)
            continue
        if word.encode() in body[:-1].split(b"\n"):
            bad_text.append("%s: a line is the heredoc word %s alone; it would end the page early" % (rel, word))
            continue
        lines[i] = filled(body[:-1])
    pieces.append(b"\n".join(lines))
for top, dirs, files in os.walk(root + "/cli/text"):
    dirs.sort()
    for f in sorted(files):
        rel = os.path.relpath(os.path.join(top, f), root).replace(os.sep, "/")
        if rel not in used:
            broken.append("%s is not included by any part: add its `%%include` line to the heredoc that prints it, or remove the file" % rel)
for line in broken + bad_text:
    print("build-cli: " + line, file=sys.stderr)
if broken or bad_text:
    sys.exit(2 if broken else 1)
joined = b"".join(pieces)
left = slots.raw_slots(joined.decode("utf-8", "replace"))
if left:
    print("build-cli: the join still holds %s: a slot is filled only in a cli/text page or a cli/src/schema entry" % " and ".join(left), file=sys.stderr)
    sys.exit(1)
with open(out, "wb") as f:
    f.write(joined)
PY
}

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
join_parts "$tmp" || exit $?

if [ "$CHECK" = 1 ]; then
    if [ -f "$OUT" ] && cmp -s "$tmp" "$OUT"; then
        echo "build-cli: cli/k2 is current (${#parts[@]} parts)"
        exit 0
    fi
    echo "build-cli: cli/k2 is not the join of cli/src (with cli/text). Edit the part in cli/src (never cli/k2), or the page in cli/text, then run scripts/build-cli.sh." >&2
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
