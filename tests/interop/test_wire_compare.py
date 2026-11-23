#!/usr/bin/env python3
"""Unit tests for tests/interop/wire_compare.py on the fixture captures in
tests/interop/fixtures (a baseline of two default-brand servers and a mixed
session with an `xy` server). Each test changes a copy of a capture in a
temp folder and runs the comparer on it.

    python3 -B tests/interop/test_wire_compare.py

Python stdlib only; no daemon, no network.
"""
import contextlib
import copy
import io
import json
import os
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import wire_compare as wc  # noqa: E402

FIXTURES = os.path.join(HERE, "fixtures")


def load(name):
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def find(rows, **want):
    """The exchanges whose fields (and request method/target prefix) match."""
    out = []
    for r in rows:
        if r.get("kind") != "exchange":
            continue
        ok = True
        for k, v in want.items():
            if k == "target":
                ok &= r["request"]["target"].split("?")[0] == v
            elif k == "method":
                ok &= r["request"]["method"] == v
            else:
                ok &= r.get(k) == v
        if ok:
            out.append(r)
    if not out:
        raise AssertionError(f"no exchange matches {want}")
    return out


class Compare(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.base = load("baseline.jsonl")
        self.mixed = load("mixed.jsonl")

    def tearDown(self):
        self.tmp.cleanup()

    def run_compare(self, base=None, mixed=None):
        paths = []
        for name, rows in (("base.jsonl", base or self.base), ("mixed.jsonl", mixed or self.mixed)):
            p = os.path.join(self.tmp.name, name)
            with open(p, "w", encoding="utf-8") as f:
                for r in rows:
                    f.write(json.dumps(r) + "\n")
            paths.append(p)
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            rc = wc.run(*paths)
        return rc, out.getvalue()

    def assert_fails(self, *texts, base=None, mixed=None):
        rc, out = self.run_compare(base, mixed)
        self.assertEqual(rc, 1, out)
        for t in texts:
            self.assertIn(t, out)
        return out

    def assert_ok(self, base=None, mixed=None):
        rc, out = self.run_compare(base, mixed)
        self.assertEqual(rc, 0, out)
        self.assertIn("wire-compare: ok (", out)
        return out

    # ── alike ──

    def test_the_fixture_sessions_are_alike(self):
        out = self.assert_ok()
        self.assertIn("11 exchange keys", out)

    def test_boot_status_keys_are_not_compared_but_its_names_are(self):
        find(self.mixed, channel="owner", to="A", target="/boot-status")[0]["answer"]["body"]["rootHelper"] = {"state": "ok"}
        self.assert_ok()

    def test_a_null_on_one_side_is_a_note(self):
        find(self.mixed, channel="peer", target="/cli/federation/roster")[0]["answer"]["body"]["updated_at"] = None
        out = self.assert_ok()
        self.assertIn("only in mixed: ab:$.updated_at=null (a null on the other side)", out)

    def test_display_fields_differ_freely(self):
        for rows, cli in ((self.base, "k2"), (self.mixed, "xy")):
            ans = find(rows, channel="owner", to="B", target="/cli/postal/friends")[-1]["answer"]
            ans["body"]["hint"] = f"run {cli} postal decide to answer it"
            ans["body"]["error"] = f"{cli}: not yet"
        self.assert_ok()

    def test_a_poll_counts_only_its_last_state(self):
        first = find(self.base, channel="owner", to="B", target="/cli/postal/friends")[0]
        first["answer"]["body"]["friends"][0]["codeWaiting"] = True
        # The mixed run saw the state at once: one poll, the last one.
        polls = find(self.mixed, channel="owner", to="B", target="/cli/postal/friends")
        self.mixed.remove(polls[0])
        self.assert_ok()

    def test_a_background_status_is_a_note(self):
        find(self.mixed, channel="peer", target="/cli/federation/roster")[0]["answer"]["status"] = 503
        out = self.assert_ok()
        self.assertIn("wire-compare: note peer B->A GET /cli/federation/roster: statuses", out)

    def test_text_from_another_server_in_a_display_field_is_a_note(self):
        for rows, cli in ((self.base, "k2"), (self.mixed, "xy")):
            find(rows, channel="peer", target="/cli/federation/vault-recipient")[0]["answer"]["body"]["hint"] = f"run {cli} vault incoming"
        out = self.assert_ok()
        self.assertIn("the brand word 'xy' in the answer body $.hint (a display field", out)

    # ── differences ──

    def test_a_tag_spelled_with_the_brand_fails(self):
        find(self.mixed, channel="peer", target="/cli/federation/vault-recipient")[0]["answer"]["body"]["tag"] = "xy-vault/1"
        self.assert_fails(
            "peer A->B POST /cli/federation/vault-recipient: only in baseline: ab:$.tag=tag:k2-vault/1",
            "peer A->B POST /cli/federation/vault-recipient: only in mixed: ab:$.tag=tag:xy-vault/1",
            "the brand word 'xy' in the answer body $.tag: a brand name on the wire")

    def test_a_user_agent_with_the_brand_fails(self):
        hs = find(self.mixed, channel="peer", target="/cli/federation/roster")[0]["request"]["headers"]
        hs[0][1] = "xy-daemon/0.46.17"
        self.assert_fails(
            "only in mixed: rh:user-agent=xy-daemon/{version}",
            "the brand word 'xy' in the request header user-agent: a brand name on the wire")

    def test_a_product_name_in_a_body_value_fails(self):
        find(self.mixed, channel="peer", target="/cli/federation/roster")[0]["answer"]["body"]["server"] = "Globex Desk"
        self.assert_fails(
            "peer B->A GET /cli/federation/roster: only in mixed: ab:$.server=tag:Globex Desk",
            "the brand word 'Globex Desk' in the answer body $.server")

    def test_a_token_prefix_from_the_brand_fails(self):
        find(self.mixed, channel="postal", **{"from": "B"}, target="/api/postal/v1/session")[0]["answer"]["body"]["token"] = "xyst_Z9x8C7v6B5n4M3a2S1d0"
        self.assert_fails("postal B->plane POST /api/postal/v1/session: only in mixed: ab:$.token=<token:xyst_>")

    def test_a_missing_route_fails(self):
        self.mixed.remove(find(self.mixed, channel="peer", target="/cli/federation/vault-recipient")[0])
        self.assert_fails("only in baseline: peer A->B POST /cli/federation/vault-recipient")

    def test_an_extra_key_fails(self):
        find(self.mixed, channel="peer", target="/cli/federation/inbound")[0]["request"]["body"]["payload"]["brand"] = "neutral"
        self.assert_fails("peer A->B POST /cli/federation/inbound: only in mixed: rb:$.payload.brand=str")

    def test_a_changed_value_type_fails(self):
        find(self.mixed, channel="peer", target="/cli/federation/inbound")[0]["request"]["body"]["payload"]["ttl"] = "300"
        self.assert_fails("only in baseline: rb:$.payload.ttl=num", "only in mixed: rb:$.payload.ttl=str")

    def test_an_owner_status_differs_fails(self):
        find(self.mixed, channel="owner", to="A", target="/cli/federation/send")[0]["answer"]["status"] = 403
        self.assert_fails("owner owner->A POST /cli/federation/send: statuses ['st:200'] in baseline, ['st:403'] in mixed")

    # ── the three checks ──

    def test_a_local_brand_header_on_federation_fails(self):
        find(self.mixed, channel="peer", target="/cli/federation/inbound")[0]["answer"]["headers"].append(["X-K5-Server-Brand", "xy"])
        self.assert_fails("the answer carries X-K5-Server-Brand, a local header, off the machine")

    def test_a_cli_header_on_postal_fails(self):
        find(self.mixed, channel="postal", **{"from": "A"}, target="/api/postal/v1/session")[0]["request"]["headers"].append(["X-K5-Cli-Brand", "k2"])
        self.assert_fails("the request carries X-K5-Cli-Brand, a local header, off the machine")

    def test_a_server_that_is_not_the_named_build_fails(self):
        for ex in find(self.mixed, channel="owner", to="B"):
            for h in ex["answer"]["headers"]:
                if h[0] == "X-K5-Server-Brand":
                    h[1] = "k2"
        boot = find(self.mixed, channel="owner", to="B", target="/boot-status")[0]
        boot["answer"]["body"]["cliName"] = "k2"
        self.assert_fails(
            "mixed: B answered x-k5-server-brand ['k2'], want 'xy': not the build the session names",
            "mixed: B's /boot-status cliName is ['k2'], want 'xy'")

    def test_no_local_brand_header_at_all_fails(self):
        for ex in find(self.base, channel="owner", to="A"):
            ex["answer"]["headers"] = [h for h in ex["answer"]["headers"] if h[0] != "X-K5-Server-Brand"]
        self.assert_fails("baseline: no /cli/* answer from A carried x-k5-server-brand")

    # ── input ──

    def test_a_capture_without_its_session_line_is_a_bad_input(self):
        rc, out = self.run_compare(mixed=[r for r in self.mixed if r["kind"] == "exchange"])
        self.assertEqual(rc, 2, out)
        self.assertIn("no session line", out)

    def test_a_capture_without_exchanges_is_a_bad_input(self):
        rc, out = self.run_compare(base=[copy.deepcopy(self.base[0])])
        self.assertEqual(rc, 2, out)
        self.assertIn("no exchanges", out)

    def test_the_report_file_has_the_lines(self):
        find(self.mixed, channel="peer", target="/cli/federation/vault-recipient")[0]["answer"]["body"]["tag"] = "xy-vault/1"
        paths = []
        for name, rows in (("b.jsonl", self.base), ("m.jsonl", self.mixed)):
            p = os.path.join(self.tmp.name, name)
            with open(p, "w", encoding="utf-8") as f:
                f.write("\n".join(json.dumps(r) for r in rows) + "\n")
            paths.append(p)
        report = os.path.join(self.tmp.name, "report.txt")
        with contextlib.redirect_stdout(io.StringIO()):
            rc = wc.main([paths[0], paths[1], "--report", report])
        self.assertEqual(rc, 1)
        with open(report, encoding="utf-8") as f:
            text = f.read()
        self.assertIn("wire-compare: FAILED (", text)


class Classes(unittest.TestCase):
    def test_random_values_have_a_class(self):
        cases = {
            "0b6f6c9e-2f9e-4d7a-9a51-0c1d2e3f4a5b": "<uuid>",
            "01JD4X0Z7Q8R9S0T1V2W3X4Y5Z": "<ulid>",
            "9f8e7d6c5b4a39281706f5e4d3c2b1a0": "<hex>",
            "2026-11-22T10:00:01.123Z": "<time>",
            "pst_Q1w2E3r4T5y6U7i8O9p0": "<token:pst_>",
            "k2c_pd_Q1w2E3r4T5y6U7i8O9p0": "<token:k2c_pd_>",
            "-----BEGIN PUBLIC KEY-----\nMFkw\n-----END PUBLIC KEY-----\n": "<pem:PUBLIC KEY>",
            "MEUCIQDx3Lr0p7Jm2Qe9vZ1nK4tY8wB5cH6dF7gS2aX9kL0m": "<b64>",
        }
        for s, want in cases.items():
            self.assertEqual(wc.whole_class(s), want, s)

    def test_codes_and_tags_have_no_random_class(self):
        for s in ("remote_instruction_disabled", "k2-sas-v2", "inbox", "declined_by_peer", "k2", "K2-SIGNED-APPROVAL-1"):
            self.assertIsNone(wc.whole_class(s), s)

    def test_a_tag_is_found_at_the_start_of_a_token(self):
        s = wc.Session(os.path.join(FIXTURES, "mixed.jsonl"))
        sh = wc.Shaper(s, {"k2", "xy"})
        self.assertEqual(sh.value_class("k2v://ws/0b6f6c9e-2f9e-4d7a-9a51-0c1d2e3f4a5b/KEY"), "tag:k2v://ws/<uuid>/KEY")
        self.assertEqual(sh.value_class("k2-postal/0.46.17"), "tag:k2-postal/{version}")
        self.assertEqual(sh.value_class("hello ops"), "str")
        self.assertEqual(sh.text("k2-daemon/0.46.17 (relay-list) took 1234 ms"), "k2-daemon/{version} (relay-list) took {n} ms")
        self.assertEqual(sh.value_class("http://127.0.0.1:42013"), "str")
        self.assertEqual(sh.text("http://127.0.0.1:42013/x"), "http://127.0.0.1:{port:door-A}/x")


if __name__ == "__main__":
    unittest.main(verbosity=1)
