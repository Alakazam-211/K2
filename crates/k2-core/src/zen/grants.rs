//! Custom-widget grants (prd-zen-user-widgets-v2 UW24, UWA8, UWB3–UWB9).
//!
//! **Day-0 interface (Zen v2, 2026-10-08). Owner: B2.** The types,
//! [`effective`], [`canonical`] and the JSON shapes are frozen for the
//! build: B4's renderer reads [`GrantView`] (as `grant` on a custom widget in
//! `GET /cli/zen/get`) and posts [`Scope`] / [`GrantEntry`] in
//! `POST /cli/zen/widget/grant`. The TypeScript mirror is
//! `src/renderer/lib/zen/zen-custom-types.ts`.
//!
//! **Where grants live (UWB4, §13.5).** In the daemon's SQLite database,
//! table `zen_widget_grants` (migration **0138**, B2), one live row per
//! `(garden, placement)`. Each row is signed by the daemon with
//! HMAC-SHA256 under a 32-byte key in `~/.k2/zen-grant.key` (mode 0600,
//! made on the first grant). The daemon verifies the signature every time
//! it resolves a page ([`effective`]); a bad or missing signature, or a
//! missing or changed key, resolves as [`GrantState::Invalid`] with no caps,
//! and the renderer shows K2's review card. `grants.json` stays refused as a
//! target (`ZenFile::parse`), the shell seal commands of UW21 are gone, and
//! the OS keyring is not used in v2.
//!
//! **Who may write a grant (UWB3, UWB6).** Only the owner token, never a
//! passport, Connect login, `k2sk_` key or `k2skn_` app pass (403
//! `owner_only`). Taking power away (revoke, sending off) is always allowed
//! to anything Zen accepts. An agent that reads the disk owner token on
//! purpose still passes, as for every owner-only route until session-token
//! Phase 2 (R1, parked into the vault research).
//!
//! **What a grant covers (UW24, Q2, §13.2).** Code changes keep the grant.
//! Three things send it back: the manifest asks for a cap the grant lacks
//! (`partial`: the granted caps keep working), the placement's `home` /
//! `agent` text changes (`review`), or the folder is deleted (the row is
//! ignored and pruned on the next write).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::USER_WIDGET_CAPS;

/// First line of [`canonical`]. Bump the `/vN` when the signed content
/// changes; old rows then resolve `invalid` and go back to review.
pub const GRANT_CANON_TAG: &str = "k2-zen-grant/v2";

/// The key file under `~/.k2/` (UWB4). Agents never write it; it joins the
/// skill's "never write" list (UWB26).
pub const GRANT_KEY_FILE: &str = "zen-grant.key";

/// The migration that creates `zen_widget_grants` (UWB5). Reserved for Zen
/// v2; B2 writes `crates/k2-core/drizzle_sql/0138_zen_widget_grants.sql`.
pub const GRANTS_MIGRATION: &str = "0138_zen_widget_grants";

// ── Scope (UWB7) ─────────────────────────────────────────────────────────

/// What a widget may see, picked by the owner in the review dialog and
/// stored on the grant. The **renderer** resolves it to rows at Allow and
/// at every mount (Homes are device-local, `stores/homes.ts`); the daemon
/// stores and signs it but can't check it against Homes.
///
/// Wire JSON is an object with exactly one key:
/// `{"agent": "<handle::host>"}` · `{"home": "<homeId>"}` ·
/// `{"homes": ["<homeId>", …]}` · `{"allHomes": true}` ·
/// `{"server": "<hostKey>"}` · `{"allServers": true}`.
///
/// Bound rows are the union of the scope's rows **now** (QA1 "at once").
/// A scope whose Home is gone resolves to fewer rows, never to the window's
/// Home or a loose view (UW48).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "J", into = "J")]
pub enum Scope {
    /// One agent, by `handle::host` address.
    Agent(String),
    /// One Home, by id.
    Home(String),
    /// Several Homes, by id (1 or more, no repeats; order kept as picked).
    Homes(Vec<String>),
    /// Every Home on this device.
    AllHomes,
    /// Every agent the desktop can see on one server, by host key.
    Server(String),
    /// The local server plus every saved Connect host (R7: at most 8 live
    /// servers per widget, a second confirm line with sending).
    AllServers,
}

impl Scope {
    /// The wire JSON (one key).
    pub fn to_json(&self) -> J {
        match self {
            Scope::Agent(a) => json!({ "agent": a }),
            Scope::Home(h) => json!({ "home": h }),
            Scope::Homes(hs) => json!({ "homes": hs }),
            Scope::AllHomes => json!({ "allHomes": true }),
            Scope::Server(s) => json!({ "server": s }),
            Scope::AllServers => json!({ "allServers": true }),
        }
    }

    /// Parse the wire JSON. Exactly one known key; strings non-empty;
    /// `homes` non-empty with no repeats; `allHomes` / `allServers` must be
    /// `true`.
    pub fn from_json(v: &J) -> Result<Self, String> {
        let obj = v.as_object().ok_or("scope must be an object")?;
        if obj.len() != 1 {
            return Err("scope has exactly one key: agent, home, homes, allHomes, server or allServers".into());
        }
        let (k, val) = obj.iter().next().ok_or("scope is empty")?;
        let text = |what: &str| -> Result<String, String> {
            match val.as_str().map(str::trim) {
                Some(s) if !s.is_empty() => Ok(s.to_string()),
                _ => Err(format!("scope.{what} must be a non-empty string")),
            }
        };
        let yes = |what: &str| -> Result<(), String> {
            if val == &J::Bool(true) {
                Ok(())
            } else {
                Err(format!("scope.{what} must be true"))
            }
        };
        match k.as_str() {
            "agent" => Ok(Scope::Agent(text("agent")?)),
            "home" => Ok(Scope::Home(text("home")?)),
            "server" => Ok(Scope::Server(text("server")?)),
            "allHomes" => yes("allHomes").map(|()| Scope::AllHomes),
            "allServers" => yes("allServers").map(|()| Scope::AllServers),
            "homes" => {
                let arr = val.as_array().ok_or("scope.homes must be a list of Home ids")?;
                let mut out: Vec<String> = Vec::with_capacity(arr.len());
                for h in arr {
                    let h = h.as_str().map(str::trim).filter(|s| !s.is_empty());
                    let h = h.ok_or("scope.homes holds non-empty Home ids")?;
                    if out.iter().any(|o| o == h) {
                        return Err(format!("scope.homes names '{h}' twice"));
                    }
                    out.push(h.to_string());
                }
                if out.is_empty() {
                    return Err("scope.homes needs at least one Home".into());
                }
                Ok(Scope::Homes(out))
            }
            other => Err(format!(
                "unknown scope '{other}'; use agent, home, homes, allHomes, server or allServers"
            )),
        }
    }

    /// The scope's part of [`canonical`]: a JSON array, so object key order
    /// never matters. `homes` is sorted (order is display only).
    pub fn canonical(&self) -> J {
        match self {
            Scope::Agent(a) => json!(["agent", a]),
            Scope::Home(h) => json!(["home", h]),
            Scope::Homes(hs) => {
                let mut hs = hs.clone();
                hs.sort();
                json!(["homes", hs])
            }
            Scope::AllHomes => json!(["allHomes"]),
            Scope::Server(s) => json!(["server", s]),
            Scope::AllServers => json!(["allServers"]),
        }
    }
}

impl TryFrom<J> for Scope {
    type Error = String;
    fn try_from(v: J) -> Result<Self, String> {
        Scope::from_json(&v)
    }
}

impl From<Scope> for J {
    fn from(s: Scope) -> J {
        s.to_json()
    }
}

/// One bound row resolved at Allow (UWA8): `server` is the host part of the
/// row's `handle::host` address, `room` the handle. A sealed record for
/// About, audit and a later Garden → App export; the live check stays
/// scope-based (QA1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantEntry {
    pub server: String,
    pub room: String,
}

/// What the placement asked to see when it was granted: its `home` and
/// `agent` prop text (UW7). A grant made for a different ask resolves
/// `review` (UW24). Day-0 addition to UWB4's canonical form: the daemon
/// can't map a Home name to an id (Homes are device-local), so it compares
/// the text it granted against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementAsk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

// ── The signed record and its row ───────────────────────────────────────

/// Everything the signature covers. `widget_hash`, `key_id`, the pause and
/// who granted it are row data, not signed: code changes keep the grant
/// (Q2), and a hand-cleared pause can't turn sending back on because
/// `sending` is signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantRecord {
    pub garden: String,
    pub placement: String,
    /// The widget folder name, or a built-in `k2:<name>@<n>` (UWB21).
    pub widget: String,
    /// Granted caps (⊆ requested ∩ [`USER_WIDGET_CAPS`] at grant time).
    pub caps: Vec<String>,
    pub scope: Scope,
    pub entries: Vec<GrantEntry>,
    pub ask: PlacementAsk,
    /// The Sending switch (UWB9). On by default after Allow (R6).
    pub sending: bool,
    /// RFC 3339 UTC.
    pub granted_at: String,
}

/// Why sending was paused (UWB9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PauseReason {
    /// The renderer's runaway guard tripped: more than 120 posts in 10
    /// minutes, or 20 identical texts to one agent in 10 minutes (R6).
    Runaway,
}

/// A runaway pause on the grant. While set, K2 draws its card instead of
/// the frame; `POST widget/resume` (owner only) clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pause {
    pub at: String,
    pub reason: PauseReason,
}

/// One `zen_widget_grants` row as the store reads it (B2, migration 0138).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRow {
    pub record: GrantRecord,
    /// The bundle hash shown in the dialog at Allow (About, audit).
    pub widget_hash: String,
    /// First 8 hex of sha256(key) at signing time.
    pub key_id: String,
    /// Lower-case hex HMAC-SHA256 over [`canonical`]`(record)`.
    pub sig: String,
    pub paused: Option<Pause>,
    /// `owner_token` today; kept for the audit line (UWB3b).
    pub granted_by_kind: String,
}

// ── Signing ──────────────────────────────────────────────────────────────

/// The daemon's grant key (UWB4). 32 random bytes in
/// `~/.k2/` + [`GRANT_KEY_FILE`], mode 0600.
#[derive(Clone)]
pub struct GrantKey {
    bytes: [u8; 32],
}

impl std::fmt::Debug for GrantKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GrantKey({})", self.id())
    }
}

impl GrantKey {
    /// For tests and for the store after reading the file.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// First 8 hex of sha256(key): stored on each row so a changed key is
    /// told apart from a bad signature in the doctor.
    pub fn id(&self) -> String {
        use sha2::{Digest, Sha256};
        hex_lower(&Sha256::digest(self.bytes)[..4])
    }

    /// Read the key, or `None` when the file doesn't exist. B2: refuse a
    /// file that isn't exactly 32 bytes or (on unix) is readable by group or
    /// others.
    pub fn load(path: &std::path::Path) -> std::io::Result<Option<Self>> {
        let _ = path;
        todo!("B2 (UWB4): read ~/.k2/zen-grant.key")
    }

    /// Read the key, making it (32 bytes from the OS RNG, mode 0600,
    /// atomic) on the first grant. Only the grant route calls this; resolve
    /// uses [`GrantKey::load`] and never makes a key.
    pub fn load_or_create(path: &std::path::Path) -> std::io::Result<Self> {
        let _ = path;
        todo!("B2 (UWB4): create ~/.k2/zen-grant.key on the first grant")
    }
}

/// The signed text: [`GRANT_CANON_TAG`], a newline, then one compact JSON
/// array `[garden, placement, widget, caps, scope, ask, entries, sending,
/// grantedAt]`. Caps are sorted and deduplicated, entries sorted as
/// `[server, room]` pairs, `scope` is [`Scope::canonical`], `ask` is
/// `[home|null, agent|null]`. Arrays only, so object key order never
/// matters. Pinned by a golden test.
pub fn canonical(r: &GrantRecord) -> String {
    let mut caps = r.caps.clone();
    caps.sort();
    caps.dedup();
    let mut entries = r.entries.clone();
    entries.sort();
    entries.dedup();
    let entries: Vec<J> = entries.iter().map(|e| json!([e.server, e.room])).collect();
    let body = json!([
        r.garden,
        r.placement,
        r.widget,
        caps,
        r.scope.canonical(),
        [r.ask.home, r.ask.agent],
        entries,
        r.sending,
        r.granted_at,
    ]);
    format!("{GRANT_CANON_TAG}\n{body}")
}

/// HMAC-SHA256 of [`canonical`]`(r)`, lower-case hex.
pub fn sign(key: &GrantKey, r: &GrantRecord) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key.bytes).expect("HMAC takes any key length");
    mac.update(canonical(r).as_bytes());
    hex_lower(&mac.finalize().into_bytes())
}

/// Constant-time check of `sig` against `r` under `key`.
pub fn verify(key: &GrantKey, r: &GrantRecord, sig: &str) -> bool {
    use subtle::ConstantTimeEq;
    let want = sign(key, r);
    want.len() == sig.len() && bool::from(want.as_bytes().ct_eq(sig.as_bytes()))
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ── Effective caps (UW24, UW26 step 1) ──────────────────────────────────

/// A placement's grant state, sent as `grant.state` (UW35, UWB4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantState {
    /// No live grant row.
    None,
    /// A row whose signature, key or widget doesn't check out. No caps.
    Invalid,
    /// A good row granted for a different `home`/`agent` ask. No caps.
    Review,
    /// A good row, but the manifest now asks for caps it doesn't hold. The
    /// granted caps keep working; a strip offers review.
    Partial,
    /// A good row holding every requested cap.
    Granted,
}

/// What the daemon says about one custom placement's grant: `grant` in
/// `GET /cli/zen/get` (UW38). `caps` is the **effective** list: requested ∩
/// granted ∩ [`USER_WIDGET_CAPS`], in [`USER_WIDGET_CAPS`] order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantView {
    pub state: GrantState,
    pub caps: Vec<String>,
    /// The caps the row holds (empty with no good row).
    pub granted: Vec<String>,
    pub scope: Option<Scope>,
    pub entries: Vec<GrantEntry>,
    /// Sending switch; false with no good row.
    pub sending: bool,
    pub paused: Option<Pause>,
    pub granted_at: Option<String>,
    pub widget_hash: Option<String>,
}

impl GrantView {
    fn empty(state: GrantState) -> Self {
        Self {
            state,
            caps: Vec::new(),
            granted: Vec::new(),
            scope: None,
            entries: Vec::new(),
            sending: false,
            paused: None,
            granted_at: None,
            widget_hash: None,
        }
    }

    /// True when K2 must draw its review card instead of the frame: the
    /// widget asks for caps and holds none of them (none, invalid, review).
    /// A widget that asks for nothing (a clock) never needs review.
    pub fn needs_review(&self, requested: &[String]) -> bool {
        !requested.is_empty() && matches!(self.state, GrantState::None | GrantState::Invalid | GrantState::Review)
    }
}

/// One placement's question to [`effective`].
#[derive(Debug, Clone, Copy)]
pub struct GrantQuery<'a> {
    pub garden: &'a str,
    pub placement: &'a str,
    /// The placement's `widget` (folder name or `k2:<name>@<n>`).
    pub widget: &'a str,
    /// The current bundle hash (row data only; code changes keep the grant).
    pub hash: &'a str,
    /// The manifest's `caps`.
    pub requested: &'a [String],
    /// The placement's `home` / `agent` text now.
    pub ask: &'a PlacementAsk,
}

/// requested ∩ granted ∩ [`USER_WIDGET_CAPS`], in [`USER_WIDGET_CAPS`]
/// order. Pure.
pub fn effective_caps(requested: &[String], granted: &[String]) -> Vec<String> {
    USER_WIDGET_CAPS
        .iter()
        .filter(|c| requested.iter().any(|r| r == *c) && granted.iter().any(|g| g == *c))
        .map(|c| c.to_string())
        .collect()
}

/// The effective grant for one placement. **Pure**: the store loads the
/// placement's live row (`row`) and the key (`key`, `None` when the key file
/// is missing) and passes them in; B2 owns that loading (table 0138).
///
/// Fails closed:
/// - no row → `none`;
/// - a row for another garden, placement or widget, no key, a different
///   `key_id`, or a bad signature → `invalid`;
/// - the placement's ask differs from the granted ask → `review`;
/// - requested ⊄ granted → `partial` (effective caps still served);
/// - else `granted`.
pub fn effective(row: Option<&GrantRow>, key: Option<&GrantKey>, q: &GrantQuery<'_>) -> GrantView {
    let Some(row) = row else {
        return GrantView::empty(GrantState::None);
    };
    let r = &row.record;
    let same_place = r.garden == q.garden && r.placement == q.placement && r.widget == q.widget;
    let signed = match key {
        Some(k) => row.key_id == k.id() && verify(k, r, &row.sig),
        None => false,
    };
    if !same_place || !signed {
        return GrantView::empty(GrantState::Invalid);
    }
    if &r.ask != q.ask {
        return GrantView::empty(GrantState::Review);
    }
    let caps = effective_caps(q.requested, &r.caps);
    let covers = q.requested.iter().all(|c| r.caps.contains(c) && USER_WIDGET_CAPS.contains(&c.as_str()));
    GrantView {
        state: if covers { GrantState::Granted } else { GrantState::Partial },
        caps,
        granted: r.caps.clone(),
        scope: Some(r.scope.clone()),
        entries: r.entries.clone(),
        sending: r.sending,
        paused: row.paused.clone(),
        granted_at: Some(r.granted_at.clone()),
        widget_hash: Some(row.widget_hash.clone()),
    }
}

/// The placement's live row, or `None`. B2: table `zen_widget_grants`
/// (0138), `revoked_at IS NULL`.
pub fn load_row(conn: &rusqlite::Connection, garden: &str, placement: &str) -> rusqlite::Result<Option<GrantRow>> {
    let _ = (conn, garden, placement);
    todo!("B2 (UWB5): read zen_widget_grants")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn record() -> GrantRecord {
        GrantRecord {
            garden: "g-test0001".into(),
            placement: "arcade".into(),
            widget: "agent-arcade".into(),
            caps: s(&["thread:read", "agents:read"]),
            scope: Scope::Home("h-test0001".into()),
            entries: vec![
                GrantEntry { server: "bob.example.test".into(), room: "sales".into() },
                GrantEntry { server: "alice.example.test".into(), room: "cortana".into() },
            ],
            ask: PlacementAsk { home: Some("Work".into()), agent: None },
            sending: true,
            granted_at: "2026-10-08T12:00:00Z".into(),
        }
    }

    fn row(key: &GrantKey) -> GrantRow {
        let record = record();
        GrantRow {
            sig: sign(key, &record),
            record,
            widget_hash: "abc".into(),
            key_id: key.id(),
            paused: None,
            granted_by_kind: "owner_token".into(),
        }
    }

    #[test]
    fn scope_wire_json_round_trips_every_kind() {
        for (wire, scope) in [
            (json!({"agent": "cortana::alice.example.test"}), Scope::Agent("cortana::alice.example.test".into())),
            (json!({"home": "h1"}), Scope::Home("h1".into())),
            (json!({"homes": ["h2", "h1"]}), Scope::Homes(s(&["h2", "h1"]))),
            (json!({"allHomes": true}), Scope::AllHomes),
            (json!({"server": "alice.example.test"}), Scope::Server("alice.example.test".into())),
            (json!({"allServers": true}), Scope::AllServers),
        ] {
            let parsed: Scope = serde_json::from_value(wire.clone()).expect("scope parses");
            assert_eq!(parsed, scope);
            assert_eq!(serde_json::to_value(&parsed).expect("serialises"), wire);
        }
    }

    #[test]
    fn scope_refuses_bad_shapes() {
        for bad in [
            json!({}),
            json!({"home": "h1", "agent": "a"}),
            json!({"home": ""}),
            json!({"homes": []}),
            json!({"homes": ["h1", "h1"]}),
            json!({"allHomes": false}),
            json!({"world": true}),
            json!("home"),
        ] {
            assert!(serde_json::from_value::<Scope>(bad.clone()).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn canonical_form_is_pinned() {
        assert_eq!(
            canonical(&record()),
            "k2-zen-grant/v2\n[\"g-test0001\",\"arcade\",\"agent-arcade\",[\"agents:read\",\"thread:read\"],\
             [\"home\",\"h-test0001\"],[\"Work\",null],[[\"alice.example.test\",\"cortana\"],\
             [\"bob.example.test\",\"sales\"]],true,\"2026-10-08T12:00:00Z\"]"
        );
    }

    #[test]
    fn signature_checks_and_any_signed_field_change_breaks_it() {
        let key = GrantKey::from_bytes([7; 32]);
        let good = record();
        let sig = sign(&key, &good);
        assert_eq!(sig.len(), 64);
        assert!(verify(&key, &good, &sig));
        assert!(!verify(&GrantKey::from_bytes([8; 32]), &good, &sig), "another key");
        let mut reordered = good.clone();
        reordered.entries.reverse();
        reordered.caps.reverse();
        assert!(verify(&key, &reordered, &sig), "order of caps and entries is not signed");
        let edits: Vec<Box<dyn Fn(&mut GrantRecord)>> = vec![
            Box::new(|r| r.caps.push("thread:post".into())),
            Box::new(|r| r.scope = Scope::AllHomes),
            Box::new(|r| r.widget = "other".into()),
            Box::new(|r| r.placement = "other".into()),
            Box::new(|r| r.garden = "g-test0002".into()),
            Box::new(|r| r.entries[0].room = "scout".into()),
            Box::new(|r| r.ask.agent = Some("cortana".into())),
            Box::new(|r| r.sending = false),
        ];
        for (i, edit) in edits.iter().enumerate() {
            let mut r = good.clone();
            edit(&mut r);
            assert!(!verify(&key, &r, &sig), "edit {i} must break the signature");
        }
    }

    #[test]
    fn effective_fails_closed_and_intersects() {
        let key = GrantKey::from_bytes([7; 32]);
        let good = row(&key);
        let ask = PlacementAsk { home: Some("Work".into()), agent: None };
        let req = s(&["agents:read", "thread:read"]);
        let q = GrantQuery {
            garden: "g-test0001",
            placement: "arcade",
            widget: "agent-arcade",
            hash: "def",
            requested: &req,
            ask: &ask,
        };

        assert_eq!(effective(None, Some(&key), &q).state, GrantState::None);
        assert_eq!(effective(Some(&good), None, &q).state, GrantState::Invalid, "no key file");
        let other_key = GrantKey::from_bytes([9; 32]);
        assert_eq!(effective(Some(&good), Some(&other_key), &q).state, GrantState::Invalid, "changed key");
        let mut forged = good.clone();
        forged.record.caps.push("thread:post".into());
        assert_eq!(effective(Some(&forged), Some(&key), &q).state, GrantState::Invalid, "hand-edited row");

        let v = effective(Some(&good), Some(&key), &q);
        assert_eq!(v.state, GrantState::Granted);
        assert_eq!(v.caps, s(&["agents:read", "thread:read"]));
        assert!(v.sending);
        assert!(!v.needs_review(&req));

        let more = s(&["agents:read", "thread:read", "thread:post"]);
        let v = effective(Some(&good), Some(&key), &GrantQuery { requested: &more, ..q });
        assert_eq!(v.state, GrantState::Partial);
        assert_eq!(v.caps, s(&["agents:read", "thread:read"]), "granted caps keep working");

        let moved = PlacementAsk { home: Some("Play".into()), agent: None };
        let v = effective(Some(&good), Some(&key), &GrantQuery { ask: &moved, ..q });
        assert_eq!(v.state, GrantState::Review);
        assert!(v.caps.is_empty());
        assert!(v.needs_review(&req));
        assert!(!v.needs_review(&[]), "a widget that asks for nothing never needs review");

        let v = effective(Some(&good), Some(&key), &GrantQuery { widget: "agent-arcade-2", ..q });
        assert_eq!(v.state, GrantState::Invalid, "a row for another widget");
    }

    #[test]
    fn effective_caps_never_include_a_non_widget_cap() {
        let req = s(&["gardens:manage", "agents:read", "app:navigate"]);
        let granted = s(&["gardens:manage", "agents:read", "app:navigate"]);
        assert_eq!(effective_caps(&req, &granted), s(&["agents:read"]));
    }

    #[test]
    fn user_widget_caps_match_the_catalog() {
        use crate::contract::{catalog, Exposure};
        assert_eq!(catalog().caps_exposed_to(Exposure::Widget), USER_WIDGET_CAPS);
        for c in USER_WIDGET_CAPS {
            assert!(super::super::BRIDGE_CAPS.contains(c), "{c} is a bridge cap");
        }
    }
}
