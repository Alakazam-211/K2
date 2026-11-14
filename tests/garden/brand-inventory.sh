#!/usr/bin/env bash
# Tests for scripts/garden/brand-inventory.py, against a small throwaway
# git tree, then one run on the real tree.
# Run: bash tests/garden/brand-inventory.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
INV="$HERE/scripts/garden/brand-inventory.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $1" >&2; exit 1; }

mkdir -p "$T/crates/a/src" "$T/crates/a/tests" "$T/cli/src" "$T/src/renderer" "$T/vendor/x" "$T/docs" "$T/scripts"
cat > "$T/crates/a/src/lib.rs" <<'RS'
// K2 in a comment, k2 msg too, and acme.k2.dev
const A: &str = "Open K2 Connect";
const B: &str = "run k2 msg <addr>";
fn c() -> String { format!("{l}.k2.dev") }
fn d() { std::env::var("K2_PORT"); header("X-K2-Client"); let t = "k2skn_"; let l = "dev.k2.daemon"; }
fn e() { home.join(".k2"); let x = K2_PORT_FILE; let y = K2Error::new(); }
#[cfg(test)]
mod tests {
    const T: &str = "K2 says k2 msg at b.k2.dev";
}
RS
printf 'fn t() { let s = "K2"; }\n' > "$T/crates/a/tests/it.rs"
printf '# K2 comment\necho "Usage: k2 msg"\necho "K2 is up at $HOME/.k2"\nX="${K2_PORT:-}"\n' > "$T/cli/src/a.sh"
printf 'echo "k2 msg"\n' > "$T/cli/k2"
printf 'export const t = "K2 settings"\n' > "$T/src/renderer/a.ts"
printf 'export const t = "K2"\n' > "$T/src/renderer/a.test.ts"
printf 'const V: &str = "K2 k2 msg k2.dev";\n' > "$T/vendor/x/lib.rs"
printf '# K2 docs\nRun `k2 msg`. See https://k2.dev.\n' > "$T/docs/a.md"
git -C "$T" init -q
git -C "$T" add -A

json="$(cd "$T" && python3 "$INV" --json)"
cell() { printf '%s' "$json" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d[sys.argv[1]][sys.argv[2]][sys.argv[3]])' "$1" "$2" "$3"; }
want() { local got; got="$(cell "$1" "$2" "$3")"; [ "$got" = "$4" ] || die "$1/$2/$3 is $got, wanted $4"; ok; }

# Rust: one line each; the comment line and the test module are not counted;
# K2_PORT_FILE and K2Error are not the product name.
want lines product-name rust 1
want lines cli-command rust 1
want lines zone rust 1
want lines home-path rust 1
# Names: K2_PORT and K2_PORT_FILE are two env names; one header, prefix, id.
want names env-name rust 2
want names header rust 1
want names token-prefix rust 1
want names rdns-id rust 1
# The CLI's parts are counted (not their comments); the built cli/k2 is not.
want lines product-name cli 1
want lines cli-command cli 1
want lines home-path cli 1
want names env-name cli 1
# Test files are their own area, whatever their folder.
want lines product-name tests 2
want lines product-name renderer 1
# Docs count every line, headings too. vendor/ is not counted at all.
want lines product-name docs 1
want lines cli-command docs 1
want lines zone docs 1
want names env-name all 2

# --lines shows the counted lines of one cell, with the file and line.
out="$(cd "$T" && python3 "$INV" --lines zone rust)"
[ "$out" = 'crates/a/src/lib.rs:4: fn c() -> String { format!("{l}.k2.dev") }' ] || die "--lines zone rust printed: $out"; ok
out="$(cd "$T" && python3 "$INV" --lines product-name)"
[ "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" = 6 ] || die "--lines product-name printed: $out"; ok
case "$out" in *vendor/*|*"cli/k2:"*) die "--lines shows a file that is not counted: $out" ;; esac; ok

# The table has a row per kind and names the commit.
git -C "$T" -c user.email=t@t -c user.name=t commit -q -m tree
out="$(cd "$T" && python3 "$INV")"
for k in product-name cli-command zone home-path env-name header token-prefix rdns-id; do
    printf '%s\n' "$out" | grep -q "^| $k |" || die "the table has no row for $k: $out"; ok
done
printf '%s\n' "$out" | grep -q "^K2 names at $(git -C "$T" rev-parse --short HEAD):" || die "the table does not name the commit: $out"; ok

# A wrong argument is refused with the usage.
set +e; out="$(cd "$T" && python3 "$INV" --lines nope 2>&1)"; code=$?; set -e
[ "$code" = 1 ] && case "$out" in *"usage: brand-inventory.py"*) ok ;; *) die "bad argument: exit $code: $out" ;; esac

# The real tree runs and finds the product name in the renderer.
n="$(cd "$HERE" && python3 "$INV" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["lines"]["product-name"]["renderer"])')"
[ "$n" -gt 0 ] || die "the real tree: no product name in the renderer"; ok

echo "brand-inventory tests: $pass passed"
