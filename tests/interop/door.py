"""The door in front of one server, for the interop harness (brand step C4).

The other server is paired with the door's address, so everything it sends
this server goes through here: the door passes each request on as it came
(Host set to the server's own address, one exchange per connection), adds
`X-Forwarded-For` with a TEST-NET address so the server reads the request
as off the machine, as it would be on two hosts, and relays the answer
with `Connection: close`. Each exchange is captured on the `peer` channel:
the request as the sender sent it, the answer as the server gave it.
Python stdlib only."""
import socketserver
import threading

import httpwire
from capture import captured, redact_target

HOP = {"connection", "keep-alive", "proxy-connection", "host", "x-forwarded-for"}


class _Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


class Door:
    def __init__(self, upstream_port, role, peer_role, xff, rec):
        self.upstream = ("127.0.0.1", upstream_port)
        self.role = role
        self.peer_role = peer_role
        self.xff = xff
        self.rec = rec
        door = self

        class Handler(socketserver.StreamRequestHandler):
            timeout = 180

            def handle(self):
                door.serve(self.rfile, self.wfile)

        self.server = _Server(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, name=f"door-{role}", daemon=True)
        self.thread.start()

    def serve(self, rfile, wfile):
        try:
            req = httpwire.read_message(rfile, False)
        except httpwire.WireError as e:
            wfile.write(httpwire.answer_bytes(400, "Bad Request", [["Content-Type", "text/plain"]], str(e).encode()).to_bytes())
            return
        if req is None:
            return
        fwd = [["Host", f"{self.upstream[0]}:{self.upstream[1]}"]]
        fwd += [h for h in req.headers if h[0].lower() not in HOP]
        fwd += [["X-Forwarded-For", self.xff], ["Connection", "close"]]
        request = dict(method=req.method, target=redact_target(req.target), **captured(req))
        try:
            ans = httpwire.exchange(self.upstream, httpwire.Message(req.start, fwd, req.body, req.wire_body), timeout=180, method=req.method)
        except (OSError, httpwire.WireError) as e:
            body = f"door: the server did not answer: {e}".encode()
            wfile.write(httpwire.answer_bytes(502, "Bad Gateway", [["Content-Type", "text/plain"]], body).to_bytes())
            self.rec.add("peer", self.peer_role, self.role, request, {"status": 502, "headers": [], "body": {"$text": body.decode()}},
                         note="the door's own answer: the server did not answer")
            return
        out = [h for h in ans.headers if h[0].lower() not in ("connection", "keep-alive")] + [["Connection", "close"]]
        wfile.write(ans.to_bytes(out))
        self.rec.add("peer", self.peer_role, self.role, request, dict(status=ans.status, **captured(ans)))

    def close(self):
        self.server.shutdown()
        self.server.server_close()
