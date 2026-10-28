#!/usr/bin/env python3
"""Live probe P4b (prd-email-notifications-part2-v1 §5, vs-live EN-A29):
a send-only apex that is NOT a Stalwart domain.

Design under test (EN-A8): `brand.test`'s mail stays elsewhere and the box
never adds it to Stalwart. The sender account and its Identity live on
the bounce subdomain `bounce.brand.test` (an ordinary hosted domain); the
envelope MAIL FROM is that account; K2 stamps the header
`From: noreply@brand.test`. Checks:

  a  K2's submit (service key, delegated to the sender account; the Email
     says From noreply@brand.test, the envelope MAIL FROM is
     noreply@bounce.brand.test) is accepted (no forbiddenFrom), and the
     queued message's return path is the bounce address
  b  a box mailbox sending to someone@brand.test is accepted and queued
     for remote delivery (the RCPT-stage local trap of P4 does not trip,
     because brand.test is not local)
  c  an outsider on SMTP :25 to anything@brand.test is refused (relay not
     allowed), so the box is no open relay for the apex
  d  the DKIM key that signs is the bounce domain's (Stalwart signs with
     the MAIL FROM domain, probe P3): a DKIM signature object exists for
     bounce.brand.test and none for brand.test

Nothing leaves the box: `.test` names have no MX, so queued messages just
wait (the sandbox is deleted afterwards). Only throwaway principals.

    STALWART_PROBE_ADMIN='admin@example.test:<secret>' \\
      python3 -I tests/mail/stalwart-p4b-probe.py --url http://127.0.0.1:18080
"""

import argparse
import base64
import json
import os
import secrets
import socket
import sys
import urllib.error
import urllib.request

CORE = "urn:ietf:params:jmap:core"
MAIL = "urn:ietf:params:jmap:mail"
SUBMISSION = "urn:ietf:params:jmap:submission"
REGISTRY = "urn:stalwart:jmap"
SENDER_DISABLED = ["authenticate", "authenticateWithAlias", "imapAuthenticate", "pop3Authenticate", "sieveAuthenticate", "emailSend"]
APEX = "brand.test"
BOUNCE = "bounce.brand.test"

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
        self._s = None

    def _req(self, method, url, body=None):
        url = url if url.startswith("http") else self.base + url
        data = json.dumps(body).encode() if body is not None else None
        r = urllib.request.Request(url, data=data, method=method)
        r.add_header("Authorization", self.auth)
        if data is not None:
            r.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(r, timeout=30) as x:
                return x.status, json.loads(x.read() or b"null")
        except urllib.error.HTTPError as e:
            return e.code, {"httpError": e.code, "body": e.read().decode(errors="replace")[:300]}

    def session(self):
        if self._s is None:
            st, s = self._req("GET", "/jmap/session")
            if st != 200:
                raise RuntimeError(f"session {st}: {s}")
            self._s = s
        return self._s

    def api(self):
        p = self.session()["apiUrl"]
        return "/" + p.split("://", 1)[1].split("/", 1)[1] if p.startswith("http") else p

    def own(self):
        prim = self.session().get("primaryAccounts", {})
        return prim.get(REGISTRY) or prim.get(MAIL) or next(iter(self.session()["accounts"]))

    def batch(self, using, calls):
        st, r = self._req("POST", self.api(), {"using": list(using), "methodCalls": calls})
        if st != 200:
            return [["http", r, "0"]]
        return r["methodResponses"]

    def call(self, method, args, using=(CORE, REGISTRY)):
        if "accountId" not in args:
            args = dict(args, accountId=self.own())
        first = self.batch(using, [[method, args, "0"]])[0]
        return first[0], first[1]


def smtp_rcpt(port, rcpt):
    """MAIL FROM an outsider, RCPT TO `rcpt`; returns the RCPT reply."""
    s = socket.create_connection(("127.0.0.1", port), timeout=20)
    f = s.makefile("rb")

    def reply():
        lines = []
        while True:
            line = f.readline().decode(errors="replace").rstrip()
            lines.append(line)
            if len(line) < 4 or line[3] == " ":
                return " / ".join(lines)

    trail = [reply()]
    # A null reverse-path: no sender domain to resolve in the sandbox.
    for cmd in ("EHLO outsider.origin.test", "MAIL FROM:<>"):
        s.sendall((cmd + "\r\n").encode())
        trail.append(reply())
    s.sendall(f"RCPT TO:<{rcpt}>\r\n".encode())
    out = reply()
    out = out + "  (after: " + " | ".join(t[:90] for t in trail) + ")"
    try:
        s.sendall(b"QUIT\r\n")
    except OSError:
        pass
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain")
    args = ap.parse_args()
    cred = os.environ.get("STALWART_PROBE_ADMIN", "")
    if ":" not in cred:
        sys.exit("set STALWART_PROBE_ADMIN")
    user, pw = cred.split(":", 1)
    domain = args.domain or user.split("@", 1)[-1]
    tag = secrets.token_hex(3)
    admin = Client(args.url, basic=(user, pw))

    def account(client, name, did, perms=None, password=True, admin_role=False):
        acct = {"@type": "User", "name": name, "domainId": did}
        sec = None
        if password:
            sec = secrets.token_urlsafe(24)
            acct["credentials"] = {"0": {"@type": "Password", "secret": sec}}
        if admin_role:
            acct["roles"] = {"@type": "Admin"}
        else:
            acct["quotas"] = {"maxDiskQuota": 52428800, "maxEmails": 1000}
        if perms is not None:
            acct["permissions"] = perms
        n, a = client.call("x:Account/set", {"create": {"k": acct}})
        cid = ((a.get("created") or {}).get("k") or {}).get("id") if n == "x:Account/set" else None
        if not cid:
            sys.exit(f"account {name}: {n} {json.dumps(a)[:300]}")
        return cid, sec

    n, a = admin.call("x:Domain/query", {"filter": {"name": domain}})
    did = a["ids"][0]
    svc_id, _ = account(admin, f"k2p4b-svc-{tag}", did, admin_role=True)
    n, a = admin.call("x:ApiKey/set", {"accountId": svc_id, "create": {"k": {"description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {}}}})
    svc = Client(args.url, bearer=a["created"]["k"]["secret"])

    # The bounce subdomain is hosted; the apex is not.
    n, a = svc.call("x:Domain/set", {"create": {"d": {"name": BOUNCE, "dkimManagement": {"@type": "Automatic"},
                                                     "subAddressing": {"@type": "Enabled"}, "catchAllAddress": None}}})
    bid = ((a.get("created") or {}).get("d") or {}).get("id")
    check("setup: bounce subdomain added", bool(bid), f"{n} {json.dumps(a)[:200]}")
    n, a = svc.call("x:Domain/query", {"filter": {"name": APEX}})
    check("setup: the apex is not a Stalwart domain", not a.get("ids"), json.dumps(a)[:200])
    perms = {"@type": "Merge", "enabledPermissions": {}, "disabledPermissions": {p: True for p in SENDER_DISABLED}}
    sender, _ = account(svc, "noreply", bid, perms=perms, password=False)
    alice, alice_pw = account(svc, f"alice{tag}", did)
    alice_addr = f"alice{tag}@{domain}"

    def submit(client, acct, mail_from, header_from, rcpt, envid):
        n, a = client.call("Identity/get", {"accountId": acct, "ids": None}, using=(CORE, MAIL, SUBMISSION))
        ident = next((i["id"] for i in (a.get("list") or []) if i.get("email", "").lower() == mail_from), None)
        if not ident:
            n, a = client.call("Identity/set", {"accountId": acct, "create": {"i": {"email": mail_from}}}, using=(CORE, MAIL, SUBMISSION))
            ident = ((a.get("created") or {}).get("i") or {}).get("id")
        boxes = None
        for role in ("drafts", "inbox"):
            n, q = client.call("Mailbox/query", {"accountId": acct, "filter": {"role": role}}, using=(CORE, MAIL))
            if q.get("ids"):
                boxes = q["ids"][0]
                break
        if not ident or not boxes:
            return {"error": f"identity {ident} mailbox {boxes}"}
        email = {"mailboxIds": {boxes: True}, "from": [{"email": header_from, "name": "Brand"}], "to": [{"email": rcpt}],
                 "subject": f"p4b {envid}", "bodyValues": {"t": {"value": "probe"}}, "textBody": [{"partId": "t", "type": "text/plain"}]}
        resp = client.batch((CORE, MAIL, SUBMISSION), [
            ["Email/set", {"accountId": acct, "create": {"e": email}}, "0"],
            ["EmailSubmission/set", {"accountId": acct, "create": {"s": {
                "emailId": "#e", "identityId": ident,
                "envelope": {"mailFrom": {"email": mail_from, "parameters": {"RET": "HDRS", "ENVID": envid}}, "rcptTo": [{"email": rcpt}]}}},
                "onSuccessDestroyEmail": ["#s"]}, "1"],
        ])
        return {"responses": resp}

    def created(r):
        for m in r.get("responses") or []:
            if m[0] == "EmailSubmission/set":
                return bool((m[1].get("created") or {}).get("s")), json.dumps(m[1])[:300]
        return False, json.dumps(r)[:300]

    def queued(match):
        n, q = svc.call("x:QueuedMessage/query", {})
        ids = q.get("ids") or []
        if not ids:
            return []
        n, g = svc.call("x:QueuedMessage/get", {"ids": ids})
        return [m for m in (g.get("list") or []) if match(json.dumps(m).lower())]

    # a: K2's submit for the send-only apex.
    r = submit(svc, sender, f"noreply@{BOUNCE}", f"noreply@{APEX}", "someone@outside.test", f"enl_p4b{tag}")
    ok, detail = created(r)
    check("a K2 submit (MAIL FROM bounce, header From apex) accepted", ok, detail)
    q = queued(lambda s: f"enl_p4b{tag}" in s)
    info("a queued message", json.dumps(q)[:900])
    rp = json.dumps(q).lower()
    check("a the return path is the bounce address", f"noreply@{BOUNCE}" in rp, "queued: " + ("yes" if q else "nothing found"))

    # b: a box mailbox writes to someone@brand.test.
    alice_c = Client(args.url, basic=(alice_addr, alice_pw))
    r = submit(alice_c, alice, alice_addr, alice_addr, f"someone@{APEX}", f"enl_p4bb{tag}")
    ok, detail = created(r)
    check("b a box mailbox to someone@brand.test is accepted (no local trap)", ok, detail)
    q = queued(lambda s: f"someone@{APEX}" in s)
    info("b ... queued for remote delivery (or already failed: .test has no MX)", json.dumps(q)[:300])

    # c: an outsider on SMTP to the apex.
    n, ls = admin.call("x:NetworkListener/get", {"ids": None})
    smtp = None
    for l in ls.get("list") or []:
        if (l.get("protocol") or "").lower() == "smtp" and l.get("name") == "smtp":
            smtp = int(next(iter(l["bind"])).rsplit(":", 1)[1])
    if smtp is None:
        check("c an SMTP listener to test", False, json.dumps(ls)[:300])
    else:
        out = smtp_rcpt(smtp, f"anything@{APEX}")
        check("c an outsider to anything@brand.test is refused (no open relay)", out.startswith("5") and not out.startswith("503"), out)
        out = smtp_rcpt(smtp, f"nobody{tag}@{BOUNCE}")
        info("c an outsider to an unknown address on the bounce subdomain", out)
        out = smtp_rcpt(smtp, alice_addr)
        check("c control: an outsider to a real mailbox is accepted", out.startswith("250"), out)

    # d: which DKIM key exists for which domain.
    n, ds = svc.call("x:DkimSignature/query", {})
    sigs = []
    if ds.get("ids"):
        n, g = svc.call("x:DkimSignature/get", {"ids": ds["ids"]})
        sigs = g.get("list") or []
    doms = sorted({str(s.get("domainId") or s.get("domain") or "") for s in sigs})
    info("d DKIM signature domains (ids)", f"{doms}; bounce domain id {bid}")
    check("d the bounce subdomain has a DKIM key", any(str(s.get("domainId")) == str(bid) or BOUNCE in json.dumps(s) for s in sigs), f"{len(sigs)} signature(s)")
    check("d no DKIM key names the apex", not any(f'"{APEX}"' in json.dumps(s) for s in sigs), "")

    # Tidy: drop queued messages so nothing retries.
    n, q = svc.call("x:QueuedMessage/query", {})
    if q.get("ids"):
        svc.call("x:QueuedMessage/set", {"destroy": q["ids"]})
    print(f"== P4b: {len(FAILED)} failed check(s): {FAILED} ==", flush=True)
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
