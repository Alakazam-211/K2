//! K2's own teardown of a Sew it does not recognise (0.46.3): `engine sew
//! purge` when `/usr/local/bin/sew` is missing or is not a release this
//! build pins.
//!
//! **Never executes the binary.** The known-binary rule (S29 §7.3) stands:
//! an unrecognised `sew` is never run as root. So this does, with the
//! helper's own no-follow file primitives and `systemctl`, what `sew
//! uninstall --purge` does (sew-ops `install.rs` `uninstall`, media
//! `uninstall`, `sysctl::remove`), on Sew's fixed layout, and then removes
//! the binaries so a fresh `enable` starts clean:
//!
//! 1. Every unit FILE in `/etc/systemd/system` named in Sew's namespace
//!    ([`sew_unit_name_ok`]): `disable --now` each (sidecars first, the
//!    database last), `stop` the template instances, then remove the unit
//!    files, their `.d` drop-in dirs and any enablement link left in a
//!    `*.wants` / `*.requires` dir; `daemon-reload`; `reset-failed`.
//! 2. sysctls (Sew `sysctl::remove`): the originals in
//!    `/etc/sew/media-sysctl.baseline` (read strictly, BEFORE `/etc/sew`
//!    goes) are written back to `/proc/sys/net/core/{r,w}mem_max`, the
//!    drop-in `/etc/sysctl.d/60-sew-media.conf` is deleted, the live
//!    values re-read: equal = restored, else `sysctl_not_restored`. A
//!    drop-in without a readable baseline is deleted and the live values
//!    are left (`sysctl_baseline_missing`).
//! 3. `--keep-backups`: `/var/lib/sew/backups` → `/var/lib/sew-backups-<UTC>`.
//! 4. `/etc/sew`, `/var/lib/sew`, `/usr/local/lib/sew`, `/usr/local/share/sew`,
//!    Sew's credentials by name ([`SEW_CRED_NAMES`], [`SEW_CRED_PREFIXES`])
//!    in both credstores, `/usr/local/bin/sew` and both staging names.
//! 5. The helper's own Sew state: notices, kept engine tarballs, staging.
//!
//! Kept and reported: Sew's system accounts (Sew's uninstall keeps them;
//! one that looks like a login account is flagged, because Sew 0.2.0+
//! `install` refuses it, `service_account_unsafe`), and
//! `/root/sew-master-key.recovery` (a key the operator may still want).
//!
//! Anything that could not be removed fails the verb (exit 5, io) AFTER every
//! step was tried, so the daemon keeps the vault key and the row and the
//! purge can simply run again.

use std::ffi::OsStr;

use k2_root_proto::paths::{SYSTEMCTL, SYSTEMD_SYSTEM_DIR};
use k2_root_proto::sew::*;
use serde_json::json;

use crate::ctx::{Ctx, Fail, HResult};
use crate::fsx::Kind;
use crate::sew::{notices_or_error, open_parent, remove_all_notices, remove_kept, split, unstage};

type J = serde_json::Value;

/// The teardown result; `failed` non-empty becomes a refusal.
#[derive(Default)]
struct Report {
    removed: Vec<String>,
    failed: Vec<String>,
}

impl Report {
    fn fail(&mut self, what: &str, e: impl std::fmt::Display) {
        self.failed.push(format!("{what}: {e}"));
    }
}

/// `binary`: the sha256 of the unrecognised file at `/usr/local/bin/sew`,
/// `None` when there is none.
pub fn purge(ctx: &Ctx, binary: Option<&str>, install_id: &str, keep_backups: bool) -> HResult<J> {
    let mut r = Report::default();

    // Read everything the removals destroy first.
    let baseline = read_baseline(ctx);
    let recovery_present = exists(ctx, SEW_RECOVERY_FILE);

    let units = units(ctx, &mut r);
    let sysctl = sysctl_restore(ctx, baseline, &mut r);
    let kept_backups = if keep_backups { move_backups(ctx, &mut r) } else { None };

    for p in [SEW_CONFIG_DIR, SEW_DATA_DIR, SEW_LIB_DIR, SEW_SHARE_DIR, SEW_BIN, SEW_BIN_SEW_STAGING, SEW_BIN_STAGING] {
        remove_path(ctx, p, &mut r);
    }
    credentials(ctx, &mut r);
    let notices = notices_or_error(remove_all_notices(ctx));
    let kept_engines = notices_or_error(remove_kept(ctx));
    let staging = notices_or_error(unstage(ctx));
    let accounts = accounts(ctx);

    if !r.failed.is_empty() {
        return Err(Fail::io(format!(
            "K2's Sew teardown could not remove everything (run it again once fixed; nothing in the K2 vault was deleted): {}",
            r.failed.join("; ")
        )));
    }
    let mut left: Vec<String> = Vec::new();
    if recovery_present {
        left.push(format!("{SEW_RECOVERY_FILE} (Sew's recovery key text; kept: delete it by hand if you do not need it)"));
    }
    if let Some(w) = sysctl.get("warning").and_then(J::as_str) {
        left.push(w.to_string());
    }
    for a in accounts.as_array().into_iter().flatten() {
        if a["loginLike"] == true {
            left.push(format!(
                "account {} looks like a login account ({}); Sew 0.2.0+ install refuses it (service_account_unsafe): \
                 remove it (userdel -r {}) or fix it before the next enable",
                a["name"].as_str().unwrap_or("?"),
                a["reasons"].as_array().map(|v| v.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                a["name"].as_str().unwrap_or("?"),
            ));
        }
    }
    Ok(json!({
        "teardown": "k2",
        "binary": match binary {
            Some(sha) => json!({"present": true, "known": false, "sha256": sha, "executed": false}),
            None => json!({"present": false}),
        },
        // Sew checks the typed-back install id against its database; K2's
        // teardown cannot (it runs nothing of Sew's). The daemon's person
        // proof + typed hostname are the confirmation.
        "installId": install_id,
        "installIdChecked": false,
        "removed": r.removed,
        "units": units,
        "sysctl": sysctl,
        "keptBackups": kept_backups,
        "noticesRemoved": notices,
        "keptRemoved": kept_engines,
        "staging": staging,
        "accounts": accounts,
        "kept": ["the system accounts sew, sew-db and the sidecars' (Sew's own uninstall keeps them too)", SEW_RECOVERY_FILE],
        "left": left,
    }))
}

// ── small fs helpers (never follow a link) ──────────────────────────────

fn exists(ctx: &Ctx, abs: &str) -> bool {
    open_parent(ctx, abs)
        .ok()
        .flatten()
        .and_then(|(d, n)| d.kind(OsStr::new(&n)).ok())
        .is_some_and(|k| k != Kind::Missing)
}

/// Remove `abs`: a directory as a tree (links inside unlinked, never
/// followed), a file or a link itself.
fn remove_path(ctx: &Ctx, abs: &str, r: &mut Report) {
    let (d, n) = match open_parent(ctx, abs) {
        Ok(Some(x)) => x,
        Ok(None) => return,
        Err(f) => return r.fail(abs, f.msg),
    };
    let n = OsStr::new(&n);
    match d.kind(n) {
        Ok(Kind::Missing) => {}
        Ok(_) => match d.remove_tree(n) {
            Ok(()) => r.removed.push(abs.to_string()),
            Err(e) => r.fail(abs, e),
        },
        Err(e) => r.fail(abs, e),
    }
}

fn read_small(ctx: &Ctx, abs: &str, cap: usize) -> Option<Vec<u8>> {
    let (d, n) = open_parent(ctx, abs).ok().flatten()?;
    d.read_file(OsStr::new(&n), cap).ok().flatten()
}

// ── units ───────────────────────────────────────────────────────────────

/// Stop order: sidecars, the server, its timers and one-shots, the
/// database last.
fn stop_rank(u: &str) -> u8 {
    match u {
        "sew-recorder.service" | "sew-transcriber.service" | "sew-media.service" => 0,
        "sew.service" => 1,
        "sew-db.service" => 3,
        _ => 2,
    }
}

fn units(ctx: &Ctx, r: &mut Report) -> J {
    let sd = match ctx.dir(SYSTEMD_SYSTEM_DIR) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return json!({"found": []}),
        Err(e) => {
            r.fail(SYSTEMD_SYSTEM_DIR, e);
            return json!({"found": []});
        }
    };
    let entries: Vec<String> = match sd.entries() {
        Ok(v) => v.into_iter().filter_map(|n| n.into_string().ok()).collect(),
        Err(e) => {
            r.fail(SYSTEMD_SYSTEM_DIR, e);
            return json!({"found": []});
        }
    };
    let kind = |n: &str| sd.kind(OsStr::new(n)).unwrap_or(Kind::Missing);
    let mut found: Vec<String> =
        entries.iter().filter(|n| sew_unit_name_ok(n) && matches!(kind(n), Kind::File | Kind::Symlink)).cloned().collect();
    found.sort_by_key(|u| (stop_rank(u), u.clone()));

    // Stop + disable what systemd knows by these names; a unit that is not
    // loaded answers non-zero, which is fine (reported, not a failure).
    let mut not_disabled = Vec::new();
    for u in found.iter().filter(|u| !sew_unit_is_template(u)) {
        match ctx.exec.run(SYSTEMCTL, &["disable".into(), "--now".into(), u.clone()], None) {
            Ok(o) if o.status == 0 => {}
            Ok(o) => not_disabled.push(json!({"unit": u, "exit": o.status})),
            Err(e) => not_disabled.push(json!({"unit": u, "error": e.to_string()})),
        }
    }
    let mut patterns: Vec<String> = vec!["sew-backup@*.service".into(), "sew-drill@*.service".into()];
    for t in found.iter().filter(|u| sew_unit_is_template(u)) {
        let p = t.replace("@.service", "@*.service");
        if !patterns.contains(&p) {
            patterns.push(p);
        }
    }
    let mut stop = vec!["stop".to_string()];
    stop.extend(patterns.iter().cloned());
    let _ = ctx.exec.run(SYSTEMCTL, &stop, None);

    // Unit files, their drop-in dirs, and enablement links.
    for u in &found {
        match sd.unlink(OsStr::new(u)) {
            Ok(()) => r.removed.push(format!("{SYSTEMD_SYSTEM_DIR}/{u}")),
            Err(e) => r.fail(&format!("{SYSTEMD_SYSTEM_DIR}/{u}"), e),
        }
    }
    for n in &entries {
        let Some(stem) = n.strip_suffix(".d") else { continue };
        if !sew_unit_name_ok(stem) || kind(n) == Kind::Missing {
            continue;
        }
        match sd.remove_tree(OsStr::new(n)) {
            Ok(()) => r.removed.push(format!("{SYSTEMD_SYSTEM_DIR}/{n}")),
            Err(e) => r.fail(&format!("{SYSTEMD_SYSTEM_DIR}/{n}"), e),
        }
    }
    for n in &entries {
        if !(n.ends_with(".wants") || n.ends_with(".requires")) || kind(n) != Kind::Dir {
            continue;
        }
        let Ok(w) = sd.sub(OsStr::new(n)) else { continue };
        for l in w.entries().unwrap_or_default().into_iter().filter_map(|x| x.into_string().ok()) {
            if sew_unit_name_ok(&l) && w.kind(OsStr::new(&l)).ok() == Some(Kind::Symlink) {
                match w.unlink(OsStr::new(&l)) {
                    Ok(()) => r.removed.push(format!("{SYSTEMD_SYSTEM_DIR}/{n}/{l}")),
                    Err(e) => r.fail(&format!("{SYSTEMD_SYSTEM_DIR}/{n}/{l}"), e),
                }
            }
        }
    }
    let reload = ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None);
    if let Err(f) = &reload {
        r.fail("systemctl daemon-reload", &f.msg);
    }
    let _ = ctx.exec.run(
        SYSTEMCTL,
        &["reset-failed".into(), "sew.service".into(), "sew-*.service".into(), "sew-*.timer".into()],
        None,
    );
    json!({"found": found, "notDisabled": not_disabled, "stoppedInstances": patterns, "daemonReload": reload.is_ok()})
}

// ── sysctls ─────────────────────────────────────────────────────────────

enum Baseline {
    None,
    Unreadable,
    Values(u64, u64),
}

fn read_baseline(ctx: &Ctx) -> Baseline {
    if !exists(ctx, SEW_MEDIA_SYSCTL_BASELINE) {
        return Baseline::None;
    }
    match read_small(ctx, SEW_MEDIA_SYSCTL_BASELINE, MAX_SYSCTL_BASELINE).and_then(|b| parse_sysctl_baseline(&b)) {
        Some((rm, wm)) => Baseline::Values(rm, wm),
        None => Baseline::Unreadable,
    }
}

fn live(ctx: &Ctx, abs: &str) -> Option<u64> {
    let b = read_small(ctx, abs, 32)?;
    std::str::from_utf8(&b).ok()?.trim().parse().ok()
}

fn write_proc(ctx: &Ctx, abs: &str, v: u64) -> Result<(), String> {
    let (parent, name) = split(abs);
    let d = ctx.dir(parent).map_err(|e| format!("open {parent}: {e}"))?;
    d.write_existing(OsStr::new(name), format!("{v}\n").as_bytes()).map_err(|e| format!("write {abs}: {e}"))
}

/// Sew's §10.3 order: write the originals, delete the drop-in (always),
/// re-read.
fn sysctl_restore(ctx: &Ctx, baseline: Baseline, r: &mut Report) -> J {
    let dropin = exists(ctx, SEW_MEDIA_SYSCTL);
    let drop_dropin = |r: &mut Report| {
        let before = r.removed.len();
        remove_path(ctx, SEW_MEDIA_SYSCTL, r);
        r.removed.len() > before
    };
    let now = |ctx: &Ctx| json!({"rmem_max": live(ctx, PROC_RMEM_MAX), "wmem_max": live(ctx, PROC_WMEM_MAX)});
    match baseline {
        Baseline::Values(rm, wm) => {
            let mut notes = Vec::new();
            for (p, v) in [(PROC_RMEM_MAX, rm), (PROC_WMEM_MAX, wm)] {
                if let Err(e) = write_proc(ctx, p, v) {
                    notes.push(e);
                }
            }
            let dropped = drop_dropin(r);
            let (lr, lw) = (live(ctx, PROC_RMEM_MAX), live(ctx, PROC_WMEM_MAX));
            if (lr, lw) == (Some(rm), Some(wm)) {
                json!({"state": "restored", "rmem_max": rm, "wmem_max": wm, "dropinRemoved": dropped, "notes": notes})
            } else {
                json!({
                    "state": "sysctl_not_restored",
                    "want": {"rmem_max": rm, "wmem_max": wm},
                    "live": now(ctx),
                    "dropinRemoved": dropped,
                    "notes": notes,
                    "warning": format!(
                        "sysctl_not_restored: net.core.rmem_max/wmem_max are {:?}/{:?}, Sew's recorded originals {rm}/{wm} \
                         (the drop-in is gone, so a reboot restores the distro values)",
                        lr, lw
                    ),
                })
            }
        }
        Baseline::None | Baseline::Unreadable if dropin => {
            let dropped = drop_dropin(r);
            json!({
                "state": "sysctl_baseline_missing",
                "live": now(ctx),
                "dropinRemoved": dropped,
                "warning": format!(
                    "sysctl_baseline_missing: {SEW_MEDIA_SYSCTL} was deleted but there was no readable baseline, so \
                     net.core.rmem_max/wmem_max were left as they are (a reboot restores the distro values)"
                ),
            })
        }
        Baseline::Unreadable => json!({"state": "untouched", "note": format!("{SEW_MEDIA_SYSCTL_BASELINE} unreadable and no drop-in")}),
        Baseline::None => json!({"state": "untouched"}),
    }
}

// ── backups, credentials, accounts ──────────────────────────────────────

/// UTC `YYYYmmddTHHMMSSZ` (Sew's stamp for kept backups).
fn utc_stamp(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn move_backups(ctx: &Ctx, r: &mut Report) -> Option<String> {
    let data = ctx.dir(SEW_DATA_DIR).ok()?;
    if data.kind(OsStr::new("backups")).ok() != Some(Kind::Dir) {
        return None;
    }
    let (parent, _) = split(SEW_DATA_DIR);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let name = format!("sew-backups-{}", utc_stamp(now));
    let dest = format!("{parent}/{name}");
    let res = ctx.dir(parent).and_then(|p| data.rename_to(OsStr::new("backups"), &p, OsStr::new(&name)));
    match res {
        Ok(()) => Some(dest),
        Err(e) => {
            r.fail(&format!("{SEW_DATA_DIR}/backups → {dest}"), e);
            None
        }
    }
}

fn credentials(ctx: &Ctx, r: &mut Report) {
    for store in CREDSTORE_DIRS {
        let d = match ctx.dir(store) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                r.fail(store, e);
                continue;
            }
        };
        let names = match d.entries() {
            Ok(v) => v,
            Err(e) => {
                r.fail(store, e);
                continue;
            }
        };
        for n in names.into_iter().filter_map(|n| n.into_string().ok()) {
            let ours = SEW_CRED_NAMES.contains(&n.as_str()) || SEW_CRED_PREFIXES.iter().any(|p| n.starts_with(p));
            if !ours {
                continue;
            }
            let abs = format!("{store}/{n}");
            match d.kind(OsStr::new(&n)) {
                Ok(Kind::File | Kind::Symlink) => match d.unlink(OsStr::new(&n)) {
                    Ok(()) => r.removed.push(abs),
                    Err(e) => r.fail(&abs, e),
                },
                Ok(_) => r.fail(&abs, "not a regular file (left)"),
                Err(e) => r.fail(&abs, e),
            }
        }
    }
}

/// Sew's accounts as `/etc/passwd` (and `/etc/shadow`) have them, with
/// the signs of a login account Sew 0.2.0+ refuses to adopt. Read only.
fn accounts(ctx: &Ctx) -> J {
    let passwd = read_small(ctx, "/etc/passwd", 4 << 20).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let shadow = read_small(ctx, "/etc/shadow", 4 << 20).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let mut out = Vec::new();
    for name in SEW_ACCOUNTS {
        let Some(f) = passwd.lines().map(|l| l.split(':').collect::<Vec<_>>()).find(|f| f.len() >= 7 && f[0] == name) else {
            continue;
        };
        let uid: Option<u32> = f[2].parse().ok();
        let (home, shell) = (f[5], f[6]);
        let mut reasons = Vec::new();
        if uid.is_some_and(|u| u >= 1000) {
            reasons.push(format!("uid {}", f[2]));
        }
        if home == "/home" || home.starts_with("/home/") {
            reasons.push(format!("home {home}"));
        }
        let no_login = ["/usr/sbin/nologin", "/sbin/nologin", "/bin/false", "/usr/bin/false", ""];
        if !no_login.contains(&shell) {
            reasons.push(format!("login shell {shell}"));
        }
        let pw = shadow.lines().map(|l| l.split(':').collect::<Vec<_>>()).find(|s| s.len() >= 2 && s[0] == name).map(|s| s[1].to_string());
        if pw.as_deref().is_some_and(|p| p.starts_with('$')) {
            reasons.push("a password set".into());
        }
        out.push(json!({"name": name, "uid": uid, "home": home, "shell": shell, "loginLike": !reasons.is_empty(), "reasons": reasons, "kept": true}));
    }
    J::Array(out)
}

#[cfg(test)]
mod tests {
    use super::utc_stamp;

    #[test]
    fn utc_stamp_matches_known_instants() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(1_767_398_400), "20260103T000000Z");
        assert_eq!(utc_stamp(1_792_290_896), "20261018T023456Z");
        assert_eq!(utc_stamp(951_782_400), "20000229T000000Z");
    }
}
