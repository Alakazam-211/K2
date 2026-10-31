#!/usr/bin/env bash
# Run tests/mail/stalwart-forward-gap-probe.py against a throwaway Stalwart
# (copied from the API-key lane's stalwart-sandbox-probe.sh, a1746de6b;
# only the probe it runs differs).
#
#   tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25
#
# For each version: downloads K2's pinned tarball (sha256-checked against
# crates/k2-root-proto/src/stalwart.rs), starts it as the CURRENT user in a
# temp dir (no root, no systemd), completes bootstrap with
# x:Bootstrap/set, moves every listener onto 127.0.0.1 high ports
# (nothing listens on a public interface after the first restart), runs
# the probe, then kills Stalwart and deletes the temp dir. No mail is
# sent. Meant for a disposable Linux x86_64 machine (a K2 compute job);
# never a mail box with real mailboxes.
#
# Secrets (the recovery admin and the provisioned admin) live only in
# this process's environment and the temp dir; never in argv or output.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# STALWART_PROBE: another probe script in this folder (default: the forwarding gap).
PROBE="$HERE/${STALWART_PROBE:-stalwart-forward-gap-probe.py}"
[ "$#" -ge 1 ] || { echo "usage: $0 <stalwart version>..." >&2; exit 2; }

sha_for() {
  case "$1" in
    0.16.10) echo 3ec4ab7eff49f61280f2fe2e4f9645ce5308d1840286ef0c0437524f92ac6a33 ;;
    0.16.20) echo 55184f166f89a0918c6523bb70be300370b63934357731b199dc26a12ca15abf ;;
    0.16.25) echo 8ba9cc0ea2795121df5c1264af41e82353c5a3d6860161c4ab70817906d912d3 ;;
    *) echo "no pinned sha256 for Stalwart $1" >&2; return 1 ;;
  esac
}

wait_http() { # url, seconds
  local i
  for ((i = 0; i < $2 * 2; i++)); do
    if curl -s -o /dev/null --max-time 2 "$1/jmap/session"; then return 0; fi
    sleep 0.5
  done
  return 1
}

run_one() {
  local v="$1" sha work bin pid rc=0
  sha="$(sha_for "$v")"
  work="$(mktemp -d)"
  echo "== Stalwart $v =="
  curl -fsSL --retry 3 -o "$work/s.tgz" \
    "https://github.com/Alakazam-211/stalwart/releases/download/v$v/stalwart-x86_64-unknown-linux-gnu.tar.gz"
  echo "$sha  $work/s.tgz" | sha256sum -c --quiet -
  mkdir -p "$work/x" "$work/etc" "$work/data"
  tar -xzf "$work/s.tgz" -C "$work/x"
  bin="$work/x/stalwart"
  [ -f "$bin" ] || { echo "no 'stalwart' member in the tarball:" >&2; tar -tzf "$work/s.tgz" >&2; return 1; }
  chmod 0755 "$bin"

  STALWART_RECOVERY_ADMIN="admin:$(python3 -I -c 'import secrets;print(secrets.token_urlsafe(24))')"
  export STALWART_RECOVERY_ADMIN
  "$bin" --config "$work/etc/config.json" >"$work/log.1" 2>&1 &
  pid=$!
  wait_http http://127.0.0.1:8080 60 || { tail -20 "$work/log.1"; kill "$pid"; return 1; }

  # Bootstrap (K2's bootstrap_complete shape) + record the admin to a 0600 file.
  WORK="$work" python3 -I - <<'PY' || { kill "$pid"; return 1; }
import base64, json, os, urllib.request
work = os.environ["WORK"]
user, pw = os.environ["STALWART_RECOVERY_ADMIN"].split(":", 1)
auth = "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()
def req(url, body=None):
    r = urllib.request.Request(url, data=json.dumps(body).encode() if body is not None else None)
    r.add_header("Authorization", auth)
    if body is not None:
        r.add_header("Content-Type", "application/json")
    return json.loads(urllib.request.urlopen(r, timeout=30).read())
base = "http://127.0.0.1:8080"
s = req(base + "/jmap/session")
api = "/" + s["apiUrl"].split("://", 1)[1].split("/", 1)[1] if s["apiUrl"].startswith("http") else s["apiUrl"]
acct = s["primaryAccounts"].get("urn:stalwart:jmap") or next(iter(s["accounts"]))
r = req(base + api, {"using": ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"], "methodCalls": [["x:Bootstrap/set", {
    "accountId": acct,
    "update": {"singleton": {
        "serverHostname": "mail.example.test", "defaultDomain": "example.test",
        "requestTlsCertificate": False, "generateDkimKeys": True,
        "dataStore": {"@type": "RocksDb", "path": work + "/data/data"}}}}, "0"]]})
up = r["methodResponses"][0][1].get("updated", {}).get("singleton") or {}
if not up.get("username") or not up.get("secret"):
    raise SystemExit("Bootstrap/set gave no admin: " + json.dumps(r)[:300])
fd = os.open(work + "/admin", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, f"{up['username']}:{up['secret']}".encode())
os.close(fd)
print("bootstrap done; admin", up["username"])
PY
  kill "$pid"; wait "$pid" 2>/dev/null || true

  # First normal start: the default listeners (some can't bind as a
  # normal user). Move every listener to 127.0.0.1:2<port> and restart.
  "$bin" --config "$work/etc/config.json" >"$work/log.2" 2>&1 &
  pid=$!
  if ! wait_http http://127.0.0.1:8080 60; then
    echo "normal mode did not come up on :8080:"; tail -30 "$work/log.2"; kill "$pid" 2>/dev/null; return 1
  fi
  STALWART_PROBE_ADMIN="$(cat "$work/admin")" python3 -I - <<'PY' || { kill "$pid"; return 1; }
import base64, json, os, urllib.request
user, pw = os.environ["STALWART_PROBE_ADMIN"].split(":", 1)
auth = "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()
def req(url, body=None):
    r = urllib.request.Request(url, data=json.dumps(body).encode() if body is not None else None)
    r.add_header("Authorization", auth)
    if body is not None:
        r.add_header("Content-Type", "application/json")
    return json.loads(urllib.request.urlopen(r, timeout=30).read())
base = "http://127.0.0.1:8080"
s = req(base + "/jmap/session")
api = "/" + s["apiUrl"].split("://", 1)[1].split("/", 1)[1] if s["apiUrl"].startswith("http") else s["apiUrl"]
acct = s["primaryAccounts"].get("urn:stalwart:jmap") or next(iter(s["accounts"]))
def call(m, a):
    a["accountId"] = acct
    return req(base + api, {"using": ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"], "methodCalls": [[m, a, "0"]]})["methodResponses"][0][:2]
n, g = call("x:NetworkListener/get", {"ids": None})
upd = {}
for l in g["list"]:
    port = int(next(iter(l["bind"])).rsplit(":", 1)[1])
    new = 18080 if l["name"] == "http" else 20000 + port
    upd[l["id"]] = {"bind": {f"127.0.0.1:{new}": True}}
    print("listener", l["name"], port, "->", new)
n, r = call("x:NetworkListener/set", {"update": upd})
bad = r.get("notUpdated")
if n != "x:NetworkListener/set" or bad:
    raise SystemExit(f"listener move failed: {n} {json.dumps(r)[:300]}")
PY
  kill "$pid"; wait "$pid" 2>/dev/null || true
  unset STALWART_RECOVERY_ADMIN
  "$bin" --config "$work/etc/config.json" >"$work/log.3" 2>&1 &
  pid=$!
  if ! wait_http http://127.0.0.1:18080 60; then
    echo "did not come up on 127.0.0.1:18080:"; tail -30 "$work/log.3"; kill "$pid" 2>/dev/null; return 1
  fi
  echo "Stalwart $v up on 127.0.0.1:18080 (loopback only)"
  STALWART_PROBE_ADMIN="$(cat "$work/admin")" python3 -I "$PROBE" --url http://127.0.0.1:18080 --domain example.test || rc=$?
  if [ "$rc" -ne 0 ] && [ -n "${STALWART_PROBE_LOG_TAIL:-}" ]; then
    echo "== Stalwart $v: last $STALWART_PROBE_LOG_TAIL log lines (sieve / queue / delivery) =="
    grep -iE "sieve|queue|deliver|redirect|smtp\\.|spam|loop" "$work/log.3" | tail -n "$STALWART_PROBE_LOG_TAIL" || true
  fi
  kill "$pid"; wait "$pid" 2>/dev/null || true
  rm -rf "$work"
  echo "== Stalwart $v: probe exit $rc =="
  return "$rc"
}

overall=0
for v in "$@"; do
  run_one "$v" || overall=1
done
exit "$overall"
