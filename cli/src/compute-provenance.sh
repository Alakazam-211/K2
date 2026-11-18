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

# Write the `trust/` files a statement's reader needs into <dir>/trust/
# (JSON with a `trust` object on stdin; prd §5): each node's public key as
# nodes/<fingerprint>.pem (a stock DSSE tool checks with it), the node
# self-statements as statements/<sha256>.json, the owners as owners.json
# and each one's newest roster as rosters/<roster key fp>.json. Prints how
# many files it wrote.
_compute_write_trust() {
    python3 -c '
import hashlib, json, os, sys
t = json.load(sys.stdin).get("trust") or {}
root = os.path.join(sys.argv[1], "trust")
n = 0
def put(rel, text):
    global n
    p = os.path.join(root, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w") as f:
        f.write(text)
    n += 1
for fp, k in sorted((t.get("nodeKeys") or {}).items()):
    put(os.path.join("nodes", fp + ".pem"), k["pem"])
for s in t.get("statements") or []:
    put(os.path.join("statements", hashlib.sha256(s["body"].encode()).hexdigest() + ".json"), json.dumps(s, sort_keys=True) + "\n")
owners = t.get("owners") or []
put("owners.json", json.dumps(owners, indent=1, sort_keys=True) + "\n")
for o, r in zip(owners, t.get("rosters") or []):
    put(os.path.join("rosters", o["rosterKeyFp"] + ".json"), json.dumps(r, sort_keys=True) + "\n")
print(n)' "$1"
}

_study_page_provenance() {
    cat <<'PAGE'
%include cli/text/study/provenance.txt
PAGE
}

# Write each statement as <dir>/provenance/<label>.intoto.jsonl (JSON on
# stdin: `statements` {label: {line}} or `bundle.provenance` {label:
# envelope}). Prints how many.
_compute_write_statements() {
    python3 -c '
import json, os, sys
v = json.load(sys.stdin)
lines = {k: s["line"] for k, s in (v.get("statements") or {}).items()}
for k, env in ((v.get("bundle") or {}).get("provenance") or {}).items():
    lines[k] = json.dumps(env, separators=(",", ":")) + "\n"
root = os.path.join(sys.argv[1], "provenance")
if lines:
    os.makedirs(root, exist_ok=True)
for k, line in sorted(lines.items()):
    with open(os.path.join(root, k + ".intoto.jsonl"), "w") as f:
        f.write(line)
print(len(lines))' "$1"
}
