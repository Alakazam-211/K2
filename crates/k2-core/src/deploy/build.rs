//! `k2 app build <folder>` (§8.1, S6): validate the folder, store it as a
//! Seed, record its row (manifest, report, signer, `createdWith`, data
//! revision, source commit). Deploys nothing.
//!
//! Stage 1 snapshots the folder as the agent built it. Running the
//! manifest's `build` inside the wall (registry-only egress, no host
//! credentials) is stage 3 (S16); the seam is [`BuildRunner`].

use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;

use super::seed::{self, SeedError};
use super::store::{self, SeedRow};
use super::validate::{self, Options, Report};

/// Where a manifest `build` runs. Stage 1 has none (the folder is used as
/// built); stage 3 gives the container engine's in-wall builder.
pub trait BuildRunner {
    fn run(&self, folder: &Path, argv: &[String]) -> Result<(), String>;
}

/// What `createdWith` records (§7.1).
pub fn created_with(checker: &str) -> serde_json::Value {
    json!({
        "k2": env!("CARGO_PKG_VERSION"),
        "catalog": crate::contract::catalog().catalog_version,
        "sdk": serde_json::Value::Null,
        "checker": checker,
        "baseImage": serde_json::Value::Null,
        "engine": "process",
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Built {
    pub seed: SeedRow,
    pub reused: bool,
    pub entries: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildFailure {
    pub code: &'static str,
    pub message: String,
    /// Present when validation blocked the build.
    pub report: Option<Report>,
}

impl From<SeedError> for BuildFailure {
    fn from(e: SeedError) -> Self {
        BuildFailure { code: e.code, message: e.message, report: None }
    }
}

/// The git HEAD of `folder` when it is a git checkout (litmus L3).
pub fn git_head(folder: &Path) -> Option<String> {
    let repo = git2::Repository::discover(folder).ok()?;
    let head = repo.head().ok()?;
    head.peel_to_commit().ok().map(|c| c.id().to_string())
}

/// Validate, store and record. `signer` is `owner`, `user:<name>` or
/// `agent:<workspace>`.
pub fn build_folder(conn: &Connection, folder: &Path, signer: &str, opts: &Options) -> Result<Built, BuildFailure> {
    if !folder.is_dir() {
        return Err(BuildFailure {
            code: super::codes::SEED_INVALID,
            message: format!("{} is not a folder", folder.display()),
            report: None,
        });
    }
    let (report, manifest) = validate::validate_tree(folder, opts);
    let Some(m) = manifest.filter(|_| report.ok) else {
        return Err(BuildFailure {
            code: super::codes::SEED_INVALID,
            message: format!("validate found {} error(s); fix them and build again", report.errors.len()),
            report: Some(report),
        });
    };
    let stored = seed::store_folder(folder, &seed::build_excludes(&m.exclude))?;
    let now = super::now();
    let checker = report.checks.iter().find(|c| c.starts_with("types:")).cloned().unwrap_or_else(|| validate::CHECKER.into());
    let row = SeedRow {
        hash: stored.hash.clone(),
        seed_id: m.id.clone(),
        version: Some(m.version.clone()),
        host_mode: m.host.mode.clone(),
        manifest: serde_json::to_value(&m).expect("manifest serializes"),
        created_with: created_with(&format!("{}+{checker}", validate::CHECKER)),
        report: serde_json::to_value(&report).expect("report serializes"),
        signer: signer.to_string(),
        origin: "local".into(),
        built_for: None,
        built_from: None,
        source_ref: git_head(folder),
        data_revision: m.data_revision as i64,
        size_bytes: stored.size_bytes as i64,
        created_at: now,
        last_used_at: now,
    };
    store::upsert_seed(conn, &row).map_err(|e| BuildFailure { code: "db_error", message: e.to_string(), report: None })?;
    let seed = store::get_seed(conn, &stored.hash)
        .map_err(|e| BuildFailure { code: "db_error", message: e.to_string(), report: None })?
        .ok_or_else(|| BuildFailure { code: "db_error", message: "Seed row vanished".into(), report: None })?;
    Ok(Built { seed, reused: stored.reused, entries: stored.entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::TempHome;

    const M: &str = r#"{"schema":1,"kind":"app","id":"jim","name":"Jim","version":"1.0.0","host":{"mode":"cmd","start":["node","s.js"]}}"#;

    #[test]
    fn build_stores_a_seed_with_report_and_created_with() {
        let h = TempHome::new();
        let c = super::super::store::tests::conn();
        let f = h.path().join("app");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("manifest.json"), M).unwrap();
        std::fs::write(f.join("s.js"), "x").unwrap();
        let b = build_folder(&c, &f, "agent:jimws", &Options::default()).expect("build");
        assert!(!b.reused);
        assert_eq!(b.seed.seed_id, "jim");
        assert_eq!(b.seed.signer, "agent:jimws");
        assert_eq!(b.seed.report["ok"], true);
        assert_eq!(b.seed.created_with["k2"], env!("CARGO_PKG_VERSION"));
        assert!(seed::seed_path(&b.seed.hash).join("s.js").is_file());
        // Unchanged folder: same Seed; the first signer stays.
        let again = build_folder(&c, &f, "owner", &Options::default()).unwrap();
        assert_eq!(again.seed.hash, b.seed.hash);
        assert!(again.reused);
        assert_eq!(again.seed.signer, "agent:jimws");
        // A failing validate stores nothing.
        std::fs::write(f.join("manifest.json"), M.replace("\"cmd\"", "\"bogus\"")).unwrap();
        let e = build_folder(&c, &f, "owner", &Options::default()).unwrap_err();
        assert_eq!(e.code, super::super::codes::SEED_INVALID);
        assert!(!e.report.unwrap().ok);
        assert_eq!(store::list_seeds(&c).unwrap().len(), 1);
    }

    #[test]
    fn sweep_keeps_referenced_seeds() {
        let h = TempHome::new();
        let c = super::super::store::tests::conn();
        let mk = |name: &str, body: &str| {
            let f = h.path().join(name);
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(f.join("manifest.json"), M).unwrap();
            std::fs::write(f.join("s.js"), body).unwrap();
            build_folder(&c, &f, "owner", &Options::default()).unwrap().seed.hash
        };
        let live = mk("a", "1");
        let old = mk("b", "2");
        let fresh = mk("c", "3");
        store::insert_app(
            &c,
            &store::NewApp {
                app_id: "jim",
                service_id: "svc",
                seed_id: Some("jim"),
                owner_workspace: "ws",
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 1,
                created_by: "owner",
            },
        )
        .unwrap();
        store::set_live(&c, "jim", Some(&live), None).unwrap();
        c.execute("UPDATE seeds SET last_used_at = 0 WHERE hash != ?1", [&fresh]).unwrap();
        let now = super::super::now();
        let removed = store::sweep_seeds(&c, now, 30 * 86400, 10).unwrap();
        assert_eq!(removed, vec![old.clone()], "only the old unreferenced Seed goes");
        assert!(seed::seed_path(&live).is_dir());
        assert!(seed::seed_path(&fresh).is_dir());
        assert!(!seed::seed_path(&old).exists());
        assert!(store::get_seed(&c, &old).unwrap().is_none());
    }
}
