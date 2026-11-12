#!/usr/bin/env bash
# release.sh Step 1.5 with What's New fragments: the step's own lines, run
# in a throwaway tree with the real scripts/whatsnew.sh.
# Run: bash tests/garden/release-whatsnew-hook.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

# The step: from its echo to the line before the SKIP_BUILD fi.
awk '/^echo "Step 1.5: Verifying WHATS_NEW.md/ {on=1} /^fi # SKIP_BUILD/ {on=0} on' "$HERE/scripts/release.sh" > "$T/step.sh"
grep -q 'scripts/whatsnew.sh build' "$T/step.sh" || { echo "FAIL: Step 1.5 was not found in release.sh, or has no whatsnew hook" >&2; exit 1; }

fresh() {
    rm -rf "$T/root"
    mkdir -p "$T/root/scripts"
    cp "$HERE/scripts/whatsnew.sh" "$T/root/scripts/"
    printf '# What'"'"'s New\n\n## 1.0.0 — The first one\n\n### Fixes\n- An old fix.\n\n' > "$T/root/WHATS_NEW.md"
}
# run <version> <want exit> <want text>
run() {
    local version="$1" want="$2" text="$3" code
    set +e; OUT="$(cd "$T/root" && VERSION="$version" NOTES_FILE="" bash -c 'set -euo pipefail; source "$1"' _ "$T/step.sh" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL ($version): exit $code, wanted $want: $OUT" >&2; exit 1; fi
    case "$OUT" in *"$text"*) ;; *) echo "FAIL ($version): output lacks '$text': $OUT" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
same() { if [ "$1" != "$2" ]; then echo "FAIL: $3" >&2; printf '%s\n--- wanted ---\n%s\n' "$1" "$2" >&2; exit 1; fi; pass=$((pass + 1)); }

# No folder, a hand-written section: as before, and the file is untouched.
fresh
before="$(cat "$T/root/WHATS_NEW.md")"
run 1.0.0 0 "Found '## 1.0.0' entry. OK to proceed."
same "$(cat "$T/root/WHATS_NEW.md")" "$before" "a version with no folder changed WHATS_NEW.md"
# No folder and no section: the old error.
run 1.1.0 1 "WHATS_NEW.md has no '## 1.1.0 — ...' section."

# A folder: the fragments are checked, joined and written, then found.
D="$T/root/whatsnew.d/1.1.0"
mkdir -p "$D"
printf 'Fragments\n' > "$D/title"
printf 'Fixes\n' > "$D/sections"
printf '### Fixes\n- A new fix.\n' > "$D/fix.md"
run 1.1.0 0 "Found '## 1.1.0' entry. OK to proceed."
same "$(sed -n '3,6p' "$T/root/WHATS_NEW.md")" "$(printf '## 1.1.0 — Fragments\n\n### Fixes\n- A new fix.')" "the joined section"
same "$(grep -c '^## ' "$T/root/WHATS_NEW.md")" "2" "the older section is still there"

# A folder with no title stops the release before anything is written.
fresh
mkdir -p "$D"
printf 'Fixes\n' > "$D/sections"
printf '### Fixes\n- A new fix.\n' > "$D/fix.md"
before="$(cat "$T/root/WHATS_NEW.md")"
run 1.1.0 1 "whatsnew.d/1.1.0/title is missing"
same "$(cat "$T/root/WHATS_NEW.md")" "$before" "a failed check changed WHATS_NEW.md"

echo "release whatsnew hook tests: $pass passed"
