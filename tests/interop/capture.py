"""What the interop harness captures, and what it keeps out (brand step
C4): one session's exchanges in order, a secret's value replaced by its
class (a token keeps its prefix, `<token:pst_>`), the owner token out of
every query. The format is in wire_compare.py. Python stdlib only."""
import json
import re
import threading

import wire_compare

SECRET_KEYS = {"password", "valueB64", "confirmToken", "token", "decide_token", "secret"}


class Recorder:
    """Every exchange of one session, in the order they ended."""

    def __init__(self):
        self.lock = threading.Lock()
        self.rows = []
        self.seq = 0
        self.step = "start"

    def add(self, channel, frm, to, request, answer, poll=False, note=None):
        with self.lock:
            self.seq += 1
            row = {"kind": "exchange", "seq": self.seq, "step": self.step, "channel": channel,
                   "from": frm, "to": to, "poll": poll, "request": request, "answer": answer}
            if note:
                row["note"] = note
            self.rows.append(row)

    def write(self, path, meta):
        with open(path, "w", encoding="utf-8") as f:
            f.write(json.dumps(dict(meta, kind="session"), sort_keys=True) + "\n")
            with self.lock:
                for r in self.rows:
                    f.write(json.dumps(r, sort_keys=True) + "\n")


def redact_body(v):
    """A body as captured: a secret's value is its class, never its bytes."""
    if isinstance(v, dict):
        return {k: (redact_value(x) if k in SECRET_KEYS and isinstance(x, str) else redact_body(x)) for k, x in v.items()}
    if isinstance(v, list):
        return [redact_body(x) for x in v]
    return v


def redact_value(s):
    return wire_compare.whole_class(s) or "<redacted>"


def redact_headers(headers):
    out = []
    for n, v in headers:
        low = n.lower()
        if low in ("authorization", "proxy-authorization"):
            scheme, _, rest = v.partition(" ")
            v = f"{scheme} {redact_value(rest.strip())}"
        elif low in ("cookie", "set-cookie"):
            v = "; ".join((p.strip().split("=", 1)[0] + "=<v>") if "=" in p else p.strip() for p in v.split(";"))
        out.append([n, v])
    return out


def redact_target(target):
    return re.sub(r"([?&]token=)[^&]*", r"\1<redacted>", target)


def captured(msg):
    """A parsed message as the capture keeps it."""
    return {"headers": redact_headers(msg.headers), "body": redact_body(msg.body_for_capture())}
