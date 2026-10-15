//! The chat-pass signing keyring, stored in the K2 vault (S28 §7, amended
//! by Q-K16: vault entries, not `~/.k2/chat-pass/*.pem`).
//!
//! ## Storage
//!
//! Every entry is a `system` secret in scope `engine:chat-pass` (no route
//! ever returns a `system` value; delivery `none`):
//!
//! | Name | Kind | Holds |
//! |---|---|---|
//! | `key.<kid>` | `signing_key` | PKCS#8 PEM, P-256 private key |
//! | `state` | `generic` | the lifecycle JSON below (no private material) |
//! | `state.corrupt.<unix>` | `generic` | a corrupt `state` kept for forensics by an emergency rotation |
//!
//! `state`:
//!
//! ```json
//! {"format":1,
//!  "keys":[{"kid":"a1b2…","state":"current","created_at":…,"published_at":…,"activated_at":…},
//!          {"kid":"c3d4…","state":"next","created_at":…,"published_at":…},
//!          {"kid":"e5f6…","state":"retiring","created_at":…,"published_at":…,"activated_at":…,"last_used_at":…}],
//!  "history":[{"kid":"…","retired_at":…}]}
//! ```
//!
//! ## Lifecycle (§7.2–7.3)
//!
//! `next` → `current` → `retiring` → destroyed. Exactly one `current`, at
//! most one `next`; only `current` signs. [`plan`] is pure; [`Keyring`]
//! applies it. Write order (crash safety, §7.5): new key entries first,
//! then `state`, then destroy dropped keys. A `key.*` entry that `state`
//! doesn't list is an orphan from a crash and is destroyed on load. A
//! listed kid without a matching key, an unparseable `state`, or a broken
//! invariant is corruption: everything fails closed until an emergency
//! rotation (which needs a person, see `routes.rs`).
//!
//! The private keys never leave this module: no `Debug`, no log line, no
//! response carries one. Log lines go through [`klog`].

use std::collections::HashMap;
use std::sync::Arc;

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use k2_vault::{
    Actor, Delivery, Kind, ListFilter, LocalVault, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes,
    SecretStatus, SecretStore, Selector, Use,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{ChatPassClaims, CHAT_PASS_TYP};

/// Vault engine scope for the chat-pass keys.
pub const PASS_ENGINE: &str = "chat-pass";
/// Reserved for KM13's widget-approval attestation key: a separate
/// keyring (own `kid`s, own JWKS URL), never the pass key. Not built.
pub const APPROVAL_ENGINE_RESERVED: &str = "chat-approval";

pub const STATE_NAME: &str = "state";
pub const KEY_PREFIX: &str = "key.";
pub const FORMAT: u32 = 1;

/// Promote `next` this long after `current` was activated (30 days).
pub const ROTATE_AFTER_SECS: i64 = 30 * 24 * 60 * 60;
/// `next` must have been in the JWKS this long before it may sign.
pub const PUBLISH_LEAD_MIN_SECS: i64 = 600;
/// A `retiring` key stays in the JWKS this long after its last signature.
pub const RETIRE_AFTER_LAST_USE_SECS: i64 = 600;
/// Retired kids kept in `history` for audit.
pub const HISTORY_MAX: usize = 10;

// ── state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyRole {
    Next,
    Current,
    Retiring,
}

impl KeyRole {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyRole::Next => "next",
            KeyRole::Current => "current",
            KeyRole::Retiring => "retiring",
        }
    }
}

/// One key's lifecycle record (public facts only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyEntry {
    pub kid: String,
    pub state: KeyRole,
    pub created_at: i64,
    pub published_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retired {
    pub kid: String,
    pub retired_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyState {
    pub format: u32,
    pub keys: Vec<KeyEntry>,
    #[serde(default)]
    pub history: Vec<Retired>,
}

impl KeyState {
    pub fn current(&self) -> Option<&KeyEntry> {
        self.keys.iter().find(|k| k.state == KeyRole::Current)
    }

    pub fn next(&self) -> Option<&KeyEntry> {
        self.keys.iter().find(|k| k.state == KeyRole::Next)
    }

    /// JWKS order: `current`, then `next`, then any `retiring`.
    pub fn published(&self) -> Vec<&KeyEntry> {
        let mut out: Vec<&KeyEntry> = vec![];
        out.extend(self.keys.iter().filter(|k| k.state == KeyRole::Current));
        out.extend(self.keys.iter().filter(|k| k.state == KeyRole::Next));
        out.extend(self.keys.iter().filter(|k| k.state == KeyRole::Retiring));
        out
    }
}

fn is_kid(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The §7.2 invariants. A violation is corruption.
pub fn check_invariants(s: &KeyState) -> Result<(), String> {
    if s.format != FORMAT {
        return Err(format!("state format {} is not {FORMAT}", s.format));
    }
    let currents = s.keys.iter().filter(|k| k.state == KeyRole::Current).count();
    if currents != 1 {
        return Err(format!("{currents} current keys (want exactly one)"));
    }
    let nexts = s.keys.iter().filter(|k| k.state == KeyRole::Next).count();
    if nexts > 1 {
        return Err(format!("{nexts} next keys (want at most one)"));
    }
    let mut seen = std::collections::HashSet::new();
    for k in &s.keys {
        if !is_kid(&k.kid) {
            return Err("a kid is not 16 lowercase hex characters".into());
        }
        if !seen.insert(k.kid.as_str()) {
            return Err(format!("kid {} listed twice", k.kid));
        }
        match k.state {
            KeyRole::Current if k.activated_at.is_none() => return Err(format!("current key {} has no activated_at", k.kid)),
            KeyRole::Retiring if k.last_used_at.is_none() => return Err(format!("retiring key {} has no last_used_at", k.kid)),
            _ => {}
        }
    }
    Ok(())
}

// ── pure plan (§7.3) ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// No keyring yet: a `current` (signs at once) and a `next`.
    Bootstrap,
    /// No `next` (after an emergency or a crash): make one.
    GenerateNext,
    /// `next` → `current`, `current` → `retiring`, then a new `next`.
    Promote,
    /// Rotation is due but `next` hasn't been published long enough.
    LeadTooShort,
    /// Drop this `retiring` key.
    Retire(String),
}

/// What maintenance should do at `now`. Pure.
pub fn plan(state: Option<&KeyState>, now: i64) -> Vec<KeyAction> {
    let Some(s) = state else { return vec![KeyAction::Bootstrap] };
    let mut out = vec![];
    if s.next().is_none() {
        out.push(KeyAction::GenerateNext);
    }
    if let Some(cur) = s.current() {
        let activated = cur.activated_at.unwrap_or(cur.created_at);
        if now - activated >= ROTATE_AFTER_SECS {
            match s.next() {
                Some(n) if now - n.published_at >= PUBLISH_LEAD_MIN_SECS => out.push(KeyAction::Promote),
                _ => out.push(KeyAction::LeadTooShort),
            }
        }
    }
    for k in &s.keys {
        if k.state == KeyRole::Retiring && now - k.last_used_at.unwrap_or(now) >= RETIRE_AFTER_LAST_USE_SECS {
            out.push(KeyAction::Retire(k.kid.clone()));
        }
    }
    out
}

fn new_entry(kid: String, role: KeyRole, now: i64) -> KeyEntry {
    KeyEntry {
        kid,
        state: role,
        created_at: now,
        published_at: now,
        activated_at: (role == KeyRole::Current).then_some(now),
        last_used_at: None,
    }
}

fn push_history(h: &mut Vec<Retired>, kid: String, now: i64) {
    h.push(Retired { kid, retired_at: now });
    if h.len() > HISTORY_MAX {
        let extra = h.len() - HISTORY_MAX;
        h.drain(..extra);
    }
}

/// Apply `actions` to `state`. `fresh()` makes a new key and returns its
/// kid. Returns the new state and the kids to destroy. Pure apart from
/// `fresh`.
pub fn transform(
    state: Option<&KeyState>,
    actions: &[KeyAction],
    now: i64,
    fresh: &mut dyn FnMut() -> Result<String, String>,
) -> Result<(KeyState, Vec<String>), String> {
    let mut s = state.cloned().unwrap_or(KeyState { format: FORMAT, keys: vec![], history: vec![] });
    let mut dropped = vec![];
    for a in actions {
        match a {
            KeyAction::Bootstrap => {
                s.keys.push(new_entry(fresh()?, KeyRole::Current, now));
                s.keys.push(new_entry(fresh()?, KeyRole::Next, now));
            }
            KeyAction::GenerateNext => {
                if s.next().is_none() {
                    s.keys.push(new_entry(fresh()?, KeyRole::Next, now));
                }
            }
            KeyAction::Promote => {
                if s.next().is_none() {
                    return Err("promote without a next key".into());
                }
                for k in s.keys.iter_mut() {
                    match k.state {
                        KeyRole::Current => {
                            k.state = KeyRole::Retiring;
                            k.last_used_at = Some(now);
                        }
                        KeyRole::Next => {
                            k.state = KeyRole::Current;
                            k.activated_at = Some(now);
                        }
                        KeyRole::Retiring => {}
                    }
                }
                s.keys.push(new_entry(fresh()?, KeyRole::Next, now));
            }
            KeyAction::LeadTooShort => {}
            KeyAction::Retire(kid) => {
                if let Some(i) = s.keys.iter().position(|k| &k.kid == kid && k.state == KeyRole::Retiring) {
                    s.keys.remove(i);
                    push_history(&mut s.history, kid.clone(), now);
                    dropped.push(kid.clone());
                }
            }
        }
    }
    check_invariants(&s)?;
    Ok((s, dropped))
}

// ── errors ──────────────────────────────────────────────────────────

/// Keyring failures. Messages never carry key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The vault isn't open (or a restore is swapping it).
    VaultClosed(String),
    /// The stored keyring is broken. Fail closed; recovery is an
    /// emergency rotation.
    Corrupt(String),
    /// A vault read or write failed.
    Store(String),
    /// The claims don't make a valid pass.
    BadClaims(String),
    /// A normal rotation asked for before `next` was published long enough.
    LeadTooShort { retry_after_secs: i64 },
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::VaultClosed(c) => write!(f, "the vault isn't open ({c})"),
            KeyError::Corrupt(m) => write!(f, "chat-pass keyring is corrupt: {m}"),
            KeyError::Store(m) => write!(f, "chat-pass keyring store: {m}"),
            KeyError::BadClaims(m) => write!(f, "bad chat-pass claims: {m}"),
            KeyError::LeadTooShort { retry_after_secs } => {
                write!(f, "the next key has not been published long enough (retry in {retry_after_secs}s)")
            }
        }
    }
}

// ── logging ─────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) static LOG_CAPTURE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Every keyring log line goes through here (tests read them back to
/// prove no key material is ever logged).
pub(crate) fn klog(line: String) {
    k2_core::log_debug!("[chat-pass] {line}");
    #[cfg(test)]
    LOG_CAPTURE.lock().push(line);
}

// ── the keyring ─────────────────────────────────────────────────────

/// A parsed key. Deliberately not `Debug`.
struct LoadedKey {
    encoding: EncodingKey,
    jwk: serde_json::Value,
}

struct Loaded {
    state: KeyState,
    keys: HashMap<String, LoadedKey>,
}

enum Cached {
    Unknown,
    Absent,
    Present(Loaded),
}

struct Inner {
    cached: Cached,
    lead_short_logged: bool,
}

/// A keyring over one vault. Every operation holds one lock, so no
/// signature with a key happens after the promotion that retires it.
pub struct Keyring {
    vault: Arc<LocalVault>,
    engine: &'static str,
    inner: Mutex<Inner>,
}

/// A freshly generated key: kid + PEM (written to the vault, then
/// dropped).
struct FreshKey {
    kid: String,
    pem: String,
}

fn generate() -> Result<FreshKey, String> {
    let sk = crate::es256::random_secret()?;
    let pem = crate::es256::private_pem(&sk)?;
    let (kid, _) = crate::es256::jwk_from_private_pem(&pem)?;
    Ok(FreshKey { kid, pem })
}

fn parse_key(kid: &str, pem: &str) -> Result<LoadedKey, String> {
    let (got, jwk) = crate::es256::jwk_from_private_pem(pem).map_err(|_| format!("key {kid} doesn't parse"))?;
    if got != kid {
        return Err(format!("key {kid} hashes to a different kid"));
    }
    let encoding = EncodingKey::from_ec_pem(pem.as_bytes()).map_err(|_| format!("key {kid} doesn't load for signing"))?;
    Ok(LoadedKey { encoding, jwk })
}

impl Keyring {
    pub fn new(vault: Arc<LocalVault>, engine: &'static str) -> Keyring {
        Keyring { vault, engine, inner: Mutex::new(Inner { cached: Cached::Unknown, lead_short_logged: false }) }
    }

    pub fn vault(&self) -> &Arc<LocalVault> {
        &self.vault
    }

    /// Drop the in-memory copy; the next call reads the vault again (a
    /// daemon restart, for tests).
    pub fn forget_cache(&self) {
        self.inner.lock().cached = Cached::Unknown;
    }

    fn actor(&self) -> Actor {
        Actor::System(self.engine.to_string())
    }

    fn requester(&self) -> Requester {
        Requester::System(self.engine.to_string())
    }

    fn sel(&self, name: &str) -> Selector {
        Selector::system(Scope::engine(self.engine), name)
    }

    /// `Ok(None)` when the entry doesn't exist or was destroyed.
    fn read(&self, name: &str) -> Result<Option<String>, KeyError> {
        match self.vault.meta(&self.sel(name), &self.actor()) {
            Ok(m) if m.status == SecretStatus::Active => {}
            Ok(_) => return Ok(None),
            Err(e) if e.status == 404 => return Ok(None),
            Err(e) => return Err(KeyError::Store(format!("vault meta: {}", e.code))),
        }
        let r = self
            .vault
            .resolve(&self.sel(name), Use::System, &self.requester())
            .map_err(|e| KeyError::Store(format!("vault read: {}", e.code)))?;
        String::from_utf8(r.value.expose().to_vec())
            .map(Some)
            .map_err(|_| KeyError::Corrupt(format!("{name} isn't text")))
    }

    fn write(&self, name: &str, kind: Kind, value: &str, mode: PutMode) -> Result<(), KeyError> {
        let mut r = PutRequest::new(Namespace::System, Scope::engine(self.engine), name, kind);
        r.default_delivery = Some(Delivery::None);
        r.delivery_ceiling = Some(Delivery::None);
        r.origin = format!("system:{}", self.engine);
        r.description = match kind {
            Kind::SigningKey => "Sew chat-pass ES256 signing key".into(),
            _ => "Sew chat-pass keyring state".into(),
        };
        r.mode = mode;
        self.vault
            .put(r, SecretBytes::from_slice(value.as_bytes()), &self.actor())
            .map(|_| ())
            .map_err(|e| KeyError::Store(format!("vault write {name}: {}", e.code)))
    }

    fn destroy(&self, name: &str) -> Result<(), KeyError> {
        match self.vault.destroy(&self.sel(name), &self.actor()) {
            Ok(()) => Ok(()),
            Err(e) if e.status == 404 || e.code == "secret_destroyed" => Ok(()),
            Err(e) => Err(KeyError::Store(format!("vault destroy {name}: {}", e.code))),
        }
    }

    /// Live `key.*` entries in the vault.
    fn stored_key_names(&self) -> Result<Vec<String>, KeyError> {
        let f = ListFilter { include_system: true, scope: Some(Scope::engine(self.engine)), include_destroyed: false };
        let list = self.vault.list(&f, &self.actor()).map_err(|e| KeyError::Store(format!("vault list: {}", e.code)))?;
        Ok(list
            .into_iter()
            .filter(|m| m.namespace == Namespace::System && m.status == SecretStatus::Active && m.name.starts_with(KEY_PREFIX))
            .map(|m| m.name)
            .collect())
    }

    fn sweep_orphans(&self, listed: &[String]) -> Result<(), KeyError> {
        for name in self.stored_key_names()? {
            let kid = &name[KEY_PREFIX.len()..];
            if !listed.iter().any(|k| k == kid) {
                self.destroy(&name)?;
                klog(format!("destroyed orphan key {kid} (not in state; a crash between writes)"));
            }
        }
        Ok(())
    }

    /// Read the keyring from the vault.
    fn load_from_vault(&self) -> Result<Option<Loaded>, KeyError> {
        let Some(raw) = self.read(STATE_NAME)? else { return Ok(None) };
        let state: KeyState = serde_json::from_str(&raw).map_err(|_| KeyError::Corrupt("state doesn't parse".into()))?;
        check_invariants(&state).map_err(KeyError::Corrupt)?;
        let mut keys = HashMap::new();
        for k in &state.keys {
            let pem = self
                .read(&format!("{KEY_PREFIX}{}", k.kid))?
                .ok_or_else(|| KeyError::Corrupt(format!("key {} is listed but missing", k.kid)))?;
            let lk = parse_key(&k.kid, &pem).map_err(KeyError::Corrupt)?;
            keys.insert(k.kid.clone(), lk);
        }
        let listed: Vec<String> = state.keys.iter().map(|k| k.kid.clone()).collect();
        self.sweep_orphans(&listed)?;
        Ok(Some(Loaded { state, keys }))
    }

    fn ensure_loaded(&self, inner: &mut Inner) -> Result<(), KeyError> {
        if matches!(inner.cached, Cached::Unknown) {
            inner.cached = match self.load_from_vault()? {
                Some(l) => Cached::Present(l),
                None => Cached::Absent,
            };
        }
        Ok(())
    }

    /// Write a new state (keys first, then `state`, then destroy dropped
    /// keys) and rebuild the in-memory copy.
    fn commit(
        &self,
        old: Option<Loaded>,
        new_state: KeyState,
        fresh: Vec<FreshKey>,
        dropped: &[String],
    ) -> Result<Loaded, KeyError> {
        let mut parsed: HashMap<String, LoadedKey> = HashMap::new();
        for f in &fresh {
            let lk = parse_key(&f.kid, &f.pem).map_err(KeyError::Store)?;
            self.write(&format!("{KEY_PREFIX}{}", f.kid), Kind::SigningKey, &f.pem, PutMode::Create)?;
            parsed.insert(f.kid.clone(), lk);
        }
        drop(fresh);
        let json = serde_json::to_string(&new_state).map_err(|e| KeyError::Store(format!("encode state: {e}")))?;
        self.write(STATE_NAME, Kind::Generic, &json, PutMode::Upsert)?;
        for kid in dropped {
            // State no longer lists it; a failed destroy leaves an orphan
            // the next load sweeps.
            if let Err(e) = self.destroy(&format!("{KEY_PREFIX}{kid}")) {
                klog(format!("retired key {kid} not destroyed yet: {e}"));
            }
        }
        let mut keys = old.map(|o| o.keys).unwrap_or_default();
        keys.retain(|kid, _| new_state.keys.iter().any(|k| &k.kid == kid));
        keys.extend(parsed);
        for k in &new_state.keys {
            if !keys.contains_key(&k.kid) {
                return Err(KeyError::Store(format!("key {} missing after commit", k.kid)));
            }
        }
        Ok(Loaded { state: new_state, keys })
    }

    /// Run `actions` (from [`plan`] or a route) against the cached state.
    fn run(&self, inner: &mut Inner, actions: &[KeyAction], now: i64) -> Result<bool, KeyError> {
        let effective: Vec<&KeyAction> = actions.iter().filter(|a| **a != KeyAction::LeadTooShort).collect();
        if actions.contains(&KeyAction::LeadTooShort) {
            if !inner.lead_short_logged {
                klog("rotation is due but the next key was published under 10 minutes ago; waiting".into());
                inner.lead_short_logged = true;
            }
        } else {
            inner.lead_short_logged = false;
        }
        if effective.is_empty() {
            return Ok(false);
        }
        let old = match std::mem::replace(&mut inner.cached, Cached::Unknown) {
            Cached::Present(l) => Some(l),
            _ => None,
        };
        let mut fresh: Vec<FreshKey> = vec![];
        let mut gen = || -> Result<String, String> {
            let k = generate()?;
            let kid = k.kid.clone();
            fresh.push(k);
            Ok(kid)
        };
        let owned: Vec<KeyAction> = effective.into_iter().cloned().collect();
        let (new_state, dropped) = match transform(old.as_ref().map(|l| &l.state), &owned, now, &mut gen) {
            Ok(x) => x,
            Err(e) => {
                // Keep the old copy; nothing was written.
                inner.cached = old.map(Cached::Present).unwrap_or(Cached::Unknown);
                return Err(KeyError::Store(e));
            }
        };
        let summary: Vec<String> = owned
            .iter()
            .map(|a| match a {
                KeyAction::Retire(k) => format!("retire {k}"),
                other => format!("{other:?}").to_lowercase(),
            })
            .collect();
        match self.commit(old, new_state, fresh, &dropped) {
            Ok(l) => {
                let kids: Vec<String> = l.state.published().iter().map(|k| format!("{}={}", k.state.as_str(), k.kid)).collect();
                klog(format!("{}; published {}", summary.join(", "), kids.join(" ")));
                inner.cached = Cached::Present(l);
                Ok(true)
            }
            Err(e) => {
                // Re-read from the vault next time (a partial write is
                // either an orphan key or a full new state).
                inner.cached = Cached::Unknown;
                Err(e)
            }
        }
    }

    fn maintain_locked(&self, inner: &mut Inner, now: i64, create: bool) -> Result<bool, KeyError> {
        self.ensure_loaded(inner)?;
        let state = match &inner.cached {
            Cached::Present(l) => Some(&l.state),
            Cached::Absent if create => {
                // Leftover keys from a crash during bootstrap.
                self.sweep_orphans(&[])?;
                None
            }
            _ => return Ok(false),
        };
        let actions = plan(state, now);
        self.run(inner, &actions, now)
    }

    /// Run the §7.3 schedule. `create`: bootstrap a missing keyring (the
    /// JWKS and mints do; the background tick never does). Returns
    /// whether anything changed.
    pub fn maintain(&self, now: i64, create: bool) -> Result<bool, KeyError> {
        let mut g = self.inner.lock();
        self.maintain_locked(&mut g, now, create)
    }

    /// `{"keys":[…]}`: public JWKs of `current`, `next` and any `retiring`.
    pub fn public_jwks(&self, now: i64) -> Result<serde_json::Value, KeyError> {
        let mut g = self.inner.lock();
        self.maintain_locked(&mut g, now, true)?;
        let Cached::Present(l) = &g.cached else {
            return Err(KeyError::Store("keyring missing after maintenance".into()));
        };
        let keys: Vec<serde_json::Value> =
            l.state.published().iter().filter_map(|k| l.keys.get(&k.kid).map(|lk| lk.jwk.clone())).collect();
        Ok(serde_json::json!({ "keys": keys }))
    }

    /// Sign one pass with `current` (after maintenance).
    pub fn sign(&self, claims: &ChatPassClaims, now: i64) -> Result<String, KeyError> {
        claims.validate().map_err(KeyError::BadClaims)?;
        let mut g = self.inner.lock();
        self.maintain_locked(&mut g, now, true)?;
        let Cached::Present(l) = &g.cached else {
            return Err(KeyError::Store("keyring missing after maintenance".into()));
        };
        let cur = l.state.current().ok_or_else(|| KeyError::Corrupt("no current key".into()))?;
        let key = l.keys.get(&cur.kid).ok_or_else(|| KeyError::Corrupt("current key not loaded".into()))?;
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(CHAT_PASS_TYP.into());
        header.kid = Some(cur.kid.clone());
        jsonwebtoken::encode(&header, claims, &key.encoding).map_err(|e| KeyError::Store(format!("sign: {:?}", e.kind())))
    }

    /// The lifecycle record, without creating a keyring (`None` = none yet).
    pub fn state(&self) -> Result<Option<KeyState>, KeyError> {
        let mut g = self.inner.lock();
        self.ensure_loaded(&mut g)?;
        Ok(match &g.cached {
            Cached::Present(l) => Some(l.state.clone()),
            _ => None,
        })
    }

    /// Owner "rotate now": promote `next` if it has been published at
    /// least [`PUBLISH_LEAD_MIN_SECS`]; else [`KeyError::LeadTooShort`].
    pub fn rotate_now(&self, now: i64) -> Result<KeyState, KeyError> {
        let mut g = self.inner.lock();
        self.maintain_locked(&mut g, now, true)?;
        let Cached::Present(l) = &g.cached else {
            return Err(KeyError::Store("keyring missing after maintenance".into()));
        };
        let next = l.state.next().ok_or_else(|| KeyError::Corrupt("no next key after maintenance".into()))?;
        let waited = now - next.published_at;
        if waited < PUBLISH_LEAD_MIN_SECS {
            return Err(KeyError::LeadTooShort { retry_after_secs: PUBLISH_LEAD_MIN_SECS - waited });
        }
        self.run(&mut g, &[KeyAction::Promote], now)?;
        match &g.cached {
            Cached::Present(l) => Ok(l.state.clone()),
            _ => Err(KeyError::Store("keyring missing after rotation".into())),
        }
    }

    /// Owner "drop previous": retire every `retiring` key now, without
    /// waiting out [`RETIRE_AFTER_LAST_USE_SECS`]. Passes signed with it in
    /// the last minute stop verifying once Sew refetches the JWKS.
    pub fn drop_previous(&self, now: i64) -> Result<(Option<KeyState>, Vec<String>), KeyError> {
        let mut g = self.inner.lock();
        self.ensure_loaded(&mut g)?;
        let kids: Vec<String> = match &g.cached {
            Cached::Present(l) => l.state.keys.iter().filter(|k| k.state == KeyRole::Retiring).map(|k| k.kid.clone()).collect(),
            _ => return Ok((None, vec![])),
        };
        let actions: Vec<KeyAction> = kids.iter().cloned().map(KeyAction::Retire).collect();
        self.run(&mut g, &actions, now)?;
        let st = match &g.cached {
            Cached::Present(l) => Some(l.state.clone()),
            _ => None,
        };
        Ok((st, kids))
    }

    /// Emergency rotation (§7.4): a new `current` and `next`, every old
    /// key destroyed, no lead and no grace. Also the recovery from a
    /// corrupt keyring (§7.5): the corrupt `state` is kept as
    /// `state.corrupt.<now>`. The caller must have checked a person-proof.
    pub fn emergency(&self, now: i64) -> Result<KeyState, KeyError> {
        let mut g = self.inner.lock();
        let loaded = match self.ensure_loaded(&mut g) {
            Ok(()) => match std::mem::replace(&mut g.cached, Cached::Unknown) {
                Cached::Present(l) => Some(l),
                _ => None,
            },
            Err(KeyError::Corrupt(why)) => {
                if let Ok(Some(raw)) = self.read(STATE_NAME) {
                    self.write(&format!("{STATE_NAME}.corrupt.{now}"), Kind::Generic, &raw, PutMode::Upsert)?;
                }
                klog(format!("emergency rotation over a corrupt keyring ({why})"));
                None
            }
            Err(e) => return Err(e),
        };
        let mut history = loaded.as_ref().map(|l| l.state.history.clone()).unwrap_or_default();
        let old_kids: Vec<String> = loaded.as_ref().map(|l| l.state.keys.iter().map(|k| k.kid.clone()).collect()).unwrap_or_default();
        for kid in &old_kids {
            push_history(&mut history, kid.clone(), now);
        }
        let mut fresh: Vec<FreshKey> = vec![];
        let mut gen = || -> Result<String, String> {
            let k = generate()?;
            let kid = k.kid.clone();
            fresh.push(k);
            Ok(kid)
        };
        let (mut new_state, _) = transform(None, &[KeyAction::Bootstrap], now, &mut gen).map_err(KeyError::Store)?;
        new_state.history = history;
        // Every stored key that the new state doesn't list goes, whether
        // the old state knew it or not.
        let mut dropped: Vec<String> = self
            .stored_key_names()?
            .into_iter()
            .map(|n| n[KEY_PREFIX.len()..].to_string())
            .filter(|kid| !new_state.keys.iter().any(|k| &k.kid == kid))
            .collect();
        for kid in old_kids {
            if !dropped.contains(&kid) {
                dropped.push(kid);
            }
        }
        match self.commit(None, new_state, fresh, &dropped) {
            Ok(l) => {
                let st = l.state.clone();
                klog(format!(
                    "emergency rotation: dropped {} key(s); current={} next={}",
                    dropped.len(),
                    st.current().map(|k| k.kid.as_str()).unwrap_or("-"),
                    st.next().map(|k| k.kid.as_str()).unwrap_or("-"),
                ));
                g.cached = Cached::Present(l);
                g.lead_short_logged = false;
                Ok(st)
            }
            Err(e) => {
                g.cached = Cached::Unknown;
                Err(e)
            }
        }
    }
}

// ── the daemon's keyring ────────────────────────────────────────────

static GLOBAL: Mutex<Option<Arc<Keyring>>> = Mutex::new(None);

/// The daemon's chat-pass keyring over the open vault. A vault restore
/// (a new vault object) gets a fresh keyring.
pub fn global() -> Result<Arc<Keyring>, KeyError> {
    let v = crate::vault::get().map_err(|e| KeyError::VaultClosed(e.code.to_string()))?;
    let mut g = GLOBAL.lock();
    if let Some(k) = g.as_ref() {
        if Arc::ptr_eq(&k.vault, &v) {
            return Ok(k.clone());
        }
    }
    let k = Arc::new(Keyring::new(v, PASS_ENGINE));
    *g = Some(k.clone());
    Ok(k)
}

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
