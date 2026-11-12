# ── P1: stage local files onto the daemon before /cli/inbox/deliver ───
# When the active daemon is remote (K2 Connect / --host) OR the daemon
# cannot open the client path, the CLI uploads bytes first:
#   < 50 MB  → POST /cli/fs/upload-binary  (single-shot base64)
#   ≥ 50 MB  → POST /cli/fs/upload-chunk   (8 MB ordered chunks)
# Staging dir on the daemon: <target-ws>/.k2/tmp/msg-inbox/
# Then deliver with the staged absolute path; best-effort fs/delete after.
# Local loopback + readable path keeps the direct-path fast path.

# BEGIN_MSG_INBOX_UPLOAD_POLICY
# Single-shot ceiling for msg-inbox staging (PRD P1). Renderer clone/drop
# uses 64 MB; tray file delivery uses 50 MB so mid-size files stream early.
_MSG_INBOX_SINGLE_SHOT_MAX=50000000
_MSG_INBOX_CHUNK_BYTES=8388608

# Pure: exit 0 when size should use chunked upload-chunk.
_msg_inbox_use_chunked() {
    local size="${1:-0}"
    # Guard non-numeric.
    case "$size" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$size" -ge "${_MSG_INBOX_SINGLE_SHOT_MAX}" ]
}
# END_MSG_INBOX_UPLOAD_POLICY

# True when BASE_URL points at loopback (daemon and CLI share a host FS view).
_msg_daemon_is_local() {
    case "${BASE_URL:-}" in
        http://127.0.0.1:*|http://localhost:*|http://[::1]:*|\
        https://127.0.0.1:*|https://localhost:*|https://[::1]:*)
            return 0 ;;
        *)
            return 1 ;;
    esac
}

# True when a /cli/inbox/deliver JSON body means the daemon could not open path.
_msg_inbox_is_not_readable_error() {
    local response="$1"
    if ! command -v python3 >/dev/null 2>&1; then
        case "$response" in
            *"not a readable file"*|*"source path is not"*) return 0 ;;
            *) return 1 ;;
        esac
    fi
    echo "$response" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    s = raw.lower()
    sys.exit(0 if ("not a readable file" in s or "source path is not" in s) else 1)
err = d.get("error")
if isinstance(err, dict):
    s = " ".join(str(err.get(k) or "") for k in ("hint", "message", "code", "error"))
else:
    s = str(err or "")
s = s.lower()
sys.exit(0 if ("not a readable file" in s or "source path is not" in s) else 1)
' 2>/dev/null
}

# Resolve target workspace token → absolute path ON THE ACTIVE DAEMON.
# Echoes path or empty.
_msg_inbox_resolve_ws_path() {
    local target="$1"
    local out
    out=$(cli_send cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null) || out=""
    if command -v python3 >/dev/null 2>&1; then
        echo "$out" | python3 -c '
import json, sys
try:
    print(json.load(sys.stdin).get("path") or "")
except Exception:
    pass
' 2>/dev/null
    else
        # Crude fallback: "path":"..."
        echo "$out" | sed -n 's/.*"path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1
    fi
}

# POST JSON body from a file (avoids ARG_MAX on multi-MB base64 payloads).
# Uses $TOKEN like cli_post_json (owner or connect-user; not UDS-scoped fs).
_msg_inbox_cli_post_json_file() {
    local endpoint="$1"
    local body_file="$2"
    local max_time="${3:-300}"
    local _K2_CFG=""
    _k2_cfg_add url "${BASE_URL}${endpoint}?token=${TOKEN}"
    _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 10 --max-time "$max_time" \
        -H "Content-Type: application/json" \
        --data-binary @"$body_file" 2>/dev/null
}

# Small JSON body (delete etc.). Token + body ride the curl config.
_msg_inbox_cli_post_json() {
    local endpoint="$1"
    local body="$2"
    local max_time="${3:-30}"
    local _K2_CFG=""
    _k2_cfg_add url "${BASE_URL}${endpoint}?token=${TOKEN}"
    _k2_cfg_add data-raw "$body"
    _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 10 --max-time "$max_time" \
        -H "Content-Type: application/json" 2>/dev/null
}

# BEGIN_MSG_INBOX_FS_AUTH
# True when /cli/fs/upload-* JSON body means gate/auth failure (403, invalid
# token, forbidden, not-owner). Scoped in-cell passports hit this path —
# they cannot stage via /cli/fs/* (owner or connect-user token required).
_msg_inbox_is_fs_auth_error() {
    local response="$1"
    if [ -z "$response" ]; then
        return 1
    fi
    if ! command -v python3 >/dev/null 2>&1; then
        case "$response" in
            *"invalid or missing token"*|*"403"*|*"forbidden"*|*"Forbidden"*|\
            *"not owner"*|*"owner_only"*|*"unauthorized"*|*"Unauthorized"*)
                return 0 ;;
            *) return 1 ;;
        esac
    fi
    echo "$response" | python3 -c '
import json, sys
raw = sys.stdin.read()
needles = (
    "invalid or missing token",
    "forbidden",
    "not owner",
    "owner_only",
    "unauthorized",
    "not authorized",
    "permission denied",
    "invalid token",
    "missing token",
)
try:
    d = json.loads(raw)
except Exception:
    s = raw.lower()
    sys.exit(0 if any(n in s for n in needles) or "403" in s else 1)
err = d.get("error")
parts = []
if isinstance(err, dict):
    for k in ("code", "hint", "message", "error", "reason"):
        v = err.get(k)
        if v is not None:
            parts.append(str(v))
    code = str(err.get("code") or "").lower()
    if code in ("forbidden", "owner_only", "unauthorized", "invalid_token",
                "not_owner", "gated", "auth", "permission_denied"):
        sys.exit(0)
elif isinstance(err, str):
    parts.append(err)
elif err is not None:
    parts.append(str(err))
# Also scan top-level code/hint (some arms use flat shape).
for k in ("code", "hint", "message", "status"):
    v = d.get(k)
    if v is not None:
        parts.append(str(v))
s = " ".join(parts).lower()
if any(n in s for n in needles):
    sys.exit(0)
if "403" in s:
    sys.exit(0)
sys.exit(1)
' 2>/dev/null
}

# Clear teaching for passport/token failures on tray staging uploads.
# Prefer exit 3 (gate-like) at call sites after this prints.
_msg_inbox_teach_fs_auth() {
    cat >&2 <<'EOF'
Error: staging a file onto the host requires an owner or K2 Connect user token.
In-cell agent passports cannot call /cli/fs/* for remote upload.
Same-machine: ensure the daemon can read the path (loopback) so no upload is needed.
Remote: run k2 as the human/owner session against that server, or use a connect-user token.
EOF
}
# END_MSG_INBOX_FS_AUTH

# Upload local file to daemon staging dir. Echoes remote absolute path.
# Args: <local_abs_path> <remote_dir> <filename>
_msg_inbox_upload_file() {
    local local_path="$1"
    local remote_dir="$2"
    local filename="$3"
    local size
    size=$(wc -c < "$local_path" | tr -d '[:space:]') || size=0
    case "$size" in
        ''|*[!0-9]*) size=0 ;;
    esac

    if ! command -v python3 >/dev/null 2>&1; then
        echo "Error: python3 required to stage file for remote inbox deliver" >&2
        return 1
    fi

    local body_file use_chunked="false"
    if _msg_inbox_use_chunked "$size"; then
        use_chunked="true"
    fi
    body_file=$(mktemp "${TMPDIR:-/tmp}/k2-msg-inbox-upload.XXXXXX") || {
        echo "Error: cannot create temp file for upload body" >&2
        return 1
    }

    if [[ "$use_chunked" != "true" ]]; then
        # Single-shot upload-binary (body via temp file — ARG_MAX safe).
        if ! python3 -c '
import base64, json, sys
path, directory, filename, out = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
with open(path, "rb") as f:
    b64 = base64.b64encode(f.read()).decode("ascii")
with open(out, "w", encoding="utf-8") as o:
    json.dump({"dir": directory, "filename": filename, "base64": b64},
              o, ensure_ascii=False, separators=(",", ":"))
' "$local_path" "$remote_dir" "$filename" "$body_file"; then
            rm -f "$body_file"
            echo "Error: failed to encode file for upload: $local_path" >&2
            return 1
        fi
        # Scale timeout with size (min 60s, ~2s/MB headroom, cap 600s).
        local mt=60
        if [ "$size" -gt 0 ]; then
            mt=$(( 60 + size / 500000 ))
            [ "$mt" -gt 600 ] && mt=600
        fi
        local resp remote_path
        resp=$(_msg_inbox_cli_post_json_file "/cli/fs/upload-binary" "$body_file" "$mt")
        rm -f "$body_file"
        if _msg_inbox_is_fs_auth_error "${resp:-}"; then
            _msg_inbox_teach_fs_auth
            return 3
        fi
        remote_path=$(echo "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(1)
if d.get("error"):
    sys.stderr.write(str(d.get("error")) + "\n")
    sys.exit(1)
p = d.get("path") or ""
if not p:
    sys.exit(1)
print(p)
' 2>/dev/null) || {
            if _msg_inbox_is_fs_auth_error "${resp:-}"; then
                _msg_inbox_teach_fs_auth
                return 3
            fi
            echo "Error: upload-binary failed for $(basename "$local_path"): ${resp:-no response}" >&2
            return 1
        }
        printf '%s' "$remote_path"
        return 0
    fi

    # Chunked upload-chunk loop (same wire format as renderer upload-to-remote).
    local upload_id offset=0 chunk_bytes="${_MSG_INBOX_CHUNK_BYTES}"
    upload_id="msg-inbox-$$-$(date +%s)-$RANDOM"
    local remote_path=""
    while [ "$offset" -lt "$size" ]; do
        local len=$(( size - offset ))
        [ "$len" -gt "$chunk_bytes" ] && len=$chunk_bytes
        local is_last="false"
        [ $(( offset + len )) -ge "$size" ] && is_last="true"
        if ! python3 -c '
import base64, json, sys
path, offset, length = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
upload_id, directory, filename = sys.argv[4], sys.argv[5], sys.argv[6]
is_last = sys.argv[7] == "true"
total = int(sys.argv[8])
out = sys.argv[9]
with open(path, "rb") as f:
    f.seek(offset)
    data = f.read(length)
body = {
    "upload_id": upload_id,
    "dir": directory,
    "filename": filename,
    "offset": offset,
    "base64": base64.b64encode(data).decode("ascii"),
    "is_last": is_last,
}
if offset == 0:
    body["total_bytes"] = total
with open(out, "w", encoding="utf-8") as o:
    json.dump(body, o, ensure_ascii=False, separators=(",", ":"))
' "$local_path" "$offset" "$len" "$upload_id" "$remote_dir" "$filename" \
            "$is_last" "$size" "$body_file"; then
            rm -f "$body_file"
            echo "Error: failed to build upload-chunk body at offset $offset" >&2
            return 1
        fi
        local resp
        resp=$(_msg_inbox_cli_post_json_file "/cli/fs/upload-chunk" "$body_file" 120)
        if _msg_inbox_is_fs_auth_error "${resp:-}"; then
            rm -f "$body_file"
            _msg_inbox_teach_fs_auth
            return 3
        fi
        if [ "$is_last" = "true" ]; then
            remote_path=$(echo "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(1)
if d.get("error"):
    sys.stderr.write(str(d.get("error")) + "\n")
    sys.exit(1)
p = d.get("path") or ""
if not p:
    sys.exit(1)
print(p)
' 2>/dev/null) || {
                rm -f "$body_file"
                if _msg_inbox_is_fs_auth_error "${resp:-}"; then
                    _msg_inbox_teach_fs_auth
                    return 3
                fi
                echo "Error: upload-chunk finalize failed: ${resp:-no response}" >&2
                return 1
            }
        else
            if ! echo "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(1)
if d.get("error"):
    sys.stderr.write(str(d.get("error")) + "\n")
    sys.exit(1)
' 2>/dev/null; then
                rm -f "$body_file"
                if _msg_inbox_is_fs_auth_error "${resp:-}"; then
                    _msg_inbox_teach_fs_auth
                    return 3
                fi
                echo "Error: upload-chunk failed at offset $offset: ${resp:-no response}" >&2
                return 1
            fi
        fi
        offset=$(( offset + len ))
    done
    rm -f "$body_file"
    if [ -z "$remote_path" ]; then
        echo "Error: chunked upload sent no data (empty file?)" >&2
        return 1
    fi
    printf '%s' "$remote_path"
    return 0
}

# Stage local file under <ws>/.k2/tmp/msg-inbox/ on the daemon. Echoes remote path.
# Exit 3 on passport/fs-auth gate (teaching already printed); 1 on other errors.
_msg_inbox_stage_file() {
    local target="$1"
    local local_path="$2"
    local ws_path filename remote_dir remote_path
    local rc=0
    ws_path=$(_msg_inbox_resolve_ws_path "$target")
    if [ -z "$ws_path" ]; then
        # Fall back: if target looks absolute, use it; else fail loudly.
        case "$target" in
            /*) ws_path="$target" ;;
            *)
                echo "Error: cannot resolve workspace '$target' on daemon for file staging" >&2
                return 1
                ;;
        esac
    fi
    filename=$(basename "$local_path")
    # Prefer .k2/ layout (0.40+); write_upload creates the tree.
    remote_dir="${ws_path}/.k2/tmp/msg-inbox"
    remote_path=$(_msg_inbox_upload_file "$local_path" "$remote_dir" "$filename") || rc=$?
    if [ "$rc" -ne 0 ]; then
        return "$rc"
    fi
    printf '%s' "$remote_path"
    return 0
}

# Best-effort permanent delete of a staged path on the daemon.
_msg_inbox_delete_staged() {
    local staged_path="$1"
    [ -z "$staged_path" ] && return 0
    if ! command -v python3 >/dev/null 2>&1; then
        return 0
    fi
    local body
    body=$(python3 -c 'import json,sys; print(json.dumps({"paths":[sys.argv[1]],"permanent":True}))' "$staged_path") || return 0
    _msg_inbox_cli_post_json "/cli/fs/delete" "$body" 30 >/dev/null 2>&1 || true
}

# Shared JSON error mapper for inbox compose / deliver responses.
# Exit 3 for not_connected/gated/forbidden; exit 1 for other structured
# errors; exit 0 on success. For wake-after-package: package ok + wake
# fail still exits 0 (wake field present for callers).
_msg_inbox_map_response() {
    local response="$1"
    local wake_mode="${2:-false}"
    if ! command -v python3 >/dev/null 2>&1; then
        echo "$response"
        return 0
    fi
    # An error is said once, as the stderr teaching line below; the raw
    # body on stdout too printed it twice (smoke r6).
    if ! printf '%s' "$response" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(1)
sys.exit(0 if isinstance(d, dict) and (isinstance(d.get("error"), dict) or (isinstance(d.get("error"), str) and d.get("error"))) else 1)' 2>/dev/null; then
        echo "$response"
    fi
    echo "$response" | WAKE_MODE="$wake_mode" python3 -c '
import json, sys, os
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
err = d.get("error")
if isinstance(err, dict):
    code = err.get("code") or ""
    hint = err.get("hint") or code
    sys.stderr.write(json.dumps({"error": {"code": code, "hint": hint}},
                                ensure_ascii=False, separators=(",", ":")) + "\n")
    if code in ("not_connected", "gated", "forbidden"):
        sys.exit(3)
    sys.exit(1)
if isinstance(err, str) and err:
    sys.stderr.write(json.dumps({"error": {"code": "error", "hint": err}},
                                ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.exit(1)
if d.get("ok") is False and "id" not in d and "filename" not in d:
    sys.exit(1)
# A7: a federated agent::host typed by another name of that workspace
# (the daemon says which), same wording as the live k2 msg note.
if isinstance(d.get("addressNote"), str) and d["addressNote"]:
    sys.stderr.write("note: %s\n" % d["addressNote"])
# Package primary: if wake failed after package landed, note it but exit 0.
if os.environ.get("WAKE_MODE") == "true":
    wake = d.get("wake")
    if isinstance(wake, dict) and wake.get("success") is False:
        reason = wake.get("reason") or "wake_failed"
        hint = wake.get("hint") or ""
        sys.stderr.write(
            f"note: package landed; wake notify failed ({reason})"
            + (f": {hint}" if hint else "")
            + "\n"
        )
sys.exit(0)
' || return $?
}

# Resolve a client path to absolute. Echoes path or empty.
_msg_inbox_abspath() {
    local path="$1"
    local abs_path=""
    if command -v python3 >/dev/null 2>&1; then
        abs_path=$(python3 -c 'import os,sys; print(os.path.abspath(os.path.expanduser(sys.argv[1])))' "$path" 2>/dev/null) || abs_path=""
    fi
    if [[ -z "$abs_path" ]]; then
        case "$path" in
            /*) abs_path="$path" ;;
            ~/*) abs_path="${HOME}/${path#~/}" ;;
            *) abs_path="$(pwd)/$path" ;;
        esac
    fi
    printf '%s' "$abs_path"
}

# Build a flat .tar.gz of absolute files (basenames only; collisions get -N
# suffix). Echoes the temp archive path. Caller must rm it.
_msg_inbox_pack_tar_gz() {
    local out=""
    if ! command -v python3 >/dev/null 2>&1; then
        echo "Error: python3 required to pack multi-file inbox bundle" >&2
        return 1
    fi
    out=$(mktemp "${TMPDIR:-/tmp}/k2-msg-inbox-bundle.XXXXXX.tar.gz") || {
        echo "Error: cannot create temp archive for multi-file inbox" >&2
        return 1
    }
    if ! python3 -c '
import os, sys, tarfile
out = sys.argv[1]
paths = sys.argv[2:]
used = set()
with tarfile.open(out, "w:gz") as tf:
    for p in paths:
        base = os.path.basename(p) or "attachment"
        name = base
        n = 2
        while name in used:
            stem, dot, ext = base.rpartition(".")
            if dot and stem:
                name = f"{stem}-{n}.{ext}"
            else:
                name = f"{base}-{n}"
            n += 1
        used.add(name)
        tf.add(p, arcname=name)
' "$out" "$@"; then
        rm -f "$out"
        echo "Error: failed to pack multi-file inbox bundle" >&2
        return 1
    fi
    printf '%s' "$out"
    return 0
}

# BEGIN_MSG_INBOX_TRAY_RESTORE
# Globals (not function-locals): a RETURN trap after `local` vars are
# torn down used to unbound `_tray_remote_applied` under `set -u` (#64).
_K2_TRAY_SAVED_BASE_URL=""
_K2_TRAY_SAVED_TOKEN=""
_K2_TRAY_REMOTE_APPLIED=0
_tray_restore_env() {
    if [ "${_K2_TRAY_REMOTE_APPLIED:-0}" = "1" ]; then
        BASE_URL="${_K2_TRAY_SAVED_BASE_URL}"
        TOKEN="${_K2_TRAY_SAVED_TOKEN}"
        _K2_TRAY_REMOTE_APPLIED=0
        _K2_FORCE_TCP=0
    fi
}
# END_MSG_INBOX_TRAY_RESTORE

# BEGIN_MSG_INBOX_MERGE_BRIEF
# Merge --body with leftover non-file positionals. Usage:
#   _msg_inbox_merge_brief "<existing-body>" <file> [file...] -- <positional...>
# Files listed before `--` are the delivered paths (as the user typed them).
# Positionals after `--` that are not in that file list become the brief.
_msg_inbox_merge_brief() {
    local existing="${1-}"
    shift
    local files=() extras=() seen_sep=0
    local a already fp
    for a in "$@"; do
        if [ "$seen_sep" = "0" ] && [ "$a" = "--" ]; then
            seen_sep=1
            continue
        fi
        if [ "$seen_sep" = "0" ]; then
            files+=("$a")
        else
            already=0
            for fp in "${files[@]+"${files[@]}"}"; do
                if [ "$a" = "$fp" ]; then
                    already=1
                    break
                fi
            done
            if [ "$already" = "0" ]; then
                extras+=("$a")
            fi
        fi
    done
    local joined=""
    if [ ${#extras[@]} -gt 0 ]; then
        joined="${extras[*]}"
    fi
    if [ -n "$existing" ] && [ -n "$joined" ]; then
        printf '%s' "${existing} ${joined}"
    elif [ -n "$joined" ]; then
        printf '%s' "$joined"
    else
        printf '%s' "$existing"
    fi
}
# END_MSG_INBOX_MERGE_BRIEF

cmd_msg_inbox_form() {
    # Wrapper so Connect host BASE_URL/TOKEN restore always runs after
    # inner returns — never via a RETURN trap on function-locals (#64).
    _K2_TRAY_SAVED_BASE_URL="${BASE_URL}"
    _K2_TRAY_SAVED_TOKEN="${TOKEN}"
    _K2_TRAY_REMOTE_APPLIED=0
    _cmd_msg_inbox_form_inner "$@"
    local rc=$?
    _tray_restore_env
    return "$rc"
}

_cmd_msg_inbox_form_inner() {
    # `msg <ws> --inbox-silent|wake [path …] [--title ...] [--body ...] [--from ...]`
    #
    # P2 multi-file: multiple positionals that are files become ONE tray
    # package (cover + sidecars under <id>.files/). Local loopback posts
    # paths[] to /cli/inbox/deliver; remote multi packs a flat tar.gz,
    # stages via upload-binary/chunk, then /cli/inbox/deliver-bundle.
    #
    # Files + sender brief: --body and leftover non-file positionals are
    # posted as `body=` on deliver (cover note). Not XOR with paths (#64).
    #
    # Remote tray routing (agent::host / name@host): SAME plane as live
    # `k2 msg agent::host "text"` — local daemon federation. Connected peer
    # (`k2 connections`) is the gate, not a Connect host token. The CLI
    # keeps BASE_URL on this box and posts /cli/inbox/deliver with
    # workspace=agent::host; the daemon reads the files and forwards a
    # tray envelope. `--host` / explicit remote daemon still uses Connect
    # HTTP (then _K2_TRAY_REMOTE_APPLIED forces TCP so UDS cannot split).
    #
    # #36: recipient is workspace=<token> (same as live msg) — stamp_principal
    # never overwrites workspace=/target=.
    local target="$1"
    shift
    local mode="$1"   # silent | wake
    shift

    _split_remote_target "$target"
    if [ "${_IS_WIRE_FORM:-0}" = "1" ]; then
        echo "k2 msg: '${target}' looks like federation wire form (<fp>::<workspace>::<agent>)." >&2
        echo "  Tray file send uses the user form:  k2 msg <agent>::<host> --inbox-wake <path>" >&2
        echo "  Wire form is for:              k2 fed send <fp>::<ws>::<agent> \"text\"" >&2
        return 2
    fi
    # Keep the original agent::host token. Do NOT require a Connect token
    # and do NOT _apply_remote_host — that UDS-split deliver onto this
    # box while staging on the peer (Bug A) and blocked federation-only
    # peers (Bug B).

    local title="" body="" priority="" from="" path="" summary=""
    local positional=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --summary)  summary="$2"; shift 2 ;;
            --title)    title="$2"; shift 2 ;;
            --body)     body="$2"; shift 2 ;;
            --priority) priority="$2"; shift 2 ;;
            --from)     from="$2"; shift 2 ;;
            --path)     path="$2"; shift 2 ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 msg --help')" >&2
                exit 2 ;;
            *)
                positional+=("$1"); shift ;;
        esac
    done

    # Collect ALL positional paths that are readable files (P2 multi-file).
    # Explicit --path is treated as a single-file override when set.
    local file_paths=()
    if [[ -n "$path" ]]; then
        file_paths+=("$path")
    else
        local cand
        for cand in "${positional[@]+"${positional[@]}"}"; do
            if [[ -f "$cand" ]]; then
                file_paths+=("$cand")
            fi
        done
    fi
    # If no file found but one positional looks path-like, keep it for
    # a clearer error (daemon will reject missing file).
    if [[ ${#file_paths[@]} -eq 0 && ${#positional[@]} -gt 0 ]]; then
        local p0="${positional[0]}"
        case "$p0" in
            ./*|/*|~/*|*.md|*.txt|*.pdf|*.png|*.jpg|*.zip|*.json|*.csv)
                file_paths+=("$p0") ;;
        esac
    fi

    # Sender brief: --body plus any positional that is not a delivered file.
    if [[ ${#file_paths[@]} -gt 0 ]]; then
        body="$(_msg_inbox_merge_brief "$body" "${file_paths[@]}" -- "${positional[@]+"${positional[@]}"}")"
    fi

    local wake_flag="false"
    [[ "$mode" == "wake" ]] && wake_flag="true"

    # ── File package path (primary; single or multi) ─────────────────
    if [[ ${#file_paths[@]} -gt 0 ]]; then
        local abs_paths=()
        local fp abs_path
        for fp in "${file_paths[@]}"; do
            abs_path=$(_msg_inbox_abspath "$fp")
            if [[ ! -f "$abs_path" ]]; then
                echo "Error: file not found: $fp" >&2
                echo "Usage: k2 msg <workspace> --inbox-${mode} <path> [path…] [--title \"...\"] [--body \"...\"] [--from <name>]" >&2
                exit 2
            fi
            abs_paths+=("$abs_path")
        done

        local need_upload="false"
        if ! _msg_daemon_is_local; then
            need_upload="true"
        fi

        _msg_inbox_post_deliver_single() {
            local pth="$1"
            local params="workspace=$(urlencode "$target")&path=$(urlencode "$pth")&wake=$(urlencode "$wake_flag")&mode=$(urlencode "$mode")&source=msg-inbox"
            [[ -n "$title" ]] && params="${params}&title=$(urlencode "$title")"
            [[ -n "$body" ]] && params="${params}&body=$(urlencode "$body")"
            [[ -n "$summary" ]] && params="${params}&summary=$(urlencode "$summary")"
            [[ -n "$from" ]] && params="${params}&from=$(urlencode "$from")"
            cli_send cli_post_form "/cli/inbox/deliver" "$params"
        }

        _msg_inbox_post_deliver_multi() {
            # paths= is a JSON array of absolute paths the daemon can open.
            local paths_json
            paths_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${abs_paths[@]}") || {
                echo "Error: failed to encode paths JSON for multi-file deliver" >&2
                return 1
            }
            local params="workspace=$(urlencode "$target")&paths=$(urlencode "$paths_json")&wake=$(urlencode "$wake_flag")&mode=$(urlencode "$mode")&source=msg-inbox"
            [[ -n "$title" ]] && params="${params}&title=$(urlencode "$title")"
            [[ -n "$body" ]] && params="${params}&body=$(urlencode "$body")"
            [[ -n "$summary" ]] && params="${params}&summary=$(urlencode "$summary")"
            [[ -n "$from" ]] && params="${params}&from=$(urlencode "$from")"
            cli_send cli_post_form "/cli/inbox/deliver" "$params"
        }

        _msg_inbox_post_deliver_bundle() {
            local pth="$1"
            local params="workspace=$(urlencode "$target")&path=$(urlencode "$pth")&wake=$(urlencode "$wake_flag")&mode=$(urlencode "$mode")&source=msg-inbox"
            [[ -n "$title" ]] && params="${params}&title=$(urlencode "$title")"
            [[ -n "$body" ]] && params="${params}&body=$(urlencode "$body")"
            [[ -n "$summary" ]] && params="${params}&summary=$(urlencode "$summary")"
            [[ -n "$from" ]] && params="${params}&from=$(urlencode "$from")"
            cli_send cli_post_form "/cli/inbox/deliver-bundle" "$params"
        }

        local response staged_path=""

        # ── Multi-file ──────────────────────────────────────────────
        if [[ ${#abs_paths[@]} -gt 1 ]]; then
            if [[ "$need_upload" != "true" ]]; then
                response=$(_msg_inbox_post_deliver_multi) || true
                if _msg_inbox_is_not_readable_error "$response"; then
                    need_upload="true"
                else
                    _msg_inbox_map_response "$response" "$wake_flag"
                    return $?
                fi
            fi
            # Remote (or local FS skew): pack flat tar.gz → stage → deliver-bundle.
            local bundle_local stage_rc=0
            bundle_local=$(_msg_inbox_pack_tar_gz "${abs_paths[@]}") || exit 1
            staged_path=$(_msg_inbox_stage_file "$target" "$bundle_local") || stage_rc=$?
            if [ "$stage_rc" -ne 0 ]; then
                rm -f "$bundle_local"
                # Passport/fs-auth teaching already printed (exit 3).
                [ "$stage_rc" -eq 3 ] && exit 3
                echo "Error: failed to stage multi-file bundle on daemon for inbox deliver" >&2
                exit 1
            fi
            rm -f "$bundle_local"
            response=$(_msg_inbox_post_deliver_bundle "$staged_path")
            _msg_inbox_map_response "$response" "$wake_flag"
            local bundle_rc=$?
            # Never delete the staged archive on deliver failure — the
            # peer (or local daemon) may still need it for retry.
            if [ "$bundle_rc" -eq 0 ]; then
                _msg_inbox_delete_staged "$staged_path"
            fi
            return "$bundle_rc"
        fi

        # ── Single-file (P0/P1 path unchanged) ───────────────────────
        local deliver_path="${abs_paths[0]}"
        if [[ "$need_upload" != "true" ]]; then
            response=$(_msg_inbox_post_deliver_single "$deliver_path")
            if _msg_inbox_is_not_readable_error "$response"; then
                need_upload="true"
            else
                _msg_inbox_map_response "$response" "$wake_flag"
                return $?
            fi
        fi

        if [[ "$need_upload" == "true" ]]; then
            local stage_rc=0
            staged_path=$(_msg_inbox_stage_file "$target" "${abs_paths[0]}") || stage_rc=$?
            if [ "$stage_rc" -ne 0 ]; then
                [ "$stage_rc" -eq 3 ] && exit 3
                echo "Error: failed to stage local file on daemon for inbox deliver" >&2
                exit 1
            fi
            deliver_path="$staged_path"
            response=$(_msg_inbox_post_deliver_single "$deliver_path")
            _msg_inbox_map_response "$response" "$wake_flag"
            local staged_rc=$?
            # Best-effort cleanup of staging copy only after the package
            # landed. A failed deliver must leave the staged file in place.
            if [ "$staged_rc" -eq 0 ]; then
                _msg_inbox_delete_staged "$staged_path"
            fi
            return "$staged_rc"
        fi
    fi

    # ── Legacy string compose (--title/--body, no path) ──────────────
    if [[ -z "$title" ]]; then
        echo "Usage: k2 msg <workspace> --inbox-${mode} <path> [path…] [--title \"...\"] [--body \"...\"] [--from <name>]" >&2
        echo "   or: k2 msg <workspace> --inbox-${mode} --title \"...\" [--body \"...\"]" >&2
        exit 2
    fi

    local params="workspace=$(urlencode "$target")&title=$(urlencode "$title")&source=msg-inbox"
    [[ -n "$body" ]] && params="${params}&body=$(urlencode "$body")"
    [[ -n "$summary" ]] && params="${params}&summary=$(urlencode "$summary")"
    [[ -n "$priority" ]] && params="${params}&priority=$(urlencode "$priority")"
    [[ -n "$from" ]] && params="${params}&from=$(urlencode "$from")"
    local response

    # Federated agent::host (or legacy agent@host): the wake rides the
    # tray and the PEER wakes its own workspace, answering with `wake`.
    # Never knock /cli/workspace/msg on this box with a federated token —
    # it can't resolve here and always answered workspace_not_found
    # (0.45.6, appa 2026-10-11..13).
    _split_remote_target "$target"
    if [[ -n "${_REMOTE_HOST:-}" ]]; then
        params="${params}&wake=$(urlencode "$wake_flag")&mode=$(urlencode "$mode")"
        response=$(cli_send cli_post_form "/cli/inbox/compose" "$params")
        _msg_inbox_map_response "$response" "$wake_flag"
        return $?
    fi

    response=$(cli_send cli_post_form "/cli/inbox/compose" "$params")

    # Compose-only for silent. For wake: after compose success, deliver_live
    # with the inbox pointer (package ok is primary).
    if [[ "$wake_flag" != "true" ]]; then
        _msg_inbox_map_response "$response" "false"
        return $?
    fi

    # Parse compose result; on error, map and exit. On success, wake.
    local compose_id="" compose_title="$title"
    if command -v python3 >/dev/null 2>&1; then
        local mapped
        mapped=$(echo "$response" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("PARSE_FAIL"); sys.exit(0)
err = d.get("error")
if isinstance(err, dict):
    code = err.get("code") or ""
    hint = err.get("hint") or code
    print("ERR\t" + code + "\t" + hint)
    sys.exit(0)
if isinstance(err, str) and err:
    print("ERR\terror\t" + err)
    sys.exit(0)
if d.get("ok") is False and "id" not in d:
    print("ERR\terror\tcompose failed")
    sys.exit(0)
print("OK\t" + (d.get("id") or "") + "\t" + (d.get("title") or ""))
') || true
        case "$mapped" in
            ERR$'\t'*)
                local ecode hint
                ecode=$(printf '%s' "$mapped" | cut -f2)
                hint=$(printf '%s' "$mapped" | cut -f3-)
                # Said once, on stderr (smoke r6: the raw body on stdout
                # repeated it).
                printf '%s\n' "$(python3 -c 'import json,sys; print(json.dumps({"error":{"code":sys.argv[1],"hint":sys.argv[2]}},separators=(",",":")))' "$ecode" "$hint")" >&2
                if [[ "$ecode" == "not_connected" || "$ecode" == "gated" || "$ecode" == "forbidden" ]]; then
                    return 3
                fi
                return 1
                ;;
            OK$'\t'*)
                compose_id=$(printf '%s' "$mapped" | cut -f2)
                compose_title=$(printf '%s' "$mapped" | cut -f3-)
                [[ -z "$compose_title" ]] && compose_title="$title"
                ;;
            *)
                echo "$response"
                return 0
                ;;
        esac
    else
        echo "$response"
        return 0
    fi

    # Wake knock with pointer payload.
    local from_tag="${from:-external}"
    local pointer="[inbox:${compose_id}] ${compose_title}"$'\n'"Open: k2 inbox read ${compose_id}"
    local wake_params="workspace=$(urlencode "$target")&text=$(urlencode "$pointer")&from=$(urlencode "$from_tag")&wake=true"
    local wake_resp
    wake_resp=$(cli_send cli_post_form "/cli/workspace/msg" "$wake_params")
    # Merge package + wake into one JSON line for the agent.
    if command -v python3 >/dev/null 2>&1; then
        python3 -c '
import json, sys
pkg = json.loads(sys.argv[1])
try:
    wake = json.loads(sys.argv[2])
except Exception:
    wake = {"success": False, "reason": "wake_parse_error", "hint": sys.argv[2][:200]}
pkg["wake"] = {
    "success": bool(wake.get("success")),
    "reason": wake.get("reason"),
    "hint": wake.get("hint"),
    "targetSessionId": wake.get("target_session_id") or wake.get("targetSessionId"),
    "woke": wake.get("woke"),
    "wakeMs": wake.get("wake_ms") or wake.get("wakeMs"),
    "attempts": wake.get("attempts"),
}
print(json.dumps(pkg, ensure_ascii=False, separators=(",", ":")))
if not wake.get("success"):
    reason = wake.get("reason") or "wake_failed"
    hint = wake.get("hint") or ""
    sys.stderr.write(
        f"note: package landed; wake notify failed ({reason})"
        + (f": {hint}" if hint else "")
        + "\n"
    )
' "$response" "$wake_resp" || {
            echo "$response"
            echo "note: package landed; wake notify failed (merge error)" >&2
        }
    else
        echo "$response"
    fi
    return 0
}

cmd_msg_signal_form() {
    # Forward to legacy cmd_signal which builds the AgentSignal wire payload.
    # `msg <ws> --signal <kind> [--payload <json>] [--from <name>]`
    local target="$1"
    shift
    local kind="msg" payload="{}" from="${K2SO_AGENT_NAME:-cli}" delivery="live"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --signal)  kind="$2"; shift 2 ;;
            --payload) payload="$2"; shift 2 ;;
            --from)    from="$2"; shift 2 ;;
            --inbox|--inbox-silent|--inbox-wake) delivery="inbox"; shift ;;
            *) shift ;;
        esac
    done
    # Delegate to cmd_signal — preserves wire-format consistency with
    # the now-deprecated `signal` top-level verb.
    local args=("$target" "$kind" "$payload" "--from" "$from")
    [[ "$delivery" == "inbox" ]] && args+=("--inbox")
    cmd_signal "${args[@]}"
}

# Wrapper that splits `k2 msg <ws> ...` into live / tray / --signal forms.
cmd_msg_v2() {
    # Detect tray flags or --signal anywhere in the args.
    local has_inbox_bare="false"
    local has_inbox_silent="false"
    local has_inbox_wake="false"
    local has_signal="false"
    for arg in "$@"; do
        case "$arg" in
            --inbox) has_inbox_bare="true" ;;
            --inbox-silent) has_inbox_silent="true" ;;
            --inbox-wake) has_inbox_wake="true" ;;
            --signal) has_signal="true" ;;
        esac
    done
    if [[ "$has_signal" == "true" ]]; then
        local target="$1"; shift
        cmd_msg_signal_form "$target" "$@"
        return $?
    fi
    # Bare --inbox without silent/wake → hard teaching error (exit 2).
    if [[ "$has_inbox_bare" == "true" && "$has_inbox_silent" != "true" && "$has_inbox_wake" != "true" ]]; then
        cmd_msg_inbox_bare_error
    fi
    if [[ "$has_inbox_silent" == "true" && "$has_inbox_wake" == "true" ]]; then
        echo "Error: pass exactly one of --inbox-silent or --inbox-wake" >&2
        exit 2
    fi
    if [[ "$has_inbox_silent" == "true" || "$has_inbox_wake" == "true" ]]; then
        local target="$1"; shift
        local mode="silent"
        [[ "$has_inbox_wake" == "true" ]] && mode="wake"
        local rest=()
        for arg in "$@"; do
            case "$arg" in
                --inbox|--inbox-silent|--inbox-wake) continue ;;
                *) rest+=("$arg") ;;
            esac
        done
        cmd_msg_inbox_form "$target" "$mode" "${rest[@]+"${rest[@]}"}"
        return $?
    fi
    # Bare --inbox alone already exited; any leftover bare with mode was stripped.
    cmd_msg "$@"
}

