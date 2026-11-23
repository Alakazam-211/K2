# shellcheck shell=bash
# root-helper-size.sh — the root helper's size gate.
#
# A box's installed k2-root-helper updates itself (`helper self-update`)
# from a payload of <u32 BE sig len><minisign sig><ELF> on stdin, and it
# refuses a payload over its own cap. Every helper up to 0.46.17 has an
# 8 MiB cap (k2_root_proto::verbs::CAP_SELF_UPDATE_LEGACY); 0.46.18 and
# later take 32 MiB (CAP_SELF_UPDATE). A release must reach the OLDEST
# helper still in the field, so the limit is the smallest cap among them:
# 8 MiB, less 4 KiB for the signature and its length prefix
# (SELF_UPDATE_SIG_ROOM). 0.46.17's helper was 8,836,200 bytes (x86_64),
# so no 0.46.16 helper could update to it.
#
# Raise K2_ROOT_HELPER_FIELDED_CAP only once no supported box runs a
# helper older than 0.46.18 (k2-root-proto's tests keep these two numbers
# equal to the Rust constants).
#
# Called by scripts/ci/build-linux-bins.sh (the build job), k2cr_stage
# (scripts/lib/compute-release.sh, before release.sh publishes) and
# scripts/sign-root-helper.sh (before the root-helper key signs).
# Sets no shell options.

K2_ROOT_HELPER_FIELDED_CAP=8388608
K2_ROOT_HELPER_SIG_ROOM=4096

# k2_root_helper_size_gate <helper>...: prints each helper's size; returns
# 1 when any is missing or over the limit (and when given no file at all).
k2_root_helper_size_gate() {
    local max=$((K2_ROOT_HELPER_FIELDED_CAP - K2_ROOT_HELPER_SIG_ROOM))
    local warn=$((K2_ROOT_HELPER_FIELDED_CAP - 1024 * 1024))
    local f n bad=0
    if [ "$#" = 0 ]; then
        echo "FATAL: root-helper size gate: no helper given" >&2
        return 1
    fi
    for f in "$@"; do
        if [ ! -f "$f" ]; then
            echo "FATAL: root-helper size gate: $f is missing" >&2
            bad=1
            continue
        fi
        n="$(wc -c < "$f" | tr -d ' ')"
        if [ "$n" -gt "$max" ]; then
            echo "FATAL: $(basename "$f") is $n bytes, over the $max-byte limit." >&2
            echo "  Helpers already on boxes (0.46.17 and older) refuse a self-update payload over 8 MiB" >&2
            echo "  ($K2_ROOT_HELPER_FIELDED_CAP bytes, signature included), so they could never update to this build." >&2
            echo "  Shrink crates/k2-root-helper (the embedded K2 CLI is compressed; look for new embeds or" >&2
            echo "  dependencies) before releasing." >&2
            bad=1
        elif [ "$n" -gt "$warn" ]; then
            echo "root-helper size: $(basename "$f") $n bytes (limit $max, $((max - n)) to spare)"
            echo "WARNING: $(basename "$f") is within 1 MiB of the 8 MiB self-update cap fielded helpers enforce." >&2
        else
            echo "root-helper size: $(basename "$f") $n bytes (limit $max, $((max - n)) to spare)"
        fi
    done
    return "$bad"
}
