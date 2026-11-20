# ── k2 push — phones and end-to-end encrypted pushes ────────────────────
# (prd-e2e-push-v1 rev 2 §4a, 0.46.3). Owner only: the owner token, over
# TCP (never the cell socket's passport). `off` is the instant rollback:
# the next push takes today's path, no app update needed.

_push_usage() {
    cat <<'EOF'
%include cli/text/help/push.txt
EOF
}

# _push_send <GET|POST> <path> [json body] [k=v GET params...] → prints
# "body\ncode". TCP with the owner token (the cell socket's passport is
# never the owner).
_push_send() {
    local method="$1" path="$2" payload="${3:-}" _K2_CFG="" kv out
    if [ "$method" = "GET" ]; then
        _k2_cfg_auth "$TOKEN"
        for kv in "${@:4}"; do
            _k2_cfg_add data-urlencode "$kv"
        done
        out="$(_k2_curl "$_K2_CFG" -s -G "${BASE_URL}${path}" \
            --connect-timeout 5 --max-time 30 \
            -w $'\n%{http_code}' 2>/dev/null; printf x)" || true
    else
        _k2_cfg_add url "${BASE_URL}${path}"
        _k2_cfg_auth "$TOKEN"
        _k2_cfg_add data-raw "$payload"
        out="$(_k2_curl "$_K2_CFG" -s -X POST \
            --connect-timeout 5 --max-time 30 \
            -H "Content-Type: application/json" \
            -w $'\n%{http_code}' 2>/dev/null; printf x)" || true
    fi
    out="${out%x}"
    # 0.46.16: a refusal's `permission` object closes the command.
    _permission_note "$out"
    printf '%s' "$out"
}

# _push_render <as_json> <view> <raw "body\ncode">
_push_render() {
    local as_json="$1" view="$2" raw="$3" code body
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then
        echo "k2 push: daemon unreachable" >&2
        exit 1
    fi
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$body"
        [ "$code" = "200" ] || exit "$(_sidecar_exit_for "$code")"
        return 0
    fi
    K2_PR_BODY="$body" K2_PR_CODE="$code" K2_PR_VIEW="$view" python3 -c '
import json, os, sys
raw, code, view = os.environ["K2_PR_BODY"], os.environ["K2_PR_CODE"], os.environ["K2_PR_VIEW"]
exits = {"400": 2, "413": 2, "403": 3, "404": 4}
try:
    d = json.loads(raw)
except Exception:
    print("k2 push: error: HTTP " + code + ": " + raw.strip()[:200], file=sys.stderr); sys.exit(1)
if code != "200" or d.get("ok") is False:
    e = d.get("error")
    if isinstance(e, dict):
        msg = (e.get("code") or "error") + (": " + e["hint"] if e.get("hint") else "")
    else:
        msg = str(e or ("HTTP " + code))
    print("k2 push: error: " + msg, file=sys.stderr)
    sys.exit(exits.get(code, 1))
if view == "devices":
    print("encrypted pushes:", d.get("mode"), "" if d.get("relaySealed") else "(relay not ready for encrypted pushes yet)")
    rows = d.get("devices") or []
    if not rows:
        print("no phones registered")
    for r in rows:
        e2e = r.get("e2e") or {}
        bits = [r.get("deviceId", ""), r.get("platform", ""), r.get("username", ""),
                "previews " + ("on" if r.get("previews") else "off"), r.get("status", "")]
        if r.get("tokenFingerprint"):
            bits.append("token " + str(r.get("tokenFingerprint")))
        if e2e:
            bits.append("key " + str(e2e.get("keyId")))
            bits.append("active" if e2e.get("active") else "inactive")
            if e2e.get("enforce"):
                bits.append("enforce")
            if e2e.get("override"):
                bits.append("override " + str(e2e.get("override")))
        print("  " + " · ".join(b for b in bits if b))
elif view == "mode":
    print("encrypted pushes:", d.get("mode"))
    if "relaySealed" in d:
        print("relay           :", "ready (s1)" if d.get("relaySealed") else "not ready: phones get today\x27s pushes")
elif view == "override":
    print("device", d.get("deviceId"), "override:", d.get("override") or "default (follows the server switch)")
elif view == "removed":
    print("removed device", d.get("deviceId"), "(its registration and push key)")
elif view == "log":
    import datetime
    rows = [(a.get("atMs") or 0, 0, i, "send", a) for i, a in enumerate(d.get("attempts") or [])]
    rows += [(e.get("atMs") or 0, 1, i, str(e.get("action")), e) for i, e in enumerate(d.get("events") or [])]
    rows.sort(key=lambda r: (r[0], r[1], r[2]))
    if not rows:
        print("no push attempts or phone changes since the daemon started" + (" for device " + str(d.get("device")) if d.get("device") else ""))
    for at_ms, src, _, what, a in rows:
        at = datetime.datetime.fromtimestamp(at_ms / 1000).strftime("%Y-%m-%d %H:%M:%S")
        if src == 1:
            bits = ["reason=" + str(a.get("reason")), "device=" + str(a.get("device")), "platform=" + str(a.get("platform")),
                    "login=" + str(a.get("login")), "session=" + str(a.get("session") or "-"), "token=" + str(a.get("token"))]
            if a.get("app"):
                bits.append("app=" + str(a.get("app")))
            if a.get("detail"):
                bits.append(str(a.get("detail")))
            print(at + "  " + what.ljust(6) + "  " + " ".join(bits))
            continue
        bits = ["device=" + str(a.get("device")), "kind=" + str(a.get("kind")), "lane=" + str(a.get("lane")),
                "bytes=" + str(a.get("bytes"))]
        if a.get("key"):
            bits.append("key=" + str(a.get("key")))
        bits.append("status=" + (str(a.get("status")) if a.get("status") is not None else "-"))
        if a.get("error"):
            bits.append("error=" + str(a.get("error")))
        bits.append("path=" + str(a.get("path")))
        bits.append("relay=" + str(a.get("relay")))
        if a.get("vendor"):
            bits.append("vendor=" + str(a.get("vendor")))
        bits.append("ms=" + str(a.get("ms")))
        print(at + "  " + "send".ljust(6) + "  " + " ".join(bits))
'
}

cmd_push() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _push_usage; return 0 ;;
    esac
    shift
    local as_json=0 args=() a raw dashes=0
    for a in "$@"; do
        case "$a" in
            --json) as_json=1 ;;
            -h|--help) [ "$dashes" = 1 ] || { _push_usage; return 0; }; args+=("$a") ;;
            --) dashes=1; args+=("$a") ;;
            *) args+=("$a") ;;
        esac
    done
    case "$sub" in
        devices|ls)
            [ "${#args[@]}" -eq 0 ] || { echo "usage: k2 push devices [--json]" >&2; exit 2; }
            raw="$(_push_send GET /cli/push/devices)"
            _push_render "$as_json" devices "$raw"
            ;;
        e2e)
            if [ "${#args[@]}" -eq 0 ]; then
                raw="$(_push_send GET /cli/push/e2e)"
            else
                [ "${#args[@]}" -eq 1 ] || { echo "usage: k2 push e2e [off|sealed-when-supported|sealed-only]" >&2; exit 2; }
                case "${args[0]}" in
                    off|sealed-when-supported|sealed-only) ;;
                    *) echo "usage: k2 push e2e off|sealed-when-supported|sealed-only (got '${args[0]}')" >&2; exit 2 ;;
                esac
                raw="$(_push_send POST /cli/push/e2e "{\"mode\":\"${args[0]}\"}")"
            fi
            _push_render "$as_json" mode "$raw"
            ;;
        device)
            if [ "${args[0]:-}" = "remove" ] || [ "${args[0]:-}" = "rm" ]; then
                [ "${#args[@]}" -eq 2 ] && [ -n "${args[1]}" ] || {
                    echo "usage: k2 push device remove <deviceId>" >&2; exit 2; }
                local rbody
                rbody="$(K2_PD_ID="${args[1]}" python3 -c 'import json,os; print(json.dumps({"deviceId": os.environ["K2_PD_ID"]}))')"
                raw="$(_push_send POST /cli/push/device/remove "$rbody")"
                _push_render "$as_json" removed "$raw"
                return 0
            fi
            [ "${args[0]:-}" = "e2e" ] && [ "${#args[@]}" -eq 3 ] || {
                echo "usage: k2 push device e2e <deviceId> off|sealed|default | k2 push device remove <deviceId>" >&2; exit 2; }
            case "${args[2]}" in
                off|sealed|default) ;;
                *) echo "usage: k2 push device e2e <deviceId> off|sealed|default (got '${args[2]}')" >&2; exit 2 ;;
            esac
            local body
            body="$(K2_PD_ID="${args[1]}" K2_PD_MODE="${args[2]}" python3 -c 'import json,os; print(json.dumps({"deviceId": os.environ["K2_PD_ID"], "mode": os.environ["K2_PD_MODE"]}))')"
            raw="$(_push_send POST /cli/push/device/e2e "$body")"
            _push_render "$as_json" override "$raw"
            ;;
        log)
            local dev="" i=0
            while [ "$i" -lt "${#args[@]}" ]; do
                case "${args[$i]}" in
                    --device)
                        i=$((i + 1))
                        dev="${args[$i]:-}"
                        [ -n "$dev" ] || { echo "usage: k2 push log [--json] [--device <id>]" >&2; exit 2; }
                        ;;
                    --device=*) dev="${args[$i]#--device=}" ;;
                    *) echo "usage: k2 push log [--json] [--device <id>] (got '${args[$i]}')" >&2; exit 2 ;;
                esac
                i=$((i + 1))
            done
            if [ -n "$dev" ]; then
                raw="$(_push_send GET /cli/push/log "" "device=${dev}")"
            else
                raw="$(_push_send GET /cli/push/log)"
            fi
            _push_render "$as_json" log "$raw"
            ;;
        *)
            echo "usage: unknown push verb '$sub' (devices|e2e|device e2e|device remove|log)" >&2
            exit 2
            ;;
    esac
}

