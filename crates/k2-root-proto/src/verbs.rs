//! The verb catalog (`prd-k2-root-helper-v1` §7): one exact argv per verb,
//! its class (`open` = safe whoever asks; `daemon` = caller must be the
//! daemon's main process, §6), its stdin cap, and whether it takes the
//! global lock.
//!
//! Rules (§5.3): argv is an exact match. Tokens are enum names, never
//! paths. Ids are validated by one rule per kind. Meta characters, empty
//! tokens and extra tokens are refused. Refusals are static strings: caller
//! bytes are never echoed.

/// Exit codes (§5.2).
pub mod exit {
    pub const OK: i32 = 0;
    /// Refused argv.
    pub const ARGV: i32 = 2;
    /// Refused state (unit running, no space, missing view, missing file).
    pub const STATE: i32 = 3;
    /// Signature or self-update refusal.
    pub const SIGNATURE: i32 = 4;
    /// I/O failure.
    pub const IO: i32 = 5;
    /// The caller is not the K2 daemon's main process.
    pub const NOT_DAEMON: i32 = 77;
}

/// Stdin caps.
pub const CAP_TEMPLATE: usize = 64 * 1024;
pub const CAP_SQL: usize = 64 * 1024;
pub const CAP_INSTALL_BIN: usize = 256 * 1024 * 1024;
pub const CAP_SELF_UPDATE: usize = 8 * 1024 * 1024;
/// `apps install-engine`: `<u32 BE sig len><sig><ELF>` of k2-app-engine.
pub const CAP_ENGINE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Read-only, converges to a state K2 would set anyway, only restarts a
    /// service, or is gated by a signature or sha pin.
    Open,
    /// Loses data, disables a service, sets a credential, hands over a
    /// secret, or undoes anti-rollback.
    Daemon,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Daemon => "daemon",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Legacy {
    Mail,
    Pg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MkdirTarget {
    Config,
    Data,
    Log,
    DropinDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChownTarget {
    Config,
    Data,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveTarget {
    Unit,
    DropinDir,
    Bin,
    Data,
    Log,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailSystemctl {
    DaemonReload,
    EnableNow,
    Restart,
    Stop,
    DisableNow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgOp {
    Start,
    Restart,
    Enable,
    Stop,
    Disable,
    IsActive,
}

/// `postgresql`, `postgresql.service` or `postgresql@<n>-main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgUnit(String);

impl PgUnit {
    pub fn parse(s: &str) -> Option<Self> {
        let ok = s == "postgresql"
            || s == "postgresql.service"
            || s.strip_prefix("postgresql@")
                .and_then(|r| r.strip_suffix("-main"))
                .is_some_and(|n| {
                    !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit())
                });
        ok.then(|| Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `^[a-z0-9_]{1,63}$`, not `template0`/`template1` (RH10: `k2 db create
/// --name` makes names without the `ws_` prefix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbName(String);

impl DbName {
    pub fn parse(s: &str) -> Option<Self> {
        let ok = !s.is_empty()
            && s.len() <= 63
            && s != "template0"
            && s != "template1"
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        ok.then(|| Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PsqlFlags {
    pub on_error_stop: bool,
    pub tuples_only: bool,
    pub unaligned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    // helper
    HelperVersion,
    HelperSelfUpdate,
    HelperSelfRollback,
    HelperLegacyRetire(Legacy),
    HelperAuditTail,
    // mail
    MailEnsureUser,
    MailMkdir(MkdirTarget),
    MailChown(ChownTarget),
    MailRenderUnit,
    MailRenderDropin,
    MailInstallBin,
    MailSystemctl(MailSystemctl),
    MailRemove(RemoveTarget),
    MailUsage,
    MailSnapshotData,
    MailRestoreData,
    // pg
    PgSystemctl { op: PgOp, now: bool, unit: PgUnit },
    PgDaemonReload,
    PgInstallRamFence,
    PgPsql { db: DbName, flags: PsqlFlags },
    /// `pg hba-app add|remove <app-id>` (protocol 6, AR17/AR38): one canned
    /// `host app_<id> app_<id> 127.0.0.1/32 scram-sha-256` line in a
    /// K2-owned block at the top of each cluster's `pg_hba.conf`, then a
    /// reload. The App's own database and role only; nothing else.
    PgHbaApp { add: bool, id: AppId },
    // keep
    KeepStatus,
    KeepKekInit,
    KeepKekGet,
    KeepKekStage,
    KeepKekCommit,
    KeepKekAbort,
    KeepGenGet,
    KeepGenBump,
    // daemon (protocol 5, runtime stage H)
    /// Write the daemon unit's hand-off drop-in (`KillMode=process`,
    /// `NotifyAccess=main`, `ExecReload=` SIGUSR2) and `daemon-reload`.
    DaemonHandoffDropinInstall,
    /// Remove it again and `daemon-reload`.
    DaemonHandoffDropinRemove,
    // apps (protocol 6, App runtime box prep; see `crate::apps`)
    AppsStatus,
    AppsEnsureUser,
    /// `apps ensure-process-user`: the `process` exception's own system
    /// user `k2appx` (never `k2app`, which owns every container's user
    /// namespace).
    AppsEnsureProcessUser,
    AppsSubid,
    AppsLinger(bool),
    AppsDelegate(bool),
    AppsIoCost(bool),
    AppsReservePorts(bool),
    AppsNftApply { gw: PortRange, px: PortRange, db: PortRange },
    AppsNftRemove,
    /// `apps nft app <id> <gw-port> [<box-port>…]`: the App's own gateway
    /// port, matched on its cgroups (slice 3), plus its owner-approved box
    /// services' host loopback ports (0.46.0, at most
    /// [`crate::apps::MAX_BOX_SERVICES`], each [`crate::apps::box_port_ok`]).
    /// The list replaces the App's previous one (none = no box services).
    /// Re-run after each App start (nft resolves a cgroup path to an inode
    /// when the rule loads).
    AppsNftApp { id: AppId, gw: u16, box_ports: Vec<u16> },
    AppsNftAppRemove(AppId),
    /// Install (or update) `k2-app-engine` from a signed payload on stdin,
    /// its user unit for `k2app` and the socket dir (slice 3, S11).
    AppsInstallEngine,
    /// `apps logs <id> <deployment> <lines>` (daemon): the last lines of a
    /// guarded App's output from the journal. The engine sends it there
    /// (rate limited per unit, W §5.3), but k2app is a system user, so
    /// journald keeps its lines in the system journal, which only root
    /// (or the systemd-journal group, which k2app never gets) reads.
    AppsLogs { id: AppId, deployment: String, lines: u32 },
    AppsPackagesInstall,
    AppsPackagesInstallYouki,
    AppsRuntime(OciRuntime),
    AppsProcessUnit(bool),
    /// `apps apparmor on|off`: load (or unload) the canned `k2-app`
    /// AppArmor profile, the engine's marker and the process template's
    /// `AppArmorProfile=` drop-in. A no-op where SELinux enforces or no
    /// MAC is on (the status says which).
    AppsApparmor(bool),
    AppsProcessStart(AppId),
    AppsProcessStop(AppId),
    /// `apps process purge <id>` (daemon): a stopped process App's state
    /// (`/var/lib/k2appx/<id>`) and its runtime caps drop-in.
    AppsProcessPurge(AppId),
    /// `apps process caps <id> <mem MiB> <rbps.wbps.riops.wiops>
    /// [<device>…]` (daemon): the per-instance runtime drop-in with the
    /// App's memory and disk ceilings and its device grants (enum tokens,
    /// at most [`crate::apps::MAX_DEVICES`]). `-` = no ceiling for a knob.
    /// `process start` refuses until it exists. The one verb allowed more
    /// than 8 tokens (6 + the devices).
    AppsProcessCaps { id: AppId, mem_mib: u64, io: IoMax, devices: Vec<String> },
    /// `Some(MemoryMin MiB)` = write; `None` = `off`.
    AppsCoresSlice(Option<u64>),
    /// `Some((MemoryMax MiB, TasksMax))` = write; `None` = `off`.
    AppsBudget(Option<(u64, u64)>),
    AppsUnprep,
}

use crate::apps::{bounded, AppId, IoMax, OciRuntime, PortRange, BUDGET_MEM_MIB, BUDGET_TASKS, CORES_MIN_MIB};

const REFUSED: &str = "refused arguments";

fn on_off(t: &str) -> Option<bool> {
    match t {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

fn on_off_name(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// Bytes no token may contain. Tokens are enum names and ids only.
fn token_ok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 64
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@' | b'='))
}

fn mkdir_target(t: &str) -> Option<MkdirTarget> {
    Some(match t {
        "config" => MkdirTarget::Config,
        "data" => MkdirTarget::Data,
        "log" => MkdirTarget::Log,
        "dropin-dir" => MkdirTarget::DropinDir,
        _ => return None,
    })
}

fn chown_target(t: &str) -> Option<ChownTarget> {
    Some(match t {
        "config" => ChownTarget::Config,
        "data" => ChownTarget::Data,
        "log" => ChownTarget::Log,
        _ => return None,
    })
}

fn remove_target(t: &str) -> Option<RemoveTarget> {
    Some(match t {
        "unit" => RemoveTarget::Unit,
        "dropin-dir" => RemoveTarget::DropinDir,
        "bin" => RemoveTarget::Bin,
        "data" => RemoveTarget::Data,
        "log" => RemoveTarget::Log,
        "config" => RemoveTarget::Config,
        _ => return None,
    })
}

fn pg_op(t: &str) -> Option<PgOp> {
    Some(match t {
        "start" => PgOp::Start,
        "restart" => PgOp::Restart,
        "enable" => PgOp::Enable,
        "stop" => PgOp::Stop,
        "disable" => PgOp::Disable,
        "is-active" => PgOp::IsActive,
        _ => return None,
    })
}

fn parse_psql(rest: &[&str]) -> Option<Verb> {
    let mut db: Option<DbName> = None;
    let mut flags = PsqlFlags::default();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "-d" => {
                if db.is_some() {
                    return None;
                }
                db = Some(DbName::parse(rest.get(i + 1)?)?);
                i += 2;
            }
            "-v" => {
                if flags.on_error_stop || *rest.get(i + 1)? != "ON_ERROR_STOP=1" {
                    return None;
                }
                flags.on_error_stop = true;
                i += 2;
            }
            "-t" if !flags.tuples_only => {
                flags.tuples_only = true;
                i += 1;
            }
            "-A" if !flags.unaligned => {
                flags.unaligned = true;
                i += 1;
            }
            "-tA" if !flags.tuples_only && !flags.unaligned => {
                flags.tuples_only = true;
                flags.unaligned = true;
                i += 1;
            }
            _ => return None,
        }
    }
    Some(Verb::PgPsql { db: db?, flags })
}

impl Verb {
    /// Exact argv or a static refusal. Nothing is stripped.
    pub fn parse(args: &[&str]) -> Result<Verb, &'static str> {
        if args.is_empty() {
            return Err("empty argv");
        }
        // 8 tokens, except `apps process caps` (6 + its device grants).
        let max = if args.starts_with(&["apps", "process", "caps"]) { 6 + crate::apps::MAX_DEVICES } else { 8 };
        if args.len() > max || !args.iter().all(|a| token_ok(a)) {
            return Err(REFUSED);
        }
        let v = match args {
            ["helper", "version"] => Verb::HelperVersion,
            ["helper", "self-update"] => Verb::HelperSelfUpdate,
            ["helper", "self-rollback"] => Verb::HelperSelfRollback,
            ["helper", "legacy-retire", "mail"] => Verb::HelperLegacyRetire(Legacy::Mail),
            ["helper", "legacy-retire", "pg"] => Verb::HelperLegacyRetire(Legacy::Pg),
            ["helper", "audit-tail"] => Verb::HelperAuditTail,

            ["mail", "ensure-user"] => Verb::MailEnsureUser,
            ["mail", "mkdir", t] => Verb::MailMkdir(mkdir_target(t).ok_or(REFUSED)?),
            ["mail", "chown", t] => Verb::MailChown(chown_target(t).ok_or(REFUSED)?),
            ["mail", "render-unit"] => Verb::MailRenderUnit,
            ["mail", "render-dropin"] => Verb::MailRenderDropin,
            ["mail", "install-bin"] => Verb::MailInstallBin,
            ["mail", "systemctl", "daemon-reload"] => {
                Verb::MailSystemctl(MailSystemctl::DaemonReload)
            }
            ["mail", "systemctl", "enable-now"] => Verb::MailSystemctl(MailSystemctl::EnableNow),
            ["mail", "systemctl", "restart"] => Verb::MailSystemctl(MailSystemctl::Restart),
            ["mail", "systemctl", "stop"] => Verb::MailSystemctl(MailSystemctl::Stop),
            ["mail", "systemctl", "disable-now"] => Verb::MailSystemctl(MailSystemctl::DisableNow),
            ["mail", "remove", t] => Verb::MailRemove(remove_target(t).ok_or(REFUSED)?),
            ["mail", "usage"] => Verb::MailUsage,
            ["mail", "snapshot-data"] => Verb::MailSnapshotData,
            ["mail", "restore-data"] => Verb::MailRestoreData,

            ["pg", "systemctl", "daemon-reload"] => Verb::PgDaemonReload,
            ["pg", "systemctl", op, unit] => {
                let op = pg_op(op).ok_or(REFUSED)?;
                Verb::PgSystemctl {
                    op,
                    now: false,
                    unit: PgUnit::parse(unit).ok_or(REFUSED)?,
                }
            }
            ["pg", "systemctl", op, "--now", unit] => {
                let op = pg_op(op).ok_or(REFUSED)?;
                if !matches!(op, PgOp::Enable | PgOp::Disable) {
                    return Err(REFUSED);
                }
                Verb::PgSystemctl {
                    op,
                    now: true,
                    unit: PgUnit::parse(unit).ok_or(REFUSED)?,
                }
            }
            ["pg", "install-ram-fence"] => Verb::PgInstallRamFence,
            ["pg", "psql", rest @ ..] => parse_psql(rest).ok_or(REFUSED)?,
            ["pg", "hba-app", op, id] => Verb::PgHbaApp {
                add: match *op {
                    "add" => true,
                    "remove" => false,
                    _ => return Err(REFUSED),
                },
                id: AppId::parse(id).ok_or(REFUSED)?,
            },

            ["keep", "status"] => Verb::KeepStatus,
            ["keep", "kek", "init"] => Verb::KeepKekInit,
            ["keep", "kek", "get"] => Verb::KeepKekGet,
            ["keep", "kek", "stage"] => Verb::KeepKekStage,
            ["keep", "kek", "commit"] => Verb::KeepKekCommit,
            ["keep", "kek", "abort"] => Verb::KeepKekAbort,
            ["keep", "gen", "get"] => Verb::KeepGenGet,
            ["keep", "gen", "bump"] => Verb::KeepGenBump,

            ["daemon", "handoff-dropin", "install"] => Verb::DaemonHandoffDropinInstall,
            ["daemon", "handoff-dropin", "remove"] => Verb::DaemonHandoffDropinRemove,

            ["apps", "status"] => Verb::AppsStatus,
            ["apps", "ensure-user"] => Verb::AppsEnsureUser,
            ["apps", "ensure-process-user"] => Verb::AppsEnsureProcessUser,
            ["apps", "subid"] => Verb::AppsSubid,
            ["apps", "linger", t] => Verb::AppsLinger(on_off(t).ok_or(REFUSED)?),
            ["apps", "delegate", t] => Verb::AppsDelegate(on_off(t).ok_or(REFUSED)?),
            ["apps", "io-cost", t] => Verb::AppsIoCost(on_off(t).ok_or(REFUSED)?),
            ["apps", "reserve-ports", t] => Verb::AppsReservePorts(on_off(t).ok_or(REFUSED)?),
            ["apps", "nft", "apply", gw, px, db] => Verb::AppsNftApply {
                gw: PortRange::parse(gw).ok_or(REFUSED)?,
                px: PortRange::parse(px).ok_or(REFUSED)?,
                db: PortRange::parse(db).ok_or(REFUSED)?,
            },
            ["apps", "nft", "remove"] => Verb::AppsNftRemove,
            ["apps", "nft", "app", id, port, rest @ ..] => {
                let gw = PortRange::parse(port).filter(|r| r.lo == r.hi).ok_or(REFUSED)?.lo;
                if !(crate::apps::GATEWAY_PORTS.lo..=crate::apps::GATEWAY_PORTS.hi).contains(&gw) {
                    return Err(REFUSED);
                }
                if rest.len() > crate::apps::MAX_BOX_SERVICES {
                    return Err(REFUSED);
                }
                let mut box_ports = Vec::with_capacity(rest.len());
                for t in rest {
                    // Canonical single ports only, each off the deny-list, no repeats.
                    let p = PortRange::parse(t).filter(|r| r.lo == r.hi && r.token() == *t).ok_or(REFUSED)?.lo;
                    if !crate::apps::box_port_ok(p) || box_ports.contains(&p) {
                        return Err(REFUSED);
                    }
                    box_ports.push(p);
                }
                Verb::AppsNftApp { id: AppId::parse(id).ok_or(REFUSED)?, gw, box_ports }
            }
            ["apps", "nft", "app-remove", id] => Verb::AppsNftAppRemove(AppId::parse(id).ok_or(REFUSED)?),
            ["apps", "install-engine"] => Verb::AppsInstallEngine,
            ["apps", "logs", id, dep, n] => {
                if !crate::engine::deployment_ok(dep) {
                    return Err(REFUSED);
                }
                Verb::AppsLogs {
                    id: AppId::parse(id).ok_or(REFUSED)?,
                    deployment: dep.to_string(),
                    lines: bounded(n, (1, 5000)).ok_or(REFUSED)? as u32,
                }
            }
            ["apps", "packages", "install"] => Verb::AppsPackagesInstall,
            ["apps", "packages", "install-youki"] => Verb::AppsPackagesInstallYouki,
            ["apps", "runtime", r] => Verb::AppsRuntime(OciRuntime::parse(r).ok_or(REFUSED)?),
            ["apps", "process-unit", t] => Verb::AppsProcessUnit(on_off(t).ok_or(REFUSED)?),
            ["apps", "apparmor", t] => Verb::AppsApparmor(on_off(t).ok_or(REFUSED)?),
            ["apps", "process", "start", id] => {
                Verb::AppsProcessStart(AppId::parse(id).ok_or(REFUSED)?)
            }
            ["apps", "process", "stop", id] => Verb::AppsProcessStop(AppId::parse(id).ok_or(REFUSED)?),
            ["apps", "process", "purge", id] => Verb::AppsProcessPurge(AppId::parse(id).ok_or(REFUSED)?),
            ["apps", "process", "caps", id, mem, io, devs @ ..] => {
                if devs.len() > crate::apps::MAX_DEVICES {
                    return Err(REFUSED);
                }
                let mut devices: Vec<String> = Vec::with_capacity(devs.len());
                for d in devs.iter() {
                    if crate::apps::device(d).is_none() || devices.iter().any(|x| x == d) {
                        return Err(REFUSED);
                    }
                    devices.push(d.to_string());
                }
                Verb::AppsProcessCaps {
                    id: AppId::parse(id).ok_or(REFUSED)?,
                    mem_mib: bounded(mem, crate::apps::PROCESS_MEM_MIB).ok_or(REFUSED)?,
                    io: IoMax::from_token(io).ok_or(REFUSED)?,
                    devices,
                }
            }
            ["apps", "cores-slice", "off"] => Verb::AppsCoresSlice(None),
            ["apps", "cores-slice", n] => {
                Verb::AppsCoresSlice(Some(bounded(n, CORES_MIN_MIB).ok_or(REFUSED)?))
            }
            ["apps", "budget", "off"] => Verb::AppsBudget(None),
            ["apps", "budget", m, t] => Verb::AppsBudget(Some((
                bounded(m, BUDGET_MEM_MIB).ok_or(REFUSED)?,
                bounded(t, BUDGET_TASKS).ok_or(REFUSED)?,
            ))),
            ["apps", "unprep"] => Verb::AppsUnprep,
            _ => return Err(REFUSED),
        };
        Ok(v)
    }

    /// The canonical argv (`parse(argv())` round-trips).
    pub fn argv(&self) -> Vec<String> {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match self {
            Verb::HelperVersion => s(&["helper", "version"]),
            Verb::HelperSelfUpdate => s(&["helper", "self-update"]),
            Verb::HelperSelfRollback => s(&["helper", "self-rollback"]),
            Verb::HelperLegacyRetire(Legacy::Mail) => s(&["helper", "legacy-retire", "mail"]),
            Verb::HelperLegacyRetire(Legacy::Pg) => s(&["helper", "legacy-retire", "pg"]),
            Verb::HelperAuditTail => s(&["helper", "audit-tail"]),
            Verb::MailEnsureUser => s(&["mail", "ensure-user"]),
            Verb::MailMkdir(t) => s(&[
                "mail",
                "mkdir",
                match t {
                    MkdirTarget::Config => "config",
                    MkdirTarget::Data => "data",
                    MkdirTarget::Log => "log",
                    MkdirTarget::DropinDir => "dropin-dir",
                },
            ]),
            Verb::MailChown(t) => s(&[
                "mail",
                "chown",
                match t {
                    ChownTarget::Config => "config",
                    ChownTarget::Data => "data",
                    ChownTarget::Log => "log",
                },
            ]),
            Verb::MailRenderUnit => s(&["mail", "render-unit"]),
            Verb::MailRenderDropin => s(&["mail", "render-dropin"]),
            Verb::MailInstallBin => s(&["mail", "install-bin"]),
            Verb::MailSystemctl(m) => s(&[
                "mail",
                "systemctl",
                match m {
                    MailSystemctl::DaemonReload => "daemon-reload",
                    MailSystemctl::EnableNow => "enable-now",
                    MailSystemctl::Restart => "restart",
                    MailSystemctl::Stop => "stop",
                    MailSystemctl::DisableNow => "disable-now",
                },
            ]),
            Verb::MailRemove(t) => s(&[
                "mail",
                "remove",
                match t {
                    RemoveTarget::Unit => "unit",
                    RemoveTarget::DropinDir => "dropin-dir",
                    RemoveTarget::Bin => "bin",
                    RemoveTarget::Data => "data",
                    RemoveTarget::Log => "log",
                    RemoveTarget::Config => "config",
                },
            ]),
            Verb::MailUsage => s(&["mail", "usage"]),
            Verb::MailSnapshotData => s(&["mail", "snapshot-data"]),
            Verb::MailRestoreData => s(&["mail", "restore-data"]),
            Verb::PgDaemonReload => s(&["pg", "systemctl", "daemon-reload"]),
            Verb::PgSystemctl { op, now, unit } => {
                let mut v = s(&["pg", "systemctl", pg_op_name(*op)]);
                if *now {
                    v.push("--now".into());
                }
                v.push(unit.as_str().into());
                v
            }
            Verb::PgInstallRamFence => s(&["pg", "install-ram-fence"]),
            Verb::PgPsql { db, flags } => {
                let mut v = s(&["pg", "psql", "-d", db.as_str()]);
                if flags.on_error_stop {
                    v.push("-v".into());
                    v.push("ON_ERROR_STOP=1".into());
                }
                match (flags.tuples_only, flags.unaligned) {
                    (true, true) => v.push("-tA".into()),
                    (true, false) => v.push("-t".into()),
                    (false, true) => v.push("-A".into()),
                    (false, false) => {}
                }
                v
            }
            Verb::PgHbaApp { add, id } => s(&[
                "pg",
                "hba-app",
                if *add { "add" } else { "remove" },
                id.as_str(),
            ]),
            Verb::KeepStatus => s(&["keep", "status"]),
            Verb::KeepKekInit => s(&["keep", "kek", "init"]),
            Verb::KeepKekGet => s(&["keep", "kek", "get"]),
            Verb::KeepKekStage => s(&["keep", "kek", "stage"]),
            Verb::KeepKekCommit => s(&["keep", "kek", "commit"]),
            Verb::KeepKekAbort => s(&["keep", "kek", "abort"]),
            Verb::KeepGenGet => s(&["keep", "gen", "get"]),
            Verb::KeepGenBump => s(&["keep", "gen", "bump"]),
            Verb::DaemonHandoffDropinInstall => s(&["daemon", "handoff-dropin", "install"]),
            Verb::DaemonHandoffDropinRemove => s(&["daemon", "handoff-dropin", "remove"]),
            Verb::AppsStatus => s(&["apps", "status"]),
            Verb::AppsEnsureUser => s(&["apps", "ensure-user"]),
            Verb::AppsEnsureProcessUser => s(&["apps", "ensure-process-user"]),
            Verb::AppsSubid => s(&["apps", "subid"]),
            Verb::AppsLinger(on) => s(&["apps", "linger", on_off_name(*on)]),
            Verb::AppsDelegate(on) => s(&["apps", "delegate", on_off_name(*on)]),
            Verb::AppsIoCost(on) => s(&["apps", "io-cost", on_off_name(*on)]),
            Verb::AppsReservePorts(on) => s(&["apps", "reserve-ports", on_off_name(*on)]),
            Verb::AppsNftApply { gw, px, db } => {
                s(&["apps", "nft", "apply", &gw.token(), &px.token(), &db.token()])
            }
            Verb::AppsNftRemove => s(&["apps", "nft", "remove"]),
            Verb::AppsNftApp { id, gw, box_ports } => {
                let mut v = vec!["apps".to_string(), "nft".into(), "app".into(), id.as_str().into(), gw.to_string()];
                v.extend(box_ports.iter().map(u16::to_string));
                v
            }
            Verb::AppsNftAppRemove(id) => s(&["apps", "nft", "app-remove", id.as_str()]),
            Verb::AppsInstallEngine => s(&["apps", "install-engine"]),
            Verb::AppsLogs { id, deployment, lines } => {
                s(&["apps", "logs", id.as_str(), deployment, &lines.to_string()])
            }
            Verb::AppsPackagesInstall => s(&["apps", "packages", "install"]),
            Verb::AppsPackagesInstallYouki => s(&["apps", "packages", "install-youki"]),
            Verb::AppsRuntime(r) => s(&["apps", "runtime", r.as_str()]),
            Verb::AppsProcessUnit(on) => s(&["apps", "process-unit", on_off_name(*on)]),
            Verb::AppsApparmor(on) => s(&["apps", "apparmor", on_off_name(*on)]),
            Verb::AppsProcessStart(id) => s(&["apps", "process", "start", id.as_str()]),
            Verb::AppsProcessStop(id) => s(&["apps", "process", "stop", id.as_str()]),
            Verb::AppsProcessPurge(id) => s(&["apps", "process", "purge", id.as_str()]),
            Verb::AppsProcessCaps { id, mem_mib, io, devices } => {
                let mem = mem_mib.to_string();
                let t = io.token();
                let mut v = vec!["apps", "process", "caps", id.as_str(), mem.as_str(), t.as_str()];
                v.extend(devices.iter().map(String::as_str));
                s(&v)
            }
            Verb::AppsCoresSlice(None) => s(&["apps", "cores-slice", "off"]),
            Verb::AppsCoresSlice(Some(n)) => s(&["apps", "cores-slice", &n.to_string()]),
            Verb::AppsBudget(None) => s(&["apps", "budget", "off"]),
            Verb::AppsBudget(Some((m, t))) => {
                s(&["apps", "budget", &m.to_string(), &t.to_string()])
            }
            Verb::AppsUnprep => s(&["apps", "unprep"]),
        }
    }

    /// `open` or `daemon` (§5.4, §7).
    pub fn class(&self) -> Class {
        use Class::*;
        match self {
            Verb::HelperVersion | Verb::HelperSelfUpdate | Verb::HelperAuditTail => Open,
            Verb::HelperSelfRollback | Verb::HelperLegacyRetire(_) => Daemon,

            Verb::MailEnsureUser
            | Verb::MailMkdir(_)
            | Verb::MailChown(_)
            | Verb::MailRenderDropin
            | Verb::MailInstallBin
            | Verb::MailUsage => Open,
            Verb::MailSystemctl(
                MailSystemctl::DaemonReload | MailSystemctl::EnableNow | MailSystemctl::Restart,
            ) => Open,
            Verb::MailSystemctl(MailSystemctl::Stop | MailSystemctl::DisableNow) => Daemon,
            Verb::MailRenderUnit
            | Verb::MailRemove(_)
            | Verb::MailSnapshotData
            | Verb::MailRestoreData => Daemon,

            Verb::PgDaemonReload | Verb::PgInstallRamFence => Open,
            Verb::PgSystemctl { op, .. } => match op {
                PgOp::Start | PgOp::Restart | PgOp::Enable | PgOp::IsActive => Open,
                PgOp::Stop | PgOp::Disable => Daemon,
            },
            Verb::PgPsql { .. } => Daemon,
            // Changes who may log in to Postgres: the daemon's call only.
            Verb::PgHbaApp { .. } => Daemon,

            Verb::KeepStatus | Verb::KeepGenGet => Open,
            Verb::KeepKekInit
            | Verb::KeepKekGet
            | Verb::KeepKekStage
            | Verb::KeepKekCommit
            | Verb::KeepKekAbort
            | Verb::KeepGenBump => Daemon,

            // Changes how systemd stops the daemon's own unit (what
            // survives a stop) and which process may name its main pid:
            // only the daemon itself may ask.
            Verb::DaemonHandoffDropinInstall | Verb::DaemonHandoffDropinRemove => Daemon,

            // apps: reading, tightening, or converging to the state box
            // prep sets anyway is open. Loosening, stopping, changing limits
            // or the OCI runtime away from the default is daemon-only, and
            // so is starting an App: only the daemon decides what runs as
            // k2app.
            Verb::AppsStatus
            | Verb::AppsEnsureUser
            | Verb::AppsEnsureProcessUser
            | Verb::AppsSubid
            | Verb::AppsLinger(true)
            | Verb::AppsDelegate(true)
            | Verb::AppsIoCost(true)
            | Verb::AppsReservePorts(true)
            | Verb::AppsPackagesInstall
            | Verb::AppsPackagesInstallYouki
            | Verb::AppsRuntime(OciRuntime::Crun)
            | Verb::AppsProcessUnit(true)
            | Verb::AppsApparmor(true) => Open,
            Verb::AppsLinger(false)
            | Verb::AppsDelegate(false)
            | Verb::AppsIoCost(false)
            | Verb::AppsReservePorts(false)
            | Verb::AppsNftRemove
            // Which App reaches which gateway port, and which engine binary
            // runs as k2app: the daemon's call (signature-gated as well).
            | Verb::AppsNftApp { .. }
            | Verb::AppsNftAppRemove(_)
            | Verb::AppsInstallEngine
            // An App's output is its owner's: the daemon decides who sees it.
            | Verb::AppsLogs { .. }
            | Verb::AppsRuntime(OciRuntime::Youki)
            | Verb::AppsProcessUnit(false)
            // Unloading the MAC profile loosens every App.
            | Verb::AppsApparmor(false)
            | Verb::AppsProcessStart(_)
            | Verb::AppsProcessStop(_)
            // Limits and device grants: the daemon decides (owner-approved).
            | Verb::AppsProcessCaps { .. }
            // Deletes an App's data.
            | Verb::AppsProcessPurge(_)
            | Verb::AppsCoresSlice(_)
            | Verb::AppsBudget(_)
            | Verb::AppsUnprep => Daemon,
            // Re-applying the canonical ranges is a tightening anyone may
            // ask for (boot, drift). Other ranges could open more loopback
            // ports to Apps: the daemon's call.
            Verb::AppsNftApply { gw, px, db } => {
                if (*gw, *px, *db)
                    == (crate::apps::GATEWAY_PORTS, crate::apps::PROXY_PORTS, crate::apps::DB_PORTS)
                {
                    Open
                } else {
                    Daemon
                }
            }
        }
    }

    /// `Some(cap)` when the verb reads stdin (up to `cap` bytes); `None`
    /// when it takes none and stdin is never read.
    pub fn stdin_cap(&self) -> Option<usize> {
        match self {
            Verb::HelperSelfUpdate => Some(CAP_SELF_UPDATE),
            Verb::MailRenderUnit => Some(CAP_TEMPLATE),
            Verb::MailInstallBin => Some(CAP_INSTALL_BIN),
            Verb::AppsInstallEngine => Some(CAP_ENGINE),
            Verb::PgPsql { .. } => Some(CAP_SQL),
            _ => None,
        }
    }

    /// Mutating verbs take the global lock. Reads and `psql` don't (SQL
    /// concurrency is Postgres's job, and a 60 s statement must not starve
    /// a restart).
    pub fn takes_lock(&self) -> bool {
        !matches!(
            self,
            Verb::HelperVersion
                | Verb::HelperAuditTail
                | Verb::MailUsage
                | Verb::PgPsql { .. }
                | Verb::PgSystemctl {
                    op: PgOp::IsActive,
                    ..
                }
                | Verb::KeepStatus
                | Verb::KeepKekGet
                | Verb::KeepGenGet
                | Verb::AppsStatus
                | Verb::AppsLogs { .. }
        )
    }

    /// `helper version` is the one verb that runs without euid 0 (§5.3).
    pub fn needs_root(&self) -> bool {
        !matches!(self, Verb::HelperVersion)
    }

    /// Namespace + verb for the audit line (never ids from argv beyond the
    /// validated enum names).
    pub fn audit_name(&self) -> String {
        let v = self.argv();
        match self {
            // DB names are validated ids; keep them out of the audit verb
            // field anyway (the db goes in its own field).
            Verb::PgPsql { .. } => "pg psql".to_string(),
            _ => v.join(" "),
        }
    }

    /// The namespace word.
    pub fn namespace(&self) -> &'static str {
        match self.argv()[0].as_str() {
            "helper" => "helper",
            "mail" => "mail",
            "pg" => "pg",
            "daemon" => "daemon",
            "apps" => "apps",
            _ => "keep",
        }
    }
}

fn pg_op_name(op: PgOp) -> &'static str {
    match op {
        PgOp::Start => "start",
        PgOp::Restart => "restart",
        PgOp::Enable => "enable",
        PgOp::Stop => "stop",
        PgOp::Disable => "disable",
        PgOp::IsActive => "is-active",
    }
}

/// Every verb the catalog has, one representative each (tests, docs).
pub fn catalog() -> Vec<Verb> {
    let unit = PgUnit::parse("postgresql").expect("unit");
    let db = DbName::parse("postgres").expect("db");
    let mut v = vec![
        Verb::HelperVersion,
        Verb::HelperSelfUpdate,
        Verb::HelperSelfRollback,
        Verb::HelperLegacyRetire(Legacy::Mail),
        Verb::HelperLegacyRetire(Legacy::Pg),
        Verb::HelperAuditTail,
        Verb::MailEnsureUser,
        Verb::MailRenderUnit,
        Verb::MailRenderDropin,
        Verb::MailInstallBin,
        Verb::MailUsage,
        Verb::MailSnapshotData,
        Verb::MailRestoreData,
        Verb::PgDaemonReload,
        Verb::PgInstallRamFence,
        Verb::PgPsql {
            db,
            flags: PsqlFlags {
                on_error_stop: true,
                tuples_only: true,
                unaligned: true,
            },
        },
        Verb::PgHbaApp {
            add: true,
            id: AppId::parse("jobs").expect("id"),
        },
        Verb::PgHbaApp {
            add: false,
            id: AppId::parse("nsi-skin").expect("id"),
        },
        Verb::KeepStatus,
        Verb::KeepKekInit,
        Verb::KeepKekGet,
        Verb::KeepKekStage,
        Verb::KeepKekCommit,
        Verb::KeepKekAbort,
        Verb::KeepGenGet,
        Verb::KeepGenBump,
        Verb::DaemonHandoffDropinInstall,
        Verb::DaemonHandoffDropinRemove,
        Verb::AppsStatus,
        Verb::AppsEnsureUser,
        Verb::AppsEnsureProcessUser,
        Verb::AppsSubid,
        Verb::AppsNftApply {
            gw: crate::apps::GATEWAY_PORTS,
            px: crate::apps::PROXY_PORTS,
            db: crate::apps::DB_PORTS,
        },
        Verb::AppsNftRemove,
        Verb::AppsNftApp {
            id: AppId::parse("nsi-skin").expect("id"),
            gw: 38_601,
            box_ports: vec![],
        },
        Verb::AppsNftApp {
            id: AppId::parse("hearth").expect("id"),
            gw: 38_602,
            box_ports: vec![8470, 7880],
        },
        Verb::AppsNftAppRemove(AppId::parse("nsi-skin").expect("id")),
        Verb::AppsInstallEngine,
        Verb::AppsLogs {
            id: AppId::parse("jobs").expect("id"),
            deployment: "d-0123456789ab".into(),
            lines: 200,
        },
        Verb::AppsPackagesInstall,
        Verb::AppsPackagesInstallYouki,
        Verb::AppsRuntime(OciRuntime::Crun),
        Verb::AppsRuntime(OciRuntime::Youki),
        Verb::AppsProcessStart(AppId::parse("jobs").expect("id")),
        Verb::AppsProcessStop(AppId::parse("jobs").expect("id")),
        Verb::AppsProcessPurge(AppId::parse("jobs").expect("id")),
        Verb::AppsProcessCaps {
            id: AppId::parse("jobs").expect("id"),
            mem_mib: 512,
            io: crate::apps::IoMax::k2_default(),
            devices: vec![],
        },
        Verb::AppsProcessCaps {
            id: AppId::parse("gpu-job").expect("id"),
            mem_mib: 8192,
            io: crate::apps::IoMax::unlimited(),
            devices: vec!["dri-render128".into(), "nvidia0".into(), "nvidiactl".into()],
        },
        Verb::AppsCoresSlice(Some(768)),
        Verb::AppsCoresSlice(None),
        Verb::AppsBudget(Some((4096, 4096))),
        Verb::AppsBudget(None),
        Verb::AppsUnprep,
    ];
    for on in [true, false] {
        v.push(Verb::AppsLinger(on));
        v.push(Verb::AppsDelegate(on));
        v.push(Verb::AppsIoCost(on));
        v.push(Verb::AppsReservePorts(on));
        v.push(Verb::AppsProcessUnit(on));
        v.push(Verb::AppsApparmor(on));
    }
    for t in [
        MkdirTarget::Config,
        MkdirTarget::Data,
        MkdirTarget::Log,
        MkdirTarget::DropinDir,
    ] {
        v.push(Verb::MailMkdir(t));
    }
    for t in [ChownTarget::Config, ChownTarget::Data, ChownTarget::Log] {
        v.push(Verb::MailChown(t));
    }
    for t in [
        RemoveTarget::Unit,
        RemoveTarget::DropinDir,
        RemoveTarget::Bin,
        RemoveTarget::Data,
        RemoveTarget::Log,
        RemoveTarget::Config,
    ] {
        v.push(Verb::MailRemove(t));
    }
    for m in [
        MailSystemctl::DaemonReload,
        MailSystemctl::EnableNow,
        MailSystemctl::Restart,
        MailSystemctl::Stop,
        MailSystemctl::DisableNow,
    ] {
        v.push(Verb::MailSystemctl(m));
    }
    for op in [
        PgOp::Start,
        PgOp::Restart,
        PgOp::Enable,
        PgOp::Stop,
        PgOp::Disable,
        PgOp::IsActive,
    ] {
        v.push(Verb::PgSystemctl {
            op,
            now: false,
            unit: unit.clone(),
        });
    }
    for op in [PgOp::Enable, PgOp::Disable] {
        v.push(Verb::PgSystemctl {
            op,
            now: true,
            unit: unit.clone(),
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_verb_round_trips() {
        for v in catalog() {
            let argv = v.argv();
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            assert_eq!(Verb::parse(&refs).as_ref(), Ok(&v), "{argv:?}");
        }
    }

    /// The class table, written out once more by hand (§7). A verb moving
    /// class must change this test on purpose.
    #[test]
    fn class_table() {
        let daemon: &[&[&str]] = &[
            &["helper", "self-rollback"],
            &["helper", "legacy-retire", "mail"],
            &["helper", "legacy-retire", "pg"],
            &["mail", "render-unit"],
            &["mail", "systemctl", "stop"],
            &["mail", "systemctl", "disable-now"],
            &["mail", "remove", "unit"],
            &["mail", "remove", "dropin-dir"],
            &["mail", "remove", "bin"],
            &["mail", "remove", "data"],
            &["mail", "remove", "log"],
            &["mail", "remove", "config"],
            &["mail", "snapshot-data"],
            &["mail", "restore-data"],
            &["pg", "systemctl", "stop", "postgresql"],
            &["pg", "systemctl", "disable", "--now", "postgresql"],
            &["pg", "systemctl", "disable", "postgresql"],
            &["pg", "psql", "-d", "postgres"],
            &["pg", "hba-app", "add", "jobs"],
            &["pg", "hba-app", "remove", "jobs"],
            &["keep", "kek", "init"],
            &["keep", "kek", "get"],
            &["keep", "kek", "stage"],
            &["keep", "kek", "commit"],
            &["keep", "kek", "abort"],
            &["keep", "gen", "bump"],
            &["daemon", "handoff-dropin", "install"],
            &["daemon", "handoff-dropin", "remove"],
        ];
        let open: &[&[&str]] = &[
            &["helper", "version"],
            &["helper", "self-update"],
            &["helper", "audit-tail"],
            &["mail", "ensure-user"],
            &["mail", "mkdir", "data"],
            &["mail", "chown", "data"],
            &["mail", "render-dropin"],
            &["mail", "install-bin"],
            &["mail", "systemctl", "daemon-reload"],
            &["mail", "systemctl", "enable-now"],
            &["mail", "systemctl", "restart"],
            &["mail", "usage"],
            &["pg", "systemctl", "daemon-reload"],
            &["pg", "systemctl", "start", "postgresql"],
            &["pg", "systemctl", "restart", "postgresql@16-main"],
            &["pg", "systemctl", "enable", "--now", "postgresql.service"],
            &["pg", "systemctl", "is-active", "postgresql"],
            &["pg", "install-ram-fence"],
            &["keep", "status"],
            &["keep", "gen", "get"],
        ];
        for a in daemon {
            assert_eq!(Verb::parse(a).unwrap().class(), Class::Daemon, "{a:?}");
        }
        for a in open {
            assert_eq!(Verb::parse(a).unwrap().class(), Class::Open, "{a:?}");
        }
    }

    #[test]
    fn attack_table_is_refused() {
        let rejects: &[&[&str]] = &[
            &[],
            &["helper"],
            &["helper", "version", "--json"],
            &["helper", "self-update", "/tmp/x"],
            &["helper", "legacy-retire", "all"],
            &["helper", "legacy-retire", "/etc/sudoers"],
            // Paths are never tokens.
            &["mail", "mkdir", "/etc/stalwart"],
            &["mail", "mkdir", "../etc"],
            &["mail", "chown", "/var/lib/stalwart"],
            &["mail", "chown", "dropin-dir"],
            &["mail", "remove", "/"],
            &["mail", "remove", "data", "--force"],
            &["mail", "remove", "data/.."],
            // Meta characters.
            &["mail", "mkdir", "data;id"],
            &["mail", "mkdir", "data id"],
            &["mail", "mkdir", "data\n"],
            &["mail", "mkdir", "data|x"],
            &["mail", "mkdir", "data$(id)"],
            &["mail", "mkdir", ""],
            // Old (protocol 2) argv shapes are not root-helper argv.
            &["write", "/etc/systemd/system/stalwart.service"],
            &["mail", "write", "unit"],
            &["mail", "systemctl", "enable", "--now", "stalwart"],
            &["mail", "systemctl", "start"],
            &["mail", "systemctl", "kill"],
            &["mail", "systemctl", "restart", "k2-daemon"],
            &["mail", "snapshot-data", "/tmp"],
            &["mail", "restore-data", "/etc"],
            // pg units and flags.
            &["pg", "systemctl", "start", "k2-daemon"],
            &["pg", "systemctl", "start", "postgresql@x-main"],
            &["pg", "systemctl", "start", "postgresql@123-main"],
            &["pg", "systemctl", "start", "postgresql@16-main;id"],
            &["pg", "systemctl", "start", "--now", "postgresql"],
            &["pg", "systemctl", "restart", "--now", "postgresql"],
            &["pg", "systemctl", "enable", "postgresql", "--now"],
            &["pg", "systemctl", "mask", "postgresql"],
            &["pg", "psql"],
            &["pg", "psql", "-d"],
            &["pg", "psql", "-c", "select 1"],
            &["pg", "psql", "-d", "postgres", "-c", "x"],
            &["pg", "psql", "-d", "postgres", "-f", "/etc/shadow"],
            &["pg", "psql", "-d", "postgres", "-o", "/tmp/x"],
            &["pg", "psql", "-d", "postgres", "-h", "evil"],
            &["pg", "psql", "-d", "postgres", "-U", "root"],
            &["pg", "psql", "-d", "postgres", "-L", "/tmp/log"],
            &["pg", "psql", "-d", "postgres", "-v", "X=1"],
            &[
                "pg",
                "psql",
                "-d",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
                "-v",
                "ON_ERROR_STOP=1",
            ],
            &["pg", "psql", "-d", "postgres", "-d", "ws_x"],
            &["pg", "psql", "-d", "template0"],
            &["pg", "psql", "-d", "template1"],
            &["pg", "psql", "-d", "Postgres"],
            &["pg", "psql", "-d", "a'b"],
            &["pg", "psql", "-d", "a b"],
            &["pg", "psql", "-d", "a;b"],
            &["pg", "psql", "-d", "postgres", "-tA", "-t"],
            // hba-app: an App id, never a db, role, address or method.
            &["pg", "hba-app"],
            &["pg", "hba-app", "add"],
            &["pg", "hba-app", "add", "app_jobs"],
            &["pg", "hba-app", "add", "jobs", "0.0.0.0/0"],
            &["pg", "hba-app", "add", "jobs", "trust"],
            &["pg", "hba-app", "add", "Jobs"],
            &["pg", "hba-app", "add", "a--b"],
            &["pg", "hba-app", "add", "jobs\nhost"],
            &["pg", "hba-app", "replace", "jobs"],
            &["pg", "hba-app", "add", "../pg_hba.conf"],
            // keep never takes a user, a path or a value.
            &["keep", "kek", "get", "k2"],
            &["keep", "kek", "rewrap"],
            &["keep", "gen", "set", "9"],
            &["keep", "status", "root"],
            // Unknown namespaces.
            &["cell", "prepare"],
            &["daemon", "update"],
            // The daemon namespace takes no unit, path or setting.
            &["daemon", "handoff-dropin"],
            &["daemon", "handoff-dropin", "install", "k2-daemon.service"],
            &["daemon", "handoff-dropin", "/etc/systemd/system/x.service.d"],
            &["daemon", "handoff-dropin", "KillMode=none"],
            &["daemon", "systemctl", "restart"],
        ];
        for r in rejects {
            assert!(Verb::parse(r).is_err(), "expected refusal: {r:?}");
        }
        let too_long = "a".repeat(65);
        assert!(Verb::parse(&["pg", "psql", "-d", &too_long]).is_err());
    }

    /// The `apps` namespace (protocol 6): classes written out by hand.
    #[test]
    fn apps_class_table() {
        let open: &[&[&str]] = &[
            &["apps", "status"],
            &["apps", "ensure-user"],
            &["apps", "ensure-process-user"],
            &["apps", "subid"],
            &["apps", "linger", "on"],
            &["apps", "delegate", "on"],
            &["apps", "io-cost", "on"],
            &["apps", "reserve-ports", "on"],
            &["apps", "nft", "apply", "38600-38899", "38900-38999", "5432"],
            &["apps", "packages", "install"],
            &["apps", "packages", "install-youki"],
            &["apps", "runtime", "crun"],
            &["apps", "process-unit", "on"],
            &["apps", "apparmor", "on"],
        ];
        let daemon: &[&[&str]] = &[
            &["apps", "linger", "off"],
            &["apps", "delegate", "off"],
            &["apps", "io-cost", "off"],
            &["apps", "reserve-ports", "off"],
            &["apps", "nft", "remove"],
            &["apps", "runtime", "youki"],
            &["apps", "process-unit", "off"],
            &["apps", "apparmor", "off"],
            &["apps", "process", "start", "jobs"],
            &["apps", "process", "stop", "jobs"],
            &["apps", "process", "purge", "jobs"],
            &["apps", "process", "caps", "jobs", "512", "-.41943040.-.2000"],
            &["apps", "process", "caps", "jobs", "512", "-.-.-.-", "dri-render128", "dri-card0", "nvidia0", "nvidiactl", "nvidia-uvm", "nvidia-uvm-tools", "nvidia-modeset", "nvidia1"],
            &["apps", "cores-slice", "768"],
            &["apps", "cores-slice", "off"],
            &["apps", "budget", "4096", "4096"],
            &["apps", "budget", "off"],
            &["apps", "unprep"],
            &["apps", "nft", "app", "jobs", "38600"],
            &["apps", "nft", "app", "hearth", "38602", "8470", "7880"],
            &["apps", "nft", "app-remove", "jobs"],
            &["apps", "install-engine"],
            &["apps", "logs", "jobs", "d-0123456789ab", "200"],
            // Non-canonical ranges could widen what Apps reach.
            &["apps", "nft", "apply", "1024-2047", "38900-38999", "5432"],
            &["apps", "nft", "apply", "38600-38899", "6379", "5432"],
            &["apps", "nft", "apply", "38600-38899", "38900-38999", "5433"],
        ];
        for a in open {
            assert_eq!(Verb::parse(a).unwrap().class(), Class::Open, "{a:?}");
        }
        for a in daemon {
            assert_eq!(Verb::parse(a).unwrap().class(), Class::Daemon, "{a:?}");
        }
        assert_eq!(Verb::parse(&["apps", "status"]).unwrap().namespace(), "apps");
        assert!(!Verb::AppsStatus.takes_lock());
        assert!(Verb::AppsStatus.needs_root());
        for v in catalog().into_iter().filter(|v| v.namespace() == "apps") {
            if v == Verb::AppsInstallEngine {
                assert_eq!(v.stdin_cap(), Some(CAP_ENGINE));
                continue;
            }
            assert_eq!(v.stdin_cap(), None, "{v:?}: apps verbs read no stdin");
            if v != Verb::AppsStatus && !matches!(v, Verb::AppsLogs { .. }) {
                assert!(v.takes_lock(), "{v:?}");
            }
        }
    }

    /// Paths, users, units, names and free-form values never parse.
    #[test]
    fn apps_attack_table_is_refused() {
        let rejects: &[&[&str]] = &[
            &["apps"],
            &["apps", "status", "--json"],
            &["apps", "ensure-user", "root"],
            &["apps", "ensure-user", "k2"],
            &["apps", "ensure-user", "--uid", "0"],
            &["apps", "subid", "100000-165535"],
            &["apps", "subid", "k2app"],
            &["apps", "linger"],
            &["apps", "linger", "k2"],
            &["apps", "linger", "on", "k2"],
            &["apps", "linger", "ON"],
            &["apps", "delegate", "on", "cpu"],
            &["apps", "delegate", "Delegate=yes"],
            &["apps", "io-cost", "on", "8:0"],
            &["apps", "io-cost", "8:0"],
            &["apps", "reserve-ports", "1-65535"],
            &["apps", "reserve-ports", "on", "38600-38999"],
            // nft: ranges are bounded and exact.
            &["apps", "nft", "apply"],
            &["apps", "nft", "apply", "38600-38899", "38900-38999"],
            &["apps", "nft", "apply", "1-65535", "38900-38999", "5432"],
            &["apps", "nft", "apply", "22", "38900-38999", "5432"],
            &["apps", "nft", "apply", "38600-40000", "38900-38999", "5432"],
            &["apps", "nft", "apply", "38600-38899", "38900-38999", "5432;flush ruleset"],
            &["apps", "nft", "apply", "38600-38899", "38900-38999", "5432", "53"],
            &["apps", "nft", "flush"],
            &["apps", "nft", "remove", "k2app"],
            // packages: never a name.
            &["apps", "packages", "install", "podman"],
            &["apps", "packages", "install", "netcat"],
            &["apps", "packages", "remove"],
            &["apps", "packages", "install-youki", "/tmp/youki.deb"],
            // runtime: an enum, never a path.
            &["apps", "runtime", "/usr/bin/crun"],
            &["apps", "runtime", "runc"],
            &["apps", "runtime", "runsc"],
            &["apps", "runtime", "krun"],
            &["apps", "runtime"],
            // process: the App id is the regex and nothing else.
            &["apps", "process", "start"],
            &["apps", "process", "start", "../k2-daemon"],
            &["apps", "process", "start", "k2-daemon.service"],
            &["apps", "process", "start", "jobs.service"],
            &["apps", "process", "start", "Jobs"],
            &["apps", "process", "start", "1jobs"],
            &["apps", "process", "start", "jobs-"],
            &["apps", "process", "start", "a--b"],
            &["apps", "process", "start", "jobs@x"],
            &["apps", "process", "start", "jobs", "--now"],
            &["apps", "process", "restart", "jobs"],
            &["apps", "process", "kill", "jobs"],
            // caps: bounded numbers, `-`, and device tokens only.
            &["apps", "process", "caps", "jobs"],
            &["apps", "process", "caps", "jobs", "512"],
            &["apps", "process", "caps", "jobs", "512", "-.-.-"],
            &["apps", "process", "caps", "jobs", "512", "-", "-", "-", "-"],
            &["apps", "process", "caps", "jobs", "512M", "-.-.-.-"],
            &["apps", "process", "caps", "jobs", "63", "-.-.-.-"],
            &["apps", "process", "caps", "jobs", "512", "-.40M.-.-"],
            &["apps", "process", "caps", "jobs", "512", "-.1.-.-"],
            &["apps", "process", "caps", "jobs", "512", "-.-.-.infinity"],
            &["apps", "process", "caps", "jobs", "512", "-.-.-.-", "/dev/sda"],
            &["apps", "process", "caps", "jobs", "512", "-.-.-.-", "kvm"],
            &["apps", "process", "caps", "jobs", "512", "-.-.-.-", "dri-render128", "dri-render128"],
            &["apps", "process", "caps", "jobs", "512", "-.-.-.-", "nvidia0", "nvidia1", "nvidia2", "nvidia3", "nvidia4", "nvidia5", "nvidia6", "nvidia7", "nvidia8"],
            &["apps", "process", "caps", "../x", "512", "-.-.-.-"],
            // The longer argv is for caps only.
            &["apps", "process", "start", "jobs", "a", "b", "c", "d", "e", "f"],
            &["apps", "process", "purge", "../x"],
            &["apps", "process", "purge", "jobs", "--all"],
            &["apps", "ensure-process-user", "k2app"],
            &["apps", "ensure-process-user", "root"],
            &["apps", "process-unit", "on", "/etc/systemd/system/x.service"],
            // apparmor: on|off and nothing else (never a profile or a path).
            &["apps", "apparmor"],
            &["apps", "apparmor", "on", "k2-app"],
            &["apps", "apparmor", "/etc/apparmor.d/k2-app"],
            &["apps", "apparmor", "complain"],
            &["apps", "apparmor", "unconfined"],
            // numbers: bounded decimal, no units.
            &["apps", "cores-slice", "0"],
            &["apps", "cores-slice", "127"],
            &["apps", "cores-slice", "768M"],
            &["apps", "cores-slice", "0768"],
            &["apps", "cores-slice", "-1"],
            &["apps", "cores-slice", "99999999"],
            &["apps", "budget", "4096"],
            &["apps", "budget", "4096", "4096", "x"],
            &["apps", "budget", "100", "4096"],
            &["apps", "budget", "4096", "10"],
            &["apps", "budget", "4096", "100000"],
            &["apps", "budget", "infinity", "4096"],
            &["apps", "unprep", "--force"],
            &["apps", "install-podman"],
            &["apps", "kvm", "on"],
            // nft app: one App id and one port inside the gateway range.
            &["apps", "nft", "app", "jobs"],
            &["apps", "nft", "app", "jobs", "5432"],
            &["apps", "nft", "app", "jobs", "38900"],
            &["apps", "nft", "app", "jobs", "38600-38601"],
            &["apps", "nft", "app", "jobs", "22"],
            &["apps", "nft", "app", "../x", "38600"],
            &["apps", "nft", "app", "jobs", "38600", "accept"],
            // Box services: never the deny-list, never a range, no repeats,
            // at most eight.
            &["apps", "nft", "app", "jobs", "38600", "22"],
            &["apps", "nft", "app", "jobs", "38600", "5432"],
            &["apps", "nft", "app", "jobs", "38600", "38601"],
            &["apps", "nft", "app", "jobs", "38600", "38950"],
            &["apps", "nft", "app", "jobs", "38600", "2019"],
            &["apps", "nft", "app", "jobs", "38600", "8470-8471"],
            &["apps", "nft", "app", "jobs", "38600", "08470"],
            &["apps", "nft", "app", "jobs", "38600", "8470", "8470"],
            &["apps", "nft", "app", "jobs", "38600", "2001", "2002", "2003", "2004", "2005", "2006", "2007", "2008", "2009"],
            &["apps", "nft", "app", "user.slice/x", "38600"],
            &["apps", "nft", "app-remove"],
            &["apps", "nft", "app-remove", "*"],
            // install-engine: never a path or a version.
            &["apps", "install-engine", "/tmp/k2-app-engine"],
            &["apps", "install-engine", "0.46.1"],
            // logs: an App id, a deployment id, a bounded count.
            &["apps", "logs", "jobs", "d-0123456789ab"],
            &["apps", "logs", "jobs", "d-0123456789ab", "0"],
            &["apps", "logs", "jobs", "d-0123456789ab", "5001"],
            &["apps", "logs", "jobs", "d-0123456789AB", "10"],
            &["apps", "logs", "jobs", "_UID=0", "10"],
            &["apps", "logs", "jobs", "k2-daemon.service", "10"],
            &["apps", "logs", "../x", "d-0123456789ab", "10"],
            &["apps", "logs", "jobs", "d-0123456789ab", "10", "--all"],
        ];
        for r in rejects {
            assert!(Verb::parse(r).is_err(), "expected refusal: {r:?}");
        }
        let long_id = format!("a{}", "b".repeat(32));
        assert!(Verb::parse(&["apps", "process", "start", &long_id]).is_err());
    }

    #[test]
    fn accepted_ids() {
        for db in ["postgres", "ws_abc_123", "myapp_test", &"a".repeat(63)] {
            assert!(Verb::parse(&["pg", "psql", "-d", db]).is_ok(), "{db}");
        }
        for unit in [
            "postgresql",
            "postgresql.service",
            "postgresql@1-main",
            "postgresql@16-main",
        ] {
            assert!(
                Verb::parse(&["pg", "systemctl", "restart", unit]).is_ok(),
                "{unit}"
            );
        }
        let v = Verb::parse(&["pg", "psql", "-tA", "-v", "ON_ERROR_STOP=1", "-d", "ws_x"]).unwrap();
        assert_eq!(
            v.argv(),
            vec!["pg", "psql", "-d", "ws_x", "-v", "ON_ERROR_STOP=1", "-tA"]
        );
    }

    #[test]
    fn stdin_caps_and_locks() {
        for v in catalog() {
            let cap = v.stdin_cap();
            match v {
                Verb::HelperSelfUpdate => assert_eq!(cap, Some(CAP_SELF_UPDATE)),
                Verb::MailRenderUnit => assert_eq!(cap, Some(CAP_TEMPLATE)),
                Verb::MailInstallBin => assert_eq!(cap, Some(CAP_INSTALL_BIN)),
                Verb::PgPsql { .. } => assert_eq!(cap, Some(CAP_SQL)),
                Verb::AppsInstallEngine => assert_eq!(cap, Some(CAP_ENGINE)),
                _ => assert_eq!(cap, None, "{v:?} reads no stdin"),
            }
            if v.class() == Class::Daemon && !matches!(v, Verb::PgPsql { .. } | Verb::KeepKekGet | Verb::AppsLogs { .. }) {
                assert!(v.takes_lock(), "{v:?}");
            }
        }
        assert!(!Verb::HelperVersion.needs_root());
        assert!(catalog().iter().filter(|v| !v.needs_root()).count() == 1);
    }
}
