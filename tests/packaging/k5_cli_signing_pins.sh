#!/usr/bin/env bash
# The k5-cli program in the macOS app is signed one way, and every check of
# it looks for that same way. scripts/macos-daemon-helper-app.sh holds the
# sign step and the checks; release.sh, build-app.sh and dev-build.sh call
# them. This reads those files as text and fails, naming the file and line,
# when:
#
# 1. The identifier drifts: K2_K5_CLI_IDENTIFIER is set once, in the helper
#    script, to dev.k2.app.k5-cli, and nothing else under scripts/ or
#    .github/ sets it. Any dev.k2.app.k5... string there (a comment, the
#    signer's policy) is exactly that identifier.
# 2. k2_k5_cli_sign stops signing it as itself: its one codesign command
#    has --force, --options runtime, --timestamp and
#    --identifier "$K2_K5_CLI_IDENTIFIER", and no --entitlements (so no
#    keychain access group), no --deep and no other --identifier.
# 3. A check stops looking for that identifier, or a step stops running
#    the one signature check: k2_k5_cli_check_signature matches
#    ^Identifier=${K2_K5_CLI_IDENTIFIER}$ once, runs codesign --verify
#    --strict, checks the hardened runtime flag, the daemon's Team ID and
#    that there are no entitlements, and spells no identifier of its own.
#    k2_k5_cli_verify_signed (after signing), k2_k5_cli_verify_shipped
#    (after notarization) and k2_k5_cli_verify_tarball (the update bundle)
#    each call it exactly once and run no codesign of their own (reuse,
#    not a copy); verify_signed also runs its --version, verify_shipped
#    asks Gatekeeper, and verify_tarball calls it on the app it unpacked.
# 4. A caller signs or checks it out of order: each caller signs it once,
#    before the outer app's codesign, and checks it once after; release.sh
#    then zips that app, notarizes the zip, staples the app, and only then
#    checks the shipped app and the update bundle. No caller codesigns
#    k5-cli itself or signs with --deep (which would sign it again).
#
# It proves it bites first: on a scratch copy it must pass, then fail for
# each broken rule. No cargo, no codesign, no network. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

HELPER="scripts/macos-daemon-helper-app.sh"
CALLERS="scripts/release.sh scripts/build-app.sh scripts/dev-build.sh"
# The two trees the identifier scan reads.
TREES="scripts .github"

# scan <root>: every rule. Prints each problem; exit 1 when there is any.
scan() {
    # shellcheck disable=SC2086
    python3 -I - "$1" "$HELPER" "$TREES" $CALLERS <<'PY'
import os, re, sys

root, helper, trees, callers = sys.argv[1], sys.argv[2], sys.argv[3].split(), sys.argv[4:]
WANT = "dev.k2.app.k5-cli"
problems = []

def bad(msg):
    problems.append(msg)

def read(rel):
    p = os.path.join(root, rel)
    if not os.path.isfile(p):
        bad(f"MISSING: {rel}")
        return None
    return open(p, encoding="utf-8", errors="replace").read().split("\n")

def logical(lines):
    """Code lines with backslash continuations joined: (first line number, text)."""
    out, buf, start = [], "", None
    for n, l in enumerate(lines, 1):
        if start is None:
            if l.lstrip().startswith("#"):
                continue
            start = n
        if l.rstrip().endswith("\\"):
            buf += l.rstrip()[:-1] + " "
            continue
        out.append((start, (buf + l).strip()))
        buf, start = "", None
    return out

def body(lines, name):
    """The lines of shell function <name>, from its opening line to the
    closing brace in column 0: (first line number, its lines)."""
    for i, l in enumerate(lines):
        if re.match(r"^" + re.escape(name) + r"\(\)\s*\{", l):
            for j in range(i + 1, len(lines)):
                if lines[j] == "}":
                    return i + 1, lines[i:j + 1]
            bad(f"UNCLOSED: {helper}:{i + 1}: {name} never ends")
            return None
    bad(f"MISSING: {helper} has no function {name}")
    return None

def fn_logical(lines, name):
    got = body(lines, name)
    if got is None:
        return None
    first, part = got
    return [(first + n - 1, t) for n, t in logical(part)]

# 1. The identifier.
h = read(helper)
if h is not None:
    sets = [(n, t) for n, t in logical(h) if re.match(r"^(export\s+|readonly\s+)?K2_K5_CLI_IDENTIFIER=", t)]
    if len(sets) != 1:
        bad(f"IDENTIFIER: {helper} sets K2_K5_CLI_IDENTIFIER {len(sets)} times, want once")
    for n, t in sets:
        if t != f'K2_K5_CLI_IDENTIFIER="{WANT}"':
            bad(f"IDENTIFIER: {helper}:{n}: {t} (want K2_K5_CLI_IDENTIFIER=\"{WANT}\")")
for tree in trees:
    top = os.path.join(root, tree)
    if not os.path.isdir(top):
        bad(f"MISSING: {tree}/")
        continue
    for d, dirs, files in os.walk(top):
        dirs[:] = sorted(x for x in dirs if x != "__pycache__")
        for f in sorted(files):
            p = os.path.join(d, f)
            rel = os.path.relpath(p, root)
            try:
                text = open(p, encoding="utf-8").read()
            except (UnicodeDecodeError, OSError):
                continue
            for n, l in enumerate(text.split("\n"), 1):
                for m in re.finditer(r"dev\.k2\.app\.k5[A-Za-z0-9._-]*", l):
                    if m.group(0).rstrip(".") != WANT:
                        bad(f"IDENTIFIER: {rel}:{n}: {m.group(0)} (want {WANT})")
                if rel != helper and re.search(r"(^|[\s;(])(export\s+|readonly\s+|local\s+)?K2_K5_CLI_IDENTIFIER=", l) \
                        and not l.lstrip().startswith("#"):
                    bad(f"IDENTIFIER: {rel}:{n}: sets K2_K5_CLI_IDENTIFIER outside {helper}")

# 2. The sign step.
if h is not None:
    sign = fn_logical(h, "k2_k5_cli_sign")
    if sign is not None:
        cs = [(n, t) for n, t in sign if re.match(r"^codesign\s", t)]
        if len(cs) != 1:
            bad(f"SIGN: {helper}: k2_k5_cli_sign runs codesign {len(cs)} times, want once")
        for n, t in cs:
            for need in ("--force", "--options runtime", "--timestamp", '--identifier "$K2_K5_CLI_IDENTIFIER"', "--sign "):
                if need not in t:
                    bad(f"SIGN: {helper}:{n}: the k5-cli codesign has no {need.strip()}")
            for never in ("--entitlements", "--deep"):
                if never in t:
                    bad(f"SIGN: {helper}:{n}: the k5-cli codesign has {never}")
            if t.count("--identifier") != 1:
                bad(f"SIGN: {helper}:{n}: the k5-cli codesign has more than one --identifier")

# 3. The checks.
ID_MATCH = 'grep -q "^Identifier=${K2_K5_CLI_IDENTIFIER}\\$"'
CHECK = "k2_k5_cli_check_signature"
check_needs = (
    'codesign --verify --strict "$bin"',
    'grep -q "flags=.*runtime"',
    '[ -n "$team" ] && [ "$team" = "$dteam" ]',
    'codesign -d --entitlements -',
    'grep -q "<key>"',
)
steps = {
    "k2_k5_cli_verify_signed": ('"$bin" --version',),
    "k2_k5_cli_verify_shipped": ("spctl --assess",),
    "k2_k5_cli_verify_tarball": ('tar -xzf "$tgz" -C "$dir" K2.app', CHECK + ' "$dir/K2.app"'),
}
def no_identifier(name, lines):
    for n, t in lines:
        if re.search(r"(?<!Team)Identifier=(?!\$\{K2_K5_CLI_IDENTIFIER\}|\$\{K2_DAEMON_HELPER_BUNDLE_ID\})", t):
            bad(f"CHECK: {helper}:{n}: {name} looks for an identifier of its own: {t}")
        if "dev.k2.app" in t:
            bad(f"CHECK: {helper}:{n}: {name} spells an identifier: {t}")
if h is not None:
    lines = fn_logical(h, CHECK)
    if lines is not None:
        joined = "\n".join(t for _, t in lines)
        if joined.count(ID_MATCH) != 1:
            bad(f"CHECK: {helper}: {CHECK} matches ^Identifier=${{K2_K5_CLI_IDENTIFIER}}$ {joined.count(ID_MATCH)} times, want once")
        no_identifier(CHECK, lines)
        for need in check_needs:
            if need not in joined:
                bad(f"CHECK: {helper}: {CHECK} no longer runs {need}")
    for name, extra in steps.items():
        lines = fn_logical(h, name)
        if lines is None:
            continue
        joined = "\n".join(t for _, t in lines)
        calls = len(re.findall(r"(^|[\s;(]|&&|\|\|)" + CHECK + r"\s", joined, re.M))
        if calls != 1:
            bad(f"CHECK: {helper}: {name} calls {CHECK} {calls} times, want once")
        for n, t in lines:
            if re.search(r"(^|[\s;(]|\$\()codesign\s", t):
                bad(f"CHECK: {helper}:{n}: {name} runs codesign itself (use {CHECK}): {t}")
        no_identifier(name, lines)
        for need in extra:
            if need not in joined:
                bad(f"CHECK: {helper}: {name} no longer runs {need}")

# 4. The callers.
OUTER = re.compile(r'^codesign\s.*--sign\s+\S+\s+(\S+)$')
for c in callers:
    lines = read(c)
    if lines is None:
        continue
    code = logical(lines)
    def where(pat, what):
        hits = [(n, t) for n, t in code if re.match(pat, t)]
        if len(hits) != 1:
            bad(f"ORDER: {c}: {what} {len(hits)} times, want once")
            return None, None
        return hits[0]
    sn, st = where(r"^k2_k5_cli_sign\s", "signs k5-cli (k2_k5_cli_sign)")
    vn, vt = where(r"^k2_k5_cli_verify_signed\s", "checks the signed k5-cli (k2_k5_cli_verify_signed)")
    for n, t in code:
        if re.match(r"^codesign\s", t) and "--sign" in t:
            if "k5" in t:
                bad(f"SIGN: {c}:{n}: codesigns k5-cli outside k2_k5_cli_sign: {t}")
            if "--deep" in t:
                bad(f"SIGN: {c}:{n}: signs with --deep (it would sign k5-cli again): {t}")
    if sn is None or vn is None:
        continue
    app = st.split()[1]
    if vt.split()[1] != app:
        bad(f"ORDER: {c}:{vn}: checks {vt.split()[1]}, but signed k5-cli in {app}")
    outer = [n for n, t in code if (m := OUTER.match(t)) and m.group(1) == app]
    if len(outer) != 1:
        bad(f"ORDER: {c}: signs the outer app {app} {len(outer)} times, want once")
        continue
    if not sn < outer[0] < vn:
        bad(f"ORDER: {c}: k5-cli must be signed (line {sn}) before the outer app (line {outer[0]}) and checked after it (line {vn})")
    if c == "scripts/release.sh":
        steps = [
            (r'^ditto -c -k --keepParent "K2\.app" "/tmp/K2_\$\{VERSION\}\.zip"$', "zips the signed app"),
            (r'^xcrun notarytool submit "/tmp/K2_\$\{VERSION\}\.zip"', "notarizes that zip"),
            (r'^staple_with_retry "K2\.app"$', "staples the app"),
            (r'^k2_k5_cli_verify_shipped "K2\.app" \|\| exit 1$', "checks the shipped k5-cli"),
            (r'^k2_k5_cli_verify_tarball "target/release/bundle/macos/K2\.app\.tar\.gz" \|\| exit 1$', "checks the update bundle's k5-cli"),
        ]
        last = vn
        for pat, what in steps:
            n, _ = where(pat, what)
            if n is None:
                continue
            if n <= last:
                bad(f"ORDER: {c}:{n}: {what} comes before the step it follows (line {last})")
            last = n

for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PY
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

# ── The checks bite: a scratch copy of scripts/ and .github/ ───────────
T="$(mktemp -d -t k5-signing-pins-XXXXXX)"
trap 'rm -rf "$T"' EXIT
for tree in $TREES; do cp -R "$ROOT/$tree" "$T/$tree"; done
fresh() { cp "$ROOT/$1" "$T/$1"; }
# edit <file> <python expression on s, the text>: rewrite a scratch file,
# and fail when the edit changes nothing (the pattern no longer matches).
edit() {
    python3 -I - "$T/$1" "$2" <<'PY' || die "edit of $1 did not apply: $2"
import sys
p, expr = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
t = eval(expr, {"s": s, "re": __import__("re")})
if t == s:
    sys.exit(1)
open(p, "w", encoding="utf-8").write(t)
PY
}

out="$(scan "$T")" || die "the scratch copy fails before any change: $out"
pass=$((pass + 1))

H="$HELPER"
# Rule 1: the identifier.
edit "$H" 's.replace("K2_K5_CLI_IDENTIFIER=\"dev.k2.app.k5-cli\"", "K2_K5_CLI_IDENTIFIER=\"dev.k2.app.k5cli\"")'
expect_fail "a renamed identifier" "IDENTIFIER: $H:"
fresh "$H"
echo 'K2_K5_CLI_IDENTIFIER="dev.k2.app.k5-cli"' >>"$T/$H"
expect_fail "the identifier set twice" "sets K2_K5_CLI_IDENTIFIER 2 times"
fresh "$H"
echo 'K2_K5_CLI_IDENTIFIER="dev.k2.app.daemon"' >>"$T/scripts/release.sh"
expect_fail "a caller that sets the identifier" "IDENTIFIER: scripts/release.sh:"
fresh scripts/release.sh
mkdir -p "$T/scripts/k2sign"
echo 'K5_CLI_ID = "dev.k2.app.k5_cli"' >>"$T/scripts/k2sign/k2sign.py"
expect_fail "a signer policy with another spelling" "IDENTIFIER: scripts/k2sign/k2sign.py:"
fresh scripts/k2sign/k2sign.py

# Rule 2: the sign step.
edit "$H" 're.sub(r"codesign --force --options runtime --timestamp \\\n(\s+--identifier \"\$K2_K5_CLI_IDENTIFIER\")", r"codesign --force --timestamp \\\n\1", s)'
expect_fail "k5-cli signed without the hardened runtime" "has no --options runtime"
fresh "$H"
edit "$H" 's.replace("--identifier \"$K2_K5_CLI_IDENTIFIER\" \\\n", "--identifier \"$K2_K5_CLI_IDENTIFIER\" \\\n        --entitlements \"$ENTS\" \\\n", 1)'
expect_fail "k5-cli signed with entitlements" "the k5-cli codesign has --entitlements"
fresh "$H"
edit "$H" 's.replace("--identifier \"$K2_K5_CLI_IDENTIFIER\" \\\n", "--identifier \"$K2_DAEMON_HELPER_BUNDLE_ID\" \\\n", 1)'
expect_fail "k5-cli signed under the daemon's identifier" "has no --identifier \"\$K2_K5_CLI_IDENTIFIER\""
fresh "$H"

# Rule 3: the checks.
edit "$H" 's.replace("grep -q \"^Identifier=${K2_K5_CLI_IDENTIFIER}\\$\" <<<\"$info\" \\\n        || { echo \"FATAL: k5-cli${where} code identifier", "grep -q \"^Identifier=dev.k2.app.k5-cli\\$\" <<<\"$info\" \\\n        || { echo \"FATAL: k5-cli${where} code identifier", 1)'
expect_fail "the signature check spelling its own identifier" "k2_k5_cli_check_signature matches"
fresh "$H"
edit "$H" 's.replace("    grep -q \"flags=.*runtime\" <<<\"$info\" \\\n        || { echo \"FATAL: k5-cli${where} is not signed", "    true \\\n        || { echo \"FATAL: k5-cli${where} is not signed", 1)'
expect_fail "a signature check that drops the runtime flag" "k2_k5_cli_check_signature no longer runs grep -q \"flags=.*runtime\""
fresh "$H"
edit "$H" 's.replace("    codesign --verify --strict \"$bin\" \\\n        || { echo \"FATAL: k5-cli${where} does not verify", "    codesign --verify \"$bin\" \\\n        || { echo \"FATAL: k5-cli${where} does not verify", 1)'
expect_fail "a signature check that drops --strict" "k2_k5_cli_check_signature no longer runs codesign --verify --strict"
fresh "$H"
edit "$H" 's.replace("    [ -n \"$team\" ] && [ \"$team\" = \"$dteam\" ] \\\n        || { echo \"FATAL: k5-cli${where} Team ID", "    [ -n \"$team\" ] \\\n        || { echo \"FATAL: k5-cli${where} Team ID", 1)'
expect_fail "a signature check that drops the Team ID" "k2_k5_cli_check_signature no longer runs [ -n \"\$team\" ] && [ \"\$team\" = \"\$dteam\" ]"
fresh "$H"
edit "$H" 's.replace("    if grep -q \"<key>\" <<<\"$ents\"; then\n        echo \"FATAL: k5-cli${where} carries entitlements", "    if false; then\n        echo \"FATAL: k5-cli${where} carries entitlements", 1)'
expect_fail "a signature check that drops the entitlements test" "k2_k5_cli_check_signature no longer runs grep -q \"<key>\""
fresh "$H"
edit "$H" 's.replace("    spctl --assess --type execute \"$app\" \\\n        || { echo \"FATAL: Gatekeeper rejects the notarized app (with its k5-cli)", "    true \\\n        || { echo \"FATAL: Gatekeeper rejects the notarized app (with its k5-cli)", 1)'
expect_fail "a notarized check that drops Gatekeeper" "k2_k5_cli_verify_shipped no longer runs spctl --assess"
fresh "$H"
edit "$H" 're.sub(r"(k2_k5_cli_verify_tarball\(\) \{.*?)        k2_k5_cli_check_signature \"\$dir/K2\.app\" \" in the update bundle\" \|\| rc=1\n", r"\1        true\n", s, count=1, flags=re.S)'
expect_fail "a tarball check that no longer checks the signature" "k2_k5_cli_verify_tarball calls k2_k5_cli_check_signature 0 times"
fresh "$H"
edit "$H" 're.sub(r"(k2_k5_cli_verify_tarball\(\) \{.*?)k2_k5_cli_check_signature \"\$dir/K2\.app\"", r"\1k2_k5_cli_check_signature \"$(dirname \"$tgz\")/K2.app\"", s, count=1, flags=re.S)'
expect_fail "a tarball check of the packed app, not the bundle" "k2_k5_cli_verify_tarball no longer runs k2_k5_cli_check_signature \"\$dir/K2.app\""
fresh "$H"
edit "$H" 's.replace("    k2_k5_cli_check_signature \"$app\" \" in the notarized app\" || return 1\n", "    codesign --verify --strict \"$(k2_k5_cli_path \"$app\")\" || return 1\n", 1)'
expect_fail "a notarized check with its own copy of the signature check" "k2_k5_cli_verify_shipped runs codesign itself"
fresh "$H"

# Rule 4: the callers.
R=scripts/release.sh
edit "$R" 's.replace("k2_k5_cli_sign \"target/release/bundle/macos/K2.app\" \"$SIGNING_IDENTITY\" || exit 1\n", "", 1).replace("k2_k5_cli_verify_signed \"target/release/bundle/macos/K2.app\" || exit 1\n", "k2_k5_cli_verify_signed \"target/release/bundle/macos/K2.app\" || exit 1\nk2_k5_cli_sign \"target/release/bundle/macos/K2.app\" \"$SIGNING_IDENTITY\" || exit 1\n", 1)'
expect_fail "release.sh signing k5-cli after the outer app" "ORDER: $R: k5-cli must be signed"
fresh "$R"
edit "$R" 's.replace("k2_k5_cli_verify_shipped \"K2.app\" || exit 1\n", "", 1).replace("xcrun notarytool submit \"/tmp/K2_${VERSION}.zip\"", "k2_k5_cli_verify_shipped \"K2.app\" || exit 1\nxcrun notarytool submit \"/tmp/K2_${VERSION}.zip\"", 1)'
expect_fail "release.sh checking the shipped k5-cli before notarizing" "checks the shipped k5-cli comes before"
fresh "$R"
B=scripts/build-app.sh
echo 'codesign --force --timestamp --sign "$SIGNING_IDENTITY" "$APP/Contents/MacOS/k5-cli"' >>"$T/$B"
expect_fail "build-app.sh codesigning k5-cli itself" "SIGN: $B:"
fresh "$B"
D=scripts/dev-build.sh
echo 'codesign --force --deep --sign "$SIGNING_IDENTITY" /Applications/K2.app' >>"$T/$D"
expect_fail "dev-build.sh signing with --deep" "signs with --deep"
fresh "$D"
grep -v '^k2_k5_cli_verify_signed ' "$ROOT/$D" >"$T/$D"
expect_fail "dev-build.sh no longer checking k5-cli" "ORDER: $D: checks the signed k5-cli"
fresh "$D"

out="$(scan "$T")" || die "the scratch copy fails after the checks were undone: $out"
rm "$T/$H"
expect_fail "a missing helper script" "MISSING: $H"

# ── The repo ───────────────────────────────────────────────────────────
if ! out="$(scan "$ROOT")"; then
    echo "$out" >&2
    die "k5-cli is not signed and checked the same way: the lines above break that"
fi
pass=$((pass + 1))

echo "k5_cli_signing_pins: $pass passed, 0 failed"
