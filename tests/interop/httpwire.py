"""HTTP/1.1 messages as bytes, for the interop harness (brand step C4): read
one request or answer, keep its head and body as they were on the wire, and
decode a copy of the body for the capture. Every connection carries one
exchange (`Connection: close`). Python stdlib only."""
import gzip
import json
import socket
import zlib

MAX_LINE = 65536
MAX_BODY = 32 * 1024 * 1024


class WireError(Exception):
    """A message that is not HTTP/1.1 as this reads it."""


class Message:
    """A request or an answer: its start line, its headers in order (as
    [name, value] pairs), and its body as framed on the wire."""

    def __init__(self, start, headers, body=b"", wire_body=None):
        self.start = start
        self.headers = [list(h) for h in headers]
        self.body = body
        self.wire_body = body if wire_body is None else wire_body

    def header(self, name):
        low = name.lower()
        for n, v in self.headers:
            if n.lower() == low:
                return v
        return None

    @property
    def method(self):
        return self.start.split(" ", 1)[0]

    @property
    def target(self):
        parts = self.start.split(" ")
        if len(parts) < 3:
            raise WireError(f"not a request line: {self.start!r}")
        return parts[1]

    @property
    def status(self):
        parts = self.start.split(" ")
        if len(parts) < 2 or not parts[1].isdigit():
            raise WireError(f"not a status line: {self.start!r}")
        return int(parts[1])

    def to_bytes(self, headers=None):
        hs = self.headers if headers is None else headers
        head = self.start + "\r\n" + "".join(f"{n}: {v}\r\n" for n, v in hs) + "\r\n"
        return head.encode("latin-1") + self.wire_body

    def decoded(self):
        """The body with its content encoding undone (None when it can't be)."""
        enc = (self.header("content-encoding") or "").lower().strip()
        try:
            if enc == "gzip":
                return gzip.decompress(self.body)
            if enc == "deflate":
                return zlib.decompress(self.body)
        except (OSError, zlib.error):
            return None
        return self.body

    def body_for_capture(self):
        """JSON as parsed, else {"$text": ...}, else {"$bytes": n}; None for no body."""
        raw = self.decoded()
        if raw is None:
            return {"$bytes": len(self.body)}
        if not raw:
            return None
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            return {"$bytes": len(raw)}
        try:
            return json.loads(text)
        except ValueError:
            return {"$text": text}

    def json_or_none(self):
        v = self.body_for_capture()
        if isinstance(v, dict) and set(v) <= {"$text", "$bytes"}:
            return None
        return v


def _line(rfile):
    line = rfile.readline(MAX_LINE + 1)
    if len(line) > MAX_LINE:
        raise WireError("a head line over 64 KiB")
    return line


def _exact(rfile, n):
    if n > MAX_BODY:
        raise WireError(f"a body of {n} bytes")
    data = rfile.read(n)
    if len(data) != n:
        raise WireError(f"the body ended after {len(data)} of {n} bytes")
    return data


def _chunked(rfile):
    """(payload, wire bytes) of a chunked body."""
    payload, wire = [], []
    while True:
        size_line = _line(rfile)
        if not size_line:
            raise WireError("the chunked body ended early")
        wire.append(size_line)
        size = int(size_line.split(b";", 1)[0].strip() or b"0", 16)
        if size == 0:
            while True:
                trailer = _line(rfile)
                wire.append(trailer)
                if trailer in (b"\r\n", b"\n", b""):
                    break
            return b"".join(payload), b"".join(wire)
        chunk = _exact(rfile, size)
        payload.append(chunk)
        wire.append(chunk)
        wire.append(_line(rfile))


def read_message(rfile, is_response, method=None):
    """One message from `rfile`, or None when the peer closed first."""
    start = _line(rfile)
    if not start:
        return None
    start = start.decode("latin-1").rstrip("\r\n")
    headers = []
    while True:
        line = _line(rfile)
        if not line:
            raise WireError("the head ended early")
        if line in (b"\r\n", b"\n"):
            break
        name, sep, value = line.decode("latin-1").rstrip("\r\n").partition(":")
        if not sep:
            raise WireError(f"a header line with no colon: {line!r}")
        headers.append([name.strip(), value.strip()])
    msg = Message(start, headers)
    te = (msg.header("transfer-encoding") or "").lower()
    cl = msg.header("content-length")
    if is_response and (method == "HEAD" or msg.status in (204, 304) or 100 <= msg.status < 200):
        body, wire = b"", b""
    elif "chunked" in te:
        body, wire = _chunked(rfile)
    elif cl is not None:
        if not cl.strip().isdigit():
            raise WireError(f"a Content-Length that is not a number: {cl!r}")
        body = wire = _exact(rfile, int(cl))
    elif is_response:
        body = wire = rfile.read(MAX_BODY + 1)
        if len(body) > MAX_BODY:
            raise WireError("an answer over 32 MiB")
    else:
        body = wire = b""
    msg.body, msg.wire_body = body, wire
    return msg


def exchange(addr, msg, timeout=60, method=None):
    """Send one request to `addr` on its own connection; its answer."""
    with socket.create_connection(addr, timeout=timeout) as s:
        s.sendall(msg.to_bytes())
        with s.makefile("rb") as rfile:
            ans = read_message(rfile, True, method or msg.method)
    if ans is None:
        raise WireError(f"{addr[0]}:{addr[1]} closed with no answer to {msg.start}")
    return ans


def answer_bytes(status, reason, headers, body):
    """An answer to write: `headers` plus Content-Length and Connection: close."""
    hs = [list(h) for h in headers] + [["Content-Length", str(len(body))], ["Connection", "close"]]
    return Message(f"HTTP/1.1 {status} {reason}", hs, body)
