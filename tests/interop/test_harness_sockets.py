#!/usr/bin/env python3
"""The interop harness's own pieces over real sockets on 127.0.0.1 (brand
step C4): the door (tests/interop/door.py) passes a request on with the
server's Host, X-Forwarded-For and Connection: close, relays the answer
with Connection: close, and captures both, a gzip body decoded; the test
plane (tests/interop/postal_plane.py) answers over HTTP with Postal-Api: 1;
the capture keeps no owner token and no credential.

    python3 -B tests/interop/test_harness_sockets.py

Opens ports: on K2 Compute only (tests/interop/two-brands.sh runs it
before the builds). Python stdlib only."""
import gzip
import json
import os
import socketserver
import sys
import threading
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture  # noqa: E402
import door  # noqa: E402
import httpwire  # noqa: E402
import postal_plane  # noqa: E402


class Upstream:
    """A server that answers what it was sent, gzip'd, and asks to keep
    the connection (the door must not)."""

    def __init__(self):
        seen = self.seen = []

        class H(socketserver.StreamRequestHandler):
            def handle(self):
                req = httpwire.read_message(self.rfile, False)
                seen.append(req)
                body = gzip.compress(json.dumps({"host": req.header("host"), "xff": req.header("x-forwarded-for"),
                                                 "connection": req.header("connection"), "body": req.body.decode()}).encode())
                ans = httpwire.Message("HTTP/1.1 200 OK", [["Content-Type", "application/json"], ["Content-Encoding", "gzip"],
                                                           ["Content-Length", str(len(body))], ["Connection", "keep-alive"]], body)
                self.wfile.write(ans.to_bytes())

        class S(socketserver.ThreadingTCPServer):
            daemon_threads = True
            allow_reuse_address = True

        self.server = S(("127.0.0.1", 0), H)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class Sockets(unittest.TestCase):
    def test_the_door_passes_on_relays_and_captures(self):
        up = Upstream()
        rec = capture.Recorder()
        d = door.Door(up.port, "B", "A", "192.0.2.12", rec)
        try:
            body = json.dumps({"hello": "door"}).encode()
            req = httpwire.Message("POST /cli/federation/inbound?token=s3cret HTTP/1.1",
                                   [["Host", f"127.0.0.1:{d.port}"], ["Content-Type", "application/json"],
                                    ["Content-Length", str(len(body))], ["Connection", "keep-alive"], ["User-Agent", "k2-daemon/0.0.0"]], body)
            ans = httpwire.exchange(("127.0.0.1", d.port), req)
        finally:
            d.close()
            up.close()
        self.assertEqual(ans.status, 200)
        self.assertEqual(ans.header("connection"), "close")
        got = json.loads(gzip.decompress(ans.body))
        self.assertEqual(got, {"host": f"127.0.0.1:{up.port}", "xff": "192.0.2.12", "connection": "close", "body": '{"hello": "door"}'})
        self.assertEqual(len(rec.rows), 1)
        row = rec.rows[0]
        self.assertEqual((row["channel"], row["from"], row["to"]), ("peer", "A", "B"))
        self.assertEqual(row["request"]["target"], "/cli/federation/inbound?token=<redacted>")
        self.assertNotIn("x-forwarded-for", [h[0].lower() for h in row["request"]["headers"]], "the capture is what the sender sent")
        self.assertEqual(row["request"]["body"], {"hello": "door"})
        self.assertEqual(row["answer"]["body"]["xff"], "192.0.2.12", "a gzip answer is captured decoded")
        self.assertNotIn("s3cret", json.dumps(rec.rows))

    def test_the_door_answers_502_when_the_server_is_gone(self):
        up = Upstream()
        port = up.port
        up.close()
        rec = capture.Recorder()
        d = door.Door(port, "B", "A", "192.0.2.12", rec)
        try:
            ans = httpwire.exchange(("127.0.0.1", d.port), httpwire.Message("GET /cli/federation/roster HTTP/1.1", [["Host", "x"], ["Content-Length", "0"]], b""))
        finally:
            d.close()
        self.assertEqual(ans.status, 502)
        self.assertEqual(rec.rows[0]["answer"]["status"], 502)

    def test_the_plane_answers_over_http_and_keeps_no_credential(self):
        rec = capture.Recorder()
        plane = postal_plane.Plane({"k2c_interop_a": "acme"}, rec, {"acme": "A"})
        try:
            req = httpwire.Message("POST /api/postal/v1/session HTTP/1.1",
                                   [["Host", "x"], ["Authorization", "Bearer k2c_interop_a"], ["Content-Type", "application/json"], ["Content-Length", "2"]], b"{}")
            ans = httpwire.exchange(("127.0.0.1", plane.port), req)
            bad = httpwire.exchange(("127.0.0.1", plane.port), httpwire.Message("GET /api/postal/v1/pairs HTTP/1.1", [["Host", "x"], ["Content-Length", "0"]], b""))
        finally:
            plane.close()
        self.assertEqual((ans.status, ans.header("postal-api")), (200, "1"))
        token = ans.json_or_none()["token"]
        self.assertTrue(token.startswith("pst_"))
        self.assertEqual((bad.status, bad.json_or_none()["error"]["code"]), (401, "unauthorized"))
        self.assertEqual([(r["channel"], r["from"]) for r in rec.rows], [("postal", "A"), ("postal", "?")])
        text = json.dumps(rec.rows)
        self.assertNotIn("k2c_interop_a", text)
        self.assertNotIn(token, text)
        self.assertEqual(rec.rows[0]["answer"]["body"]["token"], "<token:pst_>")


if __name__ == "__main__":
    unittest.main(verbosity=1)
