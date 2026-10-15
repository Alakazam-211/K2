//! Local Grok Bots change poller (0.45.7) — daemon-owned, runs headless.
//!
//! Grok Bot has no live feed. Its own advice (samples 2026-10-15): poll
//! `listAgents` every few seconds and re-read `store.db` only when an
//! agent's `snapshotEpoch`/`snapshotSeq`/`newestEntryId` move. This loop
//! does exactly that and tells clients with one app-level
//! `grokbots_changed {rev, roster, agents}` event (ids only). While the
//! gateway is down it falls back to a stat of `store.db` + WAL (size and
//! mtime; no file is opened).
//!
//! For a **stale** agent (live conversation outside this computer) it also
//! records each new latest entry the roster reports into the agent's live
//! tail (`k2_core::external_agents::live_tail`). That is only what the
//! roster summary exposes — the latest entry at each poll — so a burst of
//! several lines between two polls shows only the last one (marked as a
//! possible gap when the snapshot counter jumped).
//!
//! It does nothing — reads nothing under `~/sand-data` — while
//! Settings → 3rd Party Bots → Grok Bot → Show is off.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use k2_core::external_agents::grokbot::GrokBot;
use k2_core::external_agents::{live_tail, settings, ExtError, ExternalAgentProvider};
use k2_core::log_debug;

use crate::session_events::{emit, SessionEvent};

/// How often the roster and status are polled while Show is on.
pub const POLL: Duration = Duration::from_secs(3);

/// What the last poll saw.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Seen {
    /// Per-agent chat change marker.
    revs: HashMap<String, String>,
    /// Roster + per-agent status + box status, as one comparable string.
    roster_key: String,
    primed: bool,
}

/// One change set to announce.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Change {
    pub roster: bool,
    pub agents: Vec<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        !self.roster && self.agents.is_empty()
    }
}

/// One poll against `p`. The first poll only primes (no event).
pub fn tick(p: &GrokBot, seen: &mut Seen) -> Change {
    let list = match p.list_agents() {
        Ok(l) => l,
        Err(ExtError::NotInstalled) => Vec::new(),
        Err(_) => return Change::default(),
    };
    let status = p.status();
    let now = chrono::Utc::now().timestamp_millis();
    let mut tail_moved = Vec::new();
    for a in list.iter().filter(|a| a.stale) {
        let Some(live) = &a.live else { continue };
        if live_tail::observe(&a.id, live, now, |id| {
            p.store_has_entry(&a.id, id) == Some(true)
        }) {
            tail_moved.push(a.id.clone());
        }
    }
    let mut revs = HashMap::new();
    let mut roster_parts = vec![format!(
        "busy={}:{:?}:{}",
        status.busy, status.active_agent_id, status.reachable
    )];
    for a in &list {
        let rev = if a.rev.is_empty() {
            p.store_fingerprint(&a.id).unwrap_or_default()
        } else {
            a.rev.clone()
        };
        revs.insert(a.id.clone(), rev);
        roster_parts.push(format!(
            "{}|{}|{:?}|{}|{}|{}|{}|{}|{:?}|{:?}",
            a.id,
            a.name,
            a.title,
            a.working,
            a.needs_you,
            a.unread_count,
            a.source,
            a.stale,
            a.last_message_preview,
            a.last_activity_at
        ));
    }
    let roster_key = roster_parts.join("\n");
    let mut change = Change::default();
    if seen.primed {
        // A new live-tail line also changes the row's preview.
        change.roster = roster_key != seen.roster_key || !tail_moved.is_empty();
        let mut agents: Vec<String> = revs
            .iter()
            .filter(|(id, rev)| seen.revs.get(*id).is_some_and(|old| old != *rev))
            .map(|(id, _)| id.clone())
            .chain(tail_moved)
            .collect();
        agents.sort();
        agents.dedup();
        change.agents = agents;
    }
    *seen = Seen {
        revs,
        roster_key,
        primed: true,
    };
    change
}

struct Shared {
    seen: Mutex<Seen>,
    wake: Mutex<bool>,
    cv: Condvar,
    rev: std::sync::atomic::AtomicI64,
}

fn shared() -> &'static Shared {
    static S: OnceLock<Shared> = OnceLock::new();
    S.get_or_init(|| Shared {
        seen: Mutex::new(Seen::default()),
        wake: Mutex::new(false),
        cv: Condvar::new(),
        rev: std::sync::atomic::AtomicI64::new(0),
    })
}

fn announce(c: &Change) {
    let rev = shared()
        .rev
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
    let _ = emit(SessionEvent::GrokbotsChanged {
        rev,
        roster: c.roster,
        agents: c.agents.clone(),
    });
}

fn wake_now() {
    let s = shared();
    *s.wake.lock().unwrap_or_else(|p| p.into_inner()) = true;
    s.cv.notify_all();
}

/// The setting changed: tell clients to refetch now, and poll at once.
pub fn nudge_roster() {
    announce(&Change {
        roster: true,
        agents: Vec::new(),
    });
    wake_now();
}

/// `agent`'s chat changed outside a poll (a send recorded in its live
/// tail): tell clients now (roster too: the row preview moved).
pub fn announce_agent(agent: &str) {
    announce(&Change {
        roster: true,
        agents: vec![agent.to_string()],
    });
}

/// A send went out to `agent`: poll at once so the echo shows quickly.
pub fn nudge_agent(_agent: &str) {
    wake_now();
}

/// Start the poller (idempotent).
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("grokbots-watch".into())
        .spawn(|| {
            log_debug!("[grokbots] poller started");
            loop {
                {
                    let s = shared();
                    let mut woke = s.wake.lock().unwrap_or_else(|p| p.into_inner());
                    if !*woke {
                        woke =
                            s.cv.wait_timeout(woke, POLL)
                                .map(|(g, _)| g)
                                .unwrap_or_else(|p| p.into_inner().0);
                    }
                    *woke = false;
                }
                let mut seen = shared().seen.lock().unwrap_or_else(|p| p.into_inner());
                if !settings::load().show {
                    *seen = Seen::default();
                    continue;
                }
                let change = tick(&GrokBot::for_current_user(), &mut seen);
                drop(seen);
                if !change.is_empty() {
                    announce(&change);
                }
            }
        });
    if let Err(e) = spawned {
        log_debug!("[grokbots] could not start poller: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::external_agents::grokbot::fixture::{self, FakeGateway, FakeSandData};
    use serde_json::json;

    const A: &str = "0e5f5de8-7619-4ba4-9753-32c5470b2346";

    #[test]
    fn tick_primes_then_reports_chat_and_status_changes() {
        let gw = FakeGateway::start(json!([fixture::list_item(A, "Cortana")]));
        let home = crate::test_support::TempHome::new();
        let sand = FakeSandData::create(home.path(), gw.port);
        sand.add_agent(A, "Cortana", "x");
        // The entry the gateway will name as newest is in store.db (else
        // the agent would turn stale, a roster change).
        sand.add_entry(A, json!({"kind":"message","id":"e1","role":"user","content":"hi"}));
        let p = GrokBot::for_current_user();
        let mut seen = Seen::default();
        assert!(tick(&p, &mut seen).is_empty(), "first poll only primes");
        assert!(tick(&p, &mut seen).is_empty(), "nothing moved");

        // newestEntryId moves → that agent's chat changed
        gw.set(|s| s.agents[0]["newestEntryId"] = json!("e1"));
        let c = tick(&p, &mut seen);
        assert_eq!(c.agents, vec![A.to_string()]);
        assert!(!c.roster);

        // the agent starts a turn → roster/status changed
        gw.set(|s| s.agents[0]["isRunningTurn"] = json!(true));
        let c = tick(&p, &mut seen);
        assert!(c.roster);
        assert!(c.agents.is_empty());

        // box-wide busy flips → roster/status changed
        gw.set(|s| s.busy = true);
        assert!(tick(&p, &mut seen).roster);

        // the conversation moves elsewhere (stale) → roster changed
        gw.set(|s| {
            s.agents[0]["lastActivityAt"] = json!(1_792_070_040_000_i64);
            s.agents[0]["localActivityAt"] = json!(1_788_000_000_000_i64);
        });
        assert!(tick(&p, &mut seen).roster);
    }

    #[test]
    fn tick_records_a_stale_agents_new_latest_lines_in_its_live_tail() {
        use k2_core::external_agents::live_tail;
        let now = chrono::Utc::now().timestamp_millis();
        let mut item = fixture::list_item(A, "Grok");
        item["lastActivityAt"] = json!(now - 5_000);
        item["localActivityAt"] = json!(now - 40 * 86_400_000_i64);
        item["lastMessageId"] = json!("r1");
        item["lastEntry"] = json!({"kind": "send-message", "text": "first reply"});
        item["snapshotSeq"] = json!(3);
        let gw = FakeGateway::start(json!([item]));
        let home = crate::test_support::TempHome::new();
        let sand = FakeSandData::create(home.path(), gw.port);
        sand.add_agent(A, "Grok", "x");
        let p = GrokBot::for_current_user();
        let mut seen = Seen::default();
        assert!(tick(&p, &mut seen).is_empty(), "first poll only primes");
        let t = live_tail::tail(A).expect("followed from the first poll");
        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.entries[0].text, "first reply");

        // A new latest line → the tail grows and clients hear about it.
        gw.set(|s| {
            s.agents[0]["lastMessageId"] = json!("r2");
            s.agents[0]["lastEntry"] = json!({"kind": "send-message", "text": "second reply"});
            s.agents[0]["snapshotSeq"] = json!(4);
        });
        let c = tick(&p, &mut seen);
        assert_eq!(c.agents, vec![A.to_string()]);
        assert!(c.roster, "the row preview moved");
        let t = live_tail::tail(A).expect("tail");
        let texts: Vec<&str> = t.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["first reply", "second reply"]);
        assert!(!t.entries[1].gap_before);

        // Nothing new → nothing announced, nothing added.
        assert!(tick(&p, &mut seen).is_empty());
        assert_eq!(live_tail::tail(A).expect("tail").entries.len(), 2);
    }

    #[test]
    fn tick_falls_back_to_store_stat_when_the_gateway_is_down() {
        let home = crate::test_support::TempHome::new();
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = closed.local_addr().expect("addr").port();
        drop(closed);
        let sand = FakeSandData::create(home.path(), port);
        sand.add_agent(A, "Cortana", "x");
        let p = GrokBot::for_current_user();
        let mut seen = Seen::default();
        let _ = tick(&p, &mut seen);
        sand.add_entry(A, json!({"kind":"message","role":"user","content":"new"}));
        let c = tick(&p, &mut seen);
        assert_eq!(c.agents, vec![A.to_string()], "store stat moved");
    }
}
