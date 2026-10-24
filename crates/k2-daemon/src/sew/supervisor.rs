//! The Sew supervisor (Sew S29 §10–§17, K12): preflight, the resumable
//! enable machine, status reconcile + verdict, the health loop, disable /
//! uninstall / purge, upgrade with health check and automatic rollback,
//! manual rollback, backups and restore, master-key rotation and the
//! recovery key.
//!
//! Every machine takes an [`Env`]: the side effects ([`SewOps`]), the vault
//! store ([`SewSecrets`]), the lifecycle latch and the mgmt-token cache.
//! [`live`] is production; tests build one over fakes, so they never share
//! a latch or touch a real box.
//!
//! Rules kept from hosted mail (and S29):
//! - `ok` in a status body is the envelope, never the verdict; the state
//!   is never `running`/`degraded` unless BOTH `sew-db` and `sew` are active.
//! - One latch for every lifecycle step; a second call is 409.
//! - Enable is resumable: master key, stage, install-bin and install are
//!   marked and skipped on resume; the systemd steps run on EVERY enable
//!   (the hostmail H4 lesson).
//! - Upgrades only when a person or a Manage-Sew agent asks, only to this
//!   build's pin, never from an auto-update path.
//! - The master key reaches Sew only as helper stdin; it never enters
//!   argv, a log line, a progress record, an event or a route body (except
//!   the person-only recovery-key reveal).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use k2_root_proto::sew::{
    self as pin, pinned, pinned_artifact, version_cmp, MAX_TARBALL, SEW_CLIENT_PORT, SEW_CONFIG_DIR, SEW_DATA_DIR,
    SEW_MEDIA_LOOPBACK_PORT, SEW_MEDIA_TCP_PORT, SEW_MEDIA_UDP_PORT, SEW_TURN_TLS_PORT, SEW_UPGRADE_STATE, UNIT_DB,
    UNIT_MIGRATE, UNIT_SEW,
};
use k2_root_proto::version::SEW_MIN_PROTOCOL;

use super::ops::{MgmtError, RealOps, SewOps, UnitInfo};
use super::secrets::{kek_id, SewSecrets, VaultSecrets};
use super::sew_wire as wire;
use super::store::{self, Row};

// ── environment ──────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Env<'a> {
    pub ops: &'a dyn SewOps,
    pub secrets: &'a dyn SewSecrets,
    pub latch: &'a AtomicBool,
    pub token: &'a Mutex<Option<String>>,
}

static LATCH: AtomicBool = AtomicBool::new(false);
static TOKEN: Mutex<Option<String>> = Mutex::new(None);
static REAL: RealOps = RealOps;
static VAULT: VaultSecrets = VaultSecrets;

/// A Sew lifecycle step (enable, upgrade, rollback, restore, …) is running
/// now. The App re-host wave waits for it (update continuity R29).
pub fn lifecycle_busy() -> bool {
    LATCH.load(std::sync::atomic::Ordering::SeqCst)
}

/// A newer pinned Sew waits to install on this box: this build pins a
/// released Sew newer than the one installed (status `upgradeAvailable`).
/// The App re-host wave waits for it as for a running upgrade (update
/// continuity R29, Sew C). `None`: no Sew here, it is current, or this
/// build pins no released Sew (nothing could install it).
pub fn pin_pending() -> Option<String> {
    if !k2_root_proto::sew::sew_release_pinned() {
        return None;
    }
    k2_core::db::try_shared()?;
    pin_pending_for(store::load().as_ref(), pinned().version)
}

/// [`pin_pending`] over a row and a pin (tests).
pub fn pin_pending_for(row: Option<&Row>, pin_version: &str) -> Option<String> {
    let row = row?;
    if !matches!(row.status.as_str(), "running" | "degraded" | "stopped" | "upgrading") {
        return None;
    }
    let have = row.installed_version.as_deref()?;
    (version_cmp(have, pin_version) == Some(std::cmp::Ordering::Less))
        .then(|| format!("a pending Sew upgrade ({have} to the pinned {pin_version})"))
}

/// Production: the real box, the open vault, the process-wide latch.
pub fn live() -> Env<'static> {
    Env { ops: &REAL, secrets: &VAULT, latch: &LATCH, token: &TOKEN }
}

// ── refusals ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Fail {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
    pub extra: Option<Value>,
}

pub type R<T> = Result<T, Fail>;

pub fn no(code: &'static str, status: u16, hint: impl Into<String>) -> Fail {
    Fail { code, status, hint: hint.into(), extra: None }
}

fn with(mut f: Fail, extra: Value) -> Fail {
    f.extra = Some(extra);
    f
}

/// The lifecycle latch, released on drop.
pub struct Latch<'a>(&'a AtomicBool);

impl Drop for Latch<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub fn begin<'a>(env: &Env<'a>) -> R<Latch<'a>> {
    env.latch
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .map(|_| Latch(env.latch))
        .map_err(|_| {
            no(
                "lifecycle_in_progress",
                409,
                "another Sew step (enable, upgrade, rollback, restore, rotation, disable, uninstall) is running: \
                 wait for it (k2 hostchat status), then retry",
            )
        })
}

/// `sew_not_released` (409): this K2 release pins no published Sew
/// release yet (placeholder pins), so nothing installs.
pub fn not_released() -> Fail {
    no(pin::SEW_NOT_RELEASED, 409, pin::SEW_NOT_RELEASED_MESSAGE)
}

/// Refuse every install path (enable, preflight, upgrade, voice install)
/// before any effect while this build has no Sew release pinned.
pub fn need_released(env: &Env) -> R<()> {
    if env.ops.sew_released() {
        Ok(())
    } else {
        Err(not_released())
    }
}

fn unsupported() -> Fail {
    no("unsupported", 409, "Sew runs on Linux boxes only (Ubuntu 24.04 / Debian 12+, systemd 250 or newer)")
}

// ── the root helper ──────────────────────────────────────────────────────

fn argv(tail: &[&str]) -> Vec<String> {
    let mut v = vec!["engine".to_string(), "sew".to_string()];
    v.extend(tail.iter().map(|s| s.to_string()));
    v
}

fn helper(env: &Env, tail: &[&str], stdin: Option<&[u8]>) -> Result<Value, String> {
    env.ops.helper(&argv(tail), stdin)
}

/// Sew's exit code from an `engine sew` answer (`None` = not a Sew run).
fn sew_exit(v: &Value) -> Option<i64> {
    v.get("sewExit").and_then(Value::as_i64)
}

/// Why Sew said no, never empty: the helper's `reason` (protocol 8: Sew's
/// `--json` error message, else its stderr, else its stdout), else the
/// same fields read here (an older helper), else the bare exit code.
fn sew_message(v: &Value) -> String {
    let field = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let m = field("/reason")
        .or_else(|| field("/sew/error/message"))
        .or_else(|| field("/stderrTail"))
        .or_else(|| field("/stdoutTail"))
        .unwrap_or_else(|| match sew_exit(v) {
            Some(n) => format!("Sew exited {n} and gave no reason (helper answer: {})", short(v)),
            None => format!("the helper answered no Sew exit code: {}", short(v)),
        });
    m.chars().take(600).collect()
}

/// A bounded rendering of a helper answer for an error message.
fn short(v: &Value) -> String {
    v.to_string().chars().take(200).collect()
}

/// A Sew run that must exit 0.
fn sew_ok(env: &Env, tail: &[&str], stdin: Option<&[u8]>) -> Result<Value, String> {
    let v = helper(env, tail, stdin)?;
    match sew_exit(&v) {
        Some(0) | None => Ok(v),
        Some(n) => Err(format!("sew {}: exit {n}: {}", tail.join(" "), sew_message(&v))),
    }
}

/// `engine sew <tail>` with no stdin (voice, users).
pub fn helper_json(env: &Env, tail: &[&str]) -> Result<Value, String> {
    helper(env, tail, None)
}

/// `engine sew <tail>` with `stdin` bytes.
pub fn helper_stdin(env: &Env, tail: &[&str], stdin: &[u8]) -> Result<Value, String> {
    helper(env, tail, Some(stdin))
}

/// A Sew run through the helper that must exit 0.
pub fn sew_run(env: &Env, tail: &[&str], stdin: Option<&[u8]>) -> Result<Value, String> {
    sew_ok(env, tail, stdin)
}

/// Wait until every unit is active (`secs` one-second polls).
pub fn poll_units(env: &Env, units: &[&str], secs: u32) -> Result<(), String> {
    poll_active(env, units, secs)
}

/// Tell every client the Sew server changed (`sew:changed` hook, mirrored
/// as the app-level `sew_changed` session event). A refetch signal: the
/// reason only, never a link, a key or a token.
pub fn changed(reason: &str) {
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SewChanged, json!({"reason": reason}));
}

/// Protocol 7 AND the `sew` namespace AND this daemon's pin.
pub fn helper_check(env: &Env) -> R<()> {
    let st = env.ops.helper_state();
    let ns = st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/sew")).and_then(Value::as_bool) == Some(true);
    if !st.usable(SEW_MIN_PROTOCOL) || !ns {
        return Err(no("helper_needed", 409, crate::root_helper::helper_needed("Sew", SEW_MIN_PROTOCOL, &st)));
    }
    let theirs = st.version_json.as_ref().and_then(|v| v.get("sewPin")).and_then(Value::as_str).unwrap_or("");
    if theirs != pinned().version {
        return Err(no(
            "helper_outdated",
            409,
            format!(
                "the root helper pins Sew {theirs}, this daemon pins {}: update the helper to this daemon's version \
                 (k2 root-helper update)",
                pinned().version
            ),
        ));
    }
    Ok(())
}

fn helper_pin_matches(env: &Env) -> bool {
    helper_check(env).is_ok()
}

/// `engine sew status` (open, read-only).
fn box_status(env: &Env) -> Option<Value> {
    helper(env, &["status"], None).ok()
}

/// (present, known version) of `/usr/local/bin/sew`.
fn on_disk(st: &Value) -> (bool, Option<String>, bool) {
    let present = st.pointer("/binary/present").and_then(Value::as_bool).unwrap_or(false);
    let known = st.pointer("/binary/known").and_then(Value::as_bool).unwrap_or(false);
    let v = st.pointer("/binary/version").and_then(Value::as_str).map(str::to_string);
    (present, v, known)
}

// ── mgmt (token from the helper, cached; one refresh on 401) ─────────────

fn fetch_token(env: &Env) -> Result<String, String> {
    let v = helper(env, &["token"], None)?;
    let t = v.get("token").and_then(Value::as_str).filter(|t| pin::mgmt_token_ok(t)).ok_or("the root helper returned no usable Sew mgmt token")?;
    *env.token.lock().unwrap_or_else(|p| p.into_inner()) = Some(t.to_string());
    Ok(t.to_string())
}

/// The mgmt token for other mgmt clients (S27's grant-hook worker): the
/// cached one, or a fresh one from the helper when there is none or
/// `refresh` (Sew answered 401). Never logged, never on disk.
pub fn mgmt_token(env: &Env, refresh: bool) -> Result<String, String> {
    if !refresh {
        if let Some(t) = env.token.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return Ok(t);
        }
    }
    fetch_token(env)
}

pub fn mgmt(env: &Env, method: &str, path: &str, body: Option<&Value>) -> Result<Value, MgmtError> {
    let cached = env.token.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let token = match cached {
        Some(t) => t,
        None => fetch_token(env).map_err(MgmtError::Unreachable)?,
    };
    match env.ops.mgmt(method, path, &token, body) {
        Err(MgmtError::Unauthorized) => {
            let t = fetch_token(env).map_err(MgmtError::Unreachable)?;
            env.ops.mgmt(method, path, &t, body)
        }
        r => r,
    }
}

// ── preflight ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct EnableRequest {
    pub tarball_path: Option<std::path::PathBuf>,
    pub recovery_key_file: Option<std::path::PathBuf>,
    /// `k2` (default) or `standalone` (Sew's own password logins). Fixed
    /// for the life of an install: switching is a fresh install.
    pub provider: Option<String>,
    /// Also set up voice once chat runs (K13).
    pub voice: Option<super::voice::InstallRequest>,
}

impl EnableRequest {
    pub fn provider(&self) -> &str {
        self.provider.as_deref().unwrap_or(pin::PROVIDER_K2)
    }
}

/// The identity provider an install was made with (recorded in the enable
/// progress; `k2` for installs from before the choice existed).
pub fn row_provider(row: &Row) -> String {
    row.enable_progress
        .as_ref()
        .and_then(|p| p.get("provider"))
        .and_then(Value::as_str)
        .unwrap_or(pin::PROVIDER_K2)
        .to_string()
}

pub const GIB: u64 = 1024 * 1024 * 1024;

fn check(id: &str, status: &str, detail: impl Into<String>, hint: impl Into<String>) -> Value {
    json!({"id": id, "status": status, "detail": detail.into(), "hint": hint.into()})
}

/// Sew's `shared` profile split on this box (S6 §13): `sew` 15% (1–8 GiB),
/// `sew-db` 15% (1–16 GiB).
pub fn memory_split(total: u64) -> Value {
    let pct = total / 100 * 15;
    let sew = pct.clamp(GIB, 8 * GIB);
    let db = pct.clamp(GIB, 16 * GIB);
    json!({"totalBytes": total, "sewBytes": sew, "sewDbBytes": db})
}

fn gib(b: u64) -> String {
    format!("{:.1} GiB", b as f64 / GIB as f64)
}

fn master_key_present(env: &Env, row: Option<&Row>) -> bool {
    row.and_then(|r| r.master_key_ref.as_deref())
        .is_some_and(|r| matches!(env.secrets.resolve(r), Ok(Some(_))))
}

/// The read-only checks (`GET /cli/sew/preflight`; enable refuses on any
/// `fail`). Never changes anything.
pub fn preflight(env: &Env, req: &EnableRequest) -> Vec<Value> {
    let mut c = Vec::new();
    if !env.ops.linux() {
        c.push(check("os", "fail", "not Linux", "Sew runs on Linux boxes only"));
        return c;
    }
    c.push(check("os", "ok", "Linux", ""));
    let arch = env.ops.arch();
    if k2_root_proto::version::arch_ok(&arch) {
        c.push(check("arch", "ok", &arch, ""));
    } else {
        c.push(check("arch", "fail", &arch, "Sew ships x86_64 and aarch64"));
    }
    match env.ops.systemd_version() {
        Some(v) if v >= 250 => c.push(check("systemd", "ok", format!("systemd {v}"), "")),
        Some(v) => c.push(check(
            "systemd",
            "fail",
            format!("systemd {v}"),
            "Sew needs systemd 250 or newer (encrypted credentials): Ubuntu 24.04 or Debian 12+",
        )),
        None => c.push(check("systemd", "fail", "systemctl --version unreadable", "Sew needs systemd 250 or newer")),
    }
    // Exactly the major `sew install` is told (`pg_major`), never "any ≥ 16":
    // K2's own PGDG 17 next to it must not satisfy this check.
    let pg = env.ops.pg_majors();
    let want = pin::SEW_PG_MAJOR;
    if pg.contains(&want) {
        c.push(check("postgres", "ok", format!("PostgreSQL {want} binaries (sew-db)"), ""));
    } else {
        c.push(check(
            "postgres",
            "fail",
            if pg.is_empty() {
                "no PostgreSQL server binaries under /usr/lib/postgresql".to_string()
            } else {
                format!("PostgreSQL {pg:?} found, not {want}")
            },
            format!(
                "Sew needs the postgresql-{want} package (/usr/lib/postgresql/{want}/bin/postgres, initdb, pg_dump, \
                 pg_restore) without a second cluster: as root, `k2-root-helper pg add-major {want}` uses K2's pinned \
                 apt.postgresql.org source and never creates a cluster (older helpers: install postgresql-{want} from \
                 apt.postgresql.org on Ubuntu 22.04 and Debian 12). Keep it installed while Sew is; the daemon never runs apt"
            ),
        ));
    }
    let helper_ok = match helper_check(env) {
        Ok(()) => {
            c.push(check("helper", "ok", format!("k2-root-helper protocol {}", env.ops.helper_state().protocol), ""));
            true
        }
        Err(f) => {
            c.push(check("helper", "fail", f.code, f.hint));
            false
        }
    };
    match pinned_artifact(&arch) {
        Ok(_) => c.push(check("pin", "ok", format!("Sew {}", pinned().version), "")),
        Err(e) => c.push(check("pin", "fail", e, "this K2 build has no Sew release pinned yet (Sew SW1)")),
    }
    let st = if helper_ok { box_status(env) } else { None };
    let (present, version, known) = st.as_ref().map(on_disk).unwrap_or((false, None, false));
    let pin_v = pinned().version;
    if present && !known {
        // The helper's own words when it refused the file itself (owner,
        // mode, not a regular file), else an unknown sha.
        match st.as_ref().and_then(|s| s.pointer("/binary/error")).and_then(Value::as_str) {
            Some(why) => c.push(check(
                "binary",
                "fail",
                why.trim_start_matches("sew helper: ").to_string(),
                "Sew's binary must be root:root and not group/other-writable: reinstall it from a pinned release",
            )),
            None => c.push(check("binary", "fail", "/usr/local/bin/sew is not a Sew release this K2 knows", "remove it or reinstall from a pinned release")),
        }
    } else if let Some(v) = &version {
        if v == pin_v {
            c.push(check("binary", "ok", format!("Sew {v} installed"), ""));
        } else {
            c.push(check("binary", "fail", format!("Sew {v} installed, the pin is {pin_v}"), "enable installs nothing over it: k2 hostchat upgrade"));
        }
    } else {
        c.push(check("binary", "ok", "not installed", ""));
    }
    let have_bin = version.as_deref() == Some(pin_v);
    match (&req.tarball_path, have_bin) {
        (_, true) => c.push(check("source", "ok", "the pinned binary is already installed", "")),
        (Some(p), _) => c.push(check("source", "ok", format!("--tarball {}", p.display()), "")),
        (None, false) => c.push(check(
            "source",
            "fail",
            "no Sew release source",
            "Sew has not published a pinned release yet (SW1): k2 hostchat enable --tarball <sew-<v>-<triple>.tar.gz>",
        )),
    }
    match env.ops.mem_total() {
        Some(m) if m < 4 * GIB => c.push(check("ram", "fail", gib(m), "Sew on a K2 box needs at least 4 GiB of RAM")),
        Some(m) => {
            let split = memory_split(m);
            let detail = format!(
                "{} total; sew {}, sew-db {}",
                gib(m),
                gib(split["sewBytes"].as_u64().unwrap_or(0)),
                gib(split["sewDbBytes"].as_u64().unwrap_or(0))
            );
            if m < 8 * GIB {
                c.push(check("ram", "warn", detail, "under 8 GiB the 1 GiB floors give Sew a large share of the box"));
            } else {
                c.push(check("ram", "ok", detail, ""));
            }
        }
        None => c.push(check("ram", "warn", "MemTotal unreadable", "")),
    }
    match env.ops.disk_free("/var/lib") {
        Some(f) if f < 10 * GIB => c.push(check("disk", "fail", format!("{} free on /var/lib", gib(f)), "Sew needs 10 GiB free on /var/lib")),
        Some(f) if f < 20 * GIB => c.push(check("disk", "warn", format!("{} free on /var/lib", gib(f)), "20 GiB or more is safer")),
        Some(f) => c.push(check("disk", "ok", format!("{} free on /var/lib", gib(f)), "")),
        None => c.push(check("disk", "warn", "statvfs(/var/lib) failed", "")),
    }
    let sew_active = env.ops.unit(UNIT_SEW).is_active();
    if !sew_active && env.ops.port_taken("127.0.0.1", SEW_CLIENT_PORT, false) {
        c.push(check("port", "fail", format!("127.0.0.1:{SEW_CLIENT_PORT} is taken"), "Sew's client API listens there"));
    } else {
        c.push(check("port", "ok", format!("127.0.0.1:{SEW_CLIENT_PORT}"), ""));
    }
    // Voice (K13): reported now so the owner sees it before turning voice
    // on; never a hard stop for chat.
    let media_taken: Vec<String> = [
        ("127.0.0.1", SEW_MEDIA_LOOPBACK_PORT, false),
        ("0.0.0.0", SEW_MEDIA_TCP_PORT, false),
        ("0.0.0.0", SEW_MEDIA_UDP_PORT, true),
        ("0.0.0.0", SEW_TURN_TLS_PORT, false),
    ]
    .iter()
    .filter(|(a, p, u)| env.ops.port_taken(a, *p, *u))
    .map(|(_, p, u)| format!("{} {p}", if *u { "UDP" } else { "TCP" }))
    .collect();
    if media_taken.is_empty() {
        c.push(check("voice_ports", "ok", "TCP 7880 (loopback), TCP 7881, UDP 7882, TCP 5349 free", "open 7881/tcp, 7882/udp and 5349/tcp in the host and provider firewalls for voice"));
    } else {
        c.push(check("voice_ports", "warn", format!("taken: {}", media_taken.join(", ")), "voice needs these ports; chat does not"));
    }
    if env.ops.port_taken("0.0.0.0", 443, false) {
        c.push(check("turn_443", "info", ":443 is taken on this box (Stalwart or Caddy)", "TURN on 443 is not offered here; TURN-TLS uses 5349"));
    } else {
        c.push(check("turn_443", "info", ":443 is free", "TURN on 443 could be offered (voice)"));
    }
    // Sew adopts an existing `sew` user as its service account: never a
    // login account (sewlab 2026-10-16). Sew's own account passes.
    c.push(super::service_user::preflight_check(env.ops));
    let row = store::load();
    let has_key = master_key_present(env, row.as_ref());
    let data = env.ops.path_exists(SEW_DATA_DIR) || env.ops.path_exists(SEW_CONFIG_DIR);
    if data && !has_key && req.recovery_key_file.is_none() {
        c.push(check(
            "existing_data",
            "fail",
            "Sew data or config is on this box and the K2 vault holds no Sew master key",
            "give the recovery key: k2 hostchat enable --recovery-key-file <file>",
        ));
    } else {
        c.push(check("existing_data", "ok", if data { "existing Sew data will be adopted" } else { "no Sew data" }, ""));
    }
    match env.ops.daemon_port() {
        Some(p) => c.push(check("daemon_port", "ok", format!("JWKS at http://127.0.0.1:{p}/v1/chat/jwks"), "")),
        None => c.push(check("daemon_port", "fail", "the daemon's HTTP port is unknown", "retry once the daemon is ready")),
    }
    // A read probe: never writes the vault.
    if !has_key && env.secrets.resolve("sewsec_probe_000000000000").is_err() {
        c.push(check("vault", "fail", "the K2 vault isn't open", "Sew's master key lives in the vault: open it first (k2 vault status)"));
    } else if has_key {
        c.push(check("vault", "ok", "the K2 vault holds Sew's master key", ""));
    } else {
        c.push(check("vault", "ok", "the K2 vault is open (enable stores a new Sew master key there)", ""));
    }
    c
}

fn failed(checks: &[Value]) -> bool {
    checks.iter().any(|c| c["status"] == "fail")
}

/// The enable refusal's hint: the failing checks in words, so `k2 hostchat
/// enable` says why without the report.
fn refusal_hint(checks: &[Value]) -> String {
    let why: Vec<String> = checks
        .iter()
        .filter(|c| c["status"] == "fail")
        .map(|c| {
            let d = c["detail"].as_str().unwrap_or_default().trim_end_matches('.');
            match c["hint"].as_str().unwrap_or_default() {
                "" => format!("{d}."),
                h => format!("{d}. {h}"),
            }
        })
        .collect();
    format!("the box is not ready for Sew; nothing was changed. {}", why.join(" "))
}

// ── progress ─────────────────────────────────────────────────────────────

fn step_done(p: &Option<Value>, step: &str) -> bool {
    p.as_ref().and_then(|p| p.pointer(&format!("/steps/{step}"))).is_some()
}

fn mark(p: &mut Option<Value>, step: &str, now: i64) {
    let v = p.get_or_insert_with(|| json!({"steps": {}, "current": null}));
    if !v["steps"].is_object() {
        v["steps"] = json!({});
    }
    v["steps"][step] = json!({"at": now});
}

fn set_current(p: &mut Option<Value>, step: Option<&str>) {
    let v = p.get_or_insert_with(|| json!({"steps": {}, "current": null}));
    v["current"] = json!(step);
}

fn save_enable(env: &Env, f: impl FnOnce(&mut Row)) {
    if let Err(e) = store::update(env.ops.now(), f) {
        k2_core::log_debug!("[sew] {e}");
    }
}

// ── enable ───────────────────────────────────────────────────────────────

/// The synchronous part of `POST /cli/sew/server/enable`: already-healthy
/// no-op, latch, preflight, row. `Ok(Err(body))` = answered without a
/// machine (already enabled).
pub fn enable_begin<'a>(env: &Env<'a>, req: &EnableRequest) -> R<Result<Latch<'a>, Value>> {
    need_released(env)?;
    if !env.ops.linux() {
        return Err(unsupported());
    }
    if let Some(r) = store::load() {
        if matches!(r.status.as_str(), "running" | "degraded")
            && r.installed_version.as_deref() == Some(pinned().version)
            && env.ops.unit(UNIT_DB).is_active()
            && env.ops.unit(UNIT_SEW).is_active()
        {
            return Ok(Err(json!({"ok": true, "alreadyEnabled": true, "state": r.status})));
        }
        if r.status == "upgrading" {
            return Err(no("upgrade_in_progress", 409, "a Sew upgrade is recorded as running: wait for it or check k2 hostchat status"));
        }
        let had = row_provider(&r);
        let installed_once = r.install_id.is_some() || step_done(&r.enable_progress, "install");
        if installed_once && had != req.provider() {
            return Err(no(
                "provider_fixed",
                409,
                format!(
                    "this Sew install uses identity '{had}'; it can only change with a fresh install \
                     (k2 hostchat purge --confirm-hostname <host>, then k2 hostchat enable --identity {})",
                    req.provider()
                ),
            ));
        }
    }
    if !matches!(req.provider(), pin::PROVIDER_K2 | pin::PROVIDER_STANDALONE) {
        return Err(no("bad_request", 400, "identity is k2 or standalone"));
    }
    let latch = begin(env)?;
    let checks = preflight(env, req);
    if failed(&checks) {
        return Err(with(no("preflight_failed", 409, refusal_hint(&checks)), json!({"report": checks})));
    }
    let now = env.ops.now();
    let mut row = store::load().unwrap_or_else(|| Row::new(pinned().version, now));
    row.status = "installing".into();
    row.pinned_version = pinned().version.into();
    row.last_error = None;
    row.updated_at = now;
    set_current(&mut row.enable_progress, Some("master-key"));
    if let Some(p) = row.enable_progress.as_mut() {
        p["provider"] = json!(req.provider());
    }
    store::save(&row).map_err(|e| no("db_error", 500, e))?;
    Ok(Ok(latch))
}

/// The enable machine (S29 §11.3). Records progress; any failure leaves
/// `status = error` with `last_error = "<step>: <why>"`; re-POST resumes.
pub fn enable_machine(env: &Env, req: &EnableRequest) -> Result<(), String> {
    let res = enable_steps(env, req);
    if let Err((step, e)) = &res {
        let msg = format!("{step}: {e}");
        save_enable(env, |r| {
            r.status = "error".into();
            r.last_error = Some(msg.clone());
        });
        changed("enable-failed");
        return Err(msg);
    }
    changed("enabled");
    Ok(())
}

type StepErr = (&'static str, String);

fn step(env: &Env, name: &'static str) {
    save_enable(env, |r| set_current(&mut r.enable_progress, Some(name)));
}

fn done(env: &Env, name: &'static str) {
    let now = env.ops.now();
    save_enable(env, |r| mark(&mut r.enable_progress, name, now));
}

fn poll_active(env: &Env, units: &[&str], secs: u32) -> Result<(), String> {
    for _ in 0..secs {
        let infos: Vec<UnitInfo> = units.iter().map(|u| env.ops.unit(u)).collect();
        if infos.iter().all(UnitInfo::is_active) {
            return Ok(());
        }
        if let Some((u, i)) = units.iter().zip(&infos).find(|(_, i)| i.active == "failed") {
            if i.exec_main_status == 78 {
                return Err(format!("{u} refused its config or credentials (exit 78): journalctl -u {u}"));
            }
            return Err(format!("{u} failed ({}): journalctl -u {u}", if i.result.is_empty() { "failed" } else { &i.result }));
        }
        env.ops.sleep_ms(1000);
    }
    Err(format!("{} not active after {secs} s", units.join(" + ")))
}

/// Read and verify the tarball the owner named; stage it through the helper.
fn stage_tarball(env: &Env, path: Option<&std::path::Path>) -> Result<(), String> {
    let p = path.ok_or("no Sew release source: pass --tarball (Sew SW1 has not published a pinned release)")?;
    if !p.is_absolute() {
        return Err("--tarball must be an absolute path".into());
    }
    let bytes = env.ops.read_file(p, MAX_TARBALL)?;
    let art = pinned_artifact(&env.ops.arch())?;
    if !crate::update_routes::verify_sha256(&bytes, art.tarball_sha256) {
        return Err(format!("sha256 mismatch: {} is not Sew {} for {} — NOT installing", p.display(), pinned().version, art.triple));
    }
    helper(env, &["stage"], Some(&bytes)).map(|_| ())
}

fn enable_steps(env: &Env, req: &EnableRequest) -> Result<(), StepErr> {
    let pin_v = pinned().version;
    let row = store::load().ok_or(("row", "the sew_server row vanished".to_string()))?;
    let marked = |s: &str| step_done(&row.enable_progress, s);

    // 1. master-key (marked): keep the vault's, adopt a recovery key, or make one.
    step(env, "master-key");
    if !marked("master-key") || !master_key_present(env, Some(&row)) {
        let existing = row.master_key_ref.as_deref().and_then(|r| env.secrets.resolve(r).ok().flatten());
        let from_file = match &req.recovery_key_file {
            Some(p) => {
                let raw = env.ops.read_file(p, 64).map_err(|e| ("master-key", e))?;
                let text = String::from_utf8(raw).map_err(|_| ("master-key", "the recovery key file is not text".to_string()))?;
                let text = text.strip_suffix('\n').unwrap_or(&text).to_string();
                if !pin::master_key_text_ok(&text) {
                    return Err(("master-key", "the recovery key file does not hold a Sew recovery key (43 base64url characters)".into()));
                }
                Some(text)
            }
            None => None,
        };
        let key_ref = match (existing, from_file) {
            (Some(k), Some(f)) if k != f => {
                return Err(("master-key", "the vault already holds a different Sew master key; K2 will not replace it".into()))
            }
            (Some(_), _) => row.master_key_ref.clone().expect("resolved above"),
            (None, Some(f)) => env.secrets.store("master", &f).map_err(|e| ("master-key", e))?,
            (None, None) => {
                let k = super::secrets::generate_master_key().map_err(|e| ("master-key", e))?;
                env.secrets.store("master", &k).map_err(|e| ("master-key", e))?
            }
        };
        let now = env.ops.now();
        let fresh = row.master_key_ref.as_deref() != Some(key_ref.as_str());
        save_enable(env, |r| {
            if fresh {
                r.master_key_created_at = Some(now);
                r.recovery_shown_at = None;
            }
            r.master_key_ref = Some(key_ref.clone());
        });
    }
    done(env, "master-key");
    let row = store::load().ok_or(("row", "the sew_server row vanished".to_string()))?;
    let key = row
        .master_key_ref
        .as_deref()
        .and_then(|r| env.secrets.resolve(r).ok().flatten())
        .ok_or(("master-key", "the vault lost Sew's master key".to_string()))?;

    // 2–3. stage + install-bin (marked; skipped when the pin is on disk).
    let st = box_status(env).ok_or(("stage", "engine sew status failed".to_string()))?;
    let (_, on_disk_v, _) = on_disk(&st);
    if on_disk_v.as_deref() != Some(pin_v) {
        step(env, "stage");
        stage_tarball(env, req.tarball_path.as_deref()).map_err(|e| ("stage", e))?;
        done(env, "stage");
        step(env, "install-bin");
        helper(env, &["install-bin"], None).map_err(|e| ("install-bin", e))?;
    } else {
        done(env, "stage");
    }
    done(env, "install-bin");

    // 4. install (marked): Sew writes its config, units and credential.
    let mut allow_changed = false;
    if !marked("install") {
        step(env, "install");
        let port = env.ops.daemon_port().ok_or(("install", "the daemon's HTTP port is unknown".to_string()))?;
        let body = if req.provider() == pin::PROVIDER_STANDALONE {
            json!({"provider": pin::PROVIDER_STANDALONE, "masterKey": key})
        } else {
            json!({"jwksUrl": format!("http://127.0.0.1:{port}/v1/chat/jwks"), "masterKey": key})
        };
        let out = helper(env, &["install"], Some(body.to_string().as_bytes())).map_err(|e| ("install", e))?;
        if let Some(n) = sew_exit(&out).filter(|n| *n != 0) {
            return Err(("install", format!("exit {n}: {}", sew_message(&out))));
        }
        let installed = out.pointer("/sew/versions/sew").and_then(Value::as_str).unwrap_or(pin_v);
        if installed.split('+').next() != Some(pin_v) {
            return Err(("install", format!("Sew reports version {installed}, the pin is {pin_v}")));
        }
        // SW5: Sew wrote the daemon's uid to [mgmt] allow_uids itself; a
        // sew.service that was already up must restart to read it.
        allow_changed = out.pointer("/allowUid/restartNeeded").and_then(Value::as_bool).unwrap_or(false);
        done(env, "install");
    }

    // 5+. systemd and the links: EVERY enable.
    step(env, "enable-units");
    helper(env, &["systemctl", "enable"], None).map_err(|e| ("enable-units", e))?;
    done(env, "enable-units");

    step(env, "db");
    helper(env, &["systemctl", "start-db"], None).map_err(|e| ("db", e))?;
    poll_active(env, &[UNIT_DB], 120).map_err(|e| ("db", e))?;
    done(env, "db");

    step(env, "migrate");
    helper(env, &["systemctl", "migrate"], None).map_err(|e| ("migrate", e))?;
    let m = env.ops.unit(UNIT_MIGRATE);
    if m.exec_main_status == 78 {
        return Err(("migrate", "the database schema is newer than this binary or a migration checksum changed (exit 78)".into()));
    }
    if m.active == "failed" {
        return Err(("migrate", "sew-migrate.service failed: journalctl -u sew-migrate.service".into()));
    }
    done(env, "migrate");

    step(env, "start");
    helper(env, &["systemctl", "start"], None).map_err(|e| ("start", e))?;
    if allow_changed {
        // SW7: Sew reads [mgmt] allow_uids at start; a sew that was already
        // running must restart to let the daemon onto the socket.
        helper(env, &["systemctl", "restart"], None).map_err(|e| ("start", e))?;
    }
    poll_active(env, &[UNIT_DB, UNIT_SEW], 120).map_err(|e| ("start", e))?;
    done(env, "start");

    step(env, "token");
    fetch_token(env).map_err(|e| ("token", e))?;
    done(env, "token");

    step(env, "health");
    wait_health(env, 30).map_err(|e| ("health", e))?;
    done(env, "health");

    step(env, "link");
    let status = mgmt(env, "GET", "/mgmt/v1/status", None).map_err(|e| ("link", e.to_string()))?;
    let install_id = link_check(&status, pin_v, &key, req.provider()).map_err(|e| ("link", e))?;
    done(env, "link");

    let _ = helper(env, &["unstage"], None);
    let now = env.ops.now();
    save_enable(env, |r| {
        r.status = "running".into();
        r.installed_version = Some(pin_v.into());
        r.install_id = Some(install_id.clone());
        r.installed_at = r.installed_at.or(Some(now));
        r.last_error = None;
        set_current(&mut r.enable_progress, None);
        if let Some(p) = r.enable_progress.as_mut() {
            p["result"] = json!("done");
        }
    });
    // S27: the grant hook now points at this install; `k2:owner` and the
    // chat grants provision through its outbox. Recorded, never a failure.
    if let Some(row) = store::load() {
        let hook = grant_hook_on(env, &row);
        save_enable(env, |r| {
            if let Some(p) = r.enable_progress.as_mut() {
                p["grantHook"] = hook.clone();
                p["owner"] = json!(if hook["target"] == "on" { "queued" } else { "pending" });
            }
        });
    }
    Ok(())
}

/// The link checks (S29 §11.3 step 11): provider k2, version = pin, an
/// install id, and the KEK id Sew reports = the vault key's.
pub fn link_check(status: &Value, pin_v: &str, key: &str, want_provider: &str) -> Result<String, String> {
    let provider = status.get("provider").and_then(Value::as_str).unwrap_or("");
    if provider == "standalone" && want_provider != "standalone" {
        return Err("a standalone Sew install is on this box; K2 will not adopt it".into());
    }
    if provider != want_provider {
        return Err(format!("Sew reports provider '{provider}', expected {want_provider}"));
    }
    let v = status.get("version").and_then(Value::as_str).unwrap_or("");
    if v.split('+').next() != Some(pin_v) {
        return Err(format!("Sew reports version {v}, the pin is {pin_v}"));
    }
    let id = status
        .get("install_id")
        .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
        .filter(|s| s != "none" && pin::install_id_ok(s))
        .ok_or("Sew reports no install id")?;
    if let Some(kek) = status.pointer("/keys/kek_id").and_then(Value::as_str) {
        if kek != kek_id(key)? {
            return Err("Sew's KEK id does not match the master key in the K2 vault (k2_vault_key_mismatch)".into());
        }
    }
    Ok(id)
}

fn wait_health(env: &Env, tries: u32) -> Result<(), String> {
    let mut last = String::new();
    for i in 0..tries {
        match mgmt(env, "GET", "/mgmt/v1/health", None) {
            Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => return Ok(()),
            Ok(v) => last = format!("health answered {v}"),
            Err(e) => last = e.to_string(),
        }
        if i + 1 < tries {
            env.ops.sleep_ms(1000);
        }
    }
    Err(format!("Sew never reported healthy: {last}"))
}

/// Units active + mgmt healthy + Sew's version = `want`.
fn health_check(env: &Env, want: &str) -> Result<Value, String> {
    poll_active(env, &[UNIT_DB, UNIT_SEW], 60)?;
    *env.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
    wait_health(env, 30)?;
    let st = mgmt(env, "GET", "/mgmt/v1/status", None).map_err(|e| e.to_string())?;
    let v = st.get("version").and_then(Value::as_str).unwrap_or("");
    if v.split('+').next() != Some(want) {
        return Err(format!("Sew reports version {v}, expected {want}"));
    }
    Ok(st)
}

// ── status ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    pub state: String,
    pub consistent: bool,
    pub note: Option<String>,
}

/// S29 §12.2: never `running`/`degraded` unless both units are active.
/// `verdict` is `running`/`degraded` from §12.3 (used only when both are
/// active and the row says the server should be up).
pub fn reconcile(row: &str, db: &UnitInfo, sew: &UnitInfo, verdict: &str) -> Reconciled {
    let both = db.is_active() && sew.is_active();
    let r = |state: &str, consistent: bool, note: Option<String>| Reconciled { state: state.into(), consistent, note };
    match row {
        "installing" | "upgrading" | "removed" => r(row, true, None),
        "disabled" => {
            if both {
                r("disabled", false, Some("systemd reports sew active while disabled".into()))
            } else {
                r("disabled", true, None)
            }
        }
        // A `stopped` row whose units came back (systemd restarted Sew)
        // takes the verdict: the health loop moves across running /
        // degraded / stopped (S29 §12.4).
        "running" | "degraded" | "stopped" => {
            if both {
                r(verdict, true, None)
            } else if row == "stopped" {
                r("stopped", true, None)
            } else if !db.is_active() {
                let st = if db.active == "failed" { "error" } else { "stopped" };
                r(st, false, Some(format!("systemd reports sew-db.service is '{}'", db.active)))
            } else if sew.active == "failed" && sew.exec_main_status == 78 {
                r("error", false, Some("sew.service refused its config or credentials (exit 78)".into()))
            } else {
                let st = if sew.active == "failed" { "error" } else { "stopped" };
                r(st, false, Some(format!("systemd reports sew.service is '{}'", sew.active)))
            }
        }
        // error: only a person's enable clears it.
        other => {
            if both {
                r(other, false, Some("systemd reports Sew running; the record says otherwise".into()))
            } else {
                r(other, true, None)
            }
        }
    }
}

/// K2-side alarms (prefix `k2_`, S29 §12.3).
pub fn k2_alarms(env: &Env, row: &Row, sew_status: Option<&Value>, now: i64) -> Vec<String> {
    let mut a = Vec::new();
    if row.recovery_shown_at.is_none() && row.master_key_ref.is_some() {
        a.push("k2_recovery_key_not_saved".into());
    }
    if row.master_key_created_at.is_some_and(|t| now - t > 180 * 86_400) {
        a.push("k2_master_key_due".into());
    }
    if let (Some(kek), Some(r)) = (sew_status.and_then(|s| s.pointer("/keys/kek_id")).and_then(Value::as_str), row.master_key_ref.as_deref()) {
        let ours = env.secrets.resolve(r).ok().flatten().and_then(|k| kek_id(&k).ok());
        if ours.as_deref() != Some(kek) {
            a.push("k2_vault_key_mismatch".into());
        }
    }
    if !helper_pin_matches(env) {
        a.push("k2_helper_pin_mismatch".into());
    }
    if env.ops.record_load(JWKS_RECORD).is_some() {
        a.push(ALARM_JWKS_DRIFT.into());
    }
    if row.ship_kind == "none" {
        a.push("k2_backup_offbox_none".into());
    }
    if row.upgrade_progress.as_ref().and_then(|p| p.get("result")).and_then(Value::as_str) == Some("rollback_failed") {
        a.push("k2_upgrade_rollback_failed".into());
    }
    a
}

/// `status.backup`: K2's off-box ship setting plus Sew's own backup state
/// (SW15 `status.backup`; null fields while Sew is down or has none).
fn backup_status(row: &Row, sew_status: Option<&Value>) -> Value {
    let b: wire::StatusBackup = sew_status
        .and_then(|s| s.get("backup"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    json!({
        "ship": {"kind": row.ship_kind},
        "lastOkAt": b.last_ok_at,
        "lastId": b.last_id,
        "lastKind": b.last_kind,
        "ageMs": b.age_ms,
        "lastError": b.last_error,
        "lastErrorAt": b.last_error_at,
        "alarms": sew_alarms(sew_status, &["backup_failed", "backup_stale"]),
    })
}

/// `status.drill`: Sew's restore-drill state (SW15 `status.drill`).
fn drill_status(sew_status: Option<&Value>) -> Value {
    let d: wire::StatusDrill = sew_status
        .and_then(|s| s.get("drill"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    json!({
        "lastOkAt": d.last_ok_at,
        "lastOk": d.last_ok,
        "lastError": d.last_error,
        "ageMs": d.age_ms,
        "alarms": sew_alarms(sew_status, &["drill_failed", "drill_stale"]),
    })
}

/// Sew's alarm for a keyring mirror that is stale or missing.
pub const ALARM_KEYS_MIRROR_STALE: &str = "keys_mirror_stale";

/// Setup warnings: Sew alarms that describe a choice not made yet rather
/// than something broken, so they never make the verdict `degraded`.
/// Today one: `keys_mirror_stale` while `keys.mirror.kind` is `none`
/// (Sew, intended: the keyring is never inside bundles, so with no
/// off-box key copy the bundles are unreadable after a disk loss). A dir
/// mirror that went stale (kind != none) stays an alarm. Removes the
/// warned alarms from `alarms` and returns `[{code, alarm, message}]`.
pub fn split_setup_warnings(alarms: &mut Vec<String>, sew_status: Option<&Value>) -> Vec<Value> {
    let kind = sew_status.and_then(|s| s.pointer("/keys/mirror/kind")).and_then(Value::as_str);
    let mut w = Vec::new();
    if kind == Some("none") && alarms.iter().any(|a| a == ALARM_KEYS_MIRROR_STALE) {
        alarms.retain(|a| a != ALARM_KEYS_MIRROR_STALE);
        w.push(json!({
            "code": "no_offbox_key_copy",
            "alarm": ALARM_KEYS_MIRROR_STALE,
            "message": "no off-box key copy: backups can't be read after a disk loss without one; \
                        set a keyring mirror (keys.mirror) or keep the recovery key off the box",
        }));
    }
    w
}

/// The subset of Sew's alarms named in `which`.
fn sew_alarms(sew_status: Option<&Value>, which: &[&str]) -> Vec<String> {
    debug_assert!(which.iter().all(|w| wire::BACKUP_ALARMS.contains(w)));
    sew_status
        .and_then(|s| s.get("alarms"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|x| which.contains(x)).map(str::to_string).collect())
        .unwrap_or_default()
}

/// What a purge must be confirmed with (K2's lean on Rosson's open
/// question, 2026-10-15): Sew's public host — the S29 §18 chat host, else
/// the host of the `server.public_url` K2 set (`k2 hostchat config`), the
/// name people reach Sew by — else the box's hostname.
pub fn purge_hostname(env: &Env, row: &Row) -> String {
    row.host
        .clone()
        .filter(|h| !h.is_empty())
        .or_else(|| public_url(env).and_then(|u| url_host_of(&u)))
        .unwrap_or_else(|| env.ops.hostname())
}

/// `https://host[:port][/…]` → `host` (lowercase); `None` when empty.
fn url_host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let host = host.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// `GET /cli/sew/status[?health=1]`. `ok` is the envelope.
/// `GET /cli/sew/status`. `released: false` (with `notice`) while this
/// build pins no published Sew release: `k2 hostchat status` says so
/// plainly instead of an error.
pub fn status(env: &Env, with_health: bool) -> Value {
    let mut body = status_body(env, with_health);
    // The meet label binds without Sew installed (Meet can run standalone).
    body["meet"] = super::meet::status(env);
    let released = env.ops.sew_released();
    body["released"] = json!(released);
    if !released {
        body["notice"] = json!(pin::SEW_NOT_RELEASED_MESSAGE);
    }
    body
}

fn status_body(env: &Env, with_health: bool) -> Value {
    if !env.ops.linux() {
        return json!({"ok": true, "supported": false, "state": "not-installed", "pinnedVersion": pinned().version});
    }
    let helper_json = {
        let st = env.ops.helper_state();
        json!({"protocol": st.protocol, "sew": helper_check(env).is_ok(), "error": helper_check(env).err().map(|f| f.code)})
    };
    let Some(row) = store::load() else {
        return json!({
            "ok": true, "supported": true, "state": "not-installed", "consistent": true,
            "pinnedVersion": pinned().version, "pinPlaceholder": pinned_artifact(&env.ops.arch()).is_err(),
            "helper": helper_json,
        });
    };
    let now = env.ops.now();
    let db = env.ops.unit(UNIT_DB);
    let sew = env.ops.unit(UNIT_SEW);
    // A `stopped` row whose units came back is judged like a running one
    // (the health loop then records running / degraded).
    let up = matches!(row.status.as_str(), "running" | "degraded" | "stopped") && db.is_active() && sew.is_active();
    let (sew_status, health_err) = if up {
        match mgmt(env, "GET", "/mgmt/v1/status", None) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };
    let health_ok = up && health_err.is_none() && mgmt(env, "GET", "/mgmt/v1/health", None).is_ok_and(|v| v["ok"] == true);
    let mut alarms: Vec<String> = sew_status
        .as_ref()
        .and_then(|s| s.get("alarms"))
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect())
        .unwrap_or_default();
    let warnings = split_setup_warnings(&mut alarms, sew_status.as_ref());
    if up && (health_err.is_some() || !health_ok) {
        alarms.push("k2_mgmt_unreachable".into());
    }
    alarms.extend(k2_alarms(env, &row, sew_status.as_ref(), now));
    let mut warnings = warnings;
    let public_url = public_url(env);
    if public_url.is_none() {
        warnings.push(public_url_warning(env));
    }
    let (voice, voice_alarms, voice_warnings) = if up { super::voice::status(env, with_health) } else { super::voice::status_offline(env) };
    alarms.extend(voice_alarms);
    warnings.extend(voice_warnings);
    let db_ok = sew_status.as_ref().and_then(|s| s.pointer("/db/ok")).and_then(Value::as_bool) != Some(false);
    let verdict = if health_ok && db_ok && alarms.is_empty() { "running" } else { "degraded" };
    let rec = reconcile(&row.status, &db, &sew, verdict);
    let installed = row.installed_version.clone();
    let upgrade_available = installed.as_deref().and_then(|v| version_cmp(v, pinned().version)) == Some(std::cmp::Ordering::Less);
    let mut body = json!({
        "ok": true,
        "supported": true,
        "state": rec.state,
        "consistent": rec.consistent,
        "note": rec.note,
        "pinnedVersion": pinned().version,
        "installedVersion": installed,
        "previousVersion": row.previous_version,
        "upgradeAvailable": upgrade_available,
        "installId": row.install_id,
        "provider": sew_status.as_ref().and_then(|s| s.get("provider")).cloned(),
        "identityProvider": row_provider(&row),
        "voice": voice,
        "units": {"sewDb": db.json(), "sew": sew.json()},
        "alarms": alarms,
        "warnings": warnings,
        "recoveryKeySaved": row.recovery_shown_at.is_some(),
        "masterKeyAgeDays": row.master_key_created_at.map(|t| (now - t) / 86_400),
        "rotationInProgress": row.master_prev_ref.is_some(),
        "backup": backup_status(&row, sew_status.as_ref()),
        "drill": drill_status(sew_status.as_ref()),
        "host": {"hostname": row.host, "mode": row.host_mode},
        "appUrl": row.app_url,
        "publicUrl": public_url,
        "helper": helper_json,
        "enableProgress": row.enable_progress,
        "upgradeProgress": row.upgrade_progress,
        "restoreProgress": row.restore_progress,
        "rotateProgress": row.rotate_progress,
        "lastError": row.last_error,
        "purgeConfirmHostname": purge_hostname(env, &row),
    });
    if with_health {
        let disk = box_status(env);
        body["onDiskVersion"] = disk.as_ref().and_then(|s| on_disk(s).1).map(Value::from).unwrap_or(Value::Null);
        body["health"] = json!({
            "state": rec.state,
            "mgmt": if health_ok { "ok" } else if up { "unreachable" } else { "down" },
            "detail": health_err,
            "sewStatus": sew_status,
        });
    }
    body
}

/// One health-loop tick: persist transitions among running / degraded /
/// stopped only (never over installing, disabled, error, upgrading,
/// removed). Returns the new state when it changed.
pub fn health_tick(env: &Env) -> Option<String> {
    if env.latch.load(Ordering::SeqCst) {
        return None; // never fight a lifecycle step
    }
    let row = store::load()?;
    if !matches!(row.status.as_str(), "running" | "degraded" | "stopped") {
        return None;
    }
    let s = status(env, false);
    let new = s["state"].as_str()?.to_string();
    if new == row.status || !matches!(new.as_str(), "running" | "degraded" | "stopped") {
        return None;
    }
    store::update(env.ops.now(), |r| r.status = new.clone()).ok()?;
    changed("state");
    Some(new)
}

// ── boot: JWKS URL drift + install id (S29 §19 steps 2–3) ──────────────

/// K2's record of a JWKS drift it could not fix (`k2_jwks_url_drift`).
pub const JWKS_RECORD: &str = "jwks";
/// The alarm while Sew verifies chat passes against an old JWKS URL.
pub const ALARM_JWKS_DRIFT: &str = "k2_jwks_url_drift";

/// The JWKS URL Sew must use on a K2-identity install (S28: the daemon's
/// own loopback port; the same string `enable` passes to `sew install`).
pub fn jwks_url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/chat/jwks")
}

/// Whether a [`jwks_drift_check`] answer is final for this daemon process
/// (the port only changes with a restart). The retry results (Sew down,
/// port unknown, a lifecycle step running, mgmt unreachable) are not.
pub fn jwks_settled(v: &Value) -> bool {
    matches!(
        v["result"].as_str(),
        Some("match" | "fixed" | "failed" | "unreported" | "not_applicable")
    )
}

fn jwks_drift_failed(env: &Env, have: &str, want: &str, why: String) -> Value {
    let rec = json!({"have": have, "want": want, "error": why, "at": env.ops.now()});
    if let Err(e) = env.ops.record_save(JWKS_RECORD, Some(&rec)) {
        k2_core::log_debug!("[sew] jwks record: {e}");
    }
    changed("jwks");
    json!({"result": "failed", "have": have, "want": want, "error": why})
}

/// S29 §19 boot task: a K2-identity install verifies chat passes against
/// `http://127.0.0.1:<daemon port>/v1/chat/jwks`. When the daemon came back
/// on another port, Sew still asks the old one and every pass fails. Read
/// Sew's `status.identity.jwks_url`; if it differs, set
/// `identity.k2.jwks_url` through the helper's `config` verb, restart
/// `sew`, wait for health and read it back. A fix that does not take is
/// the `k2_jwks_url_drift` alarm until a later check matches. Also stores
/// Sew's install id when it changed (a restore from another box's bundle),
/// so person confirms bind to the live install.
///
/// Answers `{result, …}`: `match`, `fixed`, `failed`, `unreported` (Sew
/// does not report the URL yet: skipped), `not_applicable` (no install,
/// standalone identity), or a retry (`sew_down`, `no_port`, `busy`,
/// `mgmt_unreachable`).
pub fn jwks_drift_check(env: &Env) -> Value {
    if !env.ops.linux() {
        return json!({"result": "not_applicable", "why": "not Linux"});
    }
    let Some(row) = store::load() else {
        return json!({"result": "not_applicable", "why": "no Sew install"});
    };
    if row_provider(&row) != pin::PROVIDER_K2 {
        return json!({"result": "not_applicable", "why": "standalone identity has no K2 JWKS"});
    }
    if !matches!(row.status.as_str(), "running" | "degraded" | "stopped" | "error") {
        return json!({"result": "sew_down", "why": format!("Sew is {}", row.status)});
    }
    if !(env.ops.unit(UNIT_DB).is_active() && env.ops.unit(UNIT_SEW).is_active()) {
        return json!({"result": "sew_down", "why": "sew-db or sew is not active"});
    }
    let Some(port) = env.ops.daemon_port() else {
        return json!({"result": "no_port"});
    };
    let want = jwks_url_for(port);
    let st = match mgmt(env, "GET", "/mgmt/v1/status", None) {
        Ok(s) => s,
        Err(e) => return json!({"result": "mgmt_unreachable", "error": e.to_string()}),
    };
    let mut out = serde_json::Map::new();
    if let Some(id) = st
        .get("install_id")
        .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
        .filter(|s| s != "none" && pin::install_id_ok(s))
    {
        if row.install_id.as_deref() != Some(id.as_str()) {
            if store::update(env.ops.now(), |r| r.install_id = Some(id.clone())).is_ok() {
                out.insert("installId".into(), json!({"from": row.install_id, "to": id}));
                changed("install-id");
            }
        }
    }
    let have = st.pointer("/identity/jwks_url").and_then(Value::as_str).map(str::to_string);
    let Some(have) = have else {
        out.insert("result".into(), json!("unreported"));
        return Value::Object(out);
    };
    if have == want {
        if env.ops.record_load(JWKS_RECORD).is_some() {
            let _ = env.ops.record_save(JWKS_RECORD, None);
            changed("jwks");
        }
        out.insert("result".into(), json!("match"));
        return Value::Object(out);
    }
    if helper_check(env).is_err() {
        return jwks_drift_failed(env, &have, &want, "the root helper can't run engine sew".into());
    }
    let Ok(latch) = begin(env) else {
        return json!({"result": "busy", "have": have, "want": want});
    };
    let body = json!({"identity.k2.jwks_url": want});
    let fixed = (|| -> Result<(), String> {
        let o = helper(env, &["config"], Some(body.to_string().as_bytes()))?;
        if let Some(n) = sew_exit(&o).filter(|n| *n != 0) {
            return Err(format!("Sew refused identity.k2.jwks_url (exit {n}): {}", sew_message(&o)));
        }
        helper(env, &["systemctl", "restart"], None)?;
        poll_active(env, &[UNIT_DB, UNIT_SEW], 60)?;
        *env.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
        wait_health(env, 30)?;
        let back = mgmt(env, "GET", "/mgmt/v1/status", None).map_err(|e| e.to_string())?;
        match back.pointer("/identity/jwks_url").and_then(Value::as_str) {
            Some(u) if u == want => Ok(()),
            other => Err(format!("after the restart Sew reports {}", other.unwrap_or("no JWKS URL"))),
        }
    })();
    drop(latch);
    match fixed {
        Ok(()) => {
            let _ = env.ops.record_save(JWKS_RECORD, None);
            changed("jwks");
            out.insert("result".into(), json!("fixed"));
            out.insert("from".into(), json!(have));
            out.insert("to".into(), json!(want));
            Value::Object(out)
        }
        Err(e) => jwks_drift_failed(env, &have, &want, e),
    }
}

/// The 60 s health cadence (Linux only), plus the boot recovery of an
/// upgrade or restore the last daemon died in.
pub fn spawn_background() {
    if !super::sew_supported() {
        return;
    }
    let _ = std::thread::Builder::new().name("sew-health".into()).spawn(|| {
        let env = live();
        let hook = grant_hook_boot(&env);
        k2_core::log_debug!("[sew] grant hook at boot: {hook}");
        if let Some(r) = store::load() {
            if r.status == "upgrading" {
                if let Ok(l) = begin(&env) {
                    let _ = recover_interrupted_upgrade(&env);
                    drop(l);
                }
            }
        }
        // S29 §19: the JWKS check runs until it settles for this process
        // (the daemon port changes only with a restart).
        let mut jwks_done = false;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            if let Some(s) = health_tick(&env) {
                k2_core::log_debug!("[sew] state → {s}");
            }
            if !jwks_done {
                let j = jwks_drift_check(&env);
                if j["result"] != "match" && j["result"] != "not_applicable" {
                    k2_core::log_debug!("[sew] jwks check: {j}");
                }
                jwks_done = jwks_settled(&j);
            }
            super::voice::tick(&env);
        }
    });
}

// ── the S27 grant hook's chat target ─────────────────────────────────────

/// Actor on the outbox rows the Sew lifecycle itself writes (attribution).
const HOOK_ACTOR: &str = "system:sew";

/// Point S27's grant hook at this install (K2 identity mode only): Sew's
/// mgmt socket, and Sew's own token file, which the worker never reads (it
/// asks the root helper, [`mgmt_token`]). The first turn-on queues
/// `k2:owner` and a reconcile. Idempotent. Never fails a lifecycle step:
/// the answer is recorded.
pub fn grant_hook_on(env: &Env, row: &Row) -> Value {
    if row_provider(row) != pin::PROVIDER_K2 {
        return json!({"target": "none", "why": "a standalone install takes no K2 grants"});
    }
    let socket = env.ops.mgmt_socket();
    match env.ops.hook_set(true, Some(&socket), Some(pin::SEW_MGMT_TOKEN), HOOK_ACTOR) {
        Ok(()) => json!({"target": "on"}),
        Err(e) => {
            k2_core::log_debug!("[sew] grant hook target not set: {e}");
            json!({"target": "error", "error": e})
        }
    }
}

/// Pause S27's grant hook (`drop` false: disable, an uninstall that keeps
/// grants; rows wait for the next enable) or drop it (`drop` true: an
/// uninstall that drops grants, purge): every chat grant deleted and every
/// open outbox row skipped, so nothing retries against a Sew that is gone.
/// A box whose hook was never set is left alone (no `skin.db` is created).
pub fn grant_hook_off(env: &Env, drop: bool) -> Value {
    match env.ops.hook_targets() {
        Ok(t) if t.iter().any(|k| k == k2_core::tool_hook::CHAT_KIND) => {}
        Ok(_) => return json!({"target": "none"}),
        Err(e) => return json!({"target": "error", "error": e}),
    }
    let dropped = if drop {
        match env.ops.hook_drop() {
            Ok(r) => Some(r),
            Err(e) => return json!({"target": "error", "error": e}),
        }
    } else {
        None
    };
    match env.ops.hook_set(false, None, None, HOOK_ACTOR) {
        Ok(()) => json!({"target": "off", "dropped": dropped}),
        Err(e) => json!({"target": "error", "error": e, "dropped": dropped}),
    }
}

/// Daemon boot: the hook follows the install the last daemon left (an
/// install from before this build, or a crash between the two writes).
pub fn grant_hook_boot(env: &Env) -> Value {
    match store::load() {
        Some(r) if r.status == "removed" || r.status == "disabled" => grant_hook_off(env, false),
        Some(r) if r.installed_version.is_some() => grant_hook_on(env, &r),
        _ => json!({"target": "none"}),
    }
}

// ── disable / uninstall / purge ──────────────────────────────────────────

fn need_row(allow_removed: bool) -> R<Row> {
    let r = store::load().ok_or_else(|| no("not_installed", 404, "Sew is not installed on this box (k2 hostchat enable)"))?;
    if !allow_removed && r.status == "removed" {
        return Err(no("not_installed", 409, "Sew was uninstalled (data kept): k2 hostchat enable brings it back"));
    }
    Ok(r)
}

fn fail_row(env: &Env, msg: &str) {
    let m = msg.to_string();
    save_enable(env, |r| {
        r.status = "error".into();
        r.last_error = Some(m);
    });
}

/// Stop Sew, keep everything.
pub fn disable(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    need_row(false)?;
    helper_check(env)?;
    let _l = begin(env)?;
    if let Err(e) = helper(env, &["systemctl", "disable-now"], None) {
        fail_row(env, &format!("disable: {e}"));
        return Err(no("disable_failed", 500, e));
    }
    *env.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
    save_enable(env, |r| {
        r.status = "disabled".into();
        r.last_error = None;
    });
    // Grant changes wait in the outbox until Sew is back (S27).
    let hook = grant_hook_off(env, false);
    changed("disabled");
    Ok(json!({
        "ok": true,
        "state": "disabled",
        "grantHook": hook,
        "warning": "Chat is down for everyone on this box. Agents' k2 chat calls fail. Data, keys and backups are kept: \
                    k2 hostchat enable brings it back.",
    }))
}

/// Remove the server, keep data, config, secrets, backups and the vault key.
pub fn uninstall(env: &Env, drop_grants: bool) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(true)?;
    if row.status == "removed" {
        return Ok(json!({"ok": true, "state": "removed", "alreadyRemoved": true}));
    }
    helper_check(env)?;
    let _l = begin(env)?;
    let stopped = helper(env, &["systemctl", "disable-now"], None).is_ok();
    match sew_ok(env, &["uninstall"], None) {
        Ok(_) => {}
        // Keep-data uninstall of a Sew this K2 does not recognise stays
        // refused: only that Sew knows what of its layout is data to keep,
        // and the data would wait for a Sew K2 cannot run. It is stopped
        // (disable-now never runs Sew), so say so instead of `error`, and
        // point at the two ways out.
        Err(e) if e.contains("unknown sew binary") => {
            let m = "uninstall refused: /usr/local/bin/sew is not a Sew release this K2 knows, so K2 will not run it to \
                     remove Sew while keeping its data. Sew is stopped. Either wipe it (k2 hostchat purge \
                     --confirm-hostname <host>: K2 removes it without running it) or put a pinned Sew binary back and \
                     uninstall again";
            let msg = m.to_string();
            save_enable(env, |r| {
                if stopped {
                    r.status = "disabled".into();
                } else {
                    r.status = "error".into();
                }
                r.last_error = Some(msg);
            });
            if stopped {
                *env.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
                grant_hook_off(env, false);
            }
            return Err(no("unknown_sew_binary", 409, m));
        }
        Err(e) => {
            fail_row(env, &format!("uninstall: {e}"));
            return Err(no("uninstall_failed", 500, e));
        }
    }
    *env.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
    save_enable(env, |r| {
        r.status = "removed".into();
        r.last_error = None;
        // Sew's units and config are gone: the next enable must run
        // `sew install` again (the master key and the binary stay).
        if let Some(steps) = r.enable_progress.as_mut().and_then(|p| p.get_mut("steps")).and_then(Value::as_object_mut) {
            steps.remove("install");
        }
    });
    // S27: drop the chat grants (and skip their open rows) or keep them
    // paused for the next enable.
    let hook = grant_hook_off(env, drop_grants);
    changed("uninstalled");
    Ok(json!({
        "ok": true,
        "state": "removed",
        "kept": ["data", "config", "secrets", "backups", "the vault master key", "the install id"],
        "grants": if drop_grants { "dropped" } else { "kept" },
        "grantHook": hook,
    }))
}

/// Delete every byte of Sew. The route proved a person confirmed it (vault
/// confirm) before this runs. Vault keys go LAST, only after Sew's purge
/// succeeded, so a failure leaves everything resumable.
pub fn purge(env: &Env, confirm_hostname: &str, keep_backups: bool) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(true)?;
    let want = purge_hostname(env, &row);
    if want.is_empty() || confirm_hostname.trim() != want {
        return Err(no("confirm_hostname_mismatch", 400, format!("type this box's hostname to purge: --confirm-hostname {want}")));
    }
    helper_check(env)?;
    let _l = begin(env)?;
    // Sew checks `--yes-delete-all-data=<id>` against the install id it
    // reads from its database, and the id is `none` when that database is
    // unreadable (Sew's contract). So: never stop sew-db first (`sew
    // uninstall --purge` disables every unit itself), and type back `none`
    // when sew-db is down (after an uninstall). S29 VM cycle: K2 stopped
    // the db first and passed the stored id, so every purge was refused.
    // The person proof (vault confirm) stays bound to the real install id.
    let db_up = env.ops.unit(UNIT_DB).is_active();
    let id = if db_up { row.install_id.clone().unwrap_or_else(|| "none".into()) } else { "none".into() };
    let mut tail = vec!["purge", id.as_str()];
    if keep_backups {
        tail.push("keep-backups");
    }
    // A known binary: Sew's own `uninstall --purge` (`teardown: "sew"`). A
    // missing or unrecognised one (0.46.3 helper): the helper never runs
    // it and tears Sew's layout down itself (`teardown: "k2"`).
    let out = match sew_ok(env, &tail, None) {
        Ok(v) => v,
        Err(e) => {
            fail_row(env, &format!("purge: {e}"));
            let hint = if e.contains("unknown sew binary") || e.contains("sew is not installed") {
                format!(
                    "{e}: this root helper predates K2's own teardown of a Sew it does not recognise; update it \
                     (k2 root-helper update) and purge again (nothing in the K2 vault was deleted)"
                )
            } else {
                format!("{e} (nothing in the K2 vault was deleted; retry when fixed)")
            };
            return Err(no("purge_failed", 500, hint));
        }
    };
    let teardown = out.get("teardown").and_then(Value::as_str).unwrap_or("sew").to_string();
    // `--keep-backups` moves them to /var/lib/sew-backups-<stamp>: say where.
    let backups_at = out
        .pointer("/sew/kept_backups")
        .or_else(|| out.get("keptBackups"))
        .and_then(Value::as_str)
        .map(str::to_string);
    for r in [&row.master_key_ref, &row.master_next_ref, &row.master_prev_ref].into_iter().flatten() {
        if let Err(e) = env.secrets.delete(r) {
            return Err(no("vault_error", 500, format!("Sew was purged but the vault key could not be deleted: {e}")));
        }
    }
    *env.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
    store::delete().map_err(|e| no("db_error", 500, e))?;
    // Sew's config went with it: K2's records of what it set there (the
    // public URL, a JWKS drift) describe nothing now.
    for rec in [SERVER_RECORD, JWKS_RECORD] {
        if let Err(e) = env.ops.record_save(rec, None) {
            k2_core::log_debug!("[sew] purge: record {rec}: {e}");
        }
    }
    // Sew's purge removed sew-media too: drop K2's voice record, TURN DNS
    // record, nested label and TURN key.
    let voice_left = super::voice::forget(env, &super::voice::load(env));
    // S27: Sew's members are gone with it; so are the chat grants and any
    // row that would still try to reach it.
    let hook = grant_hook_off(env, true);
    changed("purged");
    // What the teardown left on the box (K2's path: the kept recovery
    // file, a sysctl it could not restore, a login-like `sew` account).
    let left: Vec<Value> = out.get("left").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut body = json!({
        "ok": true,
        "state": "not-installed",
        "purged": true,
        "teardown": teardown,
        "grantHook": hook,
        "keptBackups": keep_backups,
        "backupsAt": backups_at,
        "voiceLeftovers": voice_left,
        "left": left,
        "warning": "Off-box backups and the keyring mirror can no longer be read unless you kept the recovery key. \
                    K2 did not delete off-box copies.",
    });
    if teardown == "k2" {
        body["note"] = json!(
            "/usr/local/bin/sew was missing or not a Sew release this K2 knows, so K2 removed Sew's units, config, data, \
             credentials and binaries itself (it never ran that binary). Accounts are kept."
        );
        for k in ["removed", "units", "sysctl", "accounts", "binary"] {
            if let Some(v) = out.get(k) {
                body[k] = v.clone();
            }
        }
    }
    Ok(body)
}

// ── config ───────────────────────────────────────────────────────────────

/// `sew config set server.public_url` (a host on this box's own k2.dev
/// label or a bound custom domain), then a `sew.service` restart (Sew reads
/// it at start). Sew builds setup links as `<public_url>/setup`, so for the
/// 0.46.0 demo `https://<hearth origin>` points them at the App. The
/// setup-link key is not on Sew's settable list and is never sent;
/// `voice.public_url` goes through `sew media install` (voice::set_url).
/// K2's record of what it set with [`server_config`] (`publicUrl`).
pub const SERVER_RECORD: &str = "server";

/// The `server.public_url` K2 set (`None` = never set through K2).
pub fn public_url(env: &Env) -> Option<String> {
    env.ops
        .record_load(SERVER_RECORD)
        .and_then(|v| v.get("publicUrl").and_then(Value::as_str).map(str::to_string))
        .filter(|u| !u.is_empty())
}

/// The warning while Sew's public URL is unset: setup links and invites
/// would carry Sew's loopback address. Names the fix.
pub fn public_url_warning(env: &Env) -> Value {
    let host = env
        .ops
        .box_label()
        .map(|l| format!("<app>.{l}.k2.dev"))
        .unwrap_or_else(|| "<host>".into());
    json!({
        "code": "public_url_unset",
        "message": format!(
            "Sew's public URL isn't set, so password setup links and invites won't open from outside this box. \
             Set it to the https origin people use (your chat App, a host on this box's k2.dev label or a bound custom domain): \
             k2 hostchat config --public-url https://{host}"
        ),
        "fix": format!("k2 hostchat config --public-url https://{host}"),
    })
}

pub fn server_config(env: &Env, public_url: &str) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    need_row(false)?;
    let mut body = serde_json::Map::new();
    body.insert("server.public_url".into(), json!(super::voice::check_https_url(env, public_url)?));
    helper_check(env)?;
    let _l = begin(env)?;
    let v = Value::Object(body.clone());
    let out = helper(env, &["config"], Some(v.to_string().as_bytes())).map_err(|e| no("config_failed", 500, e))?;
    if let Some(n) = sew_exit(&out).filter(|n| *n != 0) {
        let detail = out
            .get("results")
            .and_then(Value::as_array)
            .and_then(|r| r.last())
            .map(|r| sew_message(&r["result"]))
            .unwrap_or_else(|| sew_message(&out));
        return Err(no("config_failed", 409, format!("Sew refused the config (exit {n}): {detail}")));
    }
    let restarted = if env.ops.unit(UNIT_SEW).is_active() {
        helper(env, &["systemctl", "restart"], None).map_err(|e| no("restart_failed", 500, e))?;
        true
    } else {
        false
    };
    // K2 remembers what it set (Sew's status doesn't report it), so
    // status can warn while it's unset.
    if let Err(e) = env.ops.record_save(SERVER_RECORD, Some(&json!({"publicUrl": public_url, "setAt": env.ops.now()}))) {
        k2_core::log_debug!("[sew] server record: {e}");
    }
    changed("config");
    Ok(json!({"ok": true, "set": body, "restarted": restarted, "setupLinks": format!("{public_url}/setup")}))
}

// ── upgrade / rollback ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct UpgradeRequest {
    pub tarball_path: Option<std::path::PathBuf>,
    /// The pinned sew-media tarball: required when voice is installed (the
    /// NEW core upgrades the engine right after the core, Sew 2026-10-17).
    pub media_tarball_path: Option<std::path::PathBuf>,
    pub dry_run: bool,
}

/// Preconditions + latch. `Ok(Err(body))` = answered without a machine
/// (already current, dry run).
pub fn upgrade_begin<'a>(env: &Env<'a>, req: &UpgradeRequest) -> R<Result<(Latch<'a>, String), Value>> {
    need_released(env)?;
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(false)?;
    if !matches!(row.status.as_str(), "running" | "degraded" | "stopped") {
        return Err(no("not_running", 409, format!("Sew is '{}': upgrade needs a running or stopped server", row.status)));
    }
    helper_check(env)?;
    let st = box_status(env).ok_or_else(|| no("helper_error", 500, "engine sew status failed"))?;
    let (present, version, known) = on_disk(&st);
    if !present || !known {
        return Err(no("unknown_sew_binary", 409, "/usr/local/bin/sew is not a Sew release this K2 knows; nothing was changed"));
    }
    let from = version.unwrap_or_default();
    let to = pinned().version;
    match version_cmp(&from, to) {
        Some(std::cmp::Ordering::Equal) => {
            // The binary on disk already IS the pin (a hand-upgraded box):
            // bring the row's record of it in line, so status, enable's
            // already-enabled check and the next upgrade agree with the
            // box. previous_version is left alone: K2 did not see the move.
            let mut resynced = Value::Null;
            if row.installed_version.as_deref() != Some(to) || row.pinned_version != to {
                let _l = begin(env)?;
                let was = row.installed_version.clone();
                store::update(env.ops.now(), |r| {
                    r.installed_version = Some(to.into());
                    r.pinned_version = to.into();
                })
                .map_err(|e| no("db_error", 500, e))?;
                resynced = json!({"installedVersion": {"from": was, "to": to}});
                changed("upgraded");
            }
            return Ok(Err(json!({"ok": true, "alreadyCurrent": true, "version": to, "resynced": resynced})));
        }
        Some(std::cmp::Ordering::Greater) => {
            return Err(no("downgrade_refused", 409, format!("Sew {from} is newer than this daemon's pin {to}: a downgrade is a restore")))
        }
        _ => {}
    }
    if req.tarball_path.is_none() {
        return Err(no(
            "release_source_unset",
            409,
            format!("no Sew release source: k2 hostchat upgrade --tarball <sew-{to}-<triple>.tar.gz> (Sew SW1 is not published)"),
        ));
    }
    let voice = super::voice::load(env).installed;
    if voice {
        upgrade_media_source(env, req)?;
    }
    if req.dry_run {
        let mut steps = vec![
            "stage the pinned tarball",
            "sew upgrade --binary (the installed Sew backs up, swaps, migrates, checks health, rolls back on failure)",
            "K2 health check: units active, mgmt healthy, version = pin",
            "on a failed K2 health check: automatic rollback",
        ];
        if voice {
            steps.insert(1, "stage the pinned sew-media tarball");
            steps.push("voice: the NEW sew runs sew media upgrade --now (Sew rolls the engine back itself on failure)");
        }
        return Ok(Err(json!({"ok": true, "dryRun": true, "from": from, "to": to, "voice": voice, "steps": steps})));
    }
    let latch = begin(env)?;
    let now = env.ops.now();
    let prior = row.status.clone();
    store::update(now, |r| {
        r.status = "upgrading".into();
        r.last_error = None;
        r.upgrade_progress = Some(json!({"from": from, "to": to, "prior": prior, "steps": {}, "current": "stage", "result": null, "startedAt": now}));
    })
    .map_err(|e| no("db_error", 500, e))?;
    Ok(Ok((latch, from)))
}

/// With voice installed, a sew-media source must be in hand before the
/// core moves: a tarball path now, or the media pin already staged.
fn upgrade_media_source(env: &Env, req: &UpgradeRequest) -> R<()> {
    if req.media_tarball_path.is_some() {
        return Ok(());
    }
    let (v, art) = k2_root_proto::sew::pinned_media_artifact(&env.ops.arch()).map_err(|e| no("media_unpinned", 409, e))?;
    let staged = helper(env, &["status"], None).ok().and_then(|s| s.pointer("/staged/media").and_then(Value::as_str).map(str::to_string));
    if staged.as_deref() == Some(v) {
        return Ok(());
    }
    Err(no(
        "media_tarball_needed",
        409,
        format!(
            "voice is installed: the voice engine is upgraded right after the core, so name the sew-media {v} release tarball \
             for {} too (k2 hostchat upgrade --tarball <sew> --media-tarball <sew-media>); nothing was changed",
            art.triple
        ),
    ))
}

/// Record a step change; with `result`, the end of the upgrade. A result
/// other than `done` ALWAYS has its reason in the row's `last_error` and
/// in `upgradeProgress.lastError`: one that reached here without one is a
/// K2 bug, recorded and logged as such, never a silent null.
fn upgrade_note(env: &Env, step: Option<&str>, result: Option<&str>) {
    let now = env.ops.now();
    save_enable(env, |r| {
        let p = r.upgrade_progress.get_or_insert_with(|| json!({"steps": {}}));
        if let Some(cur) = p.get("current").and_then(Value::as_str).map(str::to_string) {
            if step != Some(cur.as_str()) {
                // `at` is whole seconds (several steps share one) and the
                // JSON object is key-sorted: `seq` is the execution order.
                let seq = p["steps"].as_object().map_or(0, |m| m.len()) + 1;
                p["steps"][cur] = json!({"at": now, "seq": seq});
            }
        }
        p["current"] = json!(step);
        if let Some(res) = result {
            p["result"] = json!(res);
            p["finishedAt"] = json!(now);
            if res != "done" && r.last_error.as_deref().is_none_or(|e| e.trim().is_empty()) {
                let m = format!("the upgrade ended '{res}' but no reason was recorded (a K2 bug: report it with the k2-daemon log)");
                k2_core::log_debug!("[sew] upgrade: {m}");
                r.last_error = Some(m);
            }
            p["lastError"] = json!(if res == "done" { None } else { r.last_error.clone() });
            k2_core::log_debug!("[sew] upgrade to {}: result {res}; lastError: {}", p["to"].as_str().unwrap_or("?"), r.last_error.as_deref().unwrap_or("none"));
        }
    });
}

/// Upgrade the voice engine through the NEW core (Sew 2026-10-17: the
/// sidecar pins are baked into `sew`, so only the pin core accepts the pin
/// media tarball). Runs only after K2's own health check of the core
/// passed, so a core rollback never strands a new engine. `Err(why)` = the
/// core stays upgraded and the engine did not move (Sew rolls a failed
/// engine restart back itself).
fn upgrade_media(env: &Env, to: &str) -> Result<Value, String> {
    let v = helper(env, &["media", "upgrade"], None).map_err(|e| format!("the helper refused the voice engine upgrade: {e}"))?;
    match sew_exit(&v) {
        Some(0) => Ok(v),
        Some(1) => Err(format!("sew media upgrade failed and Sew rolled the voice engine back: {}", sew_message(&v))),
        Some(2) => Err(format!(
            "sew media upgrade failed AND Sew's engine rollback failed: {}. A person must act (journalctl -u sew-media.service)",
            sew_message(&v)
        )),
        Some(n) => Err(format!("sew media upgrade refused (exit {n}): {}", sew_message(&v))),
        None => Err(format!("the helper answered the voice engine upgrade to {to} without a Sew exit: {}", short(&v))),
    }
}

/// The upgrade machine (Sew does backup, swap, migrate and its own
/// rollback; K2 then checks health and rolls back itself if it fails).
/// Returns the final progress record.
pub fn upgrade_machine(env: &Env, req: &UpgradeRequest, from: &str) -> Value {
    let to = pinned().version;
    let prior = store::load()
        .and_then(|r| r.upgrade_progress)
        .and_then(|p| p.get("prior").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "running".into());
    let voice = super::voice::load(env).installed;
    let staged = stage_tarball(env, req.tarball_path.as_deref()).and_then(|_| {
        if voice {
            super::voice::stage_media_bytes(env, req.media_tarball_path.as_deref()).map_err(|f| format!("sew-media: {}", f.hint))
        } else {
            Ok(())
        }
    });
    if let Err(e) = staged {
        let m = format!("upgrade to {to} aborted before any change: {e}");
        save_enable(env, |r| {
            r.status = prior.clone();
            r.last_error = Some(m);
        });
        let _ = helper(env, &["unstage"], None);
        upgrade_note(env, None, Some("aborted"));
        return store::load().and_then(|r| r.upgrade_progress).unwrap_or_default();
    }
    upgrade_note(env, Some("upgrade"), None);
    let out = helper(env, &["upgrade"], None);
    let result = match out {
        Err(e) => {
            // The helper never ran Sew (a refusal): nothing changed.
            save_enable(env, |r| {
                r.status = prior.clone();
                r.last_error = Some(format!("upgrade to {to} refused: {e}"));
            });
            "refused"
        }
        Ok(v) => match sew_exit(&v) {
            // A Sew verb always answers Sew's exit: an answer without one is
            // a broken helper, never a silent success or a reasonless refusal.
            None => {
                let m = format!("upgrade to {to}: the helper answered without Sew's exit code: {}", short(&v));
                save_enable(env, |r| {
                    r.status = prior.clone();
                    r.last_error = Some(m);
                });
                "refused"
            }
            Some(0) => {
                upgrade_note(env, Some("health"), None);
                match health_check(env, to) {
                    Ok(_) => {
                        save_enable(env, |r| {
                            r.status = "running".into();
                            r.previous_version = Some(from.to_string());
                            r.installed_version = Some(to.into());
                            r.last_error = None;
                        });
                        // An install from before S27 gets its hook here.
                        if let Some(row) = store::load() {
                            grant_hook_on(env, &row);
                        }
                        if voice {
                            upgrade_note(env, Some("media"), None);
                            match upgrade_media(env, to) {
                                Ok(_) => "done",
                                Err(why) => {
                                    let m = format!("Sew is upgraded to {to}, but the voice engine was not: {why}");
                                    save_enable(env, |r| r.last_error = Some(m));
                                    "media_failed"
                                }
                            }
                        } else {
                            "done"
                        }
                    }
                    Err(health) => {
                        // K2's own check failed after Sew said yes: roll back.
                        upgrade_note(env, Some("rollback"), None);
                        auto_rollback(env, from, to, &health)
                    }
                }
            }
            Some(1) => {
                let m = format!("upgrade to {to} failed; Sew rolled back to {from}: {}", sew_message(&v));
                let healthy = health_check(env, from).is_ok();
                save_enable(env, |r| {
                    r.status = if healthy { "running".into() } else { "error".into() };
                    r.last_error = Some(m);
                });
                "rolled_back"
            }
            Some(2) => {
                let m = format!(
                    "upgrade to {to} failed AND Sew's rollback failed: {}. A person must act: read {SEW_UPGRADE_STATE} \
                     and the sew journal (journalctl -u sew.service); K2 will not retry",
                    sew_message(&v)
                );
                save_enable(env, |r| {
                    r.status = "error".into();
                    r.last_error = Some(m);
                });
                "rollback_failed"
            }
            Some(n) => {
                let m = format!("upgrade to {to} refused by Sew (exit {n}): {}", sew_message(&v));
                save_enable(env, |r| {
                    r.status = prior.clone();
                    r.last_error = Some(m);
                });
                "refused"
            }
        },
    };
    let _ = helper(env, &["unstage"], None);
    upgrade_note(env, None, Some(result));
    store::load().and_then(|r| r.upgrade_progress).unwrap_or_default()
}

fn auto_rollback(env: &Env, from: &str, to: &str, why: &str) -> &'static str {
    let rb = sew_ok(env, &["rollback"], None);
    let healthy = rb.is_ok() && health_check(env, from).is_ok();
    if healthy {
        let m = format!("upgrade to {to} failed K2's health check ({why}); rolled back to {from}");
        save_enable(env, |r| {
            r.status = "running".into();
            r.installed_version = Some(from.into());
            r.last_error = Some(m);
        });
        "rolled_back"
    } else {
        let m = format!(
            "upgrade to {to} failed K2's health check ({why}) and the automatic rollback failed ({}). A person must act: \
             read {SEW_UPGRADE_STATE}; k2 hostchat rollback, or a restore",
            rb.err().unwrap_or_else(|| "Sew still unhealthy after rollback".into())
        );
        save_enable(env, |r| {
            r.status = "error".into();
            r.last_error = Some(m);
        });
        "rollback_failed"
    }
}

/// The boot side of an upgrade the last daemon died in: healthy on the
/// pin → done; healthy on the old version → interrupted (nothing to undo);
/// unhealthy → roll back.
pub fn recover_interrupted_upgrade(env: &Env) -> Value {
    let Some(row) = store::load() else { return Value::Null };
    if row.status != "upgrading" {
        return Value::Null;
    }
    let p = row.upgrade_progress.clone().unwrap_or_default();
    let from = p.get("from").and_then(Value::as_str).unwrap_or("").to_string();
    let to = pinned().version;
    let result = if health_check(env, to).is_ok() {
        save_enable(env, |r| {
            r.status = "running".into();
            r.previous_version = Some(from.clone());
            r.installed_version = Some(to.into());
            r.last_error = None;
        });
        "done"
    } else if !from.is_empty() && health_check(env, &from).is_ok() {
        save_enable(env, |r| {
            r.status = "running".into();
            r.last_error = Some(format!("the upgrade to {to} was interrupted by a daemon restart; Sew {from} is serving"));
        });
        "interrupted"
    } else {
        auto_rollback(env, &from, to, "the daemon restarted mid-upgrade and Sew is unhealthy")
    };
    upgrade_note(env, None, Some(result));
    store::load().and_then(|r| r.upgrade_progress).unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
pub struct RollbackRequest {
    /// The PREVIOUS release's sew-media tarball, when voice is installed and
    /// the helper does not keep it yet (it keeps the ones it installed).
    pub media_tarball_path: Option<std::path::PathBuf>,
}

/// `k2 hostchat rollback` without a media tarball.
pub fn rollback(env: &Env) -> R<Value> {
    rollback_with(env, &RollbackRequest::default())
}

/// With voice installed, the previous release's engine tarball must be in
/// hand before the core moves (Sew 2026-10-17: the OLD core reinstalls it
/// after the core rollback). Stages a given tarball (the helper keeps an
/// older release's engine). Returns the engine version the rollback puts
/// back.
fn rollback_media_source(env: &Env, prev: Option<&str>, req: &RollbackRequest) -> R<Option<String>> {
    let Some(prev) = prev else {
        return Err(no(
            "rollback_target_unknown",
            409,
            "voice is installed but K2 has no record of the previous Sew version, so it can't put the matching voice \
             engine back; nothing was changed",
        ));
    };
    let (mv, art) = k2_root_proto::sew::media_artifact_of(prev, &env.ops.arch()).map_err(|e| no("media_unpinned", 409, e))?;
    if let Some(p) = &req.media_tarball_path {
        let bytes = env.ops.read_file(p, MAX_TARBALL).map_err(|e| no("bad_request", 400, e))?;
        if !crate::update_routes::verify_sha256(&bytes, art.tarball_sha256) {
            return Err(no(
                "sha256_mismatch",
                400,
                format!("{} is not sew-media {mv} for {} (Sew {prev}'s engine); nothing was changed", p.display(), art.triple),
            ));
        }
        helper(env, &["stage-media"], Some(&bytes)).map_err(|e| no("helper_error", 500, e))?;
    }
    let kept = helper(env, &["status"], None)
        .ok()
        .and_then(|s| s.pointer("/staged/keptMedia").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    if !kept.iter().any(|k| k.as_str() == Some(mv)) {
        return Err(no(
            "previous_media_tarball_needed",
            409,
            format!(
                "voice is installed: after the core goes back to {prev}, that core reinstalls its own voice engine, sew-media \
                 {mv}, and this box doesn't keep that tarball. Name it: k2 hostchat rollback --media-tarball \
                 sew-media-{mv}-{}.tar.gz; nothing was changed",
                art.triple
            ),
        ));
    }
    Ok(Some(mv.to_string()))
}

/// `sew doctor`'s voice-engine checks after a rollback: `media.binary` ok
/// and `media.version` = `sew-media <v>` matching the core. `(ok, json)`.
fn doctor_media(env: &Env, want: &str) -> (bool, Value) {
    let v = match helper(env, &["doctor"], None) {
        Ok(v) => v,
        Err(e) => return (false, json!({"error": e})),
    };
    let check = |id: &str| {
        v.pointer("/sew/checks")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|c| c["id"] == id).cloned())
            .unwrap_or_else(|| json!({"id": id, "status": "missing", "detail": "sew doctor did not report it"}))
    };
    let (bin, ver) = (check("media.binary"), check("media.version"));
    let ok = bin["status"] == "ok" && ver["status"] == "ok" && ver["detail"].as_str() == Some(&format!("sew-media {want}"));
    (ok, json!({"ok": ok, "sewExit": sew_exit(&v), "mediaBinary": bin, "mediaVersion": ver}))
}

/// `k2 hostchat rollback`: back to the previous version and its data
/// (only when the schema did not change; else Sew refuses and a restore is
/// the way back). With voice: the reverse of the upgrade (Sew 2026-10-17):
/// core back → K2's check of the old core → the OLD core reinstalls the
/// previous release's engine (`engine sew media rollback`) → `sew doctor`'s
/// media checks in the answer.
pub fn rollback_with(env: &Env, req: &RollbackRequest) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(false)?;
    if !matches!(row.status.as_str(), "running" | "degraded" | "stopped" | "error") {
        return Err(no("not_running", 409, format!("Sew is '{}'", row.status)));
    }
    helper_check(env)?;
    let _l = begin(env)?;
    let voice = super::voice::load(env).installed;
    let media_to = if voice { rollback_media_source(env, row.previous_version.as_deref(), req)? } else { None };
    let v = helper(env, &["rollback"], None).map_err(|e| no("helper_error", 500, e))?;
    match sew_exit(&v).unwrap_or(0) {
        0 => {}
        3 => {
            return Err(no(
                "rollback_refused",
                409,
                format!(
                    "{}. A rollback across a schema change needs a restore: list the backups with k2 hostchat backups, \
                     then k2 hostchat restore <bundle-id> --confirm",
                    sew_message(&v)
                ),
            ))
        }
        n => {
            let m = format!("rollback: exit {n}: {}", sew_message(&v));
            fail_row(env, &m);
            return Err(no("rollback_failed", 500, m));
        }
    }
    let to = v.pointer("/sew/to").and_then(Value::as_str).filter(|s| k2_root_proto::version::Semver::parse(s).is_some()).map(str::to_string);
    let want = to.clone().or(row.previous_version.clone()).unwrap_or_default();
    let healthy = health_check(env, &want);
    let ok = healthy.is_ok();
    let health_err = healthy.err();
    save_enable(env, |r| {
        r.status = if ok { "running".into() } else { "error".into() };
        if let Some(t) = &to {
            r.installed_version = Some(t.clone());
        }
        r.previous_version = None;
        r.last_error = health_err.clone().map(|e| format!("rolled back, but Sew is unhealthy: {e}"));
    });
    let mut body = json!({"ok": ok, "state": if ok { "running" } else { "error" }, "version": to, "sew": v["sew"], "error": health_err});
    let Some(mv) = media_to else { return Ok(body) };
    if !ok {
        // The old core is not healthy: its engine is not touched.
        body["media"] = json!({"result": "skipped", "to": mv, "error": "the old core failed K2's health check"});
        return Ok(body);
    }
    let media = match helper(env, &["media", "rollback"], None) {
        Err(e) => Err(format!("the helper refused the voice engine rollback: {e}")),
        Ok(m) => match sew_exit(&m) {
            Some(0) => Ok(m),
            Some(n) => Err(format!("sew media upgrade (back to sew-media {mv}) exit {n}: {}", sew_message(&m))),
            None => Err(format!("the helper answered the voice engine rollback without a Sew exit: {}", short(&m))),
        },
    };
    let (doc_ok, doctor) = doctor_media(env, &mv);
    body["doctor"] = doctor.clone();
    let err = match &media {
        Err(e) => Some(format!("Sew is back on {want}, but its voice engine was not put back: {e}")),
        Ok(_) if !doc_ok => Some(format!(
            "Sew and its voice engine are back on {want} / sew-media {mv}, but sew doctor disagrees: media.binary {} ({}), \
             media.version {} ({})",
            doctor["mediaBinary"]["status"].as_str().unwrap_or("?"),
            doctor["mediaBinary"]["detail"].as_str().unwrap_or(""),
            doctor["mediaVersion"]["status"].as_str().unwrap_or("?"),
            doctor["mediaVersion"]["detail"].as_str().unwrap_or(""),
        )),
        Ok(_) => None,
    };
    body["media"] = match &media {
        Ok(m) => json!({"result": "done", "to": mv, "notices": m.get("notices")}),
        Err(e) => json!({"result": "failed", "to": mv, "error": e}),
    };
    if let Some(e) = err {
        k2_core::log_debug!("[sew] rollback: {e}");
        body["ok"] = json!(false);
        body["error"] = json!(e);
        save_enable(env, |r| r.last_error = Some(e));
    }
    Ok(body)
}

// ── backups / restore ────────────────────────────────────────────────────

/// How long `backup run` waits for Sew's backup job (1 s polls).
pub const BACKUP_WAIT_SECS: u32 = 1800;

/// A mgmt failure of the backup routes as a refusal.
fn backup_mgmt_fail(e: MgmtError) -> Fail {
    match e {
        MgmtError::Status(409, b) if b.contains(wire::BACKUP_RUNNING) => {
            no("backup_busy", 409, "a Sew backup is already running; try again when it finishes")
        }
        MgmtError::Status(503, b) if b.contains(wire::BACKUPS_UNAVAILABLE) => {
            no("backups_unavailable", 503, "this Sew server has no backup engine (backups_unavailable)")
        }
        MgmtError::Status(c, b) => no("backup_failed", 500, format!("Sew mgmt answered {c}: {b}")),
        e => no("mgmt_unreachable", 503, format!("{e}: backups go through Sew's mgmt socket, so Sew must be running")),
    }
}

/// `GET /mgmt/v1/backups` (SW15): every complete bundle, newest first.
fn backup_list(env: &Env) -> R<Vec<wire::MgmtBackup>> {
    let v = mgmt(env, "GET", "/mgmt/v1/backups", None).map_err(backup_mgmt_fail)?;
    let list: wire::MgmtBackupList =
        serde_json::from_value(v).map_err(|e| no("backup_list_failed", 500, format!("Sew's backup list does not parse: {e}")))?;
    Ok(list.items)
}

/// `POST /mgmt/v1/backup` (SW15) and wait for its `backup` job. A second
/// POST while a backup job is queued or running answers that same job.
fn backup_job(env: &Env) -> R<(String, wire::BackupJobResult)> {
    let v = mgmt(env, "POST", "/mgmt/v1/backup", Some(&json!({}))).map_err(backup_mgmt_fail)?;
    let start: wire::StartJobResponse =
        serde_json::from_value(v).map_err(|e| no("backup_failed", 500, format!("Sew's backup answer does not parse: {e}")))?;
    let id = start.job_id;
    let mut last = String::from("queued");
    for i in 0..BACKUP_WAIT_SECS {
        match mgmt(env, "GET", &format!("/mgmt/v1/jobs/{id}"), None) {
            Ok(v) => {
                let j: wire::JobStatus =
                    serde_json::from_value(v).map_err(|e| no("backup_failed", 500, format!("Sew's job status does not parse: {e}")))?;
                if j.kind != wire::BACKUP_JOB_KIND {
                    return Err(no("backup_failed", 500, format!("Sew answered job {id} of kind '{}', not a backup", j.kind)));
                }
                let why = j.last_error.clone().unwrap_or_else(|| "no detail".into());
                match j.state {
                    wire::JobState::Succeeded => {
                        let r = j.result.ok_or_else(|| no("backup_failed", 500, format!("backup job {id} succeeded without a result")))?;
                        let r: wire::BackupJobResult =
                            serde_json::from_value(r).map_err(|e| no("backup_failed", 500, format!("backup job result does not parse: {e}")))?;
                        return Ok((id, r));
                    }
                    // Lost the race to another backup (the timer, a CLI run).
                    wire::JobState::Failed | wire::JobState::Dead if why.contains(wire::BACKUP_RUNNING) => {
                        return Err(no("backup_busy", 409, "a Sew backup is already running; try again when it finishes"));
                    }
                    wire::JobState::Dead => return Err(no("backup_failed", 500, format!("Sew backup job {id} failed: {why}"))),
                    s => last = format!("{s:?}").to_lowercase(),
                }
            }
            Err(e @ MgmtError::Unreachable(_)) => last = e.to_string(),
            Err(e) => return Err(backup_mgmt_fail(e)),
        }
        if i + 1 < BACKUP_WAIT_SECS {
            env.ops.sleep_ms(1000);
        }
    }
    Err(with(
        no("backup_timeout", 504, format!("Sew's backup job {id} is still {last}; follow it with /cli/sew/job")),
        json!({"jobId": id}),
    ))
}

pub fn backups(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(true)?;
    helper_check(env)?;
    let list = backup_list(env)?;
    let latest = list.first().cloned();
    Ok(json!({
        "ok": true,
        "backups": list,
        "latest": latest,
        "ship": {"kind": row.ship_kind, "path": row.ship_path},
    }))
}

/// `k2 hostchat backup run`: Sew's mgmt backup job (SW15), waited for.
pub fn backup_run(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(false)?;
    if !matches!(row.status.as_str(), "running" | "degraded") {
        return Err(no("not_running", 409, format!("Sew is '{}': a backup needs a running server", row.status)));
    }
    helper_check(env)?;
    let (job, r) = backup_job(env)?;
    Ok(json!({"ok": true, "jobId": job, "backup": r}))
}

/// Replace ALL Sew data with a backup. Sew takes its own `pre-restore`
/// safety bundle first (no bundle, no restore); anything newer than the
/// restored bundle is lost from the live data.
pub fn restore(env: &Env, bundle_id: &str) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    if !pin::bundle_id_ok(bundle_id) {
        return Err(no("bad_request", 400, "name a backup id from k2 hostchat backups"));
    }
    let row = need_row(false)?;
    // A stopped or failed Sew is when a restore is needed most (S29 VM
    // cycle: a broken restore left schema sew empty and sew-migrate
    // failing). Sew needs only sew-db up; the helper stops sew and starts
    // it again.
    if !matches!(row.status.as_str(), "running" | "degraded" | "stopped" | "error") {
        return Err(no("not_installed", 409, format!("Sew is '{}': there is no install to restore into", row.status)));
    }
    helper_check(env)?;
    // The list is a mgmt read: with Sew down, `sew restore` checks the id
    // itself (exit 4, nothing touched); bundle_id_ok keeps it an id.
    match backups(env) {
        Ok(listed) => {
            let exists = listed["backups"].as_array().is_some_and(|a| a.iter().any(|b| b["id"] == bundle_id));
            if !exists {
                return Err(no("bundle_not_found", 404, format!("no backup '{bundle_id}' (k2 hostchat backups)")));
            }
        }
        Err(f) if f.code == "mgmt_unreachable" => {}
        Err(f) => return Err(f),
    }
    let _l = begin(env)?;
    let now = env.ops.now();
    let note = |env: &Env, step: &str, result: Option<&str>| {
        let now = env.ops.now();
        let (s, res) = (step.to_string(), result.map(str::to_string));
        save_enable(env, |r| {
            let p = r.restore_progress.get_or_insert_with(|| json!({}));
            p["current"] = json!(s);
            if let Some(x) = res {
                p["result"] = json!(x);
                p["finishedAt"] = json!(now);
            }
        });
    };
    save_enable(env, |r| r.restore_progress = Some(json!({"bundle": bundle_id, "startedAt": now, "current": "restore", "result": null})));
    // No K2 pre-restore backup: the only backup mgmt makes is kind
    // `manual`, and Sew's retention (newest bundle per UTC day) then
    // deletes an older same-day bundle, the very one being restored (found
    // in the S29 VM cycle). Sew's own `pre-restore` safety bundle (a
    // special kind, kept 30 days; K2 never passes --no-safety-backup) is
    // the "no backup, no restore" guarantee: when it fails, Sew restores
    // nothing.
    // Sew restores (sew stopped, sew-db up, its safety bundle first).
    note(env, "restore", None);
    let v = helper(env, &["restore", bundle_id], None).map_err(|e| no("helper_error", 500, e))?;
    let exit = sew_exit(&v).unwrap_or(0);
    let healthy = health_check(env, row.installed_version.as_deref().unwrap_or(pinned().version));
    let serving = healthy.is_ok();
    let ok = exit == 0 && serving;
    let err = if exit != 0 {
        // Only Sew's refusals (usage, precondition, not found, unavailable,
        // busy, no access) leave the data alone. Exit 1 is a runtime error
        // that can come after Sew dropped the schema (S29 VM cycle: "schema
        // sew is now empty"); never claim the data is untouched then.
        let untouched = matches!(exit, 2 | 3 | 4 | 69 | 75 | 77);
        let tail = if untouched {
            "(the data was not replaced)".to_string()
        } else {
            let safety = v.pointer("/sew/safety_bundle").and_then(Value::as_str).map(|s| format!(" {s}")).unwrap_or_default();
            format!(
                "(the restore stopped partway: Sew's data may be changed or empty; restore Sew's pre-restore safety \
                 bundle{safety} with k2 hostchat restore <bundle-id> --confirm; k2 hostchat backups lists it)"
            )
        };
        Some(format!("sew restore: exit {exit}: {} {tail}", sew_message(&v)))
    } else {
        healthy.err().map(|e| format!("restored, but Sew is unhealthy: {e}"))
    };
    let e2 = err.clone();
    save_enable(env, |r| {
        r.status = if serving { "running".into() } else { "error".into() };
        r.last_error = e2;
    });
    note(env, "done", Some(if ok { "done" } else { "failed" }));
    let body = json!({
        "ok": ok,
        "restored": exit == 0,
        "bundle": bundle_id,
        "safetyBundle": v.pointer("/sew/safety_bundle"),
        "error": err,
    });
    if ok {
        Ok(body)
    } else {
        Err(with(no("restore_failed", 500, err.unwrap_or_default()), body))
    }
}

// ── keys ─────────────────────────────────────────────────────────────────

/// Rotation phase 1 (Manage Sew): a new master key into the vault and Sew;
/// the old one is kept as `prev` until a person drops it.
pub fn rotate_start(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(false)?;
    if !matches!(row.status.as_str(), "running" | "degraded") {
        return Err(no("not_running", 409, "rotation needs a running server"));
    }
    if row.master_prev_ref.is_some() {
        return Err(no("rotation_in_progress", 409, "a previous master key is still loaded: a person finishes it (k2 hostchat rotate-master drop-prev)"));
    }
    helper_check(env)?;
    let _l = begin(env)?;
    let next_ref = match row.master_next_ref.clone().filter(|r| matches!(env.secrets.resolve(r), Ok(Some(_)))) {
        Some(r) => r, // an earlier start crashed: reuse its key
        None => {
            let k = super::secrets::generate_master_key().map_err(|e| no("random", 500, e))?;
            let r = env.secrets.store("master-next", &k).map_err(|e| no("vault_error", 500, e))?;
            save_enable(env, |row| row.master_next_ref = Some(r.clone()));
            r
        }
    };
    let key = env.secrets.resolve(&next_ref).ok().flatten().ok_or_else(|| no("vault_error", 500, "the new key vanished from the vault"))?;
    let v = sew_ok(env, &["rotate-master"], Some(format!("{key}\n").as_bytes())).map_err(|e| no("rotate_failed", 500, e))?;
    let want_kek = kek_id(&key).map_err(|e| no("rotate_failed", 500, e))?;
    if let Some(k) = v.pointer("/sew/kek_id").and_then(Value::as_str) {
        if k != want_kek {
            return Err(no("rotate_failed", 500, "Sew's new KEK id does not match the key K2 sent"));
        }
    }
    let ver = row.installed_version.clone().unwrap_or_else(|| pinned().version.into());
    health_check(env, &ver).map_err(|e| no("rotate_failed", 500, format!("Sew took the new key but is unhealthy: {e}")))?;
    let job = mgmt(env, "POST", "/mgmt/v1/keys/rotate-master", Some(&json!({}))).map_err(|e| no("rotate_failed", 500, e.to_string()))?;
    let now = env.ops.now();
    let old = row.master_key_ref.clone();
    store::update(now, |r| {
        r.master_prev_ref = old.clone();
        r.master_key_ref = Some(next_ref.clone());
        r.master_next_ref = None;
        r.master_key_created_at = Some(now);
        r.recovery_shown_at = None;
        r.rotate_progress = Some(json!({"phase": "rewrapping", "jobId": job.get("job_id"), "startedAt": now}));
    })
    .map_err(|e| no("db_error", 500, e))?;
    Ok(json!({
        "ok": true,
        "phase": "rewrapping",
        "jobId": job.get("job_id"),
        "kekId": want_kek,
        "next": "when the rewrap job is done, a person drops the old key: k2 hostchat rotate-master drop-prev; \
                 save the NEW recovery key (the old one no longer opens new data)",
    }))
}

/// Rotation phase 2 (person-only; the route checked the vault confirm).
pub fn rotate_drop_prev(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(false)?;
    let prev = row.master_prev_ref.clone().ok_or_else(|| no("no_rotation", 409, "no previous master key is loaded"))?;
    helper_check(env)?;
    let _l = begin(env)?;
    let st = mgmt(env, "GET", "/mgmt/v1/status", None).map_err(|e| no("mgmt_unreachable", 503, e.to_string()))?;
    let pending = st.pointer("/keys/rewrap_pending").and_then(Value::as_u64).unwrap_or(0);
    if pending > 0 {
        return Err(no("rewrap_pending", 409, format!("{pending} items still use the old key; wait for the rewrap job")));
    }
    let v = helper(env, &["drop-prev"], None).map_err(|e| no("helper_error", 500, e))?;
    match sew_exit(&v).unwrap_or(0) {
        0 => {}
        3 => return Err(no("rewrap_pending", 409, sew_message(&v))),
        n => return Err(no("drop_prev_failed", 500, format!("sew keys drop-prev: exit {n}: {}", sew_message(&v)))),
    }
    env.secrets.delete(&prev).map_err(|e| no("vault_error", 500, e))?;
    store::update(env.ops.now(), |r| {
        r.master_prev_ref = None;
        r.rotate_progress = None;
    })
    .map_err(|e| no("db_error", 500, e))?;
    Ok(json!({"ok": true, "dropped": true}))
}

pub const RECOVERY_WARNING: &str = "This key plus a backup bundle plus the keyring mirror can read every message. Store it off \
     this machine. If this box dies, it is the only way back.";

/// The recovery key (person-only; the route checked the vault confirm).
pub fn recovery_key(env: &Env) -> R<Value> {
    let row = need_row(true)?;
    let r = row.master_key_ref.clone().ok_or_else(|| no("no_master_key", 404, "Sew has no master key yet"))?;
    let key = env
        .secrets
        .resolve(&r)
        .map_err(|e| no("vault_error", 500, e))?
        .ok_or_else(|| no("no_master_key", 404, "the vault holds no Sew master key"))?;
    store::update(env.ops.now(), |row| row.recovery_shown_at = Some(env.ops.now())).map_err(|e| no("db_error", 500, e))?;
    Ok(json!({"ok": true, "recoveryKey": key, "warning": RECOVERY_WARNING}))
}

pub fn doctor(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    need_row(true)?;
    helper_check(env)?;
    let v = helper(env, &["doctor"], None).map_err(|e| no("helper_error", 500, e))?;
    let now = env.ops.now();
    let report = v["sew"].clone();
    let exit = sew_exit(&v).unwrap_or(0);
    store::update(now, |r| {
        r.doctor = Some(report.clone());
        r.doctor_at = Some(now);
    })
    .map_err(|e| no("db_error", 500, e))?;
    // A person (or a Manage-Sew agent) re-runs the S29 §19 JWKS check here
    // after a `k2_jwks_url_drift` alarm.
    let jwks = jwks_drift_check(env);
    Ok(json!({"ok": true, "exit": exit, "report": report, "at": now, "jwks": jwks}))
}

pub fn job(env: &Env, id: &str) -> R<Value> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(no("bad_request", 400, "name a job id"));
    }
    need_row(false)?;
    mgmt(env, "GET", &format!("/mgmt/v1/jobs/{id}"), None).map_err(|e| match e {
        MgmtError::Status(404, _) => no("job_not_found", 404, format!("no Sew job '{id}'")),
        e => no("mgmt_unreachable", 503, e.to_string()),
    })
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
