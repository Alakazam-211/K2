"""A fake GitHub REST API for the k2sign tests (test mode only).

  python3 -I fake_github.py <routes.json> <token file> <port file> <log file>

Serves GET <path> from routes.json ({"<path>": <json body>}), re-read on
every request so a test can change it; any other path is 404 Not Found.
Every request must carry "Authorization: Bearer <token>" (else 401), which
proves the signer sends its own token. Listens on 127.0.0.1, an OS-chosen
port written to <port file>. Each request is one line in <log file>.
"""
import http.server
import json
import sys

routes_path, token_path, port_path, log_path = sys.argv[1:5]


class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send_json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        with open(log_path, "a") as f:
            f.write(self.path + "\n")
        want = "Bearer " + open(token_path).read().strip()
        if self.headers.get("Authorization") != want:
            return self.send_json(401, {"message": "Bad credentials"})
        routes = json.load(open(routes_path))
        if self.path in routes:
            return self.send_json(200, routes[self.path])
        return self.send_json(404, {"message": "Not Found"})


srv = http.server.HTTPServer(("127.0.0.1", 0), H)
with open(port_path + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
import os
os.rename(port_path + ".tmp", port_path)
srv.serve_forever()
