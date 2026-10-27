//! The one-time carry of widget signatures to the 0.46.5 version hash
//! (prd-widget-size-and-shared-libs-v1 vs-live L1, slice S0; migration
//! 0231).
//!
//! From 0.46.5 a widget's version hash covers its manifest's run fields
//! (`widgets::version_digest`) as well as its bundled document, so a
//! manifest-only change (a new cap, a neighbour read, a library or a CDN
//! file) is a new version that needs a sign, like a code change. Every
//! existing widget therefore gets a new hash at upgrade. Once, at the first
//! boot, this pass:
//!
//! - **carries each signature** (`zen_widget_authors`) from the old hash
//!   to the new one: the folder's live version, its last good copy, and
//!   every locked Garden's frozen copy (same maker, same signing time,
//!   `source = import`). An unsigned version stays unsigned;
//! - **carries even when the live manifest differs** from the one saved
//!   with the signed version (`last-good.json`), and logs each difference
//!   (Rosson 2026-10-26: "so nothing on his canvas breaks"): a row in
//!   `zen_version_carry`, one Access Audit line, and one `k2 zen news`
//!   notice;
//! - **rewrites each old saved copy** (`last-good.json`, a frozen copy's
//!   `last-good.json`) in the new shape, keeping when it was made good. A
//!   saved copy in the old shape is never read after this
//!   (`widget_store::read_saved_bundle`), so an old-shape file put on disk
//!   later, with an edited manifest, is refused instead of run.
//!
//! The run is recorded in `zen_version_carry_runs`; a second boot does
//! nothing. Stamps and the record are written in one transaction before
//! any file is rewritten: a crash between leaves old-shape files that are
//! refused (each one is logged), never a wider gate.
//!
//! Honest bar (as `garden_data`): a guardrail against mistakes and injected
//! prompts, not a wall against a same-user process.

use std::fs;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value as J};

use super::lock::LOCKED_WIDGETS_DIR;
use super::store::{valid_garden_id, ZenFiles};
use super::widget_store::{read_legacy_saved_bundle, write_saved_bundle_at, WidgetState};
use super::widgets::{self, Manifest};
use crate::garden_data::{authors, DataError, MadeBy};

/// The Access Audit event: one line per run, one per difference.
pub const AUDIT_EVENT: &str = "zen-widget-version-carry";

/// The `k2 zen news` notice id (written only when there is something to say).
pub const NOTICE_ID: &str = "notice:widget-version-carry";

/// Where a carried version lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Live,
    LastGood,
    Frozen,
}

impl Place {
    fn as_str(self) -> &'static str {
        match self {
            Place::Live => "live",
            Place::LastGood => "last_good",
            Place::Frozen => "frozen",
        }
    }
}

/// One version the pass looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Carried {
    pub widget: String,
    pub place: Place,
    /// The locked Garden, for a frozen copy.
    pub garden: Option<String>,
    pub old_hash: String,
    pub new_hash: String,
    /// `owner` / `agent:<id>` when a signature moved.
    pub maker: Option<String>,
    /// How the live manifest differs from the signed one.
    pub differs: Option<String>,
}

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CarryReport {
    pub versions: Vec<Carried>,
    /// Saved copies rewritten in the new shape.
    pub rewritten: usize,
    /// Saved copies that couldn't be rewritten (refused from now on).
    pub failed: Vec<String>,
}

impl CarryReport {
    pub fn carried(&self) -> usize {
        self.versions.iter().filter(|v| v.maker.is_some()).count()
    }

    pub fn differing(&self) -> Vec<&Carried> {
        self.versions.iter().filter(|v| v.differs.is_some()).collect()
    }
}

fn maker_label(m: &MadeBy) -> String {
    match m {
        MadeBy::Owner => "owner".into(),
        MadeBy::Agent { project_id } => format!("agent:{project_id}"),
        MadeBy::K2 => "k2".into(),
        MadeBy::Unclaimed => "unclaimed".into(),
    }
}

/// Has the carry run on this database?
pub fn ran(conn: &Connection) -> Result<bool, DataError> {
    Ok(conn.query_row("SELECT 1 FROM zen_version_carry_runs WHERE id = 1", [], |_| Ok(())).optional()?.is_some())
}

/// How the run fields of `live` differ from `signed` (what was saved with
/// the signed version), in a short line; `None` when they don't.
pub fn manifest_diff(signed: &Manifest, live: &Manifest) -> Option<String> {
    if widgets::version_digest(signed) == widgets::version_digest(live) {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let added = |a: &[String], b: &[String]| -> Vec<String> { b.iter().filter(|x| !a.contains(x)).cloned().collect() };
    let caps_in = added(&signed.caps, &live.caps);
    let caps_out = added(&live.caps, &signed.caps);
    if !caps_in.is_empty() {
        parts.push(format!("caps +{}", caps_in.join(" +")));
    }
    if !caps_out.is_empty() {
        parts.push(format!("caps -{}", caps_out.join(" -")));
    }
    let reads_in = added(&signed.reads, &live.reads);
    let reads_out = added(&live.reads, &signed.reads);
    if !reads_in.is_empty() {
        parts.push(format!("reads +{}", reads_in.join(" +")));
    }
    if !reads_out.is_empty() {
        parts.push(format!("reads -{}", reads_out.join(" -")));
    }
    if signed.libs != live.libs {
        let show = |l: &[J]| l.iter().map(|x| x.as_str().map_or_else(|| x["url"].as_str().unwrap_or("?").to_string(), str::to_string)).collect::<Vec<_>>().join(", ");
        parts.push(format!("libs [{}] → [{}]", show(&signed.libs), show(&live.libs)));
    }
    if signed.sources != live.sources {
        parts.push("sources changed".into());
    }
    if signed.entry != live.entry {
        parts.push(format!("entry {} → {}", signed.entry, live.entry));
    }
    if signed.color_scheme != live.color_scheme {
        parts.push("colorScheme changed".into());
    }
    if parts.is_empty() {
        // Only order (caps or reads sorted the same) can't get here; keep a
        // line anyway so a difference is never silent.
        parts.push("run fields changed".into());
    }
    Some(parts.join("; "))
}

/// A saved copy to carry and rewrite.
struct Saved {
    dir: std::path::PathBuf,
    manifest: Manifest,
    bundle: super::bundle::Bundle,
    at: String,
}

/// Run the carry once on `conn` (callers hold the shared DB lock) over the
/// Zen folder `files`. `Ok(None)` when it already ran. The folder may not
/// exist (Zen never set up): the run is recorded with nothing carried.
pub fn carry_once(conn: &Connection, files: &ZenFiles, now: i64) -> Result<Option<CarryReport>, DataError> {
    if ran(conn)? {
        return Ok(None);
    }
    let mut report = CarryReport::default();
    let mut saved: Vec<Saved> = Vec::new();
    let tx = conn.unchecked_transaction()?;
    if files.exists() {
        for name in files.widget_names() {
            let history = files.widget_history_dir(&name);
            let legacy = read_legacy_saved_bundle(&history);
            let st = files.widget_status(&name);
            if st.state == WidgetState::Ok {
                if let (Some(m), Some(b)) = (&st.manifest, &st.bundle) {
                    let old = b.template_hash();
                    // What was signed: the copy saved when this document became good.
                    let differs = legacy
                        .as_ref()
                        .filter(|(_, _, _, lh)| *lh == old)
                        .and_then(|(lm, _, _, _)| manifest_diff(lm, m));
                    let maker = authors::carry(&tx, &name, &old, &b.hash)?;
                    report.versions.push(Carried {
                        widget: name.clone(),
                        place: Place::Live,
                        garden: None,
                        old_hash: old,
                        new_hash: b.hash.clone(),
                        maker: maker.as_ref().map(maker_label),
                        differs,
                    });
                }
            }
            if let Some((m, b, at, old)) = legacy {
                let maker = authors::carry(&tx, &name, &old, &b.hash)?;
                report.versions.push(Carried {
                    widget: name.clone(),
                    place: Place::LastGood,
                    garden: None,
                    old_hash: old,
                    new_hash: b.hash.clone(),
                    maker: maker.as_ref().map(maker_label),
                    differs: None,
                });
                saved.push(Saved { dir: history, manifest: m, bundle: b, at });
            }
        }
        // Every locked Garden's frozen copies (`.locks/<id>/widgets/<name>/`).
        let locks = files.locks_dir();
        let mut gardens: Vec<String> = fs::read_dir(&locks)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| valid_garden_id(n)).collect())
            .unwrap_or_default();
        gardens.sort();
        for g in gardens {
            let wdir = locks.join(&g).join(LOCKED_WIDGETS_DIR);
            let mut names: Vec<String> = fs::read_dir(&wdir)
                .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| super::valid_theme_name(n)).collect())
                .unwrap_or_default();
            names.sort();
            for name in names {
                let dir = wdir.join(&name);
                let Some((m, b, at, old)) = read_legacy_saved_bundle(&dir) else { continue };
                let maker = authors::carry(&tx, &name, &old, &b.hash)?;
                report.versions.push(Carried {
                    widget: name.clone(),
                    place: Place::Frozen,
                    garden: Some(g.clone()),
                    old_hash: old,
                    new_hash: b.hash.clone(),
                    maker: maker.as_ref().map(maker_label),
                    differs: None,
                });
                saved.push(Saved { dir, manifest: m, bundle: b, at });
            }
        }
    }
    for v in &report.versions {
        tx.execute(
            "INSERT INTO zen_version_carry (widget, place, garden, old_hash, new_hash, carried, maker, differs, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                v.widget,
                v.place.as_str(),
                v.garden,
                v.old_hash,
                v.new_hash,
                v.maker.is_some() as i64,
                v.maker,
                v.differs,
                now
            ],
        )?;
    }
    let summary = json!({
        "versions": report.versions.len(),
        "carried": report.carried(),
        "differing": report.differing().len(),
        "savedCopies": saved.len(),
    });
    tx.execute(
        "INSERT INTO zen_version_carry_runs (id, ran_at, report) VALUES (1, ?1, ?2)",
        params![now, summary.to_string()],
    )?;
    tx.commit()?;
    // The files after the commit: a failure leaves an old-shape copy that
    // is refused from now on (logged), never one that runs unchecked.
    for s in saved {
        match write_saved_bundle_at(&s.dir, &s.manifest, &s.bundle, &s.at) {
            Ok(()) => report.rewritten += 1,
            Err(e) => {
                crate::log_debug!("[zen/version-carry] couldn't rewrite {}: {e}; that saved copy is not used from now on", s.dir.display());
                report.failed.push(s.dir.display().to_string());
            }
        }
    }
    for v in report.differing() {
        crate::log_debug!(
            "[zen/version-carry] {}: carried {}'s signature although the live manifest differs from the signed one: {}",
            v.widget,
            v.maker.as_deref().unwrap_or("nobody"),
            v.differs.as_deref().unwrap_or_default()
        );
        crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
            AUDIT_EVENT,
            "k2",
            format!(
                "differs widget={} new={} maker={} {}",
                v.widget,
                &v.new_hash[..12],
                v.maker.as_deref().unwrap_or("none"),
                v.differs.as_deref().unwrap_or_default()
            ),
            "-",
            "-",
            "daemon",
        ));
    }
    crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
        AUDIT_EVENT,
        "k2",
        format!(
            "ok versions={} carried={} differing={} rewritten={} failed={}",
            report.versions.len(),
            report.carried(),
            report.differing().len(),
            report.rewritten,
            report.failed.len()
        ),
        "-",
        "-",
        "daemon",
    ));
    if files.is_set_up() {
        if let Some(n) = notice(&report) {
            if let Err(e) = files.record_notice(n) {
                crate::log_debug!("[zen/version-carry] news notice: {e}");
            }
        }
    }
    Ok(Some(report))
}

/// The `k2 zen news` line: only when a live manifest differs from its
/// signed one, or a saved copy couldn't be rewritten.
pub fn notice(r: &CarryReport) -> Option<super::news::NoticeItem> {
    let differing = r.differing();
    if differing.is_empty() && r.failed.is_empty() {
        return None;
    }
    let mut detail: Vec<String> = differing
        .iter()
        .map(|v| format!("{}: signed by {}; {}", v.widget, v.maker.as_deref().unwrap_or("nobody"), v.differs.as_deref().unwrap_or_default()))
        .collect();
    detail.extend(r.failed.iter().map(|p| format!("couldn't update {p}; that saved copy isn't used")));
    let mut body = String::from(
        "From this K2, a widget's manifest (its caps, the data it reads, its libraries) is part of the version you sign, like its code.",
    );
    if !differing.is_empty() {
        body.push_str(&format!(
            " K2 kept the signature on {} widget{} whose manifest changed after it was signed; check the changes below, and sign again in its History if they're yours.",
            differing.len(),
            if differing.len() == 1 { "" } else { "s" }
        ));
    }
    Some(super::news::NoticeItem {
        id: NOTICE_ID.into(),
        title: "Widget manifests are now signed with their code".into(),
        body,
        detail,
        at: super::sync::now(),
    })
}

/// Daemon boot: run the carry once with the shared DB over this computer's
/// Zen folder. Logs and returns what it did; a failure is logged and tried
/// again next boot (nothing was recorded).
pub fn boot() -> Option<CarryReport> {
    let files = ZenFiles::local();
    let now = chrono::Utc::now().timestamp();
    let db = crate::db::shared();
    let conn = db.lock();
    match carry_once(&conn, &files, now) {
        Ok(r) => r,
        Err(e) => {
            crate::log_debug!("[zen/version-carry] {e}; tries again next boot");
            None
        }
    }
}

#[cfg(test)]
#[path = "version_carry_tests.rs"]
mod tests;
