//! `k2 app resources clean [--dry-run]` (prd-app-resources-disk-guard-v1
//! §18 Slice B, research §4.7; 0.46.4): free only what K2 made and can make
//! again.
//!
//! ```text
//! POST /cli/app/resources/clean {dryRun?}   owner, Admin login, any agent passport
//! ```
//!
//! **What it frees** (each item names its kind, its App or owner, and its
//! size; nothing else is ever touched):
//!
//! | Kind | What | Why it is safe |
//! |---|---|---|
//! | `seed` | the daemon's copy of an unused Seed (`~/.k2/seeds/<hash>`) | the daily sweep's rule (§9.1): not live, not preview, not in the last 10 deployments, not named by a pending request or an unfinished deployment, unused 30 days |
//! | `engineSeed` | the walled engine's copy of such a Seed, or one no daemon row names | the same keep set; the engine refuses one a unit still mounts |
//! | `image` | a container image nothing uses (no container, no unit, not a current base) | Podman loads it again when needed |
//! | `engineLeftover` | the engine's cut-off build tars, image loads, half-written Seeds, over an hour old | temp files of a step that ended |
//! | `rotatedLog` | `*.log.1` beside an App's or a `--cmd` service's log | an old copy of a capped log (never `egress.log.1`, an audit trail) |
//! | `deploymentLog` | the log of an old deployment: not live, not in the App's last 10, ended over 30 days ago | its Seed is past rollback too |
//! | `buildTemp` | `~/.k2/apps/tmp/*` and `~/.k2/apps/images/.fetch-*`, over an hour old | temp files of a build or fetch that ended |
//! | `updateStaging` | an update push's staging copy (`~/.k2/apps/<app>/updates/staging/<release>`) whose release isn't uploading, over a day old | the archive was signed and pushed from the workspace; push again |
//! | `imageArchive` | a base-image archive in `~/.k2/apps/images` for a base K2 no longer pins (not on an air-gapped box) | K2 fetches the current one by digest |
//!
//! **Never:** App data, databases, mail, uploads, workspace folders,
//! transcripts, backups, the K2 Trash, the current base images.
//!
//! **Who:** the owner and Admin logins clean the whole box. An agent (and
//! a K2 shell tab) cleans only what belongs to its own workspace's Apps
//! and `--cmd` services: their rotated and old deployment logs, and unused
//! Seeds only its Apps (or it) made. A real clean by an agent needs
//! "Build and run Apps" (switch 1); a dry run needs only a reader's
//! credential. App passes and Member logins never.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_core::deploy::store;
use k2_core::disk_guard as dg;
use k2_root_proto::engine::{self as wire, CleanAnswer, Request};
use serde::Serialize;
use serde_json::{json, Value as J};

use super::disk_watch::EngineSeeds;
use super::routes::{actor, agent_ws, db_err, json_body, need_build, R, Who};

pub const ROUTE: &str = "/cli/app/resources/clean";

/// Temp files of a step that ended are this old before `clean` takes them.
pub const TEMP_AGE_SECS: i64 = 3600;

/// What `clean` never touches (said in every answer).
pub const NEVER: &str =
    "Never touched: App data, databases, mail, uploads, workspace folders, transcripts, backups, the K2 Trash and the current base images.";

/// One thing `clean` frees (or would).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub kind: &'static str,
    /// A Seed hash (12 characters), an image id, or a path under `~/.k2`
    /// (or under k2app's home for the engine's), never an absolute path.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Allocated bytes; `null` when unknown (never 0 for unknown).
    pub bytes: Option<u64>,
    pub removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Whose things a call may clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The owner, an Admin login.
    Box,
    /// An agent or a K2 shell tab: its own workspace's Apps and services.
    Workspace { id: String, signer: String },
}

impl Scope {
    fn word(&self) -> &'static str {
        match self {
            Scope::Box => "box",
            Scope::Workspace { .. } => "workspace",
        }
    }
}

/// The walled engine's `clean` (the live one talks to the engine; tests
/// record).
pub trait EngineClean {
    /// `ok` (the engine lists `clean`), `old`, `missing`, or an error text.
    fn status(&self) -> String;
    fn clean(&self, dry_run: bool, keep_images: &[String]) -> Result<CleanAnswer, String>;
}

/// The box's walled engine (Linux).
pub struct LiveEngine;

impl EngineClean for LiveEngine {
    fn status(&self) -> String {
        // Tests never reach a real engine (a test box may run one).
        if cfg!(test) || !cfg!(target_os = "linux") || !Path::new(wire::SOCKET).exists() {
            return "missing".into();
        }
        match super::guarded::call(&Request::Version, None, Duration::from_secs(10)) {
            Ok(v) if v["verbs"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(wire::VERB_CLEAN))) => "ok".into(),
            Ok(_) => "old".into(),
            Err(e) => format!("error: {}", e.message),
        }
    }

    fn clean(&self, dry_run: bool, keep_images: &[String]) -> Result<CleanAnswer, String> {
        let a = super::guarded::call(
            &Request::Clean { dry_run, keep_images: keep_images.to_vec() },
            None,
            Duration::from_secs(900),
        )
        .map_err(|e| e.message)?;
        serde_json::from_value(a).map_err(|e| format!("unreadable clean answer: {e}"))
    }
}

/// The base images K2 pins now (kept by the engine's `clean`; their
/// archives are kept here).
pub fn current_bases() -> Vec<super::guarded::BaseImage> {
    let arch = std::env::consts::ARCH;
    [super::guarded::base_image_for(arch), super::guarded::full_image_for(arch)].into_iter().flatten().collect()
}

fn archive_name(b: &super::guarded::BaseImage) -> String {
    let hex = b.digest.trim_start_matches("sha256:");
    format!("{}-{}.oci.tar", b.name, &hex[..12.min(hex.len())])
}

/// Allocated bytes of a file or folder (`None` when it is gone).
fn alloc(p: &Path) -> Option<u64> {
    let opts = super::disk_scan::WalkOpts { skip: &[], deadline: Instant::now() + Duration::from_secs(30) };
    super::disk_scan::walk(p, &opts, 0).map(|f| f.bytes)
}

fn rel(p: &Path) -> String {
    let home = k2_core::paths::k2_home();
    let r = p.strip_prefix(&home).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    // An item's id reads with '/' on every OS (Windows: '\' → '/'; on Unix
    // a '\' is part of a file name and stays).
    if cfg!(windows) { r.replace('\\', "/") } else { r }
}

fn older_than(p: &Path, secs: i64, now: i64) -> bool {
    std::fs::symlink_metadata(p)
        .ok()
        .filter(|m| !m.file_type().is_symlink())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|t| now - t.as_secs() as i64 >= secs)
}

/// A local file or folder K2 may delete.
#[derive(Debug, Clone)]
struct LocalItem {
    item: Item,
    path: PathBuf,
}

fn local(kind: &'static str, app: Option<&str>, path: PathBuf) -> LocalItem {
    LocalItem { item: Item { kind, id: rel(&path), app: app.map(str::to_string), bytes: alloc(&path), ..Item::default() }, path }
}

/// `*.log.1` files in `dir`, but never `egress.log.1`.
fn rotated_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.ends_with(".log.1") && n != "egress.log.1" && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file())
        })
        .collect();
    v.sort();
    v
}

/// The Apps and services a scope covers, and the Seeds it may clean.
struct Picked {
    apps: Vec<store::AppRow>,
    /// Workspace ids whose `--cmd` services' logs are in scope (`None` = every one).
    service_ws: Option<BTreeSet<String>>,
    seeds: Vec<store::SeedRow>,
    /// Old deployments' logs: (app, deployment).
    old_deployments: Vec<(String, String)>,
    /// Every daemon Seed row (the engine's orphans are the hashes not here).
    seed_rows: BTreeSet<String>,
    keep: BTreeSet<String>,
}

/// Does an unused Seed belong to the workspace? The Apps that name it (the
/// App it was built for, its manifest id when that is an App, every
/// deployment to or from it) must all be the workspace's; a Seed no App
/// names belongs to whoever signed it.
fn seed_belongs(conn: &rusqlite::Connection, s: &store::SeedRow, own_apps: &BTreeSet<String>, signer: &str) -> bool {
    let mut apps: BTreeSet<String> = BTreeSet::new();
    if let Some(b) = &s.built_for {
        apps.insert(b.clone());
    }
    if store::get_app(conn, &s.seed_id).ok().flatten().is_some() {
        apps.insert(s.seed_id.clone());
    }
    if let Ok(mut st) = conn.prepare("SELECT DISTINCT app_id FROM deployments WHERE to_seed = ?1 OR from_seed = ?1") {
        if let Ok(rows) = st.query_map(rusqlite::params![s.hash], |r| r.get::<_, String>(0)) {
            apps.extend(rows.flatten());
        }
    }
    if apps.is_empty() {
        return s.signer == signer;
    }
    apps.iter().all(|a| own_apps.contains(a))
}

fn pick(scope: &Scope, now: i64) -> R<Picked> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let all = store::list_apps(&conn, None).map_err(db_err)?;
    let (apps, service_ws) = match scope {
        Scope::Box => (all, None),
        Scope::Workspace { id, .. } => {
            (all.into_iter().filter(|a| &a.owner_workspace == id).collect(), Some(BTreeSet::from([id.clone()])))
        }
    };
    let own: BTreeSet<String> = apps.iter().map(|a| a.app_id.clone()).collect();
    let mut seeds = store::sweep_candidates(&conn, now, store::SWEEP_MAX_AGE_SECS, store::SWEEP_KEEP_DEPLOYMENTS).map_err(db_err)?;
    if let Scope::Workspace { signer, .. } = scope {
        seeds.retain(|s| seed_belongs(&conn, s, &own, signer));
    }
    // Old deployments: ended, not live, not in the App's last 10, over 30
    // days ago (the Seed rule's numbers).
    let mut old_deployments = Vec::new();
    for a in &apps {
        let mut st = conn
            .prepare(
                "SELECT id, COALESCE(finished_at, live_at, started_at) FROM deployments WHERE app_id = ?1 \
                 AND result NOT IN ('pending', 'starting') ORDER BY started_at DESC",
            )
            .map_err(db_err)?;
        let rows: Vec<(String, i64)> =
            st.query_map(rusqlite::params![a.app_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db_err)?.flatten().collect();
        for (i, (dep, ended)) in rows.into_iter().enumerate() {
            if i < store::SWEEP_KEEP_DEPLOYMENTS || a.live_deployment.as_deref() == Some(dep.as_str()) || now - ended < store::SWEEP_MAX_AGE_SECS {
                continue;
            }
            old_deployments.push((a.app_id.clone(), dep));
        }
    }
    let seed_rows = store::list_seeds(&conn).map_err(db_err)?.into_iter().map(|s| s.hash).collect();
    let keep = store::referenced_seeds(&conn, store::SWEEP_KEEP_DEPLOYMENTS).map_err(db_err)?;
    Ok(Picked { apps, service_ws, seeds, old_deployments, seed_rows, keep })
}

/// The local files a scope may clean (logs, temps, stale archives).
fn local_items(scope: &Scope, p: &Picked, now: i64) -> Vec<LocalItem> {
    let mut out = Vec::new();
    for a in &p.apps {
        let logs = k2_core::deploy::app_dir(&a.app_id).join("logs");
        for f in rotated_in(&logs) {
            out.push(local("rotatedLog", Some(&a.app_id), f));
        }
        // 0.46.7: an update push's staging copy whose release ended (failed
        // or a stopped upload) over a day ago.
        for f in super::updates_routes::stale_staging(&a.app_id, now) {
            out.push(local("updateStaging", Some(&a.app_id), f));
        }
    }
    for (app, dep) in &p.old_deployments {
        let log = k2_core::deploy::deployment_log_path(app, dep);
        for f in [log.clone(), log.with_extension("log.1")] {
            if std::fs::symlink_metadata(&f).is_ok_and(|m| m.is_file()) && !out.iter().any(|o: &LocalItem| o.path == f) {
                out.push(local("deploymentLog", Some(app), f));
            }
        }
    }
    let publish = k2_core::paths::k2_home().join("publish-logs");
    if let Ok(rd) = std::fs::read_dir(&publish) {
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|d| d.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let ws = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if p.service_ws.as_ref().is_some_and(|s| !s.contains(&ws)) {
                continue;
            }
            for f in rotated_in(&d) {
                out.push(local("rotatedLog", None, f));
            }
        }
    }
    if *scope == Scope::Box {
        let apps_home = k2_core::paths::k2_home().join("apps");
        if let Ok(rd) = std::fs::read_dir(apps_home.join("tmp")) {
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|f| older_than(f, TEMP_AGE_SECS, now)).collect();
            v.sort();
            out.extend(v.into_iter().map(|f| local("buildTemp", None, f)));
        }
        let images = apps_home.join("images");
        if let Ok(rd) = std::fs::read_dir(&images) {
            let keep: Vec<String> = current_bases().iter().map(archive_name).collect();
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            v.sort();
            for f in v {
                let n = f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                if n.starts_with(".fetch-") && older_than(&f, TEMP_AGE_SECS, now) {
                    out.push(local("buildTemp", None, f));
                } else if n.ends_with(".oci.tar")
                    && !n.starts_with('.')
                    && !keep.contains(&n)
                    && !k2_core::airgap::enabled()
                    && std::fs::symlink_metadata(&f).is_ok_and(|m| m.is_file())
                {
                    out.push(local("imageArchive", None, f));
                }
            }
        }
    }
    out
}

fn short(h: &str) -> String {
    h.chars().take(12).collect()
}

/// The clean, planned and (unless `dry_run`) done. The route's body.
pub(crate) fn run(scope: &Scope, dry_run: bool, now: i64, engine: &dyn EngineClean, seeds_engine: &dyn EngineSeeds, by: &str) -> R<J> {
    let p = pick(scope, now)?;
    let mut items: Vec<Item> = Vec::new();

    // Seeds, both copies.
    let engine_bytes: BTreeMap<String, u64> =
        super::disk_scan::cache_now().engine.and_then(|e| e.usage).map(|u| u.seeds).unwrap_or_default();
    let can_gc = seeds_engine.can_gc();
    if dry_run {
        for s in &p.seeds {
            items.push(Item { kind: "seed", id: short(&s.hash), app: s.built_for.clone(), bytes: u64::try_from(s.size_bytes).ok(), ..Item::default() });
            if can_gc && engine_bytes.contains_key(&s.hash) {
                items.push(Item { kind: "engineSeed", id: short(&s.hash), app: s.built_for.clone(), bytes: engine_bytes.get(&s.hash).copied(), ..Item::default() });
            }
        }
        if *scope == Scope::Box && can_gc {
            for h in seeds_engine.hashes() {
                if !p.seed_rows.contains(&h) && !p.keep.contains(&h) {
                    items.push(Item { kind: "engineSeed", id: short(&h), bytes: engine_bytes.get(&h).copied(), ..Item::default() });
                }
            }
        }
    } else if *scope == Scope::Box {
        // The daily sweep, now: the same rule, both copies, orphans too.
        let r = super::disk_watch::run_sweep(now, seeds_engine);
        for h in &r.seeds {
            let row = p.seeds.iter().find(|s| &s.hash == h);
            items.push(Item {
                kind: "seed",
                id: short(h),
                app: row.and_then(|s| s.built_for.clone()),
                bytes: row.and_then(|s| u64::try_from(s.size_bytes).ok()),
                removed: true,
                ..Item::default()
            });
        }
        for h in &r.engine_removed {
            items.push(Item { kind: "engineSeed", id: short(h), bytes: engine_bytes.get(h).copied(), removed: true, ..Item::default() });
        }
        if let Some(e) = r.error.or(r.engine_error) {
            items.push(Item { kind: "seed", id: "sweep".into(), error: Some(e), ..Item::default() });
        }
    } else {
        // An agent's own unused Seeds only (checked again under the lock).
        let mut gone = Vec::new();
        for s in &p.seeds {
            let aside = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                store::detach_seed(&conn, &s.hash, now, store::SWEEP_MAX_AGE_SECS, store::SWEEP_KEEP_DEPLOYMENTS)
            };
            let mut it = Item { kind: "seed", id: short(&s.hash), app: s.built_for.clone(), bytes: u64::try_from(s.size_bytes).ok(), ..Item::default() };
            match aside {
                Ok(Some(path)) => {
                    k2_core::deploy::seed::remove_detached(&path);
                    it.removed = true;
                    gone.push(s.hash.clone());
                }
                Ok(None) => it.error = Some("in use again since the pick: kept".into()),
                Err(e) => it.error = Some(e.to_string()),
            }
            items.push(it);
        }
        if can_gc && !gone.is_empty() {
            match seeds_engine.gc(&gone) {
                Ok(a) => {
                    for h in a.removed {
                        items.push(Item { kind: "engineSeed", id: short(&h), bytes: engine_bytes.get(&h).copied(), removed: true, ..Item::default() });
                    }
                }
                Err(e) => items.push(Item { kind: "engineSeed", id: "seeds-gc".into(), error: Some(e), ..Item::default() }),
            }
        }
    }

    // Logs, temps, stale archives.
    for mut l in local_items(scope, &p, now) {
        if !dry_run {
            let r = if l.path.is_dir() { std::fs::remove_dir_all(&l.path) } else { std::fs::remove_file(&l.path) };
            match r {
                Ok(()) => l.item.removed = true,
                Err(e) => l.item.error = Some(e.to_string()),
            }
        }
        items.push(l.item);
    }

    // The engine: images and its own leftovers (the box only).
    let mut engine_status = J::Null;
    if *scope == Scope::Box {
        let st = engine.status();
        engine_status = json!(st);
        if st == "ok" {
            let keep: Vec<String> = current_bases().iter().map(|b| b.local()).collect();
            match engine.clean(dry_run, &keep) {
                Ok(a) => {
                    for i in a.images {
                        let id = if i.names.is_empty() { short(i.id.trim_start_matches("sha256:")) } else { i.names.join(", ") };
                        items.push(Item { kind: "image", id, bytes: Some(i.bytes), removed: i.removed, error: i.error, ..Item::default() });
                    }
                    for l in a.leftovers {
                        items.push(Item { kind: "engineLeftover", id: l.name, bytes: Some(l.bytes), removed: l.removed, ..Item::default() });
                    }
                    if let Some(e) = a.images_error {
                        items.push(Item { kind: "image", id: "podman".into(), error: Some(e), ..Item::default() });
                    }
                }
                Err(e) => engine_status = json!(format!("error: {e}")),
            }
        }
    }

    let total: u64 = items.iter().filter(|i| i.error.is_none()).filter_map(|i| i.bytes).sum();
    let freed: u64 = items.iter().filter(|i| i.removed).filter_map(|i| i.bytes).sum();
    let mut by_kind: BTreeMap<&str, u64> = BTreeMap::new();
    for i in items.iter().filter(|i| i.error.is_none()) {
        *by_kind.entry(i.kind).or_insert(0) += i.bytes.unwrap_or(0);
    }
    let engine_note = match engine_status.as_str() {
        Some("old") => Some("the walled engine predates clean: images and its leftovers were not looked at (k2 app engine install)"),
        _ => None,
    };
    if !dry_run {
        dg::log_event(
            "clean",
            json!({ "by": by, "scope": scope.word(), "items": items.iter().filter(|i| i.removed).count(), "freedBytes": freed,
                    "errors": items.iter().filter(|i| i.error.is_some()).count() }),
        );
        let _ = super::disk_scan::refresh_live_if_older(Duration::ZERO);
    }
    let hint = if items.is_empty() {
        "Nothing to clean: K2 found nothing it made and can make again.".to_string()
    } else if dry_run {
        format!("{} item(s), about {}. Run without --dry-run to free them.", items.iter().filter(|i| i.error.is_none()).count(), dg::human_gb(total))
    } else {
        format!("Freed about {}.", dg::human_gb(freed))
    };
    Ok(json!({
        "ok": true,
        "dryRun": dry_run,
        "scope": scope.word(),
        "items": items,
        "totals": { "bytes": total, "freedBytes": freed, "byKind": by_kind },
        "engine": { "status": engine_status, "note": engine_note },
        "never": NEVER,
        "hint": hint,
    }))
}

/// POST `/cli/app/resources/clean {dryRun?}`.
pub(crate) fn route(who: &Who, body: &[u8]) -> R<J> {
    super::resources::need_reader(who)?;
    let v = json_body(body)?;
    let dry_run = v.get("dryRun").and_then(J::as_bool).unwrap_or(false);
    let scope = match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
            // `agent:<handle>` (what the agent's Seeds are signed with) or
            // `shell:<handle>` (signs nothing: only its Apps' Seeds count).
            Scope::Workspace { id: project_id.clone(), signer: actor(who) }
        }
        _ => Scope::Box,
    };
    if !dry_run && agent_ws(who).is_some() {
        need_build(who)?;
    }
    run(&scope, dry_run, k2_core::deploy::now(), &LiveEngine, &super::disk_watch::LiveEngine, &actor(who))
}

#[cfg(test)]
#[path = "disk_clean_tests.rs"]
mod tests;
