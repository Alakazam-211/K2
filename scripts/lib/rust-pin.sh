#!/usr/bin/env bash
# rust-pin.sh — the Rust version K2 is pinned to (rust-toolchain.toml),
# for scripts that send K2 Compute jobs or check a build machine.
#
# Sourced; sets no shell options.
#
#   k2_rust_pin [REV]    the channel in rust-toolchain.toml at commit REV
#                        (git show REV:rust-toolchain.toml, in the repo
#                        $K2_RUST_PIN_REPO, default this checkout), or in
#                        this checkout's working tree. Exit 1 and no output when
#                        there is no file there or it names no channel; exit
#                        2 when REV isn't a commit in that repo.
#   k2_rust_need [REV]   rust@<pin>. For a commit from before the pin (old
#                        tags) it prints rust@stable and a note on stderr:
#                        those were built that way. A REV that isn't in the
#                        repo at all fails (exit 1), never a guess.
#
# Why the pin is read at the commit a job builds: an explicit
# `--needs rust@X` sets RUSTUP_TOOLCHAIN=X on the node, which overrides the
# job checkout's rust-toolchain.toml. Sending the working tree's pin (or
# rust@stable) for another commit would build it with the wrong rustc.

K2_RUST_PIN_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# The channel from rust-toolchain.toml text on stdin.
_k2_rust_pin_parse() {
    sed -n -E 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"([^"]+)".*$/\1/p' | head -n 1
}

k2_rust_pin() {
    local rev="${1:-}" pin repo="${K2_RUST_PIN_REPO:-$K2_RUST_PIN_ROOT}"
    if [ -n "$rev" ]; then
        if ! git -c safe.directory='*' -C "$repo" cat-file -e "${rev}^{commit}" 2>/dev/null; then
            echo "FATAL: commit $rev is not in $repo (fetch it first)" >&2
            return 2
        fi
        pin="$(git -c safe.directory='*' -C "${K2_RUST_PIN_REPO:-$K2_RUST_PIN_ROOT}" show "${rev}:rust-toolchain.toml" 2>/dev/null | _k2_rust_pin_parse)"
    elif [ -f "$K2_RUST_PIN_ROOT/rust-toolchain.toml" ]; then
        pin="$(_k2_rust_pin_parse < "$K2_RUST_PIN_ROOT/rust-toolchain.toml")"
    fi
    [ -n "${pin:-}" ] || return 1
    printf '%s\n' "$pin"
}

k2_rust_need() {
    local pin rc=0
    pin="$(k2_rust_pin "${1:-}")" || rc=$?
    case "$rc" in
        0) printf 'rust@%s\n' "$pin" ;;
        1)
            echo "  note: no rust-toolchain.toml at ${1:-this checkout}; using rust@stable (built before the pin)" >&2
            printf 'rust@stable\n'
            ;;
        *) return 1 ;;
    esac
}

# k2_rust_pin_check — this machine's cargo builds with the pin (release.sh
# Step 0.4). With rustup: install the pinned toolchain if it's missing
# (a no-op otherwise), then `rustc -V` here must name it. Without rustup
# (Homebrew's cargo ignores rust-toolchain.toml): refuse, with the fix.
# Escape: K2_SKIP_RUST_PIN_CHECK=1 (loud). Returns non-zero on refusal.
k2_rust_pin_check() {
    local pin got
    if [ "${K2_SKIP_RUST_PIN_CHECK:-0}" = "1" ]; then
        echo "  ⚠⚠ RUST PIN CHECK SKIPPED (K2_SKIP_RUST_PIN_CHECK=1): this build may use another rustc than rust-toolchain.toml ⚠⚠" >&2
        return 0
    fi
    pin="$(k2_rust_pin)" || { echo "FATAL: no channel in $K2_RUST_PIN_ROOT/rust-toolchain.toml" >&2; return 1; }
    if ! command -v rustup >/dev/null 2>&1; then
        echo "FATAL: rustup is not installed, so cargo here ignores rust-toolchain.toml (Rust $pin)." >&2
        echo "  Install rustup (https://rustup.rs), then: rustup toolchain install $pin --profile minimal -c rustfmt,clippy" >&2
        echo "  (Homebrew's rust can stay, but rustup's cargo must come first on PATH.)" >&2
        return 1
    fi
    if ! rustup toolchain list 2>/dev/null | grep -q "^${pin}-"; then
        echo "  Installing Rust $pin (rust-toolchain.toml)..."
        rustup toolchain install "$pin" --profile minimal -c rustfmt,clippy --no-self-update >/dev/null \
            || { echo "FATAL: rustup toolchain install $pin failed" >&2; return 1; }
    fi
    got="$(cd "$K2_RUST_PIN_ROOT" && rustc -V 2>/dev/null || true)"
    case "$got" in
        "rustc $pin "*|"rustc $pin") echo "  Rust: $got (rust-toolchain.toml pins $pin)." ;;
        *)
            echo "FATAL: rustc here is '${got:-missing}', but rust-toolchain.toml pins $pin." >&2
            echo "  Is RUSTUP_TOOLCHAIN set, or a non-rustup rustc first on PATH? (which rustc: $(command -v rustc || echo none))" >&2
            return 1
            ;;
    esac
}
