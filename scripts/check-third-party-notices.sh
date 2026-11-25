#!/usr/bin/env bash
# check-third-party-notices.sh — the third-party licence notices ship with
# every K2 output: third-party licence notices must ship in every package.
#
#   scripts/check-third-party-notices.sh               the source tree and the
#                                                      packaging (no build)
#   scripts/check-third-party-notices.sh --web-out D   a web build's folder
#                                                      (out/web/app/<version>)
#   scripts/check-third-party-notices.sh --deb F       a built k2-daemon .deb
#   scripts/check-third-party-notices.sh --app D       a built app's resource
#                                                      folder (K2.app/Contents/
#                                                      Resources, or Tauri's
#                                                      resource dir on Windows
#                                                      or Linux)
#   scripts/check-third-party-notices.sh --npm         part 5's npm copies
#                                                      against node_modules
#                                                      (after bun install)
#   scripts/check-third-party-notices.sh --sync        copy NOTICE.md into
#                                                      THIRD-PARTY-NOTICES.md
#                                                      (part 4), then check
#
# K2_NOTICES_ROOT overrides the tree it reads (tests/packaging/
# third-party-notices.sh points it at broken copies).
#
# The file, resources/THIRD-PARTY-NOTICES.md, is kept by hand. Its part 4 is
# NOTICE.md byte for byte, between the BEGIN/END NOTICE.md markers. Its part 5
# holds the licence files NOTICE.md points to that do not ship themselves:
# each BEGIN FILE block is checked against its file, and each repo file a
# NOTICE.md "full text(s) in `...`" names must have one.
#
# The three outputs and where the file lands in each:
#   desktop app   src-tauri/tauri.conf.json bundle.resources
#                 (Resources/_up_/resources/ on macOS; the same _up_ folder
#                 beside the app on Windows and in the Linux deb/rpm/AppImage)
#   server .deb   scripts/package-daemon-deb.sh → /usr/share/doc/k2-daemon/
#                 Arch package: packaging/arch/PKGBUILD → /usr/share/licenses/k2/
#   web app       vite.config.web.ts → scripts/notices/web-notices-plugin.mjs
#                 → out/web/app/<version>/notices/, linked from Help
#
# Fails loudly: every missing piece is listed, and the exit code is 1.
set -euo pipefail

ROOT="${K2_NOTICES_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
FILE_REL="resources/THIRD-PARTY-NOTICES.md"
FILE="$ROOT/$FILE_REL"
BEGIN_MARK="<!-- BEGIN NOTICE.md -->"
END_MARK="<!-- END NOTICE.md -->"

problems=0
bad() { echo "  FAIL: $*" >&2; problems=$((problems + 1)); }
ok() { echo "  ok: $*"; }

# NOTICE.md's copy inside the file (the lines between the markers).
notice_copy() {
    awk -v b="$BEGIN_MARK" -v e="$END_MARK" '
        $0 == e { inside = 0 }
        inside { print }
        $0 == b { inside = 1 }
    ' "$FILE"
}

sync_notice() {
    [ -f "$FILE" ] || { echo "check-third-party-notices: FATAL — $FILE missing" >&2; exit 1; }
    grep -qxF "$BEGIN_MARK" "$FILE" && grep -qxF "$END_MARK" "$FILE" || {
        echo "check-third-party-notices: FATAL — $FILE_REL has no $BEGIN_MARK / $END_MARK pair" >&2
        exit 1
    }
    local tmp
    tmp="$(mktemp)"
    awk -v b="$BEGIN_MARK" -v e="$END_MARK" -v src="$ROOT/NOTICE.md" '
        $0 == b { print; while ((getline line < src) > 0) print line; skip = 1; next }
        $0 == e { skip = 0 }
        !skip { print }
    ' "$FILE" > "$tmp"
    mv "$tmp" "$FILE"
    echo "check-third-party-notices: copied NOTICE.md into $FILE_REL"
}

# Part 5's FILE blocks: each holds its file(s) byte for byte (lines=a-b: those
# lines only). And every repo file NOTICE.md's "full text(s) in `…`" names
# has a block, unless it ships itself (resources/notices/, in every package).
check_file_blocks() {
    python3 -I - "$FILE" "$ROOT" <<'PY'
import os, re, sys
path, root = sys.argv[1], sys.argv[2]
s = open(path, encoding="utf-8").read()
bad = 0
def fail(msg):
    global bad
    print(f"  FAIL: {msg}", file=sys.stderr)
    bad += 1
pat = re.compile(r"<!-- BEGIN FILE(?: lines=(\d+)-(\d+))?: ([^>]+?) -->\n~~~~text\n(.*?)~~~~\n<!-- END FILE -->\n", re.S)
blocks = list(pat.finditer(s))
if s.count("<!-- BEGIN FILE") != len(blocks):
    fail("a BEGIN FILE block is malformed (want: marker, ~~~~text, the bytes, ~~~~, <!-- END FILE -->)")
covered = set()
for m in blocks:
    a, b, names, text = m.group(1), m.group(2), m.group(3).split(), m.group(4)
    for n in names:
        covered.add(n)
        full = os.path.join(root, n)
        if not os.path.isfile(full):
            fail(f"part 5 copies {n}, which is gone")
            continue
        want = open(full, encoding="utf-8").read()
        if a:
            want = "".join(want.splitlines(keepends=True)[int(a) - 1 : int(b)])
        if not want.endswith("\n"):
            want += "\n"
        if text != want:
            fail(f"part 5's copy of {n} differs from the file (copy it in again)")
notice = open(os.path.join(root, "NOTICE.md"), encoding="utf-8").read()
for line in notice.splitlines():
    for seg in re.findall(r"full texts?[^|]*", line):
        for ref in re.findall(r"`([^`\s]+)`", seg):
            if ref.startswith("resources/notices/"):
                continue
            if os.path.isfile(os.path.join(root, ref)) and ref not in covered:
                fail(f"NOTICE.md points to {ref}, which does not ship; copy it into part 5")
if not blocks:
    fail("part 5 has no FILE blocks")
# COMMENTS blocks: the /*! Bundled license information */ comments a
# prebuilt file keeps, joined by blank lines.
cpat = re.compile(r"<!-- BEGIN COMMENTS: (\S+) -->\n~~~~text\n(.*?)~~~~\n<!-- END COMMENTS -->\n", re.S)
cblocks = list(cpat.finditer(s))
if s.count("<!-- BEGIN COMMENTS") != len(cblocks):
    fail("a BEGIN COMMENTS block is malformed")
for m in cblocks:
    n, text = m.group(1), m.group(2)
    full = os.path.join(root, n)
    if not os.path.isfile(full):
        fail(f"part 5 copies the licence comments of {n}, which is gone")
        continue
    found = re.findall(r"/\*! Bundled license information:[\s\S]*?\*/", open(full, encoding="utf-8").read())
    if not found or text != "\n\n".join(found) + "\n":
        fail(f"part 5's copy of {n}'s bundled licence comments differs from the file (copy them in again)")
sys.exit(1 if bad else 0)
PY
}

# Part 5's NPM blocks: the package's file in node_modules (after bun install)
# at the version bun.lock pins.
check_npm_blocks() {
    python3 -I - "$FILE" "$ROOT" <<'PY'
import json, os, re, sys
path, root = sys.argv[1], sys.argv[2]
s = open(path, encoding="utf-8").read()
lock = open(os.path.join(root, "bun.lock"), encoding="utf-8").read()
blocks = list(re.finditer(r"<!-- BEGIN NPM: (\S+)@([0-9][^ ]*) (\S+) -->\n~~~~text\n(.*?)~~~~\n<!-- END NPM -->\n", s, re.S))
bad = 0
if not blocks or s.count("<!-- BEGIN NPM") != len(blocks):
    print("  FAIL: no NPM blocks, or a malformed one", file=sys.stderr); bad += 1
for m in blocks:
    pkg, ver, fname, text = m.groups()
    if f'"{pkg}@{ver}"' not in lock:
        print(f"  FAIL: bun.lock no longer pins {pkg}@{ver} (copy the new version's {fname} in)", file=sys.stderr); bad += 1
        continue
    d = os.path.join(root, "node_modules", pkg)
    try:
        got_ver = json.load(open(os.path.join(d, "package.json")))["version"]
        want = open(os.path.join(d, fname), encoding="utf-8").read()
    except OSError as e:
        print(f"  FAIL: {pkg}: {e} (run bun install first)", file=sys.stderr); bad += 1
        continue
    if got_ver != ver:
        print(f"  FAIL: node_modules has {pkg} {got_ver}, part 5 names {ver}", file=sys.stderr); bad += 1
    elif text != (want if want.endswith("\n") else want + "\n"):
        print(f"  FAIL: part 5's copy of {pkg}/{fname} differs from node_modules", file=sys.stderr); bad += 1
    else:
        print(f"  ok: part 5's {pkg}@{ver} {fname} matches node_modules")
sys.exit(1 if bad else 0)
PY
}

# The file itself: the notices it must hold, and NOTICE.md unchanged.
check_file() {
    if [ ! -f "$FILE" ]; then
        bad "$FILE_REL is missing"
        return
    fi
    local needle before="$problems"
    for needle in \
        "PostgreSQL Database Management System" \
        "Portions Copyright (c) 1994, The Regents of the University of California" \
        "Copyright (c) 2015, Lukas Fittl" \
        "Copyright (c) 2008-2018, Dave Benson and the protobuf-c authors." \
        "Copyright (C) 2012-2020 Yann Collet" \
        "Copyright (c) 1983, 1995, 1996 Eric P. Allman" \
        "Copyright (c) 1998 Todd C. Miller" \
        "The author disclaims copyright to this source code." \
        "May you share freely, never taking more than you give." \
        "Copyright (c) 2014 The rusqlite developers" \
        "Earlier K2 releases included libgit2." \
        "https://crates.io/crates/libgit2-sys/0.17.0+1.8.1" \
        "https://github.com/libgit2/libgit2/tree/v1.8.1" \
        "Written offer:" \
        "LINKING EXCEPTION" \
        "GNU GENERAL PUBLIC LICENSE" \
        "GNU LESSER GENERAL PUBLIC LICENSE" \
        "### libssh2 and OpenSSL in earlier releases" \
        "Copyright (C) 2004-2007 Sara Golemon" \
        "OpenSSL 3 ships no NOTICE file" \
        "## 5. Licence texts NOTICE.md points to" \
        "commit \`36f7e21ad757a3dacc58cf7944329da6bc1d6e96\`" \
        "libgit2-1.8.1-source.tar.gz" \
        "https://github.com/Alakazam-211/K2/blob/v<version>/crates/k2-vault/data/public_suffix_list.dat" \
        "https://publicsuffix.org/list/public_suffix_list.dat" \
        "#### Mermaid's bundled dependencies" \
        "K2 takes it under the **Apache License 2.0**" \
        "##### Known incomplete"
    do
        grep -qF -- "$needle" "$FILE" || bad "$FILE_REL lacks: $needle"
    done
    check_file_blocks || bad "$FILE_REL: part 5's copies of files (above)"
    if ! grep -qxF "$BEGIN_MARK" "$FILE" || ! grep -qxF "$END_MARK" "$FILE"; then
        bad "$FILE_REL has no $BEGIN_MARK / $END_MARK pair around its copy of NOTICE.md"
    elif ! notice_copy | cmp -s - "$ROOT/NOTICE.md"; then
        bad "$FILE_REL's copy of NOTICE.md differs from NOTICE.md (run scripts/check-third-party-notices.sh --sync)"
    elif [ "$(grep -c '^Mozilla Public License Version 2.0$' "$FILE")" != 1 ]; then
        bad "$FILE_REL must hold the MPL-2.0 text exactly once (the Public Suffix List and the shiki grammars share it)"
    elif [ "$problems" -eq "$before" ]; then
        ok "$FILE_REL holds the third-party notices and NOTICE.md unchanged"
    fi
}

# The packaging, read without a build.
check_packaging() {
    local conf="$ROOT/src-tauri/tauri.conf.json"
    if python3 -I - "$conf" "$ROOT/resources/notices" <<'PY'
import json, os, sys
conf, notices = sys.argv[1], sys.argv[2]
res = json.load(open(conf, encoding="utf-8"))["bundle"]["resources"]
names = list(res) if isinstance(res, list) else list(res.keys())
want = ["../resources/THIRD-PARTY-NOTICES.md"] + sorted(
    f"../resources/notices/{n}" for n in os.listdir(notices))
missing = [w for w in want if w not in names]
for m in missing:
    print(f"  FAIL: src-tauri/tauri.conf.json bundle.resources lacks {m}", file=sys.stderr)
sys.exit(1 if missing else 0)
PY
    then
        ok "desktop app: tauri.conf.json bundles $FILE_REL and resources/notices/*"
    else
        bad "desktop app: tauri.conf.json resources (above)"
    fi

    local deb="$ROOT/scripts/package-daemon-deb.sh"
    if grep -qF '"$PROJECT_DIR/resources/THIRD-PARTY-NOTICES.md"' "$deb" \
        && grep -qF '"$PKGROOT/usr/share/doc/k2-daemon/THIRD-PARTY-NOTICES.md"' "$deb" \
        && grep -qF 'check-third-party-notices.sh" --deb "$OUT"' "$deb"; then
        ok "server .deb: package-daemon-deb.sh installs it in /usr/share/doc/k2-daemon/ and checks the .deb"
    else
        bad "server .deb: scripts/package-daemon-deb.sh does not install $FILE_REL into /usr/share/doc/k2-daemon/ (or no longer checks the built .deb)"
    fi

    local pkgb="$ROOT/packaging/arch/PKGBUILD"
    if grep -qF '"${tree}/resources/THIRD-PARTY-NOTICES.md"' "$pkgb" \
        && grep -qF '"${pkgdir}/usr/share/licenses/${pkgname}/THIRD-PARTY-NOTICES.md"' "$pkgb" \
        && grep -qF '"${tree}"/resources/notices/*' "$pkgb"; then
        ok "Arch package: PKGBUILD installs it and resources/notices/* in /usr/share/licenses/k2/"
    else
        bad "Arch package: packaging/arch/PKGBUILD does not install $FILE_REL and resources/notices/* into /usr/share/licenses/\${pkgname}/"
    fi

    local web="$ROOT/vite.config.web.ts"
    if grep -qF "from './scripts/notices/web-notices-plugin.mjs'" "$web" \
        && grep -qF "webNoticesPlugin(__dirname)" "$web"; then
        ok "web app: vite.config.web.ts runs webNoticesPlugin"
    else
        bad "web app: vite.config.web.ts does not run webNoticesPlugin (scripts/notices/web-notices-plugin.mjs)"
    fi
}

# A web build: the file and every shiki notice, byte for byte, and the page.
check_web_out() {
    local out="$1" n
    [ -d "$out" ] || { bad "web build folder $out does not exist"; return; }
    cmp -s "$FILE" "$out/notices/THIRD-PARTY-NOTICES.md" \
        || bad "web build lacks notices/THIRD-PARTY-NOTICES.md (or it differs from $FILE_REL)"
    local count=0
    for n in "$ROOT"/resources/notices/shiki-*; do
        count=$((count + 1))
        cmp -s "$n" "$out/notices/${n##*/}" || bad "web build lacks notices/${n##*/} (or it differs)"
    done
    [ "$count" -gt 0 ] || bad "no resources/notices/shiki-* files to check"
    if [ -f "$out/notices/index.html" ]; then
        for n in THIRD-PARTY-NOTICES.md "$ROOT"/resources/notices/shiki-*; do
            grep -qF "href=\"${n##*/}\"" "$out/notices/index.html" || bad "notices/index.html does not link ${n##*/}"
        done
    else
        bad "web build lacks notices/index.html"
    fi
    [ "$problems" -eq 0 ] && ok "web build $out carries $FILE_REL and the $count shiki notices"
}

# A built .deb: the file at /usr/share/doc/k2-daemon/, same bytes.
check_deb() {
    local deb="$1" tmp
    [ -f "$deb" ] || { bad "$deb does not exist"; return; }
    command -v dpkg-deb >/dev/null 2>&1 || { bad "dpkg-deb is needed to read $deb"; return; }
    tmp="$(mktemp -d)"
    dpkg-deb -x "$deb" "$tmp"
    if cmp -s "$FILE" "$tmp/usr/share/doc/k2-daemon/THIRD-PARTY-NOTICES.md"; then
        ok "$deb carries /usr/share/doc/k2-daemon/THIRD-PARTY-NOTICES.md"
    else
        bad "$deb lacks /usr/share/doc/k2-daemon/THIRD-PARTY-NOTICES.md (or it differs from $FILE_REL)"
    fi
    rm -rf "$tmp"
}

# A built app's resource folder (Tauri puts ../resources/x under _up_/resources/x).
check_app() {
    local dir="$1" n
    cmp -s "$FILE" "$dir/_up_/$FILE_REL" || bad "app resources $dir lack _up_/$FILE_REL (or it differs)"
    for n in "$ROOT"/resources/notices/*; do
        cmp -s "$n" "$dir/_up_/resources/notices/${n##*/}" || bad "app resources $dir lack _up_/resources/notices/${n##*/}"
    done
    [ "$problems" -eq 0 ] && ok "app resources $dir carry $FILE_REL and resources/notices/*"
}

case "${1:-}" in
    "") check_file; check_packaging ;;
    --sync) sync_notice; check_file; check_packaging ;;
    --web-out) [ -n "${2:-}" ] || { echo "usage: $0 --web-out <dir>" >&2; exit 2; }; check_web_out "$2" ;;
    --deb) [ -n "${2:-}" ] || { echo "usage: $0 --deb <file.deb>" >&2; exit 2; }; check_deb "$2" ;;
    --app) [ -n "${2:-}" ] || { echo "usage: $0 --app <resources dir>" >&2; exit 2; }; check_app "$2" ;;
    --npm) check_npm_blocks || bad "$FILE_REL: part 5's npm copies (above)" ;;
    *) echo "usage: $0 [--sync | --npm | --web-out <dir> | --deb <file.deb> | --app <resources dir>]" >&2; exit 2 ;;
esac

if [ "$problems" -gt 0 ]; then
    echo "check-third-party-notices: $problems problem(s)" >&2
    exit 1
fi
echo "check-third-party-notices: ok"
