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
  test) exec cargo test "$@" ;;
  # The lane's targeted tests (the store, migrations, the composer, the
  # lockdown allowlist, the sync, the routes, the verify pages).
  targeted)
    rc=0
    cargo test -p k2-core --lib -- mail_forward db::tests "$@" || rc=1
    cargo test -p k2-daemon --lib -- mail::forward mail::sieve_user mail::sieve_lockdown email_notify::tests::v_ \
      email_notify::tests::webhook mail_routes routes::route_policy "$@" || rc=1
    exit $rc ;;
  *) echo "usage: $0 check|test|targeted [cargo args…]" >&2; exit 2 ;;
esac
