//! What happened to each send (prd-custom-app-push-v1 CP28, CP39): counts
//! only, in memory, the last [`KEEP`] sends per App for an hour. Every send
//! also writes two `app-audit.db` rows: when it is queued (`push.send`:
//! actor, target kind, recipient count, topic) and when its last device
//! answers (`push.done`: the counts). Never a title, a body, a data value,
//! a token or a JWT.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::sender::Outcome;

/// Sends kept per App.
pub const KEEP: usize = 500;
/// How long a send is kept.
pub const KEEP_FOR: Duration = Duration::from_secs(3600);

/// Recipients a send skipped, by reason (CP28).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Skipped {
    pub not_a_guest: usize,
    pub no_devices: usize,
    pub muted_topic: usize,
    pub rate_limited: usize,
    pub platform_not_configured: usize,
}

/// One send's counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendRecord {
    pub send_id: String,
    /// Seconds since the epoch.
    pub at: i64,
    pub actor: String,
    /// `guest` | `guests` | `room` | `agent-reply`.
    pub target: String,
    pub recipients: usize,
    pub topic: Option<String>,
    pub queued: usize,
    pub sent: usize,
    pub dead: usize,
    pub failed: usize,
    pub wrong_app: usize,
    pub dropped: usize,
    pub skipped: Skipped,
    #[serde(skip)]
    started: Option<Instant>,
}

impl SendRecord {
    pub fn new(send_id: &str, actor: &str, target: &str, recipients: usize, topic: Option<String>, queued: usize, skipped: Skipped) -> Self {
        SendRecord {
            send_id: send_id.to_string(),
            at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
            actor: actor.to_string(),
            target: target.to_string(),
            recipients,
            topic,
            queued,
            sent: 0,
            dead: 0,
            failed: 0,
            wrong_app: 0,
            dropped: 0,
            skipped,
            started: Some(Instant::now()),
        }
    }

    pub fn done(&self) -> bool {
        self.sent + self.dead + self.failed + self.dropped >= self.queued
    }
}

fn log() -> &'static Mutex<HashMap<String, VecDeque<SendRecord>>> {
    static L: OnceLock<Mutex<HashMap<String, VecDeque<SendRecord>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

fn trim(q: &mut VecDeque<SendRecord>) {
    while q.len() > KEEP {
        q.pop_front();
    }
    while q.front().is_some_and(|r| r.started.is_some_and(|s| s.elapsed() >= KEEP_FOR)) {
        q.pop_front();
    }
}

/// Record a new send (before its jobs are queued) and write its
/// `push.send` audit row.
pub fn start(app: &str, rec: SendRecord) {
    audit(
        app,
        &rec.actor,
        &format!(
            "push.send to={} n={} queued={} topic={}",
            rec.target,
            rec.recipients,
            rec.queued,
            rec.topic.as_deref().unwrap_or("-")
        ),
    );
    let done = rec.done();
    let snapshot = rec.clone();
    {
        let mut m = log().lock().unwrap_or_else(|e| e.into_inner());
        let q = m.entry(app.to_string()).or_default();
        q.push_back(rec);
        trim(q);
    }
    if done {
        finish_audit(app, &snapshot);
    }
}

/// The jobs of a started send couldn't be queued: forget it and say so.
pub fn refused(app: &str, send_id: &str, code: &str) {
    let gone = {
        let mut m = log().lock().unwrap_or_else(|e| e.into_inner());
        m.get_mut(app).and_then(|q| q.iter().position(|r| r.send_id == send_id).and_then(|i| q.remove(i)))
    };
    if let Some(r) = gone {
        audit(app, &r.actor, &format!("push.refused send={} code={code}", r.send_id));
    }
}

/// One device answered.
pub fn record(app: &str, send_id: &str, outcome: &Outcome) {
    let finished = {
        let mut m = log().lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = m.get_mut(app).and_then(|q| q.iter_mut().rev().find(|r| r.send_id == send_id)) else { return };
        let was_done = r.done();
        match outcome {
            Outcome::Sent => r.sent += 1,
            Outcome::Dead { wrong_app } => {
                r.dead += 1;
                if *wrong_app {
                    r.wrong_app += 1;
                }
            }
            Outcome::Failed(_) => r.failed += 1,
            Outcome::Dropped => r.dropped += 1,
        }
        (!was_done && r.done()).then(|| r.clone())
    };
    if let Some(r) = finished {
        finish_audit(app, &r);
    }
}

fn finish_audit(app: &str, r: &SendRecord) {
    audit(
        app,
        &r.actor,
        &format!(
            "push.done send={} queued={} sent={} dead={} failed={} wrongApp={} dropped={} skipped={}",
            r.send_id,
            r.queued,
            r.sent,
            r.dead,
            r.failed,
            r.wrong_app,
            r.dropped,
            r.skipped.not_a_guest + r.skipped.no_devices + r.skipped.muted_topic + r.skipped.rate_limited + r.skipped.platform_not_configured
        ),
    );
}

/// One send of one App.
pub fn get(app: &str, send_id: &str) -> Option<SendRecord> {
    let mut m = log().lock().unwrap_or_else(|e| e.into_inner());
    let q = m.get_mut(app)?;
    trim(q);
    q.iter().rev().find(|r| r.send_id == send_id).cloned()
}

/// One App's recent sends, newest first.
pub fn list(app: &str) -> Vec<SendRecord> {
    let mut m = log().lock().unwrap_or_else(|e| e.into_inner());
    let Some(q) = m.get_mut(app) else { return Vec::new() };
    trim(q);
    q.iter().rev().cloned().collect()
}

/// One `app-audit.db` row (counts and kinds only).
fn audit(app: &str, actor: &str, verb: &str) {
    let app_id = super::audit_app_id(app);
    crate::apps::audit::record(k2_core::app_audit::Call {
        at_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0),
        app_id,
        seed: None,
        actor: actor.to_string(),
        on_behalf_of: None,
        verb: verb.to_string(),
        room: None,
        result: 202,
        code: None,
        bytes_in: 0,
        bytes_out: 0,
        latency_ms: 0,
    });
}
