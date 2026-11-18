#!/usr/bin/env bash
# k2sign-admin-scratch.sh: the admin verbs' scratch folder (k2sign.py
# Scratch), outside and inside test mode.
#
# Outside test mode (a production install: admin verbs run as root) the
# scratch is exactly tempfile.mkdtemp(prefix="k2sign-admin-"), then chmod
# 0700: no dir= and no other argument. Inside test mode it is a fresh 0700
# folder under the test root, passed as dir=, removed at the end, and when
# it can't be made the verb stops: it never falls back to the host's /tmp.
# Also: an admin Ctx outside test mode is refused unless it runs as root,
# and only K2SIGN_TEST_ROOT turns test mode on.
#
# It imports k2sign.py and drives Scratch the way every admin verb does
# (`with Scratch(ctx) as d:`), with tempfile.mkdtemp recorded (the
# production call's folder is made inside this test's temp folder, never
# in /tmp). Pure python: no minisign, no keys, no install. macOS and Linux.
#   bash tests/release/k2sign-admin-scratch.sh
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
[ "$(id -u)" != 0 ] || { echo "FAIL: run this as a normal user (it checks the non-root refusal)" >&2; exit 1; }
work="$(mktemp -d "${TMPDIR:-/tmp}/k2sign-admin-scratch.XXXXXX")"
work="$(cd "$work" && pwd -P)"
trap 'chmod -R u+w "$work" 2>/dev/null || true; rm -rf "$work"' EXIT

env -u K2SIGN_TEST_ROOT -u K2SIGN_TEST_SIGNER_USER PYTHONDONTWRITEBYTECODE=1 \
    python3 -I - "$REPO/scripts/k2sign/k2sign.py" "$work" <<'PY'
import importlib.util, os, stat, sys

src, work = sys.argv[1], sys.argv[2]
sys.dont_write_bytecode = True   # no __pycache__ beside the signer
spec = importlib.util.spec_from_file_location("k2sign", src)
k = importlib.util.module_from_spec(spec)
spec.loader.exec_module(k)

npass = nfail = 0
def check(name, ok, detail=""):
    global npass, nfail
    if ok:
        npass += 1
        print("  ok: " + name)
    else:
        nfail += 1
        print("  FAIL: %s %s" % (name, detail), file=sys.stderr)

calls = []
real_mkdtemp = k.tempfile.mkdtemp
prod_home = os.path.join(work, "prod-mkdtemp")
os.mkdir(prod_home, 0o700)
def recording_mkdtemp(*args, **kw):
    calls.append((args, dict(kw)))
    if "dir" not in kw:
        # The production call names no folder; make it here, not in /tmp.
        kw = dict(kw, dir=prod_home)
    return real_mkdtemp(*args, **kw)
k.tempfile.mkdtemp = recording_mkdtemp

def ctx(admin, test_root):
    c = object.__new__(k.Ctx)
    c.admin, c.test_root, c.test = admin, test_root, bool(test_root)
    return c

def listing(d):
    return sorted(os.listdir(d)) if os.path.isdir(d) else None

print("== outside test mode: the production scratch call ==")
calls.clear()
c = ctx(True, "")
s = k.Scratch(c)
with s as d:
    mode = stat.S_IMODE(os.stat(d).st_mode)
    inside = os.path.dirname(d) == prod_home
check("mkdtemp is called once, as mkdtemp(prefix=\"k2sign-admin-\") and nothing else",
      calls == [((), {"prefix": "k2sign-admin-"})], repr(calls))
check("the folder is 0700", mode == 0o700, oct(mode))
check("(the recorder made it in this test's folder)", inside, d)
check("it is removed at the end", not os.path.exists(d), d)
check("no test-mode folder is made outside test mode", not hasattr(s, "base") and
      not any(n.startswith("admin-tmp-") for n in os.listdir(work)), repr(os.listdir(work)))

print("== outside test mode: only root runs admin verbs, and only K2SIGN_TEST_ROOT sets test mode ==")
os.environ.pop("K2SIGN_TEST_ROOT", None)
try:
    k.Ctx(True)
    check("an admin Ctx as a normal user outside test mode is refused", False, "it was made")
except k.Refused as e:
    check("an admin Ctx as a normal user outside test mode is refused", "runs only as root" in str(e), str(e))
root = os.path.join(work, "troot")
os.mkdir(root, 0o700)
os.environ["K2SIGN_TEST_ROOT"] = root
c = k.Ctx(True)
check("K2SIGN_TEST_ROOT turns test mode on (and nothing else is needed)", c.test and c.test_root == root,
      repr((c.test, c.test_root)))
os.environ.pop("K2SIGN_TEST_ROOT")

print("== test mode: a fresh 0700 folder under the test root, as dir= ==")
calls.clear()
c = ctx(True, root)
seen = []
for _ in range(2):
    with k.Scratch(c) as d:
        base = os.path.dirname(d)
        seen.append(base)
        bmode = stat.S_IMODE(os.stat(base).st_mode)
        check("its folder is admin-tmp-<16 hex> right under the test root",
              os.path.dirname(base) == root and len(os.path.basename(base)) == len("admin-tmp-") + 16
              and os.path.basename(base).startswith("admin-tmp-"), base)
        check("that folder is 0700", bmode == 0o700, oct(bmode))
        check("mkdtemp got dir= that folder", calls[-1] == ((), {"prefix": "k2sign-admin-", "dir": base}),
              repr(calls[-1]))
    check("both are removed at the end", not os.path.exists(d) and not os.path.exists(base), base)
check("each run gets a fresh folder", len(set(seen)) == 2, repr(seen))
check("the test root is left as it was", listing(root) == [], repr(listing(root)))
check("every mkdtemp in test mode named its folder", all("dir" in kw for _, kw in calls), repr(calls))

print("== test mode: no folder, no fallback ==")
calls.clear()
os.chmod(root, 0o500)
try:
    with k.Scratch(ctx(True, root)):
        pass
    check("a test root it can't write is refused", False, "it went on")
except k.Refused as e:
    check("a test root it can't write is refused", "cannot make the admin scratch folder" in str(e), str(e))
os.chmod(root, 0o700)
try:
    with k.Scratch(ctx(True, os.path.join(work, "gone"))):
        pass
    check("a missing test root is refused", False, "it went on")
except k.Refused as e:
    check("a missing test root is refused", "cannot make the admin scratch folder" in str(e), str(e))
check("mkdtemp was never called (no fallback to /tmp)", calls == [], repr(calls))
def failing_mkdtemp(*a, **kw):
    raise OSError("mkdtemp failing on purpose")
k.tempfile.mkdtemp = failing_mkdtemp
try:
    with k.Scratch(ctx(True, root)):
        pass
    check("a failed mkdtemp stops the verb", False, "it went on")
except OSError:
    check("a failed mkdtemp stops the verb", True)
check("...and its fresh folder is removed", listing(root) == [], repr(listing(root)))
k.tempfile.mkdtemp = recording_mkdtemp

print()
print("k2sign-admin-scratch.sh: %d passed, %d failed" % (npass, nfail))
sys.exit(1 if nfail else 0)
PY
