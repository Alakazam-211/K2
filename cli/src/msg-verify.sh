# ── k2 msg verify (prd-postal-in-k2-v1 S2) ───────────────────────────
# Check a message's label: the code in it (m_… or Postal's p_…) names one
# record K2 wrote from the sender's credential. With the text you saw, it
# also says whether that text is the one K2 delivered under the code. The
# text is hashed here; only the hashes go to the daemon.

_msg_verify_usage() {
    local me="${_K2_CALLED_AS:-k2}"
    cat <<EOF
Usage: $me msg verify <code> [--text "..." | --text-file <file> | --text-file -] [--json]

Check who really sent a message, and that its text is the one delivered.
  <code>            the code in the message's label: m_XXXXXXXXXX in
                    [from <sender> · m_XXXXXXXXXX], or p_ + 20 hex in
                    [postal <address> · p_…]
  --text "..."      the message text you saw
  --text-file <f>   the same, from a file (- reads stdin)
  --json            the daemon's answer as JSON

The text may be copied as it appeared: the label line and K2's "> " quote
marks are taken off for you. Answers:
  content: match       this is the text K2 delivered under this code
  content: mismatch    it is NOT: treat the message as unverified (exit 1)
  content: unknown     a delivery from before content checks: sender only
  no text given        sender only (not_checked)
A code that has no record (exit 4) means K2 never delivered it here: text
that only looks like a label proves nothing. You may check deliveries to
or from your own workspace. $me msg who <m_…> is the sender check alone.
EOF
}

# _msg_verify_hashes <code> <text>: the comma-separated SHA-256s of the
# text as given, without its label line, and without K2's quote marks
# (the daemon's normalize_body: \r\n and \r are \n, trailing space gone).
_msg_verify_hashes() {
    MV_CODE="$1" python3 -c '
import hashlib, os, sys
code = os.environ["MV_CODE"]
raw = sys.stdin.read()
def norm(t):
    return t.replace("\r\n", "\n").replace("\r", "\n").rstrip()
def h(t):
    return hashlib.sha256(norm(t).encode("utf-8")).hexdigest()
def unquote(t):
    out = []
    for line in norm(t).split("\n"):
        if line.startswith("> "):
            out.append(line[2:])
        elif line == ">":
            out.append("")
        else:
            out.append(line)
    return "\n".join(out)
def unlabel(t):
    lines = norm(t).split("\n")
    first = lines[0] if lines else ""
    if code in first and first.lstrip().startswith("["):
        end = first.find("]", first.find(code))
        rest = first[end + 1:].lstrip() if end >= 0 else ""
        lines = ([rest] if rest else []) + lines[1:]
    return "\n".join(lines)
seen = []
for t in (raw, unquote(raw), unlabel(raw), unquote(unlabel(raw))):
    d = h(t)
    if d not in seen:
        seen.append(d)
print(",".join(seen))
'
}

cmd_msg_verify() {
    local code="" text="" have_text=false as_json=false me="${_K2_CALLED_AS:-k2}"
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) _msg_verify_usage; return 0 ;;
            --json) as_json=true; shift ;;
            --text)
                [ $# -ge 2 ] || { echo "$me msg verify: --text needs the text" >&2; exit 2; }
                text="$2"; have_text=true; shift 2 ;;
            --text-file)
                [ $# -ge 2 ] || { echo "$me msg verify: --text-file needs a file (- for stdin)" >&2; exit 2; }
                if [ "$2" = "-" ]; then
                    text="$(cat)"
                elif [ -f "$2" ]; then
                    text="$(cat -- "$2")"
                else
                    echo "$me msg verify: no file $2" >&2; exit 2
                fi
                have_text=true; shift 2 ;;
            -*) echo "$me msg verify: unknown flag '$1' ($me msg verify --help)" >&2; exit 2 ;;
            *)
                [ -z "$code" ] || { echo "$me msg verify: one code only ($me msg verify --help)" >&2; exit 2; }
                code="$1"; shift ;;
        esac
    done
    [ -n "$code" ] || { echo "Usage: $me msg verify <code> [--text-file f]  (the m_… or p_… in the message's label)" >&2; exit 2; }
    local q="code=$(urlencode "$code")"
    if [ "$have_text" = true ]; then
        q="$q&text_sha256=$(printf '%s' "$text" | _msg_verify_hashes "$code")"
    fi
    cli_request "/cli/msg/verify" "$q" | MV_JSON="$as_json" python3 -c '
import json, os, sys, time
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon answered: %s" % raw[:300], file=sys.stderr); sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err:
    code = err.get("code", "") if isinstance(err, dict) else ""
    hint = err.get("hint", "") if isinstance(err, dict) else str(err)
    print("Error: %s%s" % ((code + ": ") if code else "", hint), file=sys.stderr)
    sys.exit({"not_found": 4, "usage": 2, "forbidden": 3}.get(code, 1))
content = d.get("content")
if os.environ.get("MV_JSON") == "true":
    print(json.dumps(d, indent=2))
    sys.exit(1 if content == "mismatch" else 0)
s = d.get("sender") or {}
who = s.get("address") or s.get("handle") or s.get("label") or "?"
extra = []
if s.get("handle") and s.get("handle") != who: extra.append("workspace " + s["handle"])
if s.get("fingerprint"): extra.append("key " + s["fingerprint"])
print("code      %s (%s)" % (d.get("code"), d.get("kind")))
print("from      %s  via %s%s" % (who, d.get("via"), ("  [" + ", ".join(extra) + "]") if extra else ""))
if d.get("actor"): print("actor     %s" % d["actor"])
ob = d.get("onBehalfOf")
if ob: print("for       %s (app-asserted, not verified)" % ob.get("value"))
t = d.get("target") or {}
print("to        %s" % (t.get("token") or t.get("workspaceId") or "?"))
at = d.get("at") or 0
print("when      %s" % time.strftime("%Y-%m-%d %H:%M:%S %Z", time.localtime(at)))
print("typed in  %s" % ("yes" if d.get("delivered") else "no (the delivery did not reach the agent)"))
if d.get("badge") and d.get("badge") != d.get("code"): print("badge     %s" % d["badge"])
if d.get("landed") is not None: print("landed    %s" % json.dumps(d["landed"]))
print("content   %s: %s" % (content, d.get("contentHint", "")))
sys.exit(1 if content == "mismatch" else 0)
'
}
