#!/usr/bin/env python3
"""Live probe: mailbox API keys on a real Stalwart (0.46.6 security fix).

The gap: a mailbox login (its password) can mint its own Stalwart API key
(`sysApiKeyCreate`, on in Stalwart's default user role). A key is its own
credential: it survives a password change made as a `credentials/0` patch
(section 0 prints what K2's own whole-`credentials` write does: on 0.16.10
and 0.16.25 it drops every key and app password too). K2's fix:

  1. every mailbox K2 mints is created with `sysApiKeyCreate` in
     `disabledPermissions` (merged with the Sieve lock);
  2. existing mailboxes get the same Merge patch at boot/upgrade;
  3. a mailbox password rotation destroys the mailbox's API keys
     (`x:ApiKey/query` + `x:ApiKey/set destroy`, service key, delegated
     by `accountId`), after the new password is set;
  4. K2's own key (`x:ApiKey/set` on the service account) still mints.

This script runs those exact calls (the shapes in
crates/k2-daemon/src/mail/jmap.rs) against a Stalwart you point it at.
It creates ONLY throwaway principals (`k2probe-*-<tag>`) and destroys them
at the end. It sends no mail.

    STALWART_PROBE_ADMIN='admin@example.test:<secret>' \\
      python3 -I tests/mail/stalwart-apikey-probe.py --url http://127.0.0.1:8080 \\
      [--domain example.test]

The admin credential comes from the environment only (never argv). Run it
only against a disposable Stalwart (tests/mail/stalwart-sandbox-probe.sh
starts one), never a customer box. Every check prints [PASS] / [FAIL] /
[INFO]; the exit code is 1 if any check failed (all checks always run).
"""

import argparse
import base64
import json
import os
import secrets
import sys
import urllib.error
import urllib.request

CORE = "urn:ietf:params:jmap:core"
MAIL = "urn:ietf:params:jmap:mail"
REGISTRY = "urn:stalwart:jmap"

# Must match crates/k2-daemon/src/mail/sieve_lockdown.rs and
# api_key_lockdown.rs (what K2 puts in disabledPermissions at mint).
SIEVE_WRITE = ["jmapSieveScriptCreate", "jmapSieveScriptUpdate", "jmapSieveScriptDestroy"]
API_KEY_DENIED = ["sysApiKeyCreate"]
# Names probed for information only (does this version know them?).
INFO_NAMES = ["sysApiKeyUpdate", "sysApiKeyDestroy", "sysApiKeyGet", "sysApiKeyQuery"]

FAILED = []


def check(step, ok, detail=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {step}: {detail}", flush=True)
    if not ok:
        FAILED.append(step)


def info(step, detail=""):
    print(f"[INFO] {step}: {detail}", flush=True)


class Client:
    """One principal: basic auth (user:password) or a bearer API key."""

    def __init__(self, base, basic=None, bearer=None):
        self.base = base.rstrip("/")
        if basic:
            tok = base64.b64encode(f"{basic[0]}:{basic[1]}".encode()).decode()
            self.auth = f"Basic {tok}"
        else:
            self.auth = f"Bearer {bearer}"
        self._session = None

    def _req(self, method, path_or_url, body=None):
        url = path_or_url if path_or_url.startswith("http") else self.base + path_or_url
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Authorization", self.auth)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.status, json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            text = e.read().decode(errors="replace")[:300]
            return e.code, {"httpError": e.code, "body": text}

    def session(self):
        """(status, session doc). Not cached: a revoked key must show."""
        return self._req("GET", "/jmap/session")

    def api_url(self):
        if self._session is None:
            st, doc = self.session()
            if st != 200:
                raise RuntimeError(f"session HTTP {st}: {doc}")
            self._session = doc
        path = self._session["apiUrl"]
        if path.startswith("http"):
            path = "/" + path.split("://", 1)[1].split("/", 1)[1]
        return self.base + path

    def own_account(self):
        self.api_url()
        prim = self._session.get("primaryAccounts", {})
        return prim.get(REGISTRY) or prim.get(MAIL) or next(iter(self._session["accounts"]))

    def call(self, method, args, using=(CORE, REGISTRY)):
        """(name, args) of the first method response; ('http', {...}) on HTTP error."""
        if "accountId" not in args:
            args = dict(args, accountId=self.own_account())
        st, resp = self._req("POST", self.api_url(), {"using": list(using), "methodCalls": [[method, args, "0"]]})
        if st != 200:
            return "http", resp
        first = resp["methodResponses"][0]
        return first[0], first[1]


def nonempty(a, k):
    v = a.get(k)
    return bool(v) if isinstance(v, (dict, list)) else False


def refused(name, a):
    """A /set that wrote nothing: method error, HTTP error, or a not* entry."""
    if name in ("error", "http"):
        return True
    return not (nonempty(a, "created") or nonempty(a, "updated") or nonempty(a, "destroyed")) and (
        nonempty(a, "notCreated") or nonempty(a, "notUpdated") or nonempty(a, "notDestroyed")
    )


def merge(existing, denied):
    """dav::merge_permissions (Inherit/Merge only; enough for the probe)."""
    kind = (existing or {}).get("@type", "Inherit")
    en = dict((existing or {}).get("enabledPermissions") or {}) if kind != "Inherit" else {}
    dis = dict((existing or {}).get("disabledPermissions") or {}) if kind != "Inherit" else {}
    before = (dict(en), dict(dis))
    for n in denied:
        en.pop(n, None)
        dis[n] = True
    if (en, dis) == before:
        return None
    return {"@type": "Merge", "enabledPermissions": en, "disabledPermissions": dis}


def is_denied(perms, names):
    return isinstance(perms, dict) and perms.get("@type") in ("Merge", "Replace") and all(
        (perms.get("disabledPermissions") or {}).get(n) is True for n in names
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", required=True, help="Stalwart HTTP base URL (management/JMAP listener)")
    ap.add_argument("--domain", help="hosted domain for the throwaway mailboxes (default: the admin's domain)")
    args = ap.parse_args()
    cred = os.environ.get("STALWART_PROBE_ADMIN", "")
    if ":" not in cred:
        sys.exit("set STALWART_PROBE_ADMIN=<admin user>:<secret> in the environment")
    admin_user, admin_pw = cred.split(":", 1)
    domain = args.domain or admin_user.split("@", 1)[-1]
    tag = secrets.token_hex(3)
    admin = Client(args.url, basic=(admin_user, admin_pw))

    st, sess = admin.session()
    if st != 200:
        sys.exit(f"admin session HTTP {st}: {sess}")
    n, a = admin.call("x:Domain/query", {"filter": {"name": domain}})
    if n != "x:Domain/query" or not a.get("ids"):
        sys.exit(f"domain {domain} not found: {n} {a}")
    did = a["ids"][0]
    info("fixture", f"Stalwart at {args.url}; domain {domain} ({did}); run {tag}")

    created_accounts = []

    def account_create(client, name, perms, admin_role=False):
        pw = secrets.token_urlsafe(24)
        acct = {
            "@type": "User",
            "name": name,
            "domainId": did,
            "credentials": {"0": {"@type": "Password", "secret": pw}},
        }
        if admin_role:
            acct["roles"] = {"@type": "Admin"}
            acct["description"] = "k2 probe service account"
        else:
            acct["quotas"] = {"maxDiskQuota": 104857600, "maxEmails": 1000}
        if perms is not None:
            acct["permissions"] = perms
        n, a = client.call("x:Account/set", {"create": {"k2": acct}})
        cid = (a.get("created") or {}).get("k2", {}).get("id") if n == "x:Account/set" else None
        if cid:
            created_accounts.append(cid)
            return cid, pw, None
        return None, None, f"{n} {json.dumps(a)[:300]}"

    def perms_of(client, aid):
        n, a = client.call("x:Account/get", {"ids": [aid], "properties": ["@type", "name", "roles", "permissions"]})
        lst = a.get("list") or []
        return lst[0].get("permissions") if lst else None

    def mint_key(client, account_id, desc, ips=None):
        """K2's api_key_create shape. Returns (secret, id, raw)."""
        body = {"description": desc, "permissions": {"@type": "Inherit"}, "allowedIps": ips or {}}
        n, a = client.call("x:ApiKey/set", {"accountId": account_id, "create": {"k2": body}})
        c = (a.get("created") or {}).get("k2") if n == "x:ApiKey/set" else None
        return (c or {}).get("secret"), (c or {}).get("id"), (n, a)

    def key_works(secret):
        st, _ = Client(args.url, bearer=secret).session()
        return st == 200, st

    def keys_on(client, account_id):
        n, a = client.call("x:ApiKey/query", {"accountId": account_id})
        if n != "x:ApiKey/query":
            return None, f"{n} {json.dumps(a)[:300]}"
        return list(a.get("ids") or []), None

    def set_password(client, account_id):
        """K2's account_set_password shape (the whole credentials map)."""
        pw = secrets.token_urlsafe(24)
        n, a = client.call(
            "x:Account/set",
            {"update": {account_id: {"credentials": {"0": {"@type": "Password", "secret": pw}}}}},
        )
        return (n == "x:Account/set" and account_id in (a.get("updated") or {})), pw, (n, a)

    def app_pw_create(account_id):
        n, a = svc.call("x:AppPassword/set", {"accountId": account_id, "create": {"k2": {
            "description": "probe", "permissions": {"@type": "Inherit"}, "allowedIps": {}}}})
        c = ((a.get("created") or {}).get("k2") or {}) if n == "x:AppPassword/set" else {}
        return c.get("secret"), c.get("id"), (n, a)

    def app_pws_on(account_id):
        n, a = svc.call("x:AppPassword/query", {"accountId": account_id})
        return list(a.get("ids") or []) if n == "x:AppPassword/query" else None

    def cred_shape(account_id):
        """x:Account/get credentials with every secret redacted."""
        n, a = svc.call("x:Account/get", {"ids": [account_id], "properties": ["credentials"]})
        creds = ((a.get("list") or [{}])[0] or {}).get("credentials")

        def red(v):
            if isinstance(v, dict):
                return {k: ("<redacted>" if k.lower() in ("secret", "hash", "password") else red(x)) for k, x in v.items()}
            if isinstance(v, list):
                return [red(x) for x in v]
            return v
        return json.dumps(red(creds))[:400] if n == "x:Account/get" else f"{n} {json.dumps(a)[:200]}"

    # ── K2's service account + loopback key (the enable flow) ────────
    svc_id, _, err = account_create(admin, f"k2probe-svc-{tag}", None, admin_role=True)
    if not svc_id:
        sys.exit(f"service account create failed: {err}")
    svc_secret, _, raw = mint_key(admin, svc_id, "K2 daemon mail supervisor", {"127.0.0.1": True})
    if not svc_secret:
        sys.exit(f"service key mint failed: {raw}")
    svc = Client(args.url, bearer=svc_secret)
    check("fixture: service key works", key_works(svc_secret)[0], f"svc {svc_id}")

    # ── 0. The gap, before the fix ───────────────────────────────────
    old_id, old_pw, err = account_create(svc, f"k2probe-old-{tag}", None)
    check("0 mailbox without the lock created", bool(old_id), err or old_id)
    old_user = Client(args.url, basic=(f"k2probe-old-{tag}@{domain}", old_pw))
    st, _ = old_user.session()
    check("0 mailbox password logs in", st == 200, f"HTTP {st}")
    user_key, user_key_id, raw = mint_key(old_user, old_id, "user-minted")
    check("0 gap: mailbox password mints its own API key (pre-fix)", bool(user_key), f"id {user_key_id}; {str(raw)[:200] if not user_key else ''}")
    check("0 the user-minted key works", bool(user_key) and key_works(user_key)[0], "")
    ap_secret, ap_id, raw = app_pw_create(old_id)
    info("0 K2-style app password on the same mailbox", f"id {ap_id}" if ap_id else str(raw)[:200])
    ids, err = keys_on(svc, old_id)
    check("0 service key lists the mailbox's API keys (x:ApiKey/query)", ids is not None and user_key_id in ids, f"{ids} {err or ''}")
    check("0 the API key list holds no app password", ids is not None and ap_id not in ids, f"{ids}")
    info("0 credentials before rotation (redacted)", cred_shape(old_id))
    # Rotation semantics (the brief's premise: keys survive a rotation).
    ok, pw2, raw = set_password(svc, old_id)
    check("0 service key rotates the password (K2's credentials {0: Password} shape)", ok, str(raw)[:200] if not ok else "")
    info("0 credentials after rotation (redacted)", cred_shape(old_id))
    works, st = key_works(user_key) if user_key else (False, None)
    keys_after, _ = keys_on(svc, old_id)
    aps_after = app_pws_on(old_id)
    info("0 ROTATION SEMANTICS", f"user API key after K2's rotation: {'STILL WORKS' if works else 'refused'} (HTTP {st}); "
         f"x:ApiKey/query {keys_after}; x:AppPassword/query {aps_after} (app password {ap_id} "
         f"{'kept' if aps_after and ap_id in aps_after else 'GONE'})")
    st, _ = Client(args.url, basic=(f"k2probe-old-{tag}@{domain}", old_pw)).session()
    check("0 the old password is refused after rotation", st != 200, f"HTTP {st}")
    old_user = Client(args.url, basic=(f"k2probe-old-{tag}@{domain}", pw2))
    # The same with a JMAP patch path (only credential 0), for comparison.
    k_b, k_b_id, _ = mint_key(old_user, old_id, "user-minted-b")
    ap_b, ap_b_id, _ = app_pw_create(old_id)
    pw2b = secrets.token_urlsafe(24)
    n, a = svc.call("x:Account/set", {"update": {old_id: {"credentials/0": {"@type": "Password", "secret": pw2b}}}})
    patched = n == "x:Account/set" and old_id in (a.get("updated") or {})
    if patched:
        pw2 = pw2b
        old_user = Client(args.url, basic=(f"k2probe-old-{tag}@{domain}", pw2))
        st_new, _ = old_user.session()
        works_b, st_b = key_works(k_b) if k_b else (False, None)
        aps_b = app_pws_on(old_id)
        info("0 ROTATION SEMANTICS (patch path credentials/0)", f"accepted; new password HTTP {st_new}; user key "
             f"{'STILL WORKS' if works_b else 'refused'} (HTTP {st_b}); x:ApiKey/query {keys_on(svc, old_id)[0]}; "
             f"app password {'kept' if aps_b and ap_b_id in aps_b else 'GONE'}")
    else:
        info("0 ROTATION SEMANTICS (patch path credentials/0)", f"refused: {n} {json.dumps(a)[:200]}")

    # ── 1. Mint with the lock (addresses::mint_address path) ─────────
    lock = merge(None, SIEVE_WRITE + API_KEY_DENIED)
    new_id, new_pw, err = account_create(svc, f"k2probe-new-{tag}", lock)
    check("1 mint with sysApiKeyCreate denied accepted", bool(new_id), err or new_id)
    if not new_id:
        for name in API_KEY_DENIED:
            i, _, e = account_create(svc, f"k2probe-name-{tag}", merge(None, [name]))
            info("1 name probe", f"{name}: {'ACCEPTED' if i else 'REJECTED ' + str(e)}")
    else:
        p = perms_of(svc, new_id)
        check("1 read back: sysApiKeyCreate + Sieve writes disabled", is_denied(p, SIEVE_WRITE + API_KEY_DENIED), json.dumps(p))
        new_user = Client(args.url, basic=(f"k2probe-new-{tag}@{domain}", new_pw))
        st, _ = new_user.session()
        check("1 locked mailbox still logs in", st == 200, f"HTTP {st}")
        n, a = new_user.call("Mailbox/get", {"ids": None, "properties": ["name", "role"]}, using=(CORE, MAIL))
        check("1 locked mailbox still reads its mail (Mailbox/get)", n == "Mailbox/get", f"{n}")
        k, kid, raw = mint_key(new_user, new_user.own_account(), "should-be-refused")
        check("1 locked mailbox: API key mint refused", not k and refused(*raw), json.dumps(raw[1])[:300])
        if k:
            info("1 UNEXPECTED key minted", kid)
    for name in INFO_NAMES:
        i, _, e = account_create(svc, f"k2probe-info{INFO_NAMES.index(name)}-{tag}", merge(None, [name]))
        info("1 name known to this version?", f"{name}: {'yes (accepted)' if i else 'no (' + str(e)[:160] + ')'}")

    # ── 2. Patch an existing mailbox (the boot/upgrade step) ─────────
    cur = perms_of(svc, old_id)
    nxt = merge(cur, API_KEY_DENIED)
    n, a = svc.call("x:Account/set", {"update": {old_id: {"permissions": nxt}}})
    check("2 patch existing mailbox (Merge sysApiKeyCreate)", n == "x:Account/set" and old_id in (a.get("updated") or {}), f"{n} {json.dumps(a)[:200]}")
    p = perms_of(svc, old_id)
    check("2 read back denies sysApiKeyCreate", is_denied(p, API_KEY_DENIED), json.dumps(p))
    check("2 second patch is a no-op (idempotent)", merge(p, API_KEY_DENIED) is None, "merge → None")
    k, kid, raw = mint_key(old_user, old_id, "after-patch")
    check("2 patched mailbox: API key mint refused", not k and refused(*raw), json.dumps(raw[1])[:300])
    if user_key:
        works, st = key_works(user_key)
        info("2 the key minted before the patch", f"{'still works' if works else 'refused'} (HTTP {st}) — the patch alone does not revoke")

    # ── 3. K2's own minting still works ──────────────────────────────
    k, kid, raw = mint_key(svc, svc_id, "K2 daemon mail supervisor", {"127.0.0.1": True})
    check("3 service key mints a new service key (x:ApiKey/set)", bool(k), f"id {kid}" if k else str(raw)[:300])
    if k:
        check("3 the new service key works", key_works(k)[0], "")
    if new_id:
        k2, kid2, raw = mint_key(svc, new_id, "service-minted on a locked mailbox")
        info("3 service key minting ON a locked mailbox (not used by K2)", f"{'allowed id ' + str(kid2) if k2 else 'refused ' + json.dumps(raw[1])[:200]}")
        if kid2:
            svc.call("x:ApiKey/set", {"accountId": new_id, "destroy": [kid2]})

    # ── 4. List + destroy a mailbox's keys (the rotation revoke) ─────
    # A fresh unlocked mailbox: the user mints keys, K2 adds an app
    # password. The destroy is tested on its own first (no rotation), so
    # it is proven even where a rotation would drop the key by itself.
    rot_id, rot_pw, err = account_create(svc, f"k2probe-rot-{tag}", None)
    check("4 fixture mailbox created", bool(rot_id), err or rot_id)
    rot_user = Client(args.url, basic=(f"k2probe-rot-{tag}@{domain}", rot_pw))
    k4, k4_id, raw = mint_key(rot_user, rot_id, "user-minted")
    k4b, k4b_id, _ = mint_key(rot_user, rot_id, "user-minted-2")
    check("4 user minted two keys", bool(k4 and k4b), str(raw)[:200] if not k4 else "")
    ap4, ap4_id, _ = app_pw_create(rot_id)
    ids, err = keys_on(svc, rot_id)
    check("4 service key lists both keys (x:ApiKey/query, accountId = mailbox)", ids is not None and {k4_id, k4b_id} <= set(ids), f"{ids} {err or ''}")
    check("4 the list holds no app password", ids is not None and ap4_id not in ids, f"{ids}")
    n, a = svc.call("x:ApiKey/get", {"accountId": rot_id, "ids": ids or [], "properties": ["id", "description", "createdAt"]})
    info("4 x:ApiKey/get rows", f"{n} {json.dumps(a.get('list'))[:300]}")
    check("4 x:ApiKey/get never returns a secret", n == "x:ApiKey/get" and all("secret" not in r for r in (a.get("list") or [])), "")
    if ids:
        n, a = svc.call("x:ApiKey/set", {"accountId": rot_id, "destroy": ids})
        check("4 x:ApiKey/set destroy every listed key", n == "x:ApiKey/set" and sorted(a.get("destroyed") or []) == sorted(ids), f"{n} {json.dumps(a)[:300]}")
    left, _ = keys_on(svc, rot_id)
    check("4 no keys left on the mailbox", left == [], f"{left}")
    check("4 both user keys are refused after the destroy", bool(k4 and k4b) and not key_works(k4)[0] and not key_works(k4b)[0], "")
    st, _ = rot_user.session()
    check("4 the mailbox password still logs in (destroy touched only keys)", st == 200, f"HTTP {st}")
    aps = app_pws_on(rot_id)
    check("4 the app password is untouched by the key destroy", aps is not None and ap4_id in aps, f"{aps}")
    n, a = svc.call("x:ApiKey/set", {"accountId": rot_id, "destroy": []})
    check("4 an empty destroy is harmless (idempotent revoke)", n == "x:ApiKey/set", f"{n}")
    ids_svc, _ = keys_on(svc, svc_id)
    check("4 the service account's own keys are not touched", bool(ids_svc), f"{ids_svc}")

    # ── 5. K2's full rotation: new password first, then the revoke ───
    k5, k5_id, _ = mint_key(rot_user, rot_id, "user-minted-3")
    check("5 user key minted", bool(k5), "")
    ok, pw5, raw = set_password(svc, rot_id)
    check("5 new password set", ok, str(raw)[:200] if not ok else "")
    ids, _ = keys_on(svc, rot_id)
    info("5 keys listed after the password set", f"{ids}")
    if ids:
        n, a = svc.call("x:ApiKey/set", {"accountId": rot_id, "destroy": ids})
        check("5 destroy after the password set", n == "x:ApiKey/set" and sorted(a.get("destroyed") or []) == sorted(ids), f"{n} {json.dumps(a)[:200]}")
    check("5 the user key is refused after the rotation", bool(k5) and not key_works(k5)[0], "")
    st, _ = Client(args.url, basic=(f"k2probe-rot-{tag}@{domain}", pw5)).session()
    check("5 the new password logs in", st == 200, f"HTTP {st}")
    aps = app_pws_on(rot_id)
    info("5 app password after K2's rotation", f"{'kept' if aps and ap4_id in aps else 'GONE'} ({aps})")

    # ── Cleanup: every throwaway principal ───────────────────────────
    for aid in reversed(created_accounts):
        if aid == svc_id:
            continue
        svc.call("x:Account/set", {"destroy": [aid]})
    admin.call("x:Account/set", {"destroy": [svc_id]})
    info("cleanup", f"destroyed {len(created_accounts)} throwaway account(s)")

    print(f"== {len(FAILED)} failed check(s): {FAILED} ==", flush=True)
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
