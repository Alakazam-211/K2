# ── The widget dev loop (0.46.1 S3) ────────────────────────────────────
# A staged widget folder anywhere on disk goes to the daemon whole: code
# files as text, images and fonts as base64. The daemon never reads the
# path. Same rules as a widget folder: top-level files only, dotfiles and
# README/LICENSE skipped quietly, other files and subfolders skipped with a
# note, links refused.

# _zen_dir_body <dir> <out-file> <extra-json> <name-key>
#   Writes {...extra, files: [{name, text|base64}]} to <out-file>. With
#   <name-key> "widget" and no "widget" in extra, adds the manifest's id (or
#   the folder's own name); with "fallback", adds the folder's own name as
#   "fallback". Exit 2 on a usage problem (no folder, a link, too big).
_zen_dir_body() {
    ZD_DIR="$1" ZD_OUT="$2" ZD_EXTRA="${3:-}" ZD_NAME_KEY="${4:-}" python3 -c '
import base64, json, os, sys
d = os.environ["ZD_DIR"]
if not os.path.isdir(d):
    print("Error: no folder %s" % d, file=sys.stderr); sys.exit(2)
CODE = ("html", "js", "css", "json")
ASSET = ("png", "jpg", "jpeg", "gif", "webp", "woff2", "svg")
files, skipped, total = [], [], 0
for name in sorted(os.listdir(d)):
    p = os.path.join(d, name)
    low = name.lower()
    if low.startswith(".") or low.startswith("readme") or low == "license" or low.startswith("license."):
        continue
    if os.path.islink(p):
        print("Error: %s is a link; put the file itself in the folder" % p, file=sys.stderr); sys.exit(2)
    if os.path.isdir(p):
        skipped.append(name + "/"); continue
    ext = low.rsplit(".", 1)[-1] if "." in low else ""
    if ext in CODE:
        raw = open(p, "rb").read()
        try:
            files.append({"name": name, "text": raw.decode("utf-8")})
        except UnicodeDecodeError:
            print("Error: %s is not UTF-8 text" % p, file=sys.stderr); sys.exit(2)
    elif ext in ASSET:
        raw = open(p, "rb").read()
        files.append({"name": name, "base64": base64.b64encode(raw).decode("ascii")})
    else:
        skipped.append(name); continue
    total += len(raw)
    if total > 4 * 1024 * 1024:
        print("Error: %s is over 4 MB of widget files" % d, file=sys.stderr); sys.exit(2)
if skipped:
    print("note: skipped %s (a widget is one flat folder of .html .js .css .json, images and .woff2 fonts)" % ", ".join(skipped), file=sys.stderr)
if not files:
    print("Error: %s has no widget files (manifest.json, the entry .html, ...)" % d, file=sys.stderr); sys.exit(2)
body = json.loads(os.environ.get("ZD_EXTRA") or "{}")
body["files"] = files
own = os.path.basename(os.path.abspath(d))
key = os.environ.get("ZD_NAME_KEY")
if key == "widget" and not body.get("widget"):
    mid = None
    for f in files:
        if f["name"] == "manifest.json":
            try:
                mid = json.loads(f["text"]).get("id")
            except Exception:
                mid = None
    body["widget"] = mid if isinstance(mid, str) and mid else own
elif key == "fallback":
    body["fallback"] = own
with open(os.environ["ZD_OUT"], "w") as out:
    json.dump(body, out)
'
}

# POST a JSON body from a file, with the session passport inside a K2
# terminal (like _zen_session_post) and the owner token outside. For the
# folder routes, whose bodies can be megabytes.
_zen_session_post_file() {
    local endpoint="$1" file="$2" hook_sock="${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}" tok="$TOKEN"
    if [ -n "$hook_sock" ]; then
        tok="${SCOPED_TOKEN:-${K2_HOOK_TOKEN:-${K2SO_HOOK_TOKEN:-}}}"
    fi
    local _K2_CFG=""
    _k2_cfg_add url "${BASE_URL}${endpoint}?token=${tok}"
    _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 5 --max-time 120 \
        -H "Content-Type: application/json" --data-binary "@$file" 2>/dev/null
}

# _zen_dir_call <endpoint> <dir> <extra-json> <name-key>: build, post, clean up.
_zen_dir_call() {
    local tmp rc=0
    tmp="$(mktemp "${TMPDIR:-/tmp}/k2-zen-dir.XXXXXX")" || { echo "Error: can't make a temp file" >&2; exit 1; }
    _zen_dir_body "$2" "$tmp" "$3" "$4" || rc=$?
    if [ "$rc" -ne 0 ]; then rm -f "$tmp"; exit "$rc"; fi
    # A curl failure prints nothing: the printer then says the daemon didn't answer.
    _zen_session_post_file "$1" "$tmp" || true
    rm -f "$tmp"
}

_zen_help_validate() {
    cat <<'EOF'
Usage: k2 zen validate [--json] [--strict]
       k2 zen validate --garden <name|id> [--from <file>|-] [--json]
       k2 zen validate --theme <name> | --file <file> [--json]
       k2 zen validate --widget <name> [--types[=all]] [--sign] [--strict] [--json]
       k2 zen validate --dir <folder> [--as <name>] [--json]

Check Zen files without changing what Zen shows. Prints file:line:col:
message for each error (with a suggestion line) and warnings.

  (no target)            every file in ~/.k2/zen, and one line per widget:
                         where it is placed and its runtime state.
  --garden <g>           one Garden page as it is on disk.
  --garden <g> --from F  a STAGED Garden page (a file anywhere, - for stdin)
                         checked against that Garden. Nothing is written.
  --widget <name>        a widget folder as it is on disk: manifest, folder
                         rules, bundle, scripts (oxc parse + catalog lints)
                         and the runtime errors its saved version hit:
                         errors only when a Garden places it and they are
                         from the last 24 h; otherwise warnings.
  --dir <folder>         a STAGED widget folder anywhere on disk, checked
                         exactly like a widget folder. Nothing is written and
                         Zen needn't be set up. --as names it (default:
                         manifest.json's "id", else the folder's name).
  --types[=all]          --widget: also type-check its scripts (tsc-rs).
  --sign                 --widget: when clean, sign this version as made by
                         you (or this agent) for its Garden data calls. A
                         check never signs on its own (0.46.1); --no-sign is
                         accepted and does nothing. k2 zen widget sign <name>
                         signs without checking.
  --strict               exit 2 while a placed widget's saved version hasn't
                         run in a Zen window yet ("pending"), or while old
                         runtime errors are logged (not confirmed clean).
                         Without it, both exit 0 with a note.

The dev loop for a widget kept outside ~/.k2:
  k2 zen validate --dir ./board              check (writes nothing)
  k2 zen widget preview --dir ./board        try it on K2's preview Garden
  k2 zen widget preview --dir ./board --shot board.png --theme dark
                                             a headless PNG (writes nothing else)
  k2 zen widget put board --dir ./board      write it (you need the Garden's list)
  k2 zen widget errors board                 its runtime errors in Zen
  k2 zen widget errors board --clear         drop old ones validate ignores

Exit: 0 clean (or pending without --strict) · 1 errors · 2 usage, or pending
or old runtime errors with --strict · 3 Zen isn't set up, or the Garden is
locked.

New to Gardens? k2 zen guide.
  k2 zen guide [<topic>] [--json]    short pages; no daemon needed
EOF
}

_zen_help_widget_put() {
    cat <<'EOF'
Usage: k2 zen widget put <name> <file> --from <path>|- [--note "…"] [--json]
       k2 zen widget put [<name>] --dir <folder> [--prune] [--note "…"] [--json]
       k2 zen widget apply --from <folder> [<name>] [--prune] [--note "…"] [--json]

Change a custom widget (~/.k2/zen/widgets/<name>/) through K2, from anywhere
on disk. You never edit ~/.k2 yourself.

  <file> --from <path>   one code file (.html .js .css .json); "-" reads stdin.
                         Written even if the widget then has errors (the last
                         good version keeps running); the errors are printed.
  --dir <folder>         the whole folder in one checked write: code files,
                         images and .woff2 fonts. A folder with errors is
                         refused and NOTHING is written. <name> defaults to
                         manifest.json's "id", else the folder's own name.
                         Files the folder doesn't have are left alone.
  --prune                with --dir: also remove code files, images and fonts
                         the folder doesn't have (never README or dotfiles).
  --note "…"             one paragraph for the Garden's History.
  apply --from <folder>  the same as put --dir.

Who may write: the person (outside K2), or an agent on the list of every
Garden that places the widget (a draft placed nowhere is open). Any other
agent is refused (garden_access_needed, exit 3) and K2 asks the person (a
garden.access card); nothing is proposed. Sign what you wrote: k2 zen
validate --widget <name> --sign. K2's built-in widgets (k2:<name>) are never
written. k2 study garden-access.

Check first, write nothing:  k2 zen validate --dir <folder>
Try it on K2's preview page: k2 zen widget preview --dir <folder>

Exit: 0 written · 1 refused (errors, a bad file name) · 2 usage ·
3 Zen isn't set up / not on the Garden's list / locked · 4 no such widget.
EOF
}

cmd_zen_widget_put() {
    local as_json=false from="" note="" dir="" prune=false
    local -a pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --from) [ -n "${2:-}" ] || { echo "Error: --from needs a file (or - for stdin)" >&2; exit 2; }; from="$2"; shift 2 ;;
            --dir) [ -n "${2:-}" ] || { echo "Error: --dir needs a widget folder" >&2; exit 2; }; dir="$2"; shift 2 ;;
            --dir=*) dir="${1#*=}"; shift ;;
            --prune) prune=true; shift ;;
            --note) note="${2:-}"; shift 2 ;;
            -h|--help) _zen_help_widget_put; exit 0 ;;
            -*) echo "Error: unexpected '$1' (k2 zen widget put --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    if [ -n "$dir" ]; then
        [ -z "$from" ] || { echo "Error: --dir puts a whole folder; --from is for one file" >&2; exit 2; }
        [ "${#pos[@]}" -le 1 ] || { echo "Error: k2 zen widget put [<name>] --dir <folder> [--prune]" >&2; exit 2; }
        local extra raw
        extra=$(ZW="${pos[0]:-}" ZN="$note" ZP="$prune" python3 -c '
import json, os
b = {"prune": os.environ["ZP"] == "true"}
if os.environ.get("ZW"): b["widget"] = os.environ["ZW"]
if os.environ.get("ZN"): b["note"] = os.environ["ZN"]
print(json.dumps(b))')
        raw=$(_zen_dir_call "/cli/zen/widget/folder/put" "$dir" "$extra" widget) || exit $?
        _zen_agent_print folder "$as_json" "$raw"
        return
    fi
    [ "$prune" = false ] || { echo "Error: --prune is for k2 zen widget put --dir" >&2; exit 2; }
    [ "${#pos[@]}" -eq 2 ] && [ -n "$from" ] || { echo "Error: k2 zen widget put <widget> <file.html|.js|.css|.json> --from <path>|- [--note \"…\"] (or --dir <folder>; k2 zen widget put --help)" >&2; exit 2; }
    local body raw
    # 0.46.5 (vs-live L5): the text goes as UTF-8, never \uXXXX escapes, so a
    # 1 MB file of CJK copy or emoji stays inside the 1,280 KB body cap.
    body="$(_zen_read_text "$from" | ZW="${pos[0]}" ZF="${pos[1]}" ZN="$note" python3 -c '
import json, os, sys
try:
    text = sys.stdin.buffer.read().decode("utf-8")
except UnicodeDecodeError:
    sys.stderr.write("Error: %s is not UTF-8 text; a widget code file is UTF-8\n" % os.environ["ZF"])
    sys.exit(2)
b = {"widget": os.environ["ZW"], "file": os.environ["ZF"], "text": text}
if os.environ.get("ZN"): b["note"] = os.environ["ZN"]
sys.stdout.buffer.write(json.dumps(b, ensure_ascii=False).encode("utf-8") + b"\n")')" || exit 2
    raw=$(_zen_session_post "/cli/zen/widget/put" "$body")
    _zen_agent_print put "$as_json" "$raw"
}

_zen_help_widget_delete() {
    cat <<'EOF'
Usage: k2 zen widget delete <name> [--json]

Delete a custom widget (~/.k2/zen/widgets/<name>/) that no Garden places.
While any Garden places it, K2 refuses and names those Gardens: take it off
each Garden's page first. Nothing is lost: its code is kept as a history
snapshot and the whole folder (images and fonts too) beside it, and the
answer prints the undo:  k2 zen reset --widget <name> --to <snapshot>.
Its data and its last good version stay. K2's built-in widgets (k2:<name>)
can't be deleted.

Who may delete: the person (outside K2), or an agent with "Manage Gardens
and data sources" (with Garden access on: as for put, a widget placed
nowhere is open). Any other agent is refused (person_only, exit 3).

Exit: 0 deleted · 1 refused (still placed) · 2 usage ·
3 Zen isn't set up / not allowed · 4 no such widget.
EOF
}

# k2 zen widget delete <name> (0.46.5): POST /cli/zen/widget/delete; the
# daemon refuses a placed widget and keeps the undo point.
cmd_zen_widget_delete() {
    local as_json=false
    local -a pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -h|--help) _zen_help_widget_delete; exit 0 ;;
            -*) echo "Error: unexpected '$1' (k2 zen widget delete --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    [ "${#pos[@]}" -eq 1 ] && [ -n "${pos[0]}" ] || { echo "Error: k2 zen widget delete <name> [--json]" >&2; exit 2; }
    local body raw
    body="$(ZW="${pos[0]}" python3 -c 'import json, os; print(json.dumps({"widget": os.environ["ZW"]}))')"
    raw=$(_zen_session_post "/cli/zen/widget/delete" "$body")
    _zen_agent_print delete "$as_json" "$raw"
}

# k2 zen widget apply --from <folder> [<name>] = k2 zen widget put [<name>] --dir <folder>.
cmd_zen_widget_apply() {
    local -a args=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --from) [ -n "${2:-}" ] || { echo "Error: --from needs a widget folder" >&2; exit 2; }; args+=(--dir "$2"); shift 2 ;;
            --from=*) args+=(--dir "${1#*=}"); shift ;;
            --dir) echo "Error: k2 zen widget apply takes --from <folder> (or use k2 zen widget put --dir)" >&2; exit 2 ;;
            -h|--help) _zen_help_widget_put; exit 0 ;;
            *) args+=("$1"); shift ;;
        esac
    done
    case " ${args[*]:-} " in
        *" --dir "*) ;;
        *) echo "Error: k2 zen widget apply --from <folder> [<name>] [--prune]" >&2; exit 2 ;;
    esac
    cmd_zen_widget_put "${args[@]}"
}

_zen_help_widget_preview() {
    cat <<'EOF'
Usage: k2 zen widget preview --dir <folder> [<name>] [--config key=value]... [--json]
       k2 zen widget preview --dir <folder> [<name>] --shot <png> [--theme dark|light]
                             [--width N] [--height N] [--zen-theme <name>]
                             [--data <file.json>] [--config key=value]... [--json]
       k2 zen widget preview --clear [--json]

Try a staged widget folder in Zen without touching your Gardens. K2 writes it
as widgets/preview-<name>/ (its manifest id renamed to match) and places it,
alone, on K2's own "Widget preview" Garden (made the first time). Each preview
replaces the last. Open Zen and switch to "Widget preview" to see it.

  --dir <folder>         the staged folder (checked first; errors refuse it).
  <name>                 defaults to manifest.json's "id", else the folder name.
  --config key=value     the placement's config, repeatable. A card that reads
                         data sources takes them here, for example
                         --config sources=fleet,k2-ops-boxes
  --clear                remove the preview Garden and every preview-* widget.

The preview runs as its own widget (preview-<name>): it has its own empty
store, and its runtime errors are k2 zen widget errors preview-<name>.
Who may preview: the person, or any agent in a K2 terminal (it never
touches the person's Gardens). Check without writing: k2 zen validate --dir.

--shot <png>: a picture instead, with nobody at a Zen window. K2 checks and
bundles the folder exactly as Zen would, puts it in K2's real widget frame
on a Garden canvas, answers its calls from a stand-in bridge with sample
data, and renders it headless in WebKit (the engine of K2's desktop app).
It writes only the PNG: no Garden, no widget folder, no store. Any local
agent may shoot, and Zen needn't be set up.
  --theme dark|light     the Garden's scheme (default: the theme's own, else
                         light). The card follows the Garden, never the OS.
  --zen-theme <name>     a built-in Zen theme: basic (default), paper,
                         midnight, haunted, glass.
  --width N --height N   the card's size in CSS pixels (default 420x360;
                         120 to 2400). The PNG is drawn at 2x.
  --data <file.json>     your own data instead of the samples: one object,
                         keys "<verb> <first arg>" or "<verb>" (for example
                         "sources.subscribe fleet": a card view, "agents.list":
                         rows) and "store": {key: value} preloaded.
  --libs <dir>           K2's zen-lib folder, for a widget with
                         requires.libs (found by itself in a K2 checkout).
  --wait-ms N            settle time after the widget says ready (default 400).
Needs Node.js and Playwright's WebKit:
  npm install -g playwright && npx playwright install webkit

Exit: 0 previewed, cleared or shot (and the widget said ready) · 1 refused
(errors), or the shot widget threw or never said ready (the PNG is still
written) · 2 usage · 3 needs a person, Zen isn't set up, or --shot has no
Node/Playwright WebKit.
EOF
}

# k2 zen widget preview --dir <folder> --shot <png> (0.46.3): the daemon
# answers the shot page (POST /cli/zen/widget/shot, writes nothing); the
# runner next to this script (k2-widget-shot.mjs) renders it in Playwright's
# WebKit and writes the PNG.
# _zen_widget_shot <json> <dir> <name> <png> <theme> <zen-theme> <width> <height> <data> <libs> <wait-ms> [key=value...]
_zen_widget_shot() {
    local as_json="$1" dir="$2" name="$3" png="$4" theme="$5" zen_theme="$6" width="$7" height="$8" data="$9" libs="${10}" wait_ms="${11}"
    shift 11
    case "$theme" in ""|dark|light) ;; *) echo "Error: --theme is dark or light" >&2; exit 2 ;; esac
    case "$png" in *.png) ;; *) echo "Error: --shot writes a .png file (got $png)" >&2; exit 2 ;; esac
    for n in "$width" "$height" "$wait_ms"; do
        case "$n" in ""|*[!0-9]*) [ -z "$n" ] || { echo "Error: --width, --height and --wait-ms are whole numbers" >&2; exit 2; } ;; esac
    done
    [ -z "$data" ] || [ -f "$data" ] || { echo "Error: no --data file $data" >&2; exit 2; }
    local self runner
    self="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$0")"
    runner="$(dirname "$self")/k2-widget-shot.mjs"
    [ -f "$runner" ] || { echo "Error: this K2 has no shot runner ($runner)" >&2; exit 3; }
    command -v node >/dev/null 2>&1 || { echo "Error: --shot needs Node.js and Playwright's WebKit: install Node, then npm install -g playwright && npx playwright install webkit" >&2; exit 3; }
    local extra
    extra=$(ZW="$name" ZT="$theme" ZZ="$zen_theme" ZWI="$width" ZH="$height" ZD="$data" python3 -c '
import json, os, sys
b = {"config": {}}
e = os.environ
if e.get("ZW"): b["widget"] = e["ZW"]
if e.get("ZT"): b["scheme"] = e["ZT"]
if e.get("ZZ"): b["theme"] = e["ZZ"]
if e.get("ZWI"): b["width"] = int(e["ZWI"])
if e.get("ZH"): b["height"] = int(e["ZH"])
if e.get("ZD"):
    try:
        d = json.load(open(e["ZD"]))
    except Exception as x:
        print("Error: --data %s is not JSON: %s" % (e["ZD"], x), file=sys.stderr); sys.exit(2)
    if not isinstance(d, dict):
        print("Error: --data is one JSON object (keys: \"<verb>\" or \"<verb> <address>\", and \"store\")", file=sys.stderr); sys.exit(2)
    # 0.46.6 (prd-garden-board-needs-v1 GB18): a blobs/ folder beside the
    # fixture file answers k2.blobs.get (each file by its sha256 ref).
    bdir = os.path.join(os.path.dirname(os.path.abspath(e["ZD"])), "blobs")
    if os.path.isdir(bdir):
        import base64, hashlib, mimetypes
        blobs = {}
        for fn in sorted(os.listdir(bdir)):
            p = os.path.join(bdir, fn)
            if not os.path.isfile(p) or fn.startswith("."):
                continue
            raw = open(p, "rb").read()
            ref = "k2blob:sha256-" + hashlib.sha256(raw).hexdigest()
            blobs[ref] = {"name": fn, "mime": mimetypes.guess_type(fn)[0] or "application/octet-stream", "base64": base64.b64encode(raw).decode()}
        d["blobs"] = blobs
    b["data"] = d
for kv in sys.argv[1:]:
    if "=" not in kv:
        print("Error: --config takes key=value, not %r" % kv, file=sys.stderr); sys.exit(2)
    k, v = kv.split("=", 1)
    b["config"][k.strip()] = v
print(json.dumps(b))' "$@") || exit $?
    local body answer rc=0
    body="$(mktemp "${TMPDIR:-/tmp}/k2-zen-shot-body.XXXXXX")" || { echo "Error: can't make a temp file" >&2; exit 1; }
    answer="$(mktemp "${TMPDIR:-/tmp}/k2-zen-shot-page.XXXXXX")" || { rm -f "$body"; echo "Error: can't make a temp file" >&2; exit 1; }
    _zen_dir_body "$dir" "$body" "$extra" fallback || rc=$?
    if [ "$rc" -ne 0 ]; then rm -f "$body" "$answer"; exit "$rc"; fi
    _zen_session_post_file "/cli/zen/widget/shot" "$body" > "$answer" || true
    rm -f "$body"
    # The daemon's answer: refused, a folder with errors, or the page.
    ZA="$answer" ZJ="$as_json" python3 -c '
import json, os, sys
raw = open(os.environ["ZA"]).read()
if not raw.strip():
    print("Error: no response from the K2 daemon on this computer", file=sys.stderr); sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    print("Error: the daemon answered something that is not JSON: %s" % raw[:200], file=sys.stderr); sys.exit(1)
err = d.get("error")
if err:
    if os.environ["ZJ"] == "true": print(json.dumps(d, indent=2))
    else: print("Error: %s: %s" % (err, d.get("message") or ""), file=sys.stderr)
    sys.exit(3 if err in ("person_only", "zen_local_only", "owner_only") else 1)
if d.get("ok") is not True:
    if os.environ["ZJ"] == "true": print(json.dumps(d, indent=2))
    else:
        print("The folder has errors; nothing was shot:", file=sys.stderr)
        for e in d.get("errors") or []:
            print("%s:%s:%s: %s" % (e.get("file"), e.get("line"), e.get("col"), e.get("message")), file=sys.stderr)
    sys.exit(1)
' || { rc=$?; rm -f "$answer"; exit "$rc"; }
    local -a rargs=(--page "$answer" --out "$png")
    [ -z "$libs" ] || rargs+=(--libs "$libs")
    [ -z "$wait_ms" ] || rargs+=(--wait-ms "$wait_ms")
    local out
    out="$(node "$runner" "${rargs[@]}")" || rc=$?
    rm -f "$answer"
    [ -n "$out" ] || exit "${rc:-1}"
    if [ "$as_json" = true ]; then
        printf '%s\n' "$out"
    else
        ZO="$out" python3 -c '
import json, os
d = json.loads(os.environ["ZO"])
print("Shot %s (%s, %sx%s) in %s ms" % (d["png"], d["scheme"], d["width"], d["height"], d["ms"]))
if not d["ready"]:
    print("The widget never said ready (k2.ready()): in Zen it would be stopped as not started.")
for e in d["errors"]:
    print("error: %s" % e)
if d["refusals"]:
    print("refused: %s" % ", ".join(d["refusals"]))
print("The data is the K2 samples (alpha, bravo, charlie; a sample agent and Thread) unless --data <file.json> gave your own.")
'
    fi
    exit "$rc"
}

cmd_zen_widget_preview() {
    local as_json=false dir="" clear=false raw shot="" theme="" zen_theme="" width="" height="" data="" libs="" wait_ms=""
    local -a pos=() cfg=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --dir) [ -n "${2:-}" ] || { echo "Error: --dir needs a widget folder" >&2; exit 2; }; dir="$2"; shift 2 ;;
            --dir=*) dir="${1#*=}"; shift ;;
            --config) [ -n "${2:-}" ] || { echo "Error: --config needs key=value" >&2; exit 2; }; cfg+=("$2"); shift 2 ;;
            --config=*) cfg+=("${1#*=}"); shift ;;
            --clear) clear=true; shift ;;
            --shot) [ -n "${2:-}" ] || { echo "Error: --shot needs a .png path" >&2; exit 2; }; shot="$2"; shift 2 ;;
            --theme) [ -n "${2:-}" ] || { echo "Error: --theme needs dark or light" >&2; exit 2; }; theme="$2"; shift 2 ;;
            --zen-theme) [ -n "${2:-}" ] || { echo "Error: --zen-theme needs a built-in theme name" >&2; exit 2; }; zen_theme="$2"; shift 2 ;;
            --width) [ -n "${2:-}" ] || { echo "Error: --width needs pixels" >&2; exit 2; }; width="$2"; shift 2 ;;
            --height) [ -n "${2:-}" ] || { echo "Error: --height needs pixels" >&2; exit 2; }; height="$2"; shift 2 ;;
            --data) [ -n "${2:-}" ] || { echo "Error: --data needs a JSON file" >&2; exit 2; }; data="$2"; shift 2 ;;
            --libs) [ -n "${2:-}" ] || { echo "Error: --libs needs K2's zen-lib folder" >&2; exit 2; }; libs="$2"; shift 2 ;;
            --wait-ms) [ -n "${2:-}" ] || { echo "Error: --wait-ms needs milliseconds" >&2; exit 2; }; wait_ms="$2"; shift 2 ;;
            -h|--help) _zen_help_widget_preview; exit 0 ;;
            -*) echo "Error: unexpected '$1' (k2 zen widget preview --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    if [ -n "$shot" ] || [ -n "$theme$zen_theme$width$height$data$libs$wait_ms" ]; then
        [ -n "$shot" ] || { echo "Error: --theme, --zen-theme, --width, --height, --data, --libs and --wait-ms go with --shot <png>" >&2; exit 2; }
        [ "$clear" = false ] || { echo "Error: --shot takes a staged folder, not --clear" >&2; exit 2; }
        [ -n "$dir" ] && [ "${#pos[@]}" -le 1 ] || { echo "Error: k2 zen widget preview --dir <folder> [<name>] --shot <png> [--theme dark|light] [--width N] [--height N]" >&2; exit 2; }
        _zen_widget_shot "$as_json" "$dir" "${pos[0]:-}" "$shot" "$theme" "$zen_theme" "$width" "$height" "$data" "$libs" "$wait_ms" ${cfg[@]+"${cfg[@]}"}
        return
    fi
    if [ "$clear" = true ]; then
        [ -z "$dir" ] && [ "${#pos[@]}" -eq 0 ] && [ "${#cfg[@]}" -eq 0 ] || { echo "Error: k2 zen widget preview --clear takes only --json" >&2; exit 2; }
        raw=$(_zen_session_post "/cli/zen/widget/preview" '{"clear":true}')
        _zen_agent_print preview "$as_json" "$raw"
        return
    fi
    [ -n "$dir" ] && [ "${#pos[@]}" -le 1 ] || { echo "Error: k2 zen widget preview --dir <folder> [<name>] [--config key=value]..." >&2; exit 2; }
    local extra
    extra=$(ZW="${pos[0]:-}" python3 -c '
import json, os, sys
b = {"config": {}}
if os.environ.get("ZW"): b["widget"] = os.environ["ZW"]
for kv in sys.argv[1:]:
    if "=" not in kv:
        print("Error: --config takes key=value, not %r" % kv, file=sys.stderr); sys.exit(2)
    k, v = kv.split("=", 1)
    b["config"][k.strip()] = v
print(json.dumps(b))' ${cfg[@]+"${cfg[@]}"}) || exit $?
    raw=$(_zen_dir_call "/cli/zen/widget/preview" "$dir" "$extra" fallback) || exit $?
    _zen_agent_print preview "$as_json" "$raw"
}

