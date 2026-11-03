#!/usr/bin/env python3
"""End to end: a mailbox at K2's forward limit, through a real k2-daemon and a
real (throwaway) Stalwart, on both versions in the fleet. No real mail.

What only K2's own code can show: the script it writes, the limit it raises on
a box that updates, and what actually arrives. The fixture is
tests/mail/stalwart-forward-daemon-e2e.py's: a temp HOME, an empty environment,
never the real ~/.k2, the keychain or a running daemon.

  F1  a box that updates: a mailbox set up with 5 addresses, Stalwart back on
      the old limits (5, 6). Those 5 keep getting mail. Adding 95 more through
      K2's route raises Stalwart's limit, and all 100 same-box mailboxes get
      the next message, once each
  F2  the 101st is refused, by add and by set, and nothing changes
  F3  on Stalwart older than 0.16.25, outside addresses are refused: the rest
      is outside forwards, so it runs on 0.16.25
  F4  100 new outside addresses past the day's limit (20): refused whole, says
      how to raise it, nobody mailed
  F5  from Stalwart's default limits, through K2's routes: 100 outside addresses
      of 254 bytes each, one notice each. A message reaches all 100, each its
      own message with the mailbox as MAIL FROM and the marker, and a copy stays
  F6  out-of-office at its longest on top of that: K2 writes it, the script has
      100 redirects, all 100 still get the next message
  F8  mail that arrives during a pause stays home, even after the resume; the
      next message goes to all 100
  F7  100 at once on one mailbox: the first 60 go on and the rest wait their
      turn, and how long K2 takes to answer

  K2_E2E_DAEMON=<k2-daemon> STALWART_PROBE=stalwart-forward-many-e2e.py \\
    tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25

Not covered here, because the daemon's background loops only run with K2's
systemd unit up: the hourly reconcile (its side of raising the limit on an old
box is the Rust test a_mailbox_from_the_old_limit_keeps_working_and_can_grow)
and the minute pass that turns on what waits (a_mailbox_forwards_to_100_...).
"""

import argparse
import importlib.util
import json
import os
import secrets
import sqlite3
import sys
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
LONG_DOMAIN = f"{'b' * 60}.{'c' * 60}.{'d' * 54}.outside.test"


def long_address(i):
    return f"d{i:03d}{'a' * 60}@{LONG_DOMAIN}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain", required=True)
    args = ap.parse_args()
    binary = os.environ.get("K2_E2E_DAEMON", "")
    if not os.access(binary, os.X_OK):
        sys.exit(f"set K2_E2E_DAEMON to a built k2-daemon (got {binary!r})")
    version = os.environ.get("STALWART_VERSION", "")
    if not version:
        sys.exit("STALWART_VERSION is unset (tests/mail/stalwart-sandbox-forward-gap.sh sets it)")
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
            acct["quotas"] = {"maxDiskQuota": 104857600, "maxEmails": 1000}
        n, a = admin.call("x:Account/set", {"create": {"k": acct}})
        cid = ((a.get("created") or {}).get("k") or {}).get("id")
        if not cid:
            sys.exit(f"account {name}: {n} {json.dumps(a)[:300]}")
        return cid

    svc_id = account(f"k2fn-svc-{tag}", admin_role=True)
    n, a = admin.call("x:ApiKey/set", {"accountId": svc_id, "create": {"k": {"description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {"127.0.0.1": True}}}})
    key = ((a.get("created") or {}).get("k") or {}).get("secret")
    if not key:
        sys.exit(f"service key: {n} {json.dumps(a)[:300]}")
    svc = Client(args.url, bearer=key)
    alice, bob, carol = account(f"k2fn-alice-{tag}"), account(f"k2fn-bob-{tag}"), account(f"k2fn-carol-{tag}")
    alice_addr, bob_addr, carol_addr = f"k2fn-alice-{tag}@{domain}", f"k2fn-bob-{tag}@{domain}", f"k2fn-carol-{tag}@{domain}"
    boxes = [(account(f"k2fn-m{i:03d}-{tag}"), f"k2fn-m{i:03d}-{tag}@{domain}") for i in range(MANY + 1)]

    # outside.test and the long domain → the sink.
    n, a = admin.call("x:MtaRoute/set", {"create": {"r": {"@type": "Relay", "name": f"k2fnsink{tag}", "address": "127.0.0.1", "port": sink.port, "protocol": "smtp", "implicitTls": False}}})
    if not ((a.get("created") or {}).get("r") or {}).get("id"):
        sys.exit(f"sink route: {n} {json.dumps(a)[:300]}")
    n, g = admin.call("x:MtaOutboundStrategy/get", {"ids": ["singleton"]})
    old = g["list"][0]["route"]
    shifted = {str(int(k) + 2): v for k, v in (old.get("match") or {}).items()}
    shifted["0"] = {"if": "rcpt_domain == 'outside.test'", "then": f"'k2fnsink{tag}'"}
    shifted["1"] = {"if": f"rcpt_domain == '{LONG_DOMAIN}'", "then": f"'k2fnsink{tag}'"}
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

    reload_settings()

    def limits():
        n, g = admin.call("x:SieveUserInterpreter/get", {"ids": ["singleton"], "properties": ["maxRedirects", "maxOutMessages"]})
        row = (g.get("list") or [{}])[0]
        return row.get("maxRedirects"), row.get("maxOutMessages")

    def set_limits(r, o):
        n, a = admin.call("x:SieveUserInterpreter/set", {"update": {"singleton": {"maxRedirects": r, "maxOutMessages": o}}})
        check(f"Stalwart back on ({r}, {o})", n == "x:SieveUserInterpreter/set" and not a.get("notUpdated"), f"{n} {json.dumps(a)[:200]}")
        reload_settings()

    def ham(mid):
        """bob → alice, authenticated (the spam filter leaves it alone)."""
        n, a = svc.call("Identity/get", {"accountId": bob, "ids": None}, using=(CORE, MAIL, SUBMISSION))
        ident = next((i["id"] for i in (a.get("list") or []) if i.get("email", "").lower() == bob_addr.lower()), None)
        if not ident:
            n, a = svc.call("Identity/set", {"accountId": bob, "create": {"i": {"email": bob_addr}}}, using=(CORE, MAIL, SUBMISSION))
            ident = ((a.get("created") or {}).get("i") or {}).get("id")
        n, q = svc.call("Mailbox/query", {"accountId": bob, "filter": {"role": "drafts"}}, using=(CORE, MAIL))
        box = (q.get("ids") or [None])[0]
        if not box:
            n, q = svc.call("Mailbox/query", {"accountId": bob, "filter": {"role": "inbox"}}, using=(CORE, MAIL))
            box = (q.get("ids") or [None])[0]
        email = {"mailboxIds": {box: True}, "from": [{"email": bob_addr, "name": "Bob"}], "to": [{"email": alice_addr}], "subject": f"probe {mid}",
                 "bodyValues": {"t": {"value": f"Hello ({mid})."}}, "textBody": [{"partId": "t", "type": "text/plain"}]}
        svc.batch((CORE, MAIL, SUBMISSION), [
            ["Email/set", {"accountId": bob, "create": {"e": email}}, "0"],
            ["EmailSubmission/set", {"accountId": bob, "create": {"s": {"emailId": "#e", "identityId": ident,
                "envelope": {"mailFrom": {"email": bob_addr}, "rcptTo": [{"email": alice_addr}]}}}, "onSuccessDestroyEmail": ["#s"]}, "1"],
        ])

    def kept(mid, account):
        n, a = svc.call("Email/query", {"accountId": account, "filter": {"subject": f"probe {mid}"}}, using=(CORE, MAIL))
        return len(a.get("ids") or []) if n == "Email/query" else -1

    def boxes_with(mid, want, secs=120):
        wait_for(lambda: sum(1 for acc, _ in boxes if kept(mid, acc) >= 1) >= want, secs)
        time.sleep(5)
        return [kept(mid, acc) for acc, _ in boxes]

    def rcpts_of(mid, want, secs=180):
        wait_for(lambda: len(sink.with_id(mid)) >= want, secs)
        time.sleep(5)
        return sorted(r for m in sink.with_id(mid) for r in m["rcpt"])

    def script_of(account_id):
        """The script K2 wrote, as Stalwart holds it."""
        n, g = svc.call("SieveScript/get", {"accountId": account_id, "ids": None}, using=(CORE, SIEVE))
        row = next((x for x in (g.get("list") or []) if x.get("name") == "k2"), None)
        if not row:
            return ""
        url = svc.path("downloadUrl").replace("{accountId}", account_id).replace("{blobId}", row["blobId"]).replace("{name}", "k2").replace("{type}", "application%2Foctet-stream")
        st, raw = svc._req("GET", url)
        return raw.decode(errors="replace") if isinstance(raw, bytes) else json.dumps(raw)

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
            rows = [("ma-alice", alice_addr, alice), ("ma-bob", bob_addr, bob), ("ma-carol", carol_addr, carol)]
            rows += [(f"ma-m{i:03d}", addr, acc) for i, (acc, addr) in enumerate(boxes)]
            for rid, addr, acct in rows:
                c.execute(
                    "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) VALUES (?, ?, 'd1', ?, 'e2e', 'active', ?)",
                    (rid, addr, acct, now),
                )
        info("fixture", f"Stalwart {version}; daemon on :{d.port}; limits as shipped {limits()}")

        def timed(method, path, body):
            t0 = time.time()
            st, v = d.call(method, path, body, timeout=600)
            return st, v, time.time() - t0

        same_box = [addr for _, addr in boxes[:MANY]]

        # ── F1 a box that updates ───────────────────────────────────
        st, v, took = timed("POST", "/cli/mail/forward", {"address": alice_addr, "to": same_box[:5], "keep": True})
        check("F1 5 same-box addresses go on", st == 200, f"{st} in {took:.1f}s {json.dumps(v)[:300]}")
        set_limits(5, 6)
        mid = f"f1a-{tag}"
        ham(mid)
        got = boxes_with(mid, 5)
        check("F1 on the old limit those 5 keep getting mail", sum(1 for c in got if c >= 1) == 5 and all(c == 1 for c in got[:5]), f"{sum(1 for c in got if c >= 1)} mailboxes")
        check("F1 and a copy stays", kept(mid, alice) == 1, f"kept {kept(mid, alice)}")
        st, v, took = timed("POST", "/cli/mail/forward/add", {"address": alice_addr, "to": same_box[5:]})
        check("F1 95 more go on", st == 200, f"{st} {json.dumps(v)[:300]}")
        info("F1 how long 95 same-box addresses took", f"{took:.1f}s")
        check(f"F1 K2 raised Stalwart's limit to ({MANY}, {MANY + 1})", limits() == (MANY, MANY + 1), json.dumps(limits()))
        src = script_of(alice)
        check(f"F1 the script has {MANY} redirects", src.count("redirect ") == MANY, f"{src.count('redirect ')}; {len(src.encode())} bytes")
        mid = f"f1b-{tag}"
        t0 = time.time()
        ham(mid)
        got = boxes_with(mid, MANY)
        check(f"F1 all {MANY} mailboxes get it", sum(1 for c in got[:MANY] if c >= 1) == MANY, f"{sum(1 for c in got[:MANY] if c >= 1)} in {time.time() - t0:.0f}s")
        check("F1 each exactly once", all(c == 1 for c in got[:MANY]), json.dumps(sorted(set(got[:MANY]))))
        check("F1 and a copy stays", kept(mid, alice) == 1, f"kept {kept(mid, alice)}")

        # ── F2 the 101st ────────────────────────────────────────────
        extra = boxes[MANY][1]
        st, v, _ = timed("POST", "/cli/mail/forward/add", {"address": alice_addr, "to": extra})
        hint = (v.get("error") or {}).get("hint", "") if isinstance(v, dict) else ""
        check("F2 add: the 101st is refused", st == 409 and v["error"]["code"] == "too_many_destinations", f"{st} {json.dumps(v)[:300]}")
        check("F2 add: it says the limit and what to do", "at most 100 addresses" in hint and "would make 101" in hint and "forward rm" in hint, hint)
        st, v, _ = timed("POST", "/cli/mail/forward", {"address": alice_addr, "to": same_box + [extra]})
        hint = (v.get("error") or {}).get("hint", "") if isinstance(v, dict) else ""
        check("F2 set: 101 are refused", st == 409 and v["error"]["code"] == "too_many_destinations", f"{st} {json.dumps(v)[:300]}")
        check("F2 set: it says the limit", "at most 100 addresses" in hint and "this list has 101" in hint, hint)
        st, v = d.call("GET", "/cli/mail/forward", params={"address": alice_addr})
        check("F2 nothing changed", st == 200 and len(v.get("forwards") or []) == MANY and v.get("maxDestinations") == MANY, f"{st} {len(v.get('forwards') or [])} {v.get('maxDestinations')}")
        st, v, _ = timed("POST", "/cli/mail/forward/unset", {"address": alice_addr})
        check("F2 unset removes all 100", st == 200 and v.get("count") == MANY, f"{st} {json.dumps(v)[:200]}")

        # ── F3 outside needs 0.16.25 ────────────────────────────────
        if version != "0.16.25":
            st, v, _ = timed("POST", "/cli/mail/forward", {"address": alice_addr, "to": "me@outside.test"})
            check(f"F3 Stalwart {version}: outside addresses are refused", st == 409 and v["error"]["code"] == "upgrade_required", f"{st} {json.dumps(v)[:300]}")
            print(f"== forward many e2e, Stalwart {version}: {len(probe.FAILED)} failed check(s): {probe.FAILED} ==", flush=True)
            sys.exit(1 if probe.FAILED else 0)

        # ── F4 past the day's limit ─────────────────────────────────
        dests = [long_address(i) for i in range(MANY)]
        check("F4 the addresses are 254 bytes", all(len(x) == 254 for x in dests), f"{len(dests[0])}")
        st, v, _ = timed("POST", "/cli/mail/forward", {"address": alice_addr, "to": dests})
        hint = (v.get("error") or {}).get("hint", "") if isinstance(v, dict) else ""
        check("F4 100 new outside addresses are refused by the day's limit", st == 429 and v["error"]["code"] == "new_outside_limit", f"{st} {json.dumps(v)[:300]}")
        check("F4 it says how to raise it", "k2 hostmail config --forward-new-per-day" in hint and "this would add 100" in hint and "at most 20" in hint, hint)
        time.sleep(3)
        check("F4 nobody was mailed", not sink.got, f"{len(sink.got)} at the sink")
        st, v = d.call("POST", "/cli/mail/config/forward-new-per-day", {"perDay": 1000})
        check("F4 the owner raises it", st == 200 and v.get("perDay") == 1000, f"{st} {json.dumps(v)[:200]}")

        # ── F5 100 long outside addresses, from Stalwart's defaults ─
        set_limits(1, 3)

        def age_notices():
            # The burst is per hour; the minute pass that drains it isn't running here.
            with sqlite3.connect(d.db, timeout=30) as c:
                c.execute("UPDATE mail_forward_events SET at = at - 7200 WHERE event IN ('notice_sent', 'notice_failed')")

        st, v, took = timed("POST", "/cli/mail/forward", {"address": alice_addr, "to": dests[:60]})
        check("F5 60 go on, and K2 answers", st == 200, f"{st} {json.dumps(v)[:300]}")
        info("F5 how long 60 new outside addresses took (one notice each)", f"{took:.1f}s, {took / 60:.2f}s each")
        states = [x.get("state") for x in (v.get("destinations") or [])]
        check("F5 all 60 are on", states == ["active"] * 60, json.dumps(sorted(set(states))))
        age_notices()
        st, v, took = timed("POST", "/cli/mail/forward/add", {"address": alice_addr, "to": dests[60:]})
        check("F5 40 more go on", st == 200, f"{st} {json.dumps(v)[:300]}")
        info("F5 how long 40 more took", f"{took:.1f}s")
        check(f"F5 K2 raised Stalwart's limit to ({MANY}, {MANY + 1})", limits() == (MANY, MANY + 1), json.dumps(limits()))
        st, v = d.call("GET", "/cli/mail/forward", params={"address": alice_addr})
        rows = v.get("forwards") or []
        check("F5 show has 100, all on, each told", len(rows) == MANY and all(r.get("state") == "active" and r.get("notice") == "sent" for r in rows), f"{len(rows)}; {json.dumps(sorted({(r.get('state'), r.get('notice')) for r in rows}))}")
        notices = wait_for(lambda: len([m for m in sink.got if "k2es_" in m["data"]]) >= MANY and [m for m in sink.got if "k2es_" in m["data"]], 120) or []
        check("F5 each address got its notice", sorted(r for m in notices for r in m["rcpt"]) == sorted(dests), f"{len(notices)} notices")
        src = script_of(alice)
        check(f"F5 the script has {MANY} redirects", src.count("redirect ") == MANY, f"{src.count('redirect ')}; {len(src.encode())} bytes")
        mid = f"f5-{tag}"
        t0 = time.time()
        ham(mid)
        got = rcpts_of(mid, MANY)
        check(f"F5 a message reaches all {MANY}", got == sorted(dests), f"{len(got)} at the sink, {len(set(got))} different, in {time.time() - t0:.0f}s")
        copies = sink.with_id(mid)
        check("F5 each is its own message with one recipient", len(copies) == MANY and all(len(m["rcpt"]) == 1 for m in copies), f"{len(copies)} messages")
        check("F5 each has the mailbox as MAIL FROM and the marker once", bool(copies) and all((m["from"] or "").lower() == alice_addr.lower() and m["data"].count("X-K2-Forwarded:") == 1 for m in copies), "")
        check("F5 a copy stays", kept(mid, alice) == 1, f"kept {kept(mid, alice)}")

        # ── F6 out-of-office at its longest on top ──────────────────
        text = ("x" * 75 + "\n") * 53
        text = text[:4096].rstrip("\n")
        st, v, _ = timed("POST", "/cli/mail/ooo", {"address": alice_addr, "text": text})
        check("F6 out-of-office goes on", st == 200, f"{st} {json.dumps(v)[:300]}")
        src = script_of(alice)
        check(f"F6 the script still has {MANY} redirects, and the reply", src.count("redirect ") == MANY and "vacation" in src, f"{src.count('redirect ')} redirects")
        info("F6 the biggest script K2 wrote", f"{len(src.encode())} bytes")
        mid = f"f6-{tag}"
        ham(mid)
        got = rcpts_of(mid, MANY)
        check(f"F6 all {MANY} still get it", got == sorted(dests), f"{len(got)} at the sink")
        n, q = svc.call("Email/query", {"accountId": bob, "filter": {"from": alice_addr}}, using=(CORE, MAIL))
        check("F6 and the reply goes too", len(q.get("ids") or []) >= 1, f"bob has {len(q.get('ids') or [])} from alice")
        # K2's cap on the text is 4096 bytes; Stalwart's string limit bites just under
        # it. A refused script must leave the forwards running.
        st, v, _ = timed("POST", "/cli/mail/ooo", {"address": alice_addr, "text": "x" * 4096})
        info("F6 out-of-office text at K2's cap of 4096 bytes", f"{st} {json.dumps(v)[:300]}")
        src = script_of(alice)
        check(f"F6 whatever that answered, the script still has {MANY} redirects", src.count("redirect ") == MANY, f"{src.count('redirect ')}")
        mid = f"f6b-{tag}"
        ham(mid)
        got = rcpts_of(mid, MANY)
        check(f"F6 and all {MANY} still get mail", got == sorted(dests), f"{len(got)} at the sink")
        st, v, _ = timed("POST", "/cli/mail/ooo/unset", {"address": alice_addr})
        check("F6 unset", st == 200, f"{st}")

        # ── F8 a pause, then a resume ───────────────────────────────
        st, v, _ = timed("POST", "/cli/mail/forward/pause", {"address": alice_addr})
        check("F8 pause all 100", st == 200 and len(v.get("forwards") or []) == MANY, f"{st} {len(v.get('forwards') or []) if isinstance(v, dict) else v}")
        during = f"f8a-{tag}"
        ham(during)
        check("F8 mail during the pause stays in the mailbox", bool(wait_for(lambda: kept(during, alice) >= 1, 30)), f"kept {kept(during, alice)}")
        st, v, _ = timed("POST", "/cli/mail/forward/resume", {"address": alice_addr})
        check("F8 resume all 100", st == 200, f"{st} {json.dumps(v)[:300]}")
        after = f"f8b-{tag}"
        ham(after)
        got = rcpts_of(after, MANY)
        check(f"F8 the next message reaches all {MANY}", got == sorted(dests), f"{len(got)} at the sink")
        check("F8 the mail from the pause was never sent", not sink.with_id(during), f"{len(sink.with_id(during))} at the sink")

        # ── F7 100 at once ──────────────────────────────────────────
        age_notices()
        at_once = [f"c{i:03d}@outside.test" for i in range(MANY)]
        st, v, took = timed("POST", "/cli/mail/forward", {"address": carol_addr, "to": at_once})
        check("F7 100 at once answers", st == 200, f"{st} {json.dumps(v)[:300]}")
        info("F7 how long 100 at once took (60 notices)", f"{took:.1f}s")
        states = [x.get("state") for x in (v.get("destinations") or [])]
        check("F7 60 on, 40 waiting their turn", states.count("active") == 60 and states.count("pending_verify") == 40, f"{states.count('active')} active, {states.count('pending_verify')} waiting")
        src = script_of(carol)
        check("F7 the script has only those that are on", src.count("redirect ") == 60, f"{src.count('redirect ')}")
    finally:
        d.stop()
    print(f"== forward many e2e, Stalwart {version}: {len(probe.FAILED)} failed check(s): {probe.FAILED} ==", flush=True)
    sys.exit(1 if probe.FAILED else 0)


if __name__ == "__main__":
    main()
