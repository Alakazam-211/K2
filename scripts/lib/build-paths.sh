# shellcheck shell=bash
# build-paths.sh: keep the builder's own paths out of release binaries.
#
# A panic location (file!()), a C __FILE__ or a path into a build script's
# OUT_DIR is an absolute path unless the compiler is told to rewrite it, so
# a plain release build carries the builder's home folder and user name
# (0.46.15: every macOS binary held $HOME/.cargo/registry/src/... from its
# dependencies' panic locations).
#
#   k2_release_path_maps <source-dir>
#       Works out the maps, broadest first, because rustc, gcc and clang
#       apply the LAST map that matches a path:
#         $HOME         -> /home
#         <source-dir>  -> /k2
#         $CARGO_HOME   -> /cargo   (default $HOME/.cargo)
#       each as given and, when it differs, as its physical path. A source
#       dir or cargo home inside $HOME therefore maps to /k2 or /cargo, and
#       anything else under $HOME to /home. Sets, exports nothing:
#         K2_RELEASE_RUSTFLAGS  cargo's CARGO_ENCODED_RUSTFLAGS format (one
#                               flag per field, 0x1f between): the caller's
#                               CARGO_ENCODED_RUSTFLAGS, else its RUSTFLAGS,
#                               then the --remap-path-prefix maps.
#         K2_RELEASE_CC_DIR     cc, c++, clang, clang++, gcc, g++ wrappers
#                               that add the same maps as -ffile-prefix-map.
#       Encoded, not RUSTFLAGS: RUSTFLAGS is split on spaces, so it can't
#       carry a checkout path with a space in it. Wrappers, not CFLAGS: the
#       cc crate splits CFLAGS on spaces and cmake joins its flags into one
#       string, so a map with a space breaks both; a wrapper passes each map
#       as one argument. CARGO_ENCODED_RUSTFLAGS, like RUSTFLAGS, replaces
#       any [build] or [target] rustflags in .cargo/config.toml; the repo's
#       has none (tests/packaging/build-paths.sh fails if one appears).
#
#   k2_path_mapped <command...>
#       Runs one command with both: CARGO_ENCODED_RUSTFLAGS and the wrappers
#       first on PATH. Scoped to that one command on purpose: an exported
#       CARGO_ENCODED_RUSTFLAGS would make cargo ignore every later
#       RUSTFLAGS, such as release-gates.sh's RUSTFLAGS='-D warnings'.
#
#   k2_assert_no_build_paths <file-or-dir>...
#       The release gate: fails, naming each file and the string it holds,
#       when a file (every regular file under a dir) holds one of
#         $HOME, $CARGO_HOME and the current dir (every caller runs it from
#               the checkout), each also as its physical path, and only
#               when it has two or more parts (so "/" or "/root" never
#               match every path; a cargo home or checkout outside $HOME,
#               as on a compute node, is caught too),
#         /Users/<user> and /home/<user>, for the user from `id -un` and
#               for $USER,
#       ending at a path boundary (the next byte is not a letter, digit,
#       ".", "_" or "-"), so /home/al never matches /home/alice. Never the
#       bare user name: shipped text names users on purpose (What's New,
#       help examples) and a short name would match almost anything.
#
# Sourced. Sets no shell options. Never reads ~/.k2, never talks to a daemon.

# _k2_path_sq <string>: the string as one single-quoted shell word.
_k2_path_sq() {
    local q="'\\''"
    printf "'%s'" "${1//\'/$q}"
}

# _k2_path_real_tool <name> <skip-dir>: the first <name> on PATH outside
# skip-dir (the wrappers' own folder).
_k2_path_real_tool() {
    local name="$1" skip="$2" d
    local IFS=:
    for d in $PATH; do
        [ -n "$d" ] && [ "$d" != "$skip" ] || continue
        if [ -x "$d/$name" ] && [ ! -d "$d/$name" ]; then
            printf '%s\n' "$d/$name"
            return 0
        fi
    done
    return 1
}

k2_release_path_maps() {
    local src="${1:?usage: k2_release_path_maps <source-dir>}"
    local home="${HOME:-}" cargo_home="${CARGO_HOME:-${HOME:-}/.cargo}"
    local -a flags=() maps=()
    local pair from to phys f
    case "$src" in /*) ;; *) src="$PWD/$src" ;; esac
    # The caller's flags first, the way cargo itself would read them.
    if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
        IFS=$'\x1f' read -r -a flags <<<"$CARGO_ENCODED_RUSTFLAGS"
    elif [ -n "${RUSTFLAGS:-}" ]; then
        read -r -a flags <<<"$RUSTFLAGS"
    fi
    for pair in "$home"$'\t'/home "$src"$'\t'/k2 "$cargo_home"$'\t'/cargo; do
        from="${pair%$'\t'*}" to="${pair##*$'\t'}"
        while [ "${#from}" -gt 1 ] && [ "${from%/}" != "$from" ]; do from="${from%/}"; done
        [ -n "$from" ] && [ "$from" != "/" ] || continue
        case "$from" in
            *$'\n'*|*"'"*)
                echo "FATAL: k2_release_path_maps: a build path has a newline or a quote ($from)" >&2
                return 1
                ;;
        esac
        maps+=("$from=$to")
        phys="$(cd "$from" 2>/dev/null && pwd -P)" || phys=""
        if [ -n "$phys" ] && [ "$phys" != "$from" ]; then
            maps+=("$phys=$to")
        fi
    done
    for f in "${maps[@]}"; do flags+=("--remap-path-prefix=$f"); done
    K2_RELEASE_RUSTFLAGS="$(IFS=$'\x1f'; printf '%s' "${flags[*]}")"

    # The C/C++ wrappers. One folder per set of maps, outside the checkout
    # (a compiler path never has the checkout's space in it), rewritten on
    # every call with the same bytes, so cmake caches keep a stable compiler.
    local tmp="${TMPDIR:-/tmp}" key dir name real w
    tmp="${tmp%/}"
    key="$(printf '%s\n' "${maps[@]}" | cksum | cut -d' ' -f1)"
    dir="$tmp/k2-path-maps-$key"
    mkdir -p "$dir" || { echo "FATAL: k2_release_path_maps: can't make $dir" >&2; return 1; }
    for name in cc c++ clang clang++ gcc g++; do
        real="$(_k2_path_real_tool "$name" "$dir")" || continue
        w="$dir/.$name.$$"
        {
            printf '#!/bin/sh\n# %s with the release path maps (scripts/lib/build-paths.sh)\n' "$name"
            printf 'exec %s' "$(_k2_path_sq "$real")"
            for f in "${maps[@]}"; do printf ' %s' "$(_k2_path_sq "-ffile-prefix-map=$f")"; done
            printf ' "$@"\n'
        } > "$w" && chmod +x "$w" && mv -f "$w" "$dir/$name" \
            || { echo "FATAL: k2_release_path_maps: can't write the $name wrapper in $dir" >&2; return 1; }
    done
    K2_RELEASE_CC_DIR="$dir"
    echo "  path maps on: \$HOME -> /home, the checkout -> /k2, the cargo home -> /cargo (rustc and C/C++)"
}

k2_path_mapped() {
    if [ -z "${K2_RELEASE_RUSTFLAGS:-}" ] || [ ! -d "${K2_RELEASE_CC_DIR:-}" ]; then
        echo "FATAL: k2_path_mapped: no path maps (call k2_release_path_maps first)" >&2
        return 1
    fi
    CARGO_ENCODED_RUSTFLAGS="$K2_RELEASE_RUSTFLAGS" PATH="$K2_RELEASE_CC_DIR:$PATH" "$@"
}

k2_assert_no_build_paths() {
    [ $# -gt 0 ] || { echo "FATAL: k2_assert_no_build_paths: no files given" >&2; return 1; }
    local p home phys u
    local -a needles=()
    for p in "$@"; do
        [ -e "$p" ] || { echo "FATAL: k2_assert_no_build_paths: $p does not exist" >&2; return 1; }
    done
    for home in "${HOME:-}" "${CARGO_HOME:-}" "$PWD"; do
        while [ "${#home}" -gt 1 ] && [ "${home%/}" != "$home" ]; do home="${home%/}"; done
        case "$home" in
            /?*/?*)
                needles+=("$home")
                phys="$(cd "$home" 2>/dev/null && pwd -P)" || phys=""
                [ -z "$phys" ] || [ "$phys" = "$home" ] || needles+=("$phys")
                ;;
        esac
    done
    for u in "$(id -un 2>/dev/null || true)" "${USER:-}"; do
        case "$u" in ""|*/*|*$'\n'*) continue ;; esac
        needles+=("/Users/$u" "/home/$u")
    done
    python3 -I - "$(printf '%s\n' "${needles[@]}")" "$@" <<'PY'
import mmap, os, re, sys

needles = sorted({n for n in sys.argv[1].split("\n") if n}, key=len, reverse=True)
pat = re.compile(b"(?:" + b"|".join(re.escape(n.encode()) for n in needles) + b")(?![A-Za-z0-9._-])")

def files(root):
    if os.path.isfile(root):
        yield root
        return
    for d, _, names in os.walk(root):
        for n in sorted(names):
            p = os.path.join(d, n)
            if os.path.isfile(p) and not os.path.islink(p):
                yield p

def shown(data, m):
    # The printable run around the hit, at most 160 bytes.
    s, e = m.start(), m.end()
    while s > 0 and m.start() - s < 80 and 32 <= data[s - 1] < 127:
        s -= 1
    while e < len(data) and e - m.end() < 80 and 32 <= data[e] < 127:
        e += 1
    return data[s:e].decode("ascii", "replace")

bad = 0
scanned = 0
for root in sys.argv[2:]:
    for p in files(root):
        scanned += 1
        if os.path.getsize(p) == 0:
            continue
        with open(p, "rb") as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as data:
            hits = list(pat.finditer(data))
            if hits:
                bad += 1
                first = hits[0]
                print("FATAL: %s holds the builder's path %r (%d times), e.g. %r"
                      % (p, first.group().decode("utf-8", "replace"), len(hits), shown(data, first)), file=sys.stderr)
if bad:
    print("FATAL: %d file(s) carry the builder's paths (home, cargo home, checkout or user name); the release path maps"
          " (scripts/lib/build-paths.sh) missed them" % bad, file=sys.stderr)
    sys.exit(1)
print("  no builder paths in %d file(s) (home, cargo home, checkout and user name checked)" % scanned)
PY
}
