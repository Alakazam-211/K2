#!/usr/bin/env bash
# check-omarchy-recipe.sh — catch drift in the k2-bin recipe we hand to
# omacom/omarchy-pkgs, without Arch. Needs bash, python3, and sha256sum or
# shasum. No makepkg, no network.
#
# Checks:
#   - the install hooks are the same bytes as packaging/arch/k2.install, and the
#     recipe's sum for them is right;
#   - the licence is the SPDX id of what LICENSE.md actually says, so a licence
#     change can't leave the recipe claiming the old one;
#   - depends and optdepends list the same names as packaging/arch/PKGBUILD;
#   - nothing is compiled: no build() or prepare(), no OAuth or signing names;
#   - the source is our GitHub release, with a real sha256 and noextract;
#   - .omarchy/package.json watches our releases and holds new ones back 24 h,
#     and doesn't ask for the fast ring;
#   - bash -n on the PKGBUILD and the install file.
#
# Usage: scripts/check-omarchy-recipe.sh [repo-root]
#   repo-root defaults to this script's repo. The tests point it at a temp copy
#   with a deliberately broken recipe. Exit 0 when every check passes, else 1.
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
ROOT="$(cd "$ROOT" && pwd)"
RECIPE="$ROOT/packaging/omarchy/k2-bin"
ARCH_DIR="$ROOT/packaging/arch"

fails=0
ok() { echo "  ok:   $1"; }
bad() { echo "  FAIL: $1" >&2; fails=$((fails + 1)); }

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

echo "== Omarchy recipe check ($RECIPE) =="

for f in "$RECIPE/PKGBUILD" "$RECIPE/k2-bin.install" "$RECIPE/.omarchy/package.json" \
    "$ARCH_DIR/PKGBUILD" "$ARCH_DIR/k2.install" "$ROOT/LICENSE.md"; do
    if [ ! -f "$f" ]; then
        echo "FATAL: missing $f" >&2
        exit 1
    fi
done

if bash -n "$RECIPE/PKGBUILD"; then ok "bash -n PKGBUILD"; else bad "bash -n PKGBUILD"; fi
if bash -n "$RECIPE/k2-bin.install"; then ok "bash -n k2-bin.install"; else bad "bash -n k2-bin.install"; fi

if cmp -s "$RECIPE/k2-bin.install" "$ARCH_DIR/k2.install"; then
    ok "k2-bin.install is packaging/arch/k2.install"
else
    bad "k2-bin.install differs from packaging/arch/k2.install (copy it, and update its sum in the PKGBUILD)"
fi

# The array values, one per line, as bash itself reads the recipe.
values() { # <PKGBUILD> <array name>
    bash -c 'source "$1" >/dev/null; eval "printf \"%s\\n\" \"\${$2[@]}\""' _ "$1" "$2"
}

# ── Licence ──
ABBR="$(sed -n '/^## Abbreviation/,/^## /{/^## /d;/^$/d;p;}' "$ROOT/LICENSE.md" | head -1)"
case "$ABBR" in
    FSL-1.1-Apache-2.0) SPDX=FSL-1.1-ALv2 ;;
    FSL-1.1-MIT) SPDX=FSL-1.1-MIT ;;
    *)
        # No "## Abbreviation" (MIT's text has none): read the first heading or line.
        if grep -q '^MIT License\|^# MIT License\|Permission is hereby granted, free of charge' "$ROOT/LICENSE.md"; then
            SPDX=MIT
        else
            SPDX=""
        fi
        ;;
esac
HAVE_LIC="$(values "$RECIPE/PKGBUILD" license | tr '\n' ' ' | sed 's/ $//')"
if [ -z "$SPDX" ]; then
    bad "can't tell which licence LICENSE.md is; add it to this script's SPDX table (recipe says $HAVE_LIC)"
elif [ "$HAVE_LIC" = "$SPDX" ]; then
    ok "license=($HAVE_LIC) is what LICENSE.md says"
else
    bad "license=($HAVE_LIC), but LICENSE.md is $SPDX"
fi

# ── Dependencies match the package we repackage ──
same_names() { # <array name>
    local a b
    a="$(values "$RECIPE/PKGBUILD" "$1" | sed 's/:.*//' | LC_ALL=C sort)"
    b="$(values "$ARCH_DIR/PKGBUILD" "$1" | sed 's/:.*//' | LC_ALL=C sort)"
    if [ "$a" = "$b" ]; then
        ok "$1 match packaging/arch/PKGBUILD"
    else
        bad "$1 differ from packaging/arch/PKGBUILD: recipe [$(echo "$a" | tr '\n' ' ')] vs [$(echo "$b" | tr '\n' ' ')]"
    fi
}
same_names depends
same_names optdepends

# ── Nothing is compiled, nothing secret ──
if grep -Eq '^[[:space:]]*(build|prepare|pkgver)[[:space:]]*\(\)' "$RECIPE/PKGBUILD"; then
    bad "the recipe has a build(), prepare() or pkgver(): it must only unpack our release"
else
    ok "no build(), prepare() or pkgver()"
fi
if grep -Eq 'K2_GMAIL|K2_MICROSOFT|CLIENT_SECRET|OAUTH|TAURI_SIGNING|require-mail-oauth' "$RECIPE/PKGBUILD"; then
    bad "the recipe names OAuth or signing settings"
else
    ok "no OAuth or signing names"
fi

# ── The source: our release, pinned ──
if python3 -I - "$RECIPE/PKGBUILD" "$RECIPE/k2-bin.install" <<'PY'
import hashlib, re, subprocess, sys
pkgbuild, install = sys.argv[1:3]
out = subprocess.run(
    ["bash", "-c", 'source "$1" >/dev/null; printf "%s\\n" "$pkgname" "$pkgver" "${#source[@]}" "${source[@]}" "${sha256sums[@]}" "${noextract[@]}"', "_", pkgbuild],
    capture_output=True, text=True, check=True).stdout.splitlines()
name, ver, n = out[0], out[1], int(out[2])
src, sums, noext = out[3:3 + n], out[3 + n:3 + 2 * n], out[3 + 2 * n:]
problems = []
if name != "k2-bin":
    problems.append("pkgname is %s" % name)
if n != 2 or len(sums) != 2:
    problems.append("expected 2 sources and 2 sums, got %d and %d" % (n, len(sums)))
else:
    url = "https://github.com/Alakazam-211/K2/releases/download/v%s/k2-%s-x86_64.pkg.tar.zst" % (ver, ver)
    if src[0] != url:
        problems.append("source is %s, expected %s" % (src[0], url))
    if not re.fullmatch(r"[0-9a-f]{64}", sums[0]):
        problems.append("the release package's sum is %r, not a sha256" % sums[0])
    if src[1] != "k2-bin.install":
        problems.append("second source is %s" % src[1])
    have = hashlib.sha256(open(install, "rb").read()).hexdigest()
    if sums[1] != have:
        problems.append("k2-bin.install's sum is %s, the file is %s" % (sums[1], have))
    if noext != ["k2-%s-x86_64.pkg.tar.zst" % ver]:
        problems.append("noextract is %s" % noext)
if problems:
    print("\n".join("        " + p for p in problems), file=sys.stderr)
    sys.exit(1)
PY
then ok "source is our GitHub release, sha256 pinned, noextract set"; else bad "the recipe's source, sums or noextract are wrong (above)"; fi

# ── Their metadata ──
if python3 -I - "$RECIPE/.omarchy/package.json" <<'PY'
import json, re, sys
d = json.load(open(sys.argv[1]))
w = (d.get("upstream") or {}).get("watch") or {}
problems = []
if d.get("source") != "local":
    problems.append("source is %r" % d.get("source"))
if d.get("min_release_age") != "24h":
    problems.append("min_release_age is %r, must be 24h: the package reaches a release after the release itself" % d.get("min_release_age"))
if w.get("github") != "Alakazam-211/K2":
    problems.append("watch.github is %r" % w.get("github"))
p = w.get("pattern") or ""
try:
    ok = re.fullmatch(p, "v0.46.7") and re.fullmatch(p, "v0.46.7").group("version") == "0.46.7" and not re.fullmatch(p, "v0.40.150a")
except (re.error, IndexError):
    ok = False
if not ok:
    problems.append("watch.pattern %r must take v0.46.7 as version 0.46.7 and refuse v0.40.150a" % p)
if "release_ring" in d:
    problems.append("release_ring is set: the GUI links WebKitGTK and glibc from a rolling build host, so the fast ring is not asked for")
if problems:
    print("\n".join("        " + p for p in problems), file=sys.stderr)
    sys.exit(1)
PY
then ok ".omarchy/package.json: watch, 24 h hold, normal ring"; else bad ".omarchy/package.json (above)"; fi

echo
if [ "$fails" -ne 0 ]; then
    echo "Omarchy recipe check: $fails failed" >&2
    exit 1
fi
echo "Omarchy recipe check: all passed"
