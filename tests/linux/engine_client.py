#!/usr/bin/env python3
"""Talk to k2-app-engine like the daemon does (tests only, inside the
throwaway VM of tests/linux/apps_guarded_e2e.sh).

    engine_client.py '<request json>'      → prints the answer line
    engine_client.py --raw '<line>'        → sends the line as-is

Exit 0 when the answer is ok, 1 when it is a refusal, 2 when the socket
couldn't be reached (the error on stderr)."""
import json
import socket
import sys

SOCK = "/run/k2app/engine.sock"


def main() -> int:
    args = sys.argv[1:]
    raw = False
    if args and args[0] == "--raw":
        raw = True
        args = args[1:]
    line = args[0] if raw else json.dumps(json.loads(args[0]))
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(SOCK)
    except OSError as e:
        print(f"connect: {e.__class__.__name__}: {e}", file=sys.stderr)
        return 2
    s.sendall(line.encode() + b"\n")
    s.shutdown(socket.SHUT_WR)
    buf = b""
    while True:
        b = s.recv(65536)
        if not b:
            break
        buf += b
    out = buf.decode().strip()
    print(out)
    try:
        return 0 if json.loads(out).get("ok") else 1
    except Exception:
        return 1


if __name__ == "__main__":
    sys.exit(main())
