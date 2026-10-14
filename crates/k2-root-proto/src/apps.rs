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
/// per-App gateway links, the egress proxy, the box's Postgres.
pub const GATEWAY_PORTS: PortRange = PortRange {
    lo: 38_600,
    hi: 38_899,
};
pub const PROXY_PORTS: PortRange = PortRange {
    lo: 38_900,
    hi: 38_949,
};
pub const DB_PORTS: PortRange = PortRange {
    lo: 5_432,
    hi: 5_432,
};

/// The nftables table. Coexists with the old sandbox's `k2sandbox_<uid>`
/// tables (AR24).
pub const NFT_TABLE: &str = "k2app";
/// The chain name doubles as the ruleset version marker (v2: gateway
/// ports only through per-App rules, runtime slice 3).
pub const NFT_CHAIN: &str = "k2app_v2";
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
pub fn delegate_dropin() -> &'static str {
    "# K2 App runtime box prep (k2-root-helper `apps delegate on`).\n\
     [Service]\n\
     Delegate=cpu cpuset io memory pids\n"
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
/// `User=k2app`, where systemd's sandboxing is real on every supported
/// systemd (249+). One instance per App, started and stopped only by the
/// daemon (`apps process start|stop <id>`). The engine reads the App's
/// spec; the unit carries no command of its own.
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
     User=k2app\n\
     Group=k2app\n\
     Slice=k2apps.slice\n\
     ExecStart=/usr/local/libexec/k2-app-engine process-run %i\n\
     StateDirectory=k2app/apps/%i\n\
     StateDirectoryMode=0700\n\
     WorkingDirectory=/var/lib/k2app/apps/%i\n\
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

/// One per-App accept: sockets in `cgroup` (a path under
/// `/sys/fs/cgroup`, `level` components deep) may reach `gw_port` on
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
    format!(
        "socket cgroupv2 level {} \"{}\" oifname \"lo\" tcp dport {} accept",
        r.level, r.cgroup, r.gw_port
    )
}

/// The `inet k2app` table (§10.4, W §5.6): for sockets owned by `k2app`
/// (container Apps reach the host through pasta, which runs as `k2app`;
/// process Apps run as `k2app`), allow replies, each App's own gateway
/// port (per-App cgroup rules, [`NFT_APPS_CHAIN`]), the egress proxy and
/// the box's Postgres on loopback, and drop everything else: other
/// loopback ports (other Apps' gateway ports included), any address of
/// this host (`host.containers.internal` reached a 0.0.0.0 service in W),
/// DNS, private and link-local nets, and the internet (egress goes through
/// the proxy on loopback). The leading `table` + `delete table` make
/// `nft -f` replace it atomically. `gw` is the range per-App ports must
/// sit in; nothing accepts it as a whole.
pub fn nft_ruleset(uid: u32, gw: PortRange, px: PortRange, db: PortRange, apps: &[AppRule]) -> String {
    let _ = gw;
    let app_lines: String = apps.iter().map(|r| format!("\t\t{}\n", app_rule_line(r))).collect();
    format!(
        "table inet {t}\n\
         delete table inet {t}\n\
         table inet {t} {{\n\
         \tchain output {{\n\
         \t\ttype filter hook output priority filter; policy accept;\n\
         \t\tmeta skuid {uid} jump {c}\n\
         \t}}\n\
         \tchain {a} {{\n\
         {app_lines}\
         \t}}\n\
         \tchain {c} {{\n\
         \t\tct state established,related accept\n\
         \t\tjump {a}\n\
         \t\toifname \"lo\" tcp dport {px} accept\n\
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
        px = px.nft(),
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
            "\nUser=k2app\n",
            "\nSlice=k2apps.slice\n",
            "\nProtectHome=yes\n",
            "\nCapabilityBoundingSet=\n",
            "\nRestrictNamespaces=yes\n",
            "\nNoNewPrivileges=yes\n",
            "\nRestart=no\n",
        ] {
            assert!(p.contains(line), "{line:?}");
        }
        assert!(!p.contains("[Install]"), "never enabled at boot by itself");
        assert!(sysctl_conf().contains("= 38600-38999\n"));
        assert!(containers_conf(OciRuntime::Youki).contains("runtime = \"youki\"\n"));
        assert!(GATEWAY_PORTS.lo >= RESERVED_PORTS.0 && PROXY_PORTS.hi <= RESERVED_PORTS.1);
    }

    #[test]
    fn nft_ruleset_shape() {
        let jobs = AppId::parse("nsi-skin").unwrap();
        let cg = app_cgroups(997, &jobs);
        assert_eq!(cg[0].0, "user.slice/user-997.slice/user@997.service/k2apps.slice/k2apps-nsi_skin.slice");
        assert_eq!(cg[1].0, "k2apps.slice/k2app-process@nsi-skin.service");
        let rule = AppRule { cgroup: cg[0].0.clone(), level: cg[0].1, gw_port: 38_601 };
        let r = nft_ruleset(997, GATEWAY_PORTS, PROXY_PORTS, DB_PORTS, std::slice::from_ref(&rule));
        assert!(r.starts_with("table inet k2app\ndelete table inet k2app\n"));
        assert!(r.contains("meta skuid 997 jump k2app_v2"));
        // No App reaches the whole gateway range: only its own port, by cgroup.
        assert!(!r.contains("38600-38899"), "{r}");
        assert!(r.contains(
            "socket cgroupv2 level 5 \"user.slice/user-997.slice/user@997.service/k2apps.slice/k2apps-nsi_skin.slice\" oifname \"lo\" tcp dport 38601 accept"
        ));
        assert!(r.contains("oifname \"lo\" tcp dport 38900-38949 accept"));
        assert!(r.contains("oifname \"lo\" tcp dport 5432 accept"));
        assert!(r.contains("fib daddr type local counter drop"));
        // Established first, then the per-App chain, the catch-all drop last.
        let est = r.find("ct state established").unwrap();
        let jump = r.find("jump k2app_apps").unwrap();
        let px = r.find("tcp dport 38900").unwrap();
        assert!(est < jump && jump < px);
        assert!(r.trim_end().ends_with("counter drop\n\t}\n}"));
        let empty = nft_ruleset(997, GATEWAY_PORTS, PROXY_PORTS, DB_PORTS, &[]);
        assert!(empty.contains("\tchain k2app_apps {\n\t}\n"));
        let sc = nft_apps_script(&[rule]);
        assert!(sc.starts_with("flush chain inet k2app k2app_apps\nadd rule inet k2app k2app_apps socket cgroupv2 level 5 "));
    }
}
