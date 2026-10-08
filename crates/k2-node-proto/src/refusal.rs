//! The one refusal function (FICC `refusal()`, §8.5).
//!
//! The node calls [`refusal`] with its live view when an `assign`
//! arrives; the controller's scheduler calls [`offer_refusal`] with the
//! last offer before it places a job. Same order, same codes, so an agent
//! sees one stable reason whichever side said no.

use std::collections::BTreeMap;

use crate::frames::{Control, Offer, Refusal};

pub const LOCAL_PAUSED: &str = "local_paused";
pub const LOCAL_DRAINING: &str = "local_draining";
pub const LOCAL_STOPPED: &str = "local_stopped";
pub const LOCAL_RESOURCE_LIMIT: &str = "local_resource_limit";
pub const OUTSIDE_WINDOW: &str = "outside_window";
pub const OWNER_ACTIVE: &str = "owner_active";
pub const ON_BATTERY: &str = "on_battery";
pub const WORKSPACE_REFUSED: &str = "workspace_refused";
pub const DISK_LOW: &str = "disk_low";
pub const FOREIGN_LOCK: &str = "foreign_lock";
pub const UPGRADE_REQUIRED: &str = "upgrade_required";
pub const TOOL_MISSING: &str = "tool_missing";
pub const PARALLEL_FULL: &str = "parallel_full";
pub const EXCLUSIVE_RUNNING: &str = "exclusive_running";
pub const WAITING_FOR_IDLE: &str = "waiting_for_idle";

/// Every code above, for docs and tests.
pub const ALL_CODES: &[&str] = &[
    LOCAL_PAUSED,
    LOCAL_DRAINING,
    LOCAL_STOPPED,
    LOCAL_RESOURCE_LIMIT,
    OUTSIDE_WINDOW,
    OWNER_ACTIVE,
    ON_BATTERY,
    WORKSPACE_REFUSED,
    DISK_LOW,
    FOREIGN_LOCK,
    UPGRADE_REQUIRED,
    TOOL_MISSING,
    PARALLEL_FULL,
    EXCLUSIVE_RUNNING,
    WAITING_FOR_IDLE,
];

/// The node's live state, as the node sees it.
#[derive(Debug, Clone)]
pub struct NodeView<'a> {
    pub control: Control,
    /// Availability refusal codes right now (empty = available).
    pub unavailable: &'a [String],
    /// A foreign lock file that is present and younger than 12 h.
    pub foreign_lock: Option<&'a str>,
    pub disk_free_bytes: u64,
    pub disk_floor_bytes: u64,
    /// Empty = any workspace the controller grants.
    pub workspaces_allow: &'a [String],
    pub tools: &'a BTreeMap<String, String>,
    pub running: u32,
    pub parallel_max: u32,
    pub exclusive_running: bool,
    pub free_cpu_millis: u64,
    pub free_mem_bytes: u64,
}

/// What a job asks for.
#[derive(Debug, Clone)]
pub struct JobAsk<'a> {
    pub workspace_id: &'a str,
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    pub disk_bytes: u64,
    pub exclusive: bool,
    /// The job syncs code (needs `git`).
    pub needs_git: bool,
}

fn control_refusal(c: Control) -> Option<Refusal> {
    match c {
        Control::Active => None,
        Control::Paused => Some(Refusal::new(LOCAL_PAUSED, "the machine's owner paused this node")),
        Control::Draining => Some(Refusal::new(LOCAL_DRAINING, "the machine's owner is draining this node")),
        Control::Stopped => Some(Refusal::new(LOCAL_STOPPED, "the machine's owner stopped this node")),
    }
}

fn availability_refusal(reasons: &[String]) -> Option<Refusal> {
    let first = reasons.first()?;
    let msg = match first.as_str() {
        OUTSIDE_WINDOW => "outside the node's availability window",
        OWNER_ACTIVE => "the machine's owner is using it",
        ON_BATTERY => "the machine is on battery",
        _ => "the node is not available right now",
    };
    Some(Refusal::new(first, msg))
}

/// Refusals that don't depend on any particular job.
pub fn node_refusal(v: &NodeView<'_>) -> Option<Refusal> {
    if let Some(r) = control_refusal(v.control) {
        return Some(r);
    }
    if let Some(r) = availability_refusal(v.unavailable) {
        return Some(r);
    }
    if let Some(lock) = v.foreign_lock {
        return Some(Refusal::new(FOREIGN_LOCK, format!("another tool holds {lock}")));
    }
    if v.disk_free_bytes < v.disk_floor_bytes {
        return Some(Refusal::new(DISK_LOW, "free disk is under the node's floor"));
    }
    None
}

/// The node's answer to one job: `None` = may start now.
pub fn refusal(v: &NodeView<'_>, ask: &JobAsk<'_>) -> Option<Refusal> {
    if let Some(r) = node_refusal(v) {
        return Some(r);
    }
    if !v.workspaces_allow.is_empty() && !v.workspaces_allow.iter().any(|w| w == ask.workspace_id) {
        return Some(Refusal::new(WORKSPACE_REFUSED, "the machine's owner doesn't allow this workspace"));
    }
    if ask.needs_git && !v.tools.contains_key("git") {
        return Some(Refusal::new(TOOL_MISSING, "git is not installed on the node"));
    }
    if ask.disk_bytes > v.disk_free_bytes.saturating_sub(v.disk_floor_bytes) {
        return Some(Refusal::new(DISK_LOW, "not enough free disk above the floor for this job"));
    }
    if v.exclusive_running {
        return Some(Refusal::new(EXCLUSIVE_RUNNING, "an exclusive job is running"));
    }
    if ask.exclusive && v.running > 0 {
        return Some(Refusal::new(WAITING_FOR_IDLE, "an exclusive job waits until the node is idle"));
    }
    if v.running >= v.parallel_max {
        return Some(Refusal::new(PARALLEL_FULL, "the node is running as many jobs as it allows"));
    }
    if ask.cpu_millis.is_some_and(|c| c > v.free_cpu_millis) || ask.mem_bytes.is_some_and(|m| m > v.free_mem_bytes) {
        return Some(Refusal::new(LOCAL_RESOURCE_LIMIT, "not enough free CPU or memory under the node's caps"));
    }
    None
}

/// The controller's view of the same rule, from the node's last offer.
/// `exclusive_running` and `running` come from the controller's own
/// registry when fresher than the offer.
pub fn offer_refusal(o: &Offer, running: u32, exclusive_running: bool, ask: &JobAsk<'_>) -> Option<Refusal> {
    if o.protocol < crate::frames::PROTOCOL {
        return Some(Refusal::new(UPGRADE_REQUIRED, "the node runs an older protocol; update K2 on it"));
    }
    if let Some(r) = control_refusal(o.control) {
        return Some(r);
    }
    if !o.availability.ok {
        if let Some(r) = availability_refusal(&o.availability.reasons) {
            return Some(r);
        }
        return Some(Refusal::new(OUTSIDE_WINDOW, "the node is not available right now"));
    }
    if let Some(r) = &o.refusal {
        return Some(r.clone());
    }
    let none: [String; 0] = [];
    let v = NodeView {
        control: o.control,
        unavailable: &none,
        foreign_lock: None,
        disk_free_bytes: o.free.disk_free_bytes,
        disk_floor_bytes: 0,
        workspaces_allow: &none,
        tools: &o.tools,
        running,
        parallel_max: o.parallel_max,
        exclusive_running,
        free_cpu_millis: o.free.cpu_millis,
        free_mem_bytes: o.free.mem_bytes,
    };
    refusal(&v, ask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{Availability, Caps, Free};

    fn tools() -> BTreeMap<String, String> {
        BTreeMap::from([("git".to_string(), "2.47".to_string())])
    }

    fn view<'a>(t: &'a BTreeMap<String, String>, un: &'a [String], allow: &'a [String]) -> NodeView<'a> {
        NodeView {
            control: Control::Active,
            unavailable: un,
            foreign_lock: None,
            disk_free_bytes: 100 << 30,
            disk_floor_bytes: 30 << 30,
            workspaces_allow: allow,
            tools: t,
            running: 0,
            parallel_max: 2,
            exclusive_running: false,
            free_cpu_millis: 8000,
            free_mem_bytes: 16 << 30,
        }
    }

    fn ask() -> JobAsk<'static> {
        JobAsk { workspace_id: "w", cpu_millis: None, mem_bytes: None, disk_bytes: 1 << 30, exclusive: false, needs_git: true }
    }

    fn code(r: Option<Refusal>) -> Option<String> {
        r.map(|r| r.code)
    }

    #[test]
    fn a_free_node_takes_the_job() {
        let t = tools();
        assert_eq!(refusal(&view(&t, &[], &[]), &ask()), None);
    }

    #[test]
    fn control_wins_over_everything_and_keeps_its_order() {
        let t = tools();
        let un = vec![ON_BATTERY.to_string()];
        let mut v = view(&t, &un, &[]);
        v.foreign_lock = Some("/x.lock");
        for (c, want) in [(Control::Stopped, LOCAL_STOPPED), (Control::Paused, LOCAL_PAUSED), (Control::Draining, LOCAL_DRAINING)] {
            v.control = c;
            assert_eq!(code(refusal(&v, &ask())).as_deref(), Some(want));
        }
        v.control = Control::Active;
        assert_eq!(code(refusal(&v, &ask())).as_deref(), Some(ON_BATTERY));
        v.unavailable = &[];
        assert_eq!(code(refusal(&v, &ask())).as_deref(), Some(FOREIGN_LOCK));
    }

    #[test]
    fn each_job_rule_has_its_code() {
        let t = tools();
        let allow = vec!["other".to_string()];
        assert_eq!(code(refusal(&view(&t, &[], &allow), &ask())).as_deref(), Some(WORKSPACE_REFUSED));
        let empty = BTreeMap::new();
        assert_eq!(code(refusal(&view(&empty, &[], &[]), &ask())).as_deref(), Some(TOOL_MISSING));
        let mut v = view(&t, &[], &[]);
        v.disk_free_bytes = 30 << 30;
        assert_eq!(code(refusal(&v, &ask())).as_deref(), Some(DISK_LOW));
        let mut v = view(&t, &[], &[]);
        v.running = 2;
        assert_eq!(code(refusal(&v, &ask())).as_deref(), Some(PARALLEL_FULL));
        let mut v = view(&t, &[], &[]);
        v.running = 1;
        let mut a = ask();
        a.exclusive = true;
        assert_eq!(code(refusal(&v, &a)).as_deref(), Some(WAITING_FOR_IDLE));
        v.exclusive_running = true;
        assert_eq!(code(refusal(&v, &ask())).as_deref(), Some(EXCLUSIVE_RUNNING));
        let mut a = ask();
        a.mem_bytes = Some(32 << 30);
        assert_eq!(code(refusal(&view(&t, &[], &[]), &a)).as_deref(), Some(LOCAL_RESOURCE_LIMIT));
    }

    #[test]
    fn offer_refusal_passes_node_codes_through() {
        let o = Offer {
            revision: 1,
            control: Control::Active,
            control_by: None,
            control_at: None,
            availability: Availability { ok: true, reasons: vec![] },
            caps: Caps { cpu_millis: 8000, mem_bytes: 16 << 30, disk_budget_bytes: 100 << 30, hard: true },
            free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 100 << 30 },
            slots: vec![],
            running: 0,
            parallel_max: 1,
            refusal: None,
            labels: BTreeMap::new(),
            tools: tools(),
            protocol: crate::frames::PROTOCOL,
            boot_id: "b".into(),
            ledger_id: "l".into(),
            node_version: "0.1.0".into(),
        };
        assert_eq!(offer_refusal(&o, 0, false, &ask()), None);
        assert_eq!(code(offer_refusal(&o, 1, false, &ask())).as_deref(), Some(PARALLEL_FULL));
        let mut lock = o.clone();
        lock.refusal = Some(Refusal::new(FOREIGN_LOCK, "x"));
        assert_eq!(code(offer_refusal(&lock, 0, false, &ask())).as_deref(), Some(FOREIGN_LOCK));
        let mut paused = o.clone();
        paused.control = Control::Paused;
        assert_eq!(code(offer_refusal(&paused, 0, false, &ask())).as_deref(), Some(LOCAL_PAUSED));
        let mut away = o.clone();
        away.availability = Availability { ok: false, reasons: vec![OUTSIDE_WINDOW.into()] };
        assert_eq!(code(offer_refusal(&away, 0, false, &ask())).as_deref(), Some(OUTSIDE_WINDOW));
        let mut old = o;
        old.protocol = 0;
        assert_eq!(code(offer_refusal(&old, 0, false, &ask())).as_deref(), Some(UPGRADE_REQUIRED));
    }

    #[test]
    fn codes_are_unique() {
        let mut v = ALL_CODES.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), ALL_CODES.len());
    }
}
