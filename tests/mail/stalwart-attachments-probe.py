#!/usr/bin/env python3
"""Live probe: hosted-mail attachments, Reply-To and forwards on a real
(throwaway) Stalwart.

Sends K2's EXACT JMAP shape (`submission_email_json` + `submission_request`
in crates/k2-daemon/src/mail/send.rs and jmap.rs, with the service account's
delegated accountId) from a loopback Stalwart started by
tests/mail/stalwart-sandbox-forward-gap.sh. The "outside" recipient is a
loopback SMTP sink this script runs: an `x:MtaRoute` relay sends every
`outside.test` recipient there, so no mail ever leaves the machine.

  A1  the session's limits (recorded: maxSizeUpload, maxSizeRequest, the mail
      capability's attachment limit)
  A2  one attachment + Reply-To reach the sink: headers, decoded bytes,
      filename; the From is the mailbox
  A3  K2's own limits fit: 10 files adding up to 25 MiB
  A4  three 25 MiB sends in a row (one file each; the upload quota)
  A5  an upload bigger than the session's limit (recorded: K2's service key)
  A6  --as-eml: the original's raw blob, attached as message/rfc822, arrives
      with its bytes unchanged
  A7  default forward: an inbound message's attachment, downloaded and
      re-uploaded, arrives with its bytes unchanged
  A8  what Stalwart does with a line break in `subject`, `name` and `replyTo`
      (K2 refuses or cleans them first; recorded so a change shows)
  A9  the largest message K2 allows: 25 MiB of files + 9.5 MB of text
  A10 text attachments: which `type`/`charset` shapes keep the bytes unchanged

    STALWART_PROBE_ADMIN='admin@example.test:<secret>' \\
      python3 -I tests/mail/stalwart-attachments-probe.py --url http://127.0.0.1:18080 --domain example.test

Run it only against a disposable Stalwart:
    STALWART_PROBE=stalwart-attachments-probe.py tests/mail/stalwart-sandbox-forward-gap.sh 0.16.25
"""

import argparse
import base64
import email
import email.policy
import hashlib
import json
import os
import secrets
import smtplib
import socket
import sys
import threading
import time
import urllib.error
import urllib.request
from email.message import EmailMessage

CORE = "urn:ietf:params:jmap:core"
MAIL = "urn:ietf:params:jmap:mail"
SUBMISSION = "urn:ietf:params:jmap:submission"
REGISTRY = "urn:stalwart:jmap"
MIB = 1024 * 1024
FAILED = []


def check(step, ok, detail=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {step}: {detail}", flush=True)
    if not ok:
        FAILED.append(step)


def info(step, detail=""):
    print(f"[INFO] {step}: {detail}", flush=True)


class Client:
    def __init__(self, base, basic=None, bearer=None):
        self.base = base.rstrip("/")
        self.auth = ("Basic " + base64.b64encode(f"{basic[0]}:{basic[1]}".encode()).decode()) if basic else f"Bearer {bearer}"
        self._session = None

    def _req(self, method, url, body=None, raw=None, ctype="application/json", want_bytes=False):
        url = url if url.startswith("http") else self.base + url
        data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Authorization", self.auth)
        if data is not None:
            req.add_header("Content-Type", ctype)
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                out = r.read()
                if want_bytes:
                    return r.status, out
                try:
                    return r.status, json.loads(out or b"null")
                except ValueError:
                    return r.status, out
        except urllib.error.HTTPError as e:
            return e.code, {"httpError": e.code, "body": e.read().decode(errors="replace")[:300]}

    def session(self):
        if self._session is None:
            st, doc = self._req("GET", "/jmap/session")
            if st != 200:
                raise RuntimeError(f"session HTTP {st}: {doc}")
            self._session = doc
        return self._session

    def path(self, key):
        p = self.session()[key]
        return "/" + p.split("://", 1)[1].split("/", 1)[1] if p.startswith("http") else p

    def own_account(self):
        prim = self.session().get("primaryAccounts", {})
        return prim.get(REGISTRY) or prim.get(MAIL) or next(iter(self.session()["accounts"]))

    def call(self, method, args, using=(CORE, REGISTRY)):
        if "accountId" not in args:
            args = dict(args, accountId=self.own_account())
        st, resp = self._req("POST", self.path("apiUrl"), {"using": list(using), "methodCalls": [[method, args, "0"]]})
        if st != 200:
            return "http", resp
        first = resp["methodResponses"][0]
        return first[0], first[1]

    def batch(self, using, calls):
        st, resp = self._req("POST", self.path("apiUrl"), {"using": list(using), "methodCalls": calls})
        return resp.get("methodResponses") if st == 200 and isinstance(resp, dict) else [["http", resp, "0"]]

    def upload(self, account, data):
        st, resp = self._req("POST", self.path("uploadUrl").replace("{accountId}", account), raw=data, ctype="application/octet-stream")
        if st not in (200, 201) or not isinstance(resp, dict) or "blobId" not in resp:
            return None, f"upload HTTP {st}: {resp}"
        return resp["blobId"], None

    def download(self, account, blob, name="m", mime="application/octet-stream"):
        url = self.path("downloadUrl").replace("{accountId}", account).replace("{blobId}", blob).replace("{name}", name).replace("{type}", mime)
        return self._req("GET", url, want_bytes=True)


class Sink:
    """A minimal SMTP server on 127.0.0.1 that records every message."""

    def __init__(self):
        self.got = []
        self.lock = threading.Lock()
        self.srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(16)
        self.port = self.srv.getsockname()[1]
        threading.Thread(target=self._serve, daemon=True).start()

    def _serve(self):
        while True:
            try:
                c, _ = self.srv.accept()
            except OSError:
                return
            threading.Thread(target=self._one, args=(c,), daemon=True).start()

    def _one(self, c):
        f = c.makefile("rb")
        send = lambda s: c.sendall(s.encode() + b"\r\n")
        send("220 sink.outside.test ESMTP")
        mail_from, rcpt = None, []
        try:
            while True:
                line = f.readline()
                if not line:
                    return
                cmd = line.decode(errors="replace").strip()
                up = cmd.upper()
                if up.startswith("EHLO") or up.startswith("HELO"):
                    c.sendall(b"250-sink.outside.test\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n")
                elif up.startswith("MAIL FROM:"):
                    mail_from = cmd[10:].strip().split(" ")[0].strip("<>")
                    rcpt = []
                    send("250 ok")
                elif up.startswith("RCPT TO:"):
                    rcpt.append(cmd[8:].strip().split(" ")[0].strip("<>").lower())
                    send("250 ok")
                elif up == "DATA":
                    send("354 go")
                    data = []
                    while True:
                        l = f.readline()
                        if not l or l.rstrip(b"\r\n") == b".":
                            break
                        data.append(l[1:] if l.startswith(b"..") else l)
                    with self.lock:
                        self.got.append({"from": mail_from, "rcpt": rcpt, "data": b"".join(data)})
                    send("250 queued")
                elif up == "QUIT":
                    send("221 bye")
                    return
                else:
                    send("250 ok")
        finally:
            c.close()

    def by_subject(self, subject):
        needle = subject.encode()
        with self.lock:
            return [m for m in self.got if needle in m["data"][:8192]]


def wait_for(pred, secs=90):
    end = time.time() + secs
    while time.time() < end:
        v = pred()
        if v:
            return v
        time.sleep(0.5)
    return pred()


def parse(raw):
    return email.message_from_bytes(raw, policy=email.policy.compat32)


def parts(msg):
    """(filename, content type, cte, decoded bytes) per attachment part."""
    out = []
    for p in msg.walk():
        if p.is_multipart() and p.get_content_type() != "message/rfc822":
            continue
        name = p.get_filename()
        disp = (p.get("Content-Disposition") or "").lower()
        if name is None and "attachment" not in disp:
            continue
        cte = (p.get("Content-Transfer-Encoding") or "").lower()
        if p.get_content_type() == "message/rfc822" and p.is_multipart():
            out.append((name, p.get_content_type(), cte, p.get_payload(0).as_bytes()))
        else:
            out.append((name, p.get_content_type(), cte, p.get_payload(decode=True)))
    return out


def rfc822_part_bytes(outer):
    """The decoded bytes of the one message/rfc822 part, read off the wire
    form: Python's parser treats a message/* part as a nested message and
    would not undo its transfer encoding."""
    at = outer.index(b"message/rfc822")
    start = outer.rfind(b"\r\n--", 0, at)
    head_end = outer.index(b"\r\n\r\n", at)
    head = outer[start:head_end].lower()
    boundary = outer[start + 2:].split(b"\r\n", 1)[0]
    end = outer.index(b"\r\n" + boundary, head_end)
    body = outer[head_end + 4:end]
    if b"content-transfer-encoding: base64" in head:
        return base64.b64decode(body), "base64"
    return body, "as is"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain")
    args = ap.parse_args()
    cred = os.environ.get("STALWART_PROBE_ADMIN", "")
    if ":" not in cred:
        sys.exit("set STALWART_PROBE_ADMIN=<admin user>:<secret> in the environment")
    admin_user, admin_pw = cred.split(":", 1)
    domain = args.domain or admin_user.split("@", 1)[-1]
    tag = secrets.token_hex(3)
    admin = Client(args.url, basic=(admin_user, admin_pw))
    n, a = admin.call("x:Domain/query", {"filter": {"name": domain}})
    if n != "x:Domain/query" or not a.get("ids"):
        sys.exit(f"domain {domain} not found: {n} {a}")
    did = a["ids"][0]
    sink = Sink()
    made = []

    def account(name, admin_role=False):
        pw = secrets.token_urlsafe(24)
        acct = {"@type": "User", "name": name, "domainId": did, "credentials": {"0": {"@type": "Password", "secret": pw}}}
        if admin_role:
            acct["roles"] = {"@type": "Admin"}
        else:
            acct["quotas"] = {"maxDiskQuota": 4 * 1024 * MIB, "maxEmails": 1000}
        n, a = admin.call("x:Account/set", {"create": {"k": acct}})
        cid = ((a.get("created") or {}).get("k") or {}).get("id") if n == "x:Account/set" else None
        if not cid:
            sys.exit(f"account {name}: {n} {json.dumps(a)[:300]}")
        made.append(cid)
        return cid, pw

    svc_id, _ = account(f"k2at-svc-{tag}", admin_role=True)
    n, a = admin.call("x:ApiKey/set", {"accountId": svc_id, "create": {"k": {"description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {}}}})
    key = ((a.get("created") or {}).get("k") or {}).get("secret")
    if not key:
        sys.exit(f"service key: {n} {json.dumps(a)[:300]}")
    svc = Client(args.url, bearer=key)
    alice, _ = account(f"k2at-alice-{tag}")
    alice_addr = f"k2at-alice-{tag}@{domain}"

    n, a = admin.call("x:MtaRoute/set", {"create": {"r": {"@type": "Relay", "name": f"k2atsink{tag}", "address": "127.0.0.1",
                                                           "port": sink.port, "protocol": "smtp", "implicitTls": False}}})
    rid = ((a.get("created") or {}).get("r") or {}).get("id")
    if not rid:
        sys.exit(f"sink route: {n} {json.dumps(a)[:300]}")
    n, g = admin.call("x:MtaOutboundStrategy/get", {"ids": ["singleton"]})
    old_route = g["list"][0]["route"]
    m = dict((old_route.get("match") or {}))
    shifted = {str(int(k) + 1): v for k, v in m.items()}
    shifted["0"] = {"if": "rcpt_domain == 'outside.test'", "then": f"'k2atsink{tag}'"}
    n, a = admin.call("x:MtaOutboundStrategy/set", {"update": {"singleton": {"route": {"match": shifted, "else": old_route["else"]}}}})
    if a.get("notUpdated"):
        sys.exit(f"route bind: {json.dumps(a)[:300]}")
    smtp_port = None
    n, ls = admin.call("x:NetworkListener/get", {"ids": None})
    for l in ls.get("list") or []:
        if (l.get("protocol") or "").lower() == "smtp" and l.get("name") == "smtp":
            smtp_port = int(next(iter(l["bind"])).rsplit(":", 1)[1])
    n, a = admin.call("x:MtaStageAuth/set", {"update": {"singleton": {"require": {"else": f"local_port != {smtp_port}"}}}})
    n, q = admin.call("x:SpamDnsblServer/query", {})
    if q.get("ids"):
        n, gg = admin.call("x:SpamDnsblServer/get", {"ids": q["ids"]})
        bad = [x["id"] for x in (gg.get("list") or []) if "bit_and" in json.dumps(x)]
        if bad:
            admin.call("x:SpamDnsblServer/set", {"destroy": bad})
    admin.call("x:Action/set", {"create": {"r": {"@type": "ReloadSettings"}}})

    # ── K2's submission shape ──────────────────────────────────────
    n, ids = svc.call("Identity/get", {"accountId": alice, "ids": None}, using=(CORE, MAIL, SUBMISSION))
    ident = next((i["id"] for i in (ids.get("list") or []) if i.get("email", "").lower() == alice_addr), None)
    if not ident:
        n, cr = svc.call("Identity/set", {"accountId": alice, "create": {"k": {"email": alice_addr}}}, using=(CORE, MAIL, SUBMISSION))
        ident = ((cr.get("created") or {}).get("k") or {}).get("id")
    if not ident:
        sys.exit(f"no identity: {n} {json.dumps(ids)[:300]}")
    n, mbx = svc.call("Mailbox/query", {"accountId": alice, "filter": {"role": "drafts"}}, using=(CORE, MAIL))
    drafts = mbx["ids"][0]

    def send(subject, atts=(), reply_to=(), to="sink@outside.test", body="hello from k2", extra_attach=None):
        e = {
            "from": [{"name": "k2 agent", "email": alice_addr}],
            "messageId": [f"{secrets.token_hex(8)}@{domain}"],
            "to": [{"email": to}],
            "subject": subject,
            "keywords": {"$seen": True},
            "bodyValues": {"k2body": {"value": body}},
            "textBody": [{"partId": "k2body", "type": "text/plain"}],
            "mailboxIds": {drafts: True},
        }
        if reply_to:
            e["replyTo"] = [{"email": r} for r in reply_to]
        if atts:
            e["attachments"] = [dict({"blobId": b, "type": t, "name": nm, "disposition": "attachment"}, **(extra_attach or {})) for b, t, nm in atts]
        r = svc.batch((CORE, MAIL, SUBMISSION), [
            ["Email/set", {"accountId": alice, "create": {"k2out": e}}, "0"],
            ["EmailSubmission/set", {"accountId": alice, "create": {"k2sub": {
                "emailId": "#k2out", "identityId": ident,
                "envelope": {"mailFrom": {"email": alice_addr}, "rcptTo": [{"email": to}]}}}}, "1"],
        ])
        ok = any(x[0] == "EmailSubmission/set" and (x[1].get("created") or {}).get("k2sub") for x in r)
        return ok, r

    def received(subject, secs=120):
        return wait_for(lambda: sink.by_subject(subject), secs)

    def summ(r):
        return json.dumps(r)[:500]

    # ── A1 limits ──────────────────────────────────────────────────
    caps = svc.session().get("capabilities") or {}
    info("A1 core", json.dumps(caps.get(CORE)))
    acct_caps = (svc.session().get("accounts") or {}).get(svc.own_account(), {}).get("accountCapabilities", {})
    info("A1 mail (session)", json.dumps(caps.get(MAIL)))
    info("A1 mail (account)", json.dumps(acct_caps.get(MAIL)))
    info("A1 submission (account)", json.dumps(acct_caps.get(SUBMISSION)))
    core_upload = (caps.get(CORE) or {}).get("maxSizeUpload")
    check("A1 maxSizeUpload holds a 25 MiB file", isinstance(core_upload, int) and core_upload >= 25 * MIB, str(core_upload))

    # ── A2 one attachment + Reply-To ───────────────────────────────
    payload = os.urandom(4096)
    blob, err = svc.upload(alice, payload)
    check("A2 upload", bool(blob), str(err))
    subj = f"a2-{tag}"
    ok, r = send(subj, [(blob, "application/pdf", "report.pdf")], ["replies@outside.test"])
    check("A2 submitted", ok, summ(r))
    got = received(subj)
    check("A2 reached the sink", bool(got), "")
    if got:
        msg = parse(got[0]["data"])
        check("A2 Reply-To header", "replies@outside.test" in (msg.get("Reply-To") or ""), str(msg.get("Reply-To")))
        check("A2 From is the mailbox", alice_addr in (msg.get("From") or ""), str(msg.get("From")))
        ps = parts(msg)
        check("A2 one attachment, named, bytes unchanged",
              len(ps) == 1 and ps[0][0] == "report.pdf" and ps[0][3] == payload and ps[0][1] == "application/pdf",
              str([(p[0], p[1], p[2], len(p[3])) for p in ps]))
        info("A2 message size", str(len(got[0]["data"])))

    # ── A3 K2's limits ─────────────────────────────────────────────
    files = [os.urandom(25 * MIB // 10) for _ in range(10)]
    blobs = []
    for i, f in enumerate(files):
        b, err = svc.upload(alice, f)
        if not b:
            check(f"A3 upload file {i}", False, str(err))
        blobs.append((b, "application/octet-stream", f"f{i}.bin"))
    subj = f"a3-ten-{tag}"
    t0 = time.time()
    ok, r = send(subj, blobs)
    check("A3 10 files, 25 MiB total: submitted", ok, summ(r))
    got = received(subj)
    check("A3 10 files reached the sink", bool(got), f"{time.time() - t0:.1f}s")
    if got:
        ps = parts(parse(got[0]["data"]))
        same = len(ps) == 10 and all(hashlib.sha256(ps[i][3]).digest() == hashlib.sha256(files[i]).digest() for i in range(10))
        check("A3 10 files: bytes unchanged", same, f"{len(ps)} parts; message {len(got[0]['data'])} bytes")

    big = os.urandom(25 * MIB)
    for i in range(3):
        t0 = time.time()
        b, err = svc.upload(alice, big)
        check(f"A4 25 MiB upload #{i + 1}", bool(b), f"{err} {time.time() - t0:.1f}s")
        if not b:
            continue
        subj = f"a4-one-{i}-{tag}"
        ok, r = send(subj, [(b, "application/zip", "big.zip")])
        check(f"A4 25 MiB send #{i + 1} submitted", ok, summ(r))
        got = received(subj, 180)
        check(f"A4 25 MiB send #{i + 1} reached the sink", bool(got), f"{time.time() - t0:.1f}s")
        if got:
            ps = parts(parse(got[0]["data"]))
            check(f"A4 25 MiB send #{i + 1}: bytes unchanged", len(ps) == 1 and ps[0][3] == big, f"message {len(got[0]['data'])} bytes")

    # ── A5 the session's limit and K2's service key ────────────────
    if isinstance(core_upload, int) and core_upload < 200 * MIB:
        b, err = svc.upload(alice, b"x" * (core_upload + 1024))
        info("A5 an upload past maxSizeUpload with the service key", f"accepted: {bool(b)} {str(err)[:200]}")

    # ── A9 the largest message K2 allows: 25 MiB of files + the text limit ──
    # (MAX_HOSTED_TEXT_JSON_BYTES in send.rs; the request limit is 10,000,000.)
    b, err = svc.upload(alice, big)
    subj = f"a9-max-{tag}"
    ok, r = send(subj, [(b, "application/zip", "big.zip")], body="y" * 9_499_000)
    check("A9 25 MiB + 9.5 MB of text: submitted", ok, summ(r)[:300])
    got = received(subj, 180) if ok else None
    check("A9 25 MiB + 9.5 MB of text reached the sink", bool(got), f"message {len(got[0]['data']) if got else 0} bytes")
    ok, r = send(f"a9-over-{tag}", (), body="y" * 10_000_100)
    check("A9 text past the request limit is refused (K2 refuses it first)", not ok, summ(r)[:200])

    # ── A10 text attachments: bytes unchanged however K2 types them ──
    lf_text = b"line one\nline two\n\ttabbed  \nlast line no newline"
    for label, data, ctype, extra, must in [
        ("bare text/plain, LF", lf_text, "text/plain", {}, False),
        ("text/plain + charset, LF", lf_text, "text/plain", {"charset": "utf-8"}, False),
        ("type with a parameter", lf_text, "text/plain; charset=utf-8", {}, False),
        ("application/octet-stream, LF", lf_text, "application/octet-stream", {}, True),
        ("text/csv, CRLF", b"a,b\r\n1,2\r\n", "text/csv", {}, False),
        ("text/html", b"<p>hi</p>\n", "text/html", {}, False),
        ("not UTF-8 as text/plain", b"caf\xe9 cr\xe8me\n", "text/plain", {}, False),
        ("binary as text/plain", bytes(range(256)) * 8, "text/plain", {}, False),
        ("empty file", b"", "application/octet-stream", {}, False),
    ]:
        b, err = svc.upload(alice, data)
        if not b:
            info(f"A10 {label}", f"upload refused: {err}")
            continue
        subj = f"a10-{secrets.token_hex(3)}-{tag}"
        ok, r = send(subj, [(b, ctype, "file.txt")], extra_attach=extra)
        got = received(subj, 40) if ok else None
        if not got:
            info(f"A10 {label}", f"refused or not delivered: {summ(r)[:300]}")
            if must:
                check(f"A10 {label}: delivered", False, "")
            continue
        ps = parts(parse(got[0]["data"]))
        same = len(ps) == 1 and ps[0][3] == data
        info(f"A10 {label}", f"bytes unchanged: {same}; part {[(p[0], p[1], p[2], len(p[3])) for p in ps]}")
        if must:
            check(f"A10 {label}: bytes unchanged", same, "")

    # ── Inbound message for A6/A7 ──────────────────────────────────
    att_bytes = os.urandom(8192)
    subj_in = f"inbound-{tag}"
    em = EmailMessage()
    em["From"] = "Sender <sender@sender.example>"
    em["To"] = alice_addr
    em["Subject"] = subj_in
    em["Message-ID"] = f"<{tag}@sender.example>"
    em["Date"] = time.strftime("%a, %d %b %Y %H:%M:%S +0000", time.gmtime())
    em.set_content("Original text body.\n")
    em.add_attachment(att_bytes, maintype="application", subtype="octet-stream", filename="orig.dat")
    with smtplib.SMTP("127.0.0.1", smtp_port, local_hostname="sender.example", timeout=30) as s:
        s.sendmail("sender@sender.example", [alice_addr], em.as_bytes())

    def find_inbound():
        n, a = svc.call("Email/query", {"accountId": alice, "filter": {"subject": subj_in}}, using=(CORE, MAIL))
        return (a.get("ids") or [None])[0]

    eid = wait_for(find_inbound, 40)
    check("A6 inbound message stored", bool(eid), "")
    if eid:
        n, e = svc.call("Email/get", {"accountId": alice, "ids": [eid], "properties": ["blobId", "attachments", "subject", "from"]}, using=(CORE, MAIL))
        row = e["list"][0]
        st, raw = svc.download(alice, row["blobId"], "message.eml", "message/rfc822")
        check("A6 raw blob downloaded", st == 200 and b"Original" in raw, f"{st} {len(raw)} bytes")
        b, err = svc.upload(alice, raw)
        subj = f"a6-eml-{tag}"
        ok, r = send(subj, [(b, "message/rfc822", "original.eml")])
        check("A6 .eml forward submitted", ok, summ(r))
        got = received(subj)
        check("A6 .eml reached the sink", bool(got), "")
        if got:
            outer = got[0]["data"]
            m0 = parse(outer)
            rfc = [p for p in m0.walk() if p.get_content_type() == "message/rfc822"]
            check("A6 a message/rfc822 part", len(rfc) == 1, str([p.get_content_type() for p in m0.walk()]))
            if rfc:
                info("A6 part headers", str(rfc[0].items()))
                inner_bytes, how = rfc822_part_bytes(outer)
                check("A6 the part's bytes are exactly the stored message", inner_bytes == raw, f"{how}; {len(inner_bytes)} vs {len(raw)}")
                inner = parse(inner_bytes)
                check("A6 inner message parses, Subject and attachment intact",
                      inner.get("Subject") == subj_in and any(x[3] == att_bytes for x in parts(inner)), how)

        # ── A7 default forward, attachment re-uploaded ─────────────
        at = row["attachments"][0]
        st, data = svc.download(alice, at["blobId"], at.get("name") or "a", at.get("type") or "application/octet-stream")
        check("A7 attachment downloaded", st == 200 and data == att_bytes, f"{st} {len(data)}")
        b, err = svc.upload(alice, data)
        subj = f"Fwd: a7-{tag}"
        ok, r = send(subj, [(b, at.get("type"), at.get("name"))], body="---------- Forwarded message ---------\nFrom: sender@sender.example\n\nOriginal text body.\n")
        check("A7 forward submitted", ok, summ(r))
        got = received(subj)
        check("A7 forward reached the sink", bool(got), "")
        if got:
            ps = parts(parse(got[0]["data"]))
            check("A7 forward carries the attachment", len(ps) == 1 and ps[0][3] == att_bytes and ps[0][0] == "orig.dat",
                  str([(p[0], p[1], len(p[3])) for p in ps]))

    # ── A8 what Stalwart does with injection (K2 refuses first) ────
    subj = f"a8-subject-{tag}\r\nX-Evil-Subject: 1"
    ok, r = send(subj, ())
    got = received(f"a8-subject-{tag}", 20) if ok else None
    if got:
        m0 = parse(got[0]["data"])
        info("A8 subject with CRLF", f"X-Evil-Subject header present: {m0.get('X-Evil-Subject') is not None}; Subject {m0.get('Subject')!r}")
    else:
        info("A8 subject with CRLF", f"refused or not delivered: {summ(r)}")
    b, _ = svc.upload(alice, b"x")
    subj = f"a8-name-{tag}"
    ok, r = send(subj, [(b, "text/plain", "a\r\nX-Evil-Name: 1.txt")])
    got = received(subj, 20) if ok else None
    if got:
        info("A8 name with CRLF", f"X-Evil-Name line present: {b'X-Evil-Name: 1' in got[0]['data']}; parts {[p[0] for p in parts(parse(got[0]['data']))]}")
    else:
        info("A8 name with CRLF", f"refused or not delivered: {summ(r)}")
    subj = f"a8-replyto-{tag}"
    ok, r = send(subj, (), ["x@outside.test\r\nX-Evil-Reply: 1"])
    got = received(subj, 20) if ok else None
    if got:
        m0 = parse(got[0]["data"])
        info("A8 replyTo with CRLF", f"X-Evil-Reply present: {m0.get('X-Evil-Reply') is not None}; Reply-To {m0.get('Reply-To')!r}")
    else:
        info("A8 replyTo with CRLF", f"refused or not delivered: {summ(r)}")

    admin.call("x:MtaOutboundStrategy/set", {"update": {"singleton": {"route": old_route}}})
    admin.call("x:MtaRoute/set", {"destroy": [rid]})
    for aid in made:
        admin.call("x:Account/set", {"destroy": [aid]})
    print(f"== attachments: {len(FAILED)} failed check(s): {FAILED} ==", flush=True)
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
