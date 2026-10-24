#!/usr/bin/env bash
# U0 spike (c) — prd-compute-update-continuity-v1 §5.1, C8, R4, Q8.
#
# Question: can the node's spool tailer free log space it already read
# while the job keeps appending? A job writes its stdout to a regular file
# opened O_APPEND (the spool); the tailer reads new bytes from its offset
# and punches a hole over what it read (Linux fallocate PUNCH_HOLE |
# KEEP_SIZE; macOS fcntl F_PUNCHHOLE). Disk use must stay small under a
# flood while the file's size keeps growing. Also: what the tailer sees
# when the job truncates the file, or jumps far ahead (a sparse write).
#
# No root needed. It compiles a small C probe with `cc` (Xcode CLT on a
# Mac; build-essential on Linux) into its own temp dir and writes only
# under --dir. Run it on the SAME filesystem as the node home:
#   Linux node: --dir /var/tmp (when /var/lib is on the same fs; check df)
#   Mac node:   --dir /var/tmp (APFS, same container as /var/k2node)
# Boxes: one Linux node with ext4 (k2n4-vin/k2n5-vin), one with xfs if the
# fleet has one, and a Mac mini (APFS). It can run as a compute job:
#   k2 compute run <node> --sha <sha> -- bash scripts/test/compute-continuity-spike/spool-punch.sh --dir /var/tmp
#
#   bash spool-punch.sh [--dir DIR] [--mb 512] [--report PATH]
#
# Exit 0 = the probe ran (read the table; "unsupported" is a result, R4);
# 1 = the probe itself failed.
set -euo pipefail

DIR="${TMPDIR:-/var/tmp}"
MB=512
REPORT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --dir) DIR="${2:-}"; shift 2 ;;
        --mb) MB="${2:-}"; shift 2 ;;
        --report) REPORT="${2:-}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
command -v cc >/dev/null || { echo "needs cc (Xcode CLT / build-essential)" >&2; exit 2; }
W="$(mktemp -d "$DIR/k2spike-spool.XXXXXX")"
trap 'rm -rf "$W"' EXIT
REPORT="${REPORT:-$W/../k2-spike-spool-punch.txt}"

cat > "$W/probe.c" <<'EOF'
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* Punch [off, off+len): 0 or errno. */
static int punch(int fd, off_t off, off_t len) {
#ifdef __APPLE__
    struct fpunchhole a = {0, 0, off, len};
    return fcntl(fd, F_PUNCHHOLE, &a) == 0 ? 0 : errno;
#else
    return fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, off, len) == 0 ? 0 : errno;
#endif
}

/* probe <spool> <done-flag>: tail until the writer is done and all is read. */
int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: probe <spool> <done-flag>\n"); return 2; }
    int fd = open(argv[1], O_RDWR | O_NOFOLLOW);
    if (fd < 0) { perror("open"); return 1; }
    struct stat st;
    if (fstat(fd, &st) != 0) { perror("fstat"); return 1; }
    off_t blk = st.st_blksize > 0 ? st.st_blksize : 4096;
    off_t off = 0, punched = 0, read_total = 0, max_alloc = 0, truncs = 0, jumps = 0;
    int punch_err = -1;
    static char buf[1 << 20];
    for (;;) {
        if (fstat(fd, &st) != 0) { perror("fstat"); return 1; }
        off_t alloc = (off_t)st.st_blocks * 512;
        if (alloc > max_alloc) max_alloc = alloc;
        if (st.st_size < off) { truncs++; off = st.st_size; punched = off & ~(blk - 1); }
        if (st.st_size - off > (off_t)256 << 20) {   /* a jump past the cap: skip, never read it */
            jumps++; off = st.st_size;
        }
        int got = 0;
        while (off < st.st_size) {
            off_t want = st.st_size - off; if (want > (off_t)sizeof buf) want = sizeof buf;
            ssize_t n = pread(fd, buf, (size_t)want, off);
            if (n <= 0) break;
            off += n; read_total += n; got = 1;
        }
        off_t upto = off & ~(blk - 1);
        if (upto > punched && punch_err != 0 && punch_err != -1) { /* unsupported: stop trying */ }
        else if (upto > punched) {
            int e = punch(fd, punched, upto - punched);
            if (punch_err == -1) punch_err = e;
            if (e == 0) punched = upto;
        }
        if (!got) {
            if (access(argv[2], F_OK) == 0) {
                if (fstat(fd, &st) == 0 && st.st_size <= off) break;
            }
            usleep(100000); /* the tailer polls (<= 250 ms) */
        }
    }
    fstat(fd, &st);
    printf("read=%lld size=%lld max_alloc=%lld final_alloc=%lld blksize=%lld punch=%s truncations=%lld jumps=%lld\n",
           (long long)read_total, (long long)st.st_size, (long long)max_alloc, (long long)st.st_blocks * 512,
           (long long)blk, punch_err == 0 ? "ok" : (punch_err == -1 ? "never-tried" : strerror(punch_err)),
           (long long)truncs, (long long)jumps);
    return 0;
}
EOF
cc -O2 -o "$W/probe" "$W/probe.c"

FS="$( (df -T "$W" 2>/dev/null || df "$W") | awk 'NR==2{print $2}')"
[ "$(uname -s)" = "Darwin" ] && FS="$(diskutil info "$(df "$W" | awk 'NR==2{print $1}')" 2>/dev/null | awk -F: '/Type \(Bundle\)/{gsub(/ /,"",$2); print $2}')"

# Case 1: a flood. The writer appends MB megabytes (O_APPEND, like a job's
# stdout on the spool fd) while the probe tails and punches.
: > "$W/flood.out"
( exec >>"$W/flood.out"; head -c $((MB * 1024 * 1024)) /dev/zero | tr '\0' 'x' | fold -w 200; touch "$W/flood.done" ) &
FLOOD="$("$W/probe" "$W/flood.out" "$W/flood.done")"
wait

# Case 2: the job truncates its stdout halfway, then keeps writing.
: > "$W/trunc.out"
(
    exec >>"$W/trunc.out"
    for i in $(seq 1 2000); do echo "line $i before"; done
    sleep 0.5
    : > "$W/trunc.out"   # truncate through another fd, as `> file` in the job would
    for i in $(seq 1 2000); do echo "line $i after"; done
    sleep 0.5
    touch "$W/trunc.done"
) &
TRUNC="$("$W/probe" "$W/trunc.out" "$W/trunc.done")"
wait

# Case 3: a sparse jump far past the log cap (dd seek), then more lines.
: > "$W/jump.out"
(
    exec 3>>"$W/jump.out"
    echo "first" >&3
    sleep 0.4
    dd if=/dev/zero of="$W/jump.out" bs=1 count=1 seek=$((2 * 1024 * 1024 * 1024)) conv=notrunc 2>/dev/null
    sleep 0.4
    echo "after the jump" >&3
    sleep 0.4
    touch "$W/jump.done"
) &
JUMP="$("$W/probe" "$W/jump.out" "$W/jump.done")"
wait

{
    echo "U0 spike (c): spool files and freeing read log space — $(date -u +%FT%TZ)"
    echo "host $(hostname)  $(uname -sr)  dir $DIR  fs ${FS:-unknown}"
    echo
    echo "flood ${MB} MB:      $FLOOD"
    echo "truncate halfway:   $TRUNC"
    echo "sparse 2 GB jump:   $JUMP"
    echo
    echo "Read: flood punch=ok with max_alloc far below ${MB} MB = hole punching bounds the spool (Q8)."
    echo "punch=<error> (e.g. Operation not supported) = R4: drop past the cap without punching; warn in k2-node status."
    echo "truncations>=1 = the tailer saw size < offset and went on from the new end (C8)."
    echo "jumps>=1 with read far below the size = the sparse jump was skipped, never read byte by byte."
} > "$REPORT"
echo "report: $REPORT"
cat "$REPORT"
