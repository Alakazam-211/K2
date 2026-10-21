//! `/usr/local/libexec/k2-root-helper` — K2's one root door on Linux
//! (`prd-k2-root-helper-v1`).
//!
//! `sudo -n /usr/local/libexec/k2-root-helper <namespace> <verb> [<token>…]`
//!
//! One call, in order (§5.3):
//! 1. argv must parse exactly against the verb catalog (`k2-root-proto`);
//! 2. euid 0 (except `helper version`);
//! 3. stdin only for verbs that declare it, up to the verb's cap;
//! 4. `daemon`-class verbs: the caller must be the daemon's main process
//!    (exit 77 otherwise, §6);
//! 5. mutating verbs take `flock(/run/k2-root-helper.lock)` (30 s);
//! 6. the verb runs; stdout is one JSON object (or the 32-byte KEK);
//! 7. one audit line (never stdin, never a key, never a password).
//!
//! A bug in any namespace is a root bug: changes here need two
//! independent reviewers, one of them a security pass (Q10).

pub mod apps;
pub mod caller;
pub mod core;
pub mod ctx;
pub mod daemon_unit;
pub mod front;
pub mod fsx;
pub mod keep;
pub mod mail;
pub mod mail_data;
pub mod pg;
pub mod pg_major;
pub mod sew;
pub mod sew_teardown;
pub mod podman;
pub mod sys;

#[cfg(test)]
mod tests;

use std::io::Read;

use k2_root_proto::verbs::{exit, Class, Verb};

use ctx::{Ctx, Fail, HResult};

/// The marker a signed build must carry exactly once (§9.1, §9.2 rule 4).
#[cfg(target_arch = "x86_64")]
#[used]
pub static BUILD_MARKER: &str = concat!(
    "K2_ROOT_HELPER_BUILD v=",
    env!("K2_PRODUCT_VERSION"),
    " proto=9 arch=x86_64;"
);
#[cfg(target_arch = "aarch64")]
#[used]
pub static BUILD_MARKER: &str = concat!(
    "K2_ROOT_HELPER_BUILD v=",
    env!("K2_PRODUCT_VERSION"),
    " proto=9 arch=aarch64;"
);

/// Present only in a `test-keys` build; release signing refuses it.
#[cfg(feature = "test-keys")]
#[used]
pub static TEST_KEYS_FLAG: &str = "K2_ROOT_HELPER_TEST_KEYS_ENABLED";

/// Referenced from `helper version` so the linker keeps the flag string.
pub fn test_keys_flag() -> Option<&'static str> {
    #[cfg(feature = "test-keys")]
    {
        Some(TEST_KEYS_FLAG)
    }
    #[cfg(not(feature = "test-keys"))]
    {
        None
    }
}

/// What one call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub stdout: Vec<u8>,
    /// One short line (empty on success).
    pub stderr: String,
}

/// Read stdin for a verb: nothing unless it declares a cap; refuse more
/// than the cap.
pub fn read_stdin(verb: &Verb, mut r: impl Read) -> HResult<Vec<u8>> {
    let Some(cap) = verb.stdin_cap() else {
        return Ok(Vec::new());
    };
    let mut buf = Vec::new();
    (&mut r)
        .take(cap as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| Fail::io("read stdin failed"))?;
    if buf.len() > cap {
        return Err(Fail::argv("stdin too large"));
    }
    Ok(buf)
}

/// The exclusive helper lock (held until drop).
pub struct Lock {
    _file: std::fs::File,
}

/// One lock per namespace (`/run/k2-root-helper.<ns>.lock`): a long mail
/// snapshot never makes a Postgres restart or a keep call "busy" (review).
pub fn take_lock(ctx: &Ctx, ns: &str) -> HResult<Lock> {
    use std::os::fd::AsRawFd;
    let d = ctx
        .dir(k2_root_proto::paths::LOCK_DIR)
        .map_err(|e| Fail::io_err("open /run", e))?;
    let name = format!("k2-root-helper.{ns}.lock");
    let name = std::ffi::OsStr::new(&name);
    let f = match d.kind(name).map_err(|e| Fail::io_err("stat lock", e))? {
        fsx::Kind::Missing => match d.create_excl(name) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => open_lock(&d, name)?,
            Err(e) => return Err(Fail::io_err("create lock", e)),
        },
        fsx::Kind::File => open_lock(&d, name)?,
        _ => return Err(Fail::state("lock path is not a regular file")),
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(ctx.lock_timeout_ms);
    loop {
        // SAFETY: fd owned by f.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Lock { _file: f });
        }
        if std::time::Instant::now() >= deadline {
            return Err(Fail::state("busy: another helper call holds the lock"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn open_lock(d: &fsx::Dir, name: &std::ffi::OsStr) -> HResult<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(name.as_bytes()).map_err(|_| Fail::io("lock name"))?;
    // SAFETY: valid dirfd and name; O_NOFOLLOW refuses a planted link.
    let fd = unsafe {
        libc::openat(
            d.raw(),
            c.as_ptr(),
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(Fail::io_err("open lock", std::io::Error::last_os_error()));
    }
    // SAFETY: fresh fd.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn dispatch(
    ctx: &Ctx,
    verb: &Verb,
    stdin: &[u8],
    who: &caller::Caller,
) -> HResult<Option<serde_json::Value>> {
    let user = || caller::require_daemon(who);
    Ok(match verb {
        Verb::HelperVersion => Some(core::version_json(ctx)),
        Verb::HelperSelfUpdate => Some(core::self_update(ctx, stdin)?),
        Verb::HelperSelfRollback => Some(core::self_rollback(ctx)?),
        Verb::HelperLegacyRetire(which) => Some(core::legacy_retire(ctx, *which)?),
        Verb::HelperAuditTail => Some(core::audit_tail(ctx)?),

        Verb::MailEnsureUser => mail::ensure_user(ctx).map(|_| None)?,
        Verb::MailMkdir(t) => mail::mkdir(ctx, *t).map(|_| None)?,
        Verb::MailChown(t) => mail::chown(ctx, *t).map(|_| None)?,
        Verb::MailRenderUnit => mail::render_unit(ctx, stdin).map(|_| None)?,
        Verb::MailRenderDropin => mail::render_dropin(ctx).map(|_| None)?,
        Verb::MailInstallBin => Some(mail::install_bin(ctx, stdin)?),
        Verb::MailSystemctl(w) => mail::systemctl(ctx, *w).map(|_| None)?,
        Verb::MailRemove(t) => mail::remove(ctx, *t).map(|_| None)?,
        Verb::MailUsage => Some(mail::usage(ctx)?),
        Verb::MailSnapshotData => Some(mail::snapshot(ctx)?),
        Verb::MailRestoreData => Some(mail::restore(ctx)?),

        Verb::PgSystemctl { op, now, unit } => pg::systemctl(ctx, *op, *now, unit).map(|_| None)?,
        Verb::PgDaemonReload => pg::daemon_reload(ctx).map(|_| None)?,
        Verb::PgInstallRamFence => Some(pg::install_ram_fence(ctx)?),
        Verb::PgPsql { db, flags, cluster } => pg::psql(ctx, db, *flags, *cluster, stdin).map(|_| None)?,
        Verb::PgHbaApp { add, id } => Some(pg::hba_app(ctx, *add, id)?),
        Verb::PgClusters => Some(pg_major::clusters(ctx)?),
        Verb::PgAddMajor { major, dry_run } => Some(pg_major::add_major(ctx, *major, *dry_run)?),
        Verb::PgEnsureCluster { major, port } => Some(pg_major::ensure_cluster(ctx, *major, *port)?),
        Verb::PgStartConf { major, auto } => Some(pg_major::start_conf(ctx, *major, *auto)?),
        Verb::PgInstallSlice => Some(pg_major::install_slice(ctx)?),
        Verb::PgCopyRoles { from, to, dry_run } => Some(pg_major::copy_roles(ctx, *from, *to, *dry_run, stdin)?),
        Verb::PgMoveDump { from, to, db, tag } => Some(pg_major::move_dump(ctx, *from, *to, db, tag)?),
        Verb::PgMoveRestore { to, db, tag } => Some(pg_major::move_restore(ctx, *to, db, tag, stdin)?),
        Verb::PgMoveFileRemove { db, tag } => Some(pg_major::move_file_remove(ctx, db, tag)?),
        Verb::PgAllowConn { major, db, mode } => Some(pg_major::allow_conn(ctx, *major, db, *mode)?),
        Verb::PgDropDb { major, db } => Some(pg_major::drop_db(ctx, *major, db)?),

        Verb::KeepStatus => Some(keep::status(ctx, who.user.as_deref())?),
        Verb::KeepKekInit => keep::kek_init(ctx, &user()?).map(|_| None)?,
        Verb::KeepKekGet => keep::kek_get(ctx, &user()?).map(|_| None)?,
        Verb::KeepKekStage => keep::kek_stage(ctx, &user()?).map(|_| None)?,
        Verb::KeepKekCommit => keep::kek_commit(ctx, &user()?).map(|_| None)?,
        Verb::KeepKekAbort => keep::kek_abort(ctx, &user()?).map(|_| None)?,
        Verb::KeepGenGet => Some(keep::gen_get(ctx, who.user.as_deref())?),
        Verb::KeepGenBump => Some(keep::gen_bump(ctx, &user()?)?),

        Verb::DaemonHandoffDropinInstall => Some(daemon_unit::handoff_dropin_set(ctx, who, true)?),
        Verb::DaemonHandoffDropinRemove => Some(daemon_unit::handoff_dropin_set(ctx, who, false)?),

        Verb::AppsStatus => Some(apps::status(ctx)?),
        Verb::AppsEnsureUser => Some(apps::ensure_user(ctx)?),
        Verb::AppsEnsureProcessUser => Some(apps::ensure_process_user(ctx)?),
        Verb::AppsSubid => Some(apps::subid(ctx)?),
        Verb::AppsLinger(on) => Some(apps::linger(ctx, *on)?),
        Verb::AppsDelegate(on) => Some(apps::delegate(ctx, *on)?),
        Verb::AppsIoCost(on) => Some(apps::io_cost(ctx, *on)?),
        Verb::AppsReservePorts(on) => Some(apps::reserve_ports(ctx, *on)?),
        Verb::AppsNftApply { gw, px, db } => Some(apps::nft_apply(ctx, *gw, *px, *db)?),
        Verb::AppsNftRemove => Some(apps::nft_remove(ctx)?),
        Verb::AppsNftApp { id, gw, box_ports } => Some(apps::nft_app(ctx, id, *gw, box_ports)?),
        Verb::AppsNftAppRemove(id) => Some(apps::nft_app_remove(ctx, id)?),
        Verb::AppsInstallEngine => Some(apps::install_engine(ctx, stdin)?),
        Verb::AppsLogs { id, deployment, lines } => Some(apps::logs(ctx, id, deployment, *lines)?),
        Verb::AppsPackagesInstall => Some(apps::packages_install(ctx)?),
        Verb::AppsPackagesInstallYouki => Some(apps::packages_install_youki(ctx)?),
        Verb::AppsRuntime(rt) => Some(apps::runtime(ctx, *rt)?),
        Verb::AppsProcessUnit(on) => Some(apps::process_unit_set(ctx, *on)?),
        Verb::AppsApparmor(on) => Some(apps::apparmor(ctx, *on)?),
        Verb::AppsProcessStart(id) => Some(apps::process_start(ctx, id)?),
        Verb::AppsProcessStop(id) => Some(apps::process_stop(ctx, id)?),
        Verb::AppsProcessPurge(id) => Some(apps::process_purge(ctx, id)?),
        Verb::AppsProcessCaps { id, mem_mib, io, devices } => {
            Some(apps::process_caps(ctx, id, *mem_mib, io, devices)?)
        }
        Verb::AppsCoresSlice(n) => Some(apps::cores_slice(ctx, who, *n)?),
        Verb::AppsBudget(b) => Some(apps::budget(ctx, *b)?),
        Verb::AppsUnprep => Some(apps::unprep(ctx, who)?),

        Verb::Sew(v) => Some(sew::run(ctx, v, stdin, who)?),
        Verb::AppsPodmanInstall => Some(podman::install(ctx, stdin)?),
        Verb::AppsPodmanUninstall => Some(podman::uninstall(ctx)?),

        Verb::FrontStatus => Some(front::status(ctx)?),
        Verb::FrontInstall => Some(front::install(ctx, who)?),
        Verb::FrontRemove => Some(front::remove(ctx, who)?),
    })
}

fn audit(
    ctx: &Ctx,
    verb: Option<&Verb>,
    who: Option<&caller::Caller>,
    code: i32,
    extra: Option<&serde_json::Value>,
) {
    let (ns, name, class) = match verb {
        Some(v) => (v.namespace(), v.audit_name(), v.class().as_str()),
        None => ("?", "refused-argv".to_string(), "-"),
    };
    let caller = who.map(|c| {
        serde_json::json!({"pid": c.pid, "uid": c.uid, "isDaemon": c.is_daemon, "whyNot": c.why_not})
    });
    let mut line = serde_json::json!({
        "ns": ns,
        "verb": name,
        "class": class,
        "caller": caller,
        "result": if code == exit::OK { "ok" } else if code == exit::NOT_DAEMON { "caller_not_daemon" } else { "refused_or_failed" },
        "code": code,
    });
    // Self-update / rollback: from → to, sha, key id. Never stdin.
    if let (Some(Verb::HelperSelfUpdate | Verb::HelperSelfRollback), Some(x)) = (verb, extra) {
        for k in ["from", "to", "sha256", "keyId", "result"] {
            if let Some(v) = x.get(k) {
                line[if k == "result" { "update" } else { k }] = v.clone();
            }
        }
    }
    // Box prep added AppArmor's userspace: say which fixed package.
    if let (Some(Verb::AppsPackagesInstall), Some(x)) = (verb, extra) {
        let p = x.pointer("/apparmor/installed").filter(|p| p.as_array().is_some_and(|a| !a.is_empty()));
        if let Some(p) = p {
            line["apparmorInstalled"] = p.clone();
        }
    }
    // A Sew run answers helper code 0 whatever Sew did: the journal says
    // what Sew exited with, and why when it was not 0 (bounded, cleaned).
    if let (Some(Verb::Sew(_)), Some(x)) = (verb, extra) {
        if let Some(e) = x.get("sewExit") {
            line["sewExit"] = e.clone();
            if let Some(r) = x.get("reason") {
                line["sewReason"] = r.clone();
            }
        }
    }
    // K2's Podman bundle: which build went in (or what came out).
    if let (Some(Verb::AppsPodmanInstall), Some(x)) = (verb, extra) {
        for k in ["installed", "sha256", "arch", "os"] {
            if let Some(v) = x.get(k) {
                line[if k == "installed" { "podman" } else { k }] = v.clone();
            }
        }
    }
    if let (Some(Verb::AppsPodmanUninstall), Some(x)) = (verb, extra) {
        if let Some(v) = x.get("removed") {
            line["removed"] = v.clone();
        }
    }
    if let Some(Verb::PgPsql { db, .. }) = verb {
        line["db"] = serde_json::json!(db.as_str());
    }
    // A move's steps: which database (validated), which tag; what a copy
    // or an install did. Never a role's password.
    match verb {
        Some(
            Verb::PgMoveDump { db, tag, .. }
            | Verb::PgMoveRestore { db, tag, .. }
            | Verb::PgMoveFileRemove { db, tag },
        ) => {
            line["db"] = serde_json::json!(db.as_str());
            line["tag"] = serde_json::json!(tag);
        }
        Some(Verb::PgAllowConn { db, .. } | Verb::PgDropDb { db, .. }) => {
            line["db"] = serde_json::json!(db.as_str());
        }
        _ => {}
    }
    if let (Some(Verb::PgCopyRoles { .. }), Some(x)) = (verb, extra) {
        line["rolesCreated"] = x["created"].clone();
    }
    if let (Some(Verb::PgAddMajor { .. }), Some(x)) = (verb, extra) {
        for k in ["result", "source", "major"] {
            if let Some(v) = x.get(k) {
                line[k] = v.clone();
            }
        }
    }
    ctx.audit.line(&line.to_string());
}

/// One full call. `stdin` is read only when the verb takes it.
pub fn run(ctx: &Ctx, args: &[&str], stdin: impl Read) -> Outcome {
    let fail = |f: Fail| Outcome {
        code: f.code,
        stdout: Vec::new(),
        stderr: f.msg,
    };
    let verb = match Verb::parse(args) {
        Ok(v) => v,
        Err(e) => {
            audit(ctx, None, None, exit::ARGV, None);
            return fail(Fail::argv(e));
        }
    };
    if verb.needs_root() && ctx.euid != 0 {
        return fail(Fail::state("not running as root (use sudo -n)"));
    }
    // Open verbs record pid/uid (and the daemon user, for keep status);
    // daemon verbs run the full MainPID check. Both happen BEFORE stdin is
    // read, and the lock is taken before it too, so parallel callers can't
    // make root buffer 256 MiB each (security review #8).
    let needs_daemon = verb.class() == Class::Daemon;
    let who = if verb == Verb::HelperVersion {
        caller::Caller {
            pid: 0,
            uid: None,
            user: None,
            is_daemon: false,
            why_not: None,
        }
    } else {
        caller::identify(ctx, needs_daemon)
    };
    if needs_daemon && !who.is_daemon {
        audit(ctx, Some(&verb), Some(&who), exit::NOT_DAEMON, None);
        return fail(Fail::not_daemon());
    }
    let _lock = if verb.takes_lock() {
        match take_lock(ctx, verb.namespace()) {
            Ok(l) => Some(l),
            Err(f) => {
                audit(ctx, Some(&verb), Some(&who), f.code, None);
                return fail(f);
            }
        }
    } else {
        None
    };
    let input = match read_stdin(&verb, stdin) {
        Ok(b) => b,
        Err(f) => {
            audit(ctx, Some(&verb), Some(&who), f.code, None);
            return fail(f);
        }
    };
    let res = dispatch(ctx, &verb, &input, &who);
    let (code, json, err) = match res {
        Ok(j) => (exit::OK, j, String::new()),
        Err(f) => (f.code, None, f.msg),
    };
    if verb != Verb::HelperVersion && verb != Verb::HelperAuditTail {
        audit(ctx, Some(&verb), Some(&who), code, json.as_ref());
    }
    if let Some(j) = &json {
        ctx.print_json(j);
    }
    let stdout = std::mem::take(&mut *ctx.out.borrow_mut());
    Outcome {
        code,
        stdout,
        stderr: err,
    }
}

/// The binary's entry point.
pub fn main_entry() -> i32 {
    // SAFETY: umask has no preconditions.
    unsafe { libc::umask(0o022) };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let ctx = Ctx::production();
    let out = run(&ctx, &refs, std::io::stdin().lock());
    use std::io::Write;
    let _ = std::io::stdout().write_all(&out.stdout);
    let _ = std::io::stdout().flush();
    if !out.stderr.is_empty() {
        eprintln!("k2-root-helper: {}", out.stderr);
    }
    out.code
}
