//! The daemon unit's hand-off drop-in (runtime stage H, vs-live AR3/K37)
//! and the `/cli/daemon/handoff*` routes.
//!
//! The drop-in (`k2_root_proto::templates::handoff_dropin`) gives the
//! systemd unit `KillMode=process` (frpc, published services and skin
//! helpers survive a stop, restart or crash and are re-adopted),
//! `NotifyAccess=main` (a hand-off can move the unit's main pid),
//! `ExecReload=` SIGUSR2 (`systemctl reload` = a no-downtime hand-off) and
//! `OOMPolicy=continue` (an OOM-killed agent in the unit's cgroup no longer
//! makes systemd stop and restart the whole daemon) and, since 0.46.4,
//! `RuntimeDirectory=k2` (0700, preserved): the runtime dir systemd-logind
//! can't delete (`k2_core::runtime_dir`; from the unit's next start).
//!
//! - **System unit** (provisioned boxes): through the root helper's
//!   `daemon handoff-dropin install` (protocol 5). The unit comes from the
//!   helper's `daemon.toml`, never from us.
//! - **User unit** (Arch package, `k2 daemon install`): the daemon writes
//!   `~/.config/systemd/user/<unit>.d/k2-handoff.conf` itself and runs
//!   `systemctl --user daemon-reload` — no root.
//!
//! [`ensure_on_boot`] converges it once per boot (Linux, under systemd;
//! `K2_HANDOFF_DROPIN=off` opts out). A changed drop-in takes effect for
//! the unit's next stop; `NotifyAccess` applies at once, so the very next
//! hand-off works without a restart (the notify socket is the manager's
//! well-known one).
//!
//! **After a helper update** (0.46.5 patch-up, K2 Web's 0.46.4 roll): the
//! first boot of a new daemon with the previous release's helper gets that
//! helper's older drop-in, so the boot loop below keeps retrying. When the
//! helper is updated after that loop gave up (`k2 root-helper update`, or
//! the daily auto-heal), [`after_helper_update`] converges the drop-in at
//! once through the new helper (which writes it and runs `systemctl
//! daemon-reload` itself), instead of waiting for another daemon restart.
//! One restart is still needed, and the update's output says so: systemd
//! applies the drop-in's `RuntimeDirectory=` (and `KillMode=`) when the
//! unit next starts/stops, so a unit that started without this build's
//! drop-in runs without them until then ([`restart_needed`]). Before: the
//! upgrade boot, one restart to write the drop-in, one to apply it. Now:
//! the upgrade boot and one restart.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use k2_core::log_debug;
use k2_root_proto::templates::handoff_dropin;
use k2_root_proto::version::HANDOFF_MIN_PROTOCOL;

/// Opt-out for the boot-time convergence.
pub const OPT_OUT_ENV: &str = "K2_HANDOFF_DROPIN";

fn under_systemd() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
}

/// `~/.config/systemd/user/<unit>.d/k2-handoff.conf`.
fn user_dropin_path(unit: &str) -> Option<std::path::PathBuf> {
    Some(
        dirs::config_dir()?
            .join("systemd")
            .join("user")
            .join(format!("{unit}.d"))
            .join(k2_root_proto::paths::HANDOFF_DROPIN_FILE),
    )
}

/// The drop-in path for `unit` in `scope` ("user" or "system").
fn dropin_path(unit: &str, scope: &str) -> Option<std::path::PathBuf> {
    if scope == "user" {
        user_dropin_path(unit)
    } else {
        Some(
            std::path::Path::new(k2_root_proto::paths::SYSTEMD_SYSTEM_DIR)
                .join(format!("{unit}.d"))
                .join(k2_root_proto::paths::HANDOFF_DROPIN_FILE),
        )
    }
}

/// Whether the file at `path` holds exactly this build's drop-in bytes.
fn holds_current_dropin(path: Option<&std::path::Path>) -> bool {
    path.and_then(|p| std::fs::read(p).ok())
        .is_some_and(|b| b == handoff_dropin().as_bytes())
}

/// Set once this daemon has seen its unit's drop-in file without this
/// build's bytes (or rewrote it): the running unit started without them,
/// so `RuntimeDirectory=k2` and `KillMode=process` apply only from the
/// unit's next start. Per process: a restart clears it.
static RESTART_NEEDED: AtomicBool = AtomicBool::new(false);

/// Whether this daemon's unit must restart once before its drop-in is
/// fully in effect (see [`RESTART_NEEDED`]).
pub fn restart_needed() -> bool {
    RESTART_NEEDED.load(Ordering::SeqCst)
}

/// The command that restarts this daemon's unit.
fn restart_hint(unit: &str, scope: &str) -> String {
    if scope == "user" {
        format!("k2 daemon restart (or: systemctl --user restart {unit})")
    } else {
        format!("k2 daemon restart from a root shell (or: sudo systemctl restart {unit})")
    }
}

/// Where the drop-in for this daemon's unit lives, and whether it holds
/// the canned bytes.
pub fn dropin_state() -> Value {
    if !cfg!(target_os = "linux") || !under_systemd() {
        return json!({ "applies": false, "why": "not a systemd unit" });
    }
    let (_user, unit, scope) = crate::root_helper::own_unit();
    let path = dropin_path(&unit, &scope);
    let current = holds_current_dropin(path.as_deref());
    json!({
        "applies": true,
        "unit": unit,
        "scope": scope,
        "path": path.map(|p| p.display().to_string()),
        "installed": current,
        "restartNeeded": current && restart_needed(),
    })
}

/// What one convergence of the drop-in did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Converge {
    /// Not a systemd unit (macOS, a dev run, no `INVOCATION_ID`).
    NotApplicable(String),
    /// The file holds this build's bytes; `changed` = written just now.
    Current { changed: bool },
    /// The helper answered but wrote its own (older) drop-in: it is older
    /// than this daemon; converge again after it updates.
    Stale,
    Failed(String),
}

/// Everything a convergence touches, so tests never run sudo or systemd.
pub trait DropinOps {
    /// `Err(why)` when the drop-in doesn't apply to this daemon.
    fn applies(&self) -> Result<(), String>;
    /// Whether the unit's drop-in file holds this build's bytes now.
    fn is_current(&self) -> bool;
    /// Write it (system unit: the root helper, which also reloads systemd).
    fn install(&self) -> Result<bool, String>;
}

/// The live drop-in of this daemon's own unit.
pub struct LiveDropin;

impl DropinOps for LiveDropin {
    fn applies(&self) -> Result<(), String> {
        if !cfg!(target_os = "linux") || !under_systemd() {
            return Err("not a systemd unit".into());
        }
        Ok(())
    }
    fn is_current(&self) -> bool {
        let (_user, unit, scope) = crate::root_helper::own_unit();
        holds_current_dropin(dropin_path(&unit, &scope).as_deref())
    }
    fn install(&self) -> Result<bool, String> {
        install()
    }
}

/// Converge the drop-in once. `only_if_stale`: when the file already holds
/// this build's bytes, make no root call at all (the daily auto-heal);
/// otherwise always ask the helper (an owner's `k2 root-helper update`:
/// the helper also reloads systemd when the unit's loaded config is stale).
/// A file found without this build's bytes, or rewritten, marks the
/// running unit as needing one restart (`restart_flag`).
pub fn converge_with(
    ops: &dyn DropinOps,
    restart_flag: &AtomicBool,
    only_if_stale: bool,
) -> Converge {
    if let Err(why) = ops.applies() {
        return Converge::NotApplicable(why);
    }
    if ops.is_current() {
        if only_if_stale {
            return Converge::Current { changed: false };
        }
    } else {
        restart_flag.store(true, Ordering::SeqCst);
    }
    match ops.install() {
        Ok(changed) => {
            if changed {
                restart_flag.store(true, Ordering::SeqCst);
            }
            if ops.is_current() {
                Converge::Current { changed }
            } else {
                Converge::Stale
            }
        }
        Err(e) => Converge::Failed(e),
    }
}

/// The `dropin` block of `POST /cli/root-helper/update`'s answer.
pub fn converge_value(c: &Converge, restart_needed: bool, unit: &str, scope: &str) -> Value {
    match c {
        Converge::NotApplicable(why) => json!({"result": "not_applicable", "reason": why}),
        Converge::Current { changed } => {
            let mut v = json!({
                "result": if *changed { "installed" } else { "current" },
                "unit": unit,
                "scope": scope,
                "restartNeeded": restart_needed,
            });
            if restart_needed {
                v["restart"] = json!(restart_hint(unit, scope));
                v["why"] = json!(
                    "the daemon unit started without this drop-in: RuntimeDirectory=k2 and \
                     KillMode=process take effect when the unit next starts (one restart, \
                     when convenient)"
                );
            }
            v
        }
        Converge::Stale => json!({
            "result": "stale",
            "unit": unit,
            "scope": scope,
            "reason": "the root helper wrote an older drop-in than this daemon's: update the \
                       helper (k2 root-helper update); the daemon converges it right after",
        }),
        Converge::Failed(e) => {
            json!({"result": "failed", "unit": unit, "scope": scope, "reason": e})
        }
    }
}

/// Right after the root helper was updated (or found current): converge
/// the drop-in now, through the new helper, instead of at the next boot.
/// `only_if_stale` as in [`converge_with`]. Honors [`OPT_OUT_ENV`].
pub fn after_helper_update(only_if_stale: bool) -> Value {
    if opted_out() {
        return json!({"result": "opted_out", "reason": format!("{OPT_OUT_ENV}=off")});
    }
    let c = converge_with(&LiveDropin, &RESTART_NEEDED, only_if_stale);
    let (_user, unit, scope) = if matches!(c, Converge::NotApplicable(_)) {
        (String::new(), String::new(), String::new())
    } else {
        crate::root_helper::own_unit()
    };
    match &c {
        Converge::Current { changed: true } => log_debug!(
            "[daemon/handoff] unit drop-in installed after the root helper update; \
             it takes full effect at the unit's next start"
        ),
        Converge::Stale => log_debug!(
            "[daemon/handoff] unit drop-in still differs from this build's after the helper update"
        ),
        Converge::Failed(e) => log_debug!("[daemon/handoff] unit drop-in not installed: {e}"),
        _ => {}
    }
    converge_value(&c, restart_needed(), &unit, &scope)
}

fn opted_out() -> bool {
    std::env::var(OPT_OUT_ENV)
        .map(|v| v == "off" || v == "0")
        .unwrap_or(false)
}

/// Install (or re-assert) the drop-in. `Ok(changed)`.
pub fn install() -> Result<bool, String> {
    if !cfg!(target_os = "linux") {
        return Err("the hand-off drop-in is for systemd units (Linux)".into());
    }
    if !under_systemd() {
        return Err("this daemon is not running under systemd".into());
    }
    let (_user, unit, scope) = crate::root_helper::own_unit();
    if scope == "user" {
        let path = user_dropin_path(&unit).ok_or("no config dir for the user unit")?;
        let dir = path.parent().ok_or("drop-in path has no parent")?;
        if std::fs::read(&path).ok().as_deref() == Some(handoff_dropin().as_bytes()) {
            return Ok(false);
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        let tmp = dir.join(format!(".k2-handoff.conf.tmp.{}", std::process::id()));
        std::fs::write(&tmp, handoff_dropin()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))?;
        let st = k2_core::bg_proc::command("systemctl")
            .args(["--user", "daemon-reload"])
            .status()
            .map_err(|e| format!("systemctl --user daemon-reload: {e}"))?;
        if !st.success() {
            return Err(format!("systemctl --user daemon-reload exited {st}"));
        }
        return Ok(true);
    }
    let st = crate::root_helper::state();
    if !st.usable(HANDOFF_MIN_PROTOCOL) {
        return Err(crate::root_helper::helper_needed(
            "the daemon unit's hand-off drop-in",
            HANDOFF_MIN_PROTOCOL,
            &st,
        ));
    }
    let out = crate::root_helper::call_text(
        &[
            "daemon".to_string(),
            "handoff-dropin".to_string(),
            "install".to_string(),
        ],
        None,
    )?;
    let v: Value = serde_json::from_str(&out).map_err(|e| format!("root helper answer: {e}"))?;
    Ok(v["changed"].as_bool().unwrap_or(false))
}

/// Boot: converge the drop-in once, in the background. Never fails boot.
pub fn ensure_on_boot() {
    if !cfg!(target_os = "linux") || !under_systemd() {
        return;
    }
    if opted_out() {
        log_debug!("[daemon/handoff] {OPT_OUT_ENV}=off — not touching the unit's drop-in");
        return;
    }
    // Retries cover a box whose root helper is still protocol 4 at boot:
    // the helper auto-heal installs this version's helper ~2 min in. They
    // also cover an older helper that answers but renders its own (older)
    // drop-in, e.g. a 0.46.0 helper without `OOMPolicy=continue`: the
    // system unit counts as done only once the file holds this build's
    // bytes, which it does after the helper self-updates. A helper updated
    // after the last try (`k2 root-helper update`, the daily auto-heal)
    // converges it through [`after_helper_update`] (0.46.5).
    std::thread::Builder::new()
        .name("k2-handoff-dropin".into())
        .spawn(|| {
            for wait in [0u64, 240, 900] {
                std::thread::sleep(std::time::Duration::from_secs(wait));
                if k2_core::handoff_state::draining() {
                    return;
                }
                match converge_with(&LiveDropin, &RESTART_NEEDED, false) {
                    Converge::Current { changed } => {
                        if changed {
                            log_debug!(
                                "[daemon/handoff] unit drop-in installed (KillMode=process, \
                                 NotifyAccess=main, ExecReload=SIGUSR2, OOMPolicy=continue, \
                                 RuntimeDirectory=k2): frpc and Apps survive a daemon restart, \
                                 and an OOM-killed agent no longer stops the daemon; it takes \
                                 full effect at the unit's next start"
                            );
                        }
                        return;
                    }
                    Converge::Stale => log_debug!(
                        "[daemon/handoff] unit drop-in differs from this build's \
                         (older root helper?); retrying after the helper updates \
                         (k2 root-helper update converges it at once)"
                    ),
                    Converge::Failed(e) => {
                        log_debug!("[daemon/handoff] unit drop-in not installed (yet): {e}")
                    }
                    Converge::NotApplicable(_) => return,
                }
            }
        })
        .ok();
}

/// `GET /cli/daemon/handoff/status`.
pub fn status_json() -> Value {
    let capable = crate::handoff::capable();
    json!({
        "capable": capable.is_ok(),
        "why": capable.err(),
        "handedOff": crate::handoff::handed_off(),
        "bootedFromHandoff": crate::handoff::booted_from_handoff(),
        "liveSplices": crate::handoff::live_splices(),
        "dropin": dropin_state(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const NONE: u8 = 0;
    const OLD: u8 = 1;
    const NEW: u8 = 2;

    /// A fake unit drop-in + root helper. `file` is what the drop-in file
    /// holds (none, the older helper's bytes, this build's bytes); `helper`
    /// is what the helper renders (an older helper writes its own bytes).
    struct Fake {
        applies: bool,
        file: Cell<u8>,
        helper: Cell<u8>,
        fail: RefCell<Option<String>>,
        installs: Cell<u32>,
    }

    impl Fake {
        fn new(file: u8, helper: u8) -> Self {
            Self {
                applies: true,
                file: Cell::new(file),
                helper: Cell::new(helper),
                fail: RefCell::new(None),
                installs: Cell::new(0),
            }
        }
    }

    impl DropinOps for Fake {
        fn applies(&self) -> Result<(), String> {
            if self.applies {
                Ok(())
            } else {
                Err("not a systemd unit".into())
            }
        }
        fn is_current(&self) -> bool {
            self.file.get() == NEW
        }
        fn install(&self) -> Result<bool, String> {
            self.installs.set(self.installs.get() + 1);
            if let Some(e) = self.fail.borrow().clone() {
                return Err(e);
            }
            let changed = self.file.get() != self.helper.get();
            self.file.set(self.helper.get());
            Ok(changed)
        }
    }

    /// K2 Web's 0.46.4 roll: the boot finds the older helper's drop-in;
    /// the helper update must converge it at once (one helper call, no
    /// daemon restart in between) and say ONE restart is still needed.
    #[test]
    fn a_helper_update_converges_a_stale_dropin_without_a_restart() {
        let flag = AtomicBool::new(false);
        // Boot, previous release's helper: it keeps its own older bytes.
        let ops = Fake::new(OLD, OLD);
        assert_eq!(converge_with(&ops, &flag, false), Converge::Stale);
        assert!(
            flag.load(Ordering::SeqCst),
            "the unit started without this build's drop-in"
        );
        // `k2 root-helper update` (or the auto-heal) installs the new helper;
        // even the auto-heal's only-if-stale path converges it.
        ops.helper.set(NEW);
        let before = ops.installs.get();
        let c = converge_with(&ops, &flag, true);
        assert_eq!(c, Converge::Current { changed: true });
        assert_eq!(ops.installs.get(), before + 1, "exactly one helper call");
        let v = converge_value(
            &c,
            flag.load(Ordering::SeqCst),
            "k2-daemon.service",
            "system",
        );
        assert_eq!(v["result"], "installed");
        assert_eq!(v["unit"], "k2-daemon.service");
        assert_eq!(v["restartNeeded"], true);
        assert_eq!(
            v["restart"],
            "k2 daemon restart from a root shell (or: sudo systemctl restart k2-daemon.service)"
        );
        let why = v["why"].as_str().expect("a why for the restart");
        assert!(why.contains("RuntimeDirectory=k2"), "{why}");
    }

    /// A fresh box: no drop-in at boot, an up-to-date helper writes it;
    /// the running unit still started without it.
    #[test]
    fn a_first_install_needs_one_restart() {
        let flag = AtomicBool::new(false);
        let ops = Fake::new(NONE, NEW);
        assert_eq!(
            converge_with(&ops, &flag, false),
            Converge::Current { changed: true }
        );
        assert!(flag.load(Ordering::SeqCst));
    }

    /// The daily auto-heal makes no root call when the file is current, and
    /// a unit that started with this build's drop-in needs no restart.
    #[test]
    fn only_if_stale_skips_the_helper_when_current() {
        let flag = AtomicBool::new(false);
        let ops = Fake::new(NEW, NEW);
        assert_eq!(
            converge_with(&ops, &flag, true),
            Converge::Current { changed: false }
        );
        assert_eq!(ops.installs.get(), 0);
        assert!(!flag.load(Ordering::SeqCst));
        let v = converge_value(
            &Converge::Current { changed: false },
            false,
            "k2.service",
            "user",
        );
        assert_eq!(v["result"], "current");
        assert_eq!(v["restartNeeded"], false);
        assert!(v.get("restart").is_none(), "{v}");
    }

    /// An owner's update always asks the helper (which also repairs a stale
    /// systemd load); a current file still needs no restart.
    #[test]
    fn an_owner_update_asks_the_helper_even_when_current() {
        let flag = AtomicBool::new(false);
        let ops = Fake::new(NEW, NEW);
        assert_eq!(
            converge_with(&ops, &flag, false),
            Converge::Current { changed: false }
        );
        assert_eq!(ops.installs.get(), 1);
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn a_failed_install_and_a_non_unit_are_reported() {
        let flag = AtomicBool::new(false);
        let ops = Fake::new(OLD, OLD);
        *ops.fail.borrow_mut() = Some("helper_needed: protocol 4".into());
        let c = converge_with(&ops, &flag, false);
        assert_eq!(c, Converge::Failed("helper_needed: protocol 4".into()));
        let v = converge_value(&c, true, "k2-daemon.service", "system");
        assert_eq!(v["result"], "failed");
        assert_eq!(v["reason"], "helper_needed: protocol 4");
        assert!(v.get("restartNeeded").is_none(), "{v}");

        let mut ops = Fake::new(OLD, NEW);
        ops.applies = false;
        let c = converge_with(&ops, &flag, false);
        assert_eq!(c, Converge::NotApplicable("not a systemd unit".into()));
        assert_eq!(ops.installs.get(), 0);
        assert_eq!(converge_value(&c, false, "", "")["result"], "not_applicable");
    }

    #[test]
    fn a_stale_answer_points_at_the_helper_update() {
        let v = converge_value(&Converge::Stale, true, "k2-daemon.service", "system");
        assert_eq!(v["result"], "stale");
        let reason = v["reason"].as_str().expect("reason");
        assert!(reason.contains("k2 root-helper update"), "{reason}");
        assert!(
            v.get("restartNeeded").is_none(),
            "no restart advice before it is written"
        );
    }

    #[test]
    fn a_user_unit_restart_hint_needs_no_root() {
        assert_eq!(
            restart_hint("k2.service", "user"),
            "k2 daemon restart (or: systemctl --user restart k2.service)"
        );
    }
}
