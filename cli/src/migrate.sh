# ── k2 migrate (box moves carry the vault; prd-box-move-carry-v1) ────

_migrate_help() {
    cat <<'EOF'
k2 migrate — move this K2 box's vault to new hardware (0.46.6+ on both boxes)

The vault (every secret, including the K2 Connect tunnel token and the mail
and database passwords) moves as one encrypted file. The old box's key never
leaves it: the new box makes a one-time move key, the old box exports its
vault to that key, and the new box re-wraps the vault to its own key.

THE STEPS (in this order)
  0. Final /home sync from the old box to the new one WITHOUT ~/.k2/vault/
     (rsync --exclude .k2/vault/): an old vault.db copied over an imported
     one breaks it (vault_substituted). Never copy
     /var/lib/k2-root-helper/keep/ (the old box's key stays there). Start
     the new box's daemon with its tunnel parked (k2 tunnel disable there).
  1. NEW box:  k2 migrate import-secrets begin
               prints the move key (age1…) and its fingerprint. Send both to
               the old box's owner. The key lasts 24 h or one import; begin
               again shows the same key (--new makes another, and files made
               for the old one then no longer open).
  2. OLD box:  stop its other agents and heartbeats, then
               k2 migrate export-secrets --to age1… --out ~/move.k2mv
               writes the file and FREEZES the old vault: reads keep working,
               writes wait (vault_moved_out). Run it as late as you can, right
               before the cut-over. Check the printed fingerprint is the one
               the new box showed.
  3. Copy the file from the old box to the new one (scp). It is encrypted to
     the move key. Delete the old box's copy of the file once step 5 worked.
  4. NEW box:  k2 migrate import-secrets ~/move.k2mv
               without --apply it is a DRY RUN (it prints "DRY RUN — nothing
               changed"): checks the file and lists what it holds (secret
               counts, tunnel token present or MISSING, systemd credentials to
               re-place). Anyone may run it.
  5. NEW box:  k2 migrate import-secrets ~/move.k2mv --apply
               replaces this box's vault (the old files go to
               ~/.k2/vault/replaced-<time>/), works even when this box's vault
               is closed (vault_key_missing). Then: k2 vault status (open,
               rollback: false, the same secret count).
  6. Re-place each systemd credential the import (and the dry run) lists:
               k2 vault systemd-cred NAME --replace
  The tunnel is never started by these steps; the token is inside the moved
  vault. Hand the tunnel over as your move runbook says (the old box's frpc
  stops first).

WHO
  export, import --apply, --undo and --unfreeze are for the box owner as a
  person, with the vault confirm (Touch ID, the vault password or a K2
  Connect owner login). Run from your own terminal or SSH login (not a K2
  terminal), k2 asks for the confirm. Anyone else (an agent, a script, an
  SSH session without a terminal) gets the step HELD as mv_… (exit 2, a
  "HELD: …" line on stderr; with --json, "status": "held" on stdout).
  Nothing runs yet:
    owner:  k2 migrate approve mv_…        (own terminal; asks the confirm)
    agent:  k2 permission request vault.move-approve request=mv_… --why "…"
            (a card on the owner's Thread; they approve with the confirm)
    anyone: k2 migrate status              (held steps: pending/done/failed,
                                           and the result once it ran)
  An approved step runs exactly as it was asked (an export writes its --out
  file then). A held step waits 24 h, then expires: run the step again.
  begin, status and the dry run need no approval (an agent may run them).
  On a box whose vault is closed there is no vault confirm: there the
  person's own terminal or SSH login (or a K2 Connect owner login) is the
  check.

CALLING IT OFF
  New box, before the cut-over (before it takes over the tunnel):
                                 k2 migrate import-secrets --undo
                                 (puts the replaced vault back; keeps the
                                 imported one in ~/.k2/vault/undone-<time>/)
  Old box:                       k2 migrate export-secrets --unfreeze
                                 (lifts the freeze and drops the move key)
  Nothing on the old box is ever deleted by a move.

REFUSALS (code: what to do)
  move_key_wrong         the file was made for another move key: compare the
                         fingerprints; export again to this box's key (k2
                         migrate status shows it; begin only if it expired)
  move_stale             a vault.db of the SAME vault on this box is newer than
                         the file (the old box was unfrozen and wrote, and that
                         vault.db reached this box): export again (the same
                         move key works while it lasts)
  move_file_bad          the file was changed or isn't a move file: copy again
  move_file_needs_newer  made by a newer K2: update K2 on this box
  no_move_key            run k2 migrate import-secrets begin first (24 h)
  kek_rotation_pending   the old box is mid key rotation: finish it first
  vault_moved_out (423)  this vault is frozen for a move (writes wait)
  vault_substituted      a vault.db made with another box's key is in
                         ~/.k2/vault/ (a sync copied it): import over it

COMMANDS
  k2 migrate status [--json]
  k2 migrate import-secrets begin [--new] [--json]
  k2 migrate export-secrets --to <age1…> --out <file> [--json]
  k2 migrate export-secrets --unfreeze [--json]
  k2 migrate import-secrets <file> [--apply] [--json]
  k2 migrate import-secrets --undo [--json]
  k2 migrate approve <mv_…> [--json]

Exit: 0 done, 2 held for the owner (or a usage error; --json tells them
apart), 3 refused, 4 not found.
More: k2 study box-move
EOF
}

# POST/GET a /cli/migrate/secrets route. Inside a K2 terminal it goes with
# the session's passport (an agent: held, with the permission card hint);
# elsewhere over vault.sock, where the daemon checks for a person.
_migrate_send() {
    local method="$1" route="$2" payload="${3:-}" tty="0" out
    if [ -n "${K2_HOOK_SOCK:-}${K2_HOOK_TOKEN:-}${K2SO_HOOK_SOCK:-}${K2SO_HOOK_TOKEN:-}" ] || [ ! -S "$(_vault_sock)" ]; then
        _sidecar_split "$(_sidecar_send "$method" "/cli/migrate/secrets/${route}" "$payload")"
        return 0
    fi
    [ -t 0 ] && [ -t 1 ] && tty="1"
    [ -t 0 ] && [ -t 2 ] && tty="1"
    if [ "$method" = "GET" ]; then
        out="$(curl -s -G --unix-socket "$(_vault_sock)" "http://localhost/cli/migrate/secrets/${route}" \
            -H "X-K2-Tty: ${tty}" --connect-timeout 5 --max-time 600 -w $'\n%{http_code}' 2>/dev/null)" || out=""
    else
        out="$(printf '%s' "$payload" | curl -s -X POST --unix-socket "$(_vault_sock)" "http://localhost/cli/migrate/secrets/${route}" \
            -H "X-K2-Tty: ${tty}" -H "Content-Type: application/json" --data-binary @- \
            --connect-timeout 5 --max-time 600 -w $'\n%{http_code}' 2>/dev/null)" || out=""
    fi
    _sidecar_split "$out"
}

# One gated step: send it; when the daemon asks for the vault confirm (a
# person at a terminal), get it and send again. A held step exits 2.
_migrate_step() {
    local route="$1" payload="$2" as_json="$3" render="$4"
    _migrate_send POST "$route" "$payload"
    if [ "$_SC_STATUS" = "403" ] && printf '%s' "$_SC_BODY" | grep -q '"vault_confirm_required"'; then
        local binding label tok
        binding="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("confirm") or {}).get("secretId") or "")')"
        label="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("confirm") or {}).get("label") or "")')"
        if [ -n "$binding" ]; then
            tok="$(_vault_confirm_token vault-move "$binding" "$label")" || exit 3
            payload="$(python3 -c 'import json,sys; o=json.loads(sys.argv[1]); o["confirmToken"]=sys.argv[2]; print(json.dumps(o))' "$payload" "$tok")"
            _migrate_send POST "$route" "$payload"
        fi
    fi
    _migrate_finish "$as_json" "$render"
}

# Print a migrate answer: --json raw; a hold → its hint, exit 2; a refusal
# → code: hint, exit 3/4/…; else the step's own rendering.
_migrate_finish() {
    local as_json="$1" render="${2:-plain}"
    if [ "$_SC_STATUS" = "202" ]; then
        if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; else
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("HELD: " + v.get("hint",""))' >&2
        fi
        exit 2
    fi
    if [ "$_SC_STATUS" != "200" ]; then
        if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit "$(_vault_exit_for "$_SC_STATUS")"; fi
        [ "$_SC_STATUS" = "000" ] && { echo "vault_unavailable: the K2 daemon didn't answer (is it running on this machine?)" >&2; exit 1; }
        _vault_fail "$_SC_STATUS" "$_SC_BODY"
    fi
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; return 0; fi
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
kind = sys.argv[1]
if kind == "plain":
    # k2 migrate approve: the held step it ran, by its answer.
    kind = "export" if "out" in v else "unfreeze" if "wasFrozen" in v else "undo" if "undone" in v else "import" if "replaced" in v else "plain"
def key(k):
    k = k or {}
    return "%s  (fingerprint %s, expires %s)" % (k.get("recipient"), k.get("fingerprint"), k.get("expiresAt"))
def creds(v):
    c = v.get("systemdCreds") or []
    if c:
        print("re-place:     " + ", ".join("%s (%s)" % (x.get("secret"), x.get("unit")) for x in c))
        print("              k2 vault systemd-cred NAME --replace, for each")
if kind == "begin":
    print(("new move key: " if v.get("created") else "move key:     ") + key(v.get("moveKey")))
    print("next:         on the OLD box: k2 migrate export-secrets --to %s --out ~/move.k2mv" % (v.get("moveKey") or {}).get("recipient"))
elif kind == "export":
    print("exported:     %s (%s bytes) for move key %s" % (v.get("out"), v.get("bytes"), v.get("moveKey")))
    s = v.get("secrets") or {}
    print("vault:        %s gen %s, %s yours / %s K2 system secrets" % (v.get("vaultId"), v.get("gen"), s.get("user"), s.get("system")))
    print("FROZEN:       this vault reads but refuses writes until the move is done or called off")
    print("next:         " + (v.get("next") or ""))
elif kind == "unfreeze":
    print("unfrozen:     writes work again; the move key holder is gone" if v.get("wasFrozen") else "not frozen")
elif kind == "dry":
    s = v.get("secrets") or {}
    src = v.get("source") or {}
    print("DRY RUN — nothing changed")
    print("vault:        %s gen %s, %s yours / %s K2 system secrets" % (v.get("vaultId"), v.get("gen"), s.get("user"), s.get("system")))
    print("from:         %s (%s%s), exported %s" % (src.get("host") or "?", v.get("sourceKeyHome"), (", tunnel " + src["tunnelSubdomain"]) if src.get("tunnelSubdomain") else "", v.get("exportedAt")))
    print("tunnel token: " + str(v.get("tunnelToken")))
    creds(v)
    r = v.get("replaces") or {}
    if r.get("open"):
        print("replaces:     this box’s vault %s (%s yours); it is kept in ~/.k2/vault/replaced-<time>/" % (r.get("vaultId"), r.get("userSecrets")))
    else:
        print("replaces:     this box’s closed vault (%s); kept in ~/.k2/vault/replaced-<time>/" % r.get("error"))
    print("next:         " + (v.get("next") or ""))
elif kind == "import":
    s = v.get("secrets") or {}
    print("imported:     %s gen %s, key home %s, rollback %s" % (v.get("vaultId"), v.get("gen"), v.get("keyHome"), "yes" if v.get("rollback") else "no"))
    print("secrets:      %s yours / %s K2 system" % (s.get("user"), s.get("system")))
    print("tunnel token: " + str(v.get("tunnelToken")))
    creds(v)
    print("replaced:     " + str(v.get("replaced")))
    for line in v.get("next") or []:
        print("next:         " + line)
elif kind == "undo":
    vv = v.get("vault") or {}
    print("undone:       the import of %s; the replaced vault is back (%s)" % (v.get("undone"), "open" if vv.get("open") else "closed: " + str(vv.get("error"))))
    print(v.get("note") or "")
elif kind == "status":
    vv = v.get("vault") or {}
    if vv.get("open"):
        print("vault:        open (%s, key home %s)" % (vv.get("vaultId"), vv.get("keyHome")))
    else:
        e = vv.get("error") or {}
        print("vault:        CLOSED — %s: %s" % (e.get("code"), e.get("hint")))
    f = v.get("frozen")
    print("frozen:       " + ("YES since %s for move key %s (k2 migrate export-secrets --unfreeze to call it off)" % (f.get("at"), f.get("moveKey")) if f else "no"))
    print("move key:     " + (key(v.get("moveKey")) if v.get("moveKey") else "none (k2 migrate import-secrets begin)"))
    li = v.get("lastImport")
    print("last import:  " + ("%s at %s (undo: k2 migrate import-secrets --undo)" % (li.get("vaultId"), li.get("at")) if li else "none"))
    for h in v.get("held") or []:
        print("held:         %s %-8s %-8s by %s, expires %s" % (h.get("id"), h.get("step"), h.get("status"), h.get("by"), h.get("expiresAt")))
else:
    print(json.dumps(v, indent=2))
' "$render"
}

cmd_migrate() {
    local sub="${1:-}"
    [ $# -gt 0 ] && shift
    case "$sub" in
        ""|-h|--help|help) _migrate_help; return 0 ;;
        status)
            local as_json=0
            [ "${1:-}" = "--json" ] && as_json=1
            _migrate_send GET status ""
            _migrate_finish "$as_json" status
            ;;
        approve)
            local id="" as_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=1; shift ;;
                    -h|--help) echo "k2 migrate approve <mv_…> — run a held move step (box owner, own terminal; asks the vault confirm)"; return 0 ;;
                    -*) echo "usage: k2 migrate approve <mv_…> [--json]" >&2; exit 2 ;;
                    *) id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "usage: k2 migrate approve <mv_…> (k2 migrate status lists held steps)" >&2; exit 2; }
            local kind="plain"
            _migrate_step approve "$(python3 -c 'import json,sys; print(json.dumps({"request": sys.argv[1]}))' "$id")" "$as_json" "$kind"
            ;;
        export-secrets)
            local to="" out="" unfreeze=0 as_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --to) to="${2:-}"; shift 2 ;;
                    --out|-o) out="${2:-}"; shift 2 ;;
                    --unfreeze) unfreeze=1; shift ;;
                    --json) as_json=1; shift ;;
                    -h|--help) _migrate_help; return 0 ;;
                    *) echo "usage: k2 migrate export-secrets --to <age1…> --out <file> | --unfreeze (k2 migrate --help)" >&2; exit 2 ;;
                esac
            done
            if [ "$unfreeze" = "1" ]; then
                _migrate_step unfreeze '{}' "$as_json" unfreeze
                return 0
            fi
            [ -n "$to" ] && [ -n "$out" ] || { echo "usage: k2 migrate export-secrets --to <age1… from the new box> --out <file>" >&2; exit 2; }
            case "$out" in /*) ;; *) out="$(pwd)/$out" ;; esac
            _migrate_step export "$(python3 -c 'import json,sys; print(json.dumps({"to": sys.argv[1], "out": sys.argv[2]}))' "$to" "$out")" "$as_json" export
            ;;
        import-secrets)
            local file="" apply=0 undo=0 new=0 as_json=0 begin=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    begin) begin=1; shift ;;
                    --new) new=1; shift ;;
                    --apply) apply=1; shift ;;
                    --undo) undo=1; shift ;;
                    --json) as_json=1; shift ;;
                    -h|--help) _migrate_help; return 0 ;;
                    -*) echo "usage: k2 migrate import-secrets begin | <file> [--apply] | --undo (k2 migrate --help)" >&2; exit 2 ;;
                    *) file="$1"; shift ;;
                esac
            done
            if [ "$begin" = "1" ]; then
                _migrate_send POST begin "$(python3 -c 'import json,sys; print(json.dumps({"new": sys.argv[1] == "1"}))' "$new")"
                _migrate_finish "$as_json" begin
                return 0
            fi
            if [ "$undo" = "1" ]; then
                _migrate_step undo '{}' "$as_json" undo
                return 0
            fi
            [ -n "$file" ] || { echo "usage: k2 migrate import-secrets <file> [--apply] (or: begin, --undo)" >&2; exit 2; }
            case "$file" in /*) ;; *) file="$(pwd)/$file" ;; esac
            local body
            body="$(python3 -c 'import json,sys; print(json.dumps({"file": sys.argv[1], "apply": sys.argv[2] == "1"}))' "$file" "$apply")"
            if [ "$apply" = "1" ]; then
                _migrate_step import "$body" "$as_json" import
            else
                _migrate_send POST import "$body"
                _migrate_finish "$as_json" dry
            fi
            ;;
        *)
            echo "usage: k2 migrate status | import-secrets … | export-secrets … | approve mv_… (k2 migrate --help)" >&2
            exit 2
            ;;
    esac
}

# Every verb `k2 vault` knows (cmd_vault and the dispatchers it calls). A
# new verb goes here too: a word not listed is an unknown verb, with or
# without --help (prd-vault-for-agents-v1 X9: `k2 vault exec --help` used to
# print the top help and exit 0, so an agent took the guess as real).
_VAULT_VERBS=" status list ls show set rotate hosts sign public-key agent-limit delivery grant revoke open disable enable destroy get audit requests presets doctor env-file file proxy-url sync import service reveal recovery confirm-password kek rekey rollback recipient carry backup restore systemd-cred placement placements fetch send incoming sent accept-from send-allow ask asks unsend run "

# k2 vault run (prd-vault-for-agents-v1 A1): parse the flags before `--`, ask
# the daemon where its binary is, and exec that as the helper. Policy, audit
# and per-run files are the daemon's; the masking is the helper's. Only names
# go on the helper's argv.
_vault_run() {
    local usage="usage: k2 vault run [--secret NAME[=ENV_NAME]]... [--env-file .env.k2] [--scope workspace|server|session] [--no-mask] -- cmd [args…]"
    local helper_args=() named=0 dashes=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --help|-h) _vault_sub_usage run; return 0 ;;
            --) dashes=1; shift; break ;;
            --secret)
                [ -n "${2:-}" ] || { echo "usage: --secret NAME[=ENV_NAME]" >&2; exit 2; }
                helper_args+=(--secret "$2"); named=1; shift 2 ;;
            --env-file)
                [ -n "${2:-}" ] || { echo "usage: --env-file <file>" >&2; exit 2; }
                helper_args+=(--env-file "$2"); named=1; shift 2 ;;
            --scope)
                [ -n "${2:-}" ] || { echo "usage: --scope workspace|server|session" >&2; exit 2; }
                helper_args+=(--scope "$2"); shift 2 ;;
            --no-mask) helper_args+=(--no-mask); shift ;;
            --value|--value=*|--secret-value*)
                echo "value_on_argv: run takes names, never values (k2 vault run --help)" >&2
                exit 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 vault run --help)" >&2; exit 2 ;;
            # NAME like `k2 vault file NAME`; --secret NAME works too.
            *) helper_args+=(--secret "$1"); named=1; shift ;;
        esac
    done
    if [ "$dashes" != 1 ] || [ $# -eq 0 ] || [ "$named" != 1 ]; then
        echo "$usage" >&2
        echo "Put -- before the command: k2 vault run --secret API_KEY -- node app.js" >&2
        exit 2
    fi
    _vault_call GET run-helper ''
    local helper
    helper="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("helper") or "")')"
    if [ -z "$helper" ] || [ ! -x "$helper" ]; then
        echo "run_helper_missing: the K2 daemon named no usable helper; restart the K2 daemon and try again" >&2
        exit 1
    fi
    K2_HOOK_SOCK="$(_hook_sock)" K2_HOOK_TOKEN="${SCOPED_TOKEN:-${K2_HOOK_TOKEN:-}}" exec "$helper" --vault-run "${helper_args[@]}" -- "$@"
}

_vault_unknown_verb() {
    local sub="$1" try=""
    case "$sub" in
        # A command that needs its secrets.
        exec|env|inject) try="run" ;;
    esac
    echo "usage: unknown vault verb '$sub' (k2 vault --help)" >&2
    [ -z "$try" ] || echo "did you mean $try? (k2 vault $try --help)" >&2
    exit 2
}

cmd_vault() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _vault_usage; return 0 ;;
    esac
    shift
    case "$_VAULT_VERBS" in
        *" $sub "*) ;;
        *) _vault_unknown_verb "$sub" ;;
    esac
    if [ "$sub" = "fetch" ]; then
        _vault_fetch "$@"
        return
    fi
    # Before the --help scan: the words after `--` are the child's own.
    if [ "$sub" = "run" ]; then
        _vault_run "$@"
        return
    fi
    case "$sub" in
        send|incoming|sent|accept-from|send-allow|ask|asks|unsend) _vault_handoff "$sub" "$@"; return ;;
    esac
    if _wants_help "$@"; then
        _vault_sub_usage "$sub"
        return 0
    fi
    case "$sub" in
        systemd-cred) _vault_systemd_cred "$@"; return ;;
        placement|placements) _vault_placement "$sub" "$@"; return ;;
        sync) _vault_sync "$@"; return ;;
    esac
    local as_json=0 scope="" system=0 kind="" hosts="" preset="" delivery="" ceiling="" describe="" rotate_every="" ttl=""
    local inject_header="" inject_format="" inject_dst="" to="" from="" as_d="" env_name="" file_name="" grant_id="" yes=0 since="" to_version=""
    local sign_scheme="" sign_header="" sign_encoding="" sign_prefix="" sign_ts_header="" sign_template="" sign_value=""
    local sign_prefix_set=0 sign_template_set=0 sign_value_set=0
    local oauth_token_url="" oauth_client_auth=""
    local agent_burst="" agent_per_minute="" allow_fwd=0 no_fwd=0
    local remove_original=0 env_file_opt="" done_path="" ttl_secs="" wait_for="" transcripts=0
    local carry_recipient="" carry_target="" carry_target_ws="" own_box_move=0 out_file="" holder="" dry_run=0 replace=0 apply=0 accept_agent=0
    local add_hosts=() rm_hosts=() pos=() use_secrets=() carry_ws=() carry_server=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --system) system=1; shift ;;
            --yes) yes=1; shift ;;
            --remove-original) remove_original=1; shift ;;
            --own-box-move) own_box_move=1; shift ;;
            --transcripts)
                [ "$sub" = "doctor" ] || { echo "usage: --transcripts goes with k2 vault doctor" >&2; exit 2; }
                transcripts=1; shift ;;
            --dry-run) dry_run=1; shift ;;
            --replace) replace=1; shift ;;
            --apply) apply=1; shift ;;
            --accept-agent-value) accept_agent=1; shift ;;
            --allow-forwarded) allow_fwd=1; shift ;;
            --no-forwarded) no_fwd=1; shift ;;
            --burst|--per-minute)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a number" >&2; exit 2; }
                case "$2" in *[!0-9]*) echo "usage: $1 needs a number" >&2; exit 2 ;; esac
                if [ "$1" = "--burst" ]; then agent_burst="$2"; else agent_per_minute="$2"; fi
                shift 2 ;;
            --recipient|--target|--target-workspace|--workspace|--server|--out|--holder)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --recipient) carry_recipient="$2" ;; --target) carry_target="$2" ;; --target-workspace) carry_target_ws="$2" ;;
                    --workspace) carry_ws+=("$2") ;; --server) carry_server+=("$2") ;;
                    --out) out_file="$2" ;; --holder) holder="$2" ;;
                esac
                shift 2 ;;
            --secret) [ -n "${2:-}" ] || { echo "usage: --secret NAME" >&2; exit 2; }; use_secrets+=("$2"); shift 2 ;;
            --sign|--sign-header|--sign-encoding|--sign-ts-header|--oauth-token-url|--oauth-client-auth)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --sign) sign_scheme="$2" ;; --sign-header) sign_header="$2" ;;
                    --sign-encoding) sign_encoding="$2" ;; --sign-ts-header) sign_ts_header="$2" ;;
                    --oauth-token-url) oauth_token_url="$2" ;; --oauth-client-auth) oauth_client_auth="$2" ;;
                esac
                shift 2 ;;
            --sign-prefix|--sign-template|--sign-value)
                # May be empty (--sign-prefix ''), so only the count is checked.
                [ $# -ge 2 ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --sign-prefix) sign_prefix="$2"; sign_prefix_set=1 ;;
                    --sign-template) sign_template="$2"; sign_template_set=1 ;;
                    --sign-value) sign_value="$2"; sign_value_set=1 ;;
                esac
                shift 2 ;;
            --env-file) [ -n "${2:-}" ] || { echo "usage: --env-file <file>" >&2; exit 2; }; env_file_opt="$2"; shift 2 ;;
            --wait) [ -n "${2:-}" ] || { echo "usage: --wait <duration> (10m, 2h)" >&2; exit 2; }; wait_for="$2"; shift 2 ;;
            --done) [ -n "${2:-}" ] || { echo "usage: --done <path>" >&2; exit 2; }; done_path="$2"; shift 2 ;;
            --scope|--kind|--for|--preset|--delivery|--ceiling|--describe|--rotate-every|--ttl|--inject-header|--inject-format|--inject-dst|--to|--from|--as|--env-name|--file-name|--grant|--add|--remove|--since)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --scope) scope="$2" ;; --kind) kind="$2" ;; --for) hosts="$2" ;; --preset) preset="$2" ;;
                    --delivery) delivery="$2" ;; --ceiling) ceiling="$2" ;; --describe) describe="$2" ;;
                    --rotate-every) rotate_every="$2" ;; --ttl) ttl="$2" ;; --inject-header) inject_header="$2" ;;
                    --inject-format) inject_format="$2" ;; --inject-dst) inject_dst="$2" ;; --to)
                        if [ "$sub" = "rollback" ]; then to_version="$2"; else to="$2"; fi ;;
                    --from) from="$2" ;; --as) as_d="$2" ;; --env-name) env_name="$2" ;; --file-name) file_name="$2" ;;
                    --grant) grant_id="$2" ;; --add) add_hosts+=("$2") ;; --remove) rm_hosts+=("$2") ;; --since) since="$2" ;;
                esac
                shift 2 ;;
            --value|--value=*|--secret-value*)
                echo "value_on_argv: values are read from stdin, never an argument (k2 vault set NAME < file)" >&2
                exit 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 vault --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local sj
    sj="$(_vault_scope_json "$scope")" || exit 2
    local name="${pos[0]:-}"
    local nj
    nj="$(python3 -c 'import json,sys; print(json.dumps({"name": sys.argv[1]} if sys.argv[1] else {}))' "$name")"
    [ "$system" = "1" ] && nj="$(_vault_merge "$nj" '{"system": true}')"
    case "$sub" in
        status)
            _vault_call GET status
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            _vault_render_status
            ;;
        list|ls)
            local q=""
            [ "$system" = "1" ] && q="system=1"
            if [ -n "$scope" ]; then
                q="${q}${q:+&}$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); print(urllib.parse.urlencode(o))' "$sj")"
            fi
            _vault_call GET list "$q"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            _vault_render_list
            ;;
        show)
            [ -n "$name" ] || { echo "usage: k2 vault show NAME [--scope <s>]" >&2; exit 2; }
            _vault_call GET show "$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); o.update(json.loads(sys.argv[2])); print(urllib.parse.urlencode(o))' "$sj" "$nj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin); m = v.get("secret") or {}
print("%s  (%s, %s, v%s, %s)" % (m.get("name"), m.get("kind"), m.get("status"), m.get("latestVersion"), m.get("ref") or "system"))
print("sites:     %s" % (", ".join(m.get("hosts") or []) or "none (fetch is refused until a site is added)"))
inj = m.get("inject") or {}
sg = inj.get("sign")
if sg:
    sch = sg.get("scheme")
    if sch == "ovh":
        print("signs:     ovh (X-Ovh-Application, X-Ovh-Consumer, X-Ovh-Timestamp, X-Ovh-Signature; OVH clock)")
    elif sch == "oauth-refresh":
        print("signs:     oauth-refresh (K2 refreshes the token at %s; client auth %s)" % (sg.get("tokenUrl"), sg.get("clientAuth", "post")))
        oa = m.get("oauth") or {}
        print("sign-in:   %s" % (oa.get("message") or ("ok, token good until " + oa["expiresAt"] if oa.get("expiresAt") else "ok")))
    elif sch == "hmac-sha256":
        print("signs:     hmac-sha256 over the body → %s: %s<%s>" % (sg.get("header"), sg.get("prefix",""), sg.get("encoding","hex")))
    else:
        print("signs:     hmac-sha256-ts over %s → %s: %s (%s)%s" % (sg.get("template"), sg.get("header"), sg.get("valueFmt"), sg.get("encoding","hex"), (", " + sg["tsHeader"] + ": {ts}") if sg.get("tsHeader") else ""))
elif inj: print("goes in:   %s as %s" % (inj.get("dst"), inj.get("fmt")))
print("delivery:  %s (at most %s)%s" % (m.get("defaultDelivery"), m.get("deliveryCeiling"), " — agents can see it" if m.get("agentCanSee") else ""))
print("given by:  %s%s" % (m.get("createdBy"), " (a person)" if m.get("humanGiven") else ""))
ho = m.get("handoff") or {}
if ho:
    print("sent by:   %s (%s)%s" % (ho.get("from"), ho.get("deliveryId"), (", accepted by %s up to %s" % (ho.get("decidedBy"), ho["acceptedCeiling"])) if ho.get("acceptedCeiling") else ""))
if v.get("use"): print("use:       %s" % v["use"])
if v.get("canTake"): print("more:      %s" % v["canTake"])
ap = v.get("agentPolicy")
if ap:
    print("agent:     at most %s signatures a second, %s a minute per session; forwarded agents %s" % (ap.get("burst"), ap.get("perMinute"), "allowed" if ap.get("allowForwarded") else "refused"))
for g in v.get("grants") or []:
    print("shared:    %s as %s%s%s" % (g.get("consumerLabel"), g.get("delivery"), (" env " + g["envName"]) if g.get("envName") else "", (" until " + g["expiresAt"]) if g.get("expiresAt") else ""))
for p in v.get("placements") or []:
    where = ("systemd credential %s on %s" % (p.get("credName"), p.get("unit"))) if p.get("target") == "systemd_cred" else (p.get("where") or "")
    st = p.get("state")
    tail = {"due": " — due: put v%s there, then k2 vault placement done %s" % (p.get("latestVersion"), p.get("id")),
            "stale": " — stale: k2 vault systemd-cred %s --unit %s" % (m.get("name"), p.get("unit")),
            "inactive": " — the secret is %s; take it off" % p.get("secretStatus")}.get(st, "")
    print("placed:    %s (v%s)%s" % (where, p.get("version"), tail))
for a in (v.get("audit") or [])[:10]:
    print("  %s %s %s %s%s" % (a.get("ts"), a.get("action"), a.get("outcome"), a.get("actor"), (" x%d" % a["count"]) if a.get("count",1) > 1 else ""))
'
            ;;
        set)
            [ -n "$name" ] || { echo "usage: k2 vault set NAME [options] < value" >&2; exit 2; }
            [ "${#pos[@]}" -eq 1 ] || { echo "value_on_argv: values are read from stdin, never an argument (k2 vault set NAME < file)" >&2; exit 2; }
            local opts sjson
            sjson="$(_vault_sign_json "$sign_scheme" "$sign_header" "$sign_encoding" "$sign_prefix" "$sign_prefix_set" "$sign_ts_header" "$sign_template" "$sign_template_set" "$sign_value" "$sign_value_set" "$oauth_token_url" "$oauth_client_auth")" || exit 2
            opts="$(python3 -c '
import json, sys
kind, hosts, preset, delivery, ceiling, describe, rot, ttl, ih, iformat, idst, sign = sys.argv[1:13]
if sign and (ih or iformat or idst):
    print("usage: a signed secret has no --inject-*: K2 sets the signer'"'"'s own headers", file=sys.stderr); sys.exit(2)
if ih and idst:
    print("usage: --inject-header X is --inject-dst header:X; give one of them", file=sys.stderr); sys.exit(2)
o = {}
if kind: o["kind"] = kind
if hosts: o["hosts"] = [h.strip() for h in hosts.split(",") if h.strip()]
if preset: o["preset"] = preset
if delivery: o["delivery"] = delivery
if ceiling: o["ceiling"] = ceiling
if describe: o["description"] = describe
if rot: o["rotateEveryDays"] = int(rot)
# As typed (90s, 10m, 2h, 7d or seconds): the daemon parses it (k2_vault::duration).
if ttl: o["ttl"] = ttl
if ih or iformat or idst:
    o["inject"] = {"dst": idst or ("header:" + (ih or "Authorization")), "fmt": iformat or "{}"}
if sign:
    o["inject"] = {"sign": json.loads(sign)}
print(json.dumps(o))' "$kind" "$hosts" "$preset" "$delivery" "$ceiling" "$describe" "$rotate_every" "$ttl" "$inject_header" "$inject_format" "$inject_dst" "$sjson")" || exit 2
            local body
            body="$(_vault_body_with_value "$(_vault_merge "$sj" "$nj" "$opts")")" || exit 2
            _vault_call POST set "$body"
            body=""
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin); m = v.get("secret") or {}
print("%s %s → %s (v%s)" % ("saved" if v.get("created") else "new version of", m.get("name"), v.get("ref"), m.get("latestVersion")))
sg = (m.get("inject") or {}).get("sign")
if sg: print("signs requests: %s (k2 vault show %s)" % (sg.get("scheme"), m.get("name")))
if v.get("trimmed"): print("(trimmed spaces/newlines around the value)")
print("use: " + (v.get("use") or ""))
if v.get("ttlCut"): print(v.get("ttlNote") or "kept for 7 d (the most for one session)")'
            ;;
        rotate)
            [ -n "$name" ] || { echo "usage: k2 vault rotate NAME < value" >&2; exit 2; }
            [ "${#pos[@]}" -eq 1 ] || { echo "value_on_argv: values are read from stdin, never an argument" >&2; exit 2; }
            local body
            body="$(_vault_body_with_value "$(_vault_merge "$sj" "$nj")")" || exit 2
            _vault_call POST rotate "$body"
            body=""
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s is now v%s" % ((v.get("secret") or {}).get("name"), v.get("version")))
for r in v.get("replaced") or []:
    if r.get("ok"):
        print("re-placed into %s (systemd credential %s)%s" % (r.get("unit"), r.get("credName"), ", restart queued" if r.get("restarted") else ""))
    else:
        e = r.get("error") or {}
        print("NOT re-placed into %s: %s" % (r.get("unit"), e.get("hint")))'
            ;;
        hosts)
            [ -n "$name" ] || { echo "usage: k2 vault hosts NAME --add <host> | --remove <host>" >&2; exit 2; }
            local hj
            hj="$(python3 -c 'import json,sys; a=[x for x in sys.argv[1].split("\n") if x]; r=[x for x in sys.argv[2].split("\n") if x]; print(json.dumps({"addHosts": a, "removeHosts": r}))' \
                "$(printf '%s\n' "${add_hosts[@]+"${add_hosts[@]}"}")" "$(printf '%s\n' "${rm_hosts[@]+"${rm_hosts[@]}"}")")"
            _vault_call POST policy "$(_vault_merge "$sj" "$nj" "$hj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; m=json.load(sys.stdin).get("secret") or {}; print("%s sites: %s" % (m.get("name"), ", ".join(m.get("hosts") or []) or "none"))'
            ;;
        sign)
            [ -n "$name" ] && [ -n "${pos[1]:-}" ] || { echo "usage: k2 vault sign NAME ovh|hmac-sha256|hmac-sha256-ts|none [--sign-header …] (k2 vault sign --help)" >&2; exit 2; }
            [ -z "$sign_scheme" ] || { echo "usage: the scheme is the second word: k2 vault sign NAME ${sign_scheme}" >&2; exit 2; }
            local gj
            if [ "${pos[1]}" = "none" ]; then
                if [ -n "$sign_header$sign_encoding$sign_ts_header$oauth_token_url$oauth_client_auth" ] || [ "$sign_prefix_set$sign_template_set$sign_value_set" != "000" ]; then
                    echo "usage: k2 vault sign NAME none takes no --sign-* options" >&2; exit 2
                fi
                gj='{"sign": "none"}'
            else
                gj="$(_vault_sign_json "${pos[1]}" "$sign_header" "$sign_encoding" "$sign_prefix" "$sign_prefix_set" "$sign_ts_header" "$sign_template" "$sign_template_set" "$sign_value" "$sign_value_set" "$oauth_token_url" "$oauth_client_auth")" || exit 2
                gj="$(python3 -c 'import json,sys; print(json.dumps({"sign": json.loads(sys.argv[1])}))' "$gj")"
            fi
            _vault_call POST policy "$(_vault_merge "$sj" "$nj" "$gj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; m=json.load(sys.stdin).get("secret") or {}; sg=(m.get("inject") or {}).get("sign"); print("%s: %s" % (m.get("name"), ("signs requests: " + sg.get("scheme")) if sg else "no signer (fetch adds the value as set)"))'
            ;;
        public-key)
            [ -n "$name" ] || { echo "usage: k2 vault public-key NAME [--scope <s>]" >&2; exit 2; }
            _vault_call GET public-key "$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); o.update(json.loads(sys.argv[2])); print(urllib.parse.urlencode(o))' "$sj" "$nj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["publicKey"]); print("# %s" % v.get("fingerprint", ""), file=sys.stderr)'
            ;;
        agent-limit)
            [ -n "$name" ] || { echo "usage: k2 vault agent-limit NAME [--burst N] [--per-minute M] [--allow-forwarded|--no-forwarded]" >&2; exit 2; }
            [ "$allow_fwd$no_fwd" != "11" ] || { echo "usage: give --allow-forwarded or --no-forwarded, not both" >&2; exit 2; }
            local aj
            aj="$(python3 -c 'import json,sys; o={}
if sys.argv[1]: o["burst"]=int(sys.argv[1])
if sys.argv[2]: o["perMinute"]=int(sys.argv[2])
if sys.argv[3]=="1": o["allowForwarded"]=True
if sys.argv[4]=="1": o["allowForwarded"]=False
print(json.dumps(o))' "$agent_burst" "$agent_per_minute" "$allow_fwd" "$no_fwd")"
            _vault_call POST agent-policy "$(_vault_merge "$sj" "$nj" "$aj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); m=v.get("secret") or {}; p=v.get("agentPolicy") or {}
print("%s: at most %s signatures a second, %s a minute per session; forwarded agents (ssh -A) %s" % (m.get("name"), p.get("burst"), p.get("perMinute"), "allowed" if p.get("allowForwarded") else "refused"))'
            ;;
        delivery)
            [ -n "$name" ] && [ -n "${pos[1]:-}" ] || { echo "usage: k2 vault delivery NAME fetch|agent|proxy|file|env|api [--ceiling <d>]" >&2; exit 2; }
            # A secret has no env/file name of its own; only a grant does.
            # Refuse rather than drop the flag and print success.
            if [ -n "$env_name" ] || [ -n "$file_name" ]; then
                local nflag="--env-name" nval="$env_name" nas="env"
                [ -n "$env_name" ] || { nflag="--file-name"; nval="$file_name"; nas="file"; }
                echo "usage: k2 vault delivery doesn't take $nflag; a secret has no $nas name of its own. Name it on a grant:" >&2
                echo "  k2 vault grant ${name} --to ws:<name> --as ${nas} ${nflag} ${nval}" >&2
                exit 2
            fi
            local dj
            dj="$(python3 -c 'import json,sys; o={"delivery": sys.argv[1]};
if sys.argv[2]: o["ceiling"]=sys.argv[2]
print(json.dumps(o))' "${pos[1]}" "$ceiling")"
            _vault_call POST policy "$(_vault_merge "$sj" "$nj" "$dj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; m=json.load(sys.stdin).get("secret") or {}; print("%s: %s (at most %s)" % (m.get("name"), m.get("defaultDelivery"), m.get("deliveryCeiling")))'
            ;;
        grant)
            [ -n "$name" ] && [ -n "$to" ] || { echo "usage: k2 vault grant NAME --to ws:<name>|ws:*|session:<id>|service:<label>|job:<name>|compute:<ws>/<node> [--as <d>] [--ttl 2h]" >&2; exit 2; }
            local gj
            # --ttl goes as typed (90s, 10m, 2h, 7d); the daemon turns it
            # into the share's end (V9: it used to be dropped here).
            gj="$(python3 -c 'import json,sys; o={"to": sys.argv[1]}
for k,v in (("as",sys.argv[2]),("envName",sys.argv[3]),("fileName",sys.argv[4]),("ttl",sys.argv[5])):
    if v: o[k]=v
print(json.dumps(o))' "$to" "$as_d" "$env_name" "$file_name" "$ttl")"
            _sidecar_split "$(_sidecar_send POST /cli/vault/grant "$(_vault_merge "$sj" "$nj" "$gj")")"
            # VC-B2: a grant they can read (file, env, api) from the owner
            # token needs a person's confirm, like reveal. From your own
            # terminal: confirm here (Touch ID or the vault password) and
            # grant over the vault socket. A K2 terminal can't.
            if [ "$_SC_STATUS" = "403" ] && printf '%s' "$_SC_BODY" | grep -q '"vault_confirm_required"' \
                && [ -z "${K2_HOOK_SOCK:-}${K2_HOOK_TOKEN:-}${K2SO_HOOK_SOCK:-}${K2SO_HOOK_TOKEN:-}" ] && [ -S "$(_vault_sock)" ]; then
                _vault_sock_call GET "show?$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); o.update(json.loads(sys.argv[2])); print(urllib.parse.urlencode(o))' "$sj" "$nj")"
                [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
                local gsid gtok
                gsid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["secret"]["id"])')"
                echo "Sharing ${name} this way lets ${to} read the value: confirm to go ahead (nothing is shared if you cancel)." >&2
                gtok="$(_vault_confirm_token grant-read "$gsid" "share $name with $to")" || exit 3
                _vault_sock_call POST grant "$(_vault_merge "$sj" "$nj" "$gj" "$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1]}))' "$gtok")")"
            fi
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            # --ttl asked for an end: a server older than 0.46.11 drops it
            # and makes a share that never ends. Say so, never "shared".
            if [ -n "$ttl" ] && ! printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; sys.exit(0 if (json.load(sys.stdin).get("grant") or {}).get("expiresAt") else 1)'; then
                local ngid
                ngid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("grant") or {}).get("id") or "")')"
                echo "grant_ttl_ignored: this K2 server made the share WITHOUT an end time (it is older than 0.46.11 and drops --ttl). Take it back when you're done: k2 vault revoke --grant ${ngid}" >&2
                exit 1
            fi
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
g = json.load(sys.stdin).get("grant") or {}
print("shared %s with %s%s" % (sys.argv[1], sys.argv[2], (" until " + g["expiresAt"]) if g.get("expiresAt") else ""))' "$name" "$to"
            # What it means for whoever got it (an App: when it arrives).
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin).get("reach")
if r: print("note: %s" % r)'
            # A compute: grant to a node another K2 server owns.
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
w = json.load(sys.stdin).get("warning")
if w: sys.stderr.write(w + "\n")'
            ;;
        revoke)
            local rj
            rj="$(python3 -c 'import json,sys; o={}
if sys.argv[1]: o["grantId"]=sys.argv[1]
if sys.argv[2]: o["from"]=sys.argv[2]
print(json.dumps(o))' "$grant_id" "$from")"
            _vault_call POST revoke "$(_vault_merge "$sj" "$nj" "$rj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            if [ -n "$grant_id" ]; then
                echo "share $grant_id removed; the secret itself is still in the vault"
            else
                echo "${name}: no longer shared with ${from}; the secret itself is still in the vault"
            fi
            _vault_print_recall
            # A compute grant (secret-env SE28): jobs that already got it keep it.
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for j in v.get("stillRunning") or []:
    print("still running with it: job %s on %s (%s, %s); revoking stops its next attempt, not this one (k2 compute cancel %s)" % (j["short"], j["node"], j["env"], j["state"], j["short"]))'
            ;;
        open)
            _vault_call POST open '{}'
            [ "$as_json" = "1" ] && { printf '%s\n' "$_SC_BODY"; exit 0; }
            echo "the vault is open"
            ;;
        disable|enable)
            [ -n "$name" ] || { echo "usage: k2 vault $sub NAME" >&2; exit 2; }
            _vault_call POST "$sub" "$(_vault_merge "$sj" "$nj")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_SC_BODY"; exit 0; }
            echo "${name}: ${sub}d"
            _vault_print_recall
            ;;
        destroy)
            [ -n "$name" ] || { echo "usage: k2 vault destroy NAME --yes" >&2; exit 2; }
            _vault_call POST destroy "$(_vault_merge "$sj" "$nj" "$( [ "$yes" = 1 ] && echo '{"yes": true}' || echo '{}')")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_SC_BODY"; exit 0; }
            echo "${name}: destroyed on this box (backups made earlier keep it until they expire)"
            _vault_print_recall
            ;;
        get)
            [ -n "$name" ] || { echo "usage: k2 vault get NAME" >&2; exit 2; }
            _vault_call POST get "$(_vault_merge "$sj" "$nj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import base64,json,sys; sys.stdout.buffer.write(base64.b64decode(json.load(sys.stdin)["valueB64"]))'
            ;;
        audit)
            local q=""
            if [ -n "$name" ]; then
                q="$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); o.update(json.loads(sys.argv[2])); print(urllib.parse.urlencode(o))' "$sj" "$nj")"
            fi
            [ -n "$since" ] && q="${q}${q:+&}since=$(urlencode "$since")"
            _vault_call GET audit "$q"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
for a in json.load(sys.stdin).get("audit") or []:
    print("%s %-8s %-28s %-22s %s%s%s" % (a.get("ts"), a.get("action"), a.get("outcome"), a.get("actor"), a.get("targetHost") or "", (" x%d" % a["count"]) if a.get("count",1) > 1 else "", ""))'
            ;;
        requests)
            _vault_call GET requests
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin).get("requests") or []
if not r: print("no secret cards yet")
for x in r:
    i = x.get("inject") or {}
    how = ("  sends %s %s" % (i.get("dst"), i.get("fmt"))) if i else ""
    print("%s %-8s %-24s asked by %s%s" % (x.get("createdAt"), x.get("status"), x.get("name"), x.get("requestedBy"), how))'
            ;;
        presets)
            _vault_call GET presets
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
for p in json.load(sys.stdin).get("presets") or []:
    how = ("signs: " + p["sign"]) if p.get("sign") else (p["dst"] + " " + p["fmt"])
    print("%-10s %-16s %-48s %s" % (p["id"], p["label"], ",".join(p["hosts"]) or "(your project host)", how))'
            ;;
        doctor)
            if [ "$transcripts" = "1" ]; then _vault_call GET doctor "transcripts=1"; else _vault_call GET doctor; fi
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
f = v.get("findings") or []
if not f: print("vault: nothing to fix")
for x in f:
    print("%-7s %s" % (x.get("level"), x.get("message")))
    paths = x.get("files") or []
    for p in paths[:5]: print("        %s" % p)
    more = (x.get("fileCount") or len(paths)) - min(len(paths), 5)
    if more > 0: print("        ... and %d more (--json lists up to %d)" % (more, len(paths) or 5))
if v.get("checkedNames"): print(".env %s" % v["checkedNames"])
if v.get("summary"): print(v["summary"])
sys.exit(0 if v.get("ok") else 1)'
            ;;
        env-file)
            local ej
            [ -z "$wait_for" ] || [ -z "$done_path" ] || { echo "usage: --wait doesn't go with --done" >&2; exit 2; }
            # X5: env-file NAME [NAME2 …] like file NAME; --secret stays.
            local efs=("${pos[@]+"${pos[@]}"}" "${use_secrets[@]+"${use_secrets[@]}"}")
            [ "${#efs[@]}" -gt 0 ] || [ -n "$done_path" ] || { echo "usage: k2 vault env-file NAME [NAME2 …] [--ttl 90s] | --done <path>" >&2; exit 2; }
            ej="$(python3 -c 'import json,sys
o={}
if sys.argv[1]: o["done"]=sys.argv[1]
if sys.argv[2]: o["ttl"]=sys.argv[2]
if sys.argv[3]: o["wait"]=sys.argv[3]
o["secrets"]=[x for x in sys.argv[4:] if x]
print(json.dumps(o))' "$done_path" "$ttl" "$wait_for" "${efs[@]+"${efs[@]}"}")"
            _vault_wait_call env-file "$(_vault_merge "$ej" "$sj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("path") or ("removed " + str(v.get("removed"))))'
            ;;
        proxy-url)
            # A3: a placeholder and base URL for this session; the daemon
            # picks the names and the session (from the passport).
            local psecret="${use_secrets[0]:-$name}"
            [ "${#use_secrets[@]}" -le 1 ] && [ -n "$psecret" ] || { echo "usage: k2 vault proxy-url NAME [--ttl 2h]  (one secret)" >&2; exit 2; }
            local pj
            pj="$(python3 -c 'import json,sys
o={"secret":sys.argv[1]}
if sys.argv[2]: o["ttl"]=sys.argv[2]
print(json.dumps(o))' "$psecret" "$ttl")"
            _vault_call POST proxy-url "$(_vault_merge "$pj" "$sj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("%s=%s" % (v["envName"], v["placeholder"]))
print("%s=%s" % (v["baseUrlEnv"], v["baseUrl"]))'
            ;;
        file)
            # 2026-10-28: one secret delivered as file (or env), as a 0600 file.
            local fsecret="${use_secrets[0]:-$name}"
            [ "${#use_secrets[@]}" -le 1 ] || { echo "usage: k2 vault file NAME [--ttl 90s]  (one secret per file)" >&2; exit 2; }
            [ -n "$fsecret" ] || [ -n "$done_path" ] || { echo "usage: k2 vault file NAME [--ttl 90s] | --done <path>" >&2; exit 2; }
            local fj
            [ -z "$wait_for" ] || [ -z "$done_path" ] || { echo "usage: --wait doesn't go with --done" >&2; exit 2; }
            # --ttl and --wait go as typed (90s, 10m, 2h or seconds); the daemon parses them.
            fj="$(python3 -c 'import json,sys
o={}
if sys.argv[1]: o["done"]=sys.argv[1]
if sys.argv[2]: o["ttl"]=sys.argv[2]
if sys.argv[3] and not sys.argv[1]: o["secret"]=sys.argv[3]
if sys.argv[4]: o["wait"]=sys.argv[4]
print(json.dumps(o))' "$done_path" "$ttl" "$fsecret" "$wait_for")"
            _vault_wait_call file "$(_vault_merge "$fj" "$sj")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("path") or ("removed " + str(v.get("removed"))))'
            ;;
        import)
            [ -n "$name" ] || { echo "usage: k2 vault import <.env file> [--scope ws:<name>|server] [--delivery env|file|fetch] [--remove-original]" >&2; exit 2; }
            local ij abs
            abs="$(cli_abs_path "$name")"
            ij="$(python3 -c 'import json,sys
o=json.loads(sys.argv[1]); o["path"]=sys.argv[2]
if sys.argv[3]: o["delivery"]=sys.argv[3]
if sys.argv[4]=="1": o["removeOriginal"]=True
print(json.dumps(o))' "$sj" "$abs" "$delivery" "$remove_original")"
            _vault_call POST import "$ij"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("moved into the vault: %s" % (", ".join(v.get("moved") or []) or "nothing"))
print("wrote %s (references only; safe to commit)" % v.get("envK2"))
if not v.get("originalRemoved"):
    print("the original .env is still there; delete it when you are ready (or --remove-original)")'
            ;;
        service)
            [ -n "$name" ] || { echo "usage: k2 vault service <published service> [clean|legacy] [--env-file .env.k2]" >&2; exit 2; }
            if [ -n "${pos[1]:-}" ] || [ -n "$env_file_opt" ]; then
                _vault_call POST service "$(python3 -c 'import json,sys
o={"service": sys.argv[1]}
if sys.argv[2]: o["mode"]=sys.argv[2]
if sys.argv[3]: o["envFile"]=sys.argv[3]
print(json.dumps(o))' "$name" "${pos[1]:-}" "$env_file_opt")"
            else
                _vault_call GET service "service=$(urlencode "$name")"
            fi
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: environment %s%s" % (v.get("service"), v.get("mode"), (", .env.k2 = " + v["envFile"]) if v.get("envFile") else ""))
for s in v.get("secrets") or []:
    print("  %s as %s%s" % (s.get("name"), s.get("delivery"), (" (" + s["envName"] + ")") if s.get("envName") else ""))
if v.get("inheritedNames"):
    print("  inherits the daemon environment: %s" % ", ".join(v["inheritedNames"]))
if v.get("mode") == "legacy":
    ends = v.get("envRemovedIn")
    if ends:
        when = ("with this release (K2 %s)" if v.get("cutoverNow") else "in K2 %s") % ends
        print("  deprecated: inheriting the daemon environment ends %s — switch with: k2 publish env %s clean" % (when, v.get("service")))
    else:
        # No end is set (Rosson 2026-10-26).
        print("  inherits the daemon environment (it keeps it as a --cmd service) — start it clean with: k2 publish env %s clean" % v.get("service"))
if v.get("restart"): print(v["restart"])'
            ;;
        reveal)
            [ -n "$name" ] || { echo "usage: k2 vault reveal NAME [--scope <s>]" >&2; exit 2; }
            _vault_person_guard
            _vault_sock_call GET "show?$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); o.update(json.loads(sys.argv[2])); print(urllib.parse.urlencode(o))' "$sj" "$nj")"
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            local sid
            sid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["secret"]["id"])')"
            local tok
            tok="$(_vault_confirm_token reveal "$sid" "$name")" || exit 3
            _vault_sock_call POST reveal "$(_vault_merge "$sj" "$nj" "$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1]}))' "$tok")")"
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            printf '%s' "$_SC_BODY" | python3 -c 'import base64,json,sys; sys.stdout.buffer.write(base64.b64decode(json.load(sys.stdin)["valueB64"])); print()'
            ;;
        recovery)
            local act="${pos[0]:-status}"
            case "$act" in
                status)
                    _vault_call GET status
                    printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("recovery key: %s%s" % (v.get("recovery"), (" (" + v["recoveryConfirmedAt"] + ")") if v.get("recoveryConfirmedAt") else "")); print(v.get("recoveryHint",""))'
                    ;;
                show-once|rotate)
                    _vault_person_guard
                    local rb='{}'
                    if [ "$act" = "rotate" ]; then
                        local tok
                        tok="$(_vault_confirm_token recovery-rotate "" "recovery key")" || exit 3
                        rb="$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1]}))' "$tok")"
                    fi
                    _vault_sock_call POST "recovery/${act}" "$rb"
                    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
                    local rid
                    rid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
                    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
w = v["phrase"].split(" ")
print()
print("Your vault recovery key (write it down, keep it offline; K2 keeps only the public half):")
print()
for i in range(0, 24, 4):
    print("   " + "   ".join("%2d. %-9s" % (i + j + 1, w[i + j]) for j in range(4)))
print()' >&2
                    local words=() wd n
                    for n in 3 11 20; do
                        printf 'Type word %s: ' "$n" >&2
                        IFS= read -r wd || wd=""
                        words+=("$wd")
                    done
                    printf '\033[2J\033[H' >&2
                    _vault_sock_call POST recovery/confirm "$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1], "words": sys.argv[2:5]}))' "$rid" "${words[@]}")"
                    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
                    echo "recovery key confirmed. If it ever leaks: k2 vault recovery rotate, then k2 vault rekey."
                    ;;
                *) echo "usage: k2 vault recovery status|show-once|rotate" >&2; exit 2 ;;
            esac
            ;;
        confirm-password)
            [ "${pos[0]:-}" = "set" ] || { echo "usage: k2 vault confirm-password set" >&2; exit 2; }
            _vault_person_guard
            local tokj='{}'
            _vault_sock_call GET status
            if printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("confirmPassword") else 1)'; then
                local tok
                tok="$(_vault_confirm_token confirm-password-change "" "vault password")" || exit 3
                tokj="$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1]}))' "$tok")"
            fi
            local p1="" p2=""
            printf 'New vault password (10+ characters): ' >&2; IFS= read -rs p1 || p1=""; echo >&2
            printf 'Again: ' >&2; IFS= read -rs p2 || p2=""; echo >&2
            [ -n "$p1" ] && [ "$p1" = "$p2" ] || { echo "usage: the two entries don't match" >&2; exit 2; }
            local pb
            pb="$(printf '%s' "$p1" | python3 -c 'import json,sys; o=json.loads(sys.argv[1]); o["password"]=sys.stdin.read(); print(json.dumps(o))' "$tokj")"
            p1=""; p2=""
            _vault_sock_call POST confirm-password/set "$pb"
            pb=""
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            echo "vault password set. K2 only asks for it inside the K2 window or your own terminal."
            ;;
        kek|rekey)
            local action route
            if [ "$sub" = "kek" ]; then
                [ "${pos[0]:-}" = "rotate" ] || { echo "usage: k2 vault kek rotate" >&2; exit 2; }
                action="kek-rotate"; route="kek/rotate"
            else
                action="rekey"; route="rekey"
            fi
            _vault_person_guard
            local tok
            tok="$(_vault_confirm_token "$action" "" "")" || exit 3
            _vault_sock_call POST "$route" "$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1]}))' "$tok")"
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            echo "done"
            ;;
        rollback)
            [ -n "$name" ] && [ -n "$to_version" ] || { echo "usage: k2 vault rollback NAME --to <version>" >&2; exit 2; }
            _vault_person_guard
            _vault_sock_call GET "show?$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); o.update(json.loads(sys.argv[2])); print(urllib.parse.urlencode(o))' "$sj" "$nj")"
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            local sid tok
            sid="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["secret"]["id"])')"
            if printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; sys.exit(0 if any(p.get("target") == "systemd_cred" and (p.get("version") or 0) > 0 for p in json.load(sys.stdin).get("placements") or []) else 1)'; then
                # Root units load it: the confirm names the target version,
                # who wrote it, and the units it re-places (bound to them).
                tok="$(_vault_confirm_token_plan rollback "$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1], "version": int(sys.argv[2]), "acceptAgentValue": sys.argv[3] == "1"}))' "$sid" "$to_version" "$accept_agent")" rollbackPlan)" || exit 3
            else
                tok="$(_vault_confirm_token rollback "$sid" "$name")" || exit 3
            fi
            _vault_sock_call POST rollback "$(_vault_merge "$sj" "$nj" "$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1], "version": int(sys.argv[2]), "acceptAgentValue": sys.argv[3] == "1"}))' "$tok" "$to_version" "$accept_agent")")"
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            echo "${name}: back to the value of v${to_version} (as a new version)"
            ;;
        recipient)
            _vault_call GET recipient
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("recipient: %s" % v["recipient"])
print("vault id:  %s" % v["vaultId"])
print("On the old box: k2 vault carry export --recipient %s --target %s --workspace <ws> --out carry.age" % (v["recipient"], v["vaultId"]))'
            ;;
        carry)
            case "${pos[0]:-}" in
                export)
                    [ -n "$carry_recipient" ] && [ -n "$carry_target" ] && [ -n "$out_file" ] || {
                        echo "usage: k2 vault carry export --recipient <age1…> --target <vault-id> --workspace <ws> [--server NAME] --out <file>" >&2; exit 2; }
                    [ ! -e "$out_file" ] || { echo "usage: $out_file already exists" >&2; exit 2; }
                    local cb
                    cb="$(python3 -c '
import json, sys
r, t, own = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
ws = [w for w in sys.argv[4].split("\x1f") if w]
srv = [x for x in sys.argv[5].split("\x1f") if x]
body = {"targetRecipient": r, "targetVaultId": t, "ownBoxMove": own, "workspaces": ws, "server": srv}
if sys.argv[6]:
    body["targetWorkspaceId"] = sys.argv[6]
print(json.dumps(body))' \
                        "$carry_recipient" "$carry_target" "$own_box_move" \
                        "$(IFS=$'\x1f'; echo "${carry_ws[*]:-}")" "$(IFS=$'\x1f'; echo "${carry_server[*]:-}")" "$carry_target_ws")"
                    _vault_call POST carry/export "$cb"
                    ( umask 077; printf '%s' "$_SC_BODY" | python3 -c '
import base64, json, sys
v = json.load(sys.stdin)
open(sys.argv[1], "xb").write(base64.b64decode(v["carryB64"]))
print("%d secret(s) sealed for %s:" % (v["count"], v["targetFingerprint"]), file=sys.stderr)
for n in v["secrets"]:
    print("  " + n, file=sys.stderr)' "$out_file" ) || exit 1
                    echo "wrote $out_file (only the target vault can open it)"
                    ;;
                import)
                    local f="${pos[1]:-}"
                    [ -n "$f" ] && [ -f "$f" ] || { echo "usage: k2 vault carry import <file> [--workspace <id>] [--apply]" >&2; exit 2; }
                    [ "${#carry_ws[@]}" -le 1 ] || { echo "usage: k2 vault carry import takes one --workspace <id>" >&2; exit 2; }
                    local ib
                    ib="$(python3 -c '
import base64, json, sys
body = {"carryB64": base64.b64encode(open(sys.argv[1], "rb").read()).decode(), "apply": sys.argv[2] == "1"}
if sys.argv[3]:
    body["workspace"] = sys.argv[3]
print(json.dumps(body))' "$f" "$apply" "${carry_ws[0]:-}")"
                    _vault_call POST carry/import "$ib"
                    if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
                    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("dryRun"):
    print("from vault %s (key %s)%s:" % (v["sourceVaultId"], v["sourceKeyFingerprint"], ", owner move" if v.get("ownBoxMove") else ""))
    if v.get("madeForWorkspaceId"):
        print("made for workspace %s" % v["madeForWorkspaceId"])
    for s in v["secrets"]:
        if s.get("skip"):
            dest = "-> skipped"
        elif s.get("targetWorkspaceId"):
            dest = "-> %s (%s)" % (s.get("targetWorkspace") or "?", s["targetWorkspaceId"])
        else:
            dest = "-> server"
        print("  %-24s %-28s %-40s %s" % (s["scope"], s["name"], dest, ", ".join(s["hosts"]) or "-"))
        if s.get("skip"):
            print("      " + s["skip"])
    print("nothing changed yet; add --apply to put these in this vault")
else:
    print("imported: %s" % (", ".join(v["imported"]) or "none"))
    for x in v["skipped"]:
        print("skipped:  " + x)'
                    ;;
                *) echo "usage: k2 vault carry export|import" >&2; exit 2 ;;
            esac
            ;;
        backup)
            if [ "${pos[0]:-}" = "key" ]; then
                [ -n "${pos[1]:-}" ] || { echo "usage: k2 vault backup key <age1…> | remove" >&2; exit 2; }
                _vault_person_guard
                local tok
                if [ "${pos[1]}" = "remove" ]; then
                    # Bind the confirm to the key that's there now.
                    _vault_sock_call GET status
                    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
                    local bfp
                    bfp="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(next((h.get("fingerprint") or "" for h in json.load(sys.stdin).get("holders") or [] if h.get("kind") == "backup"), ""))')"
                    [ -n "$bfp" ] || { echo "not_found: this vault has no backup key" >&2; exit 1; }
                    tok="$(_vault_confirm_token backup-key "remove:$bfp" "")" || exit 3
                    _vault_sock_call POST backup/recipient "$(python3 -c 'import json,sys; print(json.dumps({"remove": True, "confirmToken": sys.argv[1]}))' "$tok")"
                    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
                    printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("backup key %s removed; %s" % (v["removed"], v["note"]))'
                else
                    tok="$(_vault_confirm_token backup-key "add:${pos[1]}" "")" || exit 3
                    _vault_sock_call POST backup/recipient "$(python3 -c 'import json,sys; print(json.dumps({"recipient": sys.argv[1], "confirmToken": sys.argv[2]}))' "${pos[1]}" "$tok")"
                    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
                    printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("backup key %s added%s; a backup can now be opened with it on a new box" % (v["fingerprint"], (" (replaced %s)" % v["replaced"]) if v.get("replaced") else ""))'
                fi
            else
                [ -n "${pos[0]:-}" ] || { echo "usage: k2 vault backup <dir>" >&2; exit 2; }
                _vault_person_guard
                local d
                d="$(cd "${pos[0]}" 2>/dev/null && pwd)" || { echo "usage: ${pos[0]} isn't a folder" >&2; exit 2; }
                _vault_sock_call POST backup "$(python3 -c 'import json,sys; print(json.dumps({"destDir": sys.argv[1]}))' "$d")"
                [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
                printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("wrote %s (%s)" % (v["path"], v["note"]))'
            fi
            ;;
        restore)
            [ -n "$from" ] || { echo "usage: k2 vault restore --from <dir> [--holder recovery|backup] [--dry-run] [--replace]" >&2; exit 2; }
            _vault_person_guard
            local rd h="${holder:-recovery}" secret_in=""
            rd="$(cd "$from" 2>/dev/null && pwd)" || { echo "usage: $from isn't a folder" >&2; exit 2; }
            [ -f "$rd/vault.db" ] || { echo "usage: no vault.db in $rd" >&2; exit 2; }
            if [ -t 0 ]; then
                if [ "$h" = "recovery" ]; then printf 'Recovery words (24, not shown): ' >&2; else printf 'Backup key (AGE-SECRET-KEY-1…, not shown): ' >&2; fi
                IFS= read -rs secret_in || secret_in=""
                echo >&2
            else
                secret_in="$(cat)"
            fi
            [ -n "$secret_in" ] || { echo "usage: no recovery words or backup key given" >&2; exit 2; }
            local rb
            rb="$(printf '%s' "$secret_in" | python3 -c '
import json, sys
o = {"fromDir": sys.argv[1], "holder": sys.argv[2], "dryRun": sys.argv[3] == "1", "replace": sys.argv[4] == "1"}
v = sys.stdin.read().strip()
o["phrase" if sys.argv[2] == "recovery" else "identity"] = v
print(json.dumps(o))' "$rd" "$h" "$dry_run" "$replace")"
            secret_in=""
            if [ "$replace" = "1" ] && [ "$dry_run" = "0" ]; then
                local tok
                tok="$(_vault_confirm_token restore "" "vault")" || exit 3
                rb="$(_vault_merge "$rb" "$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1]}))' "$tok")")"
            fi
            _vault_sock_call POST restore "$rb"
            rb=""
            [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("dryRun"):
    print("backup opens: vault %s, %d secret(s). Nothing changed; run again without --dry-run." % (v["vaultId"], v["secrets"]))
else:
    print("restored vault %s (%d secret(s)). The previous vault file is kept at %s" % (v["vaultId"], v["secrets"], v["previous"]))'
            ;;
        *) _vault_unknown_verb "$sub" ;;
    esac
}

