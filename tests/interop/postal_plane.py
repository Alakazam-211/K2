"""A Postal plane v1 for the interop harness (brand step C4): the routes a
K2 server's Postal loop calls (crates/k2-daemon/src/postal_net, through
postal-sdk's sans-IO `plane` module), kept in memory on 127.0.0.1, so two
servers of different brands can become friends and send each other mail
with no network. It relays and checks who may call what; it never opens or
checks a seal or a signature (the boxes do that, as with the real plane). `Plane(..., serve=False)` with `call()` runs the same
routes with no socket, for tests/interop/test_postal_plane.py.

What it keeps to, from postal-sdk's plane.rs and the daemon's calls:
- the box's Connect credential (a `k2c_` value in the server's
  tunnel.json, known to the plane by its label) gets a `pst_` session from
  `POST /session`, and the owner's `k2c_pd_` decide token from
  `POST /decide-token` (one at a time per label: `409 already_minted`);
- box calls (`/me`, pairs, respond, reveal, hold, events) take the `pst_`;
  confirm and revoke take the decide token only (`403 owner_required` for
  a `pst_`);
- SAS v2 pairs move requested, responded, revealed, then each side's
  signed confirmation; both accepting with the same transcript hash is
  `accepted`; a pair is listed from each side's view (`side`, `handle`,
  `peer`, `peer_key_pem`, `status` pending_in when that side must act);
- hold: first write wins on (from, client_id); a receipt per item (held,
  delivered on ACK, refused:<code>); `/hold/sent` answers the sender's;
- events are hints: hold, pair and receipt, per label, behind a cursor, in
  a long poll of at most 5 s;
- every answer carries `Postal-Api: 1`, and an error is
  `{"error": {"code", "message"}}`.

Each exchange is captured on the `postal` channel, from the role whose
credential made the call. Python stdlib only."""
import base64
import json
import secrets
import socketserver
import threading
import time
from urllib.parse import parse_qs, urlsplit

import httpwire
from capture import captured, redact_target

API = "/api/postal/v1"
ZONE = ".postal.bot"
MAX_WAIT = 5


def now_iso(offset=0):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + offset))


def label_of(address):
    """`handle::label.postal.bot` -> `label`, or None."""
    if not isinstance(address, str) or "::" not in address:
        return None
    host = address.split("::", 1)[1]
    return host[:-len(ZONE)] if host.endswith(ZONE) and len(host) > len(ZONE) else None


class Reply(Exception):
    """An error answer."""

    def __init__(self, status, code, message=""):
        super().__init__(code)
        self.status, self.code, self.message = status, code, message


REASONS = {200: "OK", 400: "Bad Request", 401: "Unauthorized", 403: "Forbidden", 404: "Not Found",
           409: "Conflict", 410: "Gone", 413: "Payload Too Large"}


class _Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


class Plane:
    def __init__(self, creds, rec, role_of_label, serve=True):
        self.creds = dict(creds)          # Connect credential -> label
        self.rec = rec
        self.role_of_label = dict(role_of_label)
        self.cv = threading.Condition()
        self.sessions = {}                # pst_ -> label
        self.owners = {}                  # k2c_pd_ -> label
        self.minted = {}                  # label -> its active k2c_pd_
        self.handles = {}                 # address -> {label, pem, typ}
        self.pairs = {}                   # id -> pair
        self.items = []                   # held items, oldest first
        self.seq = 0
        self.receipts = {}                # (from address, client_id) -> state
        self.events = []                  # (label, event), the cursor is an index
        self.server = None
        if serve:
            self._listen()

    def _listen(self):
        plane = self

        class Handler(socketserver.StreamRequestHandler):
            timeout = 120

            def handle(self):
                plane.serve(self.rfile, self.wfile)

        self.server = _Server(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        self.base = f"http://127.0.0.1:{self.port}"
        self.thread = threading.Thread(target=self.server.serve_forever, name="postal-plane", daemon=True)
        self.thread.start()

    def close(self):
        if self.server:
            self.server.shutdown()
            self.server.server_close()

    def call(self, method, target, bearer="", body=None):
        """One call with no socket (the plane's own tests): (status, body)."""
        raw = b"" if body is None else json.dumps(body).encode()
        headers = [["Authorization", f"Bearer {bearer}"]] if bearer else []
        req = httpwire.Message(f"{method} {target} HTTP/1.1", headers, raw)
        try:
            return self.route(req, bearer)
        except Reply as r:
            return r.status, {"error": {"code": r.code, "message": r.message}}

    # ── the wire ──

    def serve(self, rfile, wfile):
        try:
            req = httpwire.read_message(rfile, False)
        except httpwire.WireError as e:
            wfile.write(self._answer(400, {"error": {"code": "bad_request", "message": str(e)}}).to_bytes())
            return
        if req is None:
            return
        bearer = ""
        auth = req.header("authorization") or ""
        if auth.lower().startswith("bearer "):
            bearer = auth[7:].strip()
        try:
            status, body = self.route(req, bearer)
        except Reply as r:
            status, body = r.status, {"error": {"code": r.code, "message": r.message}}
        ans = self._answer(status, body)
        wfile.write(ans.to_bytes())
        role = self.role_of_label.get(self._who(bearer), "?")
        self.rec.add("postal", role, "plane",
                     dict(method=req.method, target=redact_target(req.target), **captured(req)),
                     dict(status=status, **captured(ans)))

    def _answer(self, status, body):
        raw = json.dumps(body).encode()
        return httpwire.answer_bytes(status, REASONS.get(status, "Error"),
                                     [["Content-Type", "application/json"], ["Postal-Api", "1"]], raw)

    def _who(self, bearer):
        with self.cv:
            return self.sessions.get(bearer) or self.owners.get(bearer) or self.creds.get(bearer)

    def _emit(self, label, event):
        self.events.append((label, event))
        self.cv.notify_all()

    def _box(self, bearer):
        label = self.sessions.get(bearer)
        if not label:
            raise Reply(401, "unauthorized", "a box call needs a pst_ session")
        return label

    def _owner(self, bearer):
        if bearer in self.sessions:
            raise Reply(403, "owner_required", "a box session can't decide; the owner's token can")
        label = self.owners.get(bearer)
        if not label:
            raise Reply(401, "unauthorized", "an owner call needs the owner's decide token")
        return label

    def _connect(self, bearer):
        label = self.creds.get(bearer)
        if not label:
            raise Reply(401, "unauthorized", "unknown Connect credential")
        return label

    def route(self, req, bearer):
        url = urlsplit(req.target)
        path, q = url.path, {k: v[-1] for k, v in parse_qs(url.query, keep_blank_values=True).items()}
        m = req.method
        body = req.json_or_none() if req.body else None
        if path.startswith("/api/postal/cli/"):
            # The owner's device grant: a person on the Postal website; not here.
            return 400, {"error": "unsupported_in_test_plane"}
        if not path.startswith(API + "/"):
            raise Reply(404, "not_found", path)
        p = path[len(API):]
        if m == "GET" and p == "/events":
            return 200, self.events_page(bearer, q)
        with self.cv:
            if m == "POST" and p == "/session":
                label = self._connect(bearer)
                token = "pst_" + secrets.token_hex(16)
                self.sessions[token] = label
                return 200, {"token": token, "label": label, "expires_at": now_iso(3600)}
            if m == "POST" and p == "/decide-token":
                label = self._connect(bearer)
                if label in self.minted:
                    raise Reply(409, "already_minted", "one decide token at a time")
                token = "k2c_pd_" + secrets.token_hex(16)
                self.minted[label] = token
                self.owners[token] = label
                return 200, {"token": token, "label": label}
            if m == "POST" and p == "/decide-token/revoke":
                label = self._connect(bearer)
                old = self.minted.pop(label, None)
                if old:
                    self.owners.pop(old, None)
                return 200, {"revoked": 1 if old else 0}
            if m == "PUT" and p == "/me":
                label = self._box(bearer)
                addr = (body or {}).get("addr")
                if label_of(addr) != label:
                    raise Reply(403, "not_your_handle", "the address is not on this box's label")
                if not (body or {}).get("public_key_pem"):
                    raise Reply(400, "bad_request", "public_key_pem")
                self.handles[addr] = {"label": label, "pem": body["public_key_pem"], "typ": body.get("typ")}
                return 200, {"ok": True, "addr": addr}
            if p == "/pairs" and m == "POST":
                return 200, self.pair_create(self._box(bearer), body or {})
            if p == "/pairs" and m == "GET":
                label = self._box(bearer)
                return 200, {"pairs": [self.view(x, label) for x in self.pairs.values() if label in (label_of(x["from"]), label_of(x["to"]))]}
            if p.startswith("/pairs/"):
                return 200, self.pair_route(m, p[len("/pairs/"):], bearer, body or {})
            if p == "/hold" and m == "POST":
                return 200, self.hold_put(self._box(bearer), body or {})
            if p == "/hold" and m == "GET":
                return 200, self.hold_list(self._box(bearer), q)
            if p in ("/hold/ack", "/hold/refuse") and m == "POST":
                return 200, self.hold_done(self._box(bearer), body or {}, p == "/hold/refuse")
            if p == "/hold/sent" and m == "GET":
                label = self._box(bearer)
                ids = [i for i in (q.get("client_ids") or "").split(",") if i]
                return 200, {cid: st for (frm, cid), st in self.receipts.items() if cid in ids and label_of(frm) == label}
        raise Reply(404, "not_found", f"{m} {p}")

    # ── pairs ──

    def pair_create(self, label, b):
        pid, frm, to = b.get("id"), b.get("from"), b.get("to")
        if label_of(frm) != label:
            raise Reply(403, "not_your_handle", "from is not on this box's label")
        if frm not in self.handles:
            raise Reply(404, "not_found", "from is not registered")
        if to not in self.handles:
            raise Reply(404, "not_found", "no such address")
        if not pid or pid in self.pairs:
            raise Reply(409, "pair_id_taken", "pair id")
        if b.get("sas_version") != 2 or not b.get("commitment") or not b.get("public_key_pem"):
            raise Reply(400, "bad_request", "sas_version 2, commitment and public_key_pem")
        self.pairs[pid] = {"id": pid, "from": frm, "to": to, "from_key": b["public_key_pem"], "to_key": None,
                           "from_typ": b.get("typ"), "to_typ": None, "commitment": b["commitment"], "nb": None, "na": None,
                           "step": "requested", "decided": None, "failed_reason": None, "confirmations": [],
                           "created_at": now_iso(), "decided_at": None, "expires_at": now_iso(3600)}
        self._pair_event(self.pairs[pid])
        return {"id": pid, "status": "pending_out", "existing": False}

    def _pair_event(self, x):
        for addr in (x["from"], x["to"]):
            v = self.view(x, label_of(addr))
            self._emit(label_of(addr), {"type": "pair", "id": x["id"], "status": v["status"], "step": x["step"]})

    def view(self, x, label):
        side = "from" if label_of(x["from"]) == label else "to"
        mine, peer = (x["from"], x["to"]) if side == "from" else (x["to"], x["from"])
        if x["decided"]:
            status = x["decided"]
        elif x["step"] == "requested":
            status = "pending_in" if side == "to" else "pending_out"
        elif x["step"] == "responded":
            status = "pending_in" if side == "from" else "pending_out"
        else:
            status = "pending_out" if any(c["side"] == side for c in x["confirmations"]) else "pending_in"
        return {
            "id": x["id"], "side": side, "handle": mine, "peer": peer,
            "peer_typ": x["to_typ"] if side == "from" else x["from_typ"],
            "peer_key_pem": x["to_key"] if side == "from" else x["from_key"],
            "direction": "out" if side == "from" else "in", "status": status, "step": x["step"],
            "sas_version": 2, "commitment": x["commitment"], "nb": x["nb"], "na": x["na"],
            "confirmed_by": [c["side"] for c in x["confirmations"]],
            "confirmations": [dict(c) for c in x["confirmations"]],
            "failed_reason": x["failed_reason"], "rekey_of": None, "rekey_pending": False,
            "created_at": x["created_at"], "decided_at": x["decided_at"], "expires_at": x["expires_at"],
        }

    def pair_route(self, m, rest, bearer, b):
        pid, _, action = rest.partition("/")
        x = self.pairs.get(pid)
        if m == "GET" and not action:
            label = self.sessions.get(bearer) or self.owners.get(bearer)
            if not label:
                raise Reply(401, "unauthorized", "")
            if not x or label not in (label_of(x["from"]), label_of(x["to"])):
                raise Reply(404, "not_found", "no such pair")
            return self.view(x, label)
        if m != "POST" or action not in ("respond", "reveal", "confirm", "revoke"):
            raise Reply(404, "not_found", f"{m} pairs/{rest}")
        label = self._owner(bearer) if action in ("confirm", "revoke") else self._box(bearer)
        if not x or label not in (label_of(x["from"]), label_of(x["to"])):
            raise Reply(404, "not_found", "no such pair")
        side = "from" if label_of(x["from"]) == label else "to"
        if action == "respond":
            if side != "to":
                raise Reply(403, "forbidden", "only the asked side responds")
            if x["step"] != "requested":
                raise Reply(409, "already_responded", "")
            if not b.get("public_key_pem") or not b.get("nb"):
                raise Reply(400, "bad_request", "public_key_pem and nb")
            x.update(to_key=b["public_key_pem"], nb=b["nb"], to_typ=b.get("typ"), step="responded")
        elif action == "reveal":
            if side != "from":
                raise Reply(403, "forbidden", "only the asking side reveals")
            if x["step"] == "requested":
                raise Reply(409, "not_responded", "")
            if x["step"] != "responded":
                raise Reply(409, "already_revealed", "")
            if not b.get("na"):
                raise Reply(400, "bad_request", "na")
            x.update(na=b["na"], step="revealed")
        elif action == "confirm":
            if b.get("side") != side:
                raise Reply(403, "forbidden", "the confirmation's side is not this owner's")
            if x["step"] != "revealed":
                raise Reply(409, "not_revealed", "")
            if x["decided"] or any(c["side"] == side for c in x["confirmations"]):
                raise Reply(409, "already_decided", "")
            for k in ("decision", "transcript_sha256", "sig"):
                if not isinstance(b.get(k), str) or not b[k]:
                    raise Reply(400, "bad_confirmation", k)
            x["confirmations"].append({"side": side, "decision": b["decision"], "transcript_sha256": b["transcript_sha256"],
                                       "sig": b["sig"], "at": now_iso()})
            cs = x["confirmations"]
            if b["decision"] == "mismatch":
                x.update(decided="failed", failed_reason="mismatch", decided_at=now_iso())
            elif b["decision"] != "accept":
                x.update(decided="rejected", decided_at=now_iso())
            elif len(cs) == 2:
                if cs[0]["transcript_sha256"] != cs[1]["transcript_sha256"]:
                    x.update(decided="failed", failed_reason="transcript_mismatch", decided_at=now_iso())
                else:
                    x.update(decided="accepted", decided_at=now_iso())
        else:  # revoke
            if x["decided"] != "accepted":
                raise Reply(409, "pair_not_accepted", "")
            x.update(decided="revoked", decided_at=now_iso())
        self._pair_event(x)
        return self.view(x, label)

    # ── hold ──

    def hold_put(self, label, b):
        frm, to, cid, sealed = b.get("from"), b.get("to"), b.get("client_id"), b.get("sealed")
        if label_of(frm) != label:
            raise Reply(403, "not_your_handle", "from is not on this box's label")
        if to not in self.handles:
            raise Reply(404, "not_found", "no such address")
        if b.get("v") != 2 or not isinstance(sealed, str) or not cid:
            raise Reply(400, "bad_sealed", "v 2, client_id and sealed")
        try:
            # Standard padded base64, as postal-sdk's b64 writes it.
            raw = base64.b64decode(sealed, validate=True)
        except ValueError:
            raise Reply(400, "bad_sealed", "sealed is not standard base64")
        if len(raw) < 61 or raw[0] != 2:
            raise Reply(400, "bad_sealed", "not HoldSeal-v2 framing")
        for it in self.items:
            if it["from"] == frm and it["client_id"] == cid:
                return {"id": it["id"], "expires_at": it["expires_at"], "duplicate": True}
        if (frm, cid) in self.receipts:
            return {"id": "gone", "expires_at": now_iso(), "duplicate": True}
        self.seq += 1
        it = {"id": f"h{self.seq:08d}{secrets.token_hex(4)}", "seq": self.seq, "from": frm, "to": to, "client_id": cid,
              "sealed": sealed, "size": len(raw), "created_at": now_iso(), "expires_at": now_iso(7 * 86400)}
        self.items.append(it)
        self.receipts[(frm, cid)] = "held"
        waiting = sum(1 for i in self.items if i["to"] == to)
        self._emit(label_of(to), {"type": "hold", "handle": to, "count": waiting})
        self._emit(label, {"type": "receipt", "handle": frm, "client_id": cid, "state": "held"})
        return {"id": it["id"], "expires_at": it["expires_at"], "duplicate": False}

    def hold_list(self, label, q):
        after = q.get("after")
        try:
            limit = max(1, min(100, int(q.get("limit") or 100)))
        except ValueError:
            raise Reply(400, "bad_request", "limit")
        after_seq = 0
        if after:
            hit = next((i for i in self.items if i["id"] == after), None)
            after_seq = hit["seq"] if hit else 0
        mine = [i for i in self.items if label_of(i["to"]) == label and i["seq"] > after_seq][:limit]
        return {"items": [{k: i[k] for k in ("id", "from", "to", "client_id", "sealed", "size", "created_at", "expires_at")} for i in mine],
                "next_after": None}

    def hold_done(self, label, b, refuse):
        ids = b.get("ids")
        if not isinstance(ids, list) or not 1 <= len(ids) <= 100:
            raise Reply(400, "bad_request", "1 to 100 ids")
        code = b.get("code") if refuse else None
        if refuse and code not in ("no_agent", "gated", "too_large"):
            raise Reply(400, "bad_request", "code")
        done = []
        for it in list(self.items):
            if it["id"] in ids and label_of(it["to"]) == label:
                self.items.remove(it)
                state = f"refused:{code}" if refuse else "delivered"
                self.receipts[(it["from"], it["client_id"])] = state
                self._emit(label_of(it["from"]), {"type": "receipt", "handle": it["from"], "client_id": it["client_id"], "state": state})
                done.append(it["id"])
        return {"refused": done} if refuse else {"acked": done}

    # ── events ──

    def events_page(self, bearer, q):
        with self.cv:
            label = self._box(bearer)
            cursor = q.get("cursor")
            if not cursor:
                return {"cursor": str(len(self.events)), "events": []}
            if not cursor.isdigit() or int(cursor) > len(self.events):
                raise Reply(410, "cursor_expired", "")
            try:
                wait = max(0, min(MAX_WAIT, int(q.get("wait") or 0)))
            except ValueError:
                raise Reply(400, "bad_request", "wait")
            start = int(cursor)
            deadline = time.time() + wait

            def mine():
                return [e for lab, e in self.events[start:] if lab == label]

            while not mine() and time.time() < deadline:
                self.cv.wait(timeout=max(0.05, deadline - time.time()))
            return {"cursor": str(len(self.events)), "events": mine()}
