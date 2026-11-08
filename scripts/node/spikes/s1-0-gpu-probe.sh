#!/usr/bin/env bash
# S1-0 probe — prd-compute-gpu-jobs-v1 (GPU jobs on K2 Compute, GH#79).
#
# Answers what code can't (the vs-live's C1–C5) on a real GPU node, so the
# executor's device mode is a one-line choice
# (`k2-node/src/admin/gpu.rs: DEVICE_MODE`):
#   C1  does this systemd let ONE render node into a job unit?
#       primary:  PrivateDevices=yes + BindPaths=<renderD> + DeviceAllow=<renderD> rw
#       fallback: PrivateDevices=no + DevicePolicy=closed + an allow list
#                 + InaccessiblePaths= for every other DRM node and /dev/kfd
#       Each with a negative control: without DeviceAllow the open must FAIL
#       (the device cgroup is what enforces it; the private /dev only hides
#       names).
#   C2  the render node's mode and group (world rw, or a group the unit needs)
#   C3  eglinfo / vulkaninfo inside the unit see the real GPU, not llvmpipe
#   B21 a background process outlives the main one in the unit's cgroup
#   C4  GPU buffers vs MemoryMax (4 GB of GPU memory under MemoryMax=1G)
#   C5  residue: a new allocation never shows the last job's pattern
#
# Run it as root AT THE GPU NODE (a person, never an agent):
#   sudo bash s1-0-gpu-probe.sh [--user k2job] [--device renderD128] [--report PATH]
# Options:
#   --user NAME      the job user the units run as (default k2job; the
#                    tenant users are k2job-<slot>)
#   --device NAME    the render node to test (default: the first renderD*)
#   --report PATH    the report (default /var/tmp/k2-gpu-s1-0-report.txt)
#   --no-wait-locks  don't wait for the CI job lock or K2's smoke lock
#
# It changes nothing that lasts: transient units (k2-gpu-probe-*, collected
# when they end) and a scratch folder in /run/k2-gpu-probe (removed at the
# end). C3 needs `eglinfo` (mesa-utils) and/or `vulkaninfo` (vulkan-tools);
# C4/C5 need `cc` and the Vulkan headers (Arch: vulkan-headers, Debian:
# libvulkan-dev). A missing tool is reported SKIPPED with the package, never
# a pass. Lid open, mains power: a GPU fault during the probe can end the
# desktop session. Takes about a minute.

set -uo pipefail

JOB_USER="k2job"
DEVICE=""
REPORT="/var/tmp/k2-gpu-s1-0-report.txt"
WAIT_LOCKS=1
while [ $# -gt 0 ]; do
    case "$1" in
        --user) JOB_USER="${2:-}"; shift 2 ;;
        --device) DEVICE="${2:-}"; shift 2 ;;
        --report) REPORT="${2:-}"; shift 2 ;;
        --no-wait-locks) WAIT_LOCKS=0; shift ;;
        -h|--help) sed -n '2,37p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1 (--help)" >&2; exit 2 ;;
    esac
done

[ "$(id -u)" = "0" ] || { echo "run it as root: sudo bash $0" >&2; exit 2; }
command -v systemd-run >/dev/null || { echo "systemd-run not found: GPU jobs need the Linux job wall (systemd)" >&2; exit 2; }
id "$JOB_USER" >/dev/null 2>&1 || { echo "no user $JOB_USER (the job wall's user; sudo k2-node setup makes k2job)" >&2; exit 2; }

: > "$REPORT"
say() { printf '%s\n' "$*" | tee -a "$REPORT"; }
res() { printf '%-6s %-4s %s\n' "$1" "$2" "$3" | tee -a "$REPORT"; }

# The CI runner's job lock and K2's smoke lock (the same convention as the
# other spikes here): wait for the first, refuse beside the second, hold it.
LOCK_CI="/home/sew-ci/sew-build/.job.lock"
LOCK_K2="/var/tmp/k2-smoke.lock"
WROTE_LOCK=0
SCRATCH=/run/k2-gpu-probe
cleanup() {
    systemctl stop 'k2-gpu-probe-*' >/dev/null 2>&1 || true
    rm -rf "$SCRATCH"
    if [ "$WROTE_LOCK" = 1 ]; then rm -f "$LOCK_K2"; fi
}
trap cleanup EXIT
if [ "$WAIT_LOCKS" = 1 ]; then
    while [ -e "$LOCK_CI" ]; do echo "waiting: a CI job holds $LOCK_CI"; sleep 30; done
    if [ -e "$LOCK_K2" ]; then
        echo "another K2 smoke holds $LOCK_K2:" >&2; cat "$LOCK_K2" >&2; exit 3
    fi
    printf 'k2 GPU jobs S1-0 probe\nstarted %s\npid %s\n' "$(date -u +%FT%TZ)" "$$" > "$LOCK_K2"
    WROTE_LOCK=1
fi
mkdir -p "$SCRATCH" && chmod 0755 "$SCRATCH"

JOB_GID="$(id -g "$JOB_USER")"
SYSTEMD_V="$(systemctl --version | head -1 | awk '{print $2}')"

say "K2 GPU jobs — S1-0 probe ($(date -u +%FT%TZ))"
say "systemd $SYSTEMD_V · kernel $(uname -r) · $(. /etc/os-release 2>/dev/null; echo "${PRETTY_NAME:-unknown}")"
say "job user: $JOB_USER (uid $(id -u "$JOB_USER"), gid $JOB_GID, groups: $(id -Gn "$JOB_USER"))"
say ""
say "== GPUs (sysfs) =="
for r in /sys/class/drm/renderD*; do
    [ -e "$r" ] || continue
    d="$(readlink -f "$r/device")"
    say "$(basename "$r"): pci $(basename "$d") vendor $(cat "$d/vendor" 2>/dev/null) device $(cat "$d/device" 2>/dev/null) driver $(basename "$(readlink -f "$d/driver" 2>/dev/null)")" \
        "vram $(cat "$d/mem_info_vram_total" 2>/dev/null || echo -) vis $(cat "$d/mem_info_vis_vram_total" 2>/dev/null || echo -) gtt $(cat "$d/mem_info_gtt_total" 2>/dev/null || echo -)"
done
say "$(ls -l /dev/dri 2>&1)"
say "$(ls -l /dev/kfd 2>&1)"
say ""

if [ -z "$DEVICE" ]; then
    DEVICE="$(ls /dev/dri 2>/dev/null | grep -m1 '^renderD')"
fi
R="/dev/dri/$DEVICE"
[ -c "$R" ] || { say "no render node $R: nothing to test"; exit 1; }

# ── C2: mode and group ─────────────────────────────────────────────
MODE="$(stat -c '%a' "$R")"
GRP="$(stat -c '%G' "$R")"
GID="$(stat -c '%g' "$R")"
SUPP=""
if [ $((8#$MODE & 8#006)) -eq $((8#006)) ]; then
    res C2 ok "$R is $MODE $GRP: world read-write, no group needed"
elif [ $((8#$MODE & 8#060)) -eq $((8#060)) ]; then
    res C2 ok "$R is $MODE $GRP: the unit needs SupplementaryGroups=$GID ($GRP)"
    SUPP="SupplementaryGroups=$GID"
else
    res C2 FAIL "$R is $MODE $GRP: no group read-write; GPU jobs can't open it"
fi

# The job template's sandbox (the parts that touch devices and /sys).
BASE=(
    NoNewPrivileges=yes ProtectSystem=strict ProtectHome=yes PrivateTmp=yes
    ProtectKernelTunables=yes ProtectKernelModules=yes ProtectKernelLogs=yes
    ProtectControlGroups=yes ProtectClock=yes RestrictSUIDSGID=yes RestrictRealtime=yes
    LockPersonality=yes TemporaryFileSystem=/dev/shm:mode=1777 RuntimeMaxSec=120
)
[ "$SYSTEMD_V" -ge 247 ] && BASE+=(ProtectProc=invisible)
[ "$SYSTEMD_V" -ge 248 ] && BASE+=(PrivateIPC=yes)

OTHERS=()
for n in /dev/dri/*; do
    [ "$n" = "$R" ] && continue
    OTHERS+=("-$n")
done
OTHERS+=("-/dev/kfd")

primary_props() {
    printf '%s\n' "${BASE[@]}" PrivateDevices=yes "BindPaths=$R"
    [ "${1:-allow}" = allow ] && echo "DeviceAllow=$R rw"
    [ -n "$SUPP" ] && echo "$SUPP"
}
fallback_props() {
    printf '%s\n' "${BASE[@]}" PrivateDevices=no DevicePolicy=closed
    for a in "/dev/null rw" "/dev/zero rw" "/dev/full rw" "/dev/random rw" "/dev/urandom rw" "/dev/tty rw" "/dev/ptmx rw" "char-pts rw"; do
        echo "DeviceAllow=$a"
    done
    [ "${1:-allow}" = allow ] && echo "DeviceAllow=$R rw"
    echo "SystemCallFilter=~@raw-io"
    echo "InaccessiblePaths=${OTHERS[*]}"
    [ -n "$SUPP" ] && echo "$SUPP"
}

N=0
# run_unit <props-fn> <props-arg> <extra props…> -- <script>: stdout of the unit.
run_unit() {
    local fn="$1" arg="$2"; shift 2
    local -a extra=()
    while [ $# -gt 0 ] && [ "$1" != "--" ]; do extra+=("$1"); shift; done
    shift
    N=$((N + 1))
    local -a a=(--unit="k2-gpu-probe-$$-$N" --quiet --wait --collect --pipe --uid="$JOB_USER" --gid="$JOB_GID" --setenv=R="$R" --setenv=PATH=/usr/local/bin:/usr/bin:/bin)
    while IFS= read -r p; do a+=(-p "$p"); done < <("$fn" "$arg"; printf '%s\n' "${extra[@]+"${extra[@]}"}" | sed '/^$/d')
    systemd-run "${a[@]}" -- /bin/sh -c "$1" </dev/null 2>&1
}

SELFTEST='test -c "$R" && head -c0 <"$R" || { echo "cannot open $R"; exit 1; }
for d in /dev/dri/* /dev/kfd; do
  [ "$d" = "$R" ] && continue; [ -c "$d" ] || continue
  if head -c0 <"$d" 2>/dev/null; then echo "opened $d"; exit 1; fi
done
echo "dri: $(ls /dev/dri | tr "\n" " ")"; echo k2-gpu-ok'
DENIED='if head -c0 <"$R" 2>/dev/null; then echo "OPENED without DeviceAllow"; else echo k2-gpu-denied; fi'

say ""
say "== C1: one render node into a unit =="
PRIMARY=0
FALLBACK=0
out="$(run_unit primary_props allow -- "$SELFTEST")"
if printf '%s' "$out" | grep -q '^k2-gpu-ok$'; then
    neg="$(run_unit primary_props deny -- "$DENIED")"
    if printf '%s' "$neg" | grep -q '^k2-gpu-denied$'; then
        PRIMARY=1
        res C1 ok "primary (BindPaths + DeviceAllow under PrivateDevices): $(printf '%s' "$out" | grep '^dri:') · without DeviceAllow the open fails"
    else
        res C1 FAIL "primary opens the node, but it also opens WITHOUT DeviceAllow (no device cgroup?): $neg"
    fi
else
    res C1 FAIL "primary: $(printf '%s' "$out" | tr '\n' ' ')"
fi
out="$(run_unit fallback_props allow -- "$SELFTEST")"
if printf '%s' "$out" | grep -q '^k2-gpu-ok$'; then
    neg="$(run_unit fallback_props deny -- "$DENIED")"
    if printf '%s' "$neg" | grep -q '^k2-gpu-denied$'; then
        FALLBACK=1
        res C1 ok "fallback (DevicePolicy=closed + allow list + InaccessiblePaths): opens only $R · without DeviceAllow the open fails"
    else
        res C1 FAIL "fallback opens the node, but also WITHOUT DeviceAllow: $neg"
    fi
else
    res C1 FAIL "fallback: $(printf '%s' "$out" | tr '\n' ' ')"
fi

MODE_FN=""
if [ "$PRIMARY" = 1 ]; then MODE_FN=primary_props; elif [ "$FALLBACK" = 1 ]; then MODE_FN=fallback_props; fi

say ""
say "== C3: the real GPU inside the unit (not llvmpipe) =="
if [ -z "$MODE_FN" ]; then
    res C3 SKIP "neither mode gives the unit the device"
else
    if command -v eglinfo >/dev/null; then
        out="$(run_unit "$MODE_FN" allow -- 'eglinfo -B -p surfaceless 2>&1 | grep -E "OpenGL (ES )?(renderer|version)|EGL driver name"')"
        if printf '%s' "$out" | grep -q 'renderer' && ! printf '%s' "$out" | grep -qi llvmpipe; then
            res C3 ok "eglinfo surfaceless: $(printf '%s' "$out" | tr '\n' ';')"
        else
            res C3 FAIL "eglinfo surfaceless: $(printf '%s' "$out" | tr '\n' ';')"
        fi
    else
        res C3 SKIP "eglinfo not installed (Arch: mesa-utils; Debian: mesa-utils)"
    fi
    if command -v vulkaninfo >/dev/null; then
        out="$(run_unit "$MODE_FN" allow -- 'vulkaninfo --summary 2>&1 | grep -E "deviceName|driverName|deviceType"')"
        if printf '%s' "$out" | grep -q 'deviceName' && ! printf '%s' "$out" | grep -qi 'llvmpipe'; then
            res C3 ok "vulkaninfo: $(printf '%s' "$out" | tr -s ' ' | tr '\n' ';')"
        else
            res C3 FAIL "vulkaninfo: $(printf '%s' "$out" | tr -s ' ' | tr '\n' ';')"
        fi
    else
        res C3 SKIP "vulkaninfo not installed (vulkan-tools)"
    fi
fi

say ""
say "== B21: a background process holds the unit's cgroup after the main one ends =="
N=$((N + 1))
U="k2-gpu-probe-$$-$N"
systemd-run --unit="$U" --quiet --uid="$JOB_USER" --gid="$JOB_GID" -p RemainAfterExit=yes -p KillMode=control-group -p RuntimeMaxSec=60 \
    -- /bin/sh -c 'sleep 20 & echo started' >/dev/null 2>&1
sleep 2
st="$(systemctl show -p ActiveState,SubState,ControlGroup "$U" 2>/dev/null | tr '\n' ' ')"
cg="$(systemctl show -p ControlGroup --value "$U" 2>/dev/null)"
procs="$( [ -n "$cg" ] && cat "/sys/fs/cgroup$cg/cgroup.procs" 2>/dev/null | wc -l | tr -d ' ')"
systemctl stop "$U" >/dev/null 2>&1
systemctl reset-failed "$U" >/dev/null 2>&1
after="$(systemctl show -p LoadState --value "$U" 2>/dev/null)"
res B21 info "after the main process ended: $st· processes left in its cgroup: ${procs:-0} · after stop: ${after:-gone} (the GPU lease frees only once that count is 0 or the unit is gone)"

say ""
say "== C4 / C5: GPU memory (needs cc + Vulkan headers) =="
ALLOC_OK=0
if [ -z "$MODE_FN" ]; then
    res C4 SKIP "neither mode gives the unit the device"
elif ! command -v cc >/dev/null || ! [ -e /usr/include/vulkan/vulkan.h ]; then
    res C4 SKIP "needs cc and the Vulkan headers (Arch: base-devel vulkan-headers; Debian: build-essential libvulkan-dev)"
    res C5 SKIP "same as C4"
else
    cat > "$SCRATCH/gpu-alloc.c" <<'C'
#include <vulkan/vulkan.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* gpu-alloc alloc|fill|scan MB: allocate MB of host-visible GPU memory in
 * 256 MB blocks and write it (alloc: 0x01, fill: 0xA5) or count the
 * nonzero 8-byte words it starts with (scan). Holds it all until exit. */
static void die(const char *m, int r) { fprintf(stderr, "gpu-alloc: %s (%d)\n", m, r); exit(2); }

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: gpu-alloc alloc|fill|scan MB\n"); return 2; }
    const char *mode = argv[1];
    uint64_t total = strtoull(argv[2], NULL, 10) << 20, chunk = 256ull << 20, done = 0, nonzero = 0;
    VkApplicationInfo app = { .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .pApplicationName = "k2-gpu-probe", .apiVersion = VK_API_VERSION_1_0 };
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
    uint32_t want = VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
    int type = -1;
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if ((mp.memoryTypes[i].propertyFlags & want) == want) { type = (int)i; break; }
    }
    if (type < 0) die("no host-visible memory type", 0);
    while (done < total) {
        VkMemoryAllocateInfo ai = { .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = chunk, .memoryTypeIndex = (uint32_t)type };
        VkDeviceMemory m;
        r = vkAllocateMemory(dev, &ai, NULL, &m);
        if (r != VK_SUCCESS) { printf("allocation stopped at %llu MB (%d)\n", (unsigned long long)(done >> 20), (int)r); break; }
        void *p;
        r = vkMapMemory(dev, m, 0, chunk, 0, &p);
        if (r != VK_SUCCESS) die("vkMapMemory", r);
        if (strcmp(mode, "scan") == 0) {
            const uint64_t *w = p;
            for (uint64_t i = 0; i < chunk / 8; i++) nonzero += w[i] != 0;
        } else {
            memset(p, strcmp(mode, "fill") == 0 ? 0xA5 : 0x01, chunk);
        }
        vkUnmapMemory(dev, m);
        done += chunk;
    }
    printf("%s: %llu MB\n", mode, (unsigned long long)(done >> 20));
    if (strcmp(mode, "scan") == 0) printf("nonzero words: %llu\n", (unsigned long long)nonzero);
    fflush(stdout);
    return 0;
}
C
    if cc -O1 -o "$SCRATCH/gpu-alloc" "$SCRATCH/gpu-alloc.c" -lvulkan >"$SCRATCH/cc.log" 2>&1; then
        chmod 0755 "$SCRATCH/gpu-alloc"
        ALLOC_OK=1
    else
        res C4 FAIL "gpu-alloc didn't compile: $(tr '\n' ' ' < "$SCRATCH/cc.log" | cut -c1-300)"
        res C5 SKIP "same as C4"
    fi
fi
if [ "$ALLOC_OK" = 1 ]; then
    out="$(run_unit "$MODE_FN" allow MemoryMax=1G MemorySwapMax=0 -- "$SCRATCH/gpu-alloc alloc 4096; echo exit=\$?")"
    if printf '%s' "$out" | grep -q 'alloc: 4096 MB'; then
        res C4 info "4 GB of GPU memory under MemoryMax=1G: allowed, NOT charged to MemoryMax ($(printf '%s' "$out" | tr '\n' ';')). GPU buffers bypass --mem (S6's watchdog)."
    else
        res C4 info "4 GB of GPU memory under MemoryMax=1G: stopped ($(printf '%s' "$out" | tr '\n' ';')). Charged to the unit, or the driver refused."
    fi
    fill="$(run_unit "$MODE_FN" allow -- "$SCRATCH/gpu-alloc fill 1024")"
    scan="$(run_unit "$MODE_FN" allow -- "$SCRATCH/gpu-alloc scan 1024")"
    nz="$(printf '%s' "$scan" | sed -n 's/^nonzero words: //p')"
    if [ "${nz:-x}" = "0" ]; then
        res C5 ok "a new 1 GB allocation after a 1 GB 0xA5 fill reads all zero (no residue between jobs)"
    else
        res C5 FAIL "residue: $(printf '%s' "$scan" | tr '\n' ';') after $(printf '%s' "$fill" | tr '\n' ';') (the executor must scrub between jobs; amend the PRD)"
    fi
fi

say ""
say "== the choice =="
if [ "$PRIMARY" = 1 ]; then
    say "DEVICE_MODE = DeviceMode::BindPaths   (primary works; keep the line as it is)"
elif [ "$FALLBACK" = 1 ]; then
    say "DEVICE_MODE = DeviceMode::DevicePolicy   (primary failed, fallback works: change the line in crates/k2-node/src/admin/gpu.rs)"
else
    say "neither mode gives a unit exactly one render node: GPU jobs can't ship on this node yet"
fi
say "report: $REPORT"
