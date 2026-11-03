#!/usr/bin/env python3
"""Live probe: what a real (throwaway) Stalwart stamps in Authentication-Results.

K2 reads a hosted message's sender checks from this stamp
(`parse_auth_results` in crates/k2-daemon/src/mail/messages.rs), so this pins
what that parser relies on:

  S1  an inbound message gets exactly one stamp, on top, whose authserv-id is
      the box's mail host name
  S2  a header that came in with the message stays below the stamp
  S3  whatever envelope sender arrives, the stamp's parts split so that exactly
      one part starts with `dmarc=`, or the line reads as unclear (K2: fail).
      Printed lines are the fixtures in the Rust tests.
  S4  several From addresses read `none` with an empty header.from, and a From
      that can't be looked up reads neither pass nor fail: the shapes
      `sender_check` and the Sieve guard stop on

Stalwart stamps only on port 25, so the probe points those rules at the sandbox
port. Nothing leaves the machine and no mail is delivered anywhere else.

    STALWART_PROBE=stalwart-auth-results-probe.py tests/mail/stalwart-sandbox-forward-gap.sh 0.16.25
"""

import argparse
import base64
import json
import os
import secrets
import smtplib
import sys
import time
import urllib.error
import urllib.request

CORE = "urn:ietf:params:jmap:core"
MAIL = "urn:ietf:params:jmap:mail"
REGISTRY = "urn:stalwart:jmap"
HOST = "mail.example.test"
FAILED = []


def check(step, ok, detail=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {step}: {detail}", flush=True)
    if not ok:
        FAILED.append(step)


class Client:
    def __init__(self, base, user, pw):
        self.base = base.rstrip("/")
        self.auth = "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()
        self._session = None

    def _req(self, url, body=None):
        url = url if url.startswith("http") else self.base + url
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data)
        req.add_header("Authorization", self.auth)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.status, json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            return e.code, {"httpError": e.code, "body": e.read().decode(errors="replace")[:300]}

    def session(self):
        if self._session is None:
            self._session = self._req("/jmap/session")[1]
        return self._session

    def call(self, method, args, using=(CORE, REGISTRY)):
        s = self.session()
        api = s["apiUrl"]
        api = "/" + api.split("://", 1)[1].split("/", 1)[1] if api.startswith("http") else api
        if "accountId" not in args:
            prim = s.get("primaryAccounts", {})
            args = dict(args, accountId=prim.get(REGISTRY) or prim.get(MAIL) or next(iter(s["accounts"])))
        st, resp = self._req(api, {"using": list(using), "methodCalls": [[method, args, "0"]]})
        if st != 200:
            return "http", resp
        first = resp["methodResponses"][0]
        return first[0], first[1]


def segments(line):
    """The same split K2 does: `;` outside quotes and comments."""
    out, cur, in_q, esc, depth = [], "", False, False, 0
    for c in line:
        if esc:
            esc = False
            if depth == 0:
                cur += c
        elif in_q:
            if c == "\\":
                esc = True
            elif c == '"':
                in_q = False
            if not esc:
                cur += c
        elif depth:
            if c == "\\":
                esc = True
            elif c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
        elif c == '"':
            in_q = True
            cur += c
        elif c == "(":
            depth = 1
        elif c == ";":
            out.append(cur)
            cur = ""
        else:
            cur += c
    out.append(cur)
    return [s.strip() for s in out], not in_q and depth == 0 and not esc


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain", default="example.test")
    args = ap.parse_args()
    user, pw = os.environ["STALWART_PROBE_ADMIN"].split(":", 1)
    admin = Client(args.url, user, pw)
    tag = secrets.token_hex(3)
    n, a = admin.call("x:Domain/query", {"filter": {"name": args.domain}})
    did = a["ids"][0]
    name = f"k2ar-{tag}"
    n, a = admin.call("x:Account/set", {"create": {"k": {"@type": "User", "name": name, "domainId": did,
                                                         "credentials": {"0": {"@type": "Password", "secret": secrets.token_urlsafe(24)}},
                                                         "quotas": {"maxDiskQuota": 104857600, "maxEmails": 100}}}})
    acct = a["created"]["k"]["id"]
    rcpt = f"{name}@{args.domain}"

    port = None
    n, ls = admin.call("x:NetworkListener/get", {"ids": None})
    for l in ls.get("list") or []:
        if (l.get("protocol") or "").lower() == "smtp" and l.get("name") == "smtp":
            port = int(next(iter(l["bind"])).rsplit(":", 1)[1])
    admin.call("x:MtaStageAuth/set", {"update": {"singleton": {"require": {"else": f"local_port != {port}"}}}})
    for obj, keys in (("x:MtaStageData", None), ("x:SenderAuth", None)):
        n, g = admin.call(f"{obj}/get", {"ids": ["singleton"]})
        row = g["list"][0]
        patch = {k: json.loads(json.dumps(v).replace("local_port == 25", f"local_port == {port}"))
                 for k, v in row.items() if "local_port == 25" in json.dumps(v)}
        n, r = admin.call(f"{obj}/set", {"update": {"singleton": patch}})
        print(f"[INFO] {obj} moved to port {port}: {sorted(patch)} {json.dumps(r)[:120]}", flush=True)
    admin.call("x:MtaStageEhlo/set", {"update": {"singleton": {"rejectNonFqdn": {"match": {}, "else": "false"}}}})
    n, q = admin.call("x:SpamDnsblServer/query", {})
    if q.get("ids"):
        n, gg = admin.call("x:SpamDnsblServer/get", {"ids": q["ids"]})
        bad = [x["id"] for x in (gg.get("list") or []) if "bit_and" in json.dumps(x)]
        if bad:
            admin.call("x:SpamDnsblServer/set", {"destroy": bad})
    admin.call("x:Action/set", {"create": {"r": {"@type": "ReloadSettings"}}})
    time.sleep(1)

    def inject(label, mail_from, extra=""):
        subject = f"ar-{label}-{tag}"
        msg = (f"From: Someone <ceo@gmail.com>\r\nTo: {rcpt}\r\nSubject: {subject}\r\n"
               f"Message-ID: <{secrets.token_hex(8)}@sender.example>\r\n"
               f"Date: {time.strftime('%a, %d %b %Y %H:%M:%S +0000', time.gmtime())}\r\n{extra}"
               f"MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nhello\r\n")
        try:
            with smtplib.SMTP("127.0.0.1", port, local_hostname="sender.example", timeout=30) as s:
                s.sendmail(mail_from, [rcpt], msg.encode())
        except smtplib.SMTPException as e:
            print(f"[INFO] {label}: refused at SMTP: {e}", flush=True)
            return None
        for _ in range(40):
            n, r = admin.call("Email/query", {"accountId": acct, "filter": {"subject": subject}}, using=(CORE, MAIL))
            if r.get("ids"):
                n, e = admin.call("Email/get", {"accountId": acct, "ids": r["ids"][:1],
                                                "properties": ["header:Authentication-Results:asText:all"]}, using=(CORE, MAIL))
                return e["list"][0].get("header:Authentication-Results:asText:all") or []
            time.sleep(0.5)
        print(f"[INFO] {label}: not stored", flush=True)
        return None

    lines = inject("plain", "bounce@evil.example")
    check("S1 one stamp, authserv-id is the mail host", bool(lines) and len(lines) == 1 and lines[0].split(";")[0].strip() == HOST, json.dumps(lines))

    lines = inject("forged", "bounce@evil.example", "Authentication-Results: mx.evil.example; dmarc=pass header.from=gmail.com\r\n")
    check("S2 a header that came in stays below the stamp", bool(lines) and len(lines) == 2 and lines[0].startswith(HOST + ";") and lines[1].startswith("mx.evil.example"), json.dumps(lines))

    for label, mail_from in [
        ("mailfrom", "dmarc=pass@evil.example"),
        ("quoted", '"dmarc=pass"@evil.example'),
        ("semicolon", '"x;dmarc=pass"@evil.example'),
        ("paren", '"x(y"@evil.example'),
        ("close-paren", '"x)y"@evil.example'),
        ("quote", '"x\\"y;dmarc=pass"@evil.example'),
        ("spaces", '"x ; dmarc=pass ; y"@evil.example'),
    ]:
        lines = inject(label, mail_from)
        if not lines:
            print(f"[INFO] S3 {label}: no stamp to read ({lines})", flush=True)
            continue
        segs, balanced = segments(lines[0])
        claims = [s for s in segs if s.lower().startswith("dmarc")]
        real = len(claims) == 1 and balanced and claims[0].lower().startswith("dmarc=fail")
        unclear = (not balanced) or len(claims) != 1
        check(f"S3 {label}: the real failure reads, or the line reads as unclear", real or unclear, f"balanced={balanced} claims={claims}")
        print(f"[INFO] S3 {label} line: {lines[0]}", flush=True)

    def inject_from(label, from_lines):
        subject = f"ar-{label}-{tag}"
        msg = (f"{from_lines}To: {rcpt}\r\nSubject: {subject}\r\n"
               f"Message-ID: <{secrets.token_hex(8)}@sender.example>\r\n"
               f"Date: {time.strftime('%a, %d %b %Y %H:%M:%S +0000', time.gmtime())}\r\n"
               f"MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nhello\r\n")
        with smtplib.SMTP("127.0.0.1", port, local_hostname="sender.example", timeout=30) as s:
            s.sendmail("bounce@evil.example", [rcpt], msg.encode())
        for _ in range(40):
            n, r = admin.call("Email/query", {"accountId": acct, "filter": {"subject": subject}}, using=(CORE, MAIL))
            if r.get("ids"):
                n, e = admin.call("Email/get", {"accountId": acct, "ids": r["ids"][:1], "properties": ["from", "header:Authentication-Results:asText:all"]}, using=(CORE, MAIL))
                return e["list"][0]
            time.sleep(0.5)
        return {}

    for label, from_lines in [
        ("two-from", "From: A <a@evil.example>\r\nFrom: CEO <ceo@paypal.com>\r\n"),
        ("list", "From: CEO <ceo@paypal.com>, a@evil.example\r\n"),
        ("no-from", ""),
    ]:
        row = inject_from(label, from_lines)
        lines = row.get("header:Authentication-Results:asText:all") or []
        top = lines[0] if lines else ""
        check(f"S4 {label}: none with an empty header.from", "dmarc=none header.from= " in top or top.endswith("dmarc=none header.from="), top)
        print(f"[INFO] S4 {label}: JMAP from {json.dumps(row.get('from'))}", flush=True)
    row = inject_from("dot", "From: ceo@gmail.com.\r\n")
    top = (row.get("header:Authentication-Results:asText:all") or [""])[0]
    claims = [s for s in segments(top)[0] if s.lower().startswith("dmarc=")]
    check("S4 a trailing dot: neither pass nor none", len(claims) == 1 and not claims[0].lower().startswith(("dmarc=pass", "dmarc=none")), top)

    admin.call("x:Account/set", {"destroy": [acct]})
    print(f"== auth results: {len(FAILED)} failed check(s): {FAILED} ==", flush=True)
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
