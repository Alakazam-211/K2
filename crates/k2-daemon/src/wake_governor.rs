//! The wake governor (0.46.11; Rosson 2026-11-10: "How can we stagger it
//! so that we don't just blast the server out of nowhere by waking up
//! potentially hundreds of agents at the same time?").
//!
//! One server-wide queue for scheduled LLM wakes. A wake that would start
//! an agent's turn on a schedule first claims a slot here; it delivers only
//! with one, and the slot is held until the agent's work is done. Today the
//! Org Chart's night run, last call and `ask` fan-out go through it;
//! heartbeats and Reminders can join by calling [`Governor::claim`] before
//! they deliver and [`Governor::reap`] on their own tick.
//!
//! **How many at once comes from the machine, not a constant:**
//! - at most `max(1, cores ÷ 4)` wakes run at once,
//! - capped by available memory (about 1.5 GiB per agent session: a new
//!   start needs that much free, except that one may always run),
//! - and no new start while the 1-minute load average is above the number
//!   of cores (one log line when it starts holding, then at most every
//!   15 minutes while it holds).
//!
//! One agent holds at most one slot. **A slot ends** when its agent goes
//! idle (seen working, then not; or never seen working after
//! [`IDLE_GRACE_SECS`]), when the source says the work is done (an Org
//! Chart head wrote its bulletin; a member answered), or at its time cap.
//! Slots live in memory; after a restart each source re-adopts the wakes it
//! made recently from its own ledger ([`Governor::adopt_once`]).

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

/// About this much memory per agent session.
pub const SESSION_BYTES: u64 = 1536 * 1024 * 1024;
/// One slot per this many cores (at least one slot).
pub const CORES_PER_SLOT: usize = 4;
/// An agent never seen working is counted done after this long.
pub const IDLE_GRACE_SECS: i64 = 180;
/// While holding for load, the log line repeats at most this often.
pub const HOLD_LOG_EVERY_SECS: i64 = 15 * 60;

/// What the governor reads about the machine.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Machine {
    pub cores: usize,
    /// Memory a new process could get; `None` when the OS gives no honest
    /// number (then memory doesn't cap).
    pub available_bytes: Option<u64>,
    /// The 1-minute load average; `None` where there is none (Windows).
    pub load1: Option<f64>,
}

impl Machine {
    /// This machine, now.
    pub fn read() -> Machine {
        Machine { cores: k2_core::disk_guard::cores().unwrap_or(1).max(1), available_bytes: available_bytes(), load1: load1() }
    }

    /// Slots from the cores alone.
    pub fn cpu_slots(&self) -> usize {
        (self.cores / CORES_PER_SLOT).max(1)
    }

    /// The cap with `running` wakes already going: the cores' slots, cut
    /// to what memory can take on top of those running, never below one.
    pub fn slots(&self, running: usize) -> usize {
        let cpu = self.cpu_slots();
        let mem = match self.available_bytes {
            Some(a) => running.saturating_add((a / SESSION_BYTES) as usize),
            None => cpu,
        };
        cpu.min(mem).max(1)
    }

    /// Too loaded to start anything new?
    pub fn overloaded(&self) -> bool {
        self.load1.is_some_and(|l| l > self.cores as f64)
    }

    pub fn to_json(&self, running: usize) -> Value {
        json!({
            "cores": self.cores,
            "availableBytes": self.available_bytes,
            "load1": self.load1,
            "slots": self.slots(running),
            "overloaded": self.overloaded(),
        })
    }
}

#[cfg(target_os = "linux")]
fn available_bytes() -> Option<u64> {
    k2_core::disk_guard::box_mem().1
}

/// macOS: `kern.memorystatus_level` is the percent of memory available
/// (the number Activity Monitor's pressure graph is built on).
#[cfg(target_os = "macos")]
fn available_bytes() -> Option<u64> {
    let total = k2_core::disk_guard::box_mem().0?;
    let mut level: u32 = 0;
    let mut len = std::mem::size_of::<u32>();
    let name = c"kern.memorystatus_level";
    // SAFETY: a sysctl read into a u32 of the size we pass.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), (&mut level as *mut u32).cast(), &mut len, std::ptr::null_mut(), 0) };
    (rc == 0 && level <= 100).then(|| total / 100 * u64::from(level))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn available_bytes() -> Option<u64> {
    None
}

fn load1() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut l = [0f64; 3];
        // SAFETY: getloadavg writes at most 3 doubles into a 3-double array.
        let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
        (n >= 1).then_some(l[0])
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The machine and the agents' activity (injectable for tests).
pub trait Probe: Send + Sync {
    fn machine(&self) -> Machine;
    /// Is this agent mid-turn (or its subagents running)?
    fn busy(&self, project_id: &str) -> bool;
}

impl<T: Probe> Probe for Arc<T> {
    fn machine(&self) -> Machine {
        (**self).machine()
    }
    fn busy(&self, project_id: &str) -> bool {
        (**self).busy(project_id)
    }
}

/// This machine, and the activity rows' D1 `busy`.
pub struct RealProbe;

impl Probe for RealProbe {
    fn machine(&self) -> Machine {
        Machine::read()
    }
    fn busy(&self, project_id: &str) -> bool {
        crate::activity_events::displays().values().any(|(_, row)| {
            crate::activity_events::RowView::from_json(row).is_some_and(|v| v.project_id.as_deref() == Some(project_id) && v.is_busy())
        })
    }
}

/// One wake holding a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// Unique per wake (`org:night_run:<agent>`).
    pub key: String,
    /// Who made it (`org_chart`; later `heartbeat`, `reminder`).
    pub source: &'static str,
    /// The source's word for it (`night_run`, `last_call`, `ask`).
    pub kind: String,
    pub project_id: String,
    pub started_at: i64,
    pub cap_secs: i64,
    /// The agent was seen working since it started.
    pub seen_busy: bool,
}

impl Slot {
    pub fn new(source: &'static str, kind: &str, project_id: &str, started_at: i64, cap_secs: i64) -> Slot {
        Slot {
            key: format!("{source}:{kind}:{project_id}"),
            source,
            kind: kind.to_string(),
            project_id: project_id.to_string(),
            started_at,
            cap_secs,
            seen_busy: false,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "source": self.source, "kind": self.kind, "projectId": self.project_id, "startedAt": self.started_at, "capSecs": self.cap_secs })
    }
}

/// Why a claim waits.
#[derive(Debug, Clone, PartialEq)]
pub enum Hold {
    /// Every slot is taken.
    Full { running: usize, slots: usize },
    /// The 1-minute load is above the cores.
    Load { load1: f64, cores: usize },
    /// This agent already holds a slot.
    AgentBusy,
}

impl Hold {
    pub fn word(&self) -> String {
        match self {
            Hold::Full { running, slots } => format!("all {slots} slot{} taken ({running} running)", if *slots == 1 { "" } else { "s" }),
            Hold::Load { load1, cores } => format!("load {load1:.1} is above {cores} cores"),
            Hold::AgentBusy => "that agent is already in a wake".to_string(),
        }
    }
}

#[derive(Default)]
struct Inner {
    running: Vec<Slot>,
    adopted: HashSet<&'static str>,
    /// When the last "holding for load" line was logged (`None` = not
    /// holding).
    load_logged_at: Option<i64>,
}

/// The governor. One per daemon ([`current`]).
pub struct Governor {
    probe: Box<dyn Probe>,
    inner: Mutex<Inner>,
}

/// What the governor looks like now (`k2 organization night`).
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub machine: Machine,
    pub slots: usize,
    pub running: Vec<Slot>,
    /// Why new starts wait right now, if they do (load).
    pub holding: Option<String>,
}

impl Snapshot {
    pub fn to_json(&self) -> Value {
        json!({
            "machine": self.machine.to_json(self.running.len()),
            "slots": self.slots,
            "running": self.running.iter().map(Slot::to_json).collect::<Vec<_>>(),
            "holding": self.holding,
        })
    }
}

impl Governor {
    pub fn new(probe: Box<dyn Probe>) -> Governor {
        Governor { probe, inner: Mutex::new(Inner::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn machine(&self) -> Machine {
        self.probe.machine()
    }

    /// Take a slot for `slot`, or say why it waits. The caller delivers
    /// only on `Ok`, and calls [`Governor::release`] when the delivery
    /// didn't happen after all.
    pub fn claim(&self, slot: Slot) -> Result<(), Hold> {
        let m = self.probe.machine();
        let now = slot.started_at;
        let mut g = self.lock();
        if g.running.iter().any(|s| s.project_id == slot.project_id || s.key == slot.key) {
            return Err(Hold::AgentBusy);
        }
        if m.overloaded() {
            let load1 = m.load1.unwrap_or_default();
            let due = g.load_logged_at.is_none_or(|t| now - t >= HOLD_LOG_EVERY_SECS);
            if due {
                g.load_logged_at = Some(now);
                k2_core::log_debug!(
                    "[wake-governor] holding new wakes: load {load1:.1} is above {} cores ({} running; {} {} waits)",
                    m.cores,
                    g.running.len(),
                    slot.source,
                    slot.kind
                );
            }
            return Err(Hold::Load { load1, cores: m.cores });
        }
        if g.load_logged_at.take().is_some() {
            k2_core::log_debug!("[wake-governor] load is back under {} cores; starting wakes again", m.cores);
        }
        let slots = m.slots(g.running.len());
        if g.running.len() >= slots {
            return Err(Hold::Full { running: g.running.len(), slots });
        }
        g.running.push(slot);
        Ok(())
    }

    /// Give a slot back (the delivery didn't happen).
    pub fn release(&self, key: &str) {
        self.lock().running.retain(|s| s.key != key);
    }

    /// End `source`'s slots that are done at `now`: the time cap, the
    /// agent idle, or `ended` (the source's own rule). Returns the ended.
    pub fn reap(&self, source: &'static str, now: i64, ended: &dyn Fn(&Slot) -> bool) -> Vec<Slot> {
        let mine: Vec<Slot> = self.lock().running.iter().filter(|s| s.source == source).cloned().collect();
        let mut done = Vec::new();
        let mut seen = Vec::new();
        for s in mine {
            let age = now - s.started_at;
            let busy = self.probe.busy(&s.project_id);
            let finished = age >= s.cap_secs
                || ended(&s)
                || (!busy && (s.seen_busy || age >= IDLE_GRACE_SECS));
            if finished {
                done.push(s);
            } else if busy && !s.seen_busy {
                seen.push(s.key.clone());
            }
        }
        let mut g = self.lock();
        g.running.retain(|s| !done.iter().any(|d| d.key == s.key));
        for s in g.running.iter_mut() {
            if seen.contains(&s.key) {
                s.seen_busy = true;
            }
        }
        done
    }

    /// After a restart: take `slots` (the source's recent wakes from its
    /// own ledger) as running, once per process per source.
    pub fn adopt_once(&self, source: &'static str, slots: impl FnOnce() -> Vec<Slot>) {
        if self.lock().adopted.contains(source) {
            return;
        }
        let found = slots();
        let mut g = self.lock();
        if !g.adopted.insert(source) {
            return;
        }
        for s in found {
            if !g.running.iter().any(|r| r.project_id == s.project_id || r.key == s.key) {
                g.running.push(s);
            }
        }
    }

    pub fn running(&self) -> Vec<Slot> {
        self.lock().running.clone()
    }

    pub fn snapshot(&self) -> Snapshot {
        let machine = self.probe.machine();
        let running = self.running();
        let holding = machine.overloaded().then(|| Hold::Load { load1: machine.load1.unwrap_or_default(), cores: machine.cores }.word());
        Snapshot { slots: machine.slots(running.len()), machine, running, holding }
    }
}

static GLOBAL: OnceLock<Arc<Governor>> = OnceLock::new();

thread_local! {
    static OVERRIDE: RefCell<Option<Arc<Governor>>> = const { RefCell::new(None) };
}

/// The daemon's governor (a test's, on its thread).
pub fn current() -> Arc<Governor> {
    OVERRIDE
        .with(|c| c.borrow().clone())
        .unwrap_or_else(|| GLOBAL.get_or_init(|| Arc::new(Governor::new(Box::new(RealProbe)))).clone())
}

/// Run `f` with `g` as this thread's governor (tests).
#[cfg(test)]
pub(crate) fn with_governor<R>(g: Arc<Governor>, f: impl FnOnce() -> R) -> R {
    let prev = OVERRIDE.with(|c| c.replace(Some(g)));
    let out = f();
    OVERRIDE.with(|c| *c.borrow_mut() = prev);
    out
}

/// A machine and agents a test sets.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeProbe {
    pub machine: Mutex<Machine>,
    pub busy: Mutex<HashSet<String>>,
}

#[cfg(test)]
impl FakeProbe {
    pub fn new(cores: usize, available_gib: Option<u64>, load1: Option<f64>) -> Arc<FakeProbe> {
        Arc::new(FakeProbe {
            machine: Mutex::new(Machine { cores, available_bytes: available_gib.map(|g| g * 1024 * 1024 * 1024), load1 }),
            busy: Mutex::new(HashSet::new()),
        })
    }
    pub fn set_load(&self, l: Option<f64>) {
        self.machine.lock().unwrap().load1 = l;
    }
    pub fn set_busy(&self, id: &str, on: bool) {
        let mut b = self.busy.lock().unwrap();
        if on {
            b.insert(id.to_string());
        } else {
            b.remove(id);
        }
    }
    pub fn governor(self: &Arc<Self>) -> Arc<Governor> {
        Arc::new(Governor::new(Box::new(self.clone())))
    }
}

#[cfg(test)]
impl Probe for FakeProbe {
    fn machine(&self) -> Machine {
        *self.machine.lock().unwrap()
    }
    fn busy(&self, project_id: &str) -> bool {
        self.busy.lock().unwrap().contains(project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn capacity_comes_from_cores_and_memory() {
        let m = |cores, gib: Option<u64>| Machine { cores, available_bytes: gib.map(|g| g * GIB), load1: Some(0.5) };
        assert_eq!(m(1, None).slots(0), 1, "at least one");
        assert_eq!(m(8, None).slots(0), 2);
        assert_eq!(m(16, Some(64)).slots(0), 4);
        assert_eq!(m(64, Some(64)).slots(0), 16, "64 GiB is 42 sessions; the cores cap at 16");
        assert_eq!(m(64, Some(6)).slots(0), 4, "6 GiB free takes 4 more sessions");
        assert_eq!(m(64, Some(6)).slots(3), 7, "…on top of the 3 already running (their memory is spent)");
        assert_eq!(m(64, Some(1)).slots(0), 1, "short of memory: still one");
    }

    #[test]
    fn claims_stop_at_the_cap_and_one_slot_per_agent() {
        let p = FakeProbe::new(8, Some(32), Some(0.5));
        let g = p.governor();
        assert_eq!(g.claim(Slot::new("t", "x", "a", 0, 600)), Ok(()));
        assert_eq!(g.claim(Slot::new("t", "y", "a", 0, 600)), Err(Hold::AgentBusy));
        assert_eq!(g.claim(Slot::new("t", "x", "b", 0, 600)), Ok(()));
        assert_eq!(g.claim(Slot::new("t", "x", "c", 0, 600)), Err(Hold::Full { running: 2, slots: 2 }));
        g.release("t:x:a");
        assert_eq!(g.claim(Slot::new("t", "x", "c", 0, 600)), Ok(()));
    }

    #[test]
    fn load_above_the_cores_holds_new_starts() {
        let p = FakeProbe::new(8, Some(32), Some(8.5));
        let g = p.governor();
        assert_eq!(g.claim(Slot::new("t", "x", "a", 0, 600)), Err(Hold::Load { load1: 8.5, cores: 8 }));
        assert!(g.running().is_empty());
        assert_eq!(g.snapshot().holding.as_deref(), Some("load 8.5 is above 8 cores"));
        p.set_load(Some(8.0));
        assert_eq!(g.claim(Slot::new("t", "x", "a", 60, 600)), Ok(()), "at the cores exactly: starts");
        assert_eq!(g.snapshot().holding, None);
    }

    #[test]
    fn a_slot_ends_when_its_agent_goes_idle_the_source_says_so_or_at_the_cap() {
        let p = FakeProbe::new(64, None, None);
        let g = p.governor();
        for a in ["idle", "worker", "done", "capped"] {
            g.claim(Slot::new("t", "x", a, 0, 1200)).unwrap();
        }
        p.set_busy("worker", true);
        p.set_busy("done", true);
        p.set_busy("capped", true);
        let never = |_: &Slot| false;
        // Just started: nobody's done (the idle one gets its grace).
        assert!(g.reap("t", 60, &never).is_empty());
        // The source ends `done`.
        let ended = g.reap("t", 120, &|s: &Slot| s.project_id == "done");
        assert_eq!(ended.iter().map(|s| s.project_id.as_str()).collect::<Vec<_>>(), ["done"]);
        // Never seen working: done after the grace.
        let ended = g.reap("t", IDLE_GRACE_SECS, &never);
        assert_eq!(ended.iter().map(|s| s.project_id.as_str()).collect::<Vec<_>>(), ["idle"]);
        // Seen working, then not: done at once.
        p.set_busy("worker", false);
        let ended = g.reap("t", IDLE_GRACE_SECS + 60, &never);
        assert_eq!(ended.iter().map(|s| s.project_id.as_str()).collect::<Vec<_>>(), ["worker"]);
        // Busy past the cap: done.
        assert!(g.reap("t", 1199, &never).is_empty());
        let ended = g.reap("t", 1200, &never);
        assert_eq!(ended.iter().map(|s| s.project_id.as_str()).collect::<Vec<_>>(), ["capped"]);
        // Another source's slots are its own to reap.
        g.claim(Slot::new("other", "x", "z", 0, 10)).unwrap();
        assert!(g.reap("t", 10_000, &never).is_empty());
        assert_eq!(g.running().len(), 1);
    }

    #[test]
    fn adoption_after_a_restart_happens_once() {
        let p = FakeProbe::new(64, None, None);
        let g = p.governor();
        g.adopt_once("t", || vec![Slot::new("t", "x", "a", 0, 600)]);
        g.adopt_once("t", || vec![Slot::new("t", "x", "b", 0, 600)]);
        assert_eq!(g.running().iter().map(|s| s.project_id.as_str()).collect::<Vec<_>>(), ["a"]);
    }
}
