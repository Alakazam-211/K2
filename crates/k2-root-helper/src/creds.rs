//! `creds` namespace (protocol 10, 0.46.4, `prd-vault-gaps-v1` item 4):
//! systemd encrypted credentials for units K2 doesn't run, written for
//! `k2 vault systemd-cred`. Every verb is daemon-class.
//!
//! - `creds status`: systemd version, `systemd-creds has-tpm2`, the
//!   credentials and drop-ins THIS daemon wrote.
//! - `creds put <cred>`: stdin = the value (≤ 64 KiB, never argv, never in
//!   the audit line) → `systemd-creds encrypt --name=<cred> --with-key=auto
//!   - -` → `/etc/k2/credstore.encrypted/<cred>.cred` (K2's own store, root
//!   0700, outside systemd's credential search path), root 0600, temp +
//!   fsync + rename in a directory opened without following links. The
//!   plaintext buffers are wiped after use.
//! - `creds bind <unit> <cred>`: the canned drop-in
//!   `/etc/systemd/system/<unit>.d/k2-cred-<cred>.conf` (one
//!   `LoadCredentialEncrypted=<cred>:<absolute path>` line), then
//!   `daemon-reload`, then a check that systemd now loads K2's path for
//!   that ID.
//! - `creds unbind <unit> <cred>` / `creds remove <cred>`: undo.
//! - `creds restart <unit>`: `systemctl --no-block try-restart <unit>`, at
//!   most once per unit every 30 s.
//!
//! Which units (fail closed): the unit must be on the box's allow list
//! (`/etc/k2-root-helper/systemd-cred-units`, root-owned, written by the
//! box's admin), pass the deny list (K2's own units, its generated
//! families, core system units, every unit `daemon.toml` names, read now),
//! be `loaded` with `Id` equal to the name asked for (no aliases), and
//! every one of its `Names` must pass both lists. It must not already get a
//! credential with the same ID from anywhere else.
//!
//! Ownership: `creds.json` records which daemon (its user, from the caller
//! check) wrote each credential and drop-in, and the sha256 of each `.cred`
//! it wrote. A daemon may overwrite, unbind, remove or restart only its own
//! entries, and only while the file on disk is still the one it wrote. A
//! record is made after a write succeeds, never before; every error path
//! after a write takes the new file back out. Refusal lines are static
//! (caller bytes are never echoed). systemd ≥ 250.

use std::ffi::OsStr;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use k2_root_proto::creds::{
    apply_prop, credential_conflict, credential_is_ours, cred_dropin, cred_path, dropin_file, parse_allow_list,
    parse_systemd_version, parse_unit_path, parse_unit_show, unit_refusal, UnitCreds, ALLOW_LIST_MAX, BUSCTL, CREDSTORE_DIR,
    CREDSTORE_PARENT, CRED_PROPS, CRED_SUFFIX, DROPIN_PREFIX, DROPIN_SUFFIX, MIN_SYSTEMD, OPTIONAL_CRED_PROPS,
    RESTART_MIN_INTERVAL_SECS, SYSTEMD_CREDS,
};
use k2_root_proto::paths::SYSTEMCTL;

use crate::caller::{self, Caller};
use crate::ctx::{Ctx, Fail, HResult};
use crate::daemon_unit::set_dropin;
use crate::fsx::{Dir, Kind};

const STATE_FILE: &str = "creds.json";
const STATE_PARENT: &str = "/var/lib";
const STATE_DIR_NAME: &str = "k2-root-helper";
const STATE_CAP: usize = 256 * 1024;
/// Largest ciphertext `systemd-creds` may hand back for a 64 KiB value.
const CIPHER_CAP: usize = 512 * 1024;

/// One credential K2 wrote.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredEntry {
    pub name: String,
    /// The daemon user that wrote it.
    pub owner: String,
    /// sha256 (hex) of the bytes written: a file changed by hand is no
    /// longer K2's to overwrite or remove.
    pub sha256: String,
}

/// One drop-in K2 wrote.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DropinEntry {
    pub unit: String,
    pub cred: String,
    pub owner: String,
}

/// What K2 wrote (`/var/lib/k2-root-helper/creds.json`, root 0600).
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredsState {
    #[serde(default)]
    pub creds: Vec<CredEntry>,
    #[serde(default)]
    pub dropins: Vec<DropinEntry>,
    /// unit → Unix seconds of its last K2 restart (the rate limit).
    #[serde(default)]
    pub restarts: std::collections::BTreeMap<String, u64>,
}

impl CredsState {
    fn valid(&self) -> bool {
        use k2_root_proto::caller::user_ok;
        use k2_root_proto::creds::{cred_name_ok, unit_name_ok};
        self.creds.iter().all(|c| cred_name_ok(&c.name) && user_ok(&c.owner) && c.sha256.len() == 64)
            && self.dropins.iter().all(|d| unit_name_ok(&d.unit) && cred_name_ok(&d.cred) && user_ok(&d.owner))
            && self.restarts.keys().all(|u| unit_name_ok(u))
    }
    fn cred(&self, c: &str) -> Option<&CredEntry> {
        self.creds.iter().find(|x| x.name == c)
    }
    fn dropin(&self, u: &str, c: &str) -> Option<&DropinEntry> {
        self.dropins.iter().find(|d| d.unit == u && d.cred == c)
    }
}

fn hex_sha(b: &[u8]) -> String {
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// A directory root alone may write: root-owned (in production) and not
/// group/other-writable.
fn root_only(ctx: &Ctx, d: &Dir, what: &'static str) -> HResult<()> {
    let st = d.stat_self().map_err(|e| Fail::io_err("stat", e))?;
    if (st.st_mode as u32) & 0o022 != 0 || (ctx.real_owners && st.st_uid != 0) {
        return Err(Fail::state(what));
    }
    Ok(())
}

fn state_dir(ctx: &Ctx, create: bool) -> HResult<Option<Dir>> {
    let base = ctx.dir(STATE_PARENT).map_err(|e| Fail::io_err("open /var/lib", e))?;
    let name = OsStr::new(STATE_DIR_NAME);
    match base.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing if !create => return Ok(None),
        Kind::Missing => {
            base.mkdir(name, 0o755).map_err(|e| Fail::io_err("mkdir state", e))?;
            let d = base.sub(name).map_err(|e| Fail::io_err("open state dir", e))?;
            if let Some((u, g)) = ctx.root_owner() {
                d.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
            }
            d.fchmod(0o755).map_err(|e| Fail::io_err("chmod state", e))?;
        }
        Kind::Dir => {}
        _ => return Err(Fail::state("state dir is not a directory")),
    }
    let d = base.sub(name).map_err(|e| Fail::io_err("open state dir", e))?;
    root_only(ctx, &d, "refused: /var/lib/k2-root-helper is not root's alone")?;
    Ok(Some(d))
}

pub fn load_state(ctx: &Ctx) -> HResult<CredsState> {
    let Some(d) = state_dir(ctx, false)? else {
        return Ok(CredsState::default());
    };
    let Some((f, st)) = d.open_regular(OsStr::new(STATE_FILE)).map_err(|e| Fail::io_err("open creds state", e))? else {
        return Ok(CredsState::default());
    };
    if (st.st_mode as u32) & 0o022 != 0 || (ctx.real_owners && st.st_uid != 0) {
        return Err(Fail::state("refused: creds.json is not root's alone"));
    }
    use std::io::Read;
    let mut b = Vec::new();
    f.take(STATE_CAP as u64 + 1).read_to_end(&mut b).map_err(|e| Fail::io_err("read creds state", e))?;
    if b.len() > STATE_CAP {
        return Err(Fail::state("creds.json too large"));
    }
    let s: CredsState = serde_json::from_slice(&b).map_err(|_| Fail::state("creds.json unreadable"))?;
    if !s.valid() {
        return Err(Fail::state("creds.json holds an invalid entry"));
    }
    Ok(s)
}

/// Write `creds.json` (root 0600, temp + rename). Public for the live
/// check's cleanup (examples/creds_live.rs).
pub fn save_state(ctx: &Ctx, s: &CredsState) -> HResult<()> {
    let d = state_dir(ctx, true)?.ok_or_else(|| Fail::io("state dir"))?;
    let bytes = serde_json::to_vec_pretty(s).map_err(|_| Fail::io("creds state encode"))?;
    d.write_atomic(OsStr::new(STATE_FILE), &bytes, 0o600, ctx.root_owner())
        .map_err(|e| Fail::io_err("write creds state", e))
}

/// `/etc/k2/credstore.encrypted`, opened without following links; created
/// root 0700 (and `/etc/k2` root 0755) when `create`. Both must be root's
/// alone; the store must be 0700 (no group or other bits at all).
fn credstore(ctx: &Ctx, create: bool) -> HResult<Option<Dir>> {
    let (etc, k2) = CREDSTORE_PARENT.rsplit_once('/').ok_or_else(|| Fail::io("credstore path"))?;
    let (_, name) = CREDSTORE_DIR.rsplit_once('/').ok_or_else(|| Fail::io("credstore path"))?;
    let etc = ctx.dir(etc).map_err(|e| Fail::io_err("open /etc", e))?;
    let k2n = OsStr::new(k2);
    match etc.kind(k2n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing if !create => return Ok(None),
        Kind::Missing => {
            etc.mkdir(k2n, 0o755).map_err(|e| Fail::io_err("mkdir /etc/k2", e))?;
            let d = etc.sub(k2n).map_err(|e| Fail::io_err("open /etc/k2", e))?;
            if let Some((u, g)) = ctx.root_owner() {
                d.fchown(u, g).map_err(|e| Fail::io_err("chown /etc/k2", e))?;
            }
            d.fchmod(0o755).map_err(|e| Fail::io_err("chmod /etc/k2", e))?;
        }
        Kind::Dir => {}
        _ => return Err(Fail::state("refused: /etc/k2 is not a directory")),
    }
    let parent = etc.sub(k2n).map_err(|e| Fail::io_err("open /etc/k2", e))?;
    root_only(ctx, &parent, "refused: /etc/k2 is not root's alone")?;
    let n = OsStr::new(name);
    match parent.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing if !create => return Ok(None),
        Kind::Missing => {
            parent.mkdir(n, 0o700).map_err(|e| Fail::io_err("mkdir credstore", e))?;
            let d = parent.sub(n).map_err(|e| Fail::io_err("open credstore", e))?;
            if let Some((u, g)) = ctx.root_owner() {
                d.fchown(u, g).map_err(|e| Fail::io_err("chown credstore", e))?;
            }
            d.fchmod(0o700).map_err(|e| Fail::io_err("chmod credstore", e))?;
        }
        Kind::Dir => {}
        _ => return Err(Fail::state("refused: /etc/k2/credstore.encrypted is not a directory")),
    }
    let d = parent.sub(n).map_err(|e| Fail::io_err("open credstore", e))?;
    let st = d.stat_self().map_err(|e| Fail::io_err("stat credstore", e))?;
    if (st.st_mode as u32) & 0o077 != 0 || (ctx.real_owners && st.st_uid != 0) {
        return Err(Fail::state("refused: /etc/k2/credstore.encrypted must be root's, mode 0700"));
    }
    Ok(Some(d))
}

fn cred_file(cred: &str) -> String {
    format!("{cred}{CRED_SUFFIX}")
}

/// sha256 of the `.cred` on disk now (`None` = absent). Never through a link.
fn disk_sha(d: &Dir, cred: &str) -> HResult<Option<String>> {
    match d.kind(OsStr::new(&cred_file(cred))).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => Ok(None),
        Kind::File => {
            let b = d
                .read_file(OsStr::new(&cred_file(cred)), CIPHER_CAP)
                .map_err(|e| Fail::io_err("read .cred", e))?
                .unwrap_or_default();
            Ok(Some(hex_sha(&b)))
        }
        _ => Err(Fail::state("refused: that .cred is not a regular file")),
    }
}

fn systemd_version(ctx: &Ctx) -> Option<u32> {
    let out = ctx.exec.run(SYSTEMCTL, &["--version".to_string()], None).ok()?;
    if out.status != 0 {
        return None;
    }
    parse_systemd_version(&String::from_utf8_lossy(&out.stdout))
}

fn need_systemd(ctx: &Ctx) -> HResult<u32> {
    match systemd_version(ctx) {
        Some(v) if v >= MIN_SYSTEMD => Ok(v),
        Some(_) => Err(Fail::state("refused: systemd 250 or newer is needed for encrypted credentials")),
        None => Err(Fail::state("refused: systemctl --version gave no systemd version")),
    }
}

fn has_tpm2(ctx: &Ctx) -> bool {
    ctx.exec.run(SYSTEMD_CREDS, &["has-tpm2".to_string()], None).is_ok_and(|o| o.status == 0)
}

/// The box's allow list, read now. Missing = empty (nothing allowed). It
/// must be a root-owned regular file, not group/other-writable, read
/// without following a link; any bad line refuses the whole file.
pub fn allow_list(ctx: &Ctx) -> HResult<Vec<String>> {
    let (dir, name) = k2_root_proto::creds::ALLOW_LIST.rsplit_once('/').ok_or_else(|| Fail::io("allow list path"))?;
    let d = match ctx.dir(dir) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(Fail::io_err("open /etc/k2-root-helper", e)),
    };
    // The directory too: whoever can write it can swap the file (B2-N5).
    root_only(ctx, &d, "refused: /etc/k2-root-helper is not root's alone")?;
    let Some((f, st)) = d.open_regular(OsStr::new(name)).map_err(|_| Fail::state("refused: the systemd-cred allow list is not a regular file"))? else {
        return Ok(vec![]);
    };
    if (st.st_mode as u32) & 0o022 != 0 || (ctx.real_owners && st.st_uid != 0) {
        return Err(Fail::state("refused: the systemd-cred allow list is not root's alone"));
    }
    use std::io::Read;
    let mut b = Vec::new();
    f.take(ALLOW_LIST_MAX as u64 + 1).read_to_end(&mut b).map_err(|e| Fail::io_err("read allow list", e))?;
    if b.len() > ALLOW_LIST_MAX {
        return Err(Fail::state("refused: the systemd-cred allow list is too large"));
    }
    let text = std::str::from_utf8(&b).map_err(|_| Fail::state("refused: the systemd-cred allow list is not text"))?;
    parse_allow_list(text).map_err(Fail::state)
}

/// The daemon units `daemon.toml` names, read now (never cached).
fn daemon_units(ctx: &Ctx) -> HResult<Vec<String>> {
    let ds = caller::load_daemons(ctx).map_err(Fail::state)?;
    Ok(ds.into_iter().map(|d| d.unit).collect())
}

/// The unit, resolved by systemd and checked on every name it has: allow
/// list + deny list, `loaded`, `Id` = the name asked for.
fn resolve_unit(ctx: &Ctx, unit: &str) -> HResult<()> {
    let allow = allow_list(ctx)?;
    let daemons = daemon_units(ctx)?;
    // Static words: the unit (caller bytes) is never echoed.
    if let Some(why) = unit_refusal(unit, &allow, &daemons) {
        return Err(Fail::argv(format!("refused unit: {why}")));
    }
    let args: Vec<String> = ["show", "-p", "LoadState", "-p", "Id", "-p", "Names", unit].iter().map(|s| s.to_string()).collect();
    let out = ctx
        .exec
        .run(SYSTEMCTL, &args, None)
        .map_err(|e| Fail::io(format!("{SYSTEMCTL}: {}", crate::ctx::KindStr::kind_str(&e))))?;
    if out.status != 0 {
        return Err(Fail::state("refused: systemd knows no such unit"));
    }
    let show = parse_unit_show(&String::from_utf8_lossy(&out.stdout));
    if show.load_state != "loaded" {
        return Err(Fail::state("refused: the unit is not loaded (missing, masked or broken)"));
    }
    if show.id != unit {
        return Err(Fail::argv("refused unit: that name is an alias; name the unit itself"));
    }
    if show.names.is_empty() {
        return Err(Fail::state("refused: systemd gave no names for the unit"));
    }
    for n in &show.names {
        if let Some(why) = unit_refusal(n, &allow, &daemons) {
            return Err(Fail::argv(format!("refused unit: one of its names is {why}")));
        }
    }
    Ok(())
}

fn busctl(ctx: &Ctx, args: &[&str]) -> HResult<crate::ctx::ExecOut> {
    let mut a: Vec<String> = vec!["--json=short".into()];
    a.extend(args.iter().map(|s| s.to_string()));
    ctx.exec.run(BUSCTL, &a, None).map_err(|e| Fail::io(format!("{BUSCTL}: {}", crate::ctx::KindStr::kind_str(&e))))
}

/// The unit's credentials as systemd holds them (every file loaded), read
/// over D-Bus with busctl: `systemctl show` prints these properties as
/// `[unprintable]` or not at all (review A2-N1). Any error or unexpected
/// shape refuses (fail closed); only `ImportCredential` may be absent
/// (systemd < 254).
fn unit_creds(ctx: &Ctx, unit: &str) -> HResult<UnitCreds> {
    // LoadUnit, not GetUnit: an inactive, unreferenced unit is
    // garbage-collected between calls, and GetUnit refuses it (review
    // A3-P1). Same `s → o` signature; it loads the unit when needed.
    let out = busctl(
        ctx,
        &["call", "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "LoadUnit", "s", unit],
    )?;
    let path = (out.status == 0)
        .then(|| parse_unit_path(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
        .ok_or_else(|| Fail::state("refused: systemd gave no object for the unit (busctl LoadUnit)"))?;
    let mut c = UnitCreds::default();
    for (prop, _) in CRED_PROPS {
        let out = busctl(ctx, &["get-property", "org.freedesktop.systemd1", &path, "org.freedesktop.systemd1.Service", prop])?;
        if out.status != 0 {
            if OPTIONAL_CRED_PROPS.contains(prop) {
                continue;
            }
            return Err(Fail::state("refused: can't read the unit's credentials (busctl get-property)"));
        }
        apply_prop(&mut c, prop, &String::from_utf8_lossy(&out.stdout)).map_err(Fail::state)?;
    }
    Ok(c)
}

fn reload(ctx: &Ctx) -> HResult<()> {
    ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None).map(|_| ())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Overwrite a plaintext buffer before it is freed.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

const NOT_YOURS: &str = "refused: another K2 daemon on this box owns that";

pub fn put(ctx: &Ctx, who: &Caller, cred: &str, value: &[u8]) -> HResult<serde_json::Value> {
    let me = caller::require_daemon(who)?;
    if value.is_empty() {
        return Err(Fail::argv("refused: empty value"));
    }
    need_systemd(ctx)?;
    let mut st = load_state(ctx)?;
    let dir = credstore(ctx, true)?.ok_or_else(|| Fail::io("credstore"))?;
    let on_disk = disk_sha(&dir, cred)?;
    match (st.cred(cred), &on_disk) {
        (Some(e), _) if e.owner != me => return Err(Fail::state(NOT_YOURS)),
        (Some(e), Some(sha)) if *sha != e.sha256 => {
            return Err(Fail::state("refused: that .cred was changed by hand since K2 wrote it"))
        }
        (None, Some(_)) => return Err(Fail::state("refused: that .cred exists and K2 didn't write it")),
        _ => {}
    }
    let args = vec![
        "encrypt".to_string(),
        format!("--name={cred}"),
        "--with-key=auto".to_string(),
        "-".to_string(),
        "-".to_string(),
    ];
    let out = ctx
        .exec
        .run(SYSTEMD_CREDS, &args, Some(value))
        .map_err(|e| Fail::io(format!("{SYSTEMD_CREDS}: {}", crate::ctx::KindStr::kind_str(&e))))?;
    let mut cipher = out.stdout;
    if out.status != 0 {
        wipe(&mut cipher);
        let err = String::from_utf8_lossy(&out.stderr);
        let err: String = err.trim().chars().filter(|c| !c.is_control()).take(300).collect();
        // Never let the value into an error line, even if a tool echoed it.
        let leaks = std::str::from_utf8(value).ok().filter(|v| v.len() >= 4).is_some_and(|v| err.contains(v));
        let err = if leaks { "(output withheld)".to_string() } else { err };
        return Err(Fail::io(format!("systemd-creds encrypt exit {}: {err}", out.status)));
    }
    if cipher.is_empty() || cipher.len() > CIPHER_CAP {
        wipe(&mut cipher);
        return Err(Fail::io("systemd-creds encrypt gave no credential"));
    }
    let sha = hex_sha(&cipher);
    let f = cred_file(cred);
    let res = dir.write_atomic(OsStr::new(&f), &cipher, 0o600, ctx.root_owner());
    let bytes = cipher.len();
    wipe(&mut cipher);
    res.map_err(|e| Fail::io_err("write .cred", e))?;
    // Recorded after the write; a failed record takes a NEW file back out
    // (an overwritten one stays, still recorded under its old hash, and
    // the next put or remove refuses it as changed: loud, never silent).
    st.creds.retain(|c| c.name != cred);
    st.creds.push(CredEntry { name: cred.to_string(), owner: me, sha256: sha });
    if let Err(f2) = save_state(ctx, &st) {
        if on_disk.is_none() {
            let _ = dir.unlink(OsStr::new(&f));
        }
        return Err(f2);
    }
    let tpm2 = has_tpm2(ctx);
    Ok(serde_json::json!({
        "cred": cred,
        "path": cred_path(cred),
        "written": true,
        "bytes": bytes,
        "withKey": "auto",
        "tpm2": tpm2,
    }))
}

pub fn bind(ctx: &Ctx, who: &Caller, unit: &str, cred: &str) -> HResult<serde_json::Value> {
    let me = caller::require_daemon(who)?;
    resolve_unit(ctx, unit)?;
    need_systemd(ctx)?;
    let mut st = load_state(ctx)?;
    let Some(entry) = st.cred(cred).cloned() else {
        return Err(Fail::state("refused: K2 wrote no credential by that name (creds put first)"));
    };
    if entry.owner != me {
        return Err(Fail::state(NOT_YOURS));
    }
    let store = credstore(ctx, false)?.ok_or_else(|| Fail::state("refused: K2 wrote no credential by that name"))?;
    if disk_sha(&store, cred)?.as_deref() != Some(entry.sha256.as_str()) {
        return Err(Fail::state("refused: that .cred is missing or was changed by hand"));
    }
    let file = dropin_file(cred);
    let want = cred_dropin(cred).ok_or_else(|| Fail::argv("refused arguments"))?;
    let ours = cred_path(cred);
    let recorded = st.dropin(unit, cred).cloned();
    match &recorded {
        Some(d) if d.owner != me => return Err(Fail::state(NOT_YOURS)),
        Some(_) => {}
        None => {
            if crate::daemon_unit::dropin_matches(ctx, unit, &file, want.as_bytes()).is_some() {
                return Err(Fail::state("refused: a drop-in K2 didn't write is in the way"));
            }
        }
    }
    // The unit must not already get this credential ID from elsewhere (a
    // later LoadCredential*= with the same ID replaces an earlier one).
    if credential_conflict(&unit_creds(ctx, unit)?, cred, &ours).is_some() {
        return Err(Fail::state("refused: the unit already gets a credential with that ID from elsewhere"));
    }
    let changed = set_dropin(ctx, unit, &file, Some(want.as_bytes()), "creds bind")?;
    let undo = |ctx: &Ctx| {
        if recorded.is_none() {
            let _ = set_dropin(ctx, unit, &file, None, "creds bind");
            let _ = reload(ctx);
        }
    };
    if changed {
        if let Err(f) = reload(ctx) {
            undo(ctx);
            return Err(f);
        }
    }
    // systemd must now load K2's path for that ID (an admin drop-in that
    // sorts after K2's would win).
    match unit_creds(ctx, unit) {
        Ok(now) if credential_is_ours(&now, cred, &ours) => {}
        Ok(_) => {
            undo(ctx);
            return Err(Fail::state("refused: after the drop-in, systemd still takes that credential ID from elsewhere"));
        }
        Err(f) => {
            undo(ctx);
            return Err(f);
        }
    }
    if recorded.is_none() {
        st.dropins.push(DropinEntry { unit: unit.to_string(), cred: cred.to_string(), owner: me });
        if let Err(f) = save_state(ctx, &st) {
            undo(ctx);
            return Err(f);
        }
    }
    Ok(serde_json::json!({
        "unit": unit,
        "cred": cred,
        "dropin": format!("{}/{unit}.d/{file}", k2_root_proto::paths::SYSTEMD_SYSTEM_DIR),
        "changed": changed,
    }))
}

pub fn unbind(ctx: &Ctx, who: &Caller, unit: &str, cred: &str) -> HResult<serde_json::Value> {
    let me = caller::require_daemon(who)?;
    let mut st = load_state(ctx)?;
    let file = dropin_file(cred);
    match st.dropin(unit, cred) {
        Some(d) if d.owner != me => return Err(Fail::state(NOT_YOURS)),
        Some(_) => {}
        None => {
            if crate::daemon_unit::dropin_matches(ctx, unit, &file, b"").is_some() {
                return Err(Fail::state("refused: K2 didn't write that drop-in"));
            }
            return Ok(serde_json::json!({"unit": unit, "cred": cred, "changed": false}));
        }
    }
    let changed = set_dropin(ctx, unit, &file, None, "creds unbind")?;
    if changed {
        reload(ctx)?;
    }
    st.dropins.retain(|d| !(d.unit == unit && d.cred == cred));
    save_state(ctx, &st)?;
    Ok(serde_json::json!({"unit": unit, "cred": cred, "changed": changed}))
}

pub fn restart(ctx: &Ctx, who: &Caller, unit: &str) -> HResult<serde_json::Value> {
    let me = caller::require_daemon(who)?;
    resolve_unit(ctx, unit)?;
    let mut st = load_state(ctx)?;
    if !st.dropins.iter().any(|d| d.unit == unit && d.owner == me) {
        return Err(Fail::state("refused: this daemon bound no credential to that unit"));
    }
    let now = now_secs();
    if let Some(last) = st.restarts.get(unit) {
        if now < last.saturating_add(RESTART_MIN_INTERVAL_SECS) {
            return Err(Fail::state("refused: that unit was restarted by K2 less than 30 s ago"));
        }
    }
    st.restarts.insert(unit.to_string(), now);
    save_state(ctx, &st)?;
    // --no-block: queue the job and answer; a slow stop never holds the
    // creds lock or the daemon's request. try-restart: a stopped unit
    // stays stopped.
    ctx.run_ok(SYSTEMCTL, &["--no-block", "try-restart", unit], None)?;
    Ok(serde_json::json!({"unit": unit, "restart": "try-restart", "queued": true}))
}

pub fn remove(ctx: &Ctx, who: &Caller, cred: &str) -> HResult<serde_json::Value> {
    let me = caller::require_daemon(who)?;
    let mut st = load_state(ctx)?;
    let dir = credstore(ctx, false)?;
    let on_disk = match &dir {
        Some(d) => disk_sha(d, cred)?,
        None => None,
    };
    let Some(entry) = st.cred(cred).cloned() else {
        if on_disk.is_some() {
            return Err(Fail::state("refused: K2 didn't write that credential"));
        }
        return Ok(serde_json::json!({"cred": cred, "removed": false}));
    };
    if entry.owner != me {
        return Err(Fail::state(NOT_YOURS));
    }
    if st.dropins.iter().any(|d| d.cred == cred) {
        return Err(Fail::state("refused: a unit still loads that credential (creds unbind first)"));
    }
    let removed = match on_disk {
        None => false,
        Some(sha) if sha != entry.sha256 => {
            return Err(Fail::state("refused: that .cred was changed by hand since K2 wrote it"))
        }
        Some(_) => {
            let d = dir.as_ref().ok_or_else(|| Fail::io("credstore"))?;
            d.unlink(OsStr::new(&cred_file(cred))).map_err(|e| Fail::io_err("remove .cred", e))?;
            d.fsync().map_err(|e| Fail::io_err("fsync credstore", e))?;
            true
        }
    };
    st.creds.retain(|c| c.name != cred);
    save_state(ctx, &st)?;
    Ok(serde_json::json!({"cred": cred, "removed": removed}))
}

/// Daemon-class: the caller's own entries only (never another daemon's
/// targets), plus whether each still matches what K2 wrote.
pub fn status(ctx: &Ctx, who: &Caller) -> HResult<serde_json::Value> {
    let me = caller::require_daemon(who)?;
    let version = systemd_version(ctx);
    let (state, state_error) = match load_state(ctx) {
        Ok(s) => (s, None),
        Err(f) => (CredsState::default(), Some(f.msg)),
    };
    let store = credstore(ctx, false).ok().flatten();
    let creds: Vec<serde_json::Value> = state
        .creds
        .iter()
        .filter(|c| c.owner == me)
        .map(|c| {
            let disk = store.as_ref().and_then(|d| disk_sha(d, &c.name).ok()).flatten();
            serde_json::json!({
                "cred": c.name,
                "path": cred_path(&c.name),
                "present": disk.is_some(),
                "matches": disk.as_deref() == Some(c.sha256.as_str()),
            })
        })
        .collect();
    let dropins: Vec<serde_json::Value> = state
        .dropins
        .iter()
        .filter(|d| d.owner == me)
        .map(|d| {
            let want = cred_dropin(&d.cred).unwrap_or_default();
            let m = crate::daemon_unit::dropin_matches(ctx, &d.unit, &dropin_file(&d.cred), want.as_bytes());
            serde_json::json!({
                "unit": d.unit,
                "cred": d.cred,
                "dropin": match m { None => "missing", Some(true) => "canned", Some(false) => "other" },
            })
        })
        .collect();
    let creds_tool = ctx
        .dir("/usr/bin")
        .ok()
        .and_then(|d| d.kind(OsStr::new("systemd-creds")).ok())
        .is_some_and(|k| matches!(k, Kind::File | Kind::Symlink));
    let allow = allow_list(ctx);
    let orphans = orphans(ctx, &state);
    Ok(serde_json::json!({
        "systemd": version,
        "minSystemd": MIN_SYSTEMD,
        "supported": version.is_some_and(|v| v >= MIN_SYSTEMD) && creds_tool,
        "systemdCreds": creds_tool,
        "tpm2": creds_tool && has_tpm2(ctx),
        "credstore": CREDSTORE_DIR,
        "allowList": k2_root_proto::creds::ALLOW_LIST,
        // The caller is the daemon (daemon-class): it uses the list to
        // refuse early with the admin's step. Agents never reach this verb.
        "allowedUnits": allow.as_ref().cloned().unwrap_or_default(),
        "allowListError": allow.err().map(|f| f.msg),
        "creds": creds,
        "dropins": dropins,
        "stateError": state_error,
        // Files in K2's own places that no record names (a crash between a
        // write and its record, review B2-N3): K2 refuses to touch them, so
        // a person removes them, as root.
        "orphans": orphans,
    }))
}

/// A root command to paste, only for a name K2 could have written (a
/// valid cred name; the unit was checked by the caller), single-quoted
/// anyway (review A3-P4). Anything else gets words, never a command.
fn cleanup_cmd(path: &str, cred: &str, reload: bool) -> String {
    if !k2_root_proto::creds::cred_name_ok(cred) || path.contains('\'') {
        return "not a name K2 writes: look at it and remove it by hand".into();
    }
    format!("rm -- '{path}'{}", if reload { " && systemctl daemon-reload" } else { "" })
}

/// `.cred` files in K2's store and `k2-cred-*.conf` drop-ins that no
/// record (of any daemon) names, each with its root cleanup step.
fn orphans(ctx: &Ctx, st: &CredsState) -> Vec<serde_json::Value> {
    let mut out = vec![];
    if let Ok(Some(store)) = credstore(ctx, false) {
        for e in store.entries().unwrap_or_default() {
            let n = e.to_string_lossy().to_string();
            let Some(cred) = n.strip_suffix(CRED_SUFFIX) else { continue };
            if n.starts_with('.') || st.cred(cred).is_some() {
                continue;
            }
            let path = format!("{CREDSTORE_DIR}/{n}");
            out.push(serde_json::json!({ "path": path, "kind": "cred", "cleanup": cleanup_cmd(&path, cred, false) }));
        }
    }
    let sys = k2_root_proto::paths::SYSTEMD_SYSTEM_DIR;
    if let Ok(base) = ctx.dir(sys) {
        for e in base.entries().unwrap_or_default() {
            let d = e.to_string_lossy().to_string();
            let Some(unit) = d.strip_suffix(".d") else { continue };
            if !k2_root_proto::creds::unit_name_ok(unit) {
                continue;
            }
            let Ok(sub) = base.sub(&e) else { continue };
            for f in sub.entries().unwrap_or_default() {
                let f = f.to_string_lossy().to_string();
                let Some(cred) = f.strip_prefix(DROPIN_PREFIX).and_then(|x| x.strip_suffix(DROPIN_SUFFIX)) else { continue };
                if st.dropin(unit, cred).is_some() {
                    continue;
                }
                let path = format!("{sys}/{d}/{f}");
                out.push(serde_json::json!({
                    "path": path, "kind": "dropin", "unit": unit,
                    "cleanup": cleanup_cmd(&path, cred, true),
                }));
            }
        }
    }
    out
}
