#!/usr/bin/env python3
"""Two servers of different brands talk to each other (brand step C4).

    python3 tests/interop/harness.py --default-daemon BIN --brand-daemon BIN \\
        --default-brand brand/k2 --brand brand/xy --out DIR [--keep]

Run by tests/interop/two-brands.sh, which builds both daemons from one
commit. Starts real k2-daemon children with throwaway homes (a folder under
$TMPDIR per server, never a real home), on free ports, and runs one script
in three sessions:

  baseline  A and B are the default brand's daemon
  mixed     A is the default brand's, B the other brand's
  swapped   A is the other brand's, B the default brand's

The script, through the real paths:
  1. both answer /boot-status (each names its own command);
  2. federation pairing v2 (init, respond, reveal, verify, both confirm with
     the same code), as an owner pairs two servers by hand;
  3. A reads B's roster through federation;
  4. a message each way that lands (an inbox tray, to `<peer>::<ws>::<agent>`);
  5. a refused call each way: live text to an agent whose "Let agents on
     other servers message this agent" switch is off, so the peer declines
     and the answer carries a hint (display text);
  6. a vault hand-off A to B: refused first (a person must confirm), then
     confirmed with an Admin login's password, pending on B, accepted on B;
  7. Postal through a test plane (tests/interop/postal_plane.py): both turn
     Postal on, claim an address, A asks B to be friends, each owner types
     the code shown on the other box, then a message each way through hold.

Every request and answer is captured, head and body:
  - owner: the script's own calls to each server (the CLI's session);
  - peer: what one server sends the other, through a door in front of each
    (tests/interop/door.py). The door adds X-Forwarded-For with a TEST-NET
    address, so each server reads its peer as off the machine, as it would
    be on two hosts;
  - postal: what each server sends the test plane, and its answers.
Captures go to DIR/<session>.jsonl (the format is in wire_compare.py), with
each server's log beside them. Then tests/interop/wire_compare.py compares
baseline with mixed and baseline with swapped.

Lines `M <what>=<rc>` say what passed; the last line names every failure.
Exits non-zero when anything failed. Python stdlib only. Runs daemons and
opens ports: on K2 Compute only.
"""
import argparse
import base64
import hashlib
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import door as door_mod  # noqa: E402
from capture import Recorder, captured, redact_target  # noqa: E402
import httpwire  # noqa: E402
import postal_plane  # noqa: E402
import wire_compare  # noqa: E402

# The home folder and the workspace folder every brand shares in tier 1
# (the eval's 3.5, "sockets and port files a peer reads": tier 2 renames it
# per brand on one machine). A brand that names its own makes the wait for
# daemon.port fail with the path it looked in.
DOT = ".k2"
LABELS = {"A": "acme", "B": "initech"}
WORKSPACES = {"A": "sales", "B": "ops"}
HANDLES = {"A": "alice", "B": "bob"}
XFF = {"A": "192.0.2.11", "B": "192.0.2.12"}
# What each session's capture must hold, or the traffic went around the
# door or the plane: (channel, method, path) seen at least once each way
# where it goes both ways.
REQUIRED = [
    ("peer", "A", "B", "POST", "/cli/federation/inbound"),
    ("peer", "B", "A", "POST", "/cli/federation/inbound"),
    ("peer", "A", "B", "GET", "/cli/federation/roster"),
    ("peer", "A", "B", "POST", "/cli/federation/vault-handoff"),
    ("postal", "A", "plane", "POST", "/api/postal/v1/pairs"),
    ("postal", "B", "plane", "POST", "/api/postal/v1/hold"),
    ("postal", "A", "plane", "POST", "/api/postal/v1/hold"),
    ("postal", "A", "plane", "POST", "/api/postal/v1/hold/ack"),
    ("postal", "B", "plane", "POST", "/api/postal/v1/hold/ack"),
]
PASSWORD = "Interop-Two-Brands-9!"


class Fail(Exception):
    """A step that did not do what it must."""


def brand_of(folder):
    """{id, cli, product} from a brand folder's brand.toml (the three keys
    this needs, read by line: tomllib is not in every Python on the nodes)."""
    path = os.path.join(folder, "brand.toml")
    with open(path, encoding="utf-8") as f:
        text = f.read()
    section, out = "", {}
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip() if not raw.strip().startswith("#") else ""
        if not line:
            continue
        m = re.fullmatch(r"\[([A-Za-z0-9_.-]+)\]", line)
        if m:
            section = m.group(1)
            continue
        m = re.fullmatch(r'([A-Za-z0-9_]+)\s*=\s*"([^"]*)"', line)
        if not m:
            continue
        key = (section, m.group(1))
        if key == ("", "id"):
            out["id"] = m.group(2)
        elif key == ("names", "product"):
            out["product"] = m.group(2)
        elif key == ("cli", "name"):
            out["cli"] = m.group(2)
    for k in ("id", "cli", "product"):
        if k not in out:
            raise Fail(f"{path}: no {k} (want id, [names] product, [cli] name)")
    return out


def known_brands(brand_root):
    out = []
    for name in sorted(os.listdir(brand_root)):
        if os.path.isfile(os.path.join(brand_root, name, "brand.toml")):
            out.append(brand_of(os.path.join(brand_root, name)))
    return out


class Daemon:
    """One k2-daemon child under its own throwaway home."""

    def __init__(self, role, binary, root, plane_base, connect_token):
        self.role = role
        self.binary = binary
        self.home = os.path.join(root, role.lower())
        self.plane_base = plane_base
        self.connect_token = connect_token
        self.proc = None
        self.port = None
        self.owner = None
        self.log = os.path.join(self.home, "daemon.stderr.log")

    def dot(self):
        return os.path.join(self.home, DOT)

    def start(self):
        for d in (self.dot(), os.path.join(self.home, "tmp"), os.path.join(self.home, "run"),
                  os.path.join(self.home, "agent-shim-empty"), os.path.join(self.home, "ws")):
            os.makedirs(d, mode=0o700, exist_ok=True)
        tunnel = os.path.join(self.dot(), "tunnel.json")
        with open(tunnel, "w", encoding="utf-8") as f:
            json.dump({"token": self.connect_token, "subdomain": LABELS[self.role], "auto_start": False}, f)
        os.chmod(tunnel, 0o600)
        # A test process: nothing of the job's own K2 or K5 settings reaches it.
        env = {k: v for k, v in os.environ.items() if not k.startswith(("K2_", "K5_", "K2SO_"))}
        env.update({
            "HOME": self.home,
            "TMPDIR": os.path.join(self.home, "tmp"),
            "XDG_RUNTIME_DIR": os.path.join(self.home, "run"),
            "K2_RUNTIME_DIR": "",
            "RUNTIME_DIRECTORY": "",
            "K2_TEST_AGENT_SHIM_DIR": os.path.join(self.home, "agent-shim-empty"),
            "K2SO_WATCHDOG_DISABLED": "1",
            "K2_FEDERATION": "1",
            "K2_POSTAL_PLANE_URL": self.plane_base,
            "K2_POSTAL_EVENTS_WAIT": "2",
        })
        err = open(self.log, "ab")
        self.proc = subprocess.Popen([self.binary], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=err, cwd=self.home)
        err.close()
        port_file, token_file = os.path.join(self.dot(), "daemon.port"), os.path.join(self.dot(), "daemon.token")
        deadline = time.time() + 90
        while True:
            if self.proc.poll() is not None:
                raise Fail(f"daemon {self.role} exited with {self.proc.returncode} before it published its port (log {self.log})")
            port, owner = read_trimmed(port_file), read_trimmed(token_file)
            if port and port.isdigit() and owner:
                self.port, self.owner = int(port), owner
                break
            if time.time() > deadline:
                raise Fail(f"daemon {self.role} never published {port_file} and {token_file}")
            time.sleep(0.1)

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=15)


def read_trimmed(path):
    try:
        with open(path, encoding="utf-8") as f:
            t = f.read().strip()
    except OSError:
        return None
    return t or None


class Owner:
    """The scripted session: each server's owner, over loopback, as the CLI
    talks to its own server. Every call is captured on the owner channel."""

    def __init__(self, rec, daemons):
        self.rec = rec
        self.d = daemons

    def call(self, role, method, path, body=None, poll=False, auth=True, token=None):
        d = self.d[role]
        target = path
        if auth:
            target += ("&" if "?" in path else "?") + "token=" + (token or d.owner)
        raw = b"" if body is None else json.dumps(body).encode()
        headers = [["Host", f"127.0.0.1:{d.port}"], ["Content-Type", "application/json"],
                   ["Content-Length", str(len(raw))], ["Connection", "close"]]
        req = httpwire.Message(start=f"{method} {target} HTTP/1.1", headers=headers, body=raw)
        ans = httpwire.exchange(("127.0.0.1", d.port), req, timeout=120)
        self.rec.add("owner", "owner", role,
                     dict(method=method, target=redact_target(target), **captured(req)),
                     dict(status=ans.status, **captured(ans)), poll=poll)
        return ans.status, ans.json_or_none()

    def ok(self, role, method, path, body=None, want=200, **kw):
        st, v = self.call(role, method, path, body, **kw)
        if st != want:
            raise Fail(f"{role} {method} {path}: {st}, want {want}: {json.dumps(v)[:600]}")
        return v

    def until(self, what, secs, fn):
        """Call `fn` every half second until it answers something, at most
        `secs`. A refused connection counts as no answer yet."""
        deadline = time.time() + secs
        last = None
        while True:
            try:
                got = fn()
            except (ConnectionError, socket.timeout, httpwire.WireError) as e:
                got, last = None, e
            if got:
                return got
            if time.time() > deadline:
                raise Fail(f"timed out after {secs}s waiting for: {what}" + (f" (last error: {last})" if last else ""))
            time.sleep(0.5)

    def poll(self, what, secs, role, path, ok):
        """GET `path` on `role` (a poll on the owner channel) until `ok(answer)`;
        the answer."""
        def once():
            st, v = self.call(role, "GET", path, poll=True, auth=not path.startswith("/boot-status"))
            return v if st == 200 and isinstance(v, dict) and ok(v) else None
        return self.until(what, secs, once)


def find_file_with(root, needle):
    """A file under `root` whose bytes hold `needle`, or None."""
    for dirpath, _, files in os.walk(root):
        for name in files:
            p = os.path.join(dirpath, name)
            try:
                with open(p, "rb") as f:
                    if needle in f.read():
                        return p
            except OSError:
                continue
    return None


def free_tmp_root():
    """A short folder for the session's homes: a cell socket under a home
    (`<home>/.k2/run/cells/<id>.sock`) must fit a Unix socket path (108
    bytes on Linux), so a long $TMPDIR gives way to /tmp."""
    base = os.environ.get("TMPDIR") or "/tmp"
    if len(base) > 30:
        base = "/tmp"
    return tempfile.mkdtemp(prefix="k2ic-", dir=base)


class SessionRun:
    def __init__(self, name, bins, brands, known, out, keep):
        self.name = name
        self.bins = bins            # {"A": path, "B": path}
        self.brands = brands        # {"A": {...}, "B": {...}}
        self.known = known
        self.out = out
        self.keep = keep
        self.rec = Recorder()
        self.root = free_tmp_root()
        self.plane = None
        self.doors = {}
        self.daemons = {}

    def step(self, name):
        self.rec.step = name
        print(f"interop: {self.name}: {name}", flush=True)

    def run(self):
        try:
            self._run()
            self._covered()
            return 0
        finally:
            for d in self.daemons.values():
                d.stop()
            for dr in self.doors.values():
                dr.close()
            if self.plane:
                self.plane.close()
            self._write()
            if not self.keep:
                shutil.rmtree(self.root, ignore_errors=True)

    def _covered(self):
        """Each REQUIRED exchange is in the capture; the counts per channel."""
        seen = {(r["channel"], r["from"], r["to"], r["request"]["method"], r["request"]["target"].split("?")[0]) for r in self.rec.rows}
        counts = {}
        for r in self.rec.rows:
            counts[r["channel"]] = counts.get(r["channel"], 0) + 1
        print(f"interop: {self.name}: captured " + ", ".join(f"{counts.get(c, 0)} {c}" for c in ("owner", "peer", "postal")), flush=True)
        missing = [" ".join(k) for k in REQUIRED if k not in seen]
        if missing:
            raise Fail("the capture lacks: " + "; ".join(missing))

    def _write(self):
        vars_ = {}
        for role, d in self.daemons.items():
            if d.port:
                vars_[str(d.port)] = f"{{port:{role}}}"
            vars_[d.home] = f"{{home:{role}}}"
            # The physical path, if TMPDIR goes through a link.
            vars_[os.path.realpath(d.home)] = f"{{home:{role}}}"
        for role, dr in self.doors.items():
            vars_[str(dr.port)] = f"{{port:door-{role}}}"
        if self.plane:
            vars_[str(self.plane.port)] = "{port:plane}"
        vars_[self.root] = "{tmp}"
        meta = {"session": self.name, "default": self.brands["default"]["id"],
                "brands": {r: self.brands[r] for r in ("A", "B")}, "known_brands": self.known, "vars": vars_}
        self.rec.write(os.path.join(self.out, f"{self.name}.jsonl"), meta)
        for role, d in self.daemons.items():
            if os.path.exists(d.log):
                shutil.copyfile(d.log, os.path.join(self.out, f"{self.name}-{role}-daemon.log"))

    def _run(self):
        rec = self.rec
        creds = {r: "k2c_" + uuid.uuid4().hex + uuid.uuid4().hex[:8].upper() for r in ("A", "B")}
        self.plane = postal_plane.Plane({creds[r]: LABELS[r] for r in creds}, rec,
                                        role_of_label={LABELS[r]: r for r in LABELS})
        self.step("boot")
        for r in ("A", "B"):
            d = Daemon(r, self.bins[r], self.root, self.plane.base, creds[r])
            self.daemons[r] = d
            d.start()
        for r, d in self.daemons.items():
            self.doors[r] = door_mod.Door(d.port, r, "B" if r == "A" else "A", XFF[r], rec)
        own = Owner(rec, self.daemons)
        for r in ("A", "B"):
            v = own.poll(f"{r} ready", 90, r, "/boot-status", lambda v: v.get("phase") == "ready")
            want = self.brands[r]["cli"]
            if v.get("cliName") != want:
                raise Fail(f"{r}'s /boot-status cliName is {v.get('cliName')!r}, want {want!r}: not the build this session names")

        self.step("workspaces")
        ws = {}
        for r in ("A", "B"):
            d = self.daemons[r]
            path = os.path.join(d.home, "ws", WORKSPACES[r])
            persona = os.path.join(path, DOT, "agent", "ROLE.md")
            os.makedirs(os.path.dirname(persona), exist_ok=True)
            with open(persona, "w", encoding="utf-8") as f:
                f.write(f"---\nname: {WORKSPACES[r]}\n---\n# {WORKSPACES[r]}\n")
            v = own.ok(r, "POST", "/cli/projects/create", {"name": WORKSPACES[r], "path": path})
            if not isinstance(v, dict) or not v.get("id"):
                raise Fail(f"{r} projects/create answered no id: {v}")
            # Reachable from the other server (a tray lands), but live text
            # is declined: the switch "Let agents on other servers message
            # this agent" stays off.
            own.ok(r, "POST", f"/cli/agents-create-connections?project={quote(path)}&enable=1")
            ws[r] = {"path": path, "id": v["id"]}

        self.step("fed-pair")
        a, b = "A", "B"
        door_url = {r: f"http://127.0.0.1:{self.doors[r].port}" for r in ("A", "B")}
        offer = own.ok(a, "POST", "/cli/federation/pair/v2/init", {"label": LABELS[a], "subdomain": LABELS[a], "baseUrl": door_url[a]})
        answer = own.ok(b, "POST", "/cli/federation/pair/v2/respond", dict(offer, label=LABELS[b], subdomain=LABELS[b], baseUrl=door_url[b]))
        revealed = own.ok(a, "POST", "/cli/federation/pair/v2/reveal", answer)
        verified = own.ok(b, "POST", "/cli/federation/pair/v2/verify", {"reveal": revealed["reveal"]})
        if revealed["sas"] != verified["sas"]:
            raise Fail(f"the pairing codes differ: {revealed['sas']} on A, {verified['sas']} on B")
        caps = ["inbound", "roster"]
        own.ok(a, "POST", "/cli/federation/pair/confirm", {"fingerprint": revealed["peer_fingerprint"], "sas": revealed["sas"], "capabilities": caps})
        own.ok(b, "POST", "/cli/federation/pair/confirm", {"fingerprint": verified["peer_fingerprint"], "sas": verified["sas"], "capabilities": caps})
        for r in ("A", "B"):
            peers = own.ok(r, "GET", "/cli/federation/peers")
            rows = peers.get("peers") if isinstance(peers, dict) else peers
            if not rows or not any(str(p.get("trust", "")).lower() == "trusted" for p in rows):
                raise Fail(f"{r} has no trusted peer after the pairing: {peers}")

        self.step("fed-roster")
        own.ok(a, "GET", f"/cli/federation/peer-roster?peer={LABELS[b]}")

        self.step("fed-msg")
        for frm, to in (("A", "B"), ("B", "A")):
            marker = f"interop tray {frm} to {to}\n".encode()
            tray = {"title": f"Interop {frm} to {to}", "files": [{"name": "note.md", "bytes_b64": base64.b64encode(b"# " + marker).decode()}]}
            v = own.ok(frm, "POST", "/cli/federation/send", {"to": f"{LABELS[to]}::{ws[to]['id']}::{WORKSPACES[to]}", "tray": tray})
            if v.get("status") != "sent":
                raise Fail(f"the tray {frm} to {to} was not sent: {v}")
            own.until(f"the tray from {frm} in {to}'s workspace", 30, lambda to=to, marker=marker: find_file_with(ws[to]["path"], marker))

        self.step("fed-refused")
        for frm, to in (("A", "B"), ("B", "A")):
            v = own.ok(frm, "POST", "/cli/federation/send", {"to": f"{LABELS[to]}::{ws[to]['id']}::{WORKSPACES[to]}", "text": f"live text {frm} to {to}"})
            if v.get("status") != "declined" or v.get("reason") != "remote_instruction_disabled" or not v.get("hint"):
                raise Fail(f"the live text {frm} to {to} was not declined with a hint: {v}")

        self.step("vault")
        secret = "k2test_INTEROP_SENTINEL_" + uuid.uuid4().hex
        own.ok(a, "POST", "/cli/vault/set", {"name": "STRIPE_KEY", "kind": "api_key", "hosts": ["api.stripe.com"], "scope": "workspace",
                                             "workspace": WORKSPACES[a], "valueB64": base64.b64encode(secret.encode()).decode()})
        send = {"name": "STRIPE_KEY", "scope": "workspace", "workspace": WORKSPACES[a], "to": f"{WORKSPACES[b]}::{LABELS[b]}", "note": "for the refund job"}
        st, v = own.call(a, "POST", "/cli/vault/send", send)
        code = (v or {}).get("error", {}).get("code") if isinstance((v or {}).get("error"), dict) else None
        if (st, code) != (403, "confirm_required"):
            raise Fail(f"a person-given secret went to another server without a confirm: {st} {v}")
        secret_id = v["secretId"]
        own.ok(a, "POST", "/cli/users/add", {"username": "lee", "password": PASSWORD, "mustChangePassword": False})
        own.ok(a, "POST", "/cli/users/set-role", {"username": "lee", "role": "admin"})
        login = own.ok(a, "POST", "/cli/auth/login", {"username": "lee", "password": PASSWORD}, auth=False)["token"]
        c = own.ok(a, "POST", "/cli/vault/confirm/finish", {"action": "handoff-send", "secretId": secret_id, "method": "connect-password", "password": PASSWORD}, token=login)
        v = own.ok(a, "POST", "/cli/vault/send", dict(send, confirmToken=c["token"]), token=login)
        if v.get("status") != "pending" or v.get("cross") is not True:
            raise Fail(f"the hand-off did not reach B as pending: {v}")
        delivery = v["deliveryId"]
        inc = own.poll("the hand-off on B", 30, b, f"/cli/vault/incoming?workspace={WORKSPACES[b]}",
                       lambda v: any(i.get("deliveryId") == delivery for i in v.get("items") or []))
        item = next(i for i in inc["items"] if i.get("deliveryId") == delivery)
        if item.get("status") != "pending":
            raise Fail(f"the hand-off on B is not pending: {item}")
        dec = own.ok(b, "POST", "/cli/vault/incoming/decide", {"id": delivery, "action": "accept"})
        if (dec.get("delivery") or {}).get("status") != "landed":
            raise Fail(f"B's accept did not land the secret: {dec}")

        self.step("postal")
        addr = {r: f"{HANDLES[r]}::{LABELS[r]}.postal.bot" for r in ("A", "B")}
        for r in ("A", "B"):
            v = own.ok(r, "POST", "/cli/postal/settings", {"enabled": True})
            if v.get("enabled") is not True:
                raise Fail(f"Postal did not turn on on {r}: {v}")
        for r in ("A", "B"):
            own.ok(r, "POST", "/cli/postal/claim", {"workspace": ws[r]["path"], "address": addr[r]})
            own.poll(f"{addr[r]} on the test plane", 120, r, f"/cli/postal/status?workspace={quote(ws[r]['path'])}",
                     lambda v: (v.get("plane") or {}).get("connected") is True and (v.get("handle") or {}).get("status") == "claimed")
            own.ok(r, "POST", "/cli/postal/route", {"workspace": ws[r]["path"], "targets": "inbox", "wake": False})
        own.ok(a, "POST", "/cli/postal/invite", {"workspace": ws[a]["path"], "to": addr[b], "why": "interop"})

        def friend(r, status, extra=None):
            st, v = own.call(r, "GET", f"/cli/postal/friends?workspace={quote(ws[r]['path'])}", poll=True)
            if st != 200 or not isinstance(v, dict):
                return None
            other = addr["B" if r == "A" else "A"]
            for f in v.get("friends") or []:
                if f.get("peer") == other and f.get("status") == status and (extra is None or extra(f)):
                    return f
            return None

        fb = own.until("the request at B, with its code", 120, lambda: friend(b, "pending_in", lambda f: f.get("codeWaiting") is True))
        fa = own.until("A's handshake, with its code", 120, lambda: friend(a, "pending_in", lambda f: f.get("codeWaiting") is True))
        code_a = own.ok(a, "GET", f"/cli/postal/code?workspace={quote(ws[a]['path'])}&id={fa['id']}")["code"].replace(" ", "")
        code_b = own.ok(b, "GET", f"/cli/postal/code?workspace={quote(ws[b]['path'])}&id={fb['id']}")["code"].replace(" ", "")
        if code_a != code_b:
            raise Fail(f"the Postal codes differ: {code_a} on A, {code_b} on B")
        # Each person types the code shown on the other box.
        own.ok(b, "POST", "/cli/postal/decide", {"workspace": ws[b]["path"], "id": fb["id"], "decision": "accept", "code": code_a})
        own.ok(a, "POST", "/cli/postal/decide", {"workspace": ws[a]["path"], "id": fa["id"], "decision": "accept", "code": code_b})
        for r in ("A", "B"):
            own.until(f"friends on {r}", 120, lambda r=r: friend(r, "accepted", lambda f: isinstance(f.get("peerFingerprint"), str)))
        for frm, to in (("A", "B"), ("B", "A")):
            text = f"interop postal {frm} to {to}"
            s = own.ok(frm, "POST", "/cli/postal/send", {"workspace": ws[frm]["path"], "to": addr[to], "text": text, "wake": False})
            if s.get("status") not in ("held", "delivered") or not s.get("badge"):
                raise Fail(f"the Postal message {frm} to {to} was not sent: {s}")
            # The daemon's body_sha256: CRLF to LF, trailing space off, then SHA-256.
            sha = hashlib.sha256(text.replace("\r\n", "\n").replace("\r", "\n").rstrip().encode()).hexdigest()
            got = own.poll(f"the Postal message from {frm} on {to}", 120, to, f"/cli/msg/verify?code={s['badge']}&text_sha256={sha}",
                           lambda v: v.get("delivered") is True)
            if got.get("content") != "match" or (got.get("sender") or {}).get("address") != addr[frm] or got.get("via") != "postal":
                raise Fail(f"the Postal message on {to} did not verify: {got}")
        self.step("done")


def quote(s):
    from urllib.parse import quote as q
    return q(s, safe="")


def main(argv):
    # A cancelled job stops its daemons: each session's `finally` runs.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    ap = argparse.ArgumentParser(description="Two servers of different brands talk to each other (brand step C4).")
    ap.add_argument("--default-daemon", required=True)
    ap.add_argument("--brand-daemon", required=True)
    ap.add_argument("--default-brand", required=True, help="the default brand's folder, e.g. brand/k2")
    ap.add_argument("--brand", required=True, help="the other brand's folder, e.g. brand/xy")
    ap.add_argument("--out", required=True)
    ap.add_argument("--keep", action="store_true", help="keep the throwaway homes")
    ap.add_argument("--sessions", default="baseline,mixed,swapped")
    args = ap.parse_args(argv)
    for p in (args.default_daemon, args.brand_daemon):
        if not os.access(p, os.X_OK):
            print(f"interop: {p} is not an executable daemon", file=sys.stderr)
            return 2
    dflt, other = brand_of(args.default_brand), brand_of(args.brand)
    if dflt["id"] == other["id"]:
        print(f"interop: both brands are {dflt['id']}: nothing to compare", file=sys.stderr)
        return 2
    known = known_brands(os.path.dirname(os.path.abspath(args.default_brand)))
    os.makedirs(args.out, exist_ok=True)
    plans = {
        "baseline": ({"A": args.default_daemon, "B": args.default_daemon}, {"A": dflt, "B": dflt}),
        "mixed": ({"A": args.default_daemon, "B": args.brand_daemon}, {"A": dflt, "B": other}),
        "swapped": ({"A": args.brand_daemon, "B": args.default_daemon}, {"A": other, "B": dflt}),
    }
    failed = []
    ran = []
    for name in [s.strip() for s in args.sessions.split(",") if s.strip()]:
        if name not in plans:
            print(f"interop: no session {name!r} (have {', '.join(plans)})", file=sys.stderr)
            return 2
        bins, brands = plans[name]
        run = SessionRun(name, bins, dict(brands, default=dflt), known, args.out, args.keep)
        try:
            rc = run.run()
        except Fail as e:
            print(f"interop: {name}: FAIL at {run.rec.step}: {e}", flush=True)
            rc = 1
        except Exception as e:  # a harness bug is a failure too, named
            print(f"interop: {name}: FAIL at {run.rec.step}: {type(e).__name__}: {e}", flush=True)
            rc = 1
        print(f"M tests/interop/harness.py session {name}={rc}", flush=True)
        if rc:
            failed.append(f"session {name}")
        else:
            ran.append(name)
    if "baseline" in ran:
        for name in ("mixed", "swapped"):
            if name not in ran:
                continue
            report = os.path.join(args.out, f"compare-baseline-{name}.txt")
            rc = wire_compare.run(os.path.join(args.out, "baseline.jsonl"), os.path.join(args.out, f"{name}.jsonl"), report)
            print(f"M tests/interop/wire_compare.py baseline vs {name}={rc}", flush=True)
            if rc:
                failed.append(f"wire-compare baseline vs {name}")
    elif not failed:
        failed.append("no baseline session ran, so nothing was compared")
    if failed:
        print(f"M interop=1 failed: [{'] ['.join(failed)}]")
        return 1
    print("M interop=0")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
