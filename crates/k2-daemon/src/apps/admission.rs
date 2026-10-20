//! Admission-lite (prd-agent-app-permissions-v1 §4.3, N5; runtime §6.6):
//! the sum of App memory budgets on this box stays inside the Apps' pool.
//!
//! ```text
//! pool = box RAM − cores reserve − agents' reserve − headroom
//!        (MemTotal)   (1 GiB)        (25% of RAM, or      (1 GiB)
//!                                     K2_AGENTS_MEMORY_HIGH when set)
//! ```
//!
//! Memory only in 0.46.3 (disk and CPU, a Settings bar and real usage are
//! the full shared budget, 0.46.4). Every App counts by its budget
//! (`apps.memory_limit_bytes`); an App with no limit (an owner's choice,
//! or a row from before 0.46.3) counts as the default 512 MiB. Parked Apps
//! don't run and don't count; unparking re-checks.
//!
//! **The one entry point is [`check_memory`].** Install, config (a memory
//! raise), unpark and hand-over call it here; the switch-2 "raise" path
//! (`quota/set` and budget raises an agent makes on its own Apps) must
//! call it too, before it stores the new budget:
//!
//! ```ignore
//! crate::apps::admission::check_memory(&app.app_id, Some(new_bytes))?;
//! ```
//!
//! A raise or install that doesn't fit is `409 box_budget_full` with the
//! numbers and the biggest holders, each named by its maintaining agent and
//! address (never a folder path), so the caller can `k2 msg` them.
//!
//! The owner sizes the pool directly with `K2_APPS_MEMORY_POOL=6G` in the
//! daemon's environment (the override wins over the formula).

use serde_json::{json, Value as J};

use k2_core::deploy::{self, store};

use super::routes::{db_err, no, Refusal};

/// The refusal code.
pub const BOX_BUDGET_FULL: &str = "box_budget_full";
/// What the K2 cores keep (daemon, Postgres, Stalwart, Sew) in admission-lite.
pub const CORES_RESERVE: i64 = 1 << 30;
/// Room left over on top of everything.
pub const HEADROOM: i64 = 1 << 30;
/// The agents' reserve when the owner hasn't sized it: a quarter of RAM.
/// (The agents' leaf `memory.high` default, 75%, is a throttle ceiling,
/// not a reservation; reserving it would leave a Basic box no Apps.)
pub const AGENTS_SHARE_PCT: i64 = 25;
/// How an App with no limit counts.
pub const UNSET_COUNTS_AS: i64 = super::limits::DEFAULT_MEMORY;
/// How many holders a refusal names.
const HOLDERS_NAMED: usize = 5;

/// The Apps' memory pool on this box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    pub total: i64,
    pub cores: i64,
    pub agents: i64,
    pub headroom: i64,
    /// What the Apps' budgets may add up to.
    pub size: i64,
    /// `formula` | `K2_APPS_MEMORY_POOL` | `unknown` (the box's memory
    /// couldn't be read: admission doesn't refuse on a guess).
    pub source: &'static str,
}

impl Pool {
    pub fn json(&self) -> J {
        json!({
            "totalBytes": self.total,
            "coresReserveBytes": self.cores,
            "agentsReserveBytes": self.agents,
            "headroomBytes": self.headroom,
            "poolBytes": self.size,
            "source": self.source,
        })
    }
}

fn box_total() -> Option<i64> {
    static TOTAL: std::sync::OnceLock<Option<i64>> = std::sync::OnceLock::new();
    *TOTAL.get_or_init(|| {
        let linux = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| k2_core::agents_leaf::mem_total(&m));
        let mac = || {
            let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok()
        };
        linux.or_else(mac).and_then(|b| i64::try_from(b).ok())
    })
}

/// The pool from the box's numbers (pure; the tests drive it).
pub fn pool_from(total: Option<i64>, agents_env: Option<&str>, pool_env: Option<&str>) -> Pool {
    if let Some(p) = pool_env.map(str::trim).filter(|p| !p.is_empty()) {
        if let Ok(Some(size)) = deploy::parse_memory(p) {
            return Pool { total: total.unwrap_or(0), cores: 0, agents: 0, headroom: 0, size, source: "K2_APPS_MEMORY_POOL" };
        }
        k2_core::log_debug!("[apps] K2_APPS_MEMORY_POOL {p:?} isn't a size (6G, 512M); using the formula");
    }
    let Some(total) = total.filter(|t| *t > 0) else {
        return Pool { total: 0, cores: CORES_RESERVE, agents: 0, headroom: HEADROOM, size: i64::MAX, source: "unknown" };
    };
    let agents = agents_env
        .and_then(|raw| k2_core::agents_leaf::budget(Some(raw), Some(total as u64)).ok())
        .and_then(|b| b.parse::<i64>().ok())
        .unwrap_or(total * AGENTS_SHARE_PCT / 100);
    let size = (total - CORES_RESERVE - agents - HEADROOM).max(0);
    Pool { total, cores: CORES_RESERVE, agents, headroom: HEADROOM, size, source: "formula" }
}

#[cfg(test)]
thread_local! {
    static POOL_OVERRIDE: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Tests: the pool fixed at a size on this thread until the guard drops.
#[cfg(test)]
pub(crate) struct PoolGuard;

#[cfg(test)]
impl Drop for PoolGuard {
    fn drop(&mut self) {
        POOL_OVERRIDE.with(|c| c.set(None));
    }
}

#[cfg(test)]
pub(crate) fn pool_for_test(bytes: i64) -> PoolGuard {
    POOL_OVERRIDE.with(|c| c.set(Some(bytes)));
    PoolGuard
}

/// Tests: run `f` with the pool fixed at `bytes` (this thread only).
#[cfg(test)]
pub(crate) fn with_pool<T>(bytes: i64, f: impl FnOnce() -> T) -> T {
    let _g = pool_for_test(bytes);
    f()
}

/// This box's pool now.
pub fn pool() -> Pool {
    #[cfg(test)]
    if let Some(size) = POOL_OVERRIDE.with(|c| c.get()) {
        return Pool { total: size, cores: 0, agents: 0, headroom: 0, size, source: "test" };
    }
    pool_from(box_total(), std::env::var("K2_AGENTS_MEMORY_HIGH").ok().as_deref(), std::env::var("K2_APPS_MEMORY_POOL").ok().as_deref())
}

/// How one App counts against the pool.
pub fn counted(limit: Option<i64>) -> i64 {
    limit.unwrap_or(UNSET_COUNTS_AS)
}

/// A fit: the numbers after the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fit {
    pub pool: Pool,
    /// Every other App's budget (parked Apps don't count).
    pub others: i64,
    pub asked: i64,
}

impl Fit {
    pub fn json(&self) -> J {
        json!({
            "pool": self.pool.json(),
            "othersBytes": self.others,
            "askedBytes": self.asked,
            "freeAfterBytes": self.pool.size.saturating_sub(self.others).saturating_sub(self.asked),
        })
    }
}

/// **Admission-lite.** Would App `app_id` with memory budget
/// `new_limit_bytes` (`None` = no limit, counted as 512 MiB) fit in the
/// box's pool next to every other App? The App may not exist yet (an
/// install). Reads only; the caller stores the budget after `Ok`.
///
/// `Err` is `409 box_budget_full` naming the biggest holders by agent and
/// address. Call it before storing any memory budget that grows (install,
/// config raise, quota/budget raise, unpark, hand-over).
pub(crate) fn check_memory(app_id: &str, new_limit_bytes: Option<i64>) -> Result<Fit, Refusal> {
    let pool = pool();
    let apps = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_apps(&conn, None).map_err(db_err)?
    };
    let mut others: Vec<&store::AppRow> =
        apps.iter().filter(|a| a.app_id != app_id && a.state != store::STATE_PARKED).collect();
    let used: i64 = others.iter().map(|a| counted(a.memory_limit_bytes)).sum();
    let asked = counted(new_limit_bytes);
    let fit = Fit { pool: pool.clone(), others: used, asked };
    if used.saturating_add(asked) <= pool.size {
        return Ok(fit);
    }
    others.sort_by(|a, b| counted(b.memory_limit_bytes).cmp(&counted(a.memory_limit_bytes)).then(a.app_id.cmp(&b.app_id)));
    let holders: Vec<J> = others
        .iter()
        .take(HOLDERS_NAMED)
        .map(|a| {
            let m = super::coordination::maintainer_of(&a.owner_workspace);
            json!({
                "app": a.app_id,
                "memoryBytes": counted(a.memory_limit_bytes),
                "limitSet": a.memory_limit_bytes.is_some(),
                "agent": m.agent,
                "address": m.address,
            })
        })
        .collect();
    let named: Vec<String> = others
        .iter()
        .take(3)
        .map(|a| {
            let m = super::coordination::maintainer_of(&a.owner_workspace);
            format!("{} {} MiB ({})", a.app_id, counted(a.memory_limit_bytes) >> 20, m.phrase())
        })
        .collect();
    let mib = |b: i64| b >> 20;
    let mut r = no(
        BOX_BUDGET_FULL,
        409,
        format!(
            "this box's App memory is full: {} MiB asked, {} MiB free of {} MiB (Apps hold {} MiB). The biggest: {}. \
             Ask an agent to lower its App (k2 msg <address> …), lower one of yours, or ask the owner to size the pool \
             (K2_APPS_MEMORY_POOL).",
            mib(asked),
            mib(pool.size.saturating_sub(used).max(0)),
            mib(pool.size),
            mib(used),
            if named.is_empty() { "none".to_string() } else { named.join("; ") },
        ),
    );
    let mut extra = fit.json();
    extra["holders"] = json!(holders);
    r.extra = Some(extra);
    Err(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: i64 = 1 << 30;

    #[test]
    fn the_pool_is_ram_less_cores_agents_and_headroom() {
        let p = pool_from(Some(16 * G), None, None);
        assert_eq!((p.cores, p.agents, p.headroom, p.size, p.source), (G, 4 * G, G, 10 * G, "formula"));
        let p = pool_from(Some(8 * G), Some("3G"), None);
        assert_eq!((p.agents, p.size), (3 * G, 3 * G), "the owner's agents budget is reserved");
        let p = pool_from(Some(8 * G), Some("max"), None);
        assert_eq!(p.agents, 2 * G, "max is no reservation: the quarter stands");
        let p = pool_from(Some(2 * G), None, None);
        assert_eq!(p.size, 0, "never negative");
        let p = pool_from(Some(16 * G), None, Some("6G"));
        assert_eq!((p.size, p.source), (6 * G, "K2_APPS_MEMORY_POOL"));
        let p = pool_from(None, None, None);
        assert_eq!((p.size, p.source), (i64::MAX, "unknown"), "an unreadable box never refuses on a guess");
    }

    #[test]
    fn an_app_with_no_limit_counts_as_the_default() {
        assert_eq!(counted(None), 512 << 20);
        assert_eq!(counted(Some(G)), G);
    }
}
