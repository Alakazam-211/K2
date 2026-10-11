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
}

const REFUSED: &str = "refused arguments";

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
        }
    }

    /// `Some(cap)` when the verb reads stdin (up to `cap` bytes); `None`
    /// when it takes none and stdin is never read.
    pub fn stdin_cap(&self) -> Option<usize> {
        match self {
            Verb::HelperSelfUpdate => Some(CAP_SELF_UPDATE),
            Verb::MailRenderUnit => Some(CAP_TEMPLATE),
            Verb::MailInstallBin => Some(CAP_INSTALL_BIN),
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
    ];
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
                _ => assert_eq!(cap, None, "{v:?} reads no stdin"),
            }
            if v.class() == Class::Daemon && !matches!(v, Verb::PgPsql { .. } | Verb::KeepKekGet) {
                assert!(v.takes_lock(), "{v:?}");
            }
        }
        assert!(!Verb::HelperVersion.needs_root());
        assert!(catalog().iter().filter(|v| !v.needs_root()).count() == 1);
    }
}
