//! A node's sim sessions: admission, the stream grace, the hard end,
//! reconcile on attach and restart, and memory (PRD §3.2, review IR-6,
//! IR-7, IR-8). Pure: the caller passes the clock and does the effects
//! (shut the device down, send the state frame), so k2-node and the test
//! fakes share one set of rules.

use super::{codes, SessionState, SimOpen, SimStateFrame, GRACE_SECS, MAX_SESSIONS_PER_NODE, MEM_SLACK_BYTES, RESERVE_MILLICPU};

/// What the node knows when a `sim_open` arrives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Admission {
    /// This slot turned sim on and the Simulator service is ready.
    pub enabled: bool,
    pub paused: bool,
    pub draining: bool,
    /// A foreign lock holds the node (Sew's VM CI): who holds it.
    pub foreign_lock: Option<String>,
    /// Free memory under the node's cap, after running jobs.
    pub free_mem: u64,
    /// Above the node's disk floor.
    pub disk_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub session: String,
    pub slot: u8,
    pub workspace_key: String,
    pub mem_bytes: u64,
    pub state: SessionState,
    pub ends_at: u64,
    /// When the stream went down (or the session opened, until it's up).
    pub down_since: Option<u64>,
}

/// One thing the caller must do: send `frame` to `slot`'s server and, when
/// `shutdown`, shut the session's device down and stop its helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub slot: u8,
    pub workspace_key: String,
    pub frame: SimStateFrame,
    pub shutdown: bool,
}

#[derive(Debug, Default)]
pub struct Leases {
    list: Vec<Lease>,
}

fn frame(session: &str, state: SessionState, code: Option<&str>, message: Option<String>) -> SimStateFrame {
    SimStateFrame { session: session.to_string(), state, code: code.map(str::to_string), message }
}

impl Leases {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, session: &str) -> Option<&Lease> {
        self.list.iter().find(|l| l.session == session)
    }

    pub fn all(&self) -> &[Lease] {
        &self.list
    }

    /// This slot's live sessions (`Offer.sim.sessions`).
    pub fn sessions_for(&self, slot: u8) -> Vec<String> {
        self.list.iter().filter(|l| l.slot == slot).map(|l| l.session.clone()).collect()
    }

    /// Memory the sessions reserve: counted in the node's `used()` and the
    /// offer's free memory (IR-8).
    pub fn reserved_bytes(&self) -> u64 {
        self.list.iter().map(|l| l.mem_bytes).sum()
    }

    pub fn reserved_millicpu(&self) -> u64 {
        self.list.len() as u64 * RESERVE_MILLICPU as u64
    }

    /// A job with no `--mem` gets the node cap minus the sessions'
    /// reservation as its RSS limit, not the whole cap (IR-8).
    pub fn default_job_mem(&self, node_cap: u64) -> u64 {
        node_cap.saturating_sub(self.reserved_bytes())
    }

    /// Admit or refuse a `sim_open`. A session of the same slot and
    /// workspace still open or in grace is replaced, not counted.
    pub fn open(&mut self, slot: u8, o: &SimOpen, adm: &Admission, now: u64) -> Vec<Effect> {
        let refuse = |code: &str, msg: String| {
            vec![Effect { slot, workspace_key: o.workspace_key.clone(), frame: frame(&o.session, SessionState::Refused, Some(code), Some(msg)), shutdown: false }]
        };
        if !o.valid() {
            return refuse(codes::BAD_FRAME, "the sim_open frame is malformed".into());
        }
        if self.get(&o.session).is_some() {
            return refuse(codes::BAD_FRAME, "that session is already open".into());
        }
        if !adm.enabled {
            return refuse(codes::NOT_ENABLED, "this Mac doesn't run iPhones for this server (the node owner turns it on per server)".into());
        }
        if let Some(who) = &adm.foreign_lock {
            return refuse(codes::NODE_BUSY, format!("holding for {who}"));
        }
        if adm.draining {
            return refuse(codes::NODE_BUSY, "draining".into());
        }
        if adm.paused {
            return refuse(codes::NODE_BUSY, "paused by its owner".into());
        }
        let mut out = Vec::new();
        if let Some(i) = self.list.iter().position(|l| l.slot == slot && l.workspace_key == o.workspace_key) {
            let old = self.list.remove(i);
            // The device is reused by the new session: no shutdown.
            out.push(Effect { slot, workspace_key: old.workspace_key, frame: frame(&old.session, SessionState::Closed, Some(codes::REPLACED), None), shutdown: false });
        }
        if self.list.len() >= MAX_SESSIONS_PER_NODE {
            let msg = "running another iPhone".to_string();
            out.extend(refuse(codes::NODE_BUSY, msg));
            return out;
        }
        if !adm.disk_ok {
            out.extend(refuse(codes::NODE_BUSY, "low on disk".into()));
            return out;
        }
        if adm.free_mem < o.mem_bytes {
            out.extend(refuse(codes::NODE_BUSY, format!("low on memory ({} GB free, {} GB needed)", adm.free_mem >> 30, o.mem_bytes >> 30)));
            return out;
        }
        self.list.push(Lease {
            session: o.session.clone(),
            slot,
            workspace_key: o.workspace_key.clone(),
            mem_bytes: o.mem_bytes,
            state: SessionState::Opening,
            ends_at: now + o.lease_secs,
            down_since: Some(now),
        });
        out.push(Effect { slot, workspace_key: o.workspace_key.clone(), frame: frame(&o.session, SessionState::Opening, None, None), shutdown: false });
        out
    }

    fn set(&mut self, session: &str, state: SessionState, down: Option<u64>) -> Vec<Effect> {
        match self.list.iter_mut().find(|l| l.session == session) {
            Some(l) if l.state != state => {
                l.state = state;
                l.down_since = down;
                vec![Effect { slot: l.slot, workspace_key: l.workspace_key.clone(), frame: frame(session, state, None, None), shutdown: false }]
            }
            _ => vec![],
        }
    }

    /// The stream for `session` is up (a first dial or a redial in grace).
    pub fn stream_up(&mut self, session: &str) -> Vec<Effect> {
        self.set(session, SessionState::Open, None)
    }

    /// The stream dropped: keep the device for [`GRACE_SECS`].
    pub fn stream_down(&mut self, session: &str, now: u64) -> Vec<Effect> {
        self.set(session, SessionState::Reconnecting, Some(now))
    }

    fn close_where(&mut self, code: &str, mut pred: impl FnMut(&Lease) -> bool) -> Vec<Effect> {
        let (gone, keep): (Vec<Lease>, Vec<Lease>) = std::mem::take(&mut self.list).into_iter().partition(|l| pred(l));
        self.list = keep;
        gone.into_iter()
            .map(|l| Effect { slot: l.slot, workspace_key: l.workspace_key, frame: frame(&l.session, SessionState::Closed, Some(code), None), shutdown: true })
            .collect()
    }

    /// `sim_close` from the session's own server.
    pub fn close(&mut self, slot: u8, session: &str) -> Vec<Effect> {
        self.close_where(codes::CLOSED, |l| l.slot == slot && l.session == session)
    }

    /// `sim_sync` after an attach: close at once every session of `slot`
    /// the server no longer holds (IR-7), so a server restart doesn't keep
    /// the node busy for the grace.
    pub fn sync(&mut self, slot: u8, held: &[String]) -> Vec<Effect> {
        self.close_where(codes::NOT_HELD, |l| l.slot == slot && !held.contains(&l.session))
    }

    /// Grace and hard end. Call every few seconds.
    pub fn tick(&mut self, now: u64) -> Vec<Effect> {
        let mut out = self.close_where(codes::LEASE_EXPIRED, |l| now >= l.ends_at);
        out.extend(self.close_where(codes::LEASE_LOST, |l| l.down_since.is_some_and(|d| now.saturating_sub(d) >= GRACE_SECS)));
        out
    }

    /// The Simulator user's RSS, summed by uid: above every reservation +
    /// [`MEM_SLACK_BYTES`], end the sessions (IR-8).
    pub fn police_memory(&mut self, sim_user_rss: u64) -> Vec<Effect> {
        if self.list.is_empty() || sim_user_rss <= self.reserved_bytes() + MEM_SLACK_BYTES {
            return vec![];
        }
        self.close_where(codes::MEM_CAP, |_| true)
    }

    /// The Simulator service exited: every session ends.
    pub fn simhost_exited(&mut self) -> Vec<Effect> {
        self.close_where(codes::SIMHOST_EXITED, |_| true)
    }

    /// k2-node started (after a crash or an update): simhost `reset` shuts
    /// every device; nothing survives (IR-7).
    pub fn reset(&mut self) -> Vec<Effect> {
        self.close_where(codes::NODE_RESTARTED, |_| true)
    }

    /// `node update` and drain wait while this is true (IR-7).
    pub fn busy(&self) -> bool {
        !self.list.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::RESERVE_MEM_BYTES;
    use super::*;

    const WS: &str = "0123456789abcdef01234567";
    const WS2: &str = "ffffffffffffffffffffffff";

    fn sid(n: u8) -> String {
        format!("{n:0>32x}")
    }

    fn open(n: u8, ws: &str) -> SimOpen {
        SimOpen { session: sid(n), workspace_key: ws.into(), mem_bytes: RESERVE_MEM_BYTES, lease_secs: 8 * 3600 }
    }

    fn adm() -> Admission {
        Admission { enabled: true, free_mem: 8 << 30, disk_ok: true, ..Default::default() }
    }

    fn states(e: &[Effect]) -> Vec<(String, SessionState, Option<String>, bool)> {
        e.iter().map(|e| (e.frame.session.clone(), e.frame.state, e.frame.code.clone(), e.shutdown)).collect()
    }

    #[test]
    fn redial_within_the_grace_keeps_the_device() {
        let mut l = Leases::new();
        assert_eq!(states(&l.open(0, &open(1, WS), &adm(), 0))[0].1, SessionState::Opening);
        assert_eq!(states(&l.stream_up(&sid(1)))[0].1, SessionState::Open);
        assert_eq!(states(&l.stream_down(&sid(1), 100))[0].1, SessionState::Reconnecting);
        assert!(l.tick(100 + GRACE_SECS - 1).is_empty(), "inside the grace");
        assert_eq!(states(&l.stream_up(&sid(1)))[0].1, SessionState::Open);
        assert!(l.tick(100 + GRACE_SECS + 10).is_empty(), "back up: no lease kill");
    }

    #[test]
    fn no_redial_ends_the_lease_and_shuts_the_device_down() {
        let mut l = Leases::new();
        l.open(0, &open(1, WS), &adm(), 0);
        l.stream_up(&sid(1));
        l.stream_down(&sid(1), 50);
        let e = l.tick(50 + GRACE_SECS);
        assert_eq!(states(&e), vec![(sid(1), SessionState::Closed, Some(codes::LEASE_LOST.into()), true)]);
        assert!(!l.busy());
    }

    #[test]
    fn a_stream_that_never_comes_up_is_lost_too() {
        let mut l = Leases::new();
        l.open(0, &open(1, WS), &adm(), 0);
        assert_eq!(states(&l.tick(GRACE_SECS))[0].2.as_deref(), Some(codes::LEASE_LOST));
    }

    #[test]
    fn the_hard_end_closes_even_an_open_stream() {
        let mut l = Leases::new();
        let mut o = open(1, WS);
        o.lease_secs = 60;
        l.open(0, &o, &adm(), 0);
        l.stream_up(&sid(1));
        assert_eq!(states(&l.tick(60))[0].2.as_deref(), Some(codes::LEASE_EXPIRED));
    }

    #[test]
    fn one_session_per_node_and_the_holds_refuse_new_ones() {
        let mut l = Leases::new();
        l.open(0, &open(1, WS), &adm(), 0);
        let e = l.open(1, &open(2, WS2), &adm(), 0);
        assert_eq!(states(&e), vec![(sid(2), SessionState::Refused, Some(codes::NODE_BUSY.into()), false)]);
        assert!(e[0].frame.message.as_deref().unwrap().contains("another iPhone"));
        // A foreign lock refuses a new session but leaves the running one.
        let mut held = Leases::new();
        held.open(0, &open(1, WS), &adm(), 0);
        let mut lock = adm();
        lock.foreign_lock = Some("Sew CI".into());
        let e = held.open(1, &open(3, WS2), &lock, 0);
        assert_eq!(e[0].frame.code.as_deref(), Some(codes::NODE_BUSY));
        assert!(e[0].frame.message.as_deref().unwrap().contains("Sew CI"), "the hold is named");
        assert!(held.get(&sid(1)).is_some());
        for a in [Admission { draining: true, ..adm() }, Admission { paused: true, ..adm() }, Admission { disk_ok: false, ..adm() }, Admission { free_mem: 1 << 30, ..adm() }] {
            assert_eq!(Leases::new().open(0, &open(4, WS), &a, 0)[0].frame.code.as_deref(), Some(codes::NODE_BUSY), "{a:?}");
        }
        assert_eq!(Leases::new().open(0, &open(4, WS), &Admission { enabled: false, ..adm() }, 0)[0].frame.code.as_deref(), Some(codes::NOT_ENABLED));
    }

    #[test]
    fn sync_after_a_server_restart_frees_the_node_at_once() {
        let mut l = Leases::new();
        l.open(0, &open(1, WS), &adm(), 0);
        l.stream_down(&sid(1), 10);
        // Another server's sync never touches slot 0's session.
        assert!(l.sync(1, &[]).is_empty());
        let e = l.sync(0, &[]);
        assert_eq!(states(&e), vec![(sid(1), SessionState::Closed, Some(codes::NOT_HELD.into()), true)]);
        // Booting again right away works.
        assert_eq!(l.open(0, &open(2, WS), &adm(), 11)[0].frame.state, SessionState::Opening);
        // A held session survives a sync.
        assert!(l.sync(0, &[sid(2)]).is_empty());
    }

    #[test]
    fn the_same_workspace_replaces_its_own_session_in_grace() {
        let mut l = Leases::new();
        l.open(0, &open(1, WS), &adm(), 0);
        l.stream_down(&sid(1), 5);
        let e = l.open(0, &open(2, WS), &adm(), 6);
        assert_eq!(states(&e), vec![(sid(1), SessionState::Closed, Some(codes::REPLACED.into()), false), (sid(2), SessionState::Opening, None, false)]);
    }

    #[test]
    fn a_k2_node_restart_resets_every_device() {
        let mut l = Leases::new();
        l.open(0, &open(1, WS), &adm(), 0);
        assert!(l.busy(), "update and drain wait");
        assert_eq!(states(&l.reset()), vec![(sid(1), SessionState::Closed, Some(codes::NODE_RESTARTED.into()), true)]);
        assert!(!l.busy());
    }

    #[test]
    fn memory_is_reserved_capped_and_policed() {
        let mut l = Leases::new();
        let cap = 8u64 << 30;
        assert_eq!(l.default_job_mem(cap), cap);
        l.open(0, &open(1, WS), &adm(), 0);
        assert_eq!(l.reserved_bytes(), RESERVE_MEM_BYTES);
        assert_eq!(l.reserved_millicpu(), RESERVE_MILLICPU as u64);
        assert_eq!(l.default_job_mem(cap), 5 << 30, "rig1-mac: jobs get 5 GB while an iPhone runs (D3)");
        assert!(l.police_memory(RESERVE_MEM_BYTES + MEM_SLACK_BYTES).is_empty());
        let e = l.police_memory(RESERVE_MEM_BYTES + MEM_SLACK_BYTES + 1);
        assert_eq!(states(&e), vec![(sid(1), SessionState::Closed, Some(codes::MEM_CAP.into()), true)]);
    }

    #[test]
    fn a_simhost_crash_ends_every_session() {
        let mut l = Leases::new();
        l.open(0, &open(1, WS), &adm(), 0);
        assert_eq!(l.simhost_exited()[0].frame.code.as_deref(), Some(codes::SIMHOST_EXITED));
    }
}
