#!/usr/bin/env python3
"""Tests for tests/interop/postal_plane.py with no socket (Plane(serve=False)
and call()): the steps a K2 server's Postal loop takes, in the order the
daemon takes them (crates/k2-daemon/src/postal_net/pairing.rs: respond
when `side == to, step == requested, status == pending_in`, reveal when
`side == from, step == responded, pending_in`, finish when `side == to,
step == revealed, pending_in`), and the answers it parses.

    python3 -B tests/interop/test_postal_plane.py

Python stdlib only."""
import base64
import os
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture  # noqa: E402
import postal_plane  # noqa: E402

P = postal_plane.API
ALICE, BOB = "alice::acme.postal.bot", "bob::initech.postal.bot"
PAIR = "01JD4X0Z7Q8R9S0T1V2W3X4Y5Z"
SEALED = base64.b64encode(bytes([2]) + bytes(80)).decode()


class Flow(unittest.TestCase):
    def setUp(self):
        self.plane = postal_plane.Plane({"k2c_a": "acme", "k2c_b": "initech"}, capture.Recorder(), {"acme": "A", "initech": "B"}, serve=False)
        self.pst = {}
        self.owner = {}
        for who, cred in (("A", "k2c_a"), ("B", "k2c_b")):
            st, v = self.plane.call("POST", f"{P}/session", cred, {})
            self.assertEqual(st, 200, v)
            self.assertTrue(v["token"].startswith("pst_"), v)
            self.pst[who] = v["token"]
            st, v = self.plane.call("POST", f"{P}/decide-token", cred, {})
            self.assertEqual(st, 200, v)
            self.assertTrue(v["token"].startswith("k2c_pd_"), v)
            self.owner[who] = v["token"]
        for who, addr in (("A", ALICE), ("B", BOB)):
            st, v = self.plane.call("PUT", f"{P}/me", self.pst[who], {"addr": addr, "public_key_pem": f"PEM-{who}", "typ": "session"})
            self.assertEqual(st, 200, v)

    def view(self, who):
        st, v = self.plane.call("GET", f"{P}/pairs", self.pst[who])
        self.assertEqual(st, 200, v)
        self.assertEqual(len(v["pairs"]), 1, v)
        return v["pairs"][0]

    def pair_to_revealed(self):
        st, v = self.plane.call("POST", f"{P}/pairs", self.pst["A"], {"id": PAIR, "from": ALICE, "to": BOB, "typ": "session",
                                                                      "public_key_pem": "PEM-A", "sas_version": 2, "commitment": "c0ffee"})
        self.assertEqual((st, v.get("existing")), (200, False), v)
        st, v = self.plane.call("POST", f"{P}/pairs/{PAIR}/respond", self.pst["B"], {"public_key_pem": "PEM-B", "nb": "beef", "typ": "session"})
        self.assertEqual(st, 200, v)
        st, v = self.plane.call("POST", f"{P}/pairs/{PAIR}/reveal", self.pst["A"], {"na": "f00d"})
        self.assertEqual(st, 200, v)

    def confirm(self, who, side, transcript="ab" * 32, decision="accept"):
        return self.plane.call("POST", f"{P}/pairs/{PAIR}/confirm", self.owner[who],
                               {"side": side, "decision": decision, "transcript_sha256": transcript, "sig": f"sig-{who}"})

    def test_a_box_registers_only_its_own_label(self):
        st, v = self.plane.call("PUT", f"{P}/me", self.pst["A"], {"addr": BOB, "public_key_pem": "x", "typ": "session"})
        self.assertEqual((st, v["error"]["code"]), (403, "not_your_handle"))
        st, v = self.plane.call("POST", f"{P}/session", "k2c_nobody", {})
        self.assertEqual((st, v["error"]["code"]), (401, "unauthorized"))

    def test_one_decide_token_at_a_time(self):
        st, v = self.plane.call("POST", f"{P}/decide-token", "k2c_a", {})
        self.assertEqual((st, v["error"]["code"]), (409, "already_minted"))
        st, v = self.plane.call("POST", f"{P}/decide-token/revoke", "k2c_a", {})
        self.assertEqual((st, v), (200, {"revoked": 1}))
        st, v = self.plane.call("POST", f"{P}/decide-token", "k2c_a", {})
        self.assertEqual(st, 200, v)

    def test_each_side_sees_its_turn(self):
        st, v = self.plane.call("POST", f"{P}/pairs", self.pst["A"], {"id": PAIR, "from": ALICE, "to": BOB, "typ": "session",
                                                                      "public_key_pem": "PEM-A", "sas_version": 2, "commitment": "c0ffee"})
        self.assertEqual(st, 200, v)
        b = self.view("B")
        self.assertEqual((b["side"], b["step"], b["status"], b["handle"], b["peer"], b["peer_key_pem"], b["commitment"]),
                         ("to", "requested", "pending_in", BOB, ALICE, "PEM-A", "c0ffee"))
        self.assertEqual(self.view("A")["status"], "pending_out")
        st, v = self.plane.call("POST", f"{P}/pairs/{PAIR}/reveal", self.pst["A"], {"na": "f00d"})
        self.assertEqual((st, v["error"]["code"]), (409, "not_responded"))
        st, v = self.plane.call("POST", f"{P}/pairs/{PAIR}/respond", self.pst["B"], {"public_key_pem": "PEM-B", "nb": "beef", "typ": "session"})
        self.assertEqual(st, 200, v)
        a = self.view("A")
        self.assertEqual((a["side"], a["step"], a["status"], a["peer_key_pem"], a["nb"]), ("from", "responded", "pending_in", "PEM-B", "beef"))
        self.assertEqual(self.view("B")["status"], "pending_out")
        st, v = self.plane.call("POST", f"{P}/pairs/{PAIR}/reveal", self.pst["A"], {"na": "f00d"})
        self.assertEqual(st, 200, v)
        b = self.view("B")
        self.assertEqual((b["step"], b["status"], b["na"]), ("revealed", "pending_in", "f00d"))
        self.assertEqual(self.view("A")["status"], "pending_in")

    def test_only_the_owner_confirms_and_both_accepting_is_accepted(self):
        self.pair_to_revealed()
        st, v = self.plane.call("POST", f"{P}/pairs/{PAIR}/confirm", self.pst["B"], {"side": "to", "decision": "accept", "transcript_sha256": "ab", "sig": "s"})
        self.assertEqual((st, v["error"]["code"]), (403, "owner_required"))
        st, v = self.confirm("B", "from")
        self.assertEqual((st, v["error"]["code"]), (403, "forbidden"))
        st, v = self.confirm("B", "to")
        self.assertEqual((st, v["status"], v["side"]), (200, "pending_out", "to"), v)
        self.assertEqual(self.view("A")["status"], "pending_in")
        st, v = self.confirm("B", "to")
        self.assertEqual((st, v["error"]["code"]), (409, "already_decided"))
        st, v = self.confirm("A", "from")
        self.assertEqual((st, v["status"]), (200, "accepted"), v)
        a = self.view("A")
        peer = [c for c in a["confirmations"] if c["side"] != a["side"]]
        self.assertEqual([(c["decision"], c["sig"]) for c in peer], [("accept", "sig-B")])
        self.assertEqual(self.view("B")["status"], "accepted")
        st, v = self.plane.call("GET", f"{P}/pairs/{PAIR}", self.pst["B"])
        self.assertEqual((st, v["status"]), (200, "accepted"))
        st, v = self.plane.call("POST", f"{P}/pairs/{PAIR}/revoke", self.owner["A"], {})
        self.assertEqual((st, v["status"]), (200, "revoked"), v)

    def test_different_transcripts_fail_the_pair(self):
        self.pair_to_revealed()
        self.confirm("B", "to", transcript="ab" * 32)
        st, v = self.confirm("A", "from", transcript="cd" * 32)
        self.assertEqual((st, v["status"], v["failed_reason"]), (200, "failed", "transcript_mismatch"), v)

    def test_held_mail_reaches_its_box_and_the_sender_gets_its_receipt(self):
        st, v = self.plane.call("GET", f"{P}/events?wait=0", self.pst["A"])
        self.assertEqual((st, v["events"]), (200, []), v)
        cursor_a = v["cursor"]
        cursor_b = self.plane.call("GET", f"{P}/events?wait=0", self.pst["B"])[1]["cursor"]
        put = {"v": 2, "from": ALICE, "to": BOB, "client_id": "01JD4X0Z7Q8R9S0T1V2W3X4Y60", "sealed": SEALED}
        st, v = self.plane.call("POST", f"{P}/hold", self.pst["B"], put)
        self.assertEqual((st, v["error"]["code"]), (403, "not_your_handle"))
        st, v = self.plane.call("POST", f"{P}/hold", self.pst["A"], dict(put, sealed=base64.b64encode(b"plain text, not a seal" * 4).decode()))
        self.assertEqual((st, v["error"]["code"]), (400, "bad_sealed"))
        st, first = self.plane.call("POST", f"{P}/hold", self.pst["A"], put)
        self.assertEqual((st, first["duplicate"]), (200, False), first)
        st, again = self.plane.call("POST", f"{P}/hold", self.pst["A"], put)
        self.assertEqual((st, again["id"], again["duplicate"]), (200, first["id"], True))
        st, v = self.plane.call("GET", f"{P}/events?cursor={cursor_b}&wait=0", self.pst["B"])
        self.assertEqual([e["type"] for e in v["events"]], ["hold"], v)
        st, page = self.plane.call("GET", f"{P}/hold?limit=100", self.pst["B"])
        self.assertEqual([(i["id"], i["from"], i["sealed"]) for i in page["items"]], [(first["id"], ALICE, SEALED)])
        self.assertEqual(self.plane.call("GET", f"{P}/hold?limit=100", self.pst["A"])[1]["items"], [])
        cid = put["client_id"]
        self.assertEqual(self.plane.call("GET", f"{P}/hold/sent?client_ids={cid}", self.pst["A"]), (200, {cid: "held"}))
        st, v = self.plane.call("POST", f"{P}/hold/ack", self.pst["B"], {"ids": [first["id"]]})
        self.assertEqual((st, v), (200, {"acked": [first["id"]]}))
        self.assertEqual(self.plane.call("GET", f"{P}/hold/sent?client_ids={cid}", self.pst["A"]), (200, {cid: "delivered"}))
        st, v = self.plane.call("GET", f"{P}/events?cursor={cursor_a}&wait=0", self.pst["A"])
        self.assertEqual([(e["type"], e.get("state")) for e in v["events"]], [("receipt", "held"), ("receipt", "delivered")], v)
        st, v = self.plane.call("GET", f"{P}/events?cursor=999999&wait=0", self.pst["A"])
        self.assertEqual((st, v["error"]["code"]), (410, "cursor_expired"))

    def test_an_unknown_route_is_a_v1_error(self):
        st, v = self.plane.call("GET", f"{P}/nothing", self.pst["A"])
        self.assertEqual((st, v["error"]["code"]), (404, "not_found"))


if __name__ == "__main__":
    unittest.main(verbosity=1)
