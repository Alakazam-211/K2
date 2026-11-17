#!/usr/bin/env bash
# The k5-cli binary is built by the workspace but not shipped yet (K2's
# terms for R1): no release, bundle, packaging or CLI-install script names
# it. This fails, naming the file and line, if one does.
#
# One exception, for the owner's "New k5 command (Beta)" switch (slice 10c):
# a listed Rust file may name the binary inside a guard, the lines from
# `// k5-cli guard: begin` to `// k5-cli guard: end`, and nowhere else. In
# a guard:
#   - a `const NAME: &str = "...k5-cli..."` is read only inside a guard of
#     that file (NAME on any line outside one fails);
#   - every function whose first line names `PathBuf` (it returns a path)
#     takes `on: bool` on that line, ends it with `{`, and its first line of
#     code is `if !on {`, so no path to the binary leaves while the switch
#     is off;
#   - guards pair up: no guard inside another, none left open.
# Other listed files (scripts, packaging, Tauri configs) still may not name
# it at all.
#
# Scanned: scripts/release.sh, scripts/package-*.sh, scripts/install-*.sh,
# scripts/provision-k2-server.sh, scripts/arch-release-asset.sh,
# scripts/sign-*.sh, scripts/windows-sign-after-release.sh, every file
# under packaging/, the Tauri bundle configs (src-tauri/tauri*.json), and
# the code that installs or stages the CLI (k2-client's cli_install,
# cli_copy and win_cli, k2-core's cli_stage, the root helper's cli_refresh, the app's
# Install CLI in src-tauri's settings). A listed file or glob that matches
# nothing fails too: a rename must update this list.
#
# It proves it bites first: on a scratch copy of those files it must pass,
# then fail once a copy names k5-cli, then fail once a listed file is gone.
# No cargo, no daemon, no network. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# One path or glob per line, from the repo root. A trailing /** means
# every file under that folder.
LISTED="scripts/release.sh
scripts/package-*.sh
scripts/install-*.sh
scripts/provision-k2-server.sh
scripts/arch-release-asset.sh
scripts/sign-*.sh
scripts/windows-sign-after-release.sh
packaging/**
src-tauri/tauri*.json
crates/k2-client/src/cli_install.rs
crates/k2-client/src/cli_copy.rs
crates/k2-client/src/win_cli.rs
crates/k2-core/src/cli_stage.rs
crates/k2-daemon/src/root_helper/cli_refresh.rs
src-tauri/src/commands/settings.rs"

# scan_rust <root> <file>: the guard rules above for one Rust file. Prints
# each problem; exit 1 when there is any.
scan_rust() {
    awk -v F="$2" '
        function report(msg) { print msg; bad = 1 }
        BEGIN { inreg = 0; bad = 0; want_on = 0 }
        /\/\/ k5-cli guard: begin/ {
            if (inreg) report("GUARD: " F ":" NR ": a guard begins inside another")
            inreg = 1; began = NR; next
        }
        /\/\/ k5-cli guard: end/ {
            if (!inreg) report("GUARD: " F ":" NR ": a guard ends that never began")
            if (want_on) report("GUARD: " F ":" fn_at ": the function ends the guard before its if !on {")
            inreg = 0; want_on = 0; next
        }
        {
            line = $0
            if (!inreg) {
                if (index(line, "k5-cli")) report("NAMES k5-cli: " F ":" NR ":" line)
                outside[NR] = line
                next
            }
            if (want_on && line !~ /^[ \t]*$/) {
                t = line; gsub(/^[ \t]+|[ \t]+$/, "", t)
                if (t != "if !on {") report("GUARD: " F ":" fn_at ": a function that returns a path must start with if !on {")
                want_on = 0
            }
            if (line ~ /^[ \t]*(pub(\([a-z]+\))? )?const [A-Z0-9_]+: &str = / && index(line, "k5-cli")) {
                name = line
                sub(/^[ \t]*(pub(\([a-z]+\))? )?const /, "", name)
                sub(/:.*/, "", name)
                consts[name] = 1
            }
            if (line ~ /^[ \t]*(pub(\([a-z]+\))? )?fn / && index(line, "PathBuf")) {
                fn_at = NR
                if (line !~ /on: bool/ || line !~ /[{][ \t]*$/) {
                    report("GUARD: " F ":" NR ": a function that returns a path must take on: bool and open on its first line")
                } else {
                    want_on = 1
                }
            }
        }
        END {
            if (inreg) report("GUARD: " F ":" began ": a guard that never ends")
            for (n in consts) {
                for (i in outside) {
                    if (outside[i] ~ ("(^|[^A-Za-z0-9_])" n "([^A-Za-z0-9_]|$)")) {
                        report("GUARD: " F ":" i ": " n " is read outside the guard")
                    }
                }
            }
            exit bad
        }
    ' "$1/$2"
}

# scan <root>: prints each file:line that names k5-cli outside a guard (any
# line, for a file that isn't Rust), each guard rule broken, and each listed
# entry that matches nothing; exit 1 when there is any.
scan() {
    local root="$1" entry f bad=0 matched hits
    local -a files
    while IFS= read -r entry; do
        files=()
        case "$entry" in
            */\*\*)
                if [ -d "$root/${entry%/\*\*}" ]; then
                    while IFS= read -r f; do files+=("$f"); done < <(cd "$root" && find "${entry%/\*\*}" -type f | LC_ALL=C sort)
                fi
                ;;
            *)
                matched="$(cd "$root" && compgen -G "$entry" || true)"
                if [ -n "$matched" ]; then
                    while IFS= read -r f; do files+=("$f"); done <<<"$matched"
                fi
                ;;
        esac
        if [ "${#files[@]}" -eq 0 ]; then
            echo "MISSING: $entry matches no file (update the list in tests/packaging/k5_cli_not_shipped.sh)"
            bad=1
            continue
        fi
        for f in "${files[@]}"; do
            case "$f" in
                *.rs)
                    scan_rust "$root" "$f" || bad=1
                    ;;
                *)
                    if hits="$(grep -n -F -- "k5-cli" "$root/$f")"; then
                        while IFS= read -r line; do echo "NAMES k5-cli: $f:$line"; done <<<"$hits"
                        bad=1
                    fi
                    ;;
            esac
        done
    done <<<"$LISTED"
    return "$bad"
}

pass=0
die() { echo "FAIL: $*" >&2; exit 1; }

# ── The check bites: a scratch copy of the listed files ────────────────
T="$(mktemp -d -t k5-not-shipped-XXXXXX)"
trap 'rm -rf "$T"' EXIT
copied=0
while IFS= read -r entry; do
    case "$entry" in
        */\*\*) list="$(cd "$ROOT" && find "${entry%/\*\*}" -type f)" ;;
        *) list="$(cd "$ROOT" && compgen -G "$entry" || true)" ;;
    esac
    [ -n "$list" ] || die "the repo has nothing for $entry (update the list)"
    while IFS= read -r f; do
        mkdir -p "$T/$(dirname "$f")"
        cp "$ROOT/$f" "$T/$f"
        copied=$((copied + 1))
    done <<<"$list"
done <<<"$LISTED"
[ "$copied" -ge 20 ] || die "only $copied files copied: the globs no longer match what they did"

out="$(scan "$T")" || die "the scratch copy fails before any change: $out"
pass=$((pass + 1))

echo 'cargo build --release -p k5-cli' >>"$T/scripts/release.sh"
want_line="$(wc -l <"$T/scripts/release.sh" | tr -d ' ')"
if out="$(scan "$T")"; then die "a release.sh that builds k5-cli passed"; fi
grep -qxF "NAMES k5-cli: scripts/release.sh:$want_line:cargo build --release -p k5-cli" <<<"$out" \
    || die "the failure doesn't name scripts/release.sh:$want_line: $out"
pass=$((pass + 1))
cp "$ROOT/scripts/release.sh" "$T/scripts/release.sh"

first_pkg="$(cd "$T" && find packaging -type f | LC_ALL=C sort | head -1)"
printf 'install -m755 target/release/k5-cli "$pkgdir/usr/bin/k5"\n' >>"$T/$first_pkg"
if out="$(scan "$T")"; then die "a packaging file that installs k5-cli passed"; fi
grep -q "^NAMES k5-cli: $first_pkg:" <<<"$out" || die "the failure doesn't name $first_pkg: $out"
pass=$((pass + 1))
cp "$ROOT/$first_pkg" "$T/$first_pkg"

printf '{"bundle": {"externalBin": ["binaries/k5-cli"]}}\n' >"$T/src-tauri/tauri.conf.json"
if out="$(scan "$T")"; then die "a Tauri config that bundles k5-cli passed"; fi
grep -q "^NAMES k5-cli: src-tauri/tauri.conf.json:1:" <<<"$out" || die "the failure doesn't name tauri.conf.json: $out"
pass=$((pass + 1))
cp "$ROOT/src-tauri/tauri.conf.json" "$T/src-tauri/tauri.conf.json"

# The guard (slice 10c). The real cli_install.rs has one, and the copy
# passed above with it.
CI="crates/k2-client/src/cli_install.rs"
grep -q '^// k5-cli guard: begin$' "$T/$CI" || die "$CI has no k5-cli guard to test"
fresh() { cp "$ROOT/$CI" "$T/$CI"; }

# A path naming the binary outside the guard.
printf 'const LEAK: &str = "/usr/bin/k5-cli";\n' >>"$T/$CI"
want_line="$(wc -l <"$T/$CI" | tr -d ' ')"
if out="$(scan "$T")"; then die "a k5-cli path outside the guard passed"; fi
grep -qxF "NAMES k5-cli: $CI:$want_line:const LEAK: &str = \"/usr/bin/k5-cli\";" <<<"$out" \
    || die "the failure doesn't name $CI:$want_line: $out"
pass=$((pass + 1)); fresh

# The guarded name, read outside the guard.
guarded="$(sed -n '/^\/\/ k5-cli guard: begin$/,/^\/\/ k5-cli guard: end$/p' "$T/$CI" | sed -n 's/^const \([A-Z0-9_]*\): &str = "k5-cli";$/\1/p')"
[ -n "$guarded" ] || die "no const names the binary inside the guard of $CI"
printf 'pub fn leak(dir: &Path) -> PathBuf {\n    dir.join(%s)\n}\n' "$guarded" >>"$T/$CI"
if out="$(scan "$T")"; then die "$guarded read outside the guard passed"; fi
grep -q "^GUARD: $CI:[0-9]*: $guarded is read outside the guard$" <<<"$out" || die "the failure doesn't say $guarded leaked: $out"
pass=$((pass + 1)); fresh

# A function in the guard that returns the path with no switch at all,
# then one that takes the switch but doesn't check it first.
for body in 'pub fn sneaky(dir: &Path) -> PathBuf {\n    dir.join(BINARY_FILE)\n}' \
            'pub fn sneaky(on: bool, dir: &Path) -> PathBuf {\n    let _ = on;\n    dir.join(BINARY_FILE)\n}'; do
    awk -v add="$body" '{ print } /^\/\/ k5-cli guard: begin$/ { gsub(/\\n/, "\n", add); print add }' "$ROOT/$CI" >"$T/$CI"
    if out="$(scan "$T")"; then die "an unguarded path function inside the guard passed: $body"; fi
    grep -q "^GUARD: $CI:[0-9]*: a function that returns a path must" <<<"$out" || die "the failure doesn't name the function: $out"
    pass=$((pass + 1))
done
fresh

# A guard left open (its end line gone).
grep -v '^// k5-cli guard: end$' "$ROOT/$CI" >"$T/$CI"
if out="$(scan "$T")"; then die "a guard that never ends passed"; fi
grep -q "^GUARD: $CI:[0-9]*: a guard that never ends$" <<<"$out" || die "the failure doesn't name the open guard: $out"
pass=$((pass + 1)); fresh

# The Linux stage has no guard: naming the binary there fails.
printf '    let _ = dir.join("k5-cli");\n' >>"$T/crates/k2-core/src/cli_stage.rs"
if out="$(scan "$T")"; then die "cli_stage.rs naming k5-cli passed"; fi
grep -q "^NAMES k5-cli: crates/k2-core/src/cli_stage.rs:" <<<"$out" || die "the failure doesn't name cli_stage.rs: $out"
pass=$((pass + 1))
cp "$ROOT/crates/k2-core/src/cli_stage.rs" "$T/crates/k2-core/src/cli_stage.rs"

out="$(scan "$T")" || die "the scratch copy fails after the guard checks were undone: $out"

rm "$T/crates/k2-core/src/cli_stage.rs"
if out="$(scan "$T")"; then die "a missing cli_stage.rs passed"; fi
grep -q "^MISSING: crates/k2-core/src/cli_stage.rs " <<<"$out" || die "the failure doesn't name the missing file: $out"
pass=$((pass + 1))

rm "$T"/scripts/sign-*.sh
if out="$(scan "$T")"; then die "no scripts/sign-*.sh passed"; fi
grep -q "^MISSING: scripts/sign-\*.sh " <<<"$out" || die "the failure doesn't name the empty glob: $out"
pass=$((pass + 1))

# ── The repo ───────────────────────────────────────────────────────────
if ! out="$(scan "$ROOT")"; then
    echo "$out" >&2
    die "k5-cli is not shipped yet: the files above must not name it"
fi
pass=$((pass + 1))

echo "k5_cli_not_shipped: $pass passed, 0 failed ($copied files scanned)"
