//! `requires.sew` / `requires.voice` (0.46.3): a Seed that needs this
//! box's Sew chat server (a version range) or its voice engine says so in
//! its manifest, and K2 checks it against `k2 hostchat status` at install
//! (when the install names a Seed or folder) and at every deploy.
//!
//! - Sew counts as there when its row is `running`, `degraded` or
//!   `stopped` (enabled; `stopped` = its units are down, K2 restarts them)
//!   and it has an installed version. `installing`, `upgrading`,
//!   `disabled` and `removed` don't.
//! - Voice counts as on when Sew is there and `sew media install`
//!   succeeded (`~/.k2/sew/voice.json` `installed`).
//! - On failure the refusal (`requires_unmet`, 409) says what is missing
//!   and the owner command that fixes it. K2 never turns Sew or voice on
//!   by itself here.
//! - Rollbacks, restarts and crash restarts don't check: they put back or
//!   keep what already ran, and refusing them could block a recovery.
//!
//! `k2 app validate` checks only the form (`k2_core::deploy::validate`).

use k2_core::deploy::codes;
use k2_core::deploy::manifest::Requires;
use k2_core::deploy::validate::k2_range_ok;
use serde_json::{json, Value as J};

/// The box's Sew, as the check sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSew {
    /// Sew runs on this OS at all (Linux boxes).
    pub supported: bool,
    /// The `sew_server` row's status (`None` = never installed).
    pub status: Option<String>,
    pub installed_version: Option<String>,
    /// The Sew release this K2 build pins (what `k2 hostchat upgrade` gets).
    pub pinned_version: String,
    /// `sew media install` succeeded.
    pub voice_installed: bool,
}

impl BoxSew {
    /// Read now: the store row and the voice record.
    pub fn read() -> BoxSew {
        let row = crate::sew::store::load();
        let voice = crate::sew::voice::load(&crate::sew::supervisor::live()).installed;
        BoxSew {
            supported: crate::sew::sew_supported(),
            status: row.as_ref().map(|r| r.status.clone()),
            installed_version: row.and_then(|r| r.installed_version),
            pinned_version: k2_root_proto::sew::pinned().version.to_string(),
            voice_installed: voice,
        }
    }

    fn sew_on(&self) -> bool {
        self.installed_version.is_some() && matches!(self.status.as_deref(), Some("running" | "degraded" | "stopped"))
    }

    pub fn json(&self) -> J {
        json!({
            "supported": self.supported,
            "state": self.status.as_deref().unwrap_or("not-installed"),
            "installedVersion": self.installed_version,
            "pinnedVersion": self.pinned_version,
            "voice": self.sew_on() && self.voice_installed,
        })
    }
}

/// Why a Seed's `requires` isn't met: one line per gap, and the owner
/// commands that close them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmet {
    pub missing: Vec<String>,
    pub fix: Vec<String>,
}

impl Unmet {
    pub fn message(&self, app: &str) -> String {
        format!(
            "{app} needs {} on this box. The owner fixes it with: {}. K2 doesn't turn chat or voice hosting on by itself; \
             then install or deploy again.",
            self.missing.join("; "),
            self.fix.join(", then ")
        )
    }

    pub fn json(&self, st: &BoxSew) -> J {
        json!({ "missing": self.missing, "fix": self.fix, "sew": st.json() })
    }
}

/// Pure: does `st` meet `req`?
pub fn check(req: Option<&Requires>, st: &BoxSew) -> Result<(), Unmet> {
    let Some(req) = req else { return Ok(()) };
    if req.sew.is_none() && !req.voice {
        return Ok(());
    }
    let mut missing = Vec::new();
    let mut fix = Vec::new();
    let voice_flag = if req.voice { " --voice" } else { "" };
    let want = req.sew.as_deref().map(|r| format!("Sew {r}; ")).unwrap_or_default();
    if !st.supported {
        missing.push(format!("Chat / Voice Hosting ({want}it runs on Linux boxes only, not on this one)"));
        if req.voice {
            missing.push("voice (Sew's media engine)".to_string());
        }
        fix.push("install the App on a Linux K2 box".to_string());
        return Err(Unmet { missing, fix });
    }
    if !st.sew_on() {
        let state = match st.status.as_deref() {
            None => "not installed".to_string(),
            Some(s) => s.to_string(),
        };
        missing.push(format!("Chat / Voice Hosting ({want}it is {state} here)"));
        if req.voice {
            missing.push("voice (Sew's media engine)".to_string());
        }
        match st.status.as_deref() {
            Some("installing" | "upgrading") => fix.push("wait for it to finish (k2 hostchat status)".to_string()),
            _ => fix.push(format!("k2 hostchat enable{voice_flag}")),
        }
        if let Some(range) = &req.sew {
            if k2_range_ok(range, &st.pinned_version) != Ok(true) {
                fix.insert(0, format!("update K2 first (this K2 pins Sew {}, which isn't {range}): k2 update", st.pinned_version));
            }
        }
        return Err(Unmet { missing, fix });
    }
    let have = st.installed_version.clone().unwrap_or_default();
    if let Some(range) = &req.sew {
        match k2_range_ok(range, &have) {
            Ok(true) => {}
            Ok(false) => {
                missing.push(format!("Sew {range} (this box runs Sew {have})"));
                if k2_range_ok(range, &st.pinned_version) == Ok(true) {
                    fix.push(format!("k2 hostchat upgrade (to Sew {})", st.pinned_version));
                } else {
                    fix.push(format!("update K2 (this K2 pins Sew {}, which isn't {range}): k2 update, then k2 hostchat upgrade", st.pinned_version));
                }
            }
            Err(e) => {
                missing.push(format!("a valid requires.sew ({e})"));
                fix.push("fix the manifest (k2 app validate)".to_string());
            }
        }
    }
    if req.voice && !st.voice_installed {
        missing.push("voice (Sew's media engine is off)".to_string());
        fix.push("k2 hostchat media install".to_string());
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Unmet { missing, fix })
    }
}

/// A refused `requires` (the route's and the pipeline's answer).
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub code: &'static str,
    pub status: u16,
    pub msg: String,
    pub extra: J,
}

/// Sew and voice only (install with a Seed checks these before the App
/// exists; `requires.postgres` is the install's database plan, PA7).
fn check_sew_live(app: &str, req: Option<&Requires>) -> Result<(), Refused> {
    if req.is_none_or(|r| r.sew.is_none() && !r.voice) {
        return Ok(());
    }
    let st = BoxSew::read();
    check(req, &st).map_err(|u| Refused { code: codes::REQUIRES_UNMET, status: 409, msg: u.message(app), extra: u.json(&st) })
}

/// The live check for an App's Seed manifest (the deploy pipeline's entry
/// and the re-host pre-flight): Sew and voice, then (0.46.4, PA5)
/// `requires.postgres` against the App's database. An App not installed
/// yet passes the Postgres half: the install's database plan checks it
/// (PA7).
pub fn check_live(app: &str, req: Option<&Requires>) -> Result<(), Refused> {
    check_sew_live(app, req)?;
    let Some(range) = req.and_then(|r| r.postgres.as_deref()) else { return Ok(()) };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::get_app(&conn, app).ok().flatten()
    };
    let Some(row) = row else { return Ok(()) };
    super::pg_choice::check_live(&row, Some(range)).map_err(|u| Refused { code: u.code, status: u.status, msg: u.hint, extra: u.extra })
}

fn to_refusal(r: Refused) -> super::routes::Refusal {
    let mut out = super::routes::no(r.code, r.status, r.msg);
    out.extra = Some(r.extra);
    out
}

/// `k2 app install … --seed S | --folder F`: check the Seed's (or the
/// folder's `manifest.json`) `requires.sew`/`voice` before anything is
/// created, so an unmet one leaves no half-installed App, and hand back
/// its `requires` (the install's database plan reads `postgres`, PA7). A
/// manifest that doesn't parse is left to the build, which reports it
/// with its place.
pub(crate) fn install_precheck(who: &super::routes::Who, app: &str, v: &J) -> Result<Option<Requires>, super::routes::Refusal> {
    use super::routes::s;
    let m: Option<k2_core::deploy::manifest::Manifest> = if let Some(seed) = s(v, "seed") {
        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::deploy::store::find_seed(&conn, seed).ok().flatten()
        };
        row.and_then(|r| serde_json::from_value(r.manifest).ok())
    } else if let Some(folder) = s(v, "folder") {
        let folder = super::routes::resolve_folder(who, folder)?;
        std::fs::read_to_string(folder.join(k2_core::deploy::manifest::MANIFEST_FILE))
            .ok()
            .and_then(|t| k2_core::deploy::manifest::parse(&t).ok())
    } else {
        None
    };
    match m {
        Some(m) => {
            check_sew_live(app, m.requires.as_ref()).map_err(to_refusal)?;
            Ok(m.requires)
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(sew: Option<&str>, voice: bool) -> Requires {
        Requires { k2: None, features: vec![], sew: sew.map(str::to_string), voice, postgres: None }
    }

    fn running(v: &str, voice: bool) -> BoxSew {
        BoxSew {
            supported: true,
            status: Some("running".into()),
            installed_version: Some(v.into()),
            pinned_version: "0.3.1".into(),
            voice_installed: voice,
        }
    }

    #[test]
    fn nothing_required_always_passes() {
        let none = BoxSew { supported: false, status: None, installed_version: None, pinned_version: "0.3.1".into(), voice_installed: false };
        assert_eq!(check(None, &none), Ok(()));
        assert_eq!(check(Some(&req(None, false)), &none), Ok(()));
    }

    #[test]
    fn a_met_range_and_voice_pass() {
        assert_eq!(check(Some(&req(Some(">=0.3.0"), true)), &running("0.3.1", true)), Ok(()));
        assert_eq!(check(Some(&req(Some(">=0.2.0"), false)), &running("0.3.1", false)), Ok(()));
        let mut degraded = running("0.3.1", true);
        degraded.status = Some("degraded".into());
        assert_eq!(check(Some(&req(Some(">=0.3.0 <0.4.0"), true)), &degraded), Ok(()));
        // Voice alone (Meet) needs Sew there and its media engine on.
        assert_eq!(check(Some(&req(None, true)), &running("0.2.0", true)), Ok(()));
    }

    #[test]
    fn no_sew_names_enable_with_voice() {
        let st = BoxSew { supported: true, status: None, installed_version: None, pinned_version: "0.3.1".into(), voice_installed: false };
        let u = check(Some(&req(Some(">=0.3.0"), true)), &st).unwrap_err();
        assert_eq!(u.fix, vec!["k2 hostchat enable --voice".to_string()], "{u:?}");
        assert!(u.missing[0].contains("not installed") && u.missing[0].contains(">=0.3.0"), "{u:?}");
        assert!(u.missing.iter().any(|m| m.contains("voice")), "{u:?}");
        let msg = u.message("meet");
        assert!(msg.starts_with("meet needs") && msg.contains("k2 hostchat enable --voice"), "{msg}");
        // Chat only: no --voice.
        let u = check(Some(&req(Some(">=0.2.0"), false)), &st).unwrap_err();
        assert_eq!(u.fix, vec!["k2 hostchat enable".to_string()]);
        // Disabled: enable again. Mid-install: wait.
        let mut off = running("0.3.1", false);
        off.status = Some("disabled".into());
        let u = check(Some(&req(Some(">=0.2.0"), false)), &off).unwrap_err();
        assert!(u.missing[0].contains("disabled"), "{u:?}");
        assert_eq!(u.fix, vec!["k2 hostchat enable".to_string()]);
        off.status = Some("upgrading".into());
        let u = check(Some(&req(Some(">=0.2.0"), false)), &off).unwrap_err();
        assert!(u.fix[0].contains("wait"), "{u:?}");
    }

    #[test]
    fn an_old_sew_names_upgrade_or_a_k2_update() {
        let u = check(Some(&req(Some(">=0.3.0"), false)), &running("0.2.4", false)).unwrap_err();
        assert_eq!(u.missing, vec!["Sew >=0.3.0 (this box runs Sew 0.2.4)".to_string()]);
        assert_eq!(u.fix, vec!["k2 hostchat upgrade (to Sew 0.3.1)".to_string()]);
        // This K2's pin doesn't reach it either: update K2 first.
        let u = check(Some(&req(Some(">=0.4.0"), false)), &running("0.3.1", false)).unwrap_err();
        assert!(u.fix[0].starts_with("update K2") && u.fix[0].contains("k2 hostchat upgrade"), "{u:?}");
        // Too new is unmet too (an upper bound).
        let u = check(Some(&req(Some(">=0.2.0 <0.3.0"), false)), &running("0.3.1", false)).unwrap_err();
        assert!(u.missing[0].contains("0.3.1"), "{u:?}");
    }

    #[test]
    fn voice_off_names_media_install() {
        let u = check(Some(&req(Some(">=0.3.0"), true)), &running("0.3.1", false)).unwrap_err();
        assert_eq!(u.missing, vec!["voice (Sew's media engine is off)".to_string()]);
        assert_eq!(u.fix, vec!["k2 hostchat media install".to_string()]);
        // Both gaps at once: both named, in order.
        let u = check(Some(&req(Some(">=0.3.0"), true)), &running("0.2.0", false)).unwrap_err();
        assert_eq!(u.missing.len(), 2, "{u:?}");
        assert_eq!(u.fix[1], "k2 hostchat media install");
    }

    #[test]
    fn off_linux_says_so() {
        let mac = BoxSew { supported: false, status: None, installed_version: None, pinned_version: "0.3.1".into(), voice_installed: false };
        let u = check(Some(&req(Some(">=0.2.0"), false)), &mac).unwrap_err();
        assert!(u.missing[0].contains("Linux"), "{u:?}");
    }

    /// The live read: the `sew_server` row and the voice record under the
    /// temp `~/.k2`.
    #[test]
    fn box_state_reads_the_store_and_the_voice_record() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let st = BoxSew::read();
        assert_eq!(st.status, None);
        assert!(!st.voice_installed);
        assert_eq!(st.json()["state"], "not-installed");
        let mut row = crate::sew::store::Row::new(&st.pinned_version, 10);
        row.status = "running".into();
        row.installed_version = Some("0.3.0".into());
        crate::sew::store::save(&row).unwrap();
        let p = crate::sew::ops::voice_state_path();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, r#"{"installed": true, "updatedAt": 1}"#).unwrap();
        let st = BoxSew::read();
        assert_eq!(st.status.as_deref(), Some("running"));
        assert_eq!(st.installed_version.as_deref(), Some("0.3.0"));
        assert!(st.voice_installed);
        assert_eq!(st.supported, cfg!(target_os = "linux"));
        assert_eq!(st.json()["state"], "running");
        // What the box read meets, given an OS that runs Sew.
        let on_linux = BoxSew { supported: true, ..st.clone() };
        assert_eq!(check(Some(&req(Some(">=0.3.0"), true)), &on_linux), Ok(()));
        assert_eq!(on_linux.json()["voice"], true);
        assert!(check(Some(&req(Some(">=0.4.0"), false)), &on_linux).is_err());
        // Disabled: the row stays, Sew doesn't count.
        crate::sew::store::update(20, |r| r.status = "disabled".into()).unwrap();
        let off = BoxSew { supported: true, ..BoxSew::read() };
        assert_eq!(off.json()["voice"], false, "voice needs Sew on");
        assert!(check(Some(&req(None, true)), &off).is_err());
    }
}
