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
/// `engine sew rotate-master`: 43 base64url chars + an optional `\n`.
pub const CAP_SEW_KEY: usize = 64;
/// `engine sew install|config|media install`: one small JSON object.
pub const CAP_SEW_JSON: usize = 64 * 1024;

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

/// `engine sew systemctl <op>`: fixed unit sets, never a unit name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SewSystemctl {
    /// `enable sew-db.service sew.service sew-backup.timer sew-drill.timer`
    Enable,
    /// `start sew-db.service`
    StartDb,
    /// `start sew-migrate.service` (runs the binary's own migrations)
    Migrate,
    /// `start sew.service sew-backup.timer sew-drill.timer`
    Start,
    /// `restart sew.service`
    Restart,
    /// `stop sew.service` (boot enablement untouched)
    Stop,
    /// `disable --now sew.service sew-db.service sew-backup.timer sew-drill.timer`
    DisableNow,
}

impl SewSystemctl {
    pub fn token(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::StartDb => "start-db",
            Self::Migrate => "migrate",
            Self::Start => "start",
            Self::Restart => "restart",
            Self::Stop => "stop",
            Self::DisableNow => "disable-now",
        }
    }
    fn parse(t: &str) -> Option<Self> {
        Some(match t {
            "enable" => Self::Enable,
            "start-db" => Self::StartDb,
            "migrate" => Self::Migrate,
            "start" => Self::Start,
            "restart" => Self::Restart,
            "stop" => Self::Stop,
            "disable-now" => Self::DisableNow,
            _ => return None,
        })
    }
    pub const ALL: [SewSystemctl; 7] =
        [Self::Enable, Self::StartDb, Self::Migrate, Self::Start, Self::Restart, Self::Stop, Self::DisableNow];
}

/// The `engine sew` namespace (protocol 7, Sew S29 K11). Argv is enum
/// tokens plus, for purge and restore, one validated id
/// ([`crate::sew::install_id_ok`], [`crate::sew::bundle_id_ok`]). URLs,
/// hosts, paths and keys come on stdin as JSON or key text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SewVerb {
    /// Pin, on-disk binary (sha → known version), staged tarballs, units,
    /// config/data presence. Read-only.
    Status,
    /// Stdin: the core release tarball. Kept only when its sha256 is the
    /// pin's for this arch.
    Stage,
    /// Stdin: the `sew-media` release tarball, pin-checked the same way.
    StageMedia,
    /// Remove the staged files.
    Unstage,
    /// Install `sew` from the staged tarball when `/usr/local/bin/sew` is
    /// absent (member sha = pin). Never replaces an installed binary.
    InstallBin,
    /// Stdin: [`crate::sew::InstallRequest`]. `sew install --config-json
    /// <staged json> --master-key-from - --no-start`, then the daemon's uid
    /// (from the caller check) into `[mgmt] allow_uids` (SW7).
    Install,
    /// Stdin: [`crate::sew::ConfigRequest`] → `sew config set` per key.
    Config,
    Systemctl(SewSystemctl),
    /// `sew upgrade --tarball <staged pin tarball> --sha256 <pin>` run by
    /// the known installed binary. Sew backs up, swaps, migrates, checks
    /// health and rolls back by itself; its exit code comes back as data.
    Upgrade,
    /// `sew upgrade --rollback` (never `--accept-data-loss-since`).
    Rollback,
    /// `sew uninstall` (data, config and secrets kept).
    Uninstall,
    /// `sew uninstall --purge --yes-delete-all-data=<id>` [`--keep-backups`].
    Purge { install_id: String, keep_backups: bool },
    Doctor,
    /// Stdin: the new master key text → `sew keys rotate-master --new-from -`.
    RotateMaster,
    /// `sew keys drop-prev`.
    DropPrev,
    /// `sew backup now`.
    Backup,
    /// `sew backup list`.
    Backups,
    /// Stop `sew.service`, `sew restore <id> --replace` (Sew takes its
    /// safety bundle first), start it again.
    Restore(String),
    /// The mgmt token (`sewsvc_…`) for the daemon: SW7 lets its uid onto the
    /// socket without the `sew-mgmt` group, so it can't read the file.
    Token,
    /// Stdin: [`crate::sew::MediaRequest`] → `sew media install
    /// --config-json` with the staged media tarball (Sew writes its own
    /// sysctl drop-in when `sysctl` is `apply`).
    MediaInstall,
    MediaUninstall,
}

impl SewVerb {
    fn parse(rest: &[&str]) -> Option<Self> {
        Some(match rest {
            ["status"] => Self::Status,
            ["stage"] => Self::Stage,
            ["stage-media"] => Self::StageMedia,
            ["unstage"] => Self::Unstage,
            ["install-bin"] => Self::InstallBin,
            ["install"] => Self::Install,
            ["config"] => Self::Config,
            ["systemctl", op] => Self::Systemctl(SewSystemctl::parse(op)?),
            ["upgrade"] => Self::Upgrade,
            ["rollback"] => Self::Rollback,
            ["uninstall"] => Self::Uninstall,
            ["purge", id] if crate::sew::install_id_ok(id) => {
                Self::Purge { install_id: id.to_string(), keep_backups: false }
            }
            ["purge", id, "keep-backups"] if crate::sew::install_id_ok(id) => {
                Self::Purge { install_id: id.to_string(), keep_backups: true }
            }
            ["doctor"] => Self::Doctor,
            ["rotate-master"] => Self::RotateMaster,
            ["drop-prev"] => Self::DropPrev,
            ["backup"] => Self::Backup,
            ["backups"] => Self::Backups,
            ["restore", id] if crate::sew::bundle_id_ok(id) => Self::Restore(id.to_string()),
            ["token"] => Self::Token,
            ["media", "install"] => Self::MediaInstall,
            ["media", "uninstall"] => Self::MediaUninstall,
            _ => return None,
        })
    }

    fn tail(&self) -> Vec<String> {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match self {
            Self::Status => s(&["status"]),
            Self::Stage => s(&["stage"]),
            Self::StageMedia => s(&["stage-media"]),
            Self::Unstage => s(&["unstage"]),
            Self::InstallBin => s(&["install-bin"]),
            Self::Install => s(&["install"]),
            Self::Config => s(&["config"]),
            Self::Systemctl(op) => s(&["systemctl", op.token()]),
            Self::Upgrade => s(&["upgrade"]),
            Self::Rollback => s(&["rollback"]),
            Self::Uninstall => s(&["uninstall"]),
            Self::Purge { install_id, keep_backups: false } => s(&["purge", install_id]),
            Self::Purge { install_id, keep_backups: true } => s(&["purge", install_id, "keep-backups"]),
            Self::Doctor => s(&["doctor"]),
            Self::RotateMaster => s(&["rotate-master"]),
            Self::DropPrev => s(&["drop-prev"]),
            Self::Backup => s(&["backup"]),
            Self::Backups => s(&["backups"]),
            Self::Restore(id) => s(&["restore", id]),
            Self::Token => s(&["token"]),
            Self::MediaInstall => s(&["media", "install"]),
            Self::MediaUninstall => s(&["media", "uninstall"]),
        }
    }

    /// `open`: read-only, sha-pin gated, or converging to what K2 sets
    /// anyway (start, restart, enable, a backup). `daemon`: sets a
    /// credential, hands over a secret, changes config, stops or removes
    /// the service, migrates or replaces data, or swaps the binary.
    fn class(&self) -> Class {
        use SewSystemctl as S;
        match self {
            Self::Status
            | Self::Stage
            | Self::StageMedia
            | Self::Unstage
            | Self::InstallBin
            | Self::Doctor
            | Self::Backup
            | Self::Backups
            | Self::Systemctl(S::Enable | S::StartDb | S::Start | S::Restart) => Class::Open,
            Self::Install
            | Self::Config
            | Self::Systemctl(S::Migrate | S::Stop | S::DisableNow)
            | Self::Upgrade
            | Self::Rollback
            | Self::Uninstall
            | Self::Purge { .. }
            | Self::RotateMaster
            | Self::DropPrev
            | Self::Restore(_)
            | Self::Token
            | Self::MediaInstall
            | Self::MediaUninstall => Class::Daemon,
        }
    }

    fn stdin_cap(&self) -> Option<usize> {
        match self {
            Self::Stage | Self::StageMedia => Some(CAP_INSTALL_BIN),
            Self::Install | Self::Config | Self::MediaInstall => Some(CAP_SEW_JSON),
            Self::RotateMaster => Some(CAP_SEW_KEY),
            _ => None,
        }
    }

    fn reads_only(&self) -> bool {
        matches!(self, Self::Status | Self::Doctor | Self::Backups)
    }

    /// One of each (tests, docs).
    pub fn catalog() -> Vec<SewVerb> {
        let mut v = vec![
            Self::Status,
            Self::Stage,
            Self::StageMedia,
            Self::Unstage,
            Self::InstallBin,
            Self::Install,
            Self::Config,
            Self::Upgrade,
            Self::Rollback,
            Self::Uninstall,
            Self::Purge { install_id: "48831765147549701".into(), keep_backups: false },
            Self::Purge { install_id: "none".into(), keep_backups: true },
            Self::Doctor,
            Self::RotateMaster,
            Self::DropPrev,
            Self::Backup,
            Self::Backups,
            Self::Restore("20260927T031512Z".into()),
            Self::Token,
            Self::MediaInstall,
            Self::MediaUninstall,
        ];
        v.extend(SewSystemctl::ALL.into_iter().map(Self::Systemctl));
        v
    }
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
    AppsSubid,
    AppsLinger(bool),
    AppsDelegate(bool),
    AppsIoCost(bool),
    AppsReservePorts(bool),
    AppsNftApply { gw: PortRange, px: PortRange, db: PortRange },
    AppsNftRemove,
    /// `apps nft app <id> <gw-port>`: the App's own gateway port, matched
    /// on its cgroups (slice 3). Re-run after each App start (nft resolves
    /// a cgroup path to an inode when the rule loads).
    AppsNftApp { id: AppId, gw: u16 },
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
    AppsProcessStart(AppId),
    AppsProcessStop(AppId),
    /// `Some(MemoryMin MiB)` = write; `None` = `off`.
    AppsCoresSlice(Option<u64>),
    /// `Some((MemoryMax MiB, TasksMax))` = write; `None` = `off`.
    AppsBudget(Option<(u64, u64)>),
    AppsUnprep,
    // engine sew (protocol 7, Sew S29 K11; see `crate::sew`)
    Sew(SewVerb),
}

use crate::apps::{bounded, AppId, OciRuntime, PortRange, BUDGET_MEM_MIB, BUDGET_TASKS, CORES_MIN_MIB};

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
        if args.len() > 8 || !args.iter().all(|a| token_ok(a)) {
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
            ["apps", "nft", "app", id, port] => {
                let gw = PortRange::parse(port).filter(|r| r.lo == r.hi).ok_or(REFUSED)?.lo;
                if !(crate::apps::GATEWAY_PORTS.lo..=crate::apps::GATEWAY_PORTS.hi).contains(&gw) {
                    return Err(REFUSED);
                }
                Verb::AppsNftApp { id: AppId::parse(id).ok_or(REFUSED)?, gw }
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
            ["apps", "process", "start", id] => {
                Verb::AppsProcessStart(AppId::parse(id).ok_or(REFUSED)?)
            }
            ["apps", "process", "stop", id] => Verb::AppsProcessStop(AppId::parse(id).ok_or(REFUSED)?),
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
            ["engine", "sew", rest @ ..] => Verb::Sew(SewVerb::parse(rest).ok_or(REFUSED)?),
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
            Verb::AppsSubid => s(&["apps", "subid"]),
            Verb::AppsLinger(on) => s(&["apps", "linger", on_off_name(*on)]),
            Verb::AppsDelegate(on) => s(&["apps", "delegate", on_off_name(*on)]),
            Verb::AppsIoCost(on) => s(&["apps", "io-cost", on_off_name(*on)]),
            Verb::AppsReservePorts(on) => s(&["apps", "reserve-ports", on_off_name(*on)]),
            Verb::AppsNftApply { gw, px, db } => {
                s(&["apps", "nft", "apply", &gw.token(), &px.token(), &db.token()])
            }
            Verb::AppsNftRemove => s(&["apps", "nft", "remove"]),
            Verb::AppsNftApp { id, gw } => s(&["apps", "nft", "app", id.as_str(), &gw.to_string()]),
            Verb::AppsNftAppRemove(id) => s(&["apps", "nft", "app-remove", id.as_str()]),
            Verb::AppsInstallEngine => s(&["apps", "install-engine"]),
            Verb::AppsLogs { id, deployment, lines } => {
                s(&["apps", "logs", id.as_str(), deployment, &lines.to_string()])
            }
            Verb::AppsPackagesInstall => s(&["apps", "packages", "install"]),
            Verb::AppsPackagesInstallYouki => s(&["apps", "packages", "install-youki"]),
            Verb::AppsRuntime(r) => s(&["apps", "runtime", r.as_str()]),
            Verb::AppsProcessUnit(on) => s(&["apps", "process-unit", on_off_name(*on)]),
            Verb::AppsProcessStart(id) => s(&["apps", "process", "start", id.as_str()]),
            Verb::AppsProcessStop(id) => s(&["apps", "process", "stop", id.as_str()]),
            Verb::AppsCoresSlice(None) => s(&["apps", "cores-slice", "off"]),
            Verb::AppsCoresSlice(Some(n)) => s(&["apps", "cores-slice", &n.to_string()]),
            Verb::AppsBudget(None) => s(&["apps", "budget", "off"]),
            Verb::AppsBudget(Some((m, t))) => {
                s(&["apps", "budget", &m.to_string(), &t.to_string()])
            }
            Verb::AppsUnprep => s(&["apps", "unprep"]),
            Verb::Sew(v) => {
                let mut a = s(&["engine", "sew"]);
                a.extend(v.tail());
                a
            }
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
            | Verb::AppsSubid
            | Verb::AppsLinger(true)
            | Verb::AppsDelegate(true)
            | Verb::AppsIoCost(true)
            | Verb::AppsReservePorts(true)
            | Verb::AppsPackagesInstall
            | Verb::AppsPackagesInstallYouki
            | Verb::AppsRuntime(OciRuntime::Crun)
            | Verb::AppsProcessUnit(true) => Open,
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
            | Verb::AppsProcessStart(_)
            | Verb::AppsProcessStop(_)
            | Verb::AppsCoresSlice(_)
            | Verb::AppsBudget(_)
            | Verb::AppsUnprep => Daemon,
            Verb::Sew(v) => v.class(),
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
            Verb::Sew(v) => v.stdin_cap(),
            _ => None,
        }
    }

    /// Mutating verbs take the global lock. Reads and `psql` don't (SQL
    /// concurrency is Postgres's job, and a 60 s statement must not starve
    /// a restart).
    pub fn takes_lock(&self) -> bool {
        if let Verb::Sew(v) = self {
            return !v.reads_only();
        }
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
            "engine" => "sew",
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
        Verb::AppsCoresSlice(Some(768)),
        Verb::AppsCoresSlice(None),
        Verb::AppsBudget(Some((4096, 4096))),
        Verb::AppsBudget(None),
        Verb::AppsUnprep,
    ];
    v.extend(SewVerb::catalog().into_iter().map(Verb::Sew));
    for on in [true, false] {
        v.push(Verb::AppsLinger(on));
        v.push(Verb::AppsDelegate(on));
        v.push(Verb::AppsIoCost(on));
        v.push(Verb::AppsReservePorts(on));
        v.push(Verb::AppsProcessUnit(on));
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
            &["apps", "subid"],
            &["apps", "linger", "on"],
            &["apps", "delegate", "on"],
            &["apps", "io-cost", "on"],
            &["apps", "reserve-ports", "on"],
            &["apps", "nft", "apply", "38600-38899", "38900-38949", "5432"],
            &["apps", "packages", "install"],
            &["apps", "packages", "install-youki"],
            &["apps", "runtime", "crun"],
            &["apps", "process-unit", "on"],
        ];
        let daemon: &[&[&str]] = &[
            &["apps", "linger", "off"],
            &["apps", "delegate", "off"],
            &["apps", "io-cost", "off"],
            &["apps", "reserve-ports", "off"],
            &["apps", "nft", "remove"],
            &["apps", "runtime", "youki"],
            &["apps", "process-unit", "off"],
            &["apps", "process", "start", "jobs"],
            &["apps", "process", "stop", "jobs"],
            &["apps", "cores-slice", "768"],
            &["apps", "cores-slice", "off"],
            &["apps", "budget", "4096", "4096"],
            &["apps", "budget", "off"],
            &["apps", "unprep"],
            &["apps", "nft", "app", "jobs", "38600"],
            &["apps", "nft", "app-remove", "jobs"],
            &["apps", "install-engine"],
            &["apps", "logs", "jobs", "d-0123456789ab", "200"],
            // Non-canonical ranges could widen what Apps reach.
            &["apps", "nft", "apply", "1024-2047", "38900-38949", "5432"],
            &["apps", "nft", "apply", "38600-38899", "6379", "5432"],
            &["apps", "nft", "apply", "38600-38899", "38900-38949", "5433"],
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
            &["apps", "nft", "apply", "38600-38899", "38900-38949"],
            &["apps", "nft", "apply", "1-65535", "38900-38949", "5432"],
            &["apps", "nft", "apply", "22", "38900-38949", "5432"],
            &["apps", "nft", "apply", "38600-40000", "38900-38949", "5432"],
            &["apps", "nft", "apply", "38600-38899", "38900-38949", "5432;flush ruleset"],
            &["apps", "nft", "apply", "38600-38899", "38900-38949", "5432", "53"],
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
            &["apps", "process-unit", "on", "/etc/systemd/system/x.service"],
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
                Verb::Sew(SewVerb::Stage | SewVerb::StageMedia) => assert_eq!(cap, Some(CAP_INSTALL_BIN)),
                Verb::Sew(SewVerb::Install | SewVerb::Config | SewVerb::MediaInstall) => {
                    assert_eq!(cap, Some(CAP_SEW_JSON))
                }
                Verb::Sew(SewVerb::RotateMaster) => assert_eq!(cap, Some(CAP_SEW_KEY)),
                _ => assert_eq!(cap, None, "{v:?} reads no stdin"),
            }
            if v.class() == Class::Daemon && !matches!(v, Verb::PgPsql { .. } | Verb::KeepKekGet | Verb::AppsLogs { .. }) {
                assert!(v.takes_lock(), "{v:?}");
            }
        }
        assert!(!Verb::HelperVersion.needs_root());
        assert!(catalog().iter().filter(|v| !v.needs_root()).count() == 1);
    }

    /// `engine sew` (protocol 7): classes written out by hand. A verb
    /// moving class must change this test on purpose.
    #[test]
    fn sew_class_table() {
        let open: &[&[&str]] = &[
            &["engine", "sew", "status"],
            &["engine", "sew", "stage"],
            &["engine", "sew", "stage-media"],
            &["engine", "sew", "unstage"],
            &["engine", "sew", "install-bin"],
            &["engine", "sew", "doctor"],
            &["engine", "sew", "backup"],
            &["engine", "sew", "backups"],
            &["engine", "sew", "systemctl", "enable"],
            &["engine", "sew", "systemctl", "start-db"],
            &["engine", "sew", "systemctl", "start"],
            &["engine", "sew", "systemctl", "restart"],
        ];
        let daemon: &[&[&str]] = &[
            &["engine", "sew", "install"],
            &["engine", "sew", "config"],
            &["engine", "sew", "systemctl", "migrate"],
            &["engine", "sew", "systemctl", "stop"],
            &["engine", "sew", "systemctl", "disable-now"],
            &["engine", "sew", "upgrade"],
            &["engine", "sew", "rollback"],
            &["engine", "sew", "uninstall"],
            &["engine", "sew", "purge", "48831765147549701"],
            &["engine", "sew", "purge", "none", "keep-backups"],
            &["engine", "sew", "rotate-master"],
            &["engine", "sew", "drop-prev"],
            &["engine", "sew", "restore", "20260927T031512Z"],
            &["engine", "sew", "token"],
            &["engine", "sew", "media", "install"],
            &["engine", "sew", "media", "uninstall"],
        ];
        for a in open {
            let v = Verb::parse(a).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            assert_eq!(v.class(), Class::Open, "{a:?}");
            assert_eq!(v.namespace(), "sew");
        }
        for a in daemon {
            let v = Verb::parse(a).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            assert_eq!(v.class(), Class::Daemon, "{a:?}");
            assert!(v.takes_lock(), "{a:?}");
        }
        assert_eq!(open.len() + daemon.len(), SewVerb::catalog().len(), "every sew verb is in one table");
        for read in [&["engine", "sew", "status"][..], &["engine", "sew", "doctor"], &["engine", "sew", "backups"]] {
            assert!(!Verb::parse(read).unwrap().takes_lock(), "{read:?}");
        }
        assert!(Verb::parse(&["engine", "sew", "stage"]).unwrap().takes_lock());
    }

    /// URLs, hosts, paths, keys, units, users and Sew flags never parse as
    /// `engine sew` argv: they come on stdin or not at all.
    #[test]
    fn sew_attack_table_is_refused() {
        let rejects: &[&[&str]] = &[
            &["engine"],
            &["engine", "sew"],
            &["engine", "stalwart", "status"],
            &["engine", "sew", "status", "--json"],
            &["engine", "sew", "install", "http://127.0.0.1:1/v1/chat/jwks"],
            &["engine", "sew", "install", "--provider", "standalone"],
            &["engine", "sew", "install", "--cred-mode", "plain"],
            &["engine", "sew", "stage", "/tmp/sew.tar.gz"],
            &["engine", "sew", "install-bin", "/tmp/sew"],
            &["engine", "sew", "config", "server.public_url"],
            &["engine", "sew", "config-set", "keys.mirror.path", "/mnt/x"],
            &["engine", "sew", "systemctl", "start", "k2-daemon.service"],
            &["engine", "sew", "systemctl", "restart", "sew-db.service"],
            &["engine", "sew", "systemctl", "daemon-reexec"],
            &["engine", "sew", "systemctl", "edit"],
            &["engine", "sew", "systemctl", "start", "postgresql@16-main"],
            &["engine", "sew", "systemctl"],
            &["engine", "sew", "upgrade", "--accept-data-loss-since=x"],
            &["engine", "sew", "rollback", "--accept-data-loss-since=20260927T031512Z"],
            &["engine", "sew", "uninstall", "--purge"],
            &["engine", "sew", "purge"],
            &["engine", "sew", "purge", "0"],
            &["engine", "sew", "purge", "01"],
            &["engine", "sew", "purge", "abc"],
            &["engine", "sew", "purge", "1", "keep"],
            &["engine", "sew", "purge", "1", "keep-backups", "x"],
            &["engine", "sew", "purge", "1;id"],
            &["engine", "sew", "purge", "--yes-delete-all-data=1"],
            &["engine", "sew", "restore"],
            &["engine", "sew", "restore", "/var/lib/sew/backups/x"],
            &["engine", "sew", "restore", ".."],
            &["engine", "sew", "restore", "x.tar"],
            &["engine", "sew", "restore", "20260927T031512Z", "--replace"],
            &["engine", "sew", "restore", "20260927T031512Z", "--no-safety-backup"],
            &["engine", "sew", "rotate-master", "--new-from", "-"],
            &["engine", "sew", "token", "k2"],
            &["engine", "sew", "media", "install", "wss://x.y"],
            &["engine", "sew", "media", "install", "--turn-cert", "/etc/shadow"],
            &["engine", "sew", "media"],
            &["engine", "sew", "media", "upgrade"],
            &["engine", "sew", "mgmt-group", "add"],
            &["engine", "sew", "sysctl", "net.core.rmem_max=1"],
            &["engine", "sew", "status;id"],
            &["engine", "sew", "status id"],
            &["engine", "sew", ""],
        ];
        for r in rejects {
            assert!(Verb::parse(r).is_err(), "expected refusal: {r:?}");
        }
    }
}
