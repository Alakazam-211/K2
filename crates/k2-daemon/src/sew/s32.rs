//! Sew S32 on the daemon side (plan-sew-s30-s32 K26, K27 routes, K31
//! routes): what the Chat Hosting settings page and `k2 hostchat` need
//! beyond S29's lifecycle.
//!
//! - **Status fields** ([`decorate_status`]): `you` (what THIS caller may
//!   do, so a client never guesses from a 200), `owner` (`k2:owner` in Sew:
//!   set / pending / blocked / none), `target` (S27's chat hook on/off),
//!   `mirror`, and `backup.ship` with the last ship result.
//! - **`POST /cli/sew/server/app-url {url}`**: where the owner's Sew web
//!   app lives (Open Sew as owner, S32 §11.2). `""` clears it. Stored on
//!   the `sew_server` row (`app_url`, migration 0166: no new migration).
//! - **`POST /cli/sew/server/ship {kind, path?}`**: off-box backup copies
//!   (S29 §15.2). `dir` = a folder on another disk the owner mounted; the
//!   health loop ([`ship_tick`]) streams each new bundle there over the
//!   mgmt socket (`GET /mgmt/v1/backups/{id}/archive`) and keeps the newest
//!   [`SHIP_KEEP`]. `none` is allowed and audited (alarm
//!   `k2_backup_offbox_none`). `k2` (an Alakazam target) is not built.
//! - **`POST /cli/sew/server/mirror {kind, path?}`**: Sew's keyring mirror
//!   (`keys.mirror.*` through the root helper's `engine sew config`, then a
//!   restart). Never the K2 vault or a system path (root-proto validator).
//! - **`GET /cli/sew/app-spaces`, `POST /cli/sew/app-spaces/{bind,unbind}`**
//!   (K31): which Sew Space belongs to which App. Membership then follows
//!   that App's user grants only ([`k2_core::tool_hook::app_space`]).
//!
//! The public client-API host of the S32 PRD (§8.9, `/cli/sew/server/host`)
//! is NOT here: the Hearth answers (2026-10-14) keep Sew's raw API
//! loopback-only; Apps reach it through their own front ends. The public
//! URL Sew builds links from is S29's `/cli/sew/server/config`.
//!
//! Gates: every mutation is Manage Sew (owner / Owner-Admin login / an
//! agent whose workspace has `chat_manage` on), POST-only, Admin floor in
//! `route_policy`. Reads take any real credential.
//!
//! K26 emit points ([`super::supervisor::changed`] → `sew_changed`):
//! `app-url`, `ship`, `mirror`, `app-space`, `backup` (the health loop saw
//! a new bundle or a backup error), `backup-shipped`, `ship-failed`; the
//! grant-hook worker emits `grant-hook` once per pass that changed a row.
//! `approvals` arrives with S30's queue.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::routes::{need_manage, Who};
use super::store;
use super::supervisor::{self as sup, no, Env, R};

pub const APP_URL: &str = "/cli/sew/server/app-url";
pub const SHIP: &str = "/cli/sew/server/ship";
pub const MIRROR: &str = "/cli/sew/server/mirror";
pub const APP_SPACES: &str = "/cli/sew/app-spaces";
pub const APP_SPACES_BIND: &str = "/cli/sew/app-spaces/bind";
pub const APP_SPACES_UNBIND: &str = "/cli/sew/app-spaces/unbind";

pub const ROUTES: &[&str] = &[APP_URL, SHIP, MIRROR, APP_SPACES, APP_SPACES_BIND, APP_SPACES_UNBIND];
pub const POST_ROUTES: &[&str] = &[APP_URL, SHIP, MIRROR, APP_SPACES_BIND, APP_SPACES_UNBIND];

/// Bundles kept in an off-box folder (S29 §15.2).
pub const SHIP_KEEP: usize = 7;
/// The ship state record (`~/.k2/sew/ship.json`, no secret).
pub const SHIP_RECORD: &str = "ship";
/// Longest app URL.
pub const APP_URL_MAX: usize = 512;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

fn need_credential(who: &Who) -> R<()> {
    match who {
        Who::Invalid => Err(no("forbidden", 403, "invalid or missing token")),
        Who::Wrong => Err(no("wrong_credential", 403, "App passes, API keys and API sessions can't manage Sew")),
        _ => Ok(()),
    }
}

fn unsupported() -> sup::Fail {
    no("unsupported", 409, "Sew runs on Linux boxes only (Ubuntu 24.04 / Debian 12+, systemd 250 or newer)")
}

fn need_row() -> R<store::Row> {
    let r = store::load().ok_or_else(|| no("not_installed", 404, "Sew is not installed on this box (k2 hostchat enable)"))?;
    if r.status == "removed" {
        return Err(no("not_installed", 409, "Sew was uninstalled (data kept): k2 hostchat enable brings it back"));
    }
    Ok(r)
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim)
}

// ── K26 emit ─────────────────────────────────────────────────────────────

#[cfg(test)]
thread_local! {
    static EMITTED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests: the reasons this thread emitted since the last call (drained).
#[cfg(test)]
pub fn take_emitted() -> Vec<String> {
    EMITTED.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

/// `sew:changed {reason}` → the app-level `sew_changed` event (K26). A
/// refetch signal only: the reason, never a path, key or member.
pub fn emit(reason: &str) {
    #[cfg(test)]
    EMITTED.with(|e| e.borrow_mut().push(reason.to_string()));
    sup::changed(reason);
}

// ── you: what this caller may do ─────────────────────────────────────────

/// `status.you`. A hint for clients (enable / grey out); every route still
/// checks on its own (S32-A5: a 200 on status is not permission).
///
/// Person-only steps follow S29's gate: purge is the owner's (token or
/// Owner login); the recovery key and `drop-prev` are the owner's or an
/// Admin login's (S29 shipped `PersonFloor::Admin` in 0.46.3); each with a
/// vault confirm, never a passport.
pub fn you(who: &Who) -> Value {
    let passport = who.passport().is_some();
    let person = !passport && need_credential(who).is_ok();
    json!({
        "canManage": need_manage(who).is_ok(),
        "isOwner": person && who.is_owner(),
        "isAdmin": person && who.is_admin(),
        "passport": passport,
        "canRecoveryKey": person && if RECOVERY_KEY_ADMIN { who.is_admin() } else { who.is_owner() },
        "canPurge": person && who.is_owner(),
        // `chat_pass::mint::owner_gate`: the owner token or an Owner login.
        "canOpenAsOwner": person && who.is_owner(),
    })
}

/// Whether an Admin login (with its own confirm) may reveal the recovery
/// key: `true` since S29 landed (0.46.3, `route_policy` Admin floor).
pub const RECOVERY_KEY_ADMIN: bool = true;

// ── status decoration ───────────────────────────────────────────────────

#[cfg(test)]
thread_local! {
    static READ_HOOK_IN_TESTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests: let [`decorate_status`] read the grant hook (`skin.db` under the
/// test's temp HOME). Off by default, so no test reads a real `~/.k2`.
#[cfg(test)]
pub fn read_hook_in_tests(on: bool) {
    READ_HOOK_IN_TESTS.with(|c| c.set(on));
}

/// The `chat` hook as the status shows it: `(owner state, target on)`.
/// Never creates `skin.db` (no target yet = `("none", false)`).
fn hook_view() -> (&'static str, bool) {
    use k2_core::tool_hook::outbox::OwnerState;
    #[cfg(test)]
    if !READ_HOOK_IN_TESTS.with(|c| c.get()) {
        return ("none", false);
    }
    let Some(t) = k2_core::tool_hook::list_targets()
        .ok()
        .and_then(|ts| ts.into_iter().find(|t| t.kind == k2_core::tool_hook::CHAT_KIND))
    else {
        return ("none", false);
    };
    let owner = match k2_core::tool_hook::owner_state(k2_core::tool_hook::CHAT_KIND) {
        Ok(OwnerState::Set) => "set",
        Ok(OwnerState::Pending) => "pending",
        Ok(OwnerState::Blocked) => "blocked",
        Ok(OwnerState::None) | Err(_) => "none",
    };
    (owner, t.enabled)
}

/// Add the S32 fields to a `GET /cli/sew/status` body (K26).
pub fn decorate_status(env: &Env, body: &mut Value, who: &Who) {
    body["you"] = you(who);
    let Some(row) = store::load() else { return };
    if !env.ops.linux() {
        return;
    }
    let (owner, enabled) = hook_view();
    body["owner"] = json!(owner);
    body["target"] = json!({"enabled": enabled});
    body["mirror"] = json!({"kind": row.mirror_kind, "path": row.mirror_path});
    let rec = env.ops.record_load(SHIP_RECORD).unwrap_or(Value::Null);
    let ship = json!({
        "kind": row.ship_kind,
        "path": row.ship_path,
        "lastShippedId": rec.get("lastShippedId").cloned().unwrap_or(Value::Null),
        "lastShippedAt": rec.get("lastShippedAt").cloned().unwrap_or(Value::Null),
        "lastError": rec.get("lastError").cloned().unwrap_or(Value::Null),
        "lastErrorAt": rec.get("lastErrorAt").cloned().unwrap_or(Value::Null),
    });
    if body.get("backup").is_some_and(Value::is_object) {
        body["backup"]["ship"] = ship;
    } else {
        body["backup"] = json!({"ship": ship});
    }
}

/// K2-side alarms S32 adds (S29 §12.3 table): a `dir` destination whose
/// last copy failed.
pub fn k2_alarms(env: &Env, row: &store::Row) -> Vec<String> {
    let mut a = Vec::new();
    if row.ship_kind == "dir" {
        let failed = env.ops.record_load(SHIP_RECORD).is_some_and(|r| r.get("lastError").is_some_and(|e| !e.is_null()));
        if failed {
            a.push("k2_backup_not_shipped".into());
        }
    }
    a
}

// ── app URL ──────────────────────────────────────────────────────────────

/// `https://<host>[:port][/path]`, or `http://127.0.0.1|localhost[:port][/path]`
/// for development; no query, no fragment, ≤ 512. `""` = clear (`None`).
/// Returns the stored form (trailing `/` dropped, host lowercased).
pub fn check_app_url(raw: &str) -> R<Option<String>> {
    let u = raw.trim();
    if u.is_empty() {
        return Ok(None);
    }
    let bad = |why: &str| no("invalid_url", 400, format!("{why}: the app URL is https://<host>[:port][/path] (or http://127.0.0.1 / localhost for development)"));
    if u.len() > APP_URL_MAX {
        return Err(bad("too long"));
    }
    if !u.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(bad("spaces or non-ASCII characters"));
    }
    if u.contains('?') || u.contains('#') {
        return Err(bad("no query or fragment"));
    }
    let (scheme, rest) = u.split_once("://").ok_or_else(|| bad("no scheme"))?;
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err(bad("no user info"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => {
            let n: u32 = p.parse().map_err(|_| bad("bad port"))?;
            if n == 0 || n > 65_535 {
                return Err(bad("bad port"));
            }
            (h, Some(n))
        }
        None => (authority, None),
    };
    let host = host.to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    match scheme.as_str() {
        "https" => {
            if host.len() > 253 || !host.split('.').all(label_ok) {
                return Err(bad("bad host name"));
            }
        }
        "http" => {
            if host != "127.0.0.1" && host != "localhost" {
                return Err(bad("plain http only for 127.0.0.1 or localhost"));
            }
        }
        _ => return Err(bad("the scheme is https")),
    }
    if path.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(bad("no . or .. in the path"));
    }
    let mut out = format!("{scheme}://{host}");
    if let Some(p) = port {
        out.push_str(&format!(":{p}"));
    }
    out.push_str(path.trim_end_matches('/'));
    Ok(Some(out))
}

fn set_app_url(env: &Env, raw: &str) -> R<Value> {
    need_row()?;
    let url = check_app_url(raw)?;
    store::update(env.ops.now(), |r| r.app_url = url.clone()).map_err(|e| no("db_error", 500, e))?;
    emit("app-url");
    Ok(json!({"ok": true, "appUrl": url}))
}

// ── off-box copies ──────────────────────────────────────────────────────

/// What a ship wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipReceipt {
    pub bytes: u64,
    pub sha256: String,
}

/// The filesystem + archive side of shipping, behind a trait so the route
/// and tick rules run in tests without a disk or a Sew.
pub trait ShipFs: Sync {
    /// The destination folder exists, is a folder the daemon user can
    /// write, and is NOT on the disk Sew's data lives on.
    fn check_dir(&self, root: &str) -> Result<(), String>;
    /// Stream bundle `bundle_id` from Sew into `<root>/<install>/`.
    fn ship(&self, env: &Env, root: &str, install_id: &str, bundle_id: &str) -> Result<ShipReceipt, String>;
    /// Keep the newest `keep` bundles there; returns how many went.
    fn prune(&self, root: &str, install_id: &str, keep: usize) -> Result<usize, String>;
}

/// The real disk and Sew's mgmt socket.
pub struct RealShipFs;

pub static REAL_FS: RealShipFs = RealShipFs;

#[cfg(test)]
thread_local! {
    static TEST_FS: std::cell::RefCell<Option<&'static dyn ShipFs>> = const { std::cell::RefCell::new(None) };
}

/// Tests: route this thread's ship/mirror checks to `fs`.
#[cfg(test)]
pub fn use_fs_for_tests(fs: Option<&'static dyn ShipFs>) {
    TEST_FS.with(|f| *f.borrow_mut() = fs);
}

fn fs() -> &'static dyn ShipFs {
    #[cfg(test)]
    if let Some(f) = TEST_FS.with(|f| *f.borrow()) {
        return f;
    }
    &REAL_FS
}

/// Bundle file name in an off-box folder.
pub fn bundle_file(bundle_id: &str) -> String {
    format!("sew-backup-{bundle_id}.tar")
}

/// Syntax: absolute, normalised, outside system folders, `/var/lib/sew`
/// and any `~/.k2` (the root-proto mirror rule: never the vault).
pub fn offbox_path_ok(p: &str) -> bool {
    k2_root_proto::sew::mirror_path_ok(p)
}

#[cfg(unix)]
fn same_device(a: &Path, b: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    Some(std::fs::metadata(a).ok()?.dev() == std::fs::metadata(b).ok()?.dev())
}

#[cfg(not(unix))]
fn same_device(_a: &Path, _b: &Path) -> Option<bool> {
    None
}

/// The real folder checks (no syntax check: [`offbox_path_ok`] runs first).
/// `data_dir` is Sew's data folder (`/var/lib/sew`; a test folder in tests).
pub fn check_dir_on_disk(root: &Path, data_dir: &Path) -> Result<(), String> {
    let md = std::fs::metadata(root).map_err(|e| format!("{} can't be read: {e}", root.display()))?;
    if !md.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    if same_device(root, data_dir) == Some(true) {
        return Err(format!(
            "{} is on the same disk as Sew's data ({}): an off-box copy must be on another disk (an NFS/SMB/sshfs mount or a replicated volume)",
            root.display(),
            data_dir.display()
        ));
    }
    let mut r = [0u8; 6];
    let _ = getrandom::getrandom(&mut r);
    let probe = root.join(format!(".k2-sew-probe-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    std::fs::write(&probe, b"k2").map_err(|e| format!("K2 can't write to {}: {e}", root.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

impl ShipFs for RealShipFs {
    fn check_dir(&self, root: &str) -> Result<(), String> {
        check_dir_on_disk(Path::new(root), Path::new(k2_root_proto::sew::SEW_DATA_DIR))
    }
    fn ship(&self, env: &Env, root: &str, install_id: &str, bundle_id: &str) -> Result<ShipReceipt, String> {
        if !k2_core::tool_hook::sew_wire::is_sew_id(bundle_id) || !k2_core::tool_hook::sew_wire::is_sew_id(install_id) {
            return Err(format!("unexpected bundle or install id ({bundle_id} / {install_id})"));
        }
        let dir = Path::new(root).join(install_id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let fin = dir.join(bundle_file(bundle_id));
        let part = dir.join(format!("{}.partial", bundle_file(bundle_id)));
        let sock = Path::new(k2_root_proto::sew::SEW_MGMT_SOCKET);
        let mut last = String::new();
        for refresh in [false, true] {
            let token = sup::mgmt_token(env, refresh)?;
            let mut f = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            match download_archive(sock, &token, bundle_id, &mut f) {
                Ok(receipt) => {
                    f.sync_all().map_err(|e| format!("fsync {}: {e}", part.display()))?;
                    drop(f);
                    std::fs::rename(&part, &fin).map_err(|e| format!("rename {}: {e}", fin.display()))?;
                    if let Ok(d) = std::fs::File::open(&dir) {
                        let _ = d.sync_all();
                    }
                    return Ok(receipt);
                }
                Err(sup_err) => {
                    let _ = std::fs::remove_file(&part);
                    match sup_err {
                        super::ops::MgmtError::Unauthorized if !refresh => {
                            last = "mgmt token rejected".into();
                            continue;
                        }
                        e => return Err(e.to_string()),
                    }
                }
            }
        }
        Err(last)
    }
    fn prune(&self, root: &str, install_id: &str, keep: usize) -> Result<usize, String> {
        prune_dir(&Path::new(root).join(install_id), keep)
    }
}

/// Keep the newest `keep` `sew-backup-*.tar` (bundle ids sort by time).
pub fn prune_dir(dir: &Path, keep: usize) -> Result<usize, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("sew-backup-") && n.ends_with(".tar"))
        .collect();
    names.sort();
    let drop_n = names.len().saturating_sub(keep);
    for n in &names[..drop_n] {
        std::fs::remove_file(dir.join(n)).map_err(|e| format!("{n}: {e}"))?;
    }
    Ok(drop_n)
}

/// Idle read timeout while streaming an archive (no whole-call timeout:
/// a large bundle takes as long as it takes).
const ARCHIVE_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

/// `GET /mgmt/v1/backups/{id}/archive` over the mgmt socket, streamed into
/// `out` (Content-Length, chunked, or to EOF). Returns size and sha256.
pub fn download_archive(sock: &Path, token: &str, bundle_id: &str, out: &mut dyn Write) -> Result<ShipReceipt, super::ops::MgmtError> {
    use super::ops::MgmtError as E;
    use sha2::{Digest, Sha256};
    #[cfg(unix)]
    {
        let mut s = std::os::unix::net::UnixStream::connect(sock).map_err(|e| E::Unreachable(e.to_string()))?;
        s.set_read_timeout(Some(ARCHIVE_IDLE)).map_err(|e| E::Unreachable(e.to_string()))?;
        s.set_write_timeout(Some(ARCHIVE_IDLE)).map_err(|e| E::Unreachable(e.to_string()))?;
        let req = format!(
            "GET /mgmt/v1/backups/{bundle_id}/archive HTTP/1.1\r\nHost: sew\r\nAuthorization: Bearer {token}\r\nConnection: close\r\nSew-Actor: k2\r\n\r\n"
        );
        s.write_all(req.as_bytes()).map_err(|e| E::Unreachable(e.to_string()))?;
        let mut r = std::io::BufReader::new(s);
        // Head.
        let mut head = Vec::new();
        loop {
            let mut b = [0u8; 1];
            let n = r.read(&mut b).map_err(|e| E::Unreachable(e.to_string()))?;
            if n == 0 {
                return Err(E::Unreachable("truncated HTTP answer".into()));
            }
            head.push(b[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > 64 * 1024 {
                return Err(E::Unreachable("HTTP head too large".into()));
            }
        }
        let head = String::from_utf8_lossy(&head).to_string();
        let mut lines = head.lines();
        let code: u16 = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| E::Unreachable("bad HTTP status line".into()))?;
        let header = |name: &str| {
            head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_ascii_lowercase())
            })
        };
        if code == 401 {
            return Err(E::Unauthorized);
        }
        if !(200..300).contains(&code) {
            let mut b = Vec::new();
            let _ = r.take(4096).read_to_end(&mut b);
            return Err(E::Status(code, String::from_utf8_lossy(&b).chars().take(400).collect()));
        }
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut sink = |chunk: &[u8]| -> Result<(), E> {
            out.write_all(chunk).map_err(|e| E::Unreachable(format!("write: {e}")))?;
            hasher.update(chunk);
            total += chunk.len() as u64;
            Ok(())
        };
        let mut buf = vec![0u8; 64 * 1024];
        if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
            loop {
                let mut line = String::new();
                std::io::BufRead::read_line(&mut r, &mut line).map_err(|e| E::Unreachable(e.to_string()))?;
                let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16)
                    .map_err(|_| E::Unreachable("bad chunk size".into()))?;
                if size == 0 {
                    break;
                }
                let mut left = size;
                while left > 0 {
                    let want = left.min(buf.len());
                    r.read_exact(&mut buf[..want]).map_err(|e| E::Unreachable(format!("archive cut short: {e}")))?;
                    sink(&buf[..want])?;
                    left -= want;
                }
                let mut crlf = [0u8; 2];
                r.read_exact(&mut crlf).map_err(|e| E::Unreachable(e.to_string()))?;
            }
        } else if let Some(len) = header("content-length").and_then(|v| v.parse::<u64>().ok()) {
            let mut left = len;
            while left > 0 {
                let want = (left as usize).min(buf.len());
                r.read_exact(&mut buf[..want]).map_err(|e| E::Unreachable(format!("archive cut short: {e}")))?;
                sink(&buf[..want])?;
                left -= want as u64;
            }
        } else {
            loop {
                let n = r.read(&mut buf).map_err(|e| E::Unreachable(e.to_string()))?;
                if n == 0 {
                    break;
                }
                sink(&buf[..n])?;
            }
        }
        let sha256 = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        Ok(ShipReceipt { bytes: total, sha256 })
    }
    #[cfg(not(unix))]
    {
        let _ = (sock, token, bundle_id, out);
        Err(E::Unreachable("not a unix box".into()))
    }
}

fn set_ship(env: &Env, who: &Who, v: &Value) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    need_row()?;
    let (kind, path) = match s(v, "kind").unwrap_or("") {
        "none" => ("none", None),
        "dir" => {
            let p = s(v, "path").filter(|p| !p.is_empty()).ok_or_else(|| no("bad_request", 400, "kind dir needs path (a folder on another disk)"))?;
            if !offbox_path_ok(p) {
                return Err(no(
                    "invalid_path",
                    400,
                    "the folder must be an absolute path outside system folders, /var/lib/sew and ~/.k2 (never the K2 vault)",
                ));
            }
            fs().check_dir(p).map_err(|e| no("invalid_path", 400, e))?;
            ("dir", Some(p.to_string()))
        }
        "k2" => return Err(no("not_available", 400, "an Alakazam-hosted backup destination is not built yet: use a folder on another disk (kind dir)")),
        _ => return Err(no("bad_request", 400, "kind is none or dir")),
    };
    let now = env.ops.now();
    store::update(now, |r| {
        r.ship_kind = kind.to_string();
        r.ship_path = path.clone();
    })
    .map_err(|e| no("db_error", 500, e))?;
    // A new destination ships the latest bundle again on the next tick.
    let mut rec = env.ops.record_load(SHIP_RECORD).unwrap_or_else(|| json!({}));
    for k in ["lastShippedId", "lastShippedAt", "lastError", "lastErrorAt"] {
        rec[k] = Value::Null;
    }
    env.ops.record_save(SHIP_RECORD, Some(&rec)).map_err(|e| no("io_error", 500, e))?;
    if kind == "none" {
        // Backups stay on this box only: audited (plan §2).
        crate::vault::auth_audit("sew-ship-none", &who.label(), "backups stay on this box", "cli");
    }
    emit("ship");
    Ok(json!({"ok": true, "ship": {"kind": kind, "path": path}}))
}

fn set_mirror(env: &Env, v: &Value) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    need_row()?;
    let (kind, path) = match s(v, "kind").unwrap_or("") {
        "none" => ("none", None),
        "dir" => {
            let p = s(v, "path").filter(|p| !p.is_empty()).ok_or_else(|| no("bad_request", 400, "kind dir needs path (a folder on another disk)"))?;
            if !offbox_path_ok(p) {
                return Err(no(
                    "invalid_path",
                    400,
                    "the keyring mirror must be an absolute path outside system folders, /var/lib/sew and ~/.k2 (never the K2 vault)",
                ));
            }
            if env.ops.path_probe(p) == Some(false) {
                return Err(no("invalid_path", 400, format!("{p} does not exist: mount the off-box folder first")));
            }
            ("dir", Some(p.to_string()))
        }
        "k2" => return Err(no("not_available", 400, "an Alakazam-hosted key copy is not built yet: use a folder on another disk (kind dir)")),
        _ => return Err(no("bad_request", 400, "kind is none or dir")),
    };
    sup::helper_check(env)?;
    let _latch = sup::begin(env)?;
    let mut cfg = serde_json::Map::new();
    if let Some(p) = &path {
        cfg.insert("keys.mirror.path".into(), json!(p));
    }
    cfg.insert("keys.mirror.kind".into(), json!(kind));
    sup::sew_run(env, &["config"], Some(Value::Object(cfg).to_string().as_bytes()))
        .map_err(|e| no("config_failed", 409, format!("Sew refused the keyring mirror: {e}")))?;
    let restarted = if env.ops.unit(k2_root_proto::sew::UNIT_SEW).is_active() {
        sup::helper_json(env, &["systemctl", "restart"]).map_err(|e| no("restart_failed", 500, e))?;
        true
    } else {
        false
    };
    store::update(env.ops.now(), |r| {
        r.mirror_kind = kind.to_string();
        r.mirror_path = path.clone();
    })
    .map_err(|e| no("db_error", 500, e))?;
    emit("mirror");
    Ok(json!({"ok": true, "mirror": {"kind": kind, "path": path}, "restarted": restarted}))
}

/// One health-loop step: note Sew's newest bundle / backup error (K26
/// `backup` signal) and copy a new bundle to the `dir` destination.
/// Returns the reason it emitted, if any.
pub fn ship_tick(env: &Env) -> Option<String> {
    ship_tick_with(env, &REAL_FS)
}

pub fn ship_tick_with(env: &Env, fs: &dyn ShipFs) -> Option<String> {
    if env.latch.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let row = store::load()?;
    if !matches!(row.status.as_str(), "running" | "degraded") {
        return None;
    }
    let st = sup::mgmt(env, "GET", "/mgmt/v1/status", None).ok()?;
    let last_id = st.pointer("/backup/last_id").and_then(Value::as_str).map(str::to_string);
    let last_err = st.pointer("/backup/last_error").and_then(Value::as_str).map(str::to_string);
    let mut rec = env.ops.record_load(SHIP_RECORD).unwrap_or_else(|| json!({}));
    let mut reason: Option<&str> = None;
    if rec.get("seenBackupId").and_then(Value::as_str) != last_id.as_deref()
        || rec.get("seenBackupError").and_then(Value::as_str) != last_err.as_deref()
    {
        rec["seenBackupId"] = json!(last_id);
        rec["seenBackupError"] = json!(last_err);
        reason = Some("backup");
    }
    if let (true, Some(root), Some(id), Some(install)) = (row.ship_kind == "dir", row.ship_path.as_deref(), last_id.as_deref(), row.install_id.as_deref()) {
        if rec.get("lastShippedId").and_then(Value::as_str) != Some(id) {
            let now = env.ops.now();
            match fs.check_dir(root).and_then(|()| fs.ship(env, root, install, id)) {
                Ok(r) => {
                    rec["lastShippedId"] = json!(id);
                    rec["lastShippedAt"] = json!(now);
                    rec["lastBytes"] = json!(r.bytes);
                    rec["lastSha256"] = json!(r.sha256);
                    rec["lastError"] = Value::Null;
                    rec["lastErrorAt"] = Value::Null;
                    if let Err(e) = fs.prune(root, install, SHIP_KEEP) {
                        k2_core::log_debug!("[sew] ship prune: {e}");
                    }
                    reason = Some("backup-shipped");
                }
                Err(e) => {
                    let e: String = e.chars().take(400).collect();
                    let same = rec.get("lastError").and_then(Value::as_str) == Some(e.as_str());
                    rec["lastError"] = json!(e);
                    rec["lastErrorAt"] = json!(now);
                    if !same {
                        reason = Some("ship-failed");
                    }
                }
            }
        }
    }
    if let Some(r) = reason {
        if let Err(e) = env.ops.record_save(SHIP_RECORD, Some(&rec)) {
            k2_core::log_debug!("[sew] ship record: {e}");
        }
        emit(r);
    }
    reason.map(str::to_string)
}

// ── App Spaces (K31) ─────────────────────────────────────────────────────

fn app_spaces_list() -> R<Value> {
    let v = k2_core::tool_hook::app_spaces().map_err(|e| no("db_error", 500, e))?;
    Ok(json!({
        "ok": true,
        "spaces": v,
        "note": "members come only from each App's user grants (k2 app grant); Admin Access never adds anyone",
    }))
}

fn app_space_bind(env: &Env, who: &Who, v: &Value) -> R<Value> {
    // Sew for Apps: a runtime App's id or an App's name resolves to its
    // service id (Sew's `app_id` is the pass's `app` claim); anything else
    // is taken as given, as before.
    let app = match s(v, "app").filter(|a| !a.is_empty()) {
        Some(a) => super::home::resolve_app(a).map(|r| r.0).unwrap_or_else(|_| a.to_ascii_lowercase()),
        None => String::new(),
    };
    let space = s(v, "spaceId").unwrap_or("").to_string();
    k2_core::tool_hook::app_space::check_ids(&app, &space).map_err(|e| no("bad_request", 400, e))?;
    let row = need_row()?;
    if !matches!(row.status.as_str(), "running" | "degraded") {
        return Err(no("sew_not_running", 409, "Sew must be running to bind a Space (k2 hostchat status)"));
    }
    if sup::row_provider(&row) != k2_root_proto::sew::PROVIDER_K2 {
        return Err(no("provider_is_standalone", 409, "a standalone Sew install takes no K2 grants, so it has no App Spaces"));
    }
    // Sew's side first (once; the same App again is a no-op there).
    let body = json!({"space_id": space, "app_id": app});
    match sup::mgmt(env, "POST", k2_core::tool_hook::sew_wire::MGMT_SPACES_BIND, Some(&body)) {
        Ok(_) => {}
        Err(super::ops::MgmtError::Status(409, b)) => {
            return Err(no("space_bound", 409, format!("Sew says that Space belongs to another App: {b}")));
        }
        Err(super::ops::MgmtError::Status(404, _)) => {
            return Err(no("space_not_found", 404, format!("Sew has no Space {space}")));
        }
        Err(super::ops::MgmtError::Status(c, b)) => return Err(no("sew_refused", 502, format!("Sew answered {c}: {b}"))),
        Err(e) => return Err(no("sew_unreachable", 503, e.to_string())),
    }
    let (created, plan) = k2_core::tool_hook::app_space_bind(&app, &space, &who.label()).map_err(|e| {
        if e.starts_with("space_bound") {
            no("space_bound", 409, e)
        } else {
            no("bad_request", 400, e)
        }
    })?;
    emit("app-space");
    Ok(json!({"ok": true, "app": app, "spaceId": space, "created": created, "plan": plan}))
}

fn app_space_unbind(who: &Who, v: &Value) -> R<Value> {
    let app = s(v, "app").unwrap_or("").to_ascii_lowercase();
    let space = s(v, "spaceId").unwrap_or("").to_string();
    if app.is_empty() || space.is_empty() {
        return Err(no("bad_request", 400, "name the App and the Space (app, spaceId)"));
    }
    // S32 Q3 (Rosson 2026-10-27): a choice at unbind time, default keep
    // (reversible). Removing only ever touches the members K2 added.
    let remove = match v.get("removeAppMembers") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(no("bad_request", 400, "removeAppMembers is true or false")),
    };
    let (gone, plan) =
        k2_core::tool_hook::app_space_unbind(&app, &space, &who.label(), remove).map_err(|e| no("db_error", 500, e))?;
    if !gone {
        return Err(no("not_found", 404, format!("Space {space} is not bound to App '{app}' here")));
    }
    emit("app-space");
    let hint = if remove {
        "the members K2 added there are being removed (people added by hand stay)"
    } else {
        "everyone stays in the Space; K2 no longer manages its members (bind again to resume)"
    };
    Ok(json!({"ok": true, "app": app, "spaceId": space, "removeAppMembers": remove, "plan": plan, "hint": hint}))
}

// ── entry ────────────────────────────────────────────────────────────────

/// Route an S32 path. `None` = not an S32 route.
pub fn route(env: &Env, path: &str, who: &Who, body: &[u8], _params: &HashMap<String, String>) -> Option<R<Value>> {
    if !is_route(path) {
        return None;
    }
    let v = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(v) if v.is_object() => v,
            Ok(_) => return Some(Err(no("bad_request", 400, "expected a JSON object"))),
            Err(e) => return Some(Err(no("bad_request", 400, format!("invalid JSON body: {e}")))),
        }
    };
    Some(match path {
        APP_SPACES => need_credential(who).and_then(|()| app_spaces_list()),
        _ => need_manage(who).and_then(|()| match path {
            APP_URL => set_app_url(env, s(&v, "url").ok_or_else(|| no("bad_request", 400, "name url (\"\" clears it)"))?),
            SHIP => set_ship(env, who, &v),
            MIRROR => set_mirror(env, &v),
            APP_SPACES_BIND => app_space_bind(env, who, &v),
            APP_SPACES_UNBIND => app_space_unbind(who, &v),
            _ => Err(no("not_found", 404, "no such Sew route")),
        }),
    })
}

/// Where a shipped bundle lands (for messages and tests).
pub fn shipped_path(root: &str, install_id: &str, bundle_id: &str) -> PathBuf {
    Path::new(root).join(install_id).join(bundle_file(bundle_id))
}

#[cfg(test)]
#[path = "s32_tests.rs"]
mod tests;
