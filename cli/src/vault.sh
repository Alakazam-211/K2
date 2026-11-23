# ── k2 vault (prd-k2-vault-v1 §14) ──────────────────────────────────
# One encrypted store for passwords, tokens and keys. Values are read from
# stdin only (never an argument, never a URL). Agent verbs go over the
# session passport (cell socket / scoped token); the steps that are for
# people only (recovery key, vault password, reveal, key operations) go
# over ~/.k2/vault.sock from a terminal outside K2.

_vault_usage() {
    cat <<'EOF'
%include cli/text/help/vault.txt
EOF
}

# Exit code for a vault refusal.
_vault_exit_for() {
    case "$1" in
        200) echo 0 ;;
        400|413|422) echo 2 ;;
        401|403) echo 3 ;;
        404) echo 4 ;;
        # A waited-on card closed without a value: card_dismissed /
        # card_withdrawn (prd-popup-close-and-card-dismiss-v1 B5).
        410) echo 5 ;;
        *) echo 1 ;;
    esac
}

_vault_fail() {
    local status="$1" body="$2"
    printf '%s' "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    e = json.loads(raw).get("error") or {}
    if isinstance(e, dict):
        print("%s: %s" % (e.get("code", "error"), e.get("hint", "")), file=sys.stderr)
    else:
        print(str(e), file=sys.stderr)
except Exception:
    print(raw[:300] or "the daemon did not answer", file=sys.stderr)
'
    # Permission cards on: the refusal's closing line ("Ask with: …" or
    # "It can't be requested."), printed after the line above by the EXIT
    # trap. The vault family talks through _sidecar_send, which never
    # noted it (K2 Cores field report 2026-10-28). Cards off: no
    # `permission` object, nothing printed.
    _permission_note "$body"
    exit "$(_vault_exit_for "$status")"
}

# _vault_call GET|POST <route-suffix> [json|query] → _SC_BODY/_SC_STATUS, fails loud.
_vault_call() {
    local method="$1" route="$2" payload="${3:-}"
    _sidecar_split "$(_sidecar_send "$method" "/cli/vault/${route}" "$payload")"
    if [ "$_SC_STATUS" != "200" ]; then
        _vault_fail "$_SC_STATUS" "$_SC_BODY"
    fi
}

# k2 vault file|env-file --wait: one blocking call, then keep calling
# until the file is there, a person answered, or this CLI's own deadline.
_vault_wait_call() {
    local route="$1" body="$2" start=$SECONDS deadline="" card="" told=""
    while :; do
        _vault_call POST "$route" "$body"
        local waiting wsecs
        IFS=$'\t' read -r waiting card wsecs < <(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("%d\t%s\t%d" % (1 if v.get("waiting") else 0, v.get("card") or "", v.get("waitSecs") or 0))')
        [ "$waiting" = "1" ] || return 0
        if [ -z "$deadline" ]; then
            deadline=$((start + wsecs))
            echo "k2 vault: waiting for a person to save the card ($card)…" >&2
        fi
        local left=$((deadline - SECONDS))
        if [ "$left" -le 0 ]; then
            echo "no_answer: no answer yet; the card is still open (card $card)" >&2
            exit 3
        fi
        body="$(_vault_merge "$body" "$(python3 -c 'import json,sys; print(json.dumps({"wait": int(sys.argv[1]), "waitCard": sys.argv[2]}))' "$left" "$card")")"
    done
}

_vault_sock() { echo "${K2_HOME}/vault.sock"; }

# k2 vault sync [--json] (0.46.5, papercut 8): this running session's
# granted files into its $K2_SECRETS_DIR now; says what needs a restart.
_vault_sync() {
    local as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --system) echo "usage: only k2 vault list takes --system" >&2; exit 2 ;;
            *) echo "usage: k2 vault sync [--json]" >&2; exit 2 ;;
        esac
    done
    _vault_call POST sync '{}'
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("dir"):
    print("secrets dir: %s" % v["dir"])
for f in v.get("delivered") or []:
    print("delivered  %s" % f)
for f in v.get("present") or []:
    print("present    %s" % f)
for r in v.get("restart") or []:
    print("restart    %s" % r)
for s in v.get("skipped") or []:
    print("skipped    %s" % s)
print(v.get("hint") or "")
sys.exit(1 if v.get("skipped") else 0)'
}

# People-only routes: refuse inside a K2 terminal, then talk to vault.sock.
_vault_person_guard() {
    if _envn_any HOOK_SOCK HOOK_TOKEN; then
        echo "owner_only: this is for a person, from your own terminal or SSH login, not a K2 terminal." >&2
        echo "Agents: don't run it; tell your owner." >&2
        exit 3
    fi
    if [ ! -S "$(_vault_sock)" ]; then
        echo "vault_unavailable: $(_vault_sock) isn't there (is the K2 daemon running on this machine?)" >&2
        exit 1
    fi
}

# _vault_sock_call POST <route> <json> → _SC_BODY/_SC_STATUS (no exit).
_vault_sock_call() {
    # $4: the call's time limit in seconds (180; the phone confirm waits up
    # to 5 minutes for an answer).
    local method="$1" route="$2" payload="${3:-}" tty="0" out maxt="${4:-180}"
    [ -t 0 ] && [ -t 1 ] && tty="1"
    [ -t 0 ] && [ -t 2 ] && tty="1"
    if [ "$method" = "GET" ]; then
        out="$(_k2_curl "" -s -G --unix-socket "$(_vault_sock)" "http://localhost/cli/vault/${route}" \
            -H "X-K2-Tty: ${tty}" --connect-timeout 5 --max-time "$maxt" -w $'\n%{http_code}' 2>/dev/null)" || out=""
    else
        out="$(printf '%s' "$payload" | _k2_curl "" -s -X POST --unix-socket "$(_vault_sock)" "http://localhost/cli/vault/${route}" \
            -H "X-K2-Tty: ${tty}" -H "Content-Type: application/json" --data-binary @- \
            --connect-timeout 5 --max-time "$maxt" -w $'\n%{http_code}' 2>/dev/null)" || out=""
    fi
    _sidecar_split "$out"
}

# Read a value from stdin (prompt without echo on a terminal) and print a
# JSON body with it as valueB64. The value travels stdin→python→stdout→
# curl stdin; never argv.
_vault_body_with_value() {
    local base_json="$1"
    if [ -t 0 ]; then
        local v=""
        _k2_read_hidden v 'Value (not shown): '
        [ -n "$v" ] || { echo "usage: no value on stdin" >&2; exit 2; }
        printf '%s' "$v" | python3 -c '
import base64, json, sys
b = json.loads(sys.argv[1]); v = sys.stdin.buffer.read()
b["valueB64"] = base64.b64encode(v).decode()
print(json.dumps(b))' "$base_json"
        v=""
    else
        python3 -c '
import base64, json, sys
b = json.loads(sys.argv[1]); v = sys.stdin.buffer.read()
if v.endswith(b"\n") and v.count(b"\n") == 1:
    v = v[:-1]
if not v:
    print("usage: no value on stdin", file=sys.stderr); sys.exit(2)
b["valueB64"] = base64.b64encode(v).decode()
print(json.dumps(b))' "$base_json"
    fi
}

# Scope flag → JSON fields.
_vault_scope_json() {
    local s="${1:-}"
    python3 -c '
import json, sys
s = sys.argv[1]
o = {}
if s.startswith("ws:") or s.startswith("workspace:"):
    o["scope"] = "workspace"; o["workspace"] = s.split(":", 1)[1]
elif s in ("server", "session", "workspace"):
    o["scope"] = s
elif s.startswith("session:"):
    o["scope"] = "session"; o["session"] = s.split(":", 1)[1]
elif s.startswith("engine:"):
    # K2s own secrets by engine (k2 vault reveal sew-launch-key --scope engine:sew).
    o["scope"] = s
elif s:
    print("usage: --scope ws:<name>|server|session|engine:<engine>", file=sys.stderr); sys.exit(2)
print(json.dumps(o))' "$s"
}

# What a destroy, disable or revoke did to delivered files, and can't undo.
_vault_print_recall() {
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
words = json.load(sys.stdin).get("recall")
if words:
    print(words)'
}

_vault_merge() {
    python3 -c '
import json, sys
o = {}
for a in sys.argv[1:]:
    o.update(json.loads(a))
print(json.dumps(o))' "$@"
}

_vault_render_list() {
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
items = v.get("secrets") or []
hidden = v.get("systemHidden")
if not items:
    print(("none of yours to show" if hidden else "no secrets yet") + " (k2 vault set NAME < value, or ask a person with k2 thread secret)")
for m in items:
    sc = m.get("scope") or {}
    where = "server" if sc.get("kind") == "server" else ("ws:" + (m.get("workspace") or sc.get("id","")) if sc.get("kind") == "workspace" else sc.get("kind","") + ":" + sc.get("id","")[:8])
    hosts = ",".join(h.split("://",1)[-1].rsplit(":",1)[0] for h in (m.get("hosts") or [])) or "-"
    see = " (agent can see it)" if m.get("agentCanSee") else ""
    ns = " [system]" if m.get("namespace") == "system" else ""
    st = "" if m.get("status") == "active" else " [" + m.get("status","") + "]"
    sg = (m.get("inject") or {}).get("sign")
    st += (" [signs: " + sg.get("scheme", "") + "]") if sg else ""
    st += " [expired: sign in again]" if (m.get("oauth") or {}).get("state") == "expired" else ""
    print("%-28s %-18s %-10s %-8s v%-3s %s%s%s%s" % (m.get("name"), where, m.get("kind"), m.get("defaultDelivery"), m.get("latestVersion"), hosts, see, ns, st))
if hidden:
    print("%s K2 system secrets hidden from you: %s" % (hidden.get("count"), hidden.get("why")))
'
}

# The signer JSON for set/sign (PRD vault-gaps item 2), or "" for none.
# Args: scheme header encoding prefix prefix_set ts_header template
# template_set value value_set.
_vault_sign_json() {
    python3 -c '
import json, sys
scheme, header, enc, prefix, prefix_set, tsh, tpl, tpl_set, val, val_set, token_url, client_auth = sys.argv[1:13]
if not scheme:
    if header or enc or tsh or prefix_set == "1" or tpl_set == "1" or val_set == "1" or token_url or client_auth:
        print("usage: --sign-* and --oauth-* options need --sign ovh|hmac-sha256|hmac-sha256-ts|oauth-refresh", file=sys.stderr); sys.exit(2)
    print(""); sys.exit(0)
o = {"scheme": scheme}
if token_url: o["tokenUrl"] = token_url
if client_auth: o["clientAuth"] = client_auth
if header: o["header"] = header
if enc: o["encoding"] = enc
if prefix_set == "1": o["prefix"] = prefix
if tsh: o["tsHeader"] = tsh
if tpl_set == "1": o["template"] = tpl
if val_set == "1": o["valueFmt"] = val
print(json.dumps(o))' "$@"
}

_vault_render_status() {
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if not v.get("open", True) and v.get("error"):
    e = v["error"]; print("vault: closed — %s: %s" % (e.get("code"), e.get("hint"))); sys.exit(0)
print("vault:          open")
print("key:            %s (%s)" % (v.get("keyHomeLabel"), v.get("keyHome")))
print("rollback guard: %s" % v.get("rollbackGuard"))
print("recovery key:   %s" % v.get("recovery"))
print("vault password: %s" % ("set" if v.get("confirmPassword") else "not set"))
px = v.get("proxy") or {}
print("proxy:          %s" % (("127.0.0.1:%s%s" % (px["port"], "  (%s)" % px["hint"] if px.get("moved") else "")) if px.get("port") else (px.get("hint") or "not listening")))
names = {"box": "box key (this machine)", "recovery": "recovery key", "backup": "backup key"}
hs = v.get("holders") or []
if hs:
    print("key holders:    (who can open the vault)")
    for h in hs:
        print("  %-22s %s%s" % (names.get(h.get("kind"), "%s key" % h.get("kind")), h.get("fingerprint") or "", ("  added %s" % h["createdAt"]) if h.get("createdAt") else ""))
c = v.get("counts") or {}
print("secrets:        %s yours, %s K2 system, %s shares, %s pending cards" % (c.get("user"), c.get("system"), c.get("grants"), c.get("pendingRequests")))
if v.get("tamper") != "ok":
    print("PROBLEM:        %s" % v.get("tamper"))
if v.get("newerRows"):
    print("NEEDS UPDATE:   %s" % v.get("newerRows"))
if v.get("rollback"):
    print("PROBLEM:        vault.db looks older than expected; the owner confirms in Settings → Vault")
if v.get("movedOut"):
    m = v["movedOut"]
    print("FROZEN:         exported for a box move at %s (move key %s): reads work, writes wait. Called off? k2 migrate export-secrets --unfreeze" % (m.get("at"), m.get("moveKey")))
print()
print(v.get("honest", ""))
if v.get("recoveryHint"):
    print(); print(v["recoveryHint"])
'
}

# Add a confirm token to a JSON body ($1) as field $3 (confirmToken). The
# token reaches python on stdin, never argv or the environment (ps shows a
# command line to every account on this computer, and the environment to this
# user's processes). An empty token sets null.
_with_confirm_token() {
    local body="${1:-}" tok="${2:-}" field="${3:-confirmToken}"
    [ -n "$body" ] || body='{}'
    printf '%s\n%s\n%s' "$field" "$tok" "$body" | python3 -c '
import json, sys
field = sys.stdin.readline().rstrip("\n")
tok = sys.stdin.readline().rstrip("\n")
b = json.loads(sys.stdin.read())
b[field] = tok or None
print(json.dumps(b))'
}

# Raise the phone card for this confirm (0.46.17): the owner answers it on
# their phone with the confirm sheet, the first answer (this computer's or
# the phone's) wins. $1 action, $2 secretId, $3 label, $4 extra JSON (a
# share's `grant`). The card goes on the Thread of the workspace this
# terminal is in, or K2_VAULT_CONFIRM_THREAD's. Sets _VPC_REQ and _VPC_LINE
# (where to answer); on a refusal prints why (and sets neither).
_vault_phone_raise() {
    local action="$1" secret_id="${2:-}" label="${3:-}" extra="${4:-}"
    local where thread
    _envn_into where PROJECT_PATH "$PWD"
    _envn_into thread VAULT_CONFIRM_THREAD ""
    _VPC_REQ=""; _VPC_LINE=""
    _vault_sock_call POST "confirm/start" "$(printf '%s' "$extra" | python3 -c '
import json, sys
o = {"action": sys.argv[1], "secretId": sys.argv[2] or None, "label": sys.argv[3], "phone": True, "workspacePath": sys.argv[4]}
if sys.argv[5]: o["thread"] = sys.argv[5]
o.update(json.loads(sys.stdin.read() or "{}"))
print(json.dumps(o))' "$action" "$secret_id" "$label" "$where" "$thread")"
    if [ "$_SC_STATUS" != "200" ]; then
        printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
try:
    e = json.load(sys.stdin).get("error") or {}
except Exception:
    e = {}
print("(no phone card: %s)" % (e.get("hint") or e.get("code") or "the daemon did not answer"), file=sys.stderr)'
        return 0
    fi
    IFS=$'\t' read -r _VPC_REQ _VPC_LINE < <(printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
p = json.load(sys.stdin).get("phone") or {}
why = (" — " + p["thread"]) if p.get("thread") else ""
print("%s\tcard %s on %s'"'"'s Thread%s (%s)" % (p.get("request", ""), p.get("request", ""), p.get("addr", ""), why, p.get("text", "")))')
}

# Ask the daemon for a confirm token over vault.sock (Touch ID sheet on a
# Mac, else the vault password typed here). 0.46.17, the owner's phone
# (`phoneOffered`), only where this computer's own prompt can't be seen or
# isn't answered: over SSH the phone card goes up beside the prompt (the
# first answer wins); otherwise the prompt alone, and the phone card when it
# gets no answer (nobody at the screen, or it timed out); with no check here
# at all (`phoneOnly`, a Linux server with no vault password) the phone
# alone. K2_VAULT_CONFIRM_METHOD=phone: the phone only; =local: never the
# phone. $4: extra JSON for the phone card (a share's `grant`). Prints the token. Some steps
# take this computer's own user check instead (the daemon's presence
# actions: vault-password-set, recovery-setup, backup-export-initial,
# box-move-export, backup-key-add, owner-login): the Mac's sheet, Windows
# Hello, on a Linux server an owner login's sign-in password, or for a
# server's first owner login, this terminal (outside K2).
_vault_confirm_token() {
    local action="$1" secret_id="${2:-}" label="${3:-}" phone_extra="${4:-}"
    _vault_sock_call POST "confirm/start" "$(python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None}))' "$action" "$secret_id")"
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    local methods login
    methods="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(" ".join(json.load(sys.stdin).get("methods") or []))')"
    login="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("login") or "")')"
    local body
    local first_setup=0
    case "$action" in vault-password-set|recovery-setup|backup-export-initial|box-move-export|backup-key-add|owner-login) first_setup=1 ;; esac
    local text
    text="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("confirmText") or "")')"
    [ "$first_setup" = 1 ] && [ -n "$text" ] && echo "You are approving: $text" >&2
    # K2_VAULT_CONFIRM_METHOD=vault-password: use the vault password even on
    # a Mac with Touch ID (a headless run, e.g. the two-server harness). The
    # same check as always: the daemon verifies the vault password. Not for
    # the first-setup steps (the vault password isn't their check).
    if [ "$first_setup" = 0 ] && [ "$(_envn VAULT_CONFIRM_METHOD)" = "vault-password" ]; then
        case " $methods " in
            *" vault-password "*) methods="vault-password" ;;
            *) echo "no_confirm_method: K2_VAULT_CONFIRM_METHOD=vault-password, but no vault password is set (k2 vault confirm-password set)" >&2; exit 3 ;;
        esac
    fi
    local phone_ok phone_only want_m maxt=180 over_ssh=""
    _VPC_REQ=""; _VPC_LINE=""
    phone_ok="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print("1" if json.load(sys.stdin).get("phoneOffered") else "")')"
    phone_only="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print("1" if json.load(sys.stdin).get("phoneOnly") else "")')"
    want_m="$(_envn VAULT_CONFIRM_METHOD)"
    { [ -n "${SSH_CONNECTION:-}" ] || [ -n "${SSH_TTY:-}" ]; } && over_ssh=1
    if [ "$want_m" = "phone" ] || { [ "$phone_only" = "1" ] && [ "$phone_ok" = "1" ] && [ -z "$want_m" ]; }; then
        [ "$phone_ok" = "1" ] || { echo "no_phone_confirm: this step can't be confirmed from a phone here (the owner's own terminal, permission cards on, and not an owner-login change); unset K2_VAULT_CONFIRM_METHOD to confirm at this computer" >&2; exit 3; }
        methods="phone"
    fi
    # The phone goes with this computer's own prompt only; a typed password
    # (the vault password, an owner login's) stays as it was.
    # K2_VAULT_CONFIRM_METHOD=local (or =vault-password): never the phone.
    [ -n "$want_m" ] && [ "$want_m" != "phone" ] && phone_ok=""
    case " $methods " in
        *" phone "*)
            _vault_phone_raise "$action" "$secret_id" "$label" "$phone_extra"
            [ -n "$_VPC_REQ" ] || { echo "no_phone_confirm: the phone card could not be raised (above)" >&2; exit 3; }
            echo "Approve on your phone: ${_VPC_LINE}. Waiting for your phone… (5 minutes)" >&2
            maxt=330
            body="$(python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": "phone", "label": sys.argv[3], "phoneRequest": sys.argv[4]}))' "$action" "$secret_id" "$label" "$_VPC_REQ")"
            ;;
        *" local-auth "*|*" windows-hello "*)
            local lm="local-auth" where_word="this Mac (Touch ID or your login password)"
            case " $methods " in *" local-auth "*) ;; *) lm="windows-hello"; where_word="this PC (Windows Hello or your Windows password)" ;; esac
            _VPC_REQ=""
            # Over SSH the prompt shows on a screen you can't see: the phone
            # card goes up beside it.
            [ "$phone_ok" = "1" ] && [ -n "$over_ssh" ] && _vault_phone_raise "$action" "$secret_id" "$label" "$phone_extra"
            if [ -n "$_VPC_REQ" ]; then
                echo "Approve on ${where_word}, or on your phone: ${_VPC_LINE}. Waiting for your phone… (5 minutes; the first answer wins)" >&2
                [ "$lm" = "windows-hello" ] && echo "(If you answer on the phone, the Windows Hello prompt on this PC may stay open: dismiss it.)" >&2
                maxt=330
            else
                echo "Approve on ${where_word}…" >&2
            fi
            body="$(python3 -c '
import json, sys
o = {"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": sys.argv[4], "label": sys.argv[3]}
if sys.argv[5]: o["phoneRequest"] = sys.argv[5]
print(json.dumps(o))' "$action" "$secret_id" "$label" "$lm" "$_VPC_REQ")"
            ;;
        *" terminal "*)
            # A server's first owner login: this terminal, outside K2, is
            # the check (the daemon verifies it on the vault socket).
            echo "Confirming from this terminal (this server has no owner login yet)…" >&2
            body="$(python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": "terminal"}))' "$action" "$secret_id")"
            ;;
        *" owner-login-password "*)
            local pw=""
            if [ -z "$login" ]; then
                printf 'Which owner login on this server (k2 users lists them): ' >&2
                IFS= read -r login || login=""
            fi
            _k2_read_hidden pw "Sign-in password for owner login $login: "
            body="$(printf '%s' "$pw" | python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": "owner-login-password", "username": sys.argv[3], "password": sys.stdin.read()}))' "$action" "$secret_id" "$login")"
            pw=""
            ;;
        *" vault-password "*)
            local pw=""
            _k2_read_hidden pw 'Vault password: '
            body="$(printf '%s' "$pw" | python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": "vault-password", "password": sys.stdin.read()}))' "$action" "$secret_id")"
            pw=""
            ;;
        *)
            echo "no_confirm_method: set a vault password first: k2 vault confirm-password set" >&2
            exit 3
            ;;
    esac
    _vault_sock_call POST "confirm/finish" "$body" "$maxt"
    # Nobody answered this computer's own prompt (no one at its screen, or
    # it timed out): ask the phone instead, once.
    if [ "$_SC_STATUS" = "409" ] && [ "$phone_ok" = "1" ] && [ -z "$_VPC_REQ" ] && [ "$methods" != "phone" ] \
        && printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; sys.exit(0 if (json.load(sys.stdin).get("error") or {}).get("code") in ("confirm_unanswered", "vault_presence_required") else 1)'; then
        echo "Nobody answered on this computer." >&2
        _vault_phone_raise "$action" "$secret_id" "$label" "$phone_extra"
        [ -n "$_VPC_REQ" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
        echo "Approve on your phone: ${_VPC_LINE}. Waiting for your phone… (5 minutes)" >&2
        body="$(python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": "phone", "label": sys.argv[3], "phoneRequest": sys.argv[4]}))' "$action" "$secret_id" "$label" "$_VPC_REQ")"
        _vault_sock_call POST "confirm/finish" "$body" 330
    fi
    body=""
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("confirmedOn") == "phone":
    print("Confirmed on your phone.", file=sys.stderr)
print(v["token"])'
}

# k2 vault fetch — K2 makes the HTTPS call and adds the secret where its
# creator said; you never see the value. Echoes come back as [k2v:NAME].
_vault_fetch() {
    # -h/--help anywhere prints the fetch help and sends nothing, even where
    # a value flag (--json, -d, -H, -o, -X, --dst, --fmt, --max-bytes,
    # --timeout) would take it as its value (0.46.17: `k2 vault fetch URL
    # --secret A --json --help` sent the fetch).
    if _wants_help "$@"; then set -- --help; fi
    local url="" method="" max_bytes="" timeout="" include=0 out="" body_inline="" body_file="" ctype="" as_json=0
    local secrets=() headers=() dst="" fmt="" use_scope="" use_ws=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --secret|-s) [ -n "${2:-}" ] || { echo "usage: --secret NAME" >&2; exit 2; }; secrets+=("$2"); shift 2 ;;
            --scope)
                case "${2:-}" in
                    session|server) use_scope="$2" ;;
                    ws:?*) use_scope="workspace"; use_ws="${2#ws:}" ;;
                    *) echo "usage: --scope session|server|ws:<name> (this session's own secret of that name, or one shared with you from there)" >&2; exit 2 ;;
                esac
                shift 2 ;;
            -X|--method) [ -n "${2:-}" ] || { echo "usage: -X METHOD" >&2; exit 2; }; method="$2"; shift 2 ;;
            -H|--header) [ -n "${2:-}" ] || { echo "usage: -H 'Name: value'" >&2; exit 2; }; headers+=("$2"); shift 2 ;;
            -d|--data)
                [ -n "${2:-}" ] || { echo "usage: -d 'text' | -d @file" >&2; exit 2; }
                if [ "${2#@}" != "$2" ]; then body_file="${2#@}"; else body_inline="$2"; fi
                shift 2 ;;
            --json)
                [ -n "${2:-}" ] || { echo "usage: --json @file | --json 'text'" >&2; exit 2; }
                if [ "${2#@}" != "$2" ]; then body_file="${2#@}"; else body_inline="$2"; fi
                ctype="application/json"; shift 2 ;;
            --max-bytes) _need_value "$@"; max_bytes="${2:-}"; shift 2 ;;
            --timeout) _need_value "$@"; timeout="${2:-}"; shift 2 ;;
            --include-headers|-i) include=1; shift ;;
            -o|--output) [ -n "${2:-}" ] || { echo "usage: -o file" >&2; exit 2; }; out="$2"; shift 2 ;;
            --dst) _need_value "$@"; dst="${2:-}"; shift 2 ;;
            --fmt) _need_value "$@"; fmt="${2:-}"; shift 2 ;;
            --json-out) as_json=1; shift ;;
            --help|-h)
                cat <<'EOF'
%include cli/text/help/vault/fetch.txt
EOF
                return 0 ;;
            --system) echo "usage: only k2 vault list takes --system" >&2; exit 2 ;;
            -*) echo "usage: unknown fetch flag '$1' (k2 vault fetch --help)" >&2; exit 2 ;;
            *) [ -z "$url" ] || { echo "usage: one URL" >&2; exit 2; }; url="$1"; shift ;;
        esac
    done
    [ -n "$url" ] || { echo "usage: k2 vault fetch --secret NAME URL" >&2; exit 2; }
    [ "${#secrets[@]}" -gt 0 ] || { echo "usage: name the secret: --secret NAME" >&2; exit 2; }
    [ -z "$ctype" ] || headers+=("Content-Type: $ctype")
    [ -z "$body_file" ] || body_file="$(cli_abs_path "$body_file")"
    [ -z "$out" ] || out="$(cli_abs_path "$out")"
    local req
    req="$(printf '%s' "$body_inline" | V_USE_SCOPE="$use_scope" V_USE_WS="$use_ws" python3 -c '
import base64, json, os, sys
url, method, mb, to, inc, out, bfile, dst, fmt = sys.argv[1:10]
rest = sys.argv[10:]
i = rest.index("--")
secrets, headers = rest[:i], rest[i+1:]
o = {"url": url, "secrets": [], "headers": []}
for s in secrets:
    e = {"name": s}
    if dst: e["dst"] = dst
    if fmt: e["fmt"] = fmt
    if os.environ.get("V_USE_SCOPE"): e["scope"] = os.environ["V_USE_SCOPE"]
    if os.environ.get("V_USE_WS"): e["workspace"] = os.environ["V_USE_WS"]
    o["secrets"].append(e)
for h in headers:
    if ":" not in h:
        print("usage: -H takes Name: value", file=sys.stderr); sys.exit(2)
    k, v = h.split(":", 1)
    o["headers"].append([k.strip(), v.strip()])
if method: o["method"] = method
if mb: o["maxBytes"] = int(mb)
if to: o["timeoutSecs"] = int(to)
if inc == "1": o["includeHeaders"] = True
if out: o["out"] = out
if bfile: o["bodyFile"] = bfile
data = sys.stdin.buffer.read()
if data: o["bodyB64"] = base64.b64encode(data).decode()
print(json.dumps(o))' "$url" "$method" "$max_bytes" "$timeout" "$include" "$out" "$body_file" "$dst" "$fmt" "${secrets[@]}" -- "${headers[@]+"${headers[@]}"}")" || exit 2
    _vault_call POST fetch "$req"
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
    printf '%s' "$_SC_BODY" | python3 -c '
import base64, json, sys
v = json.load(sys.stdin)
inc = sys.argv[1] == "1"
if inc:
    print("HTTP %s" % v.get("status"))
    for k, val in v.get("headers") or []:
        print("%s: %s" % (k, val))
    print()
if v.get("out"):
    print("wrote %s bytes to %s (status %s)" % (v.get("bytes"), v["out"], v.get("status")), file=sys.stderr)
elif v.get("body") is not None:
    sys.stdout.write(v["body"])
elif v.get("bodyB64"):
    sys.stdout.buffer.write(base64.b64decode(v["bodyB64"]))
if v.get("truncated"):
    print("\n(truncated at the size limit; --max-bytes or -o)", file=sys.stderr)
st = int(v.get("status") or 0)
if st >= 400 and not inc:
    print("(HTTP %d)" % st, file=sys.stderr)
' "$include"
}

_vault_handoff_usage() {
    cat <<'EOF'
%include cli/text/help/vault/send.txt
EOF
}

# Exit code for a handoff answer: HTTP status + error code (PRD R6).
_vault_handoff_exit() {
    local status="$1" code="$2"
    case "$code" in
        pending|held|declined|not_connected|send_not_allowed|handoff_person_only|confirm_required|owner_only|too_many_pending|rate_limited|vault_not_yours|vault_owner_confirm_required|not_yours) echo 3; return ;;
        peer_too_old|federation_off|never_sendable|send_by_name_only|use_vault_set|note_has_value|note_too_long|value_too_large|same_workspace|name_taken|not_pending|not_unsendable|no_wildcard|bad_sender|bad_ttl|bad_name|bad_address|to_required|name_required|bad_json|bad_action|workspace_required|secret_not_active|secret_expired) echo 2; return ;;
    esac
    case "$status" in
        200) echo 0 ;;
        400|413|422) echo 2 ;;
        401|403|429) echo 3 ;;
        404) echo 4 ;;
        *) echo 1 ;;
    esac
}

_vault_handoff_code() {
    printf '%s' "$1" | python3 -c '
import json, sys
try:
    v = json.loads(sys.stdin.read())
    e = v.get("error")
    print((e.get("code") if isinstance(e, dict) else v.get("code")) or "")
except Exception:
    print("")'
}

_vault_handoff_fail() {
    local status="$1" body="$2" code
    code="$(_vault_handoff_code "$body")"
    printf '%s' "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    v = json.loads(raw)
    e = v.get("error")
    if isinstance(e, dict):
        print("%s: %s" % (e.get("code", "error"), e.get("hint", "")), file=sys.stderr)
    else:
        print(str(e or raw[:300]), file=sys.stderr)
except Exception:
    print(raw[:300] or "the daemon did not answer", file=sys.stderr)'
    # Permission cards on: the refusal's closing line (`Ask with: k2
    # permission request vault.send …`, 0.46.11), printed by the EXIT trap
    # after the line above. Cards off: no `permission` object, nothing.
    _permission_note "$body"
    exit "$(_vault_handoff_exit "$status" "$code")"
}

# Inside a K2 terminal: the passport. Outside: the person channel
# (vault.sock) when it's there, else the owner token.
_vault_handoff_call() {
    local method="$1" route="$2" payload="${3:-}"
    if ! _envn_any HOOK_SOCK HOOK_TOKEN && [ -S "$(_vault_sock)" ]; then
        if [ "$method" = "GET" ] && [ -n "$payload" ]; then
            _vault_sock_call GET "${route}?${payload}"
        else
            _vault_sock_call "$method" "$route" "$payload"
        fi
    else
        _sidecar_split "$(_sidecar_send "$method" "/cli/vault/${route}" "$payload")"
    fi
}

_vault_handoff() {
    local sub="$1"; shift
    if _wants_help "$@"; then _vault_handoff_usage; return 0; fi
    local to="" as_n="" note="" ttl="" reply="" scope="" workspace="" via="" kind="" hosts="" hdelivery="" as_json=0 stdin=0 replace=0 always=0
    local pos=() seen=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; seen+=("$1"); shift ;;
            --stdin) stdin=1; seen+=("$1"); shift ;;
            --replace) replace=1; seen+=("$1"); shift ;;
            --always) always=1; seen+=("$1"); shift ;;
            --to|--as|--note|--ttl|--reply|--scope|--workspace|--via|--kind|--for|--delivery)
                [ $# -ge 2 ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --to) to="$2" ;; --as) as_n="$2" ;; --note) note="$2" ;; --ttl) ttl="$2" ;; --reply) reply="$2" ;;
                    --scope) scope="$2" ;; --workspace) workspace="$2" ;; --via) via="$2" ;; --kind) kind="$2" ;; --for) hosts="$2" ;;
                    --delivery) hdelivery="$2" ;;
                esac
                seen+=("$1")
                shift 2 ;;
            --value|--value=*|--secret-value*)
                echo "value_on_argv: values are read from stdin, never an argument" >&2; exit 2 ;;
            --system) echo "usage: only k2 vault list takes --system" >&2; exit 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 vault send --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    # One parser, many verbs: each verb takes only the flags it uses
    # (K2, 2026-11-16). Any other is refused here, before a request goes
    # out, never parsed and dropped.
    local takes="" verb_words="$sub"
    case "$sub" in
        send) takes="--to --as --note --ttl --reply --scope --workspace --via --kind --for --delivery --stdin --json" ;;
        incoming)
            case "${pos[0]:-}" in
                accept) takes="--as --replace --always --json"; verb_words="incoming accept" ;;
                decline) takes="--json"; verb_words="incoming decline" ;;
                *) takes="--workspace --json" ;;
            esac ;;
        sent|asks|accept-from) takes="--workspace --json" ;;
        ask) takes="--note --workspace --json" ;;
        unsend) takes="--json" ;;
        send-allow)
            case "${pos[0]:-list}" in
                add|remove) takes="--to --workspace --json"; verb_words="send-allow ${pos[0]}" ;;
                *) takes="--workspace --json" ;;
            esac ;;
    esac
    local f
    for f in "${seen[@]+"${seen[@]}"}"; do
        case " $takes " in
            *" $f "*) ;;
            *)
                local shown="${takes/ --via/}"
                echo "usage: k2 vault $verb_words doesn't take $f (it takes: ${shown// /, }; k2 vault send --help)" >&2; exit 2 ;;
        esac
    done
    if [ "$sub" = "send" ]; then
        # A named send keeps the secret's own sites; --for is for a value a
        # person types in (--stdin), which has no secret to pick by scope.
        if [ "$stdin" = "0" ] && [ -n "$hosts" ]; then
            echo "usage: --for goes with --stdin (a named send keeps the secret's own sites; k2 vault hosts NAME to change them)" >&2; exit 2
        fi
        if [ "$stdin" = "1" ]; then
            [ -z "$scope" ] || { echo "usage: --scope picks a secret in the vault; --stdin sends a value you type in" >&2; exit 2; }
            [ -z "$workspace" ] || { echo "usage: --workspace picks a secret in the vault; --stdin sends a value you type in" >&2; exit 2; }
        fi
        # In a K2 terminal the daemon sends the agent's own workspace's
        # secret, whatever --scope or --workspace says: refuse, don't drop.
        if _envn_any HOOK_SOCK HOOK_TOKEN; then
            [ -z "$scope" ] || { echo "usage: --scope is for people; an agent sends its own workspace's secret (drop --scope)" >&2; exit 2; }
            [ -z "$workspace" ] || { echo "usage: --workspace is for people; an agent sends its own workspace's secret (drop --workspace)" >&2; exit 2; }
        fi
    fi
    local q=""
    [ -n "$workspace" ] && q="workspace=$(urlencode "$workspace")"
    case "$sub" in
        send)
            local name="${pos[0]:-}"
            [ -n "$name" ] && [ -n "$to" ] || { echo "usage: k2 vault send NAME --to <agent>|<agent>::<host> [--note …]" >&2; exit 2; }
            [ "${#pos[@]}" -eq 1 ] || { echo "value_on_argv: name the secret only; values are read from stdin" >&2; exit 2; }
            local body
            body="$(python3 -c '
import json, re, sys
name, to, as_n, note, ttl, reply, scope, via, kind, hosts, delivery, workspace = sys.argv[1:13]
o = {"name": name, "to": to}
if as_n: o["as"] = as_n
if note: o["note"] = note
if ttl:
    m = re.fullmatch(r"(\d+)([smhdw]?)", ttl.strip())
    if not m:
        print("usage: --ttl 7d | 12h | 3600", file=sys.stderr); sys.exit(2)
    o["ttlSecs"] = int(m.group(1)) * {"": 1, "s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}[m.group(2)]
if reply: o["replyTo"] = reply
if scope.startswith("ws:") or scope.startswith("workspace:"):
    o["scope"] = "workspace"; o["workspace"] = scope.split(":", 1)[1]
elif scope:
    o["scope"] = scope
# --workspace WS (a person): the secret of that workspace, the same as --scope ws:WS.
if workspace:
    if o.get("workspace", workspace) != workspace or (scope and o.get("scope") != "workspace"):
        print("usage: --workspace %s and --scope %s name different secrets; give one" % (workspace, scope), file=sys.stderr); sys.exit(2)
    o["workspace"] = workspace
if via: o["via"] = via
if kind: o["kind"] = kind
if hosts: o["hosts"] = [h.strip() for h in hosts.split(",") if h.strip()]
if delivery: o["delivery"] = delivery
print(json.dumps(o))' "$name" "$to" "$as_n" "$note" "$ttl" "$reply" "$scope" "$via" "$kind" "$hosts" "$hdelivery" "$workspace")" || exit 2
            if [ "$stdin" = "1" ]; then
                # A value a person types in: the person channel only (H15).
                _vault_person_guard
                body="$(_vault_body_with_value "$body")" || exit 2
                _vault_sock_call POST send "$body"
            else
                _vault_handoff_call POST send "$body"
            fi
            if [ "$_SC_STATUS" = "403" ] && [ "$(_vault_handoff_code "$_SC_BODY")" = "confirm_required" ] && ! _envn_any HOOK_SOCK HOOK_TOKEN && [ -S "$(_vault_sock)" ]; then
                # A person gave this secret: confirm sending it to another server.
                local sid tok
                sid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("secretId") or "")')"
                tok="$(_vault_confirm_token handoff-send "$sid" "send $name to $to")" || exit 3
                body="$(_with_confirm_token "$body" "$tok")"
                _vault_sock_call POST send "$body"
                tok=""
            fi
            body=""
            [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; else
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
st = v.get("status")
line = "sent %s to %s (%s): %s" % (v.get("name"), v.get("to"), v.get("deliveryId"), st)
if st == "landed" and v.get("ref"): line += " → " + v["ref"]
if v.get("landedName") and v.get("landedName") != v.get("name"): line += " as " + v["landedName"]
print(line)
la = v.get("landsAs") or {}
if la.get("delivery"): print("they get it as %s (at most %s)" % (la["delivery"], la.get("ceiling")))
if v.get("who"): print(v["who"], file=sys.stderr)
if v.get("addressNote"): print("note: " + v["addressNote"], file=sys.stderr)'
            fi
            local st
            st="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status") or "")')"
            [ "$st" = "landed" ] && exit 0
            exit 3
            ;;
        incoming|sent)
            local verb="${pos[0]:-}"
            if [ "$sub" = "incoming" ] && { [ "$verb" = "accept" ] || [ "$verb" = "decline" ]; }; then
                local id="${pos[1]:-}"
                [ -n "$id" ] || { echo "usage: k2 vault incoming $verb <delivery-id>" >&2; exit 2; }
                if _envn_any HOOK_SOCK HOOK_TOKEN; then
                    # 0.46.10: an agent never runs this; the person answers
                    # K2's permission card (desktop and phone).
                    echo "owner_only: a person accepts or declines $id on the card K2 put on your Thread (Allow accepts, Deny declines; it shows on their phone too). Nothing for you to run." >&2
                    echo "No card there? k2 vault incoming puts it up, or ask: k2 permission request vault.incoming.accept id=$id --why \"<what you need it for>\"" >&2
                    exit 3
                fi
                _vault_person_guard
                local body
                body="$(python3 -c '
import json, sys
id_, action, as_n, replace, always = sys.argv[1:6]
o = {"id": id_, "action": action}
if as_n: o["as"] = as_n
if replace == "1": o["replace"] = True
if always == "1": o["always"] = True
print(json.dumps(o))' "$id" "$verb" "$as_n" "$replace" "$always")"
                _vault_sock_call POST "incoming/decide" "$body"
                [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
                if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
                printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin); d = v.get("delivery") or {}
print("%s %s from %s (%s)%s" % (d.get("status"), d.get("landedName") or d.get("name"), d.get("peer"), d.get("deliveryId"), (" → " + v["ref"]) if v.get("ref") else ""))
m = v.get("secret") or {}
if m:
    print("delivered to the agent as %s (at most %s)%s" % (m.get("delivery"), m.get("ceiling"), (", until " + m["expiresAt"]) if m.get("expiresAt") else ""))
    print("it uses it with: %s" % m.get("use"))
if v.get("more"): print(v["more"])
if v.get("alwaysAdded"): print("always accepting from " + v["alwaysAdded"] + " now")'
                exit 0
            fi
            [ -z "$verb" ] || { echo "usage: k2 vault $sub [--workspace <ws>] [--json]  (accept/decline: k2 vault incoming accept <id>)" >&2; exit 2; }
            _vault_handoff_call GET "$sub" "$q"
            [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
sub = sys.argv[1]
v = json.load(sys.stdin)
items = v.get("items") or []
if not items:
    print("nothing %s yet" % ("received" if sub == "incoming" else "sent"))
for d in items:
    who = ("from " if sub == "incoming" else "to ") + (d.get("peer") or "?")
    extra = ""
    if d.get("status") == "pending" and sub == "incoming":
        c = d.get("card") or {}
        if c.get("card"):
            extra = "  (waiting for a person: the card on the Thread of %s)" % (c.get("addr") or d.get("workspace") or "the agent")
        else:
            extra = "  (no card yet: %s)" % (d.get("ask") or "k2 permission request vault.incoming.accept id=%s" % d.get("deliveryId"))
    if d.get("ref"): extra = "  " + d["ref"]
    print("%-22s %-10s %-28s %-24s %s%s" % (d.get("landedName") or d.get("name"), d.get("status"), who, d.get("createdAt") or "", d.get("deliveryId"), extra))
    if d.get("note"): print("    note: %s" % d["note"])
    if sub == "incoming":
        m = d.get("secret")
        if m:
            print("    yours as %s%s; use: %s" % (m.get("delivery"), (" until " + m["expiresAt"]) if m.get("expiresAt") else "", m.get("use")))
        elif d.get("status") == "pending" and d.get("landsAs"):
            la = d["landsAs"]
            print("    lands as %s (at most %s)%s" % (la.get("delivery"), la.get("ceiling"), (", kept until " + la["expiresAt"]) if la.get("expiresAt") else ""))' "$sub"
            ;;
        ask)
            local to_a="${pos[0]:-}" aname="${pos[1]:-}"
            [ -n "$to_a" ] && [ -n "$aname" ] || { echo "usage: k2 vault ask <agent>|<agent>::<host> NAME [--note …]" >&2; exit 2; }
            _vault_handoff_call POST ask "$(python3 -c '
import json, sys
o = {"to": sys.argv[1], "name": sys.argv[2]}
if sys.argv[3]: o["note"] = sys.argv[3]
if sys.argv[4]: o["workspace"] = sys.argv[4]
print(json.dumps(o))' "$to_a" "$aname" "$note" "$workspace")"
            [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("asked %s for %s (%s)%s; open %s days" % (v.get("to"), v.get("name"), v.get("askId"), "" if v.get("delivered") else " (they get it when they wake)", v.get("expiresInDays")))'
            ;;
        asks)
            _vault_handoff_call GET asks "$q"
            [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
items = json.load(sys.stdin).get("items") or []
if not items: print("no asks")
for a in items:
    arrow = "asked" if a.get("direction") == "out" else "asked by"
    print("%-22s %-9s %s %-28s %s" % (a.get("name"), a.get("status"), arrow, a.get("peer"), a.get("askId")))'
            ;;
        unsend)
            local uid="${pos[0]:-}"
            [ -n "$uid" ] || { echo "usage: k2 vault unsend <delivery-id> (k2 vault sent)" >&2; exit 2; }
            _vault_handoff_call POST unsend "$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1]}))' "$uid")"
            [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("disabled"): print("took back %s: their copy is disabled" % v.get("deliveryId"))
elif v.get("keptReplaced"): print("took back %s: their owner had replaced it, so it was left alone" % v.get("deliveryId"))
else: print("took back %s" % v.get("deliveryId"))
print(v.get("honest"), file=sys.stderr)'
            ;;
        accept-from)
            local verb="${pos[0]:-list}"
            case "$verb" in
                list)
                    _vault_handoff_call GET "accept-from" "$q"
                    [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
                    ;;
                add|remove)
                    local sender="${pos[1]:-}"
                    [ -n "$sender" ] && [ -n "$workspace" ] || { echo "usage: k2 vault accept-from $verb <sender> --workspace <ws>" >&2; exit 2; }
                    # 0.46.17: an agent asks a person on one card instead.
                    if [ "$verb" = add ] && _envn_any HOOK_SOCK HOOK_TOKEN; then
                        echo "handoff_person_only: a person decides who an agent takes secrets from without a card." >&2
                        echo "Ask with: k2 permission request vault.accept-from sender=$sender receivers=$workspace --why \"<what you need it for>\" (several receivers: receivers=a,b; one person: --assign <login>)" >&2
                        exit 3
                    fi
                    _vault_person_guard
                    _vault_sock_call POST "accept-from" "$(python3 -c 'import json,sys; print(json.dumps({"workspace": sys.argv[1], "sender": sys.argv[2], "action": sys.argv[3]}))' "$workspace" "$sender" "$verb")"
                    [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
                    ;;
                *) echo "usage: k2 vault accept-from list|add|remove …" >&2; exit 2 ;;
            esac
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
e = v.get("entries") or []
if not e: print("accepts from no one yet (every secret waits for a person)")
for a in e:
    print("%-40s added by %s at %s" % (a.get("senderLabel") and ("%s (%s)" % (a["sender"], a["senderLabel"])) or a.get("sender"), a.get("addedBy"), a.get("addedAt")))'
            ;;
        send-allow)
            # 0.46.11: the person-given secrets this agent may send to an
            # agent on another server without asking. Add/remove are an
            # Owner's or Admin's (Settings → Workspaces → <agent> →
            # Permissions, the vault.send card, or here from their own
            # terminal; add asks for the vault confirm).
            local verb="${pos[0]:-list}"
            case "$verb" in
                list)
                    _vault_handoff_call GET "send-allow" "$q"
                    [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
                    ;;
                add|remove)
                    local sname="${pos[1]:-}"
                    [ -n "$sname" ] && [ -n "$to" ] && [ -n "$workspace" ] || { echo "usage: k2 vault send-allow $verb NAME --to <agent>::<host> --workspace <ws>" >&2; exit 2; }
                    if _envn_any HOOK_SOCK HOOK_TOKEN; then
                        echo "handoff_person_only: an Owner or Admin decides which secrets you may send to another server. To ask, run k2 vault send $sname --to $to and run the Ask with: line it prints." >&2
                        exit 3
                    fi
                    _vault_person_guard
                    local sbody
                    sbody="$(python3 -c 'import json,sys; print(json.dumps({"workspace": sys.argv[1], "name": sys.argv[2], "to": sys.argv[3], "action": sys.argv[4]}))' "$workspace" "$sname" "$to" "$verb")"
                    if [ "$verb" = "add" ]; then
                        # The same vault confirm as sending it (handoff-send, bound to the secret).
                        _vault_sock_call GET "send-allow?${q}"
                        [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
                        local ssid stok
                        ssid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys
n = sys.argv[1]
print(next((s["id"] for s in json.load(sys.stdin).get("secrets") or [] if s.get("name") == n), ""))' "$sname")"
                        [ -n "$ssid" ] || { echo "not_found: $workspace has no secret a person gave named $sname (k2 vault list --scope ws:$workspace)" >&2; exit 4; }
                        stok="$(_vault_confirm_token handoff-send "$ssid" "let $workspace send $sname to $to")" || exit 3
                        sbody="$(_with_confirm_token "$(python3 -c 'import json,sys; o=json.loads(sys.argv[1]); o["secretId"]=sys.argv[2]; print(json.dumps(o))' "$sbody" "$ssid")" "$stok")"
                        stok=""
                    fi
                    _vault_sock_call POST "send-allow" "$sbody"
                    [ "$_SC_STATUS" = "200" ] || _vault_handoff_fail "$_SC_STATUS" "$_SC_BODY"
                    ;;
                *) echo "usage: k2 vault send-allow list|add|remove …" >&2; exit 2 ;;
            esac
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
e = v.get("entries") or []
ws = v.get("workspace") or "this agent"
if not e: print("%s may send no person-given secret to another server without asking yet" % ws)
for a in e:
    print("may send %-28s to %-36s added by %s at %s" % (a.get("name"), a.get("to"), a.get("addedBy"), a.get("addedAt")))'
            ;;
    esac
}

# Per-verb help: `k2 vault <verb> --help` (fetch and send have their own,
# above). Unknown verbs fall back to the top-level help.
_vault_sub_usage() {
    case "$1" in
        open) cat <<'EOF'
%include cli/text/help/vault/open.txt
EOF
            ;;
        status) cat <<'EOF'
%include cli/text/help/vault/status.txt
EOF
            ;;
        list|ls) cat <<'EOF'
%include cli/text/help/vault/list.txt
EOF
            ;;
        show) cat <<'EOF'
%include cli/text/help/vault/show.txt
EOF
            ;;
        set) cat <<'EOF'
%include cli/text/help/vault/set.txt
EOF
            ;;
        rotate) cat <<'EOF'
%include cli/text/help/vault/rotate.txt
EOF
            ;;
        hosts) cat <<'EOF'
%include cli/text/help/vault/hosts.txt
EOF
            ;;
        sign) cat <<'EOF'
%include cli/text/help/vault/sign.txt
EOF
            ;;
        delivery) cat <<'EOF'
%include cli/text/help/vault/delivery.txt
EOF
            ;;
        grant) cat <<'EOF'
%include cli/text/help/vault/grant.txt
EOF
            ;;
        revoke) cat <<'EOF'
%include cli/text/help/vault/revoke.txt
EOF
            ;;
        disable|enable) cat <<'EOF'
%include cli/text/help/vault/disable.txt
EOF
            ;;
        destroy) cat <<'EOF'
%include cli/text/help/vault/destroy.txt
EOF
            ;;
        get) cat <<'EOF'
%include cli/text/help/vault/get.txt
EOF
            ;;
        audit) cat <<'EOF'
%include cli/text/help/vault/audit.txt
EOF
            ;;
        requests) cat <<'EOF'
%include cli/text/help/vault/requests.txt
EOF
            ;;
        presets) cat <<'EOF'
%include cli/text/help/vault/presets.txt
EOF
            ;;
        doctor) cat <<'EOF'
%include cli/text/help/vault/doctor.txt
EOF
            ;;
        env-file) cat <<'EOF'
%include cli/text/help/vault/env-file.txt
EOF
            ;;
        run) cat <<'EOF'
%include cli/text/help/vault/run.txt
EOF
            ;;
        proxy-url) cat <<'EOF'
%include cli/text/help/vault/proxy-url.txt
EOF
            ;;
        file) cat <<'EOF'
%include cli/text/help/vault/file.txt
EOF
            ;;
        sync) cat <<'EOF'
%include cli/text/help/vault/sync.txt
EOF
            ;;
        import) cat <<'EOF'
%include cli/text/help/vault/import.txt
EOF
            ;;
        service) cat <<'EOF'
%include cli/text/help/vault/service.txt
EOF
            ;;
        reveal) cat <<'EOF'
%include cli/text/help/vault/reveal.txt
EOF
            ;;
        recovery) cat <<'EOF'
%include cli/text/help/vault/recovery.txt
EOF
            ;;
        confirm-password) cat <<'EOF'
%include cli/text/help/vault/confirm-password.txt
EOF
            ;;
        kek|rekey) cat <<'EOF'
%include cli/text/help/vault/kek.txt
EOF
            ;;
        rollback) cat <<'EOF'
%include cli/text/help/vault/rollback.txt
EOF
            ;;
        backup) cat <<'EOF'
%include cli/text/help/vault/backup.txt
EOF
            ;;
        restore) cat <<'EOF'
%include cli/text/help/vault/restore.txt
EOF
            ;;
        recipient) cat <<'EOF'
%include cli/text/help/vault/recipient.txt
EOF
            ;;
        public-key) cat <<'EOF'
%include cli/text/help/vault/public-key.txt
EOF
            ;;
        agent-limit) cat <<'EOF'
%include cli/text/help/vault/agent-limit.txt
EOF
            ;;
        systemd-cred) cat <<'EOF'
%include cli/text/help/vault/systemd-cred.txt
EOF
            ;;
        placement|placements) cat <<'EOF'
%include cli/text/help/vault/placement.txt
EOF
            ;;
        carry) cat <<'EOF'
%include cli/text/help/vault/carry.txt
EOF
            ;;
        *) _vault_usage ;;
    esac
}

