"""Turn a file's git history into an editing trace (automerge-paper format).

Usage: python3 -I git_history_trace.py <repo> <path> <out.json>

Each commit's version is diffed against the previous one (difflib, per line,
then per char inside replaced lines). Inserts up to 80 chars are "typed" one
char per patch; longer inserts are one paste. Positions are Unicode code
points into the doc as it stands when the patch is applied, so patches apply
in order. The output stays local (spike/out is gitignored): it is derived from
private content.
"""
import difflib, json, subprocess, sys

repo, path, out = sys.argv[1:4]
shas = subprocess.run(["git", "-C", repo, "log", "--format=%H", "--reverse", "--", path],
                      check=True, capture_output=True, text=True).stdout.split()
versions = []
for sha in shas:
    r = subprocess.run(["git", "-C", repo, "show", f"{sha}:{path}"], capture_output=True)
    if r.returncode == 0:
        versions.append(r.stdout.decode("utf-8"))

txns = []
def emit(pos, dele, ins):
    if dele:
        txns.append({"patches": [[pos, dele, ""]]})
    if not ins:
        return
    if len(ins) <= 80:
        for i, ch in enumerate(ins):
            txns.append({"patches": [[pos + i, 0, ch]]})
    else:
        txns.append({"patches": [[pos, 0, ins]]})

def diff_into(a, b, base):
    # Char-level diff of a -> b, applied at doc offset `base`. Returns nothing;
    # emits patches left to right, tracking the shift.
    sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
    shift = 0
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            continue
        emit(base + i1 + shift, i2 - i1, b[j1:j2])
        shift += (j2 - j1) - (i2 - i1)

prev = ""
for v in versions:
    a, b = prev.splitlines(keepends=True), v.splitlines(keepends=True)
    sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
    pos = 0      # char offset in the evolving doc
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        old = "".join(a[i1:i2]); new = "".join(b[j1:j2])
        if tag == "equal":
            pos += len(new); continue
        if tag == "replace" and len(old) < 4000 and len(new) < 4000:
            diff_into(old, new, pos)
        else:
            emit(pos, len(old), new)
        pos += len(new)
    prev = v

json.dump({"startContent": "", "endContent": prev, "txns": txns,
           "meta": {"versions": len(versions)}}, open(out, "w"))
print(f"{len(versions)} versions, {len(txns)} patches, end {len(prev)} chars")
