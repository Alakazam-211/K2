# ── K2 Mail — `k2 mail` command group (prd-email-server-v1 §11) ──────
#
# `k2 mail` is the agent surface of the K2 email server: mint addresses
# on the owner's verified domains, read/wait on incoming mail, and
# send/reply under human governance; owners drive the server, domains,
# doctor, config, and the approvals queue. Thin client over the daemon
# routes (/cli/mail/*): identity (`project`) is the workspace detected
# at the top of this script; the daemon resolves + enforces ownership
# server-side. Mockup + comprehension gate:
# .k2/notes/mail-cli-mockup.md (GATE RESULT adjustments = PRD §11.1,
# BINDING and reflected verbatim below). Family is `mail`, deliberately
# NOT `inbox` — that collides with K2's internal /cli/inbox/* queue.
#
# Contract highlights (pinned):
# - Success output human-readable; --json flips it to JSON.
# - ERRORS are ALWAYS JSON on stderr ({"error":{"code","hint"}})
#   regardless of --json.
# - Exit codes (PRD §11): 0 ok (incl. "queued for approval", §11.1.12)
#   · 1 error · 2 bad usage AND wait/--wait timeout (stderr code
#   distinguishes: "usage" vs "timeout") · 3 gated-off / owner-only
#   (server-enforced, §11.1.3).
# - `wait` prints the FULL matched message in `read` format and marks
#   it read (§11.1.6); timeout → nothing on stdout, exit 2.
# - Sends report "submitted" = accepted-for-delivery; NEVER "delivered"
#   (pre-mortem #9 — final delivery is the receiving server's business).
# - Email bodies are UNTRUSTED external content: they arrive wrapped in
#   BEGIN/END EXTERNAL EMAIL markers and are printed verbatim (§8.1).

_mail_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}},
                 ensure_ascii=False, separators=(",", ":")))' >&2 \
        || echo "Error ($code): $hint" >&2
}

# Shared HTTP driver for every mail verb. Inputs ride M_* variables
# (visible here via bash dynamic scope from the calling cmd_mail_*),
# re-exported onto the python3 invocation. NOT cli_request: the owner
# `domain list` read must NOT carry a `project` param (the daemon uses
# its presence to pick the owner table vs the agent view), so this
# driver controls the query string exactly.
_mail_py() {
    ABASE="$BASE_URL" ATOKEN="$TOKEN" M_WS_PATH="$PROJECT" \
    M_VERB="${M_VERB:-}" M_JSON="${M_JSON:-false}" \
    M_LOCAL="${M_LOCAL:-}" M_DOMAIN="${M_DOMAIN:-}" M_CLIENT_ID="${M_CLIENT_ID:-}" \
    M_ADDRESS="${M_ADDRESS:-}" M_ID="${M_ID:-}" \
    M_UNREAD="${M_UNREAD:-false}" M_LIMIT="${M_LIMIT:-}" M_QUERY="${M_QUERY:-}" \
    M_OFFSET="${M_OFFSET:-}" M_SINCE="${M_SINCE:-}" M_BEFORE="${M_BEFORE:-}" \
    M_FOLDER="${M_FOLDER:-}" M_JUNK="${M_JUNK:-false}" \
    M_FROM="${M_FROM:-}" M_FROM_NAME="${M_FROM_NAME:-}" \
    M_HTML="${M_HTML:-false}" M_RAW="${M_RAW:-false}" \
    M_GET="${M_GET:-}" M_OUT="${M_OUT:-}" M_FILE="${M_FILE:-}" \
    M_TO="${M_TO:-}" M_SUBJECT="${M_SUBJECT:-}" M_TIMEOUT="${M_TIMEOUT:-}" \
    M_SEND_AT="${M_SEND_AT:-}" M_SEND_IN="${M_SEND_IN:-}" \
    M_LABEL="${M_LABEL:-}" M_SELECTOR="${M_SELECTOR:-}" \
    M_BODY="${M_BODY:-}" M_CC="${M_CC:-}" M_BCC="${M_BCC:-}" M_ATTACH="${M_ATTACH:-}" \
    M_REPLY_TO="${M_REPLY_TO:-}" M_REPLY_TO_ORIGINAL="${M_REPLY_TO_ORIGINAL:-false}" M_AS_EML="${M_AS_EML:-false}" \
    M_ALWAYS_BCC="${M_ALWAYS_BCC:-}" M_ALWAYS_BCC_SET="${M_ALWAYS_BCC_SET:-false}" \
    M_WAIT="${M_WAIT:-false}" M_CONFIRM="${M_CONFIRM:-false}" M_PURGE="${M_PURGE:-false}" \
    M_NOTE="${M_NOTE:-}" M_HEALTH="${M_HEALTH:-false}" \
    M_SEND_MODE="${M_SEND_MODE:-}" M_RELAY_HOST="${M_RELAY_HOST:-}" \
    M_RELAY_PORT="${M_RELAY_PORT:-}" M_RELAY_USER="${M_RELAY_USER:-}" \
    M_RELAY_PASS="${M_RELAY_PASS:-}" M_AGENT_SEND="${M_AGENT_SEND:-}" \
    M_ADDRESS_CAP="${M_ADDRESS_CAP:-}" M_WORKSPACE="${M_WORKSPACE:-}" \
    M_QUOTA_BYTES="${M_QUOTA_BYTES:-}" M_QUOTA_GB="${M_QUOTA_GB:-}" \
    M_QUOTA_MESSAGES="${M_QUOTA_MESSAGES:-}" \
    M_ALIAS="${M_ALIAS:-}" M_MAILBOX="${M_MAILBOX:-}" M_KEEP="${M_KEEP:-false}" \
    M_KEEP_ARG="${M_KEEP_ARG:-}" M_RESTART="${M_RESTART:-}" M_PER_DAY="${M_PER_DAY:-}" M_STATE="${M_STATE:-}" M_KIND="${M_KIND:-}" \
    M_TEXT="${M_TEXT:-}" M_DAYS="${M_DAYS:-}" M_UNTIL="${M_UNTIL:-}" \
    M_LEVEL="${M_LEVEL:-}" \
    M_READ="${M_READ:-}" M_FLAGGED="${M_FLAGGED:-}" \
    M_MANAGE="${M_MANAGE:-}" M_DELETE="${M_DELETE:-}" \
    M_FOLDER_FROM="${M_FOLDER_FROM:-}" M_FOLDER_TO="${M_FOLDER_TO:-}" \
    M_HOST="${M_HOST:-}" M_PORT="${M_PORT:-}" M_TLS="${M_TLS:-}" \
    M_USER="${M_USER:-}" M_NAME="${M_NAME:-}" \
    M_DRAFTS_FOLDER="${M_DRAFTS_FOLDER:-}" \
    M_PASS="${M_PASS:-}" M_PASS_REF="${M_PASS_REF:-}" \
    M_PROVIDER="${M_PROVIDER:-}" M_FEATURES="${M_FEATURES:-}" M_TENANT="${M_TENANT:-}" \
    M_OAUTH_SECRET="${M_OAUTH_SECRET:-}" M_LINK_CAL="${M_LINK_CAL:-}" \
    M_SMTP_HOST="${M_SMTP_HOST:-}" M_SMTP_PORT="${M_SMTP_PORT:-}" M_SMTP_TLS="${M_SMTP_TLS:-}" \
    M_HOSTNAME="${M_HOSTNAME:-}" \
    M_IP="${M_IP:-}" M_ALL="${M_ALL:-false}" \
    M_REASON="${M_REASON:-}" M_EXPIRES="${M_EXPIRES:-}" \
    M_HOURS="${M_HOURS:-}" \
    M_MAILDIR="${M_MAILDIR:-}" M_IMAPSYNC_HOST="${M_IMAPSYNC_HOST:-}" \
    M_IMAPSYNC_USER="${M_IMAPSYNC_USER:-}" M_IMAPSYNC_PASS="${M_IMAPSYNC_PASS:-}" \
    M_IMAPSYNC_PORT="${M_IMAPSYNC_PORT:-}" M_SKIP_INBOX="${M_SKIP_INBOX:-false}" \
    M_PERSON="${M_PERSON:-}" \
    M_MEMBERS="${M_MEMBERS:-}" M_LIST_OP="${M_LIST_OP:-}" \
    M_SENDER="${M_SENDER:-}" M_CLASS="${M_CLASS:-}" \
    M_MAILBOX="${M_MAILBOX:-}" M_RIGHTS="${M_RIGHTS:-}" \
    M_ACL_READ="${M_ACL_READ:-false}" M_ACL_WRITE="${M_ACL_WRITE:-false}" \
    M_ACL_POST="${M_ACL_POST:-false}" M_ACL_ADMIN="${M_ACL_ADMIN:-false}" \
    M_DAV_ACTION="${M_DAV_ACTION:-}" M_FILES="${M_FILES:-}" \
    M_GROUP="${M_GROUP:-}" M_GROUP_MAIL="${M_GROUP_MAIL:-false}" \
    M_CAL_ACTION="${M_CAL_ACTION:-}" M_CALENDAR="${M_CALENDAR:-}" M_COLOR="${M_COLOR:-}" \
    M_WITH="${M_WITH:-}" M_SHARE_LEVEL="${M_SHARE_LEVEL:-}" M_MAX_SHARES="${M_MAX_SHARES:-}" \
    M_FORCE="${M_FORCE:-false}" \
    M_CERT_ACTION="${M_CERT_ACTION:-}" M_OUTBOUND_ACTION="${M_OUTBOUND_ACTION:-}" \
    M_DRY_RUN="${M_DRY_RUN:-false}" M_ACK_FAILED="${M_ACK_FAILED:-false}" \
    M_BK_MODE="${M_BK_MODE:-}" M_BK_BANDWIDTH="${M_BK_BANDWIDTH:-}" \
    M_BK_KEEP_DAILY="${M_BK_KEEP_DAILY:-}" M_BK_KEEP_WEEKLY="${M_BK_KEEP_WEEKLY:-}" \
    M_BK_OFF_DAILY="${M_BK_OFF_DAILY:-}" M_BK_OFF_WEEKLY="${M_BK_OFF_WEEKLY:-}" \
    M_BK_OFF_MONTHLY="${M_BK_OFF_MONTHLY:-}" M_BK_WINDOW="${M_BK_WINDOW:-}" \
    M_BK_MAX_LOCAL_GB="${M_BK_MAX_LOCAL_GB:-}" M_BK_CONFIRM="${M_BK_CONFIRM:-}" \
    python3 - <<'PYEOF'
import base64, json, os, socket, sys, time
import urllib.error, urllib.parse, urllib.request

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
verb = os.environ["M_VERB"]
as_json = os.environ.get("M_JSON") == "true"
ws_path = os.environ.get("M_WS_PATH", "")

def dumps(obj):
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))

def die(err, exit_code=None, permission=None):
    # Errors are ALWAYS JSON on stderr, regardless of --json.
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    code = err.get("code", "error")
    hint = err.get("hint", "")
    if code in ("forbidden", "owner_only") and "ask your human" not in hint:
        # §11.1.3 / GH #34 teaching line — owner verbs are futile for
        # agents, by server-side enforcement; the fix is always a human.
        hint = (hint + " — " if hint else "") + "requires owner/admin — ask your human"
    out = {"code": code, "hint": hint}
    # MCS4: a mail verb on a link consented for its calendar only answers
    # capability_missing; keep what is missing, why and the owner's fix.
    for k in ("cap", "reason", "fix"):
        if err.get(k):
            out[k] = err[k]
    sys.stderr.write(dumps({"error": out}) + "\n")
    # Permission cards (vs-live C4): a refusal sent to an agent while its
    # switches are on carries `permission`; it ends with the one closing
    # line the other families print ("Ask with: …" / "It can't be requested.").
    if isinstance(permission, dict):
        cmd = permission.get("command")
        if isinstance(cmd, str) and cmd:
            sys.stderr.write("Ask with: " + cmd + "\n")
        elif permission.get("requestable") is False:
            if isinstance(permission.get("hint"), str) and permission["hint"]:
                sys.stderr.write(permission["hint"] + "\n")
            sys.stderr.write("It can't be requested.\n")
    if exit_code is None:
        # PRD §11 exit contract: 3 = gated-off / owner-only · 2 = bad
        # usage (timeouts also exit 2, via explicit die(..., 2) at the
        # wait/--wait sites) · 1 = everything else.
        # GH #34: daemon returns code owner_only on hostmail/access/…
        # owner surfaces for valid agent passports (not "invalid token").
        # 0.45.0: agent_send_path_only = an agent asked for an IMAP/SMTP
        # secret; the hint already names `k2 mail send` + the owner verb.
        exit_code = 3 if code in ("gated", "forbidden", "owner_only", "agent_send_path_only", "capability_missing") else 2 if code in ("usage", "attachment_secret", "attachment_too_large") else 1
    sys.exit(exit_code)

def _is_socket_timeout(exc):
    # urllib wraps a read/connect timeout as URLError(TimeoutError) (or
    # socket.timeout, an alias of TimeoutError on 3.10+). Connection
    # refused is a different OSError and must stay daemon_unreachable.
    cur = exc
    for _ in range(4):
        if isinstance(cur, TimeoutError) or isinstance(cur, socket.timeout):
            return True
        nxt = getattr(cur, "reason", None)
        if nxt is None or nxt is cur:
            break
        cur = nxt
    return False

def _timeout_error_body(timeout_secs):
    try:
        n = int(timeout_secs)
    except (TypeError, ValueError):
        n = timeout_secs
    return {"error": {
        "code": "timeout",
        "hint": "daemon did not finish within %ss (still running — the mail fetch is slow, not down)" % n,
    }}

def request(method, route, params=None, body=None, timeout=30):
    env_t = os.environ.get("K2_MAIL_HTTP_TIMEOUT")
    if env_t:
        timeout = float(env_t)
    qs = urllib.parse.urlencode(dict(params or {}, token=token))
    url = base + route + "?" + qs
    data = dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {})
    # k2-ptyd S5 (P47, P48): a daemon restart is not a failure. A refused
    # connect and the daemon's pre-ready 503 ("state":"migrating") never
    # reached a handler: retried (4 tries, 1 s apart). A GET whose
    # connection dropped with no answer (`mail wait` cut by the restart)
    # is retried once. Anything that got an answer is final.
    dropped = False
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read().decode("utf-8", "replace"))
        except urllib.error.HTTPError as e:
            raw = e.read().decode("utf-8", "replace")
            if e.code == 503 and '"state":"migrating"' in raw and attempt < 3:
                time.sleep(1)
                continue
            try:
                return json.loads(raw)
            except Exception:
                return {"error": {"code": "error", "hint": "HTTP %s on %s" % (e.code, route)}}
        except Exception as e:
            if _is_socket_timeout(e):
                return _timeout_error_body(timeout)
            kind = _restart_kind(e)
            if kind == "refused" and attempt < 3:
                time.sleep(1)
                continue
            if kind == "dropped" and method == "GET" and not dropped and attempt < 3:
                dropped = True
                time.sleep(1)
                continue
            return None
    return None

def _restart_kind(exc):
    # "refused": nothing accepted the connect. "dropped": the connection
    # closed before any answer (RemoteDisconnected is a ConnectionResetError).
    import http.client
    cur = exc
    for _ in range(4):
        if isinstance(cur, ConnectionRefusedError):
            return "refused"
        if isinstance(cur, (ConnectionResetError, BrokenPipeError, http.client.IncompleteRead)):
            return "dropped"
        nxt = getattr(cur, "reason", None)
        if nxt is None or nxt is cur:
            break
        cur = nxt
    return None

def call(method, route, params=None, body=None, timeout=30, timeout_hint=None):
    d = request(method, route, params, body, timeout)
    if d is None:
        die({"code": "daemon_unreachable", "hint": "no response from the K2 daemon (is it running?)"})
    if timeout_hint and (d.get("error") or {}).get("code") == "timeout":
        die({"code": "timeout", "hint": timeout_hint(d["error"].get("hint", ""))})
    if d.get("error"):
        die(d["error"], permission=d.get("permission"))
    if not d.get("ok"):
        die({"code": "error", "hint": "unexpected daemon response: %s" % dumps(d)[:200]})
    return d

def env(k):
    return os.environ.get(k, "")

def env_list(k):
    # Bash joins repeatable flags with newlines; a single value may
    # also be comma-separated (addresses cannot contain commas).
    out = []
    for chunk in env(k).split("\n"):
        for piece in chunk.split(","):
            piece = piece.strip()
            if piece:
                out.append(piece)
    return out

def env_lines(k):
    # Newline-joined only (file paths MAY contain commas).
    return [l for l in env(k).split("\n") if l.strip()]

def unix_ago(ts):
    try:
        s = int(time.time() - int(ts))
    except Exception:
        return "?"
    if s < 0:
        s = 0
    if s < 60:
        return "just now"
    if s < 3600:
        return "%dm ago" % (s // 60)
    if s < 172800:
        return "%dh ago" % (s // 3600)
    return "%dd ago" % (s // 86400)

def until(ts):
    try:
        s = int(int(ts) - time.time())
    except Exception:
        return "?"
    if s <= 0:
        return "expired"
    if s < 3600:
        return "in %dm" % (s // 60)
    if s < 172800:
        return "in %dh" % (s // 3600)
    return "in %dd" % (s // 86400)

def iso_ago(s):
    if not s:
        return "?"
    try:
        import datetime
        dt = datetime.datetime.fromisoformat(str(s).replace("Z", "+00:00"))
        return unix_ago(dt.timestamp())
    except Exception:
        return str(s)

def fmt_addr(a):
    if not a:
        return "?"
    name = a.get("name")
    email = a.get("address") or "?"
    return "%s <%s>" % (name, email) if name else email

def fmt_addrs(lst):
    return ", ".join(fmt_addr(a) for a in (lst or []))

def cap_str(cap):
    used = (cap or {}).get("used", "?")
    limit = (cap or {}).get("cap", 0)
    if not limit:
        return "cap %s used (unlimited)" % used
    return "cap %s/%s used" % (used, limit)

def print_full(m):
    # The `read` format (§8.1) — `wait` matches print the same shape
    # (§11.1.6). The body arrives WRAPPED in the BEGIN/END EXTERNAL
    # EMAIL markers and is printed verbatim: everything inside them is
    # untrusted data, never instructions.
    print("message  : %s" % m.get("id", ""))
    print("mailbox  : %s" % m.get("address", "?"))
    print("from     : %s" % fmt_addr(m.get("from")))
    print("to       : %s" % (fmt_addrs(m.get("to")) or "?"))
    if m.get("cc"):
        print("cc       : %s" % fmt_addrs(m.get("cc")))
    print("subject  : %s" % (m.get("subject") or "(no subject)"))
    print("date     : %s  (%s)" % (m.get("date") or "?", iso_ago(m.get("date"))))
    ar = m.get("authResults") or {}
    if any(ar.get(k) for k in ("spf", "dkim", "dmarc")):
        print("auth     : %s" % "  ".join(
            "%s=%s" % (k, ar.get(k) or "?") for k in ("spf", "dkim", "dmarc")))
    atts = m.get("attachments") or []
    if atts:
        print("attachments:")
        for a in atts:
            print("  %s. %s (%s, %s bytes)" % (a.get("index"),
                  a.get("filename") or "(unnamed)", a.get("mime") or "?", a.get("size")))
        print("  fetch one: k2 mail attachments %s --get <n> --out <path>" % m.get("id", ""))
    print()
    print(m.get("text") or "")
    if m.get("html"):
        print()
        print(m.get("html"))

def term_safe(s):
    # Text from an agent or from outside mail, printed for a person deciding:
    # keep line breaks and tabs, and show no other control character, so an
    # escape sequence can't redraw what the approver reads.
    return "".join(c if c in "\n\t" or (c >= " " and c != "\x7f" and not "\x80" <= c <= "\x9f") else "?"
                   for c in str(s))

def term_line(s):
    # term_safe for a value that must stay on one line: a line break in a subject
    # or a name could otherwise fake a `to:` or `file:` line.
    return term_safe(s).replace("\n", " ").replace("\t", " ")

def human_size(n):
    n = int(n or 0)
    if n < 1024:
        return "%d bytes" % n
    if n < 1024 * 1024:
        return "%.1f KiB" % (n / 1024.0)
    return "%.1f MiB" % (n / (1024.0 * 1024.0))

def file_line(f):
    return "%s (%s, %s%s)" % (term_line(f.get("name") or "?"), human_size(f.get("size")),
                              term_line(f.get("contentType") or "?"),
                              "" if f.get("scanned") else ", not scanned")

def shown_as(r):
    # The display name the recipient sees, which the sender chose.
    return " shown as \"%s\"" % term_line(r["fromName"]) if r.get("fromName") else ""

def dmarc_note(verdict):
    return " (its sender may be forged)" if verdict == "fail" else ""

def forward_line(f):
    return "a message in %s from %s (unverified)%s" % (
        term_line(f.get("mailbox") or "?"), term_line(f.get("sender") or "?"),
        ", whole, as an .eml file" if f.get("asEml") else "")

def print_outbound_row(r):
    print("%s  %-17s %-11s to %s — %s" % (
        r.get("id", ""), r.get("status", "?"), unix_ago(r.get("createdAt")),
        ", ".join(r.get("to") or []) or "?", r.get("subject") or "(no subject)"))
    if r.get("bcc"):
        print("    bcc: %s" % ", ".join(r["bcc"]))
    if r.get("policyBcc"):
        print("    bcc (owner policy): %s" % ", ".join(r["policyBcc"]))
    if r.get("replyTo"):
        print("    reply-to: %s" % ", ".join(r["replyTo"]))
    if r.get("forwardOf"):
        print("    forwards: %s" % forward_line(r["forwardOf"]))
    att = r.get("attachments") or {}
    if att.get("count"):
        # What rode this send — so an agent can verify its own attachment
        # send without opening the recipient inbox (#31.5 follow-up).
        print("    attachments: %d (%s)" % (att["count"], ", ".join(att.get("names") or [])))
    if r.get("note"):
        # §11.1.9: the owner's note rides inline (esp. on rejected).
        print("    note from your human: %s" % r["note"])

def print_outbound_detail(r):
    print("outbound : %s" % r.get("id", ""))
    print("status   : %s — %s" % (r.get("status", "?"), r.get("statusNote", "")))
    print("from     : %s" % (r.get("from") or "?"))
    print("to       : %s" % (", ".join(r.get("to") or []) or "?"))
    if r.get("cc"):
        print("cc       : %s" % ", ".join(r["cc"]))
    if r.get("bcc"):
        print("bcc      : %s" % ", ".join(r["bcc"]))
    if r.get("policyBcc"):
        print("bcc (owner policy): %s" % ", ".join(r["policyBcc"]))
    if r.get("replyTo"):
        print("reply-to : %s" % ", ".join(r["replyTo"]))
    if r.get("forwardOf"):
        print("forwards : %s" % forward_line(r["forwardOf"]))
    print("subject  : %s" % (r.get("subject") or "(no subject)"))
    att = r.get("attachments") or {}
    if att.get("count"):
        print("attach   : %d (%s)" % (att["count"], ", ".join(att.get("names") or [])))
    print("queued   : %s" % unix_ago(r.get("createdAt")))
    if r.get("decidedAt"):
        by = " by %s" % r["decidedBy"] if r.get("decidedBy") else ""
        print("decided  : %s%s" % (unix_ago(r.get("decidedAt")), by))
    if r.get("sentAt"):
        print("submitted: %s" % unix_ago(r.get("sentAt")))
    if r.get("note"):
        print("note     : %s" % r["note"])

DOMAIN_MARKS = {"valid": "valid", "missing": "MISSING", "wrong": "WRONG",
                "pending": "pending", "unverifiable": "manual",
                "optional": "optional", "unknown": "?",
                # 0.46.1: don't publish yet (external zone, no certificate)
                "skip": "skip for now"}

def print_domain(d):
    print("domain  : %s  (%s)" % (d.get("domain", "?"), d.get("status", "?")))
    print("send    : %s" % d.get("sendMode", "?"))
    if d.get("dmarcNag"):
        print("dmarc   : strongly recommended, not yet valid (starts at p=none — observe first)")
    print()
    print("DNS records (set these at your registrar):")
    for r in d.get("records") or []:
        st = r.get("status", "?")
        print("  [%s] %s %s — %s" % (DOMAIN_MARKS.get(st, st),
              r.get("type", ""), r.get("name", ""), r.get("purpose", "")))
        print("      value: %s" % r.get("expected", ""))
        if r.get("live"):
            # Pre-mortem #4: never a bare "missing" — show what DNS
            # actually serves so the user can see the diff.
            print("      live : %s" % " | ".join(r["live"]))
    print()
    if d.get("note"):
        print("note: %s" % d["note"])
    print("re-check now: k2 hostmail domain check %s" % d.get("domain", ""))

def wait_timeout_secs():
    raw = env("M_TIMEOUT")
    return int(raw) if raw else 300

def send_outcome(d, waiting, timeout_secs):
    # Shared tail for send + reply. Shapes (routes_send.rs):
    #   queued (approval, no wait): {ok,queued,id,status,hint}
    #   decided (--wait):           {ok,id,timedOut:false,status,statusNote,note}
    #   wait timeout:               {ok,id,timedOut:true,status,hint}
    #   submitted (on mode):        {ok,id,status:"submitted",hint}
    if waiting and d.get("timedOut"):
        # §11.1.4: exit 2 — the message is STILL QUEUED.
        die({"code": "timeout", "hint": d.get("hint") or
             "not decided within %ss — the message is still queued; check 'k2 mail outbox %s'"
             % (timeout_secs, d.get("id", ""))}, 2)
    if as_json:
        print(dumps(d))
        return
    if d.get("queued"):
        # Queued IS success — exit 0 (§11.1.12). The daemon hint carries
        # the track-with-outbox pointer.
        print(d.get("hint") or "queued for approval (%s) — track with 'k2 mail outbox %s'"
              % (d.get("id", ""), d.get("id", "")))
        return
    if "timedOut" in d:
        # Decided under --wait: approved-and-submitted, rejected, or
        # failed — the status carries the outcome; deciding is what we
        # waited for, so this is exit 0 either way.
        print("%s %s" % (d.get("id", ""), d.get("statusNote") or d.get("status", "")))
        if d.get("note"):
            print("note from your human: %s" % d["note"])
        print("track with: k2 mail outbox %s" % d.get("id", ""))
        return
    # on-mode immediate submit ("submitted" = accepted-for-delivery,
    # never "delivered").
    print("%s %s" % (d.get("id", ""), d.get("hint") or d.get("status", "")))

# ── S11 unified inbox helpers (MUST be defined before the verb chain —
#    a `def` cannot sit between `elif` branches). ──────────────────────
def _level_word(lvl):
    return {"read": "read", "draft": "draft", "send": "send"}.get(lvl, lvl or "?")

def print_inboxes(rows, agent_view):
    if not rows:
        print("No inboxes you can access — mint a hosted address (k2 hostmail create) "
              "or ask your human to link/grant one." if agent_view
              else "No inboxes yet — mint one (k2 hostmail create) or link one (k2 mail link add).")
        return
    w = max(max(len(r.get("address", "")) for r in rows), len("ADDRESS")) + 2
    # SEND = what level 'send' really allows once the workspace's
    # agent-send gate applies (daemon `sendState`; older daemons omit it).
    def _send_word(state):
        return {"on": "yes", "approval": "approval", "blocked_off": "BLOCKED",
                "blocked_approval": "BLOCKED", "no_send_level": "-"}.get(state or "", "?")
    blocked = []
    if agent_view:
        print(("%-" + str(w) + "s%-9s%-7s%-10s%-7s%s") % (
            "ADDRESS", "SOURCE", "LEVEL", "SEND", "MAX", "STATUS"))
        for r in rows:
            st = r.get("yourSendState")
            if st in ("blocked_off", "blocked_approval"):
                blocked.append((r.get("address", ""), st, None))
            print(("%-" + str(w) + "s%-9s%-7s%-10s%-7s%s") % (
                r.get("address", ""), r.get("source", "?"),
                _level_word(r.get("yourLevel")), _send_word(st),
                r.get("maxLevel", "?"), r.get("status", "?")))
        print()
        print("read: k2 mail messages [<address>]   send needs level 'send' (hosted, or a linked inbox with SMTP)")
        print("MAX is the CEILING, not a promise — sending also needs a send transport configured.")
        if blocked:
            mode = "approval" if any(b[1] == "blocked_approval" for b in blocked) else "off"
            print()
            if mode == "off":
                print("SEND BLOCKED: this workspace's agent sending is off, so level 'send' can't "
                      "send yet (k2 mail send/reply answer 'gated').")
            else:
                print("SEND BLOCKED: this workspace's agent sending is 'approval', which only "
                      "queues hosted mailboxes — a linked inbox needs 'on'.")
            print("Ask your human to run: k2 mail sending <this workspace> on   (or Settings → "
                  "Workspaces / Agents → <workspace> → Email sending)")
    else:
        print(("%-" + str(w) + "s%-9s%-22s%s") % ("ADDRESS", "SOURCE", "PRIMARY", "GRANTS"))
        for r in rows:
            pri = r.get("primary") or {}
            who = pri.get("workspace") or pri.get("projectId") or "?"
            grants = r.get("grants") or []
            def _lvl(e):
                st = e.get("sendState")
                if st in ("blocked_off", "blocked_approval"):
                    blocked.append((r.get("address", ""), st,
                                    e.get("workspace") or e.get("projectId", "?")))
                    return "%s, BLOCKED" % e.get("level", "?")
                return e.get("level", "?")
            gsum = ", ".join("%s:%s" % (g.get("workspace") or g.get("projectId", "?"),
                                        _lvl(g)) for g in grants) or "-"
            print(("%-" + str(w) + "s%-9s%-22s%s") % (
                r.get("address", ""), r.get("source", "?"),
                "%s (%s)" % (who, _lvl(pri)), gsum))
        print()
        print("share: k2 mail access grant <address> <workspace> --level read|draft|send")
        if blocked:
            print()
            print("BLOCKED = level 'send', but that workspace's agent sending is off "
                  "(or 'approval' on a linked inbox), so its sends are gated. Turn it on:")
            for ws in sorted(set(b[2] for b in blocked)):
                print("  k2 mail sending %s on" % ws)


# ── Verb dispatch ───────────────────────────────────────────────────

if verb == "create":
    body = {"project": ws_path, "localPart": env("M_LOCAL")}
    if env("M_DOMAIN"):
        body["domain"] = env("M_DOMAIN")
    if env("M_CLIENT_ID"):
        body["clientId"] = env("M_CLIENT_ID")
    if env("M_PERSON") == "true":
        body["person"] = True
    d = call("POST", "/cli/mail/address/create", body=body)
    if as_json:
        print(dumps(d))
    else:
        word = "existing" if d.get("existing") else "created"
        print("%s %s   (%s)" % (word, d.get("address", "?"), cap_str(d.get("cap"))))
        if d.get("note"):
            print("note: %s" % d["note"])
        if d.get("existing"):
            print("(idempotent --id match — this address was already minted with this id)")
        else:
            if d.get("password"):
                print("password : %s  (shown once — save it for Mail.app / IMAP)" % d["password"])
            elif d.get("passwordWithheld"):
                print("password : withheld — %s" % (d.get("passwordHint") or
                      "agents send with `k2 mail send`; only the owner can set an IMAP/SMTP password"))
            print("username : %s" % (d.get("username") or d.get("address") or "?"))
            def _ep(label, key, default_port):
                ep = d.get(key) or {}
                host = ep.get("host") or "?"
                port = ep.get("port") or default_port
                extra = "  alpn" if ep.get("alpn") else ""
                print("%s : %s:%s%s" % (label, host, port, extra))
            _ep("imap    ", "imap", 443)
            _ep("submit  ", "submission", 465)
            _ep("jmap    ", "jmap", 443)

elif verb == "addresses":
    d = call("GET", "/cli/mail/address/list", params={"project": ws_path})
    rows = d.get("addresses") or []
    if as_json:
        print(dumps(d))
    elif not rows:
        print("No addresses yet — mint one: k2 hostmail create <localpart>   (%s)"
              % cap_str(d.get("cap")))
    else:
        w = max(max(len(r.get("address", "")) for r in rows), len("ADDRESS")) + 2
        print(("%-" + str(w) + "s%s") % ("ADDRESS", "CREATED"))
        for r in rows:
            print(("%-" + str(w) + "s%s") % (r.get("address", ""), unix_ago(r.get("createdAt"))))
        print()
        print(cap_str(d.get("cap")))

elif verb == "domains":
    # §11.1.2: the agent view — Verified domains you can mint on, with
    # the workspace default marked (`?project=` selects it route-side).
    d = call("GET", "/cli/mail/domain/list", params={"project": ws_path})
    rows = d.get("domains") or []
    pending = d.get("pending") or []
    if as_json:
        print(dumps(d))
    elif not rows:
        if pending:
            names = ", ".join(p if isinstance(p, str) else str(p.get("domain", p)) for p in pending)
            print("No mintable domains yet. Pending: %s" % names)
            print("details: k2 hostmail domain list")
        else:
            print("No domains yet — add one: k2 hostmail domain add <domain>")
    else:
        for r in rows:
            print("%s%s" % (r.get("domain", "?"), "   (default)" if r.get("isDefault") else ""))
        print()
        print("mint here: k2 hostmail create <localpart>[@<domain>]")
        if pending:
            print("pending (not mintable): k2 hostmail domain list")

elif verb == "messages":
    params = {"project": ws_path}
    if env("M_ADDRESS"):
        params["address"] = env("M_ADDRESS")
    if env("M_UNREAD") == "true":
        params["unread"] = "1"
    if env("M_LIMIT"):
        params["limit"] = env("M_LIMIT")
    if env("M_OFFSET"):
        params["offset"] = env("M_OFFSET")
    if env("M_SINCE"):
        params["since"] = env("M_SINCE")
    if env("M_BEFORE"):
        params["before"] = env("M_BEFORE")
    if env("M_JUNK") == "true":
        params["junk"] = "1"
    elif env("M_FOLDER"):
        params["folder"] = env("M_FOLDER")
    if env("M_QUERY"):
        params["query"] = env("M_QUERY")
    if env("M_FROM"):
        params["from"] = env("M_FROM")
    d = call("GET", "/cli/mail/messages", params=params)
    msgs = d.get("messages") or []
    next_offset = d.get("nextOffset")
    # #31.4: the all-inbox sweep degrades per inbox — some may have
    # errored while the healthy ones' messages still came back. Show a
    # compact count (never the raw hint, which could carry host detail).
    inbox_errors = d.get("inboxErrors") or []
    # Rebuild the flags that select a DIFFERENT set so the "more:" hint
    # (and the empty-set line) reflect the actual query.
    def _scope_flags():
        parts = []
        if env("M_ADDRESS"):
            parts.append(env("M_ADDRESS"))
        if env("M_JUNK") == "true":
            parts.append("--junk")
        elif env("M_FOLDER"):
            parts.append("--folder %s" % env("M_FOLDER"))
        if env("M_UNREAD") == "true":
            parts.append("--unread")
        for envk, flag in (("M_SINCE", "--since"), ("M_BEFORE", "--before"),
                           ("M_QUERY", "--query"), ("M_FROM", "--from"),
                           ("M_LIMIT", "--limit")):
            if env(envk):
                parts.append("%s %s" % (flag, env(envk)))
        return parts
    if as_json:
        print(dumps(d))
    elif not msgs:
        filtered = any(env(k) for k in ("M_QUERY", "M_FROM", "M_ADDRESS", "M_SINCE",
                       "M_BEFORE", "M_FOLDER")) or env("M_UNREAD") == "true" or env("M_JUNK") == "true"
        print("No messages%s." % (" matching the filters" if filtered else ""))
    else:
        for m in msgs:
            extras = ""
            if m.get("hasAttachment"):
                extras += " · attachment"
            if m.get("unread"):
                extras += " · UNREAD"
            print("%s · %s · %s · %s%s" % (m.get("id", ""), fmt_addr(m.get("from")),
                  m.get("subject") or "(no subject)", iso_ago(m.get("date")), extras))
        print()
        print("read one: k2 mail read <id>")
        if next_offset is not None:
            more = _scope_flags() + ["--offset %s" % next_offset]
            print("more:     k2 mail messages %s" % " ".join(more))
    if not as_json:
        # D4: linked IMAP search-window hint prints on hit AND miss.
        hint = (d.get("searchWindow") or {}).get("hint")
        if hint:
            print(hint)
    if not as_json and inbox_errors:
        print("note:     %d inbox(es) had errors and were skipped — read one directly "
              "(k2 mail messages <address>) to see why." % len(inbox_errors))

elif verb == "read":
    params = {"project": ws_path, "id": env("M_ID")}
    raw = env("M_RAW") == "true"
    if env("M_HTML") == "true":
        params["html"] = "1"
    if raw:
        params["raw"] = "1"
    d = call("GET", "/cli/mail/read", params=params)
    if as_json:
        print(dumps(d))
    elif raw:
        sys.stdout.flush()
        sys.stdout.buffer.write(base64.b64decode(d.get("rawBase64") or ""))
    else:
        print_full(d.get("message") or {})

elif verb == "attachments":
    params = {"project": ws_path, "id": env("M_ID")}
    if env("M_GET"):
        params["get"] = env("M_GET")
        params["out"] = env("M_OUT")
    if env("M_GET"):
        # A big attachment from a linked inbox can take minutes; the
        # daemon caps one at 100 MB. On a timeout the daemon is STILL
        # downloading (it does not stop when we stop waiting) and will
        # write the file — say so, so nobody retries in a loop.
        def _attach_timeout_hint(_hint):
            return ("the daemon did not finish attachment #%s within the wait and is still "
                    "downloading it; it writes to '%s' (a folder gets '<folder>/<attachment "
                    "name>') when done. Check for the file before retrying; a retry while it "
                    "runs answers 'busy'" % (env("M_GET"), env("M_OUT")))
        d = call("POST", "/cli/mail/attachments", params=params, timeout=300,
                 timeout_hint=_attach_timeout_hint)
    else:
        d = call("GET", "/cli/mail/attachments", params=params)
    if as_json:
        print(dumps(d))
    elif env("M_GET"):
        print("wrote attachment #%s: %s (%s, %s bytes) -> %s" % (d.get("index"),
              d.get("filename") or "?", d.get("mime") or "?", d.get("bytes"), d.get("wrote")))
    else:
        atts = d.get("attachments") or []
        if not atts:
            print("No attachments on %s." % d.get("id", ""))
        else:
            for a in atts:
                print("  %s. %s (%s, %s bytes)" % (a.get("index"),
                      a.get("filename") or "(unnamed)", a.get("mime") or "?", a.get("size")))
            print()
            print("fetch one: k2 mail attachments %s --get <n> --out <path>   (n is 1-based)"
                  % d.get("id", ""))

elif verb == "wait":
    params = {"project": ws_path}
    for envk, key in (("M_TO", "to"), ("M_FROM", "from"),
                      ("M_SUBJECT", "subject"), ("M_TIMEOUT", "timeout")):
        if env(envk):
            params[key] = env(envk)
    t = wait_timeout_secs()
    # The daemon HOLDS this request (long-poll, §8.2) — the HTTP
    # timeout must outlast the wait budget.
    d = call("GET", "/cli/mail/wait", params=params, timeout=t + 30)
    if d.get("timedOut"):
        # §8.2/§11: nothing on stdout, exit 2 (stderr carries the why).
        die({"code": "timeout",
             "hint": "no matching message within %ss — loop 'k2 mail wait' to keep "
                     "listening (the blessed pattern)" % t}, 2)
    if as_json:
        print(dumps(d))
    else:
        print_full(d.get("message") or {})

elif verb == "send":
    body = {"project": ws_path, "to": env_list("M_TO"),
            "subject": env("M_SUBJECT"), "body": env("M_BODY")}
    cc = env_list("M_CC")
    if cc:
        body["cc"] = cc
    bcc = env_list("M_BCC")
    if bcc:
        # Envelope-only blind copies (the daemon never writes a Bcc header).
        body["bcc"] = bcc
    if env("M_FROM"):
        body["from"] = env("M_FROM")
    if env("M_FROM_NAME"):
        # From DISPLAY NAME only — cosmetic; the daemon validates it and
        # still stamps the address (hosted mailboxes only).
        body["fromName"] = env("M_FROM_NAME")
    reply_to = env_list("M_REPLY_TO")
    if reply_to:
        # The Reply-To header; the daemon validates and audits it.
        body["replyTo"] = reply_to
    attach = env_lines("M_ATTACH")
    if attach:
        # Workspace-relative file PATHS — the daemon reads the bytes
        # (CLI stays dumb: no file I/O, no base64 over the wire).
        body["attachments"] = attach
    if env("M_SEND_AT"):
        body["sendAt"] = env("M_SEND_AT")
    if env("M_SEND_IN"):
        body["sendIn"] = env("M_SEND_IN")
    waiting = env("M_WAIT") == "true"
    t = wait_timeout_secs()
    if waiting:
        body["wait"] = True
        if env("M_TIMEOUT"):
            body["timeout"] = int(env("M_TIMEOUT"))
    # Files go up to the mail server inside the call.
    d = call("POST", "/cli/mail/send", body=body, timeout=(t + 30) if waiting else (180 if attach else 30))
    send_outcome(d, waiting, t)

elif verb == "reply":
    body = {"project": ws_path, "id": env("M_ID"), "body": env("M_BODY")}
    if env("M_FROM_NAME"):
        # From DISPLAY NAME only (as on send) — the daemon validates it;
        # the address stays locked to the receiving one.
        body["fromName"] = env("M_FROM_NAME")
    attach = env_lines("M_ATTACH")
    if attach:
        # Workspace-relative file PATHS — the daemon reads the bytes.
        body["attachments"] = attach
    waiting = env("M_WAIT") == "true"
    t = wait_timeout_secs()
    if waiting:
        body["wait"] = True
        if env("M_TIMEOUT"):
            body["timeout"] = int(env("M_TIMEOUT"))
    d = call("POST", "/cli/mail/reply", body=body, timeout=(t + 30) if waiting else (180 if attach else 30))
    send_outcome(d, waiting, t)

elif verb == "forward_message":
    body = {"project": ws_path, "id": env("M_ID"), "to": env_list("M_TO")}
    for envk, key in (("M_CC", "cc"), ("M_BCC", "bcc"), ("M_REPLY_TO", "replyTo")):
        vals = env_list(envk)
        if vals:
            body[key] = vals
    for envk, key in (("M_NOTE", "note"), ("M_FROM", "from"), ("M_FROM_NAME", "fromName")):
        if env(envk):
            body[key] = env(envk)
    if env("M_AS_EML") == "true":
        body["asEml"] = True
    if env("M_REPLY_TO_ORIGINAL") == "true":
        body["replyToOriginal"] = True
    waiting = env("M_WAIT") == "true"
    t = wait_timeout_secs()
    if waiting:
        body["wait"] = True
        if env("M_TIMEOUT"):
            body["timeout"] = int(env("M_TIMEOUT"))
    # The original's files come down and go back up inside the call.
    d = call("POST", "/cli/mail/forward-message", body=body, timeout=(t + 180) if waiting else 180)
    send_outcome(d, waiting, t)

elif verb == "outbox_cancel":
    d = call("POST", "/cli/mail/outbox/cancel",
             body={"project": ws_path, "id": env("M_ID")})
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or ("cancelled %s" % d.get("id", "")))

elif verb == "outbox":
    params = {"project": ws_path}
    if env("M_ID"):
        params["id"] = env("M_ID")
    if env("M_LIMIT"):
        params["limit"] = env("M_LIMIT")
    d = call("GET", "/cli/mail/outbox", params=params)
    if as_json:
        print(dumps(d))
    elif env("M_ID"):
        print_outbound_detail(d.get("outbound") or {})
    else:
        rows = d.get("outbox") or []
        if not rows:
            print("Outbox empty — nothing queued or sent yet.")
        else:
            for r in rows:
                print_outbound_row(r)

elif verb == "drafts":
    # Your drafted replies and what a person did with each (records only).
    params = {"project": ws_path}
    if env("M_LIMIT"):
        params["limit"] = env("M_LIMIT")
    d = call("GET", "/cli/mail/drafts", params=params)
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("drafts") or []
        if not rows:
            print("No drafted replies yet — 'k2 mail draft <message-id> --body <text>' makes one.")
        for r in rows:
            st = r.get("status") or "?"
            line = "%s  %-9s %-34s answers %s" % (r.get("id"), st, r.get("inbox"), r.get("replyOf"))
            if r.get("editedBy"):
                line += "  · edited by %s" % r["editedBy"]
            if st in ("sent", "held", "discarded") and r.get("stateBy"):
                line += "  · %s by %s" % (st, r["stateBy"])
            if r.get("outboundId"):
                line += "  · outbox %s" % r["outboundId"]
            print(line)

elif verb == "address_delete":
    # Retire a HOSTED address (k2 hostmail delete, and k2 mail delete when
    # the argument is an address). Distinct verb from the message-Trash
    # "delete" below so the two never collide in this if/elif chain.
    d = call("POST", "/cli/mail/address/delete",
             body={"project": ws_path, "address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        print("retired %s — its cap slot is free immediately." % d.get("address", "?"))
        print("(the mailbox data is kept for the retention window, then purged)")

elif verb == "status":
    # fresh=1: Stalwart's ACME task + registry reads now, not the 60 s
    # cache Settings polls through.
    params = {"fresh": "1"}
    if env("M_HEALTH") == "true":
        params["health"] = "1"
    # NOT call(): the envelope `ok` means "read succeeded" and the systemd
    # verdict rides `consistent`. A fb449bc1-era daemon put the verdict in
    # `ok`, so a body that carries `state` is a real status even when ok is
    # false — show it instead of dying on exactly the case worth reporting.
    d = request("GET", "/cli/mail/status", params=params)
    if d is None:
        die({"code": "daemon_unreachable", "hint": "no response from the K2 daemon (is it running?)"})
    if d.get("error"):
        die(d["error"])
    if not d.get("ok") and "state" not in d:
        die({"code": "error", "hint": "unexpected daemon response: %s" % dumps(d)[:200]})
    if "consistent" not in d:
        d["consistent"] = bool(d.get("ok"))
        d["ok"] = True
    if as_json:
        print(dumps(d))
    elif not d.get("supported") or d.get("state") == "not-installed":
        # 0.46.1: one clear line instead of empty cert/owner/calendar rows.
        if not d.get("supported"):
            print("server   : not available here — hosted mail runs on Linux K2 servers."
                  " For linked inboxes use `k2 mail inboxes`.")
        else:
            print("server   : not installed — hosted mail runs on Linux K2 servers"
                  " (k2 hostmail enable --hostname <mail host>). For linked inboxes use `k2 mail inboxes`.")
            if d.get("helper"):
                print("helper   : %s" % d["helper"])
                if d.get("helperFix"):
                    print("fix      : %s" % d["helperFix"])
    else:
        print("server   : %s  (email supported on this daemon: %s)"
              % (d.get("state", "?"), "yes" if d.get("supported") else "no"))
        # AGPL source pointer (Email Notifications E23; NOTICE.md).
        sv = d.get("version") or d.get("pinnedVersion") or "?"
        print("license  : Stalwart %s (AGPL-3.0-only), unmodified; source:"
              " https://github.com/Alakazam-211/stalwart/tree/v%s" % (sv, sv))
        if d.get("consistent") is False:
            print("WARNING  : status and systemd disagree — %s"
                  % (d.get("lastError") or "no detail from the daemon"))
            print("           ground truth: systemctl is-active stalwart (read-only;"
                  " do not start/stop it by hand)")
        print("version  : %s  (pinned %s)"
              % (d.get("version") or "(not installed)", d.get("pinnedVersion", "?")))
        if d.get("upgradeAvailable"):
            # Calendars S1 (CAL15): explicit only, never automatic.
            print("upgrade  : available — k2 hostmail upgrade --dry-run, then k2 hostmail upgrade")
        up = d.get("upgrade")
        if isinstance(up, dict) and up.get("state"):
            line = "last upgrade: %s" % up["state"]
            if up.get("step"):
                line += " (step %s)" % up["step"]
            if up.get("error"):
                line += " — %s" % up["error"]
            print(line)
            if up.get("state") in ("rollback_failed", "interrupted"):
                print("           manual steps: k2 hostmail status --json (upgrade.manualSteps),"
                      " then k2 hostmail upgrade --acknowledge-failed")
        if d.get("hostname"):
            print("hostname : %s" % d["hostname"])
        if d.get("portPlan"):
            print("plan     : %s" % d["portPlan"])
        prog = d.get("enableProgress")
        if isinstance(prog, dict) and prog.get("current"):
            print("install  : %s" % prog["current"])
        if d.get("lastError"):
            print("last err : %s" % d["lastError"])
        if d.get("helper"):
            # Root door for enable / cert restart: installed | missing |
            # not allowed by sudoers. helperFix is the one root command.
            print("helper   : %s" % d["helper"])
            if d.get("helperFix"):
                print("fix      : %s" % d["helperFix"])
        cert = d.get("cert")
        if isinstance(cert, dict):
            names = cert.get("names") or []
            host = cert.get("host") or (names[0] if names else "")
            state = cert.get("state") or "?"
            extra = " self-signed" if cert.get("selfSigned") else ""
            exp = (" expires %s" % cert["expiresAt"]) if cert.get("expiresAt") else ""
            line = "cert     : %s%s%s%s" % (state, (" " + host) if host else "", extra, exp)
            acme = cert.get("acme") if isinstance(cert.get("acme"), dict) else {}
            task = acme.get("lastTask") if isinstance(acme.get("lastTask"), dict) else None
            if task and task.get("state") in ("Retry", "Failed") and state != "issued":
                line += " — ACME %s %s: %s" % ("failed" if task["state"] == "Failed" else "retrying",
                                               task.get("at") or "?", task.get("reason") or "?")
            if acme.get("orderNames") is not None and acme.get("mode") == "Automatic":
                order = acme.get("orderNames") or []
                line += " (orders: %s%s)" % (", ".join(order) or "?",
                                             " only" if len(order) == 1 else "")
            print(line)
            # 0.46.1: the live probe reads 443, 465 and 993; show them when
            # they disagree (443 can be fronted by something else).
            ports = [p for p in (cert.get("ports") or []) if isinstance(p, dict)]
            if len({p.get("state") for p in ports}) > 1:
                print("           ports: %s" % " · ".join(
                    "%s %s" % (p.get("port"), p.get("state")) for p in ports))
            if len(names) > 1 or (names and host and names[0] != host):
                print("           serves: %s" % ", ".join(names))
            if cert.get("note"):
                print("           %s" % cert["note"])
            if cert.get("owner"):
                # Who owns the mail certificate (k2 | stalwart-acme | unknown).
                why = {"k2": "K2 issues, installs and renews it; Stalwart ACME is Manual",
                       "stalwart-acme": "Stalwart's own ACME; K2 locks its names to the mail host",
                       "unknown": "attached, but no K2-planted certificate yet (k2 hostmail cert renew)"}
                print("owner    : %s — %s" % (cert["owner"], why.get(cert["owner"], "?")))
        ob = d.get("outbound")
        if isinstance(ob, dict) and (ob.get("ipStrategy") or ob.get("daneSummary")):
            # 0.45.1 A1: outbound : v4Only · DANE optional · DNSSEC ok
            print("outbound : %s · DANE %s · DNSSEC %s" % (
                ob.get("ipStrategy") or "?", ob.get("daneSummary") or "?",
                ob.get("dnssec") or "not checked (k2 hostmail doctor)"))
            if ob.get("pendingReload"):
                print("WARNING  : outbound settings saved but not live — %s"
                      % (ob["pendingReload"].get("error") or "?"))
        ren = d.get("certRenewal")
        mh = ren.get("mailHost") if isinstance(ren, dict) else None
        if isinstance(mh, dict) and mh.get("k2Issued"):
            # K2-issued mail cert: the daemon renews it 30 days before expiry.
            line = "renewal  : %s" % (mh.get("state") or "?")
            if mh.get("failures"):
                line += " — last attempt failed (%s): %s; next %s" % (
                    mh.get("failures"), mh.get("lastError") or "?", mh.get("nextAttempt") or "?")
            print(line)
        extra_names = d.get("extraNames")
        if isinstance(extra_names, list) and extra_names:
            # CAL44: last DNS answer + per-name cert (k2 hostmail cert names).
            here = [e for e in extra_names if e.get("pointsHere") is True]
            bad = [e.get("name") or "?" for e in here
                   if (e.get("cert") or {}).get("state") != "issued"]
            print("names    : %d of %d extra names point here%s" % (
                len(here), len(extra_names),
                ("; no own cert: " + ", ".join(bad)) if bad else ""))
        cal = d.get("calendar")
        if isinstance(cal, dict):
            # Calendars S2: owner DAV policy + whether clients can reach it.
            print("calendar : %s, files %s, %s%s" % (
                "on" if cal.get("enabled") else "off",
                "on" if cal.get("files") else "off",
                "reachable" if cal.get("reachable") else "not reachable",
                (" " + cal["url"]) if cal.get("url") else ""))
            if cal.get("reason"):
                print("           %s" % cal["reason"])
        if d.get("health") is not None:
            print("health   : %s" % dumps(d["health"]))
    # C17: print state + lastError, then exit ≠ 0 when systemd disagrees
    # or the server is not running. Envelope `ok` stays read-succeeded.
    if d.get("consistent") is False or d.get("state") != "running":
        sys.exit(1)

elif verb == "domain_add":
    d = call("POST", "/cli/mail/domain/add", body={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        print_domain(d)
        # The zone is on K2: the exact k2 dns lines (nothing is planted
        # for you; MX and SPF change live mail routing).
        cmds = d.get("dnsCommands") or []
        if cmds:
            print("")
            print("This domain's zone is on K2. To publish the records:")
            for c in cmds:
                print("  " + c)

elif verb == "domain_list":
    # Owner table — deliberately NO `project` param (route contract).
    d = call("GET", "/cli/mail/domain/list")
    rows = d.get("domains") or []
    if as_json:
        print(dumps(d))
    elif not rows:
        print("No domains yet — add one: k2 hostmail domain add <domain>")
    else:
        w = max(max(len(r.get("domain", "")) for r in rows), len("DOMAIN")) + 2
        print(("%-" + str(w) + "s%-11s%-14s%s") % ("DOMAIN", "STATUS", "SEND-MODE", "RECORDS"))
        for r in rows:
            recs = r.get("records") or {}
            parts = []
            for k in ("valid", "missing", "wrong", "pending", "unknown"):
                if recs.get(k):
                    parts.append("%s %s" % (recs[k], k))
            summary = " · ".join(parts) or "-"
            if r.get("dmarcNag"):
                summary += "  (DMARC recommended)"
            print(("%-" + str(w) + "s%-11s%-14s%s") % (r.get("domain", ""),
                  r.get("status", "?"), r.get("sendMode", "?"), summary))
        print()
        print("details: k2 hostmail domain show <domain>")

elif verb == "domain_show":
    d = call("GET", "/cli/mail/domain/show", params={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        print_domain(d)

elif verb == "domain_check":
    d = call("POST", "/cli/mail/domain/check", body={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        print_domain(d)

elif verb == "domain_remove":
    body = {"domain": env("M_DOMAIN"), "confirm": env("M_CONFIRM") == "true",
            "purge": env("M_PURGE") == "true"}
    d = request("POST", "/cli/mail/domain/remove", body=body)
    if d is None:
        die({"code": "daemon_unreachable", "hint": "no response from the K2 daemon (is it running?)"})
    err = d.get("error")
    if err:
        if isinstance(err, dict) and err.get("code") == "confirm_required":
            # Translate the route-body spelling into this CLI's flag.
            err = dict(err, hint=str(err.get("hint", "")).replace(
                "repeat with confirm: true", "re-run with --yes"))
        die(err)
    if not d.get("ok"):
        die({"code": "error", "hint": "unexpected daemon response: %s" % dumps(d)[:200]})
    if as_json:
        print(dumps(d))
    else:
        print("Removed %s (retired %s address(es), mail-server domain deleted: %s)."
              % (d.get("domain", "?"), d.get("retiredAddresses", 0),
                 "yes" if d.get("stalwartDeleted") else "no"))
        if d.get("purge"):
            print("mail data purge requested.")

elif verb == "doctor":
    # POST runs probes now. GET is the last stored run and will lie
    # after PTR/DNS already moved (it-email 2026-09-19).
    body = {}
    if env("M_DOMAIN"):
        body["domain"] = env("M_DOMAIN")
    d = call("POST", "/cli/mail/doctor", body=body)
    print(dumps(d))

elif verb == "config_get":
    d = call("GET", "/cli/mail/config")
    print(dumps(d))

elif verb == "config_forward_cap":
    d = call("POST", "/cli/mail/config/forward-new-per-day", body={"perDay": int(env("M_PER_DAY"))})
    if as_json:
        print(dumps(d))
    else:
        print("this server may now add %s new outside forward addresses a day (default %s, most %s)" % (
            d.get("perDay"), d.get("default"), d.get("max")))

elif verb in ("config_forward_copies_day", "config_forward_copies_hour"):
    daily = verb == "config_forward_copies_day"
    field = "perDay" if daily else "perHour"
    d = call("POST", "/cli/mail/config/forward-copies-per-" + ("day" if daily else "hour"), body={field: int(env("M_PER_DAY"))})
    if as_json:
        print(dumps(d))
    else:
        print("each mailbox on this server may now forward %s copies %s (default %s, most %s)" % (
            d.get(field), "in 24 hours" if daily else "an hour", d.get("default"), d.get("max")))
        if d.get("note"):
            print("note: %s" % d["note"])

elif verb == "quota_get":
    d = call("GET", "/cli/mail/quota", params={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        q = d.get("quotas") or {}
        print("%s  disk=%s  messages=%s" % (
            d.get("address") or env("M_ADDRESS") or "?",
            q.get("maxDiskQuota") if q.get("maxDiskQuota") is not None else "?",
            q.get("maxEmails") if q.get("maxEmails") is not None else "?"))

elif verb == "quota_set":
    body = {"address": env("M_ADDRESS")}
    if env("M_QUOTA_BYTES") != "":
        body["bytes"] = int(env("M_QUOTA_BYTES"))
    if env("M_QUOTA_GB") != "":
        body["gb"] = int(env("M_QUOTA_GB"))
    if env("M_QUOTA_MESSAGES") != "":
        body["messages"] = int(env("M_QUOTA_MESSAGES"))
    d = call("POST", "/cli/mail/quota", body=body)
    if as_json:
        print(dumps(d))
    else:
        q = d.get("quotas") or {}
        print("quota set for %s  disk=%s  messages=%s" % (
            d.get("address") or env("M_ADDRESS") or "?",
            q.get("maxDiskQuota") if q.get("maxDiskQuota") is not None else "?",
            q.get("maxEmails") if q.get("maxEmails") is not None else "?"))

elif verb == "catchall_get":
    d = call("GET", "/cli/mail/catchall", params={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        addr = d.get("catchAllAddress")
        print("%s  catch-all=%s" % (d.get("domain") or env("M_DOMAIN") or "?",
                                    addr if addr else "none"))

elif verb == "catchall_set":
    body = {"domain": env("M_DOMAIN"), "address": env("M_ADDRESS")}
    d = call("POST", "/cli/mail/catchall", body=body)
    if as_json:
        print(dumps(d))
    else:
        addr = d.get("catchAllAddress")
        print("%s  catch-all=%s" % (d.get("domain") or env("M_DOMAIN") or "?",
                                    addr if addr else "none"))

elif verb == "alias_list":
    d = call("GET", "/cli/mail/alias", params={"mailbox": env("M_MAILBOX") or env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("aliases") or []
        if not rows:
            print("%s  aliases=none" % (d.get("mailbox") or env("M_MAILBOX") or "?"))
        else:
            print("%s  aliases=%s" % (d.get("mailbox") or "?", ", ".join(rows)))

elif verb == "alias_add":
    body = {"mailbox": env("M_MAILBOX") or env("M_ADDRESS"), "alias": env("M_ALIAS")}
    d = call("POST", "/cli/mail/alias", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("alias %s -> %s" % (d.get("alias") or env("M_ALIAS"), d.get("mailbox") or "?"))

elif verb == "alias_remove":
    body = {"mailbox": env("M_MAILBOX") or env("M_ADDRESS"), "alias": env("M_ALIAS")}
    d = call("POST", "/cli/mail/alias/remove", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("removed alias %s from %s" % (d.get("removed") or env("M_ALIAS"), d.get("mailbox") or "?"))

elif verb.startswith("forward_") or verb.startswith("self_forward_"):
    # Hosted mail forwards (prd-hostmail-external-forwarding-v1 §4.1, 0.46.7).
    # forward_*      = /cli/mail/forward*      (k2 hostmail forward; owner,
    #                  Admin, mail-manage agents)
    # self_forward_* = /cli/mail/self/forward* (k2 mail forward; the
    #                  mailbox's own person: a box login, or the owner)
    is_self = verb.startswith("self_")
    op = verb[len("self_forward_"):] if is_self else verb[len("forward_"):]
    base_route = "/cli/mail/self/forward" if is_self else "/cli/mail/forward"
    addr_arg = env("M_ADDRESS")
    addr_hint = addr_arg or "<address>"
    family = "mail" if is_self else "hostmail"

    def fwd_die(err, permission=None):
        # Refusals that need more than the daemon's hint to act on.
        code = (err or {}).get("code", "error")
        hint = (err or {}).get("hint", "")
        if code == "person_only":
            die({"code": "person_only", "hint":
                 "k2 mail forward is for a person's own mailbox (a box login, or the owner). "
                 "An agent uses k2 hostmail forward <set|add|rm|show|unset|pause|resume> <address> "
                 "(needs \"Allow agents to manage hosted mail on this host\" on)"}, 3)
        if code in ("destination_stopped", "too_many_destinations", "new_outside_limit", "notice_limit",
                    "suppressed", "no_front_door", "no_mail_path", "forward_loop"):
            # The daemon's hint says what to do. Exit 1, no "ask your human" tail.
            die({"code": code, "hint": hint}, 1)
        if code == "owner_only" and op == "resume":
            # Who resumes an automatic pause depends on the reason, and the
            # hint names who. Print it as is (no "ask your human" tail).
            sys.stderr.write(dumps({"error": {"code": code, "hint": hint}}) + "\n")
            sys.exit(3)
        die(err, permission=permission)

    def fwd_call(method, route, params=None, body=None, timeout=30):
        d = request(method, route, params, body, timeout)
        if d is None:
            die({"code": "daemon_unreachable", "hint": "no response from the K2 daemon (is it running?)"})
        if d.get("error"):
            fwd_die(d["error"], d.get("permission"))
        if not d.get("ok"):
            die({"code": "error", "hint": "unexpected daemon response: %s" % dumps(d)[:200]})
        return d

    def when(ts):
        try:
            return time.strftime("%Y-%m-%d %H:%M", time.localtime(int(ts)))
        except Exception:
            return "?"

    def fwd_line(f):
        # One forward, one line: to, keep, kind, state (+ the pause reason).
        s = "to=%s  keep=%s  kind=%s  state=%s" % (
            f.get("to") or "?", "true" if f.get("keep") else "false",
            f.get("kind") or "?", f.get("state") or "?")
        if f.get("state") == "paused" and f.get("pauseReason"):
            s += "  reason=%s" % f["pauseReason"]
        return s

    def fwd_detail(f):
        out = []
        if f.get("state") == "paused" and f.get("pauseReason") == "upgrade_required":
            out.append("  not forwarding: the mail server needs upgrading: k2 hostmail upgrade")
            out.append("  it resumes by itself after the upgrade")
        elif f.get("state") == "paused" and f.get("pauseWords"):
            out.append("  paused because %s" % f["pauseWords"])
            reason = f.get("pauseReason") or ""
            if f.get("heldBy"):
                out.append("  someone also paused it, so it stays paused when K2 lets go; resume takes that back")
            if reason == "not_told":
                out.append("  it goes back on by itself when K2 can email the address its stop link; or remove it")
            elif reason == "agent_send_off":
                out.append("  it goes back on when agent sending does; or the owner, an Admin or the mailbox's person resumes it, and it's theirs")
            elif reason == "revoked":
                out.append("  it can't be resumed; only the owner or an Admin can choose it again (--restart-stopped), and should ask that address first")
            elif reason in ("air_gap", "receive_only"):
                out.append("  it resumes by itself when that changes")
            elif reason == "box_brake":
                out.append("  the owner or an Admin resumes it (k2 hostmail forward resume)")
            elif reason != "manual":
                out.append("  the owner, an Admin or the mailbox's person resumes it")
        if f.get("cantArrive"):
            out.append("  %s" % f["cantArrive"])
        if f.get("noticeWords"):
            out.append("  %s" % f["noticeWords"])
        elif f.get("notice") == "sent" and f.get("noticeAt"):
            out.append("  told %s: emailed it a stop link %s" % (f.get("to") or "the address", when(f["noticeAt"])))
        c = f.get("last24h") or {}
        if f.get("kind") == "external" or any(c.get(k) for k in ("forwarded", "delivered", "refused")):
            out.append("  last 24h: forwarded %s, delivered %s, refused %s" % (
                c.get("forwarded", 0), c.get("delivered", 0), c.get("refused", 0)))
        if f.get("setBy"):
            out.append("  set by %s" % f["setBy"])
        return out

    def note_line(d):
        if d.get("note"):
            print("note: %s" % d["note"])

    if op == "get":
        params = {"address": addr_arg} if addr_arg else {}
        d = fwd_call("GET", base_route, params=params)
        if as_json:
            print(dumps(d))
        else:
            a = d.get("address") or addr_arg or "?"
            rows = d.get("forwards")
            if rows is None:
                rows = [d["forward"]] if d.get("forward") else []
            if not rows:
                print("%s  forward=none" % a)
            else:
                for f in rows:
                    print("%s  %s" % (a, fwd_line(f)))
                    for line in fwd_detail(f):
                        print(line)
                vol = d.get("volume") or {}
                if vol:
                    print("volume: %s of %s copies counted in 24 hours (at the limit K2 pauses its outside forwards), %s an hour (above that the mail server holds them)" % (
                        vol.get("copiesCounted", 0), vol.get("copiesPerDay"), vol.get("copiesPerHour")))
                evs = d.get("events") or []
                if evs:
                    print("  recent events (newest first):")
                    for e in evs[:5]:
                        det = e.get("detail") or {}
                        extra = ""
                        if isinstance(det, dict) and det.get("reason"):
                            extra = "  (%s)" % det["reason"]
                        print("    %s  %-12s %s%s" % (when(e.get("at")), e.get("event") or "?", e.get("actor") or "?", extra))

    elif op in ("set", "add"):
        tos = [t for t in env("M_TO").split("\n") if t]
        body = {"to": tos[0] if len(tos) == 1 else tos}
        if addr_arg:
            body["address"] = addr_arg
        # keep rides ONLY when --keep / --no-keep was given: the daemon
        # defaults outside forwards to keeping a copy, same-box ones to not.
        if env("M_KEEP_ARG") in ("true", "false"):
            body["keep"] = env("M_KEEP_ARG") == "true"
        if env("M_RESTART") == "true":
            body["restartStopped"] = True
        route = base_route if op == "set" else base_route + "/add"
        # K2 emails each new outside address before it answers, so a long list
        # takes longer than the usual wait.
        d = fwd_call("POST", route, body=body, timeout=30 + 2 * len(tos))
        if as_json:
            print(dumps(d))
        else:
            a = d.get("address") or addr_arg or "?"
            for one in d.get("destinations") or [d]:
                dest = one.get("to") or "?"
                keep = "keeps a copy" if one.get("keep") else "no copy kept"
                print("forward %s -> %s (%s): %s" % (a, dest, keep, one.get("state") or "active"))
                if one.get("message"):
                    print(one["message"])
                if one.get("note") and one.get("note") != d.get("note"):
                    print("note: %s" % one["note"])
            note_line(d)

    elif op == "rm":
        tos = [t for t in env("M_TO").split("\n") if t]
        body = {"to": tos[0] if len(tos) == 1 else tos}
        if addr_arg:
            body["address"] = addr_arg
        d = fwd_call("POST", base_route + "/rm", body=body)
        if as_json:
            print(dumps(d))
        else:
            a = d.get("address") or addr_arg or "?"
            print("%s no longer forwards to %s" % (a, ", ".join(d.get("removed") or tos)))
            left = d.get("remaining") or []
            print("still forwarding to: %s" % (", ".join(left) if left else "nothing"))
            note_line(d)

    elif op == "unset":
        body = {"address": addr_arg} if addr_arg else {}
        d = fwd_call("POST", base_route + "/unset", body=body)
        if as_json:
            print(dumps(d))
        else:
            a = d.get("address") or addr_arg or "?"
            if d.get("removed") is False:
                print("%s had no forward (nothing to remove)" % a)
            else:
                print("forward removed for %s" % a)
            note_line(d)

    elif op in ("pause", "resume"):
        body = {"address": addr_arg} if addr_arg else {}
        if env("M_TO"):
            body["to"] = env("M_TO")
        d = fwd_call("POST", base_route + "/" + op, body=body)
        if as_json:
            print(dumps(d))
        else:
            a = d.get("address") or addr_arg or "?"
            rows = d.get("forwards") or []
            which = " (%s)" % ", ".join(r.get("to") or "?" for r in rows) if rows else ""
            if op == "pause":
                print("forward paused for %s%s: new mail stays in the mailbox" % (a, which))
            else:
                print("forward resumed for %s%s (state %s)" % (a, which, d.get("state") or "?"))
            for sk in d.get("skipped") or []:
                print("left alone: %s: %s" % (sk.get("to") or "?", sk.get("why") or "?"))
            note_line(d)

    elif op == "list":
        params = {}
        if env("M_STATE"):
            params["state"] = env("M_STATE")
        if env("M_KIND"):
            params["kind"] = env("M_KIND")
        d = fwd_call("GET", "/cli/mail/forwards", params=params)
        if as_json:
            print(dumps(d))
        else:
            rows = d.get("forwards") or []
            oc = d.get("outcomes") or {}
            if oc.get("ok") is False:
                print("warning: delivery reports aren't reaching K2 (%s), so forwards won't pause by themselves; K2 retries hourly" % (oc.get("error") or "unknown"))
            if not rows:
                print("no forwards")
            for f in rows:
                print("%s  %s" % (f.get("address") or "?", fwd_line(f)))
                if f.get("state") == "paused" and f.get("pauseWords"):
                    print("  paused because %s" % f["pauseWords"])
                if f.get("cantArrive"):
                    print("  %s" % f["cantArrive"])
                if f.get("noticeWords"):
                    print("  %s" % f["noticeWords"])
    else:
        die({"code": "usage", "hint": "unknown forward verb %s" % op})

elif verb == "person_link":
    who = env("M_PERSON")
    d = call("POST", "/cli/mail/address/person-link",
             body={"address": env("M_ADDRESS"), "person": None if who == "" else who})
    if as_json:
        print(dumps(d))
    elif d.get("person"):
        print("%s is now linked to %s, who can manage its forwarding (k2 mail forward, or Settings)"
              % (d.get("address") or env("M_ADDRESS") or "?", d["person"]))
    else:
        print("%s is no longer linked to a person" % (d.get("address") or env("M_ADDRESS") or "?"))

elif verb == "config_set":
    body = {}
    for envk, key in (("M_SEND_MODE", "sendMode"), ("M_DOMAIN", "domain"),
                      ("M_RELAY_HOST", "relayHost"), ("M_RELAY_PORT", "relayPort"),
                      ("M_RELAY_USER", "relayUser"), ("M_RELAY_PASS", "relayPass"),
                      ("M_AGENT_SEND", "agentSend"), ("M_ADDRESS_CAP", "addressCap"),
                      ("M_QUOTA_BYTES", "quotaBytes"), ("M_QUOTA_MESSAGES", "quotaMessages"),
                      ("M_WORKSPACE", "workspace")):
        v = env(envk)
        if v != "":
            body[key] = int(v) if key in ("relayPort", "addressCap", "quotaBytes", "quotaMessages") else v
    if env("M_QUOTA_GB") != "":
        body["quotaBytes"] = int(env("M_QUOTA_GB")) * (1 << 30)
    if env("M_ALWAYS_BCC_SET") == "true":
        # The owner's always-BCC policy; [] clears it (owner/admin only).
        body["alwaysBcc"] = env_list("M_ALWAYS_BCC")
    quota_keys = ("quotaBytes", "quotaMessages")
    if any(k in body for k in quota_keys) and "workspace" not in body:
        defaults = body.setdefault("defaults", {})
        for k in quota_keys:
            if k in body:
                defaults[k] = body.pop(k)
    d = call("POST", "/cli/mail/config/set", body=body)
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or "mail config updated.")

elif verb in ("sending_get", "sending_set"):
    # `k2 mail sending <ws> [off|approval|on]` — the per-workspace
    # agent-send mode. Same daemon route + owner gate as
    # `k2 hostmail config --workspace <ws> --agent-send`, but works on
    # every OS (it gates linked/BYO sends, which run on macOS too).
    ws = env("M_WORKSPACE")
    if verb == "sending_set":
        call("POST", "/cli/mail/config/set",
             body={"workspace": ws, "agentSend": env("M_AGENT_SEND")})
    d = call("GET", "/cli/mail/config", params={"workspace": ws})
    w = d.get("workspace") or {}
    a = w.get("agentSend") or {}
    if as_json:
        print(dumps({"ok": True, "workspace": ws, "project": w.get("project"),
                     "agentSend": a.get("effective"), "override": a.get("override"),
                     "default": a.get("default"), "changed": verb == "sending_set"}))
    else:
        eff = a.get("effective") or "?"
        src = "set for this workspace" if a.get("override") is not None \
            else "server default"
        lead = "agent email sending for %s is now %s" if verb == "sending_set" \
            else "agent email sending for %s: %s"
        print((lead % (ws, eff)) + " (%s)" % src)
        if eff == "approval":
            print("note: approval queues hosted K2 mailboxes only; a linked inbox "
                  "(Gmail, Outlook, IMAP) needs 'on' to send")
        elif eff == "off":
            print("agents in this workspace can't send email. Turn it on with: "
                  "k2 mail sending %s on" % ws)

elif verb == "approvals_list":
    d = call("GET", "/cli/mail/approvals/list")
    rows = d.get("pending") or []
    if as_json:
        print(dumps(d))
    elif not rows:
        print("No sends waiting for approval.")
    else:
        print("%s send(s) waiting for approval:" % d.get("count", len(rows)))
        print()
        for r in rows:
            who = r.get("workspace") or r.get("agentName") or "?"
            exp = ""
            if r.get("expiresAt"):
                u = until(r["expiresAt"])
                exp = ", auto-denies soon" if u == "expired" else ", expires %s" % u
            print("%s  from %s%s (%s) — queued %s%s" % (r.get("id", ""),
                  r.get("from") or "?", shown_as(r), who, unix_ago(r.get("createdAt")), exp))
            print("  to      : %s" % (", ".join(r.get("to") or []) or "?"))
            if r.get("cc"):
                print("  cc      : %s" % ", ".join(r["cc"]))
            if r.get("bcc"):
                print("  bcc     : %s" % ", ".join(r["bcc"]))
            if r.get("policyBcc"):
                print("  bcc (owner policy): %s" % ", ".join(r["policyBcc"]))
            if r.get("replyTo"):
                print("  reply-to: %s (answers go here, not to the sender)" % ", ".join(r["replyTo"]))
            if r.get("forwardOf"):
                print("  forwards: %s" % forward_line(r["forwardOf"]))
            print("  subject : %s" % term_line(r.get("subject") or "(no subject)"))
            if r.get("sourceDmarc"):
                print("  original: DMARC %s%s" % (term_line(r["sourceDmarc"]), dmarc_note(r["sourceDmarc"])))
            if r.get("sourceCheck"):
                print("  original: %s. Its sender may be forged." % term_line(r["sourceCheck"]))
            for f in r.get("files") or []:
                print("  file %-3s: %s" % (f.get("n", "?"), file_line(f)))
            if r.get("bodyPreview") is not None:
                print("  preview : %s" % term_line(r["bodyPreview"]))
            print("  open    : k2 hostmail approvals show %s" % r.get("id", ""))
            print("  decide  : k2 hostmail approvals approve %s | deny %s --note \"...\""
                  % (r.get("id", ""), r.get("id", "")))
            print()

elif verb == "approvals_show":
    d = call("GET", "/cli/mail/approvals/show", params={"id": env("M_ID")})
    if as_json:
        print(dumps(d))
    else:
        m = d.get("message") or {}
        who = m.get("workspace") or m.get("agentName") or "?"
        print("%s  from %s%s (%s), waiting for approval" % (m.get("id", ""), m.get("from") or "?", shown_as(m), who))
        print("to      : %s" % (", ".join(m.get("to") or []) or "?"))
        if m.get("cc"):
            print("cc      : %s" % ", ".join(m["cc"]))
        if m.get("bcc"):
            print("bcc     : %s" % ", ".join(m["bcc"]))
        if m.get("policyBcc"):
            print("bcc (owner policy): %s" % ", ".join(m["policyBcc"]))
        if m.get("replyTo"):
            print("reply-to: %s (answers go here, not to the sender)" % ", ".join(m["replyTo"]))
        if m.get("forwardOf"):
            print("forwards: %s" % forward_line(m["forwardOf"]))
        print("subject : %s" % term_line(m.get("subject") or "(no subject)"))
        if m.get("sourceDmarc"):
            print("original: DMARC %s%s" % (term_line(m["sourceDmarc"]), dmarc_note(m["sourceDmarc"])))
        if m.get("sourceCheck"):
            print("original: %s. Its sender may be forged." % term_line(m["sourceCheck"]))
        for f in m.get("files") or []:
            print("file %-3s: %s" % (f.get("n", "?"), file_line(f)))
        if m.get("files"):
            print("          save one to look at it: k2 hostmail approvals save %s --file <n> --out <path>" % m.get("id", ""))
        print()
        print(term_safe(m.get("text") or ""))

elif verb == "approvals_save":
    out = env("M_OUT")
    # Never over a file: the name and the bytes came from an agent or from
    # outside mail. O_EXCL + O_NOFOLLOW below hold this against a race.
    if os.path.lexists(out):
        die({"code": "usage", "hint": "--out '%s' already exists — pick a new path" % out})
    d = call("GET", "/cli/mail/approvals/show",
             params={"id": env("M_ID"), "file": env("M_FILE")}, timeout=120)
    data = base64.b64decode(d.get("dataBase64") or "")
    try:
        fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    except OSError as e:
        die({"code": "usage", "hint": "can't write --out '%s': %s" % (out, e)})
    with os.fdopen(fd, "wb") as f:
        f.write(data)
    if as_json:
        print(dumps({"ok": True, "id": d.get("id"), "n": d.get("n"), "name": d.get("name"),
                     "size": len(data), "path": out}))
    else:
        print("saved %s (%s) to %s. It isn't opened: it would leave exactly as it is, and it "
              "may have come from outside, so treat it as untrusted."
              % (term_safe(d.get("name") or "?"), human_size(len(data)), out))

elif verb == "approvals_approve":
    body = {"id": env("M_ID")}
    if env("M_NOTE"):
        body["note"] = env("M_NOTE")
    d = call("POST", "/cli/mail/approvals/approve", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("%s %s" % (d.get("id", ""), d.get("hint") or d.get("status", "")))

elif verb == "approvals_deny":
    d = call("POST", "/cli/mail/approvals/deny",
             body={"id": env("M_ID"), "note": env("M_NOTE")})
    if as_json:
        print(dumps(d))
    else:
        print("%s rejected — your note reaches the agent in its outbox." % d.get("id", ""))

elif verb == "draft":
    # Reply DRAFT (id) or compose DRAFT (--to/--subject). APPEND \Draft;
    # nothing is ever sent. Gate is can_draft, not mailAgentSend.
    body = {"project": ws_path, "body": env("M_BODY")}
    if env("M_ID"):
        body["id"] = env("M_ID")
    to = env_list("M_TO")
    if to:
        body["to"] = to
    if env("M_SUBJECT"):
        body["subject"] = env("M_SUBJECT")
    cc = env_list("M_CC")
    if cc:
        body["cc"] = cc
    bcc = env_list("M_BCC")
    if bcc:
        body["bcc"] = bcc
    if env("M_FROM"):
        body["from"] = env("M_FROM")
    attach = env_lines("M_ATTACH")
    if attach:
        body["attachments"] = attach
    d = call("POST", "/cli/mail/draft", body=body)
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or "draft saved to '%s' in %s"
              % (d.get("folder", "?"), d.get("address", "?")))

elif verb == "external_add":
    # S9 owner verb: connect an external account and bind it to ONE
    # workspace. The credential rides the request body once and is
    # vaulted daemon-side — it never appears in any response or log.
    body = {"project": env("M_WORKSPACE"), "address": env("M_ADDRESS"),
            "host": env("M_HOST")}
    for envk, key in (("M_PORT", "port"), ("M_TLS", "tls"), ("M_USER", "username"),
                      ("M_NAME", "displayName"), ("M_DRAFTS_FOLDER", "draftsFolder"),
                      ("M_SMTP_HOST", "smtpHost"), ("M_SMTP_PORT", "smtpPort"),
                      ("M_SMTP_TLS", "smtpTls")):
        v = env(envk)
        if v != "":
            body[key] = int(v) if key in ("port", "smtpPort") else v
    if env("M_PASS"):
        body["password"] = env("M_PASS")
    elif env("M_PASS_REF"):
        body["passwordRef"] = env("M_PASS_REF")
    # The live connect check can take a moment against a slow host.
    d = call("POST", "/cli/mail/external/add", body=body, timeout=90)
    if as_json:
        print(dumps(d))
    else:
        print("connected %s -> workspace %s" % (d.get("address", "?"), env("M_WORKSPACE")))
        if d.get("draftsFolder"):
            print("drafts folder: %s" % d["draftsFolder"])
        else:
            print("no Drafts folder detected — set one with --drafts-folder if 'k2 mail draft' errors")
        print("agents in that workspace can now READ this inbox and save drafts")
        print("(k2 mail draft --to/--subject compose, or draft <id> reply);")
        print("sending FROM it needs level 'send' AND workspace Sending=on (k2 mail send/reply).")

elif verb == "link_calendars":
    # MCS3: the owner's per-server switch for calendars on LINKED inboxes.
    want = env("M_LINK_CAL")
    if want:
        d = call("POST", "/cli/mail/link/calendars", body={"enabled": want == "on"})
    else:
        d = call("GET", "/cli/mail/link/calendars")
    if as_json:
        print(dumps(d))
    else:
        state = d.get("linkedCalendars") or ("on" if d.get("enabled") else "off")
        print("Calendars on linked inboxes: %s%s" % (
            state, "" if not want else (" (changed)" if d.get("changed") else " (unchanged)")))
        if state == "on":
            print("Agents with a grant can use a linked Microsoft 365 or Google account's calendar (k2 calendar).")
        else:
            print("Every linked calendar verb answers capability_missing (owner_disabled). Turn on: k2 mail link calendars on")

elif verb in ("link_oauth_add", "link_oauth_upgrade"):
    # O4 owner verb: begin an OAuth link (Gmail loopback / Microsoft
    # device flow) and LONG-POLL to completion. The daemon runs the whole
    # flow server-side; codes/tokens NEVER reach this CLI — we only see
    # the user-facing code+URL (device) or a "browser opened" ack
    # (loopback), then poll a state. MCS1: `upgrade` re-consents an
    # existing row in place for more features (same flow, `inbox` set).
    provider = env("M_PROVIDER")
    upgrade = verb == "link_oauth_upgrade"
    if upgrade:
        body = {"inbox": env("M_ADDRESS"), "features": env("M_FEATURES")}
        if provider:
            body["provider"] = provider
    else:
        body = {"address": env("M_ADDRESS"), "provider": provider,
                "workspace": env("M_WORKSPACE")}
        if env("M_FEATURES"):
            body["features"] = env("M_FEATURES")
    # start dials the provider (device) or binds a loopback + opens the
    # browser (Gmail); a remote-daemon Gmail returns a teaching error here.
    d = call("POST", "/cli/mail/link/oauth/start", body=body, timeout=45)
    flow = d.get("flow")
    link_id = d.get("linkId")
    provider = provider or ("microsoft" if flow == "device" else "gmail")
    if flow == "device":
        sys.stderr.write("To %s %s (%s): open %s and enter the code %s — sign in as %s\n"
                         % ("upgrade" if upgrade else "connect", env("M_ADDRESS"),
                            d.get("features", "mail"), d.get("verificationUrl", "?"),
                            d.get("userCode", "?"), d.get("address") or env("M_ADDRESS")))
    else:
        sys.stderr.write("A browser window opened — approve access to %s there (%s).\n"
                         % (env("M_ADDRESS"), d.get("features", "mail")))
    sys.stderr.write("Waiting for approval (Ctrl-C to stop; the link stays pending)…\n")
    # Bounded status long-poll (the wait idiom): poll every 3s up to the
    # device-code lifetime ceiling. The daemon holds the flow, not us.
    deadline = time.time() + 900
    final = None
    while time.time() < deadline:
        s = request("GET", "/cli/mail/link/oauth/status",
                    params={"linkId": link_id})
        if s is None:
            die({"code": "daemon_unreachable",
                 "hint": "no response from the K2 daemon (is it running?)"})
        if s.get("error"):
            die(s["error"])
        state = s.get("state")
        if state in ("connected", "denied", "expired", "error"):
            final = s
            break
        time.sleep(3)
    if final is None:
        die({"code": "timeout",
             "hint": "the link did not complete in time — run "
                     "'k2 mail link add --provider %s' again" % provider}, 2)
    state = final.get("state")
    if state == "connected":
        if as_json:
            print(dumps(final))
        elif upgrade:
            print("upgraded %s (%s)" % (final.get("address") or env("M_ADDRESS"), provider))
            print(final.get("hint") or "its grants, levels and ids are unchanged.")
        else:
            print("connected %s -> workspace %s (%s)"
                  % (final.get("address") or env("M_ADDRESS"),
                     env("M_WORKSPACE"), provider))
            print(final.get("hint")
                  or "agents in that workspace can now read it and save reply drafts.")
    else:
        # denied / expired / error — a clear failure, non-zero exit. The
        # code is stable (account_mismatch, admin_consent_required,
        # device_code_blocked, scope_not_granted, client_misconfigured, …).
        hint = final.get("hint") or ("the link %s" % state)
        if final.get("adminConsentUrl") and final["adminConsentUrl"] not in hint:
            hint += " Admin consent: %s" % final["adminConsentUrl"]
        die({"code": final.get("code") or ("denied" if state == "denied" else "error"),
             "hint": hint}, 1)

elif verb == "external_remove":
    d = call("POST", "/cli/mail/external/remove", body={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        print("removed %s — its stored credential was deleted from the vault." % d.get("address", "?"))
        # MCS1: Google tokens are revoked at Google; Microsoft has no
        # per-token revoke — the hint says where to remove the consent.
        if d.get("revokeHint"):
            print(d["revokeHint"])

elif verb == "capabilities":
    # MCS1: what each inbox can do. Agent view inside a workspace (own
    # grants, at your level); the owner view outside one.
    params = {"project": ws_path} if ws_path else {}
    if env("M_ADDRESS"):
        params["address"] = env("M_ADDRESS")
    d = request("GET", "/cli/mail/capabilities", params=params)
    if d is None:
        die({"code": "daemon_unreachable",
             "hint": "no response from the K2 daemon (is it running?)"})
    # Outside any workspace the cwd is not a registered workspace: ask for
    # the owner view instead (the daemon gates it to owner/admin).
    err = d.get("error") if isinstance(d.get("error"), dict) else None
    if err and "project" in params and str(err.get("hint", "")).startswith("no agent/workspace named"):
        params.pop("project")
        d = request("GET", "/cli/mail/capabilities", params=params)
        if d is None:
            die({"code": "daemon_unreachable",
                 "hint": "no response from the K2 daemon (is it running?)"})
    if isinstance(d.get("error"), dict):
        die(d["error"])
    if as_json:
        print(dumps(d))
    else:
        rows = [d] if env("M_ADDRESS") else (d.get("inboxes") or [])
        if not rows:
            print("no inboxes — 'k2 mail inboxes' lists what you can reach")
        for r in rows:
            c = r.get("capabilities") or {}
            mail = c.get("mail") or {}
            cal = c.get("calendar") or {}
            on = lambda m: ",".join(k for k in ("read", "draft", "compose", "send", "write", "invite", "rsvp") if m.get(k) is True) or "-"
            lvl = r.get("yourLevel")
            print("%s  [%s]%s" % (r.get("address", "?"), c.get("source", "?"),
                                  ("  level %s" % lvl) if lvl else ""))
            print("  mail: %s   calendar: %s   free/busy: %s"
                  % (on(mail), on(cal), cal.get("freebusy", "none")))
            acct = c.get("account")
            if acct:
                print("  account: %s%s%s"
                      % (acct.get("kind") or "unknown",
                         (" tenant %s" % acct["tenant"]) if acct.get("tenant") else "",
                         "" if acct.get("checked") else " (not checked — 'k2 mail link upgrade' checks it)"))
            for m in c.get("missing") or []:
                print("  - %s off (%s): %s" % (m.get("cap"), m.get("reason"), m.get("fix")))

elif verb in ("oauth_config_show", "oauth_config_set", "oauth_config_clear"):
    # MCS1: the owner's own OAuth app per provider + the Microsoft tenant.
    # The Gmail secret rode stdin into M_OAUTH_SECRET; it goes out once in
    # the POST body and is never printed (the daemon never returns it).
    prov = env("M_PROVIDER")
    if verb == "oauth_config_show":
        d = call("GET", "/cli/mail/oauth-config")
    elif verb == "oauth_config_set":
        body = {"provider": prov}
        if env("M_CLIENT_ID"):
            body["clientId"] = env("M_CLIENT_ID")
        if env("M_TENANT"):
            body["tenant"] = env("M_TENANT")
        if env("M_OAUTH_SECRET"):
            body["clientSecret"] = env("M_OAUTH_SECRET")
        d = call("POST", "/cli/mail/oauth-config/set", body=body)
    else:
        d = call("POST", "/cli/mail/oauth-config/clear", body={"provider": prov})
    if as_json:
        print(dumps(d))
    else:
        provs = d.get("providers") or {d.get("provider", prov): d}
        for name in ("gmail", "microsoft"):
            p = provs.get(name)
            if p is None or (prov and name != prov and verb != "oauth_config_show"):
                continue
            if prov and verb == "oauth_config_show" and name != prov:
                continue
            line = "%s: %s app %s%s" % (
                name, "your own" if p.get("source") == "custom" else "K2's built-in",
                p.get("clientId") or "?",
                "" if p.get("configured", True) else "  (NOT CONFIGURED — linking will refuse)")
            if name == "gmail":
                line += "  secret %s" % ("set" if p.get("secretSet") else "not set")
            if name == "microsoft" and p.get("tenant"):
                line += "  tenant %s" % p["tenant"]
            print(line)

# ── S11 unified inbox surface (helpers hoisted above the verb chain) ──
elif verb == "inboxes":
    # Agent view: what THIS workspace can access (unified catalog).
    d = call("GET", "/cli/mail/inboxes", params={"project": ws_path})
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("inboxes") or []
        # Outside any workspace the daemon answers the OWNER catalog (no
        # yourLevel): show the owner table, BLOCKED grants included.
        owner_catalog = bool(rows) and all(r.get("yourLevel") is None for r in rows)
        print_inboxes(rows, agent_view=not owner_catalog)

elif verb == "inboxes_all":
    # Owner view: ALL inboxes across workspaces (no project param).
    d = call("GET", "/cli/mail/inboxes")
    if as_json:
        print(dumps(d))
    else:
        print_inboxes(d.get("inboxes") or [], agent_view=False)

elif verb == "link_list":
    # The linked (external IMAP) inboxes, from the owner catalog.
    d = call("GET", "/cli/mail/inboxes")
    rows = [r for r in (d.get("inboxes") or []) if r.get("source") == "linked"]
    if as_json:
        print(dumps({"ok": True, "count": len(rows), "inboxes": rows}))
    elif not rows:
        print("No linked accounts — add one: k2 mail link add <address> --host <imap-host> --workspace <ws> --pass-stdin")
    else:
        w = max(max(len(r.get("address", "")) for r in rows), len("ADDRESS")) + 2
        print(("%-" + str(w) + "s%-11s%-22s%s") % ("ADDRESS", "STATUS", "PRIMARY", "HOST"))
        for r in rows:
            pri = r.get("primary") or {}
            print(("%-" + str(w) + "s%-11s%-22s%s") % (
                r.get("address", ""), r.get("status", "?"),
                pri.get("workspace") or pri.get("projectId") or "?",
                "%s (%s)" % (r.get("host", "?"), r.get("tls", "?"))))
            for g in (r.get("grants") or []):
                st = g.get("sendState")
                note = ""
                if st in ("blocked_off", "blocked_approval"):
                    note = ("   BLOCKED: agent sending is %s for this workspace — "
                            "k2 mail sending %s on" % (g.get("agentSend") or "off",
                                                       g.get("workspace") or g.get("projectId", "?")))
                print("    grant: %-20s %s%s" % (
                    g.get("workspace") or g.get("projectId", "?"), g.get("level", "?"), note))

elif verb == "access_grant":
    d = call("POST", "/cli/mail/access/grant",
             body={"address": env("M_ADDRESS"), "project": env("M_WORKSPACE"),
                   "level": env("M_LEVEL")})
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or "granted %s on %s to %s"
              % (env("M_LEVEL"), d.get("address", "?"), env("M_WORKSPACE")))

elif verb == "access_revoke":
    d = call("POST", "/cli/mail/access/revoke",
             body={"address": env("M_ADDRESS"), "project": env("M_WORKSPACE")})
    if as_json:
        print(dumps(d))
    else:
        print("revoked %s's access to %s." % (env("M_WORKSPACE"), d.get("address", "?")))

elif verb == "access_primary":
    d = call("POST", "/cli/mail/access/set-primary",
             body={"address": env("M_ADDRESS"), "project": env("M_WORKSPACE")})
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or "%s now manages %s"
              % (env("M_WORKSPACE"), d.get("address", "?")))

elif verb == "access_level":
    d = call("POST", "/cli/mail/access/set-level",
             body={"address": env("M_ADDRESS"), "project": env("M_WORKSPACE"),
                   "level": env("M_LEVEL")})
    if as_json:
        print(dumps(d))
    else:
        who = "primary" if d.get("isPrimary") else env("M_WORKSPACE")
        print("set %s level on %s to %s." % (who, d.get("address", "?"), env("M_LEVEL")))

# ── 0081 inbox management + delete (move / flag / archive / delete /
#    folders + the access manage caps). DELETE is always MOVE-to-Trash. ──
elif verb == "move":
    d = call("POST", "/cli/mail/move",
             body={"project": ws_path, "id": env("M_ID"), "folder": env("M_FOLDER")})
    if as_json:
        print(dumps(d))
    else:
        print("moved %s -> %s" % (d.get("id", ""), d.get("folder") or env("M_FOLDER")))

elif verb == "flag":
    body = {"project": ws_path, "id": env("M_ID")}
    if env("M_READ") != "":
        body["read"] = (env("M_READ") == "true")
    if env("M_FLAGGED") != "":
        body["flagged"] = (env("M_FLAGGED") == "true")
    d = call("POST", "/cli/mail/flag", body=body)
    if as_json:
        print(dumps(d))
    else:
        parts = []
        if "read" in body:
            parts.append("read" if body["read"] else "unread")
        if "flagged" in body:
            parts.append("flagged" if body["flagged"] else "unflagged")
        print("%s: %s" % (d.get("id", ""), ", ".join(parts) or "no change"))

elif verb == "archive":
    d = call("POST", "/cli/mail/archive", body={"project": ws_path, "id": env("M_ID")})
    if as_json:
        print(dumps(d))
    else:
        print("archived %s → %s" % (d.get("id", ""), d.get("folder") or "Archive"))

elif verb == "delete":
    # DELETE = move to Trash (never erased); needs the delete cap.
    d = call("POST", "/cli/mail/delete", body={"project": ws_path, "id": env("M_ID")})
    if as_json:
        print(dumps(d))
    else:
        print("deleted %s -> %s (moved to Trash, not erased)"
              % (d.get("id", ""), d.get("folder") or "Trash"))

elif verb == "folder_list":
    d = call("GET", "/cli/mail/folder/list",
             params={"project": ws_path, "address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        folders = d.get("folders") or []
        if not folders:
            print("No folders.")
        else:
            for f in folders:
                print(f)

elif verb == "folder_create":
    d = call("POST", "/cli/mail/folder/create",
             body={"project": ws_path, "address": env("M_ADDRESS"), "name": env("M_NAME")})
    if as_json:
        print(dumps(d))
    else:
        print("created folder '%s' in %s" % (d.get("folder") or env("M_NAME"), d.get("address", "")))

elif verb == "folder_rename":
    d = call("POST", "/cli/mail/folder/rename",
             body={"project": ws_path, "address": env("M_ADDRESS"),
                   "from": env("M_FOLDER_FROM"), "to": env("M_FOLDER_TO")})
    if as_json:
        print(dumps(d))
    else:
        print("renamed '%s' -> '%s' in %s"
              % (d.get("from", ""), d.get("to", ""), d.get("address", "")))

elif verb == "access_manage":
    d = call("POST", "/cli/mail/access/set-manage",
             body={"address": env("M_ADDRESS"), "project": env("M_WORKSPACE"),
                   "canManage": env("M_MANAGE") == "true",
                   "canDelete": env("M_DELETE") == "true"})
    if as_json:
        print(dumps(d))
    else:
        who = "primary" if d.get("isPrimary") else env("M_WORKSPACE")
        caps = ["manage" if d.get("canManage") else "no-manage",
                "delete" if d.get("canDelete") else "no-delete"]
        print("set %s caps on %s: %s" % (who, d.get("address", ""), ", ".join(caps)))

elif verb == "server_enable":
    hostname = env("M_HOSTNAME")
    if not hostname:
        die({"code": "usage", "hint": "k2 hostmail enable --hostname <name> (e.g. mail.acme.dev)"}, 2)
    d = call("POST", "/cli/mail/server/enable", body={"hostname": hostname})
    if as_json:
        print(dumps(d))
    elif d.get("alreadyEnabled"):
        print(d.get("hint") or "already enabled (no-op)")
    else:
        # A missing helper never gets here: the daemon answers 409
        # mail_helper_missing and die() prints the root install command.
        if d.get("helper"):
            print("helper   : %s" % d["helper"])
        print(d.get("hint") or "mail server: enable requested.")

elif verb == "import":
    body = {"address": env("M_ADDRESS")}
    if env("M_MAILDIR"):
        body["maildir"] = env("M_MAILDIR")
    if env("M_SKIP_INBOX") == "true":
        body["skipInbox"] = True
    if env("M_IMAPSYNC_HOST"):
        imapsync = {"host": env("M_IMAPSYNC_HOST")}
        if env("M_IMAPSYNC_USER"):
            imapsync["user"] = env("M_IMAPSYNC_USER")
        if env("M_IMAPSYNC_PASS"):
            imapsync["password"] = env("M_IMAPSYNC_PASS")
        if env("M_IMAPSYNC_PORT"):
            imapsync["port"] = int(env("M_IMAPSYNC_PORT"))
        body["imapsync"] = imapsync
    # Large Maildirs (1G+) outlive the 30s default; daemon keeps importing.
    # Import-only wait: 7200s (2h). Leave the global 30s POST helper alone.
    d = call("POST", "/cli/mail/import", body=body, timeout=7200)
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or "imported %s message(s) into %s"
              % (d.get("imported", "?"), d.get("address", env("M_ADDRESS") or "?")))

elif verb == "server_disable":
    d = call("POST", "/cli/mail/server/disable", body={})
    print(dumps(d) if as_json else (d.get("hint") or "mail server disabled."))

elif verb == "server_uninstall":
    hostname = env("M_HOSTNAME")
    if not hostname:
        die({"code": "usage", "hint": "k2 hostmail uninstall --purge --confirm-hostname <host>"}, 2)
    d = call("POST", "/cli/mail/server/uninstall",
             body={"purgeData": True, "confirmHostname": hostname})
    print(dumps(d) if as_json else (d.get("hint") or "mail server uninstalled."))

elif verb == "server_upgrade":
    # Calendars S1: explicit Stalwart upgrade (owner/admin). The daemon
    # preflights synchronously, then runs in the background; we poll
    # /cli/mail/status (upgrade) until it ends. Ctrl-C stops the polling,
    # never the upgrade.
    def _hb(n):
        if n is None:
            return "?"
        n = float(n)
        for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
            if n < 1024 or unit == "TiB":
                return ("%d %s" % (n, unit)) if unit == "B" else ("%.1f %s" % (n, unit))
            n /= 1024.0
    if env("M_ACK_FAILED") == "true":
        d = call("POST", "/cli/mail/server/upgrade", body={"acknowledgeFailed": True})
        print(dumps(d) if as_json else (d.get("hint") or "acknowledged."))
    elif env("M_DRY_RUN") == "true":
        d = call("POST", "/cli/mail/server/upgrade", body={"dryRun": True}, timeout=120)
        if as_json:
            print(dumps(d))
        elif d.get("outcome") == "noop":
            print(d.get("hint") or "already on the pinned version (no-op)")
        else:
            print("upgrade  : Stalwart %s -> %s  (%s)" % (
                d.get("from") or "?", d.get("to") or "?",
                "ready" if d.get("ready") else "BLOCKED — nothing will run"))
            print("server   : %s (running now: %s)" % (
                d.get("state") or "?", "yes" if d.get("wasRunning") else "no"))
            for b in d.get("blockers") or []:
                print("blocked  : [%s] %s" % (b.get("code"), b.get("hint")))
            if d.get("dataBytes") is not None:
                print("data     : %s data + %s config -> snapshot %s (free %s)" % (
                    _hb(d.get("dataBytes")), _hb(d.get("configBytes")),
                    d.get("snapshotDir") or "?", _hb(d.get("freeBytes"))))
            out = d.get("outage")
            if isinstance(out, dict):
                print("outage   : ~%ss typical; up to %ss if it fails and rolls back" % (
                    out.get("typicalSecs"), out.get("worstCaseSecs")))
                print("           (%s)" % out.get("assumption"))
            print("plan     :")
            for i, s in enumerate(d.get("plan") or [], 1):
                print("  %d. %s" % (i, s))
            print("rollback :")
            for s in d.get("rollback") or []:
                print("  - %s" % s)
            for n in d.get("notes") or []:
                print("note     : %s" % n)
        if d.get("dryRun") and not d.get("ready"):
            sys.exit(1)
    else:
        d = call("POST", "/cli/mail/server/upgrade", body={}, timeout=120)
        if d.get("outcome") == "noop":
            print(dumps(d) if as_json else (d.get("hint") or "already on the pinned version (no-op)"))
            sys.exit(0)
        if not as_json:
            out = d.get("outage") or {}
            print("upgrading Stalwart %s -> %s in the background (outage ~%ss; Ctrl-C stops"
                  " watching, not the upgrade)" % (d.get("from") or "?", d.get("to") or "?",
                                                   out.get("typicalSecs", "?")))
        try:
            interval = float(os.environ.get("K2_HOSTMAIL_UPGRADE_POLL") or "2")
        except ValueError:
            interval = 2.0
        deadline = time.time() + 6 * 3600
        seen = 0
        final = None
        terminal = ("succeeded", "aborted", "rolled_back", "rollback_failed", "interrupted")
        # A daemon restart mints a new owner token, so this watcher's
        # token goes stale and every poll fails from then on. Give up after
        # a bounded gap instead of polling silently for 6 h.
        try:
            lost_after = float(os.environ.get("K2_HOSTMAIL_UPGRADE_LOST_SECS") or "120")
        except ValueError:
            lost_after = 120.0
        last_ok = time.time()
        while time.time() < deadline:
            time.sleep(interval)
            s = request("GET", "/cli/mail/status")
            if not isinstance(s, dict) or s.get("error"):
                if time.time() - last_ok >= lost_after:
                    die({"code": "daemon_lost",
                         "hint": "no upgrade status from the K2 daemon for %ds (it restarted, or "
                                 "is down). A restarted daemon finishes or rolls back the "
                                 "upgrade at start and starts Stalwart again: run "
                                 "`k2 hostmail status --json` (upgrade.state, upgrade.recovery). "
                                 "Only if upgrade.state is still 'interrupted', follow "
                                 "upgrade.manualSteps, then "
                                 "`k2 hostmail upgrade --acknowledge-failed`" % int(lost_after)})
                continue  # daemon busy or restarting — keep watching
            last_ok = time.time()
            up = s.get("upgrade") or {}
            steps = up.get("steps") or []
            if not as_json:
                for st in steps[seen:]:
                    print("  %-18s %-6s %s" % (st.get("step"), "ok" if st.get("ok") else "FAILED",
                                               st.get("detail") or ""))
            seen = len(steps)
            if up.get("state") in terminal:
                final = up
                break
        if final is None:
            die({"code": "timeout", "hint": "the upgrade is still running after 6h — check k2 hostmail status"}, 2)
        if as_json:
            print(dumps(final))
        else:
            state = final.get("state")
            if state == "succeeded":
                print("upgraded : Stalwart %s (snapshot kept at %s)" % (
                    final.get("to"), final.get("snapshotDir") or "?"))
            else:
                print("result   : %s" % state)
            if final.get("error"):
                print("error    : %s" % final["error"])
            if final.get("warning"):
                print("WARNING  : %s" % final["warning"])
            if state == "rollback_failed":
                sys.stderr.write("ROLLBACK FAILED — mail may be down. Manual steps (as root):\n")
                for s in final.get("manualSteps") or []:
                    sys.stderr.write("  %s\n" % s)
        if final.get("state") != "succeeded":
            sys.exit(1)

elif verb == "cert_renew":
    d = call("POST", "/cli/mail/cert/renew", body={})
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or "ACME renewal queued for %s" % (d.get("hostname") or "mail hostname"))

elif verb == "cert_owner":
    # Who owns the mail certificate (k2 | stalwart-acme | unknown).
    if env("M_CERT_ACTION") == "stalwart-acme":
        d = call("POST", "/cli/mail/cert/owner", body={"action": "stalwart-acme"})
        if as_json:
            print(dumps(d))
        else:
            print(d.get("hint") or "Stalwart ACME orders the mail host only")
    else:
        d = call("GET", "/cli/mail/cert/owner")
        if as_json:
            print(dumps(d))
        else:
            owner = d.get("owner") or "?"
            print("hostname : %s" % (d.get("hostname") or "?"))
            print("owner    : %s" % owner)
            acme = d.get("stalwartAcme") or {}
            if acme.get("error"):
                print("stalwart : could not read (%s)" % acme["error"])
            elif acme.get("type") == "Automatic":
                print("stalwart : ACME Automatic, names: %s" % (", ".join(acme.get("names") or []) or "(empty = Stalwart's wide default)"))
            elif acme.get("type"):
                print("stalwart : ACME %s" % acme["type"])
            if owner == "k2":
                print("           K2 issues, installs and renews it; Stalwart ACME stays Manual.")
            elif acme.get("type") == "Manual":
                print("           Stalwart ACME is off and K2 does not own it — %s" % (d.get("restoreCommand") or "k2 hostmail cert owner --stalwart-acme"))

elif verb in ("backup_status", "backup_plan", "backup_set"):
    # S8 B1 hosted-mail backup (no backups run yet).
    def _gib(n):
        try:
            n = float(n)
        except (TypeError, ValueError):
            return "?"
        if abs(n) >= (1 << 30):
            return "%.1f GiB" % (n / (1 << 30))
        if abs(n) >= (1 << 20):
            return "%.0f MiB" % (n / (1 << 20))
        return "%d B" % n
    def _dur(secs):
        try:
            s = int(secs)
        except (TypeError, ValueError):
            return "?"
        if s < 120:
            return "%d s" % s
        if s < 7200:
            return "%d min" % (s // 60)
        return "%.1f h" % (s / 3600.0)
    def _print_backup(b):
        print("mode     : %s" % (b.get("mode") or "?"))
        if b.get("configuredBy"):
            ws = (" (%s)" % b["configuredWorkspace"]) if b.get("configuredWorkspace") else ""
            print("set by   : %s%s, %s" % (b["configuredBy"], ws, unix_ago(b.get("configuredAt"))))
        if b.get("reason"):
            print("reason   : %s" % b["reason"])
        if b.get("window"):
            print("window   : %s (box time)" % b["window"])
        r = b.get("retention") or {}
        if r:
            loc = r.get("local") or {}
            off = r.get("offsite") or {}
            cap = loc.get("maxLocalGb")
            print("retention: local %s daily + %s weekly%s; offsite %s/%s/%s (d/w/m)" % (
                loc.get("keepDaily"), loc.get("keepWeekly"),
                (", max %s GiB" % cap) if cap else "",
                off.get("keepDaily"), off.get("keepWeekly"), off.get("keepMonthly")))
        o = b.get("observer") or {}
        line = "observer : %s" % (o.get("status") or "never")
        if o.get("lastSuccess"):
            line += ", last %s" % unix_ago(o["lastSuccess"])
        line += ", %s day(s) of churn" % (o.get("days") or 0)
        print(line)
        cp = o.get("churnPerDay") or {}
        if o.get("days"):
            print("churn    : deleted %s/day (worst %s), new %s/day%s" % (
                _gib(cp.get("deletedBytes")), _gib(cp.get("maxDeletedBytes")),
                _gib(cp.get("createdBytes")), "" if cp.get("measured") else " — not yet measured (3 days)"))
        if o.get("storeBytes") is not None:
            print("store    : %s (mutable %s); fs free %s of %s; view root same fs: %s" % (
                _gib(o.get("storeBytes")), _gib(o.get("mutableBytes")), _gib(o.get("fsFree")),
                _gib(o.get("fsTotal")), {True: "yes", False: "NO"}.get(o.get("sameFs"), "?")))
        if o.get("lastError"):
            print("obs err  : %s" % o["lastError"])
        if b.get("configError"):
            print("config   : %s" % b["configError"])
        print("note     : %s" % (b.get("note") or ""))
    if verb == "backup_status":
        d = call("GET", "/cli/mail/backup")
        if as_json:
            print(dumps(d))
        else:
            _print_backup(d.get("backup") or {})
    elif verb == "backup_plan":
        params = {}
        for k, e in (("bandwidthMbps", "M_BK_BANDWIDTH"), ("keepDaily", "M_BK_KEEP_DAILY"),
                     ("keepWeekly", "M_BK_KEEP_WEEKLY"), ("maxLocalGb", "M_BK_MAX_LOCAL_GB")):
            if env(e):
                params[k] = env(e)
        # A store listing on a big box takes a moment.
        d = call("GET", "/cli/mail/backup/plan", params=params, timeout=120)
        if as_json:
            print(dumps(d))
        else:
            st = d.get("store") or {}
            ch = d.get("churn") or {}
            print("store    : %s at %s (%s files hard-linkable, mutable %s, /etc/stalwart %s, K2 half %s)" % (
                _gib(st.get("bytes")), st.get("path"), st.get("immutableFiles"),
                _gib(st.get("mutableBytes")), _gib(st.get("configBytes")), _gib(st.get("k2HalfBytes"))))
            print("disk     : free %s of %s; floor %s; %s same filesystem: %s" % (
                _gib(st.get("fsFree")), _gib(st.get("fsTotal")), _gib(st.get("floorBytes")),
                st.get("viewRoot"), {True: "yes", False: "NO"}.get(st.get("sameFs"), "unknown")))
            print("churn    : %s — deleted %s/day (worst %s), new %s/day" % (
                ch.get("label"), _gib(ch.get("deletedPerDay")), _gib(ch.get("maxDeletedPerDay")),
                _gib(ch.get("createdPerDay"))))
            r = d.get("retention") or {}
            loc = r.get("local") or {}
            off = r.get("offsite") or {}
            print("retention: local %s daily + %s weekly%s; offsite %s/%s/%s (d/w/m); window %s" % (
                loc.get("keepDaily"), loc.get("keepWeekly"),
                (", max %s GiB" % loc["maxLocalGb"]) if loc.get("maxLocalGb") else "",
                off.get("keepDaily"), off.get("keepWeekly"), off.get("keepMonthly"), d.get("window")))
            mbps = d.get("bandwidthMbps")
            for name in ("local", "offsite-only", "both", "none"):
                m = (d.get("modes") or {}).get(name) or {}
                print("")
                head = "%-12s verdict %s" % (name, (m.get("verdict") or "?").upper())
                if m.get("available") is False:
                    head += "  (not available yet: %s)" % (m.get("unavailableReason") or "")
                print(head)
                if name == "none":
                    print("  nothing runs; --reason is required and recorded")
                    continue
                print("  local disk : first night %s, steady %s, peak %s (free after %s)" % (
                    _gib(m.get("firstNightLocalBytes")), _gib(m.get("steadyLocalBytes")),
                    _gib(m.get("peakBytes")), _gib(m.get("freeAfterBytes"))))
                o = m.get("offsite")
                if isinstance(o, dict):
                    fu = o.get("firstUploadBytes") or {}
                    print("  offsite    : first upload %s–%s (~%s at %s Mbit/s), nightly %s (~%s)" % (
                        _gib(fu.get("low")), _gib(fu.get("high")), _dur(o.get("firstUploadSecs")), mbps,
                        _gib(o.get("nightlyUploadBytes")), _dur(o.get("nightlyUploadSecs"))))
                    print("  temp space : first %s, nightly %s" % (
                        _gib(o.get("firstTempBytes")), _gib(o.get("nightlyTempBytes"))))
                print("  downtime   : ~%s nightly (%s)" % (_dur(m.get("downtimeSecs")), m.get("downtimeLabel") or ""))
                print("  protects   : %s" % (m.get("protects") or ""))
            print("")
            for n in d.get("notes") or []:
                print("note     : %s" % n)
            print("planHash : %s" % d.get("planHash"))
            print("next     : k2 hostmail backup set --mode <local|none> [same retention flags] --confirm-plan %s" % d.get("planHash"))
    else:
        body = {"mode": env("M_BK_MODE"), "confirmPlan": env("M_BK_CONFIRM")}
        if env("M_REASON"):
            body["reason"] = env("M_REASON")
        for k, e in (("keepDaily", "M_BK_KEEP_DAILY"), ("keepWeekly", "M_BK_KEEP_WEEKLY"),
                     ("offsiteKeepDaily", "M_BK_OFF_DAILY"), ("offsiteKeepWeekly", "M_BK_OFF_WEEKLY"),
                     ("offsiteKeepMonthly", "M_BK_OFF_MONTHLY")):
            if env(e):
                body[k] = int(env(e))
        if env("M_BK_WINDOW"):
            body["window"] = env("M_BK_WINDOW")
        if env("M_BK_MAX_LOCAL_GB"):
            try:
                body["maxLocalGb"] = float(env("M_BK_MAX_LOCAL_GB"))
            except ValueError:
                die({"code": "usage", "hint": "--max-local-gb must be a number of GiB, got '%s'" % env("M_BK_MAX_LOCAL_GB")})
        d = call("POST", "/cli/mail/backup/set", body=body, timeout=120)
        if as_json:
            print(dumps(d))
        else:
            print("backup mode: %s → %s" % (d.get("from") or "?", (d.get("backup") or {}).get("mode") or "?"))
            _print_backup(d.get("backup") or {})

elif verb == "outbound":
    # 0.45.1: mx route lookup strategy + DANE per TLS strategy (who set it).
    if env("M_OUTBOUND_ACTION") == "dane-on":
        d = call("POST", "/cli/mail/outbound", body={"dane": "on"})
        if as_json:
            print(dumps(d))
        else:
            print(d.get("hint") or "DANE is optional again")
    else:
        d = call("GET", "/cli/mail/outbound")
        if as_json:
            print(dumps(d))
        else:
            who = {"k2": "set by K2", "hand": "set by hand", "default": "Stalwart default"}
            print("mx route : %s (%s)" % (d.get("ipStrategy") or "?",
                  who.get(d.get("ipStrategySetBy"), d.get("ipStrategySetBy") or "?")))
            for t in d.get("dane") or []:
                print("DANE     : %-12s %s (%s)" % (t.get("name") or "?", t.get("dane") or "?",
                      who.get(t.get("setBy"), t.get("setBy") or "?")))
            print("DNSSEC   : %s" % (d.get("dnssec") or "not checked (k2 hostmail doctor)"))
            if d.get("pendingReload"):
                pr = d["pendingReload"]
                print("WARNING  : settings saved but not live since %s — %s" % (pr.get("at") or "?", pr.get("error") or "?"))
            if any((t.get("dane") == "disable") for t in (d.get("dane") or [])):
                print("           turn DANE back on (Stalwart 0.16.20+): k2 hostmail outbound dane on")

elif verb == "cert_names":
    # CAL44: per-name certificates for the extra mail-family names.
    action = env("M_CERT_ACTION") or "list"
    if action == "list":
        d = call("GET", "/cli/mail/cert/names", timeout=120)
    else:
        body = {"action": action}
        if env("M_NAME"):
            body["name"] = env("M_NAME")
        # Each new name is one DNS-01 order (~2 min with the NS wait).
        d = call("POST", "/cli/mail/cert/names", body=body, timeout=1800)
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("names") or []
        if not rows:
            print("no extra names (no hosted domains)")
        def _yn(v):
            return "yes" if v is True else ("no" if v is False else "?")
        for r in rows:
            if action == "list":
                cert = r.get("cert") or {}
                exp = (" expires %s" % cert["expiresAt"]) if cert.get("expiresAt") else ""
                print("%-34s here %-3s  cert %s%s  (%s)" % (
                    r.get("name") or "?", _yn(r.get("pointsHere")),
                    cert.get("state") or "?", exp, r.get("issuer") or "?"))
                if r.get("dns") and r.get("pointsHere") is not True:
                    print("%-34s   %s" % ("", r["dns"]))
                if r.get("reason"):
                    print("%-34s   %s" % ("", r["reason"]))
                if cert.get("lastError"):
                    print("%-34s   last error: %s" % ("", cert["lastError"]))
            else:
                exp = (" expires %s" % r["expiresAt"]) if r.get("expiresAt") else ""
                print("%-34s %s%s" % (r.get("name") or "?", r.get("result") or "?", exp))
                if r.get("detail"):
                    print("%-34s   %s" % ("", r["detail"]))
        if action != "list":
            rel = d.get("reload") or {}
            line = "reload   : %s" % (rel.get("method") or "?")
            if rel.get("error"):
                line += " — %s" % rel["error"]
            print(line)
    if action != "list" and d.get("failed"):
        sys.exit(1)

elif verb == "rotate_admin":
    d = call("POST", "/cli/mail/server/rotate-admin", body={})
    if as_json:
        print(dumps(d))
    else:
        print(d.get("hint") or "rotated leftover recovery principal and provisioned admin password — store kept")

elif verb == "password_rotate":
    d = call("POST", "/cli/mail/address/password", body={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        print("rotated %s" % (d.get("address") or env("M_ADDRESS") or "?"))
        if d.get("password"):
            print("password : %s  (shown once — save it for Mail.app / IMAP)" % d["password"])
        print("username : %s" % (d.get("username") or d.get("address") or env("M_ADDRESS") or "?"))
        def _ep(label, key, default_port):
            ep = d.get(key) or {}
            host = ep.get("host") or "?"
            port = ep.get("port") or default_port
            extra = "  alpn" if ep.get("alpn") else ""
            print("%s : %s:%s%s" % (label, host, port, extra))
        _ep("imap    ", "imap", 443)
        _ep("submit  ", "submission", 465)
        _ep("jmap    ", "jmap", 443)
        if "apiKeysRevoked" in d:
            print("api keys : %s revoked" % d["apiKeysRevoked"])
        if d.get("apiKeysWarning"):
            print("WARNING  : %s" % d["apiKeysWarning"])
        print("note: this invalidates the mint-time secret — Mail.app sessions must use the new password")
        print("note: the mailbox's API keys and app passwords stop working too (k2 hostmail app-password to add new ones)")

elif verb == "ooo_get":
    d = call("GET", "/cli/mail/ooo", params={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        if d.get("enabled"):
            until = d.get("until") or "none"
            print("%s  ooo on  days=%s  until=%s" % (
                d.get("address") or env("M_ADDRESS") or "?",
                d.get("days") if d.get("days") is not None else "?",
                until))
            if d.get("text"):
                print(d["text"])
        else:
            print("%s  ooo off" % (d.get("address") or env("M_ADDRESS") or "?"))

elif verb == "ooo_set":
    body = {"address": env("M_ADDRESS"), "text": env("M_TEXT")}
    if env("M_DAYS") != "":
        body["days"] = int(env("M_DAYS"))
    if env("M_UNTIL") != "":
        body["until"] = env("M_UNTIL")
    d = call("POST", "/cli/mail/ooo", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("ooo set for %s  days=%s  until=%s" % (
            d.get("address") or env("M_ADDRESS") or "?",
            d.get("days") if d.get("days") is not None else "7",
            d.get("until") or "none"))

elif verb == "ooo_unset":
    d = call("POST", "/cli/mail/ooo/unset", body={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        print("ooo unset for %s" % (d.get("address") or env("M_ADDRESS") or "?"))

elif verb == "footer_get":
    d = call("GET", "/cli/mail/footer")
    if as_json:
        print(dumps(d))
    else:
        if d.get("enabled") and d.get("text"):
            print("footer on")
            print(d["text"])
        else:
            print("footer off")

elif verb == "footer_set":
    body = {"text": env("M_TEXT")}
    if env("M_DOMAIN") != "":
        body["domain"] = env("M_DOMAIN")
    d = call("POST", "/cli/mail/footer", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("footer set")

elif verb == "footer_unset":
    body = {}
    if env("M_DOMAIN") != "":
        body["domain"] = env("M_DOMAIN")
    d = call("POST", "/cli/mail/footer/unset", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("footer unset")

elif verb == "app_password_list":
    d = call("GET", "/cli/mail/app-password", params={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("appPasswords") or []
        if not rows:
            print("no app passwords for %s" % (d.get("address") or env("M_ADDRESS") or "?"))
        else:
            print("%s app password(s) for %s:" % (len(rows), d.get("address") or env("M_ADDRESS") or "?"))
            for r in rows:
                print("  %s  %s  %s" % (r.get("id") or "?", r.get("label") or "", r.get("createdAt") or ""))

elif verb == "app_password_add":
    body = {"address": env("M_ADDRESS")}
    if env("M_LABEL") != "":
        body["label"] = env("M_LABEL")
    d = call("POST", "/cli/mail/app-password", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("created app password for %s" % (d.get("address") or env("M_ADDRESS") or "?"))
        print("id     : %s" % (d.get("id") or "?"))
        print("label  : %s" % (d.get("label") or "k2"))
        if d.get("secret"):
            print("secret : %s  (shown once — save it for Mail.app / IMAP)" % d["secret"])

elif verb == "app_password_revoke":
    d = call("POST", "/cli/mail/app-password/revoke", body={"address": env("M_ADDRESS"), "id": env("M_ID")})
    if as_json:
        print(dumps(d))
    else:
        print("revoked %s on %s" % (d.get("id") or env("M_ID") or "?", d.get("address") or env("M_ADDRESS") or "?"))

elif verb == "credentials_keep":
    # Owner only. M_ID set = an app password; empty = the mailbox password.
    body = {"address": env("M_ADDRESS")}
    if env("M_ID"):
        body["appPasswordId"] = env("M_ID")
    d = call("POST", "/cli/mail/credentials/keep", body=body)
    if as_json:
        print(dumps(d))
    elif d.get("appPasswordId"):
        print("kept app password %s on %s — the doctor will stop flagging it" % (
            d["appPasswordId"], d.get("address") or env("M_ADDRESS") or "?"))
    else:
        print("kept the mailbox password of %s — the doctor will stop flagging it" % (
            d.get("address") or env("M_ADDRESS") or "?"))

elif verb == "person_set" and env("M_ALL") == "true":
    on = env("M_PERSON") == "on"
    d = call("POST", "/cli/mail/address/person",
             body={"all": True, "workspace": env("M_WORKSPACE"), "person": on})
    if as_json:
        print(dumps(d))
    else:
        changed = d.get("changed") or []
        word = "people's" if on else "agents'"
        print("marked %d mailbox(es) in %s as %s mailboxes" % (len(changed), env("M_WORKSPACE"), word))
        for a in changed:
            print("  %s" % a)
        excluded = d.get("excludedSendIdentities") or []
        if excluded:
            print("left as the agent's own (its send identities): %s" % ", ".join(excluded))

elif verb == "person_set":
    on = env("M_PERSON") == "on"
    d = call("POST", "/cli/mail/address/person",
             body={"address": env("M_ADDRESS"), "person": on})
    if as_json:
        print(dumps(d))
    elif on:
        print("%s is now a person's mailbox — the owner and IT agents can set its passwords"
              % (d.get("address") or env("M_ADDRESS") or "?"))
    else:
        print("%s is now an agent's mailbox" % (d.get("address") or env("M_ADDRESS") or "?"))

elif verb in ("credentials_keep_all_mailbox", "credentials_keep_all_app_password"):
    # Owner only. Bulk review: only credentials created before the
    # 0.45.0 update (the 0133 cutoff); newer ones stay flagged.
    kind = "mailbox" if verb == "credentials_keep_all_mailbox" else "app_password"
    d = call("POST", "/cli/mail/credentials/keep",
             body={"allExisting": True, "kind": kind}, timeout=120)
    if as_json:
        print(dumps(d))
    else:
        kept = d.get("kept") or {}
        left = d.get("leftFlagged") or {}
        n = kept.get("mailboxPasswords") if kind == "mailbox" else kept.get("appPasswords")
        what = "mailbox password(s)" if kind == "mailbox" else "app password(s)"
        print("kept %s %s that existed before this update — the doctor will stop flagging them"
              % (n if n is not None else 0, what))
        newer = int(left.get("createdAfterCutoff") or 0)
        undated = int(left.get("noCreationTime") or 0)
        if newer or undated:
            print("still flagged: %d created after the update, %d with no creation time — "
                  "review those one by one (k2 hostmail doctor)" % (newer, undated))
        for e in d.get("listErrors") or []:
            print("could not list app passwords: %s" % e)

elif verb == "dkim_show":
    d = call("GET", "/cli/mail/dkim", params={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        print("dkim  %s" % (d.get("domain") or env("M_DOMAIN") or "?"))
        for s in d.get("signatures") or []:
            print("  %-28s  %-8s  %-22s  %s" % (
                s.get("selector") or "?",
                s.get("stage") or "?",
                s.get("type") or "?",
                s.get("match") or "?"))
            if s.get("expected"):
                print("    TXT %s" % (s.get("name") or ""))
                print("    %s" % s["expected"])

elif verb == "dkim_rotate":
    d = call("POST", "/cli/mail/dkim", body={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        print("rotated dkim on %s  planted=%s" % (
            d.get("domain") or env("M_DOMAIN") or "?",
            d.get("planted")))
        for rec in d.get("records") or []:
            print("%s  TXT  %s" % (rec.get("name") or "?", rec.get("value") or rec.get("expectedDisplay") or ""))
        if not d.get("planted"):
            print("plant this TXT at your DNS host (this box is not dnsWrite)")

elif verb == "dkim_retire":
    d = call("POST", "/cli/mail/dkim/retire",
             body={"domain": env("M_DOMAIN"), "selector": env("M_SELECTOR")})
    if as_json:
        print(dumps(d))
    else:
        print("retired selector %s on %s" % (
            d.get("selector") or env("M_SELECTOR") or "?",
            d.get("domain") or env("M_DOMAIN") or "?"))

elif verb == "dmarc_show":
    d = call("GET", "/cli/mail/dmarc", params={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        print("dmarc %s" % (d.get("domain") or env("M_DOMAIN") or "?"))
        rec = d.get("dmarc") or {}
        print("  txt     : %s" % (rec.get("expected") or "?"))
        print("  live    : %s" % (rec.get("match") or "?"))
        print("  report  : %s%s" % (
            d.get("reportAddressUri") or "mailto:postmaster",
            "" if d.get("reportAddressMinted") else "  (not a minted inbox)"))
        print("  plant   : %s" % (d.get("suggestedRua") or ""))
        if d.get("note"):
            print("  note    : %s" % d["note"])

elif verb == "dmarc_report_to":
    d = call("POST", "/cli/mail/dmarc",
             body={"domain": env("M_DOMAIN"), "address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        print("dmarc report-to %s → %s" % (
            d.get("domain") or env("M_DOMAIN") or "?",
            d.get("reportAddressUri") or env("M_ADDRESS") or "?"))
        print("plant DNS: %s" % (d.get("suggestedRua") or ""))

elif verb == "ptr_show":
    d = call("GET", "/cli/mail/ptr")
    if as_json:
        print(dumps(d))
    else:
        print("origin : %s" % (d.get("originIpv4") or "?"))
        print("ptr    : %s" % (d.get("ptr") or "(none)"))
        print("helo   : %s" % (d.get("helo") or d.get("hostname") or "(none)"))
        a = d.get("aRecords") or []
        print("A      : %s" % (", ".join(a) if a else "(none)"))
        print("match  : ptr==helo=%s  A==origin=%s  aligned=%s" % (
            d.get("ptrMatchesHelo"), d.get("aMatchesOrigin"), d.get("aligned")))
        # 0.46.6: a K2 relay name that did not resolve; ptr set waits for it.
        unresolved = (d.get("relayCheck") or {}).get("unresolved") or []
        if unresolved:
            print("relays : could not resolve %s (ptr set refuses until it does; retry)" % ", ".join(unresolved))

elif verb == "ptr_set":
    d = call("POST", "/cli/mail/ptr/set", body={"hostname": env("M_HOSTNAME")})
    if as_json:
        print(dumps(d))
    else:
        print("ptr set  hostname=%s  helo=%s  origin=%s  aligned=%s" % (
            d.get("hostname") or env("M_HOSTNAME") or "?",
            d.get("helo") or "?",
            d.get("originIpv4") or "?",
            d.get("aligned")))
        if d.get("ptrSource") == "authoritative":
            print("published ptr: %s  (k2.dev, authoritative nameservers)" % d.get("ptr"))
            print("local ptr    : %s" % (d.get("ptrLocal") or "(none)"))
        elif d.get("ptr"):
            print("live ptr: %s" % d.get("ptr"))
        if d.get("resolverNote"):
            print("note: %s" % d.get("resolverNote"))
        settings = d.get("settings") or {}
        if settings.get("warning"):
            print("warning: %s" % settings.get("warning"))
        dc = d.get("defaultCertificate") or {}
        if dc.get("state") == "moved":
            print("default certificate: moved to this name's newest (%s)" % dc.get("certificateId"))
        elif dc.get("warning"):
            print("warning: %s" % dc.get("warning"))

elif verb == "bans_list":
    d = call("GET", "/cli/mail/bans")
    if as_json:
        print(dumps(d))
    else:
        bans = d.get("bans") or []
        counts = d.get("counts") or {}
        if not bans:
            print("no blocked IPs")
        else:
            bits = ["%s=%s" % (k, counts[k]) for k in sorted(counts, key=lambda r: (0 if r == "authFailure" else 1, r))]
            print("%s blocked  %s" % (d.get("count") if d.get("count") is not None else len(bans), "  ".join(bits)))
            auth = [b for b in bans if (b.get("reason") or "") == "authFailure"]
            other = [b for b in bans if (b.get("reason") or "") != "authFailure"]
            if auth:
                print("authFailure:")
                for b in auth:
                    print("  %s  id=%s  expires=%s" % (
                        b.get("address") or "?",
                        b.get("id") or "?",
                        b.get("expiresAt") if b.get("expiresAt") is not None else "null"))
            nscan = sum(1 for b in other if (b.get("reason") or "") == "portScanning")
            nrest = len(other) - nscan
            if nscan or nrest:
                extra = []
                if nscan:
                    extra.append("%s portScanning" % nscan)
                if nrest:
                    extra.append("%s other" % nrest)
                print("scanners: %s  (full list: k2 hostmail bans list --json)" % ", ".join(extra))

elif verb == "bans_clear":
    body = {"ip": env("M_IP"), "all": env("M_ALL") == "true"}
    d = call("POST", "/cli/mail/bans/clear", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("cleared ip=%s destroyed=%s all=%s" % (
            d.get("ip") or env("M_IP"),
            ",".join(d.get("destroyed") or []) or "(none)",
            d.get("all")))
        for w in (d.get("warnings") or []):
            print("warn: %s" % w)
        print("note: unban does not reset fail counters — pair with allowlist add")

elif verb == "bans_migrate":
    hours = env("M_HOURS")
    body = {"hours": int(hours)} if hours else {}
    d = call("POST", "/cli/mail/bans/migrate", body=body)
    if as_json:
        print(dumps(d))
    else:
        if d.get("active") and hours:
            print("migrate active hours=%s restoreAt=%s authBanRate=null" % (
                d.get("hours") or hours, d.get("restoreAt")))
            print("snapshot=%s" % dumps(d.get("authBanRateSnapshot")))
        else:
            print("migrate active=%s remaining=%ss restoreAt=%s" % (
                d.get("active"), d.get("remainingSeconds"), d.get("restoreAt")))

elif verb == "bans_migrate_restore":
    d = call("POST", "/cli/mail/bans/migrate/restore", body={})
    if as_json:
        print(dumps(d))
    else:
        print("migrate restore restored=%s authBanRate=%s" % (
            d.get("restored"), dumps(d.get("authBanRate"))))

elif verb == "bans_migrate_status":
    d = call("GET", "/cli/mail/bans/migrate")
    if as_json:
        print(dumps(d))
    else:
        print("migrate active=%s remaining=%ss restoreAt=%s overdue=%s" % (
            d.get("active"), d.get("remainingSeconds"),
            d.get("restoreAt"), d.get("overdue")))

elif verb == "allowlist_list":
    d = call("GET", "/cli/mail/allowlist")
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("allowlist") or []
        if not rows:
            print("allowlist empty")
        for b in rows:
            print("%s  reason=%s  id=%s  expires=%s" % (
                b.get("address") or "?",
                b.get("reason") or "?",
                b.get("id") or "?",
                b.get("expiresAt") if b.get("expiresAt") is not None else "null"))

elif verb == "allowlist_add":
    body = {"address": env("M_ADDRESS")}
    if env("M_REASON"):
        body["reason"] = env("M_REASON")
    if env("M_EXPIRES"):
        body["expiresAt"] = env("M_EXPIRES")
    d = call("POST", "/cli/mail/allowlist/add", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("allowlist add %s id=%s reason=%s expires=%s" % (
            d.get("address") or env("M_ADDRESS"),
            d.get("id") or "?",
            d.get("reason") or "?",
            d.get("expiresAt") if d.get("expiresAt") is not None else "null"))

elif verb == "allowlist_remove":
    d = call("POST", "/cli/mail/allowlist/remove", body={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        print("allowlist remove %s destroyed=%s" % (
            d.get("address") or env("M_ADDRESS"),
            ",".join(d.get("destroyed") or []) or "(none)"))

elif verb == "list_create":
    body = {"address": env("M_ADDRESS"), "members": env_list("M_MEMBERS")}
    d = call("POST", "/cli/mail/list", body=body)
    if as_json:
        print(dumps(d))
    else:
        print("list %s members=%s" % (d.get("address") or env("M_ADDRESS"),
            ",".join(d.get("members") or [])))

elif verb == "list_show":
    d = call("GET", "/cli/mail/list", params={"address": env("M_ADDRESS")})
    print(dumps(d) if as_json else "list %s members=%s" % (
        d.get("address") or env("M_ADDRESS"), ",".join(d.get("members") or [])))

elif verb == "list_members":
    body = {"address": env("M_ADDRESS")}
    op = env("M_LIST_OP")
    members = env_list("M_MEMBERS")
    if op == "add":
        body["add"] = members
    else:
        body["remove"] = members
    d = call("POST", "/cli/mail/list/members", body=body)
    print(dumps(d) if as_json else "list %s members=%s" % (
        d.get("address") or env("M_ADDRESS"), ",".join(d.get("members") or [])))

elif verb == "list_delete":
    d = call("POST", "/cli/mail/list/delete", body={"address": env("M_ADDRESS")})
    print(dumps(d) if as_json else "deleted list %s" % (d.get("address") or env("M_ADDRESS")))

elif verb == "spam_train":
    d = call("POST", "/cli/mail/spam/train", body={
        "address": env("M_ADDRESS"), "messageId": env("M_ID"), "class": env("M_CLASS")})
    print(dumps(d) if as_json else "trained %s as %s" % (d.get("id") or env("M_ID"), d.get("class") or env("M_CLASS")))

elif verb == "spam_sender":
    route = "/cli/mail/spam/allow" if env("M_LIST_OP") == "allow" else "/cli/mail/spam/block"
    d = call("POST", route, body={"sender": env("M_SENDER")})
    print(dumps(d) if as_json else "%s %s" % (d.get("action") or env("M_LIST_OP"), d.get("key") or env("M_SENDER")))

elif verb == "spam_quarantine_list":
    d = call("GET", "/cli/mail/spam/quarantine", params={"address": env("M_ADDRESS")})
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("messages") or []
        if not rows:
            print("No quarantined messages.")
        else:
            for m in rows:
                print("%s  %s" % (m.get("id") or "?", m.get("subject") or ""))

elif verb == "spam_quarantine_id":
    op = env("M_LIST_OP")
    route = "/cli/mail/spam/quarantine/release" if op == "release" else "/cli/mail/spam/quarantine/discard"
    d = call("POST", route, body={"address": env("M_ADDRESS"), "id": env("M_ID")})
    print(dumps(d) if as_json else "%s %s -> %s" % (op, d.get("id") or env("M_ID"), d.get("folder") or "?"))

elif verb == "queue_list":
    d = call("GET", "/cli/mail/queue")
    if as_json:
        print(dumps(d))
    else:
        rows = d.get("messages") or []
        if not rows:
            print("Outbound queue is empty.")
        else:
            for m in rows:
                print("%s  retry=%s  from=%s" % (m.get("id") or "?", m.get("nextRetry") or "-", m.get("returnPath") or "-"))

elif verb == "queue_retry":
    d = call("POST", "/cli/mail/queue/retry", body={"id": env("M_ID")})
    print(dumps(d) if as_json else "retry %s next=%s" % (d.get("id") or env("M_ID"), d.get("nextRetry") or "?"))

elif verb == "queue_drop":
    d = call("POST", "/cli/mail/queue/drop", body={"id": env("M_ID")})
    print(dumps(d) if as_json else "dropped queued %s" % (d.get("id") or env("M_ID")))

elif verb == "acl_grant":
    body = {"mailbox": env("M_MAILBOX"), "user": env("M_USER")}
    if env("M_RIGHTS"):
        body["rights"] = env("M_RIGHTS")
    if env("M_ACL_READ") == "true":
        body["read"] = True
    if env("M_ACL_WRITE") == "true":
        body["write"] = True
    if env("M_ACL_POST") == "true":
        body["post"] = True
    if env("M_ACL_ADMIN") == "true":
        body["admin"] = True
    d = call("POST", "/cli/mail/acl", body=body)
    print(dumps(d) if as_json else "acl grant %s -> %s rights=%s" % (
        d.get("user") or env("M_USER"), d.get("mailbox") or env("M_MAILBOX"), d.get("rights") or "?"))

elif verb == "acl_revoke":
    d = call("POST", "/cli/mail/acl/revoke", body={"mailbox": env("M_MAILBOX"), "user": env("M_USER")})
    print(dumps(d) if as_json else "acl revoke %s from %s" % (
        d.get("user") or env("M_USER"), d.get("mailbox") or env("M_MAILBOX")))

elif verb == "acl_show":
    d = call("GET", "/cli/mail/acl", params={"mailbox": env("M_MAILBOX")})
    print(dumps(d) if as_json else dumps(d.get("shareWith") or d))

elif verb == "autoconfig_show":
    d = call("GET", "/cli/mail/autoconfig", params={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        print("thunderbird  %s" % (d.get("thunderbirdUrl") or ""))
        print("autodiscover %s" % (d.get("autodiscoverUrl") or ""))
        if d.get("caldavUrl"):
            print("caldav       %s  (CalDAV/CardDAV: _caldavs/_carddavs SRV)" % d["caldavUrl"])
        for rec in d.get("records") or []:
            print("%s  %s  %s" % (rec.get("type") or "?", rec.get("name") or "?", rec.get("value") or rec.get("display") or ""))

elif verb == "autoconfig_apply":
    d = call("POST", "/cli/mail/autoconfig", body={"domain": env("M_DOMAIN")})
    if as_json:
        print(dumps(d))
    else:
        if d.get("planted"):
            print("planted autoconfig records for %s via k2 dns (not A8 bind)" % (d.get("domain") or env("M_DOMAIN")))
        else:
            print(d.get("hint") or "print these records at your registrar:")
            for rec in d.get("records") or []:
                print("%s  %s  %s" % (rec.get("type") or "?", rec.get("name") or "?", rec.get("value") or rec.get("display") or ""))

elif verb == "audit_sieve":
    # 0.46.6: read-only. Every user account's non-K2 Sieve scripts with
    # their redirect/notify targets (domain only), which K2 mailboxes still
    # lack the Sieve lock, and whether enotify is off. Changes nothing.
    d = call("GET", "/cli/mail/sieve-audit", timeout=300)
    if as_json:
        print(dumps(d))
    elif not d.get("installed"):
        print("hosted mail is not installed here — nothing to audit")
    else:
        s = d.get("summary") or {}
        en = d.get("enotify") or {}
        enotify = "off" if en.get("disabled") is True else ("ON" if en.get("disabled") is False else "unknown (%s)" % (en.get("error") or "?"))
        print("accounts scanned : %s (%s skipped: admin/service/group)" % (s.get("accountsScanned", "?"), s.get("accountsSkipped", "?")))
        print("k2 mailboxes     : %s (%s locked, %s NOT locked)" % (s.get("k2Mailboxes", "?"), s.get("lockedMailboxes", "?"), s.get("unlockedMailboxes", "?")))
        print("user scripts     : %s" % s.get("userScripts", "?"))
        print("k2-named, other  : %s" % s.get("unrecognisedK2Scripts", "?"))
        print("of those, outside: %s reach outside this box" % s.get("scriptsWithOutsideTargets", "?"))
        print("k2's own, no row : %s redirect(s) with no active forward behind them" % s.get("k2ScriptRedirectsNoRow", "?"))
        print("enotify          : %s" % enotify)
        print("api key lock     : %s K2 mailboxes locked, %s NOT locked (can still create API keys)" % (s.get("apiKeyLockedMailboxes", "?"), s.get("apiKeyUnlockedMailboxes", "?")))
        print("api keys held    : %s on %s account(s)" % (s.get("apiKeys", "?"), s.get("accountsWithApiKeys", "?")))
        if s.get("errors"):
            print("errors           : %s" % s["errors"])
        for a in d.get("accounts") or []:
            who = a.get("address") or ("%s (not a K2 mailbox)" % (a.get("name") or a.get("accountId")))
            flags = ""
            if a.get("k2Minted") and not a.get("sieveLocked"):
                flags += "  [Sieve NOT locked]"
            if a.get("k2Minted") and a.get("apiKeyLocked") is False:
                flags += "  [API keys NOT locked]"
            if a.get("apiKeys"):
                flags += "  [%s API key(s)]" % a["apiKeys"]
            print("")
            print("%s%s" % (who, flags))
            if a.get("apiKeysError"):
                print("  api keys: could not list (%s)" % a["apiKeysError"])
            if a.get("error"):
                print("  error: %s" % a["error"])
            for r in a.get("k2RedirectsNoRow") or []:
                print("  %-24s %-8s redirect → %s%s, no active forward behind it" % ('"%s"' % r.get("script"), "active" if r.get("active") else "inactive", r.get("domain"), " (outside)" if r.get("external") else ""))
            for sc in a.get("scripts") or []:
                tg = ", ".join("%s → %s%s" % (t.get("action"), t.get("domain"), " (outside)" if t.get("external") else "")
                               for t in sc.get("targets") or []) or "no forward or notify"
                print("  %-24s %-8s %-24s %s" % ('"%s"' % sc.get("name"), "active" if sc.get("active") else "inactive", sc.get("owner"), tg))
        if not d.get("accounts"):
            print("")
            print("nothing found: no user Sieve scripts, no API keys, and every K2 mailbox is locked")
        elif s.get("apiKeys"):
            print("")
            print("To revoke a mailbox's API keys: k2 hostmail password rotate <address>")

elif verb == "dav":
    # Calendars S2: owner DAV policy. status = GET; enable/disable = POST
    # (per-account Stalwart permissions; never hostmail disable/enable).
    action = env("M_DAV_ACTION")
    if action == "status":
        d = call("GET", "/cli/mail/dav")
    else:
        body = {"action": action}
        if env("M_FILES"):
            body["files"] = env("M_FILES")
        d = call("POST", "/cli/mail/dav", body=body, timeout=300)
    if as_json:
        print(dumps(d))
    else:
        if d.get("policy") == "default":
            policy = "Stalwart default (never set — run enable or disable once)"
        elif d.get("policy") == "applied":
            policy = "set by the owner"
            if d.get("backfilledAt") is None:
                policy += "; some accounts not done yet (finished on the next daemon start)"
        else:
            policy = d.get("policy") or "?"
        print("calendars : %s  (CalDAV + CardDAV for every hosted address)" % ("on" if d.get("enabled") else "off"))
        print("files     : %s  (WebDAV file storage)" % ("on" if d.get("files") else "off"))
        print("policy    : %s" % policy)
        print("reachable : %s" % ("yes" if d.get("reachable") else "no"))
        if d.get("url"):
            print("url       : %s" % d["url"])
        if d.get("reason"):
            print("reason    : %s" % d["reason"])
        acc = d.get("accounts")
        if isinstance(acc, dict):
            print("accounts  : %s total, %s updated, %s unchanged, %s skipped" % (
                acc.get("total", "?"), acc.get("updated", "?"), acc.get("unchanged", "?"),
                len(acc.get("skipped") or [])))
        print("note      : a calendar app retrying a wrong password gets its IP banned —"
              " k2 hostmail bans list / k2 hostmail allowlist")

elif verb in ("group_create", "group_members", "group_list", "group_show", "group_delete"):
    # Calendars S4: groups (mail-manage). A group is for shared calendars;
    # a list (k2 hostmail list) is for mail fan-out.
    def print_group(g):
        print("group %s · %s member(s)%s" % (
            g.get("address") or g.get("name") or "?", len(g.get("members") or []),
            "" if g.get("receivesMail") else " · inbound mail off"))
        for m in g.get("members") or []:
            print("  %s" % m)
    if verb == "group_create":
        body = {"name": env("M_NAME"), "mail": env("M_GROUP_MAIL") == "true"}
        if env("M_DOMAIN"):
            body["domain"] = env("M_DOMAIN")
        d = call("POST", "/cli/mail/group", body=body, timeout=60)
        if as_json:
            print(dumps(d))
        else:
            print("created group %s%s" % (d.get("address"),
                  "" if d.get("receivesMail") else " (inbound mail off; --mail keeps it on)"))
            print("add members: k2 hostmail group add %s <addr> [<addr>…]" % d.get("address"))
    elif verb == "group_members":
        body = {"group": env("M_GROUP")}
        body["add" if env("M_LIST_OP") == "add" else "remove"] = env_list("M_MEMBERS")
        d = call("POST", "/cli/mail/group/members", body=body, timeout=120)
        print(dumps(d) if as_json else "group %s members=%s" % (
            d.get("group") or env("M_GROUP"), ",".join(d.get("members") or []) or "(none)"))
    elif verb == "group_list":
        d = call("GET", "/cli/mail/group", timeout=60)
        if as_json:
            print(dumps(d))
        else:
            gs = d.get("groups") or []
            if not gs:
                print("No groups. Create one: k2 hostmail group create <name>")
            for g in gs:
                print_group(g)
    elif verb == "group_show":
        d = call("GET", "/cli/mail/group", params={"group": env("M_GROUP")}, timeout=60)
        if as_json:
            print(dumps(d))
        else:
            print_group(d.get("group") or {})
    else:
        d = call("POST", "/cli/mail/group/delete", body={"group": env("M_GROUP")}, timeout=300)
        print(dumps(d) if as_json else "deleted group %s (%s calendar share(s) and %s membership(s) removed)" % (
            d.get("group") or env("M_GROUP"), d.get("sharesRemoved", 0), d.get("membersRemoved", 0)))

elif verb in ("cal_manage", "cal_share", "cal_unshare", "cal_shares", "cal_sharing"):
    # Calendars S4: calendar create/rename + sharing (mail-manage).
    if verb == "cal_manage":
        body = {"action": env("M_CAL_ACTION"), "owner": env("M_ADDRESS"), "name": env("M_NAME")}
        if env("M_CALENDAR"):
            body["calendar"] = env("M_CALENDAR")
        if env("M_COLOR"):
            body["color"] = env("M_COLOR")
        d = call("POST", "/cli/mail/calendar/manage", body=body, timeout=60)
        if as_json:
            print(dumps(d))
        else:
            c = d.get("calendar") or {}
            if d.get("created"):
                print("created calendar '%s' (%s) on %s" % (c.get("name"), c.get("id"), d.get("owner")))
            else:
                print("renamed calendar '%s' → '%s' (%s) on %s" % (
                    c.get("previousName"), c.get("name"), c.get("id"), d.get("owner")))
    elif verb in ("cal_share", "cal_unshare"):
        body = {"owner": env("M_ADDRESS"), "with": env("M_WITH")}
        if env("M_CALENDAR"):
            body["calendar"] = env("M_CALENDAR")
        if verb == "cal_share":
            body["level"] = env("M_SHARE_LEVEL")
        d = call("POST", "/cli/mail/calendar/" + ("share" if verb == "cal_share" else "unshare"),
                 body=body, timeout=120)
        if as_json:
            print(dumps(d))
        else:
            c = d.get("calendar") or {}
            if verb == "cal_share":
                prev = d.get("previousLevel")
                print("shared '%s' of %s with %s: %s%s" % (
                    c.get("name"), d.get("owner"), d.get("with"), d.get("level"),
                    (" (was %s)" % prev) if prev else ""))
                if d.get("maxShares"):
                    print("limit: %s shares per calendar (k2 hostmail calendar sharing)" % d.get("maxShares"))
            elif d.get("removed"):
                print("stopped sharing '%s' of %s with %s" % (c.get("name"), d.get("owner"), d.get("with")))
            else:
                print("'%s' of %s was not shared with %s — nothing to do" % (
                    c.get("name"), d.get("owner"), d.get("with")))
    elif verb == "cal_shares":
        d = call("GET", "/cli/mail/calendar/shares", params={"owner": env("M_ADDRESS")}, timeout=60)
        if as_json:
            print(dumps(d))
        else:
            cals = d.get("calendars") or []
            if not cals:
                print("%s has no calendars." % d.get("owner"))
            for c in cals:
                shares = c.get("shares") or []
                print("%s · %s%s · %s" % (c.get("id"), c.get("name") or "(unnamed)",
                      " · default" if c.get("isDefault") else "",
                      "not shared" if not shares else "%d share(s)" % len(shares)))
                for s in shares:
                    print("  %s (%s): %s" % (s.get("with"), s.get("withType"), s.get("level")))
            if d.get("maxShares"):
                print("limit: %s shares per calendar" % d.get("maxShares"))
    else:
        if env("M_MAX_SHARES"):
            d = call("POST", "/cli/mail/calendar/sharing",
                     body={"maxShares": int(env("M_MAX_SHARES"))}, timeout=60)
        else:
            d = call("GET", "/cli/mail/calendar/sharing", timeout=60)
        if as_json:
            print(dumps(d))
        else:
            print("max shares per calendar : %s" % d.get("maxShares"))
            print("directory queries       : %s" % ("on" if d.get("allowDirectoryQueries") else "off"))
            if isinstance(d.get("reloaded"), str):
                print("note: %s" % d.get("reloaded"))

elif verb == "profile":
    # Calendars S6: Apple .mobileconfig with a FRESH app password. The
    # secret is only inside the profile: never echoed in human output,
    # never in an error.
    out = env("M_OUT")
    force = env("M_FORCE") == "true"
    if out and os.path.lexists(out) and not force:
        die({"code": "usage", "hint": "%s already exists — pass --force to overwrite (nothing was minted)" % out})
    d = call("POST", "/cli/mail/profile", body={"address": env("M_ADDRESS"), "apple": True}, timeout=60)
    xml = d.get("profile") or ""
    if not xml:
        die({"code": "error", "hint": "daemon returned no profile"})
    revoke = d.get("revoke") or "k2 hostmail app-password revoke %s %s" % (env("M_ADDRESS"), d.get("appPasswordId") or "<id>")
    if out:
        flags = os.O_WRONLY | os.O_CREAT | (os.O_TRUNC if force else os.O_EXCL)
        try:
            fd = os.open(out, flags, 0o600)
            try:
                os.fchmod(fd, 0o600)
                os.write(fd, xml.encode("utf-8"))
            finally:
                os.close(fd)
        except OSError as e:
            die({"code": "error", "hint": "could not write %s (%s) — an app password was minted anyway; revoke it: %s" % (out, e.strerror or e, revoke)})
    if as_json:
        if out:
            d = dict(d)
            d.pop("profile", None)
            d["out"] = out
        print(dumps(d))
    elif out:
        print("wrote    : %s  (mode 0600, unsigned — iOS/macOS show \"Unverified\")" % out)
        print("address  : %s" % (d.get("address") or env("M_ADDRESS")))
        print("server   : %s" % (d.get("mailHost") or "?"))
        print("includes : %s" % ", ".join(d.get("includes") or ["mail"]))
        print("password : app password %s (%s) — inside the file only" % (d.get("appPasswordId") or "?", d.get("description") or "?"))
        print("revoke   : %s" % revoke)
        for n in d.get("notes") or []:
            print("note     : %s" % n)
        print("next     : AirDrop or email the file to the Mac/iPhone, then Settings → Profile Downloaded → Install. Delete the file afterwards.")
    else:
        # Profile XML alone on stdout (pipe-safe); the summary on stderr.
        sys.stdout.write(xml)
        sys.stdout.flush()
        sys.stderr.write("app password %s (%s) is inside this profile; revoke: %s\n" % (d.get("appPasswordId") or "?", d.get("description") or "?", revoke))
        for n in d.get("notes") or []:
            sys.stderr.write("note: %s\n" % n)

else:
    die({"code": "usage", "hint": "unknown mail verb '%s'" % verb})
PYEOF
}

