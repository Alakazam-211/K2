"""k2sign: the release signer that runs as the `_k2sign` macOS user.

prd-signing-user-v1 (rev 2) §4, slices S1 + S2. Installed by
scripts/k2sign/install.sh as /usr/local/libexec/k2sign/k2sign.py (root:wheel
0644) and run ONLY through the root-owned wrappers next to it:

  sudo -n -u _k2sign /usr/local/libexec/k2sign/k2sign <verb> ...   (z3thon, no password)
  sudo /usr/local/libexec/k2sign/k2sign-admin <verb> ...            (root, admin password)

The wrappers start this file with a fixed environment (env -i, fixed PATH and
HOME, umask 077) and the Command Line Tools python3 with -I (not Xcode's:
/Applications is admin-writable). This file runs only
root-owned binaries by absolute path: the pinned minisign next to it,
/usr/bin/openssl (LibreSSL), nothing from /opt/homebrew (owned by z3thon).

Signer verbs (the whole surface; anything else exits 2):
  preflight <version>
  challenge <version> <base-commit> <asset>... [--windows] [--reapprove]   -> challenge JSON
  approve <nonce>                       (DER approval signature on stdin)  -> session id
  sign <u1|u2> <asset> <version> raw|b64   (file bytes on stdin)           -> signature
  status <version>                                                         -> JSON
Admin verbs (root):
  set-approver <pem>    repin    approve <version> <base> <asset>... [--windows] [--reapprove]
  generate-u2           import u1 <key file> [--password-stdin]

Exit codes: 0 ok; 2 usage; 3 refused (one red line saying why); 4 internal.

Fail closed: every check that can't run is a refusal. Files come in on
stdin and are written to the signer's own tmp, so what is checked is what
is signed. No path from the caller is ever opened by the signer verbs.

Test mode: K2SIGN_TEST_ROOT=<dir> moves /usr/local/libexec/k2sign,
/usr/local/etc/k2sign and /var/k2sign under <dir>. It is REFUSED when the
process runs as root or as the signing user, so it can't open a hole even
if sudo's env_reset were ever loosened. Only in test mode are these read:
K2SIGN_TEST_NOW (epoch seconds), K2SIGN_TEST_SIGNER_USER (a user name that
counts as the signing user: it can only add refusals) and
K2SIGN_TEST_EVIDENCE=accept (stands in for the S3 per-file checks).
"""

import base64
import errno
import fcntl
import grp
import hashlib
import json
import os
import pwd
import re
import secrets
import shutil
import stat
import subprocess
import sys
import time

SIGNER_USER = "_k2sign"
EXIT_OK, EXIT_USAGE, EXIT_REFUSED, EXIT_INTERNAL = 0, 2, 3, 4
SYS_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"
OPENSSL = "/usr/bin/openssl"
SELF_TEST_ASSET = "k2-preflight-self-test"
VERSION_RE = re.compile(r"^(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})$")
ASSET_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
BASE_RE = re.compile(r"^[0-9a-f]{40}$")
NONCE_RE = re.compile(r"^[0-9a-f]{32}$")
# The files whose hashes pins.sha256 must hold (approver.pem once enrolled).
PINNED_LIBEXEC = ("k2sign", "k2sign-admin", "k2sign.py", "minisign")
PINNED_ETC = ("policy", "pubkeys")
POLICY_KEYS = {
    "approval", "session_hours", "challenge_minutes", "max_stdin_mb",
    "assets.u1", "assets.u2", "windows_assets", "repo",
}


class Refused(Exception):
    pass


class Usage(Exception):
    pass


# ── where things are ───────────────────────────────────────────────────────

class Ctx:
    def __init__(self, admin):
        self.admin = admin
        self.test_root = os.environ.get("K2SIGN_TEST_ROOT", "")
        self.test = bool(self.test_root)
        self.euid = os.geteuid()
        try:
            self.signer = pwd.getpwnam(SIGNER_USER)
        except KeyError:
            self.signer = None
        self._check_identity()
        r = self.test_root
        self.libexec = r + "/usr/local/libexec/k2sign"
        self.etc = r + "/usr/local/etc/k2sign"
        self.var = r + "/var/k2sign"
        self.minisign = self.libexec + "/minisign"
        self.keys = self.var + "/keys"
        self.state = self.var + "/state"
        self.audit_dir = self.var + "/audit"
        self.tmp = self.var + "/tmp"

    def _check_identity(self):
        if self.test:
            if not os.path.isabs(self.test_root) or not os.path.isdir(self.test_root):
                raise Refused("K2SIGN_TEST_ROOT must be an existing absolute directory")
            refused_uids = {0}
            if self.signer is not None:
                refused_uids.add(self.signer.pw_uid)
            extra = os.environ.get("K2SIGN_TEST_SIGNER_USER", "")
            if extra:
                try:
                    refused_uids.add(pwd.getpwnam(extra).pw_uid)
                except KeyError:
                    raise Refused("K2SIGN_TEST_SIGNER_USER names no user: %s" % extra)
            if self.euid in refused_uids:
                raise Refused("test mode (K2SIGN_TEST_ROOT) is refused as %s: the signing user and root never take test overrides"
                              % pwd.getpwuid(self.euid).pw_name)
            return
        if self.admin:
            if self.euid != 0:
                raise Refused("k2sign-admin runs only as root (sudo /usr/local/libexec/k2sign/k2sign-admin ...)")
            if self.signer is None:
                raise Refused("there is no %s user (run scripts/k2sign/install.sh first)" % SIGNER_USER)
        else:
            if self.signer is None or self.euid != self.signer.pw_uid:
                raise Refused("k2sign runs only as %s (sudo -n -u %s /usr/local/libexec/k2sign/k2sign ...)"
                              % (SIGNER_USER, SIGNER_USER))

    def now(self):
        if self.test and os.environ.get("K2SIGN_TEST_NOW"):
            return int(os.environ["K2SIGN_TEST_NOW"])
        return int(time.time())

    def test_evidence_accept(self):
        return self.test and os.environ.get("K2SIGN_TEST_EVIDENCE") == "accept"

    def own(self, path):
        """Admin writes under /var/k2sign belong to the signing user."""
        if self.admin and not self.test:
            os.chown(path, self.signer.pw_uid, self.signer.pw_gid)


# ── small helpers ──────────────────────────────────────────────────────────

def say(msg):
    sys.stderr.write("k2sign: %s\n" % msg)


def iso(epoch):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(epoch))


def hhmm(epoch):
    return time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime(epoch))


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def vtuple(v):
    return tuple(int(x) for x in v.split("."))


def check_version(v):
    if not VERSION_RE.match(v or ""):
        raise Usage("bad version %r (x.y.z)" % v)
    return v


def check_asset_name(a):
    if not ASSET_RE.match(a or ""):
        raise Usage("bad asset name %r" % a)
    return a


def write_file(ctx, path, data, mode=0o600):
    tmp = "%s.tmp-%s" % (path, secrets.token_hex(4))
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data if isinstance(data, bytes) else data.encode())
            f.flush()
            os.fsync(f.fileno())
        os.chmod(tmp, mode)
        ctx.own(tmp)
        os.rename(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def mkdirs(ctx, path, mode=0o700):
    if not os.path.isdir(path):
        os.makedirs(path, mode)
        os.chmod(path, mode)
        ctx.own(path)


def mode_of(path):
    return stat.S_IMODE(os.lstat(path).st_mode)


def run(argv, stdin=None, input_bytes=None):
    """Run a root-owned tool by absolute path in a fixed environment."""
    if not os.path.isabs(argv[0]):
        raise Refused("internal: refusing to run a tool by a relative name: %s" % argv[0])
    p = subprocess.run(argv, stdin=stdin, input=input_bytes, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       env={"PATH": SYS_PATH, "LANG": "C"}, cwd="/")
    return p.returncode, p.stdout, p.stderr


# ── pins, policy, public keys ──────────────────────────────────────────────

def check_root_owned(ctx, path):
    """Production: a file the signer trusts is root-owned and writable by root only."""
    if ctx.test:
        return
    st = os.lstat(path)
    if stat.S_ISLNK(st.st_mode):
        raise Refused("%s is a symlink" % path)
    if st.st_uid != 0 or (st.st_mode & 0o022):
        raise Refused("%s must be owned by root and writable only by root" % path)


def pinned_files(ctx):
    files = [("libexec/" + n, os.path.join(ctx.libexec, n)) for n in PINNED_LIBEXEC]
    files += [("etc/" + n, os.path.join(ctx.etc, n)) for n in PINNED_ETC]
    approver = os.path.join(ctx.etc, "approver.pem")
    if os.path.exists(approver):
        files.append(("etc/approver.pem", approver))
    return files


def compute_pins(ctx):
    lines = []
    for name, path in pinned_files(ctx):
        if not os.path.isfile(path):
            raise Refused("missing %s (re-run scripts/k2sign/install.sh)" % path)
        lines.append("%s  %s" % (sha256_file(path), name))
    return "\n".join(lines) + "\n"


def check_pins(ctx):
    pins_path = os.path.join(ctx.etc, "pins.sha256")
    if not os.path.isfile(pins_path):
        raise Refused("no pins.sha256 (re-run scripts/k2sign/install.sh)")
    for d in (ctx.libexec, ctx.etc):
        check_root_owned(ctx, d)
    check_root_owned(ctx, pins_path)
    want = {}
    for line in open(pins_path).read().splitlines():
        if not line.strip():
            continue
        m = re.match(r"^([0-9a-f]{64})  (\S+)$", line)
        if not m:
            raise Refused("pins.sha256 has a bad line: %r" % line)
        want[m.group(2)] = m.group(1)
    for name, path in pinned_files(ctx):
        short = name.split("/", 1)[1]
        if name not in want:
            raise Refused("pins mismatch: %s (not pinned)" % short)
        if not os.path.isfile(path):
            raise Refused("pins mismatch: %s (missing)" % short)
        check_root_owned(ctx, path)
        if sha256_file(path) != want[name]:
            raise Refused("pins mismatch: %s" % short)
    for name in want:
        if name not in dict(pinned_files(ctx)):
            raise Refused("pins mismatch: %s (pinned but missing)" % name.split("/", 1)[1])


def check_var(ctx):
    """/var/k2sign is the signing user's alone (0700)."""
    if not os.path.isdir(ctx.var):
        raise Refused("%s is missing (run scripts/k2sign/install.sh)" % ctx.var)
    if mode_of(ctx.var) != 0o700:
        raise Refused("%s must be mode 700" % ctx.var)
    if not ctx.test and os.lstat(ctx.var).st_uid != ctx.signer.pw_uid:
        raise Refused("%s must be owned by %s" % (ctx.var, SIGNER_USER))


def load_policy(ctx):
    pol = {}
    for raw in open(os.path.join(ctx.etc, "policy")).read().splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if "=" not in line:
            raise Refused("policy: bad line %r" % raw)
        k, v = (x.strip() for x in line.split("=", 1))
        if k not in POLICY_KEYS:
            raise Refused("policy: unknown key %r" % k)
        pol[k] = v
    missing = POLICY_KEYS - set(pol)
    if missing:
        raise Refused("policy: missing %s" % ", ".join(sorted(missing)))
    if pol["approval"] not in ("on", "off"):
        raise Refused("policy: approval must be on or off")
    for k in ("session_hours", "challenge_minutes", "max_stdin_mb"):
        if not re.match(r"^[1-9][0-9]{0,4}$", pol[k]):
            raise Refused("policy: %s must be a positive number" % k)
    return pol


def allowed_assets(pol, slot, version):
    return {a.replace("{version}", version) for a in pol["assets." + slot].split()}


def windows_assets(pol, version):
    return {a.replace("{version}", version) for a in pol["windows_assets"].split()}


def b64_key_id(line):
    raw = base64.b64decode(line, validate=True)
    if len(raw) != 42 or raw[:2] != b"Ed":
        raise ValueError("not a minisign public key line")
    return raw[2:10][::-1].hex().upper()


def load_pubkeys(ctx):
    keys = {}
    for raw in open(os.path.join(ctx.etc, "pubkeys")).read().splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) != 2 or parts[0] not in ("u1", "u2", "u3"):
            raise Refused("pubkeys: bad line %r" % raw)
        try:
            b64_key_id(parts[1])
        except Exception:
            raise Refused("pubkeys: %s is not a minisign public key line" % parts[0])
        if parts[0] in keys:
            raise Refused("pubkeys: %s listed twice" % parts[0])
        keys[parts[0]] = parts[1]
    return keys


def pinned_pubkey(ctx, slot):
    keys = load_pubkeys(ctx)
    if slot not in keys:
        if slot == "u2":
            raise Refused("no U2 public key pinned: the ceremony's public keys haven't been installed "
                          "(after they land, re-run scripts/k2sign/install.sh from that commit)")
        raise Refused("no %s public key pinned" % slot.upper())
    return keys[slot]


# ── state (one lock for every change) ──────────────────────────────────────

class Locked:
    def __init__(self, ctx):
        self.ctx = ctx

    def __enter__(self):
        mkdirs(self.ctx, self.ctx.state)
        path = os.path.join(self.ctx.state, ".lock")
        self.fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
        self.ctx.own(path)
        fcntl.flock(self.fd, fcntl.LOCK_EX)
        return self

    def __exit__(self, *a):
        fcntl.flock(self.fd, fcntl.LOCK_UN)
        os.close(self.fd)


def state_dirs(ctx):
    for d in ("challenges", "sessions", "assets"):
        mkdirs(ctx, os.path.join(ctx.state, d))


def read_json(path):
    with open(path) as f:
        return json.load(f)


def last_version(ctx):
    p = os.path.join(ctx.state, "last_version")
    if not os.path.exists(p):
        return None
    v = open(p).read().strip()
    if not VERSION_RE.match(v):
        raise Refused("state/last_version is corrupt: %r" % v)
    return v


def session_path(ctx, version):
    return os.path.join(ctx.state, "sessions", version + ".json")


def load_session(ctx, version):
    p = session_path(ctx, version)
    return read_json(p) if os.path.exists(p) else None


def audit(ctx, version, event, slot="", asset="", sha="", key_id="", tc="", detail=""):
    d = os.path.join(ctx.audit_dir, version)
    mkdirs(ctx, ctx.audit_dir)
    mkdirs(ctx, d)
    path = os.path.join(d, "signatures.tsv")
    new = not os.path.exists(path)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    with os.fdopen(fd, "a") as f:
        if new:
            f.write("time\tevent\tslot\tasset\tsha256\tkey_id\ttrusted_comment\tdetail\n")
        clean = lambda s: str(s).replace("\t", "\\t").replace("\n", " ")
        f.write("\t".join(clean(x) for x in (iso(ctx.now()), event, slot, asset, sha, key_id, tc, detail)) + "\n")
    ctx.own(path)


# ── minisign ───────────────────────────────────────────────────────────────

def keyfiles(ctx, slot):
    return os.path.join(ctx.keys, slot + ".key"), os.path.join(ctx.keys, slot + ".pw")


def check_keyfiles(ctx, slot):
    if not os.path.isdir(ctx.keys) or mode_of(ctx.keys) != 0o700:
        raise Refused("%s must exist with mode 700" % ctx.keys)
    key, pw = keyfiles(ctx, slot)
    for p in (key, pw):
        if not os.path.isfile(p):
            raise Refused("%s key not installed: %s missing (%s)" % (
                slot.upper(), os.path.basename(p),
                "k2sign-admin generate-u2" if slot == "u2" else "k2sign-admin import u1 <key file>"))
        if mode_of(p) != 0o600:
            raise Refused("%s must be mode 600" % p)
    return key, pw


def minisign_version(ctx):
    rc, out, err = run([ctx.minisign, "-v"])
    m = re.match(rb"^minisign ([0-9]+)\.([0-9]+)", out + err)
    if rc != 0 or not m:
        raise Refused("the pinned minisign does not run")
    if (int(m.group(1)), int(m.group(2))) < (0, 11):
        raise Refused("the pinned minisign is older than 0.11")
    return "%s.%s" % (m.group(1).decode(), m.group(2).decode())


def sig_algo(sig_text):
    lines = sig_text.splitlines()
    if len(lines) < 4 or not lines[0].startswith("untrusted comment:"):
        raise Refused("minisign wrote something that is not a signature")
    return base64.b64decode(lines[1])[:2].decode("latin-1"), b64_key_id_sig(lines[1])


def b64_key_id_sig(line):
    raw = base64.b64decode(line)
    return raw[2:10][::-1].hex().upper()


def minisign_sign(ctx, slot, path, tc):
    key, pw = check_keyfiles(ctx, slot)
    uc = "signature from tauri secret key" if slot == "u1" else "signature from K2 update key"
    sig = path + ".minisig"
    with open(pw, "rb") as pwf:
        rc, out, err = run([ctx.minisign, "-S", "-s", key, "-m", path, "-x", sig, "-c", uc, "-t", tc], stdin=pwf)
    if rc != 0 or not os.path.isfile(sig):
        raise Refused("minisign could not sign with %s (%s)" % (slot.upper(), err.decode(errors="replace").strip()[:200]))
    text = open(sig).read()
    os.unlink(sig)
    return text


def minisign_verify(ctx, pub, path, sig_text):
    sigf = path + ".verify.minisig"
    write_file(ctx, sigf, sig_text)
    try:
        rc, out, err = run([ctx.minisign, "-V", "-P", pub, "-m", path, "-x", sigf, "-Q"])
    finally:
        os.unlink(sigf)
    if rc != 0:
        return None
    return out.decode().strip().splitlines()[-1] if out.strip() else ""


def sign_and_check(ctx, slot, path, asset, version):
    """Sign `path` with `slot`, then verify with the PINNED key, the way installs check."""
    ts = "timestamp:%d" % ctx.now()
    tc = "%s\tfile:%s" % (ts, asset) if slot == "u1" else "%s\tfile:%s\tversion:%s" % (ts, asset, version)
    text = minisign_sign(ctx, slot, path, tc)
    algo, key_id = sig_algo(text)
    pub = pinned_pubkey(ctx, slot)
    want_id = b64_key_id(pub)
    if algo != "ED":
        raise Refused("%s: the %s signature is not prehashed (%r)" % (asset, slot.upper(), algo))
    if key_id != want_id:
        raise Refused("%s: signed by key %s, but the pinned %s key is %s" % (asset, key_id, slot.upper(), want_id))
    got = minisign_verify(ctx, pub, path, text)
    if got is None:
        raise Refused("%s: the new %s signature does not verify with the pinned key %s" % (asset, slot.upper(), want_id))
    if got != tc:
        raise Refused("%s: trusted comment round-trip mismatch" % asset)
    return text, key_id, tc


# ── evidence (S3 is not built yet) ─────────────────────────────────────────

def check_evidence(ctx, slot, asset, version, path, session):
    """The per-file checks the signer runs itself (prd §4.5). Not built in
    S1/S2: every real asset is refused, so a session alone never signs."""
    if ctx.test_evidence_accept():
        return
    raise Refused("%s: the per-file checks are not built yet (S3); this k2sign signs only its preflight self-test" % asset)


# ── verbs ──────────────────────────────────────────────────────────────────

def read_stdin_to(ctx, path, limit):
    n = 0
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as out:
        src = sys.stdin.buffer
        while True:
            chunk = src.read(1 << 20)
            if not chunk:
                break
            n += len(chunk)
            if n > limit:
                raise Refused("input is larger than the policy's max_stdin_mb")
            out.write(chunk)
    return n


class Scratch:
    def __init__(self, ctx):
        self.ctx = ctx

    def __enter__(self):
        mkdirs(self.ctx, self.ctx.tmp)
        self.dir = os.path.join(self.ctx.tmp, secrets.token_hex(8))
        os.mkdir(self.dir, 0o700)
        return self.dir

    def __exit__(self, *a):
        shutil.rmtree(self.dir, ignore_errors=True)


def verb_preflight(ctx, args):
    if len(args) != 1:
        raise Usage("preflight <version>")
    version = check_version(args[0])
    check_pins(ctx)
    pol = load_policy(ctx)
    check_var(ctx)
    mv = minisign_version(ctx)
    keys = load_pubkeys(ctx)
    pinned_pubkey(ctx, "u1")
    pinned_pubkey(ctx, "u2")
    if pol["approval"] == "on" and not os.path.isfile(os.path.join(ctx.etc, "approver.pem")):
        raise Refused("no approver key enrolled (open K2 Release Approve → Enrol, then k2sign-admin set-approver)")
    ids = {}
    with Locked(ctx), Scratch(ctx) as d:
        p = os.path.join(d, SELF_TEST_ASSET)
        write_file(ctx, p, "K2 updater signing preflight %s\n" % version)
        for slot in ("u1", "u2"):
            text, key_id, tc = sign_and_check(ctx, slot, p, SELF_TEST_ASSET, version)
            ids[slot] = key_id
            audit(ctx, version, "SIGNED", slot, SELF_TEST_ASSET, sha256_file(p), key_id, tc, "preflight")
    out = {"ok": True, "version": version, "minisign": mv, "approval": pol["approval"],
           "u1": ids["u1"], "u2": ids["u2"], "u3": b64_key_id(keys["u3"]) if "u3" in keys else None}
    print(json.dumps(out, sort_keys=True))
    say("preflight ok: U1 %s and U2 %s sign and verify with the pinned keys; approval %s"
        % (ids["u1"], ids["u2"], pol["approval"]))


def parse_approval_args(args, what):
    windows = reapprove = False
    rest = []
    for a in args:
        if a == "--windows":
            windows = True
        elif a == "--reapprove":
            reapprove = True
        elif a.startswith("--"):
            raise Usage("%s: unknown flag %s" % (what, a))
        else:
            rest.append(a)
    if len(rest) < 3:
        raise Usage("%s <version> <base-commit> <asset>... [--windows] [--reapprove]" % what)
    version = check_version(rest[0])
    base = rest[1]
    if not BASE_RE.match(base):
        raise Usage("base commit must be 40 lowercase hex (git rev-parse HEAD)")
    assets = []
    for a in rest[2:]:
        check_asset_name(a)
        if a in assets:
            raise Usage("asset %s listed twice" % a)
        assets.append(a)
    return version, base, sorted(assets), windows, reapprove


def check_new_session(ctx, pol, version, base, assets, windows, reapprove):
    """The rules a new approval must pass (challenge, approve, admin approve)."""
    win = windows_assets(pol, version)
    signable = allowed_assets(pol, "u1", version) | allowed_assets(pol, "u2", version)
    for a in assets:
        if a == SELF_TEST_ASSET:
            raise Refused("%s is signed by preflight only; don't list it" % a)
        if a not in signable:
            raise Refused("asset %s is not signable" % a)
        if a in win and not windows:
            raise Refused("Windows setup not approved for %s (pass --windows to include it)" % version)
    if windows and not any(a in win for a in assets):
        raise Refused("--windows without a Windows setup in the asset list")
    existing = load_session(ctx, version)
    if existing is not None:
        if not reapprove:
            raise Refused("%s already has an approved session (approved %s, base %s); re-approving needs --reapprove"
                          % (version, hhmm(existing["approved"]), existing["base_commit"][:12]))
        if existing["base_commit"] != base:
            raise Refused("base commit differs from %s's approved session (%s, now %s)"
                          % (version, existing["base_commit"][:12], base[:12]))
    elif reapprove:
        raise Refused("no earlier session for %s to re-approve" % version)
    last = last_version(ctx)
    if existing is None and last is not None and vtuple(version) <= vtuple(last):
        raise Refused("%s is not newer than %s" % (version, last))


def verb_challenge(ctx, args):
    version, base, assets, windows, reapprove = parse_approval_args(args, "challenge")
    check_pins(ctx)
    pol = load_policy(ctx)
    if pol["approval"] != "on":
        raise Refused("approval is off in the policy: there is nothing to approve")
    if not os.path.isfile(os.path.join(ctx.etc, "approver.pem")):
        raise Refused("no approver key enrolled (k2sign-admin set-approver)")
    with Locked(ctx):
        state_dirs(ctx)
        check_new_session(ctx, pol, version, base, assets, windows, reapprove)
        now = ctx.now()
        nonce = secrets.token_hex(16)
        challenge = {
            "v": 1, "kind": "k2-release-approval", "version": version, "base_commit": base,
            "assets": assets, "slots": ["u1", "u2"], "windows": windows, "reapproval": reapprove,
            "max_signatures": len(assets) * 2 + 4,
            "expires": iso(now + int(pol["session_hours"]) * 3600),
            "nonce": nonce, "issued": iso(now),
        }
        text = json.dumps(challenge, sort_keys=True, separators=(",", ":"))
        rec = {"challenge": text, "issued": now, "used": False}
        write_file(ctx, os.path.join(ctx.state, "challenges", nonce + ".json"), json.dumps(rec))
    sys.stdout.write(text)


def openssl_verify(ctx, data_path, sig_path):
    pem = os.path.join(ctx.etc, "approver.pem")
    rc, out, err = run([OPENSSL, "dgst", "-sha256", "-verify", pem, "-signature", sig_path, data_path])
    return rc == 0 and out.strip() == b"Verified OK"


def write_session(ctx, pol, challenge, by):
    now = ctx.now()
    version = challenge["version"]
    sess = {
        "version": version, "base_commit": challenge["base_commit"], "assets": challenge["assets"],
        "windows": challenge["windows"], "max_signatures": challenge["max_signatures"],
        "approved": now, "expires": now + int(pol["session_hours"]) * 3600,
        "nonce": challenge["nonce"], "reapproval": challenge["reapproval"], "by": by,
        "count": 0, "signed": [],
    }
    write_file(ctx, session_path(ctx, version), json.dumps(sess, sort_keys=True))
    last = last_version(ctx)
    if last is None or vtuple(version) > vtuple(last):
        write_file(ctx, os.path.join(ctx.state, "last_version"), version + "\n")
    audit(ctx, version, "APPROVED-BY-ADMIN" if by == "admin" else "APPROVED", detail="base %s; %s; expires %s%s" % (
        challenge["base_commit"], " ".join(challenge["assets"]), iso(sess["expires"]),
        "; reapproval" if challenge["reapproval"] else ""))
    return sess


def verb_approve(ctx, args):
    if len(args) != 1 or not NONCE_RE.match(args[0]):
        raise Usage("approve <nonce>   (the DER approval signature on stdin)")
    nonce = args[0]
    check_pins(ctx)
    pol = load_policy(ctx)
    if pol["approval"] != "on":
        raise Refused("approval is off in the policy")
    with Locked(ctx), Scratch(ctx) as d:
        state_dirs(ctx)
        cpath = os.path.join(ctx.state, "challenges", nonce + ".json")
        if not os.path.exists(cpath):
            raise Refused("unknown challenge %s" % nonce)
        rec = read_json(cpath)
        if rec["used"]:
            raise Refused("challenge already used")
        if ctx.now() > rec["issued"] + int(pol["challenge_minutes"]) * 60:
            raise Refused("challenge expired")
        sig = os.path.join(d, "approval.der")
        n = read_stdin_to(ctx, sig, 1024)
        if n == 0:
            raise Refused("no approval signature on stdin")
        data = os.path.join(d, "challenge.json")
        write_file(ctx, data, rec["challenge"])
        challenge = json.loads(rec["challenge"])
        if not openssl_verify(ctx, data, sig):
            audit(ctx, challenge["version"], "REFUSED-APPROVAL", detail="signature does not verify; nonce %s" % nonce)
            raise Refused("approval signature does not verify")
        rec["used"] = True
        write_file(ctx, cpath, json.dumps(rec))
        # The world may have moved since the challenge (another session).
        check_new_session(ctx, pol, challenge["version"], challenge["base_commit"], challenge["assets"],
                          challenge["windows"], challenge["reapproval"])
        sess = write_session(ctx, pol, challenge, "touchid")
    print(sess["nonce"])
    say("signing session for %s approved until %s" % (sess["version"], hhmm(sess["expires"])))


def verb_status(ctx, args):
    if len(args) != 1:
        raise Usage("status <version>")
    version = check_version(args[0])
    check_pins(ctx)
    pol = load_policy(ctx)
    sess = load_session(ctx, version)
    out = {"version": version, "approval": pol["approval"], "last_version": last_version(ctx), "session": None}
    if sess is not None:
        out["session"] = {
            "base_commit": sess["base_commit"], "assets": sess["assets"], "windows": sess["windows"],
            "approved": iso(sess["approved"]), "expires": iso(sess["expires"]),
            "valid": ctx.now() <= sess["expires"], "by": sess["by"], "count": sess["count"],
            "max_signatures": sess["max_signatures"], "reapproval": sess["reapproval"],
        }
    apath = os.path.join(ctx.state, "assets", version + ".tsv")
    out["signed"] = open(apath).read().split("\n")[:-1] if os.path.exists(apath) else []
    print(json.dumps(out, sort_keys=True))


def verb_sign(ctx, args):
    if len(args) != 4:
        raise Usage("sign <u1|u2> <asset> <version> raw|b64   (file bytes on stdin)")
    slot, asset, version, form = args
    if slot not in ("u1", "u2"):
        raise Usage("slot must be u1 or u2")
    check_asset_name(asset)
    check_version(version)
    if form not in ("raw", "b64"):
        raise Usage("form must be raw or b64")
    check_pins(ctx)
    pol = load_policy(ctx)
    if asset == SELF_TEST_ASSET:
        raise Refused("%s is signed by preflight only" % asset)
    if asset not in allowed_assets(pol, slot, version):
        raise Refused("asset %s is not signable" % asset)
    with Locked(ctx), Scratch(ctx) as d:
        state_dirs(ctx)
        sess = None
        if pol["approval"] == "on":
            sess = load_session(ctx, version)
            if sess is None:
                raise Refused("no approved signing session for %s" % version)
            if ctx.now() > sess["expires"]:
                raise Refused("the signing session for %s expired at %s; re-approve (release.sh asks)"
                              % (version, hhmm(sess["expires"])))
            if asset not in sess["assets"]:
                raise Refused("%s is not in %s's approved asset list" % (asset, version))
            if asset in windows_assets(pol, version) and not sess["windows"]:
                raise Refused("Windows setup not approved for %s" % version)
        path = os.path.join(d, "in")
        read_stdin_to(ctx, path, int(pol["max_stdin_mb"]) << 20)
        sha = sha256_file(path)
        apath = os.path.join(ctx.state, "assets", version + ".tsv")
        first = {}
        if os.path.exists(apath):
            for line in open(apath).read().splitlines():
                a, s = line.split("\t")
                first[a] = s
        if asset in first and first[asset] != sha:
            audit(ctx, version, "REFUSED-SWAP", slot, asset, sha, detail="already signed as %s" % first[asset])
            raise Refused("REFUSED-SWAP: %s already signed as %s" % (asset, first[asset]))
        key = "%s %s %s" % (slot, asset, sha)
        again = sess is not None and key in sess["signed"]
        if sess is not None and not again and sess["count"] >= sess["max_signatures"]:
            raise Refused("the session for %s reached its %d signatures" % (version, sess["max_signatures"]))
        check_evidence(ctx, slot, asset, version, path, sess)
        text, key_id, tc = sign_and_check(ctx, slot, path, asset, version)
        if asset not in first:
            with open(apath, "a") as f:
                f.write("%s\t%s\n" % (asset, sha))
            ctx.own(apath)
        if sess is not None and not again:
            sess["count"] += 1
            sess["signed"].append(key)
            write_file(ctx, session_path(ctx, version), json.dumps(sess, sort_keys=True))
        audit(ctx, version, "SIGNED", slot, asset, sha, key_id, tc, "re-sign, same bytes" if again else "")
    if form == "b64":
        sys.stdout.write(base64.b64encode(text.encode()).decode())
    else:
        sys.stdout.write(text)


# ── admin verbs (root) ─────────────────────────────────────────────────────

def repin(ctx):
    write_pins = compute_pins(ctx)
    path = os.path.join(ctx.etc, "pins.sha256")
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        f.write(write_pins)
    os.chmod(tmp, 0o644)
    os.rename(tmp, path)
    say("pins.sha256 rewritten:\n" + write_pins.rstrip())


def admin_set_approver(ctx, args):
    if len(args) != 1:
        raise Usage("set-approver <approver.pem>")
    src = args[0]
    if not os.path.isabs(src):
        raise Usage("set-approver needs an absolute path (the wrapper runs from /)")
    data = open(src, "rb").read()
    if len(data) > 4096 or b"-----BEGIN PUBLIC KEY-----" not in data:
        raise Refused("%s is not a PEM public key" % src)
    with Scratch(ctx) as d:
        p = os.path.join(d, "approver.pem")
        write_file(ctx, p, data, 0o644)
        rc, out, err = run([OPENSSL, "ec", "-pubin", "-in", p, "-noout", "-text"])
        if rc != 0 or (b"prime256v1" not in out + err and b"P-256" not in out + err):
            raise Refused("%s is not a P-256 public key" % src)
        dest = os.path.join(ctx.etc, "approver.pem")
        tmp = dest + ".tmp"
        shutil.copyfile(p, tmp)
        os.chmod(tmp, 0o644)
        os.rename(tmp, dest)
    repin(ctx)
    say("approver key installed: sha256 %s" % sha256_file(os.path.join(ctx.etc, "approver.pem")))


def admin_approve(ctx, args):
    version, base, assets, windows, reapprove = parse_approval_args(args, "approve")
    check_pins(ctx)
    pol = load_policy(ctx)
    with Locked(ctx):
        state_dirs(ctx)
        check_new_session(ctx, pol, version, base, assets, windows, reapprove)
        challenge = {"version": version, "base_commit": base, "assets": assets, "windows": windows,
                     "reapproval": reapprove, "max_signatures": len(assets) * 2 + 4,
                     "nonce": secrets.token_hex(16)}
        sess = write_session(ctx, pol, challenge, "admin")
    print(sess["nonce"])
    say("break-glass session for %s approved by an admin until %s (logged as APPROVED-BY-ADMIN)"
        % (version, hhmm(sess["expires"])))


def admin_generate_u2(ctx, args):
    if args:
        raise Usage("generate-u2")
    check_pins(ctx)
    mkdirs(ctx, ctx.keys)
    key, pw = keyfiles(ctx, "u2")
    if os.path.exists(key) or os.path.exists(pw):
        raise Refused("U2 already exists in %s; this runs once (rotation: docs/release/updater-key-ceremony.md)" % ctx.keys)
    password = secrets.token_hex(32)
    with Scratch(ctx) as d:
        pub = os.path.join(d, "u2.pub")
        try:
            rc, out, err = run([ctx.minisign, "-G", "-p", pub, "-s", key],
                               input_bytes=("%s\n%s\n" % (password, password)).encode())
            if rc != 0:
                raise Refused("minisign -G failed (%s)" % err.decode(errors="replace").strip()[:200])
            os.chmod(key, 0o600)
            ctx.own(key)
            write_file(ctx, pw, password + "\n")
            line = open(pub).read().splitlines()[1].strip()
            key_id = b64_key_id(line)
            # Self-test with the new key against its own public line.
            t = os.path.join(d, "t")
            write_file(ctx, t, "K2 U2 generate self-test\n")
            text = minisign_sign(ctx, "u2", t, "timestamp:%d\tfile:k2-u2-selftest" % ctx.now())
            if minisign_verify(ctx, line, t, text) is None:
                raise Refused("the new U2 key does not verify its own self-test")
        except BaseException:
            for p in (key, pw):
                if os.path.exists(p):
                    os.unlink(p)
            raise
        # A world-readable copy of the PUBLIC line, for the ceremony and for
        # install.sh's check that the tree's U2 is this one.
        gen = os.path.join(ctx.etc, "u2-generated.pub")
        with open(gen + ".tmp", "w") as f:
            f.write(line + "\n")
        os.chmod(gen + ".tmp", 0o644)
        os.rename(gen + ".tmp", gen)
    print(line)
    say("U2 generated inside %s (its password is random and was never shown). Key id %s." % (ctx.keys, key_id))
    say("Public line (safe to share): %s" % line)


def decode_secret_key(raw):
    if not raw.startswith(b"untrusted comment:"):
        try:
            raw = base64.b64decode(b"".join(raw.split()), validate=True)
        except Exception:
            raise Refused("not a minisign or Tauri secret key")
    if not raw.startswith(b"untrusted comment:"):
        raise Refused("not a minisign or Tauri secret key")
    return raw


def admin_import(ctx, args):
    stdin_pw = "--password-stdin" in args
    args = [a for a in args if a != "--password-stdin"]
    if len(args) != 2 or args[0] != "u1":
        raise Usage("import u1 <key file> [--password-stdin]")
    if not os.path.isabs(args[1]):
        raise Usage("import needs an absolute path (the wrapper runs from /)")
    check_pins(ctx)
    pub = pinned_pubkey(ctx, "u1")
    mkdirs(ctx, ctx.keys)
    key, pw = keyfiles(ctx, "u1")
    if os.path.exists(key) or os.path.exists(pw):
        raise Refused("U1 is already imported in %s" % ctx.keys)
    raw = decode_secret_key(open(args[1], "rb").read())
    if stdin_pw:
        password = sys.stdin.readline().rstrip("\n")
    else:
        import getpass
        password = getpass.getpass("U1's password (not shown): ")
    if not password:
        raise Refused("empty password")
    try:
        write_file(ctx, key, raw)
        write_file(ctx, pw, password + "\n")
        with Scratch(ctx) as d:
            t = os.path.join(d, "t")
            write_file(ctx, t, "K2 U1 import self-test\n")
            text = minisign_sign(ctx, "u1", t, "timestamp:%d\tfile:k2-u1-import" % ctx.now())
            algo, key_id = sig_algo(text)
            if key_id != b64_key_id(pub) or minisign_verify(ctx, pub, t, text) is None:
                raise Refused("the imported key and password don't make a signature the pinned U1 key %s verifies"
                              % b64_key_id(pub))
    except BaseException:
        for p in (key, pw):
            if os.path.exists(p):
                os.unlink(p)
        raise
    say("U1 imported into %s; it signs and verifies with the pinned key %s." % (ctx.keys, b64_key_id(pub)))


SIGNER_VERBS = {"preflight": verb_preflight, "challenge": verb_challenge, "approve": verb_approve,
                "sign": verb_sign, "status": verb_status}
def admin_repin(ctx, args):
    if args:
        raise Usage("repin")
    repin(ctx)


ADMIN_VERBS = {"set-approver": admin_set_approver, "repin": admin_repin, "approve": admin_approve,
               "generate-u2": admin_generate_u2, "import": admin_import}


def main(argv):
    if len(argv) < 2 or argv[1] not in ("signer", "admin"):
        say("usage: run through /usr/local/libexec/k2sign/k2sign or k2sign-admin")
        return EXIT_USAGE
    admin = argv[1] == "admin"
    verbs = ADMIN_VERBS if admin else SIGNER_VERBS
    rest = argv[2:]
    try:
        if not rest or rest[0] not in verbs:
            raise Usage("verbs: " + " ".join(sorted(verbs)))
        ctx = Ctx(admin)
        verbs[rest[0]](ctx, rest[1:])
        return EXIT_OK
    except Usage as e:
        say("usage: %s" % e)
        return EXIT_USAGE
    except Refused as e:
        sys.stderr.write("\033[1;31mk2sign: refused: %s\033[0m\n" % e)
        return EXIT_REFUSED
    except Exception as e:  # anything unexpected is an internal error, never success
        say("internal error: %s: %s" % (type(e).__name__, e))
        return EXIT_INTERNAL


if __name__ == "__main__":
    sys.exit(main(sys.argv))
