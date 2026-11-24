//! Pin day (R28): Sew's `seeds.json` (sew-web `docs/hosting-on-k2.md`,
//! "One-click Seeds"; `scripts/lib/seeds.mjs` `seedEntry`) → one catalog
//! [`Version`] per App. Nothing here downloads: the person pinning a
//! release fetches the release's files, and [`pin_release`] checks every
//! tarball (and the source tarball) against `seeds.json`, unpacks each Seed
//! with the install's own rules and records K2's tree hash (`seedHash`).
//!
//! Sew's setup step names map onto K2's closed vocabulary (R27, A2):
//! `meet-init` → `sew.meet_init` + `sew.app_space_record`;
//! `meetings-enabled` → `sew.policy:meetings.enabled=true`. Any other name
//! is refused (`setup_unknown`): a K2 that doesn't know a step never pins
//! the version.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::{FilePin, Health, Version, VariantPin, SETUP_APP_SPACE_RECORD, SETUP_MEETINGS_ENABLED, SETUP_MEET_INIT};
use crate::deploy::manifest::Requires;
use crate::brand::PRODUCT_NAME;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedsDoc {
    pub schema: u32,
    pub kind: String,
    pub commit: String,
    pub seeds: Vec<SeedRec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedFile {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoxSvc {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthRec {
    pub live: String,
    pub ready: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetupRec {
    pub step: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedRec {
    pub name: String,
    pub app: String,
    pub variant: String,
    pub sew_identity: String,
    pub version: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub content: Option<String>,
    pub commit: String,
    pub requires: serde_json::Value,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub caps: Vec<String>,
    #[serde(default)]
    pub box_services: Vec<BoxSvc>,
    pub health: HealthRec,
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub sew_setup: Vec<SetupRec>,
    /// The App's source tarball (Sew 2026-11-04, from 0.2.4).
    #[serde(default)]
    pub source: Option<NamedFile>,
}

pub fn parse(text: &str) -> Result<SeedsDoc, String> {
    let d: SeedsDoc = serde_json::from_str(text).map_err(|e| format!("seeds.json: {e}"))?;
    if d.schema != 1 || d.kind != "sew-app-seeds" {
        return Err(format!("seeds.json: schema {} kind {} ({PRODUCT_NAME} reads schema 1, sew-app-seeds)", d.schema, d.kind));
    }
    Ok(d)
}

/// Sew's step names → K2's (R27).
pub fn map_setup(steps: &[SetupRec]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for s in steps {
        match s.step.as_str() {
            "meet-init" => {
                out.push(SETUP_MEET_INIT.to_string());
                out.push(SETUP_APP_SPACE_RECORD.to_string());
            }
            "meetings-enabled" => out.push(SETUP_MEETINGS_ENABLED.to_string()),
            other => return Err(format!("{}: Sew's setup step {other} is not one this {PRODUCT_NAME} knows", super::codes::SETUP_UNKNOWN)),
        }
    }
    Ok(out)
}

/// The catalog version of `app` in `doc`, in release `tag`. `seed_hashes`
/// is K2's tree hash per variant (from [`pin_release`]).
pub fn version_for(doc: &SeedsDoc, app: &str, tag: &str, seed_hashes: &BTreeMap<String, String>) -> Result<Version, String> {
    let recs: Vec<&SeedRec> = doc.seeds.iter().filter(|s| s.app == app).collect();
    let first = *recs.first().ok_or_else(|| format!("seeds.json has no Seed for {app}"))?;
    let mut variants = BTreeMap::new();
    let mut source: Option<&NamedFile> = None;
    for r in &recs {
        if r.version != first.version || r.commit != first.commit || r.commit != doc.commit {
            return Err(format!("{}: version/commit differ inside one release", r.name));
        }
        if r.sew_identity != r.variant || !super::VARIANTS.contains(&r.variant.as_str()) {
            return Err(format!("{}: variant {} goes with Sew identity {}", r.name, r.variant, r.sew_identity));
        }
        if r.requires != first.requires || r.health.live != first.health.live || r.health.ready != first.health.ready {
            return Err(format!("{}: requires or health differ between variants", r.name));
        }
        if map_setup(&r.sew_setup)? != map_setup(&first.sew_setup)? {
            return Err(format!("{}: setup differs between variants", r.name));
        }
        match (source, r.source.as_ref()) {
            (None, s) => source = s,
            (Some(a), Some(b)) if a.name == b.name && a.sha256 == b.sha256 && a.size == b.size => {}
            _ => return Err(format!("{}: the source tarball differs between variants", r.name)),
        }
        let seed_hash = seed_hashes.get(&r.variant).ok_or_else(|| format!("{}: no seedHash recorded", r.name))?;
        let pin = VariantPin {
            file: r.name.clone(),
            sha256: r.sha256.to_ascii_lowercase(),
            size: r.size,
            seed_hash: seed_hash.clone(),
            content: r.content.clone(),
            caps: r.caps.clone(),
            box_services: r.box_services.iter().map(|b| b.name.clone()).collect(),
        };
        if variants.insert(r.variant.clone(), pin).is_some() {
            return Err(format!("{}: variant {} twice", r.name, r.variant));
        }
    }
    let source = source.ok_or_else(|| format!("seeds.json names no source tarball for {app}"))?;
    let requires: Requires = serde_json::from_value(first.requires.clone()).map_err(|e| format!("{app}: requires: {e}"))?;
    let data = first
        .data
        .get("files")
        .and_then(|f| f.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(Version {
        version: first.version.clone(),
        tag: tag.to_string(),
        commit: first.commit.clone(),
        requires,
        node: first.node.clone(),
        variants,
        source: FilePin { file: source.name.clone(), sha256: source.sha256.to_ascii_lowercase(), size: source.size },
        health: Health { live: first.health.live.clone(), ready: first.health.ready.clone() },
        data,
        setup: map_setup(&first.sew_setup)?,
    })
}

/// Pin day: `dir` holds the release (`seeds.json`, the four tarballs, the
/// source tarballs). Every file's sha256 and size must equal `seeds.json`;
/// each Seed is unpacked with the install's rules (A13), its inner sums
/// checked, and `seed_hash_of` stores it the way the install does
/// (`deploy::seed::store_folder` under a scratch K2 home) and returns K2's
/// tree hash. `Ok((app, version))` per App.
pub fn pin_release(dir: &Path, tag: &str, seed_hash_of: &dyn Fn(&Path) -> Result<String, String>) -> Result<Vec<(String, Version)>, String> {
    let text = std::fs::read_to_string(dir.join("seeds.json")).map_err(|e| format!("seeds.json: {e}"))?;
    let doc = parse(&text)?;
    let check = |name: &str, sha: &str, size: u64| -> Result<(), String> {
        let p = dir.join(name);
        let len = std::fs::metadata(&p).map_err(|e| format!("{name}: {e}"))?.len();
        let got = super::unpack::sha256_file(&p)?;
        if len != size || !got.eq_ignore_ascii_case(sha) {
            return Err(format!("{name}: sha256 {got} size {len}, seeds.json says {sha} size {size}"));
        }
        Ok(())
    };
    let mut apps: Vec<String> = doc.seeds.iter().map(|s| s.app.clone()).collect();
    apps.sort();
    apps.dedup();
    let mut out = Vec::new();
    for app in apps {
        let mut hashes = BTreeMap::new();
        for r in doc.seeds.iter().filter(|s| s.app == app) {
            check(&r.name, &r.sha256, r.size)?;
            if let Some(s) = &r.source {
                check(&s.name, &s.sha256, s.size)?;
            }
            let scratch = std::env::temp_dir().join(format!("k2-suite-pin-{}", uuid::Uuid::new_v4().simple()));
            super::unpack::unpack_tar_gz(&dir.join(&r.name), &scratch, super::unpack::Limits::seed())
                .map_err(|e| format!("{}: {e}", r.name))?;
            let res = super::unpack::verify_sums(&scratch)
                .map_err(|(f, d)| format!("{}: {f}: {d}", r.name))
                .and_then(|_| seed_hash_of(&scratch));
            let _ = std::fs::remove_dir_all(&scratch);
            hashes.insert(r.variant.clone(), res?);
        }
        let v = version_for(&doc, &app, tag, &hashes)?;
        out.push((app, v));
    }
    Ok(out)
}
