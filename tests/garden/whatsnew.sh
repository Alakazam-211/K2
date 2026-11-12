#!/usr/bin/env bash
# Tests for scripts/whatsnew.sh on a throwaway tree, plus a round trip of
# three released sections of the real WHATS_NEW.md.
# Run: bash tests/garden/whatsnew.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
WN="$HERE/scripts/whatsnew.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

# run <want exit> <want text> <args...>: both must match; output in $OUT.
run() {
    local want="$1" text="$2" code; shift 2
    set +e; OUT="$(bash "$WN" "$@" --root "$T" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL ($*): exit $code, wanted $want: $OUT" >&2; exit 1; fi
    case "$OUT" in *"$text"*) ;; *) echo "FAIL ($*): output lacks '$text': $OUT" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
same() {
    if [ "$1" != "$2" ]; then echo "FAIL: $3" >&2; printf '%s\n--- wanted ---\n%s\n' "$1" "$2" >&2; exit 1; fi
    pass=$((pass + 1))
}

printf '# K2 — What'"'"'s New\n\nIntro.\n\n## 1.0.0 — The first one\n\n### Fixes\n- An old fix.\n\n' > "$T/WHATS_NEW.md"
D="$T/whatsnew.d/1.1.0"
mkdir -p "$D"
printf 'Fragments\n' > "$D/title"
printf 'Apps\nFixes\n' > "$D/sections"
printf '### Fixes\n- **B fix.** Second by name.\n' > "$D/20-b.md"
printf '### Fixes\n- **A fix.** First by name.\n  A second line.\n' > "$D/10-a.md"
printf '### Apps\n- An app thing.\n' > "$D/zz-apps.md"

WANT='## 1.1.0 — Fragments

### Apps
- An app thing.

### Fixes
- **A fix.** First by name.
  A second line.
- **B fix.** Second by name.
'
# The join: sections in the sections order, fragments by file name.
run 0 "## 1.1.0 — Fragments" build 1.1.0 --stdout
same "$OUT" "${WANT%$'\n'}" "the join"
same "$(cat "$T/WHATS_NEW.md")" "$(printf '# K2 — What'"'"'s New\n\nIntro.\n\n## 1.0.0 — The first one\n\n### Fixes\n- An old fix.')" "--stdout wrote nothing"
run 0 "whatsnew: ok (1.1.0)" check
# Not built yet: --built says so.
run 1 "WHATS_NEW.md has no '## 1.1.0' section: run scripts/whatsnew.sh build 1.1.0" check 1.1.0 --built

# build puts it above the newest section and leaves the rest alone.
run 0 "wrote the 1.1.0 section into WHATS_NEW.md" build 1.1.0
same "$(cat "$T/WHATS_NEW.md")" "$(printf '# K2 — What'"'"'s New\n\nIntro.\n\n%s\n## 1.0.0 — The first one\n\n### Fixes\n- An old fix.' "$WANT")" "WHATS_NEW.md after build"
run 0 "whatsnew: ok (1.1.0)" check 1.1.0 --built
run 0 "already holds 1.1.0" build 1.1.0
# An optional intro prints before the first heading; a rebuild replaces the section in place.
printf 'A word first.\n' > "$D/intro.md"
run 1 "is not the join of whatsnew.d/1.1.0" check 1.1.0 --built
run 0 "wrote the 1.1.0 section" build 1.1.0
same "$(sed -n '5,8p' "$T/WHATS_NEW.md")" "$(printf '## 1.1.0 — Fragments\n\nA word first.\n')" "the intro"
same "$(grep -c '^## ' "$T/WHATS_NEW.md")" "2" "still two sections"
rm "$D/intro.md"

# What is wrong, each by name.
mv "$D/title" "$T/title.keep"
run 1 "whatsnew.d/1.1.0/title is missing: one line, the words after '## 1.1.0 — '" check 1.1.0
run 1 "title is missing" build 1.1.0 --stdout
printf '\n' > "$D/title"
run 1 "whatsnew.d/1.1.0/title is empty" check
printf 'one\ntwo\n' > "$D/title"
run 1 "title has more than one line" check
mv "$T/title.keep" "$D/title"
printf '### Nope\n- x\n' > "$D/bad.md"
run 1 "bad.md: line 1 must be '### <Section>' with a heading listed in sections; it is '### Nope'" check
printf -- '- no heading\n' > "$D/bad.md"
run 1 "bad.md: line 1 must be '### <Section>'" check
printf '### Fixes\n\n' > "$D/bad.md"
run 1 "bad.md is empty under its heading" check
printf '### Fixes\n- x\n### Apps\n- y\n' > "$D/bad.md"
run 1 "bad.md holds a second '##' or '###' heading" check
rm "$D/bad.md"
printf 'Apps\nFixes\nNobody\n' > "$D/sections"
run 1 "sections lists 'Nobody' but no fragment is under it" check
printf 'Apps\nFixes\n' > "$D/sections"
printf 'x\n' > "$D/notes.txt"
run 1 "notes.txt: only title, sections, intro.md and <name>.md fragments belong here" check
rm "$D/notes.txt"
run 0 "whatsnew: ok (1.1.0)" check
run 1 "whatsnew.d/2.0.0 does not exist" build 2.0.0
run 1 "not a version: next" build next
run 1 "usage:" frobnicate

# split: an existing section becomes fragments whose join is that section.
run 0 "split 1.0.0 into 1 fragment(s)" split 1.0.0
same "$(cat "$T/whatsnew.d/1.0.0/title"; cat "$T/whatsnew.d/1.0.0/sections"; cat "$T/whatsnew.d/1.0.0/010-fixes.md")" "$(printf 'The first one\nFixes\n### Fixes\n- An old fix.')" "the split files"
run 0 "whatsnew: ok (1.0.0)" check 1.0.0 --built
run 1 "whatsnew.d/1.0.0 already exists" split 1.0.0
# A section not laid out like a join is refused and nothing is left behind.
printf '## 0.9.0 — Tight\n\n### A\n- a\n### B\n- b\n\n' >> "$T/WHATS_NEW.md"
run 1 "the 0.9.0 section is not laid out the way a join prints" split 0.9.0
[ ! -e "$T/whatsnew.d/0.9.0" ] || { echo "FAIL: a refused split left its folder" >&2; exit 1; }
pass=$((pass + 1))

# The real file: three released sections (one with an intro and '####'
# headings) split and join back byte for byte, and a build changes nothing.
rm -rf "$T/whatsnew.d"
cp "$HERE/WHATS_NEW.md" "$T/WHATS_NEW.md"
for v in 0.46.10 0.46.8 0.46.7; do
    run 0 "their join equals the section" split "$v"
    run 0 "already holds $v" build "$v"
done
cmp -s "$HERE/WHATS_NEW.md" "$T/WHATS_NEW.md" || { echo "FAIL: build changed the real WHATS_NEW.md" >&2; exit 1; }
pass=$((pass + 1))

echo "whatsnew tests: $pass passed"
