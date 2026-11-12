# ── k2 changes — the Changes tab from the CLI (prd-changes-tab-v1, C4) ──
# Same daemon routes as the desktop Changes tab (/cli/git/snapshot,
# diff-file, stage, unstage, stage-all, commit, discard). Inside a K2
# terminal they go over the cell socket with the session passport, and
# the daemon scopes them to that agent's own workspace (leaving out
# `path` = the workspace itself). Outside K2 the owner token picks any
# workspace with --workspace. Daemon refusals exit 3 with its code.

_changes_usage() {
    cat <<'EOF'
k2 changes — review, stage and commit this workspace's git changes
(the Changes tab, from the CLI)

  k2 changes [--json] [--base <ref>]          the change list (staged, unstaged,
                                              untracked, conflicted); --base = vs a branch
  k2 changes diff [<path>...] [--staged|--unstaged] [--base <ref>] [-U <n>] [--json]
                                              unified diff text. Default: everything not
                                              committed, vs HEAD (untracked files included)
  k2 changes stage <path>... | --all          stage files (git add)
  k2 changes unstage <path>...                unstage files (back to HEAD in the index)
  k2 changes discard <path>... [--staged] [--yes]
                                              throw away edits. Every copy that would be
                                              lost goes to the Trash first; untracked files
                                              go to the Trash. Asks on a terminal; --yes is
                                              required when not on a terminal. --staged also
                                              throws away staged edits (restores from HEAD)
  k2 changes commit -m "message" [--all]      commit what is staged (--all stages
                                              everything first). Author = this server's
                                              git user.name / user.email

  k2 changes notes [list|add|resolve|reopen|edit|delete|send]
                                              review notes on changed lines; agents
                                              list and resolve theirs (k2 changes notes --help)
  k2 changes open                             open the Changes tab in the K2 window that
                                              shows this workspace
  k2 changes init                             make the workspace root a git repository

Every command takes --workspace <path> (default: the git checkout you are in,
else this workspace) and --json (the daemon's JSON as is).
Agents (inside a K2 terminal) can only use their own workspace and folders
inside it (e.g. .worktrees/<branch>); anything else is refused (forbidden).
Paths are relative to where you run the command, or to the repo root.

Exit: 0 ok · 1 daemon unreachable, or a discard left a file untouched ·
2 bad usage (incl. discard without --yes off a terminal) · 3 the daemon
refused (code on stderr: bad_path, not_a_repo, bad_ref, not_found,
conflicted, forbidden, nothing_staged, …). See `k2 study changes`.
EOF
}

_changes_split() {
    local raw="${1:-}"
    _CH_STATUS="${raw##*$'\n'}"
    _CH_BODY="${raw%$'\n'*}"
    [ "$_CH_STATUS" = "$raw" ] && _CH_BODY=""
    [ -z "$_CH_STATUS" ] && _CH_STATUS="000"
    return 0
}

# _changes_err STATUS BODY → one stderr report. Sets _CH_ERR_RC (1 when the
# daemon didn't answer, else 3). Does not exit (diff keeps going).
_changes_err() {
    local status="$1" body="$2"
    _CH_ERR_RC=3
    case "$status" in 000) _CH_ERR_RC=1 ;; esac
    printf '%s' "$body" | python3 -c '
import json, sys
status, as_json = sys.argv[1], sys.argv[2] == "1"
raw = sys.stdin.read()
by_status = {"400": "bad_request", "401": "unauthorized", "403": "forbidden", "404": "not_found",
             "405": "method_not_allowed", "409": "conflicted", "000": "daemon_unreachable"}
code = by_status.get(status, "error")
hint = ""
try:
    v = json.loads(raw)
except Exception:
    v = None
if isinstance(v, dict):
    e = v.get("error")
    if isinstance(e, dict):
        code = e.get("code") or code
        hint = e.get("hint") or e.get("message") or ""
    else:
        hint = str(e) if e is not None else ""
        code = v.get("code") or code
elif raw.strip():
    hint = raw.strip()[:300]
if status == "000":
    hint = "no answer from the K2 daemon"
if not hint:
    hint = "the daemon refused (HTTP %s)" % status
if not as_json:
    print("Error: %s" % hint, file=sys.stderr)
print(json.dumps({"error": {"code": code, "hint": hint, "status": int(status) if status.isdigit() else 0}}), file=sys.stderr)
' "$status" "${_CH_JSON:-0}"
}

# _changes_die CODE HINT RC → a CLI-side refusal in the same shape.
_changes_die() {
    local code="$1" hint="$2" rc="${3:-3}"
    [ "${_CH_JSON:-0}" = "1" ] || echo "Error: $hint" >&2
    python3 -c 'import json,sys; print(json.dumps({"error": {"code": sys.argv[1], "hint": sys.argv[2]}}), file=sys.stderr)' "$code" "$hint"
    exit "$rc"
}

# _changes_call METHOD ROUTE PAYLOAD → _CH_STATUS/_CH_BODY; exits on non-200.
_changes_call() {
    _changes_split "$(cli_send_json "$1" "$2" "${3:-}")"
    if [ "$_CH_STATUS" != "200" ]; then
        _changes_err "$_CH_STATUS" "$_CH_BODY"
        exit "$_CH_ERR_RC"
    fi
}

# Inside a K2 terminal with a passport (the cell socket answers for us).
_changes_in_cell() {
    ! _cli_skip_uds && [ -n "$(_hook_sock)" ] && [ -n "${SCOPED_TOKEN:-}" ]
}

# _changes_target [<--workspace value>] → _CH_PATH (the `path` to send;
# empty = the passport's own workspace) and _CH_DIR (that folder, physical,
# for relative file arguments).
_changes_target() {
    local ws="${1:-}" top="" top_p="" proj_p="" resolved=""
    _CH_PATH=""
    _CH_DIR=""
    if [ -n "$ws" ]; then
        if [ -d "$ws" ]; then
            _CH_PATH="$(cd "$ws" && pwd)"
            _CH_DIR="$(cd "$ws" && pwd -P)"
            return 0
        fi
        # A workspace name or id: ask the daemon for its folder.
        resolved="$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$ws")" 2>/dev/null | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read() or "{}")
except Exception:
    d = {}
print((d.get("path") or d.get("projectPath") or d.get("project_path") or "") if isinstance(d, dict) else "")
' 2>/dev/null || true)"
        _CH_PATH="${resolved:-$ws}"
        _CH_DIR="$_CH_PATH"
        return 0
    fi
    proj_p="$(cd "$PROJECT" 2>/dev/null && pwd -P || true)"
    top=""
    _k2_git_usable && top="$(git -C "$PWD" rev-parse --show-toplevel 2>/dev/null || true)"
    [ -n "$top" ] && top_p="$(cd "$top" 2>/dev/null && pwd -P || true)"
    # Inside a checkout under this workspace (e.g. .worktrees/<branch>): that checkout.
    if [ -n "$top_p" ] && [ -n "$proj_p" ] && [[ "$top_p" == "$proj_p"/* ]]; then
        _CH_PATH="$top"
        _CH_DIR="$top_p"
        return 0
    fi
    _CH_DIR="${proj_p:-$PROJECT}"
    if ! _changes_in_cell; then
        _CH_PATH="$PROJECT"
    fi
}

# _changes_files ARG... → _CH_FILES (array). A relative path is taken from
# where you stand when that is inside the workspace (sent absolute; the
# daemon makes it repo-relative), else passed as given (repo-relative).
_changes_files() {
    _CH_FILES=()
    local f
    while IFS= read -r -d '' f; do
        _CH_FILES+=("$f")
    done < <(python3 - "$(pwd -P)" "$_CH_DIR" "$@" <<'PY'
import os, sys
cwd, base, args = sys.argv[1], sys.argv[2], sys.argv[3:]
inside = bool(base) and (cwd == base or cwd.startswith(base.rstrip("/") + "/"))
for a in args:
    out = a
    if not a.startswith("/") and inside:
        out = os.path.normpath(os.path.join(cwd, a))
        if a.endswith("/") and not out.endswith("/"):
            out += "/"
    sys.stdout.write(out + "\0")
PY
)
}

# _changes_body KIND [args] → the JSON body for a POST (path left out when
# _CH_PATH is empty: the passport's own workspace).
_changes_body() {
    python3 - "$_CH_PATH" "$@" <<'PY'
import json, sys
path, kind, rest = sys.argv[1], sys.argv[2], sys.argv[3:]
d = {}
if path:
    d["path"] = path
if kind == "file":
    d["filePath"] = rest[0]
elif kind == "commit":
    d["message"] = rest[0]
elif kind == "discard":
    d["includeStaged"] = rest[0] == "1"
    d["files"] = rest[1:]
print(json.dumps(d))
PY
}

# Query prefix for the GET routes ("path=…&" or empty).
_changes_q() {
    if [ -n "$_CH_PATH" ]; then
        printf 'path=%s&' "$(urlencode "$_CH_PATH")"
    fi
}

_changes_list() {
    local ws="" base="" q
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            --base) [ $# -ge 2 ] || _changes_die usage "--base needs a ref" 2; base="$2"; shift 2 ;;
            --base=*) base="${1#*=}"; shift ;;
            *) _changes_die usage "unknown argument for k2 changes: $1 (see k2 changes --help)" 2 ;;
        esac
    done
    _changes_target "$ws"
    q="$(_changes_q)"
    [ -n "$base" ] && q="${q}base=$(urlencode "$base")&"
    _changes_call GET /cli/git/snapshot "${q%&}"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    local rc=0
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
s = json.load(sys.stdin)
if not s.get("isRepo"):
    print("Not a git repository: %s" % s.get("workspacePath", ""))
    sys.exit(4)
head = s.get("branch") or "(no branch)"
if s.get("detached"):
    head = "HEAD detached at %s" % (s.get("headOid") or "")[:7]
elif s.get("unborn"):
    head = "%s (no commits yet)" % head
line = "On %s" % head
if s.get("upstream"):
    line += " · %s ahead, %s behind %s" % (s.get("ahead", 0), s.get("behind", 0), s["upstream"])
print(line)
if s.get("state") and s["state"] != "clean":
    print("In progress: %s" % s["state"].replace("_", " "))
b = s.get("base")
if b:
    mb = (b.get("mergeBase") or "")[:7] or "none"
    print("Compared with %s (merge-base %s · %s ahead, %s behind)" % (b.get("ref"), mb, b.get("ahead", 0), b.get("behind", 0)))
files = s.get("files") or []
c = s.get("counts") or {}
if not files:
    print("No changes.")
    sys.exit(0)
titles = [("conflicted", "Conflicted"), ("staged", "Staged"), ("unstaged", "Unstaged"),
          ("base", "Changed vs base"), ("untracked", "Untracked")]
width = min(60, max(len(f["path"]) + (len(f["oldPath"]) + 3 if f.get("oldPath") else 0) for f in files))
for g, title in titles:
    rows = [f for f in files if f.get("group") == g]
    if not rows and not c.get(g):
        continue
    print()
    print("%s (%d)" % (title, c.get(g, len(rows))))
    for f in rows:
        name = f["path"] + (" <- " + f["oldPath"] if f.get("oldPath") else "")
        if f.get("dir"):
            stat = "(folder)"
        elif f.get("binary"):
            stat = "(binary)"
        elif f.get("additions") is None:
            stat = ""
        else:
            stat = "+%d -%d" % (f["additions"], f.get("deletions") or 0)
        print(("  %s  %-" + str(width) + "s  %s") % (f.get("status", "?"), name, stat))
print()
if s.get("truncated"):
    print("Showing the first %d of %d changes (too many to list)." % (s.get("limit", len(files)), c.get("total", 0)))
else:
    print("%d change%s" % (c.get("total", len(files)), "" if c.get("total", len(files)) == 1 else "s"))
' || rc=$?
    if [ "$rc" = 4 ]; then
        _changes_die not_a_repo "not a git repository (make one with: k2 changes init)" 3
    fi
    return "$rc"
}

# Renders one diff-file?sides=1 answer (stdin) as a unified diff, or with
# argv[2]=1 as one JSON line (the answer without the texts, plus "patch").
_CH_PY_RENDER='
import difflib, json, sys
ctx, as_json = int(sys.argv[1]), sys.argv[2] == "1"
d = json.load(sys.stdin)
new_path = d.get("path") or ""
old_path = d.get("oldPath") or new_path
old, new = d.get("old") or {}, d.get("new") or {}
a = "a/" + old_path if old.get("exists") else "/dev/null"
b = "b/" + new_path if new.get("exists") else "/dev/null"

def lines(t):
    if not t:
        return []
    parts = t.split("\n")
    out = [p + "\n" for p in parts[:-1]]
    if parts[-1] != "":
        out.append(parts[-1])
    return out

patch = []
# An added or deleted file gets a plain (traditional) header: the git
# form needs its file mode, which the daemon does not send, and git apply
# reads "--- /dev/null" / "+++ /dev/null" as the add or delete.
head = []
if old.get("exists") and new.get("exists"):
    head = ["diff --git a/%s b/%s\n" % (old_path, new_path)]
    if old_path != new_path:
        head += ["rename from %s\n" % old_path, "rename to %s\n" % new_path]
if d.get("binary"):
    patch = head + ["Binary files %s and %s differ\n" % (a, b)]
elif d.get("tooLarge"):
    patch = head + ["Large files %s and %s differ (over 10 MB or 120,000 lines; not shown)\n" % (a, b)]
else:
    ot, nt = old.get("text") or "", new.get("text") or ""
    if ot != nt or old.get("exists") != new.get("exists") or old_path != new_path:
        body = []
        for ln in difflib.unified_diff(lines(ot), lines(nt), a, b, n=ctx):
            if ln.endswith("\n"):
                body.append(ln)
            else:
                body.append(ln + "\n")
                body.append("\\ No newline at end of file\n")
        patch = head + body
text = "".join(patch)
if as_json:
    meta = {k: d.get(k) for k in ("path", "oldPath", "mode", "baseRef", "rev", "binary", "tooLarge", "additions", "deletions")}
    meta["patch"] = text
    print(json.dumps(meta))
else:
    sys.stdout.write(text)
'

_changes_diff() {
    local ws="" base="" mode="worktree" ctx=3 paths=() q f
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            --base) [ $# -ge 2 ] || _changes_die usage "--base needs a ref" 2; base="$2"; shift 2 ;;
            --base=*) base="${1#*=}"; shift ;;
            --staged|--cached) mode="staged"; shift ;;
            --unstaged) mode="unstaged"; shift ;;
            -U|--unified) [ $# -ge 2 ] || _changes_die usage "-U needs a number" 2; ctx="$2"; shift 2 ;;
            -U*) ctx="${1#-U}"; shift ;;
            --unified=*) ctx="${1#*=}"; shift ;;
            --) shift; paths+=("$@"); break ;;
            -*) _changes_die usage "unknown flag for k2 changes diff: $1" 2 ;;
            *) paths+=("$1"); shift ;;
        esac
    done
    case "$ctx" in ''|*[!0-9]*) _changes_die usage "-U needs a number" 2 ;; esac
    if [ -n "$base" ] && [ "$mode" != "worktree" ]; then
        _changes_die usage "--base can't be mixed with --staged/--unstaged" 2
    fi
    [ -n "$base" ] && mode="base"
    _changes_target "$ws"
    _CH_FILES=()
    [ "${#paths[@]}" -gt 0 ] && _changes_files "${paths[@]}"

    # The snapshot picks the files (and their pre-rename names).
    q="$(_changes_q)"
    [ -n "$base" ] && q="${q}base=$(urlencode "$base")&"
    _changes_call GET /cli/git/snapshot "${q%&}"
    local snap="$_CH_BODY" rows
    rows="$(printf '%s' "$snap" | python3 -c '
import json, os, sys
s = json.load(sys.stdin)
mode, want = sys.argv[1], sys.argv[2:]
if not s.get("isRepo"):
    sys.exit(4)
print(json.dumps({k: s.get(k) for k in ("workspacePath", "repoRoot", "rev")}))
groups = {"worktree": ["conflicted", "staged", "unstaged", "untracked"],
          "staged": ["staged"],
          "unstaged": ["conflicted", "unstaged"],
          "base": ["conflicted", "base", "untracked"]}[mode]
files = [f for f in (s.get("files") or []) if f.get("group") in groups]
seen, rows = set(), []
for f in files:
    if f["path"] in seen:
        continue
    seen.add(f["path"])
    rows.append(f)
root = os.path.realpath(s.get("repoRoot") or "")
def rel(p):
    if p.startswith("/"):
        d, b = os.path.split(p.rstrip("/"))
        full = os.path.join(os.path.realpath(d), b)
        if full == root or full.startswith(root + "/"):
            r = os.path.relpath(full, root)
            return r + ("/" if p.endswith("/") else "")
    return p[2:] if p.startswith("./") else p
out = []
if want:
    for w in want:
        r = rel(w)
        key = r.rstrip("/")
        hit = [f for f in rows if f["path"].rstrip("/") == key or f["path"].startswith(key + "/") or key in ("", ".")]
        if hit:
            out.extend(h for h in hit if h not in out)
        else:
            out.append({"path": r, "oldPath": None, "dir": False, "explicit": True})
else:
    out = rows
if s.get("truncated") and not want:
    print("note: the change list is capped at %d files; name paths to see the rest" % s.get("limit", 1000), file=sys.stderr)
for f in out:
    # \x1f, not a tab: read would merge the empty oldPath field.
    print("%s\x1f%s\x1f%d" % (f["path"], f.get("oldPath") or "", 1 if f.get("dir") else 0))
' "$mode" ${_CH_FILES[@]+"${_CH_FILES[@]}"})" || {
        _changes_die not_a_repo "not a git repository" 3
    }

    local meta modeq fpath fold fdir failed=0 jl=() rendered
    meta="${rows%%$'\n'*}"
    if [ "$meta" = "$rows" ]; then rows=""; else rows="${rows#*$'\n'}"; fi
    case "$mode" in
        base) modeq="base=$(urlencode "$base")" ;;
        *) modeq="mode=$mode" ;;
    esac
    while IFS=$'\x1f' read -r fpath fold fdir; do
        [ -n "$fpath" ] || continue
        if [ "$fdir" = "1" ]; then
            echo "note: $fpath is an untracked folder (not expanded); name a file inside it" >&2
            continue
        fi
        q="$(_changes_q)file_path=$(urlencode "$fpath")&sides=1&${modeq}"
        [ -n "$fold" ] && q="${q}&old_path=$(urlencode "$fold")"
        _changes_split "$(cli_send_json GET /cli/git/diff-file "$q")"
        if [ "$_CH_STATUS" != "200" ]; then
            [ "${_CH_JSON:-0}" = "1" ] || echo "($fpath)" >&2
            _changes_err "$_CH_STATUS" "$_CH_BODY"
            failed="$_CH_ERR_RC"
            [ "$_CH_STATUS" = "000" ] && break
            continue
        fi
        rendered="$(printf '%s' "$_CH_BODY" | python3 -c "$_CH_PY_RENDER" "$ctx" "${_CH_JSON:-0}"; printf x)"
        rendered="${rendered%x}"
        if [ "${_CH_JSON:-0}" = "1" ]; then
            jl+=("$rendered")
        else
            printf '%s' "$rendered"
        fi
    done <<< "$rows"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' ${jl[@]+"${jl[@]}"} | python3 -c '
import json, sys
s = json.loads(sys.argv[1] or "{}")
files = [json.loads(l) for l in sys.stdin if l.strip()]
print(json.dumps({"workspacePath": s.get("workspacePath"), "repoRoot": s.get("repoRoot"), "rev": s.get("rev"),
                  "mode": sys.argv[2], "base": sys.argv[3] or None, "files": files}))
' "$meta" "$mode" "$base"
    fi
    [ "$failed" = "0" ] || exit "$failed"
    return 0
}

_changes_stage() {
    local verb="$1"; shift
    local ws="" all=0 paths=() f done_list=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            --all|-A) [ "$verb" = "stage" ] || _changes_die usage "k2 changes unstage takes paths (no --all)" 2; all=1; shift ;;
            --) shift; paths+=("$@"); break ;;
            -*) _changes_die usage "unknown flag for k2 changes $verb: $1" 2 ;;
            *) paths+=("$1"); shift ;;
        esac
    done
    if [ "$all" = "1" ] && [ "${#paths[@]}" -gt 0 ]; then
        _changes_die usage "k2 changes stage: give paths or --all, not both" 2
    fi
    if [ "$all" = "0" ] && [ "${#paths[@]}" -eq 0 ]; then
        _changes_die usage "usage: k2 changes $verb <path>...$([ "$verb" = stage ] && echo ' | --all')" 2
    fi
    _changes_target "$ws"
    if [ "$all" = "1" ]; then
        _changes_call POST /cli/git/stage-all "$(_changes_body all)"
        if [ "${_CH_JSON:-0}" = "1" ]; then
            echo '{"success":true,"staged":"all"}'
        else
            echo "Staged all changes."
        fi
        return 0
    fi
    _changes_files "${paths[@]}"
    local i shown
    for i in "${!_CH_FILES[@]}"; do
        f="${_CH_FILES[$i]}"
        shown="${paths[$i]}"
        _changes_split "$(cli_send_json POST "/cli/git/$verb" "$(_changes_body file "$f")")"
        if [ "$_CH_STATUS" != "200" ]; then
            [ "${_CH_JSON:-0}" = "1" ] || echo "($shown)" >&2
            _changes_err "$_CH_STATUS" "$_CH_BODY"
            exit "$_CH_ERR_RC"
        fi
        done_list+=("$shown")
        [ "${_CH_JSON:-0}" = "1" ] || echo "$( [ "$verb" = stage ] && echo Staged || echo Unstaged ) $shown"
    done
    if [ "${_CH_JSON:-0}" = "1" ]; then
        python3 -c 'import json,sys; print(json.dumps({"success": True, sys.argv[1]: sys.argv[2:]}))' \
            "$( [ "$verb" = stage ] && echo staged || echo unstaged )" "${done_list[@]}"
    fi
}

_changes_discard() {
    local ws="" yes=0 staged=0 paths=() ans
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            --yes|-y) yes=1; shift ;;
            --staged|--include-staged) staged=1; shift ;;
            --) shift; paths+=("$@"); break ;;
            -*) _changes_die usage "unknown flag for k2 changes discard: $1" 2 ;;
            *) paths+=("$1"); shift ;;
        esac
    done
    [ "${#paths[@]}" -gt 0 ] || _changes_die usage "usage: k2 changes discard <path>... [--staged] [--yes]" 2
    _changes_target "$ws"
    _changes_files "${paths[@]}"
    if [ "$yes" != "1" ]; then
        if [ ! -t 0 ]; then
            _changes_die confirm_required "k2 changes discard needs --yes when not run on a terminal (untracked files and every copy it replaces go to the Trash)" 2
        fi
        local what="back to the staged copy"
        [ "$staged" = "1" ] && what="back to the last commit (staged edits too)"
        printf 'Discard changes to %d file(s)? Edited files go %s; untracked files and the current copies go to the Trash. [y/N] ' \
            "${#_CH_FILES[@]}" "$what" >&2
        IFS= read -r ans || ans=""
        case "$ans" in
            y|Y|yes|YES|Yes) ;;
            *) echo "Nothing discarded." >&2; exit 1 ;;
        esac
    fi
    _changes_call POST /cli/git/discard "$(_changes_body discard "$staged" "${_CH_FILES[@]}")"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        printf '%s' "$_CH_BODY" | python3 -c 'import json,sys; sys.exit(1 if json.load(sys.stdin).get("failed") else 0)'
        return $?
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)
restored, trashed, unstaged = r.get("restored") or [], r.get("trashed") or [], r.get("unstaged") or []
skipped, failed = r.get("skipped") or [], r.get("failed") or []
if not (restored or trashed or unstaged):
    print("Nothing discarded.")
for p in restored:
    print("  restored  %s" % p)
for p in unstaged:
    print("  unstaged  %s" % p)
for p in trashed:
    print("  trashed   %s  (moved to the Trash)" % p)
for s in skipped:
    print("  skipped   %s: %s" % (s.get("path"), s.get("reason")))
for f in failed:
    print("  FAILED    %s: %s (left untouched)" % (f.get("path"), f.get("error")), file=sys.stderr)
sys.exit(1 if failed else 0)
'
}

_changes_commit() {
    local ws="" all=0 msg="" have_msg=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            -m|--message)
                [ $# -ge 2 ] || _changes_die usage "-m needs a message" 2
                if [ "$have_msg" = "1" ]; then msg="${msg}"$'\n\n'"$2"; else msg="$2"; fi
                have_msg=1; shift 2 ;;
            --message=*) msg="${1#*=}"; have_msg=1; shift ;;
            --all|-a) all=1; shift ;;
            *) _changes_die usage "unknown argument for k2 changes commit: $1 (usage: k2 changes commit -m \"message\" [--all])" 2 ;;
        esac
    done
    if [ "$have_msg" != "1" ] || [ -z "$(printf '%s' "$msg" | tr -d '[:space:]')" ]; then
        _changes_die usage "usage: k2 changes commit -m \"message\" [--all]" 2
    fi
    _changes_target "$ws"
    if [ "$all" = "1" ]; then
        _changes_call POST /cli/git/stage-all "$(_changes_body all)"
    fi
    # The daemon would make an empty commit; refuse like git does.
    local q staged
    q="$(_changes_q)"
    _changes_call GET /cli/git/snapshot "${q%&}"
    staged="$(printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
s = json.load(sys.stdin)
print(-1 if not s.get("isRepo") else (s.get("counts") or {}).get("staged", 0))')"
    [ "$staged" = "-1" ] && _changes_die not_a_repo "not a git repository" 3
    if [ "$staged" = "0" ]; then
        _changes_die nothing_staged "nothing is staged to commit; stage files first (k2 changes stage <path>) or use --all" 3
    fi
    _changes_call POST /cli/git/commit "$(_changes_body commit "$msg")"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)
print("Committed %s: %s" % ((r.get("oid") or "")[:7], (r.get("message") or "").splitlines()[0] if r.get("message") else ""))'
}

