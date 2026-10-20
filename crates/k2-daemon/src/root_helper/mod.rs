//! The daemon's side of `/usr/local/libexec/k2-root-helper`
//! (`prd-k2-root-helper-v1` §8): one client, the probe order, protocol
//! negotiation, the legacy-argv translation, and auto-heal.
//!
//! - **Probe order** (§8.1): the root helper first; the legacy
//!   `k2-pg-helper` for pg verbs only when the root helper is absent (one
//!   release train). Mail verbs use the root helper alone: the legacy
//!   `k2-mail-helper` is retired at install (Rosson 2026-10-10).
//! - **Translation**: `mail::sysops` and `sql::sysops` keep speaking their
//!   protocol-2 / script argv. When the root helper is the door, that argv
//!   is mapped onto the root verb (named targets, render verbs) and checked
//!   against the shared catalog before sudo runs, so nothing off-catalog is
//!   ever sent.
//! - **Negotiation**: each feature compares the helper's `protocol` with its
//!   min-protocol constant (`k2_root_proto::version`).
//! - **Auto-heal** ([`heal`]): after boot (= after every daemon hop) the
//!   daemon fetches the signed helper for its own version and installs it
//!   through `helper self-update`; no human, rollback on a bad answer.
//!
//! Every privileged call is `sudo -n <absolute helper path> …` with the
//! absolute `/usr/bin/sudo` (RH12).

pub mod busy;
pub mod heal;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use k2_root_proto::paths::{
    HELPER_PATH, LEGACY_PG_HELPER, STALWART_BIN, STALWART_CONFIG_DIR, STALWART_DATA_DIR,
    STALWART_DROPIN_DIR, STALWART_DROPIN_PATH, STALWART_LOG_DIR, STALWART_UNIT, STALWART_UNIT_PATH,
    SUDO_PATH,
};
use k2_root_proto::templates::{hardening_dropin, recovery_pw_ok, systemd_unit};
use k2_root_proto::verbs::{exit, Verb};
pub use k2_root_proto::version::{
    BACKUP_MIN_PROTOCOL, KEEP_MIN_PROTOCOL, PG_MIN_PROTOCOL, PROTOCOL, SELF_UPDATE_MIN_PROTOCOL,
    UPGRADE_MIN_PROTOCOL,
};

/// Release assets live next to the daemon's own.
pub const RELEASE_DOWNLOAD_BASE: &str = "https://github.com/Alakazam-211/K2/releases/download";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// `/usr/local/libexec/k2-root-helper` present and `sudo -n` may run it.
    Root,
    /// Not installed.
    Missing,
    /// Installed but sudoers doesn't let this user run it without a password.
    NotAllowed,
}

/// What the probe found.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootHelperState {
    pub kind: Kind,
    pub version: Option<String>,
    /// 0 when unknown.
    pub protocol: u32,
    pub path: String,
    pub package_owned: bool,
    /// The full `helper version` JSON (when read).
    #[serde(skip)]
    pub version_json: Option<Value>,
    /// Why `version` couldn't be read, if it couldn't.
    pub error: Option<String>,
}

impl RootHelperState {
    pub fn missing() -> Self {
        Self {
            kind: Kind::Missing,
            version: None,
            protocol: 0,
            path: HELPER_PATH.to_string(),
            package_owned: false,
            version_json: None,
            error: None,
        }
    }

    /// Usable as a door at `min` protocol or newer.
    pub fn usable(&self, min: u32) -> bool {
        self.kind == Kind::Root && self.protocol >= min
    }
}

/// One finished helper call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOut {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// The sudo door, behind a trait so tests never run sudo.
pub trait Door: Send + Sync {
    fn exists(&self, path: &str) -> bool;
    /// `sudo -n -l <path>`: may this user run it without a password?
    fn sudo_allowed(&self, path: &str) -> bool;
    /// `sudo -n <path> <argv…>` with optional stdin.
    fn call(&self, path: &str, argv: &[String], stdin: Option<&[u8]>) -> std::io::Result<CallOut>;
}

pub struct RealDoor;

impl Door for RealDoor {
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn sudo_allowed(&self, path: &str) -> bool {
        std::process::Command::new(SUDO_PATH)
            .args(["-n", "-l", path])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn call(&self, path: &str, argv: &[String], stdin: Option<&[u8]>) -> std::io::Result<CallOut> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(SUDO_PATH);
        cmd.arg("-n").arg(path).args(argv);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd.spawn()?;
        if let Some(bytes) = stdin {
            let mut sin = child
                .stdin
                .take()
                .ok_or_else(|| std::io::Error::other("stdin"))?;
            // A refusal closes the pipe early; the exit code says why.
            let _ = sin.write_all(bytes);
        }
        let out = child.wait_with_output()?;
        Ok(CallOut {
            code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .take(2000)
                .collect(),
        })
    }
}

/// Probe the root helper (§8.1 step 1): file → `sudo -n -l` → `helper
/// version`. Never prompts.
pub fn probe_with(door: &dyn Door) -> RootHelperState {
    let mut st = RootHelperState::missing();
    if !door.exists(HELPER_PATH) {
        return st;
    }
    if !door.sudo_allowed(HELPER_PATH) {
        st.kind = Kind::NotAllowed;
        return st;
    }
    st.kind = Kind::Root;
    match door.call(HELPER_PATH, &["helper".into(), "version".into()], None) {
        Ok(out) if out.code == 0 => match serde_json::from_slice::<Value>(&out.stdout) {
            Ok(v) => {
                st.version = v["version"].as_str().map(str::to_string);
                st.protocol = v["protocol"].as_u64().unwrap_or(0) as u32;
                st.package_owned = v["packageOwned"].as_bool().unwrap_or(false);
                if let Some(p) = v["path"].as_str() {
                    st.path = p.to_string();
                }
                st.version_json = Some(v);
            }
            Err(_) => st.error = Some("helper version answer is not JSON".into()),
        },
        Ok(out) => st.error = Some(format!("helper version: exit {}: {}", out.code, out.stderr)),
        Err(e) => st.error = Some(format!("helper version: {e}")),
    }
    st
}

static CACHE: Mutex<Option<(Instant, RootHelperState)>> = Mutex::new(None);

/// The probe, cached 30 s (status pages poll; each probe is a `sudo -l`).
/// Unit tests never probe: they read [`test_seam`] (default: missing).
pub fn state() -> RootHelperState {
    if cfg!(test) {
        return seam_state();
    }
    let mut slot = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, st)) = slot.as_ref() {
        if at.elapsed() < Duration::from_secs(30) {
            return st.clone();
        }
    }
    let st = if cfg!(target_os = "linux") {
        probe_with(&RealDoor)
    } else {
        RootHelperState::missing()
    };
    *slot = Some((Instant::now(), st.clone()));
    st
}

#[cfg(test)]
fn seam_state() -> RootHelperState {
    test_seam::STATE.with(|s| s.borrow().clone().unwrap_or_else(RootHelperState::missing))
}

#[cfg(not(test))]
fn seam_state() -> RootHelperState {
    RootHelperState::missing()
}

/// Drop the cache (after a self-update / rollback / install, and on
/// **Check again**).
pub fn invalidate() {
    *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

#[cfg(test)]
pub mod test_seam {
    use super::RootHelperState;
    use std::cell::RefCell;
    thread_local! {
        pub static STATE: RefCell<Option<RootHelperState>> = const { RefCell::new(None) };
    }
    pub fn set(st: Option<RootHelperState>) {
        STATE.with(|s| *s.borrow_mut() = st);
    }
}

/// Turn a finished root-helper call into the daemon's error words. Never
/// echoes stdin.
pub fn map_call(argv: &[String], out: CallOut) -> Result<Vec<u8>, String> {
    let verb = argv.join(" ");
    match out.code {
        0 => Ok(out.stdout),
        exit::NOT_DAEMON => Err(format!(
            "root helper refused `{verb}`: caller_not_daemon — this daemon is not the process \
             named in /etc/k2-root-helper/daemon.toml (user/unit). Re-run install-root-helper.sh \
             with this daemon's --user/--unit/--scope as root: {}",
            install_command_here()
        )),
        // Exit 2 is "refused argv OR refused input" (a tarball sha, a bad
        // password, stdin over the cap). Only an unknown verb means the
        // helper is older than this daemon.
        exit::ARGV if out.stderr.contains("refused arguments") => Err(format!(
            "root helper refused `{verb}` (unknown to it) — the helper is older than this \
             daemon; `k2 root-helper update`"
        )),
        exit::ARGV => Err(format!("root helper refused `{verb}`: {}", out.stderr)),
        _ => {
            if let Some(mapped) =
                crate::mail::helper::map_helper_failure_with(None, &out.stderr, true)
            {
                invalidate();
                return Err(mapped);
            }
            Err(format!(
                "root helper {verb}: exit {}: {}",
                out.code, out.stderr
            ))
        }
    }
}

/// `sudo -n /usr/local/libexec/k2-root-helper <argv…>`. The argv must
/// parse against the catalog first (defense in depth: nothing off-catalog
/// ever reaches sudo).
pub fn call_with(
    door: &dyn Door,
    argv: &[String],
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    Verb::parse(&refs).map_err(|e| format!("root helper: {e} (not sent)"))?;
    let out = door
        .call(HELPER_PATH, argv, stdin)
        .map_err(|e| format!("sudo -n {HELPER_PATH}: {e}"))?;
    map_call(argv, out)
}

pub fn call(argv: &[String], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
    call_with(&RealDoor, argv, stdin)
}

/// [`call`] with stdout as trimmed text (JSON answers).
pub fn call_text(argv: &[String], stdin: Option<&[u8]>) -> Result<String, String> {
    call(argv, stdin).map(|b| String::from_utf8_lossy(&b).trim().to_string())
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn mail_target(path: &str, allowed: &[(&str, &'static str)]) -> Result<&'static str, String> {
    allowed
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, t)| *t)
        .ok_or_else(|| "root helper: no named target for that path".to_string())
}

const RECOVERY_ENV_PREFIX: &str = "Environment=STALWART_RECOVERY_ADMIN=admin:";

/// The recovery password inside a unit the supervisor rendered, or `None`
/// for the plain unit. `Err` for bytes that aren't one of the two canned
/// units (never sent; the secret stays out of the error).
pub fn recovery_pw_from_unit(bytes: &[u8]) -> Result<Option<String>, String> {
    let refused = || "root helper: unit bytes are not the canned unit (not sent)".to_string();
    if bytes == systemd_unit(None).as_bytes() {
        return Ok(None);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| refused())?;
    let start = text.find(RECOVERY_ENV_PREFIX).ok_or_else(refused)?;
    let after = &text[start + RECOVERY_ENV_PREFIX.len()..];
    let pw = &after[..after.find('\n').ok_or_else(refused)?];
    if !recovery_pw_ok(pw) || text.as_bytes() != systemd_unit(Some(pw)).as_bytes() {
        return Err(refused());
    }
    Ok(Some(pw.to_string()))
}

/// Map a protocol-2 `k2-mail-helper` argv (+ stdin) onto the root verb.
pub fn mail_argv(
    legacy: &[&str],
    stdin: Option<&[u8]>,
) -> Result<(Vec<String>, Option<Vec<u8>>), String> {
    let dirs = [
        (STALWART_CONFIG_DIR, "config"),
        (STALWART_DATA_DIR, "data"),
        (STALWART_LOG_DIR, "log"),
        (STALWART_DROPIN_DIR, "dropin-dir"),
    ];
    let removes = [
        (STALWART_UNIT_PATH, "unit"),
        (STALWART_DROPIN_DIR, "dropin-dir"),
        (STALWART_BIN, "bin"),
        (STALWART_DATA_DIR, "data"),
        (STALWART_LOG_DIR, "log"),
        (STALWART_CONFIG_DIR, "config"),
    ];
    let out = match legacy {
        ["ensure-user"] => (sv(&["mail", "ensure-user"]), None),
        ["mkdir", p] => (sv(&["mail", "mkdir", mail_target(p, &dirs)?]), None),
        ["chown", p] if *p != STALWART_DROPIN_DIR => {
            (sv(&["mail", "chown", mail_target(p, &dirs)?]), None)
        }
        ["remove", p] => (sv(&["mail", "remove", mail_target(p, &removes)?]), None),
        ["write", p] if *p == STALWART_UNIT_PATH => {
            let pw = recovery_pw_from_unit(stdin.unwrap_or_default())?;
            (
                sv(&["mail", "render-unit"]),
                Some(pw.map(String::into_bytes).unwrap_or_default()),
            )
        }
        ["write", p] if *p == STALWART_DROPIN_PATH => {
            if stdin.unwrap_or_default() != hardening_dropin().as_bytes() {
                return Err(
                    "root helper: drop-in bytes are not the canned drop-in (not sent)".into(),
                );
            }
            (sv(&["mail", "render-dropin"]), None)
        }
        ["install-bin"] => (sv(&["mail", "install-bin"]), stdin.map(<[u8]>::to_vec)),
        ["systemctl", "daemon-reload"] => (sv(&["mail", "systemctl", "daemon-reload"]), None),
        ["systemctl", "enable", "--now", u] if *u == STALWART_UNIT => {
            (sv(&["mail", "systemctl", "enable-now"]), None)
        }
        ["systemctl", "restart", u] if *u == STALWART_UNIT => {
            (sv(&["mail", "systemctl", "restart"]), None)
        }
        ["systemctl", "stop", u] if *u == STALWART_UNIT => {
            (sv(&["mail", "systemctl", "stop"]), None)
        }
        ["systemctl", "disable", "--now", u] if *u == STALWART_UNIT => {
            (sv(&["mail", "systemctl", "disable-now"]), None)
        }
        ["version"] => (sv(&["helper", "version"]), None),
        ["usage"] => (sv(&["mail", "usage"]), None),
        ["snapshot-data"] => (sv(&["mail", "snapshot-data"]), None),
        ["restore-data"] => (sv(&["mail", "restore-data"]), None),
        _ => return Err("root helper: no root verb for that mail argv (not sent)".into()),
    };
    let refs: Vec<&str> = out.0.iter().map(String::as_str).collect();
    Verb::parse(&refs).map_err(|e| format!("root helper: {e} (not sent)"))?;
    Ok(out)
}

/// Map a legacy `k2-pg-helper` argv onto the `pg` namespace.
pub fn pg_argv(legacy: &[&str]) -> Result<Vec<String>, String> {
    let mut v = vec!["pg".to_string()];
    v.extend(legacy.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = v.iter().map(String::as_str).collect();
    Verb::parse(&refs).map_err(|e| format!("root helper: pg argv {e} (not sent)"))?;
    Ok(v)
}

/// This daemon's systemd unit, user and scope, read from its own cgroup
/// (`/proc/self/cgroup`). Defaults: `k2`, `k2-daemon.service`, `system`.
pub fn own_unit() -> (String, String, String) {
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "k2".into());
    let cg = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    parse_own_unit(&cg).map_or_else(
        || (user.clone(), "k2-daemon.service".into(), "system".into()),
        |(unit, scope)| (user.clone(), unit, scope),
    )
}

/// `0::/system.slice/k2-daemon.service` → (unit, system);
/// `0::/user.slice/user-1001.slice/user@1001.service/app.slice/k2-daemon.service`
/// → (unit, user).
pub fn parse_own_unit(cgroup: &str) -> Option<(String, String)> {
    let line = cgroup.lines().find(|l| l.starts_with("0::"))?;
    let path = &line[3..];
    let unit = path.rsplit('/').find(|seg| seg.ends_with(".service"))?;
    let ok = unit.len() <= 72
        && unit
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'@' | b'.' | b'_' | b'-'));
    if !ok || unit.starts_with("user@") {
        return None;
    }
    let scope = if path.contains("/user@") {
        "user"
    } else {
        "system"
    };
    Some((unit.to_string(), scope.to_string()))
}

/// The one root command for a box without the helper (§11.3
/// `copy-command`). Paste it in an SSH session as an admin — never inside
/// a K2 terminal.
pub fn install_command_for(version: &str, user: &str, unit: &str, scope: &str) -> String {
    format!(
        "curl -fsSL {RELEASE_DOWNLOAD_BASE}/v{version}/install-root-helper.sh | sudo bash -s -- \
         --version {version} --user {user} --unit {unit} --scope {scope}"
    )
}

pub fn install_command_here() -> String {
    let (user, unit, scope) = own_unit();
    install_command_for(env!("CARGO_PKG_VERSION"), &user, &unit, &scope)
}

/// `rootSetup` (§11.3): what the box needs. `box-sudo` / `local-prompt`
/// are later slices.
pub fn root_setup(st: &RootHelperState) -> &'static str {
    if !cfg!(target_os = "linux") {
        return "linux-only";
    }
    if st.usable(SELF_UPDATE_MIN_PROTOCOL) {
        "not-needed"
    } else {
        "copy-command"
    }
}

/// Did the IMAP/TLS restart path use plain `sudo systemctl` (RH14)?
pub fn plain_sudo_restart_used() -> &'static std::sync::atomic::AtomicBool {
    static USED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &USED
}

/// Is the legacy pg helper there (and allowed)?
pub fn legacy_pg_present() -> bool {
    cfg!(target_os = "linux") && RealDoor.exists(LEGACY_PG_HELPER)
}

/// `GET /cli/root-helper/status` body.
pub fn status_json(
    st: &RootHelperState,
    legacy_mail: &str,
    legacy_pg: bool,
    events: Option<Value>,
) -> Value {
    // An installed-but-not-allowed root helper says so even when a legacy
    // helper still answers (correctness review #8).
    let kind = match st.kind {
        Kind::Root => "root",
        Kind::NotAllowed => "not_allowed",
        // Only the pg script still serves when the root helper is absent;
        // a leftover k2-mail-helper is reported (`legacy.mail`) but never
        // used (retired at install, Rosson 2026-10-10).
        Kind::Missing if legacy_pg => "legacy",
        Kind::Missing => "missing",
    };
    json!({
        "state": kind,
        "version": st.version,
        "protocol": st.protocol,
        "path": st.path,
        "packageOwned": st.package_owned,
        "error": st.error,
        "daemonVersion": env!("CARGO_PKG_VERSION"),
        "legacy": {"mail": legacy_mail, "pg": legacy_pg},
        "rootSetup": root_setup(st),
        "installCommand": install_command_here(),
        "minProtocol": {
            "upgrade": UPGRADE_MIN_PROTOCOL,
            "selfUpdate": SELF_UPDATE_MIN_PROTOCOL,
            "backup": BACKUP_MIN_PROTOCOL,
            "pg": PG_MIN_PROTOCOL,
            "keep": KEEP_MIN_PROTOCOL,
            "apps": k2_root_proto::version::APPS_MIN_PROTOCOL,
            "sew": k2_root_proto::version::SEW_MIN_PROTOCOL,
        },
        "autoHeal": heal::status_value(),
        "plainSudoRestartUsed": plain_sudo_restart_used().load(std::sync::atomic::Ordering::SeqCst),
        "lastEvents": events,
    })
}

/// A feature that needs `min` returns this when the helper is short.
pub fn helper_needed(feature: &str, min: u32, st: &RootHelperState) -> String {
    let have = if st.kind == Kind::Root {
        format!("protocol {}", st.protocol)
    } else {
        format!("{:?}", st.kind).to_lowercase()
    };
    format!(
        "helper_needed: {feature} needs k2-root-helper protocol {min} (this box: {have}). {}",
        match root_setup(st) {
            "not-needed" => "Run `k2 root-helper update`.".to_string(),
            _ => format!(
                "One root step, as an admin in an SSH session (not a K2 terminal): {}",
                install_command_here()
            ),
        }
    )
}

#[cfg(test)]
mod tests;
