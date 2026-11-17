#!/usr/bin/env bash
# The k5-cli binary ships, off by default (K2's terms for R1 slice
# 10c(iii)): the macOS app carries it at Contents/MacOS/k5-cli (universal),
# and the Linux daemon artifacts and packages carry it beside the daemon.
# Only the owner's "New k5 command (Beta)" switch points the k2 and k5
# links at it, and that is decided in ONE place: the guard in k2-client's
# cli_install.rs. This fails, naming the file and line, when:
#
# 1. Install code (the Rust that installs or stages the CLI) names the
#    binary outside a guard, the lines from `// k5-cli guard: begin` to
#    `// k5-cli guard: end`. In a guard:
#      - a `const NAME: &str = "...k5-cli..."` is read only inside a guard
#        of that file (NAME on any line outside one fails);
#      - every function whose first line names `PathBuf` (it returns a
#        path) takes `on: bool` on that line, ends it with `{`, and its
#        first line of code is `if !on {`, so no path to the binary leaves
#        while the switch is off;
#      - guards pair up: no guard inside another, none left open.
# 2. A shipping file (the release, build, package and install scripts,
#    packaging/, the daemon-binaries workflow) makes a link to the binary or
#    puts it under another name. A line (comments aside) that names k5-cli,
#    or a variable or shell function that holds or returns it (followed
#    through every shipping file), must not run `ln` or `symlink`, and must
#    not name a path ending in /k2 or /k5 (cli/k2, the script it reads its
#    version from, aside). The binary keeps its own name everywhere.
# 3. A shipping file stops shipping it: each line in SHIPS below must still
#    be in its file.
# 4. A Windows file names it at all: Windows keeps the script (Authenticode
#    for new programs isn't in place).
# 5. A Tauri bundle config names it (it is not a Tauri sidecar).
# A listed file or glob that matches nothing fails too: a rename must
# update these lists.
#
# It proves it bites first: on a scratch copy of those files it must pass,
# then fail for each broken rule. No cargo, no daemon, no network. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# One path or glob per line, from the repo root. A trailing /** means
# every file under that folder.
INSTALL_RS="crates/k2-client/src/cli_install.rs
crates/k2-client/src/cli_copy.rs
crates/k2-client/src/win_cli.rs
crates/k2-core/src/cli_stage.rs
crates/k2-daemon/src/root_helper/cli_refresh.rs
src-tauri/src/commands/settings.rs"
SHIPPING="scripts/release.sh
scripts/lib/build-binaries.sh
scripts/macos-daemon-helper-app.sh
scripts/build-app.sh
scripts/dev-build.sh
scripts/package-*.sh
scripts/install-*.sh
scripts/provision-k2-server.sh
scripts/arch-release-asset.sh
scripts/sign-*.sh
scripts/ci/build-linux-bins.sh
scripts/ci/compute_release.py
scripts/lib/compute-release.sh
scripts/lib/ci-client-release.sh
scripts/lib/check-k5-cli.py
.github/workflows/daemon-binaries.yml
packaging/**"
WINDOWS="src-tauri/tauri.windows.conf.json
src-tauri/windows/**
scripts/windows/**
scripts/windows-*.sh
crates/k2-client/src/win_cli.rs
crates/k2-client/src/win_autostart.rs"
TAURI="src-tauri/tauri*.json"

# file<TAB>a line it must still carry (fixed text, leading spaces trimmed).
SHIPS="scripts/lib/build-binaries.sh	K2_MAC_K5_CLI_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
scripts/lib/build-binaries.sh	cp \"\$out/k5-universal/k5-cli\" \"\$app/Contents/MacOS/k5-cli\" || return 1
scripts/macos-daemon-helper-app.sh	K2_K5_CLI_IDENTIFIER=\"dev.k2.app.k5-cli\"
scripts/macos-daemon-helper-app.sh	--identifier \"\$K2_K5_CLI_IDENTIFIER\" \\
scripts/release.sh	k2_mac_k5_cli_targets_check || exit 1
scripts/release.sh	k2_k5_cli_sign \"target/release/bundle/macos/K2.app\" \"\$SIGNING_IDENTITY\" || exit 1
scripts/release.sh	k2_k5_cli_verify_signed \"target/release/bundle/macos/K2.app\" || exit 1
scripts/release.sh	k2_k5_cli_verify_shipped \"K2.app\" || exit 1
scripts/release.sh	k2_k5_cli_verify_tarball \"target/release/bundle/macos/K2.app.tar.gz\" || exit 1
scripts/ci/build-linux-bins.sh	cargo build --release --target \"\$TRIPLE\" -p k5-cli --bin k5-cli
scripts/ci/build-linux-bins.sh	K2_BI_FILES=\"\$DAEMON \$NODE \$HELPER \$ENGINE \$K5CLI\" \\
scripts/lib/compute-release.sh	K2CR_LINUX_BINS=\"k2-daemon k2-node k2-root-helper k2-app-engine k5-cli\"
.github/workflows/daemon-binaries.yml	\"\${{ matrix.cli_asset }}.sha256\" \\
scripts/package-daemon-deb.sh	install -D -m 0755 \"\$K5_BIN\" \"\$PKGROOT/usr/bin/k5-cli\"
packaging/arch/PKGBUILD	install -Dm755 \"\${target}/release/k5-cli\" \"\${pkgdir}/usr/bin/k5-cli\"
scripts/install-daemon.sh	K5_PATH=\"\$K2_BIN_DIR/k5-cli\""

# files_of <root> <list>: every file the list matches, one per line, from
# <root>; prints "MISSING: <entry>" for an entry that matches nothing.
files_of() {
    local root="$1" entry f matched
    while IFS= read -r entry; do
        [ -n "$entry" ] || continue
        case "$entry" in
            */\*\*)
                if [ -d "$root/${entry%/\*\*}" ]; then
                    matched="$(cd "$root" && find "${entry%/\*\*}" -type f | LC_ALL=C sort)"
                else
                    matched=""
                fi
                ;;
            *) matched="$(cd "$root" && compgen -G "$entry" || true)" ;;
        esac
        if [ -z "$matched" ]; then
            echo "MISSING: $entry matches no file (update the lists in tests/packaging/k5_cli_shipped_off_by_default.sh)"
            continue
        fi
        while IFS= read -r f; do echo "FILE: $f"; done <<<"$matched"
    done <<<"$2"
}

# scan_rust <root> <file>: rule 1 for one Rust file. Prints each problem;
# exit 1 when there is any.
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

# scan_shipping <root> <file>...: rule 2 over every shipping file at once
# (a variable or function set in one is followed into the others).
scan_shipping() {
    python3 -I - "$@" <<'PY'
import os, re, sys
root, files = sys.argv[1], sys.argv[2:]
LINK = re.compile(r"(^|[^\w.-])ln(\s|$)|symlink")
RENAME = re.compile(r"(?<!cli)/(k2|k5)(?=[\"'\s;)|}]|$)")
ASSIGN = re.compile(r"(?:^|[\s;(])(?:local\s+|export\s+|readonly\s+)?([A-Za-z_]\w*)=(\"(?:[^\"\\]|\\.)*\"|'[^']*'|\S*)")
FUNC = re.compile(r"^([A-Za-z_][\w-]*)\s*\(\)\s*\{")
PRINTS = re.compile(r"^\s*(printf|echo)\s")
code = []  # (file, line number, text, the function it is in, or None)
for f in files:
    fn = None
    for n, l in enumerate(open(os.path.join(root, f), encoding="utf-8", errors="replace").read().split("\n"), 1):
        m = FUNC.match(l)
        if m:
            fn = m.group(1)
        if not l.lstrip().startswith("#"):
            code.append((f, n, l, fn))
        if l == "}":
            fn = None
# What holds the binary's name: a variable set to a value that names it
# (inside a function, only for that function), and a function that prints
# it (a path helper). Followed until nothing new turns up.
glob_vars, fn_vars, funcs = set(), set(), set()
def names(text, f, fn):
    if "k5-cli" in text:
        return True
    for v in glob_vars | {v for (ff, g, v) in fn_vars if ff == f and g == fn}:
        if re.search(r"\$\{?" + re.escape(v) + r"(?!\w)", text):
            return True
    return any(re.search(r"(?<![\w-])" + re.escape(x) + r"(?![\w-])", text) for x in funcs)
grew = True
while grew:
    grew = False
    for f, n, l, fn in code:
        for v, val in ASSIGN.findall(l):
            if names(val, f, fn):
                key = v if fn is None else (f, fn, v)
                pool = glob_vars if fn is None else fn_vars
                if key not in pool:
                    pool.add(key); grew = True
        if fn and fn not in funcs and PRINTS.match(l) and names(l, f, fn):
            funcs.add(fn); grew = True
bad = 0
for f, n, l, fn in code:
    if not names(l, f, fn):
        continue
    if LINK.search(l):
        print(f"LINKS k5-cli: {f}:{n}:{l}"); bad = 1
    if RENAME.search(l):
        print(f"RENAMES k5-cli: {f}:{n}:{l}"); bad = 1
sys.exit(bad)
PY
}

# scan <root>: every rule over the files under <root>. Prints each problem;
# exit 1 when there is any.
scan() {
    local root="$1" bad=0 out f file want
    local -a ship=()
    # 1. Install code.
    out="$(files_of "$root" "$INSTALL_RS")"
    grep '^MISSING: ' <<<"$out" && bad=1
    while IFS= read -r f; do
        scan_rust "$root" "$f" || bad=1
    done < <(sed -n 's/^FILE: //p' <<<"$out")
    # 2. Shipping files.
    out="$(files_of "$root" "$SHIPPING")"
    grep '^MISSING: ' <<<"$out" && bad=1
    while IFS= read -r f; do ship+=("$f"); done < <(sed -n 's/^FILE: //p' <<<"$out")
    scan_shipping "$root" "${ship[@]}" || bad=1
    # 3. Each still ships it.
    while IFS=$'\t' read -r file want; do
        if [ ! -f "$root/$file" ]; then
            echo "MISSING: $file (it ships k5-cli; update SHIPS in tests/packaging/k5_cli_shipped_off_by_default.sh)"
            bad=1
        elif ! W="$want" awk '{ sub(/^[ \t]+/, "") } $0 == ENVIRON["W"] { f = 1 } END { exit !f }' "$root/$file"; then
            echo "SHIPS NO MORE: $file has no line: $want"
            bad=1
        fi
    done <<<"$SHIPS"
    # 4 and 5. Windows files and the Tauri configs never name it.
    out="$(files_of "$root" "$WINDOWS
$TAURI")"
    grep '^MISSING: ' <<<"$out" && bad=1
    while IFS= read -r f; do
        if out="$(grep -n -F -- "k5-cli" "$root/$f")"; then
            while IFS= read -r line; do echo "NAMES k5-cli: $f:$line"; done <<<"$out"
            bad=1
        fi
    done < <(files_of "$root" "$WINDOWS
$TAURI" | sed -n 's/^FILE: //p' | LC_ALL=C sort -u)
    return "$bad"
}

pass=0
die() { echo "FAIL: $*" >&2; exit 1; }
# expect_fail <what> <needle>: the scratch copy now fails, and says <needle>.
expect_fail() {
    local out
    if out="$(scan "$T")"; then die "$1 passed"; fi
    grep -qF -- "$2" <<<"$out" || die "$1: the failure doesn't say '$2': $out"
    pass=$((pass + 1))
}

# ── The checks bite: a scratch copy of the listed files ────────────────
T="$(mktemp -d -t k5-shipped-off-XXXXXX)"
trap 'rm -rf "$T"' EXIT
copied=0
while IFS= read -r f; do
    mkdir -p "$T/$(dirname "$f")"
    cp "$ROOT/$f" "$T/$f"
    copied=$((copied + 1))
done < <(files_of "$ROOT" "$INSTALL_RS
$SHIPPING
$WINDOWS
$TAURI" | sed -n 's/^FILE: //p' | LC_ALL=C sort -u)
[ "$copied" -ge 30 ] || die "only $copied files copied: the globs no longer match what they did"
fresh() { cp "$ROOT/$1" "$T/$1"; }

out="$(scan "$T")" || die "the scratch copy fails before any change: $out"
pass=$((pass + 1))

# Rule 2: a link to the binary, by name, by a variable, by a variable set
# from that one, from Python, and a rename into k2 or k5.
R=scripts/release.sh
echo 'ln -s "$APP/Contents/MacOS/k5-cli" "$HOME/.local/bin/k2"' >>"$T/$R"
expect_fail "a release.sh that links k5-cli" "LINKS k5-cli: $R:$(wc -l <"$T/$R" | tr -d ' '):ln -s"
fresh "$R"
I=scripts/install-daemon.sh
echo 'ln -sf "$K5_PATH" "$K2_BIN_DIR/k2"' >>"$T/$I"
expect_fail "install-daemon.sh linking its k5-cli variable" "LINKS k5-cli: $I:"
fresh "$I"
printf 'OTHER="$K5_PATH"\ncp "$OTHER" /usr/local/bin/k5\n' >>"$T/$I"
expect_fail "a copy of k5-cli to k5 through a second variable" "RENAMES k5-cli: $I:$(wc -l <"$T/$I" | tr -d ' '):cp \"\$OTHER\" /usr/local/bin/k5"
fresh "$I"
B=scripts/lib/build-binaries.sh
printf 'k2_k5_relink() {\n    cp "$(k2_k5_cli_path "$1")" "$1/Contents/MacOS/k2"\n}\n' >>"$T/$B"
expect_fail "a copy over the app's k2 through the helper that names k5-cli's place" "RENAMES k5-cli: $B:"
fresh "$B"
P=scripts/ci/compute_release.py
echo 'os.symlink("k5-cli-linux-x86_64", "k5")' >>"$T/$P"
expect_fail "a Python symlink to k5-cli" "LINKS k5-cli: $P:"
fresh "$P"
K=packaging/arch/PKGBUILD
echo '    install -Dm755 "${target}/release/k5-cli" "${pkgdir}/usr/bin/k5"' >>"$T/$K"
expect_fail "a package that installs k5-cli as k5" "RENAMES k5-cli: $K:"
fresh "$K"
W=.github/workflows/daemon-binaries.yml
echo '          cp k5-cli-linux-x86_64 dist/k2' >>"$T/$W"
expect_fail "a workflow that copies k5-cli to k2" "RENAMES k5-cli: $W:"
fresh "$W"
# A comment may say so; only code counts.
echo '# never ln -s k5-cli to /usr/local/bin/k2' >>"$T/$R"
out="$(scan "$T")" || die "a comment about a link failed: $out"
pass=$((pass + 1))
fresh "$R"

# Rule 3: a package that stops shipping it.
grep -vF 'install -Dm755 "${target}/release/k5-cli" "${pkgdir}/usr/bin/k5-cli"' "$ROOT/$K" >"$T/$K"
expect_fail "a PKGBUILD that no longer installs k5-cli" "SHIPS NO MORE: $K has no line: install -Dm755"
fresh "$K"
grep -vF 'k2_k5_cli_sign "target/release/bundle/macos/K2.app"' "$ROOT/$R" >"$T/$R"
expect_fail "a release.sh that no longer signs k5-cli" "SHIPS NO MORE: $R has no line: k2_k5_cli_sign"
fresh "$R"

# Rules 4 and 5: Windows and Tauri configs.
printf '{"bundle": {"externalBin": ["binaries/k5-cli"]}}\n' >"$T/src-tauri/tauri.windows.conf.json"
expect_fail "a Windows Tauri config that bundles k5-cli" "NAMES k5-cli: src-tauri/tauri.windows.conf.json:1:"
fresh src-tauri/tauri.windows.conf.json
echo 'cargo build --release -p k5-cli' >>"$T/scripts/windows/build-nsis-release.ps1"
expect_fail "the Windows NSIS build naming k5-cli" "NAMES k5-cli: scripts/windows/build-nsis-release.ps1:"
fresh scripts/windows/build-nsis-release.ps1
printf '{"bundle": {"externalBin": ["binaries/k5-cli"]}}\n' >"$T/src-tauri/tauri.conf.json"
expect_fail "a Tauri config that bundles k5-cli" "NAMES k5-cli: src-tauri/tauri.conf.json:1:"
fresh src-tauri/tauri.conf.json

# Rule 1: the guard (slice 10c). The real cli_install.rs has one, and the
# copy passed above with it.
CI="crates/k2-client/src/cli_install.rs"
grep -q '^// k5-cli guard: begin$' "$T/$CI" || die "$CI has no k5-cli guard to test"
printf 'const LEAK: &str = "/usr/bin/k5-cli";\n' >>"$T/$CI"
expect_fail "a k5-cli path outside the guard" "NAMES k5-cli: $CI:$(wc -l <"$T/$CI" | tr -d ' '):const LEAK: &str = \"/usr/bin/k5-cli\";"
fresh "$CI"
guarded="$(sed -n '/^\/\/ k5-cli guard: begin$/,/^\/\/ k5-cli guard: end$/p' "$T/$CI" | sed -n 's/^const \([A-Z0-9_]*\): &str = "k5-cli";$/\1/p')"
[ -n "$guarded" ] || die "no const names the binary inside the guard of $CI"
printf 'pub fn leak(dir: &Path) -> PathBuf {\n    dir.join(%s)\n}\n' "$guarded" >>"$T/$CI"
expect_fail "$guarded read outside the guard" "$guarded is read outside the guard"
fresh "$CI"
for body in 'pub fn sneaky(dir: &Path) -> PathBuf {\n    dir.join(BINARY_FILE)\n}' \
            'pub fn sneaky(on: bool, dir: &Path) -> PathBuf {\n    let _ = on;\n    dir.join(BINARY_FILE)\n}'; do
    awk -v add="$body" '{ print } /^\/\/ k5-cli guard: begin$/ { gsub(/\\n/, "\n", add); print add }' "$ROOT/$CI" >"$T/$CI"
    expect_fail "an unguarded path function inside the guard: $body" "a function that returns a path must"
done
fresh "$CI"
grep -v '^// k5-cli guard: end$' "$ROOT/$CI" >"$T/$CI"
expect_fail "a guard that never ends" "a guard that never ends"
fresh "$CI"
printf '    let _ = dir.join("k5-cli");\n' >>"$T/crates/k2-core/src/cli_stage.rs"
expect_fail "cli_stage.rs naming k5-cli" "NAMES k5-cli: crates/k2-core/src/cli_stage.rs:"
fresh crates/k2-core/src/cli_stage.rs
printf '// k5-cli\n' >>"$T/crates/k2-client/src/win_cli.rs"
expect_fail "win_cli.rs naming k5-cli" "NAMES k5-cli: crates/k2-client/src/win_cli.rs:"
fresh crates/k2-client/src/win_cli.rs

out="$(scan "$T")" || die "the scratch copy fails after the checks were undone: $out"

# A listed file gone, and a glob that matches nothing.
rm "$T/crates/k2-core/src/cli_stage.rs"
expect_fail "a missing cli_stage.rs" "MISSING: crates/k2-core/src/cli_stage.rs "
rm "$T"/scripts/sign-*.sh
expect_fail "no scripts/sign-*.sh" "MISSING: scripts/sign-*.sh "

# ── The repo ───────────────────────────────────────────────────────────
if ! out="$(scan "$ROOT")"; then
    echo "$out" >&2
    die "k5-cli ships off by default: the lines above break that"
fi
pass=$((pass + 1))

echo "k5_cli_shipped_off_by_default: $pass passed, 0 failed ($copied files scanned)"
