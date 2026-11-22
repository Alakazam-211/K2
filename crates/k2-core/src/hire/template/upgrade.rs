//! Upgrades without clobbering (prd-agent-templates-v1 §4, slice T5).
//!
//! - **When:** a built-in upgrades at daemon start when its version is above
//!   the lock's ([`upgrade_at_start`], beside the skill pass). A library
//!   template upgrades only on `k2 agent template upgrade <ws>` (or the
//!   Upgrade button): a new library version never changes an agent by
//!   itself.
//! - **Per file, by the lock's checksum:** a file still as K2 wrote it is
//!   replaced; one the agent changed is kept and the new text lands beside
//!   it as `<file>.proposed` (the lock lists it under `kept`); one the agent
//!   deleted stays deleted. In ROLE.md only the K2:MANAGED block is K2's.
//! - **New in a version:** catalog items, starter notes, layers and skills
//!   are added. Heartbeats are only reported (they cost tokens), and so are
//!   new permission suggestions. What a version dropped stays and is
//!   reported.
//! - **Never:** permissions, tool, model, name, connections.
//! - **Packs (AT10):** a static catalog pack the template stacked follows
//!   the same checksum rule, on every pass, so `kessel:pack` can improve.
//! - **Report:** one inbox note to the agent when anything changed.
//!   [`upgrade`] with `dry_run` is `k2 agent template diff <ws>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::brand::{CLI_NAME, PRODUCT_NAME};

use super::super::HireCaller;
use super::apply::{charter, fill_note, role_body, AnswerPlan};
use super::format::{sha256_text, FileKind, Origin, Template};
use super::lock::{self, TemplateLock};
use super::render::{placeholders, render};
use super::save::TemplateError;
use crate::skills::version::{SKILL_BEGIN_MARKER, SKILL_END_MARKER};
use crate::product_name;

type R<T> = Result<T, TemplateError>;

const ROLE_REL: &str = ".k2/agent/ROLE.md";
/// The most diff lines one file shows.
const MAX_DIFF_LINES: usize = 200;

/// One line of the upgrade report.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpRow {
    /// `role`, `wiki`, `layer`, `skill`, `catalog`, `pack`, `heartbeat`,
    /// `permission`.
    pub step: &'static str,
    pub target: String,
    /// `replace`, `keep` (the agent's; `proposed` beside it), `add`,
    /// `deleted` (stays deleted), `unchanged`, `yours` (not from the
    /// template), `dropped` (a version removed it; left in place), `report`
    /// (listed, never applied).
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The `<file>.proposed` path, for `keep`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    /// A short line diff (current → new) for `replace` and `keep`, on `diff`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

fn up(step: &'static str, target: impl Into<String>, status: &'static str, reason: Option<String>) -> UpRow {
    UpRow { step, target: target.into(), status, reason, proposed: None, diff: None }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeReport {
    pub ok: bool,
    pub dry_run: bool,
    pub workspace: String,
    pub id: String,
    pub name: String,
    pub from: u32,
    pub to: u32,
    /// Something was (or, in a dry run, would be) written.
    pub changed: bool,
    pub rows: Vec<UpRow>,
    /// The inbox note's text (sent when not a dry run and changed).
    pub note: Option<String>,
}

/// Rebuild the hire's caller from the lock's actor (for `{{person}}` and
/// the answers' "hired by" line on a lock that recorded no values).
fn caller_of(by: &str) -> HireCaller {
    if let Some(u) = by.strip_prefix("user:") {
        return HireCaller::Login { username: u.to_string() };
    }
    if let Some(w) = by.strip_prefix("agent:") {
        return HireCaller::Agent { workspace_id: w.to_string() };
    }
    if let Some(k) = by.strip_prefix("api-key:") {
        return HireCaller::ApiKey { id: k.to_string(), label: None };
    }
    HireCaller::Owner
}

fn read(ws: &str, rel: &str) -> Option<String> {
    let p = Path::new(ws).join(rel);
    let meta = std::fs::symlink_metadata(&p).ok()?;
    if !meta.is_file() {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

fn write(ws: &str, rel: &str, text: &str) -> R<()> {
    let p = Path::new(ws).join(rel);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| TemplateError::new(500, "upgrade_failed", format!("{rel}: {e}")))?;
    }
    crate::fs_atomic::atomic_write_str(&p, text).map_err(|e| TemplateError::new(500, "upgrade_failed", format!("{rel}: {e}")))
}

/// `<rel>.proposed`, written only when it would change.
fn propose(ws: &str, rel: &str, text: &str, dry: bool) -> R<String> {
    let p = format!("{rel}.proposed");
    if !dry && read(ws, &p).as_deref() != Some(text) {
        write(ws, &p, text)?;
    }
    Ok(p)
}

/// The K2:MANAGED block of a ROLE.md: `(start of body, end of body)`, the
/// body being what `role_body` put between `BEGIN\n` and `END`.
fn managed_span(text: &str) -> Option<(usize, usize)> {
    let b = text.find(SKILL_BEGIN_MARKER)?;
    let mut start = b + SKILL_BEGIN_MARKER.len();
    if text[start..].starts_with('\n') {
        start += 1;
    }
    let end = start + text[start..].find(SKILL_END_MARKER)?;
    Some((start, end))
}

/// A short `-`/`+` line diff (longest common subsequence; a large file
/// gives a line count instead).
pub fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len().saturating_mul(b.len()) > 4_000_000 {
        return format!("({} lines → {} lines)", a.len(), b.len());
    }
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out: Vec<String> = Vec::new();
    let mut last_hunk: Option<usize> = None;
    // `line` is the new file's line; an added line moves it on, a removed
    // one doesn't, so one run of changes gets one header.
    let mut hunk = |out: &mut Vec<String>, line: usize, added: bool| {
        if last_hunk != Some(line) {
            out.push(format!("@@ line {}", line + 1));
        }
        last_hunk = Some(if added { line + 1 } else { line });
    };
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            hunk(&mut out, j, true);
            out.push(format!("+{}", b[j]));
            j += 1;
        } else {
            hunk(&mut out, j, false);
            out.push(format!("-{}", a[i]));
            i += 1;
        }
        if out.len() > MAX_DIFF_LINES {
            out.push("… (more)".into());
            break;
        }
    }
    out.join("\n")
}

/// Where a template file lands in a workspace.
fn ws_rel(ws: &str, kind: &FileKind) -> Option<String> {
    Some(match kind {
        FileKind::Wiki(id) => format!(".k2/wiki/{id}"),
        FileKind::Context(label) => format!(".k2/context/{label}.md"),
        FileKind::Skill(name) => {
            let dir = crate::workspace::agent_identity::skill_dir(ws, name);
            dir.strip_prefix(ws)
                .map(|p| format!("{}/SKILL.md", p.to_string_lossy()))
                .unwrap_or_else(|_| format!(".k2/skills/{name}/SKILL.md"))
        }
        _ => return None,
    })
}

/// What one pass decided, before it is written.
struct Pass<'a> {
    ws: &'a str,
    dry: bool,
    with_diff: bool,
    rows: Vec<UpRow>,
    lock: TemplateLock,
    kept: BTreeSet<String>,
    wrote: bool,
}

impl Pass<'_> {
    /// The checksum rule for one file K2 wrote before (`rel` in the lock).
    /// `risky`: the new text can't be rendered faithfully (an old lock with
    /// no recorded values), so even an untouched file is only proposed.
    fn file(&mut self, step: &'static str, rel: &str, new: &str, risky: bool) -> R<()> {
        let Some(locked) = self.lock.files.get(rel).cloned() else {
            match read(self.ws, rel) {
                None => {
                    if !self.dry {
                        write(self.ws, rel, new)?;
                        self.lock.files.insert(rel.to_string(), sha256_text(new));
                    }
                    self.wrote = true;
                    self.rows.push(up(step, rel, "add", Some("new in this version".into())));
                }
                Some(_) => self.rows.push(up(step, rel, "yours", Some("not written by the template; left alone".into()))),
            }
            return Ok(());
        };
        let Some(current) = read(self.ws, rel) else {
            self.rows.push(up(step, rel, "deleted", Some("the agent deleted it; it stays deleted".into())));
            return Ok(());
        };
        if current == new {
            self.kept.remove(rel);
            self.lock.files.insert(rel.to_string(), sha256_text(new));
            self.rows.push(up(step, rel, "unchanged", None));
            return Ok(());
        }
        let diff = self.with_diff.then(|| line_diff(&current, new));
        if sha256_text(&current) == locked && !risky {
            if !self.dry {
                write(self.ws, rel, new)?;
            }
            self.lock.files.insert(rel.to_string(), sha256_text(new));
            self.kept.remove(rel);
            self.wrote = true;
            let mut r = up(step, rel, "replace", None);
            r.diff = diff;
            self.rows.push(r);
        } else {
            let already = self.kept.contains(rel) && read(self.ws, &format!("{rel}.proposed")).as_deref() == Some(new);
            let p = propose(self.ws, rel, new, self.dry)?;
            self.kept.insert(rel.to_string());
            self.wrote |= !already;
            let why = if risky && sha256_text(&current) == locked {
                concat!("written before ", product_name!(), " kept the hire's values; review the new text")
            } else {
                "the agent changed it; kept, new text beside it"
            };
            let mut r = up(step, rel, "keep", Some(why.into()));
            r.proposed = Some(p);
            r.diff = diff;
            self.rows.push(r);
        }
        Ok(())
    }

    /// ROLE.md: only the K2:MANAGED block is K2's.
    fn role(&mut self, display: &str, new_charter: &str, risky: bool) -> R<()> {
        let Some(locked) = self.lock.files.get(ROLE_REL).cloned() else {
            self.rows.push(up("role", ROLE_REL, "yours", Some("the agent's own ROLE.md; never touched".into())));
            return Ok(());
        };
        let agent_dir = crate::workspace::agent_identity::workspace_agent_path(self.ws);
        let abs = agent_dir.join(crate::workspace::agent_identity::PERSONA_MD_NAME);
        let rel = abs
            .strip_prefix(self.ws)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ROLE_REL.to_string());
        let Some(current) = read(self.ws, &rel) else {
            self.rows.push(up("role", &rel, "deleted", Some("the agent deleted it; it stays deleted".into())));
            return Ok(());
        };
        let Some((s, e)) = managed_span(&current) else {
            // The markers are gone: the whole file is the agent's now.
            let p = propose(self.ws, &rel, &role_body(display, new_charter), self.dry)?;
            self.kept.insert(ROLE_REL.to_string());
            self.wrote = true;
            let mut r = up("role", &rel, "keep", Some("the K2:MANAGED markers are gone; kept".into()));
            r.proposed = Some(p);
            self.rows.push(r);
            return Ok(());
        };
        let block = &current[s..e];
        if block == new_charter {
            self.kept.remove(ROLE_REL);
            self.lock.files.insert(ROLE_REL.into(), sha256_text(new_charter));
            self.rows.push(up("role", &rel, "unchanged", None));
            return Ok(());
        }
        let next = format!("{}{new_charter}{}", &current[..s], &current[e..]);
        let diff = self.with_diff.then(|| line_diff(block, new_charter));
        if sha256_text(block) == locked && !risky {
            if !self.dry {
                write(self.ws, &rel, &next)?;
            }
            self.lock.files.insert(ROLE_REL.into(), sha256_text(new_charter));
            self.kept.remove(ROLE_REL);
            self.wrote = true;
            let mut r = up("role", &rel, "replace", Some("the K2:MANAGED block only; your part is untouched".into()));
            r.diff = diff;
            self.rows.push(r);
        } else {
            let p = propose(self.ws, &rel, &next, self.dry)?;
            self.kept.insert(ROLE_REL.to_string());
            self.wrote = true;
            let why = if risky && sha256_text(block) == locked {
                concat!("written before ", product_name!(), " kept the hire's values; review the new charter")
            } else {
                "the agent changed the K2:MANAGED block; kept, new text beside it"
            };
            let mut r = up("role", &rel, "keep", Some(why.into()));
            r.proposed = Some(p);
            r.diff = diff;
            self.rows.push(r);
        }
        Ok(())
    }

    /// AT10: the static packs in the lock's catalog, by checksum.
    fn packs(&mut self) -> R<()> {
        for id in self.lock.catalog.clone() {
            let Ok(e) = crate::workspace::context_layers::resolve_catalog_entry(&id) else { continue };
            let Some(body) = crate::workspace::context_layers::static_pack_body(&e.source) else { continue };
            let Some(current) = read(self.ws, &e.path) else { continue };
            if current == body {
                self.lock.files.insert(e.path.clone(), sha256_text(body));
                self.kept.remove(&e.path);
                continue;
            }
            if self.lock.files.contains_key(&e.path) {
                self.file("pack", &e.path, body, false)?;
                continue;
            }
            // Stacked before K2 kept pack checksums: nothing says the agent
            // left it alone, so the new text is only proposed.
            let p = format!("{}.proposed", e.path);
            let already = self.kept.contains(&e.path) && read(self.ws, &p).as_deref() == Some(body);
            propose(self.ws, &e.path, body, self.dry)?;
            self.kept.insert(e.path.clone());
            if !already {
                self.wrote = true;
                let mut r = up("pack", &e.path, "keep", Some(concat!("stacked before ", product_name!(), " kept pack checksums; review the new text").into()));
                r.proposed = Some(p);
                r.diff = self.with_diff.then(|| line_diff(&current, body));
                self.rows.push(r);
            }
        }
        Ok(())
    }
}

fn not_from_template(ws: &str) -> TemplateError {
    TemplateError::new(404, "no_template", format!("{ws} wasn't made from a template (no .k2/agent/template.lock)"))
}

/// Upgrade `ws` to `target` (`None`: the lock's id as it resolves now).
/// With `packs_only`, only the AT10 pack pass runs (the start pass for a
/// library template, or a built-in already current). `with_diff` adds line
/// diffs to the rows (`diff`).
pub fn upgrade(ws: &str, target: Option<&Template>, actor: &str, dry: bool, with_diff: bool) -> R<UpgradeReport> {
    let Some(lock) = lock::read(ws)? else {
        return Err(not_from_template(ws));
    };
    let resolved;
    let t = match target {
        Some(t) => t,
        None => {
            resolved = super::resolve(&lock.id).map_err(TemplateError::from)?;
            &resolved
        }
    };
    if t.meta.id != lock.id {
        return Err(TemplateError::new(
            409,
            "template_conflict",
            format!("{ws} was made from '{}', not '{}'", lock.id, t.meta.id),
        ));
    }
    run(ws, lock, Some(t), actor, dry, with_diff)
}

fn run(ws: &str, lock: TemplateLock, t: Option<&Template>, actor: &str, dry: bool, with_diff: bool) -> R<UpgradeReport> {
    let from = lock.version;
    let to = t.map(|t| t.meta.version).filter(|v| *v > from).unwrap_or(from);
    let name = t.map(|t| t.meta.name.clone()).unwrap_or_else(|| lock.id.clone());
    let kept: BTreeSet<String> = lock.kept.iter().cloned().collect();
    let mut pass = Pass { ws, dry, with_diff, rows: Vec::new(), lock: lock.clone(), kept, wrote: false };

    if let (Some(t), true) = (t, to > from) {
        version_steps(&mut pass, t)?;
    }
    pass.packs()?;

    let changed = pass.wrote;
    let mut next = pass.lock.clone();
    next.kept = pass.kept.iter().cloned().collect();
    if to > from {
        if let Some(t) = t {
            next.version = t.meta.version;
            next.hash = t.hash.clone();
            next.catalog = t.meta.context.catalog.clone();
        }
    }
    let lock_changed = !next.same_content(&lock);
    // A note only when there is something to tell (not for a version that
    // changed no file of this agent).
    let worth = changed || pass.rows.iter().any(|r| matches!(r.status, "report" | "keep" | "deleted"));
    let note = (worth && (changed || to > from)).then(|| report_note(&name, from, to, &pass.rows));
    if !dry && (lock_changed || changed) {
        next.applied_at = super::super::now_secs();
        next.by = actor.to_string();
        lock::write(ws, &next).map_err(TemplateError::from)?;
        if changed {
            // ROLE.md and layers feed the composed AGENTS.md.
            if let Err(e) = crate::workspace::context_layers::regen(ws) {
                crate::log_debug!("[agent-templates] regen after upgrade of {ws}: {}", e.hint());
            }
        }
        if let Some(text) = note.as_deref() {
            let title = if to > from {
                format!("Your {name} template went {from} → {to}")
            } else {
                format!("{PRODUCT_NAME} updated your {name} context packs")
            };
            if let Err(e) = crate::inbox::compose(Path::new(ws), &title, text, Some("normal"), Some("k2"), Some("k2")) {
                crate::log_debug!("[agent-templates] upgrade note for {ws}: {e}");
            }
        }
    }
    Ok(UpgradeReport { ok: true, dry_run: dry, workspace: ws.to_string(), id: lock.id, name, from, to, changed, rows: pass.rows, note })
}

/// The template's own files, catalog, heartbeats and suggestions, from
/// the lock's version to `t`'s.
fn version_steps(pass: &mut Pass<'_>, t: &Template) -> R<()> {
    let ws = pass.ws;
    let lock = pass.lock.clone();
    let legacy = lock.vars.is_empty();
    let mut vars = lock.vars.clone();
    let hirer = caller_of(&lock.by);
    let display = vars.get("name").cloned().unwrap_or_else(|| {
        let d = crate::workspace::display::agent_display_name(ws);
        if d.trim().is_empty() {
            Path::new(ws).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            d
        }
    });
    vars.entry("name".into()).or_insert_with(|| display.clone());
    vars.entry("person".into()).or_insert_with(|| super::person_for(&hirer));
    for k in t.meta.params.keys() {
        vars.entry(k.clone()).or_default();
    }
    vars.entry("open_questions".into()).or_default();
    // Text a lock with no recorded values can't render faithfully.
    let unsure = |text: &str| legacy && placeholders(text).unwrap_or_default().iter().any(|p| p != "name" && p != "person");

    // The answers, where the first apply put them.
    let notes_written: BTreeSet<String> = t
        .wiki_notes()
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| lock.files.contains_key(&format!(".k2/wiki/{id}")))
        .collect();
    let mut plan = AnswerPlan::new(t, &lock.answers, &notes_written);
    if !plan.role.is_empty() {
        plan.by_agent = super::apply::hirer_agent_name(&hirer);
    }

    // ROLE.md.
    let new_charter = charter(t, &vars, &plan);
    let role_risky = legacy && (!t.meta.params.is_empty() || !t.meta.questions.is_empty());
    pass.role(&display, &new_charter, role_risky)?;

    // Starter notes, layers, skills.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (path, body) in &t.files {
        let Ok(kind) = super::format::classify_path(path) else { continue };
        let Some(rel) = ws_rel(ws, &kind) else { continue };
        seen.insert(rel.clone());
        let step = match kind {
            FileKind::Wiki(_) => "wiki",
            FileKind::Context(_) => "layer",
            _ => "skill",
        };
        let mut text = render(body, &vars);
        let mut risky = unsure(body);
        if let FileKind::Wiki(id) = &kind {
            if let Some(entries) = plan.wiki.get(id) {
                text = fill_note(&text, entries);
            }
            risky |= legacy && t.meta.questions.iter().any(|q| q.fills.contains(id.trim_end_matches(".md")));
        }
        let fresh = !lock.files.contains_key(&rel) && read(ws, &rel).is_none();
        match (&kind, fresh) {
            // A new layer is stacked; a new skill is made the skills way.
            (FileKind::Context(label), true) => {
                if !pass.dry {
                    super::super::steps::add_inline_layer(ws, label, &text).map_err(TemplateError::from)?;
                    pass.lock.files.insert(rel.clone(), sha256_text(&text));
                }
                pass.wrote = true;
                pass.rows.push(up(step, &rel, "add", Some("new in this version".into())));
            }
            (FileKind::Skill(name), true) => {
                if !pass.dry {
                    super::super::steps::write_skills(ws, &[super::super::recipe::SkillIn { name: name.clone(), body: text.clone() }])
                        .map_err(TemplateError::from)?;
                    pass.lock.files.insert(rel.clone(), sha256_text(&text));
                }
                pass.wrote = true;
                pass.rows.push(up(step, &rel, "add", Some("new in this version".into())));
            }
            _ => pass.file(step, &rel, &text, risky)?,
        }
    }
    // What this version dropped stays.
    for rel in lock.files.keys() {
        let ours = rel.starts_with(".k2/wiki/") || rel.starts_with(".k2/skills/") || (rel.starts_with(".k2/context/") && !rel.starts_with(".k2/context/catalog/"));
        if ours && rel != ".k2/wiki/_Index.md" && !seen.contains(rel) {
            pass.rows.push(up("file", rel, "dropped", Some(format!("not in v{}; left in place", t.meta.version))));
        }
    }

    // Catalog: new items are added; one the agent removed isn't re-added.
    let stacked: Vec<(String, String)> = crate::workspace::context_layers::list_layers(ws)
        .map(|ls| ls.into_iter().map(|l| (l.source, l.path)).collect())
        .unwrap_or_default();
    for id in &t.meta.context.catalog {
        if lock.catalog.contains(id) {
            continue;
        }
        let Ok(e) = crate::workspace::context_layers::resolve_catalog_entry(id) else { continue };
        if stacked.iter().any(|(s, p)| *s == e.source || *p == e.path) {
            pass.rows.push(up("catalog", id, "unchanged", Some("already stacked".into())));
            continue;
        }
        if !pass.dry {
            super::super::steps::add_context(ws, std::slice::from_ref(id)).map_err(TemplateError::from)?;
            if let Some(body) = crate::workspace::context_layers::static_pack_body(&e.source) {
                if read(ws, &e.path).as_deref() == Some(body) {
                    pass.lock.files.insert(e.path.clone(), sha256_text(body));
                }
            }
        }
        pass.wrote = true;
        pass.rows.push(up("catalog", id, "add", Some("new in this version".into())));
    }
    for id in &lock.catalog {
        if !t.meta.context.catalog.contains(id) {
            pass.rows.push(up("catalog", id, "dropped", Some(format!("not in v{}; still stacked", t.meta.version))));
        }
    }
    // The packs pass reads the new catalog.
    pass.lock.catalog = t.meta.context.catalog.clone();

    // Heartbeats and permissions: reported, never applied.
    let have: Vec<String> = crate::heartbeats::k2_heartbeat_list(ws.to_string())
        .map(|hs| hs.into_iter().map(|h| h.name).collect())
        .unwrap_or_default();
    for hb in &t.meta.heartbeats {
        if !have.contains(&hb.name) {
            pass.rows.push(up(
                "heartbeat",
                &hb.name,
                "report",
                Some(format!("in v{} ({}); not added, since heartbeats cost tokens: add it with {CLI_NAME} heartbeat if you want it", t.meta.version, hb.frequency)),
            ));
        }
    }
    for id in t.suggested() {
        if !lock.suggested.contains(&id) {
            pass.rows.push(up("permission", &id, "report", Some("suggested by the new version; an upgrade never turns a switch on".into())));
        }
    }
    Ok(())
}

/// The inbox note: "Your Developer template went 1 → 2. Replaced: …; kept
/// yours: … (see .proposed)."
fn report_note(name: &str, from: u32, to: u32, rows: &[UpRow]) -> String {
    let pick = |status: &str| -> Vec<String> { rows.iter().filter(|r| r.status == status).map(|r| r.target.clone()).collect() };
    let mut out = if to > from {
        format!("Your {name} template went {from} → {to}.")
    } else {
        format!("{PRODUCT_NAME} refreshed the context packs your {name} template stacked.")
    };
    let list = |v: &[String]| v.join(", ");
    let replaced = pick("replace");
    let kept: Vec<String> = rows.iter().filter(|r| r.status == "keep").map(|r| r.proposed.clone().unwrap_or_else(|| r.target.clone())).collect();
    let added = pick("add");
    if !replaced.is_empty() {
        out.push_str(&format!("\n\nReplaced (you hadn't changed them): {}.", list(&replaced)));
    }
    if !kept.is_empty() {
        out.push_str(&format!(
            "\n\nKept yours (you had changed them); the new text is beside each, to read and merge if you want it: {}.",
            list(&kept)
        ));
    }
    if !added.is_empty() {
        out.push_str(&format!("\n\nAdded: {}.", list(&added)));
    }
    let reported: Vec<String> = rows
        .iter()
        .filter(|r| r.status == "report")
        .map(|r| format!("{} {}", r.step, r.target))
        .collect();
    if !reported.is_empty() {
        out.push_str(&format!(
            "\n\nNot applied (ask your person if you want them): {}. Upgrades never add heartbeats or permissions.",
            list(&reported)
        ));
    }
    let deleted = pick("deleted");
    if !deleted.is_empty() {
        out.push_str(&format!("\n\nYou deleted these, so they stay deleted: {}.", list(&deleted)));
    }
    out
}

/// The daemon-start pass for one workspace (beside the skill pass): a
/// built-in above the lock's version upgrades; every template's packs are
/// checked (AT10). Library templates never upgrade here. Errors are logged
/// by the caller; `Ok(None)` when the workspace has no template.
pub fn upgrade_at_start(ws: &str) -> R<Option<UpgradeReport>> {
    let Some(lock) = lock::read(ws)? else { return Ok(None) };
    let builtin = super::builtins().into_iter().flatten().find(|t| t.meta.id == lock.id && t.origin == Origin::Builtin);
    // `run` upgrades only to a higher version; otherwise just the packs.
    run(ws, lock, builtin.as_ref(), "k2-start", false, false).map(Some)
}

// ── diff between two versions ────────────────────────────────────────

/// The files of `id` at version `v`: a library version's folder, or the
/// built-in (K2 keeps only its current version).
fn version_files(id: &str, v: u32) -> R<BTreeMap<String, String>> {
    if let Some(t) = super::builtins().into_iter().flatten().find(|t| t.meta.id == id || t.meta.aliases.iter().any(|a| a == id)) {
        if t.meta.version == v {
            return Ok(t.files);
        }
        return Err(TemplateError::new(
            404,
            "unknown_template",
            format!("{PRODUCT_NAME} ships only v{} of the built-in '{}'", t.meta.version, t.meta.id),
        ));
    }
    let dir = super::library_dir().join(id).join(format!("v{v}"));
    if !dir.is_dir() {
        let have: Vec<String> = super::save::versions_of(id).into_iter().map(|(n, _)| format!("v{n}")).collect();
        let hint = if have.is_empty() { format!("no saved template '{id}'") } else { format!("'{id}' has {}", have.join(", ")) };
        return Err(TemplateError::new(404, "unknown_template", hint));
    }
    super::read_folder(&dir).map_err(|e| TemplateError::new(400, "bad_template", e))
}

/// `k2 agent template diff <id> <vA> <vB>`: each file added, removed or
/// changed (with a line diff).
pub fn diff_versions(id: &str, a: u32, b: u32) -> R<serde_json::Value> {
    let fa = version_files(id, a)?;
    let fb = version_files(id, b)?;
    let paths: BTreeSet<&String> = fa.keys().chain(fb.keys()).collect();
    let mut files = Vec::new();
    for p in paths {
        let v = match (fa.get(p), fb.get(p)) {
            (Some(x), Some(y)) if x == y => serde_json::json!({ "path": p, "status": "same" }),
            (Some(x), Some(y)) => serde_json::json!({ "path": p, "status": "changed", "diff": line_diff(x, y) }),
            (None, Some(y)) => serde_json::json!({ "path": p, "status": "added", "diff": line_diff("", y) }),
            (Some(_), None) => serde_json::json!({ "path": p, "status": "removed" }),
            (None, None) => continue,
        };
        files.push(v);
    }
    Ok(serde_json::json!({ "ok": true, "id": id, "from": a, "to": b, "files": files }))
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod tests;
