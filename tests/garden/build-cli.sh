#!/usr/bin/env bash
# Tests for scripts/build-cli.sh on a throwaway tree.
# Run: bash tests/garden/build-cli.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
B="$HERE/scripts/build-cli.sh"
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
S="$T/cli/src"
mkdir -p "$S"
printf '#!/bin/bash\nhead\n' > "$S/prelude.sh"
printf 'mail one\nmail two\n' > "$S/mail.sh"
printf 'no newline at the end' > "$S/router.sh"
printf '# the order the parts are joined in\nprelude.sh\nmail.sh\n\nrouter.sh\n' > "$S/ORDER"
WANT="$(printf '#!/bin/bash\nhead\nmail one\nmail two\nno newline at the end')"

# Nothing built yet: --check says so and writes nothing.
run 1 "cli/k2 is not the join of cli/src" --check
[ ! -e "$T/cli/k2" ] || { echo "FAIL: --check wrote cli/k2" >&2; exit 1; }
# The join is the parts in ORDER, byte for byte, and the file is executable.
run 0 "wrote cli/k2 from 3 parts"
[ "$(cat "$T/cli/k2")" = "$WANT" ] && [ "$(wc -c < "$T/cli/k2" | tr -d ' ')" = "56" ] || { echo "FAIL: the join: $(cat "$T/cli/k2")" >&2; exit 1; }
[ -x "$T/cli/k2" ] || { echo "FAIL: cli/k2 is not executable" >&2; exit 1; }
pass=$((pass + 2))
run 0 "cli/k2 is current (3 parts)" --check
run 0 "already current"
# An edit made in cli/k2 is caught, and a build puts the parts back.
echo "edited here" >> "$T/cli/k2"
run 1 "Edit the part in cli/src (never cli/k2)" --check
run 0 "wrote cli/k2"
run 0 "is current" --check
# An edit to a part shows as stale until the build runs; the mode is kept.
printf 'mail one\nmail 2\n' > "$S/mail.sh"
run 1 "not the join" --check
run 0 "wrote cli/k2"
[ -x "$T/cli/k2" ] && grep -q "mail 2" "$T/cli/k2" || { echo "FAIL: rebuild" >&2; exit 1; }
pass=$((pass + 1))
# A part that ORDER does not list, a listed part that is missing, a part
# listed twice, a path outside cli/src: each is an error, nothing written.
printf 'x\n' > "$S/stray.sh"
run 2 "cli/src/stray.sh is not in ORDER"
rm "$S/stray.sh"
printf 'prelude.sh\nmail.sh\ngone.sh\nrouter.sh\n' > "$S/ORDER"
run 2 "ORDER lists gone.sh, but cli/src/gone.sh does not exist" --check
printf 'prelude.sh\nmail.sh\nmail.sh\nrouter.sh\n' > "$S/ORDER"
run 2 "ORDER lists mail.sh twice"
printf 'prelude.sh\n../k2\n' > "$S/ORDER"
run 2 "ORDER names a part outside cli/src: ../k2"
: > "$S/ORDER"
run 2 "ORDER lists no parts"
rm "$S/ORDER"
run 2 "no $S/ORDER"
run 2 "unknown argument: --chekc" --chekc
grep -q "mail 2" "$T/cli/k2" || { echo "FAIL: an error changed cli/k2" >&2; exit 1; }
pass=$((pass + 1))

# The schema folder: runs of `k2 --schema` entries, each file checked on
# its own, joined like any part. No other folder is allowed.
rm -rf "$T/cli"; mkdir -p "$S/schema"
printf 'head\n' > "$S/schema-head.sh"
printf '    {\n      "name": "last",\n      "usage": "k2 last",\n      "description": "d"\n    }\n  ]\n' > "$S/schema-tail.sh"
mail='    {"name": "mail", "usage": "k2 mail", "description": "d", "args": [], "flags": []},'
db='    {\n      "name": "db",\n      "usage": "k2 db",\n      "description": "a \\\\ b"\n    },'
printf '%s\n' "$mail" > "$S/schema/mail.entries"
printf "$db\n" > "$S/schema/db.entries"
printf 'schema-head.sh\nschema/mail.entries\nschema/db.entries\nschema-tail.sh\n' > "$S/ORDER"
run 0 "wrote cli/k2 from 4 parts"
run 0 "cli/k2 is current (4 parts)" --check
grep -q '"name": "db",' "$T/cli/k2" || { echo "FAIL: the entries are not in the join" >&2; exit 1; }
pass=$((pass + 1))
breaks() {
    local text="$1" file="$2" body="$3"
    cp "$S/schema/$file" "$T/keep"
    printf "$body" > "$S/schema/$file"
    run 1 "cli/src/schema/$file: $text" --check
    run 1 "cli/src/schema/$file: $text"
    cp "$T/keep" "$S/schema/$file"
}
breaks 'the last entry must end with `},` and a new line' db.entries '    {"name": "db", "usage": "u", "description": "d"}\n'
breaks 'not a run of JSON entries' db.entries '    {"name": "db", "usage": "u" "description": "d"},\n'
breaks 'not a run of JSON entries' db.entries '    {"name": "db", "usage": "a \\q b", "description": "d"},\n'
breaks 'entry 1 (mail) is already in cli/src/schema/mail.entries' db.entries '    {"name": "mail", "usage": "u", "description": "d"},\n'
breaks 'entry 1 (last) is already in cli/src/schema-tail.sh' db.entries '    {"name": "last", "usage": "u", "description": "d"},\n'
breaks 'entry 2 (db): "usage" must be a string' db.entries '    {"name": "x", "usage": "u", "description": "d"},\n    {"name": "db", "description": "d"},\n'
breaks 'entry 1 (db): "flags" must be a list' db.entries '    {"name": "db", "usage": "u", "description": "d", "flags": {}},\n'
breaks 'entry 1 (no name): "name" must be a string' db.entries '    {"usage": "u", "description": "d"},\n'
breaks 'a line is the heredoc word EOF alone' db.entries '    {"name": "db", "usage": "u", "description": "d"},\nEOF\n'
run 0 "is current" --check
# A file in the folder that ORDER does not list, and any other folder.
printf '%s\n' "$mail" > "$S/schema/stray.entries"
run 2 "cli/src/schema/stray.entries is not in ORDER"
rm "$S/schema/stray.entries"
printf 'schema-head.sh\nother/mail.entries\nschema-tail.sh\n' > "$S/ORDER"
run 2 "ORDER names a part outside cli/src: other/mail.entries"
printf 'schema-head.sh\nschema/../schema-tail.sh\nschema/deep/x.entries\n' > "$S/ORDER"
run 2 "ORDER names a part outside cli/src"
printf 'schema-head.sh\nschema/mail.sh\nschema-tail.sh\n' > "$S/ORDER"
run 2 "ORDER names a part outside cli/src: schema/mail.sh"

# cli/text: a page's text file is inlined at its `%include` line, byte for
# byte (tabs, backslashes and `$` stay as written), inside the quoted
# heredoc that prints it.
rm -rf "$T/cli"; mkdir -p "$S" "$T/cli/text/study"
X="$T/cli/text/study"
page() {
    printf '%s\n' '_study_page_a() {' "    cat <<'$1'$2" '%include cli/text/study/a.txt' "$1" '}' > "$S/a.sh"
}
page EOF ''
printf 'after\n' > "$S/z.sh"
printf 'a.sh\nz.sh\n' > "$S/ORDER"
printf 'line $HOME \\n `x`\n\tTabbed\n' > "$X/a.txt"
run 0 "wrote cli/k2 from 2 parts"
want="$(printf '%s\n' '_study_page_a() {' "    cat <<'EOF'" 'line $HOME \n `x`' "$(printf '\tTabbed')" 'EOF' '}' 'after')"
[ "$(cat "$T/cli/k2")" = "$want" ] || { echo "FAIL: the inlined page: $(cat "$T/cli/k2")" >&2; exit 1; }
[ "$(bash -c "$(cat "$T/cli/k2" | sed '$d'); _study_page_a")" = "$(cat "$X/a.txt")" ] || { echo "FAIL: the page does not print its text file" >&2; exit 1; }
pass=$((pass + 2))
run 0 "is current" --check
# A changed text file shows as stale until the build runs.
printf 'line two\n' >> "$X/a.txt"
run 1 "cli/k2 is not the join of cli/src (with cli/text)" --check
run 0 "wrote cli/k2"
grep -qx 'line two' "$T/cli/k2" || { echo "FAIL: the rebuilt page lacks the new line" >&2; exit 1; }
pass=$((pass + 1))
# A pipe after the heredoc is allowed (a filter like _perm_lines).
page EOF ' | _perm_lines'
run 0 "wrote cli/k2"
# Broken includes: exit 2, nothing written.
before="$(cat "$T/cli/k2")"
page EOF ''
printf '%s\n' '_study_page_b() {' '    cat <<EOF' '%include cli/text/study/a.txt' 'EOF' '}' > "$S/b.sh"
printf 'a.sh\nb.sh\nz.sh\n' > "$S/ORDER"
run 2 "cli/src/b.sh:3: cli/text/study/a.txt must be the only line of a quoted heredoc" --check
printf '%s\n' '_study_page_b() {' "    cat <<'EOF'" '%include cli/text/study/a.txt' 'EOF' '}' > "$S/b.sh"
run 2 "cli/src/b.sh:3: cli/text/study/a.txt is already included at cli/src/a.sh:3" --check
printf '%s\n' "    cat <<'EOF'" '%include cli/text/study/gone.txt' 'EOF' > "$S/b.sh"
run 2 "cli/src/b.sh:2: cli/text/study/gone.txt does not exist"
printf '%s\n' "    cat <<'EOF'" '%include cli/text/../k2.txt' 'EOF' > "$S/b.sh"
run 2 "an include is \`%include cli/text/<path>.txt\`"
rm "$S/b.sh"; printf 'a.sh\nz.sh\n' > "$S/ORDER"
printf '%s\n' "    cat <<'EOF'" '%include cli/text/study/a.txt' 'more' 'EOF' > "$S/a.sh"
run 2 "cli/src/a.sh:2: cli/text/study/a.txt must be the only line of a quoted heredoc"
page EOF ''
# A text file nobody includes would silently drop out of the CLI.
printf 'stray\n' > "$X/stray.txt"
run 2 "cli/text/study/stray.txt is not included by any part"
rm "$X/stray.txt"
# Text that can't sit in its heredoc: exit 1.
printf 'no new line at the end' > "$X/a.txt"
run 1 "cli/text/study/a.txt: must end with a new line"
printf 'one\nEOF\ntwo\n' > "$X/a.txt"
run 1 "cli/text/study/a.txt: a line is the heredoc word EOF alone"
[ "$(cat "$T/cli/k2")" = "$before" ] || { echo "FAIL: a refused build changed cli/k2" >&2; exit 1; }
pass=$((pass + 1))
page PAGE ''
run 0 "wrote cli/k2"
[ "$(cat "$T/cli/k2")" != "$before" ] && grep -qx 'EOF' "$T/cli/k2" || { echo "FAIL: another heredoc word lets EOF be a line" >&2; exit 1; }
pass=$((pass + 1))
echo "build-cli tests: $pass passed"
