# ── k2 vault (prd-k2-vault-v1 §14) ──────────────────────────────────
# One encrypted store for passwords, tokens and keys. Values are read from
# stdin only (never an argument, never a URL). Agent verbs go over the
# session passport (cell socket / scoped token); the steps that are for
# people only (recovery key, vault password, reveal, key operations) go
# over ~/.k2/vault.sock from a terminal outside K2.

_vault_usage() {
    cat <<'EOF'
k2 vault — secrets for agents, without showing them the value

  k2 vault status                          where the key lives, recovery, honest line
  k2 vault open                            open a vault that didn't open at start (a locked
                                           or cancelled Keychain prompt); may ask once
  k2 vault list [--scope <s>] [--system]   names, scope, sites, delivery (never values)
  k2 vault show NAME [--scope <s>]         one secret: sites, delivery, shares, recent use
  k2 vault set NAME [options] < value      create, or a new version (value on stdin)
  k2 vault rotate NAME < value             a new version
  k2 vault hosts NAME --add <host> | --remove <host>
  k2 vault delivery NAME fetch|agent|file|env|api [--ceiling <d>]
                                           (no --env-name here: names live on grants)
  k2 vault sign NAME ovh|hmac-sha256|hmac-sha256-ts|none [--sign-header …]
                                           K2 signs fetch requests with the secret
  k2 vault public-key NAME                 an ssh key's .pub line (never the private key)
  k2 vault agent-limit NAME [--burst N] [--per-minute M] [--allow-forwarded|--no-forwarded]
                                           an ssh key's agent signing limits (agent delivery)
  k2 vault grant NAME --to ws:<name>|ws:*|session:<id>|service:<label>|job:<name>|compute:<ws>/<node>|compute:<ws>/lane:<owner>/<lane> [--as <d>] [--env-name X] [--ttl 2h]
  k2 vault revoke NAME --from <who>        (or --grant <id>)
  k2 vault disable|enable NAME             destroy NAME --yes
  k2 vault fetch --secret NAME [-X M] [-H 'N: v'] [-d @file] URL
                                           K2 makes the call and adds the key; you never see it
  k2 vault get NAME                        print a value (needs an api grant; it lands in your transcript)
  k2 vault audit [NAME] [--since <rfc3339>]
  k2 vault requests                        Thread secret cards (pending / set / declined)
  k2 vault presets                         known key formats (Stripe, OpenAI, Anthropic, …)
  k2 vault env-file NAME [--ttl 90s]       a short-lived 0600 file for docker/podman --env-file
  k2 vault file NAME [--ttl 90s]           a short-lived 0600 file holding one secret delivered to you
                                           as file (or env): pass the file, never the value
                                           (tool --code - < "$(k2 vault file NAME)")
  k2 vault sync                            files granted since this session started, into $K2_SECRETS_DIR now
  k2 vault import .env [--scope <s>] [--remove-original]   move a .env in; writes .env.k2 (references)
  k2 vault service <service> [clean|legacy] [--env-file .env.k2]   a published service's environment
  k2 publish secret add|remove|list <service> NAME [--as file|env]   secrets a service gets at start
  k2 vault doctor                          plaintext .env files, due rotations, problems
  k2 vault send NAME --to <agent>[::<host>] [--note …]   hand a secret to another agent by name
  k2 vault incoming | sent | accept-from | send-allow   handoffs to and from this agent (k2 vault send --help)
  k2 vault placement add NAME --where <host>:<path> [--note …] | done ID | rm ID
                                           note a place the value lives that K2 doesn't write
  k2 vault placements [NAME]               where values live outside the vault (due after a rotate)

  People only (from your own terminal or SSH, not a K2 terminal):
  k2 vault reveal NAME                     show a value (asks for a confirm)
  k2 vault recovery show-once              make and show the recovery key once; type back 3 words
  k2 vault recovery rotate                 replace the recovery key (asks for a confirm)
  k2 vault confirm-password set            set the vault password used for confirms
  k2 vault kek rotate | rekey              rotate the box key / the data key (owner, confirm)
  k2 vault rollback NAME --to <version>    put an older version back (confirm)
  k2 vault systemd-cred NAME --unit <unit.service> [--restart] [--dry-run] | --remove
                                           seal it as a root unit's systemd credential (Linux, confirm)
  k2 vault systemd-cred NAME --replace     re-place a new version into its root units (confirm)
  k2 vault backup <dir>                    a ciphertext snapshot of the vault (owner)
  k2 vault backup key <age1…> | remove     let this backup key open the vault, or take it off (owner, confirm)
  k2 vault restore --from <dir> [--holder recovery|backup] [--dry-run] [--replace]
                                           restore a backup on this box (recovery words or backup key on stdin)

  Moving secrets to another box (owner, Admin, or an agent allowed to manage server-wide secrets):
  k2 vault recipient                       this vault's carry address (run it on the target box)
  k2 vault carry export --recipient <age1…> --target <vault-id> --workspace <ws> [--target-workspace <id>] [--server NAME] [--own-box-move] --out <file>
  k2 vault carry import <file> [--workspace <id>] [--apply]
                                           lists first (with the target workspace id); --apply puts them in this box's vault.
                                           Workspace secrets match by id, never by name: --workspace <id> on this box, or the
                                           id the carry was made for (--target-workspace). A name works only if it is that id.

Scope (--scope): ws:<workspace> (default inside a workspace), server, session.
set options:
  --kind api_key|password|oauth_token|ssh_key|tls_key|dsn|url|generic   (default api_key)
  --for <host>[,<host>]       sites this secret may be sent to (exact hosts)
  --preset stripe|openai|anthropic|xai|gemini|github|slack|supabase|ovh-eu|ovh-ca|ovh-us
                              fills the sites and where the key goes in a request
  --sign ovh|hmac-sha256|hmac-sha256-ts [--sign-header <Name> …]
                              K2 signs each request instead (k2 vault set --help)
  --inject-dst header:<Name>|query:<param>|basic|body   where fetch puts the key
  --inject-header <Name>      same as --inject-dst header:<Name>
  --inject-format '<fmt>'     how it's written, one {} (e.g. 'Bearer {}'; default {})
  --delivery fetch|agent|file|env|api   how agents get it (default fetch for keys;
                              agent: ssh keys sign on $SSH_AUTH_SOCK, never shown)
  --ceiling fetch|agent|file|env|api    the loosest delivery allowed later
  --describe <text>  --rotate-every <days>  --ttl 2h (session scope)

Times (--ttl on set, grant, file, env-file and secret cards): 90s, 10m, 2h,
7d, or plain seconds (3600).

Where the key goes is fixed by whoever creates the secret (header and
format); agents name the secret and K2 adds it. The presets fill this in
for common key formats, so you usually just confirm.

Agents: use `k2 vault fetch` (K2 makes the call and adds the key), never
paste a key into a file or chat; ask a person with `k2 thread secret`.
Each verb has its own help: k2 vault <verb> --help.
More: k2 study vault
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
    if [ -n "${K2_HOOK_SOCK:-}${K2_HOOK_TOKEN:-}${K2SO_HOOK_SOCK:-}${K2SO_HOOK_TOKEN:-}" ]; then
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
    local method="$1" route="$2" payload="${3:-}" tty="0" out
    [ -t 0 ] && [ -t 1 ] && tty="1"
    [ -t 0 ] && [ -t 2 ] && tty="1"
    if [ "$method" = "GET" ]; then
        out="$(_k2_curl "" -s -G --unix-socket "$(_vault_sock)" "http://localhost/cli/vault/${route}" \
            -H "X-K2-Tty: ${tty}" --connect-timeout 5 --max-time 180 -w $'\n%{http_code}' 2>/dev/null)" || out=""
    else
        out="$(printf '%s' "$payload" | curl -s -X POST --unix-socket "$(_vault_sock)" "http://localhost/cli/vault/${route}" \
            -H "X-K2-Tty: ${tty}" -H "Content-Type: application/json" --data-binary @- \
            --connect-timeout 5 --max-time 180 -w $'\n%{http_code}' 2>/dev/null)" || out=""
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
        printf 'Value (not shown): ' >&2
        IFS= read -rs v || v=""
        echo >&2
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

# Ask the daemon for a confirm token over vault.sock (Touch ID sheet on a
# Mac, else the vault password typed here). Prints the token.
_vault_confirm_token() {
    local action="$1" secret_id="${2:-}" label="${3:-}"
    _vault_sock_call POST "confirm/start" "$(python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1]}))' "$action")"
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    local methods
    methods="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(" ".join(json.load(sys.stdin).get("methods") or []))')"
    local body
    # K2_VAULT_CONFIRM_METHOD=vault-password: use the vault password even on
    # a Mac with Touch ID (a headless run, e.g. the two-server harness). The
    # same check as always: the daemon verifies the vault password.
    if [ "${K2_VAULT_CONFIRM_METHOD:-}" = "vault-password" ]; then
        case " $methods " in
            *" vault-password "*) methods="vault-password" ;;
            *) echo "no_confirm_method: K2_VAULT_CONFIRM_METHOD=vault-password, but no vault password is set (k2 vault confirm-password set)" >&2; exit 3 ;;
        esac
    fi
    case " $methods " in
        *" local-auth "*)
            echo "Approve on this Mac (Touch ID or your login password)…" >&2
            body="$(python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": "local-auth", "label": sys.argv[3]}))' "$action" "$secret_id" "$label")"
            ;;
        *" vault-password "*)
            local pw=""
            printf 'Vault password: ' >&2
            IFS= read -rs pw || pw=""
            echo >&2
            body="$(printf '%s' "$pw" | python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "secretId": sys.argv[2] or None, "method": "vault-password", "password": sys.stdin.read()}))' "$action" "$secret_id")"
            pw=""
            ;;
        *)
            echo "no_confirm_method: set a vault password first: k2 vault confirm-password set" >&2
            exit 3
            ;;
    esac
    _vault_sock_call POST "confirm/finish" "$body"
    body=""
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])'
}

# k2 vault fetch — K2 makes the HTTPS call and adds the secret where its
# creator said; you never see the value. Echoes come back as [k2v:NAME].
_vault_fetch() {
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
            --max-bytes) max_bytes="${2:-}"; shift 2 ;;
            --timeout) timeout="${2:-}"; shift 2 ;;
            --include-headers|-i) include=1; shift ;;
            -o|--output) [ -n "${2:-}" ] || { echo "usage: -o file" >&2; exit 2; }; out="$2"; shift 2 ;;
            --dst) dst="${2:-}"; shift 2 ;;
            --fmt) fmt="${2:-}"; shift 2 ;;
            --json-out) as_json=1; shift ;;
            --help|-h)
                cat <<'EOF'
k2 vault fetch --secret NAME [--scope session|server|ws:<name>] [-X METHOD] [-H 'Name: value']... [-d 'text' | -d @file | --json @file]
               [--max-bytes N] [--timeout S] [--include-headers] [-o file] [--json-out] URL

K2 makes the call and adds the secret (header and format fixed by whoever
created it); only to the secret's sites. Bodies over 64 KiB and -o output
are files inside your workspace. Echoes of the value come back as [k2v:NAME].

fetch adds the secret as-is, in one place: a header, a query parameter,
basic auth or the body (k2 vault set --help: --inject-dst).
Some secrets are signers: K2 signs the request (OVH, HMAC, HMAC with a
timestamp). Same command. --dst and --fmt are refused for them, and so is
a -H naming one of the signer's headers. (Not yet: AWS SigV4, Google
service-account JWT; give those tools env or file delivery.)

Scripts: --json-out prints the daemon's answer as one JSON object:
  {"status": 200, "headers": [["content-type", "…"], …], "truncated": false,
   "redirected": false, "body": "…"}   (or "bodyB64" for a non-text body;
   with -o: "out" and "bytes" instead of the body; OVH: "clock": "server"
   or "local")
Headers are the short allowlist (content-type, date, etag, location,
retry-after, request ids, rate limits) unless --include-headers. A refusal
exits non-zero with {"error": {"code", "hint"}}. Running 8 at once is fine
(the limit is 30 a second and 600 a minute per session).

Example: k2 vault fetch --secret STRIPE_KEY https://api.stripe.com/v1/balance
         k2 vault fetch --secret OVH_EU --json-out https://eu.api.ovh.com/1.0/me
EOF
                return 0 ;;
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
k2 vault send — hand a vault secret to another agent by name (you never see the value)

  k2 vault send NAME --to <agent>|<agent>::<host> [--as NEWNAME] [--note "…"] [--ttl 7d] [--json]
                                    same server: vault to vault; another server: sealed to its vault
  k2 vault send NAME --to <agent> --delivery file|env [--kind generic]
                                    a code or token for a CLI: they get it as a file
                                    ($K2_SECRETS_DIR/NAME) or env; no sites, no header
  k2 vault send NAME --to <agent> --delivery fetch
                                    an HTTP API key: K2 adds it to requests to its sites
  k2 vault incoming [--workspace <ws>] [--json]     secrets sent to this agent (names only)
  k2 vault sent [--workspace <ws>] [--json]         what this agent sent
  k2 vault accept-from list [--workspace <ws>]      who may land secrets here without asking
  k2 vault ask <agent>|<agent>::<host> NAME [--note "…"]   ask an agent for a secret (no value travels)
  k2 vault send NAME --to <agent> --reply <ask-id>  answer an ask: it lands straight in
  k2 vault asks [--json]                            open and closed asks, in and out
  k2 vault unsend <delivery-id>                     take a delivery back (not a revoke: rotate the key)
  k2 msg <agent> "text" --secret NAME               the same send, the text is the note

  Sent to you and waiting (an agent):
  K2 puts a permission card on your Thread: a person taps Allow (accept) or
  Deny (decline) there, on the desktop or the phone. Nothing for you to run.
  No card there (it couldn't go up, or it's from before 0.46.10)? k2 vault
  incoming puts it up, or ask for one (permission cards on):
  k2 permission request vault.incoming.accept id=<delivery-id> [as=NEW|replace=yes] --why "…"

  A secret a person gave you, to another server (an agent):
  the send is refused (held, exit 3). Permission cards on: run the Ask with:
  line it prints (several for one agent: secret=A,B,C):
  k2 permission request vault.send secret=NAME to=<agent>::<host> --why "…"
  An Owner or Admin sends it from the card, or lets you send it there from
  now on. Cards off: ask them to add it in Settings → Workspaces → <you> →
  Permissions → Secrets it may send to other servers.
  k2 vault send-allow [--workspace <ws>]            what you may send to another server without asking

  People only (your own terminal, not a K2 terminal; the card does the same):
  k2 vault incoming accept <id> [--as NEW] [--replace] [--always]
  k2 vault incoming decline <id>
  k2 vault accept-from add|remove <ws:agent|agent|agent::host|team:id> --workspace <ws>
  k2 vault send-allow add|remove NAME --to <agent>::<host> --workspace <ws>   (add asks for the vault confirm)
  printf '%s' "$KEY" | k2 vault send NAME --to <agent>::<host> --stdin [--kind api_key] [--for host]
                                    a value you type in, to another server only

Secrets arrive pending: a person on the receiving server accepts them on
the card K2 puts on the receiving agent's Thread, unless that agent
accepts from you (k2 vault accept-from) or you answer its ask. A same-named secret is never overwritten; re-sending the
same secret makes a new version. A secret a person gave needs a person's
yes to go to another server (the vault.send card above, or a standing yes
from k2 vault send-allow). Server-wide secrets aren't sent (the owner
shares them with k2 vault grant).

The copy keeps the sender's delivery (or --delivery), at most env and
never api: fetch with sites stays fetch (k2 vault fetch), fetch with no
sites lands as file, file stays file, env stays env. Deliveries go
fetch < file < env (env is the most readable). --delivery can't go above
your own copy's ceiling (delivery_above_ceiling, exit 3: a person raises
it). A file copy is $K2_SECRETS_DIR/NAME (not there yet? k2 vault sync; no
$K2_SECRETS_DIR at all: restart the session), or k2 vault file --secret
NAME for a 0600 path to hand a tool on stdin, never the value on a command
line. The accept card shows the delivery and the ceiling; once a person
accepts it, the receiving agent may take it up to that ceiling itself, no
owner needed: k2 vault grant NAME --to ws:<you> --as env (then
k2 vault env-file --secret NAME works at once; $NAME in new sessions).
k2 vault incoming and k2 vault show NAME say how to use each one. --replace takes the incoming
delivery (a wider one needs an Owner or Admin). --ttl is kept on the other
side: the copy (and a pending card) ends that long after it was sent.

Exit codes: 0 landed · 3 a person must act (pending, held, declined, not
connected, not allowed) · 2 bad request or the peer can't (peer_too_old,
federation_off, never_sendable, send_by_name_only) · 4 no such secret or
agent · 1 unreachable or internal. Don't retry on 3.
More: k2 study vault
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
    if [ -z "${K2_HOOK_SOCK:-}${K2_HOOK_TOKEN:-}${K2SO_HOOK_SOCK:-}${K2SO_HOOK_TOKEN:-}" ] && [ -S "$(_vault_sock)" ]; then
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
    local pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --stdin) stdin=1; shift ;;
            --replace) replace=1; shift ;;
            --always) always=1; shift ;;
            --to|--as|--note|--ttl|--reply|--scope|--workspace|--via|--kind|--for|--delivery)
                [ $# -ge 2 ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --to) to="$2" ;; --as) as_n="$2" ;; --note) note="$2" ;; --ttl) ttl="$2" ;; --reply) reply="$2" ;;
                    --scope) scope="$2" ;; --workspace) workspace="$2" ;; --via) via="$2" ;; --kind) kind="$2" ;; --for) hosts="$2" ;;
                    --delivery) hdelivery="$2" ;;
                esac
                shift 2 ;;
            --value|--value=*|--secret-value*)
                echo "value_on_argv: values are read from stdin, never an argument" >&2; exit 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 vault send --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
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
name, to, as_n, note, ttl, reply, scope, via, kind, hosts, delivery = sys.argv[1:12]
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
if via: o["via"] = via
if kind: o["kind"] = kind
if hosts: o["hosts"] = [h.strip() for h in hosts.split(",") if h.strip()]
if delivery: o["delivery"] = delivery
print(json.dumps(o))' "$name" "$to" "$as_n" "$note" "$ttl" "$reply" "$scope" "$via" "$kind" "$hosts" "$hdelivery")" || exit 2
            if [ "$stdin" = "1" ]; then
                # A value a person types in: the person channel only (H15).
                _vault_person_guard
                body="$(_vault_body_with_value "$body")" || exit 2
                _vault_sock_call POST send "$body"
            else
                _vault_handoff_call POST send "$body"
            fi
            if [ "$_SC_STATUS" = "403" ] && [ "$(_vault_handoff_code "$_SC_BODY")" = "confirm_required" ] && [ -z "${K2_HOOK_SOCK:-}${K2_HOOK_TOKEN:-}${K2SO_HOOK_SOCK:-}${K2SO_HOOK_TOKEN:-}" ] && [ -S "$(_vault_sock)" ]; then
                # A person gave this secret: confirm sending it to another server.
                local sid tok
                sid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("secretId") or "")')"
                tok="$(_vault_confirm_token handoff-send "$sid" "send $name to $to")" || exit 3
                body="$(python3 -c 'import json,sys; o=json.loads(sys.argv[1]); o["confirmToken"]=sys.argv[2]; print(json.dumps(o))' "$body" "$tok")"
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
                if [ -n "${K2_HOOK_SOCK:-}${K2_HOOK_TOKEN:-}${K2SO_HOOK_SOCK:-}${K2SO_HOOK_TOKEN:-}" ]; then
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
                    if [ -n "${K2_HOOK_SOCK:-}${K2_HOOK_TOKEN:-}${K2SO_HOOK_SOCK:-}${K2SO_HOOK_TOKEN:-}" ]; then
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
                        sbody="$(python3 -c 'import json,sys; o=json.loads(sys.argv[1]); o["confirmToken"]=sys.argv[2]; o["secretId"]=sys.argv[3]; print(json.dumps(o))' "$sbody" "$stok" "$ssid")"
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
        status) cat <<'EOF'
k2 vault status [--json]

Where the vault's key lives, the recovery key, the vault password, and
counts: your secrets, K2 system secrets (K2's own, hidden from agents),
shares and pending secret cards. Ends with one honest line on what the
vault protects. Values are never shown.

Example: k2 vault status
EOF
            ;;
        list|ls) cat <<'EOF'
k2 vault list [--scope ws:<name>|server|session] [--system] [--json]

Names, scope, kind, delivery, version and sites of the secrets you can
see. Never values. "(agent can see it)" = file, env or api delivery.
  --scope <s>   one scope only
  --system      also K2's own secrets (mail and database passwords, login
                slots, signing keys). The owner and Admins see their names;
                agents and Members get a count and why they're hidden.

Example: k2 vault list --scope server
EOF
            ;;
        show) cat <<'EOF'
k2 vault show NAME [--scope <s>] [--json]

One secret: kind, sites, where fetch puts it (header/query/basic/body and
format), delivery and ceiling, who gave it, shares, recent use. Never the
value.

Example: k2 vault show STRIPE_KEY
EOF
            ;;
        set) cat <<'EOF'
k2 vault set NAME [options] < value

Create a secret, or a new version of NAME. The value comes on stdin, never
as an argument. A new version changes only the value; the options below
apply when NAME is created.

  --kind <k>       what it is (default api_key); picks the default delivery:
                     api_key, oauth_token, generic   fetch
                     password, dsn, url              env
                     ssh_key, tls_key                file
                   Every kind allows fetch, file and env; api only when
                   --delivery or --ceiling says api. Fetch needs --for.
                   ssh_key also allows agent: sessions get $SSH_AUTH_SOCK
                   and ssh/git sign through K2; nobody sees the key
                   (unencrypted ed25519 or RSA ≥ 2048 bits).
  --delivery fetch|agent|file|env|api   how agents get it (default: by kind)
  --ceiling fetch|agent|file|env|api    the loosest delivery allowed later
                                  (default env, or --delivery if looser)
  --scope ws:<name>|server|session   (default: your workspace)
  --for <host>[,<host>]   the sites fetch may send it to (exact hosts)
  --preset <id>    stripe|openai|anthropic|xai|gemini|github|slack|supabase:
                   fills --for and where the key goes (k2 vault presets);
                   ovh-eu|ovh-ca|ovh-us: fills --for and --sign ovh

  Where fetch puts the value (agents just name the secret):
  --inject-dst <d>         header:<Name>   a request header (default
                                           header:Authorization)
                           query:<param>   a URL query parameter
                           basic           Authorization: Basic base64(format)
                           body            replaces {{secret}} in the request
                                           body (a person only)
  --inject-header <Name>   same as --inject-dst header:<Name>
  --inject-format '<fmt>'  how it's written, exactly one {} (default {})
  With no preset and no --inject-*, K2 sends Authorization: Bearer <value>
  (or, for api_key/oauth_token/generic, a known provider's format when the
  value's prefix matches one).

  Signers (K2 signs each request with the secret instead of pasting it;
  default delivery and ceiling fetch; agents can't change it):
  --sign ovh               OVH API v1. The value is one JSON object:
                           {"applicationKey":…,"applicationSecret":…,
                            "consumerKey":…}; kind defaults to generic.
                           Uses OVH's clock (/1.0/auth/time, cached 15 min).
  --sign hmac-sha256       --sign-header <Name>: prefix + HMAC of the body
  --sign hmac-sha256-ts    --sign-header <Name>: HMAC of a template with a
                           Unix timestamp; kind defaults to hmac_key
  --sign-header <Name>     the signature header (HMAC schemes)
  --sign-encoding hex|base64   (default hex)
  --sign-prefix '<p>'      hmac-sha256 only, e.g. 'sha256=' (≤ 32 chars)
  --sign-ts-header <Name>  hmac-sha256-ts: also send the timestamp here
  --sign-template '<t>'    hmac-sha256-ts: what's signed, one {ts} and one
                           {body} (default '{ts}.{body}')
  --sign-value '<v>'       hmac-sha256-ts: the header value, one {sig},
                           at most one {ts} (default '{sig}')
  --sign oauth-refresh     A sign-in K2 keeps fresh. The value is one JSON
                           object: {"access_token":…,"refresh_token":…,
                            "client_id":…, optional "expires_at" (Unix seconds
                            or RFC 3339), "client_secret", "scope"}; kind
                           defaults to oauth_token. Sent as Authorization:
                           Bearer unless --inject-* says otherwise. Used
                           through k2 vault fetch only: never env, file or api.
  --oauth-token-url <url>  oauth-refresh: the provider's token URL (https)
  --oauth-client-auth basic|post|none
                           oauth-refresh: how the client proves itself
                           (default post; none = a public client)
  Change it later with k2 vault sign (k2 vault sign --help).

  --describe <text>  --rotate-every <days>
  --ttl 2h                 session scope: how long it lasts (90s, 10m, 2h, 7d
                           or seconds; default 24h, 60s to 7d; longer is cut to 7d)

Examples:
  k2 vault set STRIPE_KEY --preset stripe < key.txt
  k2 vault set WEATHER_KEY --for api.weather.example --inject-dst query:appid < key.txt
  k2 vault set MAILGUN_KEY --for api.mailgun.net --inject-dst basic --inject-format 'api:{}' < key.txt
  k2 vault set DEPLOY_KEY --kind ssh_key --delivery agent < deploy_ed25519
  k2 vault set OVH_EU --sign ovh --preset ovh-eu < ovh-keys.json
  k2 vault set HOOK_KEY --sign hmac-sha256 --sign-header X-Hub-Signature-256 --sign-prefix 'sha256=' --for hooks.example.com < key
  k2 vault set OPS_INGEST --sign hmac-sha256-ts --sign-header X-K2-Ops-Sig --sign-ts-header X-K2-Ops-Ts --for ingest.example.com < key
  k2 vault set GMAIL_API --sign oauth-refresh --oauth-token-url https://oauth2.googleapis.com/token --for gmail.googleapis.com < sign-in.json
Better than set: ask a person, so the value never passes through you:
  k2 thread secret <addr> --name STRIPE_KEY --for api.stripe.com
EOF
            ;;
        rotate) cat <<'EOF'
k2 vault rotate NAME [--scope <s>] < value

A new version of an existing secret (value on stdin). K2 stops using older
versions; sessions with env or file delivery get the new value when they
restart (until then, their env value and any file already in $K2_SECRETS_DIR
hold the old one).
An agent may rotate a secret a person gave it (the owner is told, with undo).

Example: k2 vault rotate STRIPE_KEY < new-key.txt
EOF
            ;;
        hosts) cat <<'EOF'
k2 vault hosts NAME --add <host> | --remove <host> [--scope <s>]

The sites fetch may send NAME to. Exact hosts only (wildcards are for
people). --add and --remove repeat. Adding a site widens the secret: an
agent may do it only for secrets its own workspace created.

Example: k2 vault hosts STRIPE_KEY --add files.stripe.com
EOF
            ;;
        sign) cat <<'EOF'
k2 vault sign NAME ovh | hmac-sha256 --sign-header <Name> [--sign-encoding hex|base64] [--sign-prefix '<p>']
k2 vault sign NAME hmac-sha256-ts --sign-header <Name> [--sign-ts-header <Name>] [--sign-template '<t>'] [--sign-value '<v>']
k2 vault sign NAME oauth-refresh --oauth-token-url <url> [--oauth-client-auth basic|post|none]
k2 vault sign NAME none

Set or remove how K2 signs requests made with NAME (k2 vault set --help:
Signers). k2 vault fetch then adds the signer's headers on every request,
redirects included; --dst/--fmt are refused. Changing the signer of a
secret a person gave you is the owner's call. ovh checks that the stored
value is the OVH JSON. none goes back to pasting the value as set.
oauth-refresh checks that the stored value is the sign-in JSON (k2 vault set
--help). Once set it can't be removed, and NAME stays at fetch: K2 refreshes the
token and the refresh token never leaves K2. If the provider stops accepting
it, NAME reads "expired: sign in again" and uses exit 3 (oauth_expired); a card
goes to the Thread, or a person runs k2 vault rotate NAME < sign-in.json.

Examples:
  k2 vault sign OVH_EU ovh
  k2 vault sign HOOK_KEY hmac-sha256 --sign-header X-Hub-Signature-256 --sign-prefix 'sha256='
  k2 vault sign STRIPE_HOOK hmac-sha256-ts --sign-header Stripe-Signature --sign-value 't={ts},v1={sig}'
EOF
            ;;
        delivery) cat <<'EOF'
k2 vault delivery NAME fetch|agent|proxy|file|env|api [--ceiling <d>] [--scope <s>]

NAME's default delivery (what its own workspace gets without a grant) and,
with --ceiling, the loosest delivery allowed. fetch: K2 makes the call;
proxy: fetch, plus a placeholder key and local base URL in new sessions'
env (k2 vault proxy-url NAME gets them now);
agent (ssh keys): $SSH_AUTH_SOCK, ssh and git sign, nobody sees the key;
file: $K2_SECRETS_DIR/NAME; env: $NAME; api: k2 vault get NAME. Agent,
file, env and api arrive in new sessions. No --env-name or --file-name
here: names live on grants (k2 vault grant --help).
An agent widens only secrets its own workspace created; for one a person
gave it or another agent handed it: vault_owner_confirm_required (exit 3).
With permission cards on, that refusal ends with the line to ask with:
k2 permission request vault.delivery secret=NAME delivery=file|env.

Example: k2 vault delivery DEPLOY_KEY agent
EOF
            ;;
        grant) cat <<'EOF'
k2 vault grant NAME --to <who> [--as <d>] [--env-name X] [--file-name F] [--ttl 2h] [--scope <s>]

Share NAME, or set how one consumer gets it (one grant per consumer and
delivery; a new one replaces the old).
  --to ws:<name>     a workspace; ws:* = every workspace;
                     session:<id> (a running session's id, not k2 whoami's; the
                     owner lists them: k2 sessions live <workspace>. Other ids
                     are refused, and the share ends with that session);
                     service:<label> (a k2 publish service);
                     job:<name> (a k2 job; its runs get only these);
                     compute:<ws>/<node> (that workspace's compute jobs on
                     that node: k2 compute run --secret NAME), or
                     compute:<ws>/* (every node this server owns), or
                     compute:<ws>/lane:<owner>/<lane> (every node of your
                     seat on that owner's lane, while a person here consents
                     for that owner: k2 compute owners consent <owner> on;
                     the seat request card gives it;
                     (preview: k2 settings set ciPreview true)). People
                     only (the value leaves this machine), --as env only.
  --as fetch|agent|proxy|file|env|api   default: the secret's delivery; --env-name alone
                     means env, --file-name alone means file. Never above
                     the secret's ceiling.
  --env-name X       the variable for env delivery (default: NAME in capitals,
                     - and . as _). Names that change how programs run
                     (PATH, LD_*, GIT_*, SSH_*, K2_*, *_BASE_URL…) are
                     refused.
  --file-name F      the file in $K2_SECRETS_DIR (default NAME)
  --ttl 2h           the share ends by itself that long from now (90s, 10m,
                     2h, 7d or seconds); k2 vault show lists "until <time>".
                     Without it the share stays until revoked.
A file grant also reaches running sessions: K2 writes it into their
$K2_SECRETS_DIR at once (or within a minute; k2 vault sync in a session
does it now). Env and ssh-key grants reach new sessions only: restart
your session to receive it.

A share they can read (file, env or api) asks you to confirm first, like
reveal (Touch ID or your vault password), when you run it from your own
terminal. A K2 terminal can't: share it in Settings → Vault instead.
An agent refused a share with its own service (a person gave the secret):
with permission cards on, the refusal ends with the line to ask with:
k2 permission request vault.share-service as=file|env secret=NAME service=<service>.

Example, a CLI tool that reads its token from the environment:
  k2 vault grant GITHUB_CLI --to ws:<you> --env-name GH_TOKEN
  (restart the session; gh now reads $GH_TOKEN)
EOF
            ;;
        revoke) cat <<'EOF'
k2 vault revoke NAME --from <who> [--scope <s>]
k2 vault revoke --grant <id>

Remove a share (<who> as in grant --to; grant ids are in k2 vault show
--json). New sessions and fetches stop getting it, and its file goes from
$K2_SECRETS_DIR in running sessions that no longer get it. A value already
read stays read, and an environment variable stays in a running session
until it restarts: rotate the key at the provider if that matters. A
compute: grant's running jobs keep what they got (they're listed); the
next attempt is refused.

Example: k2 vault revoke STRIPE_KEY --from ws:billing
EOF
            ;;
        disable|enable) cat <<'EOF'
k2 vault disable NAME [--scope <s>]
k2 vault enable NAME [--scope <s>]

Stop every use of NAME without deleting it, or start it again. Disabling
takes its file out of $K2_SECRETS_DIR in running sessions; enabling puts it
back within a minute. An environment variable stays in a running session
until it restarts. Enabling counts as widening: a secret a person gave needs
the owner.

Example: k2 vault disable STRIPE_KEY
EOF
            ;;
        destroy) cat <<'EOF'
k2 vault destroy NAME --yes [--scope <s>]

Delete every version on this box. The name can't be used again. Its file
goes from $K2_SECRETS_DIR in every running session at once. K2 can't take
back a value a session already read or copied, or an environment variable
in a running session (it stays until that session restarts): rotate the key
at the provider if that matters. Backups made earlier keep it until they
expire. To pause instead: k2 vault disable.

Example: k2 vault destroy OLD_KEY --yes
EOF
            ;;
        get) cat <<'EOF'
k2 vault get NAME [--scope <s>]

Print the value. Needs api delivery: a grant --as api, or your own secret
with k2 vault delivery NAME api --ceiling api. The value lands in your
transcript and logs; prefer fetch, or env/file for tools. People use
k2 vault reveal from their own terminal.

Example: k2 vault get BUILD_TOKEN
EOF
            ;;
        audit) cat <<'EOF'
k2 vault audit [NAME] [--since <rfc3339>] [--scope <s>] [--json]

Uses, changes, shares and refusals, names only. Agents see their own
workspace's; the owner and Admins see all.

Example: k2 vault audit STRIPE_KEY --since 2026-10-01T00:00:00Z
EOF
            ;;
        requests) cat <<'EOF'
k2 vault requests [--json]

Thread secret cards (k2 thread secret) and their state: pending, set or
declined.

Example: k2 vault requests
EOF
            ;;
        presets) cat <<'EOF'
k2 vault presets [--json]

Known key formats: the sites and where the key goes. Use one with
k2 vault set NAME --preset <id>. Run it to see the ids.

Presets are for k2 vault set only. A secret card (k2 thread secret) has
none: name the site and how K2 sends it there (k2 thread secret --help).

Example: k2 vault presets
EOF
            ;;
        doctor) cat <<'EOF'
k2 vault doctor [--json]
k2 vault doctor --transcripts [--json]

Problems: no confirmed recovery key, tamper, rotations due, secrets a
newer K2 wrote (this one skips them), published services on the old
inherited environment, (Linux) a runtime dir systemd-logind can delete
under the daemon (no linger; the warning names the fix), and
secret-looking values in plain .env files in your workspace and one
level of subfolders (clones, worktrees; not node_modules, .git or
target; at most 200 files and 4 MiB a run). A value that looks like a
known provider's key (sk_live_, sk-ant-, ghp_, …) is flagged under any
name, by provider, never the value. It also lists CLI login files that
hold a credential: gh hosts.yml, vercel auth.json, ~/.npmrc with a
literal _authToken, ovh.conf, ~/.ssh/id_* keys, each with the vault
recipe. The last line says which .env names it checks; other
names aren't flagged. Exit 1 on an error.

--transcripts: only the transcript scan, instead of the checks above.
Looks for vault values in what your LLM CLIs saved (Claude, Codex, Grok,
Gemini, Cursor, Pi, Hermes, and K2's per-login config folders). Reports
which secret names appear in which files, never a value or a line of the
file. Run by a person (owner, Admin): every transcript against every
value, older versions too. Run by an agent: this workspace's transcripts
against the latest value of secrets it can already read (file, env or
api); for the rest, ask your owner to run it. Reads newest files first,
at most 2 GB a run. It only reports: K2 doesn't edit other tools' files.
Exit 0 when the scan finished and found nothing; exit 1 on a leak or a
scan that couldn't cover everything (the last line says what it read).

Example: k2 vault doctor
Example: k2 vault doctor --transcripts
EOF
            ;;
        env-file) cat <<'EOF'
k2 vault env-file NAME [NAME2 …] [--scope session|server|ws:<name>] [--ttl 90s] [--wait 10m]
k2 vault env-file --done <path>

From a K2 terminal: a short-lived 0600 file of NAME=value lines for
docker/podman --env-file, in a memory-backed folder. Removed after --ttl
(90s, 10m or seconds; default 60s, 5s to 1h) or with --done. Each secret
needs env delivery for you: your workspace's own secret set to env, or a
server (or other workspace's) secret shared with your workspace as env
(k2 vault grant NAME --to ws:<you> --as env --scope server, by a person;
then name it with --scope server). A value with a line break is refused
(use file delivery). --secret NAME works too. One value for a tool's stdin or a file
flag: k2 vault file NAME. --wait 10m blocks until a person saves the card
that asks for it (see k2 vault file --help).

Example: docker run --env-file "$(k2 vault env-file DATABASE_URL)" myapp
EOF
            ;;
        run) cat <<'EOF'
k2 vault run [--secret NAME[=ENV_NAME]]... [--env-file .env.k2] [--scope workspace|server|session] [--no-mask] -- cmd [args…]

From a K2 terminal: run one command with its secrets, now, with no session
restart. What the child gets is what you already have, never more:
  env delivery    ENV_NAME=<value>, default the name the share set, else
                  NAME in capitals (--secret NAME=ENV_NAME picks another)
  file delivery   ENV_NAME_FILE=<path>, a private 0600 file for this run,
                  removed when the command exits. Never put in env
  fetch only      refused; no person needed, K2 can make the call for you:
                  k2 vault fetch --secret NAME <url>
  ssh keys        refused; ssh and git already sign with $SSH_AUTH_SOCK
  one-time codes  refused; reading one deletes it, so read it yourself:
                  tool --code-file "$(k2 vault file NAME)"
  OAuth sign-ins  a placeholder key and base URL, as k2 vault proxy-url
                  prints; the tokens stay in K2 and K2 keeps them fresh
--secret repeats; NAME alone works too. --env-file resolves each KEY=k2v://…
line through the vault and passes other lines through as they are; the file
must be inside your workspace.

The values are replaced by [k2v:NAME] (the vault's name for the secret) in
the command's output. That is hygiene, not a wall: the command has the value
and can write it anywhere. Without --no-mask the command's output goes
through pipes, so it doesn't see a terminal. --no-mask lets the command use
your terminal (TTY), for interactive tools. Output is then not masked, and
the run is logged as no-mask.

Exit code: the command's own (128 + the signal if a signal killed it).
K2's own refusals come before the command starts, on stderr as "code: hint":
2 usage, 3 refused (the hint or the last line says what to run), 4 no such
secret.

Example: k2 vault run --secret DATABASE_URL -- npm run migrate
Example: k2 vault run --secret STRIPE_KEY=STRIPE_API_KEY --env-file .env.k2 -- node app.js
EOF
            ;;
        proxy-url) cat <<'EOF'
k2 vault proxy-url NAME [--scope session] [--ttl 2h] [--json]

From a K2 terminal: a placeholder key and a loopback base URL for one
secret, so an SDK or script can use it and never hold the real key. Prints
two lines, ready for the environment:

  ANTHROPIC_API_KEY=k2v_ph_…
  ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/p/k2v_ph_…/anthropic

K2 takes the placeholder out of each request, adds the real key the way the
secret says, and calls the secret's one site. Responses stream back; an echo
of the key comes back as [k2v:NAME]. Works for any secret you may fetch that
has exactly one site (several sites: use k2 vault fetch).

The placeholder ends with this session (a daemon restart doesn't end it) or
at --ttl (90s, 10m, 1h, 7d). A process on this machine that gets hold of it
can call that one site through K2 until then, but never sees the key. Put it
only where an API key goes: in a query or path K2 refuses with 400
placeholder_misplaced rather than send it. A request body may hold it (a
chat history can); K2 passes the body on unchanged and never fills it in.

New sessions get these two variables by themselves when a secret's delivery
is proxy (k2 vault delivery NAME proxy, or grant --as proxy).

Example: set -a; eval "$(k2 vault proxy-url ANTHROPIC_KEY)"; set +a; python3 ask.py
EOF
            ;;
        file) cat <<'EOF'
k2 vault file NAME [--scope session] [--ttl 90s] [--wait 10m] [--json]
k2 vault file --done <path>

From a K2 terminal: the value of NAME, as is, in a short-lived 0600 file
in a memory-backed folder; prints the path. For a secret delivered to you
as a file (or env), e.g. one another agent handed you: a tool reads it
from the file or its stdin, so the value never goes on a command line
(ps shows command lines to every user) or into your transcript.
Removed after --ttl (90s, 10m or seconds; default 60s, 5s to 1h) or with
--done; a `< file` redirect opens it at once, so a sudo prompt after that
is fine. --secret NAME works too. Fetch-only secrets are refused (use k2
vault fetch); the refusal says who can widen it.

--wait 10m (90s, 2h, at most 24h): if the secret isn't there yet and an
open Thread card for NAME was asked by this session or is in the chat it
runs, block until a person saves it and print the path. --ttl starts when
the file is written. Exit 4 at once if no open card asks for NAME (ask with
k2 thread secret). Exit 5 when the card closed without a value:
card_dismissed (a person dismissed it; the line says who and why) or
card_withdrawn (it was withdrawn with k2 thread withdraw). Exit 3, and the
error line says which: card_expired, no_answer (your time ran out; the card
stays open and the line names it), or card_declined (an older K2 server's
word for a dismissed card). Example:
some-cli login --code-stdin < "$(k2 vault file CODE --wait 10m)"
A one-time code (k2 thread secret --once) is read here once, then deleted.

Example: sudo k2-node servers add --url URL --code - < "$(k2 vault file ENROLL_CODE)"
Example: some-cli login --token-stdin < "$(k2 vault file API_TOKEN)"
EOF
            ;;
        sync) cat <<'EOF'
k2 vault sync [--json]

From a K2 terminal: write every file secret this session is granted and
doesn't have yet into its $K2_SECRETS_DIR, now. K2 does the same by
itself right after a grant and once a minute; sync is for "now" and says
what's what: delivered, present, restart (env and ssh keys can't reach a
running session, nor can a file when the session has no $K2_SECRETS_DIR:
restart your session to receive it), skipped (exit 1).

Example: k2 vault sync && cat "$K2_SECRETS_DIR/DEPLOY_TOKEN"
EOF
            ;;
        import) cat <<'EOF'
k2 vault import <.env file> [--scope ws:<name>|server] [--delivery env|file|fetch] [--remove-original]

Move a .env's secret-looking values into the vault (the names k2 vault
doctor flags) and write .env.k2 beside it: the same lines with k2v://
references, safe to commit. Moved secrets use env delivery unless
--delivery says otherwise, so tools still find $NAME in new sessions.
The original stays unless --remove-original.

Example: k2 vault import .env --remove-original
EOF
            ;;
        service) cat <<'EOF'
k2 vault service <service> [clean|legacy] [--env-file .env.k2]

Show or set a k2 publish service's environment: clean (base environment +
granted secrets) or legacy (the daemon's whole environment; owner only,
being retired). --env-file points it at a .env.k2 of k2v:// references.
Restart the service to apply.
Give it a secret: k2 publish secret add <service> NAME [--as file|env]

Example: k2 vault service api clean --env-file .env.k2
EOF
            ;;
        reveal) cat <<'EOF'
k2 vault reveal NAME [--scope <s>]

People only, from your own terminal or SSH login (not a K2 terminal):
show a value after a confirm (Touch ID or the vault password).

Example: k2 vault reveal STRIPE_KEY

K2's own secrets are never shown, except Sew's launch key (the key Sew's
chat and voice data is sealed with), which the server's owner may see:
  k2 vault reveal sew-launch-key --scope engine:sew
Admins, Members and agents are refused.
EOF
            ;;
        recovery) cat <<'EOF'
k2 vault recovery status
k2 vault recovery show-once     people only: make and show the recovery key once, type back 3 words
k2 vault recovery rotate        people only: replace it (asks for a confirm)

The recovery key opens the vault if this box's key is lost. Agents: don't
run show-once or rotate; tell your owner.

Example: k2 vault recovery status
EOF
            ;;
        confirm-password) cat <<'EOF'
k2 vault confirm-password set

People only: set the vault password used for confirms where Touch ID
isn't available (10+ characters; asks for the old one's confirm first).

Example: k2 vault confirm-password set
EOF
            ;;
        kek|rekey) cat <<'EOF'
k2 vault kek rotate     rotate the box key that wraps the vault
k2 vault rekey          rotate the data key (re-encrypts every secret)

Owner only, from their own terminal, with a confirm.

Example: k2 vault kek rotate
EOF
            ;;
        rollback) cat <<'EOF'
k2 vault rollback NAME --to <version> [--scope <s>] [--accept-agent-value]

People only: put an older version's value back, as a new version (asks
for a confirm). Undoes an agent's rotate. When root units load the secret
(k2 vault systemd-cred), the confirm names them and the rollback re-places
the value there; a version no person wrote then needs --accept-agent-value
(and keeps its author, so it is never shown as yours).

Example: k2 vault rollback STRIPE_KEY --to 3
EOF
            ;;
        backup) cat <<'EOF'
k2 vault backup <dir>
k2 vault backup key <age1…>
k2 vault backup key remove

Owner only, from your own terminal. A ciphertext snapshot of the vault
into <dir>; it opens with the recovery words or a backup key. `backup key`
adds (or replaces) an age key that can open backups on a new box;
`backup key remove` takes it off. Both ask you to confirm, and
k2 vault status lists every key holder by fingerprint.

Example: k2 vault backup /Volumes/backup/k2
EOF
            ;;
        restore) cat <<'EOF'
k2 vault restore --from <dir> [--holder recovery|backup] [--dry-run] [--replace]

People only: restore a backup on this box. The recovery words (or the
backup key, --holder backup) come on stdin or are asked for. --dry-run
checks first; --replace overwrites a vault that already has secrets.

Example: k2 vault restore --from /Volumes/backup/k2 --dry-run
EOF
            ;;
        recipient) cat <<'EOF'
k2 vault recipient [--json]

This vault's carry address (an age recipient) and vault id. Run it on the
box secrets move TO, then k2 vault carry export on the old box.

Example: k2 vault recipient
EOF
            ;;
        public-key) cat <<'EOF'
k2 vault public-key NAME [--scope <s>] [--json]

Print an ssh key's public half (the .pub line, comment NAME) and its
SHA256 fingerprint, for authorized_keys or a deploy key. Never the
private key.

Example: k2 vault public-key DEPLOY_KEY >> authorized_keys.new
EOF
            ;;
        agent-limit) cat <<'EOF'
k2 vault agent-limit NAME [--burst N] [--per-minute M] [--allow-forwarded | --no-forwarded] [--scope <s>]

The ssh-agent settings of an ssh key with agent delivery. With no option,
prints them. Per session: at most --burst signatures a second (1-600,
default 60) and --per-minute (1-6000, default 240); over that, the
agent says no and the audit says rate_limited.
By default K2 won't sign when the ssh client says the connection is a
forwarded agent (ssh -A, then ssh from the other box): deny forwarded in
the audit. --allow-forwarded turns that off. Raising a limit or allowing
forwarding is widening: a secret a person gave you needs the owner.

Advisory, not a security control: agent delivery stops the key being
copied out, but while the session runs anything in it, or any same-user
process that finds the socket, can still sign with it. The forwarding
refusal and the host key in the audit come from what the ssh client
says; a client that says nothing, or a process that speaks the agent
protocol itself, can't be told apart.

Example: k2 vault agent-limit DEPLOY_KEY --burst 10 --per-minute 60
EOF
            ;;
        systemd-cred) cat <<'EOF'
k2 vault systemd-cred NAME --unit <unit.service> [--cred-name <id>] [--restart] [--dry-run]
k2 vault systemd-cred NAME --unit <unit.service> [--cred-name <id>] --remove
k2 vault systemd-cred NAME --replace [--dry-run]

People only, on Linux (systemd 250+), from your own terminal or SSH (not a
K2 terminal), with a confirm bound to exactly what you approve (the unit,
the cred name, the restart flag; K2 prints it first). K2 seals the
secret's current value as a systemd encrypted credential through the root
helper and binds it to a unit K2 doesn't run (a root service, a timer):
  /etc/k2/credstore.encrypted/<cred>.cred  (K2's own store; --with-key=auto)
  /etc/systemd/system/<unit>.d/k2-cred-<cred>.conf
      LoadCredentialEncrypted=<cred>:/etc/k2/credstore.encrypted/<cred>.cred
The service reads $CREDENTIALS_DIRECTORY/<cred> (%d/<cred> in the unit).
  --cred-name <id>  the credential's name (default: NAME; never a systemd
                    namespace such as tmpfiles.*, sysusers.*, ssh.*)
  --restart         queue systemctl try-restart after (at most every 30 s)
  --dry-run         show what it would write and the confirm text
  --remove          take it off: the drop-in, then the .cred
  --replace         after a rotate: re-place the new version into every
                    unit it is placed on (each keeps its own --restart)
  --accept-agent-value   place a version no person wrote (an agent's
                    rotate): review it first with k2 vault reveal NAME

The confirm names the exact version and who wrote it ("v5, written by an
agent of workspace ops, 3 min ago"); if the secret moves on before you
confirm, nothing is placed. An Owner or Admin can place a value into any
unit the box's admin listed. "Already loads" covers LoadCredential*,
SetCredential* and ImportCredential= (with systemd 257's GLOB:RENAME); a
credential source a later systemd adds is not checked.

The box's admin chooses which units may get one, once, as root over SSH:
  echo k2-watch.service >> /etc/k2-root-helper/systemd-cred-units
(root:root 0644, one unit per line; or install-root-helper.sh --creds-unit
k2-watch.service). Nothing else is allowed; K2's own units, core system
units, templates (foo@.service) and aliases are always refused, as is a
unit that already loads a credential with the same ID.

A new version (rotate, set, an agent's rotate) never reaches the unit by
itself: the placement goes stale, the owner is told, and a person runs
--replace. A rollback re-places the restored value. The secret's ceiling
must allow file delivery. k2 vault show, placements and doctor list it;
destroying a secret doesn't take its credential off a box: remove it first.

A unit that uses EnvironmentFile= with several variables: seal the whole
file as one credential (k2 vault set CP_ENV --kind generic < file, every
value in single quotes: KEY='value'), then a drop-in of your own:
  [Service]
  LoadCredentialEncrypted=k2c.env:/etc/k2/credstore.encrypted/k2c.env.cred
  ExecStart=
  ExecStart=/bin/sh -c 'set -a; . "%d/k2c.env"; set +a; exec /usr/local/bin/app'
The file is shell code run by the unit: keep it a person-only secret.
The ExecStart target must be on a filesystem mounted without noexec (on
Ubuntu /run, and often /tmp, are noexec).
Keep the old EnvironmentFile= until the unit runs on the credential (use
a cred name that differs from the old file's), then remove both.

Examples:
  k2 vault systemd-cred INGEST_KEY --unit k2-watch.service --restart
  k2 vault systemd-cred CP_ENV --unit k2-control-plane.service --cred-name k2c.env --dry-run
  k2 vault systemd-cred INGEST_KEY --replace
  k2 vault systemd-cred INGEST_KEY --unit k2-watch.service --remove
EOF
            ;;
        placement|placements) cat <<'EOF'
k2 vault placement add NAME --where <host>:<path> [--note <text>] [--scope <s>]
k2 vault placement done ID
k2 vault placement rm ID
k2 vault placements [NAME] [--json]

Where a secret's value lives outside the vault. K2 records the systemd
credentials it writes itself (k2 vault systemd-cred); add the places it
doesn't write: another box, Vercel prod, a box without K2. No value is
stored, only the place (at most 200 characters) and a note (500).
After a rotate the doctor lists each place as due until you put the new
value there and mark it: k2 vault placement done ID. Agents may keep
these for their own workspace's secrets; systemd credential rows are
people-only (k2 vault systemd-cred --remove).

Examples:
  k2 vault placement add INGEST_KEY --where vercel:prod/k2-web --note "Vercel prod env"
  k2 vault placement add INGEST_KEY --where box3:/etc/credstore.encrypted/ingest_key.cred
  k2 vault placements INGEST_KEY
  k2 vault placement done 3f2a…
EOF
            ;;
        carry) cat <<'EOF'
k2 vault carry export --recipient <age1…> --target <vault-id> --workspace <ws> [--target-workspace <id>] [--server NAME] [--own-box-move] --out <file>
k2 vault carry import <file> [--workspace <id>] [--apply]

Move secrets to another box (owner, Admin, or an agent allowed to manage
server-wide secrets). export seals them so only the target vault opens
them; import lists first and changes nothing until --apply. Workspace
secrets land by workspace id on the new box, never by name.

Example:
  new box: k2 vault recipient
  old box: k2 vault carry export --recipient age1… --target <vault-id> --workspace web --out carry.age
  new box: k2 vault carry import carry.age --workspace <id> --apply
EOF
            ;;
        *) _vault_usage ;;
    esac
}

