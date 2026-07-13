//! Agent-scheduled outbound delivery: parse `--at`/`--in`, flush due rows.
use super::send::{
    self, ApproveOutcome, DbOutboundStore, OutboundStore, SendError, SubmitBackend,
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

pub fn flush_one_scheduled(
    store: &dyn OutboundStore,
    backend: Result<&dyn SubmitBackend, String>,
    account_id_for_from: &mut dyn FnMut(&str) -> Result<String, String>,
    id: &str,
    now: i64,
) -> Result<Option<ApproveOutcome>, SendError> {
    let flipped = store
        .transition(id, "scheduled", "approved", None, None, now)
        .map_err(|e| SendError::Engine(format!("schedule flush transition failed: {e}")))?;
    if !flipped {
        return Ok(None);
    }
    // Reuse approve_and_submit's post-approved path by submitting via
    // a tiny local replica of the approved-row submit logic.
    let fail = |store: &dyn OutboundStore, reason: &str| {
        let _ = store.transition(id, "approved", "failed", None, None, now);
        let _ = store.append_note(id, &format!("submit failed: {reason}"), now);
        Ok(Some(ApproveOutcome::FailedToSubmit(reason.to_string())))
    };
    let backend = match backend {
        Ok(b) => b,
        Err(e) => return fail(store, &format!("mail server unavailable: {e}")),
    };
    let msg = match store.load_message(id) {
        Ok(m) => m,
        Err(e) => return fail(store, &format!("stored message unreadable: {e}")),
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
            Ok(Some(ApproveOutcome::Submitted))
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
        match flush_one_scheduled(&store, backend, &mut account_for_from, &id, now) {
            Ok(Some(ApproveOutcome::Submitted)) => {
                n += 1;
                k2_core::agent_hooks::emit(
                    k2_core::agent_hooks::HookEvent::MailSendDecided,
                    serde_json::json!({ "outboundId": id, "status": "submitted", "scheduled": true }),
                );
            }
            Ok(Some(ApproveOutcome::FailedToSubmit(_))) => {
                n += 1;
            }
            Ok(Some(ApproveOutcome::Scheduled { .. })) | Ok(None) => {}
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
}
