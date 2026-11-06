//! The package manager behind a [`Backend`], and the guarded install
//! (prd-compute-self-serve-environments-v1 §3.6, §5.2, §10).
//!
//! Every install K2 makes, from a job or from an owner's approval, goes
//! through [`guarded_install`] as root:
//!
//! 1. the manager's signature check must be on (`pkg_signing_off`);
//! 2. each name maps to this distro (`no_package_mapping`) and passes the
//!    deny list on its canonical and native names (`package_denied`);
//! 3. a virtual name resolves to its one provider (`package_ambiguous`);
//! 4. a stale apt or zypper index is refreshed (never `pacman -Sy`);
//! 5. a dry run: nothing removed or downgraded (`package_removes`), on
//!    pacman nothing installed is upgraded (a partial upgrade:
//!    `index_stale`), every package from the distro's own repos
//!    (`package_third_party`), every pulled-in name off the deny list, under
//!    the server's size cap (`package_too_big`);
//! 6. the install, with services kept from starting (`policy-rc.d` on
//!    Debian/Ubuntu) and a 5-minute wait for the dpkg lock
//!    (`package_busy`);
//! 7. a check that every resolved name is installed.
//!
//! [`SystemBackend`] runs the real manager by absolute path with a clean
//! env and `--` before the names. [`fake::FakeBackend`] is a scripted one
//! for tests; nothing on a real node constructs it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use k2_node_proto::host::OsRelease;
use k2_node_proto::pkgmap::{self, Mgr, Resolved, SysName};
use k2_node_proto::pkgpolicy::{codes, denied, DrySummary};

/// A refusal: a stable code and words an agent or owner can act on.
pub type Refusal = (&'static str, String);

/// apt and zypper refresh an index older than this before installing.
pub const INDEX_MAX_AGE_SECS: u64 = 24 * 3600;
/// How long apt waits for the dpkg lock (unattended upgrades).
pub const DPKG_LOCK_SECS: u64 = 300;
/// The file that keeps services from starting during an apt install.
pub const POLICY_RC_D: &str = "/usr/sbin/policy-rc.d";
const POLICY_RC_MARK: &str = "# k2-node: no services start during a K2 package install";

/// The distro the node runs, for the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCtx {
    pub mgr: Mgr,
    pub distro: String,
    pub version: String,
    /// `ID_LIKE` too, for origin checks (`Ubuntu` is Debian-like).
    pub like: Vec<String>,
}

impl HostCtx {
    pub fn from_os_release(mgr: Mgr, o: &OsRelease) -> Self {
        HostCtx { mgr, distro: o.id.clone(), version: o.version_id.clone().unwrap_or_else(|| "rolling".into()), like: o.id_like.clone() }
    }

    /// `name` (canonical, or `<manager>:<native>`) as this node installs it.
    pub fn natives(&self, name: &str) -> Option<Vec<String>> {
        SysName::parse(name)?.native(self.mgr, &self.distro, &self.version)
    }
}

/// This node's [`HostCtx`] (`None` without a supported manager).
pub fn host_ctx() -> Option<HostCtx> {
    let mgr = detect()?;
    let o = k2_node_proto::host::parse_os_release(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
    Some(HostCtx::from_os_release(mgr, &o))
}

/// The distro package manager, by absolute path (never PATH).
pub fn detect() -> Option<Mgr> {
    Mgr::ALL.into_iter().find(|m| Path::new(program(*m)).is_file())
}

pub fn program(m: Mgr) -> &'static str {
    match m {
        Mgr::Apt => "/usr/bin/apt-get",
        Mgr::Dnf => "/usr/bin/dnf",
        Mgr::Pacman => "/usr/bin/pacman",
        Mgr::Zypper => "/usr/bin/zypper",
    }
}

/// One package a dry run would install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryPkg {
    pub name: String,
    /// apt: the origin (`Ubuntu`, `Debian-Security`); pacman: the repo
    /// (`extra`); dnf/zypper: the repo id.
    pub origin: String,
    /// Already installed at another version (an upgrade).
    pub upgrade: bool,
    /// Installed size, KB, when known.
    pub size_kb: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DryRun {
    pub installs: Vec<DryPkg>,
    pub removes: Vec<String>,
    pub downgrades: Vec<String>,
}

/// What the package manager does for K2.
pub trait Backend: Send + Sync {
    fn mgr(&self) -> Mgr;
    /// A virtual name's provider (§5.2).
    fn resolve(&self, native: &str) -> Resolved;
    /// Which of `natives` are installed (as written).
    fn installed(&self, natives: &[String]) -> BTreeSet<String>;
    /// `Some(why)` when the signature check is off.
    fn signing_off(&self) -> Option<String>;
    /// Seconds since the index changed (`None` = unknown or self-refreshing).
    fn index_age_secs(&self) -> Option<u64>;
    /// apt `update` / zypper `refresh` (index only). pacman: never.
    fn refresh_index(&self) -> Result<(), String>;
    fn dry_run(&self, natives: &[String]) -> Result<DryRun, Refusal>;
    fn install(&self, natives: &[String]) -> Result<(), Refusal>;
    /// A distro wrapper the owner upgrades with (`omarchy-update`).
    fn wrapper(&self) -> Option<String> {
        None
    }
}

/// Is every native of `name` installed (virtual names through their
/// provider)? Read-only.
pub fn name_installed(b: &dyn Backend, h: &HostCtx, name: &str) -> bool {
    let Some(natives) = h.natives(name) else { return false };
    let resolved: Vec<String> = natives.iter().map(|n| match b.resolve(n) {
        Resolved::Provider(p) => p,
        _ => n.clone(),
    }).collect();
    let have = b.installed(&resolved);
    resolved.iter().all(|n| have.contains(n))
}

/// What the guarded install did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The native names asked of the manager.
    pub natives: Vec<String>,
    pub dry: DrySummary,
    /// Nothing to do: everything was installed already.
    pub nothing: bool,
}

/// The guards' inputs from the server's policy.
#[derive(Debug, Clone)]
pub struct Guards {
    pub max_install_mb: u64,
}

/// The distro's own repos, for the origin check.
fn origin_ok(h: &HostCtx, origin: &str) -> bool {
    let o = origin.trim();
    match h.mgr {
        Mgr::Apt => {
            let fam = |d: &str| h.distro == d || h.like.iter().any(|l| l == d);
            (fam("ubuntu") && matches!(o, "Ubuntu" | "Ubuntu-Security"))
                || (fam("debian") && matches!(o, "Debian" | "Debian-Security" | "Debian Backports"))
                || (h.distro == "debian" && o == "Debian")
        }
        Mgr::Pacman => matches!(o, "core" | "extra" | "multilib"),
        // Unchecked managers (no node yet): the distro's default repo ids.
        Mgr::Dnf => matches!(o, "fedora" | "updates" | "baseos" | "appstream" | "crb" | "@System" | "anaconda") || o.starts_with("fedora-") || o.starts_with("updates-"),
        Mgr::Zypper => o.starts_with("repo-") || o.contains("openSUSE") || o.starts_with("SLE"),
    }
}

/// The dry run summary for a card (no install), as the node user.
pub fn dry_summary(b: &dyn Backend, h: &HostCtx, names: &[String]) -> DrySummary {
    match plan(b, h, names) {
        Ok((natives, resolved, already)) if natives.is_empty() => DrySummary { already, resolved, ..Default::default() },
        Ok((natives, resolved, already)) => match b.dry_run(&natives) {
            Ok(d) => {
                let mut s = summary_of(&d, resolved, already);
                if let Err((c, m)) = check_dry(h, &d, &Guards { max_install_mb: u64::MAX }) {
                    s.refusal = Some(format!("{c}: {m}"));
                }
                s
            }
            Err((c, m)) => DrySummary { resolved, already, refusal: Some(format!("{c}: {m}")), ..Default::default() },
        },
        Err((c, m)) => DrySummary { refusal: Some(format!("{c}: {m}")), ..Default::default() },
    }
}

fn summary_of(d: &DryRun, resolved: Vec<String>, already: Vec<String>) -> DrySummary {
    let kb: Option<u64> = d.installs.iter().map(|p| p.size_kb).sum();
    DrySummary { installs: d.installs.iter().map(|p| p.name.clone()).collect(), size_mb: kb.map(|k| k.div_ceil(1024)), resolved, already, refusal: None }
}

/// Map, deny-check and resolve `names`; drop what's installed. Returns the
/// natives to install, the `a → b` resolutions and the names already there.
fn plan(b: &dyn Backend, h: &HostCtx, names: &[String]) -> Result<(Vec<String>, Vec<String>, Vec<String>), Refusal> {
    let mut natives: Vec<String> = Vec::new();
    let mut resolved = Vec::new();
    let mut already = Vec::new();
    let mut unmapped = Vec::new();
    for name in names {
        if let Some(d) = denied(name) {
            return Err((codes::PACKAGE_DENIED, format!("{name} is on K2's deny list ({d}): it changes how the machine boots, logs in or starts services. The machine's owner installs it by hand if they want it")));
        }
        let Some(ns) = h.natives(name) else {
            unmapped.push(name.clone());
            continue;
        };
        let mut all_there = true;
        let mut mine = Vec::new();
        for n in ns {
            if !pkgmap::valid_native(&n) {
                return Err((codes::NO_PACKAGE_MAPPING, format!("{n:?} isn't a package name")));
            }
            if let Some(d) = denied(&n) {
                return Err((codes::PACKAGE_DENIED, format!("{name} installs {n}, on K2's deny list ({d})")));
            }
            let target = match b.resolve(&n) {
                Resolved::Provider(p) => {
                    resolved.push(format!("{n} → {p} (virtual)"));
                    p
                }
                Resolved::Ambiguous(ps) => {
                    return Err((codes::PACKAGE_AMBIGUOUS, format!("{n} is a virtual package with several providers ({}); name one of them (sys:{}:<name>)", ps.join(", "), h.mgr.name())));
                }
                Resolved::Same | Resolved::Unknown => n,
            };
            if b.installed(std::slice::from_ref(&target)).is_empty() {
                all_there = false;
            }
            mine.push(target);
        }
        if all_there {
            already.push(name.clone());
        }
        for m in mine {
            if !natives.contains(&m) {
                natives.push(m);
            }
        }
    }
    if !unmapped.is_empty() {
        return Err((
            codes::NO_PACKAGE_MAPPING,
            format!(
                "no {} name for {} on this {} node: use a native name (sys:{}:<name>), pick a node of another distro (--distro), or ask K2 to add a map row",
                h.mgr.name(),
                unmapped.join(", "),
                h.distro,
                h.mgr.name()
            ),
        ));
    }
    let have = b.installed(&natives);
    natives.retain(|n| !have.contains(n));
    Ok((natives, resolved, already))
}

/// The dry run's guards (§3.6).
pub fn check_dry(h: &HostCtx, d: &DryRun, g: &Guards) -> Result<(), Refusal> {
    if !d.removes.is_empty() || !d.downgrades.is_empty() {
        let mut v: Vec<String> = d.removes.iter().map(|r| format!("removes {r}")).collect();
        v.extend(d.downgrades.iter().map(|r| format!("downgrades {r}")));
        return Err((codes::PACKAGE_REMOVES, format!("the install {}; K2 never removes or downgrades a package. The machine's owner installs it by hand if they want that", v.join(", "))));
    }
    if h.mgr == Mgr::Pacman {
        let up: Vec<&str> = d.installs.iter().filter(|p| p.upgrade).map(|p| p.name.as_str()).collect();
        if !up.is_empty() {
            return Err((
                codes::INDEX_STALE,
                format!("the install would upgrade {} alone, a partial upgrade on Arch: this node needs a full system upgrade first (its owner runs it)", up.join(", ")),
            ));
        }
    }
    let foreign: Vec<String> = d.installs.iter().filter(|p| !origin_ok(h, &p.origin)).map(|p| format!("{} ({})", p.name, if p.origin.is_empty() { "unknown origin" } else { &p.origin })).collect();
    if !foreign.is_empty() {
        return Err((codes::PACKAGE_THIRD_PARTY, format!("{} would come from outside {}'s own repos; K2 installs only from the distro's signed official repos", foreign.join(", "), h.distro)));
    }
    if let Some((n, d)) = d.installs.iter().find_map(|p| denied(&p.name).map(|d| (p.name.clone(), d))) {
        return Err((codes::PACKAGE_DENIED, format!("the install pulls in {n}, on K2's deny list ({d})")));
    }
    let kb: u64 = d.installs.iter().filter_map(|p| p.size_kb).sum();
    if kb.div_ceil(1024) > g.max_install_mb {
        return Err((codes::PACKAGE_TOO_BIG, format!("the install is {} MB with dependencies, over this server's {} MB cap per request", kb.div_ceil(1024), g.max_install_mb)));
    }
    Ok(())
}

/// The guarded install (root). See the module docs for the steps.
pub fn guarded_install(b: &dyn Backend, h: &HostCtx, names: &[String], g: &Guards, log: &mut dyn FnMut(&str)) -> Result<Report, Refusal> {
    if let Some(why) = b.signing_off() {
        return Err((codes::PKG_SIGNING_OFF, format!("{}'s signature check is off on this machine ({why}); K2 installs nothing until its owner turns it back on", h.mgr.name())));
    }
    let (natives, resolved, already) = plan(b, h, names)?;
    for r in &resolved {
        log(r);
    }
    if natives.is_empty() {
        return Ok(Report { natives, dry: DrySummary { resolved, already, ..Default::default() }, nothing: true });
    }
    if matches!(h.mgr, Mgr::Apt | Mgr::Zypper) && b.index_age_secs().is_none_or(|a| a > INDEX_MAX_AGE_SECS) {
        log(&format!("refreshing the {} package index (older than a day)", h.mgr.name()));
        b.refresh_index().map_err(|e| (codes::PACKAGE_INSTALL_FAILED, format!("refresh the package index: {e}")))?;
    }
    let d = b.dry_run(&natives)?;
    check_dry(h, &d, g)?;
    let dry = summary_of(&d, resolved, already);
    log(&format!("installing ({}): {} — {} packages with dependencies{}", h.mgr.name(), natives.join(" "), dry.installs.len(), dry.size_mb.map(|m| format!(", {m} MB")).unwrap_or_default()));
    b.install(&natives)?;
    let have = b.installed(&natives);
    let still: Vec<&String> = natives.iter().filter(|n| !have.contains(*n)).collect();
    if !still.is_empty() {
        return Err((codes::PACKAGE_INSTALL_FAILED, format!("{} still not installed after {} ran", still.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "), h.mgr.name())));
    }
    Ok(Report { natives, dry, nothing: false })
}

// ── parsers (pure; tested on captured output) ────────────────────────

/// `apt-get -s install …`: `Inst name [old] (version Origin:ver/suite [arch])`,
/// `Remv name [ver]`.
pub fn parse_apt_sim(text: &str) -> DryRun {
    let mut d = DryRun::default();
    for l in text.lines() {
        let mut w = l.split_whitespace();
        match w.next() {
            Some("Inst") => {
                let Some(name) = w.next() else { continue };
                let rest = &l[l.find(name).unwrap_or(0) + name.len()..];
                let upgrade = rest.trim_start().starts_with('[');
                let origin = rest
                    .find('(')
                    .and_then(|i| rest[i + 1..].split_once(')').map(|(inner, _)| inner))
                    .and_then(|inner| inner.split_whitespace().nth(1))
                    .and_then(|o| o.split(&[':', ','][..]).next())
                    .unwrap_or("")
                    .to_string();
                d.installs.push(DryPkg { name: name.to_string(), origin, upgrade, size_kb: None });
            }
            Some("Remv") | Some("Purg") => {
                if let Some(n) = w.next() {
                    d.removes.push(n.to_string());
                }
            }
            _ => {}
        }
    }
    d
}

/// apt-get prints the origin label with spaces (`Debian Backports`) as
/// `Debian Backports:…`; the first word plus the second when it is
/// `Backports`. Kept simple: [`parse_apt_sim`] reads the first word and
/// `Debian` passes either way.
///
/// `apt-cache show --no-all-versions …`: each stanza's `Package:` and
/// `Installed-Size:` (KB).
pub fn parse_apt_sizes(text: &str) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut cur: Option<String> = None;
    for l in text.lines() {
        if let Some(p) = l.strip_prefix("Package: ") {
            cur = Some(p.trim().to_string());
        } else if let (Some(s), Some(p)) = (l.strip_prefix("Installed-Size: "), &cur) {
            if let Ok(kb) = s.trim().parse::<u64>() {
                out.entry(p.clone()).or_insert(kb);
            }
        }
    }
    out
}

/// `pacman -Sp --print-format '%r %n %v %s'`: repo, name, version, bytes.
pub fn parse_pacman_print(text: &str) -> Vec<(String, String, u64)> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let (r, n, _v, s) = (w.next()?, w.next()?, w.next()?, w.next()?);
            Some((r.to_string(), n.to_string(), s.parse::<u64>().ok()?))
        })
        .collect()
}

/// The mirror refused a file the stale database names.
pub fn pacman_stale(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("failed retrieving file") || s.contains("404") || s.contains("could not be found") && s.contains("mirror")
}

/// A held package-manager lock.
pub fn lock_busy(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("could not get lock") || s.contains("unable to lock database") || s.contains("dpkg frontend lock") || s.contains("system management is locked")
}

/// `pacman.conf`'s `[options]` `SigLevel = Never`.
pub fn pacman_signing_off(conf: &str) -> bool {
    let mut in_opts = false;
    for l in conf.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            in_opts = t == "[options]";
            continue;
        }
        if in_opts && !t.starts_with('#') {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == "SigLevel" && v.split_whitespace().any(|x| x == "Never") {
                    return true;
                }
            }
        }
    }
    false
}

/// `apt-config dump`: `APT::Get::AllowUnauthenticated "1";` (or true).
pub fn apt_signing_off(dump: &str) -> bool {
    dump.lines().any(|l| {
        let t = l.trim();
        (t.starts_with("APT::Get::AllowUnauthenticated") || t.starts_with("Acquire::AllowInsecureRepositories"))
            && (t.contains("\"1\"") || t.to_ascii_lowercase().contains("\"true\""))
    })
}

/// dnf.conf / zypp.conf: `gpgcheck=0` / `gpgcheck = off`.
pub fn ini_gpgcheck_off(conf: &str) -> bool {
    conf.lines().any(|l| {
        let t = l.trim();
        !t.starts_with('#') && t.split_once('=').is_some_and(|(k, v)| k.trim() == "gpgcheck" && matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
    })
}

// ── the real manager ─────────────────────────────────────────────────

/// The real package manager. Read-only calls run as whoever calls them
/// (the node user, for status and dry runs); `install` and
/// `refresh_index` need root.
pub struct SystemBackend {
    pub mgr: Mgr,
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run(program: &str, args: &[&str], limit: Duration) -> Result<Out, String> {
    use std::io::Read;
    let mut child = std::process::Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t1 = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = so.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let t2 = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = se.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} ran over {}s", limit.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ok(Out { ok: status.success(), stdout: t1.join().unwrap_or_default(), stderr: t2.join().unwrap_or_default() })
}

fn tail(s: &str, n: usize) -> String {
    let v: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    v[v.len().saturating_sub(n)..].join(" | ")
}

fn newest_mtime_age(dir: &str, keep: impl Fn(&str) -> bool) -> Option<u64> {
    let rd = std::fs::read_dir(dir).ok()?;
    let newest = rd
        .flatten()
        .filter(|e| keep(&e.file_name().to_string_lossy()))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()?;
    Some(newest.elapsed().map(|d| d.as_secs()).unwrap_or(0))
}

/// Keeps services from starting while apt installs (Debian's
/// `policy-rc.d`), removed after; an owner's own file is left alone.
pub struct NoServices {
    ours: bool,
}

impl NoServices {
    pub fn start() -> Self {
        let p = Path::new(POLICY_RC_D);
        if p.exists() {
            return NoServices { ours: false };
        }
        let body = format!("#!/bin/sh\n{POLICY_RC_MARK}\nexit 101\n");
        let ours = crate::util::atomic_write(p, body.as_bytes(), 0o755).is_ok();
        NoServices { ours }
    }

    /// [`NoServices::clear_stale`] when running as root (a node runs as its
    /// own user and can't touch it; root's serve and executor do).
    pub fn clear_stale_if_root() {
        if unsafe { libc::geteuid() } == 0 {
            Self::clear_stale();
        }
    }

    /// At start-up: a file a crash left behind (it carries our mark) goes,
    /// so the owner's own service starts aren't blocked.
    pub fn clear_stale() {
        if std::fs::read_to_string(POLICY_RC_D).is_ok_and(|s| s.contains(POLICY_RC_MARK)) {
            let _ = std::fs::remove_file(POLICY_RC_D);
        }
    }
}

impl Drop for NoServices {
    fn drop(&mut self) {
        if self.ours {
            Self::clear_stale();
        }
    }
}

impl Backend for SystemBackend {
    fn mgr(&self) -> Mgr {
        self.mgr
    }

    fn resolve(&self, native: &str) -> Resolved {
        match self.mgr {
            Mgr::Apt => run("/usr/bin/apt-cache", &["showpkg", "--", native], Duration::from_secs(60)).map(|o| pkgmap::parse_apt_showpkg(&o.stdout)).unwrap_or(Resolved::Unknown),
            Mgr::Pacman => match run("/usr/bin/pacman", &["-Sp", "--print-format", "%n", "--", native], Duration::from_secs(60)) {
                Ok(o) if o.ok => {
                    let first = o.stdout.lines().last().unwrap_or("").trim().to_string();
                    if first.is_empty() || first == native { Resolved::Same } else { Resolved::Provider(first) }
                }
                _ => Resolved::Unknown,
            },
            Mgr::Dnf | Mgr::Zypper => Resolved::Same,
        }
    }

    fn installed(&self, natives: &[String]) -> BTreeSet<String> {
        if natives.is_empty() {
            return BTreeSet::new();
        }
        let mut args: Vec<&str> = Vec::new();
        let out = match self.mgr {
            Mgr::Apt => {
                args.extend(["-W", "-f=${Package} ${Status}\\n", "--"]);
                args.extend(natives.iter().map(String::as_str));
                run("/usr/bin/dpkg-query", &args, Duration::from_secs(60)).ok().map(|o| {
                    o.stdout
                        .lines()
                        .filter(|l| l.contains("install ok installed"))
                        .filter_map(|l| l.split_whitespace().next().map(|n| n.split(':').next().unwrap_or(n).to_string()))
                        .collect::<BTreeSet<_>>()
                })
            }
            Mgr::Pacman => {
                // -T prints what is NOT satisfied (provides count).
                args.extend(["-T", "--"]);
                args.extend(natives.iter().map(String::as_str));
                run("/usr/bin/pacman", &args, Duration::from_secs(60)).ok().map(|o| {
                    let missing: BTreeSet<&str> = o.stdout.lines().map(str::trim).collect();
                    natives.iter().filter(|n| !missing.contains(n.as_str())).cloned().collect()
                })
            }
            Mgr::Dnf | Mgr::Zypper => {
                args.extend(["-q", "--whatprovides", "--"]);
                args.extend(natives.iter().map(String::as_str));
                run("/usr/bin/rpm", &args, Duration::from_secs(60)).ok().map(|o| {
                    natives.iter().filter(|n| !o.stdout.contains(&format!("no package provides {n}"))).cloned().collect()
                })
            }
        };
        out.unwrap_or_default()
    }

    fn signing_off(&self) -> Option<String> {
        match self.mgr {
            Mgr::Apt => run("/usr/bin/apt-config", &["dump"], Duration::from_secs(30)).ok().filter(|o| apt_signing_off(&o.stdout)).map(|_| "APT AllowUnauthenticated".into()),
            Mgr::Pacman => std::fs::read_to_string("/etc/pacman.conf").ok().filter(|c| pacman_signing_off(c)).map(|_| "pacman.conf SigLevel = Never".into()),
            Mgr::Dnf => std::fs::read_to_string("/etc/dnf/dnf.conf").ok().filter(|c| ini_gpgcheck_off(c)).map(|_| "dnf.conf gpgcheck=0".into()),
            Mgr::Zypper => std::fs::read_to_string("/etc/zypp/zypp.conf").ok().filter(|c| ini_gpgcheck_off(c)).map(|_| "zypp.conf gpgcheck off".into()),
        }
    }

    fn index_age_secs(&self) -> Option<u64> {
        match self.mgr {
            Mgr::Apt => newest_mtime_age("/var/lib/apt/lists", |n| n.ends_with("Release") || n.ends_with("InRelease")),
            Mgr::Pacman => newest_mtime_age("/var/lib/pacman/sync", |n| n.ends_with(".db")),
            Mgr::Zypper => newest_mtime_age("/var/cache/zypp/raw", |_| true),
            Mgr::Dnf => None,
        }
    }

    fn refresh_index(&self) -> Result<(), String> {
        let o = match self.mgr {
            Mgr::Apt => run(program(Mgr::Apt), &["update", "-q"], Duration::from_secs(600))?,
            Mgr::Zypper => run(program(Mgr::Zypper), &["--non-interactive", "refresh"], Duration::from_secs(600))?,
            // Never `pacman -Sy` alone: syncing without upgrading is a
            // partial upgrade (§10, SE-A13). dnf refreshes by itself.
            Mgr::Pacman | Mgr::Dnf => return Ok(()),
        };
        if o.ok { Ok(()) } else { Err(tail(&o.stderr, 4)) }
    }

    fn dry_run(&self, natives: &[String]) -> Result<DryRun, Refusal> {
        let fail = |e: String| (codes::PACKAGE_INSTALL_FAILED, e);
        match self.mgr {
            Mgr::Apt => {
                let mut a = vec!["-s", "-q", "install", "--no-install-recommends", "--"];
                a.extend(natives.iter().map(String::as_str));
                let o = run(program(Mgr::Apt), &a, Duration::from_secs(120)).map_err(fail)?;
                if !o.ok {
                    return Err((codes::PACKAGE_INSTALL_FAILED, format!("apt-get can't install {}: {}", natives.join(" "), tail(&o.stderr, 4))));
                }
                let mut d = parse_apt_sim(&o.stdout);
                let names: Vec<&str> = d.installs.iter().map(|p| p.name.as_str()).collect();
                if !names.is_empty() {
                    let mut a = vec!["show", "--no-all-versions", "--"];
                    a.extend(names);
                    if let Ok(s) = run("/usr/bin/apt-cache", &a, Duration::from_secs(60)) {
                        let sizes = parse_apt_sizes(&s.stdout);
                        for p in &mut d.installs {
                            p.size_kb = sizes.get(&p.name).copied();
                        }
                    }
                }
                Ok(d)
            }
            Mgr::Pacman => {
                let mut a = vec!["-Sp", "--needed", "--print-format", "%r %n %v %s", "--"];
                a.extend(natives.iter().map(String::as_str));
                let o = run(program(Mgr::Pacman), &a, Duration::from_secs(120)).map_err(fail)?;
                if !o.ok {
                    return Err((codes::PACKAGE_INSTALL_FAILED, format!("pacman can't install {}: {}", natives.join(" "), tail(&o.stderr, 4))));
                }
                let rows = parse_pacman_print(&o.stdout);
                let names: Vec<String> = rows.iter().map(|r| r.1.clone()).collect();
                let have = self.installed(&names);
                Ok(DryRun {
                    installs: rows.into_iter().map(|(r, n, s)| DryPkg { upgrade: have.contains(&n), name: n, origin: r, size_kb: Some(s.div_ceil(1024)) }).collect(),
                    ..Default::default()
                })
            }
            Mgr::Dnf | Mgr::Zypper => {
                // Unchecked (no node yet): the transaction summary only.
                let mut a: Vec<&str> = if self.mgr == Mgr::Dnf { vec!["install", "--assumeno", "--"] } else { vec!["--non-interactive", "install", "--dry-run", "--"] };
                a.extend(natives.iter().map(String::as_str));
                let o = run(program(self.mgr), &a, Duration::from_secs(300)).map_err(fail)?;
                let text = format!("{}\n{}", o.stdout, o.stderr);
                let mut d = DryRun::default();
                if text.contains("Removing") || text.contains("to be REMOVED") {
                    d.removes.push("(see the manager's output)".into());
                }
                if text.contains("Downgrading") || text.contains("to be DOWNGRADED") {
                    d.downgrades.push("(see the manager's output)".into());
                }
                d.installs = natives.iter().map(|n| DryPkg { name: n.clone(), origin: if self.mgr == Mgr::Dnf { "fedora".into() } else { "repo-oss".into() }, upgrade: false, size_kb: None }).collect();
                Ok(d)
            }
        }
    }

    fn install(&self, natives: &[String]) -> Result<(), Refusal> {
        let lock = format!("DPkg::Lock::Timeout={DPKG_LOCK_SECS}");
        let mut a: Vec<&str> = match self.mgr {
            Mgr::Apt => vec!["-o", &lock, "install", "-y", "-q", "--no-install-recommends"],
            Mgr::Dnf => vec!["install", "-y", "-q"],
            Mgr::Pacman => vec!["-S", "--needed", "--noconfirm"],
            Mgr::Zypper => vec!["--non-interactive", "install"],
        };
        a.push("--");
        a.extend(natives.iter().map(String::as_str));
        let _guard = (self.mgr == Mgr::Apt).then(NoServices::start);
        let o = run(program(self.mgr), &a, Duration::from_secs(30 * 60)).map_err(|e| (codes::PACKAGE_INSTALL_FAILED, e))?;
        if o.ok {
            return Ok(());
        }
        if lock_busy(&o.stderr) {
            return Err((codes::PACKAGE_BUSY, format!("{} is busy (another install or unattended upgrades held its lock for {DPKG_LOCK_SECS}s); try again later", self.mgr.name())));
        }
        if self.mgr == Mgr::Pacman && pacman_stale(&o.stderr) {
            let w = self.wrapper().map(|w| format!(" (on this machine: {w})")).unwrap_or_default();
            return Err((codes::INDEX_STALE, format!("this Arch node's package database is out of date and its mirror no longer has those files: it needs a full system upgrade first, by its owner{w}")));
        }
        Err((codes::PACKAGE_INSTALL_FAILED, format!("{} couldn't install {}: {}", self.mgr.name(), natives.join(" "), tail(&o.stderr, 6))))
    }

    fn wrapper(&self) -> Option<String> {
        wrapper_command()
    }
}

/// Known distro wrappers that own the OS upgrade (§10): Omarchy's
/// `omarchy-update`.
pub fn wrapper_command() -> Option<String> {
    if Path::new("/usr/bin/omarchy-update").is_file() || Path::new("/usr/local/bin/omarchy-update").is_file() {
        return Some("omarchy-update".into());
    }
    // Omarchy installs its bin dir per user.
    let homes = std::fs::read_dir("/home").ok()?;
    homes.flatten().any(|h| h.path().join(".local/share/omarchy/bin/omarchy-update").is_file()).then(|| "omarchy-update".into())
}

/// The node's distro flavor from a wrapper (`omarchy`).
pub fn flavor() -> Option<String> {
    wrapper_command().map(|_| "omarchy".into())
}

/// The real backend for this machine.
pub fn system() -> Option<SystemBackend> {
    detect().map(|mgr| SystemBackend { mgr })
}

/// A scripted backend for tests (no package manager runs). Never built on
/// a real node: only a test hands one to an executor.
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, Default)]
    pub struct FakePkg {
        pub deps: Vec<String>,
        pub origin: String,
        pub size_kb: u64,
        /// Installing it removes these.
        pub removes: Vec<String>,
    }

    #[derive(Default)]
    pub struct State {
        pub available: BTreeMap<String, FakePkg>,
        /// virtual name → providers.
        pub virtuals: BTreeMap<String, Vec<String>>,
        pub installed: BTreeSet<String>,
        pub signing_off: bool,
        pub busy: bool,
        pub index_age: Option<u64>,
        pub refreshed: u32,
        /// Every `install` call, in order.
        pub installs: Vec<Vec<String>>,
    }

    pub struct FakeBackend {
        pub mgr: Mgr,
        pub state: Mutex<State>,
    }

    impl FakeBackend {
        pub fn new(mgr: Mgr) -> Self {
            FakeBackend { mgr, state: Mutex::new(State { index_age: Some(60), ..Default::default() }) }
        }

        /// An official package with `deps`.
        pub fn add(&self, name: &str, deps: &[&str], size_kb: u64) {
            let origin = match self.mgr {
                Mgr::Apt => "Ubuntu",
                Mgr::Pacman => "extra",
                Mgr::Dnf => "fedora",
                Mgr::Zypper => "repo-oss",
            };
            self.state.lock().unwrap().available.insert(name.into(), FakePkg { deps: deps.iter().map(|s| s.to_string()).collect(), origin: origin.into(), size_kb, removes: vec![] });
        }

        fn closure(st: &State, natives: &[String]) -> Result<Vec<String>, Refusal> {
            let mut out: Vec<String> = Vec::new();
            let mut todo: Vec<String> = natives.to_vec();
            while let Some(n) = todo.pop() {
                if out.contains(&n) || st.installed.contains(&n) {
                    continue;
                }
                let p = st.available.get(&n).ok_or_else(|| (codes::PACKAGE_INSTALL_FAILED, format!("unable to locate package {n}")))?;
                todo.extend(p.deps.iter().cloned());
                out.push(n);
            }
            Ok(out)
        }
    }

    impl Backend for FakeBackend {
        fn mgr(&self) -> Mgr {
            self.mgr
        }
        fn resolve(&self, native: &str) -> Resolved {
            let st = self.state.lock().unwrap();
            match st.virtuals.get(native) {
                Some(p) if p.len() == 1 => Resolved::Provider(p[0].clone()),
                Some(p) => Resolved::Ambiguous(p.clone()),
                None if st.available.contains_key(native) || st.installed.contains(native) => Resolved::Same,
                None => Resolved::Unknown,
            }
        }
        fn installed(&self, natives: &[String]) -> BTreeSet<String> {
            let st = self.state.lock().unwrap();
            natives.iter().filter(|n| st.installed.contains(*n)).cloned().collect()
        }
        fn signing_off(&self) -> Option<String> {
            self.state.lock().unwrap().signing_off.then(|| "fake".into())
        }
        fn index_age_secs(&self) -> Option<u64> {
            self.state.lock().unwrap().index_age
        }
        fn refresh_index(&self) -> Result<(), String> {
            let mut st = self.state.lock().unwrap();
            st.refreshed += 1;
            st.index_age = Some(0);
            Ok(())
        }
        fn dry_run(&self, natives: &[String]) -> Result<DryRun, Refusal> {
            let st = self.state.lock().unwrap();
            let all = Self::closure(&st, natives)?;
            let mut d = DryRun::default();
            for n in all {
                let p = &st.available[&n];
                d.removes.extend(p.removes.iter().cloned());
                d.installs.push(DryPkg { name: n.clone(), origin: p.origin.clone(), upgrade: false, size_kb: Some(p.size_kb) });
            }
            Ok(d)
        }
        fn install(&self, natives: &[String]) -> Result<(), Refusal> {
            let mut st = self.state.lock().unwrap();
            if st.busy {
                return Err((codes::PACKAGE_BUSY, "fake lock held".into()));
            }
            let all = Self::closure(&st, natives)?;
            st.installs.push(natives.to_vec());
            for n in all {
                st.installed.insert(n);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeBackend;
    use super::*;

    fn ubuntu() -> HostCtx {
        HostCtx { mgr: Mgr::Apt, distro: "ubuntu".into(), version: "24.04".into(), like: vec!["debian".into()] }
    }

    fn names(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_clean_install_runs_and_reports_the_dry_run() {
        let b = FakeBackend::new(Mgr::Apt);
        b.add("libgtk-3-dev", &["libglib2.0-dev", "libpango1.0-dev"], 4000);
        b.add("libglib2.0-dev", &[], 3000);
        b.add("libpango1.0-dev", &[], 2000);
        let mut log = Vec::new();
        let r = guarded_install(&b, &ubuntu(), &names(&["libgtk-3-dev"]), &Guards { max_install_mb: 2048 }, &mut |l| log.push(l.to_string())).unwrap();
        assert_eq!(r.natives, vec!["libgtk-3-dev"]);
        assert_eq!(r.dry.installs.len(), 3);
        assert_eq!(r.dry.size_mb, Some(9));
        assert!(b.state.lock().unwrap().installed.contains("libpango1.0-dev"));
        // Again: nothing to do.
        let again = guarded_install(&b, &ubuntu(), &names(&["libgtk-3-dev"]), &Guards { max_install_mb: 2048 }, &mut |_| {}).unwrap();
        assert!(again.nothing);
        assert_eq!(b.state.lock().unwrap().installs.len(), 1, "no second install");
    }

    #[test]
    fn the_dry_run_refuses_removals_third_party_and_size() {
        let g = Guards { max_install_mb: 2048 };
        // Removes a package.
        let b = FakeBackend::new(Mgr::Apt);
        b.add("libfoo-dev", &[], 10);
        b.state.lock().unwrap().available.get_mut("libfoo-dev").unwrap().removes = vec!["libbar1".into()];
        let e = guarded_install(&b, &ubuntu(), &names(&["libfoo-dev"]), &g, &mut |_| {}).unwrap_err();
        assert_eq!(e.0, codes::PACKAGE_REMOVES, "{}", e.1);
        assert!(b.state.lock().unwrap().installs.is_empty(), "nothing ran");
        // A dependency from a third-party repo.
        let b = FakeBackend::new(Mgr::Apt);
        b.add("libfoo-dev", &["libppa1"], 10);
        b.add("libppa1", &[], 10);
        b.state.lock().unwrap().available.get_mut("libppa1").unwrap().origin = "LP-PPA-acme".into();
        let e = guarded_install(&b, &ubuntu(), &names(&["libfoo-dev"]), &g, &mut |_| {}).unwrap_err();
        assert_eq!(e.0, codes::PACKAGE_THIRD_PARTY, "{}", e.1);
        assert!(e.1.contains("libppa1 (LP-PPA-acme)"), "{}", e.1);
        assert!(b.state.lock().unwrap().installs.is_empty());
        // Over the size cap.
        let b = FakeBackend::new(Mgr::Apt);
        b.add("libhuge-dev", &[], 3 * 1024 * 1024);
        let e = guarded_install(&b, &ubuntu(), &names(&["libhuge-dev"]), &g, &mut |_| {}).unwrap_err();
        assert_eq!(e.0, codes::PACKAGE_TOO_BIG, "{}", e.1);
        assert!(e.1.contains("3072 MB") && e.1.contains("2048 MB"), "{}", e.1);
        assert!(b.state.lock().unwrap().installs.is_empty());
    }

    #[test]
    fn denied_names_and_denied_dependencies_never_install() {
        let g = Guards { max_install_mb: 2048 };
        let b = FakeBackend::new(Mgr::Apt);
        b.add("openssh-server", &[], 10);
        assert_eq!(guarded_install(&b, &ubuntu(), &names(&["openssh-server"]), &g, &mut |_| {}).unwrap_err().0, codes::PACKAGE_DENIED);
        b.add("libsneaky-dev", &["sudo"], 10);
        b.add("sudo", &[], 10);
        let e = guarded_install(&b, &ubuntu(), &names(&["libsneaky-dev"]), &g, &mut |_| {}).unwrap_err();
        assert_eq!(e.0, codes::PACKAGE_DENIED, "{}", e.1);
        assert!(b.state.lock().unwrap().installs.is_empty());
    }

    #[test]
    fn signing_off_installs_nothing() {
        let b = FakeBackend::new(Mgr::Apt);
        b.add("libfoo-dev", &[], 10);
        b.state.lock().unwrap().signing_off = true;
        assert_eq!(guarded_install(&b, &ubuntu(), &names(&["libfoo-dev"]), &Guards { max_install_mb: 9 }, &mut |_| {}).unwrap_err().0, codes::PKG_SIGNING_OFF);
    }

    #[test]
    fn virtual_names_resolve_to_one_provider() {
        let b = FakeBackend::new(Mgr::Apt);
        b.add("libfreetype-dev", &[], 10);
        b.state.lock().unwrap().virtuals.insert("libfreetype6-dev".into(), vec!["libfreetype-dev".into()]);
        let mut log = Vec::new();
        let r = guarded_install(&b, &ubuntu(), &names(&["libfreetype6-dev"]), &Guards { max_install_mb: 9 }, &mut |l| log.push(l.to_string())).unwrap();
        assert_eq!(r.natives, vec!["libfreetype-dev"], "installs and checks the provider");
        assert!(log.iter().any(|l| l == "libfreetype6-dev → libfreetype-dev (virtual)"), "{log:?}");
        assert!(name_installed(&b, &ubuntu(), "libfreetype6-dev"), "the virtual name reads as installed");
        b.state.lock().unwrap().virtuals.insert("mail-transport-agent".into(), vec!["postfix".into(), "exim4".into()]);
        assert_eq!(guarded_install(&b, &ubuntu(), &names(&["mail-transport-agent"]), &Guards { max_install_mb: 9 }, &mut |_| {}).unwrap_err().0, codes::PACKAGE_AMBIGUOUS);
    }

    #[test]
    fn a_stale_apt_index_is_refreshed_first_pacman_never_syncs() {
        let b = FakeBackend::new(Mgr::Apt);
        b.add("zlib1g-dev", &[], 10);
        b.state.lock().unwrap().index_age = Some(3 * 24 * 3600);
        guarded_install(&b, &ubuntu(), &names(&["zlib1g-dev"]), &Guards { max_install_mb: 9 }, &mut |_| {}).unwrap();
        assert_eq!(b.state.lock().unwrap().refreshed, 1);
        let arch = HostCtx { mgr: Mgr::Pacman, distro: "arch".into(), version: "rolling".into(), like: vec![] };
        let p = FakeBackend::new(Mgr::Pacman);
        p.add("zlib", &[], 10);
        p.state.lock().unwrap().index_age = Some(30 * 24 * 3600);
        let r = guarded_install(&p, &arch, &names(&["zlib1g-dev"]), &Guards { max_install_mb: 9 }, &mut |_| {}).unwrap();
        assert_eq!(r.natives, vec!["zlib"], "the Debian name maps to Arch's");
        assert_eq!(p.state.lock().unwrap().refreshed, 0, "never pacman -Sy");
    }

    #[test]
    fn pacman_refuses_a_partial_upgrade() {
        let arch = HostCtx { mgr: Mgr::Pacman, distro: "arch".into(), version: "rolling".into(), like: vec![] };
        let d = DryRun { installs: vec![DryPkg { name: "glib2".into(), origin: "core".into(), upgrade: true, size_kb: Some(1) }], ..Default::default() };
        assert_eq!(check_dry(&arch, &d, &Guards { max_install_mb: 9 }).unwrap_err().0, codes::INDEX_STALE);
        // apt upgrades of dependencies are normal.
        let d = DryRun { installs: vec![DryPkg { name: "libglib2.0-0t64".into(), origin: "Ubuntu".into(), upgrade: true, size_kb: Some(1) }], ..Default::default() };
        assert!(check_dry(&ubuntu(), &d, &Guards { max_install_mb: 9 }).is_ok());
    }

    #[test]
    fn unmapped_names_say_what_to_do() {
        let arch = HostCtx { mgr: Mgr::Pacman, distro: "arch".into(), version: "rolling".into(), like: vec![] };
        let b = FakeBackend::new(Mgr::Pacman);
        let e = guarded_install(&b, &arch, &names(&["libfoo-dev"]), &Guards { max_install_mb: 9 }, &mut |_| {}).unwrap_err();
        assert_eq!(e.0, codes::NO_PACKAGE_MAPPING);
        assert!(e.1.contains("sys:pacman:<name>") && e.1.contains("--distro"), "{}", e.1);
    }

    #[test]
    fn apt_simulation_parses() {
        let out = "NOTE: This is only a simulation!\n\
Inst libglib2.0-dev (2.80.0-6ubuntu3.1 Ubuntu:24.04/noble-updates [amd64])\n\
Inst libssl3t64 [3.0.13-0ubuntu3.3] (3.0.13-0ubuntu3.4 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [amd64])\n\
Inst docker-ce (5:27.3.1-1~ubuntu.24.04~noble Docker CE:noble [amd64])\n\
Remv libold1 [1.0]\n\
Conf libglib2.0-dev (2.80.0-6ubuntu3.1 Ubuntu:24.04/noble-updates [amd64])\n";
        let d = parse_apt_sim(out);
        assert_eq!(d.installs.len(), 3);
        assert_eq!(d.installs[0], DryPkg { name: "libglib2.0-dev".into(), origin: "Ubuntu".into(), upgrade: false, size_kb: None });
        assert!(d.installs[1].upgrade);
        assert_eq!(d.installs[1].origin, "Ubuntu");
        assert_eq!(d.installs[2].origin, "Docker", "a third-party origin shows");
        assert_eq!(d.removes, vec!["libold1"]);
        assert!(!origin_ok(&ubuntu(), "Docker"));
        assert!(origin_ok(&ubuntu(), "Ubuntu"));
        let deb = HostCtx { mgr: Mgr::Apt, distro: "debian".into(), version: "12".into(), like: vec![] };
        assert!(origin_ok(&deb, "Debian-Security"));
        assert!(!origin_ok(&deb, "Ubuntu"));
        let sizes = parse_apt_sizes("Package: a\nVersion: 1\nInstalled-Size: 120\n\nPackage: b\nInstalled-Size: 7\n");
        assert_eq!(sizes.get("a"), Some(&120));
        assert_eq!(sizes.get("b"), Some(&7));
    }

    #[test]
    fn pacman_and_config_parsers() {
        assert_eq!(parse_pacman_print("extra zlib 1:1.3.1-2 98304\ncore glibc 2.40-1 10485760\n"), vec![("extra".into(), "zlib".into(), 98304), ("core".into(), "glibc".into(), 10485760)]);
        assert!(pacman_stale("error: failed retrieving file 'zlib-1.3.1-2-x86_64.pkg.tar.zst' from mirror : The requested URL returned error: 404"));
        assert!(lock_busy("E: Could not get lock /var/lib/dpkg/lock-frontend. It is held by process 123 (unattended-upgr)"));
        assert!(lock_busy("error: failed to init transaction (unable to lock database)"));
        assert!(pacman_signing_off("[options]\nSigLevel = Never\n[core]\nInclude = x\n"));
        assert!(!pacman_signing_off("[options]\nSigLevel = Required DatabaseOptional\n[custom]\nSigLevel = Never\n"), "only the global one");
        assert!(apt_signing_off("APT::Get::AllowUnauthenticated \"true\";\n"));
        assert!(!apt_signing_off("APT::Get::Assume-Yes \"true\";\n"));
        assert!(ini_gpgcheck_off("[main]\ngpgcheck=0\n"));
        assert!(!ini_gpgcheck_off("[main]\ngpgcheck=1\n# gpgcheck=0\n"));
    }
}
