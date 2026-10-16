//! The App runtime's box prep (`prd-k2-app-runtime-v1` §10, protocol 6,
//! vs-live 2 AR38/AR45): fixed names and paths, the token parsers for the
//! `apps` namespace, and the canned bytes the helper renders.
//!
//! One Unix user, `k2app`, owns every App on a box. Apps get their own id
//! ranges from `k2app`'s subordinate ids through rootless user namespaces
//! (`--userns=auto`), not a Unix user each (converged plan: "rootless user
//! namespaces give each App its own id range without per-App root verbs").
//! The `process` exception is the one App kind systemd runs directly: a
//! system unit with `User=k2app` (AR45; user units fail open on Ubuntu 24.04
//! and on systemd < 257).
//!
//! Nothing here comes from argv except validated numbers, port ranges, an
//! App id (`^[a-z][a-z0-9-]{0,31}$`) and the OCI runtime enum.

/// The App user.
pub const APP_USER: &str = "k2app";
/// The `process` exception's user: a second system user, never `k2app`.
/// `k2app` owns every guarded container's user namespace (and runs the
/// engine), so a process running as `k2app` could ptrace or read the
/// memory of every guarded App (the owner of a user namespace holds all
/// capabilities in it). Process Apps share this user among themselves;
/// they never share it with the engine or the containers.
pub const PROCESS_USER: &str = "k2appx";
/// Its state root (`StateDirectory=k2appx/<id>`, made by systemd). Never
/// created by useradd: nothing else lives there.
pub const PROCESS_HOME: &str = "/var/lib/k2appx";
/// Its home (0750, owned by `k2app`). App state lives under `apps/<id>`.
pub const APP_HOME: &str = "/var/lib/k2app";
pub const APP_SHELL: &str = "/usr/sbin/nologin";
/// Root-owned config the engine reads (`containers.conf` with the OCI
/// runtime). Never inside `k2app`'s home: the helper never writes there.
pub const APP_ETC: &str = "/etc/k2app";
pub const CONTAINERS_CONF_FILE: &str = "containers.conf";
pub const CONTAINERS_CONF: &str = "/etc/k2app/containers.conf";

/// Subordinate ids for `k2app`: 64 × 65536 = room for 64 Apps with their
/// own 65536-id user namespace (stage W measured 24 light Apps on a Basic
/// box). The first block starts at or above [`SUBID_FLOOR`], 65536-aligned,
/// above every existing range, so it never meets the 100000+ blocks
/// `useradd` hands to people.
pub const SUBID_COUNT: u64 = 64 * 65_536;
pub const SUBID_ALIGN: u64 = 65_536;
pub const SUBID_FLOOR: u64 = 1_048_576;
/// Highest id + 1 a range may reach (uid_t is 32-bit; 2^32-1 is "none").
pub const SUBID_LIMIT: u64 = 4_294_967_295;
pub const SUBUID_FILE: &str = "/etc/subuid";
pub const SUBGID_FILE: &str = "/etc/subgid";
pub const PASSWD_FILE: &str = "/etc/passwd";
pub const GROUP_FILE: &str = "/etc/group";
pub const SHADOW_FILE: &str = "/etc/shadow";
/// Groups that would give `k2app` more than its own files.
pub const PRIVILEGED_GROUPS: &[&str] = &[
    "sudo",
    "wheel",
    "admin",
    "adm",
    "root",
    "docker",
    "lxd",
    "kvm",
    "disk",
    "shadow",
    "systemd-journal",
];

/// systemd's linger flag file for `k2app`.
pub const LINGER_DIR: &str = "/var/lib/systemd/linger";

/// `/etc/systemd/system/user@<uid>.service.d/<these>` (the uid is looked up,
/// never taken from argv).
pub const DELEGATE_DROPIN_FILE: &str = "k2-delegate.conf";
pub const BUDGET_DROPIN_FILE: &str = "k2-budget.conf";

/// Slice names without a dash: in systemd a dash means nesting
/// (`k2-cores.slice` lives at `/k2.slice/k2-cores.slice`, and `MemoryMin`
/// on it is capped by `k2.slice`'s 0 — stage W §2.12, AR45).
pub const CORES_SLICE: &str = "k2cores.slice";
/// The process-exception units (system side) and, by the same name, the
/// slice `k2-app-engine` makes inside `user@<k2app>` for containers.
pub const APPS_SLICE: &str = "k2apps.slice";
/// The cores' drop-in file name on each core unit (never the pg helper's
/// `k2-ram.conf`, AR17). Never sets `KillMode` (AR48: stage H owns it).
pub const CORES_DROPIN_FILE: &str = "k2-cores.conf";

/// The `process` exception: one template system unit, an instance per App.
pub const PROCESS_UNIT_FILE: &str = "k2app-process@.service";
pub const PROCESS_UNIT_PREFIX: &str = "k2app-process@";
/// The engine (runtime slice 3, `apps install-engine`). The process unit
/// starts it; until it is installed a start fails with "engine missing".
pub const ENGINE_PATH: &str = "/usr/local/libexec/k2-app-engine";

/// `net.ipv4.ip_local_reserved_ports` (also covers IPv6): the App gateway
/// range sits inside every distro's ephemeral range (W §2.12).
pub const RESERVED_PORTS: (u16, u16) = (38_600, 38_999);
pub const SYSCTL_FILE: &str = "/etc/sysctl.d/90-k2-apps.conf";
pub const RESERVED_PORTS_PROC: &str = "/proc/sys/net/ipv4/ip_local_reserved_ports";
/// The port ranges the daemon passes to `apps nft apply` by default:
/// per-App gateway links, per-App egress proxy listeners, the box's
/// Postgres. An App's gateway and proxy ports share its slot offset
/// ([`proxy_port_for_gateway`]); each is admitted only to that App's
/// cgroup (per-App rules), never as a range.
pub const GATEWAY_PORTS: PortRange = PortRange {
    lo: 38_600,
    hi: 38_899,
};
pub const PROXY_PORTS: PortRange = PortRange {
    lo: 38_900,
    hi: 38_999,
};

/// The App's egress proxy port (S15) for its gateway port: the same
/// offset in [`PROXY_PORTS`]. `None` outside the gateway range or past
/// the proxy range.
pub fn proxy_port_for_gateway(gw: u16) -> Option<u16> {
    if !(GATEWAY_PORTS.lo..=GATEWAY_PORTS.hi).contains(&gw) {
        return None;
    }
    let p = PROXY_PORTS.lo as u32 + (gw - GATEWAY_PORTS.lo) as u32;
    (p <= PROXY_PORTS.hi as u32).then_some(p as u16)
}
pub const DB_PORTS: PortRange = PortRange {
    lo: 5_432,
    hi: 5_432,
};

/// The nftables table. Coexists with the old sandbox's `k2sandbox_<uid>`
/// tables (AR24).
pub const NFT_TABLE: &str = "k2app";
/// The chain name doubles as the ruleset version marker (v2: gateway
/// ports only through per-App rules, runtime slice 3; v3: the egress
/// proxy ports too, S15, so no App reaches another App's proxy).
pub const NFT_CHAIN: &str = "k2app_v3";
/// The per-App chain (`apps nft app`): one accept per App for its own
/// gateway port, matched on the App's cgroup.
pub const NFT_APPS_CHAIN: &str = "k2app_apps";
/// `apps nft app` state: which App owns which gateway port.
pub const NFT_APPS_STATE: &str = "/var/lib/k2-root-helper/apps-nft.json";

/// io.cost lives only at the cgroup v2 root.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";
pub const IO_COST_QOS: &str = "/sys/fs/cgroup/io.cost.qos";

/// Programs (absolute; env cleared by the helper).
pub const LOGINCTL: &str = "/usr/bin/loginctl";
pub const USERMOD: &str = "/usr/sbin/usermod";
pub const NFT: &str = "/usr/sbin/nft";
pub const ENV: &str = "/usr/bin/env";
pub const SYSTEMD_TMPFILES: &str = "/usr/bin/systemd-tmpfiles";
pub const APT_GET: &str = "/usr/bin/apt-get";
pub const DNF: &str = "/usr/bin/dnf";
pub const PACMAN: &str = "/usr/bin/pacman";
pub const PODMAN: &str = "/usr/bin/podman";
pub const CRUN: &str = "/usr/bin/crun";
pub const YOUKI: &str = "/usr/bin/youki";
pub const PASTA: &str = "/usr/bin/pasta";
/// conmon: Debian/Ubuntu ship it in /usr/bin, Fedora/Arch in /usr/libexec/podman.
pub const CONMON: &[&str] = &[
    "/usr/bin/conmon",
    "/usr/libexec/podman/conmon",
    "/usr/lib/podman/conmon",
];

/// Podman ≥ 5: Quadlet `Notify=healthy` and pasta by default (stage W §2.2).
pub const PODMAN_MIN_MAJOR: u32 = 5;

/// The distro packages box prep installs (Rosson 2026-10-14: Podman + crun
/// from the distro by default; youki is opt-in).
/// skopeo: the daemon (not k2app, which has no network) fetches the pinned
/// base image by digest into an OCI archive (slice 3).
pub const APT_PACKAGES: &[&str] = &["podman", "crun", "passt", "uidmap", "dbus-user-session", "skopeo", "catatonit"];
pub const DNF_PACKAGES: &[&str] = &["podman", "crun", "passt", "shadow-utils", "skopeo", "catatonit"];
pub const PACMAN_PACKAGES: &[&str] = &["podman", "crun", "passt", "skopeo", "catatonit"];
pub const YOUKI_PACKAGE: &str = "youki";

/// Bounds for the numeric tokens.
pub const CORES_MIN_MIB: (u64, u64) = (128, 65_536);
pub const BUDGET_MEM_MIB: (u64, u64) = (256, 4_194_304);
pub const BUDGET_TASKS: (u64, u64) = (64, 65_536);

/// The OCI runtime Podman uses for Apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OciRuntime {
    /// The default: the distro's crun.
    Crun,
    /// Opt-in (all-Rust path), tested in CI from day one.
    Youki,
}

impl OciRuntime {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "crun" => Some(Self::Crun),
            "youki" => Some(Self::Youki),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crun => "crun",
            Self::Youki => "youki",
        }
    }
    pub fn binary(self) -> &'static str {
        match self {
            Self::Crun => CRUN,
            Self::Youki => YOUKI,
        }
    }
}

/// `lo-hi` (or a single port `N`), 1024–65535, at most 1024 ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    pub lo: u16,
    pub hi: u16,
}

fn small_num(s: &str, max_len: usize) -> Option<u64> {
    if s.is_empty() || s.len() > max_len || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

impl PortRange {
    pub fn parse(s: &str) -> Option<Self> {
        let (a, b) = match s.split_once('-') {
            Some((a, b)) => (a, b),
            None => (s, s),
        };
        let lo = small_num(a, 5)?;
        let hi = small_num(b, 5)?;
        if lo < 1024 || hi > 65_535 || lo > hi || hi - lo >= 1024 {
            return None;
        }
        Some(Self {
            lo: lo as u16,
            hi: hi as u16,
        })
    }
    /// Canonical token (`N` for one port, `lo-hi` otherwise).
    pub fn token(&self) -> String {
        if self.lo == self.hi {
            self.lo.to_string()
        } else {
            format!("{}-{}", self.lo, self.hi)
        }
    }
    fn nft(&self) -> String {
        self.token()
    }
}

/// A bounded decimal token (no sign, no leading zero, no unit).
pub fn bounded(s: &str, (min, max): (u64, u64)) -> Option<u64> {
    let n = small_num(s, 10)?;
    (min..=max).contains(&n).then_some(n)
}

/// An App id: `^[a-z][a-z0-9-]{0,31}$`, no trailing or doubled dash (it
/// becomes a systemd instance name and a directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppId(String);

impl AppId {
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let ok = !b.is_empty()
            && b.len() <= 32
            && b[0].is_ascii_lowercase()
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
            && !s.ends_with('-')
            && !s.contains("--");
        ok.then(|| Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// `k2app-process@<id>.service`.
    pub fn process_unit(&self) -> String {
        format!("{PROCESS_UNIT_PREFIX}{}.service", self.0)
    }
    /// The App's own Postgres database and role (S21): `app_<id>` with
    /// dashes as underscores. Injective: an App id has no underscore.
    pub fn pg_name(&self) -> String {
        format!("app_{}", self.0.replace('-', "_"))
    }
    /// The id with dashes as underscores, for systemd unit and slice names
    /// (a dash there means nesting: `k2apps-nsi-skin.slice` would sit
    /// inside App `nsi`'s slice).
    pub fn unit_safe(&self) -> String {
        self.0.replace('-', "_")
    }
}

// ── pg_hba.conf (`pg hba-app`, protocol 6) ─────────────────────────────

/// The K2-owned block in each cluster's `pg_hba.conf`. It sits above every
/// other rule (first match wins) and holds only canned App lines.
pub const HBA_BEGIN: &str = "# BEGIN K2 Apps (k2-root-helper pg hba-app; do not edit)";
pub const HBA_END: &str = "# END K2 Apps";
/// Largest `pg_hba.conf` the helper edits.
pub const HBA_MAX_BYTES: usize = 1024 * 1024;

/// The one line an App gets: its own database, its own role, loopback,
/// password (SCRAM) only.
pub fn hba_line(id: &AppId) -> String {
    let n = id.pg_name();
    format!("host {n} {n} 127.0.0.1/32 scram-sha-256")
}

/// Add or remove an App's line. `Ok(None)` = already so (nothing to
/// write). The block is created above the first rule on the first add and
/// dropped when its last line goes. A damaged block (two BEGINs, no END,
/// END first, a foreign line inside) is a refusal, never a repair.
pub fn hba_edit(text: &str, add: bool, id: &AppId) -> Result<Option<String>, &'static str> {
    let line = hba_line(id);
    let lines: Vec<&str> = text.lines().collect();
    let begins: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.trim_end() == HBA_BEGIN).map(|(i, _)| i).collect();
    let ends: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.trim_end() == HBA_END).map(|(i, _)| i).collect();
    let block = match (begins.as_slice(), ends.as_slice()) {
        ([], []) => None,
        ([b], [e]) if b < e => Some((*b, *e)),
        _ => return Err("pg_hba.conf: the K2 Apps block is damaged (fix or remove it by hand)"),
    };
    let mut inside: Vec<String> = match block {
        None => Vec::new(),
        Some((b, e)) => lines[b + 1..e].iter().map(|l| l.trim_end().to_string()).collect(),
    };
    // Only canned App lines live in the block.
    for l in &inside {
        let f: Vec<&str> = l.split_whitespace().collect();
        let ok = f.len() == 5
            && f[0] == "host"
            && f[1] == f[2]
            && f[1].strip_prefix("app_").is_some_and(|n| {
                !n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            && f[3] == "127.0.0.1/32"
            && f[4] == "scram-sha-256"
            && *l == f.join(" ");
        if !ok {
            return Err("pg_hba.conf: the K2 Apps block holds a line K2 didn't write");
        }
    }
    let has = inside.contains(&line);
    if has == add {
        return Ok(None);
    }
    if add {
        inside.push(line);
        inside.sort();
    } else {
        inside.retain(|l| *l != line);
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 3);
    let render_block = |out: &mut Vec<String>| {
        if !inside.is_empty() {
            out.push(HBA_BEGIN.to_string());
            out.extend(inside.iter().cloned());
            out.push(HBA_END.to_string());
        }
    };
    match block {
        Some((b, e)) => {
            out.extend(lines[..b].iter().map(|l| l.to_string()));
            render_block(&mut out);
            out.extend(lines[e + 1..].iter().map(|l| l.to_string()));
        }
        None => {
            // Above the first rule: after the leading comments and blanks.
            let first_rule = lines
                .iter()
                .position(|l| {
                    let t = l.trim();
                    !t.is_empty() && !t.starts_with('#')
                })
                .unwrap_or(lines.len());
            out.extend(lines[..first_rule].iter().map(|l| l.to_string()));
            render_block(&mut out);
            out.extend(lines[first_rule..].iter().map(|l| l.to_string()));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    Ok(Some(s))
}

// ── Canned bytes ────────────────────────────────────────────────────────

/// `user@<k2app uid>.service.d/k2-delegate.conf`: every controller the
/// engine needs (default delegation is only memory+pids on 22.04).
///
/// Also the Apps' log rate limit (W §5.3: a log flood pushed Postgres p99
/// x3-13). journald honours `LogRateLimit*` only on SYSTEM units (it reads
/// them from /run/systemd/units, which only PID 1 writes) and keys user
/// units' lines on the system unit `user@<uid>.service`, so the limit
/// lives here and covers every App together (Linux e2e: on the Quadlet
/// unit it was ignored; here a 20000-line burst was suppressed).
pub fn delegate_dropin() -> &'static str {
    "# K2 App runtime box prep (k2-root-helper `apps delegate on`).\n\
     [Service]\n\
     Delegate=cpu cpuset io memory pids\n\
     LogRateLimitIntervalSec=30s\n\
     LogRateLimitBurst=5000\n"
}

/// `user@<k2app uid>.service.d/k2-budget.conf`: the aggregate cap for every
/// container App.
pub fn budget_dropin(mem_max_mib: u64, tasks_max: u64) -> String {
    format!(
        "# K2 App runtime box prep (k2-root-helper `apps budget`).\n\
         [Service]\n\
         MemoryAccounting=yes\n\
         MemoryMax={mem_max_mib}M\n\
         MemorySwapMax=0\n\
         TasksMax={tasks_max}\n\
         ManagedOOMMemoryPressure=kill\n"
    )
}

/// `/etc/systemd/system/k2cores.slice`: the cores' protected floor.
pub fn cores_slice_unit(mem_min_mib: u64) -> String {
    format!(
        "# K2 cores (daemon, Postgres, Stalwart): protected memory floor.\n\
         # Written by k2-root-helper `apps cores-slice`.\n\
         [Unit]\n\
         Description=K2 cores\n\
         Before=slices.target\n\
         \n\
         [Slice]\n\
         MemoryAccounting=yes\n\
         MemoryMin={mem_min_mib}M\n"
    )
}

/// Which core a cores drop-in is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Core {
    Daemon,
    Postgres,
    Stalwart,
}

/// `<unit>.d/k2-cores.conf`. Takes effect at the unit's next restart.
/// Never `KillMode` (AR48) and never `MemoryMax` (the RAM fence owns it).
pub fn cores_dropin(core: Core) -> &'static str {
    match core {
        // AR18: Delegate=yes lets the daemon move agents into its own
        // `agents/` leaf later (S17); harmless until it does.
        Core::Daemon => {
            "# K2 cores slice (k2-root-helper `apps cores-slice`).\n\
             [Service]\n\
             Slice=k2cores.slice\n\
             OOMScoreAdjust=-900\n\
             CPUWeight=400\n\
             ManagedOOMPreference=omit\n\
             Delegate=yes\n"
        }
        Core::Postgres => {
            "# K2 cores slice (k2-root-helper `apps cores-slice`).\n\
             [Service]\n\
             Slice=k2cores.slice\n\
             OOMScoreAdjust=-800\n"
        }
        Core::Stalwart => {
            "# K2 cores slice (k2-root-helper `apps cores-slice`).\n\
             [Service]\n\
             Slice=k2cores.slice\n\
             OOMScoreAdjust=-700\n\
             CPUWeight=200\n\
             IOWeight=200\n"
        }
    }
}

/// The process exception (§4.3, AR45): a **system** unit with
/// `User=k2appx` (see [`PROCESS_USER`]: never `k2app`), where systemd's
/// sandboxing is real on every supported systemd (249+). One instance per App, started and stopped only by the
/// daemon (`apps process start|stop <id>`), always with its caps drop-in
/// (`apps process caps`). The engine stages the App's code and spec under
/// `/var/lib/k2app/apps/<id>/process/` (read-only inside the unit); the
/// unit carries no command of its own.
///
/// The App's code and spec are read-only; its writable state is its own
/// `/var/lib/k2appx/<id>` (`data/`, `cache/`, `home/`). A read-only tmpfs
/// over `k2app`'s home hides the engine's state, the Seeds, Podman's
/// storage and every other App; no home directories, no engine socket,
/// loopback only
/// (`IPAddressAllow=localhost`; the `inet k2app` table narrows that to
/// its gateway port, the proxy and Postgres), no devices unless granted.
pub fn process_unit() -> &'static str {
    "# K2 App runtime: the `process` exception (prd-k2-app-runtime-v1 §4.3).\n\
     # Written by k2-root-helper `apps process-unit on`. One instance per App\n\
     # (k2app-process@<app-id>.service), started only by the K2 daemon.\n\
     [Unit]\n\
     Description=K2 App %i (process)\n\
     After=network.target\n\
     \n\
     [Service]\n\
     Type=simple\n\
     User=k2appx\n\
     Group=k2appx\n\
     Slice=k2apps.slice\n\
     ExecStart=/usr/local/libexec/k2-app-engine process-run %i\n\
     StateDirectory=k2appx/%i\n\
     StateDirectoryMode=0700\n\
     WorkingDirectory=/var/lib/k2app/apps/%i/process/code\n\
     TemporaryFileSystem=/var/lib/k2app:ro\n\
     BindReadOnlyPaths=/var/lib/k2app/apps/%i/process\n\
     InaccessiblePaths=-/run/k2app\n\
     IPAddressAllow=localhost\n\
     IPAddressDeny=any\n\
     Restart=no\n\
     KillMode=control-group\n\
     TimeoutStopSec=60\n\
     NoNewPrivileges=yes\n\
     PrivateTmp=yes\n\
     PrivateDevices=yes\n\
     ProtectSystem=strict\n\
     ProtectHome=yes\n\
     ProtectKernelTunables=yes\n\
     ProtectKernelModules=yes\n\
     ProtectKernelLogs=yes\n\
     ProtectControlGroups=yes\n\
     ProtectClock=yes\n\
     ProtectHostname=yes\n\
     ProtectProc=invisible\n\
     ProcSubset=pid\n\
     RestrictNamespaces=yes\n\
     RestrictSUIDSGID=yes\n\
     RestrictRealtime=yes\n\
     LockPersonality=yes\n\
     SystemCallArchitectures=native\n\
     SystemCallFilter=@system-service\n\
     SystemCallFilter=~@privileged\n\
     CapabilityBoundingSet=\n\
     AmbientCapabilities=\n\
     RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\n\
     UMask=0077\n\
     MemoryAccounting=yes\n\
     MemorySwapMax=0\n\
     TasksMax=512\n\
     OOMPolicy=kill\n\
     LimitNOFILE=4096\n\
     LogRateLimitIntervalSec=30s\n\
     LogRateLimitBurst=1000\n"
}

/// `/etc/sysctl.d/90-k2-apps.conf`.
pub fn sysctl_conf() -> String {
    format!(
        "# K2 App runtime: keep the App gateway ports out of the ephemeral range.\n\
         # Written by k2-root-helper `apps reserve-ports on`.\n\
         net.ipv4.ip_local_reserved_ports = {}-{}\n",
        RESERVED_PORTS.0, RESERVED_PORTS.1
    )
}

/// `/etc/k2app/containers.conf` (the engine points Podman at it with
/// `CONTAINERS_CONF_OVERRIDE`).
pub fn containers_conf(rt: OciRuntime) -> String {
    format!(
        "# K2 App runtime: the OCI runtime for Apps.\n\
         # Written by k2-root-helper `apps runtime {rt}`.\n\
         [engine]\n\
         runtime = \"{rt}\"\n",
        rt = rt.as_str()
    )
}

// ── per-App disk ceilings: cgroup v2 io.max (§6.2, stage W §5.1) ───────
//
// `IOWeight=` alone is inert on the schedulers the fleet runs (W: an App
// filling its data folder pushed Postgres p99 from ~40 ms to 10 s); a
// per-App `io.max` ceiling held Postgres at 8.8 ms. systemd writes it from
// `IO{Read,Write}{Bandwidth,IOPS}Max=<path> <n>`, resolving the path to
// the disk under the Apps' data (`k2app`'s home). Containers get the lines
// on their Quadlet unit (the user manager owns the delegated `io`
// controller); the `process` exception on its caps drop-in.

/// The path systemd resolves to the Apps' data disk.
pub const IO_PATH: &str = APP_HOME;
/// K2's default ceiling (W's drill: 40 MB/s and 2,000 write IOPS kept
/// Postgres at its baseline). Reads have no default ceiling.
pub const DEFAULT_IO_WBPS: u64 = 40 * 1024 * 1024;
pub const DEFAULT_IO_WIOPS: u64 = 2_000;
/// Bounds: a ceiling below these starves an App; above, it means nothing.
pub const IO_BPS_BOUNDS: (u64, u64) = (1024 * 1024, 64 * 1024 * 1024 * 1024);
pub const IO_IOPS_BOUNDS: (u64, u64) = (16, 10_000_000);

/// One App's ceilings; `None` = no ceiling for that knob.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IoMax {
    pub rbps: Option<u64>,
    pub wbps: Option<u64>,
    pub riops: Option<u64>,
    pub wiops: Option<u64>,
}

impl IoMax {
    /// What an App gets until someone changes it.
    pub fn k2_default() -> Self {
        Self { rbps: None, wbps: Some(DEFAULT_IO_WBPS), riops: None, wiops: Some(DEFAULT_IO_WIOPS) }
    }

    /// No ceiling at all (owner only).
    pub fn unlimited() -> Self {
        Self::default()
    }

    fn knobs(&self) -> [(&'static str, Option<u64>, bool); 4] {
        [("rbps", self.rbps, false), ("wbps", self.wbps, false), ("riops", self.riops, true), ("wiops", self.wiops, true)]
    }

    /// Every set value inside its bounds.
    pub fn check(&self) -> Result<(), String> {
        for (name, v, iops) in self.knobs() {
            if let Some(n) = v {
                let (lo, hi) = if iops { IO_IOPS_BOUNDS } else { IO_BPS_BOUNDS };
                if !(lo..=hi).contains(&n) {
                    return Err(format!("{name} {n} is outside {lo}..={hi}"));
                }
            }
        }
        Ok(())
    }

    /// The stored column: NULL = K2's default. A value that doesn't parse
    /// or check also reads as the default (never as "no ceiling").
    pub fn from_column(col: Option<&str>) -> Self {
        col.and_then(|c| serde_json::from_str::<IoMax>(c).ok())
            .filter(|m| m.check().is_ok())
            .unwrap_or_else(Self::k2_default)
    }

    pub fn to_column(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The knobs `new` allows more on than `self` does (a higher number,
    /// or no ceiling where there was one): what only the owner may do.
    pub fn loosened_by(&self, new: &IoMax) -> Vec<&'static str> {
        self.knobs()
            .iter()
            .zip(new.knobs().iter())
            .filter(|((_, old, _), (_, new, _))| match (old, new) {
                (Some(_), None) => true,
                (Some(o), Some(n)) => n > o,
                (None, _) => false,
            })
            .map(|((name, _, _), _)| *name)
            .collect()
    }

    /// The systemd lines (`[Service]` section).
    pub fn unit_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (key, v) in [
            ("IOReadBandwidthMax", self.rbps),
            ("IOWriteBandwidthMax", self.wbps),
            ("IOReadIOPSMax", self.riops),
            ("IOWriteIOPSMax", self.wiops),
        ] {
            if let Some(n) = v {
                out.push(format!("{key}={IO_PATH} {n}"));
            }
        }
        out
    }

    /// The helper's one argv token: `rbps.wbps.riops.wiops`, `-` = no
    /// ceiling (`-.41943040.-.2000` is K2's default).
    pub fn token(&self) -> String {
        let t = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
        format!("{}.{}.{}.{}", t(self.rbps), t(self.wbps), t(self.riops), t(self.wiops))
    }

    pub fn from_token(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split('.').collect();
        let [a, b, c, d] = p.as_slice() else { return None };
        Self::from_tokens([a, b, c, d])
    }

    fn from_tokens(t: [&str; 4]) -> Option<Self> {
        let one = |s: &str, b: (u64, u64)| -> Option<Option<u64>> {
            if s == "-" {
                Some(None)
            } else {
                bounded(s, b).map(Some)
            }
        };
        Some(Self {
            rbps: one(t[0], IO_BPS_BOUNDS)?,
            wbps: one(t[1], IO_BPS_BOUNDS)?,
            riops: one(t[2], IO_IOPS_BOUNDS)?,
            wiops: one(t[3], IO_IOPS_BOUNDS)?,
        })
    }
}

/// A user-typed rate: `none`, a plain number, or `<n>K|M|G` (powers of
/// 1024, an optional trailing `B` or `/s`). IOPS take plain numbers only.
pub fn parse_rate(s: &str, iops: bool) -> Result<Option<u64>, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("none") || t.eq_ignore_ascii_case("unlimited") {
        return Ok(None);
    }
    let t = t.strip_suffix("/s").unwrap_or(t);
    let (num, mul) = if iops {
        (t, 1u64)
    } else {
        let u = t.trim_end_matches(['B', 'b']);
        match u.chars().last() {
            Some('K' | 'k') => (&u[..u.len() - 1], 1024),
            Some('M' | 'm') => (&u[..u.len() - 1], 1024 * 1024),
            Some('G' | 'g') => (&u[..u.len() - 1], 1024 * 1024 * 1024),
            _ => (u, 1),
        }
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) || num.len() > 12 {
        return Err(format!("'{s}' is not a rate (try 40M, 2000 or none)"));
    }
    let n: u64 = num.parse().map_err(|_| format!("'{s}' is not a rate"))?;
    let v = n.checked_mul(mul).ok_or_else(|| format!("'{s}' is too large"))?;
    let (lo, hi) = if iops { IO_IOPS_BOUNDS } else { IO_BPS_BOUNDS };
    if !(lo..=hi).contains(&v) {
        return Err(format!("'{s}' is outside {lo}..={hi}{}", if iops { " IOPS" } else { " bytes/s" }));
    }
    Ok(Some(v))
}

// ── the `process` exception: devices and caps ─────────────────────────

/// One device the process exception may be granted (§4.3: "only the ones
/// named in App config, each a widening"). Tokens are enum-like names,
/// never paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGrant {
    pub token: String,
    pub path: String,
    /// The group that owns the node on Debian/Ubuntu/Fedora/Arch
    /// (`render` for render nodes, `video` for DRM cards); NVIDIA's nodes
    /// are 0666.
    pub group: Option<&'static str>,
}

pub const MAX_DEVICES: usize = 8;

/// `dri-render128`..`dri-render191`, `dri-card0`..`dri-card15`,
/// `nvidia0`..`nvidia15`, `nvidiactl`, `nvidia-uvm`, `nvidia-uvm-tools`,
/// `nvidia-modeset`.
pub fn device(token: &str) -> Option<DeviceGrant> {
    let num = |s: &str, lo: u32, hi: u32| -> Option<u32> {
        if s.is_empty() || s.len() > 3 || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
            return None;
        }
        s.parse().ok().filter(|n| (lo..=hi).contains(n))
    };
    let g = |path: String, group: Option<&'static str>| Some(DeviceGrant { token: token.to_string(), path, group });
    if let Some(n) = token.strip_prefix("dri-render") {
        return g(format!("/dev/dri/renderD{}", num(n, 128, 191)?), Some("render"));
    }
    if let Some(n) = token.strip_prefix("dri-card") {
        return g(format!("/dev/dri/card{}", num(n, 0, 15)?), Some("video"));
    }
    match token {
        "nvidiactl" | "nvidia-uvm" | "nvidia-uvm-tools" | "nvidia-modeset" => g(format!("/dev/{token}"), None),
        _ => {
            let n = token.strip_prefix("nvidia")?;
            g(format!("/dev/nvidia{}", num(n, 0, 15)?), None)
        }
    }
}

/// Bounds for the process exception's memory cap (MiB).
pub const PROCESS_MEM_MIB: (u64, u64) = (64, 65_536);

/// The per-instance caps drop-in, a runtime file
/// (`/run/systemd/system/k2app-process@<id>.service.d/k2-caps.conf`):
/// written by `apps process caps` before every start, gone at reboot. A
/// start refuses without it, so a process App never runs uncapped.
pub const RUNTIME_SYSTEMD_DIR: &str = "/run/systemd/system";
pub const PROCESS_CAPS_DROPIN: &str = "k2-caps.conf";

pub fn process_caps_dropin(mem_mib: u64, io: &IoMax, devices: &[DeviceGrant]) -> String {
    let mut s = String::from(
        "# K2 App runtime: this App's caps (k2-root-helper `apps process caps`).\n\
         [Service]\n",
    );
    s.push_str(&format!("MemoryMax={mem_mib}M\n"));
    for l in io.unit_lines() {
        s.push_str(&l);
        s.push('\n');
    }
    if !devices.is_empty() {
        // The cgroup device filter admits exactly these nodes (plus the
        // pseudo-devices `closed` keeps: null, zero, full, random, …).
        s.push_str("PrivateDevices=no\nDevicePolicy=closed\n");
        for d in devices {
            s.push_str(&format!("DeviceAllow={} rw\n", d.path));
        }
        let mut groups: Vec<&str> = devices.iter().filter_map(|d| d.group).collect();
        groups.sort();
        groups.dedup();
        if !groups.is_empty() {
            s.push_str(&format!("SupplementaryGroups={}\n", groups.join(" ")));
        }
    }
    s
}

// ── MAC: the `k2-app` AppArmor profile (§4.2 item 4, `apps apparmor`) ──
//
// Where SELinux is enforcing, rootless Podman already labels every
// container `container_t` and box prep leaves it alone. Where AppArmor is
// on, box prep loads the canned `k2-app` profile and:
// - guarded containers enter it through k2-app-engine's OCI shim (rootless
//   Podman refuses `--security-opt apparmor=`, so the shim writes
//   `process.apparmorProfile` into the OCI config and into every `exec`
//   process, and crun/youki apply it), when the marker below is present;
// - the `process` exception enters it through a drop-in on its template
//   (`AppArmorProfile=k2-app`, applied by PID 1; fails closed when the
//   profile is missing).

/// The profile name (also its file name under [`APPARMOR_DIR`]).
pub const APPARMOR_PROFILE: &str = "k2-app";
pub const APPARMOR_DIR: &str = "/etc/apparmor.d";
pub const APPARMOR_FILE: &str = "/etc/apparmor.d/k2-app";
/// `apparmor_parser`: merged-/usr distros have both; older ones only /sbin.
pub const APPARMOR_PARSER: &[&str] = &["/usr/sbin/apparmor_parser", "/sbin/apparmor_parser"];
/// `Y` when the AppArmor LSM is on.
pub const APPARMOR_ENABLED: &str = "/sys/module/apparmor/parameters/enabled";
/// Loaded profiles, one `name (mode)` per line (root reads it).
pub const APPARMOR_PROFILES: &str = "/sys/kernel/security/apparmor/profiles";
/// `1` when SELinux is enforcing.
pub const SELINUX_ENFORCE: &str = "/sys/fs/selinux/enforce";
/// Root-owned marker the engine's OCI shim reads (`k2-app\n`): present =
/// put every App process under the profile. Next to `containers.conf`.
pub const MAC_MARKER_FILE: &str = "apparmor";
pub const MAC_MARKER: &str = "/etc/k2app/apparmor";
/// Drop-ins on the `process` template (`k2app-process@.service.d/`).
pub const PROCESS_DROPIN_DIR: &str = "k2app-process@.service.d";
pub const PROCESS_APPARMOR_DROPIN: &str = "k2-apparmor.conf";

/// The box's MAC layer, from the two kernel files above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lsm {
    AppArmor,
    SeLinux,
    None,
}

impl Lsm {
    /// SELinux enforcing wins (the PRD: `container_t` where SELinux is on,
    /// otherwise AppArmor).
    pub fn detect(apparmor_enabled: Option<&str>, selinux_enforce: Option<&str>) -> Self {
        if selinux_enforce.map(str::trim) == Some("1") {
            Self::SeLinux
        } else if apparmor_enabled.map(str::trim) == Some("Y") {
            Self::AppArmor
        } else {
            Self::None
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppArmor => "apparmor",
            Self::SeLinux => "selinux",
            Self::None => "none",
        }
    }
}

/// The mode `k2-app` is loaded in (`enforce`, `complain`, …), from the
/// kernel's profile list; `None` when it isn't loaded.
pub fn profile_mode(profiles: &str, name: &str) -> Option<String> {
    profiles.lines().find_map(|l| {
        let (n, rest) = l.trim().rsplit_once(' ')?;
        (n == name).then(|| rest.trim_matches(|c| c == '(' || c == ')').to_string())
    })
}

/// The marker's canned bytes.
pub fn mac_marker() -> &'static str {
    "k2-app\n"
}

/// `k2app-process@.service.d/k2-apparmor.conf`.
pub fn process_apparmor_dropin() -> &'static str {
    "# K2 App runtime: the process exception runs under the k2-app AppArmor\n\
     # profile. Written by k2-root-helper `apps apparmor on`.\n\
     [Service]\n\
     AppArmorProfile=k2-app\n"
}

/// `/etc/apparmor.d/k2-app`: the container-shaped profile every App runs
/// under (Docker's and Podman's default profiles are the model). The wall
/// itself (the container's mounts, or the process unit's sandbox) decides
/// what files exist; this profile takes away the kernel's sharp edges:
/// writes to /proc and /sys, mounts, raw and packet sockets, every
/// capability (none is listed, so none is allowed), ptrace and signals
/// across the profile, and — for the `process` exception, which sees the
/// host's tree — the engine's state, other users' homes and the engine
/// socket.
pub fn apparmor_profile() -> &'static str {
    "# K2 App runtime: the MAC layer for Apps (prd-k2-app-runtime-v1 §4.2 item 4).\n\
     # Written by k2-root-helper `apps apparmor on`; edits are overwritten.\n\
     abi <abi/3.0>,\n\
     include <tunables/global>\n\
     \n\
     profile k2-app flags=(attach_disconnected,mediate_deleted) {\n\
     \x20 include <abstractions/base>\n\
     \n\
     \x20 # Sockets: TCP, UDP, Unix and netlink (getaddrinfo asks the kernel\n\
     \x20 # which address families exist). No raw IP, no packet sockets.\n\
     \x20 network,\n\
     \x20 deny network inet raw,\n\
     \x20 deny network inet6 raw,\n\
     \x20 deny network packet,\n\
     \x20 unix,\n\
     \n\
     \x20 file,\n\
     \x20 umount,\n\
     \x20 deny mount,\n\
     \x20 deny pivot_root,\n\
     \n\
     \x20 # /proc and /sys: read what a program needs, write almost nothing.\n\
     \x20 deny @{PROC}/* w,\n\
     \x20 deny @{PROC}/{[^1-9],[^1-9][^0-9],[^1-9s][^0-9y][^0-9s],[^1-9][^0-9][^0-9][^0-9/]*}/** w,\n\
     \x20 deny @{PROC}/sys/[^k]** w,\n\
     \x20 deny @{PROC}/sys/kernel/{?,??,[^s][^h][^m]**} w,\n\
     \x20 deny @{PROC}/sysrq-trigger rwklx,\n\
     \x20 deny @{PROC}/kcore rwklx,\n\
     \x20 deny @{PROC}/kmem rwklx,\n\
     \x20 deny @{PROC}/mem rwklx,\n\
     \x20 deny /sys/[^f]*/** wklx,\n\
     \x20 deny /sys/f[^s]*/** wklx,\n\
     \x20 deny /sys/fs/[^c]*/** wklx,\n\
     \x20 deny /sys/fs/c[^g]*/** wklx,\n\
     \x20 deny /sys/fs/cg[^r]*/** wklx,\n\
     \x20 deny /sys/firmware/** rwklx,\n\
     \x20 deny /sys/devices/virtual/powercap/** rwklx,\n\
     \x20 deny /sys/kernel/security/** rwklx,\n\
     \x20 deny /sys/kernel/debug/** rwklx,\n\
     \n\
     \x20 # The host tree the process exception could otherwise name (these\n\
     \x20 # paths don't exist inside a container).\n\
     \x20 deny /var/lib/k2app/engine/** rwklx,\n\
     \x20 deny /var/lib/k2app/.local/** rwklx,\n\
     \x20 deny /var/lib/k2app/.config/** rwklx,\n\
     \x20 deny /run/k2app/** rwklx,\n\
     \x20 deny /home/** rwklx,\n\
     \x20 deny /root/** rwklx,\n\
     \n\
     \x20 # Signals and ptrace stay inside the profile; the engine, conmon and\n\
     \x20 # systemd (unconfined) may signal the App. Where the distro confines\n\
     \x20 # crun with its own (unconfined-mode) profile, crun stops the App and\n\
     \x20 # the App's own tasks carry the stack crun//&k2-app.\n\
     \x20 signal (send,receive) peer=k2-app,\n\
     \x20 signal (send,receive) peer=\"crun//&k2-app\",\n\
     \x20 signal (receive) peer=unconfined,\n\
     \x20 signal (receive) peer=crun,\n\
     \x20 ptrace (trace,read,tracedby,readby) peer=k2-app,\n\
     }\n"
}

/// One per-App accept: sockets in `cgroup` (a path under
/// `/sys/fs/cgroup`, `level` components deep) may reach `gw_port`, and
/// the App's egress proxy port ([`proxy_port_for_gateway`], S15), on
/// loopback. nft resolves the path to the cgroup's inode when the rule
/// loads (W §2.12), so the daemon re-applies after every App start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRule {
    pub cgroup: String,
    pub level: u32,
    pub gw_port: u16,
}

/// The cgroup paths an App's sockets can live in: its container slice
/// inside `user@<k2app>` (pasta and conmon run in the App's unit there),
/// and its `process`-exception system unit.
pub fn app_cgroups(uid: u32, id: &AppId) -> [(String, u32); 2] {
    [
        (
            format!(
                "user.slice/user-{uid}.slice/user@{uid}.service/{APPS_SLICE}/k2apps-{}.slice",
                id.unit_safe()
            ),
            5,
        ),
        (format!("{APPS_SLICE}/{}", id.process_unit()), 2),
    ]
}

fn app_rule_line(r: &AppRule) -> String {
    let ports = match proxy_port_for_gateway(r.gw_port) {
        Some(px) => format!("{{ {}, {px} }}", r.gw_port),
        None => r.gw_port.to_string(),
    };
    format!(
        "socket cgroupv2 level {} \"{}\" oifname \"lo\" tcp dport {ports} accept",
        r.level, r.cgroup
    )
}

/// The `inet k2app` table (§10.4, W §5.6): for sockets owned by `k2app`
/// (container Apps reach the host through pasta, which runs as `k2app`)
/// or by `k2appx` (process Apps), allow replies, each App's own gateway
/// and egress proxy ports (per-App cgroup rules, [`NFT_APPS_CHAIN`]) and
/// the box's Postgres on loopback, and drop everything else: other
/// loopback ports (other Apps' gateway and proxy ports included), any
/// address of this host (`host.containers.internal` reached a 0.0.0.0
/// service in W), DNS, private and link-local nets, and the internet
/// (egress goes through the App's own proxy on loopback, S15). The
/// leading `table` + `delete table` make `nft -f` replace it atomically.
/// `gw` and `px` are the ranges per-App ports sit in; nothing accepts
/// either as a whole.
pub fn nft_ruleset(uid: u32, process_uid: Option<u32>, gw: PortRange, px: PortRange, db: PortRange, apps: &[AppRule]) -> String {
    let _ = (gw, px);
    let app_lines: String = apps.iter().map(|r| format!("\t\t{}\n", app_rule_line(r))).collect();
    let proc_line = process_uid
        .map(|pu| format!("\t\tmeta skuid {pu} jump {NFT_CHAIN}\n"))
        .unwrap_or_default();
    format!(
        "table inet {t}\n\
         delete table inet {t}\n\
         table inet {t} {{\n\
         \tchain output {{\n\
         \t\ttype filter hook output priority filter; policy accept;\n\
         \t\tmeta skuid {uid} jump {c}\n\
         {proc_line}\
         \t}}\n\
         \tchain {a} {{\n\
         {app_lines}\
         \t}}\n\
         \tchain {c} {{\n\
         \t\tct state established,related accept\n\
         \t\tjump {a}\n\
         \t\toifname \"lo\" tcp dport {db} accept\n\
         \t\tfib daddr type local counter drop\n\
         \t\tmeta l4proto {{ tcp, udp }} th dport 53 counter drop\n\
         \t\tip daddr {{ 10.0.0.0/8, 100.64.0.0/10, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 }} counter drop\n\
         \t\tip6 daddr {{ fc00::/7, fe80::/10 }} counter drop\n\
         \t\tcounter drop\n\
         \t}}\n\
         }}\n",
        t = NFT_TABLE,
        c = NFT_CHAIN,
        a = NFT_APPS_CHAIN,
        db = db.nft(),
    )
}

/// Replace only the per-App chain (`apps nft app` / `app-remove`).
pub fn nft_apps_script(apps: &[AppRule]) -> String {
    let mut s = format!("flush chain inet {NFT_TABLE} {NFT_APPS_CHAIN}\n");
    for r in apps {
        s.push_str(&format!("add rule inet {NFT_TABLE} {NFT_APPS_CHAIN} {}\n", app_rule_line(r)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_ranges() {
        assert_eq!(
            PortRange::parse("38600-38899"),
            Some(PortRange {
                lo: 38600,
                hi: 38899
            })
        );
        assert_eq!(
            PortRange::parse("5432"),
            Some(PortRange { lo: 5432, hi: 5432 })
        );
        assert_eq!(PortRange::parse("5432-5432").unwrap().token(), "5432");
        for bad in [
            "",
            "-",
            "1023",
            "80-90",
            "65536",
            "100-99999",
            "2000-1000",
            "1024-2048",
            "01024",
            "1024-",
            "-1024",
            "1024--1030",
            "1024-1030-1040",
            "１０２４",
            "+1024",
            "1024 ",
            "0x400",
        ] {
            assert!(PortRange::parse(bad).is_none(), "{bad:?}");
        }
        assert!(PortRange::parse("1024-2047").is_some());
    }

    #[test]
    fn app_ids() {
        for ok in [
            "a",
            "jobs",
            "nsi-skin",
            "a1",
            &format!("a{}", "b".repeat(31)),
        ] {
            assert!(AppId::parse(ok).is_some(), "{ok}");
        }
        for bad in [
            "",
            "1app",
            "-a",
            "a-",
            "a--b",
            "A",
            "a_b",
            "a.b",
            "a/b",
            "../x",
            "a b",
            "a@b",
            "a\n",
            &format!("a{}", "b".repeat(32)),
        ] {
            assert!(AppId::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(
            AppId::parse("jobs").unwrap().process_unit(),
            "k2app-process@jobs.service"
        );
    }

    #[test]
    fn hba_block_edits() {
        let jobs = AppId::parse("jobs").unwrap();
        let nsi = AppId::parse("nsi-skin").unwrap();
        assert_eq!(nsi.pg_name(), "app_nsi_skin");
        assert_eq!(hba_line(&jobs), "host app_jobs app_jobs 127.0.0.1/32 scram-sha-256");
        let base = "# PostgreSQL Client Authentication\n\nlocal all postgres peer\nhost all all 127.0.0.1/32 scram-sha-256\n";
        let a = hba_edit(base, true, &jobs).unwrap().unwrap();
        // Above the first rule, after the header.
        assert!(a.starts_with("# PostgreSQL Client Authentication\n\n# BEGIN K2 Apps"));
        let pos_line = a.find("host app_jobs").unwrap();
        assert!(pos_line < a.find("local all postgres").unwrap());
        assert_eq!(hba_edit(&a, true, &jobs).unwrap(), None, "idempotent");
        let b = hba_edit(&a, true, &nsi).unwrap().unwrap();
        assert_eq!(b.matches(HBA_BEGIN).count(), 1);
        assert!(b.contains("host app_jobs app_jobs") && b.contains("host app_nsi_skin app_nsi_skin"));
        let c = hba_edit(&b, false, &jobs).unwrap().unwrap();
        assert!(!c.contains("app_jobs") && c.contains("app_nsi_skin"));
        assert_eq!(hba_edit(&c, false, &jobs).unwrap(), None);
        // Last line out: the block goes, the file is as it was.
        assert_eq!(hba_edit(&c, false, &nsi).unwrap().unwrap(), base);
        assert_eq!(hba_edit(base, false, &jobs).unwrap(), None);
        for bad in [
            format!("{HBA_BEGIN}\nhost all all 0.0.0.0/0 trust\n{HBA_END}\n"),
            format!("{HBA_BEGIN}\n{HBA_BEGIN}\n{HBA_END}\n"),
            format!("{HBA_END}\n{HBA_BEGIN}\n"),
            format!("{HBA_BEGIN}\nhost app_jobs app_jobs 127.0.0.1/32 scram-sha-256\n"),
            format!("{HBA_BEGIN}\nhost app_jobs all 127.0.0.1/32 scram-sha-256\n{HBA_END}\n"),
        ] {
            assert!(hba_edit(&bad, true, &jobs).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bounded_numbers() {
        assert_eq!(bounded("512", BUDGET_MEM_MIB), Some(512));
        for bad in ["", "0", "255", "0512", "-1", "1e3", "512M", "99999999999"] {
            assert!(bounded(bad, BUDGET_MEM_MIB).is_none(), "{bad:?}");
        }
    }

    /// AR45/AR48: no dash nesting, no KillMode in the cores drop-ins, and
    /// the process unit is a system unit running as k2app.
    #[test]
    fn canned_bytes_rules() {
        assert!(!CORES_SLICE.contains('-') && !APPS_SLICE.contains('-'));
        for c in [Core::Daemon, Core::Postgres, Core::Stalwart] {
            let d = cores_dropin(c);
            assert!(d.contains("\nSlice=k2cores.slice\n"));
            assert!(!d.contains("KillMode"), "AR48");
            assert!(!d.contains("MemoryMax"), "the RAM fence owns MemoryMax");
        }
        let p = process_unit();
        for line in [
            "\nUser=k2appx\n",
            "\nSlice=k2apps.slice\n",
            "\nStateDirectory=k2appx/%i\n",
            "\nTemporaryFileSystem=/var/lib/k2app:ro\n",
            "\nBindReadOnlyPaths=/var/lib/k2app/apps/%i/process\n",
            "\nInaccessiblePaths=-/run/k2app\n",
            "\nIPAddressDeny=any\n",
            "\nPrivateDevices=yes\n",
            "\nProtectHome=yes\n",
            "\nCapabilityBoundingSet=\n",
            "\nRestrictNamespaces=yes\n",
            "\nNoNewPrivileges=yes\n",
            "\nRestart=no\n",
        ] {
            assert!(p.contains(line), "{line:?}");
        }
        assert!(!p.contains("[Install]"), "never enabled at boot by itself");
        assert!(!p.contains("User=k2app\n"), "never the engine's user: it owns every container's user namespace");
        assert!(!p.contains("StateDirectory=k2app/"), "systemd would chown k2app's tree recursively");
        assert!(!p.contains("MemoryMax"), "the caps drop-in sets it, per App");
        assert!(sysctl_conf().contains("= 38600-38999\n"));
        assert!(containers_conf(OciRuntime::Youki).contains("runtime = \"youki\"\n"));
        assert!(GATEWAY_PORTS.lo >= RESERVED_PORTS.0 && PROXY_PORTS.hi <= RESERVED_PORTS.1);
        const { assert!(GATEWAY_PORTS.hi < PROXY_PORTS.lo, "the ranges never overlap") };
    }

    #[test]
    fn proxy_port_shares_the_gateway_offset() {
        assert_eq!(proxy_port_for_gateway(38_600), Some(38_900));
        assert_eq!(proxy_port_for_gateway(38_663), Some(38_963));
        assert_eq!(proxy_port_for_gateway(38_699), Some(38_999));
        assert_eq!(proxy_port_for_gateway(38_700), None, "past the proxy range");
        assert_eq!(proxy_port_for_gateway(38_599), None);
        assert_eq!(proxy_port_for_gateway(38_900), None, "a proxy port is not a gateway port");
        // Every engine slot has both.
        for slot in 0..crate::engine::MAX_SLOTS {
            let gw = crate::engine::gateway_port(slot).unwrap();
            assert!(proxy_port_for_gateway(gw).is_some(), "slot {slot}");
        }
        let r = AppRule { cgroup: "x".into(), level: 2, gw_port: 38_899 };
        assert!(app_rule_line(&r).ends_with("tcp dport 38899 accept"), "no proxy port past the range");
    }

    #[test]
    fn io_ceilings() {
        let d = IoMax::k2_default();
        assert_eq!(d.wbps, Some(40 * 1024 * 1024));
        assert_eq!(d.wiops, Some(2000));
        assert_eq!((d.rbps, d.riops), (None, None));
        assert_eq!(IoMax::from_column(None), d, "NULL = the default ceiling");
        for bad in ["", "{", "{\"wbps\":1}", "{\"wbps\":null,\"x\":1}", "[]"] {
            assert_eq!(IoMax::from_column(Some(bad)), d, "{bad:?} must read as the default, never as no ceiling");
        }
        let u = IoMax::unlimited();
        assert_eq!(IoMax::from_column(Some(&u.to_column())), u);
        let mine = IoMax { rbps: Some(200 << 20), wbps: Some(10 << 20), riops: None, wiops: Some(500) };
        assert_eq!(IoMax::from_column(Some(&mine.to_column())), mine);

        // Loosening: a higher number, or a ceiling taken away.
        assert!(d.loosened_by(&d).is_empty());
        assert!(d.loosened_by(&mine).is_empty(), "mine is tighter on every knob d sets");
        assert_eq!(d.loosened_by(&u), vec!["wbps", "wiops"]);
        assert_eq!(mine.loosened_by(&d), vec!["rbps", "wbps", "wiops"]);
        let more = IoMax { wbps: Some(80 << 20), ..d };
        assert_eq!(d.loosened_by(&more), vec!["wbps"]);

        assert_eq!(
            d.unit_lines(),
            vec!["IOWriteBandwidthMax=/var/lib/k2app 41943040", "IOWriteIOPSMax=/var/lib/k2app 2000"]
        );
        assert!(u.unit_lines().is_empty());
        assert_eq!(d.token(), "-.41943040.-.2000");
        assert_eq!(IoMax::from_token("-.41943040.-.2000"), Some(d));
        assert_eq!(IoMax::from_token(&u.token()), Some(u));
        assert_eq!(IoMax::from_token(&mine.token()), Some(mine));
        for bad in ["x.-.-.-", "-.0.-.-", "-.1.-.-", "-.-.-.15", "-.-.-.02000", ".-.-.-", "-.-.-", "-.-.-.-.-", "", "-.-.-.-."] {
            assert_eq!(IoMax::from_token(bad), None, "{bad:?}");
        }

        assert_eq!(parse_rate("40M", false), Ok(Some(40 << 20)));
        assert_eq!(parse_rate("40MB/s", false), Ok(Some(40 << 20)));
        assert_eq!(parse_rate("1G", false), Ok(Some(1 << 30)));
        assert_eq!(parse_rate("none", false), Ok(None));
        assert_eq!(parse_rate("2000", true), Ok(Some(2000)));
        for bad in ["", "40X", "-1", "1.5M", "0", "512K", "999999999999999G", "65G", "M"] {
            assert!(parse_rate(bad, false).is_err(), "{bad:?}");
        }
        assert!(parse_rate("2k", true).is_err(), "IOPS are plain numbers");
        assert!(parse_rate("5", true).is_err(), "below the IOPS floor");
    }

    #[test]
    fn devices_are_tokens_never_paths() {
        let r = device("dri-render128").unwrap();
        assert_eq!((r.path.as_str(), r.group), ("/dev/dri/renderD128", Some("render")));
        assert_eq!(device("dri-card0").unwrap().path, "/dev/dri/card0");
        assert_eq!(device("dri-card0").unwrap().group, Some("video"));
        assert_eq!(device("nvidia3").unwrap().path, "/dev/nvidia3");
        assert_eq!(device("nvidia-uvm").unwrap().group, None);
        for bad in [
            "", "dri-render", "dri-render127", "dri-render192", "dri-render0128", "dri-card16", "dri-card-1", "nvidia",
            "nvidia16", "nvidia01", "/dev/dri/renderD128", "../sda", "sda", "kvm", "fuse", "dri-render128/x", "nvidia-caps",
        ] {
            assert!(device(bad).is_none(), "{bad:?}");
        }
        let io = IoMax::k2_default();
        let d = process_caps_dropin(512, &io, &[]);
        assert!(d.contains("\nMemoryMax=512M\n") && d.contains("\nIOWriteIOPSMax=/var/lib/k2app 2000\n"), "{d}");
        assert!(!d.contains("PrivateDevices=no") && !d.contains("DeviceAllow"), "no devices, PrivateDevices stays on");
        let d = process_caps_dropin(1024, &IoMax::unlimited(), &[device("dri-render128").unwrap(), device("dri-card1").unwrap(), device("nvidia0").unwrap()]);
        for want in [
            "\nPrivateDevices=no\nDevicePolicy=closed\n",
            "\nDeviceAllow=/dev/dri/renderD128 rw\n",
            "\nDeviceAllow=/dev/dri/card1 rw\n",
            "\nDeviceAllow=/dev/nvidia0 rw\n",
            "\nSupplementaryGroups=render video\n",
        ] {
            assert!(d.contains(want), "missing {want:?} in {d}");
        }
        assert!(!d.contains("IOWrite"));
    }

    #[test]
    fn mac_detection_and_profile_shape() {
        assert_eq!(Lsm::detect(Some("Y\n"), None), Lsm::AppArmor);
        assert_eq!(Lsm::detect(Some("Y"), Some("1\n")), Lsm::SeLinux, "SELinux enforcing wins");
        assert_eq!(Lsm::detect(Some("Y"), Some("0")), Lsm::AppArmor, "permissive SELinux is not a wall");
        assert_eq!(Lsm::detect(Some("N"), Some("0")), Lsm::None);
        assert_eq!(Lsm::detect(None, None), Lsm::None);
        let list = "docker-default (enforce)\nk2-app (enforce)\n/usr/bin/man (complain)\nk2-app-other (enforce)\n";
        assert_eq!(profile_mode(list, "k2-app").as_deref(), Some("enforce"));
        assert_eq!(profile_mode("k2-app (complain)\n", "k2-app").as_deref(), Some("complain"));
        assert_eq!(profile_mode("k2-app-other (enforce)\n", "k2-app"), None);
        assert_eq!(profile_mode("", "k2-app"), None);

        let p = apparmor_profile();
        assert!(p.contains("\nprofile k2-app flags=(attach_disconnected,mediate_deleted) {\n"), "{p}");
        assert!(p.starts_with("# K2 App runtime"));
        assert!(p.trim_end().ends_with('}'));
        assert_eq!(p.matches('{').count(), p.matches('}').count(), "braces");
        for want in [
            "  deny mount,\n",
            "  deny pivot_root,\n",
            "  deny network packet,\n",
            "  deny network inet raw,\n",
            "  deny @{PROC}/sysrq-trigger rwklx,\n",
            "  deny /sys/kernel/security/** rwklx,\n",
            "  deny /var/lib/k2app/engine/** rwklx,\n",
            "  deny /run/k2app/** rwklx,\n",
            "  ptrace (trace,read,tracedby,readby) peer=k2-app,\n",
            // Debian 13 confines crun with its own profile: crun stops the
            // App, and the App's tasks carry the stack (seen denied in the
            // guarded e2e before these rules).
            "  signal (receive) peer=crun,\n",
            "  signal (send,receive) peer=\"crun//&k2-app\",\n",
        ] {
            assert!(p.contains(want), "missing {want:?}");
        }
        // No capability is granted (AppArmor denies what isn't listed).
        assert!(!p.lines().any(|l| l.trim_start().starts_with("capability")), "{p}");
        for l in p.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            assert!(
                l.ends_with(',') || l.ends_with('{') || l == "}" || l.starts_with("include "),
                "every rule ends with a comma: {l:?}"
            );
        }
        assert_eq!(process_apparmor_dropin().lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>(), ["[Service]", "AppArmorProfile=k2-app"]);
        assert_eq!(mac_marker(), "k2-app\n");
        assert_eq!(MAC_MARKER, format!("{APP_ETC}/{MAC_MARKER_FILE}"));
        assert_eq!(APPARMOR_FILE, format!("{APPARMOR_DIR}/{APPARMOR_PROFILE}"));
    }

    #[test]
    fn nft_ruleset_shape() {
        let jobs = AppId::parse("nsi-skin").unwrap();
        let cg = app_cgroups(997, &jobs);
        assert_eq!(cg[0].0, "user.slice/user-997.slice/user@997.service/k2apps.slice/k2apps-nsi_skin.slice");
        assert_eq!(cg[1].0, "k2apps.slice/k2app-process@nsi-skin.service");
        let rule = AppRule { cgroup: cg[0].0.clone(), level: cg[0].1, gw_port: 38_601 };
        let r = nft_ruleset(997, Some(998), GATEWAY_PORTS, PROXY_PORTS, DB_PORTS, std::slice::from_ref(&rule));
        assert!(r.contains("meta skuid 998 jump k2app_v3"), "process Apps' user gets the same chain");
        assert!(r.starts_with("table inet k2app\ndelete table inet k2app\n"));
        assert!(r.contains("meta skuid 997 jump k2app_v3"));
        // No App reaches the whole gateway or proxy range: only its own
        // gateway and proxy ports, by cgroup (S15).
        assert!(!r.contains("38600-38899"), "{r}");
        assert!(!r.contains("38900-38999") && !r.contains("dport 389"), "{r}");
        assert!(r.contains(
            "socket cgroupv2 level 5 \"user.slice/user-997.slice/user@997.service/k2apps.slice/k2apps-nsi_skin.slice\" oifname \"lo\" tcp dport { 38601, 38901 } accept"
        ));
        assert!(r.contains("oifname \"lo\" tcp dport 5432 accept"));
        assert!(r.contains("fib daddr type local counter drop"));
        // Established first, then the per-App chain, the catch-all drop last.
        let est = r.find("ct state established").unwrap();
        let jump = r.find("jump k2app_apps").unwrap();
        let db = r.find("tcp dport 5432").unwrap();
        assert!(est < jump && jump < db);
        assert!(r.trim_end().ends_with("counter drop\n\t}\n}"));
        let empty = nft_ruleset(997, None, GATEWAY_PORTS, PROXY_PORTS, DB_PORTS, &[]);
        assert_eq!(empty.matches("meta skuid").count(), 1);
        assert!(empty.contains("\tchain k2app_apps {\n\t}\n"));
        let sc = nft_apps_script(&[rule]);
        assert!(sc.starts_with("flush chain inet k2app k2app_apps\nadd rule inet k2app k2app_apps socket cgroupv2 level 5 "));
    }
}
