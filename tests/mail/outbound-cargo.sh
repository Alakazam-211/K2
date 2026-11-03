#!/usr/bin/env bash
# Run the hosted-mail outbound tests (attachments, Reply-To, forwarding a
# message) on a K2 compute node, with every K2_* / K2SO_* variable cleared for
# the cargo process only (tests must never see the job's own K2 credentials).
#
#   k2 compute run <node> --sha <sha> --needs rust@stable,cmake@3 -- \
#     env CARGO_BUILD_JOBS=4 tests/mail/outbound-cargo.sh check|targeted|integration|contract [filter…]
#
# Cargo's output goes to a file and only the result lines print: a pipe would
# hide cargo's exit code.
set -uo pipefail
for v in $(compgen -e); do
  case "$v" in K2_* | K2SO_*) unset "$v" ;; esac
done
log="$(mktemp -t k2-outbound-cargo.XXXXXX)"
trap 'rm -f "$log"' EXIT

report() { # exit code of the cargo run
  grep -E '^(test result|test .* FAILED|failures:$|    [a-z_:0-9]+$|error(\[|:)|warning: `|warning: unused|thread .* panicked)' "$log" | cut -c1-300
  if [ "$1" -ne 0 ]; then
    echo "--- last lines of the log ---"
    tail -60 "$log" | cut -c1-300
  fi
}

cmd="${1:-check}"
shift || true
rc=0
case "$cmd" in
  check)
    cargo check --tests -p k2-core -p k2-daemon --message-format short >"$log" 2>&1 || rc=$?
    grep -E '^[^ ]+:[0-9]+:[0-9]+: (error|warning)|^error|could not compile' "$log" | cut -c1-300
    echo "warnings: $(grep -cE ': warning' "$log")"
    ;;
  targeted)
    if [ "$#" -gt 0 ]; then filters=("$@"); else
      filters=(mail:: mail_routes routes::route_policy email_notify:: app_user_invites apps::data_files)
    fi
    cargo test -p k2-core --lib -- secret_paths db::tests skills:: workspace::skill_regen >"$log" 2>&1 || rc=$?
    report "$rc"
    if [ "$rc" -eq 0 ]; then
      cargo test -p k2-daemon --lib -- "${filters[@]}" >"$log" 2>&1 || rc=$?
      report "$rc"
    fi
    ;;
  integration)
    cargo build -p k2-ptyd --bins >"$log" 2>&1 || rc=$?
    report "$rc"
    if [ "$rc" -eq 0 ]; then
      cargo test -p k2-daemon --test mail_manage_integration --test cell_server_uds_integration >"$log" 2>&1 || rc=$?
      report "$rc"
    fi
    ;;
  contract)
    cargo run -q -p k2-core --bin contract-gen -- --check >"$log" 2>&1 || rc=$?
    tail -20 "$log" | cut -c1-300
    ;;
  *) echo "usage: $0 check|targeted|integration|contract [filter…]" >&2; exit 2 ;;
esac
echo "RESULT $cmd exit $rc"
exit "$rc"
