//! Agent-scheduled outbound delivery: parse `--at`/`--in`, flush due rows.
use super::send::{
    self, DbOutboundStore, Gate, OutboundStore, SendError, SubmitBackend,
    DEFAULT_SCHEDULED_TICK_SECS, MAX_SCHEDULE_SECS,
};

/// Parse a relative duration for `--in`: `30m`, `2h`, `1d` (and optional `s`).
pub fn parse_relative_duration(raw: &str) -> Result<i64, SendError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(SendError::Usage(
            "empty --in duration — examples: 30m, 2h, 1d (max 30d)".to_string(),
        ));
    }
    let (num_str, unit) = match s.chars().last() {
        Some(u) if u.is_ascii_alphabetic() => (&s[..s.len() - 1], u.to_ascii_lowercase()),
        _ => {
            return Err(SendError::Usage(format!(
                "invalid --in '{raw}' — use <n><unit> with unit s|m|h|d (e.g. 30m, 2h, 1d)"
            )))
        }
    };
    let n: i64 = num_str.trim().parse().map_err(|_| {
        SendError::Usage(format!(
            "invalid --in '{raw}' — need a positive integer before the unit (e.g. 30m)"
        ))
    })?;
    if n <= 0 {
        return Err(SendError::Usage(format!(
            "invalid --in '{raw}' — duration must be positive"
        )));
    }
    let secs = match unit {
        's' => n,
        'm' => n.saturating_mul(60),
        'h' => n.saturating_mul(3_600),
        'd' => n.saturating_mul(86_400),
        _ => {
            return Err(SendError::Usage(format!(
                "invalid --in unit '{unit}' — use s, m, h, or d (e.g. 30m, 2h, 1d)"
            )))
        }
    };
    if secs > MAX_SCHEDULE_SECS {
        return Err(SendError::Usage(format!(
            "--in '{raw}' exceeds the 30-day schedule cap — use a shorter delay"
        )));
    }
    Ok(secs)
}

/// Parse `--at` RFC3339/ISO-8601 with offset. Hard-rejects past / >30d.
pub fn parse_at_timestamp(raw: &str, now: i64) -> Result<i64, SendError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(SendError::Usage(
            "empty --at timestamp — use RFC3339 with offset, e.g. 2026-07-14T09:30:00-06:00"
                .to_string(),
        ));
    }
    let dt = chrono::DateTime::parse_from_rfc3339(s).map_err(|_| {
        SendError::Usage(format!(
            "invalid --at '{raw}' — need RFC3339/ISO-8601 with offset \
             (e.g. 2026-07-14T09:30:00-06:00 or 2026-07-14T15:30:00Z)"
        ))
    })?;
    let ts = dt.timestamp();
    if ts <= now {
        return Err(SendError::Usage(format!(
            "--at '{raw}' is in the past — refuse to schedule; pick a future time"
        )));
    }
    if ts - now > MAX_SCHEDULE_SECS {
        return Err(SendError::Usage(format!(
            "--at '{raw}' is more than 30 days ahead — use a closer time"
        )));
    }
    Ok(ts)
}

pub fn resolve_send_after(
    send_at: Option<&str>,
    send_in: Option<&str>,
    now: i64,
) -> Result<Option<i64>, SendError> {
    match (
        send_at.map(str::trim).filter(|s| !s.is_empty()),
        send_in.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(_), Some(_)) => Err(SendError::Usage(
            "pass only one of --at <when> or --in <duration>, not both".to_string(),
        )),
        (Some(at), None) => Ok(Some(parse_at_timestamp(at, now)?)),
        (None, Some(rel)) => Ok(Some(now.saturating_add(parse_relative_duration(rel)?))),
        (None, None) => Ok(None),
    }
}

pub fn format_send_after(ts: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| ts.to_string())
}

pub fn cancel_scheduled(
    store: &dyn OutboundStore,
    id: &str,
    owner_project_id: &str,
    cancelled_by: &str,
    now: i64,
) -> Result<(), SendError> {
    let row = store
        .load(id)
        .map_err(|e| SendError::Engine(format!("outbound state unreadable — refusing: {e}")))?;
    let Some(row) = row else {
        return Err(SendError::NotFound(format!("no outbound message '{id}'")));
    };
    if row.owner_project_id != owner_project_id {
        return Err(SendError::NotFound(format!(
            "no outbound message '{id}' in this workspace"
        )));
    }
    if row.status != "scheduled" {
        return Err(SendError::Conflict(format!(
            "outbound '{id}' is {} — cancel applies to scheduled messages only",
            send::wire_status(&row.status)
        )));
    }
    let note = format!("cancelled by {cancelled_by} before send_after");
    let flipped = store
        .transition(id, "scheduled", "denied", Some(cancelled_by), Some(&note), now)
        .map_err(|e| SendError::Engine(format!("cancel transition failed: {e}")))?;
    if !flipped {
        return Err(SendError::Conflict(format!(
            "outbound '{id}' is no longer scheduled — it may have already flushed"
        )));
    }
    send::scrub(store, id);
    Ok(())
}

pub fn list_due_scheduled(now: i64, limit: usize) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT id FROM mail_outbound WHERE status = 'scheduled' \
         AND send_after IS NOT NULL AND send_after <= ?1 \
         ORDER BY send_after, id LIMIT ?2",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![now, limit as i64], |r| r.get::<_, String>(0))
        .map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// What a scheduled message must obey when it leaves. The owner may have
/// changed either since it was scheduled, so both are read at send time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushRules {
    pub gate: Gate,
    pub policy_bcc: Vec<String>,
    /// Why this workspace may no longer send from this address, if it may not:
    /// its send level was lowered, the mailbox went, or the domain can't send.
    pub refused: Option<String>,
}

/// The sending workspace's rules now, for this From. Any failure to read them is
/// an `Err`, and a message whose rules can't be read doesn't leave (fail-closed).
pub fn flush_rules_for(project_id: &str, from: &str) -> Result<FlushRules, String> {
    let path: String = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("the sending workspace is gone: {e}"))?
    };
    let reason = |e: SendError| format!("{e:?}");
    let refused = if crate::mail::access::can_send(project_id, from).is_err() {
        Some("this workspace can't send from this address any more".to_string())
    } else {
        match send::check_domain_send_mode(&crate::mail::secrets::FileSecretStore::default(), from) {
            Ok(()) => None,
            Err(SendError::SendMode(hint)) | Err(SendError::Engine(hint)) => Some(hint),
            Err(e) => Some(format!("{e:?}")),
        }
    };
    Ok(FlushRules {
        gate: send::gate_for_path(&path).map_err(reason)?,
        policy_bcc: send::policy_bcc_for_path(&path).map_err(reason)?,
        refused,
    })
}

/// What a flush did.
#[derive(Debug, PartialEq, Eq)]
pub enum FlushOutcome {
    /// Handed to the mail server.
    Sent,
    /// Nothing left the box; the row is `failed` with the reason in its note.
    Failed(String),
    /// Scheduled while sending was on, and sending now needs approval: the
    /// row is `pending`, for a person to decide.
    HeldForApproval,
}

/// Send one due scheduled message, under the rules in force NOW:
/// - sending off, send rights gone, a domain that can't send, or rules that
///   can't be read: the row fails, nothing leaves;
/// - sending now needs approval: a row scheduled under `on` goes to a person.
///   One a person already approved keeps that approval;
/// - the owner's always-BCC as it is now rides, and the recipient cap holds.
pub fn flush_one_scheduled(
    store: &dyn OutboundStore,
    backend: Result<&dyn SubmitBackend, String>,
    account_id_for_from: &mut dyn FnMut(&str) -> Result<String, String>,
    rules_for: &mut dyn FnMut(&str, &str) -> Result<FlushRules, String>,
    id: &str,
    now: i64,
) -> Result<Option<FlushOutcome>, SendError> {
    let row = store
        .load(id)
        .map_err(|e| SendError::Engine(format!("outbound state unreadable: {e}")))?;
    let Some(row) = row.filter(|r| r.status == "scheduled") else {
        return Ok(None);
    };
    // Not sent, from `scheduled`: only if this flush still holds the row.
    let refuse = |store: &dyn OutboundStore, reason: &str| -> Result<Option<FlushOutcome>, SendError> {
        let flipped = store
            .transition(id, "scheduled", "failed", None, None, now)
            .map_err(|e| SendError::Engine(format!("schedule flush transition failed: {e}")))?;
        if !flipped {
            return Ok(None);
        }
        let _ = store.append_note(id, reason, now);
        send::scrub(store, id);
        Ok(Some(FlushOutcome::Failed(reason.to_string())))
    };
    let rules = match rules_for(&row.owner_project_id, &row.from_address) {
        Ok(r) => r,
        Err(e) => return refuse(store, &format!("not sent: this workspace's sending rules can't be read ({e})")),
    };
    if rules.gate == Gate::Off {
        return refuse(store, "not sent: sending was turned off for this workspace before its send time");
    }
    if let Some(why) = &rules.refused {
        return refuse(store, &format!("not sent: {why}"));
    }
    let mut msg = match store.load_message(id) {
        Ok(m) => m,
        Err(e) => return refuse(store, &format!("not sent: stored message unreadable: {e}")),
    };
    if msg.policy_bcc != rules.policy_bcc {
        send::apply_policy_bcc(&mut msg, rules.policy_bcc);
        if let Err(SendError::Usage(h)) = send::check_recipient_limit(&msg) {
            return refuse(store, &format!("not sent: the owner's always-BCC policy changed. {h}"));
        }
        if let Err(e) = store.update_blind_copies(id, &msg) {
            return refuse(store, &format!("not sent: the owner's always-BCC policy couldn't be applied: {e}"));
        }
    }
    let by_policy = row.decided_by.as_deref().map_or(true, |d| d.starts_with("policy:"));
    if rules.gate == Gate::Approval && by_policy {
        let flipped = store
            .transition(
                id,
                "scheduled",
                "pending",
                None,
                Some("scheduled while sending was on; sending now needs approval"),
                now,
            )
            .map_err(|e| SendError::Engine(format!("schedule flush transition failed: {e}")))?;
        return Ok(flipped.then_some(FlushOutcome::HeldForApproval));
    }
    let flipped = store
        .transition(id, "scheduled", "approved", None, None, now)
        .map_err(|e| SendError::Engine(format!("schedule flush transition failed: {e}")))?;
    if !flipped {
        return Ok(None);
    }
    let fail = |store: &dyn OutboundStore, reason: &str| {
        let _ = store.transition(id, "approved", "failed", None, None, now);
        let _ = store.append_note(id, &format!("submit failed: {reason}"), now);
        send::scrub(store, id);
        Ok(Some(FlushOutcome::Failed(reason.to_string())))
    };
    let backend = match backend {
        Ok(b) => b,
        Err(e) => return fail(store, &format!("mail server unavailable: {e}")),
    };
    let account_id = match account_id_for_from(&msg.from) {
        Ok(a) => a,
        Err(e) => return fail(store, &format!("sender address unavailable: {e}")),
    };
    match backend.submit(&account_id, &msg) {
        Ok(()) => {
            if let Err(e) = store.transition(id, "approved", "sent", None, None, now) {
                k2_core::log_debug!("[mail/send] sent-mark failed for {id}: {e}");
            }
            send::scrub(store, id);
            Ok(Some(FlushOutcome::Sent))
        }
        Err(e) => fail(store, &e),
    }
}

pub fn flush_due_scheduled(now: i64, limit: usize) -> usize {
    let ids = list_due_scheduled(now, limit);
    if ids.is_empty() {
        return 0;
    }
    let store = DbOutboundStore::default();
    let engine = crate::mail::domains::engine_from_db();
    let mut n = 0;
    for id in ids {
        let backend: Result<&dyn SubmitBackend, String> = match &engine {
            Ok((client, _)) => Ok(client),
            Err(e) => Err(e.clone()),
        };
        let mut account_for_from = |from: &str| -> Result<String, String> {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT stalwart_account_id FROM mail_addresses \
                 WHERE address = ?1 AND status = 'active'",
                rusqlite::params![from],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .ok_or_else(|| format!("sender address '{from}' is no longer active"))
        };
        match flush_one_scheduled(&store, backend, &mut account_for_from, &mut flush_rules_for, &id, now) {
            Ok(Some(FlushOutcome::Sent)) => {
                n += 1;
                k2_core::agent_hooks::emit(
                    k2_core::agent_hooks::HookEvent::MailSendDecided,
                    serde_json::json!({ "outboundId": id, "status": "submitted", "scheduled": true }),
                );
            }
            Ok(Some(FlushOutcome::Failed(_))) => {
                n += 1;
                k2_core::agent_hooks::emit(
                    k2_core::agent_hooks::HookEvent::MailSendDecided,
                    serde_json::json!({ "outboundId": id, "status": "failed", "scheduled": true }),
                );
            }
            Ok(Some(FlushOutcome::HeldForApproval)) => {
                n += 1;
                k2_core::agent_hooks::emit(
                    k2_core::agent_hooks::HookEvent::MailSendApprovalRequested,
                    serde_json::json!({ "outboundId": id, "scheduled": true }),
                );
            }
            Ok(None) => {}
            Err(e) => {
                k2_core::log_debug!("[mail/schedule] flush {id} failed: {e:?}");
            }
        }
    }
    n
}

fn scheduled_tick_interval() -> std::time::Duration {
    let secs = std::env::var("K2_MAIL_SCHEDULED_TICK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &u64| v > 0)
        .unwrap_or(DEFAULT_SCHEDULED_TICK_SECS);
    std::time::Duration::from_secs(secs)
}

fn now_secs_wall() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn spawn_scheduled_flusher() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let tick = scheduled_tick_interval();
        k2_core::log_debug!("[mail/schedule] flusher started — tick={}s", tick.as_secs());
        loop {
            let _ = tokio::task::spawn_blocking(|| {
                let _ = flush_due_scheduled(now_secs_wall(), 50);
            })
            .await;
            tokio::time::sleep(tick).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_relative_duration_accepts_units_and_rejects_bad() {
        assert_eq!(parse_relative_duration("30m").unwrap(), 30 * 60);
        assert_eq!(parse_relative_duration("2h").unwrap(), 2 * 3_600);
        assert_eq!(parse_relative_duration("1d").unwrap(), 86_400);
        assert!(matches!(parse_relative_duration("0m"), Err(SendError::Usage(_))));
        assert!(matches!(parse_relative_duration("31d"), Err(SendError::Usage(_))));
    }

    #[test]
    fn parse_at_rejects_past() {
        let now = 1_700_000_000i64;
        let after = format_send_after(now + 3600);
        assert_eq!(parse_at_timestamp(&after, now).unwrap(), now + 3600);
        let past = format_send_after(now - 60);
        assert!(matches!(parse_at_timestamp(&past, now), Err(SendError::Usage(h)) if h.contains("past")));
    }

    #[test]
    fn resolve_mutex() {
        let now = 1_700_000_000i64;
        assert!(matches!(
            resolve_send_after(Some("2026-01-01T00:00:00Z"), Some("1h"), now),
            Err(SendError::Usage(_))
        ));
        assert_eq!(resolve_send_after(None, Some("30m"), now).unwrap(), Some(now + 1800));
    }

    // ── the rules at send time ──

    use crate::mail::send::{gate_and_dispatch, NewOutbound, OutboundMessage, OutboundRequest, SendOutcome};

    #[derive(Default)]
    struct Keep(std::sync::Mutex<Vec<OutboundMessage>>);
    impl SubmitBackend for Keep {
        fn submit(&self, _a: &str, m: &OutboundMessage) -> Result<(), String> {
            self.0.lock().unwrap().push(m.clone());
            Ok(())
        }
    }

    fn message() -> OutboundMessage {
        OutboundMessage {
            from_name: Some("Bot".to_string()),
            from: format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple()),
            to: vec!["a@b.example".to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            policy_bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: "s".to_string(),
            text_body: "t".to_string(),
            in_reply_to: None,
            references: None,
            reply_source: None,
            attachments: Vec::new(),
            ephemeral: false,
            notify: None,
            forward_of: None,
        }
    }

    struct Rig {
        store: DbOutboundStore,
        dir: std::path::PathBuf,
        project: String,
    }

    impl Rig {
        fn new() -> Rig {
            let dir = std::env::temp_dir().join(format!("k2-flush-{}", uuid::Uuid::new_v4().simple()));
            Rig { store: DbOutboundStore::at(dir.clone()), dir, project: format!("P-{}", uuid::Uuid::new_v4()) }
        }

        /// A row scheduled under `on`, due now.
        fn scheduled(&self, msg: &OutboundMessage, now: i64) -> String {
            let keep = Keep::default();
            let req = OutboundRequest { project_id: &self.project, agent_name: "bot", account_id: "a", message: msg, send_after: Some(now + 60) };
            match gate_and_dispatch(&self.store, Some(&keep), Gate::On, &req, now).expect("schedule") {
                SendOutcome::Scheduled { id, .. } => id,
                other => panic!("expected a schedule, got {other:?}"),
            }
        }

        fn flush(&self, id: &str, keep: &Keep, rules: Result<FlushRules, String>, now: i64) -> Option<FlushOutcome> {
            let mut acct = |_: &str| Ok("a".to_string());
            let mut rules_for = |_: &str, _: &str| rules.clone();
            flush_one_scheduled(&self.store, Ok(keep), &mut acct, &mut rules_for, id, now).expect("flush")
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_outbound WHERE owner_project_id = ?1", rusqlite::params![self.project]);
            drop(conn);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn on() -> FlushRules {
        FlushRules { gate: Gate::On, policy_bcc: Vec::new(), refused: None }
    }

    fn now() -> i64 {
        now_secs_wall()
    }

    #[test]
    fn sending_turned_off_stops_a_scheduled_message() {
        let rig = Rig::new();
        let t = now();
        let id = rig.scheduled(&message(), t);
        let keep = Keep::default();
        let out = rig.flush(&id, &keep, Ok(FlushRules { gate: Gate::Off, policy_bcc: Vec::new(), refused: None }), t + 61);
        assert!(matches!(&out, Some(FlushOutcome::Failed(r)) if r.contains("turned off")), "{out:?}");
        assert!(keep.0.lock().unwrap().is_empty(), "nothing left");
        let row = rig.store.load(&id).unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert!(row.note.as_deref().unwrap_or("").contains("turned off"), "{:?}", row.note);
        assert_eq!(rig.flush(&id, &keep, Ok(on()), t + 62), None, "a failed row stays failed");
    }

    #[test]
    fn rules_that_cannot_be_read_stop_a_scheduled_message() {
        let rig = Rig::new();
        let t = now();
        let id = rig.scheduled(&message(), t);
        let keep = Keep::default();
        let out = rig.flush(&id, &keep, Err("corrupt".to_string()), t + 61);
        assert!(matches!(&out, Some(FlushOutcome::Failed(r)) if r.contains("corrupt")), "{out:?}");
        assert!(keep.0.lock().unwrap().is_empty());
        assert_eq!(rig.store.load(&id).unwrap().unwrap().status, "failed");
    }

    /// Sending moved to approval: the message goes to a person, with a full
    /// 7 days however old it is, and leaves when they approve.
    #[test]
    fn a_message_scheduled_under_on_waits_for_a_person_when_approval_is_on() {
        let rig = Rig::new();
        let t = now() - 20 * 86_400;
        let id = rig.scheduled(&message(), t);
        let keep = Keep::default();
        let flush_at = t + 20 * 86_400;
        let rules = FlushRules { gate: Gate::Approval, policy_bcc: Vec::new(), refused: None };
        assert_eq!(rig.flush(&id, &keep, Ok(rules), flush_at), Some(FlushOutcome::HeldForApproval));
        assert!(keep.0.lock().unwrap().is_empty(), "nothing left");
        let row = rig.store.load(&id).unwrap().unwrap();
        assert_eq!(row.status, "pending");
        send::auto_deny_expired(flush_at + 60);
        assert_eq!(rig.store.load(&id).unwrap().unwrap().status, "pending", "not expired by its age");
        let mut acct = |_: &str| Ok("a".to_string());
        let out = send::approve_and_submit(&rig.store, Ok(&keep), &mut acct, &id, "owner", None, flush_at + 120).expect("approve");
        assert_eq!(out, send::ApproveOutcome::Submitted);
        assert_eq!(keep.0.lock().unwrap().len(), 1);
    }

    /// A person's approval stands while approval stays on; off still stops it.
    #[test]
    fn a_message_a_person_approved_keeps_that_approval() {
        let t = now();
        for (gate, sends) in [(Gate::Approval, true), (Gate::Off, false)] {
            let rig = Rig::new();
            let msg = message();
            let id = rig
                .store
                .insert(&NewOutbound {
                    owner_project_id: &rig.project,
                    agent_name: "bot",
                    message: &msg,
                    status: "pending",
                    decided_by: None,
                    attachment_names: &[],
                    send_after: Some(t + 60),
                    now: t,
                })
                .expect("insert");
            let keep = Keep::default();
            let mut acct = |_: &str| Ok("a".to_string());
            let first = send::approve_and_submit(&rig.store, Ok(&keep), &mut acct, &id, "owner", None, t);
            assert!(matches!(first, Ok(send::ApproveOutcome::Scheduled { .. })), "{first:?}");
            let out = rig.flush(&id, &keep, Ok(FlushRules { gate, policy_bcc: Vec::new(), refused: None }), t + 61);
            if sends {
                assert_eq!(out, Some(FlushOutcome::Sent));
                assert_eq!(keep.0.lock().unwrap().len(), 1);
            } else {
                assert!(matches!(out, Some(FlushOutcome::Failed(_))), "{out:?}");
                assert!(keep.0.lock().unwrap().is_empty());
            }
        }
    }

    /// A policy added after scheduling rides, in the envelope and the audit row.
    #[test]
    fn the_always_bcc_in_force_at_send_time_rides() {
        let rig = Rig::new();
        let t = now();
        let id = rig.scheduled(&message(), t);
        let keep = Keep::default();
        let rules = FlushRules { gate: Gate::On, policy_bcc: vec!["boss@acme.dev".to_string()], refused: None };
        assert_eq!(rig.flush(&id, &keep, Ok(rules), t + 61), Some(FlushOutcome::Sent));
        let seen = keep.0.lock().unwrap();
        assert_eq!(seen[0].all_recipients(), vec!["a@b.example".to_string(), "boss@acme.dev".to_string()]);
        drop(seen);
        let row = rig.store.load(&id).unwrap().unwrap();
        assert_eq!(row.policy_bcc_json.as_deref(), Some(r#"["boss@acme.dev"]"#));
        assert_eq!(rig.store.load_message(&id).unwrap().policy_bcc, vec!["boss@acme.dev".to_string()]);
    }

    /// A policy that now pushes it over the cap refuses; nobody is dropped.
    #[test]
    fn a_policy_that_breaks_the_recipient_cap_refuses() {
        let rig = Rig::new();
        let t = now();
        let mut msg = message();
        msg.to = (0..10).map(|i| format!("r{i}@b.example")).collect();
        let id = rig.scheduled(&msg, t);
        let keep = Keep::default();
        let rules = FlushRules { gate: Gate::On, policy_bcc: vec!["boss@acme.dev".to_string()], refused: None };
        let out = rig.flush(&id, &keep, Ok(rules), t + 61);
        assert!(matches!(&out, Some(FlushOutcome::Failed(r)) if r.contains("too many recipients")), "{out:?}");
        assert!(keep.0.lock().unwrap().is_empty());
    }

    /// The workspace lost the right to send from this address: nothing leaves,
    /// whoever approved it.
    #[test]
    fn a_right_lost_before_the_send_time_stops_the_message() {
        let rig = Rig::new();
        let t = now();
        let id = rig.scheduled(&message(), t);
        let keep = Keep::default();
        let rules = FlushRules { gate: Gate::On, policy_bcc: Vec::new(), refused: Some("no longer".to_string()) };
        let out = rig.flush(&id, &keep, Ok(rules), t + 61);
        assert!(matches!(&out, Some(FlushOutcome::Failed(r)) if r.contains("no longer")), "{out:?}");
        assert!(keep.0.lock().unwrap().is_empty());
        assert_eq!(rig.store.load(&id).unwrap().unwrap().status, "failed");
    }

    /// Read off the real tables: a lowered send level and a receive-only domain.
    #[test]
    fn flush_rules_read_the_send_level_and_the_domain_as_they_are_now() {
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let (id, path) = (uuid::Uuid::new_v4().to_string(), format!("/tmp/k2-flush-right-{tag}"));
        let domain = format!("d{}.example", &tag[..12]);
        let from = format!("bot@{domain}");
        let run = |sql: &str, p: &[&dyn rusqlite::ToSql]| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(sql, p).expect("sql");
        };
        run("INSERT INTO projects (id, name, path, mail_agent_send) VALUES (?1, ?1, ?2, 'on')", &[&id, &path]);
        run(
            "INSERT INTO mail_domains (id, domain, send_mode, status, created_at) VALUES (?1, ?1, 'direct', 'verified', 100)",
            &[&domain],
        );
        run(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) \
             VALUES (?1, ?1, 'dom-x', 'acc', ?2, 'active', 100)",
            &[&from, &id],
        );
        assert_eq!(flush_rules_for(&id, &from).expect("rules").refused, None);
        run("UPDATE mail_domains SET send_mode = 'receive-only' WHERE domain = ?1", &[&domain]);
        let r = flush_rules_for(&id, &from).expect("rules").refused.expect("refused");
        assert!(r.contains("receive-only"), "{r}");
        run("UPDATE mail_domains SET send_mode = 'direct' WHERE domain = ?1", &[&domain]);
        run("UPDATE mail_addresses SET primary_level = 'read' WHERE address = ?1", &[&from]);
        let r = flush_rules_for(&id, &from).expect("rules").refused.expect("refused");
        assert!(r.contains("can't send from this address"), "{r}");
        run("DELETE FROM mail_addresses WHERE address = ?1", &[&from]);
        assert!(flush_rules_for(&id, &from).expect("rules").refused.is_some(), "a mailbox that's gone");
        run("DELETE FROM mail_domains WHERE domain = ?1", &[&domain]);
        run("DELETE FROM projects WHERE id = ?1", &[&id]);
    }

    #[test]
    fn flush_rules_read_the_workspace_as_it_is_now() {
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/k2-flush-rules-{id}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, mail_agent_send) VALUES (?1, ?1, ?2, 'off')",
                rusqlite::params![id, path],
            )
            .expect("project");
        }
        assert_eq!(flush_rules_for(&id, "a@b.example").expect("rules").gate, Gate::Off);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE projects SET mail_agent_send = 'on' WHERE id = ?1", rusqlite::params![id]).expect("mode");
        }
        assert_eq!(flush_rules_for(&id, "a@b.example").expect("rules").gate, Gate::On);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]).expect("delete");
        }
        assert!(flush_rules_for(&id, "a@b.example").is_err(), "a workspace that's gone can't send");
    }
}
