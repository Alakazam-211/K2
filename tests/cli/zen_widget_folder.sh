#!/usr/bin/env bash
# The widget dev loop (0.46.1 S3) against a REAL headless daemon: a staged
# widget folder anywhere on disk is checked, written whole, previewed and
# cleared through K2, and `k2 zen validate` neither signs nor fails on
# "pending" by default.
#
# Boots the worktree's k2-daemon under a temp HOME (never the real ~/.k2,
# never a production daemon). The person's path (owner token); the agent
# paths (proposals, 403 preview) are the daemon tests in
# crates/k2-daemon/src/zen_agent_routes_tests.rs.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || { echo "FAIL: k2-daemon not built (cargo build -p k2-daemon)" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_not_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d -t k2-zen-folder-XXXXXX)"
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

mkdir -p "$SANDBOX/.k2" "$SANDBOX/shim" "$SANDBOX/tmp" "$SANDBOX/run"
env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K2_PORT -u K2SO_PORT \
    TMPDIR="$SANDBOX/tmp" XDG_RUNTIME_DIR="$SANDBOX/run" K2_RUNTIME_DIR= \
    HOME="$SANDBOX" K2_TEST_AGENT_SHIM_DIR="$SANDBOX/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON_BIN" >"$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 200); do
    [ -s "$SANDBOX/.k2/daemon.port" ] && [ -s "$SANDBOX/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SANDBOX/.k2/daemon.port" ] || { echo "FAIL: daemon never wrote daemon.port" >&2; tail -30 "$SANDBOX/daemon.log" >&2; exit 1; }
PORT="$(cat "$SANDBOX/.k2/daemon.port")"
TOKEN="$(cat "$SANDBOX/.k2/daemon.token")"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "http://127.0.0.1:$PORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; exit 1; }

ZEN="$SANDBOX/.k2/zen"

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2_PROJECT_PATH -u K2_CELL \
        HOME="$SANDBOX" TMPDIR="$SANDBOX/tmp" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="$TOKEN" \
        "$K2_CLI" "$@"
}

capture() {
    set +e
    out="$(run_k2 "$@" 2>&1)"
    rc=$?
    set -e
}

# stdout only (the --json answer; notes go to stderr).
capture_json() {
    set +e
    out="$(run_k2 "$@" 2>/dev/null)"
    rc=$?
    set -e
}

# A staged folder OUTSIDE ~/.k2, as an agent keeps it in its workspace.
STAGE="$SANDBOX/work/board"
mkdir -p "$STAGE/sub"
cat >"$STAGE/manifest.json" <<'EOF'
{"schema": 1, "id": "board", "name": "Board", "caps": ["sources:read"], "sources": "config"}
EOF
cat >"$STAGE/index.html" <<'EOF'
<!doctype html><html><body><p id="v">v1</p><script src="app.js"></script></body></html>
EOF
echo 'document.getElementById("v").textContent = "ready"' >"$STAGE/app.js"
printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR' >"$STAGE/logo.png"
echo "notes" >"$STAGE/README.md"
echo "x" >"$STAGE/notes.txt"
echo "x" >"$STAGE/sub/ignored.js"

echo "== validate --dir: checked, nothing written, even before Zen is set up =="
capture zen validate --dir "$STAGE"
assert_eq "validate --dir on a good folder exits 0" "$rc" "0"
assert_contains "it says ok for the staged copy" "$out" "ok: staged/board"
assert_contains "it notes the skipped files" "$out" "skipped notes.txt, sub/"
[ ! -e "$ZEN" ] && ok "validate --dir creates no Zen folder" || bad "validate --dir created ~/.k2/zen"
capture_json zen validate --dir "$STAGE" --json
assert_eq "validate --dir --json exits 0" "$rc" "0"
status="$(printf '%s' "$out" | python3 -c 'import json,sys; t=sys.stdin.read(); d=json.loads(t); print(d["status"], d["widget"])')"
assert_eq "--json says ok as board" "$status" "ok board"
BAD="$SANDBOX/work/broken"
mkdir -p "$BAD"
cp "$STAGE/manifest.json" "$BAD/manifest.json"
sed -i.bak 's/"board"/"broken"/' "$BAD/manifest.json" && rm -f "$BAD/manifest.json.bak"
echo '<p onclick="go()">inline handler</p>' >"$BAD/index.html"
capture zen validate --dir "$BAD"
assert_eq "validate --dir on a broken folder exits 1" "$rc" "1"
assert_contains "it says nothing was written" "$out" "in the staged copy; nothing was written"
capture zen validate --dir "$STAGE" --as other
assert_eq "--as a name the manifest id doesn't match exits 1" "$rc" "1"
assert_contains "it says why" "$out" "doesn't match the folder"
capture zen validate --dir "$SANDBOX/nope"
assert_eq "validate --dir on a missing folder exits 2" "$rc" "2"
ln -s "$STAGE/app.js" "$SANDBOX/work/link.js"
mkdir -p "$SANDBOX/work/linked" && cp "$STAGE/manifest.json" "$STAGE/index.html" "$SANDBOX/work/linked/" && ln -s "$STAGE/app.js" "$SANDBOX/work/linked/app.js"
capture zen validate --dir "$SANDBOX/work/linked"
assert_eq "a link in the folder exits 2" "$rc" "2"
capture zen validate --as x
assert_eq "--as without --dir is a usage error" "$rc" "2"
capture zen validate --dir "$STAGE" --widget board
assert_eq "--dir with --widget is a usage error" "$rc" "2"
capture zen reset --dir "$STAGE"
assert_eq "--dir on reset is a usage error" "$rc" "2"

echo "== set up (as the app does) =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/setup?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
assert_contains "setup created the folder" "$resp" '"createdFolder":true'

echo "== widget put --dir: the whole folder in one checked write =="
capture zen widget put --dir "$BAD"
assert_eq "a broken folder is refused (exit 1)" "$rc" "1"
assert_contains "it says nothing was written" "$out" "nothing was written"
[ ! -e "$ZEN/widgets/broken" ] && ok "no broken folder landed" || bad "widgets/broken appeared"
capture zen widget put --dir "$STAGE"
assert_eq "widget put --dir exits 0" "$rc" "0"
assert_contains "it lists what it wrote" "$out" "Wrote widgets/board: app.js, index.html, logo.png, manifest.json"
cmp -s "$STAGE/logo.png" "$ZEN/widgets/board/logo.png" && ok "the image arrived byte for byte" || bad "logo.png differs"
[ ! -e "$ZEN/widgets/board/README.md" ] && [ ! -e "$ZEN/widgets/board/notes.txt" ] && ok "README and unknown files stay behind" || bad "README/notes.txt were copied"
capture zen widget put --dir "$STAGE"
assert_contains "the same folder again changes nothing" "$out" "no change"
rm "$STAGE/logo.png"
echo '<!doctype html><html><body><p id="v">v2</p><script src="app.js"></script></body></html>' >"$STAGE/index.html"
capture zen widget apply --from "$STAGE"
assert_eq "apply --from exits 0" "$rc" "0"
assert_contains "apply writes the change" "$out" "index.html"
assert_contains "apply leaves the dropped image alone" "$out" "Left alone (not in the folder; --prune removes them): logo.png"
capture_json zen widget put board --dir "$STAGE" --prune --json
assert_eq "put --dir --prune exits 0" "$rc" "0"
removed="$(printf '%s' "$out" | python3 -c 'import json,sys; print(",".join(json.load(sys.stdin)["removed"]))')"
assert_eq "--prune removed the image" "$removed" "logo.png"
[ ! -e "$ZEN/widgets/board/logo.png" ] && ok "logo.png is gone" || bad "logo.png is still there"
capture zen widget put board index.html --dir "$STAGE"
assert_eq "--dir with a file name is a usage error" "$rc" "2"
capture zen widget put board index.html --from "$STAGE/index.html" --prune
assert_eq "--prune without --dir is a usage error" "$rc" "2"
capture zen widget put --help
assert_contains "put --help shows --dir" "$out" "--dir <folder>"
capture zen widget preview --help
assert_contains "preview --help shows --config" "$out" "--config key=value"
capture zen widget errors --help
assert_contains "errors --help is its own page" "$out" "Usage: k2 zen widget errors <name>"
capture zen validate --help
assert_contains "validate --help shows --dir and --strict" "$out" "--dir <folder> [--as <name>]"
assert_contains "validate --help says a check never signs" "$out" "check never signs on its own"

echo "== validate: no signing by default, pending exits 0 unless --strict =="
capture zen validate --widget board --json
assert_eq "validate --widget exits 0" "$rc" "0"
assert_not_contains "validate --widget doesn't sign" "$out" '"signed"'
capture zen validate --widget board --no-sign
assert_eq "--no-sign is still accepted" "$rc" "0"
capture zen validate --widget board --sign --json
assert_eq "validate --widget --sign exits 0" "$rc" "0"
assert_contains "--sign signs" "$out" '"signed"'
capture zen validate --widget board --sign --no-sign
assert_eq "--sign with --no-sign is a usage error" "$rc" "2"
capture zen validate --garden "Garden 2" --sign
assert_eq "--sign without --widget is a usage error" "$rc" "2"
G2="$(run_k2 zen garden list --json | python3 -c 'import json,sys; print([g["id"] for g in json.load(sys.stdin)["gardens"] if g["name"]=="Garden 2"][0])')"
PAGE="$SANDBOX/work/page.toml"
cat >"$PAGE" <<'EOF'
schema = 1

[layout]
kind = "columns"
[[layout.column]]
size = 100
min-width = 240

[[widget]]
id = "board"
kind = "custom"
widget = "board"
column = 0
[widget.props]
config = { sources = "fleet" }
EOF
echo "== validate --garden --from: a staged page, never written =="
before="$(cat "$ZEN/gardens/$G2.toml" 2>/dev/null || true)"
capture zen validate --garden "$G2" --from "$PAGE"
assert_eq "a good staged page exits 0" "$rc" "0"
printf 'schema = 1\nnot_a_key = 3\n' >"$SANDBOX/work/bad.toml"
capture zen validate --garden "$G2" --from "$SANDBOX/work/bad.toml"
assert_eq "a bad staged page exits 1" "$rc" "1"
assert_contains "it names the bad key" "$out" "not_a_key"
assert_eq "the Garden file is unchanged" "$(cat "$ZEN/gardens/$G2.toml" 2>/dev/null || true)" "$before"
capture zen validate --from "$PAGE"
assert_eq "--from without --garden is a usage error" "$rc" "2"

capture zen garden put "$G2" --from "$PAGE"
assert_eq "placing board exits 0" "$rc" "0"
capture zen validate --widget board
assert_eq "a placed widget that hasn't run: pending exits 0" "$rc" "0"
assert_contains "it says pending" "$out" "pending:"
capture zen validate --widget board --strict
assert_eq "--strict keeps exit 2 for pending" "$rc" "2"
capture zen validate --widget board --strict --json
assert_eq "--strict --json exits 2 for pending" "$rc" "2"

echo "== preview: K2's own Garden, never the person's =="
gardens_before="$(run_k2 zen garden list --json | python3 -c 'import json,sys; print(" ".join(g["id"] for g in json.load(sys.stdin)["gardens"]))')"
capture zen widget preview --dir "$STAGE" --config sources=fleet,k2-ops-boxes
assert_eq "preview exits 0" "$rc" "0"
assert_contains "it names the preview Garden" "$out" "on the \"Widget preview\" Garden"
[ -f "$ZEN/widgets/preview-board/manifest.json" ] && ok "preview-board was staged" || bad "no widgets/preview-board"
pid="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$ZEN/widgets/preview-board/manifest.json")"
assert_eq "its manifest id follows the folder" "$pid" "preview-board"
capture_json zen widget show preview-board --json
reads="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(",".join(d["placements"][0]["sources"]))')"
assert_eq "the preview placement reads the given sources" "$reads" "fleet,k2-ops-boxes"
for id in $gardens_before; do
    case "$out" in *"\"garden\": \"$id\""*) bad "the preview used the person's Garden $id" ;; esac
done
ok "no person's Garden holds the preview"
capture zen widget preview --dir "$STAGE" --config nonsense
assert_eq "a --config without = is a usage error" "$rc" "2"
capture zen widget preview --clear
assert_eq "preview --clear exits 0" "$rc" "0"
assert_contains "it says what it cleared" "$out" "widgets/preview-board"
[ ! -e "$ZEN/widgets/preview-board" ] && ok "preview-board is gone" || bad "preview-board is still there"
gardens_after="$(run_k2 zen garden list --json | python3 -c 'import json,sys; print(" ".join(g["id"] for g in json.load(sys.stdin)["gardens"]))')"
assert_eq "the person's Gardens are as they were" "$gardens_after" "$gardens_before"

echo
echo "zen_widget_folder: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
