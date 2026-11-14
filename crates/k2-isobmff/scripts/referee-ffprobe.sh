#!/bin/sh
# FFmpeg's ffprobe as a referee for k2-isobmff's sample tables (prd-video-
# tab-media-v1 C1: "frame tables equal FFmpeg's, a referee on nodes"). Run on
# a Linux x86_64 K2 Compute node only: it fetches a pinned static FFmpeg
# build (BtbN, GPL; the same release E5 used) into a scratch folder, makes
# clips with lavfi (H.264 MOV with B-frames, AAC and a 01:00:00:00 tmcd;
# ProRes HQ MOV with PCM and a 29.97 drop-frame tmcd; fragmented MP4 with
# H.264 and Opus; HEVC 10-bit; AV1; VFR; 24-bit LPCM), and compares every
# packet ffprobe lists (-ignore_editlist 1) with dump_tables: count, size,
# file offset and key flag exactly, pts and dts up to one constant shift
# per track (FFmpeg shifts dts when composition offsets are negative).
# FFmpeg is never installed, linked or shipped; everything is deleted after.
# Usage, from the repository root:  sh crates/k2-isobmff/scripts/referee-ffprobe.sh
set -eu
W=${TMPDIR:-/tmp}/k2-isobmff-referee.$$
mkdir -p "$W/c"
trap 'rm -rf "$W"' EXIT
FFREL=autobuild-2026-11-06-12-50
FFTAR=ffmpeg-n8.1.3-34-ga35157abb9-linux64-gpl-8.1.tar.xz
FFSHA=2d17c1f53aa6e15882ff76d2a0089d63c3553a401bf2bdc723d798beb5eb4f1b
curl -fsSL --retry 3 -o "$W/$FFTAR" "https://github.com/BtbN/FFmpeg-Builds/releases/download/$FFREL/$FFTAR"
[ "$(sha256sum "$W/$FFTAR" | cut -d' ' -f1)" = "$FFSHA" ] || { echo "ffmpeg sha256 mismatch"; exit 1; }
tar -xJf "$W/$FFTAR" -C "$W" && rm "$W/$FFTAR"
FF=$(ls -d "$W"/ffmpeg-*/bin)/ffmpeg; FP=$(dirname "$FF")/ffprobe
"$FF" -hide_banner -version | head -1
cargo build -q -p k2-isobmff --example dump_tables
DUMP=$(cargo metadata -q --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug/examples/dump_tables
[ -x "$DUMP" ] || { echo "no dump_tables binary at $DUMP"; exit 1; }

ff() { "$FF" -hide_banner -v error -y "$@"; }
V="-f lavfi -i testsrc2=s=640x360"
A="-f lavfi -i sine=f=440:r=48000"
c() { name=$1; shift; if ff "$@" "$W/c/$name"; then echo "clip $name"; else echo "SKIP $name (ffmpeg could not make it)"; fi; }
c h264-bframes-aac-tmcd.mov $V:r=24000/1001:d=12 $A:d=12 -map 0 -map 1 -c:v libx264 -preset veryfast -bf 3 -g 48 -pix_fmt yuv420p -c:a aac -b:a 128k -timecode 01:00:00:00
c prores-hq-pcm-df.mov $V:r=30000/1001:d=3 $A:d=3 -map 0 -map 1 -c:v prores_ks -profile:v 3 -pix_fmt yuv422p10le -c:a pcm_s16le -timecode "01:00:00;00"
c frag-h264-opus.mp4 $V:r=25:d=10 $A:d=10 -map 0 -map 1 -c:v libx264 -preset veryfast -bf 2 -g 50 -pix_fmt yuv420p -c:a libopus -b:a 96k -movflags frag_keyframe+empty_moov+default_base_moof
c hevc-main10.mp4 $V:r=30:d=4 -c:v libx265 -preset ultrafast -pix_fmt yuv420p10le -tag:v hvc1 -x265-params bframes=4:keyint=60:log-level=error -movflags +faststart
c av1.mp4 $V:r=30:d=4 -c:v libsvtav1 -preset 12 -g 60 -pix_fmt yuv420p
c vfr-h264.mp4 $V:r=60:d=6 -vf "select='not(eq(mod(n\,7)\,3))*not(eq(mod(n\,11)\,5))'" -fps_mode vfr -c:v libx264 -preset veryfast -g 120 -pix_fmt yuv420p
c lpcm24.mov $V:r=25:d=4 $A:d=4 -map 0 -map 1 -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a pcm_s24le

status=0
for f in "$W"/c/*; do
    n=$(basename "$f")
    echo "== $n ($(wc -c < "$f") bytes)"
    if ! "$DUMP" "$f" > "$W/ours.txt"; then echo "DIFF $n: our reader refused it"; status=1; continue; fi
    grep -E '^(movie|track|timecode)' "$W/ours.txt"
    "$FP" -v error -show_entries stream=index,codec_name,codec_tag_string,time_base:stream_tags=timecode -of compact=p=0 "$f" | sed 's/^/ffprobe stream: /'
    grep '^track ' "$W/ours.txt" | while read -r _ i rest; do
        codec=$(echo "$rest" | sed -n 's/.*codec=\([^ ]*\).*/\1/p')
        awk -v i="$i" '$1=="S" && $2==i {print $3, $4, $5, $6, $7}' "$W/ours.txt" > "$W/o.$i"
        "$FP" -v error -ignore_editlist 1 -select_streams "$i" -show_entries packet=pts,dts,size,pos,flags -of csv=p=0 "$f" | tr ',' ' ' > "$W/f.$i"
        case "$codec" in
        pcm.*)
            # FFmpeg reads raw PCM a chunk per packet: compare the bytes and
            # that every packet starts at one of our sample offsets.
            awk -v label="$n track $i ($codec)" 'NR==FNR { s += $3; off[$4] = 1; c++; next } { fs += $3; fc++; if (!($4 in off)) miss++ }
                END { ok = (s == fs && !miss); printf "%s %s: %d samples, %d bytes; ffprobe %d packets, %d bytes, %d packet starts not on a sample\n", ok ? "MATCH" : "DIFF", label, c, s, fc, fs, miss + 0; exit !ok }' "$W/o.$i" "$W/f.$i" || echo FAIL > "$W/failed"
            ;;
        *)
            no=$(wc -l < "$W/o.$i"); nf=$(wc -l < "$W/f.$i")
            paste -d' ' "$W/o.$i" "$W/f.$i" | awk -v label="$n track $i ($codec)" -v no="$no" -v nf="$nf" '
                { if (NR == 1) { dp = $6 - $1; dd = $7 - $2 }
                  if ($6 - $1 != dp) bp++; if ($7 - $2 != dd) bd++
                  if ($3 != $8) bs++; if ($4 != $9) bo++
                  k = substr($10, 1, 1) == "K" ? "K" : "_"; if ($5 != k) bk++ }
                END { ok = (no == nf && !bp && !bd && !bs && !bo && !bk)
                      printf "%s %s: ours %d, ffprobe %d packets; mismatches size %d, offset %d, key %d; pts shift %d (varies %d), dts shift %d (varies %d)\n", ok ? "MATCH" : "DIFF", label, no, nf, bs + 0, bo + 0, bk + 0, dp, bp + 0, dd, bd + 0; exit !ok }' || echo FAIL > "$W/failed"
            ;;
        esac
    done
    [ -f "$W/failed" ] && { status=1; rm -f "$W/failed"; }
done
[ "$status" -eq 0 ] && echo "== referee: every track matches ffprobe" || echo "== referee: differences above"
exit "$status"
