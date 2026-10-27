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
//! - **logs a saved copy** (last good, frozen) whose manifest differs
//!   from the code history of the same document (security review
//!   finding 6), carried like the rest;
//! - **refuses a late carry** (security review finding 4): when K2 0.46.5
//!   served before this pass ran (a failed first carry), the old saved
//!   copies were kept beside the new ones
//!   (`widget_store::PRE_CARRY_JSON`); a live version is carried only
//!   when its manifest is the one that copy shows, else it is left
//!   unsigned and the refusal is logged (row, audit, news).
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
use super::widget_store::{
    drop_pre_carry, read_legacy_saved_bundle, read_pre_carry_saved_bundle, write_saved_bundle_at, WidgetState, LAST_GOOD_JSON,
};
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
    /// How the live manifest differs from the signed one; for a saved copy,
    /// how its manifest differs from the one in its code history with the
    /// same document (security review finding 6).
    pub differs: Option<String>,
    /// Why a signature was not carried (security review finding 4): K2
    /// 0.46.5 served before this carry ran and the manifest can't be shown
    /// to be the one that was signed.
    pub refused: Option<String>,
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

    pub fn refused(&self) -> Vec<&Carried> {
        self.versions.iter().filter(|v| v.refused.is_some()).collect()
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
/// the signed version), in plain words for a person (the `zen.sign` card,
/// the news line, the carry log); `None` when they don't. Every cap, read,
/// library and CDN URL that changed is named.
pub fn manifest_diff(signed: &Manifest, live: &Manifest) -> Option<String> {
    if widgets::version_digest(signed) == widgets::version_digest(live) {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let added = |a: &[String], b: &[String]| -> Vec<String> { b.iter().filter(|x| !a.contains(x)).cloned().collect() };
    let caps_in = added(&signed.caps, &live.caps);
    let caps_out = added(&live.caps, &signed.caps);
    if !caps_in.is_empty() {
        parts.push(format!("now asks for {}", caps_in.join(", ")));
    }
    if !caps_out.is_empty() {
        parts.push(format!("no longer asks for {}", caps_out.join(", ")));
    }
    let reads_in = added(&signed.reads, &live.reads);
    let reads_out = added(&live.reads, &signed.reads);
    if !reads_in.is_empty() {
        parts.push(format!("now reads the data of {}", reads_in.join(", ")));
    }
    if !reads_out.is_empty() {
        parts.push(format!("no longer reads the data of {}", reads_out.join(", ")));
    }
    let lib_name = |x: &J| match x.as_str() {
        Some(s) => format!("the library {s}"),
        None => format!("the CDN file {}", x["url"].as_str().unwrap_or("?")),
    };
    let libs_in: Vec<String> = live.libs.iter().filter(|x| !signed.libs.contains(x)).map(lib_name).collect();
    let libs_out: Vec<String> = signed.libs.iter().filter(|x| !live.libs.contains(x)).map(lib_name).collect();
    if !libs_in.is_empty() {
        parts.push(format!("now loads {}", libs_in.join(", ")));
    }
    if !libs_out.is_empty() {
        parts.push(format!("no longer loads {}", libs_out.join(", ")));
    }
    if libs_in.is_empty() && libs_out.is_empty() && signed.libs != live.libs {
        parts.push("loads its libraries in another order".into());
    }
    if signed.sources != live.sources {
        let show = |s: &Option<J>| match s {
            None => "none".to_string(),
            Some(J::String(c)) => format!("\"{c}\" (the placement's config)"),
            Some(J::Array(a)) => a.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", "),
            Some(other) => other.to_string(),
        };
        parts.push(format!("its data sources were {} and are now {}", show(&signed.sources), show(&live.sources)));
    }
    if signed.entry != live.entry {
        parts.push(format!("starts from {} instead of {}", live.entry, signed.entry));
    }
    if signed.color_scheme != live.color_scheme {
        parts.push("chooses its own light or dark instead of the Garden's (or back)".into());
    }
    if signed.reasons != live.reasons {
        parts.push("gives other reasons for its caps".into());
    }
    if parts.is_empty() {
        parts.push("its manifest changed".into());
    }
    Some(parts.join("; "))
}

/// The carry's difference for `(widget, hash)` that the person hasn't
/// signed after seeing yet (`reviewed_at` NULL), if any.
pub fn unreviewed_difference(conn: &Connection, widget: &str, hash: &str) -> Result<Option<String>, DataError> {
    Ok(conn
        .query_row(
            "SELECT differs FROM zen_version_carry WHERE widget = ?1 AND new_hash = ?2 AND differs IS NOT NULL
               AND reviewed_at IS NULL ORDER BY id DESC LIMIT 1",
            params![widget, hash],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

/// The person signed `(widget, hash)`: its carry differences are reviewed.
pub fn mark_reviewed(conn: &Connection, widget: &str, hash: &str, now: i64) -> Result<usize, DataError> {
    Ok(conn.execute(
        "UPDATE zen_version_carry SET reviewed_at = ?3 WHERE widget = ?1 AND new_hash = ?2 AND reviewed_at IS NULL",
        params![widget, hash, now],
    )?)
}

/// Every carried version whose difference still waits for the person:
/// `(widget, new hash, difference)`.
pub fn unreviewed(conn: &Connection) -> Result<Vec<(String, String, String)>, DataError> {
    let mut st = conn.prepare(
        "SELECT widget, new_hash, differs FROM zen_version_carry WHERE differs IS NOT NULL AND reviewed_at IS NULL ORDER BY id",
    )?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The run fields of a manifest in plain words: every cap, neighbour read,
/// library and data source it has, each named even when there are none
/// (the `zen.sign` card always shows the whole version, never only a
/// difference: security review finding 3).
pub fn manifest_summary(m: &Manifest) -> String {
    let mut parts = vec![if m.caps.is_empty() { "asks for no caps".to_string() } else { format!("asks for {}", m.caps.join(", ")) }];
    parts.push(if m.reads.is_empty() {
        "reads no other widget's data".to_string()
    } else {
        format!("reads the data of {}", m.reads.join(", "))
    });
    parts.push(if m.libs.is_empty() {
        "loads no libraries".to_string()
    } else {
        let libs: Vec<String> = m
            .libs
            .iter()
            .map(|x| x.as_str().map_or_else(|| format!("the CDN file {}", x["url"].as_str().unwrap_or("?")), str::to_string))
            .collect();
        format!("loads {}", libs.join(", "))
    });
    parts.push(match &m.sources {
        Some(J::String(_)) => "reads the data sources its placement names".into(),
        Some(J::Array(a)) if !a.is_empty() => {
            format!("reads the data sources {}", a.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", "))
        }
        _ => "reads no data sources".into(),
    });
    parts.join("; ")
}

/// Would `maker`'s signature on `widget` count where it is placed: the
/// person, or an agent on the list of every Garden that places it (as
/// `zen_access_routes::may_vouch`). An unlisted agent's stamp, K2 and
/// nobody never count as "signed" for the card's comparison (security
/// review finding 3). Read on `conn` (callers hold the shared lock).
pub fn trusted_signer(files: &ZenFiles, conn: &Connection, widget: &str, maker: &MadeBy) -> Result<bool, DataError> {
    match maker {
        MadeBy::Owner => Ok(true),
        MadeBy::Agent { project_id } => {
            let gardens: std::collections::BTreeSet<String> =
                files.custom_placements().into_iter().filter(|p| p.widget == widget).map(|p| p.garden).collect();
            let refs: Vec<&str> = gardens.iter().map(String::as_str).collect();
            crate::zen::garden_access::listed_on_all(conn, &refs, project_id)
                .map_err(|e| DataError::Unavailable(format!("garden access list: {e:?}")))
        }
        MadeBy::K2 | MadeBy::Unclaimed => Ok(false),
    }
}

/// The words for the version the card compares against.
const BASE: &str = "Since the last version signed by you or an agent on its Gardens' lists";

/// What signing `widget`'s live version means, in plain words for the
/// `zen.sign` card (security review finding 3):
///
/// 1. always the whole version ([`manifest_summary`]: caps, reads,
///    libraries, data sources);
/// 2. then, as context, the carry's unreviewed difference; else the
///    difference from the newest history snapshot that (a) re-checks from
///    its own files to the hash its meta names
///    (`ZenFiles::verified_snapshot`) and (b) was signed by the person or
///    an agent listed on every placing Garden ([`trusted_signer`]); else
///    that no such version exists.
///
/// `None` when the widget has no live version.
pub fn version_changes(files: &ZenFiles, conn: &Connection, widget: &str) -> Result<Option<String>, DataError> {
    let st = files.widget_status(widget);
    let (Some(live_m), Some(b)) = (st.manifest.as_ref(), st.bundle.as_ref()) else { return Ok(None) };
    let whole = format!("This version of {widget} {}.", manifest_summary(live_m));
    if let Some(d) = unreviewed_difference(conn, widget, &b.hash)? {
        return Ok(Some(format!("{whole} Since it was signed, it {d}.")));
    }
    for snap in files.widget_snapshots(widget) {
        if !snap.clean || snap.hash.as_deref().is_none_or(|h| h == b.hash) {
            continue;
        }
        let Some(v) = files.verified_snapshot(widget, &snap) else { continue };
        // The hash the files make (never the meta's word for it).
        let signed = if v.legacy { v.bundle.template_hash() } else { v.bundle.hash.clone() };
        if signed == b.hash || !trusted_signer(files, conn, widget, &authors::maker_of(conn, widget, &signed)?)? {
            continue;
        }
        let context = match (manifest_diff(&v.manifest, live_m), v.legacy) {
            (Some(d), false) => format!("{BASE}, it {d}."),
            (None, false) => format!("{BASE}, only its code changed."),
            (Some(d), true) => format!("{BASE} (before K2 0.46.5, when a manifest wasn't part of what was signed), it {d}."),
            (None, true) => format!("{BASE} (before K2 0.46.5, when a manifest wasn't part of what was signed), its manifest looks the same."),
        };
        return Ok(Some(format!("{whole} {context}")));
    }
    Ok(Some(format!("{whole} No earlier version was signed by you or an agent on its Gardens' lists.")))
}

/// A saved copy to carry and rewrite.
struct Saved {
    dir: std::path::PathBuf,
    manifest: Manifest,
    bundle: super::bundle::Bundle,
    at: String,
}

/// The old-shape copy saved in `dir` and whether the carry is late: its
/// `last-good.json` while still old (the normal first boot), else the copy
/// kept beside it when K2 0.46.5 rewrote it before this carry ran
/// (`late = true`: a failed first carry; security review finding 4).
fn old_copy(dir: &std::path::Path) -> (Option<(Manifest, super::bundle::Bundle, String, String)>, bool) {
    if let Some(l) = read_legacy_saved_bundle(dir) {
        return (Some(l), false);
    }
    let late = dir.join(LAST_GOOD_JSON).exists();
    (if late { read_pre_carry_saved_bundle(dir) } else { None }, late)
}

/// How a saved copy's manifest `saved` differs from the code history of
/// the same document (`template`, the old hash): `None` when some history
/// snapshot of that document, re-checked from its own files, has this
/// manifest (or none is kept); else the difference from the newest one
/// (security review finding 6). A manifest-only edit leaves several
/// snapshots of one document, so any of them may be the one saved.
fn history_difference(files: &ZenFiles, widget: &str, template: &str, saved: &Manifest) -> Option<String> {
    let same_doc: Vec<_> = files
        .widget_snapshots(widget)
        .iter()
        .filter(|s| s.clean)
        .filter_map(|s| files.verified_snapshot(widget, s))
        .filter(|v| v.bundle.template_hash() == template)
        .collect();
    let digest = widgets::version_digest(saved);
    if same_doc.iter().any(|v| widgets::version_digest(&v.manifest) == digest) {
        return None;
    }
    same_doc.first().and_then(|v| manifest_diff(&v.manifest, saved))
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
    // Kept pre-0.46.5 copies to remove once the carry is recorded.
    let mut kept: Vec<std::path::PathBuf> = Vec::new();
    if files.exists() {
        for name in files.widget_names() {
            let history = files.widget_history_dir(&name);
            let (legacy, late) = old_copy(&history);
            if late {
                kept.push(history.clone());
            }
            let st = files.widget_status(&name);
            if st.state == WidgetState::Ok {
                if let (Some(m), Some(b)) = (&st.manifest, &st.bundle) {
                    let old = b.template_hash();
                    // What was signed: the copy saved when this document became good.
                    let base = legacy.as_ref().filter(|(_, _, _, lh)| *lh == old);
                    let differs = base.and_then(|(lm, _, _, _)| manifest_diff(lm, m));
                    // K2 0.46.5 served before this carry (a failed first
                    // carry): the live manifest may have changed since; carry
                    // only what the copy saved before then shows unchanged.
                    let refused = if late && authors::maker_of(&tx, &name, &old)? != MadeBy::Unclaimed {
                        match (base, &differs) {
                            (None, _) => Some(
                                "K2 0.46.5 ran before this carry and no copy saved before it matches this code, so K2 can't tell what was signed".to_string(),
                            ),
                            (Some(_), Some(d)) => Some(format!("K2 0.46.5 ran before this carry and since then it {d}")),
                            (Some(_), None) => None,
                        }
                    } else {
                        None
                    };
                    let maker = if refused.is_some() { None } else { authors::carry(&tx, &name, &old, &b.hash)? };
                    report.versions.push(Carried {
                        widget: name.clone(),
                        place: Place::Live,
                        garden: None,
                        old_hash: old,
                        new_hash: b.hash.clone(),
                        maker: maker.as_ref().map(maker_label),
                        // A refused carry has no signature to review.
                        differs: if refused.is_some() { None } else { differs },
                        refused,
                    });
                }
            }
            if let Some((m, b, at, old)) = legacy {
                let maker = authors::carry(&tx, &name, &old, &b.hash)?;
                report.versions.push(Carried {
                    widget: name.clone(),
                    place: Place::LastGood,
                    garden: None,
                    differs: history_difference(files, &name, &old, &m),
                    old_hash: old,
                    new_hash: b.hash.clone(),
                    maker: maker.as_ref().map(maker_label),
                    refused: None,
                });
                // A late carry leaves the newer saved copy in place.
                if !late {
                    saved.push(Saved { dir: history, manifest: m, bundle: b, at });
                }
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
                let (legacy, late) = old_copy(&dir);
                if late {
                    kept.push(dir.clone());
                }
                let Some((m, b, at, old)) = legacy else { continue };
                let maker = authors::carry(&tx, &name, &old, &b.hash)?;
                report.versions.push(Carried {
                    widget: name.clone(),
                    place: Place::Frozen,
                    garden: Some(g.clone()),
                    differs: history_difference(files, &name, &old, &m),
                    old_hash: old,
                    new_hash: b.hash.clone(),
                    maker: maker.as_ref().map(maker_label),
                    refused: None,
                });
                if !late {
                    saved.push(Saved { dir, manifest: m, bundle: b, at });
                }
            }
        }
    }
    for v in &report.versions {
        tx.execute(
            "INSERT INTO zen_version_carry (widget, place, garden, old_hash, new_hash, carried, maker, differs, refused, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                v.widget,
                v.place.as_str(),
                v.garden,
                v.old_hash,
                v.new_hash,
                v.maker.is_some() as i64,
                v.maker,
                v.differs,
                v.refused,
                now
            ],
        )?;
    }
    let summary = json!({
        "versions": report.versions.len(),
        "carried": report.carried(),
        "differing": report.differing().len(),
        "refused": report.refused().len(),
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
    for dir in &kept {
        drop_pre_carry(dir);
    }
    for v in report.refused() {
        crate::log_debug!(
            "[zen/version-carry] {}: did NOT carry the signature on {}: {}; it runs without the person's data until someone signs it",
            v.widget,
            &v.old_hash[..12],
            v.refused.as_deref().unwrap_or_default()
        );
        crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
            AUDIT_EVENT,
            "k2",
            format!("refused widget={} old={} new={} {}", v.widget, &v.old_hash[..12], &v.new_hash[..12], v.refused.as_deref().unwrap_or_default()),
            "-",
            "-",
            "daemon",
        ));
    }
    for v in report.differing() {
        let what = match (v.place, v.garden.as_deref()) {
            (Place::Live, _) => "the live manifest differs from the signed one".to_string(),
            (Place::LastGood, _) => "its saved copy's manifest differs from the one in its code history".to_string(),
            (Place::Frozen, g) => format!("its frozen copy in {}'s manifest differs from the one in its code history", g.unwrap_or("?")),
        };
        crate::log_debug!(
            "[zen/version-carry] {}: carried {}'s signature although {what}: {}",
            v.widget,
            v.maker.as_deref().unwrap_or("nobody"),
            v.differs.as_deref().unwrap_or_default()
        );
        crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
            AUDIT_EVENT,
            "k2",
            format!(
                "differs widget={} place={} new={} maker={} {}",
                v.widget,
                v.place.as_str(),
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
            "ok versions={} carried={} differing={} refused={} rewritten={} failed={}",
            report.versions.len(),
            report.carried(),
            report.differing().len(),
            report.refused().len(),
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
    let refused = r.refused();
    if differing.is_empty() && refused.is_empty() && r.failed.is_empty() {
        return None;
    }
    let mut detail: Vec<String> = differing
        .iter()
        .map(|v| {
            format!(
                "{}: signed by {}; since then it {}. Sign it in its History, or an agent asks you with k2 zen widget sign {} --ask",
                v.widget,
                v.maker.as_deref().unwrap_or("nobody"),
                v.differs.as_deref().unwrap_or_default(),
                v.widget
            )
        })
        .collect();
    detail.extend(refused.iter().map(|v| {
        format!(
            "{}: K2 didn't keep its signature: {}. It runs without your data until you sign it in its History, or an agent asks you with k2 zen widget sign {} --ask",
            v.widget,
            v.refused.as_deref().unwrap_or_default(),
            v.widget
        )
    }));
    detail.extend(r.failed.iter().map(|p| format!("couldn't update {p}; that saved copy isn't used")));
    let mut body = String::from(
        "From this K2, a widget's manifest (its caps, the data it reads, its libraries) is part of the version you sign, like its code.",
    );
    if !differing.is_empty() {
        body.push_str(&format!(
            " K2 kept the signature on {} widget{} whose manifest changed after it was signed; check the changes below and sign it if they're right (a card asks you when an agent next works on it).",
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
