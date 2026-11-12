#!/usr/bin/env bash
# Tests for scripts/regen.sh with stub generators: which commands it runs,
# in what order, with what flags, and what it does when one fails.
# Run: bash tests/garden/regen.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
REGEN="$HERE/scripts/regen.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

# A throwaway tree: stub cargo and node on PATH, a stub goldens test.
# Each stub logs its call and fails when its name is in $FAIL.
mkdir -p "$T/bin" "$T/root/tests/cli"
for tool in cargo node; do
    cat > "$T/bin/$tool" <<STUB
#!/usr/bin/env bash
echo "$tool \$*" >> "$T/calls"
key="$tool"; [ "$tool" = node ] && key="\$(basename -- "\$1")"
case " \${FAIL:-} " in *" \$key "*) exit 1 ;; esac
exit 0
STUB
    chmod +x "$T/bin/$tool"
done
cat > "$T/root/tests/cli/permission_off_golden.sh" <<STUB
echo "goldens bless=\${PERMISSION_GOLDEN_BLESS:-0}" >> "$T/calls"
case " \${FAIL:-} " in *" goldens "*) exit 1 ;; esac
exit 0
STUB

# run <want exit> <want text in output> <args...>; the calls land in $T/calls.
run() {
    local want="$1" text="$2" out code; shift 2
    : > "$T/calls"
    set +e; out="$(PATH="$T/bin:$PATH" bash "$REGEN" --root "$T/root" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
calls() {
    if [ "$(cat "$T/calls")" != "$1" ]; then echo "FAIL: calls were:" >&2; cat "$T/calls" >&2; echo "wanted:" >&2; echo "$1" >&2; exit 1; fi
    pass=$((pass + 1))
}

# Every generator, in order; the goldens are only checked.
run 0 "regen: done; everything generated is current."
calls "cargo run --locked -q -p k2-core --bin contract-gen
node scripts/sdk-server-dist.mjs
node scripts/gen-cli-explorer-catalog.mjs
node scripts/build-styles.mjs
goldens bless=0"

# --check passes --check to each and writes nothing itself.
run 0 "regen: done; everything generated is current." --check
calls "cargo run --locked -q -p k2-core --bin contract-gen -- --check
node scripts/sdk-server-dist.mjs --check
node scripts/gen-cli-explorer-catalog.mjs --check
node scripts/build-styles.mjs --check
goldens bless=0"

# --check: a stale step is named, and the steps after it still run.
FAIL="sdk-server-dist.mjs goldens" run 1 "regen: STALE: server SDK dist, CLI goldens. Run scripts/regen.sh and commit what it changes." --check
calls "cargo run --locked -q -p k2-core --bin contract-gen -- --check
node scripts/sdk-server-dist.mjs --check
node scripts/gen-cli-explorer-catalog.mjs --check
node scripts/build-styles.mjs --check
goldens bless=0"

# Not --check: the first failing generator stops everything after it.
FAIL="gen-cli-explorer-catalog.mjs" run 1 "regen: CLI explorer catalog"
calls "cargo run --locked -q -p k2-core --bin contract-gen
node scripts/sdk-server-dist.mjs
node scripts/gen-cli-explorer-catalog.mjs"
FAIL="cargo" run 1 "regen: contract-gen"
calls "cargo run --locked -q -p k2-core --bin contract-gen"

# Goldens that differ are never re-captured without being asked.
FAIL="goldens" run 1 "Read the difference; if it is what you meant, run: scripts/regen.sh --bless-goldens"
run 0 "regen: done; everything generated is current." --bless-goldens
calls "cargo run --locked -q -p k2-core --bin contract-gen
node scripts/sdk-server-dist.mjs
node scripts/gen-cli-explorer-catalog.mjs
node scripts/build-styles.mjs
goldens bless=1"

# --no-cargo skips step 1 and says so at the end.
run 0 "regen: done, but skipped: contract-gen (--no-cargo)." --no-cargo
calls "node scripts/sdk-server-dist.mjs
node scripts/gen-cli-explorer-catalog.mjs
node scripts/build-styles.mjs
goldens bless=0"

# Bad arguments run nothing.
run 2 "can't take --bless-goldens" --check --bless-goldens
calls ""
run 2 "unknown argument: --chekc" --chekc
calls ""

echo "regen tests: $pass passed"
