//! Re-host on a K2 update, the pure parts (prd-app-runtime-update-continuity-v1
//! S2, 0.46.4): the runtime fingerprint and when an App is due (R10), the
//! per-App policy and window (R14), "can't run two copies" (Rosson Q4 +
//! Q16: `pause` by default, `defer` per App), where sessions live (R31),
//! and the measured memory headroom (R12, Q15). The daemon's
//! `apps::rehost` drives the wave with these.

use serde::{Deserialize, Serialize};

/// `apps.rehost_policy` values (0221 CHECK).
pub const POLICY_AUTO: &str = "auto";
pub const POLICY_HANDOVER: &str = "handover";
pub const POLICY_WINDOW: &str = "window";
pub const POLICY_MANUAL: &str = "manual";
pub const POLICY_OFF: &str = "off";
pub const POLICIES: &[&str] = &[POLICY_AUTO, POLICY_HANDOVER, POLICY_WINDOW, POLICY_MANUAL, POLICY_OFF];

/// `apps.rehost_when_no_overlap` (0221; Rosson 2026-10-24): an App that
/// can't run two copies at once (no room for both, extra ports, the
/// process exception, `handover`) either pauses briefly and updates
/// (`pause`, the default) or is skipped for the owner to update
/// (`defer`).
pub const NO_OVERLAP_PAUSE: &str = "pause";
pub const NO_OVERLAP_DEFER: &str = "defer";

/// The box-wide off switch.
pub const REHOST_ENV: &str = "K2_APPS_REHOST";

/// The memory floor a re-host leaves free (R12).
pub const MEM_FLOOR: u64 = 512 << 20;

/// Why an App was not re-hosted this time (`deploy_audit`
/// `rehost_deferred`, `GET /cli/app/rehost`).
pub mod defer {
    pub const REQUIRES_UNMET: &str = "requires_unmet";
    pub const SESSIONS_IN_MEMORY: &str = "sessions_in_memory";
    pub const NEEDS_HANDOVER: &str = "needs_handover";
    pub const NO_HEADROOM: &str = "no_headroom";
    pub const CORE_UPGRADE_WAIT: &str = "core_upgrade_wait";
    pub const PREDECESSOR_DRAINING: &str = "predecessor_draining";
    pub const POLICY_MANUAL: &str = "policy_manual";
    pub const POLICY_OFF: &str = "policy_off";
    pub const OUTSIDE_WINDOW: &str = "outside_window";
    pub const REHOST_FAILED: &str = "rehost_failed";
    pub const BUSY: &str = "busy";
}

/// `deployments.runtime_json` (R10): what an instance runs on. Written at
/// `go_live` from the `version` answer `prepare` checked; never on adopt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub k2_version: String,
    #[serde(default)]
    pub engine_version: Option<String>,
    #[serde(default)]
    pub engine_protocol: Option<u64>,
    #[serde(default)]
    pub image_digest: Option<String>,
    /// `guarded` | `relaxed` | `process` (the exception) | `unwalled`.
    #[serde(default)]
    pub wall: String,
}

impl Fingerprint {
    pub fn parse(s: Option<&str>) -> Option<Fingerprint> {
        serde_json::from_str(s?).ok()
    }
}

/// Is an instance with fingerprint `fp` due for a re-host on this K2
/// (`k2_version`, the running engine's `engine_version`)? `Some(why)`.
/// A missing fingerprint (rows from before 0221) is due (A8).
pub fn due(fp: Option<&Fingerprint>, k2_version: &str, engine_version: Option<&str>) -> Option<&'static str> {
    let Some(fp) = fp else { return Some("no_fingerprint") };
    if fp.k2_version != k2_version {
        return Some("k2_version");
    }
    if let (Some(have), Some(now)) = (fp.engine_version.as_deref(), engine_version) {
        if have != now {
            return Some("engine_version");
        }
    }
    None
}

/// `"HH:MM-HH:MM"` → minutes of the day (start, end). `None` if malformed.
pub fn parse_window(w: &str) -> Option<(u32, u32)> {
    let (a, b) = w.trim().split_once('-')?;
    let m = |t: &str| -> Option<u32> {
        let (h, mm) = t.trim().split_once(':')?;
        let (h, mm): (u32, u32) = (h.parse().ok()?, mm.parse().ok()?);
        (h < 24 && mm < 60 && t.trim().len() == 5).then_some(h * 60 + mm)
    };
    let (s, e) = (m(a)?, m(b)?);
    (s != e).then_some((s, e))
}

/// Is `minute` (of the day, box time) inside `window`? A window may wrap
/// midnight (`22:00-02:00`).
pub fn in_window(window: (u32, u32), minute: u32) -> bool {
    let (s, e) = window;
    if s < e {
        minute >= s && minute < e
    } else {
        minute >= s || minute < e
    }
}

/// What the policy says for an automatic (update) re-host right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyGate {
    Go,
    /// Not now; the code says why.
    Wait(&'static str),
}

pub fn policy_gate(policy: &str, window: Option<&str>, minute: u32) -> PolicyGate {
    match policy {
        POLICY_OFF => PolicyGate::Wait(defer::POLICY_OFF),
        POLICY_MANUAL => PolicyGate::Wait(defer::POLICY_MANUAL),
        POLICY_WINDOW => match window.and_then(parse_window) {
            Some(w) if in_window(w, minute) => PolicyGate::Go,
            _ => PolicyGate::Wait(defer::OUTSIDE_WINDOW),
        },
        _ => PolicyGate::Go,
    }
}

/// Where an App's sessions live (R31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStore {
    Memory,
    Sealed,
    None,
    Unknown,
}

/// Templates whose releases before a version kept sessions in memory (Sew
/// keeps this current; TPL1 = hearth/meet 0.2.0 seal them into `/data`).
pub const MEMORY_SESSION_TEMPLATES: &[(&str, &str)] = &[("hearth", "0.2.0"), ("meet", "0.2.0")];

fn ver(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
}

/// `host.sessions` first, else the template table by Seed id and version,
/// else unknown. Returns the store and where the answer came from.
pub fn sessions(manifest: Option<super::manifest::Sessions>, seed_id: &str, seed_version: Option<&str>) -> (SessionStore, &'static str) {
    if let Some(s) = manifest {
        return (
            match s {
                super::manifest::Sessions::Memory => SessionStore::Memory,
                super::manifest::Sessions::Sealed => SessionStore::Sealed,
                super::manifest::Sessions::None => SessionStore::None,
            },
            "manifest",
        );
    }
    if let Some((_, sealed_from)) = MEMORY_SESSION_TEMPLATES.iter().find(|(t, _)| *t == seed_id) {
        return match seed_version {
            Some(v) if ver(v) >= ver(sealed_from) => (SessionStore::Sealed, "template"),
            Some(_) => (SessionStore::Memory, "template"),
            None => (SessionStore::Unknown, "template"),
        };
    }
    (SessionStore::Unknown, "unknown")
}

/// `MemAvailable` from `/proc/meminfo`, in bytes.
pub fn mem_available(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|k| k.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

/// R12 (Q15): a second instance fits when the live one's measured memory
/// fits in what is available minus the floor. `Err((need, room))`.
pub fn memory_headroom(live_mem_current: Option<u64>, mem_available: Option<u64>) -> Result<(), (u64, u64)> {
    let (Some(need), Some(avail)) = (live_mem_current, mem_available) else {
        // Unknown on either side: no measured reason to refuse.
        return Ok(());
    };
    let room = avail.saturating_sub(MEM_FLOOR);
    if need <= room {
        Ok(())
    } else {
        Err((need, room))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(k2: &str, eng: Option<&str>) -> Fingerprint {
        Fingerprint { k2_version: k2.into(), engine_version: eng.map(Into::into), ..Default::default() }
    }

    #[test]
    fn due_by_fingerprint() {
        assert_eq!(due(None, "0.46.4", Some("0.46.4")), Some("no_fingerprint"), "NULL = due (A8)");
        assert_eq!(due(Some(&fp("0.46.3", Some("0.46.3"))), "0.46.4", Some("0.46.4")), Some("k2_version"));
        assert_eq!(due(Some(&fp("0.46.4", Some("0.46.3"))), "0.46.4", Some("0.46.4")), Some("engine_version"));
        assert_eq!(due(Some(&fp("0.46.4", Some("0.46.4"))), "0.46.4", Some("0.46.4")), None, "same version: nothing due");
        assert_eq!(due(Some(&fp("0.46.4", None)), "0.46.4", Some("0.46.4")), None, "no engine (unwalled)");
        let back = Fingerprint::parse(Some(&serde_json::to_string(&fp("0.46.4", Some("0.46.4"))).unwrap())).unwrap();
        assert_eq!(back.engine_version.as_deref(), Some("0.46.4"));
        assert_eq!(Fingerprint::parse(Some("{bad")), None);
    }

    #[test]
    fn windows_and_policies() {
        assert_eq!(parse_window("02:00-05:30"), Some((120, 330)));
        for bad in ["2:00-05:00", "25:00-01:00", "02:00", "02:00-02:00", "aa:bb-01:00"] {
            assert_eq!(parse_window(bad), None, "{bad}");
        }
        assert!(in_window((120, 330), 200) && !in_window((120, 330), 330));
        assert!(in_window((1320, 120), 1400) && in_window((1320, 120), 60) && !in_window((1320, 120), 600), "wraps midnight");
        assert_eq!(policy_gate(POLICY_AUTO, None, 0), PolicyGate::Go);
        assert_eq!(policy_gate(POLICY_HANDOVER, None, 0), PolicyGate::Go);
        assert_eq!(policy_gate(POLICY_MANUAL, None, 0), PolicyGate::Wait(defer::POLICY_MANUAL));
        assert_eq!(policy_gate(POLICY_OFF, None, 0), PolicyGate::Wait(defer::POLICY_OFF));
        assert_eq!(policy_gate(POLICY_WINDOW, Some("02:00-05:00"), 180), PolicyGate::Go);
        assert_eq!(policy_gate(POLICY_WINDOW, Some("02:00-05:00"), 600), PolicyGate::Wait(defer::OUTSIDE_WINDOW));
        assert_eq!(policy_gate(POLICY_WINDOW, None, 180), PolicyGate::Wait(defer::OUTSIDE_WINDOW));
    }

    #[test]
    fn session_detection_manifest_then_template_then_unknown() {
        use super::super::manifest::Sessions as S;
        assert_eq!(sessions(Some(S::Memory), "x", None), (SessionStore::Memory, "manifest"));
        assert_eq!(sessions(Some(S::Sealed), "hearth", Some("0.1.0")), (SessionStore::Sealed, "manifest"), "the manifest wins");
        assert_eq!(sessions(None, "hearth", Some("0.1.9")), (SessionStore::Memory, "template"), "pre-TPL1 Hearth");
        assert_eq!(sessions(None, "hearth", Some("0.2.0")), (SessionStore::Sealed, "template"));
        assert_eq!(sessions(None, "meet", Some("0.10.0")), (SessionStore::Sealed, "template"), "numeric, not text, compare");
        assert_eq!(sessions(None, "client-status", Some("1.0.0")), (SessionStore::Unknown, "unknown"));
    }

    #[test]
    fn measured_headroom() {
        let mi = "MemTotal:       65000000 kB\nMemAvailable:    1048576 kB\n";
        assert_eq!(mem_available(mi), Some(1 << 30));
        assert_eq!(memory_headroom(Some(140 << 20), Some(1 << 30)), Ok(()));
        assert_eq!(memory_headroom(Some(600 << 20), Some(1 << 30)), Err((600 << 20, 512 << 20)), "the 512 MiB floor");
        assert_eq!(memory_headroom(None, Some(1 << 30)), Ok(()));
        assert_eq!(memory_headroom(Some(1), None), Ok(()));
    }
}
