#!/usr/bin/env python3
"""Real Stalwart proof: the default certificate, and changing the primary host.

Run by tests/mail/stalwart-sandbox-forward-gap.sh (a throwaway Stalwart on
loopback, both versions in the fleet) through `forward-cargo.sh default-cert`.

  P1  What a changed hostname does to MTA-STS: the policy Stalwart serves, the
      zone it generates and when each one changes (recorded, no K2 code).
  P2  K2's real plant and repair against this Stalwart (the ignored Rust test
      domains::store::live_tests): two certificates, SNI for each, no SNI and an
      unknown SNI get the primary, a wrong default heals, a changed primary
      takes the default, the old name's renewal doesn't take it back.
  P3  STARTTLS on 25 with no SNI gets the primary.

A real daemon boot isn't run here: its gate needs /etc/stalwart/config.json. The
live test calls cert_owner::boot_checks, the function the boot thread calls.

The admin credential comes from the environment only. DC_FACTS_ONLY=1 runs P1
alone (no cargo). A [FAIL] means the proof failed.

    STALWART_PROBE_ADMIN='admin@example.test:<secret>' STALWART_VERSION=0.16.25 \\
      K2_DC_TEST_BIN=<k2-daemon test binary> \\
      python3 -I tests/mail/stalwart-default-cert-probe.py --url http://127.0.0.1:18080
"""

import argparse
import base64
import hashlib
import json
import os
import secrets
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

CORE = "urn:ietf:params:jmap:core"
REGISTRY = "urn:stalwart:jmap"
LIVE_TEST = "domains::store::live_tests::live_default_certificate_on_a_real_stalwart"
OLD = "mail.example.test"
NEW = "mail.new.test"

FAILED = []


def check(step, ok, detail=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {step}: {detail}", flush=True)
    if not ok:
        FAILED.append(step)


def info(step, detail=""):
    print(f"[INFO] {step}: {detail}", flush=True)


class Admin:
    def __init__(self, base, user, pw):
        self.base = base.rstrip("/")
        self.auth = "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()
        self._session = None

    def _req(self, url, body=None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url if url.startswith("http") else self.base + url, data=data)
        req.add_header("Authorization", self.auth)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read() or b"null")

    def session(self):
        if self._session is None:
            self._session = self._req("/jmap/session")
        return self._session

    def call(self, method, args):
        s = self.session()
        api = s["apiUrl"]
        if api.startswith("http"):
            api = "/" + api.split("://", 1)[1].split("/", 1)[1]
        if "accountId" not in args:
            args = dict(args, accountId=s["primaryAccounts"].get(REGISTRY) or next(iter(s["accounts"])))
        r = self._req(api, {"using": [CORE, REGISTRY], "methodCalls": [[method, args, "0"]]})
        first = r["methodResponses"][0]
        return first[0], first[1]


def policy(base):
    """What Stalwart serves anonymously at /.well-known/mta-sts.txt."""
    try:
        with urllib.request.urlopen(f"{base}/.well-known/mta-sts.txt", timeout=10) as r:
            return r.read().decode().replace("\r\n", " | ").replace("\n", " | ")
    except urllib.error.HTTPError as e:
        return f"HTTP {e.code}"


def settings(admin):
    n, g = admin.call("x:SystemSettings/get", {"ids": ["singleton"], "properties": ["defaultHostname", "mailExchangers", "defaultCertificateId"]})
    if n != "x:SystemSettings/get":
        sys.exit(f"x:SystemSettings/get: {n} {json.dumps(g)[:300]}")
    return g["list"][0]


def zone_lines(admin, domain):
    n, q = admin.call("x:Domain/query", {"filter": {"name": domain}})
    ids = q.get("ids") or []
    if not ids:
        return [f"no domain {domain}: {n} {json.dumps(q)[:200]}"]
    n, g = admin.call("x:Domain/get", {"ids": ids[:1], "properties": ["dnsZoneFile"]})
    text = ((g.get("list") or [{}])[0]).get("dnsZoneFile") or ""
    want = ("\tMX\t", "mta-sts", "v=spf1", "ua-auto-config", "autoconfig", "autodiscover", "_submission", "_imaps", "_caldavs")
    return [" ".join(l.split()) for l in text.splitlines() if any(w in l for w in want)]


def reload_action(admin, kind):
    n, a = admin.call("x:Action/set", {"create": {"a": {"@type": kind}}})
    return "ok" if (a.get("created") or {}).get("a") is not None else f"{n} {json.dumps(a.get('notCreated') or a)[:200]}"


def reload_settings_past_spam_rules(admin):
    """ReloadSettings. A fresh Stalwart below 0.16.23 refuses it: the latest spam
    rules use `bit_and`. On a real box K2 pins rules that load (mail/defaults.rs);
    here the rule Stalwart names goes instead, which only affects this sandbox."""
    refused = 0
    for _ in range(10):
        n, a = admin.call("x:Action/set", {"create": {"a": {"@type": "ReloadSettings"}}})
        if (a.get("created") or {}).get("a") is not None:
            return f"ok, after removing {refused} refused spam rules" if refused else "ok"
        bad = (a.get("notCreated") or {}).get("a") or {}
        obj = bad.get("objectId") or {}
        if bad.get("type") != "validationFailed" or not obj.get("object"):
            return f"refused: {json.dumps(a)[:300]}"
        n2, d = admin.call(f"x:{obj['object']}/set", {"destroy": [obj["id"]]})
        if obj["id"] not in (d.get("destroyed") or []):
            return f"refused, and could not remove {obj}: {json.dumps(d)[:300]}"
        refused += 1
    return f"still refused after removing {refused} rules"


def smtp_names(port):
    """The name in the live SMTP banner and in the EHLO reply."""
    s = socket.create_connection(("127.0.0.1", port), timeout=15)
    f = s.makefile("rb")
    banner = f.readline().decode(errors="replace").strip()
    s.sendall(b"EHLO probe.example\r\n")
    ehlo = f.readline().decode(errors="replace").strip()
    s.close()
    return f"banner {banner!r}; ehlo {ehlo!r}"


def p1_mta_sts(admin, base, domain):
    n, g = admin.call("x:MtaSts/get", {"ids": ["singleton"]})
    info("P1 x:MtaSts as shipped", json.dumps(g.get("list") or g)[:400])
    before = settings(admin)
    info("P1 settings before", json.dumps({k: before.get(k) for k in ("defaultHostname", "mailExchangers")}))
    info("P1 served policy before", policy(base))
    for line in zone_lines(admin, domain):
        info("P1 zone before", line)

    n, a = admin.call("x:SystemSettings/set", {"update": {"singleton": {"defaultHostname": NEW}}})
    check("P1 the hostname change is accepted", "singleton" in (a.get("updated") or {}), json.dumps(a)[:300])
    smtp = listener_port(admin, "smtp")
    info("P1 served policy right after the write", policy(base))
    info("P1 SMTP names right after the write", smtp_names(smtp))
    info("P1 action ReloadTlsCertificates", reload_action(admin, "ReloadTlsCertificates"))
    info("P1 SMTP names after ReloadTlsCertificates", smtp_names(smtp))
    info("P1 action ReloadSettings", reload_settings_past_spam_rules(admin))
    info("P1 served policy after ReloadSettings", policy(base))
    names = smtp_names(smtp)
    check("P1 ReloadSettings puts the new name in the live banner and EHLO", names.count(NEW) == 2, names)
    check("P1 ReloadSettings puts the new name in the served policy", NEW in policy(base), policy(base))
    for line in zone_lines(admin, domain):
        info("P1 zone after", line)

    n, a = admin.call("x:SystemSettings/set", {"update": {"singleton": {"defaultHostname": before["defaultHostname"]}}})
    check("P1 the hostname goes back", "singleton" in (a.get("updated") or {}), json.dumps(a)[:300])
    info("P1 action ReloadSettings (rollback)", reload_settings_past_spam_rules(admin))
    info("P1 served policy after rollback", policy(base))


def mint_key(admin, domain):
    tag = secrets.token_hex(3)
    n, a = admin.call("x:Account/set", {"create": {"k": {
        "@type": "User", "name": f"k2dc-svc-{tag}", "domainId": domain_id(admin, domain),
        "credentials": {"0": {"@type": "Password", "secret": secrets.token_urlsafe(24)}},
        "roles": {"@type": "Admin"},
    }}})
    svc = ((a.get("created") or {}).get("k") or {}).get("id")
    if not svc:
        sys.exit(f"service account: {n} {json.dumps(a)[:300]}")
    n, a = admin.call("x:ApiKey/set", {"accountId": svc, "create": {"k": {
        "description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {"127.0.0.1": True},
    }}})
    key = ((a.get("created") or {}).get("k") or {}).get("secret")
    if not key:
        sys.exit(f"service key: {n} {json.dumps(a)[:300]}")
    return key


def domain_id(admin, domain):
    n, q = admin.call("x:Domain/query", {"filter": {"name": domain}})
    ids = q.get("ids") or []
    if not ids:
        sys.exit(f"no domain {domain}: {n} {json.dumps(q)[:300]}")
    return ids[0]


def listener_port(admin, name):
    n, g = admin.call("x:NetworkListener/get", {"ids": None})
    for l in g.get("list") or []:
        if l.get("name") == name:
            return int(next(iter(l["bind"])).rsplit(":", 1)[1])
    sys.exit(f"no listener {name}: {json.dumps(g)[:300]}")


def leaf_sha(sock, sni=None):
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    with ctx.wrap_socket(sock, server_hostname=sni) as t:
        return hashlib.sha256(t.getpeercert(True)).hexdigest()


def starttls_leaf(port):
    """STARTTLS on the plain SMTP listener with no SNI: the leaf's sha256."""
    s = socket.create_connection(("127.0.0.1", port), timeout=15)
    f = s.makefile("rb")

    def reply():
        while True:
            line = f.readline()
            if not line or line[3:4] != b"-":
                return line
    banner = reply()
    s.sendall(b"EHLO probe.example\r\n")
    reply()
    s.sendall(b"STARTTLS\r\n")
    ready = reply()
    if not ready.startswith(b"220"):
        sys.exit(f"STARTTLS refused: {ready!r} (banner {banner!r})")
    f.close()
    return leaf_sha(s)


def p2_live_test(test_bin, url, key, ports, home):
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": home,
        "DC_LIVE_URL": url,
        "DC_LIVE_KEY": key,
        "DC_LIVE_TLS_PORT": str(ports["tls"]),
    }
    r = subprocess.run([test_bin, "--ignored", "--exact", LIVE_TEST, "--nocapture", "--test-threads=1"],
                       env=env, capture_output=True, text=True, timeout=600)
    shas = {}
    for l in r.stdout.splitlines():
        if l.startswith("DC_LIVE"):
            info("P2 k2", l[:900])
            parts = l.split()
            if len(parts) == 3 and parts[1].startswith("sha:"):
                shas[parts[1][4:]] = parts[2]
    check("P2 K2's plant and repair behave on this Stalwart",
          r.returncode == 0 and "test result: ok. 1 passed" in r.stdout,
          f"exit {r.returncode}; {r.stdout[-2500:] if r.returncode else ''}{r.stderr[-2500:] if r.returncode else ''}")
    return shas


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain", default="example.test")
    args = ap.parse_args()
    user, pw = os.environ["STALWART_PROBE_ADMIN"].split(":", 1)
    admin = Admin(args.url, user, pw)
    version = os.environ.get("STALWART_VERSION", "")

    p1_mta_sts(admin, args.url.rstrip("/"), args.domain)
    if os.environ.get("DC_FACTS_ONLY") == "1":
        return finish()

    test_bin = os.environ.get("K2_DC_TEST_BIN", "")
    if not (test_bin and version):
        sys.exit(f"set K2_DC_TEST_BIN and STALWART_VERSION (got {test_bin!r}, {version!r})")
    key = mint_key(admin, args.domain)
    ports = {"tls": listener_port(admin, "https"), "smtp": listener_port(admin, "smtp")}
    info("listeners", json.dumps(ports))

    home = tempfile.mkdtemp(prefix="k2dchome.")
    shas = p2_live_test(test_bin, args.url, key, ports, home)
    shutil.rmtree(home, ignore_errors=True)
    if FAILED:
        return finish()

    # The real reply K2's parser reads (no key: these properties only).
    n, g = admin.call("x:Certificate/get", {"ids": None, "properties": ["subjectAlternativeNames", "notValidBefore", "notValidAfter", "issuer"]})
    info("P2 raw x:Certificate/get reply", json.dumps({"method": n, "reply": g})[:1500])

    got = starttls_leaf(ports["smtp"])
    check("P3 STARTTLS on 25 with no SNI gets the primary", got == shas.get("primary"), f"{got} want {shas.get('primary')}")
    return finish()


def finish():
    print(f"RESULT: {'FAILED ' + ', '.join(FAILED) if FAILED else 'all passed'}", flush=True)
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
