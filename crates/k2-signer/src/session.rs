//! Approval sessions: "the person wants <project> <version> at <commit>
//! signed now" (prd-signing-user-v1 §4; PRD §4.5: approvals bound to
//! project, version, base commit and assets).
//!
//! - **Project `k2`** uses k2sign's sessions as they are
//!   (`/var/k2sign/state/sessions/<version>.json`, V28): k2sign's
//!   `challenge`/`approve` open them, both signers read them and count
//!   signatures in the same file.
//! - **Client projects** get the same scheme in k2-signer's own state:
//!   `challenge` writes a `k2-signer-approval` challenge naming the project,
//!   the K2 Release Approve app signs it with its Secure Enclave key after
//!   Touch ID (V29), and `approve` checks that P-256 signature with the
//!   pinned `approver.pem` before it opens the session.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ctx::{self, hhmm, iso, Ctx};
use crate::registry::project_dir;
use crate::{refused, usage, Fail, R};

/// The kind a client challenge carries (k2sign's is `k2-release-approval`).
pub const CHALLENGE_KIND: &str = "k2-signer-approval";
/// A session lasts this long (k2sign's policy `session_hours`).
pub const SESSION_HOURS: i64 = 12;
/// A challenge must be approved within this many minutes, once.
pub const CHALLENGE_MINUTES: i64 = 15;

/// A session (k2sign's JSON shape; `project` only in client sessions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub version: String,
    pub base_commit: String,
    pub assets: Vec<String>,
    #[serde(default)]
    pub windows: bool,
    pub max_signatures: u64,
    pub approved: i64,
    #[serde(default)]
    pub opened: Option<i64>,
    pub expires: i64,
    pub nonce: String,
    #[serde(default)]
    pub reapproval: bool,
    pub by: String,
    pub count: u64,
    pub signed: Vec<String>,
}

impl Session {
    /// The version's first approval (re-approvals keep it).
    pub fn opened(&self) -> i64 {
        self.opened.unwrap_or(self.approved)
    }
}

pub fn session_path(ctx: &Ctx, project: &str, version: &str) -> PathBuf {
    if project == "k2" {
        ctx.state().join("sessions").join(format!("{version}.json"))
    } else {
        project_dir(ctx, project).join("sessions").join(format!("{version}.json"))
    }
}

pub fn load(ctx: &Ctx, project: &str, version: &str) -> R<Option<Session>> {
    let p = session_path(ctx, project, version);
    if !p.exists() {
        return Ok(None);
    }
    let s: Session = serde_json::from_slice(&std::fs::read(&p)?).map_err(|e| Fail::Refused(format!("{}: {e}", p.display())))?;
    if s.version != version || (project != "k2" && s.project.as_deref() != Some(project)) {
        return refused(format!("{} is for another project or version", p.display()));
    }
    Ok(Some(s))
}

/// The approved, unexpired session for `version`.
pub fn load_valid(ctx: &Ctx, project: &str, version: &str) -> R<Session> {
    let Some(s) = load(ctx, project, version)? else {
        return refused(format!("no approved signing session for {project} {version}"));
    };
    if ctx.now() > s.expires {
        return refused(format!("the signing session for {project} {version} expired at {}; re-approve", hhmm(s.expires)));
    }
    Ok(s)
}

pub fn save(ctx: &Ctx, project: &str, s: &Session) -> R<()> {
    let p = session_path(ctx, project, &s.version);
    if let Some(d) = p.parent() {
        ctx::mkdirs(ctx, d)?;
    }
    let text = serde_json::to_vec(s).map_err(|e| Fail::Internal(e.to_string()))?;
    ctx::write_file(ctx, &p, &text, 0o600)
}

/// `x.y.z`, each part 0 or 1–6 digits without a leading zero (k2sign's VERSION_RE).
pub fn check_version(v: &str) -> R<()> {
    let parts: Vec<&str> = v.split('.').collect();
    let ok = parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()) && (p.len() == 1 || !p.starts_with('0')));
    if !ok {
        return usage(format!("bad version {v:?} (x.y.z)"));
    }
    Ok(())
}

fn vtuple(v: &str) -> (u64, u64, u64) {
    let p: Vec<u64> = v.split('.').map(|x| x.parse().unwrap_or(0)).collect();
    (p[0], p[1], p[2])
}

pub fn check_commit(c: &str) -> R<()> {
    if c.len() != 40 || !c.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return usage("base commit must be 40 lowercase hex (git rev-parse HEAD)");
    }
    Ok(())
}

fn last_version_path(ctx: &Ctx, project: &str) -> PathBuf {
    project_dir(ctx, project).join("last_version")
}

/// The rules a new approval must pass (k2sign's `check_new_session`).
pub fn check_new(ctx: &Ctx, project: &str, version: &str, base: &str, assets: &[String], reapprove: bool) -> R<()> {
    if assets.is_empty() {
        return usage("name at least one asset");
    }
    let existing = load(ctx, project, version)?;
    match (&existing, reapprove) {
        (Some(e), false) => {
            return refused(format!(
                "{project} {version} already has an approved session (approved {}, base {}); re-approving needs --reapprove",
                hhmm(e.approved),
                &e.base_commit[..12.min(e.base_commit.len())]
            ))
        }
        (Some(e), true) if e.base_commit != base => {
            return refused(format!("base commit differs from {version}'s approved session ({}, now {})", &e.base_commit[..12], &base[..12]))
        }
        (None, true) => return refused(format!("no earlier session for {project} {version} to re-approve")),
        _ => {}
    }
    let lp = last_version_path(ctx, project);
    if existing.is_none() && lp.exists() {
        let last = std::fs::read_to_string(&lp)?.trim().to_string();
        check_version(&last).map_err(|_| Fail::Refused(format!("{} is corrupt", lp.display())))?;
        if vtuple(version) <= vtuple(&last) {
            return refused(format!("{version} is not newer than {last}"));
        }
    }
    Ok(())
}

/// The challenge text for a client project (canonical JSON, sorted keys).
pub fn challenge(ctx: &Ctx, project: &str, version: &str, base: &str, assets: &[String], reapprove: bool) -> R<String> {
    if project == "k2" {
        return refused("project k2's approvals are k2sign's (k2sign challenge / approve); k2-signer reads its sessions");
    }
    check_new(ctx, project, version, base, assets, reapprove)?;
    let now = ctx.now();
    let nonce = k2_node_proto::crypto::hex(&k2_node_proto::crypto::random_bytes(16).map_err(Fail::Internal)?);
    let c = serde_json::json!({
        "v": 1, "kind": CHALLENGE_KIND, "project": project, "version": version, "base_commit": base,
        "assets": assets, "reapproval": reapprove, "max_signatures": assets.len() as u64 * 2 + 4,
        "expires": iso(now + SESSION_HOURS * 3600), "nonce": nonce, "issued": iso(now),
    });
    let text = k2_node_proto::canonical::to_string(&c).map_err(Fail::Internal)?;
    let rec = serde_json::json!({"challenge": text, "issued": now, "used": false});
    let d = project_dir(ctx, project).join("challenges");
    ctx::mkdirs(ctx, &ctx.own_dir().join("projects"))?;
    ctx::mkdirs(ctx, &project_dir(ctx, project))?;
    ctx::mkdirs(ctx, &d)?;
    ctx::write_file(ctx, &d.join(format!("{nonce}.json")), rec.to_string().as_bytes(), 0o600)?;
    Ok(text)
}

/// Verify the approver's DER P-256 signature over SHA-256(challenge).
pub fn approver_verifies(approver_pem: &str, challenge: &[u8], sig_der: &[u8]) -> bool {
    use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1};
    let Ok(der) = k2_node_proto::crypto::pem_decode("PUBLIC KEY", approver_pem) else { return false };
    let Ok(point) = k2_node_proto::crypto::p256_point_from_spki(&der) else { return false };
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point).verify(challenge, sig_der).is_ok()
}

/// `approve <nonce>`: open the session the challenge describes.
pub fn approve(ctx: &Ctx, project: &str, nonce: &str, sig_der: &[u8]) -> R<Session> {
    if project == "k2" {
        return refused("project k2's approvals are k2sign's");
    }
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return usage("approve <project> <nonce: 32 hex>");
    }
    let pem_path = ctx.etc().join("approver.pem");
    if !pem_path.is_file() {
        return refused("no approver key enrolled (sudo k2-signer-admin set-approver, or adopt-k2sign)");
    }
    let pem = String::from_utf8(ctx::read_trusted(ctx, &pem_path, 4096)?).map_err(|_| Fail::Refused("approver.pem is not text".into()))?;
    let cpath = project_dir(ctx, project).join("challenges").join(format!("{nonce}.json"));
    if !cpath.exists() {
        return refused(format!("unknown challenge {nonce}"));
    }
    let mut rec: serde_json::Value = serde_json::from_slice(&std::fs::read(&cpath)?).map_err(|e| Fail::Internal(e.to_string()))?;
    if rec["used"].as_bool() != Some(false) {
        return refused("challenge already used");
    }
    if ctx.now() > rec["issued"].as_i64().unwrap_or(0) + CHALLENGE_MINUTES * 60 {
        return refused("challenge expired");
    }
    let text = rec["challenge"].as_str().unwrap_or_default().to_string();
    if !approver_verifies(&pem, text.as_bytes(), sig_der) {
        return refused("approval signature does not verify");
    }
    rec["used"] = serde_json::Value::Bool(true);
    ctx::write_file(ctx, &cpath, rec.to_string().as_bytes(), 0o600)?;
    let c: serde_json::Value = serde_json::from_str(&text).map_err(|e| Fail::Internal(e.to_string()))?;
    let version = c["version"].as_str().unwrap_or_default().to_string();
    let base = c["base_commit"].as_str().unwrap_or_default().to_string();
    let assets: Vec<String> = c["assets"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let reapproval = c["reapproval"].as_bool().unwrap_or(false);
    if c["project"].as_str() != Some(project) || c["kind"].as_str() != Some(CHALLENGE_KIND) {
        return refused("the challenge is for another project");
    }
    // The world may have moved since the challenge.
    check_new(ctx, project, &version, &base, &assets, reapproval)?;
    let now = ctx.now();
    let prev = load(ctx, project, &version)?;
    let opened = match (&prev, reapproval) {
        (Some(p), true) => p.opened(),
        _ => now,
    };
    let s = Session {
        project: Some(project.to_string()),
        version: version.clone(),
        base_commit: base,
        max_signatures: c["max_signatures"].as_u64().unwrap_or(0),
        assets,
        windows: false,
        approved: now,
        opened: Some(opened),
        expires: now + SESSION_HOURS * 3600,
        nonce: nonce.to_string(),
        reapproval,
        by: "touchid".into(),
        // As k2sign: a re-approval opens a fresh count (it keeps `opened`).
        count: 0,
        signed: Vec::new(),
    };
    save(ctx, project, &s)?;
    let lp = last_version_path(ctx, project);
    let newer = match std::fs::read_to_string(&lp) {
        Ok(last) => check_version(last.trim()).is_err() || vtuple(&version) > vtuple(last.trim()),
        Err(_) => true,
    };
    if newer {
        ctx::write_file(ctx, &lp, format!("{version}\n").as_bytes(), 0o600)?;
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        for ok in ["0.46.10", "1.2.3", "10.0.0"] {
            check_version(ok).unwrap();
        }
        for bad in ["1.2", "01.2.3", "1.2.3-rc1", "v1.2.3", ""] {
            assert!(check_version(bad).is_err(), "{bad}");
        }
        assert!(vtuple("0.46.10") > vtuple("0.46.9"));
    }
}
