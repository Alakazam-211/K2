#!/usr/bin/env bash
# Run cargo for the hosted mail forwarding lane on a K2 compute node with
# every K2_* / K2SO_* variable cleared for the cargo process only (tests
# must never see the job's own K2 credentials).
#
#   k2 compute run <node> --sha <sha> --needs rust@stable,cmake@3,sys:sqlite3 \
#     -- bash tests/mail/forward-cargo.sh check|test [cargo args…]
set -uo pipefail
for v in $(compgen -e); do
  case "$v" in K2_* | K2SO_*) unset "$v" ;; esac
done
cmd="${1:-check}"
shift || true
case "$cmd" in
  check) exec cargo check --tests -p k2-core -p k2-daemon --message-format short "$@" ;;
  # Some integration targets (k2-core ptyd_backend) spawn the k2-ptyd
  # binary: build it first.
  test) cargo build -p k2-ptyd --bins && exec cargo test "$@" ;;
  # The lane's targeted tests (the store, migrations, the composer, the
  # lockdown allowlist, the sync, the routes, the verify pages).
  targeted)
    rc=0
    cargo test -p k2-core --lib -- mail_forward email_verify email_outcomes db::tests "$@" || rc=1
    cargo test -p k2-daemon --lib -- mail::forward mail::sieve_user mail::sieve_lockdown email_notify::tests::v_ \
      email_notify::tests::webhook email_notify::tests::n6_ email_notify::webhook mail::defaults \
      mail_routes routes::route_policy "$@" || rc=1
    exit $rc ;;
  # What a landing needs beyond the targeted tests: the generated contract is
  # fresh and the routes match it.
  contract)
    rc=0
    cargo run -p k2-core --bin contract-gen -- --check || rc=1
    cargo test -p k2-daemon --test contract_routes "$@" || rc=1
    exit $rc ;;
  landing)
    rc=0
    bash "$0" targeted "$@" || rc=1
    bash "$0" contract || rc=1
    exit $rc ;;
  # A real k2-daemon against a real, throwaway Stalwart on loopback; no real
  # mail (tests/mail/stalwart-forward-daemon-e2e.py).
  e2e)
    cargo build -p k2-daemon --bin k2-daemon || exit 1
    target="${CARGO_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')}"
    K2_E2E_DAEMON="$target/debug/k2-daemon" STALWART_PROBE=stalwart-forward-daemon-e2e.py \
      exec bash tests/mail/stalwart-sandbox-forward-gap.sh 0.16.25 ;;
  # A mailbox at the forward limit on real Stalwart, both versions in the fleet;
  # counts what arrives (tests/mail/stalwart-forward-many-probe.py). Needs no cargo.
  many-probe)
    STALWART_PROBE=stalwart-forward-many-probe.py \
      exec bash tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25 ;;
  # The same through K2's own routes and client, both versions
  # (tests/mail/stalwart-forward-many-e2e.py).
  many-e2e)
    cargo build -p k2-daemon --bin k2-daemon || exit 1
    target="${CARGO_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')}"
    K2_E2E_DAEMON="$target/debug/k2-daemon" STALWART_PROBE=stalwart-forward-many-e2e.py \
      exec bash tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25 ;;
  # The owner's numbers for how much a mailbox may forward, both versions: K2's
  # own routes and the throttle they write, real delivery, and K2's daily pass on
  # the webhook batches a real Stalwart signs (tests/mail/stalwart-forward-volume-e2e.py).
  volume-e2e)
    cargo build -p k2-daemon --bin k2-daemon || exit 1
    bin="$(cargo test -p k2-daemon --lib --no-run --message-format json | python3 -I -c 'import json,sys
for l in sys.stdin:
    try:
        m = json.loads(l)
    except ValueError:
        continue
    if m.get("executable") and m.get("profile", {}).get("test"):
        print(m["executable"])')"
    [ -x "$bin" ] || { echo "no k2-daemon test binary: $bin" >&2; exit 1; }
    target="${CARGO_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')}"
    FW_LIVE_TEST_BIN="$bin" K2_E2E_DAEMON="$target/debug/k2-daemon" STALWART_PROBE=stalwart-forward-volume-e2e.py \
      exec bash tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25 ;;
  # The default certificate and a change of primary on real Stalwart, both
  # versions, with real TLS handshakes (tests/mail/stalwart-default-cert-probe.py).
  default-cert)
    bin="$(cargo test -p k2-daemon --lib --no-run --message-format json | python3 -I -c 'import json,sys
for l in sys.stdin:
    try:
        m = json.loads(l)
    except ValueError:
        continue
    if m.get("executable") and m.get("profile", {}).get("test"):
        print(m["executable"])')"
    [ -x "$bin" ] || { echo "no k2-daemon test binary: $bin" >&2; exit 1; }
    K2_DC_TEST_BIN="$bin" STALWART_PROBE=stalwart-default-cert-probe.py \
      exec bash tests/mail/stalwart-sandbox-forward-gap.sh 0.16.10 0.16.25 ;;
  # Every real-Stalwart proof the forward limit rests on, one after the other.
  proofs)
    rc=0
    bash "$0" volume-e2e || rc=1
    bash "$0" many-probe || rc=1
    bash "$0" many-e2e || rc=1
    bash "$0" e2e || rc=1
    STALWART_PROBE=stalwart-forward-external-probe.py bash tests/mail/stalwart-sandbox-forward-gap.sh 0.16.25 || rc=1
    exit $rc ;;
  # Any of the above with its whole output kept in a file and printed at the end,
  # with the real exit code on the last line.
  logged)
    log="$(mktemp)"
    bash "$0" "$@" >"$log" 2>&1
    rc=$?
    cat "$log"
    rm -f "$log"
    echo "== forward-cargo.sh $*: exit $rc =="
    exit $rc ;;
  *) echo "usage: $0 check|test|targeted|contract|landing|e2e|many-probe|many-e2e|volume-e2e|default-cert|proofs|logged <cmd> [cargo args…]" >&2; exit 2 ;;
esac
