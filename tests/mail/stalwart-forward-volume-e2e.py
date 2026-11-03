#!/usr/bin/env python3
"""End to end: the owner's numbers for how much a mailbox may forward, through
a real k2-daemon and a real (throwaway) Stalwart, on both versions in the fleet.
No real mail: every "outside" address is outside.test, sent to a loopback sink.

The hourly number is Stalwart's to enforce (K2 writes it as a per-sender
throttle); the daily one is K2's own pass, on the delivery events Stalwart posts.

  V0  a box from before this: Stalwart holds the throttles the last release wrote
  V1  the defaults show in config; out-of-range is refused; no credential is refused
  V2  K2 raises the hourly number on that box: the same throttle, updated in
      place, read back from Stalwart, reloaded; the whole-box one untouched
  V3  drift: someone puts the old number back; K2's reconcile (the function boot
      and the hourly pass call) puts it right
  V4  it holds. 0.16.25: a raised limit of 120, outside and same-box: a first
      message's 100 go, a second's first 20 go and the rest wait, none dropped.
      Raised again while they wait: the next message goes without a restart.
      0.16.10: K2 refuses outside forwards, and same-box copies aren't held
      (recorded). The number is written and read back all the same
  V5  (0.16.25) a raised daily limit, K2's real pass: 600 copies, past both the old
      100 an hour and the old 500 a day, all arrive. K2's route and pass then
      read the batches Stalwart really signed: nothing pauses at 500, all 100
      pause at the raised 550, the real script loses its forward, the notice names
      the setting, and the next message stays in the mailbox. (That part runs in
      a test build of the daemon: outcomes need the webhook, which K2 only sets up
      on a box with its systemd unit. It's the live_ test in forward_outcomes.)

  FW_LIVE_TEST_BIN=<k2-daemon test binary> K2_E2E_DAEMON=<k2-daemon> \\
    STALWART_PROBE=stalwart-forward-volume-e2e.py \\
    tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25

Not covered here, for the same reason: boot and the hourly pass calling the
reconcile (V3 runs the function they call; the_reconcile_writes_the_number_this_box_is_set_to
is in Rust).
"""

import argparse
import http.server
import importlib.util
import json
import os
import secrets
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("k2_forward_e2e", os.path.join(HERE, "stalwart-forward-daemon-e2e.py"))
e2e = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(e2e)
probe = e2e.probe

Client, Sink, check, info, wait_for, Daemon = probe.Client, probe.Sink, probe.check, probe.info, probe.wait_for, e2e.Daemon
CORE, MAIL, SIEVE, SUBMISSION = probe.CORE, probe.MAIL, probe.SIEVE, probe.SUBMISSION
HOST = e2e.HOST
MANY = 100
DAY, HOUR = "/cli/mail/config/forward-copies-per-day", "/cli/mail/config/forward-copies-per-hour"
LIVE_TEST = "mail::forward_outcomes::tests::live_a_raised_daily_limit_on_real_webhook_batches"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain", required=True)
    args = ap.parse_args()
    binary, test_bin = os.environ.get("K2_E2E_DAEMON", ""), os.environ.get("FW_LIVE_TEST_BIN", "")
    version = os.environ.get("STALWART_VERSION", "")
    if not os.access(binary, os.X_OK) or not os.access(test_bin, os.X_OK) or not version:
        sys.exit(f"set K2_E2E_DAEMON, FW_LIVE_TEST_BIN and STALWART_VERSION (got {binary!r}, {test_bin!r}, {version!r})")
    outside = version == "0.16.25"
    user, pw = os.environ["STALWART_PROBE_ADMIN"].split(":", 1)
    domain, tag = args.domain, secrets.token_hex(3)
    admin = Client(args.url, basic=(user, pw))
    n, a = admin.call("x:Domain/query", {"filter": {"name": domain}})
    did = a["ids"][0]
    sink = Sink()

    def account(name, admin_role=False):
        acct = {"@type": "User", "name": name, "domainId": did, "credentials": {"0": {"@type": "Password", "secret": secrets.token_urlsafe(24)}}}
        if admin_role:
            acct["roles"] = {"@type": "Admin"}
        else:
            acct["quotas"] = {"maxDiskQuota": 104857600, "maxEmails": 2000}
        n, a = admin.call("x:Account/set", {"create": {"k": acct}})
        cid = ((a.get("created") or {}).get("k") or {}).get("id")
        if not cid:
            sys.exit(f"account {name}: {n} {json.dumps(a)[:300]}")
        return cid

    svc_id = account(f"k2fv-svc-{tag}", admin_role=True)
    n, a = admin.call("x:ApiKey/set", {"accountId": svc_id, "create": {"k": {"description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {"127.0.0.1": True}}}})
    key = ((a.get("created") or {}).get("k") or {}).get("secret")
    if not key:
        sys.exit(f"service key: {n} {json.dumps(a)[:300]}")
    svc = Client(args.url, bearer=key)
    people = {name: account(f"k2fv-{name}-{tag}") for name in ("alice", "bob", "carol", "dave")}
    addr = {name: f"k2fv-{name}-{tag}@{domain}" for name in people}
    boxes = [(account(f"k2fv-m{i:03d}-{tag}"), f"k2fv-m{i:03d}-{tag}@{domain}") for i in range(MANY)]

    # outside.test → the sink.
    n, a = admin.call("x:MtaRoute/set", {"create": {"r": {"@type": "Relay", "name": f"k2fvsink{tag}", "address": "127.0.0.1", "port": sink.port, "protocol": "smtp", "implicitTls": False}}})
    if not ((a.get("created") or {}).get("r") or {}).get("id"):
        sys.exit(f"sink route: {n} {json.dumps(a)[:300]}")
    n, g = admin.call("x:MtaOutboundStrategy/get", {"ids": ["singleton"]})
    old = g["list"][0]["route"]
    shifted = {str(int(k) + 1): v for k, v in (old.get("match") or {}).items()}
    shifted["0"] = {"if": "rcpt_domain == 'outside.test'", "then": f"'k2fvsink{tag}'"}
    admin.call("x:MtaOutboundStrategy/set", {"update": {"singleton": {"route": {"match": shifted, "else": old["else"]}}}})
    # 0.16.10 refuses every reload while downloaded spam rules call `bit_and`
    # (K2's A4 repair rewrites them on a real box); drop them here.
    n, q = admin.call("x:SpamDnsblServer/query", {})
    if q.get("ids"):
        n, g = admin.call("x:SpamDnsblServer/get", {"ids": q["ids"]})
        bad = [x["id"] for x in (g.get("list") or []) if "bit_and" in json.dumps(x)]
        if bad:
            admin.call("x:SpamDnsblServer/set", {"destroy": bad})

    def reload_settings():
        admin.call("x:Action/set", {"create": {"r": {"@type": "ReloadSettings"}}})
        time.sleep(2)

    def throttles():
        n, q = admin.call("x:MtaOutboundThrottle/query", {})
        n, g = admin.call("x:MtaOutboundThrottle/get", {"ids": q.get("ids") or []})
        return [t for t in (g.get("list") or []) if (t.get("description") or "").startswith("K2 outside forwards:")]

    def per_mailbox():
        return next((t for t in throttles() if (t.get("key") or {}).get("sender")), {})

    # ── V0 the throttles the last release wrote ─────────────────────
    with open(os.path.join(HERE, "forward-throttles.json")) as f:
        for i, t in enumerate(json.load(f)):
            n, a = admin.call("x:MtaOutboundThrottle/set", {"create": {f"t{i}": t}})
            check(f"V0 '{t['description']}' is there", bool((a.get("created") or {}).get(f"t{i}")), f"{n} {json.dumps(a)[:200]}")
    reload_settings()
    stock = per_mailbox()
    stock_id = stock.get("id")
    box_id = next((t.get("id") for t in throttles() if (t.get("key") or {}).get("localIp")), None)
    check("V0 the per-mailbox throttle is the old 100", (stock.get("rate") or {}).get("count") == 100, json.dumps(stock)[:300])

    def ident_for(who):
        n, a = svc.call("Identity/get", {"accountId": people[who], "ids": None}, using=(CORE, MAIL, SUBMISSION))
        found = next((i["id"] for i in (a.get("list") or []) if i.get("email", "").lower() == addr[who].lower()), None)
        if found:
            return found
        n, a = svc.call("Identity/set", {"accountId": people[who], "create": {"i": {"email": addr[who]}}}, using=(CORE, MAIL, SUBMISSION))
        return ((a.get("created") or {}).get("i") or {}).get("id")

    bob_ident = ident_for("bob")

    def ham(mid, to):
        """bob → a mailbox, authenticated (the spam filter leaves it alone)."""
        n, q = svc.call("Mailbox/query", {"accountId": people["bob"], "filter": {"role": "drafts"}}, using=(CORE, MAIL))
        box = (q.get("ids") or [None])[0]
        if not box:
            n, q = svc.call("Mailbox/query", {"accountId": people["bob"], "filter": {"role": "inbox"}}, using=(CORE, MAIL))
            box = (q.get("ids") or [None])[0]
        email = {"mailboxIds": {box: True}, "from": [{"email": addr["bob"], "name": "Bob"}], "to": [{"email": to}], "subject": f"probe {mid}",
                 "bodyValues": {"t": {"value": f"Hello ({mid})."}}, "textBody": [{"partId": "t", "type": "text/plain"}]}
        r = svc.batch((CORE, MAIL, SUBMISSION), [
            ["Email/set", {"accountId": people["bob"], "create": {"e": email}}, "0"],
            ["EmailSubmission/set", {"accountId": people["bob"], "create": {"s": {"emailId": "#e", "identityId": bob_ident,
                "envelope": {"mailFrom": {"email": addr["bob"]}, "rcptTo": [{"email": to}]}}}, "onSuccessDestroyEmail": ["#s"]}, "1"],
        ])
        if not any(m[0] == "EmailSubmission/set" and (m[1].get("created") or {}).get("s") for m in r):
            info(f"ham {mid}", json.dumps(r)[:400])

    def kept(mid, account_id):
        n, a = svc.call("Email/query", {"accountId": account_id, "filter": {"subject": f"probe {mid}"}}, using=(CORE, MAIL))
        return len(a.get("ids") or []) if n == "Email/query" else -1

    def at_sink(mid):
        return len({r for m in sink.with_id(mid) for r in m["rcpt"]})

    def in_boxes(mid):
        return sum(1 for acc, _ in boxes if kept(mid, acc) >= 1)

    def settle(count, mid, want, secs=90):
        """Wait for `want`, then long enough that more would have shown."""
        wait_for(lambda: count(mid) >= want, secs)
        time.sleep(12)
        return count(mid)

    def held_for(sender):
        n, q = admin.call("x:QueuedMessage/query", {})
        ids = q.get("ids") or []
        if not ids:
            return 0
        n, g = admin.call("x:QueuedMessage/get", {"ids": ids})
        out = 0
        for m in g.get("list") or []:
            if (m.get("returnPath") or "").lower() == sender.lower():
                out += sum(1 for r in (m.get("recipients") or {}).values() if ((r.get("status") or {}).get("errorType")) == "rateLimited")
        return out

    def clear_the_hour():
        """Stalwart's window turns over on the clock hour: don't straddle it."""
        left = 3600 - int(time.time()) % 3600
        if left < 420:
            info("waiting out the clock hour", f"{left}s")
            time.sleep(left + 2)

    d = Daemon(binary, key)
    try:
        now = int(time.time())
        with sqlite3.connect(d.db, timeout=30) as c:
            c.execute(
                "INSERT OR REPLACE INTO mail_server (id, status, pinned_version, installed_version, hostname, api_url, api_key_ref, updated_at) "
                "VALUES (1, 'running', ?, ?, ?, ?, 'env:K2FE2E_STALWART_KEY', ?)",
                (version, version, HOST, args.url, now),
            )
            c.execute("INSERT INTO mail_domains (id, domain, stalwart_domain_id, send_mode, status, created_at) VALUES ('d1', ?, ?, 'direct', 'verified', ?)", (domain, did, now))
            rows = [(f"ma-{name}", addr[name], people[name]) for name in people] + [(f"ma-m{i:03d}", a2, acc) for i, (acc, a2) in enumerate(boxes)]
            for rid, a2, acct in rows:
                c.execute(
                    "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) VALUES (?, ?, 'd1', ?, 'e2e', 'active', ?)",
                    (rid, a2, acct, now),
                )
        info("fixture", f"Stalwart {version}; daemon on :{d.port}")

        def forwarding():
            st, v = d.call("GET", "/cli/mail/config")
            return v.get("forwarding") or {} if isinstance(v, dict) else {}

        def age_notices():
            # The burst is per hour; the minute pass that drains it isn't running here.
            with sqlite3.connect(d.db, timeout=30) as c:
                c.execute("UPDATE mail_forward_events SET at = at - 7200 WHERE event IN ('notice_sent', 'notice_failed')")

        # ── V1 the defaults, the bounds ─────────────────────────────
        fw = forwarding()
        check("V1 config shows the defaults", (fw.get("copiesPerDay"), fw.get("copiesPerHour")) == (500, 100), json.dumps(fw))
        check("V1 and the most each can be", (fw.get("copiesPerDayMax"), fw.get("copiesPerHourMax")) == (24000, 1000), json.dumps(fw))
        for path, body, why in ((DAY, {"perDay": 0}, "0"), (DAY, {"perDay": 24001}, "24001"), (HOUR, {"perHour": 0}, "0"), (HOUR, {"perHour": 1001}, "1001")):
            st, v = d.call("POST", path, body)
            check(f"V1 {path.rsplit('-', 1)[1]} {why} is refused", st == 400 and "Nothing changed" in v["error"]["hint"], f"{st} {json.dumps(v)[:200]}")
        for path, body in ((DAY, {"perDay": 900}), (HOUR, {"perHour": 900})):
            st, v = d.call("GET", path)
            check(f"V1 GET on {path.rsplit('-', 1)[1]} is 405", st == 405, f"{st}")
            st, v = d.call("POST", path, body, token=False)
            check(f"V1 no credential on {path.rsplit('-', 1)[1]} is refused", st in (401, 403), f"{st}")
        fw = forwarding()
        check("V1 nothing changed", (fw.get("copiesPerDay"), fw.get("copiesPerHour")) == (500, 100), json.dumps(fw))
        check("V1 Stalwart still has the old throttle", (per_mailbox().get("rate") or {}).get("count") == 100, "")

        # ── V2 a raised hourly number on that box ───────────────────
        st, v = d.call("POST", HOUR, {"perHour": 120})
        check("V2 K2 takes 120 an hour and says it's enforced", st == 200 and v.get("perHour") == 120 and v.get("applied") is True and v.get("note") is None, f"{st} {json.dumps(v)[:400]}")
        now_t = per_mailbox()
        check("V2 Stalwart reads back 120", (now_t.get("rate") or {}).get("count") == 120, json.dumps(now_t)[:300])
        check("V2 the same throttle, updated in place", now_t.get("id") == stock_id and (now_t.get("key") or {}).get("sender") is True, f"{stock_id} -> {now_t.get('id')}")
        check("V2 its words say so", now_t.get("description") == "K2 outside forwards: per mailbox, 120 an hour", now_t.get("description"))
        ours = throttles()
        check("V2 K2 still has exactly two", len(ours) == 2, json.dumps([t.get("description") for t in ours]))
        whole = next((t for t in ours if t.get("id") == box_id), {})
        check("V2 the whole-box one is untouched", (whole.get("rate") or {}).get("count") == 1000, json.dumps(whole)[:300])
        check("V2 config shows it", forwarding().get("copiesPerHour") == 120, json.dumps(forwarding()))

        # ── V3 drift: K2's reconcile puts it right ──────────────────
        with open(os.path.join(HERE, "forward-throttles.json")) as f:
            back = json.load(f)[0]
        n, a = admin.call("x:MtaOutboundThrottle/set", {"update": {stock_id: back}})
        check("V3 someone puts the old number back", not a.get("notUpdated"), f"{n} {json.dumps(a)[:200]}")
        reload_settings()
        check("V3 Stalwart has 100 again", (per_mailbox().get("rate") or {}).get("count") == 100, "")
        st, v = d.call("POST", HOUR, {"perHour": 120})
        now_t = per_mailbox()
        check("V3 K2's reconcile restores 120, in place", st == 200 and v.get("applied") is True and (now_t.get("rate") or {}).get("count") == 120 and now_t.get("id") == stock_id, f"{st} {json.dumps(now_t)[:300]}")

        st, v = d.call("POST", "/cli/mail/config/forward-new-per-day", {"perDay": 1000})
        check("setup: the day's new addresses", st == 200, f"{st}")

        # ── V4 it holds ─────────────────────────────────────────────
        clear_the_hour()
        same_box = [a2 for _, a2 in boxes]
        st, v = d.call("POST", "/cli/mail/forward", {"address": addr["dave"], "to": same_box}, timeout=300)
        check("V4 same-box: 100 addresses", st == 200, f"{st} {json.dumps(v)[:300]}")
        first, second = f"v4a-{tag}", f"v4b-{tag}"
        ham(first, addr["dave"])
        got = settle(in_boxes, first, MANY)
        check("V4 same-box: a first message's 100 all go", got == MANY, f"{got}")
        ham(second, addr["dave"])
        if outside:
            got = settle(in_boxes, second, 20, 60)
            check("V4 same-box at 120: a second message's next 20 go, no more", got == 20, f"{got} of {MANY}")
            held = held_for(addr["dave"])
            check("V4 same-box: the other 80 wait, none dropped", held == 80, f"{held} held rateLimited")
        else:
            got = settle(in_boxes, second, MANY, 60)
            info("V4 Stalwart 0.16.10: same-box copies under the throttle", f"{got} of {MANY} delivered; {held_for(addr['dave'])} held (it doesn't hold same-box copies)")
            st, v = d.call("POST", "/cli/mail/forward/add", {"address": addr["carol"], "to": "me@outside.test"})
            check("V4 Stalwart 0.16.10: outside addresses are refused, so nothing else here applies", st == 409 and v["error"]["code"] == "upgrade_required", f"{st} {json.dumps(v)[:200]}")
            st, v = d.call("POST", DAY, {"perDay": 550})
            check("V4 Stalwart 0.16.10: the daily number saves too", st == 200 and v.get("perDay") == 550 and forwarding().get("copiesPerDay") == 550, f"{st} {json.dumps(v)[:200]}")
            print(f"== forward volume e2e, Stalwart {version}: {len(probe.FAILED)} failed check(s): {probe.FAILED} ==", flush=True)
            sys.exit(1 if probe.FAILED else 0)

        carol_dests = [f"c{i:03d}@outside.test" for i in range(MANY)]
        for part in (carol_dests[:60], carol_dests[60:]):
            age_notices()
            st, v = d.call("POST", "/cli/mail/forward/add", {"address": addr["carol"], "to": part}, timeout=300)
            check("V4 outside: addresses go on", st == 200 and all(x.get("state") == "active" for x in v.get("destinations") or []), f"{st} {json.dumps(v)[:300]}")
        first, second, third = f"v4c-{tag}", f"v4d-{tag}", f"v4e-{tag}"
        ham(first, addr["carol"])
        got = settle(at_sink, first, MANY)
        check("V4 outside: a first message's 100 all go", got == MANY, f"{got}")
        ham(second, addr["carol"])
        got = settle(at_sink, second, 20, 60)
        check("V4 outside at 120: a second message's next 20 go, no more", got == 20, f"{got} of {MANY}")
        held = held_for(addr["carol"])
        check("V4 outside: the other 80 wait, none dropped", held == 80, f"{held} held rateLimited")
        st, v = d.call("POST", HOUR, {"perHour": 220})
        check("V4 raised again while they wait", st == 200 and v.get("applied") is True and (per_mailbox().get("rate") or {}).get("count") == 220, f"{st} {json.dumps(v)[:200]}")
        ham(third, addr["carol"])
        got = settle(at_sink, third, 80, 60)
        check("V4 outside: the new number holds with no restart", got >= 80, f"{got} of the next message's {MANY}; {held_for(addr['carol'])} held in all")

        # ── V5 a raised daily limit, K2's real pass ─────────────────
        spool = tempfile.mkdtemp(prefix="k2fvspool.")
        hook_key = secrets.token_urlsafe(32)
        count = {"n": 0}
        lock = threading.Lock()

        class Spool(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                with lock:
                    count["n"] += 1
                    name = os.path.join(spool, f"{count['n']:05d}")
                    with open(name + ".body", "wb") as f:
                        f.write(body)
                    with open(name + ".sig", "w") as f:
                        f.write(self.headers.get("X-Signature") or "")
                self.send_response(200)
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"{}")

            def log_message(self, *a):
                pass

        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Spool)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        # K2's webhook shape, as K2 writes it (crates/k2-daemon/src/email_notify/webhook.rs).
        hook = {"url": f"http://127.0.0.1:{srv.server_address[1]}/cli/email/events", "enable": True, "eventsPolicy": "include",
                "events": {e: True for e in probe.EVENTS}, "signatureKey": {"@type": "Value", "secret": hook_key},
                "httpHeaders": {"X-K2-Key-Id": "probe"}, "throttle": 100, "timeout": 30000, "lossy": False, "allowInvalidCerts": False}
        n, a = admin.call("x:WebHook/set", {"create": {"w": hook}})
        wid = ((a.get("created") or {}).get("w") or {}).get("id")
        check("V5 setup: K2's webhook shape", bool(wid), f"{n} {json.dumps(a)[:300]}")
        reload_settings()

        st, v = d.call("POST", HOUR, {"perHour": 900})
        check("V5 900 an hour", st == 200 and v.get("applied") is True and (per_mailbox().get("rate") or {}).get("count") == 900, f"{st} {json.dumps(v)[:200]}")
        st, v = d.call("POST", DAY, {"perDay": 550})
        check("V5 550 a day, and K2 says nothing to worry about", st == 200 and v.get("perDay") == 550 and v.get("note") is None, f"{st} {json.dumps(v)[:300]}")
        check("V5 K2's daily number never went to Stalwart", (per_mailbox().get("rate") or {}).get("count") == 900, "")
        fw = forwarding()
        check("V5 config shows both", (fw.get("copiesPerDay"), fw.get("copiesPerHour")) == (550, 900), json.dumps(fw))
        dests = [f"a{i:03d}@outside.test" for i in range(MANY)]
        for part in (dests[:60], dests[60:]):
            age_notices()
            st, v = d.call("POST", "/cli/mail/forward/add", {"address": addr["alice"], "to": part}, timeout=300)
            check("V5 addresses go on", st == 200 and all(x.get("state") == "active" for x in v.get("destinations") or []), f"{st} {json.dumps(v)[:300]}")
        st, v = d.call("GET", "/cli/mail/forward", params={"address": addr["alice"]})
        check("V5 show gives the numbers in force", (v.get("volume") or {}).get("copiesPerDay") == 550 and (v.get("volume") or {}).get("copiesPerHour") == 900, json.dumps(v.get("volume")))
        time.sleep(3)
        with lock:
            before = count["n"]
        info("V5 webhook batches before the messages (the notices)", f"{before}")
        total = 0
        for i in range(6):
            mid = f"v5-{i}-{tag}"
            ham(mid, addr["alice"])
            got = settle(at_sink, mid, MANY, 120)
            total += got
            check(f"V5 message {i + 1} reaches all {MANY}", got == MANY, f"{got}; {total} copies so far; {held_for(addr['alice'])} held")
        check("V5 600 copies arrived: past the old 100 an hour and the old 500 a day", total == 600, f"{total}")
        time.sleep(5)
        admin.call("x:WebHook/set", {"destroy": [wid]})
    finally:
        d.stop()

    # K2's real route and pass on what Stalwart really signed, writing to the real Stalwart.
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": tempfile.mkdtemp(prefix="k2fvhome."),
        "FW_LIVE_URL": args.url, "FW_LIVE_KEY": key, "FW_LIVE_HOST": HOST, "FW_LIVE_CAP": "550",
        "FW_LIVE_MAILBOX": addr["alice"], "FW_LIVE_ACCOUNT": people["alice"], "FW_LIVE_DESTS": ",".join(dests),
        "FW_LIVE_SPOOL": spool, "FW_LIVE_HOOK_KEY": hook_key,
    }
    r = subprocess.run([test_bin, "--ignored", "--exact", LIVE_TEST, "--nocapture", "--test-threads=1"], env=env, capture_output=True, text=True, timeout=600, cwd=HERE)
    lines = [l for l in r.stdout.splitlines() if l.startswith("FW_LIVE") or l.startswith("test ") or "panicked" in l]
    for l in lines:
        print(f"[INFO] V5 k2: {l[:900]}", flush=True)
    check("V5 K2's pass, on the batches Stalwart signed: nothing at 500, all 100 pause at 550, the real script loses its forward, the notice names the setting",
          r.returncode == 0 and "test result: ok. 1 passed" in r.stdout, f"exit {r.returncode}; {r.stdout[-1500:] if r.returncode else ''}{r.stderr[-1500:] if r.returncode else ''}")
    after = f"v5-after-{tag}"
    ham(after, addr["alice"])
    check("V5 the next message stays in the mailbox", bool(wait_for(lambda: kept(after, people["alice"]) >= 1, 30)), f"kept {kept(after, people['alice'])}")
    time.sleep(8)
    check("V5 and goes nowhere", not sink.with_id(after), f"{len(sink.with_id(after))} at the sink")
    print(f"== forward volume e2e, Stalwart {version}: {len(probe.FAILED)} failed check(s): {probe.FAILED} ==", flush=True)
    sys.exit(1 if probe.FAILED else 0)


if __name__ == "__main__":
    main()
