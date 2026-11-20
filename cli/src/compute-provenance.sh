# ── k2 compute receipt | provenance | verify ─────────────────────────
# Receipts and build statements (prd-compute-build-attestations-v1 S3).
# The node signs a receipt for every job and, when the job kept files and
# the node offers it, an in-toto build statement (SLSA provenance v1, in a
# DSSE envelope). The daemon checked and kept both; these verbs read them
# (`/cli/compute/jobs/receipt`, `/cli/compute/jobs/provenance`) and check
# one against this server's records (`/cli/compute/verify`).

cmd_compute_provenance() {
    local verb="$1"; shift
    local as_json=0 out="" file="" owner_key="" target=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --out) out="${2:-}"; [ -n "$out" ] || { echo "usage: k2 compute provenance <job> --out FILE" >&2; exit 2; }; shift 2 ;;
            --file) file="${2:-}"; [ -n "$file" ] || { echo "usage: k2 compute verify <job | FILE.intoto.jsonl> --file PATH" >&2; exit 2; }; shift 2 ;;
            --owner-key) owner_key="${2:-}"; [ -n "$owner_key" ] || { echo "usage: k2 compute verify <job | FILE.intoto.jsonl> --owner-key FP" >&2; exit 2; }; shift 2 ;;
            -*) echo "k2 compute $verb: unknown flag $1 (k2 compute --help)" >&2; exit 2 ;;
            *) [ -z "$target" ] || { echo "k2 compute $verb: one job at a time" >&2; exit 2; }; target="$1"; shift ;;
        esac
    done
    case "$verb" in
        receipt)
            [ -n "$target" ] || { echo "usage: k2 compute receipt <job>" >&2; exit 2; }
            _compute_call GET /cli/compute/jobs/receipt "job=$(urlencode "$target")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render receipt
            ;;
        provenance)
            { [ -n "$target" ] && [ -z "$file$owner_key" ]; } || { echo "usage: k2 compute provenance <job> [--out FILE] [--json]" >&2; exit 2; }
            _compute_call GET /cli/compute/jobs/provenance "job=$(urlencode "$target")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            if [ -z "$out" ]; then
                # The .intoto.jsonl line exactly as the node's envelope (one line).
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["line"])'
                exit 0
            fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
path = sys.argv[1]
with open(path, "w") as f:
    f.write(v["line"])
subs = v.get("subjects") or []
print("wrote %s: job %s'"'"'s build statement, signed by node %s, naming %d file(s):" % (path, v["short"], v.get("node") or "?", len(subs)))
for s in subs:
    print("  %s  sha256 %s" % (s["name"], s["sha256"]))
print("check it: k2 compute verify %s [--file <one of those files>]" % path)' "$out" || exit 1
            ;;
        verify)
            [ -n "$target" ] && [ -z "$out" ] || { echo "usage: k2 compute verify <job | FILE.intoto.jsonl> [--file PATH] [--owner-key FP] [--json]" >&2; exit 2; }
            if [ -n "$file" ] && [ ! -f "$file" ]; then echo "k2 compute verify: --file $file: no such file" >&2; exit 2; fi
            local body
            body="$(python3 -c '
import hashlib, json, os, sys
target, file, owner = sys.argv[1:4]
o = {}
if os.path.isfile(target):
    lines = [l for l in open(target, encoding="utf-8").read().splitlines() if l.strip()]
    if len(lines) != 1:
        sys.exit("k2 compute verify: %s holds %d lines; a .intoto.jsonl file from k2 compute provenance holds one statement" % (target, len(lines)))
    try:
        o["envelope"] = json.loads(lines[0])
    except ValueError as e:
        sys.exit("k2 compute verify: %s is not a statement line (JSON): %s" % (target, e))
else:
    o["job"] = target
if file:
    h = hashlib.sha256()
    with open(file, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    o["sha256"] = h.hexdigest()
    o["file"] = os.path.basename(file)
if owner:
    o["ownerKey"] = owner
print(json.dumps(o))' "$target" "$file" "$owner_key")" || exit 2
            _compute_call POST /cli/compute/verify "$body"
            if [ "$as_json" = "1" ]; then
                printf '%s\n' "$_CP_BODY"
            else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
what = "job %s'"'"'s build statement (node %s)" % (v["short"], v.get("node") or "?")
print(("verified: " + what) if v["verified"] else ("NOT verified: %s: %s" % (what, v.get("failed"))))
for c in v["checks"]:
    print("  %-8s %-10s %s" % ("checked" if c["ok"] else "FAILED", c["check"], c["detail"]))
print("not checked: " + v["notChecked"])'
            fi
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin)["verified"] else 1)'
            ;;
    esac
}

# What `ci evidence` and `ci provenance` write under --out (JSON answer on
# stdin), as one program run in modes:
#   check <verb> <dir>  every label and file name the answer puts under
#                       <dir>, checked before anything is written. One that
#                       is absolute, has a ".." part, holds a NUL or a
#                       newline, goes through a link or lands outside <dir>
#                       is refused (exit 3, code: hint) and nothing is
#                       written.
#   bundle <dir>        the evidence bundle as <dir>/<file>; prints its path.
#   statements <dir>    each statement (`statements` {label: {line}} or
#                       `bundle.provenance` {label: envelope}) as
#                       <dir>/provenance/<label>.intoto.jsonl; prints how many.
#   trust <dir>         the `trust/` files a statement's reader needs (prd §5):
#                       each node's public key as nodes/<fingerprint>.pem (a
#                       stock DSSE tool checks with it), the node
#                       self-statements as statements/<sha256>.json, the
#                       owners as owners.json and each one's newest roster as
#                       rosters/<roster key fp>.json; prints how many.
# Each file is made fresh: the folders under <dir> are opened one at a
# time and no link is followed, a plain file already there is removed, and
# the new one is created with O_CREAT|O_EXCL|O_NOFOLLOW.
_CI_OUT_PY='
import hashlib, json, os, stat, sys
mode, out = sys.argv[1], sys.argv[-1]
v = json.load(sys.stdin)


def folder(rel, make):
    # The open folder that holds rel under out, and the last part of rel.
    # The first `make` folders (every one when make < 0) are made when
    # missing. No link is followed.
    fd = os.open(out, os.O_RDONLY | os.O_DIRECTORY)
    parts = rel.split("/")
    try:
        for i, p in enumerate(parts[:-1]):
            if not p:
                continue
            if make < 0 or i < make:
                try:
                    os.mkdir(p, 0o777, dir_fd=fd)
                except FileExistsError:
                    pass
            sub = os.open(p, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = sub
    except BaseException:
        os.close(fd)
        raise
    return fd, parts[-1]


def put(rel, text, make):
    d, name = folder(rel, make)
    try:
        try:
            if stat.S_ISREG(os.stat(name, dir_fd=d, follow_symlinks=False).st_mode):
                os.unlink(name, dir_fd=d)
        except FileNotFoundError:
            pass
        fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o666, dir_fd=d)
    finally:
        os.close(d)
    with os.fdopen(fd, "w") as f:
        f.write(text)


def obj(x):
    return x if isinstance(x, dict) else {}


def names(verb):
    # (what, name, its path under out): each name the answer puts there,
    # then the fixed folders and files written beside them.
    a = obj(v)
    b = obj(a.get("bundle"))
    t = obj(a.get("trust"))
    got = []
    if verb == "evidence" and isinstance(a.get("file"), str):
        got.append(("file name", a["file"], a["file"]))
    st = sorted(set(obj(a.get("statements"))) | set(obj(b.get("provenance"))))
    got += [("statement label", k, "provenance/" + k + ".intoto.jsonl") for k in st]
    got += [("node key", k, "trust/nodes/" + k + ".pem") for k in sorted(obj(t.get("nodeKeys")))]
    owners, rosters = t.get("owners"), t.get("rosters")
    if isinstance(owners, list) and isinstance(rosters, list):
        for o, r in zip(owners, rosters):
            fp = obj(o).get("rosterKeyFp")
            if isinstance(fp, str):
                got.append(("roster key", fp, "trust/rosters/" + fp + ".json"))
    if verb == "evidence":
        labels = set(obj(a.get("jobs"))) | set(obj(b.get("receipts")))
        got += [("job label", k, k) for k in sorted(labels)]
    for s in t.get("statements") if isinstance(t.get("statements"), list) else []:
        if isinstance(obj(s).get("body"), str):
            got.append(("path", "", "trust/statements/" + hashlib.sha256(s["body"].encode()).hexdigest() + ".json"))
    got.append(("path", "", "trust/owners.json"))
    return got


def link_on(rel):
    # The first link on the way to rel under out, if any.
    p = out
    for part in rel.split("/"):
        if not part:
            continue
        p = os.path.join(p, part)
        try:
            if stat.S_ISLNK(os.lstat(p).st_mode):
                return p
        except (FileNotFoundError, NotADirectoryError):
            return None
    return None


def why(name, rel):
    if name.startswith("/"):
        return "is an absolute path"
    if ".." in name.split("/"):
        return "has a .. part"
    if "\0" in name or "\n" in name:
        return "holds a NUL or a newline"
    link = link_on(rel)
    if link:
        return "goes through the link " + link
    root = os.path.realpath(out)
    if os.path.commonpath([root, os.path.realpath(os.path.join(out, rel))]) != root:
        return "lands outside " + out
    return None


if mode == "check":
    for what, name, rel in names(sys.argv[2]):
        bad = why(name, rel)
        if bad:
            shown = what + " " + name if name else rel
            sys.stderr.write(json.dumps({"error": "ci_name_unsafe", "hint": "%s %s: refused, nothing written to %s" % (shown, bad, out)}) + "\n")
            sys.exit(3)
elif mode == "bundle":
    path = os.path.join(out, v["file"])
    put(v["file"], json.dumps(v["bundle"], indent=1, sort_keys=True) + "\n", 0)
    print(path)
elif mode == "statements":
    lines = {k: s["line"] for k, s in (v.get("statements") or {}).items()}
    for k, env in ((v.get("bundle") or {}).get("provenance") or {}).items():
        lines[k] = json.dumps(env, separators=(",", ":")) + "\n"
    for k, line in sorted(lines.items()):
        put("provenance/" + k + ".intoto.jsonl", line, 1)
    print(len(lines))
elif mode == "trust":
    t = v.get("trust") or {}
    files = []
    for fp, k in sorted((t.get("nodeKeys") or {}).items()):
        files.append(("nodes/" + fp + ".pem", k["pem"]))
    for s in t.get("statements") or []:
        files.append(("statements/" + hashlib.sha256(s["body"].encode()).hexdigest() + ".json", json.dumps(s, sort_keys=True) + "\n"))
    owners = t.get("owners") or []
    files.append(("owners.json", json.dumps(owners, indent=1, sort_keys=True) + "\n"))
    for o, r in zip(owners, t.get("rosters") or []):
        files.append(("rosters/" + o["rosterKeyFp"] + ".json", json.dumps(r, sort_keys=True) + "\n"))
    for rel, text in files:
        put("trust/" + rel, text, -1)
    print(len(files))
'

# Check every name a `ci evidence` or `ci provenance` answer (on stdin)
# puts under <dir>, before anything is written: `_compute_out_check
# <evidence|provenance> <dir>`. Exits 3 with code: hint on the first one
# refused.
_compute_out_check() {
    python3 -c "$_CI_OUT_PY" check "$1" "$2"
}

# `_compute_write_trust <dir>`: <dir>/trust/ (see _CI_OUT_PY).
_compute_write_trust() {
    python3 -c "$_CI_OUT_PY" trust "$1"
}

_study_page_provenance() {
    cat <<'PAGE'
%include cli/text/study/provenance.txt
PAGE
}

# `_compute_write_statements <dir>`: <dir>/provenance/<label>.intoto.jsonl
# (see _CI_OUT_PY).
_compute_write_statements() {
    python3 -c "$_CI_OUT_PY" statements "$1"
}
