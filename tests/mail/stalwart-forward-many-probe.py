#!/usr/bin/env python3
"""Live probe: a mailbox at K2's forward limit on a real (throwaway) Stalwart.

K2 allows MANY (100) destinations per mailbox and writes one `redirect` each.
A `redirect` over Stalwart's limit is silently skipped, so "the script saved"
proves nothing: this counts what actually arrives, one copy per destination.
Runs on both Stalwart versions in the fleet:

  M0  the interpreter's limits as shipped (every limit, printed)
  M1  an old box, on the limits K2 set before (5 redirects, 6 out messages):
      a script with MANY redirects saves, and only 5 go. This is the silent
      loss the limit exists to stop
  M2  K2's limits (MANY redirects, MANY + 1 out messages) are accepted, reload,
      and read back. A refusal here is a hard ceiling: the probe says so
  M3  same-box: MANY real mailboxes each get exactly one copy, the mailbox
      keeps one (`redirect :copy`, K2's keep shape)
  M4  one over: with MANY + 1 redirects exactly MANY go
  M5  how much out-of-office text Stalwart takes (K2's cap is 4096 bytes; a
      string over maxStringLength fails the whole script, loudly). Then the
      biggest script that leaves (that text, a system sender clause, the whole
      guard, MANY addresses of 254 bytes) is under Stalwart's script size limit
      and compiles. On 0.16.25: every copy reaches
      the "outside" sink, each its own message with one recipient, the mailbox
      as MAIL FROM and the marker once, with the out-of-office reply on top.
      On older versions K2 refuses outside forwards (forward_sync
      V_OUTSIDE_FORWARD), so this is recorded, not required
  M6  K2's real throttles (tests/mail/forward-throttles.json: 100 an hour per
      sender, 1000 for the box): a first message's MANY copies all go; a second
      message's copies wait in the queue, and none are dropped or bounced
  M7  the same throttle with a short window: the copies that waited arrive, even
      with the forward gone from the script by then
  M8  a mailbox with no copies yet this hour, K2's real throttles, a same-box
      list, two messages: are same-box copies held back? Recorded, not required
  M9  the same with a short window: every same-box copy arrives

Everything stays on loopback: "outside" is a sink this script runs.

    STALWART_PROBE=stalwart-forward-many-probe.py tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25
"""

import argparse
import importlib.util
import json
import os
import secrets
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("k2_forward_probe", os.path.join(HERE, "stalwart-forward-external-probe.py"))
probe = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(probe)

Client, Sink, check, info, wait_for = probe.Client, probe.Sink, probe.check, probe.info, probe.wait_for
CORE, MAIL, SIEVE, SUBMISSION = probe.CORE, probe.MAIL, probe.SIEVE, probe.SUBMISSION

# k2_core::mail_forward::MAX_DESTINATIONS (forward_sync::REDIRECT_LIMIT).
MANY = 100
OLD_LIMITS = (5, 6)
HOST = "mail.example.test"
# 254 bytes in all, as K2's address check allows: local 64, domain 189.
LONG_DOMAIN = f"{'b' * 60}.{'c' * 60}.{'d' * 54}.outside.test"


def long_address(i):
    return f"d{i:03d}{'a' * 60}@{LONG_DOMAIN}"


def goldens(name):
    with open(os.path.join(probe.GOLDENS, name)) as f:
        return f.read()


def guard_text():
    """K2's guard, as its goldens have it, with a system sender nobody here is."""
    src = goldens("external-system-sender.sieve").replace(probe.GOLDEN_HOST, HOST)
    src = src.replace("k2@brand.test", "k2@notify.example.test").replace("system@notify.brand.test", "system@notify.example.test")
    body = src.split("# k2-forward\n", 1)[1]
    return body[: body.index("redirect ")]


def biggest_script(dests, mailbox, text_len):
    """The most bytes K2's composer writes: `text_len` of out-of-office text, the
    guard, and `redirect :copy` (the longer of K2's two spellings)."""
    text = "x" * text_len
    head = 'require ["copy", "vacation", "envelope", "editheader", "spamtest", "relational", "comparator-i;ascii-numeric"];\n'
    ooo = f'# k2-ooo\nvacation :days 7 :from "{mailbox}" :addresses ["{mailbox}"] text:\n{text}\n.\n;\n'
    redirects = "".join(f'redirect :copy "{d}";\n' for d in dests)
    return f"{head}{ooo}# k2-forward\n{guard_text()}{redirects}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--domain", required=True)
    args = ap.parse_args()
    version = os.environ.get("STALWART_VERSION", "")
    outside_expected = version == "0.16.25"
    user, pw = os.environ["STALWART_PROBE_ADMIN"].split(":", 1)
    domain, tag = args.domain, secrets.token_hex(3)
    admin = Client(args.url, basic=(user, pw))
    n, a = admin.call("x:Domain/query", {"filter": {"name": domain}})
    did = a["ids"][0]
    sink = Sink()
    info("fixture", f"Stalwart {version}; MANY {MANY}; sink :{sink.port}; run {tag}")

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

    svc_id = account(f"k2fm-svc-{tag}", admin_role=True)
    n, a = admin.call("x:ApiKey/set", {"accountId": svc_id, "create": {"k": {"description": "k2", "permissions": {"@type": "Inherit"}, "allowedIps": {}}}})
    key = ((a.get("created") or {}).get("k") or {}).get("secret")
    if not key:
        sys.exit(f"service key: {n} {json.dumps(a)[:300]}")
    svc = Client(args.url, bearer=key)
    alice, bob = account(f"k2fm-alice-{tag}"), account(f"k2fm-bob-{tag}")
    alice_addr, bob_addr = f"k2fm-alice-{tag}@{domain}", f"k2fm-bob-{tag}@{domain}"
    # Fresh senders: the throttles count per mailbox, and alice has used her hour.
    dave, erin = account(f"k2fm-dave-{tag}"), account(f"k2fm-erin-{tag}")
    dave_addr, erin_addr = f"k2fm-dave-{tag}@{domain}", f"k2fm-erin-{tag}@{domain}"
    t0 = time.time()
    boxes = [(account(f"k2fm-m{i:03d}-{tag}"), f"k2fm-m{i:03d}-{tag}@{domain}") for i in range(MANY + 1)]
    info("fixture", f"{len(boxes)} mailboxes in {time.time() - t0:.1f}s")

    # The sink: outside.test and the long domain.
    n, a = admin.call("x:MtaRoute/set", {"create": {"r": {"@type": "Relay", "name": f"k2fmsink{tag}", "address": "127.0.0.1", "port": sink.port, "protocol": "smtp", "implicitTls": False}}})
    if not ((a.get("created") or {}).get("r") or {}).get("id"):
        sys.exit(f"sink route: {n} {json.dumps(a)[:300]}")
    n, g = admin.call("x:MtaOutboundStrategy/get", {"ids": ["singleton"]})
    old = g["list"][0]["route"]
    shifted = {str(int(k) + 2): v for k, v in (old.get("match") or {}).items()}
    shifted["0"] = {"if": "rcpt_domain == 'outside.test'", "then": f"'k2fmsink{tag}'"}
    shifted["1"] = {"if": f"rcpt_domain == '{LONG_DOMAIN}'", "then": f"'k2fmsink{tag}'"}
    n, a = admin.call("x:MtaOutboundStrategy/set", {"update": {"singleton": {"route": {"match": shifted, "else": old["else"]}}}})
    if a.get("notUpdated"):
        sys.exit(f"route bind: {json.dumps(a)[:300]}")
    # 0.16.10 refuses every reload while downloaded spam rules call `bit_and`
    # (K2's A4 repair rewrites them on a real box); drop them here.
    n, q = admin.call("x:SpamDnsblServer/query", {})
    if q.get("ids"):
        n, g = admin.call("x:SpamDnsblServer/get", {"ids": q["ids"]})
        bad = [x["id"] for x in (g.get("list") or []) if "bit_and" in json.dumps(x)]
        if bad:
            admin.call("x:SpamDnsblServer/set", {"destroy": bad})

    def reload_settings():
        n, a = admin.call("x:Action/set", {"create": {"r": {"@type": "ReloadSettings"}}})
        created = (a.get("created") or {}).get("r")
        time.sleep(2)
        return n == "x:Action/set" and created is not None, f"{n} {json.dumps(a)[:300]}"

    ok, why = reload_settings()
    info("sandbox: reload", why)

    def limits():
        n, g = admin.call("x:SieveUserInterpreter/get", {"ids": ["singleton"], "properties": ["maxRedirects", "maxOutMessages"]})
        row = (g.get("list") or [{}])[0]
        return row.get("maxRedirects"), row.get("maxOutMessages")

    def set_limits(r, o):
        return admin.call("x:SieveUserInterpreter/set", {"update": {"singleton": {"maxRedirects": r, "maxOutMessages": o}}})

    def put_script(account_id, src):
        blob, err = svc.upload(account_id, src.encode())
        if not blob:
            return "upload", {"error": err}
        n, a = svc.call("SieveScript/get", {"accountId": account_id, "ids": None}, using=(CORE, SIEVE))
        existing = [s["id"] for s in (a.get("list") or []) if s.get("name") == "k2"]
        if existing:
            return svc.call("SieveScript/set", {"accountId": account_id, "update": {existing[0]: {"blobId": blob}}, "onSuccessActivateScript": existing[0]}, using=(CORE, SIEVE))
        return svc.call("SieveScript/set", {"accountId": account_id, "create": {"k": {"name": "k2", "blobId": blob}}, "onSuccessActivateScript": "#k"}, using=(CORE, SIEVE))

    def saved(res):
        n, a = res
        return n == "SieveScript/set" and not a.get("notUpdated") and not a.get("notCreated")

    ident = {}

    def ham(mid, to=None):
        """bob → alice (or `to`), authenticated (the spam filter leaves it alone)."""
        to = to or alice_addr
        if "bob" not in ident:
            n, a = svc.call("Identity/get", {"accountId": bob, "ids": None}, using=(CORE, MAIL, SUBMISSION))
            found = next((i["id"] for i in (a.get("list") or []) if i.get("email", "").lower() == bob_addr.lower()), None)
            if not found:
                n, a = svc.call("Identity/set", {"accountId": bob, "create": {"i": {"email": bob_addr}}}, using=(CORE, MAIL, SUBMISSION))
                found = ((a.get("created") or {}).get("i") or {}).get("id")
            ident["bob"] = found
        n, q = svc.call("Mailbox/query", {"accountId": bob, "filter": {"role": "drafts"}}, using=(CORE, MAIL))
        box = (q.get("ids") or [None])[0]
        if not box:
            n, q = svc.call("Mailbox/query", {"accountId": bob, "filter": {"role": "inbox"}}, using=(CORE, MAIL))
            box = (q.get("ids") or [None])[0]
        email = {"mailboxIds": {box: True}, "from": [{"email": bob_addr, "name": "Bob"}], "to": [{"email": to}], "subject": f"probe {mid}",
                 "bodyValues": {"t": {"value": f"Hello ({mid})."}}, "textBody": [{"partId": "t", "type": "text/plain"}]}
        r = svc.batch((CORE, MAIL, SUBMISSION), [
            ["Email/set", {"accountId": bob, "create": {"e": email}}, "0"],
            ["EmailSubmission/set", {"accountId": bob, "create": {"s": {"emailId": "#e", "identityId": ident["bob"],
                "envelope": {"mailFrom": {"email": bob_addr}, "rcptTo": [{"email": to}]}}}, "onSuccessDestroyEmail": ["#s"]}, "1"],
        ])
        if not any(m[0] == "EmailSubmission/set" and (m[1].get("created") or {}).get("s") for m in r):
            info(f"ham {mid}", json.dumps(r)[:400])

    def kept(mid, account):
        n, a = svc.call("Email/query", {"accountId": account, "filter": {"subject": f"probe {mid}"}}, using=(CORE, MAIL))
        return len(a.get("ids") or []) if n == "Email/query" else -1

    def arrived(mid, want, secs=120):
        """Each mailbox's copies of `mid`, once enough are in and it has settled."""
        def count():
            return [kept(mid, acc) for acc, _ in boxes]
        wait_for(lambda: sum(1 for c in count() if c >= 1) >= want, secs)
        time.sleep(5)
        return count()

    def queue_size():
        n, q = admin.call("x:QueuedMessage/query", {})
        return len(q.get("ids") or [])

    same_box = [addr for _, addr in boxes[:MANY]]

    def keep_shape(dests):
        return 'require ["copy"];\n# k2-forward\n' + "".join(f'redirect :copy "{d}";\n' for d in dests)

    # ── M0 ──────────────────────────────────────────────────────────
    n, g = admin.call("x:SieveUserInterpreter/get", {"ids": ["singleton"]})
    row = (g.get("list") or [{}])[0]
    info("M0 the interpreter as shipped", json.dumps(row))
    max_script = row.get("maxScriptSize")
    info("M0 maxScriptSize", f"{max_script} ({'Stalwart reports it' if max_script is not None else 'absent: the built-in default applies'})")

    # ── M1 an old box ───────────────────────────────────────────────
    n, a = set_limits(*OLD_LIMITS)
    check("M1 the old limits are set", n == "x:SieveUserInterpreter/set" and not a.get("notUpdated"), f"{n} {json.dumps(a)[:200]}")
    ok, why = reload_settings()
    check("M1 reload", ok, why)
    check(f"M1 a script with {MANY} redirects saves on the old limit", saved(put_script(alice, keep_shape(same_box))), "")
    mid = f"m1-{tag}"
    ham(mid)
    time.sleep(10)
    got = arrived(mid, OLD_LIMITS[0], 60)
    check(f"M1 on the old limit only {OLD_LIMITS[0]} of {MANY} go: the silent loss", sum(1 for c in got if c >= 1) == OLD_LIMITS[0], f"{sum(1 for c in got if c >= 1)} mailboxes got it")
    check("M1 and the mailbox keeps its copy", kept(mid, alice) == 1, f"kept {kept(mid, alice)}")

    # ── M2 K2's limits ──────────────────────────────────────────────
    n, a = set_limits(MANY, MANY + 1)
    accepted = n == "x:SieveUserInterpreter/set" and not a.get("notUpdated")
    check(f"M2 Stalwart accepts maxRedirects {MANY}, maxOutMessages {MANY + 1}", accepted, f"{n} {json.dumps(a)[:400]}")
    if not accepted:
        print(f"== HARD CEILING: Stalwart {version} refused maxRedirects {MANY} ==", flush=True)
        sys.exit(1)
    ok, why = reload_settings()
    check("M2 reload", ok, why)
    check("M2 reads back", limits() == (MANY, MANY + 1), json.dumps(limits()))

    # ── M3 same-box: real delivery to every mailbox ─────────────────
    check("M3 the script saves", saved(put_script(alice, keep_shape(same_box))), "")
    mid = f"m3-{tag}"
    t0 = time.time()
    ham(mid)
    got = arrived(mid, MANY)
    took = time.time() - t0
    check(f"M3 all {MANY} same-box mailboxes get it", sum(1 for c in got[:MANY] if c >= 1) == MANY, f"{sum(1 for c in got[:MANY] if c >= 1)} in {took:.0f}s")
    check("M3 each exactly once", all(c == 1 for c in got[:MANY]), json.dumps(sorted(set(got[:MANY]))))
    check("M3 the mailbox keeps one copy", kept(mid, alice) == 1, f"kept {kept(mid, alice)}")

    # ── M4 one over ─────────────────────────────────────────────────
    over = [addr for _, addr in boxes[: MANY + 1]]
    check(f"M4 a script with {MANY + 1} redirects saves", saved(put_script(alice, keep_shape(over))), "")
    mid = f"m4-{tag}"
    ham(mid)
    time.sleep(10)
    got = arrived(mid, MANY)
    check(f"M4 exactly {MANY} go", sum(1 for c in got if c >= 1) == MANY, f"{sum(1 for c in got if c >= 1)}; the last in the script got {got[MANY]}")

    # ── M5 the biggest script K2 can write ──────────────────────────
    dests = [long_address(i) for i in range(MANY)]
    check("M5 the addresses are 254 bytes", all(len(d) == 254 for d in dests), f"{len(dests[0])}")

    def compiles(src):
        blob, err = svc.upload(alice, src.encode())
        n, a = svc.call("SieveScript/validate", {"accountId": alice, "blobId": blob}, using=(CORE, SIEVE)) if blob else ("upload", {"error": err})
        return n == "SieveScript/validate" and a.get("error") is None, f"{n} {json.dumps(a)[:300]}"

    text_len = next((n for n in range(4096, 4080, -1) if compiles(biggest_script(dests[:1], alice_addr, n))[0]), None)
    check("M5 Stalwart takes out-of-office text near K2's cap", text_len is not None, f"the longest it takes: {text_len} bytes (K2's cap: 4096)")
    if text_len is None:
        text_len = 4000
    src = biggest_script(dests, alice_addr, text_len)
    info("M5 the biggest script", f"{len(src.encode())} bytes; Stalwart's limit {max_script}")
    if max_script is not None:
        check("M5 under Stalwart's script size limit", len(src.encode()) < int(max_script), f"{len(src.encode())} < {max_script}")
    ok, why = compiles(src)
    check("M5 it compiles", ok, why)
    check("M5 it saves and activates", saved(put_script(alice, src)), "")
    mid = f"m5-{tag}"
    ham(mid)
    wait_for(lambda: len(sink.with_id(mid)) >= MANY, 180 if outside_expected else 40)
    time.sleep(5)
    copies = sink.with_id(mid)
    rcpts = sorted(r for m in copies for r in m["rcpt"])

    def say(step, ok, detail=""):
        if outside_expected:
            check(step, ok, detail)
        else:
            info(step, f"{'yes' if ok else 'no'}: {detail}")

    say(f"M5 all {MANY} long outside addresses get it", rcpts == sorted(dests), f"{len(rcpts)} at the sink, {len(set(rcpts))} different")
    say("M5 each is its own message with one recipient", len(copies) == MANY and all(len(m["rcpt"]) == 1 for m in copies), f"{len(copies)} messages")
    say("M5 each has the mailbox as MAIL FROM", bool(copies) and all((m["from"] or "").lower() == alice_addr.lower() for m in copies), "")
    say("M5 each carries the marker once", bool(copies) and all(m["data"].count("X-K2-Forwarded:") == 1 for m in copies), "")
    n, q = svc.call("Email/query", {"accountId": bob, "filter": {"from": alice_addr}}, using=(CORE, MAIL))
    replies = len(q.get("ids") or [])
    say("M5 the out-of-office reply fits on top", replies >= 1 and len(rcpts) == MANY, f"bob has {replies} from alice")

    # ── M6 K2's real throttles ──────────────────────────────────────
    with open(os.path.join(HERE, "forward-throttles.json")) as f:
        throttles = json.load(f)

    def set_throttles(items):
        ids = []
        for i, t in enumerate(items):
            n, a = admin.call("x:MtaOutboundThrottle/set", {"create": {f"t{i}": t}})
            tid = ((a.get("created") or {}).get(f"t{i}") or {}).get("id")
            check(f"throttle '{t['description']}' accepted", bool(tid), f"{n} {json.dumps(a)[:300]}")
            if tid:
                ids.append(tid)
        ok, why = reload_settings()
        check("throttles reload", ok, why)
        return ids

    def clear_throttles(ids):
        if ids:
            admin.call("x:MtaOutboundThrottle/set", {"destroy": ids})
        reload_settings()

    def run_throttled(label, wait_second):
        """Two messages in a row: the first must all go; returns how to count the second."""
        if outside_expected:
            short = [f"t{label}{i:03d}@outside.test" for i in range(MANY)]
            check(f"{label} the script saves", saved(put_script(alice, keep_shape(short))), "")

            def count(mid):
                return len({r for m in sink.with_id(mid) for r in m["rcpt"]})
        else:
            check(f"{label} the script saves", saved(put_script(alice, keep_shape(same_box))), "")

            def count(mid):
                return sum(1 for acc, _ in boxes[:MANY] if kept(mid, acc) >= 1)
        one, two = f"{label}a-{tag}", f"{label}b-{tag}"
        before = queue_size()
        ham(one)
        wait_for(lambda: count(one) >= MANY, 180)
        first = count(one)
        check(f"{label} a first message's {MANY} copies all go under the throttles", first == MANY, f"{first}")
        ham(two)
        time.sleep(wait_second)
        second = count(two)
        info(f"{label} the second message right after", f"{second} of {MANY} delivered; queue {before} -> {queue_size()}")
        return count, two, second

    ids = set_throttles(throttles)
    count, two, second = run_throttled("m6", 25)
    q_now = queue_size()
    check("M6 a second message's copies wait: delivered or queued, none dropped", second + q_now >= MANY, f"{second} delivered, {q_now} queued")
    info("M6 is the second message held back at all", f"{MANY - second} of {MANY} still waiting")
    n, q = admin.call("x:QueuedMessage/query", {})
    sample = (q.get("ids") or [])[:1]
    if sample:
        n, g = admin.call("x:QueuedMessage/get", {"ids": sample})
        info("M6 one queued copy", json.dumps(g)[:1200])
    clear_throttles(ids)

    # ── M7 the same with a short window: they arrive ────────────────
    short_window = [dict(t, rate={"count": t["rate"]["count"], "period": 20000}) for t in throttles]
    ids = set_throttles(short_window)
    count, two, second = run_throttled("m7", 1)
    check("M7 the forward comes off while they wait", saved(put_script(alice, "keep;\r\n")), "")
    t0 = time.time()
    wait_for(lambda: count(two) >= MANY, 420)
    done = count(two)
    check("M7 every copy that waited arrives", done == MANY, f"{done} of {MANY} after {time.time() - t0:.0f}s")
    clear_throttles(ids)

    # ── M8 do the throttles hold same-box copies back? ──────────────
    def same_box_count(mid):
        return sum(1 for acc, _ in boxes[:MANY] if kept(mid, acc) >= 1)

    ids = set_throttles(throttles)
    check("M8 the script saves", saved(put_script(dave, keep_shape(same_box))), "")
    delivered = []
    for label in ("m8a", "m8b"):
        mid = f"{label}-{tag}"
        ham(mid, dave_addr)
        wait_for(lambda: same_box_count(mid) >= MANY, 45)
        delivered.append(same_box_count(mid))
    info("M8 same-box copies delivered under K2's real throttles, two messages in a row", f"{delivered} of {MANY} each; queue {queue_size()}")
    clear_throttles(ids)

    # ── M9 and they arrive ──────────────────────────────────────────
    ids = set_throttles(short_window)
    check("M9 the script saves", saved(put_script(erin, keep_shape(same_box))), "")
    one, two = f"m9a-{tag}", f"m9b-{tag}"
    ham(one, erin_addr)
    ham(two, erin_addr)
    t0 = time.time()
    wait_for(lambda: same_box_count(one) >= MANY and same_box_count(two) >= MANY, 420)
    check("M9 every same-box copy arrives", (same_box_count(one), same_box_count(two)) == (MANY, MANY), f"{same_box_count(one)} and {same_box_count(two)} of {MANY} after {time.time() - t0:.0f}s")
    clear_throttles(ids)

    print(f"== forward many probe, Stalwart {version}: {len(probe.FAILED)} failed check(s): {probe.FAILED} ==", flush=True)
    sys.exit(1 if probe.FAILED else 0)


if __name__ == "__main__":
    main()
