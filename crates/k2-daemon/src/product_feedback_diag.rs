//! Send feedback to K2 — what the daemon knows about K2 on this computer,
//! for the consented diagnostics (prd-k2-product-feedback-v1 §6a, F6).
//!
//! [`gather`] reads versions, the computer, K2's state (names of the
//! switches that are on, counts, never contents), the vendor AI tools
//! installed (names and versions, never accounts), the page the dialog was
//! opened from, the daemon's log files, and every name the scrub must take
//! out (workspace paths and names, Apps, peers). It runs only when a person
//! ticked the box or opened the preview, or an agent asked while the owner
//! allowed agent diagnostics; `k2_core::product_feedback::diagnostics`
//! builds and scrubs the sections.

use std::time::Duration;

use k2_core::product_feedback::diagnostics::{Facts, Logs};
use k2_core::product_feedback::scrub::{NameBook, Workspace};

/// The vendor AI CLIs whose version the bundle names (never an account).
const AGENT_TOOLS: &[&str] = &["claude", "codex", "gemini", "grok", "cursor-agent", "opencode", "aider", "goose", "qwen", "pi"];
const TOOL_TIMEOUT: Duration = Duration::from_secs(3);

/// What the caller told us about where it is.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// `desktop` | `web` | `cli`.
    pub client: String,
    /// The page the dialog was opened from (`Agents`, `Settings` …).
    pub page: Option<String>,
    /// The desktop app's own version (desktop only).
    pub desktop_version: Option<String>,
    /// The CLI's version (`K2_CLI_VERSION`).
    pub cli_version: Option<String>,
    /// The desktop runs on this computer: include its fault log.
    pub include_client_faults: bool,
}

fn boot_at() -> &'static std::time::Instant {
    static B: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    B.get_or_init(std::time::Instant::now)
}

/// Mark the daemon's start (the drain calls this at boot).
pub fn note_boot() {
    let _ = boot_at();
}

fn human_secs(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d {}h", s / 86_400, (s % 86_400) / 3600),
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
}

fn on_off(b: bool) -> String {
    if b { "on".into() } else { "off".into() }
}

fn count(sql: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|n| n.to_string()).unwrap_or_else(|_| "unknown".into())
}

/// Linux `MemAvailable`; elsewhere not measured.
fn mem_available() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = s.lines().find(|l| l.starts_with("MemAvailable:"))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
}

/// The dotted names of every app setting that is `true` (names only).
fn switches_on() -> Vec<String> {
    fn walk(prefix: &str, v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Bool(true) => out.push(prefix.to_string()),
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                    walk(&p, v, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Ok(v) = serde_json::to_value(k2_core::app_settings::load()) {
        walk("", &v, &mut out);
    }
    out.sort();
    out
}

fn tool_versions() -> Vec<(String, String)> {
    let guard = k2_core::terminal::agent_spawn_guard::GuardEnv::from_process();
    let search = k2_core::terminal::login_path::augmented_path(&k2_core::terminal::login_path::process_path());
    let handles: Vec<_> = AGENT_TOOLS
        .iter()
        .map(|t| {
            let (t, search, guard) = (t.to_string(), search.clone(), guard.clone());
            std::thread::spawn(move || {
                k2_core::sidecar::resolve_binary(&t)?;
                let v = k2_core::agent_hooks::install::probe_version_with(&t, &search, &guard, TOOL_TIMEOUT)
                    .unwrap_or_else(|| "installed (version not read)".into());
                Some((t, v))
            })
        })
        .collect();
    let found: Vec<(String, String)> = handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect();
    if found.is_empty() {
        vec![("Installed".into(), "none found".into())]
    } else {
        found
    }
}

/// Everything the bundle may say, the log files and the scrub's names.
pub fn gather(ctx: &Context) -> (Facts, Logs, NameBook) {
    let home = k2_core::paths::k2_home();
    let mut f = Facts::default();

    // Versions.
    if ctx.client == "desktop" {
        if let Some(v) = &ctx.desktop_version {
            f.versions.push(("Desktop app".into(), v.clone()));
        }
    }
    f.versions.push(("Daemon".into(), env!("CARGO_PKG_VERSION").into()));
    if let Some(v) = &ctx.cli_version {
        f.versions.push(("CLI".into(), v.clone()));
    }
    f.versions.push(("Install kind".into(), crate::boot_status::install_kind().into()));
    if cfg!(target_os = "linux") {
        f.versions.push(("Root helper protocol".into(), k2_root_proto::version::PROTOCOL.to_string()));
    }
    let clusters = crate::sql::cluster::list();
    if !clusters.is_empty() {
        let pg: Vec<String> = clusters
            .iter()
            .map(|c| format!("{} ({}, {})", if c.major == 0 { "?".to_string() } else { c.major.to_string() }, c.role, c.state))
            .collect();
        f.versions.push(("Postgres".into(), pg.join(", ")));
    }

    // Computer.
    let hw = k2_core::compute::advertise::probe(&home);
    let os_v = k2_core::product_feedback::os_version();
    f.computer.push(("OS".into(), format!("{} {}", std::env::consts::OS, os_v).trim().to_string()));
    f.computer.push(("Architecture".into(), std::env::consts::ARCH.into()));
    f.computer.push(("CPUs".into(), hw.cpu_cores.to_string()));
    f.computer.push(("Memory total".into(), if hw.mem_bytes > 0 { gb(hw.mem_bytes) } else { "unknown".into() }));
    f.computer.push(("Memory free".into(), mem_available().map(gb).unwrap_or_else(|| "not measured here".into())));
    if hw.disk_total_bytes > 0 {
        let pct = hw.disk_free_bytes as f64 * 100.0 / hw.disk_total_bytes as f64;
        f.computer.push(("Disk free (K2's volume)".into(), format!("{} ({pct:.0}%)", gb(hw.disk_free_bytes))));
    }

    // K2 state: names and counts, never contents.
    f.k2_state.push(("Daemon uptime".into(), human_secs(boot_at().elapsed().as_secs())));
    f.k2_state.push(("Last restart reason".into(), "not recorded by this version".into()));
    let paired = crate::product_feedback_drain::tunnel_creds().is_some();
    f.k2_state.push(("K2 Connect paired".into(), if paired { "yes".into() } else { "no".into() }));
    f.k2_state.push(("Tunnel".into(), if k2_core::tunnel::connector::status().running { "up".into() } else { "down".into() }));
    let seen = crate::product_feedback_drain::service_seen().map(|s| s.state).unwrap_or("not tried yet");
    f.k2_state.push(("Feedback service".into(), seen.into()));
    f.k2_state.push(("Air-gap".into(), on_off(k2_core::airgap::enabled())));
    let sw = switches_on();
    f.k2_state.push(("Settings that are on".into(), if sw.is_empty() { "none".into() } else { sw.join(", ") }));
    let ptyd = k2_core::terminal::ptyd::Switch::resolve() == k2_core::terminal::ptyd::Switch::On;
    f.k2_state.push(("k2-ptyd".into(), on_off(ptyd)));
    f.k2_state.push(("Workspaces".into(), count("SELECT COUNT(*) FROM projects")));
    f.k2_state.push(("Running sessions".into(), crate::v2_session_map::snapshot().len().to_string()));
    f.k2_state.push(("Apps".into(), count("SELECT COUNT(*) FROM apps")));
    f.k2_state.push(("Compute nodes".into(), count("SELECT COUNT(*) FROM compute_nodes")));

    f.agent_tools = tool_versions();

    f.context.push(("Client".into(), ctx.client.clone()));
    if let Some(p) = ctx.page.as_deref().and_then(|p| k2_core::product_feedback::short_field(p, 40)) {
        f.context.push(("Page".into(), p));
    }

    let logs = Logs {
        daemon: vec![home.join("daemon.stderr.log"), home.join("daemon.stdout.log")],
        client_faults: (ctx.include_client_faults && ctx.client == "desktop").then(|| home.join("client-event-faults.log")),
    };
    (f, logs, name_book())
}

/// Every name the scrub replaces: workspaces (path, name, handle), Apps,
/// federation peers; this box's own Connect labels stay.
pub fn name_book() -> NameBook {
    let (workspaces, apps) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ws: Vec<Workspace> = conn
            .prepare("SELECT name, path, handle FROM projects ORDER BY name, id")
            .and_then(|mut s| {
                let rows = s.query_map([], |r| {
                    Ok(Workspace { name: r.get(0)?, path: r.get(1)?, handle: r.get(2)? })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        let apps: Vec<String> = conn
            .prepare("SELECT app_id FROM apps ORDER BY app_id")
            .and_then(|mut s| {
                let rows = s.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        (ws, apps)
    };
    let mut peers: Vec<String> = Vec::new();
    if let Ok(store) = k2_core::federation::PeerStore::load() {
        for p in store.list() {
            for n in [&p.label, &p.subdomain] {
                let n = n.trim();
                if !n.is_empty() && !peers.iter().any(|x| x == n) {
                    peers.push(n.to_string());
                }
            }
        }
    }
    let mut own: Vec<String> = Vec::new();
    if let Ok(cfg) = k2_core::tunnel::config::load() {
        if !cfg.subdomain.trim().is_empty() {
            own.push(cfg.subdomain.trim().to_string());
        }
    }
    own.extend(k2_core::tunnel::custom_names::snapshot().0);
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string());
    NameBook::new(&workspaces, &apps, &[], &peers, &own, home.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_durations() {
        assert_eq!(human_secs(5), "5s");
        assert_eq!(human_secs(125), "2m");
        assert_eq!(human_secs(3 * 3600 + 120), "3h 2m");
        assert_eq!(human_secs(2 * 86_400 + 3600), "2d 1h");
    }
}
