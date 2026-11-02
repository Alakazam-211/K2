#!/usr/bin/env python3
"""End to end: a real k2-daemon, a real (throwaway) Stalwart, no real mail.

The headless check for hosted mail forwards (Rosson 2026-11-02): several
destinations, opt-out, stop, the refusals and the owner's daily cap, through
the daemon's own HTTP routes and K2's own StalwartClient, against a loopback
Stalwart started by tests/mail/stalwart-sandbox-forward-gap.sh. Every
"outside" address is outside.test, which an x:MtaRoute sends to a loopback
SMTP sink this script runs, so nothing leaves the machine. The daemon runs
under a temp HOME with an empty environment: never the real ~/.k2, the
keychain or a running daemon.

  K2_E2E_DAEMON=<k2-daemon> STALWART_PROBE=stalwart-forward-daemon-e2e.py \\
    tests/mail/stalwart-sandbox-forward-gap.sh 0.16.25

Not covered here, because the daemon's background loops only run with K2's
systemd unit up: the minute pass (rows waiting their turn, retries, the hourly
summary) and the reconcile. The Rust tests cover those against fakes.
"""

import importlib.util
import json
import os
import re
import secrets
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("k2_forward_probe", os.path.join(HERE, "stalwart-forward-external-probe.py"))
probe = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(probe)

Client, Sink, check, info, wait_for = probe.Client, probe.Sink, probe.check, probe.info, probe.wait_for
CORE, MAIL, SIEVE, SUBMISSION = probe.CORE, probe.MAIL, probe.SIEVE, probe.SUBMISSION
HOST = "mail.example.test"


class Daemon:
    """A k2-daemon under a temp HOME, spoken to over its own HTTP routes."""

    def __init__(self, binary, key_env):
        self.dir = tempfile.mkdtemp(prefix="k2fe2e.")
        self.home = os.path.join(self.dir, "home")
        for d in ("home/.k2", "shim", "tmp", "run"):
            os.makedirs(os.path.join(self.dir, d))
        # The links in mail point at the box's main host (no Connect here).
        with open(os.path.join(self.home, ".k2", "tunnel.json"), "w") as f:
            json.dump({"subdomain": "box"}, f)
        env = {
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "HOME": self.home,
            "TMPDIR": os.path.join(self.dir, "tmp"),
            "XDG_RUNTIME_DIR": os.path.join(self.dir, "run"),
            "K2_TEST_AGENT_SHIM_DIR": os.path.join(self.dir, "shim"),
            "K2FE2E_STALWART_KEY": key_env,
        }
        self.log = open(os.path.join(self.dir, "daemon.log"), "wb")
        self.proc = subprocess.Popen([binary], env=env, stdout=self.log, stderr=self.log)
        port_file = os.path.join(self.home, ".k2", "heartbeat.port")
        for _ in range(300):
            if os.path.exists(port_file) and open(port_file).read().strip():
                self.port = open(port_file).read().strip()
                try:
                    urllib.request.urlopen(f"http://127.0.0.1:{self.port}/boot-status", timeout=2).read()
                    break
                except OSError:
                    pass
            if self.proc.poll() is not None:
                self.fail_log("the daemon exited")
            time.sleep(0.2)
        else:
            self.fail_log("the daemon never came up")
        self.token = open(os.path.join(self.home, ".k2", "heartbeat.token")).read().strip()
        self.db = next(p for p in (os.path.join(self.home, ".k2", n) for n in ("k2.db", "k2so.db")) if os.path.exists(p))

    def fail_log(self, why):
        print(open(self.log.name).read()[-3000:])
        sys.exit(f"E0 {why}")

    def call(self, method, path, body=None, params=None, token=True):
        q = dict(params or {})
        if token:
            q["token"] = self.token
        url = f"http://127.0.0.1:{self.port}{path}?{urllib.parse.urlencode(q)}"
        data = None
        if body is not None:
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
        req = urllib.request.Request(url, data=data, method=method)
        if body is not None and not isinstance(body, bytes):
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                raw, status = r.read().decode(), r.status
        except urllib.error.HTTPError as e:
            raw, status = e.read().decode(), e.code
        try:
            return status, json.loads(raw)
        except ValueError:
            return status, raw

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()
        shutil.rmtree(self.dir, ignore_errors=True)


def main():
    import argparse

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain", required=True)
    args = ap.parse_args()
    binary = os.environ.get("K2_E2E_DAEMON", "")
    if not os.access(binary, os.X_OK):
        sys.exit(f"set K2_E2E_DAEMON to a built k2-daemon (got {binary!r})")
    user, pw = os.environ["STALWART_PROBE_ADMIN"].split(":", 1)
    domain, tag = args.domain, secrets.token_hex(3)
    admin = Client(args.url, basic=(user, pw))
    n, a = admin.call("x:Domain/query", {"filter": {"name": domain}})
    did = a["ids"][0]
    sink = Sink()

    def account(name, admin_role=False):
        secret = secrets.token_urlsafe(24)
        acct = {"@type": "User", "name": name, "domainId": did, "credentials": {"0": {"@type": "Password", "secret": secret}}}
        if admin_role:
            acct["roles"] = {"@type": "Admin"}
        else:
            acct["quotas"] = {"maxDiskQuota": 104857600, "maxEmails": 1000}
        n, a = admin.call("x:Account/set", {"create": {"k": acct}})
        cid = ((a.get("created") or {}).get("k") or {}).get("id")
        if not cid:
            sys.exit(f"account {name}: {n} {json.dumps(a)[:300]}")
        return cid

    svc_id = account(f"k2fx-svc-{tag}", admin_role=True)
    n, a = admin.call("x:ApiKey/set", {"accountId": svc_id, "create": {"k": {"description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {"127.0.0.1": True}}}})
    key = ((a.get("created") or {}).get("k") or {}).get("secret")
    if not key:
        sys.exit(f"service key: {n} {json.dumps(a)[:300]}")
    svc = Client(args.url, bearer=key)
    alice, bob = account(f"k2fx-alice-{tag}"), account(f"k2fx-bob-{tag}")
    alice_addr, bob_addr = f"k2fx-alice-{tag}@{domain}", f"k2fx-bob-{tag}@{domain}"

    # outside.test → the sink.
    n, a = admin.call("x:MtaRoute/set", {"create": {"r": {"@type": "Relay", "name": f"k2fxsink{tag}", "address": "127.0.0.1", "port": sink.port, "protocol": "smtp", "implicitTls": False}}})
    if not ((a.get("created") or {}).get("r") or {}).get("id"):
        sys.exit(f"sink route: {n} {json.dumps(a)[:300]}")
    n, g = admin.call("x:MtaOutboundStrategy/get", {"ids": ["singleton"]})
    old = g["list"][0]["route"]
    shifted = {str(int(k) + 1): v for k, v in (old.get("match") or {}).items()}
    shifted["0"] = {"if": "rcpt_domain == 'outside.test'", "then": f"'k2fxsink{tag}'"}
    admin.call("x:MtaOutboundStrategy/set", {"update": {"singleton": {"route": {"match": shifted, "else": old["else"]}}}})
    admin.call("x:Action/set", {"create": {"r": {"@type": "ReloadSettings"}}})
    time.sleep(2)

    def limits():
        n, g = admin.call("x:SieveUserInterpreter/get", {"ids": ["singleton"], "properties": ["maxRedirects", "maxOutMessages"]})
        row = (g.get("list") or [{}])[0]
        return row.get("maxRedirects"), row.get("maxOutMessages")

    def ham(mid):
        """bob → alice, authenticated (the spam filter leaves it alone)."""
        n, a = svc.call("Identity/get", {"accountId": bob, "ids": None}, using=(CORE, MAIL, SUBMISSION))
        ident = next((i["id"] for i in (a.get("list") or []) if i.get("email", "").lower() == bob_addr.lower()), None)
        if not ident:
            n, a = svc.call("Identity/set", {"accountId": bob, "create": {"i": {"email": bob_addr}}}, using=(CORE, MAIL, SUBMISSION))
            ident = ((a.get("created") or {}).get("i") or {}).get("id")
        n, q = svc.call("Mailbox/query", {"accountId": bob, "filter": {"role": "drafts"}}, using=(CORE, MAIL))
        box = (q.get("ids") or [None])[0]
        email = {"mailboxIds": {box: True}, "from": [{"email": bob_addr, "name": "Bob"}], "to": [{"email": alice_addr}], "subject": f"probe {mid}",
                 "bodyValues": {"t": {"value": f"Hello ({mid})."}}, "textBody": [{"partId": "t", "type": "text/plain"}]}
        svc.batch((CORE, MAIL, SUBMISSION), [
            ["Email/set", {"accountId": bob, "create": {"e": email}}, "0"],
            ["EmailSubmission/set", {"accountId": bob, "create": {"s": {"emailId": "#e", "identityId": ident,
                "envelope": {"mailFrom": {"email": bob_addr}, "rcptTo": [{"email": alice_addr}]}}}, "onSuccessDestroyEmail": ["#s"]}, "1"],
        ])

    def rcpts_of(mid, want, secs=40):
        wait_for(lambda: len(sink.with_id(mid)) >= want, secs)
        time.sleep(3)
        return sorted(r for m in sink.with_id(mid) for r in m["rcpt"])

    # ── the daemon ──────────────────────────────────────────────────
    d = Daemon(binary, key)
    try:
        now = int(time.time())
        with sqlite3.connect(d.db, timeout=30) as c:
            c.execute(
                "INSERT OR REPLACE INTO mail_server (id, status, pinned_version, installed_version, hostname, api_url, api_key_ref, updated_at) "
                "VALUES (1, 'running', '0.16.25', '0.16.25', ?, ?, 'env:K2FE2E_STALWART_KEY', ?)",
                (HOST, args.url, now),
            )
            c.execute("INSERT INTO mail_domains (id, domain, stalwart_domain_id, send_mode, status, created_at) VALUES ('d1', ?, ?, 'direct', 'verified', ?)", (domain, did, now))
            for rid, addr, acct in (("ma-alice", alice_addr, alice), ("ma-bob", bob_addr, bob)):
                c.execute(
                    "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) VALUES (?, ?, 'd1', ?, 'e2e', 'active', ?)",
                    (rid, addr, acct, now),
                )
        info("fixture", f"daemon on :{d.port}, Stalwart limits as shipped {limits()}")

        # E1 several destinations: on at once, one notice each, before any mail.
        two = ["me@outside.test", "two@outside.test"]
        st, v = d.call("POST", "/cli/mail/forward", {"address": alice_addr, "to": two})
        check("E1 set with two outside addresses answers 200", st == 200, f"{st} {json.dumps(v)[:500]}")
        states = [x.get("state") for x in (v.get("destinations") or [])] if isinstance(v, dict) else []
        check("E1 both are active at once", states == ["active", "active"], json.dumps(states))
        notices = wait_for(lambda: len([m for m in sink.got if "k2es_" in m["data"]]) >= 2 and [m for m in sink.got if "k2es_" in m["data"]], 40) or []
        check("E1 each address got its notice with a stop link", sorted(r for m in notices for r in m["rcpt"]) == two, json.dumps([m["rcpt"] for m in notices]))
        check("E1 the notice comes from the mailbox (no system sender here)", all((m["from"] or "").lower() == alice_addr.lower() for m in notices), json.dumps([m["from"] for m in notices]))
        check("E1 the notice says who and which mailbox, and asks no confirm", all(alice_addr in m["data"] and "k2ev_" not in m["data"] for m in notices), notices[0]["data"][:400] if notices else "")
        lim = limits()
        check("E1 K2's own call raised the mail server's limit", lim == (5, 6), json.dumps(lim))

        # E2 real mail: every active destination gets its copy, and one stays.
        mid = f"e2-{tag}"
        ham(mid)
        got = rcpts_of(mid, 2)
        check("E2 a message reaches both outside addresses", got == two, json.dumps(got))
        copies = sink.with_id(mid)
        check("E2 each copy has the mailbox as MAIL FROM and the marker", all((m["from"] or "").lower() == alice_addr.lower() and f"X-K2-Forwarded: {HOST}" in m["data"] for m in copies), "")

        # E3 show, list.
        st, v = d.call("GET", "/cli/mail/forward", params={"address": alice_addr})
        rows = v.get("forwards") or [] if isinstance(v, dict) else []
        check("E3 show lists both, notice sent", [(r.get("to"), r.get("state"), r.get("notice")) for r in rows] == [(t, "active", "sent") for t in two], json.dumps(rows)[:400])
        st, v = d.call("GET", "/cli/mail/forwards")
        check("E3 list has both", st == 200 and len(v.get("forwards") or []) == 2, f"{st}")

        # E4 stop: GET renders and changes nothing; only the POST stops, one address.
        stop_mail = next(m for m in notices if m["rcpt"] == ["two@outside.test"])
        # The token may be folded by quoted-printable: undo that, then match.
        flat = stop_mail["data"].replace("=\r\n", "").replace("=\n", "")
        token = re.search(r"k2es_[A-Za-z0-9_\-]{43}", flat).group(0)
        st, page = d.call("GET", "/cli/email/stop", params={"t": token}, token=False)
        check("E4 GET renders a Stop button", st == 200 and "<button" in str(page), f"{st}")
        st, v = d.call("GET", "/cli/mail/forward", params={"address": alice_addr})
        check("E4 the GET stopped nothing", [r.get("state") for r in v.get("forwards")] == ["active", "active"], "")
        st, page = d.call("POST", "/cli/email/stop", f"t={token}".encode(), params={}, token=False)
        check("E4 POST stops", st == 200 and "Stopped" in str(page), f"{st} {str(page)[:200]}")
        st, v = d.call("GET", "/cli/mail/forward", params={"address": alice_addr})
        by = {r.get("to"): r for r in v.get("forwards")}
        check("E4 only that address stopped", by["me@outside.test"]["state"] == "active" and by["two@outside.test"]["pauseReason"] == "revoked", json.dumps(by)[:400])
        mid = f"e4-{tag}"
        ham(mid)
        got = rcpts_of(mid, 1)
        check("E4 the next message goes only to the other address", got == ["me@outside.test"], json.dumps(got))

        # E5 stopped means stopped.
        st, v = d.call("POST", "/cli/mail/forward/add", {"address": alice_addr, "to": "two@outside.test"})
        check("E5 add refuses a stopped address", st == 409 and v["error"]["code"] == "destination_stopped", f"{st} {json.dumps(v)[:300]}")
        st, v = d.call("POST", "/cli/mail/forward/resume", {"address": alice_addr, "to": "two@outside.test"})
        check("E5 resume refuses it", st == 409 and v["error"]["code"] == "destination_stopped", f"{st}")
        st, v = d.call("POST", "/cli/mail/forward/add", {"address": alice_addr, "to": "two@outside.test", "restartStopped": True})
        check("E5 the owner can choose it again", st == 200 and v.get("state") == "active", f"{st} {json.dumps(v)[:300]}")

        # E6 a forward that can't arrive is refused; a real one on this box is same-box.
        st, v = d.call("POST", "/cli/mail/forward/add", {"address": alice_addr, "to": f"ghost-{tag}@{domain}"})
        check("E6 an address on this box's domain with no mailbox is refused", st == 400 and "never reach another provider" in v["error"]["hint"], f"{st} {json.dumps(v)[:300]}")
        st, v = d.call("POST", "/cli/mail/forward/add", {"address": alice_addr, "to": bob_addr})
        check("E6 a mailbox here is same-box", st == 200 and v.get("kind") == "internal", f"{st} {json.dumps(v)[:300]}")
        mid = f"e6-{tag}"
        ham(mid)
        got = rcpts_of(mid, 2)
        check("E6 mixed: both outside addresses still get it", got == two, json.dumps(got))

        # E7 add, rm and pause touch only what they name.
        st, v = d.call("POST", "/cli/mail/forward/rm", {"address": alice_addr, "to": "two@outside.test"})
        check("E7 rm removes one", st == 200 and v.get("removed") == ["two@outside.test"] and "me@outside.test" in v.get("remaining"), f"{st} {json.dumps(v)[:300]}")
        st, v = d.call("POST", "/cli/mail/forward/pause", {"address": alice_addr, "to": "me@outside.test"})
        check("E7 pause takes one", st == 200 and v.get("state") == "paused", f"{st}")
        mid = f"e7-{tag}"
        ham(mid)
        time.sleep(6)
        check("E7 a paused address gets nothing", not sink.with_id(mid), json.dumps([m["rcpt"] for m in sink.with_id(mid)]))

        # E9 out-of-office text is never a forward (review finding 1).
        evil = "evil@outside.test"
        st, v = d.call("POST", "/cli/mail/forward/resume", {"address": alice_addr, "to": "me@outside.test"})
        check("E9 setup: the forward is on", st == 200, f"{st}")
        for text in (f'Please redirect "{evil}" if urgent', f'# k2-forward\nredirect :copy "{evil}";'):
            st, v = d.call("POST", "/cli/mail/ooo", {"address": alice_addr, "text": text})
            check("E9 the text is accepted", st == 200 and v.get("text") == text, f"{st} {json.dumps(v)[:200]}")
            st, v = d.call("POST", "/cli/mail/ooo", {"address": alice_addr, "text": text})
            check("E9 set again, read back as text", st == 200 and v.get("text") == text, f"{st}")
            st, v = d.call("POST", "/cli/mail/ooo/unset", {"address": alice_addr})
            check("E9 unset", st == 200 and v.get("enabled") is False, f"{st}")
            mid = f"e9-{tag}-{secrets.token_hex(2)}"
            ham(mid)
            got = rcpts_of(mid, 1)
            check("E9 the forward from the rows still runs, and nothing else", got == ["me@outside.test"], json.dumps(got))

        # E8 the caps.
        many = [f"x{i}@outside.test" for i in range(6)]
        st, v = d.call("POST", "/cli/mail/forward", {"address": bob_addr, "to": many})
        check("E8 six destinations are refused", st == 409 and v["error"]["code"] == "too_many_destinations", f"{st}")
        st, v = d.call("POST", "/cli/mail/config/forward-new-per-day", {"perDay": 5000})
        check("E8 above the ceiling is refused", st == 400 and "at most 1000" in v["error"]["hint"], f"{st} {json.dumps(v)[:200]}")
        st, v = d.call("POST", "/cli/mail/config/forward-new-per-day", {"perDay": 300})
        check("E8 the owner sets the daily cap", st == 200 and v.get("perDay") == 300, f"{st} {json.dumps(v)[:200]}")
        st, v = d.call("GET", "/cli/mail/config")
        check("E8 config shows it", st == 200 and (v.get("forwarding") or {}).get("newOutsidePerDay") == 300, json.dumps(v.get("forwarding") if isinstance(v, dict) else v)[:200])
        st, v = d.call("GET", "/cli/mail/config/forward-new-per-day")
        check("E8 GET on the mutation is 405", st == 405, f"{st}")
        st, v = d.call("POST", "/cli/mail/config/forward-new-per-day", {"perDay": 1}, token=False)
        check("E8 no credential is refused", st in (401, 403), f"{st}")
        st, v = d.call("POST", "/cli/mail/forward/unset", {"address": alice_addr})
        check("E8 unset removes the rest", st == 200 and v.get("removed") is True, f"{st} {json.dumps(v)[:200]}")
        mid = f"e8-{tag}"
        ham(mid)
        time.sleep(6)
        check("E8 after unset nothing is forwarded", not sink.with_id(mid), json.dumps([m["rcpt"] for m in sink.with_id(mid)]))
    finally:
        d.stop()
    print(f"== forward daemon e2e: {len(probe.FAILED)} failed check(s): {probe.FAILED} ==", flush=True)
    sys.exit(1 if probe.FAILED else 0)


if __name__ == "__main__":
    main()
