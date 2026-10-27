//! The send pipeline (prd-email-notifications-core-v1 §5.2) and K2's own
//! system mail (E28).
//!
//! In order, for an App's send:
//! 1. the box switch (`email_off`) and air-gap (`airgap`, E21);
//! 2. the caller is the App (its link pass with `email:send`, or an App
//!    key with scope `email`), or a person or the App's own agent through
//!    `k2 app email send`; guests never reach here;
//! 3. the App's email is on with a granted sender (`email_off`,
//!    `no_sender`); a `from` naming another sender is `sender_not_granted`,
//!    the system sender `system_sender` (E20);
//! 4. the sender may send now ([`super::senders::ready`]);
//! 5. composition (E19), the From name (the owner's lock, never the system
//!    sender's name, E20), Reply-To, the recipient scope (`users`: only the
//!    App's users whose address is proven, E16; `any` behind the owner's
//!    second confirm);
//! 6. idempotency (same key + same request = the same answer, no second
//!    send; same key + another request = `409 idempotency_conflict`);
//! 7. budgets (E17, [`super::budget`]);
//! 8. the App's agent through the CLI: its `agentSend` gate (`off` →
//!    `gated`, `approval` → held for the owner, E15);
//! 9. the log row first (no row, no send), then the hand-off (hosted: E8,
//!    the stored copy destroyed; relay: E12), then `submitted`/`failed`. A
//!    future `sendAt` is `scheduled` (at most 7 days ahead, counted in the
//!    budget window it leaves in); the flusher re-runs steps 3–7 then.
//!
//! Every refusal after the App is known writes a `refused` row with its
//! code (at most [`MAX_REFUSED_ROWS_PER_HOUR`] an App an hour). Nothing
//! here logs an address, a subject or a body (E22); the log keeps metadata
//! only (E7). A scheduled send's body waits in a 0600 file under
//! `~/.k2/email-notify/scheduled/` and is deleted at hand-off; immediate
//! sends never write one.

use std::collections::BTreeMap;

use serde_json::{json, Value as J};

use k2_core::email_notify::{self as store, LogRow, Sender};

use super::compose::{self, Request};
use super::{db_fail, fail, Fail, R};

/// Refused rows one App may add to the log an hour (a flood guard).
pub const MAX_REFUSED_ROWS_PER_HOUR: i64 = 100;

/// Who is sending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The App itself: its link pass or an App key.
    App { app: String, actor: String },
    /// A person (the owner, an Admin, a K2 shell tab) via `k2 app email send`.
    Person { app: String, actor: String },
    /// The App's own agent via `k2 app email send` (its agentSend gate).
    Agent { app: String, actor: String, project_id: String, project_path: String },
}

impl Caller {
    pub fn app(&self) -> &str {
        match self {
            Caller::App { app, .. } | Caller::Person { app, .. } | Caller::Agent { app, .. } => app,
        }
    }
    pub fn actor(&self) -> &str {
        match self {
            Caller::App { actor, .. } | Caller::Person { actor, .. } | Caller::Agent { actor, .. } => actor,
        }
    }
    fn origin(&self) -> &'static str {
        match self {
            Caller::App { .. } => store::ORIGIN_APP,
            Caller::Person { .. } | Caller::Agent { .. } => store::ORIGIN_AGENT,
        }
    }
}

fn log_refusal(app: &str, sender_id: &str, to: &[String], f: &Fail, now: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::count_refused(&conn, app, now - 3600) {
        Ok(n) if n < MAX_REFUSED_ROWS_PER_HOUR => {}
        _ => return,
    }
    let r = store::insert_log(
        &conn,
        &store::NewLog {
            sender_id,
            origin: store::ORIGIN_APP,
            app_service_id: Some(app),
            purpose: None,
            to,
            subject: "",
            tags: &[],
            status: store::ST_REFUSED,
            error_code: Some(f.code),
            message_id: None,
            send_after: None,
            scope_any: false,
            now,
        },
    );
    if let Err(e) = r {
        k2_core::log_debug!("[email-notify] WARN refused-row write for App {app}: {e}");
    }
}

/// The App's granted sender, if its email is on.
fn app_sender(app: &str) -> R<(store::AppState, Sender)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let st = store::app_state(&conn, app).map_err(db_fail)?;
    if !st.enabled {
        return Err(fail(
            403,
            "email_off",
            "this App doesn't send email: its owner turns it on with a sender (k2 app email on <app> --sender <address>)",
        ));
    }
    let sender = match st.sender_id.as_deref() {
        Some(id) => store::get_sender(&conn, id).map_err(db_fail)?,
        None => None,
    };
    match sender {
        Some(s) if s.is_live() && s.app() == Some(app) => Ok((st, s)),
        _ => Err(fail(409, "no_sender", "this App has no sender: its owner grants one (k2 app email on <app> --sender <address>)")),
    }
}

/// §5.2 step 3, "cap approved" (security review M2): an App with a runtime
/// row sends only once the owner approved `email:send` for it. A `--cmd`
/// service has no runtime row; `k2 app email on` (with its confirm) is the
/// owner's approval there. An unreadable store refuses.
pub fn cap_ok(app: &str) -> R<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::deploy::store::get_app_by_service(&conn, app).map_err(db_fail)?;
    let Some(row) = row else { return Ok(()) };
    let set = crate::apps::grants::recorded(&conn, &row.app_id).map_err(db_fail)?;
    if set.is_some_and(|s| s.iter().any(|c| c == k2_core::skin::CAP_EMAIL_SEND)) {
        return Ok(());
    }
    Err(fail(
        403,
        "cap_not_granted",
        "this App's email:send isn't approved: add it to the manifest and deploy; the owner approves it once",
    ))
}

/// A request's `from` must be the App's own sender.
fn check_from(req_from: Option<&str>, s: &Sender) -> R<()> {
    let Some(f) = req_from else { return Ok(()) };
    let f = f.trim();
    if f.eq_ignore_ascii_case(&s.address) || f == s.id {
        return Ok(());
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::find_sender(&conn, f).map_err(db_fail)? {
        Some(o) if o.is_system() => Err(fail(409, "system_sender", "the system sender is K2's own; an App never sends from it")),
        _ => Err(fail(403, "sender_not_granted", "from names a sender this App wasn't granted (omit from to use the App's own)")),
    }
}

/// E20: the From name for this send.
fn from_name(req: &Request, s: &Sender) -> R<String> {
    let Some(asked) = req.from_name.as_deref() else { return Ok(s.from_name.clone()) };
    if s.from_name_locked && asked != s.from_name {
        return Err(fail(400, "usage", "the owner locked this sender's From name; omit fromName"));
    }
    if compose::encoded_word(asked) {
        return Err(fail(400, "usage", "fromName can't hold an encoded word (=?…?=)"));
    }
    // E20, review L6: compared case- and whitespace-insensitively ("K 2 "
    // is "k2"); an unreadable system sender refuses.
    if let Some(sys) = super::senders::system()? {
        if compose::fold_name(asked) == compose::fold_name(&sys.from_name) && sys.id != s.id {
            return Err(fail(400, "usage", "fromName can't be the system sender's name"));
        }
    }
    Ok(asked.to_string())
}

/// The registrable domain of `domain` by the Public Suffix List (k2-vault's
/// copy, plus K2's own shared suffixes): `shop.github.io` stays
/// `shop.github.io`, `a.brand.co.kr` is `brand.co.kr` (review L13).
/// `None`: `domain` is itself a public suffix.
pub fn registrable(domain: &str) -> Option<String> {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = d.split('.').collect();
    let suffix = k2_vault::hosts::public_suffix_labels(&d);
    if labels.len() <= suffix {
        return None;
    }
    Some(labels[labels.len() - suffix - 1..].join("."))
}

/// Reply-To: the sender's defaults, addresses on the sender's own
/// registrable domain, or anything when the App's scope is `any`.
fn reply_to(req: &Request, s: &Sender, scope_any: bool) -> R<Vec<String>> {
    if req.reply_to.is_empty() {
        return Ok(s.reply_to.clone());
    }
    let org = registrable(&s.domain);
    for a in &req.reply_to {
        let d = a.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
        let own = s.reply_to.contains(a) || d == s.domain || (org.is_some() && registrable(d) == org);
        if !own && !scope_any {
            return Err(fail(
                403,
                "recipient_out_of_scope",
                "replyTo may be the sender's default or an address on the sender's own domain (any address needs the owner's any scope)",
            ));
        }
    }
    Ok(req.reply_to.clone())
}

/// E16: is `address` one of this App's users who proved they read it?
pub fn proven_user(app: &str, address: &str) -> R<bool> {
    let a = address.trim().to_ascii_lowercase();
    let principals = k2_core::skin::list_principals().map_err(db_fail)?;
    let grants = k2_core::skin::list_grants().map_err(db_fail)?;
    for p in principals.iter().filter(|p| p.email.as_deref().is_some_and(|e| e.trim().eq_ignore_ascii_case(&a))) {
        let holds = grants.iter().any(|g| g.subject_kind == "principal" && g.subject_id == p.id && g.kind == "app" && g.target_id == app && g.enabled);
        if holds && k2_core::skin::invites::email_proven(&p.id, &a).map_err(db_fail)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn check_scope(app: &str, scope_any: bool, envelope: &[String]) -> R<()> {
    if scope_any {
        return Ok(());
    }
    for a in envelope {
        if !proven_user(app, a)? {
            return Err(fail(
                403,
                "recipient_out_of_scope",
                "this App emails only its own users whose address is confirmed (they used a link K2 mailed them). \
                 Other addresses need the owner's any scope (k2 app email on <app> --scope any)",
            ));
        }
    }
    Ok(())
}

fn row_json(r: &LogRow) -> J {
    let mut v = json!({
        "id": r.id,
        "status": r.status,
        "recipients": r.recipient_count,
        "createdAt": r.created_at,
        "updatedAt": r.updated_at,
    });
    if let Some(e) = &r.error_code {
        v["error"] = json!(e);
    }
    if let Some(t) = r.send_after {
        v["sendAt"] = json!(t);
    }
    v
}

/// The `email.send` answer.
fn accepted(r: &LogRow) -> J {
    json!({ "ok": true, "accepted": true, "id": r.id, "status": r.status, "recipients": r.recipient_count })
}

/// Hand a message to its sender's path. Returns the Message-ID.
#[allow(clippy::too_many_arguments)]
pub fn deliver(
    s: &Sender,
    log_id: &str,
    from_name: &str,
    to: &[String],
    cc: &[String],
    envelope: &[String],
    subject: &str,
    text: Option<&str>,
    html: Option<&str>,
    reply_to: &[String],
    headers: &BTreeMap<String, String>,
) -> R<String> {
    if super::airgap_blocks() {
        return Err(fail(503, "airgap", "air-gap is on: nothing leaves this server"));
    }
    let mid = crate::mail::send::new_message_id_for(&s.address);
    let m = compose::Message { from_name, from: &s.address, to, cc, subject, text, html, reply_to, headers, message_id: &mid };
    if s.path == store::PATH_RELAY {
        let relay = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::get_relay(&conn, s.relay_id.as_deref().unwrap_or("")).map_err(db_fail)?
        };
        let relay = relay.ok_or_else(|| fail(409, "relay_not_ready", "the sender's relay was removed"))?;
        let raw = compose::rfc822(&m, std::time::SystemTime::now()).map_err(|e| fail(500, "failed", e))?;
        super::relay::submit(&relay, &s.address, envelope, &raw).map_err(|f| upstream(log_id, f))?;
        return Ok(mid);
    }
    let account = s.stalwart_account_id.as_deref().filter(|a| !a.is_empty()).ok_or_else(|| fail(409, "domain_not_ready", "the sender has no mail-server account"))?;
    let engine = super::engine::mail_engine()?;
    engine
        .submit(&super::engine::HostedMail {
            account_id: account,
            from: &s.address,
            rcpt: envelope,
            email: compose::email_json(&m),
            params: compose::envelope_params(log_id),
            destroy_copy: true,
        })
        .map_err(|e| upstream(log_id, fail(502, "failed", e)))?;
    Ok(mid)
}

/// The words every caller sees when the relay or the mail server refuses
/// a hand-off (security review L7).
pub const UPSTREAM_HINT: &str = "the mail server didn't take the message; the owner sees why in the daemon log";

/// A relay's or the mail server's refusal: callers get the code and
/// [`UPSTREAM_HINT`], never the upstream text (it can name an account, a
/// quota or an internal host); the owner-side daemon log keeps it, with
/// anything address-shaped masked (E22).
fn upstream(log_id: &str, f: Fail) -> Fail {
    if f.status != 502 {
        return f;
    }
    let detail = mask_addresses(&f.hint);
    k2_core::log_debug!("[email-notify] hand-off of log={log_id} refused upstream: {detail}");
    fail(502, f.code, UPSTREAM_HINT)
}

/// Every whitespace-separated word holding an `@` becomes `[addr]`.
pub fn mask_addresses(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().map(|w| if w.contains('@') { "[addr]" } else { w }).collect();
    words.join(" ").chars().take(500).collect()
}

/// Mark a handed-off (or failed) row; the answer row.
fn finish(log_id: &str, from_status: &str, outcome: &R<String>, now: i64) -> R<LogRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match outcome {
        Ok(mid) => {
            store::set_log_submitted(&conn, log_id, from_status, mid, now).map_err(db_fail)?;
        }
        Err(f) => {
            store::set_log_status(&conn, log_id, from_status, store::ST_FAILED, Some(f.code), now).map_err(db_fail)?;
        }
    }
    store::get_log(&conn, log_id).map_err(db_fail)?.ok_or_else(|| db_fail("the log row vanished"))
}

// ── scheduled bodies (0600, deleted at hand-off) ─────────────────────

/// House rule: tests never touch the real `~/.k2`. One dir per test thread,
/// so the orphan sweep never sees another test's files (a uuid name, never
/// a ThreadId: `test_env::unique_temp_path`).
#[cfg(test)]
fn scheduled_dir() -> std::path::PathBuf {
    thread_local! {
        static DIR: std::path::PathBuf = k2_core::test_env::unique_temp_path("email-notify-test").join("scheduled");
    }
    DIR.with(|d| d.clone())
}

#[cfg(not(test))]
fn scheduled_dir() -> std::path::PathBuf {
    k2_core::paths::home_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join(".k2").join("email-notify").join("scheduled")
}

/// A server-made log id (`enl_` + lowercase hex): the only names the
/// scheduled directory holds.
fn log_id_ok(log_id: &str) -> bool {
    log_id.strip_prefix("enl_").is_some_and(|h| !h.is_empty() && h.len() <= 64 && h.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
}

pub(crate) fn scheduled_path(log_id: &str) -> Option<std::path::PathBuf> {
    log_id_ok(log_id).then(|| scheduled_dir().join(format!("{log_id}.json")))
}

/// The scheduled directory, made 0700 (and narrowed to 0700 if it exists
/// wider; review L12).
fn scheduled_dir_ready() -> Result<std::path::PathBuf, String> {
    let dir = scheduled_dir();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("scheduled dir: {e}"))?;
        let meta = std::fs::symlink_metadata(&dir).map_err(|e| format!("scheduled dir: {e}"))?;
        if !meta.is_dir() {
            return Err("scheduled dir isn't a directory".into());
        }
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("scheduled dir chmod: {e}"))?;
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir).map_err(|e| format!("scheduled dir: {e}"))?;
    Ok(dir)
}

/// The body waits in a file created 0600 from the start (never a wider
/// moment, review L12), renamed into place.
fn write_scheduled(log_id: &str, req: &Request, from_name: &str, reply_to: &[String]) -> Result<(), String> {
    use std::io::Write;
    scheduled_dir_ready()?;
    let path = scheduled_path(log_id).ok_or("scheduled: not a log id")?;
    let body = json!({ "request": req, "fromName": from_name, "replyTo": reply_to });
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp).map_err(|e| format!("scheduled write: {e}"))?;
    f.write_all(body.to_string().as_bytes()).map_err(|e| format!("scheduled write: {e}"))?;
    f.sync_all().map_err(|e| format!("scheduled write: {e}"))?;
    drop(f);
    std::fs::rename(&tmp, &path).map_err(|e| format!("scheduled write: {e}"))
}

/// Is the kept file one K2 wrote: a regular file (not a link), owned by
/// this process's user, readable by nobody else (review L12)?
fn trusted_file(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    if !meta.file_type().is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid has no preconditions and can't fail.
        let me = unsafe { libc::geteuid() };
        if meta.uid() != me || meta.permissions().mode() & 0o077 != 0 {
            return false;
        }
    }
    true
}

fn read_scheduled(log_id: &str) -> Option<(Request, String, Vec<String>)> {
    let path = scheduled_path(log_id)?;
    if !trusted_file(&path) {
        return None;
    }
    let raw = std::fs::read_to_string(&path).ok()?;
    let v: J = serde_json::from_str(&raw).ok()?;
    let req: Request = serde_json::from_value(v.get("request")?.clone()).ok()?;
    let name = v.get("fromName")?.as_str()?.to_string();
    let rt: Vec<String> = serde_json::from_value(v.get("replyTo").cloned()?).ok()?;
    Some((req, name, rt))
}

pub fn drop_scheduled(log_id: &str) {
    let Some(p) = scheduled_path(log_id) else { return };
    if std::fs::symlink_metadata(&p).is_ok() {
        if let Err(e) = std::fs::remove_file(&p) {
            k2_core::log_debug!("[email-notify] WARN couldn't delete scheduled body {log_id}: {e}");
        }
    }
}

/// Scheduled bodies whose row is no longer `scheduled` (or is gone: a
/// removed App user's send was cancelled, review L2) are deleted. Returns
/// how many.
pub fn sweep_scheduled_orphans() -> usize {
    let Ok(rd) = std::fs::read_dir(scheduled_dir()) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json").or_else(|| name.strip_suffix(".tmp")) else { continue };
        if !log_id_ok(id) {
            continue;
        }
        let live = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            match store::get_log(&conn, id) {
                Ok(r) => r.is_some_and(|r| r.status == store::ST_SCHEDULED),
                // Unreadable: keep it (the next tick decides).
                Err(_) => true,
            }
        };
        if !live && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// A cancelled held send (its recipient was removed, H1) leaves the
/// owner's approvals queue: denied, its kept body scrubbed. Returns how
/// many were withdrawn.
pub fn withdraw_cancelled_held(now: i64) -> usize {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::cancelled_held(&conn, 100).unwrap_or_default()
    };
    let ob_store = crate::mail::send::DbOutboundStore::default();
    let mut n = 0;
    for (_, ob) in rows {
        let pending = matches!(crate::mail::send::OutboundStore::load(&ob_store, &ob), Ok(Some(r)) if r.status == "pending");
        if pending && crate::mail::send::deny(&ob_store, &ob, "system:recipient-removed", "the recipient is no longer one of the App's users", now).is_ok() {
            n += 1;
        }
    }
    n
}

// ── the App door ─────────────────────────────────────────────────────

/// `email.send` for `caller` (§5.2). `v` is the request body (any `app`
/// field already removed by the route).
pub fn send(caller: &Caller, v: &J, now: i64) -> R<J> {
    let app = caller.app().to_string();
    // 1. The box switch and air-gap.
    if !super::box_enabled() {
        let f = fail(403, "email_off", "email notifications are off on this server (K2_EMAIL_NOTIFICATIONS=off)");
        log_refusal(&app, "", &[], &f, now);
        return Err(f);
    }
    if super::airgap_blocks() {
        let f = fail(503, "airgap", "air-gap is on: nothing leaves this server");
        log_refusal(&app, "", &[], &f, now);
        return Err(f);
    }
    // Composition first: the recipients name the refused row.
    let req = match compose::parse(v, now) {
        Ok(r) => r,
        Err(f) => {
            log_refusal(&app, "", &[], &f, now);
            return Err(f);
        }
    };
    let envelope = req.envelope();
    let refuse = |sender_id: &str, f: Fail| -> Fail {
        log_refusal(&app, sender_id, &envelope, &f, now);
        f
    };
    // 3. On, with a granted sender.
    let (st, s) = app_sender(&app).map_err(|f| refuse("", f))?;
    // ... and the cap approved (§5.2 step 3, review M2).
    if let Caller::App { .. } = caller {
        cap_ok(&app).map_err(|f| refuse(&s.id, f))?;
    }
    check_from(req.from.as_deref(), &s).map_err(|f| refuse(&s.id, f))?;
    // 4. The sender may send.
    super::senders::ready(&s, now).map_err(|f| refuse(&s.id, f))?;
    let scope_any = st.recipient_scope == store::SCOPE_ANY;
    // 5. Name, Reply-To, scope.
    let name = from_name(&req, &s).map_err(|f| refuse(&s.id, f))?;
    let rt = reply_to(&req, &s, scope_any).map_err(|f| refuse(&s.id, f))?;
    check_scope(&app, scope_any, &envelope).map_err(|f| refuse(&s.id, f))?;
    // 8 (decided before anything is written): the agent's gate.
    let gate = match caller {
        Caller::Agent { project_path, .. } => {
            let g = crate::mail::send::gate_for_path(project_path).map_err(|_| refuse(&s.id, fail(500, "failed", "the agent's send setting is unreadable (fail-closed)")))?;
            match g {
                crate::mail::send::Gate::Off => {
                    return Err(refuse(&s.id, fail(403, "gated", crate::mail::send::agent_send_off_hint())));
                }
                crate::mail::send::Gate::Approval if req.send_at.is_some() => {
                    return Err(refuse(&s.id, fail(400, "usage", "a send held for approval can't be scheduled; drop sendAt")));
                }
                g => g,
            }
        }
        _ => crate::mail::send::Gate::On,
    };
    let warmup = super::budget::warming_up(&s, now).then_some(s.domain.as_str());
    // 6 + 7 + 9 under one lock: idempotency, budgets, the row.
    let digest = req.digest();
    let status = if gate == crate::mail::send::Gate::Approval {
        store::ST_HELD
    } else if req.send_at.is_some() {
        store::ST_SCHEDULED
    } else {
        store::ST_QUEUED
    };
    let subject_kept = crate::mail::send::hide_links(&req.subject);
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(key) = req.idempotency_key.as_deref() {
            if let Some((sha, log_id)) = store::idempotency_get(&conn, &app, key, now - store::IDEMPOTENCY_SECS).map_err(db_fail)? {
                if sha != digest {
                    return Err(fail(409, "idempotency_conflict", "this idempotencyKey was used in the last 24 h for a different email"));
                }
                if let Some(r) = store::get_log(&conn, &log_id).map_err(db_fail)? {
                    return Ok(accepted(&r));
                }
            }
        }
        // M1: counted in the window it is sent in; a scheduled send also
        // fits what the App may keep waiting.
        let kind = super::budget::Kind::App { app: &app, scope_any, warmup_domain: warmup };
        super::budget::check(&kind, envelope.len(), req.send_at.unwrap_or(now)).map_err(|f| refuse(&s.id, f))?;
        if status == store::ST_SCHEDULED {
            super::budget::check_pending(&kind, envelope.len()).map_err(|f| refuse(&s.id, f))?;
        }
        let tx = conn.unchecked_transaction().map_err(db_fail)?;
        let id = store::insert_log(
            &tx,
            &store::NewLog {
                sender_id: &s.id,
                origin: caller.origin(),
                app_service_id: Some(&app),
                purpose: None,
                to: &envelope,
                subject: &subject_kept,
                tags: &req.tags,
                status,
                error_code: None,
                message_id: None,
                send_after: req.send_at,
                scope_any,
                now,
            },
        )
        .map_err(db_fail)?;
        if let Some(key) = req.idempotency_key.as_deref() {
            store::idempotency_put(&tx, &app, key, &digest, &id, now).map_err(db_fail)?;
        }
        tx.commit().map_err(db_fail)?;
        store::get_log(&conn, &id).map_err(db_fail)?.ok_or_else(|| db_fail("the log row vanished"))?
    };
    super::audit_line("email-send", caller.actor(), &format!("app={app} log={} status={} recipients={}", row.id, row.status, row.recipient_count));
    match status {
        store::ST_HELD => {
            let Caller::Agent { project_id, actor, .. } = caller else { return Err(db_fail("held without an agent")) };
            hold(&row, &s, &req, &name, &rt, project_id, actor, now)
        }
        store::ST_SCHEDULED => {
            if let Err(e) = write_scheduled(&row.id, &req, &name, &rt) {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = store::set_log_status(&conn, &row.id, store::ST_SCHEDULED, store::ST_FAILED, Some("failed"), now);
                return Err(fail(500, "failed", format!("K2 couldn't keep the scheduled email: {e}")));
            }
            Ok(accepted(&row))
        }
        _ => {
            let out = deliver(&s, &row.id, &name, &req.to, &req.cc, &envelope, &req.subject, req.text.as_deref(), req.html.as_deref(), &rt, &req.headers);
            let done = finish(&row.id, store::ST_QUEUED, &out, now)?;
            match out {
                Ok(_) => Ok(accepted(&done)),
                Err(f) => Err(f),
            }
        }
    }
}

/// E15: the App's agent's send under `approval` waits in the approvals
/// queue (`mail_outbound` pending, From = the sender address).
#[allow(clippy::too_many_arguments)]
fn hold(row: &LogRow, s: &Sender, req: &Request, name: &str, rt: &[String], project_id: &str, actor: &str, now: i64) -> R<J> {
    use crate::mail::send::{NewOutbound, NotifyHeld, OutboundMessage, OutboundStore};
    let msg = OutboundMessage {
        from_name: Some(name.to_string()),
        from: s.address.clone(),
        to: req.to.clone(),
        cc: req.cc.clone(),
        bcc: req.bcc.clone(),
        policy_bcc: Vec::new(),
        subject: req.subject.clone(),
        text_body: req.text.clone().unwrap_or_default(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        // Review M3: the kept copy is scrubbed once decided (links hidden,
        // bodies blanked), so no live one-time link stays at rest.
        ephemeral: true,
        notify: Some(NotifyHeld {
            log_id: row.id.clone(),
            app: row.app_service_id.clone().unwrap_or_default(),
            html: req.html.clone(),
            reply_to: rt.to_vec(),
            headers: req.headers.clone(),
        }),
    };
    let store_ob = crate::mail::send::DbOutboundStore::default();
    let inserted = store_ob.insert(&NewOutbound {
        owner_project_id: project_id,
        agent_name: actor,
        message: &msg,
        status: "pending",
        decided_by: None,
        attachment_names: &[],
        send_after: None,
        now,
    });
    let db = k2_core::db::shared();
    let conn = db.lock();
    match inserted {
        Ok(ob) => {
            store::set_log_outbound(&conn, &row.id, &ob).map_err(db_fail)?;
            let r = store::get_log(&conn, &row.id).map_err(db_fail)?.ok_or_else(|| db_fail("the log row vanished"))?;
            let mut v = accepted(&r);
            v["outboundId"] = json!(ob);
            v["hint"] = json!("held for the owner's approval (k2 mail approvals)");
            Ok(v)
        }
        Err(e) => {
            let _ = store::set_log_status(&conn, &row.id, store::ST_HELD, store::ST_FAILED, Some("failed"), now);
            Err(fail(500, "failed", format!("K2 couldn't hold the email for approval: {e}")))
        }
    }
}

/// The approvals resolver's half of E15: when an approved message's From
/// is a notification sender, it leaves through here (never the mailbox
/// path). `None` = not a notification.
pub fn held_submitter(msg: &crate::mail::send::OutboundMessage) -> Option<Box<dyn crate::mail::submit::MailSubmit + 'static>> {
    let held = msg.notify.clone()?;
    Some(Box::new(HeldSubmit { held }))
}

/// Is `from` a notification sender (an approval of it is the owner's)?
pub fn is_sender_address(from: &str) -> bool {
    super::fence::reserved(from)
}

struct HeldSubmit {
    held: crate::mail::send::NotifyHeld,
}

impl crate::mail::submit::MailSubmit for HeldSubmit {
    fn backend(&self) -> &'static str {
        "notification"
    }
    fn submit(
        &self,
        msg: &crate::mail::send::OutboundMessage,
        attachments: &[crate::mail::external_smtp::OutAttachment],
    ) -> Result<crate::mail::submit::Receipt, crate::mail::submit::SubmitError> {
        use crate::mail::submit::{Receipt, SubmitError};
        if !attachments.is_empty() {
            return Err(SubmitError::Usage { code: "usage", hint: "notifications carry no attachments".into() });
        }
        let now = super::now();
        let (s, row) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = store::get_log(&conn, &self.held.log_id).map_err(SubmitError::Engine)?.ok_or_else(|| SubmitError::Engine("its log row is gone".into()))?;
            // A cancelled row (its recipient was removed, H1) never leaves.
            if row.status != store::ST_HELD {
                return Err(SubmitError::Engine(format!("this email is {} now; it won't be sent", row.status)));
            }
            let s = store::get_sender(&conn, &row.sender_id).map_err(SubmitError::Engine)?;
            match s {
                Some(s) if s.is_live() && s.address.eq_ignore_ascii_case(&msg.from) && s.app() == Some(self.held.app.as_str()) => (s, row),
                _ => return Err(SubmitError::Engine("the App's sender changed since it was held".into())),
            }
        };
        if !super::box_enabled() {
            return Err(SubmitError::Engine("email notifications are off on this server".into()));
        }
        super::senders::ready(&s, now).map_err(|f| SubmitError::Engine(f.hint))?;
        let envelope = msg.all_recipients();
        // An App's email.send (not K2's invite text) re-checks the scope the
        // App has now: narrowed to users after it was held, it doesn't leave.
        if row.purpose.is_none() {
            let (st, _) = app_sender(&self.held.app).map_err(|f| SubmitError::Engine(f.hint))?;
            check_scope(&self.held.app, st.recipient_scope == store::SCOPE_ANY, &envelope).map_err(|f| SubmitError::Engine(f.hint))?;
        }
        let text = (!msg.text_body.is_empty()).then_some(msg.text_body.as_str());
        let out = deliver(
            &s,
            &self.held.log_id,
            msg.from_name.as_deref().unwrap_or(&s.from_name),
            &msg.to,
            &msg.cc,
            &envelope,
            &msg.subject,
            text,
            self.held.html.as_deref(),
            &self.held.reply_to,
            &self.held.headers,
        );
        let _ = finish(&self.held.log_id, store::ST_HELD, &out, now);
        out.map_err(|f| SubmitError::Engine(f.hint))?;
        Ok(Receipt { to: msg.to.clone(), cc: msg.cc.clone(), subject: msg.subject.clone(), attachments: Vec::new() })
    }
    fn reply(
        &self,
        _source_email_id: &str,
        msg: &crate::mail::send::OutboundMessage,
        attachments: &[crate::mail::external_smtp::OutAttachment],
    ) -> Result<crate::mail::submit::Receipt, crate::mail::submit::SubmitError> {
        self.submit(msg, attachments)
    }
    fn reply_preview(&self, _source_email_id: &str) -> Result<Option<(String, String)>, crate::mail::submit::SubmitError> {
        Err(crate::mail::submit::SubmitError::Engine("a notification is never a reply".into()))
    }
}

// ── the scheduled flusher ────────────────────────────────────────────

/// How long a due send waits when it doesn't fit its budget yet (M1).
pub const DEFER_SECS: i64 = 300;

/// The flusher's tick: due sends, then the clean-up of bodies and
/// approvals entries a removed App user left (H1, L2).
pub fn tick(now: i64) -> usize {
    let sent = flush_due(now, 50);
    let orphans = sweep_scheduled_orphans();
    let withdrawn = withdraw_cancelled_held(now);
    if orphans > 0 || withdrawn > 0 {
        k2_core::log_debug!("[email-notify] removed {orphans} kept scheduled body file(s), withdrew {withdrawn} held approval(s)");
    }
    sent
}

/// Send what's due. Every check runs again at the time it is sent
/// (§5.2 step 9; security review M1 and L12): the kept file must be K2's
/// (0600, this user, no link) and still pass the composition rules; the
/// App on with the same sender and its cap approved; the sender ready;
/// the From name, Reply-To and recipients under the App's scope *now*;
/// and the budget of the window it actually leaves in. A send that
/// doesn't fit its budget waits ([`DEFER_SECS`]) and fails `rate_limited`
/// a day past the furthest `sendAt`. Returns how many were handed off.
pub fn flush_due(now: i64, limit: usize) -> usize {
    let due = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::due_scheduled(&conn, now, limit) {
            Ok(d) => d,
            Err(e) => {
                k2_core::log_debug!("[email-notify] WARN scheduled list: {e}");
                return 0;
            }
        }
    };
    let mut sent = 0;
    for row in due {
        let fail_row = |code: &'static str| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::set_log_status(&conn, &row.id, store::ST_SCHEDULED, store::ST_FAILED, Some(code), now);
            drop(conn);
            drop_scheduled(&row.id);
        };
        let Some((req, _kept_name, _kept_rt)) = read_scheduled(&row.id) else {
            fail_row("lost");
            continue;
        };
        if let Err(f) = compose::recheck(&req, now) {
            fail_row(f.code);
            continue;
        }
        let app = row.app_service_id.clone().unwrap_or_default();
        let (st, s) = match app_sender(&app) {
            Ok((st, s)) if s.id == row.sender_id => (st, s),
            Ok(_) => {
                fail_row("sender_not_granted");
                continue;
            }
            Err(f) => {
                fail_row(f.code);
                continue;
            }
        };
        if !super::box_enabled() {
            fail_row("email_off");
            continue;
        }
        if row.origin == store::ORIGIN_APP {
            if let Err(f) = cap_ok(&app) {
                fail_row(f.code);
                continue;
            }
        }
        if let Err(f) = super::senders::ready(&s, now) {
            fail_row(f.code);
            continue;
        }
        let scope_any = st.recipient_scope == store::SCOPE_ANY;
        let envelope = req.envelope();
        let checked = from_name(&req, &s).and_then(|n| reply_to(&req, &s, scope_any).map(|rt| (n, rt))).and_then(|nr| check_scope(&app, scope_any, &envelope).map(|_| nr));
        let (name, rt) = match checked {
            Ok(nr) => nr,
            Err(f) => {
                fail_row(f.code);
                continue;
            }
        };
        // The budget of the window it leaves in: an overdue row moves to
        // now first, then is re-counted (it's already in the sum: n = 0).
        if row.send_after.is_some_and(|t| t < now) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            if !matches!(store::reschedule(&conn, &row.id, now, now), Ok(true)) {
                continue;
            }
        }
        let warmup = super::budget::warming_up(&s, now).then_some(s.domain.as_str());
        let kind = super::budget::Kind::App { app: &app, scope_any: row.scope_any || scope_any, warmup_domain: warmup };
        if let Err(f) = super::budget::check(&kind, 0, now) {
            if f.code == "rate_limited" && now - row.created_at <= compose::MAX_SCHEDULE_SECS + super::budget::DAY {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = store::reschedule(&conn, &row.id, now + DEFER_SECS, now);
            } else {
                fail_row(f.code);
            }
            continue;
        }
        let out = deliver(&s, &row.id, &name, &req.to, &req.cc, &envelope, &req.subject, req.text.as_deref(), req.html.as_deref(), &rt, &req.headers);
        if out.is_ok() {
            sent += 1;
        }
        let _ = finish(&row.id, store::ST_SCHEDULED, &out, now);
        drop_scheduled(&row.id);
    }
    sent
}

// ── K2's own mail (E28, E27, E14) ────────────────────────────────────

/// One K2-composed message (fixed text, never an App's template when it
/// rides the system sender, E14).
#[derive(Debug, Clone)]
pub struct Composed {
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
}

/// Which sender K2's mail for `app` (or the box) uses (Q2): the App's
/// granted sender when its email is on, else the system sender. `None`:
/// no notification path (the caller keeps today's answer). With the box
/// switch off it is always `None`, so every caller answers exactly as
/// before this feature (security review L9).
pub fn sender_for(app: Option<&str>) -> R<Option<Sender>> {
    if !super::box_enabled() {
        return Ok(None);
    }
    if let Some(a) = app.filter(|a| !a.is_empty() && *a != k2_core::skin::HOST_APP_ID) {
        if let Ok((_, s)) = app_sender(a) {
            return Ok(Some(s));
        }
    }
    super::senders::system()
}

/// The App's granted sender when its email is on (invite_sender's E27
/// swap); never the system sender.
pub fn app_sender_if_on(app: &str) -> Option<Sender> {
    if app.is_empty() || app == k2_core::skin::HOST_APP_ID || !super::box_enabled() {
        return None;
    }
    app_sender(app).ok().map(|(_, s)| s)
}

/// [`system_send`] from a sender the caller already picked
/// ([`sender_for`]).
pub fn system_send_from(s: &Sender, purpose: &str, app: Option<&str>, to: &str, composed: &Composed) -> R<String> {
    send_fixed(s, store::ORIGIN_SYSTEM, purpose, app, to, composed, &super::budget::Kind::System)
}

/// E28: send one K2 message for `purpose` (`owner_reset`, `email_code`,
/// `password_changed`…) to `to`, from [`sender_for`]. The subject is not
/// kept (empty in the log), the stored copy is destroyed, the system
/// budget applies (no warm-up). Returns the log id.
pub fn system_send(purpose: &str, to: &str, composed: &Composed, app: Option<&str>) -> R<String> {
    let s = sender_for(app)?.ok_or_else(|| fail(409, "no_mail_path", "this server has no system sender (k2 notifications email system-sender set)"))?;
    send_fixed(&s, store::ORIGIN_SYSTEM, purpose, app, to, composed, &super::budget::Kind::System)
}

/// Send K2-composed text from `s` (system mail, or an App's invite from
/// its own sender, E27). The caller decided who may.
pub fn send_fixed(s: &Sender, origin: &str, purpose: &str, app: Option<&str>, to: &str, c: &Composed, kind: &super::budget::Kind<'_>) -> R<String> {
    let now = super::now();
    if !super::box_enabled() {
        return Err(fail(403, "email_off", "email notifications are off on this server"));
    }
    if super::airgap_blocks() {
        return Err(fail(503, "airgap", "air-gap is on: nothing leaves this server"));
    }
    let to = vec![compose::strict_address(to).map_err(|_| fail(400, "usage", "the recipient isn't a valid address"))?];
    if c.subject.chars().any(char::is_control) || c.subject.len() > compose::MAX_SUBJECT_BYTES || c.text.len() > compose::MAX_TEXT_BYTES {
        return Err(fail(400, "usage", "the message doesn't fit the composition limits"));
    }
    super::senders::ready(s, now)?;
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        super::budget::check(kind, to.len(), now)?;
        store::insert_log(
            &conn,
            &store::NewLog {
                sender_id: &s.id,
                origin,
                app_service_id: app,
                purpose: Some(purpose),
                to: &to,
                subject: "",
                tags: &[],
                status: store::ST_QUEUED,
                error_code: None,
                message_id: None,
                send_after: None,
                scope_any: false,
                now,
            },
        )
        .map_err(db_fail)?
    };
    let none = BTreeMap::new();
    let out = deliver(s, &id, &s.from_name, &to, &[], &to, &c.subject, Some(&c.text), c.html.as_deref(), &[], &none);
    finish(&id, store::ST_QUEUED, &out, now)?;
    k2_core::log_debug!("[email-notify] {origin} mail {purpose} log={id} ok={}", out.is_ok());
    out.map(|_| id)
}

/// E27: an App's invite or reset from the App's own sender, held for
/// approval when the sending agent's gate says so (`mail_outbound` pending
/// with From = the sender, E15). `Ok(Err(outbound id))` = held.
#[allow(clippy::too_many_arguments)]
pub fn app_link_mail(
    s: &Sender,
    app: &str,
    purpose: &str,
    to: &str,
    c: &Composed,
    gate: crate::mail::send::Gate,
    project_id: &str,
    actor: &str,
) -> R<Result<String, String>> {
    let now = super::now();
    let warmup = super::budget::warming_up(s, now).then_some(s.domain.as_str());
    let kind = super::budget::Kind::App { app, scope_any: false, warmup_domain: warmup };
    match gate {
        crate::mail::send::Gate::Off => Err(fail(403, "gated", crate::mail::send::agent_send_off_hint())),
        crate::mail::send::Gate::On => send_fixed(s, store::ORIGIN_APP, purpose, Some(app), to, c, &kind).map(Ok),
        crate::mail::send::Gate::Approval => {
            if !super::box_enabled() {
                return Err(fail(403, "email_off", "email notifications are off on this server"));
            }
            super::senders::ready(s, now)?;
            let to_list = vec![compose::strict_address(to).map_err(|_| fail(400, "usage", "the recipient isn't a valid address"))?];
            let row = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                super::budget::check(&kind, 1, now)?;
                let id = store::insert_log(
                    &conn,
                    &store::NewLog {
                        sender_id: &s.id,
                        origin: store::ORIGIN_AGENT,
                        app_service_id: Some(app),
                        purpose: Some(purpose),
                        to: &to_list,
                        subject: "",
                        tags: &[],
                        status: store::ST_HELD,
                        error_code: None,
                        message_id: None,
                        send_after: None,
                        scope_any: false,
                        now,
                    },
                )
                .map_err(db_fail)?;
                store::get_log(&conn, &id).map_err(db_fail)?.ok_or_else(|| db_fail("the log row vanished"))?
            };
            let req = Request {
                from: None,
                from_name: None,
                to: to_list,
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: c.subject.clone(),
                text: Some(c.text.clone()),
                html: c.html.clone(),
                reply_to: Vec::new(),
                headers: BTreeMap::new(),
                tags: Vec::new(),
                idempotency_key: None,
                send_at: None,
            };
            let v = hold(&row, s, &req, &s.from_name, &[], project_id, actor, now)?;
            let ob = v["outboundId"].as_str().unwrap_or_default().to_string();
            Ok(Err(ob))
        }
    }
}

// ── reads ────────────────────────────────────────────────────────────

/// `email.status`: one of this App's rows.
pub fn status(app: &str, id: &str) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::get_log(&conn, id.trim()).map_err(db_fail)? {
        // Review L8: K2's own mail about the App's users (the owner's resets)
        // isn't the App's to see.
        Some(r) if r.app_service_id.as_deref() == Some(app) && r.origin != store::ORIGIN_SYSTEM => {
            let mut v = row_json(&r);
            v["ok"] = json!(true);
            Ok(v)
        }
        _ => Err(fail(404, "not_found", "no email of this App by that id (the log keeps 30 days)")),
    }
}

/// `email.log`: this App's rows, newest first (metadata only).
pub fn log(app: &str, limit: Option<usize>, before: Option<i64>, status: Option<&str>, tag: Option<&str>) -> R<J> {
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_log(&conn, &store::LogFilter { app: Some(app), status, tag, before, limit: limit + 1, without_system: true }).map_err(db_fail)?
    };
    let more = rows.len() > limit;
    let items: Vec<J> = rows
        .iter()
        .take(limit)
        .map(|r| {
            let mut v = json!({
                "id": r.id,
                "to": r.to,
                "subject": r.subject,
                "tags": r.tags,
                "status": r.status,
                "origin": r.origin,
                "createdAt": r.created_at,
            });
            // Absent rather than null (the catalog's shape).
            if let J::String(f) = sender_address(&r.sender_id) {
                v["from"] = json!(f);
            }
            if let Some(e) = &r.error_code {
                v["error"] = json!(e);
            }
            if let Some(p) = &r.purpose {
                v["purpose"] = json!(p);
            }
            v
        })
        .collect();
    let mut v = json!({ "ok": true, "items": items });
    if more {
        if let Some(last) = rows.get(limit - 1) {
            v["next"] = json!(last.created_at);
        }
    }
    Ok(v)
}

fn sender_address(id: &str) -> J {
    if id.is_empty() {
        return J::Null;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::get_sender(&conn, id) {
        Ok(Some(s)) => json!(s.address),
        _ => J::Null,
    }
}
