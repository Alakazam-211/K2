#!/usr/bin/env bash
# `k2 doc export` end to end (prd-k2-core-exports-v1, vs-live E6) against a
# foreground sandbox daemon built from THIS worktree (temp HOME, no client
# attached), with a test-hostile `k2-export` worker (K2_EXPORT_BIN, debug
# builds only) so every limit is real:
#   - as-is now and at a past version, on a live doc and a tier-0 file:
#     byte-equal, named <stem>.v<doc version>.<ext>, " (2)" on a clash;
#   - the doc never changes (version, versions);
#   - rights: history refused without the grant, a read-only share can't
#     place beside the source (export_place_forbidden) but can fetch, or
#     place in the agent's own workspace; an App pass is refused;
#   - --out: exists, the deny list, outside every workspace = fetch;
#   - discard: a damaged blob (MPC-E705) and a worker's decode error leave
#     no output and no temp file; a flood past the output cap and a sleeper
#     past the wall are killed (export_limit); a cancel (MPC-E004);
#   - jobs belong to their caller; the audit holds metadata only;
#   - the CLI (k2 doc export, --list, --version, --out, --json, exits);
#   - after a restart: a previous run's leftover job is swept, and as-is
#     through the worker (K2_EXPORT_AS_IS_IN_WORKER) gives the same bytes.
#
#   cargo build -p k2-daemon && cargo build -p k2-export --features test-hostile \
#       && bash tests/cli/docs_export_headless.sh
#
# Every check fails loudly with the response.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

# Never inherit a session's identity from the caller (this test process only).
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d /tmp/k2exp.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run"
DAEMON_PID=""
DLOG="$SB/daemon1.log"
stop_daemon() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
        DAEMON_PID=""
    fi
}
cleanup() {
    stop_daemon
    pkill -9 -f "frpc -c $SB/" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -40 "$DLOG" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
jok() { jcheck "$1" "$2" || fail "$3: $2 is false for: $1"; ok "$3"; }
jq_() { python3 -c 'import json,sys; j=json.loads(sys.argv[1]); print(eval(sys.argv[2], {"j": j}))' "$1" "$2"; }
# body k v [k v]...: a JSON object (an @int: value is a number, @true a bool).
body() {
    python3 -c '
import json, sys
a = sys.argv[1:]
d = {}
for k, v in zip(a[0::2], a[1::2]):
    if v.startswith("@int:"):
        v = int(v[5:])
    elif v == "@true":
        v = True
    elif v.startswith("@list:"):
        v = [x for x in v[6:].split(",") if x]
    d[k] = v
print(json.dumps(d))' "$@"
}

owner() {
    local m="$1" p="$2" b="${3:-}" q="${4:-}"
    local out
    if [ "$m" = "GET" ]; then
        out="$(curl -s -G -w '\n%{http_code}' "$BASE$p" --data-urlencode "token=$DTOKEN" ${q:+--data-raw "$q"} || true)"
    else
        [ -n "$b" ] || b='{}'
        out="$(curl -s -X "$m" -w '\n%{http_code}' -H 'Content-Type: application/json' --data-raw "$b" "$BASE$p?token=$DTOKEN${q:+&$q}" || true)"
    fi
    BODY="${out%$'\n'*}"; CODE="${out##*$'\n'}"
}
agent() {
    local m="$1" p="$2" b="${3:-}" q="${4:-}"
    local out
    if [ "$m" = "GET" ]; then
        out="$(curl -s -G -w '\n%{http_code}' --unix-socket "$SOCK" -H "Authorization: Bearer $PASSPORT" "http://localhost$p" ${q:+--data-raw "$q"} || true)"
    else
        [ -n "$b" ] || b='{}'
        out="$(curl -s -X "$m" -w '\n%{http_code}' --unix-socket "$SOCK" -H "Authorization: Bearer $PASSPORT" -H 'Content-Type: application/json' --data-raw "$b" "http://localhost$p${q:+?$q}" || true)"
    fi
    BODY="${out%$'\n'*}"; CODE="${out##*$'\n'}"
}
want() { [ "$CODE" = "$1" ] || fail "$2: HTTP $CODE (want $1): $BODY"; }
code_is() { jcheck "$BODY" "j['error']['code'] == '$1'" || fail "$2: error code is not $1: $BODY"; }
exit_is() { jcheck "$BODY" "j['error']['exit'] == $1" || fail "$2: error exit is not $1: $BODY"; }

DAEMON_BIN=""
WORKER_BIN=""
start_daemon() { # $1 = log name; rest = extra env
    local log="$SB/$1"; shift
    TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
        HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
        K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
        K2_EXPORT_BIN="$WORKER_BIN" K2_EXPORT_WALL_MS=4000 K2_EXPORT_MAX_OUTPUT_BYTES=$((2 * 1024 * 1024)) \
        K2_EXPORT_MEMORY_MIB=256 "$@" \
        "$DAEMON_BIN" >"$log" 2>&1 &
    DAEMON_PID=$!
    DLOG="$log"
    local phase=""
    for _ in $(seq 1 400); do
        if [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ]; then
            DPORT="$(cat "$SB/.k2/daemon.port")"
            phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
            [ "$phase" = "ready" ] && break
        fi
        sleep 0.1
    done
    [ "$phase" = "ready" ] || fail "daemon never reached ready"
    DTOKEN="$(cat "$SB/.k2/daemon.token")"
    BASE="http://127.0.0.1:$DPORT"
    for _ in $(seq 1 100); do
        owner GET /cli/docs/status
        jcheck "$BODY" 'j.get("pool", {}).get("running")' && return 0
        sleep 0.1
    done
    fail "docs runtime never started: $BODY"
}

echo "== daemon + hostile worker =="
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || fail "k2-daemon not built (cargo build -p k2-daemon)"
WORKER_BIN="$(dirname "$DAEMON_BIN")/k2-export"
[ -x "$WORKER_BIN" ] || fail "k2-export not built beside the daemon (cargo build -p k2-export --features test-hostile)"
"$WORKER_BIN" --version | grep -q "test-hostile" || fail "$WORKER_BIN is not a test-hostile build (cargo build -p k2-export --features test-hostile)"
cat >"$SB/shim/claude" <<EOF
#!/bin/sh
env > "$SB/claude.env"
exec /bin/cat > /dev/null
EOF
chmod +x "$SB/shim/claude"
export K2_DOCS_BLOB_MAX_MIB=4
start_daemon daemon1.log
ok "sandbox daemon ready ($DAEMON_BIN, worker $WORKER_BIN)"
for _ in $(seq 1 100); do
    features="$(curl -s "$BASE/boot-status?token=$DTOKEN")"
    jcheck "$features" '"docs-export-v1" in j.get("features", [])' && break
    sleep 0.1
done
jok "$features" '"docs-export-v1" in j.get("features", [])' "docs-export-v1 advertised once the worker passed its hello"

echo "== workspaces + agent passport =="
A="$WORK/alpha"; B="$WORK/beta"
mkdir -p "$A/notes" "$A/.k2/vault" "$B/exports"
printf '# Plan\n\none\n' >"$A/notes/plan.md"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
for ws in "$A" "$B"; do
    git -c init.defaultBranch=main -C "$ws" init -q || fail "git init $ws"
    reg="$(curl -s -X POST -H 'Content-Type: application/json' \
        -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$ws")" \
        "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
    case "$reg" in *'"error"'*) fail "register $ws: $reg" ;; *"$ws"*) ;; *) fail "register $ws: $reg" ;; esac
done
resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" -H 'Content-Type: application/json' --data-raw "$(body project "$B")" || true)"
case "$resp" in *'"sessionId"'*) ;; *) fail "pinned chat: $resp" ;; esac
for _ in $(seq 1 400); do [ -s "$SB/claude.env" ] && break; sleep 0.05; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the pinned chat got no passport"
for _ in $(seq 1 200); do [ -S "$SOCK" ] && break; sleep 0.05; done
[ -S "$SOCK" ] || fail "no cell socket at '$SOCK'"
conn="$(curl -s -X POST "$BASE/cli/connections?token=$DTOKEN&action=add&project=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$B")&target=alpha")"
case "$conn" in *rror*) fail "connections add: $conn" ;; esac
owner POST /cli/docs/share "$(body workspace "$A" path notes with "$B")"; want 200 "share notes read-only"
ok "alpha + beta, beta's passport, alpha/notes shared read-only with beta"

echo "== the targets (--list) =="
owner GET /cli/docs/export/targets "" "workspace=$A&path=notes/plan.md"; want 200 "targets"
jok "$BODY" '[t for t in j["targets"] if t["id"] == "as-is"][0]["extension"] == "md" and [t for t in j["targets"] if t["id"] == "as-is"][0]["runs"] == "daemon" and [t for t in j["targets"] if t["id"] == "as-is"][0]["fidelity"] == "lossless"' "as-is: lossless, the source's extension, run by the daemon"
jok "$BODY" 'j["worker"]["available"] and any(t["id"] == "hostile-sleep" and t["runs"] == "worker" for t in j["targets"]) and j["kind"] == {"pack": "markdown"}' "the worker's targets are listed (runs: worker); a live doc's kind is its pack"

echo "== as-is now, beside the source =="
owner GET /cli/docs/status "" "workspace=$A&path=notes/plan.md"; want 200 "status"
DV0="$(jq_ "$BODY" 'j["version"]')"
owner GET /cli/docs/versions "" "workspace=$A&path=notes/plan.md"; want 200 "versions"
NV0="$(jq_ "$BODY" 'len(j["versions"])')"
owner GET /cli/docs/log "" "workspace=$A&path=notes/plan.md"; want 200 "log"
NL0="$(jq_ "$BODY" 'len(j["log"])')"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is)"; want 200 "export now"
jok "$BODY" "j['state'] == 'done' and j['version'] is None and j['docVersion'] == $DV0 and j['codec'] == 'exact' and j['placed']['path'] == 'notes/plan.v$DV0.md' and j['fetch'] is False and j['tier'] == 1" "now: placed as notes/plan.v<doc version>.md, codec exact"
cmp -s "$A/notes/plan.md" "$A/notes/plan.v$DV0.md" || fail "the export's bytes are not the file's"
ok "as-is now is byte-equal to the file"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is)"; want 200 "export now again"
jok "$BODY" "j['placed']['path'] == 'notes/plan.v$DV0 (2).md'" "a clash gets \" (2)\", never an overwrite"
cmp -s "$A/notes/plan.md" "$A/notes/plan.v$DV0 (2).md" || fail "(2) bytes differ"
ls -a "$A/notes" | grep -q 'k2x-' && fail "a temp output was left in notes/: $(ls -a "$A/notes")"
ok "no temp file is left behind"
# An export never changes the doc: same version, versions and log.
owner GET /cli/docs/status "" "workspace=$A&path=notes/plan.md"; want 200 "status after"
jok "$BODY" "j['version'] == $DV0" "the doc version is unchanged by exports"
owner GET /cli/docs/versions "" "workspace=$A&path=notes/plan.md"; want 200 "versions after"
jok "$BODY" "len(j['versions']) == $NV0" "no version was made by exports"
owner GET /cli/docs/log "" "workspace=$A&path=notes/plan.md"; want 200 "log after"
jok "$BODY" "len(j['log']) == $NL0" "the log is unchanged by exports"

echo "== at a past version (X1: the id in, the doc version in the name) =="
owner GET /cli/docs/versions "" "workspace=$A&path=notes/plan.md"; want 200 "versions"
VID="$(jq_ "$BODY" 'j["versions"][-1]["id"]')"
VSEQ="$(jq_ "$BODY" 'j["versions"][-1]["version"]')"
owner POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":"notes/plan.md","edits":[{"after":3,"text":"two"}]}))')" "workspace=$A"; want 200 "edit"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is version "@int:$VID")"; want 200 "export --version"
# (The import's doc version may equal DV0, whose names the exports above took: then " (n)".)
jok "$BODY" "j['version'] == $VID and j['docVersion'] == $VSEQ and j['placed']['path'].startswith('notes/plan.v$VSEQ') and j['placed']['path'].endswith('.md') and j['codec'] in ('exact', 'current')" "version id $VID exports doc version $VSEQ, named after the doc version"
PV="$A/$(jq_ "$BODY" 'j["placed"]["path"]')"
python3 -c 'import sys; sys.exit(0 if open(sys.argv[1],"rb").read() == b"# Plan\n\none\n" else 1)' "$PV" || fail "the past version's bytes: $(cat "$PV")"
ok "the past version's bytes, before the edit"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is version @int:999999)"; want 404 "unknown version"
exit_is 4 "unknown version"; ok "an unknown version id: 404, exit 4"

echo "== tier 0 (any file) =="
python3 -c 'import random,sys; r=random.Random(7); open(sys.argv[1],"wb").write(b"\x89PNG\r\n\x1a\n\x00" + r.randbytes(5000))' "$A/photo.png"
cp "$A/photo.png" "$WORK/photo.v0"
owner GET /cli/docs/status "" "workspace=$A&path=photo.png"; want 200 "tier-0 status"
PV0="$(jq_ "$BODY" 'j["version"]')"
owner POST /cli/docs/export "$(body workspace "$A" path photo.png target as-is)"; want 200 "tier-0 export now"
jok "$BODY" "j['tier'] == 0 and j['docVersion'] == $PV0 and j['placed']['path'] == 'photo.v$PV0.png'" "tier 0 now: photo.v<version>.png (the version id is the doc version)"
cmp -s "$A/photo.png" "$A/photo.v$PV0.png" || fail "tier-0 export bytes differ"
python3 -c 'import random,sys; r=random.Random(8); open(sys.argv[1],"wb").write(b"\x89PNG\r\n\x1a\n\x00" + r.randbytes(3000))' "$WORK/photo.new"
owner POST /cli/docs/write "$(python3 -c 'import base64,json,sys; print(json.dumps({"workspace":sys.argv[1],"path":"photo.png","base":sys.argv[2],"bytesB64":base64.b64encode(open(sys.argv[3],"rb").read()).decode()}))' "$A" "$PV0" "$WORK/photo.new")"; want 200 "tier-0 write"
owner POST /cli/docs/export "$(body workspace "$A" path photo.png target as-is version "@int:$PV0")"; want 200 "tier-0 export --version"
jok "$BODY" "j['version'] == $PV0 and j['docVersion'] == $PV0" "tier 0 at a version: both numbers are the version id"
cmp -s "$WORK/photo.v0" "$A/photo.v$PV0 (2).png" || fail "tier-0 past version bytes differ ($(ls "$A"))"
ok "tier 0: now and a past version are byte-equal to what was stored"

owner GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"; want 200 "read after"
jok "$BODY" 'j["text"] == "# Plan\n\none\ntwo\n"' "the doc holds the edit only (exports at a version changed nothing)"

echo "== rights =="
agent POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is)"; want 403 "read-only share beside"
code_is export_place_forbidden "read-only beside"; exit_is 3 "read-only beside"
jok "$BODY" '"--out" in j["error"]["hint"]' "a read-only share can't place beside the source (export_place_forbidden, exit 3, names --out)"
agent POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is version "@int:$VID")"; want 403 "no history"
code_is history_forbidden "no history"; exit_is 3 "no history"; ok "--version without the history grant: history_forbidden, exit 3"
agent POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is out "$B/exports/plan.md")"; want 200 "out in own workspace"
jok "$BODY" "j['placed']['path'] == 'exports/plan.md' and j['fetch'] is False" "--out in the agent's own workspace: placed there, by that exact name"
cmp -s "$A/notes/plan.md" "$B/exports/plan.md" || fail "out bytes differ"
agent POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is out "$A/notes/mine.md")"; want 403 "out in the sharer's workspace"
code_is export_place_forbidden "out in alpha"; ok "--out into a workspace where it can't edit: export_place_forbidden"
agent POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is fetch @true)"; want 200 "fetch"
jok "$BODY" "j['fetch'] is True and j['placed'] is None" "fetch: the job keeps the bytes, nothing is placed"
JOB="$(jq_ "$BODY" 'j["job"]')"
agent GET /cli/docs/export/fetch "" "job=$JOB"; want 200 "fetch chunk"
python3 -c 'import base64,json,sys; j=json.loads(sys.argv[1]); sys.exit(0 if base64.b64decode(j["bytesB64"]) == open(sys.argv[2],"rb").read() and j["eof"] else 1)' "$BODY" "$A/notes/plan.md" || fail "fetched bytes differ: $BODY"
ok "fetch: the bytes, base64, eof"
mkdir -p "$WORK/outside"
agent POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is out "$WORK/outside/plan.md")"; want 200 "out outside"
jok "$BODY" "j['fetch'] is True" "--out outside every workspace: fetch (the CLI writes it)"
[ ! -e "$WORK/outside/plan.md" ] || fail "the daemon wrote outside every workspace"
ok "the daemon never writes outside a workspace"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is fetch @true)"; want 200 "owner fetch"
OJOB="$(jq_ "$BODY" 'j["job"]')"
agent GET /cli/docs/export/status "" "job=$OJOB"; want 404 "another pid's job"
code_is export_job_not_found "other job"; exit_is 4 "other job"; ok "another caller's job is not found (no oracle), exit 4"
owner GET /cli/docs/export/status "" "job=$JOB"; want 200 "the owner sees an agent's job"; ok "the box owner sees every job"
out="$(curl -s -w '\n%{http_code}' -X POST -H 'Content-Type: application/json' --data-raw "$(body workspace "$A" path notes/plan.md target as-is)" "$BASE/cli/docs/export?token=k2skn_not_a_real_pass" || true)"
BODY="${out%$'\n'*}"; CODE="${out##*$'\n'}"
want 403 "app pass"; code_is app_docs_not_available "app pass"; ok "an App pass is refused (app_docs_not_available)"

echo "== --out rules, targets, options =="
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is out "$A/notes/plan.md")"; want 409 "out exists"
code_is exists "out exists"; exit_is 2 "out exists"; ok "an existing --out: 409 exists, exit 2"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is out .k2/vault/x.md)"; want 422 "out in the vault"
code_is protected "vault out"; ok "--out into the vault: protected"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target pdf)"; want 400 "unknown target"
code_is export_unsupported "unknown target"; exit_is 2 "unknown target"
jok "$BODY" 'j["error"]["id"] == "MPC-E001"' "an unknown target: export_unsupported (MPC-E001), exit 2"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is options @list:dpi=300)"; want 400 "bad option"
code_is export_declined "bad option"; exit_is 2 "bad option"
jok "$BODY" 'j["error"]["id"] == "MPC-E002" and j["error"]["details"]["code"] == "bad_option"' "an option as-is doesn't have: export_declined bad_option (MPC-E002)"

echo "== discard on error (nothing placed, no temp file) =="
python3 -c 'import random,sys; r=random.Random(9); open(sys.argv[1],"wb").write(b"\x00DMG" + r.randbytes(20000))' "$A/dmg.bin"
owner GET /cli/docs/status "" "workspace=$A&path=dmg.bin"; want 200 "dmg status"
blob="$(python3 -c '
import os, sys
best = None
for d, _, fs in os.walk(sys.argv[1]):
    if "/tmp" in d[len(sys.argv[1]):]:
        continue
    for f in fs:
        p = os.path.join(d, f)
        if best is None or os.path.getmtime(p) > os.path.getmtime(best):
            best = p
print(best or "")' "$SB/.k2/docs-blobs")"
[ -n "$blob" ] || fail "no blob file for dmg.bin"
python3 -c 'import sys; p=sys.argv[1]; b=bytearray(open(p,"rb").read()); b[len(b)//2] ^= 0xFF; open(p,"wb").write(b)' "$blob"
owner POST /cli/docs/export "$(body workspace "$A" path dmg.bin target as-is)"
[ "$CODE" != 200 ] || fail "a damaged blob exported: $BODY"
code_is blob_damaged "damaged"; exit_is 1 "damaged"
jok "$BODY" 'j["error"]["id"] == "MPC-E705"' "a damaged blob: blob_damaged (MPC-E705), exit 1"
ls -a "$A" | grep -E 'dmg\.v|k2x-' && fail "a damaged export left a file: $(ls -a "$A")"
ok "the damaged copy left nothing in the folder"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target hostile-decode)"; want 422 "decode"
code_is export_decode "decode"; exit_is 1 "decode"; ok "a worker's decode error: export_decode (MPC-E005), exit 1"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target hostile-flood)"; want 413 "flood"
code_is export_limit "flood"; exit_is 2 "flood"
jok "$BODY" 'j["error"]["details"]["limit"] == "output bytes"' "a flood past the output cap: killed, export_limit output bytes, exit 2"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target hostile-panic)"; want 500 "panic"
code_is export_internal "panic"; ok "a panicking exporter: export_internal (MPC-E007)"
ls -a "$A/notes" | grep -E 'plan\.v[0-9]+\.bin|k2x-' && fail "a failed worker export left a file: $(ls -a "$A/notes")"
ok "failed worker exports left no output and no temp file"

echo "== the worker's limits and cancel =="
t0=$(date +%s)
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target hostile-sleep)"; want 413 "sleeper"
code_is export_limit "sleeper"
jok "$BODY" 'j["error"]["details"]["limit"] == "time"' "a sleeper past the wall (4 s here): killed, export_limit time"
[ $(( $(date +%s) - t0 )) -lt 15 ] || fail "the wall kill took $(( $(date +%s) - t0 )) s"
head -c 300 /dev/zero | tr '\0' 'x' >"$A/notes/slow.txt"
owner POST /cli/docs/export "$(body workspace "$A" path notes/slow.txt target hostile-slow waitMs @int:0)"; want 200 "slow starts"
jok "$BODY" 'j["state"] in ("queued", "running")' "a long export answers running (the CLI then polls)"
SJOB="$(jq_ "$BODY" 'j["job"]')"
sleep 1
owner GET /cli/docs/export/status "" "job=$SJOB"; want 200 "slow status"
jok "$BODY" 'j["state"] == "running" and j["total"] == 300 and j["done"] >= 1' "status: running, with progress"
owner POST /cli/docs/export/cancel "$(body job "$SJOB")"; want 200 "cancel"
jok "$BODY" 'j["cancelled"] is True' "cancel accepted"
for _ in $(seq 1 50); do
    owner GET /cli/docs/export/status "" "job=$SJOB"
    [ "$CODE" != 200 ] && break
    sleep 0.1
done
want 409 "cancelled"; code_is export_cancelled "cancelled"; exit_is 1 "cancelled"
ls -a "$A/notes" | grep -E 'slow\.v|k2x-' && fail "a cancelled export left a file: $(ls -a "$A/notes")"
ok "a cancel kills the worker: export_cancelled (MPC-E004), exit 1, nothing left"

echo "== audit (metadata only) =="
audit="$SB/.k2/auth-audit.jsonl"
[ -f "$audit" ] || audit="$(ls "$SB"/.k2/*audit*.jsonl 2>/dev/null | head -1)"
[ -n "$audit" ] && [ -f "$audit" ] || fail "no access audit file under $SB/.k2"
grep '"doc-exported"' "$audit" | grep -q 'result=ok' || fail "no doc-exported result=ok line"
grep '"doc-exported"' "$audit" | grep -q 'placed=beside' || fail "no placed=beside line"
grep '"doc-exported"' "$audit" | grep -q 'result=history_forbidden' || fail "the history refusal isn't audited"
grep '"doc-exported"' "$audit" | grep -q 'result=export_limit' || fail "the limit kill isn't audited"
grep '"doc-exported"' "$audit" | grep -q 'result=blob_damaged' || fail "the damaged blob isn't audited"
if grep '"doc-exported"' "$audit" | grep -q '# Plan\|one\\ntwo'; then fail "doc content leaked into the audit"; fi
ok "one doc-exported line per export, refusals and failures too, metadata only"

echo "== k2 doc export (the CLI) =="
K2_CLI="$PROJECT_ROOT/cli/k2"
cli() {
    local who="$1" cwd="$2"; shift 2
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT")
    if [ "$who" = "agent" ]; then
        envs+=(K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$SOCK" K2_PROJECT_PATH="$B")
    fi
    set +e
    OUT="$(cd "$cwd" && env "${envs[@]}" "$K2_CLI" "$@" 2>"$WORK/cli.err" </dev/null)"
    RC=$?
    set -e
    ERR="$(cat "$WORK/cli.err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1"; }
cli owner "$A" doc export --list notes/plan.md
want_rc 0 "--list"; has "$OUT" "as-is" "--list"; has "$OUT" "lossless" "--list"; ok "k2 doc export --list prints the targets"
owner GET /cli/docs/status "" "workspace=$A&path=notes/plan.md"; DV="$(jq_ "$BODY" 'j["version"]')"
cli owner "$A" doc export --to as-is notes/plan.md
want_rc 0 "export now"
[ "$OUT" = "$A/notes/plan.v$DV.md" ] || fail "stdout is not the new file's path: $OUT"
has "$ERR" "(now, doc version $DV," "export now stderr"
cmp -s "$A/notes/plan.md" "$OUT" || fail "CLI export bytes differ"
ok "k2 doc export --to as-is: prints the new path; stderr says now and the doc version"
cli owner "$A" doc export --to as-is --version "$VID" notes/plan.md
want_rc 0 "export --version"; has "$ERR" "(version $VID, doc version $VSEQ," "both numbers"; ok "--version V: stderr prints both numbers (X1)"
cli owner "$A" doc export --to as-is --out "$WORK/outside/cli.md" notes/plan.md
want_rc 0 "--out outside"
cmp -s "$A/notes/plan.md" "$WORK/outside/cli.md" || fail "the CLI-written file differs"
[ "$OUT" = "$WORK/outside/cli.md" ] || fail "stdout: $OUT"
ls -a "$WORK/outside" | grep -q 'k2x-' && fail "the CLI left its temp file"
ok "--out outside every workspace: the CLI writes it (fetch), nothing left over"
cli owner "$A" doc export --to as-is --out "$WORK/outside/cli.md" notes/plan.md
want_rc 2 "--out exists"; has "$ERR" "already exists" "--out exists"; ok "--out onto an existing file: exit 2"
cli owner "$A" doc export --to nope notes/plan.md
want_rc 2 "unknown target"; has "$ERR" "export_unsupported" "unknown target"; ok "an unknown target: exit 2"
cli owner "$A" doc export --to as-is --json notes/plan.md
want_rc 0 "--json"; jok "$OUT" "j['docVersion'] == $DV and j['state'] == 'done'" "--json: the whole answer"
cli agent "$B" doc export -w alpha --to as-is notes/plan.md
want_rc 3 "agent beside"; has "$ERR" "export_place_forbidden" "agent beside"; ok "an agent on a read-only share: exit 3, export_place_forbidden"
cli agent "$B" doc export -w alpha --to as-is --out notes/cli.md notes/plan.md
want_rc 3 "agent -w relative out"; ok "with -w, a relative --out is in that workspace (refused there: read-only)"
cli agent "$B" doc export -w alpha --to as-is --out "$B/exports/cli.md" notes/plan.md
want_rc 0 "agent out own"; cmp -s "$A/notes/plan.md" "$B/exports/cli.md" || fail "agent out bytes"; ok "an agent exports into its own workspace with --out"
cli owner "$A" doc export notes/plan.md
want_rc 2 "no --to"; has "$ERR" "--to" "no --to"; ok "no --to: usage, exit 2"

echo "== restart: leftover jobs swept; as-is through the worker =="
stop_daemon
LEFT="0123456789abcdef0123456789abcdef"
mkdir -p "$SB/.k2/exports/jobs/$LEFT"
part="$A/notes/.plan.v1.md.k2x-$LEFT.part"
printf 'partial' >"$part"
printf '%s' "$part" >"$SB/.k2/exports/jobs/$LEFT/dest"
python3 -c 'import os,sys,time; t=time.time()-7200; [os.utime(p,(t,t)) for p in sys.argv[1:]]' "$SB/.k2/exports/jobs/$LEFT" "$part"
start_daemon daemon2.log K2_EXPORT_AS_IS_IN_WORKER=1
for _ in $(seq 1 50); do [ -e "$SB/.k2/exports/jobs/$LEFT" ] || break; sleep 0.1; done
[ ! -e "$SB/.k2/exports/jobs/$LEFT" ] || fail "the leftover job folder survived the restart"
[ ! -e "$part" ] || fail "the leftover temp output survived the restart"
ok "a previous run's job folder and its temp output are swept at start"
owner POST /cli/docs/export "$(body workspace "$A" path notes/plan.md target as-is)"; want 200 "as-is in the worker"
P="$(jq_ "$BODY" 'j["placed"]["path"]')"
cmp -s "$A/notes/plan.md" "$A/$P" || fail "as-is through the worker differs"
ok "as-is through the worker gives the same bytes as in the daemon"
echo "PASS ($pass checks)"
