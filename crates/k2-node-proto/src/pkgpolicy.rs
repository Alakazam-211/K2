//! Who may make a node install what (protocol 7,
//! prd-compute-self-serve-environments-v1 §3, §6).
//!
//! - **A policy per server**, set by the node's owner in a root-only file,
//!   [`POLICY_FILE`] (`root:root 0644`, outside the `k2nodectl`-writable
//!   `/etc/k2-node/`, so the human's agents can't widen it, and separate
//!   from the approved list, which an older k2-node rewrites with only its
//!   own key).
//! - Three [`Level`]s: `list` (only the node's approved list: an upgraded
//!   node, no change), `profiles` (plus the K2 profiles the owner turned on
//!   for that server: the default for other people's servers), `official`
//!   (anything from the distro's signed official repos, minus the
//!   [`DENY`] list: the default for the node's own server).
//! - Narrowing applies at once; widening is the owner's (privileged).
//! - A missing name becomes a **request**: filed by any server with a grant,
//!   relayed through the node to its owner server, decided there by a
//!   person ([`PkgRequest`], [`PkgRequestFwd`], [`PkgRequestDecide`],
//!   [`PkgRequestDone`]).
//! - The root executor re-checks everything right before it installs: the
//!   policy, the deny list, a dry run (nothing removed or downgraded, only
//!   official origins, under the size cap). A controller can't skip them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The policy file (root's).
pub const POLICY_FILE: &str = "/etc/k2-node-package-policy.toml";
/// Root's append-only install log (one JSON line per install).
pub const LOG_FILE: &str = "/var/log/k2-node/packages.log";

/// Offer feature: the node sends [`PkgStatus`] per session.
pub const FEATURE_PKG_STATUS: &str = "pkg-status";
/// Offer feature: the node takes `pkg_request` and relays it to its owner.
pub const FEATURE_PKG_REQUESTS: &str = "pkg-requests";

/// Open requests per server per node.
pub const MAX_OPEN_PER_SERVER: usize = 10;
/// A request expires unanswered after this long.
pub const REQUEST_TTL_SECS: i64 = 24 * 3600;
/// Longest `--why`.
pub const MAX_WHY: usize = 200;
/// Most names in one request.
pub const MAX_REQUEST_NAMES: usize = 128;
/// Most approved names an offer lists.
pub const MAX_STATUS_NAMES: usize = 512;
/// The default size cap per request, after the dry run.
pub const DEFAULT_MAX_INSTALL_MB: u64 = 2048;

/// Stable refusal codes (both ends).
pub mod codes {
    /// Not on the list or the policy: a request (a card) is the way.
    pub const PACKAGE_NOT_APPROVED: &str = "package_not_approved";
    /// No name for this node's package manager.
    pub const NO_PACKAGE_MAPPING: &str = "no_package_mapping";
    /// A virtual name with several providers.
    pub const PACKAGE_AMBIGUOUS: &str = "package_ambiguous";
    /// On the deny list (boot, login, services, containers…).
    pub const PACKAGE_DENIED: &str = "package_denied";
    /// The dry run removes or downgrades something.
    pub const PACKAGE_REMOVES: &str = "package_removes";
    /// The dry run pulls something from a repo outside the distro's own.
    pub const PACKAGE_THIRD_PARTY: &str = "package_third_party";
    /// Bigger than the server's `max_install_mb`.
    pub const PACKAGE_TOO_BIG: &str = "package_too_big";
    /// The manager's signature check is off on this machine.
    pub const PKG_SIGNING_OFF: &str = "pkg_signing_off";
    /// The package database is out of date and K2 won't sync it alone
    /// (Arch: a full upgrade first).
    pub const INDEX_STALE: &str = "index_stale";
    /// The package manager's lock stayed held (unattended upgrades).
    pub const PACKAGE_BUSY: &str = "package_busy";
    /// The manager ran and failed.
    pub const PACKAGE_INSTALL_FAILED: &str = "package_install_failed";
    /// Too many open requests from this server on this node.
    pub const TOO_MANY_PENDING: &str = "too_many_pending";
    /// This node's k2-node takes no requests (older than protocol 7).
    pub const REQUESTS_UNSUPPORTED: &str = "pkg_requests_unsupported";
    /// macOS: nothing to install (brew at the machine).
    pub const PACKAGES_LINUX_ONLY: &str = crate::admin::PACKAGES_LINUX_ONLY;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    List,
    Profiles,
    Official,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::List => "list",
            Level::Profiles => "profiles",
            Level::Official => "official",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        match s.trim() {
            "list" => Some(Level::List),
            "profiles" => Some(Level::Profiles),
            "official" => Some(Level::Official),
            _ => None,
        }
    }
}

/// One server's package policy on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerPolicy {
    pub level: Level,
    /// `tauri-linux@1`: the profiles (and the version the owner accepted)
    /// this server may install without asking, at `profiles` or above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    /// The size cap per request, after the dry run.
    #[serde(default = "default_mb")]
    pub max_install_mb: u64,
}

fn default_mb() -> u64 {
    DEFAULT_MAX_INSTALL_MB
}

impl Default for ServerPolicy {
    fn default() -> Self {
        ServerPolicy { level: Level::List, profiles: Vec::new(), max_install_mb: DEFAULT_MAX_INSTALL_MB }
    }
}

impl ServerPolicy {
    /// The default at enrol for the node's own server (Rosson, Q1).
    pub fn own_default() -> Self {
        ServerPolicy { level: Level::Official, ..Default::default() }
    }

    /// The default when another person's server is shared (Rosson, Q2):
    /// every K2 profile at its current version.
    pub fn team_default() -> Self {
        ServerPolicy {
            level: Level::Profiles,
            profiles: crate::profiles::names().iter().filter_map(|n| crate::profiles::latest(n)).map(|p| p.id()).collect(),
            ..Default::default()
        }
    }

    /// Does `to` cover anything this doesn't (a higher level, a profile or
    /// a newer profile version not here, a bigger cap)? Then it's the
    /// owner's, privileged; otherwise it narrows and applies at once.
    pub fn widened_by(&self, to: &ServerPolicy) -> bool {
        to.level > self.level || to.max_install_mb > self.max_install_mb || to.profiles.iter().any(|p| !self.profiles.contains(p))
    }

    pub fn check(&self) -> Result<(), String> {
        if !(1..=65_536).contains(&self.max_install_mb) {
            return Err("max_install_mb is 1 to 65536".into());
        }
        for p in &self.profiles {
            let (n, v) = crate::profiles::parse_ref(p)?;
            if crate::profiles::get(&n, v).is_none() {
                return Err(format!("profile {p:?} isn't one K2 ships ({})", crate::profiles::names().join(", ")));
            }
        }
        Ok(())
    }

    /// One line for `packages <node>`: `profiles (tauri-linux@1, build-base@1), ≤ 2048 MB`.
    pub fn summary(&self) -> String {
        let p = if self.profiles.is_empty() || self.level == Level::List { String::new() } else { format!(" ({})", self.profiles.join(", ")) };
        format!("{}{p}, ≤ {} MB per request", self.level.as_str(), self.max_install_mb)
    }
}

/// A non-owner server's line in the policy file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerLine {
    /// The server's key fingerprint (its 16-hex prefix or more).
    pub fp: String,
    #[serde(flatten)]
    pub policy: ServerPolicy,
}

/// The whole file: `[servers.own]` and one table per other server, keyed
/// by its label (shown), matched by `fp`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyFile {
    pub own: Option<ServerPolicy>,
    pub servers: BTreeMap<String, ServerLine>,
}

impl PolicyFile {
    pub fn parse(text: &str) -> Result<PolicyFile, String> {
        let t: toml::Table = text.parse().map_err(|e| format!("package policy file: {e}"))?;
        let mut out = PolicyFile::default();
        let Some(servers) = t.get("servers") else { return Ok(out) };
        let servers = servers.as_table().ok_or("package policy: [servers.…] tables")?;
        for (k, v) in servers {
            if k == "own" {
                let p: ServerPolicy = v.clone().try_into().map_err(|e| format!("package policy [servers.own]: {e}"))?;
                p.check().map_err(|e| format!("package policy [servers.own]: {e}"))?;
                out.own = Some(p);
            } else {
                let l: ServerLine = v.clone().try_into().map_err(|e| format!("package policy [servers.{k:?}]: {e}"))?;
                if l.fp.len() < 16 || !l.fp.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!("package policy [servers.{k:?}]: fp is the server's key fingerprint (16+ hex)"));
                }
                l.policy.check().map_err(|e| format!("package policy [servers.{k:?}]: {e}"))?;
                out.servers.insert(k.clone(), l);
            }
        }
        Ok(out)
    }

    pub fn render(&self) -> String {
        let mut s = String::from(
            "# Which system packages each server's compute jobs may install on this\n\
             # node without asking (root's file). Edit with: sudo k2-node packages policy …\n\
             # level = list | profiles | official (official = the distro's signed repos,\n\
             # minus K2's deny list). Anything else becomes a request to this node's owner.\n",
        );
        let block = |s: &mut String, head: &str, fp: Option<&str>, p: &ServerPolicy| {
            s.push_str(&format!("\n[servers.{head}]\n"));
            if let Some(fp) = fp {
                s.push_str(&format!("fp = \"{fp}\"\n"));
            }
            s.push_str(&format!("level = \"{}\"\n", p.level.as_str()));
            if !p.profiles.is_empty() {
                s.push_str(&format!("profiles = [{}]\n", p.profiles.iter().map(|x| format!("\"{x}\"")).collect::<Vec<_>>().join(", ")));
            }
            s.push_str(&format!("max_install_mb = {}\n", p.max_install_mb));
        };
        if let Some(o) = &self.own {
            block(&mut s, "own", None, o);
        }
        for (k, l) in &self.servers {
            block(&mut s, &format!("\"{}\"", k.replace(['"', '\\', '\n'], "")), Some(&l.fp), &l.policy);
        }
        s
    }

    /// The policy for a session: the owner's, or the line whose `fp`
    /// prefixes the server's fingerprint. No line = `list` (today).
    pub fn for_server(&self, is_owner: bool, server_fp: &str) -> ServerPolicy {
        if is_owner {
            return self.own.clone().unwrap_or_default();
        }
        self.servers
            .values()
            .find(|l| !server_fp.is_empty() && (server_fp.starts_with(&l.fp) || l.fp.starts_with(server_fp)))
            .map(|l| l.policy.clone())
            .unwrap_or_default()
    }
}

/// Packages K2 never installs, at any level (§3.6): what changes how the
/// machine boots or logs in, starts a network service or a container
/// runtime, or replaces the base system. `*` matches any tail. The owner
/// can still install these by hand.
pub const DENY: &[&str] = &[
    // Kernels and boot.
    "linux", "linux-image-*", "linux-headers-*", "linux-generic*", "linux-modules-*", "linux-lts", "linux-zen", "linux-hardened", "linux-firmware*",
    "kernel", "kernel-core", "kernel-default*", "kernel-modules*", "grub*", "grub2*", "shim*", "systemd-boot*", "efibootmgr", "limine", "refind", "syslinux",
    "mkinitcpio", "dracut*", "initramfs-tools*",
    // Init and the base system.
    "systemd", "systemd-sysv", "init", "sysvinit*", "openrc", "runit", "base", "base-files", "filesystem", "glibc", "libc6", "libc-bin", "coreutils", "bash", "dash",
    "util-linux", "apt", "dpkg", "pacman", "dnf", "rpm", "zypper",
    // Privilege and login.
    "sudo", "doas", "opendoas", "polkit*", "policykit*", "pam", "libpam*", "pam-*", "shadow", "passwd", "login", "accountsservice",
    // Remote access and the network.
    "openssh", "openssh-server", "ssh", "dropbear*", "telnetd", "network-manager*", "networkmanager*", "netplan*", "ifupdown*", "systemd-networkd", "iwd",
    "wpa_supplicant", "dhcpcd", "ufw", "firewalld", "iptables-persistent", "nftables", "netfilter-persistent", "tailscale", "wireguard*", "openvpn",
    // Containers and VMs.
    "docker*", "containerd*", "podman*", "runc", "crun", "cri-o", "lxc*", "lxd*", "incus*", "libvirt*", "qemu-system*", "virtualbox*",
    // Display and login managers.
    "gdm", "gdm3", "sddm", "lightdm*", "xdm", "lxdm", "greetd",
    // Servers and daemons.
    "apache2*", "httpd", "nginx*", "lighttpd", "mysql-server*", "mariadb-server*", "mariadb", "postgresql", "postgresql-1*", "postgresql-server*", "redis-server",
    "redis", "valkey", "mongodb*", "memcached", "rabbitmq-server", "bind9", "bind", "dnsmasq", "unbound", "postfix", "exim4*", "sendmail", "dovecot*", "cups",
    "cups-daemon", "avahi*", "samba", "nfs-kernel-server", "nfs-utils", "vsftpd", "proftpd*", "cron", "cronie", "anacron", "at", "snapd", "flatpak",
    "unattended-upgrades", "packagekit*",
];

/// The deny entry that matches `name`, if any.
pub fn denied(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    DENY.iter().copied().find(|d| match d.strip_suffix('*') {
        Some(prefix) => n.starts_with(prefix),
        None => n == *d,
    })
}

/// Why a name may (or may not) install without asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coverage {
    /// On the node's approved list.
    Listed,
    /// In a profile the owner turned on for this server (`tauri-linux@1`).
    Profile(String),
    /// The policy is `official` and nothing denies it.
    Official,
    /// On the deny list: never, at any level.
    Denied(&'static str),
    /// Not covered: a request.
    Ask,
}

impl Coverage {
    pub fn covered(&self) -> bool {
        matches!(self, Coverage::Listed | Coverage::Profile(_) | Coverage::Official)
    }
}

/// Is canonical `name` (installing `natives` on this node) covered for a
/// server with `policy`, given the node's approved list?
pub fn coverage(policy: &ServerPolicy, approved: &[String], name: &str, natives: &[String]) -> Coverage {
    if let Some(d) = denied(name).or_else(|| natives.iter().find_map(|n| denied(n))) {
        return Coverage::Denied(d);
    }
    if approved.iter().any(|a| a == name) {
        return Coverage::Listed;
    }
    if policy.level >= Level::Profiles {
        for p in &policy.profiles {
            let Ok((n, v)) = crate::profiles::parse_ref(p) else { continue };
            if let Some(pr) = crate::profiles::get(&n, v) {
                if pr.all_sys().iter().any(|s| s == name) {
                    return Coverage::Profile(pr.id());
                }
            }
        }
    }
    if policy.level == Level::Official {
        return Coverage::Official;
    }
    Coverage::Ask
}

// ── what the offer says (S2) ─────────────────────────────────────────

/// Protocol 7, feature [`FEATURE_PKG_STATUS`]: one server's view of the
/// node's packages (§5.3). Absent for an older server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkgStatus {
    /// `apt` | `dnf` | `pacman` | `zypper`.
    pub manager: String,
    /// This server's level and profiles.
    pub level: Level,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    pub max_install_mb: u64,
    /// The node's approved list (canonical names, at most 512).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved: Vec<String>,
    /// Which of the approved names, and of the names this server asked for
    /// in its last jobs, are installed (canonical).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installed: Vec<String>,
    /// Seconds since the package index was refreshed (apt, zypper).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_age_secs: Option<u64>,
    /// This server's open requests on the node.
    #[serde(default)]
    pub open_requests: u32,
}

impl PkgStatus {
    pub fn policy(&self) -> ServerPolicy {
        ServerPolicy { level: self.level, profiles: self.profiles.clone(), max_install_mb: self.max_install_mb }
    }
}

// ── request frames (S3) ──────────────────────────────────────────────

/// `pkg_request` (any server with a session → node): please ask the
/// node's owner for these names. The node answers with
/// [`PkgRequestDone`] when the owner decides (or it expires).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkgRequest {
    /// The asking server's id for it (`cpr_…`); unique per server.
    pub id: String,
    /// Canonical, native (`pacman:x`) or `profile:` names.
    pub names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// At most [`MAX_WHY`] characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Who asked, as the asking server shows it (`tests` agent): words only.
    pub asked_by: String,
}

/// What a dry run found (§3.6), for the card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrySummary {
    /// Every package the install pulls in (dependencies included).
    pub installs: Vec<String>,
    /// Installed size in MB (rounded up), when the manager says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_mb: Option<u64>,
    /// `libfreetype6-dev → libfreetype-dev`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved: Vec<String>,
    /// Already installed (nothing to do for them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub already: Vec<String>,
    /// A guard that would refuse it now (`package_denied: sudo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

/// `pkg_request_fwd` (node → its owner server): a request to decide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkgRequestFwd {
    /// The node's id for it (`npr_…`), what the owner decides on.
    pub id: String,
    /// The asking server's label (`ster.k2.dev`), or `own`.
    pub server: String,
    /// The asker's own id for it.
    pub server_request: String,
    pub names: Vec<String>,
    /// Canonical → native, as this node would install them.
    #[serde(default)]
    pub natives: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub asked_by: String,
    /// `Ubuntu 24.04.1 LTS`.
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry: Option<DrySummary>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// `pkg_request_decide` (owner server → node): deny a request. An approval
/// goes as the signed admin command `packages.allow {packages, install,
/// request}` instead, so root's guards run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkgRequestDecide {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Request states.
pub mod state {
    pub const PENDING: &str = "pending";
    pub const INSTALLED: &str = "installed";
    pub const DENIED: &str = "denied";
    pub const FAILED: &str = "failed";
    pub const EXPIRED: &str = "expired";
}

/// `pkg_request_done` (node → the asking server): the outcome. The asker
/// learns the owner server's label, never the owner's people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkgRequestDone {
    /// The asker's own id.
    pub server_request: String,
    /// [`state`]: installed | denied | failed | expired.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installed: Vec<String>,
    /// The owner server's label.
    pub owner: String,
}

/// `names` as a request takes them: 1–128 canonical, native or `profile:`
/// names, each valid; `why` at most 200 characters, one line.
pub fn check_request(names: &[String], why: Option<&str>) -> Result<(), String> {
    if names.is_empty() || names.len() > MAX_REQUEST_NAMES {
        return Err(format!("name 1 to {MAX_REQUEST_NAMES} packages"));
    }
    for n in names {
        if let Some(p) = n.strip_prefix("profile:") {
            crate::profiles::resolve(p)?;
        } else if crate::pkgmap::SysName::parse(n.strip_prefix("sys:").unwrap_or(n)).is_none() {
            return Err(format!("{n:?} isn't a package name (a-z 0-9 . _ + -, or apt:/pacman:/dnf:/zypper:<name>, or profile:<name>)"));
        }
    }
    if let Some(w) = why {
        if w.chars().count() > MAX_WHY || w.chars().any(char::is_control) {
            return Err(format!("--why is one line of at most {MAX_WHY} characters"));
        }
    }
    Ok(())
}

/// Expand `profile:` names into their canonical system names; strip `sys:`.
pub fn expand_names(names: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if let Some(p) = n.strip_prefix("profile:") {
            out.extend(crate::profiles::resolve(p)?.all_sys());
        } else {
            out.push(n.strip_prefix("sys:").unwrap_or(n).to_string());
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|x| seen.insert(x.clone()));
    Ok(out)
}

// ── admin verb arguments (both ends check them) ─────────────────────

/// `packages.allow`: CE-24's `{package}` (one name onto the approved list),
/// or protocol 7's `{packages, install, add, request}`: several names; with
/// `install` root runs the dry run and guards and installs them now; with
/// `add` they also join the approved list; `request` names the node's
/// request (`npr_…`) it answers, or the server's own (`cpr_…`). An older
/// executor refuses the new keys (`bad_args`); the controller then falls
/// back to one `{package}` per name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    #[serde(default)]
    pub install: bool,
    #[serde(default)]
    pub add: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}

impl AllowArgs {
    /// Every name, the old form and the new.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.package.iter().cloned().collect();
        v.extend(self.packages.iter().cloned());
        v
    }
}

pub fn check_allow_args(args: &serde_json::Value) -> Result<AllowArgs, String> {
    let a: AllowArgs = serde_json::from_value(args.clone()).map_err(|e| format!("packages.allow: {e}"))?;
    if a.package.is_some() && !a.packages.is_empty() {
        return Err("packages.allow: package or packages, not both".into());
    }
    let names = a.names();
    if names.is_empty() || names.len() > MAX_REQUEST_NAMES {
        return Err(format!("packages.allow: 1 to {MAX_REQUEST_NAMES} names"));
    }
    for n in &names {
        if crate::pkgmap::SysName::parse(n).is_none() {
            return Err(format!("packages.allow: {n:?} isn't a package name (a-z 0-9 . _ + -, or apt:/pacman:/dnf:/zypper:<name>)"));
        }
    }
    if a.package.is_some() && (a.install || a.add || a.request.is_some()) {
        return Err("packages.allow: install, add and request go with packages".into());
    }
    if !a.install && !a.add && a.package.is_none() {
        return Err("packages.allow: say install, add, or both".into());
    }
    if a.request.as_deref().is_some_and(|r| r.is_empty() || r.len() > 64 || !r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        return Err("packages.allow: request is a request id".into());
    }
    Ok(a)
}

/// `packages.policy {server, level?, profiles?, max_install_mb?}`: `server`
/// is `own`, or another server's label as `k2-node servers list` shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyArgs {
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_install_mb: Option<u64>,
}

impl PolicyArgs {
    /// `current` with these changes applied. A bare profile name pins the
    /// newest version.
    pub fn apply(&self, current: &ServerPolicy) -> ServerPolicy {
        let mut p = current.clone();
        if let Some(l) = self.level {
            p.level = l;
        }
        if let Some(pr) = &self.profiles {
            p.profiles = pr
                .iter()
                .map(|x| match crate::profiles::parse_ref(x) {
                    Ok((n, None)) => crate::profiles::latest(&n).map(|p| p.id()).unwrap_or_else(|| x.clone()),
                    _ => x.clone(),
                })
                .collect();
        }
        if let Some(m) = self.max_install_mb {
            p.max_install_mb = m;
        }
        p
    }
}

pub fn check_policy_args(args: &serde_json::Value) -> Result<PolicyArgs, String> {
    let a: PolicyArgs = serde_json::from_value(args.clone()).map_err(|e| format!("packages.policy: {e}"))?;
    if a.server.is_empty() || a.server.len() > 253 || a.server.chars().any(|c| c.is_control() || c == '"' || c == '\\') {
        return Err("packages.policy: server is own or a server's label".into());
    }
    if a.level.is_none() && a.profiles.is_none() && a.max_install_mb.is_none() {
        return Err("packages.policy: change level, profiles or max_install_mb".into());
    }
    a.apply(&ServerPolicy::default()).check().map_err(|e| format!("packages.policy: {e}"))?;
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_file_round_trips() {
        let mut f = PolicyFile { own: Some(ServerPolicy::own_default()), ..Default::default() };
        f.servers.insert(
            "ster.k2.dev".into(),
            ServerLine { fp: "c41f00aa11bb22cc".into(), policy: ServerPolicy { level: Level::Profiles, profiles: vec!["tauri-linux@1".into()], max_install_mb: 1024 } },
        );
        let back = PolicyFile::parse(&f.render()).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.for_server(true, "").level, Level::Official);
        assert_eq!(back.for_server(false, "c41f00aa11bb22cc99ee").level, Level::Profiles);
        assert_eq!(back.for_server(false, "ffff00aa11bb22cc").level, Level::List, "no line = list, today's rule");
        assert_eq!(PolicyFile::parse("").unwrap().for_server(true, "").level, Level::List, "an upgraded node changes nothing");
    }

    #[test]
    fn policy_file_refuses_junk() {
        assert!(PolicyFile::parse("[servers.own]\nlevel = \"root\"\n").is_err());
        assert!(PolicyFile::parse("[servers.own]\nlevel = \"profiles\"\nprofiles = [\"nope@1\"]\n").is_err());
        assert!(PolicyFile::parse("[servers.\"x\"]\nfp = \"zz\"\nlevel = \"list\"\n").is_err());
        assert!(PolicyFile::parse("[servers.own]\nlevel = \"list\"\nmax_install_mb = 0\n").is_err());
    }

    #[test]
    fn levels_cover_what_they_say() {
        let approved = vec!["libssl-dev".to_string()];
        let list = ServerPolicy::default();
        let prof = ServerPolicy { level: Level::Profiles, profiles: vec!["tauri-linux@1".into()], ..Default::default() };
        let off = ServerPolicy::own_default();
        let n = |x: &str| vec![x.to_string()];
        assert_eq!(coverage(&list, &approved, "libssl-dev", &n("libssl-dev")), Coverage::Listed);
        assert_eq!(coverage(&list, &approved, "libgtk-3-dev", &n("libgtk-3-dev")), Coverage::Ask);
        assert_eq!(coverage(&prof, &approved, "libgtk-3-dev", &n("libgtk-3-dev")), Coverage::Profile("tauri-linux@1".into()));
        assert_eq!(coverage(&prof, &approved, "libpq-dev", &n("libpq-dev")), Coverage::Ask, "not in the profile");
        assert_eq!(coverage(&off, &approved, "libpq-dev", &n("libpq-dev")), Coverage::Official);
        // The deny list wins at every level, on the canonical or native name.
        assert!(matches!(coverage(&off, &approved, "openssh-server", &n("openssh-server")), Coverage::Denied(_)));
        assert!(matches!(coverage(&off, &["sudo".into()], "sudo", &n("sudo")), Coverage::Denied(_)), "even if listed");
        assert!(matches!(coverage(&off, &approved, "x", &n("docker-ce")), Coverage::Denied("docker*")));
        // A profile level covers only the versions the owner accepted.
        let none = ServerPolicy { level: Level::Profiles, profiles: vec![], ..Default::default() };
        assert_eq!(coverage(&none, &approved, "libgtk-3-dev", &n("libgtk-3-dev")), Coverage::Ask);
    }

    #[test]
    fn widening_is_spotted() {
        let a = ServerPolicy { level: Level::Profiles, profiles: vec!["build-base@1".into()], max_install_mb: 1024 };
        assert!(a.widened_by(&ServerPolicy { level: Level::Official, ..a.clone() }));
        assert!(a.widened_by(&ServerPolicy { profiles: vec!["build-base@1".into(), "tauri-linux@1".into()], ..a.clone() }));
        assert!(a.widened_by(&ServerPolicy { max_install_mb: 4096, ..a.clone() }));
        assert!(!a.widened_by(&ServerPolicy { level: Level::List, profiles: vec![], max_install_mb: 512 }));
        assert!(!a.widened_by(&a));
    }

    #[test]
    fn deny_globs_match_prefixes_only() {
        assert_eq!(denied("linux-image-6.8.0-48-generic"), Some("linux-image-*"));
        assert_eq!(denied("libpam0g-dev"), Some("libpam*"));
        assert_eq!(denied("linux-libc-dev"), None, "kernel headers for userspace builds are fine");
        assert_eq!(denied("libssl-dev"), None);
        assert_eq!(denied("Docker-CE"), Some("docker*"), "case folds");
    }

    #[test]
    fn admin_args_are_checked_on_both_ends() {
        let ok = check_allow_args(&serde_json::json!({"packages": ["libgtk-3-dev", "pacman:protobuf"], "install": true, "request": "npr_ab12"})).unwrap();
        assert_eq!(ok.names().len(), 2);
        assert!(check_allow_args(&serde_json::json!({"package": "libssl-dev"})).is_ok(), "CE-24's form stays");
        assert!(check_allow_args(&serde_json::json!({"package": "x", "install": true})).is_err());
        assert!(check_allow_args(&serde_json::json!({"packages": ["x"]})).is_err(), "say install or add");
        assert!(check_allow_args(&serde_json::json!({"packages": ["-y"], "install": true})).is_err());
        assert!(check_allow_args(&serde_json::json!({"packages": ["x"], "install": true, "shell": "x"})).is_err(), "unknown keys refused");
        assert!(check_allow_args(&serde_json::json!({"packages": ["x"], "install": true, "request": "../x"})).is_err());
        let p = check_policy_args(&serde_json::json!({"server": "ster.k2.dev", "level": "profiles", "profiles": ["tauri-linux"]})).unwrap();
        assert_eq!(p.apply(&ServerPolicy::default()).profiles, vec!["tauri-linux@1".to_string()], "a bare profile pins the newest version");
        assert!(check_policy_args(&serde_json::json!({"server": "own"})).is_err());
        assert!(check_policy_args(&serde_json::json!({"server": "own", "level": "root"})).is_err());
        assert!(check_policy_args(&serde_json::json!({"server": "own", "profiles": ["nope"]})).is_err());
    }

    #[test]
    fn requests_are_checked() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(check_request(&v(&["libgtk-3-dev", "pacman:protobuf", "profile:tauri-linux"]), Some("tauri tests")).is_ok());
        assert!(check_request(&v(&[]), None).is_err());
        assert!(check_request(&v(&["-y"]), None).is_err());
        assert!(check_request(&v(&["profile:nope"]), None).is_err());
        assert!(check_request(&v(&["ok"]), Some(&"x".repeat(201))).is_err());
        assert!(check_request(&v(&["ok"]), Some("two\nlines")).is_err());
        let e = expand_names(&v(&["profile:build-base", "sys:libssl-dev", "zlib1g-dev"])).unwrap();
        assert!(e.contains(&"build-essential".to_string()) && e.contains(&"zlib1g-dev".to_string()));
        assert_eq!(e.iter().filter(|x| *x == "libssl-dev").count(), 1, "unique");
    }
}
