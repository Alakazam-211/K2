//! `helper` namespace (§7.1, §9): `version`, `self-update`,
//! `self-rollback`, `legacy-retire`, `audit-tail`.

use std::ffi::OsStr;

use serde::{Deserialize, Serialize};

use k2_root_proto::keys::trusted_keys;
use k2_root_proto::paths::*;
use k2_root_proto::sig::{decode_payload, verify_signed_build};
use k2_root_proto::stalwart::{
    artifact_for_arch, previous_artifact_for_arch, upgrade_from_artifact, STALWART_PINNED_VERSION,
    STALWART_PREVIOUS_VERSION, STALWART_UPGRADE_FROM,
};
use k2_root_proto::verbs::Legacy;
use k2_root_proto::version::{Semver, HELPER_NAME, PROTOCOL};

use crate::ctx::{Ctx, Fail, HResult};
use crate::fsx::{Dir, Kind};
use crate::mail::{parent_dir, split};

/// This build's product version (the daemon's, via build.rs).
pub const VERSION: &str = env!("K2_PRODUCT_VERSION");

pub fn running_version() -> Semver {
    Semver::parse(VERSION).expect("build.rs checked the version")
}

/// `/var/lib/k2-root-helper/state.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Versions rolled back from; self-update never installs one again.
    #[serde(default)]
    pub refused: Vec<String>,
}

pub fn load_state(ctx: &Ctx) -> HResult<State> {
    let (parent, name) = split(STATE_JSON);
    let (gp, dname) = split(parent);
    let base = ctx.dir(gp).map_err(|e| Fail::io_err("open /var/lib", e))?;
    match base
        .kind(OsStr::new(dname))
        .map_err(|e| Fail::io_err("stat", e))?
    {
        Kind::Missing => return Ok(State::default()),
        Kind::Dir => {}
        _ => return Err(Fail::state("state dir is not a directory")),
    }
    let d = base
        .sub(OsStr::new(dname))
        .map_err(|e| Fail::io_err("open state dir", e))?;
    match d
        .read_file(OsStr::new(name), 64 * 1024)
        .map_err(|e| Fail::io_err("read state", e))?
    {
        None => Ok(State::default()),
        Some(b) => serde_json::from_slice(&b).map_err(|_| Fail::state("state.json unreadable")),
    }
}

fn save_state(ctx: &Ctx, s: &State) -> HResult<()> {
    let (parent, name) = split(STATE_JSON);
    let (gp, dname) = split(parent);
    let base = ctx.dir(gp).map_err(|e| Fail::io_err("open /var/lib", e))?;
    match base
        .kind(OsStr::new(dname))
        .map_err(|e| Fail::io_err("stat", e))?
    {
        Kind::Missing => base
            .mkdir(OsStr::new(dname), 0o755)
            .map_err(|e| Fail::io_err("mkdir state", e))?,
        Kind::Dir => {}
        _ => return Err(Fail::state("state dir is not a directory")),
    }
    let d = base
        .sub(OsStr::new(dname))
        .map_err(|e| Fail::io_err("open state dir", e))?;
    if let Some((u, g)) = ctx.root_owner() {
        d.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
    }
    let bytes = serde_json::to_vec_pretty(s).map_err(|_| Fail::io("state encode"))?;
    d.write_atomic(OsStr::new(name), &bytes, 0o644, ctx.root_owner())
        .map_err(|e| Fail::io_err("write state", e))
}

/// `helper version` JSON (§7.1). Also carries the legacy mail helper's
/// field names (`protocol`, `stalwartPin*`, `stalwartUpgradeFrom`) so
/// `k2 hostmail upgrade`'s check reads it unchanged.
pub fn version_json(ctx: &Ctx) -> serde_json::Value {
    let keys: Vec<String> = trusted_keys(ctx.test_keys)
        .iter()
        .map(|k| k.key_id.to_string())
        .collect();
    let daemons = crate::caller::load_daemons(ctx)
        .map(|v| {
            v.iter()
                .map(|d| serde_json::json!({"user": d.user, "unit": d.unit, "scope": d.scope}))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let refused = load_state(ctx).map(|s| s.refused).unwrap_or_default();
    serde_json::json!({
        "name": HELPER_NAME,
        "helper": HELPER_NAME,
        "version": VERSION,
        "k2Version": VERSION,
        "protocol": PROTOCOL,
        "arch": ctx.arch,
        "path": ctx.self_exe.display().to_string(),
        "packageOwned": !ctx.is_canonical_path(),
        "keyIds": keys,
        "testKeys": ctx.test_keys,
        "namespaces": {"helper": true, "mail": true, "pg": true, "backup": false, "keep": true, "daemon": true, "apps": true, "sew": true, "front": true, "cli": true, "pgmajor": true, "creds": true},
        // `helper cli-refresh` (protocol 9): the K2 CLI this helper carries.
        "cliVersion": cli_version_of(CLI_CONTENT.as_bytes()),
        // ...and its sha256: the daemon refreshes only when this equals
        // the hash of the CLI the daemon itself embeds.
        "cliSha256": CLI_SHA256,
        "stalwartPin": STALWART_PINNED_VERSION,
        "stalwartPinSha256": artifact_for_arch(ctx.arch).map(|a| a.sha256).ok(),
        "stalwartPrevious": STALWART_PREVIOUS_VERSION,
        "stalwartPreviousSha256": previous_artifact_for_arch(ctx.arch).map(|a| a.sha256).ok(),
        // Every version `install-bin` accepts besides the pin (the
        // upgrade sources; a rollback reinstalls whichever the box had).
        // Same shape as the legacy mail helper's protocol-3 field.
        "stalwartUpgradeFrom": STALWART_UPGRADE_FROM
            .iter()
            .map(|rel| serde_json::json!({
                "version": rel.version,
                "sha256": upgrade_from_artifact(rel.version, ctx.arch).map(|a| a.sha256).ok(),
            }))
            .collect::<Vec<_>>(),
        // Sew (protocol 7): this build's pin, so the daemon can tell an
        // old helper from a bad download (S29 G8, `k2_helper_pin_mismatch`).
        "sewPin": k2_root_proto::sew::pinned().version,
        "sewPinSha256": k2_root_proto::sew::pinned_artifact(ctx.arch).map(|a| a.tarball_sha256).ok(),
        // Sew's TEST builds pinned (never in a release; signing refuses it).
        "sewTestPins": k2_root_proto::sew::test_pins_flag(),
        "refused": refused,
        "daemons": daemons,
        "build": crate::BUILD_MARKER,
        "testKeysFlag": crate::test_keys_flag(),
    })
}

fn helper_dir(ctx: &Ctx) -> HResult<Dir> {
    let d = ctx
        .dir(HELPER_DIR)
        .map_err(|e| Fail::io_err("open helper dir", e))?;
    let st = d
        .stat_self()
        .map_err(|e| Fail::io_err("stat helper dir", e))?;
    if (st.st_mode as u32) & 0o022 != 0 {
        return Err(Fail::sig(
            "refused: helper directory is group/other-writable",
        ));
    }
    if ctx.real_owners && st.st_uid != 0 {
        return Err(Fail::sig("refused: helper directory is not root-owned"));
    }
    Ok(d)
}

/// §9.2. Every check runs before any write; the candidate is never
/// executed (this function has no `Exec` in reach).
pub fn self_update(ctx: &Ctx, payload: &[u8]) -> HResult<serde_json::Value> {
    let (sig, elf) = decode_payload(payload).map_err(Fail::sig)?;
    let keys = trusted_keys(ctx.test_keys);
    let built = verify_signed_build(elf, sig, &keys, ctx.arch).map_err(Fail::sig)?;
    let running = running_version();
    if built.version == running {
        return Ok(serde_json::json!({"result": "same", "version": VERSION}));
    }
    if built.version < running {
        return Err(Fail::sig("refused: older than the running helper"));
    }
    if built.protocol < PROTOCOL {
        return Err(Fail::sig("refused: lower protocol than the running helper"));
    }
    let state = load_state(ctx)?;
    if state
        .refused
        .iter()
        .any(|v| v == &built.version.to_string())
    {
        return Err(Fail::sig("refused: that version was rolled back"));
    }
    if !ctx.is_canonical_path() {
        return Err(Fail::sig("package_owned"));
    }
    let d = helper_dir(ctx)?;
    let (cur, stage, prev) = (
        OsStr::new(HELPER_FILE),
        OsStr::new(HELPER_STAGE_FILE),
        OsStr::new(HELPER_PREV_FILE),
    );
    if d.kind(cur).map_err(|e| Fail::io_err("stat", e))? != Kind::File {
        return Err(Fail::state(
            "refused: installed helper is not a regular file",
        ));
    }
    match d.kind(stage).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => {}
        Kind::File => d
            .unlink(stage)
            .map_err(|e| Fail::io_err("remove stale stage", e))?,
        _ => return Err(Fail::sig("refused: stage path is not a regular file")),
    }
    match d.kind(prev).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing | Kind::File => {}
        _ => return Err(Fail::sig("refused: .prev is not a regular file")),
    }
    // Stage: O_EXCL|O_NOFOLLOW, root 0755, write, fsync.
    let res = (|| -> std::io::Result<()> {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let mut f = d.create_excl(stage)?;
        f.write_all(elf)?;
        if let Some((u, g)) = ctx.root_owner() {
            // SAFETY: fd owned by f.
            if unsafe { libc::fchown(f.as_raw_fd(), u, g) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        // SAFETY: fd owned by f.
        if unsafe { libc::fchmod(f.as_raw_fd(), 0o755) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        f.sync_all()
    })();
    if let Err(e) = res {
        let _ = d.unlink(stage);
        return Err(Fail::io_err("stage", e));
    }
    // .prev = the current file (hard link keeps it byte for byte).
    let swap = (|| -> std::io::Result<()> {
        if d.kind(prev)? == Kind::File {
            d.unlink(prev)?;
        }
        d.link(cur, prev)?;
        d.rename(stage, cur)?;
        d.fsync()
    })();
    if let Err(e) = swap {
        let _ = d.unlink(stage);
        return Err(Fail::io_err("install", e));
    }
    use sha2::Digest;
    let sha: String = sha2::Sha256::digest(elf)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(serde_json::json!({
        "result": "updated",
        "from": VERSION,
        "to": built.version.to_string(),
        "protocol": built.protocol,
        "sha256": sha,
        "keyId": built.key_id,
    }))
}

/// §9.3: current → `.bad`, `.prev` → current; this (running) version goes
/// on the refuse list. One step back only.
pub fn self_rollback(ctx: &Ctx) -> HResult<serde_json::Value> {
    if !ctx.is_canonical_path() {
        return Err(Fail::sig("package_owned"));
    }
    let d = helper_dir(ctx)?;
    let (cur, prev, bad) = (
        OsStr::new(HELPER_FILE),
        OsStr::new(HELPER_PREV_FILE),
        OsStr::new(HELPER_BAD_FILE),
    );
    if d.kind(prev).map_err(|e| Fail::io_err("stat", e))? != Kind::File {
        return Err(Fail::state("refused: no previous helper to roll back to"));
    }
    if d.kind(cur).map_err(|e| Fail::io_err("stat", e))? != Kind::File {
        return Err(Fail::state(
            "refused: installed helper is not a regular file",
        ));
    }
    // Record first: a crash after the swap must not let the bad build back.
    let mut state = load_state(ctx)?;
    if !state.refused.iter().any(|v| v == VERSION) {
        state.refused.push(VERSION.to_string());
    }
    save_state(ctx, &state)?;
    let swap = (|| -> std::io::Result<()> {
        match d.kind(bad)? {
            Kind::Missing => {}
            Kind::File => d.unlink(bad)?,
            _ => return Err(std::io::Error::other("bad path is not a regular file")),
        }
        d.rename(cur, bad)?;
        d.rename(prev, cur)?;
        d.fsync()
    })();
    swap.map_err(|e| Fail::io_err("rollback", e))?;
    Ok(serde_json::json!({"result": "rolled_back", "from": VERSION, "refused": state.refused}))
}

/// Remove the fixed legacy files, then `visudo -c`.
pub fn legacy_retire(ctx: &Ctx, which: Legacy) -> HResult<serde_json::Value> {
    let (bin, sudoers) = match which {
        Legacy::Mail => (LEGACY_MAIL_HELPER, LEGACY_MAIL_SUDOERS),
        Legacy::Pg => (LEGACY_PG_HELPER, LEGACY_PG_SUDOERS),
    };
    let mut removed = Vec::new();
    for abs in [sudoers, bin] {
        let (d, name) = parent_dir(ctx, abs)?;
        match d.kind(name).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Missing => {}
            Kind::File => {
                d.unlink(name).map_err(|e| Fail::io_err("remove", e))?;
                removed.push(abs);
            }
            _ => return Err(Fail::state("refused: legacy path is not a regular file")),
        }
    }
    let visudo = ctx
        .exec
        .run(VISUDO, &["-c".to_string(), "-q".to_string()], None)
        .map(|o| if o.status == 0 { "ok" } else { "failed" })
        .unwrap_or("not run");
    Ok(serde_json::json!({"removed": removed, "visudo": visudo}))
}

// ── helper cli-refresh (protocol 9) ────────────────────────────────────

/// The K2 CLI (`cli/k2`), compiled into the signed helper: `cli-refresh`
/// writes only these bytes, never anything from the caller. build.rs
/// refuses a CLI whose `K2_CLI_VERSION` is not this build's version.
pub const CLI_CONTENT: &str = include_str!("../../../cli/k2");
/// sha256 of [`CLI_CONTENT`] (build.rs): `helper version` → `cliSha256`.
pub const CLI_SHA256: &str = env!("K2_CLI_SHA256");

pub use k2_root_proto::cli::cli_version_of;
use k2_root_proto::cli::{CLI_HEAD_BYTES, CLI_MAX_BYTES};

fn cli_skipped(reason: &str, to: &Semver) -> serde_json::Value {
    serde_json::json!({"result": "skipped", "reason": reason, "to": to.to_string()})
}

/// Who may refresh the one `/usr/local/bin/k2`: the daemon whose
/// `daemon.toml` entry is the FIRST system-scope entry (the installer
/// writes the box's daemon first). A user-scope daemon, or a second
/// system daemon on a multi-daemon box, is answered `skipped`: daemons at
/// different versions would otherwise each ask root (newest wins, and
/// every older one asks again every pass).
fn cli_owner_gate(ctx: &Ctx, who: &crate::caller::Caller) -> HResult<Option<&'static str>> {
    let me = crate::daemon_unit::caller_entry(ctx, who)?;
    if me.scope != "system" {
        return Ok(Some("a user-scope daemon does not own /usr/local/bin/k2"));
    }
    let all = crate::caller::load_daemons(ctx).map_err(Fail::state)?;
    let owner = all.iter().find(|d| d.scope == "system");
    if owner.map(|d| (&d.user, &d.unit)) != Some((&me.user, &me.unit)) {
        return Ok(Some("another K2 daemon owns /usr/local/bin/k2 (the first system entry in daemon.toml)"));
    }
    Ok(None)
}

/// `helper cli-refresh`: a root-owned K2 CLI at `/usr/local/bin/k2` older
/// than this helper's copy is replaced (stage + fsync + rename, root:root
/// 0755). Idempotent: the same or a newer version is left alone (never a
/// downgrade). Everything that is not plainly ours to replace is `skipped`
/// with the reason and nothing written (the daemon reads `skipped` as
/// "not managed", never as a failure to retry and audit):
/// - `/usr/local/bin` (or `/usr/local`) a link (ostree-style) or missing;
/// - `/usr/local/bin` not root-owned, or group/other-writable;
/// - the caller not the box's system daemon ([`cli_owner_gate`]);
/// - the file missing, a symlink, not a regular file (a FIFO or device is
///   never opened blocking), not root-owned, group/other-writable, over
///   16 MB, or not the K2 CLI.
pub fn cli_refresh(ctx: &Ctx, who: &crate::caller::Caller) -> HResult<serde_json::Value> {
    let to = cli_version_of(CLI_CONTENT.as_bytes())
        .and_then(|v| Semver::parse(&v))
        .ok_or_else(|| Fail::state("this helper carries no versioned K2 CLI"))?;
    // The directory first: a walk that never follows a link, then its own
    // owner and mode (the helper_dir / sew pattern).
    let base = match ctx.dir(CLI_DIR) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cli_skipped("absent", &to)),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR)) => {
            return Ok(cli_skipped("directory is a link", &to))
        }
        Err(e) => return Err(Fail::io_err("open /usr/local/bin", e)),
    };
    let dst = base.stat_self().map_err(|e| Fail::io_err("stat /usr/local/bin", e))?;
    if (ctx.real_owners && dst.st_uid != 0) || (dst.st_mode as u32) & 0o022 != 0 {
        return Ok(cli_skipped("directory not root-owned", &to));
    }
    if let Some(why) = cli_owner_gate(ctx, who)? {
        return Ok(cli_skipped(why, &to));
    }
    let name = OsStr::new(CLI_FILE);
    let Some(st) = base.lstat(name).map_err(|e| Fail::io_err("stat", e))? else {
        return Ok(cli_skipped("absent", &to));
    };
    match crate::fsx::kind_of(st.st_mode as u32) {
        Kind::File => {}
        Kind::Symlink => return Ok(cli_skipped("symlink", &to)),
        _ => return Ok(cli_skipped("not a regular file", &to)),
    }
    // Opened non-blocking, never through a link; every check below is on
    // the opened fd's stat, not the lstat above (a swap in between is
    // caught here, and a FIFO swapped in never hangs root).
    let (file, st) = match base.open_regular(name) {
        Ok(Some(x)) => x,
        Ok(None) => return Ok(cli_skipped("absent", &to)),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Ok(cli_skipped("not a regular file", &to))
        }
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(cli_skipped("symlink", &to)),
        Err(e) => return Err(Fail::io_err("open /usr/local/bin/k2", e)),
    };
    if ctx.real_owners && st.st_uid != 0 {
        return Ok(cli_skipped("not root-owned", &to));
    }
    if (st.st_mode as u32) & 0o022 != 0 {
        return Ok(cli_skipped("file is group/other-writable", &to));
    }
    if st.st_size < 0 || st.st_size as u64 > CLI_MAX_BYTES {
        return Ok(cli_skipped("not the K2 CLI (over 16 MB)", &to));
    }
    let mut head = Vec::with_capacity(CLI_HEAD_BYTES);
    {
        use std::io::Read;
        file.take(CLI_HEAD_BYTES as u64)
            .read_to_end(&mut head)
            .map_err(|e| Fail::io_err("read /usr/local/bin/k2", e))?;
    }
    let Some(from) = cli_version_of(&head).and_then(|v| Semver::parse(&v)) else {
        return Ok(cli_skipped("not the K2 CLI", &to));
    };
    if from >= to {
        return Ok(serde_json::json!({"result": "unchanged", "version": from.to_string(), "to": to.to_string()}));
    }
    // A new inode renamed over the name: a hard link elsewhere to the old
    // file keeps the old bytes (never written through).
    base.write_atomic(name, CLI_CONTENT.as_bytes(), 0o755, ctx.root_owner())
        .map_err(|e| Fail::io_err("write /usr/local/bin/k2", e))?;
    Ok(serde_json::json!({"result": "updated", "from": from.to_string(), "to": to.to_string()}))
}

/// Last 100 of our own journal lines, as JSON (the daemon user may not be
/// in `systemd-journal`).
pub fn audit_tail(ctx: &Ctx) -> HResult<serde_json::Value> {
    let out = ctx.run_ok(
        JOURNALCTL,
        // `_UID=0`: only lines the root helper itself logged — any user
        // can `logger -t k2-root-helper` (security review #7).
        &[
            "-t",
            "k2-root-helper",
            "_UID=0",
            "-n",
            "100",
            "-o",
            "json",
            "--no-pager",
            "-q",
        ],
        None,
    )?;
    let mut lines = Vec::new();
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else {
            continue;
        };
        let at = v["__REALTIME_TIMESTAMP"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|us| us / 1_000_000);
        let msg = v["MESSAGE"].as_str().unwrap_or_default();
        let parsed =
            serde_json::from_str::<serde_json::Value>(msg).unwrap_or(serde_json::Value::Null);
        if parsed.is_null() {
            continue;
        }
        lines.push(serde_json::json!({"at": at, "event": parsed}));
    }
    Ok(serde_json::json!({"events": lines}))
}
