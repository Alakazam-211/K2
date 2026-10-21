#!/usr/bin/env bash
# Dry test of scripts/daemon-manifest.sh (release.sh Step 8.5): the
# daemon-latest.json it prints carries `notes` = this version's
# WHATS_NEW.md section. Builds, signs and uploads nothing; never runs
# release.sh. Runs on the Mac and on ubuntu-latest. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LIB="$ROOT/scripts/daemon-manifest.sh"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/WHATS_NEW.md" <<'MD'
# K2 — What's New

## 9.9.10 — Not this one
- ten

## 9.9.9 — "Quotes" & ünïcode
### Area
- **Bold** line with `code` and a "quote".
- Second line.

## 9.9.8 — Older
- old
MD

ARTS='    "macos-aarch64": {
      "url": "https://example.invalid/k2-daemon-macos-aarch64",
      "sig": "https://example.invalid/k2-daemon-macos-aarch64.sig",
      "sha256": "abc"
    },
    "linux-x86_64": {
      "url": "https://example.invalid/k2-daemon-linux-x86_64",
      "sig": "https://example.invalid/k2-daemon-linux-x86_64.sig",
      "sha256": "def"
    }'

field() { python3 -I -c 'import json,sys; v=json.load(open(sys.argv[1])); print(json.dumps(v.get(sys.argv[2])))' "$1" "$2"; }

echo "== notes come from the version's section =="
bash -c "source '$LIB'; k2_daemon_manifest 9.9.9 2026-10-20T00:00:00Z '$ARTS' '$tmp/WHATS_NEW.md'" > "$tmp/m.json"
got="$(python3 -I -c '
import json, sys
want = "## 9.9.9 — \"Quotes\" & ünïcode\n### Area\n- **Bold** line with `code` and a \"quote\".\n- Second line."
got = json.load(open(sys.argv[1]))["notes"]
print("same" if got == want else "got %r want %r" % (got, want))
' "$tmp/m.json")"
if [ "$got" = "same" ]; then pass "notes = the 9.9.9 section, exactly"; else fail "notes" "$got"; fi
if [ "$(field "$tmp/m.json" version)" = '"9.9.9"' ]; then pass "version"; else fail "version" "$(cat "$tmp/m.json")"; fi
if [ "$(field "$tmp/m.json" pub_date)" = '"2026-10-20T00:00:00Z"' ]; then pass "pub_date"; else fail "pub_date" "$(cat "$tmp/m.json")"; fi
keys="$(python3 -I -c 'import json,sys; print(",".join(sorted(json.load(open(sys.argv[1]))["artifacts"])))' "$tmp/m.json")"
if [ "$keys" = "linux-x86_64,macos-aarch64" ]; then pass "artifacts kept"; else fail "artifacts" "$keys"; fi
sha="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["artifacts"]["linux-x86_64"]["sha256"])' "$tmp/m.json")"
if [ "$sha" = "def" ]; then pass "artifact fields kept"; else fail "artifact fields" "$sha"; fi

echo "== 9.9.1 must not match 9.9.10 =="
bash -c "source '$LIB'; k2_daemon_manifest 9.9.1 2026-10-20T00:00:00Z '$ARTS' '$tmp/WHATS_NEW.md'" > "$tmp/m2.json"
if [ "$(field "$tmp/m2.json" notes)" = "null" ]; then pass "no section → no notes key"; else fail "prefix match" "$(cat "$tmp/m2.json")"; fi

echo "== long notes are cut at a line end =="
{ echo "## 1.0.0 — Long"; for i in $(seq 1 400); do echo "- line $i with some words in it"; done; } > "$tmp/long.md"
bash -c "export K2_DAEMON_MANIFEST_NOTES_MAX=500; source '$LIB'; k2_daemon_manifest 1.0.0 d '$ARTS' '$tmp/long.md'" > "$tmp/m3.json"
n="$(python3 -I -c 'import json,sys; t=json.load(open(sys.argv[1]))["notes"]; print(len(t) <= 540 and t.endswith("(More in the full release notes.)") and "\n- line 1 " in t)' "$tmp/m3.json")"
if [ "$n" = "True" ]; then pass "cut + pointer"; else fail "long notes" "$(cat "$tmp/m3.json")"; fi

echo "== bad input fails loudly =="
if bash -c "source '$LIB'; k2_daemon_manifest '' d '$ARTS' '$tmp/WHATS_NEW.md'" >/dev/null 2>&1; then
    fail "empty version" "must fail"
else
    pass "empty version fails"
fi
if bash -c "source '$LIB'; k2_daemon_manifest 1.0.0 d '\"broken\": {' '$tmp/WHATS_NEW.md'" >/dev/null 2>&1; then
    fail "broken artifacts" "must fail"
else
    pass "broken artifacts JSON fails"
fi

echo "== the real WHATS_NEW.md: the top section becomes notes =="
top="$(sed -n 's/^## \([0-9][0-9.]*\) .*/\1/p' "$ROOT/WHATS_NEW.md" | head -1)"
bash -c "source '$LIB'; k2_daemon_manifest '$top' d '$ARTS' '$ROOT/WHATS_NEW.md'" > "$tmp/m4.json"
starts="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["notes"].startswith("## " + sys.argv[2] + " "))' "$tmp/m4.json" "$top")"
if [ "$starts" = "True" ]; then pass "WHATS_NEW.md ## $top → notes"; else fail "real WHATS_NEW" "$(head -c 300 "$tmp/m4.json")"; fi

echo "== release.sh uses it =="
if grep -q 'source "$PROJECT_DIR/scripts/daemon-manifest.sh"' "$ROOT/scripts/release.sh" \
   && grep -q 'k2_daemon_manifest "${VERSION}"' "$ROOT/scripts/release.sh"; then
    pass "release.sh sources + calls k2_daemon_manifest"
else
    fail "release.sh wiring" "k2_daemon_manifest not called"
fi

echo
echo "daemon-manifest: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
