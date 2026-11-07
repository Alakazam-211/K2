//! The signer's registries. Two are **shared with k2sign** so the two
//! signers can never disagree (vs-live V28, H4):
//!
//! - **one sha per asset per version** (`state/assets/<version>.tsv` for
//!   project `k2`, k2sign's file; `k2-signer/projects/<p>/assets/` for a
//!   client): the first bytes signed as an asset are the only bytes ever
//!   signed as it for that version;
//! - **receipt replay** (`state/compute/receipts.tsv`, one file for every
//!   project): a receipt body registered for one version never registers for
//!   another. Lines stay `rid<TAB>version` for `k2` (k2sign reads two
//!   columns) and `rid<TAB><project>/<version>` for a client, which k2sign
//!   reads as "another version" (a replay): fail closed both ways.
//!
//! And one is k2-signer's own: **registered evidence**
//! (`k2-signer/evidence/<project>/<version>.json`), never replaced. The same
//! bundle again is checked again and kept; any other bundle is refused until
//! root clears the version (`clear-evidence`, audited).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ctx::{self, Ctx};
use crate::{refused, Fail, R};

pub fn project_dir(ctx: &Ctx, project: &str) -> PathBuf {
    ctx.own_dir().join("projects").join(project)
}

/// Where a version's audit lines go (k2sign's dir for project `k2`).
pub fn audit_dir(ctx: &Ctx, project: &str, version: &str) -> PathBuf {
    if project == "k2" {
        ctx.var().join("audit").join(version)
    } else {
        ctx.own_dir().join("audit").join(project).join(version)
    }
}

/// The rehearsal audit dir.
pub fn rehearsal_audit_dir(ctx: &Ctx, project: &str) -> PathBuf {
    ctx.own_dir().join("audit").join(project).join("rehearsal")
}

// ── registered evidence ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registered {
    pub project: String,
    pub version: String,
    pub bundle_sha256: String,
    pub registered: i64,
    /// The bundle file's exact text.
    pub bundle: String,
    pub summary: Value,
}

pub fn evidence_path(ctx: &Ctx, project: &str, version: &str) -> PathBuf {
    ctx.own_dir().join("evidence").join(project).join(format!("{version}.json"))
}

pub fn load_evidence(ctx: &Ctx, project: &str, version: &str) -> R<Option<Registered>> {
    let p = evidence_path(ctx, project, version);
    if !p.exists() {
        return Ok(None);
    }
    let r: Registered = serde_json::from_slice(&std::fs::read(&p)?).map_err(|e| Fail::Refused(format!("{}: {e}", p.display())))?;
    if r.project != project || r.version != version {
        return refused(format!("{} is for another project or version", p.display()));
    }
    Ok(Some(r))
}

/// `true` when this exact bundle is already registered (a re-run); a
/// refusal when other evidence is.
pub fn evidence_again(ctx: &Ctx, project: &str, version: &str, bundle_sha: &str) -> R<bool> {
    // k2sign's own S3b evidence for the same K2 version: one evidence per version.
    if project == "k2" && ctx.state().join("compute").join(format!("{version}.json")).exists() {
        return refused(format!(
            "{version} already has k2sign's S3b compute evidence: one version is signed on one evidence (clear it with k2sign-admin clear-compute-evidence {version} first)"
        ));
    }
    match load_evidence(ctx, project, version)? {
        None => Ok(false),
        Some(old) if old.bundle_sha256 == bundle_sha => Ok(true),
        Some(old) => refused(format!(
            "{project} {version} already has other evidence (registered {}, bundle sha256 {}…): registered evidence is never \
             replaced. To register new evidence, root clears it first: sudo /usr/local/libexec/k2-signer/k2-signer-admin clear-evidence {project} {version}",
            ctx::hhmm(old.registered),
            &old.bundle_sha256[..16.min(old.bundle_sha256.len())]
        )),
    }
}

pub fn save_evidence(ctx: &Ctx, r: &Registered) -> R<()> {
    let d = ctx.own_dir().join("evidence");
    ctx::mkdirs(ctx, &ctx.own_dir())?;
    ctx::mkdirs(ctx, &d)?;
    ctx::mkdirs(ctx, &d.join(&r.project))?;
    let text = serde_json::to_vec_pretty(r).map_err(|e| Fail::Internal(e.to_string()))?;
    ctx::write_file(ctx, &evidence_path(ctx, &r.project, &r.version), &text, 0o600)
}

// ── receipt replay (shared) ────────────────────────────────────────────

pub fn receipts_path(ctx: &Ctx) -> PathBuf {
    ctx.state().join("compute").join("receipts.tsv")
}

/// The replay key a version is written under.
pub fn replay_key(project: &str, version: &str) -> String {
    if project == "k2" {
        version.to_string()
    } else {
        format!("{project}/{version}")
    }
}

pub fn load_receipts(ctx: &Ctx) -> R<BTreeMap<String, String>> {
    let p = receipts_path(ctx);
    let mut out = BTreeMap::new();
    if !p.exists() {
        return Ok(out);
    }
    for line in std::fs::read_to_string(&p)?.lines() {
        let Some((rid, v)) = line.split_once('\t') else {
            return refused(format!("{}: a bad line", p.display()));
        };
        out.entry(rid.to_string()).or_insert_with(|| v.to_string());
    }
    Ok(out)
}

/// Refuse a receipt registered for another version (or project).
pub fn check_replays(ctx: &Ctx, project: &str, version: &str, rids: &[(&str, &str)]) -> R<()> {
    let used = load_receipts(ctx)?;
    let key = replay_key(project, version);
    for (label, rid) in rids {
        if let Some(v) = used.get(*rid) {
            if *v != key {
                return refused(format!("{label}: a replayed receipt: it was registered for {v}"));
            }
        }
    }
    Ok(())
}

pub fn record_receipts(ctx: &Ctx, project: &str, version: &str, rids: &[&str]) -> R<()> {
    let used = load_receipts(ctx)?;
    let key = replay_key(project, version);
    ctx::mkdirs(ctx, &ctx.state())?;
    ctx::mkdirs(ctx, &ctx.state().join("compute"))?;
    for rid in rids {
        if !used.contains_key(*rid) {
            ctx::append_line(ctx, &receipts_path(ctx), &format!("{rid}\t{key}"))?;
        }
    }
    Ok(())
}

// ── one sha per asset (shared for project k2) ──────────────────────────

pub fn assets_path(ctx: &Ctx, project: &str, version: &str) -> PathBuf {
    if project == "k2" {
        ctx.state().join("assets").join(format!("{version}.tsv"))
    } else {
        project_dir(ctx, project).join("assets").join(format!("{version}.tsv"))
    }
}

/// asset → the sha256 first signed for it in this version.
pub fn signed_assets(ctx: &Ctx, project: &str, version: &str) -> R<BTreeMap<String, String>> {
    let p = assets_path(ctx, project, version);
    let mut out = BTreeMap::new();
    if !p.exists() {
        return Ok(out);
    }
    for line in std::fs::read_to_string(&p)?.lines() {
        let Some((a, s)) = line.split_once('\t') else {
            return refused(format!("{}: a bad line", p.display()));
        };
        out.entry(a.to_string()).or_insert_with(|| s.to_string());
    }
    Ok(out)
}

pub fn record_asset(ctx: &Ctx, project: &str, version: &str, asset: &str, sha: &str) -> R<()> {
    let p = assets_path(ctx, project, version);
    if let Some(parent) = p.parent() {
        if let Some(pp) = parent.parent() {
            if let Some(ppp) = pp.parent() {
                ctx::mkdirs(ctx, ppp)?;
            }
            ctx::mkdirs(ctx, pp)?;
        }
        ctx::mkdirs(ctx, parent)?;
    }
    ctx::append_line(ctx, &p, &format!("{asset}\t{sha}"))
}
