#!/usr/bin/env bash
# S6 proof — prd-compute-gpu-jobs-v1 §3.7 (APU memory, GH#79).
#
# Proves on a real APU node what the S6 code assumes and its tests fake:
#   P1  GPU buffers (GTT, system RAM) are NOT charged to the unit's
#       MemoryMax: a job unit under MemoryMax=512M allocates well over it
#       and keeps running (C4). This is why --gpu-mem is a second budget.
#   P2  The executor's reading (`k2-node admin gpu usage`, the same code as
#       the `gpu_usage` op) sees that unit's own GPU memory: its GTT within
#       a few percent of what the program allocated, and the device's
#       sysfs mem_info_gtt_used rose by about as much.
#   P3  Two DRM clients in one unit are counted separately and a forked fd
#       once (a second process allocating adds its own).
#   P4  The GPU memory is given back when the unit stops.
#   P5  VRAM (the BIOS carve-out) allocations show in the reading too (the
#       budget counts every region), and are NOT system RAM.
#   C6  /dev/kfd (ROCm) is never in a v1 job: reported, not tested.
#
# Run it as root AT THE APU NODE (a person or K2 Hardware, never a job):
#   sudo bash s6-gpu-mem-proof.sh [--user k2job] [--device renderD128]
#        [--report PATH] [--no-wait-locks] [--no-pause] [--keep-binary]
# Options:
#   --user NAME      the job user the units run as (default k2job)
#   --device NAME    the render node (default: the first renderD* of an
#                    amdgpu device)
#   --report PATH    the report (default /var/tmp/k2-gpu-s6-report.txt)
#   --no-wait-locks  don't wait for the CI job lock or K2's smoke lock
#   --no-pause       don't pause k2-node (only if it's already paused)
#   --keep-binary    leave the allocator at /usr/local/libexec/k2-s6-gpu-alloc
#                    and print the end-to-end k2 compute command (step E2E);
#                    remove it afterwards with: sudo rm /usr/local/libexec/k2-s6-gpu-alloc
#
# Before it starts (it checks and prints these again):
#   - It pauses this node (sudo k2-node pause) for its whole run, because it
#     runs as root on a shared node, and resumes it at the end (also on
#     Ctrl-C or an error). A node paused before it started stays paused.
#   - Lid OPEN, mains power. Or tell K2 Hardware first so it can watch the
#     GPU edge temperature: the clamshell thermal regulator caps the CPU
#     only, and GPU-heavy load with the lid closed is untested.
#   - Sizes come from this boot's numbers (the carve-out, the RAM Linux
#     sees, the GTT size), never fixed ones. A test that filled RAM past
#     what Linux sees would OOM the whole box: it allocates at most a
#     quarter of MemAvailable at a time.
#
# It changes nothing that lasts (without --keep-binary): transient units
# (k2-s6-proof-*, collected), a scratch folder in /run/k2-s6-proof (removed),
# and the node's pause (undone). Needs cc and the Vulkan headers (Arch:
# base-devel vulkan-headers vulkan-icd-loader; Debian: build-essential
# libvulkan-dev); a missing tool is reported SKIP with the package, never a
# pass. About two minutes.

set -uo pipefail

JOB_USER="k2job"
DEVICE=""
REPORT="/var/tmp/k2-gpu-s6-report.txt"
WAIT_LOCKS=1
PAUSE=1
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --user) JOB_USER="${2:-}"; shift 2 ;;
        --device) DEVICE="${2:-}"; shift 2 ;;
        --report) REPORT="${2:-}"; shift 2 ;;
        --no-wait-locks) WAIT_LOCKS=0; shift ;;
        --no-pause) PAUSE=0; shift ;;
        --keep-binary) KEEP=1; shift ;;
        -h|--help) sed -n '2,52p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1 (--help)" >&2; exit 2 ;;
    esac
done

[ "$(id -u)" = "0" ] || { echo "run it as root: sudo bash $0" >&2; exit 2; }
command -v systemd-run >/dev/null || { echo "systemd-run not found: GPU jobs need the Linux job wall (systemd)" >&2; exit 2; }
command -v k2-node >/dev/null || { echo "k2-node not found: install k2-node 0.46.11 or later first" >&2; exit 2; }
id "$JOB_USER" >/dev/null 2>&1 || { echo "no user $JOB_USER (the job wall's user; sudo k2-node setup makes k2job)" >&2; exit 2; }
if ! k2-node admin gpu usage 2>&1 | grep -q -- '--unit'; then
    echo "this k2-node has no 'admin gpu usage' (older than 0.46.11): update it first" >&2; exit 2
fi

: > "$REPORT"
say() { printf '%s\n' "$*" | tee -a "$REPORT"; }
res() { printf '%-6s %-4s %s\n' "$1" "$2" "$3" | tee -a "$REPORT"; }
mib() { echo $(( $1 / 1048576 )); }

# ── the node's numbers, this boot ─────────────────────────────────────
if [ -z "$DEVICE" ]; then
    for r in /sys/class/drm/renderD*; do
        [ -e "$r" ] || continue
        if [ "$(basename "$(readlink -f "$r/device/driver" 2>/dev/null)")" = "amdgpu" ]; then DEVICE="$(basename "$r")"; break; fi
    done
fi
[ -n "$DEVICE" ] || { echo "no amdgpu render node here: this proof is for an AMD APU node" >&2; exit 2; }
R="/dev/dri/$DEVICE"
DEV="$(readlink -f "/sys/class/drm/$DEVICE/device")"
PCI="$(basename "$DEV")"
[ -c "$R" ] && [ -r "$DEV/mem_info_gtt_total" ] || { echo "$R or its mem_info_* files are missing" >&2; exit 2; }
CARVE="$(cat "$DEV/mem_info_vram_total")"
VIS="$(cat "$DEV/mem_info_vis_vram_total" 2>/dev/null || echo 0)"
GTT_TOTAL="$(cat "$DEV/mem_info_gtt_total")"
MEM_TOTAL=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) * 1024 ))
MEM_AVAIL=$(( $(awk '/^MemAvailable:/ {print $2}' /proc/meminfo) * 1024 ))
gtt_used() { cat "$DEV/mem_info_gtt_used"; }
vram_used() { cat "$DEV/mem_info_vram_used"; }

# Allocation sizes: a quarter of what's available, a third of GTT, at most
# 3 GiB; it must still be at least twice the unit's MemoryMax to prove P1.
MEMMAX_MB=512
ALLOC_MB=$(( $(mib "$MEM_AVAIL") / 4 ))
G3=$(( $(mib "$GTT_TOTAL") / 3 ))
[ "$G3" -lt "$ALLOC_MB" ] && ALLOC_MB="$G3"
[ "$ALLOC_MB" -gt 3072 ] && ALLOC_MB=3072
ALLOC_MB=$(( ALLOC_MB / 256 * 256 ))
SECOND_MB=256
VRAM_MB=$(( $(mib "$CARVE") / 8 ))
[ "$VRAM_MB" -gt 1024 ] && VRAM_MB=1024
VRAM_MB=$(( VRAM_MB / 256 * 256 ))

say "K2 GPU jobs — S6 APU memory proof ($(date -u +%FT%TZ))"
say "kernel $(uname -r) · systemd $(systemctl --version | head -1 | awk '{print $2}') · k2-node $(k2-node --version 2>/dev/null | head -1)"
say "device $DEVICE ($PCI, $(basename "$(readlink -f "$DEV/driver")"))"
say "this boot: carve-out (VRAM) $(mib "$CARVE") MiB (CPU-visible $(mib "$VIS") MiB) · RAM Linux sees $(mib "$MEM_TOTAL") MiB, available $(mib "$MEM_AVAIL") MiB · GTT $(mib "$GTT_TOTAL") MiB (used $(mib "$(gtt_used)") MiB)"
say "sizes: GTT allocation ${ALLOC_MB} MiB under MemoryMax=${MEMMAX_MB}M · second client ${SECOND_MB} MiB · VRAM allocation ${VRAM_MB} MiB"
say ""
say "NOTES: lid open and on mains (or K2 Hardware watching the GPU edge temperature: the thermal"
say "regulator caps the CPU only). The node is paused for the whole run and resumed at the end."
say "Allocations stay well under the RAM Linux sees; filling it would OOM the whole box."
say ""
if [ "$ALLOC_MB" -lt $(( MEMMAX_MB * 2 )) ]; then
    say "only ${ALLOC_MB} MiB to allocate safely (less than twice MemoryMax): free memory first; nothing tested"
    exit 1
fi

# ── locks, pause, scratch, cleanup ────────────────────────────────────
LOCK_CI="/home/sew-ci/sew-build/.job.lock"
LOCK_K2="/var/tmp/k2-smoke.lock"
WROTE_LOCK=0
PAUSED_BY_ME=0
SCRATCH=/run/k2-s6-proof
cleanup() {
    systemctl stop 'k2-s6-proof-*' >/dev/null 2>&1 || true
    rm -rf "$SCRATCH"
    if [ "$PAUSED_BY_ME" = 1 ]; then
        if k2-node resume --by s6-proof >/dev/null 2>&1; then
            echo "k2-node resumed"
        else
            echo "COULD NOT RESUME k2-node: run sudo k2-node resume" >&2
        fi
    fi
    if [ "$WROTE_LOCK" = 1 ]; then rm -f "$LOCK_K2"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
if [ "$WAIT_LOCKS" = 1 ]; then
    while [ -e "$LOCK_CI" ]; do echo "waiting: a CI job holds $LOCK_CI"; sleep 30; done
    if [ -e "$LOCK_K2" ]; then
        echo "another K2 smoke holds $LOCK_K2:" >&2; cat "$LOCK_K2" >&2; exit 3
    fi
    printf 'k2 GPU jobs S6 proof\nstarted %s\npid %s\n' "$(date -u +%FT%TZ)" "$$" > "$LOCK_K2"
    WROTE_LOCK=1
fi
if [ "$PAUSE" = 1 ]; then
    k2-node pause --by s6-proof || { echo "k2-node pause failed: nothing tested" >&2; exit 2; }
    PAUSED_BY_ME=1
    say "k2-node paused (it resumes at the end)"
    # A job that was running finishes first: wait for no k2job unit but ours.
    for _ in $(seq 1 120); do
        systemctl list-units --type=service --state=active --no-legend 'k2job-*' 2>/dev/null | grep -q . || break
        sleep 5
    done
    if systemctl list-units --type=service --state=active --no-legend 'k2job-*' 2>/dev/null | grep -q .; then
        say "K2 jobs are still running after 10 minutes; their GPU use would mix with ours: nothing tested"
        exit 3
    fi
fi
mkdir -p "$SCRATCH" && chmod 0755 "$SCRATCH"

# ── the allocator ─────────────────────────────────────────────────────
if ! command -v cc >/dev/null || ! [ -e /usr/include/vulkan/vulkan.h ]; then
    res BUILD SKIP "needs cc and the Vulkan headers (Arch: base-devel vulkan-headers; Debian: build-essential libvulkan-dev)"
    exit 1
fi
cat > "$SCRATCH/alloc.c" <<'C'
#include <vulkan/vulkan.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* k2-s6-gpu-alloc gtt|vram MB HOLD_SECS [fork]: allocate MB of GPU memory
 * in 256 MB blocks (gtt: host-visible, NOT device-local = GTT, system RAM;
 * vram: device-local = the carve-out), write every byte so it is resident,
 * print "allocated: N MB", hold it HOLD_SECS, exit. "fork": a child also
 * holds the same fds (one DRM client, counted once). */
static void die(const char *m, int r) { fprintf(stderr, "alloc: %s (%d)\n", m, r); exit(2); }

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: alloc gtt|vram MB HOLD_SECS [fork]\n"); return 2; }
    int vram = strcmp(argv[1], "vram") == 0;
    uint64_t total = strtoull(argv[2], NULL, 10) << 20, chunk = 256ull << 20, done = 0;
    unsigned hold = (unsigned)atoi(argv[3]);
    VkApplicationInfo app = { .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .pApplicationName = "k2-s6-proof", .apiVersion = VK_API_VERSION_1_0 };
    VkInstanceCreateInfo ici = { .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app };
    VkInstance inst;
    VkResult r = vkCreateInstance(&ici, NULL, &inst);
    if (r != VK_SUCCESS) die("vkCreateInstance", r);
    uint32_t n = 0;
    vkEnumeratePhysicalDevices(inst, &n, NULL);
    if (n == 0) die("no Vulkan device", 0);
    VkPhysicalDevice *pds = calloc(n, sizeof *pds);
    vkEnumeratePhysicalDevices(inst, &n, pds);
    VkPhysicalDevice pd = VK_NULL_HANDLE;
    for (uint32_t i = 0; i < n; i++) {
        VkPhysicalDeviceProperties p;
        vkGetPhysicalDeviceProperties(pds[i], &p);
        if (p.deviceType != VK_PHYSICAL_DEVICE_TYPE_CPU) { pd = pds[i]; printf("device: %s\n", p.deviceName); break; }
    }
    if (pd == VK_NULL_HANDLE) die("only a CPU (llvmpipe) Vulkan device", 0);
    float prio = 1.0f;
    VkDeviceQueueCreateInfo q = { .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, .queueFamilyIndex = 0, .queueCount = 1, .pQueuePriorities = &prio };
    VkDeviceCreateInfo dci = { .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, .queueCreateInfoCount = 1, .pQueueCreateInfos = &q };
    VkDevice dev;
    r = vkCreateDevice(pd, &dci, NULL, &dev);
    if (r != VK_SUCCESS) die("vkCreateDevice", r);
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(pd, &mp);
    uint32_t hv = VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
    int type = -1;
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        VkMemoryPropertyFlags f = mp.memoryTypes[i].propertyFlags;
        int local = (f & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) != 0;
        if ((f & hv) == hv && local == vram) { type = (int)i; break; }
    }
    if (type < 0) die(vram ? "no host-visible device-local type" : "no host-visible system-memory type", 0);
    printf("type %d heap %u\n", type, mp.memoryTypes[type].heapIndex);
    while (done < total) {
        VkMemoryAllocateInfo ai = { .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = chunk, .memoryTypeIndex = (uint32_t)type };
        VkDeviceMemory m;
        r = vkAllocateMemory(dev, &ai, NULL, &m);
        if (r != VK_SUCCESS) { printf("allocation stopped at %llu MB (%d)\n", (unsigned long long)(done >> 20), (int)r); break; }
        void *p;
        r = vkMapMemory(dev, m, 0, chunk, 0, &p);
        if (r != VK_SUCCESS) die("vkMapMemory", r);
        memset(p, 0x5a, chunk);
        vkUnmapMemory(dev, m);
        done += chunk;
    }
    printf("allocated: %llu MB\n", (unsigned long long)(done >> 20));
    fflush(stdout);
    if (argc > 4 && strcmp(argv[4], "fork") == 0 && fork() == 0) { sleep(hold); _exit(0); }
    sleep(hold);
    return 0;
}
C
if ! cc -O1 -o "$SCRATCH/alloc" "$SCRATCH/alloc.c" -lvulkan >"$SCRATCH/cc.log" 2>&1; then
    res BUILD FAIL "the allocator didn't compile: $(tr '\n' ' ' < "$SCRATCH/cc.log" | cut -c1-300)"
    exit 1
fi
chmod 0755 "$SCRATCH/alloc"
ALLOC="$SCRATCH/alloc"

# The job template's sandbox for a GPU job (the primary device mode).
JOB_GID="$(id -g "$JOB_USER")"
MODE="$(stat -c '%a' "$R")"
SUPP=()
[ $((8#$MODE & 8#006)) -eq $((8#006)) ] || SUPP=(-p "SupplementaryGroups=$(stat -c '%g' "$R")")
PROPS=(
    -p NoNewPrivileges=yes -p ProtectSystem=strict -p ProtectHome=yes -p PrivateTmp=yes
    -p ProtectKernelTunables=yes -p ProtectKernelModules=yes -p ProtectControlGroups=yes
    -p PrivateDevices=yes -p "BindPaths=$R" -p "DeviceAllow=$R rw" "${SUPP[@]}"
    -p RuntimeMaxSec=180 -p RemainAfterExit=yes -p KillMode=control-group
)
N=0
# start_unit <MemoryMax MiB> <args…>: a background unit; prints its name.
start_unit() {
    local mm="$1"; shift
    N=$((N + 1))
    local u="k2-s6-proof-$$-$N.service"
    systemd-run --unit="$u" --quiet --uid="$JOB_USER" --gid="$JOB_GID" "${PROPS[@]}" \
        -p "MemoryMax=${mm}M" -p MemorySwapMax=0 -p "StandardOutput=file:$SCRATCH/$u.out" -p "StandardError=file:$SCRATCH/$u.out" \
        --setenv=PATH=/usr/local/bin:/usr/bin:/bin -- "$ALLOC" "$@" >/dev/null 2>&1
    echo "$u"
}
# wait_allocated <unit>: until it printed "allocated:" (or ended).
wait_allocated() {
    for _ in $(seq 1 120); do
        grep -q '^allocated:' "$SCRATCH/$1.out" 2>/dev/null && return 0
        [ "$(systemctl show -p SubState --value "$1")" = "running" ] || return 1
        sleep 0.5
    done
    return 1
}
usage_json() { k2-node admin gpu usage --unit "$1" --json 2>&1; }
jget() { python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get(sys.argv[2], 0))' "$1" "$2" 2>/dev/null || echo 0; }
cg_file() { local cg; cg="$(systemctl show -p ControlGroup --value "$1")"; cat "/sys/fs/cgroup$cg/$2" 2>/dev/null; }
within() { # within <got> <want> <pct>: |got - want| <= want * pct / 100
    local d=$(( $1 > $2 ? $1 - $2 : $2 - $1 )); [ $(( d * 100 )) -le $(( $2 * $3 )) ]; }

# ── P1 + P2: GTT under MemoryMax, and the executor's reading ──────────
say "== P1/P2: ${ALLOC_MB} MiB of GTT in a unit with MemoryMax=${MEMMAX_MB}M =="
BASE_GTT="$(gtt_used)"
U="$(start_unit "$MEMMAX_MB" gtt "$ALLOC_MB" 40)"
if wait_allocated "$U"; then
    got="$(sed -n 's/^allocated: \([0-9]*\) MB/\1/p' "$SCRATCH/$U.out")"
    state="$(systemctl show -p SubState --value "$U")"
    cur="$(cg_file "$U" memory.current)"; peak="$(cg_file "$U" memory.peak)"
    if [ "${got:-0}" -ge "$ALLOC_MB" ] && [ "$state" = "running" ] && [ "${cur:-0}" -lt $(( MEMMAX_MB * 1048576 )) ]; then
        res P1 ok "allocated ${got} MiB of GTT, unit still running, memory.current $(mib "${cur:-0}") MiB (peak $(mib "${peak:-0}") MiB) under MemoryMax ${MEMMAX_MB}M: GPU buffers are NOT charged to --mem"
    else
        res P1 info "allocated ${got:-?} MiB, state $state, memory.current $(mib "${cur:-0}") MiB: GTT IS (partly) charged to MemoryMax. Tell the K2 builder: --gpu-mem then double counts on this kernel"
    fi
    j="$(usage_json "$U")"
    say "gpu usage: $j"
    gtt="$(jget "$j" gtt_bytes)"; tot="$(jget "$j" bytes)"; clients="$(jget "$j" clients)"
    want=$(( ALLOC_MB * 1048576 ))
    if within "$gtt" "$want" 5; then
        res P2 ok "the executor reads GTT $(mib "$gtt") MiB (all regions $(mib "$tot") MiB, ${clients} client) for ${ALLOC_MB} MiB allocated (±5%)"
    else
        res P2 FAIL "the executor reads GTT $(mib "$gtt") MiB (all $(mib "$tot") MiB) for ${ALLOC_MB} MiB allocated"
    fi
    dg=$(( $(gtt_used) - BASE_GTT ))
    if within "$dg" "$want" 10; then
        res P2b ok "sysfs mem_info_gtt_used rose $(mib "$dg") MiB (the whole device agrees within 10%)"
    else
        res P2b info "sysfs mem_info_gtt_used rose $(mib "$dg") MiB for ${ALLOC_MB} MiB (the host's own GPU use moves it too)"
    fi
    pid="$(cg_file "$U" cgroup.procs | head -1)"
    if [ -n "$pid" ]; then
        say "fdinfo of one DRM fd (fixture material):"
        for fd in /proc/"$pid"/fd/*; do
            case "$(readlink "$fd")" in /dev/dri/*) grep -E '^drm-' "/proc/$pid/fdinfo/$(basename "$fd")" | sed 's/^/    /' | tee -a "$REPORT"; break ;; esac
        done
    fi
    # ── P3: a second client in the same unit counts; a forked fd once ──
    say ""
    say "== P3: a second process in the unit (its own client, and a forked fd) =="
    cg="$(systemctl show -p ControlGroup --value "$U")"
    systemd-run --scope --quiet --uid="$JOB_USER" --gid="$JOB_GID" --unit="k2-s6-proof-$$-scope" \
        "$ALLOC" gtt "$SECOND_MB" 20 fork >"$SCRATCH/second.out" 2>&1 &
    SCOPE_PID=$!
    for _ in $(seq 1 60); do grep -q '^allocated:' "$SCRATCH/second.out" 2>/dev/null && break; sleep 0.5; done
    # Move it (and its forked child, which forks right after that line)
    # into the job unit's cgroup, as a job's own child process would be.
    sleep 1
    for p in $(cat "/sys/fs/cgroup/system.slice/k2-s6-proof-$$-scope.scope/cgroup.procs" 2>/dev/null); do
        echo "$p" > "/sys/fs/cgroup$cg/cgroup.procs" 2>/dev/null || true
    done
    j2="$(usage_json "$U")"
    say "gpu usage: $j2"
    gtt2="$(jget "$j2" gtt_bytes)"; clients2="$(jget "$j2" clients)"; pids2="$(jget "$j2" pids)"
    want2=$(( (ALLOC_MB + SECOND_MB) * 1048576 ))
    if [ "$clients2" = "2" ] && within "$gtt2" "$want2" 5; then
        res P3 ok "2 clients in ${pids2} processes, GTT $(mib "$gtt2") MiB for $((ALLOC_MB + SECOND_MB)) MiB: each client once, the forked fd not twice"
    else
        res P3 FAIL "clients ${clients2}, processes ${pids2}, GTT $(mib "$gtt2") MiB for $((ALLOC_MB + SECOND_MB)) MiB"
    fi
    wait "$SCOPE_PID" 2>/dev/null
    # ── P4: stopped, given back ────────────────────────────────────────
    say ""
    say "== P4: the memory comes back when the unit stops =="
    systemctl stop "$U" >/dev/null 2>&1
    sleep 3
    left=$(( $(gtt_used) - BASE_GTT ))
    if [ "$left" -lt $(( 128 * 1048576 )) ]; then
        res P4 ok "GTT back within $(mib "$left") MiB of where it started"
    else
        res P4 FAIL "GTT still $(mib "$left") MiB over where it started 3 s after the unit stopped"
    fi
else
    res P1 FAIL "the allocation didn't finish: $(tr '\n' ';' < "$SCRATCH/$U.out" | cut -c1-300) (state $(systemctl show -p SubState,Result --value "$U" | tr '\n' ' '))"
fi

# ── P5: VRAM (the carve-out) shows too ────────────────────────────────
say ""
say "== P5: ${VRAM_MB} MiB of VRAM (the carve-out, not system RAM) =="
if [ "$VRAM_MB" -lt 256 ]; then
    res P5 SKIP "the carve-out is too small to test (${CARVE} bytes)"
else
    BASE_V="$(vram_used)"
    V="$(start_unit "$MEMMAX_MB" vram "$VRAM_MB" 20)"
    if wait_allocated "$V"; then
        j="$(usage_json "$V")"
        say "gpu usage: $j"
        v="$(jget "$j" vram_bytes)"
        if within "$v" $(( VRAM_MB * 1048576 )) 5; then
            res P5 ok "the reading has VRAM $(mib "$v") MiB for ${VRAM_MB} MiB (the budget counts it; sysfs vram_used +$(mib $(( $(vram_used) - BASE_V ))) MiB)"
        else
            res P5 FAIL "the reading has VRAM $(mib "$v") MiB for ${VRAM_MB} MiB"
        fi
    else
        res P5 info "no device-local host-visible allocation: $(tr '\n' ';' < "$SCRATCH/$V.out" | cut -c1-200)"
    fi
    systemctl stop "$V" >/dev/null 2>&1
fi

say ""
if [ -e /dev/kfd ]; then
    res C6 info "/dev/kfd exists; v1 jobs never get it, so KFD memory isn't watched (open for a later --compute switch)"
else
    res C6 info "no /dev/kfd on this node"
fi

if [ "$KEEP" = 1 ]; then
    install -m 0755 "$ALLOC" /usr/local/libexec/k2-s6-gpu-alloc
    say ""
    say "== E2E (from a K2 server with a --gpu grant on this node; the node resumes when this script ends) =="
    say "  k2 compute run <this node> --gpu --gpu-mem 1G --mem 1G --no-source -- /usr/local/libexec/k2-s6-gpu-alloc gtt 2048 30"
    say "  expect: failed/gpu_mem_cap, its log ending \"the job's GPU memory reached 2.0G (its --gpu-mem is 1.0G …)\""
    say "  k2 compute run <this node> --gpu --gpu-mem 3G --mem 1G --no-source -- /usr/local/libexec/k2-s6-gpu-alloc gtt 2048 5"
    say "  expect: done, and k2 compute get shows \"gpu memory (the job itself): peak 2.0G of --gpu-mem 3.0G\""
    say "  afterwards: sudo rm /usr/local/libexec/k2-s6-gpu-alloc"
fi
say ""
say "report: $REPORT"
