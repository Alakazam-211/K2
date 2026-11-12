#!/bin/sh
# Self-test of scripts/pack-checks/carrier-row.sh: three stand-in packs in a scratch folder.
#   - keeps a carrier, no row          -> exit 1
#   - keeps a carrier, the row in tests -> exit 0
#   - no carrier (`false`)              -> exit 0
set -eu
HERE=$(cd "$(dirname "$0")/.." && pwd)
CHECK="$HERE/scripts/pack-checks/carrier-row.sh"
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mk() { mkdir -p "$T/$1/src" "$T/$1/tests"; }
mk norow
printf 'impl Pack for P {\n    fn wants_carrier(&self) -> bool {\n        true\n    }\n}\n' > "$T/norow/src/lib.rs"
mk withrow
cp "$T/norow/src/lib.rs" "$T/withrow/src/lib.rs"
printf '#[test]\nfn row() { carrier_judge(pack(), &corpus()).unwrap(); }\n' > "$T/withrow/tests/carrier.rs"
mk nocarrier
printf 'impl Pack for P {\n    fn wants_carrier(&self) -> bool { false }\n}\n' > "$T/nocarrier/src/lib.rs"
fail=0
expect() {
  want=$1; dir=$2
  set +e; sh "$CHECK" "$T/$dir" >/dev/null; got=$?; set -e
  if [ "$got" = "$want" ]; then echo "ok   $dir: exit $got"; else echo "FAIL $dir: exit $got, want $want"; fail=1; fi
}
expect 1 norow
expect 0 withrow
expect 0 nocarrier
exit $fail
