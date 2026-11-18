# ── k2 mail help text ─────────────────────────────────────────────────



# ── k2 mail reader (prd-ocmail-admin-in-k2-v1 OA7) ─────────────────────
# The mail reader for PEOPLE: the Owner and Admins open every inbox on the
# server (hosted and linked, agents' and people's); anyone else only their
# own mailbox. Opening never marks mail read; every access is logged.
# Agents are refused (exit 3): they use the rest of `k2 mail`.

cmd_help_mail_reader() {
    cat <<'EOF'
%include cli/text/help/mail/reader.txt
EOF
}

cmd_mail_reader() {
    local sub="${1:-}"
    case "$sub" in
        --help|-h|help|'') cmd_help_mail_reader; return 0 ;;
    esac
    shift
    case "$sub" in
        inboxes|folders|messages|show|thread|attachment|log|flag|move|archive|delete|reply|draft|send) ;;
        drafts|draft-show|draft-save|draft-send|draft-discard) ;;
        *) _mail_err "usage" "unknown command '$sub' — see k2 mail reader --help"; exit 2 ;;
    esac
    local MR_JSON=false MR_REASON="" MR_FOLDER="" MR_UNREAD=false MR_QUERY="" MR_FROM=""
    local MR_SINCE="" MR_BEFORE="" MR_LIMIT="" MR_OFFSET="" MR_HTML=false MR_OUT=""
    local MR_INBOX="" MR_ACTOR="" MR_READ="" MR_FLAGGED="" MR_BODY="" MR_BODY_SET=false
    local MR_TO="" MR_CC="" MR_SUBJECT=""
    local -a pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --help|-h) cmd_help_mail_reader; return 0 ;;
            --json) MR_JSON=true ;;
            --html) MR_HTML=true ;;
            --unread)
                if [ "$sub" = "flag" ]; then MR_READ=false; else MR_UNREAD=true; fi ;;
            --read) MR_READ=true ;;
            --star) MR_FLAGGED=true ;;
            --unstar) MR_FLAGGED=false ;;
            --reason|--folder|--query|--from|--since|--before|--limit|--offset|--out|--inbox|--actor|--body|--to|--cc|--subject)
                if [ $# -lt 2 ]; then _mail_err "usage" "$1 needs a value — see k2 mail reader --help"; exit 2; fi
                case "$1" in
                    --reason) MR_REASON="$2" ;;
                    --folder) MR_FOLDER="$2" ;;
                    --query) MR_QUERY="$2" ;;
                    --from) MR_FROM="$2" ;;
                    --since) MR_SINCE="$2" ;;
                    --before) MR_BEFORE="$2" ;;
                    --limit) MR_LIMIT="$2" ;;
                    --offset) MR_OFFSET="$2" ;;
                    --out) MR_OUT="$2" ;;
                    --inbox) MR_INBOX="$2" ;;
                    --actor) MR_ACTOR="$2" ;;
                    --body)
                        if [ "$2" = "-" ]; then MR_BODY="$(cat)"; else MR_BODY="$2"; fi
                        MR_BODY_SET=true ;;
                    --to) MR_TO="${MR_TO:+$MR_TO
}$2" ;;
                    --cc) MR_CC="${MR_CC:+$MR_CC
}$2" ;;
                    --subject) MR_SUBJECT="$2" ;;
                esac
                shift ;;
            --*) _mail_err "usage" "unknown flag '$1' for 'k2 mail reader $sub' — see k2 mail reader --help"; exit 2 ;;
            *) pos+=("$1") ;;
        esac
        shift
    done
    local need=0
    case "$sub" in
        folders|messages|show|thread|flag|archive|delete|reply) need=1 ;;
        drafts|draft-show|draft-save|draft-send|draft-discard) need=1 ;;
        move|attachment) need=2 ;;
        draft) [ -z "$MR_INBOX" ] && need=1 ;;
    esac
    if [ "${#pos[@]}" -lt "$need" ]; then
        _mail_err "usage" "'k2 mail reader $sub' needs $need argument(s) — see k2 mail reader --help"
        exit 2
    fi
    if [ "${#pos[@]}" -gt "$need" ]; then
        _mail_err "usage" "unexpected argument '${pos[$need]}' for 'k2 mail reader $sub' — see k2 mail reader --help"
        exit 2
    fi
    if [ "$sub" = "attachment" ] && [ -z "$MR_OUT" ]; then
        _mail_err "usage" "missing --out <file> — where to save the attachment"
        exit 2
    fi
    if [ "$sub" = "flag" ] && [ -z "$MR_READ" ] && [ -z "$MR_FLAGGED" ]; then
        _mail_err "usage" "nothing to change — pass --read, --unread, --star or --unstar"
        exit 2
    fi
    case "$sub" in
        reply|draft|send)
            if [ "$MR_BODY_SET" != true ]; then _mail_err "usage" "missing --body <text> (or --body - to read stdin)"; exit 2; fi ;;
    esac
    if [ "$sub" = "send" ] && [ -z "$MR_INBOX" ]; then
        _mail_err "usage" "missing --inbox <address> — the inbox to send from"
        exit 2
    fi
    MR_BODY_SET="$MR_BODY_SET" MR_SUB="$sub" MR_A1="${pos[0]:-}" MR_A2="${pos[1]:-}" \
    MR_JSON="$MR_JSON" MR_REASON="$MR_REASON" MR_FOLDER="$MR_FOLDER" MR_UNREAD="$MR_UNREAD" \
    MR_QUERY="$MR_QUERY" MR_FROM="$MR_FROM" MR_SINCE="$MR_SINCE" MR_BEFORE="$MR_BEFORE" \
    MR_LIMIT="$MR_LIMIT" MR_OFFSET="$MR_OFFSET" MR_HTML="$MR_HTML" MR_OUT="$MR_OUT" \
    MR_INBOX="$MR_INBOX" MR_ACTOR="$MR_ACTOR" MR_READ="$MR_READ" MR_FLAGGED="$MR_FLAGGED" \
    MR_BODY="$MR_BODY" MR_TO="$MR_TO" MR_CC="$MR_CC" MR_SUBJECT="$MR_SUBJECT" \
    ABASE="$BASE_URL" ATOKEN="$TOKEN" _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="$(_k5_py_headers)" python3 - <<'PYEOF'
import base64, json, os, sys, time
import urllib.error, urllib.parse, urllib.request
# Loopback and plain http skip the proxy; remote https may use it.
exec(os.environ["_K2_PY_PROXY_RULE"])
exec(os.environ.pop("_K5_PY_HTTP"))

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

E = os.environ.get
sub = E("MR_SUB")
as_json = E("MR_JSON") == "true"

def dumps(o):
    return json.dumps(o, ensure_ascii=False, separators=(",", ":"))

def die(err):
    code = err.get("code", "error") if isinstance(err, dict) else "error"
    hint = err.get("hint", "") if isinstance(err, dict) else str(err)
    sys.stderr.write(dumps({"error": {"code": code, "hint": hint}}) + "\n")
    if code in ("owner_only", "forbidden", "gated", "not_allowed", "role_required"):
        sys.exit(3)
    sys.exit(2 if code == "usage" else 1)

def call(method, route, params=None, body=None):
    q = {k: v for k, v in (params or {}).items() if v not in (None, "", False)}
    url = E("ABASE") + route + ("?" + urllib.parse.urlencode(q) if q else "")
    data = dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {})
    if E("ATOKEN", ""):
        req.add_header("Authorization", "Bearer " + E("ATOKEN", ""))
    try:
        with urllib.request.urlopen(k5_request(req), timeout=float(E("K5_MAIL_HTTP_TIMEOUT") or E("K2_MAIL_HTTP_TIMEOUT") or E("K2SO_MAIL_HTTP_TIMEOUT") or 60)) as r:
            d = json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        try:
            d = json.loads(raw)
        except Exception:
            die({"code": "error", "hint": "HTTP %s from %s" % (e.code, route)})
        if isinstance(d.get("error"), str):
            # The role gate's shape: {"error":"role_required","message":…}
            die({"code": d["error"], "hint": d.get("message") or d["error"]})
    except Exception as e:
        die({"code": "daemon_unreachable", "hint": "no response from the K2 daemon (is it running?): %s" % e})
    if isinstance(d.get("error"), dict):
        die(d["error"])
    if not d.get("ok"):
        die({"code": "error", "hint": "unexpected daemon answer: %s" % dumps(d)[:200]})
    return d

reason = E("MR_REASON") or None
a1, a2 = E("MR_A1"), E("MR_A2")

def who(a):
    if not a:
        return "(unknown)"
    n, ad = (a.get("name") or "").strip(), a.get("address") or ""
    return "%s <%s>" % (n, ad) if n else ad

def lst(k):
    return [x.strip() for x in (E(k) or "").replace(",", "\n").split("\n") if x.strip()]

def print_draft_head(m):
    a = m.get("author") or {}
    by = a.get("name") or "?"
    kind = "agent" if a.get("kind") == "agent" else "person"
    edited = time.strftime("%Y-%m-%d %H:%M", time.localtime(m.get("editedAt") or 0))
    line = "DRAFT, NOT SENT — drafted by %s (%s), last edited %s" % (by, kind, edited)
    if m.get("editedBy"):
        line += " by %s" % m["editedBy"]
    print(line)
    print("Draft:   %s" % (m.get("draftId") or m.get("id")))
    if m.get("hint"):
        print("Note:    %s" % m["hint"])

def print_message(m):
    if m.get("draft"):
        print_draft_head(m)
    print("From:    %s" % who(m.get("from")))
    if m.get("to"):
        print("To:      %s" % ", ".join(who(x) for x in m["to"]))
    if m.get("cc"):
        print("Cc:      %s" % ", ".join(who(x) for x in m["cc"]))
    print("Date:    %s" % m.get("date", ""))
    print("Subject: %s" % (m.get("subject") or ""))
    if not m.get("draft"):
        print("Id:      %s" % m.get("id", ""))
    ar = m.get("authResults") or {}
    if any(ar.get(k) for k in ("spf", "dkim", "dmarc")):
        print("Checks:  SPF %s · DKIM %s · DMARC %s" % (ar.get("spf") or "-", ar.get("dkim") or "-", ar.get("dmarc") or "-"))
    for a in m.get("attachments") or []:
        print("Attach:  %s. %s (%s, %s bytes)" % (a.get("index"), a.get("filename") or "(no name)", a.get("mime"), a.get("size")))
    print()
    if E("MR_HTML") == "true" and m.get("html"):
        print(m["html"])
        if m.get("remoteContentBlocked"):
            print("\n(Remote pictures and styles were taken out. Links are kept as data-k2-link, not href.)")
    else:
        print(m.get("text") or "(no text part)")

R = "/cli/mail/reader/"
if sub == "inboxes":
    d = call("GET", R + "inboxes", {"reason": reason})
    if as_json:
        print(dumps(d)); sys.exit(0)
    rows = d.get("inboxes") or []
    if not rows:
        print("No inboxes you can open." + ("" if (d.get("viewer") or {}).get("opensAny") else " You see only your own mailbox; an Owner or Admin sees every inbox."))
        sys.exit(0)
    for i in rows:
        owner = i.get("person") or i.get("workspace") or "-"
        tag = " (you)" if i.get("own") else ""
        print("%-40s %-9s %-6s %s%s" % (i["address"], i.get("provider", ""), i.get("kind", ""), owner, tag))
elif sub == "folders":
    d = call("GET", R + "folders", {"inbox": a1, "reason": reason})
    if as_json:
        print(dumps(d)); sys.exit(0)
    for f in d.get("folders") or []:
        print("%-30s %s" % (f["name"], f.get("role") or ""))
elif sub == "messages":
    d = call("GET", R + "messages", {"inbox": a1, "folder": E("MR_FOLDER"), "unread": "1" if E("MR_UNREAD") == "true" else "",
        "query": E("MR_QUERY"), "from": E("MR_FROM"), "since": E("MR_SINCE"), "before": E("MR_BEFORE"),
        "limit": E("MR_LIMIT"), "offset": E("MR_OFFSET"), "reason": reason})
    if as_json:
        print(dumps(d)); sys.exit(0)
    items = d.get("items") or []
    if not items:
        print("No messages.")
    for m in items:
        print("%s %s  %-20s  %-30s  %s" % ("*" if m.get("unread") else " ", m.get("id"), (m.get("date") or "")[:16].replace("T", " "),
            who(m.get("from"))[:30], m.get("subject") or "(no subject)"))
    if d.get("nextOffset") is not None:
        print("more: k2 mail reader messages %s --offset %s" % (a1, d["nextOffset"]))
elif sub == "show":
    d = call("GET", R + "message", {"id": a1, "reason": reason})
    if as_json:
        print(dumps(d)); sys.exit(0)
    print_message(d["message"])
elif sub == "thread":
    d = call("GET", R + "thread", {"id": a1, "folder": E("MR_FOLDER"), "reason": reason})
    if as_json:
        print(dumps(d)); sys.exit(0)
    for n, m in enumerate(d.get("messages") or []):
        if n:
            print("\n" + "─" * 60 + "\n")
        print_message(m)
elif sub == "attachment":
    try:
        part = int(a2)
    except ValueError:
        die({"code": "usage", "hint": "the attachment number is 1, 2, … (from 'k2 mail reader show')"})
    out = E("MR_OUT")
    if os.path.exists(out):
        die({"code": "usage", "hint": "'%s' already exists — pick another --out" % out})
    d = call("GET", R + "attachment", {"id": a1, "part": str(part), "reason": reason})
    data = base64.b64decode(d.get("dataBase64") or "")
    with open(out, "xb") as f:
        f.write(data)
    if as_json:
        d.pop("dataBase64", None); d["savedTo"] = out
        print(dumps(d)); sys.exit(0)
    print("saved %s (%s, %d bytes) to %s" % (d.get("filename"), d.get("mime"), len(data), out))
elif sub == "log":
    d = call("GET", R + "log", {"inbox": E("MR_INBOX"), "actor": E("MR_ACTOR"), "since": E("MR_SINCE"), "limit": E("MR_LIMIT")})
    if as_json:
        print(dumps(d)); sys.exit(0)
    rows = d.get("rows") or []
    if not rows:
        print("No rows.")
    for r in rows:
        at = time.strftime("%Y-%m-%d %H:%M", time.localtime(r.get("at") or 0))
        line = "%s  %-22s %-7s %-18s %-36s %s" % (at, r.get("actor"), r.get("actorRole"), r.get("action"), r.get("address"), r.get("outcome"))
        if r.get("reason"):
            line += "  reason: %s" % r["reason"]
        print(line)
elif sub == "drafts":
    d = call("GET", R + "drafts", {"inbox": a1, "reason": reason})
    if as_json:
        print(dumps(d)); sys.exit(0)
    rows = d.get("drafts") or []
    if not rows:
        print("No drafts K2 tracks in %s." % a1)
    for r in rows:
        a = r.get("author") or {}
        edited = time.strftime("%Y-%m-%d %H:%M", time.localtime(r.get("editedAt") or 0))
        line = "%s  %-9s by %-20s edited %s" % (r.get("id"), r.get("status"), "%s (%s)" % (a.get("name"), a.get("kind")), edited)
        if r.get("editedBy"):
            line += " by %s" % r["editedBy"]
        if r.get("stateBy") and r.get("status") in ("sent", "held", "discarded"):
            line += "  · %s by %s" % (r["status"], r["stateBy"])
        print(line)
elif sub == "draft-show":
    d = call("GET", R + "draft-open", {"id": a1, "reason": reason})
    if as_json:
        print(dumps(d)); sys.exit(0)
    print_message(d["draft"])
elif sub in ("draft-save", "draft-send", "draft-discard"):
    body = {"id": a1}
    if reason:
        body["reason"] = reason
    if sub != "draft-discard":
        if lst("MR_TO"):
            body["to"] = lst("MR_TO")
        if lst("MR_CC"):
            body["cc"] = lst("MR_CC")
        if E("MR_SUBJECT"):
            body["subject"] = E("MR_SUBJECT")
        if E("MR_BODY_SET") == "true":
            body["body"] = E("MR_BODY")
    d = call("POST", R + sub, None, body)
    if as_json:
        print(dumps(d)); sys.exit(0)
    if sub == "draft-send":
        st = d.get("status")
        print("send %s%s — draft %s %s" % (st, (" — " + d["id"]) if d.get("id") else "", a1, d.get("draftStatus")))
    elif sub == "draft-save":
        print("saved draft %s" % a1)
    else:
        print("discarded draft %s" % a1)
else:
    body = {"reason": reason} if reason else {}
    if sub in ("flag", "move", "archive", "delete", "reply") or (sub == "draft" and a1):
        body["id"] = a1
    if sub == "flag":
        if E("MR_READ"):
            body["read"] = E("MR_READ") == "true"
        if E("MR_FLAGGED"):
            body["flagged"] = E("MR_FLAGGED") == "true"
    if sub == "move":
        body["folder"] = a2
    if sub in ("reply", "draft", "send"):
        body["body"] = E("MR_BODY")
    if sub == "send" or (sub == "draft" and not a1):
        body["inbox"] = E("MR_INBOX")
        body["to"] = lst("MR_TO")
        if lst("MR_CC"):
            body["cc"] = lst("MR_CC")
        body["subject"] = E("MR_SUBJECT")
    d = call("POST", R + sub, None, body)
    if as_json:
        print(dumps(d)); sys.exit(0)
    status = d.get("status")
    if sub == "send" and status:
        print("send %s%s" % (status, (" — " + d["id"]) if d.get("id") else ""))
    elif sub == "move":
        print("moved to %s" % (d.get("folder") or a2))
    elif sub == "draft" and d.get("draftId"):
        print("draft %s saved to %s" % (d["draftId"], d.get("folder") or "Drafts"))
    else:
        print("ok")
PYEOF
}

cmd_help_mail() {
    cat <<'EOF'
%include cli/text/help/mail.txt
EOF
}

cmd_help_mail_create() {
    cat <<'EOF'
%include cli/text/help/hostmail/create.txt
EOF
}

cmd_help_mail_addresses() {
    cat <<'EOF'
List your workspace's ACTIVE addresses with creation age and cap usage.
(`k2 mail list` is an alias printing the identical table. Looking for your
incoming mail? That's `k2 mail messages`.)

USAGE
  k2 mail addresses [--json]

FLAGS
  --json                 JSON output ({ok,addresses:[...],cap:{used,cap}};
                         cap 0 = unlimited)
EOF
}

cmd_help_mail_domains() {
    cat <<'EOF'
%include cli/text/help/mail/domains.txt
EOF
}

cmd_help_mail_messages() {
    cat <<'EOF'
%include cli/text/help/mail/messages.txt
EOF
}

cmd_help_mail_read() {
    cat <<'EOF'
%include cli/text/help/mail/read.txt
EOF
}

cmd_help_mail_attachments() {
    cat <<'EOF'
%include cli/text/help/mail/attachments.txt
EOF
}

cmd_help_mail_wait() {
    cat <<'EOF'
%include cli/text/help/mail/wait.txt
EOF
}

cmd_help_mail_send() {
    cat <<'EOF'
%include cli/text/help/mail/send.txt
EOF
}

cmd_help_mail_reply() {
    cat <<'EOF'
%include cli/text/help/mail/reply.txt
EOF
}

cmd_help_mail_forward_message() {
    cat <<'EOF'
%include cli/text/help/mail/forward-message.txt
EOF
}

cmd_help_mail_outbox() {
    cat <<'EOF'
%include cli/text/help/mail/outbox.txt
EOF
}

cmd_help_mail_delete() {
    cat <<'EOF'
Retire an address you own. It stops receiving immediately and its cap slot
frees IMMEDIATELY (the cap counts active addresses). Mailbox data is kept
for the retention window, then purged. The name stays reserved during
retention — you cannot re-mint it right away.

USAGE
  k2 mail delete <address> [--json]

ARGUMENTS
  <address>              The full address to retire (see `k2 mail addresses`)
EOF
}

cmd_help_mail_status() {
    cat <<'EOF'
%include cli/text/help/hostmail/status.txt
EOF
}

cmd_help_mail_domain() {
    cat <<'EOF'
%include cli/text/help/hostmail/domain.txt
EOF
}

cmd_help_mail_doctor() {
    cat <<'EOF'
%include cli/text/help/hostmail/doctor.txt
EOF
}

cmd_help_mail_config() {
    cat <<'EOF'
%include cli/text/help/hostmail/config.txt
EOF
}

cmd_help_mail_sending() {
    cat <<'EOF'
%include cli/text/help/mail/sending.txt
EOF
}

cmd_help_mail_approvals() {
    cat <<'EOF'
%include cli/text/help/hostmail/approvals.txt
EOF
}

cmd_help_mail_draft() {
    cat <<'EOF'
%include cli/text/help/mail/draft.txt
EOF
}

cmd_help_mail_inboxes() {
    cat <<'EOF'
%include cli/text/help/mail/inboxes.txt
EOF
}

cmd_help_mail_link() {
    cat <<'EOF'
%include cli/text/help/mail/link.txt
EOF
}

cmd_help_mail_capabilities() {
    cat <<'EOF'
%include cli/text/help/mail/capabilities.txt
EOF
}

cmd_help_mail_oauth_config() {
    cat <<'EOF'
%include cli/text/help/mail/oauth-config.txt
EOF
}

cmd_help_mail_access() {
    cat <<'EOF'
%include cli/text/help/mail/access.txt
EOF
}

cmd_help_mail_manage() {
    cat <<'EOF'
%include cli/text/help/mail/delete.txt
EOF
}

cmd_help_hostmail() {
    cat <<'EOF'
%include cli/text/help/hostmail.txt
EOF
}

cmd_help_hostmail_enable() {
    cat <<'EOF'
%include cli/text/help/hostmail/enable.txt
EOF
}

cmd_help_hostmail_uninstall() {
    cat <<'EOF'
%include cli/text/help/hostmail/uninstall.txt
EOF
}

cmd_help_hostmail_upgrade() {
    cat <<'EOF'
%include cli/text/help/hostmail/upgrade.txt
EOF
}

cmd_help_hostmail_disable() {
    cat <<'EOF'
%include cli/text/help/hostmail/disable.txt
EOF
}

cmd_help_hostmail_rotate_admin() {
    cat <<'EOF'
%include cli/text/help/hostmail/rotate-admin.txt
EOF
}

cmd_help_hostmail_password() {
    cat <<'EOF'
%include cli/text/help/hostmail/password.txt
EOF
}

cmd_help_hostmail_password_keep() {
    cat <<'EOF'
%include cli/text/help/hostmail/password/keep.txt
EOF
}

cmd_help_hostmail_password_rotate() {
    cat <<'EOF'
%include cli/text/help/hostmail/password/rotate.txt
EOF
}

cmd_help_hostmail_cert() {
    cat <<'EOF'
%include cli/text/help/hostmail/cert.txt
EOF
}

cmd_help_hostmail_outbound() {
    cat <<'EOF'
%include cli/text/help/hostmail/outbound.txt
EOF
}

cmd_help_hostmail_cert_owner() {
    cat <<'EOF'
%include cli/text/help/hostmail/cert/owner.txt
EOF
}

cmd_help_hostmail_backup() {
    cat <<'EOF'
%include cli/text/help/hostmail/backup.txt
EOF
}

cmd_help_hostmail_backup_status() {
    cat <<'EOF'
%include cli/text/help/hostmail/backup/status.txt
EOF
}

cmd_help_hostmail_backup_plan() {
    cat <<'EOF'
%include cli/text/help/hostmail/backup/plan.txt
EOF
}

cmd_help_hostmail_backup_set() {
    cat <<'EOF'
%include cli/text/help/hostmail/backup/set.txt
EOF
}

cmd_help_hostmail_cert_names() {
    cat <<'EOF'
%include cli/text/help/hostmail/cert/names.txt
EOF
}

cmd_help_hostmail_cert_renew() {
    cat <<'EOF'
%include cli/text/help/hostmail/cert/renew.txt
EOF
}

cmd_help_hostmail_delete() {
    cat <<'EOF'
%include cli/text/help/hostmail/delete.txt
EOF
}

cmd_help_hostmail_import() {
    cat <<'EOF'
%include cli/text/help/hostmail/import.txt
EOF
}

cmd_help_hostmail_quota() {
    cat <<'EOF'
%include cli/text/help/hostmail/quota.txt
EOF
}

cmd_help_hostmail_quota_get() {
    cat <<'EOF'
%include cli/text/help/hostmail/quota/get.txt
EOF
}

cmd_help_hostmail_quota_set() {
    cat <<'EOF'
%include cli/text/help/hostmail/quota/set.txt
EOF
}

cmd_help_hostmail_app_password() {
    cat <<'EOF'
%include cli/text/help/hostmail/app-password.txt
EOF
}

cmd_help_hostmail_person() {
    cat <<'EOF'
%include cli/text/help/hostmail/person.txt
EOF
}

cmd_help_hostmail_app_password_keep() {
    cat <<'EOF'
%include cli/text/help/hostmail/app-password/keep.txt
EOF
}

cmd_help_hostmail_app_password_add() {
    cat <<'EOF'
%include cli/text/help/hostmail/app-password/add.txt
EOF
}

cmd_help_hostmail_app_password_list() {
    cat <<'EOF'
%include cli/text/help/hostmail/app-password/list.txt
EOF
}

cmd_help_hostmail_app_password_revoke() {
    cat <<'EOF'
%include cli/text/help/hostmail/app-password/revoke.txt
EOF
}

cmd_help_hostmail_dkim() {
    cat <<'EOF'
%include cli/text/help/hostmail/dkim.txt
EOF
}

cmd_help_hostmail_dkim_show() {
    cat <<'EOF'
%include cli/text/help/hostmail/dkim/show.txt
EOF
}

cmd_help_hostmail_dkim_rotate() {
    cat <<'EOF'
%include cli/text/help/hostmail/dkim/rotate.txt
EOF
}

cmd_help_hostmail_dkim_retire() {
    cat <<'EOF'
%include cli/text/help/hostmail/dkim/retire.txt
EOF
}

cmd_help_hostmail_dmarc() {
    cat <<'EOF'
%include cli/text/help/hostmail/dmarc.txt
EOF
}

cmd_help_hostmail_dmarc_show() {
    cat <<'EOF'
%include cli/text/help/hostmail/dmarc/show.txt
EOF
}

cmd_help_hostmail_dmarc_report_to() {
    cat <<'EOF'
%include cli/text/help/hostmail/dmarc/report-to.txt
EOF
}

cmd_help_hostmail_ptr() {
    cat <<'EOF'
%include cli/text/help/hostmail/ptr.txt
EOF
}

cmd_help_hostmail_ptr_show() {
    cat <<'EOF'
%include cli/text/help/hostmail/ptr/show.txt
EOF
}

cmd_help_hostmail_ptr_set() {
    cat <<'EOF'
%include cli/text/help/hostmail/ptr/set.txt
EOF
}

cmd_help_hostmail_bans() {
    cat <<'EOF'
%include cli/text/help/hostmail/bans.txt
EOF
}

cmd_help_hostmail_bans_list() {
    cat <<'EOF'
%include cli/text/help/hostmail/bans/list.txt
EOF
}

cmd_help_hostmail_bans_clear() {
    cat <<'EOF'
%include cli/text/help/hostmail/bans/clear.txt
EOF
}

cmd_help_hostmail_bans_migrate() {
    cat <<'EOF'
%include cli/text/help/hostmail/bans/migrate.txt
EOF
}

cmd_help_hostmail_bans_migrate_restore() {
    cat <<'EOF'
%include cli/text/help/hostmail/bans/migrate/restore.txt
EOF
}

cmd_help_hostmail_bans_migrate_status() {
    cat <<'EOF'
%include cli/text/help/hostmail/bans/migrate/status.txt
EOF
}

cmd_help_hostmail_allowlist() {
    cat <<'EOF'
%include cli/text/help/hostmail/allowlist.txt
EOF
}

cmd_help_hostmail_allowlist_list() {
    cat <<'EOF'
%include cli/text/help/hostmail/allowlist/list.txt
EOF
}

cmd_help_hostmail_allowlist_add() {
    cat <<'EOF'
%include cli/text/help/hostmail/allowlist/add.txt
EOF
}

cmd_help_hostmail_allowlist_remove() {
    cat <<'EOF'
%include cli/text/help/hostmail/allowlist/remove.txt
EOF
}

cmd_help_hostmail_catchall() {
    cat <<'EOF'
%include cli/text/help/hostmail/catchall.txt
EOF
}

cmd_help_hostmail_catchall_set() {
    cat <<'EOF'
%include cli/text/help/hostmail/catchall/set.txt
EOF
}

cmd_help_hostmail_catchall_unset() {
    cat <<'EOF'
%include cli/text/help/hostmail/catchall/unset.txt
EOF
}

cmd_help_hostmail_catchall_show() {
    cat <<'EOF'
%include cli/text/help/hostmail/catchall/show.txt
EOF
}

cmd_help_hostmail_alias() {
    cat <<'EOF'
%include cli/text/help/hostmail/alias.txt
EOF
}

cmd_help_hostmail_alias_add() {
    cat <<'EOF'
%include cli/text/help/hostmail/alias/add.txt
EOF
}

cmd_help_hostmail_alias_remove() {
    cat <<'EOF'
%include cli/text/help/hostmail/alias/remove.txt
EOF
}

cmd_help_hostmail_alias_list() {
    cat <<'EOF'
%include cli/text/help/hostmail/alias/list.txt
EOF
}

cmd_help_hostmail_forward() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward.txt
EOF
}

cmd_help_hostmail_forward_set() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/set.txt
EOF
}

cmd_help_hostmail_forward_add() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/add.txt
EOF
}

cmd_help_hostmail_forward_rm() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/rm.txt
EOF
}

cmd_help_hostmail_forward_unset() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/unset.txt
EOF
}

cmd_help_hostmail_forward_show() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/show.txt
EOF
}

cmd_help_hostmail_forward_pause() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/pause.txt
EOF
}

cmd_help_hostmail_forward_resume() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/resume.txt
EOF
}

cmd_help_hostmail_forward_list() {
    cat <<'EOF'
%include cli/text/help/hostmail/forward/list.txt
EOF
}

cmd_help_mail_forward() {
    cat <<'EOF'
%include cli/text/help/mail/forward.txt
EOF
}

cmd_help_hostmail_person_link() {
    cat <<'EOF'
%include cli/text/help/hostmail/person/link.txt
EOF
}

cmd_help_hostmail_ooo() {
    cat <<'EOF'
%include cli/text/help/hostmail/ooo.txt
EOF
}

cmd_help_hostmail_ooo_set() {
    cat <<'EOF'
%include cli/text/help/hostmail/ooo/set.txt
EOF
}

cmd_help_hostmail_ooo_unset() {
    cat <<'EOF'
%include cli/text/help/hostmail/ooo/unset.txt
EOF
}

cmd_help_hostmail_ooo_show() {
    cat <<'EOF'
%include cli/text/help/hostmail/ooo/show.txt
EOF
}

cmd_help_hostmail_footer() {
    cat <<'EOF'
%include cli/text/help/hostmail/footer.txt
EOF
}

cmd_help_hostmail_footer_set() {
    cat <<'EOF'
%include cli/text/help/hostmail/footer/set.txt
EOF
}

cmd_help_hostmail_footer_unset() {
    cat <<'EOF'
%include cli/text/help/hostmail/footer/unset.txt
EOF
}

cmd_help_hostmail_footer_show() {
    cat <<'EOF'
%include cli/text/help/hostmail/footer/show.txt
EOF
}

cmd_help_hostmail_list() {
    cat <<'EOF'
%include cli/text/help/hostmail/list.txt
EOF
}

cmd_help_hostmail_list_create() { cmd_help_hostmail_list; }
cmd_help_hostmail_list_show() { cmd_help_hostmail_list; }
cmd_help_hostmail_list_members() { cmd_help_hostmail_list; }
cmd_help_hostmail_list_delete() { cmd_help_hostmail_list; }

cmd_help_hostmail_spam() {
    cat <<'EOF'
%include cli/text/help/hostmail/spam.txt
EOF
}

cmd_help_hostmail_spam_train() { cmd_help_hostmail_spam; }
cmd_help_hostmail_spam_sender() { cmd_help_hostmail_spam; }
cmd_help_hostmail_spam_quarantine() { cmd_help_hostmail_spam; }

cmd_help_hostmail_queue() {
    cat <<'EOF'
%include cli/text/help/hostmail/queue.txt
EOF
}

cmd_help_hostmail_acl() {
    cat <<'EOF'
%include cli/text/help/hostmail/acl.txt
EOF
}

cmd_help_hostmail_autoconfig() {
    cat <<'EOF'
%include cli/text/help/hostmail/autoconfig.txt
EOF
}

cmd_help_hostmail_profile() {
    cat <<'EOF'
%include cli/text/help/hostmail/profile.txt
EOF
}

cmd_help_hostmail_audit_sieve() {
    cat <<'EOF'
%include cli/text/help/hostmail/audit-sieve.txt
EOF
}

cmd_help_hostmail_calendar() {
    cat <<'EOF'
%include cli/text/help/hostmail/calendar.txt
EOF
}

cmd_help_hostmail_calendar_status() {
    cat <<'EOF'
%include cli/text/help/hostmail/calendar/status.txt
EOF
}

cmd_help_hostmail_calendar_enable() {
    cat <<'EOF'
%include cli/text/help/hostmail/calendar/enable.txt
EOF
}

cmd_help_hostmail_calendar_disable() {
    cat <<'EOF'
%include cli/text/help/hostmail/calendar/disable.txt
EOF
}

cmd_help_hostmail_group() {
    cat <<'EOF'
%include cli/text/help/hostmail/group.txt
EOF
}

cmd_help_hostmail_calendar_share() {
    cat <<'EOF'
%include cli/text/help/hostmail/calendar/share.txt
EOF
}

