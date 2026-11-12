# ── Agents run Gardens end to end (0.46.0, migration 0181) ─────────────
# k2 zen garden put / widget put write a user Garden page or one widget code
# file THROUGH the daemon (checked first; a bad page is refused, not saved;
# K2's built-in widgets and defaults are never written). Inside a K2
# terminal they carry the session passport: an agent on the Garden's list
# writes; any other is refused and K2 asks the person (Garden access,
# 0.46.3; a K2 older than 0.46.3 made it a proposal). Outside K2, the
# owner writes.

# Print a put / proposals answer. $1 view, $2 "true" for JSON, $3 raw.
# Exit: 0 ok · 1 refused/conflict · 3 needs a person or locked · 4 not found.
_zen_agent_print() {
    ZEN_VIEW="$1" ZEN_JSON="$2" ZEN_RAW="$3" python3 -c '
import json, os, sys
view = os.environ["ZEN_VIEW"]
raw = os.environ.get("ZEN_RAW") or ""
if not raw.strip():
    print("Error: no response from the K2 daemon on this computer", file=sys.stderr); sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    print("Error: the daemon answered something that is not JSON: %s" % raw[:200], file=sys.stderr); sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if os.environ.get("ZEN_JSON") == "true":
    print(json.dumps(d, indent=2))
if err:
    if os.environ.get("ZEN_JSON") != "true":
        print("Error: %s: %s" % (err, d.get("message") or ""), file=sys.stderr)
    if err in ("person_only", "zen_local_only", "garden_locked", "owner_only", "zen_not_set_up", "garden_access_needed", "garden_delete_person", "session_gone"):
        sys.exit(3)
    if err in ("proposal_not_found", "unknown_garden", "unknown_widget", "not_found"):
        sys.exit(4)
    sys.exit(1)
if os.environ.get("ZEN_JSON") == "true":
    sys.exit(0)

def human(n):
    # K2\x27s limit words: 1 KB = 1,024 bytes ("740 KB", "1 MB", "3.5 MB").
    if n >= 1048576:
        t = (n * 10 + 524288) // 1048576
        return "%d MB" % (t // 10) if t % 10 == 0 else "%d.%d MB" % (t // 10, t % 10)
    if n >= 1024:
        return "%d KB" % ((n + 512) // 1024)
    return "%d bytes" % n

def findings(v):
    # 0.46.5 WS10/WS11: errors, then warnings (k2 zen validate\x27s format),
    # then the size line. Warnings never change the exit code.
    def line(e, warn):
        s = "%s:%s:%s: %s%s" % (e.get("file"), e.get("line"), e.get("col"), "warning: " if warn else "", e.get("message"))
        if e.get("suggestion"):
            s += "\n  suggestion: %s" % e["suggestion"]
        return s
    for e in v.get("errors") or []:
        print(line(e, False))
    for w in v.get("warnings") or []:
        print(line(w, True))
    s = v.get("size")
    if isinstance(s, dict) and isinstance(s.get("code"), int):
        large = " (large)" if s["code"] > s.get("codeWarnAt", s["code"]) else ""
        libs = human(s.get("libs") or 0) + (" + CDN files" if s.get("libsUnknown") else "")
        print("size: code %s of %s%s, page %s of %s, libraries %s (outside the limit)" % (
            human(s["code"]), human(s.get("codeMax") or 0), large, human(s.get("page") or 0), human(s.get("pageMax") or 0), libs))

if view == "put":
    if d.get("proposed"):
        p = d.get("proposal") or {}
        print("Proposed %s (%s %s): nothing written yet." % (p.get("id"), p.get("kind"), p.get("target")))
        print(d.get("message") or "")
    else:
        print("Wrote %s%s" % (d.get("file"), "" if d.get("changed", True) else " (no change)"))
        if d.get("keepAsMine"):
            kc = d.get("keepCard") or {}
            print("This widget was your person’s; its new version is not theirs yet, so it runs without its data until they keep it. " + ("K2 put a Keep as mine card on their Thread with you." if kc.get("card") else "K2 sent them an inbox note (Settings → Gardens → the Garden → Local Data → It’s mine)."))
        if d.get("snapshot"):
            print("Previous version kept: %s (k2 zen reset --to %s)" % (d["snapshot"], d["snapshot"]))
        findings(d.get("validate") or {})
elif view == "delete":
    print("Deleted %s (kept in history as %s)." % (d.get("file"), d.get("snapshot")))
    print("Undo: %s" % d.get("undo"))
elif view == "folder":
    if d.get("proposed"):
        for p in d.get("proposals") or []:
            if p.get("kind") == "folder":
                print("Proposed %s (widgets/%s/: %s): nothing written yet." % (p.get("id"), p.get("target"), ", ".join(p.get("files") or [])))
            else:
                print("Proposed %s (widgets/%s/%s)" % (p.get("id"), p.get("target"), p.get("file")))
        print(d.get("message") or "")
    elif d.get("proposed") is False:
        print("No change: widgets/%s matches the folder." % d.get("widget"))
    else:
        w = d.get("written") or []
        print("Wrote %s: %s" % (d.get("file"), ", ".join(w) if w else "no change"))
        if d.get("keepAsMine"):
            kc = d.get("keepCard") or {}
            print("This widget was your person’s; its new version is not theirs yet, so it runs without its data until they keep it. " + ("K2 put a Keep as mine card on their Thread with you." if kc.get("card") else "K2 sent them an inbox note (Settings → Gardens → the Garden → Local Data → It’s mine)."))
        if d.get("removed"):
            print("Removed: %s" % ", ".join(d["removed"]))
        if d.get("kept"):
            print("Left alone (not in the folder; --prune removes them): %s" % ", ".join(d["kept"]))
        v = d.get("validate") or {}
        findings(v)
        rt = v.get("runtime") if isinstance(v.get("runtime"), dict) else None
        if rt and rt.get("note"):
            print("runtime: %s" % rt["note"])
elif view == "preview":
    if "removed" in d and "preview" not in d:
        print("Cleared the preview%s%s" % (
            " Garden" if d.get("garden") else "",
            (" and " + ", ".join("widgets/%s" % r for r in d["removed"])) if d.get("removed") else ""))
    else:
        g = d.get("garden") or {}
        print("Previewing %s as widgets/%s on the \"%s\" Garden (%s)" % (d.get("widget"), d.get("preview"), g.get("name"), g.get("id")))
        findings(d.get("validate") or {})
        print(d.get("open") or "")
elif view == "list":
    rows = d.get("proposals") or []
    if not rows:
        print("No Garden proposals waiting."); sys.exit(0)
    for p in rows:
        if p.get("kind") == "folder":
            what = "widgets/%s/ (%d files)" % (p["target"], len(p.get("files") or []))
        elif p.get("file"):
            what = "widgets/%s/%s" % (p["target"], p["file"])
        else:
            what = "gardens/%s.toml" % p["target"]
        print("%s  %-9s %-36s by %s  %s B%s" % (p["id"], p["status"], what, p.get("madeBy"), p.get("bytes"), ("  — " + p["note"]) if p.get("note") else ""))
    print()
    print("Review: k2 zen proposals show <id> · approve <id> · reject <id>")
elif view == "show":
    p = d.get("proposal") or {}
    if p.get("kind") == "folder":
        stale = p.get("staleFiles") or []
        print("%s  %s  %s  by %s%s" % (p.get("id"), p.get("status"), p.get("path"), p.get("madeBy"), ("  (STALE: changed since: %s)" % ", ".join(stale)) if stale else ""))
        if p.get("note"): print("note: %s" % p["note"])
        print("One proposal for the folder: approving writes every file below at once (%s other staged file(s) stay as they are)." % p.get("unchanged", 0))
        for f in p.get("content") or []:
            print("--- %s (%s) ---" % (f.get("name"), "new file" if not f.get("base") else "changed"))
            print(f.get("text") or "")
    else:
        print("%s  %s  %s  by %s%s" % (p.get("id"), p.get("status"), p.get("path"), p.get("madeBy"), "  (STALE: the file changed since)" if p.get("stale") else ""))
        if p.get("note"): print("note: %s" % p["note"])
        print("--- proposed content ---")
        print(p.get("content") or "")
elif view == "decide":
    print("%s: %s" % (d.get("id"), d.get("status")))
    if d.get("keepAsMine"):
        kc = d.get("keepCard") or {}
        if kc.get("card"):
            print("The widget’s new version is not signed as yours yet: tap Keep as mine on the card in your Thread with %s." % (kc.get("addr") or "the agent"))
        else:
            print("The widget’s new version is not signed as yours, so it runs without its data. Keep it: Settings → Gardens → the Garden → Local Data → %s → It’s mine (or k2 zen widget claim %s outside K2)." % (d.get("target"), d.get("target")))
'
}

# Read a put body: --from <file> or - (stdin).
_zen_read_text() {
    local src="$1"
    if [ "$src" = "-" ]; then
        cat
    else
        [ -f "$src" ] || { echo "Error: no file $src" >&2; exit 2; }
        cat "$src"
    fi
}

cmd_zen_garden_put() {
    local as_json=false from="" note="" garden=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --from) [ -n "${2:-}" ] || { echo "Error: --from needs a file (or - for stdin)" >&2; exit 2; }; from="$2"; shift 2 ;;
            --note) note="${2:-}"; shift 2 ;;
            -h|--help) cmd_help_zen; exit 0 ;;
            -*) [ "$1" = "-" ] && { from="-"; shift; continue; }; echo "Error: unexpected '$1' (see k2 zen --help)" >&2; exit 2 ;;
            *) [ -z "$garden" ] || { echo "Error: k2 zen garden put <garden> --from <file>|-" >&2; exit 2; }; garden="$1"; shift ;;
        esac
    done
    [ -n "$garden" ] && [ -n "$from" ] || { echo "Error: k2 zen garden put <garden> --from <file>|- [--note \"…\"]" >&2; exit 2; }
    local body raw
    body="$(_zen_read_text "$from" | ZG="$garden" ZN="$note" python3 -c '
import json, os, sys
b = {"garden": os.environ["ZG"], "text": sys.stdin.read()}
if os.environ.get("ZN"): b["note"] = os.environ["ZN"]
print(json.dumps(b))')"
    raw=$(_zen_session_post "/cli/zen/garden/put" "$body")
    _zen_agent_print put "$as_json" "$raw"
}

