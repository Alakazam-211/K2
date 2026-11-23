//! This box's names, as data from the control plane (multi-zone step A1).
//!
//! The relay-list answer ([`super::relay_list`]) may carry the names the
//! box has in each zone. This module parses them, keeps the last good set
//! in memory and on disk, and exposes it read-only. NOTHING uses the names
//! yet: the first consumer is step A2 (sending the primary name in
//! `X-K2-Subdomain`), and every later use comes with its own sign-off. So
//! with the fields absent, which is every answer today, behaviour is the
//! same as before this module existed.
//!
//! **Wire** (additive under the relay list's `v:1`; unknown fields stay
//! ignored):
//!
//! ```json
//! {"names":[{"zone":"globex.example","fqdn":"alice.globex.example",
//!            "features":["e2e","voice"],"turn":"turn.alice.globex.example"}],
//!  "primaryZone":"globex.example","namesVersion":7,"namesStatus":"ok"}
//! ```
//!
//! - `namesStatus: "ok"`: replace the stored names with this list.
//! - `namesStatus: "none"`, with an empty or missing `names` and a newer
//!   `namesVersion`: the box has no names any more ([`BoxNames::Revoked`]).
//!   A1 only records that state; what the daemon does about it is a later
//!   step.
//! - `namesVersion`: a counter (a JSON integer, 0 or more) that never goes
//!   down for a box. A higher one REPLACES what is stored, never merges with
//!   it; the same one is a no-op; a lower one is ignored. It goes back to
//!   the control plane on every fetch as [`HEADER_NAMES_VERSION`], so the
//!   answer can leave the names out when nothing changed.
//! - Anything else keeps what the box has: no `namesStatus`, an empty list
//!   without it, a malformed block, a non-200, a timeout, a 401.
//!
//! **Validation**, per entry: `zone` a lowercase hostname with at least two
//! labels; `fqdn` a lowercase hostname strictly under `zone` (never the
//! zone itself, never a wildcard: no field allows one); `turn`, when
//! present, the same rule; `features` a list of short lowercase words
//! (unknown words are kept for later steps); one entry per zone. A bad
//! entry is dropped with a log line and the rest kept. The whole block is
//! refused (and the stored names kept) only when it is malformed: no usable
//! version, `names` not a list, more than [`MAX_NAMES`] entries, no valid
//! entry left, or `primaryZone` not one of the kept zones.
//!
//! **Store**: `tunnel-names.json` in the K2 home, 0600, written atomically,
//! holding the same wire block plus the fingerprint of the tunnel token it
//! was fetched with. It is read back through the same validator, and only
//! for the same token: a re-paired box (a new token) starts empty, and its
//! first answer replaces the store whatever its version. The stored names
//! are used only until the relay-list endpoint answers again.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::TunnelConfig;

/// Request header carrying the stored `namesVersion` back on each
/// relay-list fetch. Absent when the box holds no names.
pub const HEADER_NAMES_VERSION: &str = "X-K2-Names-Version";

/// Most entries accepted in one answer.
pub const MAX_NAMES: usize = 32;

/// Most feature words kept per entry.
const MAX_FEATURES: usize = 16;

/// The store file's own format version.
const STORE_V: u32 = 1;

/// One name of this box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameEntry {
    /// The zone apex, lowercase (`globex.example`).
    pub zone: String,
    /// The box's name in that zone, lowercase (`alice.globex.example`).
    pub fqdn: String,
    /// The gates the control plane says the daemon may offer in this zone
    /// (`e2e`, `voice`, `nested`, `apps_edge`, `mail`, and any later word).
    /// A word that is absent means "don't offer it".
    pub features: Vec<String>,
    /// The voice (TURN) name in this zone, when the zone has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
}

/// A validated list of this box's names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// `namesVersion`.
    pub version: u64,
    /// `primaryZone`; always the zone of one entry.
    pub primary_zone: String,
    /// The entries, in the order received.
    pub entries: Vec<NameEntry>,
}

impl Names {
    /// The entry of the primary zone.
    pub fn primary(&self) -> &NameEntry {
        self.entries
            .iter()
            .find(|e| e.zone == self.primary_zone)
            .expect("a validated list holds its primary zone")
    }
}

/// What the box knows about its names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxNames {
    /// `namesStatus: "ok"`: these are the box's names.
    Active(Names),
    /// `namesStatus: "none"`: the control plane says the box has no names
    /// any more, as of this version.
    Revoked {
        /// The `namesVersion` that said so.
        version: u64,
    },
}

impl BoxNames {
    /// The `namesVersion` behind this state.
    pub fn version(&self) -> u64 {
        match self {
            BoxNames::Active(n) => n.version,
            BoxNames::Revoked { version } => *version,
        }
    }
}

/// What one relay-list answer said about the names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamesAnswer {
    /// No names block (every server today, or "unchanged"): keep.
    Absent,
    /// A names block that can't be used: keep, and log why.
    Malformed(String),
    /// A usable answer, with a line for each entry that was dropped.
    Usable {
        /// The new state.
        names: BoxNames,
        /// Why each dropped entry was dropped.
        dropped: Vec<String>,
    },
}

/// The names fields as they arrive. Loose types on purpose: a bad names
/// block is reported here and never fails the relay list around it.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct WireNames {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) names: Option<Value>,
    #[serde(
        default,
        rename = "primaryZone",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) primary_zone: Option<Value>,
    #[serde(
        default,
        rename = "namesVersion",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) names_version: Option<Value>,
    #[serde(
        default,
        rename = "namesStatus",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) names_status: Option<Value>,
}

// ── Parse + validate ──────────────────────────────────────────────────

/// A lowercase DNS hostname (no IP literal, no wildcard).
fn lower_hostname(s: &str) -> bool {
    !s.bytes().any(|b| b.is_ascii_uppercase())
        && s.parse::<std::net::IpAddr>().is_err()
        && super::relay_list::valid_host(s)
}

/// `name` is a lowercase hostname strictly under `zone`.
fn under_zone(name: &str, zone: &str) -> bool {
    lower_hostname(name)
        && name.len() > zone.len() + 1
        && name.ends_with(zone)
        && name.as_bytes()[name.len() - zone.len() - 1] == b'.'
}

fn feature_word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn parse_entry(v: &Value) -> Result<NameEntry, String> {
    let obj = v.as_object().ok_or("not an object")?;
    let text = |k: &str| obj.get(k).and_then(Value::as_str);
    let zone = text("zone").ok_or("no zone")?;
    if !lower_hostname(zone) || !zone.contains('.') {
        return Err(format!(
            "zone {zone:?} is not a lowercase hostname with two labels"
        ));
    }
    let fqdn = text("fqdn").ok_or("no fqdn")?;
    if !under_zone(fqdn, zone) {
        return Err(format!(
            "fqdn {fqdn:?} is not a lowercase hostname under {zone}"
        ));
    }
    let turn = match obj.get("turn") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) if under_zone(t, zone) => Some(t.clone()),
        Some(t) => return Err(format!("turn {t} is not a lowercase hostname under {zone}")),
    };
    let mut features: Vec<String> = Vec::new();
    match obj.get("features") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for f in list {
                match f.as_str() {
                    Some(w) if feature_word(w) => {
                        if !features.iter().any(|x| x == w) {
                            features.push(w.to_string());
                        }
                    }
                    _ => return Err(format!("feature {f} is not a short lowercase word")),
                }
            }
            if features.len() > MAX_FEATURES {
                return Err(format!(
                    "{} features (at most {MAX_FEATURES})",
                    features.len()
                ));
            }
        }
        Some(_) => return Err("features is not a list".to_string()),
    }
    Ok(NameEntry {
        zone: zone.to_string(),
        fqdn: fqdn.to_string(),
        features,
        turn,
    })
}

/// Read the names fields of one answer (or of the store).
pub(crate) fn parse(w: &WireNames) -> NamesAnswer {
    let list = match &w.names {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a.as_slice()),
        Some(_) => return NamesAnswer::Malformed("names is not a list".to_string()),
    };
    let has_entries = list.is_some_and(|a| !a.is_empty());
    let status = match &w.names_status {
        None | Some(Value::Null) if !has_entries => return NamesAnswer::Absent,
        None | Some(Value::Null) => {
            return NamesAnswer::Malformed("names without namesStatus".to_string())
        }
        Some(Value::String(s)) => s.as_str(),
        Some(other) => {
            return NamesAnswer::Malformed(format!("namesStatus {other} is not a string"))
        }
    };
    let Some(version) = w.names_version.as_ref().and_then(Value::as_u64) else {
        return NamesAnswer::Malformed("namesVersion is missing or not an integer".to_string());
    };
    match status {
        "none" if has_entries => {
            NamesAnswer::Malformed("namesStatus \"none\" with a non-empty list".to_string())
        }
        "none" => NamesAnswer::Usable {
            names: BoxNames::Revoked { version },
            dropped: Vec::new(),
        },
        "ok" => {
            let list = list.unwrap_or_default();
            if list.is_empty() {
                return NamesAnswer::Malformed("namesStatus \"ok\" with no names".to_string());
            }
            if list.len() > MAX_NAMES {
                return NamesAnswer::Malformed(format!(
                    "{} names (at most {MAX_NAMES})",
                    list.len()
                ));
            }
            let mut entries: Vec<NameEntry> = Vec::new();
            let mut dropped = Vec::new();
            for (i, v) in list.iter().enumerate() {
                match parse_entry(v) {
                    Ok(e) if entries.iter().any(|k| k.zone == e.zone) => {
                        dropped.push(format!("name #{i}: a second entry for zone {}", e.zone))
                    }
                    Ok(e) => entries.push(e),
                    Err(why) => dropped.push(format!("name #{i}: {why}")),
                }
            }
            if entries.is_empty() {
                return NamesAnswer::Malformed(format!(
                    "no valid name left ({})",
                    dropped.join("; ")
                ));
            }
            let primary = w.names_primary_zone();
            let Some(primary_zone) = primary.filter(|p| entries.iter().any(|e| e.zone == *p))
            else {
                return NamesAnswer::Malformed(format!(
                    "primaryZone {primary:?} is not one of the valid names' zones"
                ));
            };
            NamesAnswer::Usable {
                names: BoxNames::Active(Names {
                    version,
                    primary_zone: primary_zone.to_string(),
                    entries,
                }),
                dropped,
            }
        }
        other => NamesAnswer::Malformed(format!("unknown namesStatus {other:?}")),
    }
}

impl WireNames {
    fn names_primary_zone(&self) -> Option<&str> {
        self.primary_zone.as_ref().and_then(Value::as_str)
    }

    /// The wire block for `names` (what the store holds).
    fn from_state(names: &BoxNames) -> Self {
        match names {
            BoxNames::Active(n) => WireNames {
                names: Some(serde_json::to_value(&n.entries).expect("entries serialize")),
                primary_zone: Some(Value::String(n.primary_zone.clone())),
                names_version: Some(Value::from(n.version)),
                names_status: Some(Value::String("ok".to_string())),
            },
            BoxNames::Revoked { version } => WireNames {
                names: Some(Value::Array(Vec::new())),
                primary_zone: None,
                names_version: Some(Value::from(*version)),
                names_status: Some(Value::String("none".to_string())),
            },
        }
    }
}

// ── State + store ─────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct State {
    /// The token fingerprint the state below belongs to, once the store
    /// has been read for it. `Some(None)` = read for an unpaired box.
    loaded_for: Option<Option<String>>,
    /// The names in use.
    held: Option<BoxNames>,
    /// The last problem logged (log-once).
    logged: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    loaded_for: None,
    held: None,
    logged: None,
});

fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The store file.
pub fn store_path() -> PathBuf {
    crate::paths::k2_home().join("tunnel-names.json")
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    v: u32,
    /// Fingerprint of the tunnel token the names were fetched with.
    owner: String,
    #[serde(flatten)]
    names: WireNames,
}

fn owner_of(cfg: &TunnelConfig) -> Option<String> {
    let t = cfg.token.trim();
    (!t.is_empty()).then(|| super::unpair::token_fingerprint(t))
}

fn log_once(st: &mut State, msg: String) {
    if st.logged.as_deref() != Some(msg.as_str()) {
        crate::log_debug!("[tunnel/names] {msg}");
        st.logged = Some(msg);
    }
}

/// The stored names for `owner`, or `None` (no file, another token, or a
/// file that does not validate, which is logged).
fn read_store(st: &mut State, owner: &str) -> Option<BoxNames> {
    let path = store_path();
    let raw = match fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log_once(st, format!("stored names unreadable, ignored: {e}"));
            return None;
        }
    };
    let file: StoreFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(e) => {
            log_once(st, format!("stored names malformed, ignored: {e}"));
            return None;
        }
    };
    if file.v != STORE_V || file.owner != owner {
        return None;
    }
    match parse(&file.names) {
        NamesAnswer::Usable { names, .. } => Some(names),
        NamesAnswer::Absent => None,
        NamesAnswer::Malformed(why) => {
            log_once(st, format!("stored names invalid, ignored: {why}"));
            None
        }
    }
}

fn write_store(owner: &str, names: &BoxNames) -> Result<(), String> {
    let path = store_path();
    let dir = path.parent().ok_or("store path has no folder")?;
    fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let body = serde_json::to_string_pretty(&StoreFile {
        v: STORE_V,
        owner: owner.to_string(),
        names: WireNames::from_state(names),
    })
    .map_err(|e| format!("serialize names: {e}"))?;
    let tmp = dir.join(format!("tunnel-names.json.tmp.{}", std::process::id()));
    fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", path.display())
    })
}

/// Make the in-memory state belong to `cfg`'s token, reading the store
/// once per token. An unreadable token changes nothing.
fn sync_locked(st: &mut State, cfg: &TunnelConfig) {
    if cfg.token_unreadable() {
        return;
    }
    let owner = owner_of(cfg);
    if st.loaded_for.as_ref() == Some(&owner) {
        return;
    }
    let stored = owner.as_deref().and_then(|o| read_store(st, o));
    st.held = stored;
    st.loaded_for = Some(owner);
}

/// Load the stored names for `cfg`'s token, if not loaded yet (the relay
/// refresh calls it before each fetch, so the version goes out with it).
pub(crate) fn sync(cfg: &TunnelConfig) {
    sync_locked(&mut lock(), cfg);
}

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Nothing usable in the answer, or no token: what the box had stays.
    Kept,
    /// The same `namesVersion` as stored: nothing changed.
    Unchanged,
    /// An older `namesVersion` than stored: ignored.
    Older,
    /// Stored names replaced (the store write may have failed; logged).
    Replaced,
}

/// Apply one relay-list answer's names for `cfg`'s token.
pub(crate) fn apply(answer: &NamesAnswer, cfg: &TunnelConfig) -> ApplyOutcome {
    let mut st = lock();
    sync_locked(&mut st, cfg);
    let Some(Some(owner)) = st.loaded_for.clone() else {
        return ApplyOutcome::Kept;
    };
    let (names, dropped) = match answer {
        NamesAnswer::Absent => return ApplyOutcome::Kept,
        NamesAnswer::Malformed(why) => {
            log_once(
                &mut st,
                format!("names in the relay list not used, keeping the old ones: {why}"),
            );
            return ApplyOutcome::Kept;
        }
        NamesAnswer::Usable { names, dropped } => (names, dropped),
    };
    if let Some(held) = st.held.as_ref().map(BoxNames::version) {
        if names.version() == held {
            return ApplyOutcome::Unchanged;
        }
        if names.version() < held {
            let msg = format!(
                "names version {} is older than the stored {held}, ignored",
                names.version()
            );
            log_once(&mut st, msg);
            return ApplyOutcome::Older;
        }
    }
    for d in dropped {
        crate::log_debug!("[tunnel/names] dropped {d}");
    }
    match names {
        BoxNames::Active(n) => crate::log_debug!(
            "[tunnel/names] names v{} from the relay list: {} (primary {})",
            n.version,
            n.entries
                .iter()
                .map(|e| e.fqdn.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            n.primary_zone
        ),
        BoxNames::Revoked { version } => crate::log_debug!(
            "[tunnel/names] names v{version}: the control plane says this box has no names"
        ),
    }
    if let Err(e) = write_store(&owner, names) {
        crate::log_debug!("[tunnel/names] WARN could not save the names, kept in memory: {e}");
    }
    st.held = Some(names.clone());
    st.logged = None;
    ApplyOutcome::Replaced
}

// ── Read-only accessors ───────────────────────────────────────────────

/// This box's active names, or `None` (none learned, or revoked). Memory
/// only, never I/O. Nothing calls it yet: step A2 (the primary name in
/// `X-K2-Subdomain`) is the first consumer.
pub fn current() -> Option<Names> {
    match &lock().held {
        Some(BoxNames::Active(n)) => Some(n.clone()),
        _ => None,
    }
}

/// The full state, revoked included: `None` = no names learned. Memory
/// only. Nothing acts on [`BoxNames::Revoked`] yet; that is a later step.
pub fn state() -> Option<BoxNames> {
    lock().held.clone()
}

/// The stored `namesVersion` ([`HEADER_NAMES_VERSION`]), memory only.
pub fn version() -> Option<u64> {
    lock().held.as_ref().map(BoxNames::version)
}

/// Forget the in-memory state, as at a daemon start (tests).
#[cfg(test)]
pub(crate) fn reset_for_test() {
    *lock() = State::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    const ZONE: &str = "globex.example";

    fn wire(body: &str) -> WireNames {
        serde_json::from_str(body).expect("test body is JSON")
    }

    fn entry(zone: &str, fqdn: &str) -> String {
        format!(r#"{{"zone":"{zone}","fqdn":"{fqdn}","features":["e2e"]}}"#)
    }

    fn ok_body(version: u64, primary: &str, entries: &[String]) -> String {
        format!(
            r#"{{"names":[{}],"primaryZone":"{primary}","namesVersion":{version},"namesStatus":"ok"}}"#,
            entries.join(",")
        )
    }

    fn full() -> String {
        format!(
            r#"{{"names":[
                {{"zone":"{ZONE}","fqdn":"alice.{ZONE}","features":["e2e","voice","nested","apps_edge","mail"],"turn":"turn.alice.{ZONE}"}},
                {{"zone":"partner.example","fqdn":"alice.partner.example","features":[]}}
              ],"primaryZone":"partner.example","namesVersion":7,"namesStatus":"ok"}}"#
        )
    }

    fn active(a: NamesAnswer) -> (Names, Vec<String>) {
        match a {
            NamesAnswer::Usable {
                names: BoxNames::Active(n),
                dropped,
            } => (n, dropped),
            other => panic!("expected an active list, got {other:?}"),
        }
    }

    fn malformed(a: NamesAnswer) -> String {
        match a {
            NamesAnswer::Malformed(why) => why,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    fn cfg(token: &str) -> TunnelConfig {
        TunnelConfig {
            token: token.to_string(),
            ..Default::default()
        }
    }

    // ── parse ──

    #[test]
    fn a_full_list_parses_in_order() {
        let (n, dropped) = active(parse(&wire(&full())));
        assert!(dropped.is_empty(), "{dropped:?}");
        assert_eq!(n.version, 7);
        assert_eq!(n.primary_zone, "partner.example");
        assert_eq!(
            n.entries,
            vec![
                NameEntry {
                    zone: ZONE.to_string(),
                    fqdn: format!("alice.{ZONE}"),
                    features: ["e2e", "voice", "nested", "apps_edge", "mail"]
                        .map(String::from)
                        .to_vec(),
                    turn: Some(format!("turn.alice.{ZONE}")),
                },
                NameEntry {
                    zone: "partner.example".to_string(),
                    fqdn: "alice.partner.example".to_string(),
                    features: vec![],
                    turn: None,
                },
            ]
        );
        assert_eq!(n.primary().fqdn, "alice.partner.example");
    }

    #[test]
    fn absent_fields_are_absent() {
        assert_eq!(parse(&wire("{}")), NamesAnswer::Absent);
        // "Unchanged" as the server may say it: only the version.
        assert_eq!(parse(&wire(r#"{"namesVersion":7}"#)), NamesAnswer::Absent);
        // An empty list without namesStatus keeps what the box has.
        assert_eq!(
            parse(&wire(r#"{"names":[],"namesVersion":8}"#)),
            NamesAnswer::Absent
        );
        assert_eq!(parse(&wire(r#"{"names":null}"#)), NamesAnswer::Absent);
    }

    #[test]
    fn invalid_entries_are_dropped_and_the_rest_kept() {
        let bad = [
            entry(ZONE, "Alice.globex.example"),   // not lowercase
            entry(ZONE, "alice.other.example"),    // not under its zone
            entry(ZONE, "xalice.globex.examplex"), // suffix, not a label
            entry(ZONE, ZONE),                     // the zone itself
            entry(ZONE, "*.alice.globex.example"), // a wildcard
            entry(ZONE, "*.globex.example"),       // a zone-wide wildcard
            entry("example", "alice.example"),     // a one-label zone
            entry("10.0.0.0", "1.10.0.0.0"),       // IP literals
            entry("Globex.example", "alice.Globex.example"),
            r#"{"zone":"globex.example","fqdn":"bob.globex.example","turn":"turn.other.example"}"#
                .to_string(),
            r#"{"zone":"globex.example","fqdn":"bob.globex.example","features":"e2e"}"#.to_string(),
            r#"{"zone":"globex.example","fqdn":"bob.globex.example","features":["E2E"]}"#
                .to_string(),
            r#"{"fqdn":"bob.globex.example"}"#.to_string(),
            r#""bob.globex.example""#.to_string(),
        ];
        let mut list: Vec<String> = bad.to_vec();
        list.push(entry("partner.example", "alice.partner.example"));
        list.push(entry("partner.example", "bob.partner.example")); // second entry, same zone
        let (n, dropped) = active(parse(&wire(&ok_body(3, "partner.example", &list))));
        assert_eq!(n.entries.len(), 1, "{n:?}");
        assert_eq!(n.entries[0].fqdn, "alice.partner.example");
        assert_eq!(
            dropped.len(),
            bad.len() + 1,
            "one line per dropped entry: {dropped:?}"
        );
        assert!(
            dropped.last().expect("dropped").contains("second entry"),
            "{dropped:?}"
        );
    }

    #[test]
    fn a_malformed_block_is_refused_whole() {
        let one = [entry(ZONE, &format!("alice.{ZONE}"))];
        assert!(malformed(parse(&wire(
            r#"{"names":"x","namesStatus":"ok","namesVersion":1}"#
        )))
        .contains("not a list"));
        let no_status = format!(
            r#"{{"names":[{}],"primaryZone":"{ZONE}","namesVersion":1}}"#,
            one[0]
        );
        assert!(malformed(parse(&wire(&no_status))).contains("without namesStatus"));
        for v in [r#""7""#, "-1", "1.5", "null"] {
            let body = ok_body(1, ZONE, &one)
                .replace(r#""namesVersion":1"#, &format!(r#""namesVersion":{v}"#));
            assert!(
                malformed(parse(&wire(&body))).contains("namesVersion"),
                "{v}"
            );
        }
        let status = ok_body(1, ZONE, &one).replace(r#""ok""#, r#""maybe""#);
        assert!(malformed(parse(&wire(&status))).contains("unknown namesStatus"));
        assert!(
            malformed(parse(&wire(&ok_body(1, "partner.example", &one)))).contains("primaryZone")
        );
        let no_primary = ok_body(1, ZONE, &one).replace(r#""primaryZone":"globex.example","#, "");
        assert!(malformed(parse(&wire(&no_primary))).contains("primaryZone"));
        // The primary's own entry was invalid: no usable primary.
        let list = [
            entry(ZONE, "ALICE.globex.example"),
            entry("partner.example", "alice.partner.example"),
        ];
        assert!(malformed(parse(&wire(&ok_body(1, ZONE, &list)))).contains("primaryZone"));
        assert!(
            malformed(parse(&wire(&ok_body(1, ZONE, &[entry(ZONE, "x")]))))
                .contains("no valid name")
        );
        let many: Vec<String> = (0..=MAX_NAMES)
            .map(|i| entry(&format!("z{i}.example"), &format!("a.z{i}.example")))
            .collect();
        assert!(malformed(parse(&wire(&ok_body(1, "z0.example", &many)))).contains("at most"));
        let ok_empty =
            r#"{"names":[],"primaryZone":"globex.example","namesVersion":2,"namesStatus":"ok"}"#;
        assert!(malformed(parse(&wire(ok_empty))).contains("no names"));
    }

    #[test]
    fn names_status_none_is_revoked() {
        for body in [
            r#"{"names":[],"namesVersion":9,"namesStatus":"none"}"#,
            r#"{"namesVersion":9,"namesStatus":"none"}"#,
        ] {
            assert_eq!(
                parse(&wire(body)),
                NamesAnswer::Usable {
                    names: BoxNames::Revoked { version: 9 },
                    dropped: vec![]
                },
                "{body}"
            );
        }
        let with_list = format!(
            r#"{{"names":[{}],"namesVersion":9,"namesStatus":"none"}}"#,
            entry(ZONE, &format!("alice.{ZONE}"))
        );
        assert!(malformed(parse(&wire(&with_list))).contains("non-empty"));
        assert!(
            malformed(parse(&wire(r#"{"names":[],"namesStatus":"none"}"#)))
                .contains("namesVersion")
        );
    }

    // ── apply + store ──

    fn v(version: u64) -> NamesAnswer {
        parse(&wire(&ok_body(
            version,
            ZONE,
            &[entry(ZONE, &format!("alice{version}.{ZONE}"))],
        )))
    }

    fn fqdns() -> Vec<String> {
        current()
            .expect("active names")
            .entries
            .into_iter()
            .map(|e| e.fqdn)
            .collect()
    }

    #[test]
    fn absent_or_malformed_answers_leave_the_state_and_the_store_untouched() {
        with_temp_home(|| {
            reset_for_test();
            let c = cfg("k2c_good");
            assert_eq!(apply(&v(3), &c), ApplyOutcome::Replaced);
            let before = fs::read(store_path()).expect("store written");
            let held = state();
            for a in [
                NamesAnswer::Absent,
                parse(&wire(r#"{"names":[],"namesVersion":8}"#)),
                parse(&wire(
                    r#"{"names":"x","namesStatus":"ok","namesVersion":8}"#,
                )),
            ] {
                assert_eq!(apply(&a, &c), ApplyOutcome::Kept, "{a:?}");
                assert_eq!(state(), held);
                assert_eq!(fs::read(store_path()).expect("store"), before);
            }
        });
    }

    #[test]
    fn absent_fields_on_a_fresh_box_write_nothing() {
        with_temp_home(|| {
            reset_for_test();
            assert_eq!(
                apply(&NamesAnswer::Absent, &cfg("k2c_good")),
                ApplyOutcome::Kept
            );
            assert_eq!(state(), None);
            assert_eq!(version(), None);
            assert!(!store_path().exists(), "no names, no file");
        });
    }

    #[test]
    fn a_newer_version_replaces_and_never_merges() {
        with_temp_home(|| {
            reset_for_test();
            let c = cfg("k2c_good");
            let two = [
                entry(ZONE, &format!("alice.{ZONE}")),
                entry("partner.example", "alice.partner.example"),
            ];
            assert_eq!(
                apply(&parse(&wire(&ok_body(3, ZONE, &two))), &c),
                ApplyOutcome::Replaced
            );
            assert_eq!(fqdns().len(), 2);
            let one = [entry("partner.example", "bob.partner.example")];
            assert_eq!(
                apply(&parse(&wire(&ok_body(4, "partner.example", &one))), &c),
                ApplyOutcome::Replaced
            );
            assert_eq!(
                fqdns(),
                vec!["bob.partner.example"],
                "the new list only, nothing kept from v3"
            );
            assert_eq!(version(), Some(4));
        });
    }

    #[test]
    fn the_same_version_is_a_no_op_and_an_older_one_is_ignored() {
        with_temp_home(|| {
            reset_for_test();
            let c = cfg("k2c_good");
            assert_eq!(apply(&v(5), &c), ApplyOutcome::Replaced);
            let before = fs::read(store_path()).expect("store");
            let same_version_other_list = parse(&wire(&ok_body(
                5,
                ZONE,
                &[entry(ZONE, &format!("carol.{ZONE}"))],
            )));
            assert_eq!(apply(&same_version_other_list, &c), ApplyOutcome::Unchanged);
            assert_eq!(apply(&v(4), &c), ApplyOutcome::Older);
            assert_eq!(fqdns(), vec![format!("alice5.{ZONE}")]);
            assert_eq!(fs::read(store_path()).expect("store"), before);
        });
    }

    #[test]
    fn none_revokes_the_names_and_a_later_list_brings_them_back() {
        with_temp_home(|| {
            reset_for_test();
            let c = cfg("k2c_good");
            assert_eq!(apply(&v(5), &c), ApplyOutcome::Replaced);
            let none = |ver: u64| {
                parse(&wire(&format!(
                    r#"{{"names":[],"namesVersion":{ver},"namesStatus":"none"}}"#
                )))
            };
            assert_eq!(
                apply(&none(5), &c),
                ApplyOutcome::Unchanged,
                "none needs a bumped version"
            );
            assert!(current().is_some());
            assert_eq!(apply(&none(6), &c), ApplyOutcome::Replaced);
            assert_eq!(current(), None, "revoked: no active names");
            assert_eq!(state(), Some(BoxNames::Revoked { version: 6 }));
            assert_eq!(
                version(),
                Some(6),
                "the version still goes back on the fetch"
            );
            reset_for_test();
            sync(&c);
            assert_eq!(
                state(),
                Some(BoxNames::Revoked { version: 6 }),
                "revoked survives a restart"
            );
            assert_eq!(apply(&v(7), &c), ApplyOutcome::Replaced);
            assert_eq!(fqdns(), vec![format!("alice7.{ZONE}")]);
        });
    }

    #[test]
    fn the_store_survives_a_restart() {
        with_temp_home(|| {
            reset_for_test();
            let c = cfg("k2c_good");
            assert_eq!(apply(&parse(&wire(&full())), &c), ApplyOutcome::Replaced);
            let held = state().expect("held");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(store_path())
                    .expect("stat")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600);
            }
            let raw = fs::read_to_string(store_path()).expect("read store");
            assert!(
                !raw.contains("k2c_good"),
                "the token itself is never stored: {raw}"
            );
            reset_for_test(); // a daemon restart
            assert_eq!(state(), None, "memory starts empty");
            sync(&c);
            assert_eq!(state(), Some(held));
            assert_eq!(version(), Some(7));
        });
    }

    #[test]
    fn a_new_token_starts_empty_and_its_first_list_replaces_whatever_the_version() {
        with_temp_home(|| {
            reset_for_test();
            assert_eq!(apply(&v(9), &cfg("k2c_old")), ApplyOutcome::Replaced);
            reset_for_test();
            sync(&cfg("k2c_new"));
            assert_eq!(
                state(),
                None,
                "another token's store is not this box's names"
            );
            assert_eq!(apply(&v(2), &cfg("k2c_new")), ApplyOutcome::Replaced);
            assert_eq!(version(), Some(2));
            // Unpaired: nothing held, nothing applied.
            sync(&cfg(""));
            assert_eq!(state(), None);
            assert_eq!(apply(&v(10), &cfg("")), ApplyOutcome::Kept);
        });
    }

    #[test]
    fn a_hand_edited_store_goes_through_the_same_checks() {
        with_temp_home(|| {
            reset_for_test();
            let c = cfg("k2c_good");
            assert_eq!(apply(&v(3), &c), ApplyOutcome::Replaced);
            let raw = fs::read_to_string(store_path()).expect("read");
            fs::write(store_path(), raw.replace("alice3.", "ALICE3.")).expect("edit");
            reset_for_test();
            sync(&c);
            assert_eq!(state(), None, "an invalid store is ignored");
            fs::write(store_path(), "not json").expect("edit");
            reset_for_test();
            sync(&c);
            assert_eq!(state(), None);
        });
    }
}
