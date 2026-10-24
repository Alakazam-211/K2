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

echo "== T29 (update continuity A1): engine entries never fool an installer's parser =="
# The CI merge job's script writes the Linux daemons and the App engines.
assets="$tmp/assets"; mkdir -p "$assets"
for arch in x86_64 aarch64; do
    printf 'daemon-%s' "$arch" > "$assets/k2-daemon-linux-$arch"
    printf 'engine-%s' "$arch" > "$assets/k2-app-engine-linux-$arch"
done
bash -c "source '$LIB'; k2_daemon_manifest 9.9.9 d '$ARTS' '$tmp/WHATS_NEW.md'" > "$tmp/merged.json"
python3 -I "$ROOT/scripts/merge-daemon-manifest.py" "$tmp/merged.json" "https://rel/v9.9.9" "$assets" >/dev/null
keys="$(python3 -I -c 'import json,sys; print(",".join(sorted(json.load(open(sys.argv[1]))["artifacts"])))' "$tmp/merged.json")"
if [ "$keys" = "k2-app-engine-linux-aarch64,k2-app-engine-linux-x86_64,linux-aarch64,linux-x86_64,macos-aarch64" ]; then
    pass "merge writes engines under their own keys"
else
    fail "merge keys" "$keys"
fi
sha_of() { python3 -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$1"; }

# The parsers in the field: this tree's install-daemon.sh, the 0.46.3 copy
# (frozen), and the k2 CLI's twin.
sed -n '/^manifest_field() {/,/^}/p' "$ROOT/scripts/install-daemon.sh" > "$tmp/parser-current.sh"
cp "$SCRIPT_DIR/fixtures/install-daemon-0.46.3-manifest_field.sh" "$tmp/parser-0463.sh"
sed -n '/^_daemon_manifest_field() {/,/^}/p' "$ROOT/cli/k2" | sed 's/^_daemon_manifest_field()/manifest_field()/' > "$tmp/parser-cli.sh"
for p in current 0463 cli; do
    if ! grep -q '^manifest_field()' "$tmp/parser-$p.sh"; then
        fail "parser $p" "could not extract the function"
        continue
    fi
    for arch in x86_64 aarch64; do
        for m in "$tmp/merged.json" "$SCRIPT_DIR/fixtures/daemon-latest-with-engine.json"; do
            got_url="$(bash -c "source '$tmp/parser-$p.sh'; manifest_field \"\$(cat '$m')\" linux-$arch url")"
            got_sha="$(bash -c "source '$tmp/parser-$p.sh'; manifest_field \"\$(cat '$m')\" linux-$arch sha256")"
            if [ "$m" = "$tmp/merged.json" ]; then
                want_url="https://rel/v9.9.9/k2-daemon-linux-$arch"; want_sha="$(sha_of "$assets/k2-daemon-linux-$arch")"
            else
                want_url="https://example.invalid/v9.9.9/k2-daemon-linux-$arch"
                want_sha="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["artifacts"][sys.argv[2]]["sha256"])' "$m" "linux-$arch")"
            fi
            if [ "$got_url" = "$want_url" ] && [ "$got_sha" = "$want_sha" ]; then
                pass "parser $p, linux-$arch, $(basename "$m"): the daemon's url + sha256"
            else
                fail "parser $p linux-$arch $(basename "$m")" "url=$got_url sha=$got_sha (want $want_url $want_sha)"
            fi
        done
    done
done
# The control: the layout A1 forbids (a second "linux-x86_64" object after
# the daemon's) does fool the 0.46.3 parser, so the checks above can bite.
cat > "$tmp/bad.json" <<'JSON'
{"version":"9.9.9","artifacts":{"linux-x86_64":{"url":"https://rel/k2-daemon-linux-x86_64","sig":"s","sha256":"d1"}},
 "engine":{"linux-x86_64":{"url":"https://rel/k2-app-engine-linux-x86_64","sig":"s","sha256":"e1"}}}
JSON
bad="$(bash -c "source '$tmp/parser-0463.sh'; manifest_field \"\$(cat '$tmp/bad.json')\" linux-x86_64 url")"
if [ "$bad" = "https://rel/k2-app-engine-linux-x86_64" ]; then pass "control: a platform-keyed engine fools the old parser"; else fail "control" "$bad"; fi
eng="$(bash -c "source '$tmp/parser-current.sh'; manifest_field \"\$(cat '$tmp/merged.json')\" k2-app-engine-linux-x86_64 url")"
if [ "$eng" = "https://rel/v9.9.9/k2-app-engine-linux-x86_64" ]; then pass "the engine resolves by its own key"; else fail "engine key" "$eng"; fi

echo "== the merge refuses an airgap asset and keeps working without engines =="
python3 -I -c 'import json,sys; m=json.load(open(sys.argv[1])); m["artifacts"]["linux-x86_64-airgap"]={"url":"u","sig":"s","sha256":"x"}; json.dump(m,open(sys.argv[1],"w"))' "$tmp/merged.json"
if python3 -I "$ROOT/scripts/merge-daemon-manifest.py" "$tmp/merged.json" "https://rel/v9.9.9" "$assets" >/dev/null 2>&1; then
    fail "airgap" "must refuse"
else
    pass "airgap asset refused"
fi
rm -f "$assets"/k2-app-engine-linux-*
bash -c "source '$LIB'; k2_daemon_manifest 9.9.9 d '$ARTS' '$tmp/WHATS_NEW.md'" > "$tmp/noeng.json"
python3 -I "$ROOT/scripts/merge-daemon-manifest.py" "$tmp/noeng.json" "https://rel/v9.9.9" "$assets" >/dev/null 2>&1
if python3 -I -c 'import json,sys; a=json.load(open(sys.argv[1]))["artifacts"]; sys.exit(0 if "linux-x86_64" in a and not any("engine" in k for k in a) else 1)' "$tmp/noeng.json"; then
    pass "no engine builds → no engine keys"
else
    fail "no engines" "$(cat "$tmp/noeng.json")"
fi

echo "== the installer's dry run names the engine it would stage (R3) =="
touch "$tmp/fake-engine"
out="$(K2_APP_ENGINE_PATH="$tmp/fake-engine" K2_DRY_RUN=1 sh "$ROOT/scripts/install-daemon.sh" --manifest-url "file://$SCRIPT_DIR/fixtures/daemon-latest-with-engine.json" 2>&1)"
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64|Linux-amd64) want_line="app engine:     https://example.invalid/v9.9.9/k2-app-engine-linux-x86_64" ;;
    Linux-aarch64|Linux-arm64) want_line="app engine:     https://example.invalid/v9.9.9/k2-app-engine-linux-aarch64" ;;
    *) want_line="app engine:     (none installed; nothing to stage)" ;;
esac
if printf '%s' "$out" | grep -qF "$want_line"; then pass "dry run: $want_line"; else fail "dry run engine line" "$out"; fi
out="$(K2_APP_ENGINE_PATH="$tmp/no-such-engine" K2_DRY_RUN=1 sh "$ROOT/scripts/install-daemon.sh" --manifest-url "file://$SCRIPT_DIR/fixtures/daemon-latest-with-engine.json" 2>&1)"
if printf '%s' "$out" | grep -qF "app engine:     (none installed; nothing to stage)"; then pass "no engine installed: nothing staged"; else fail "dry run without engine" "$out"; fi

echo
echo "daemon-manifest: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
