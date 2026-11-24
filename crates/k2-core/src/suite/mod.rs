//! One-click suite Apps (0.46.9, prd-one-click-suite-apps-v1 + vs-live
//! A1–A27): the compiled-in catalog, its agreement checks, the safe unpack
//! and the install-job store. The job itself (the steps, the waits, the
//! Sew calls) runs in the daemon (`k2-daemon/src/apps/catalog_job.rs`).
//!
//! - **The catalog** ([`catalog`]) is `catalog.json`, compiled into each
//!   signed K2 release (R28, Q1/Q12): K2's own signature is the trust root.
//!   No env var or file can replace it (A27); tests hand the daemon's job a
//!   [`Catalog`] of their own.
//! - **An entry** is one suite App (`sew/hearth`, `sew/meet`); its versions
//!   are Seeds pinned by tarball sha256, size, and K2's own tree hash of the
//!   stored Seed (`seedHash`, A12). Each version also pins the publisher's
//!   source tarball (`source`, Sew 2026-11-04), checked like the Seed on pin
//!   day; fork (a later slice) unpacks it.
//! - **Agreement** ([`agree`]): the unpacked Seed's manifest must name the
//!   entry's version, caps and box services exactly; the entry's `requires`
//!   may only be stricter than the manifest's; `build.json` must name the
//!   pinned app, version, variant and commit. Any difference is
//!   `catalog_mismatch` naming the field; nothing is installed.
//! - **Setup steps** are a closed vocabulary ([`SETUP_STEPS`], R27): an
//!   unknown step fails the catalog check, so an older K2 never half-installs
//!   a newer entry.
//! - **Addresses** (R7, R10, R14): the install id is not the address. An
//!   entry declares its family label (`chat`, `meet`), its default, and an
//!   optional `altLabel` for a second App of the family.

pub mod jobs;
pub mod seeds_json;
#[cfg(any(test, feature = "test-util"))]
pub mod testkit;
pub mod unpack;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::deploy::manifest::{Manifest, Requires};
use crate::brand::{CLI_NAME, PRODUCT_NAME};
use crate::product_name;

/// The compiled-in catalog (R28).
pub const CATALOG_JSON: &str = include_str!("catalog.json");
pub const SCHEMA: u32 = 1;

/// The K2 sign-in variant (`<app>-k2-<v>.tar.gz`) and Sew's own logins.
pub const VARIANT_K2: &str = "k2";
pub const VARIANT_STANDALONE: &str = "standalone";
pub const VARIANTS: &[&str] = &[VARIANT_K2, VARIANT_STANDALONE];

/// Label families (R10). `mail` and `clndr` come with later entries.
pub const FAMILIES: &[&str] = &["chat", "meet"];

/// The closed setup vocabulary (R27, A2). Variant selection is not a step:
/// the job always picks the variant from the box's Sew identity.
pub const SETUP_MEET_INIT: &str = "sew.meet_init";
pub const SETUP_APP_SPACE_RECORD: &str = "sew.app_space_record";
pub const SETUP_MEETINGS_ENABLED: &str = "sew.policy:meetings.enabled=true";
pub const SETUP_STEPS: &[&str] = &[SETUP_MEET_INIT, SETUP_APP_SPACE_RECORD, SETUP_MEETINGS_ENABLED];

/// Largest Seed tarball a catalog may pin (a Seed is ~1.6 MB today).
pub const MAX_TARBALL: u64 = 256 * 1024 * 1024;

/// Refusal codes.
pub mod codes {
    pub const CATALOG_MISMATCH: &str = "catalog_mismatch";
    pub const CATALOG_INVALID: &str = "catalog_invalid";
    pub const ENTRY_NOT_FOUND: &str = "catalog_entry_not_found";
    pub const NOT_PINNED: &str = "catalog_not_pinned";
    pub const VERSION_NOT_FOUND: &str = "catalog_version_not_found";
    pub const VARIANT_MISMATCH: &str = "variant_mismatch";
    pub const SETUP_UNKNOWN: &str = "setup_unknown";
    pub const UNPACK_REFUSED: &str = "unpack_refused";
    pub const ADDRESS_TAKEN: &str = "address_taken";
    pub const ADDRESS_RESERVED: &str = "address_reserved";
    pub const ADDRESS_INVALID: &str = "address_invalid";
    pub const ONE_PER_BOX: &str = "one_per_box";
    pub const PRO_REQUIRED: &str = "pro_required";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Publisher {
    pub id: String,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    /// `sew/hearth`.
    pub entry: String,
    pub name: String,
    pub publisher: Publisher,
    pub summary: String,
    pub explain: Vec<String>,
    /// The label family (R10): `chat`, `meet`.
    pub family: String,
    /// The family's label, the default address.
    pub default_label: String,
    /// A second App of the family prefills this (R14); `None` →
    /// `<installId>-<family>`.
    pub alt_label: Option<String>,
    /// One per box (Meet: Sew's Meetings Space binds once, R15).
    pub one_per_box: bool,
    /// The App id one-click installs as (R7): `hearth`, `sew-meet`.
    pub install_id: String,
    /// The publisher's app name in its release (`seeds.json` `app`).
    pub app: String,
    pub license: String,
    /// The version deployed by default; `None` = the newest pinned.
    pub stable: Option<String>,
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilePin {
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VariantPin {
    pub file: String,
    pub sha256: String,
    pub size: u64,
    /// K2's tree hash of the stored Seed (A12), from pin day.
    pub seed_hash: String,
    /// Sew's content digest (inner SHA256SUMS without build.json), recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub caps: Vec<String>,
    pub box_services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Health {
    pub live: String,
    pub ready: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Version {
    pub version: String,
    /// The sew-releases tag holding the files (`seeds-0.2.4`).
    pub tag: String,
    /// The publisher's source commit (40 hex), what `build.json` names.
    pub commit: String,
    pub requires: Requires,
    /// The Node floor (catalog only, A11: K2's guarded image is Node 24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub variants: BTreeMap<String, VariantPin>,
    /// The publisher's source tarball for this version (fork, later).
    pub source: FilePin,
    pub health: Health,
    #[serde(default)]
    pub data: Vec<String>,
    #[serde(default)]
    pub setup: Vec<String>,
}

/// One version of one entry, as an install job keeps it (`pin_json`): the
/// job resumes from this snapshot, never from a later catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub entry: String,
    pub name: String,
    pub app: String,
    pub family: String,
    pub version: String,
    pub tag: String,
    pub commit: String,
    pub variant: String,
    pub seed: VariantPin,
    pub requires: Requires,
    pub health: Health,
    pub setup: Vec<String>,
}

impl Pin {
    pub fn of(e: &Entry, v: &Version, variant: &str) -> Option<Pin> {
        let seed = v.variants.get(variant)?.clone();
        Some(Pin {
            entry: e.entry.clone(),
            name: e.name.clone(),
            app: e.app.clone(),
            family: e.family.clone(),
            version: v.version.clone(),
            tag: v.tag.clone(),
            commit: v.commit.clone(),
            variant: variant.to_string(),
            seed,
            requires: v.requires.clone(),
            health: v.health.clone(),
            setup: v.setup.clone(),
        })
    }
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A file name in a release: no path, no leading dot.
fn plain_file(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('.')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// A release tag (`seeds-0.2.4`, `0.7.3`).
pub fn plain_tag(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// A one-level address label (`chat`, `team-meet`).
pub fn valid_label(s: &str) -> bool {
    crate::tunnel::client_ip_labels::normalize_label(s).is_ok_and(|l| l == s) && s.len() <= 40
}

/// Why `c` is not a well-formed catalog, or `Ok`. The compiled-in file is
/// checked by a test; a malformed one never loads ([`catalog`]).
pub fn check(c: &Catalog) -> Result<(), String> {
    if c.schema != SCHEMA {
        return Err(format!("catalog schema {} (this {PRODUCT_NAME} reads {SCHEMA})", c.schema));
    }
    let mut seen = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for e in &c.entries {
        let at = &e.entry;
        let (publ, name) = e.entry.split_once('/').ok_or_else(|| format!("{at}: an entry is <publisher>/<name>"))?;
        if publ != e.publisher.id || !valid_label(publ) || !valid_label(name) {
            return Err(format!("{at}: the entry id must be <publisher id>/<name>, both labels"));
        }
        if !seen.insert(e.entry.clone()) {
            return Err(format!("{at}: listed twice"));
        }
        if !FAMILIES.contains(&e.family.as_str()) || e.default_label != e.family {
            return Err(format!("{at}: family {} / defaultLabel {} (the default is the family's label)", e.family, e.default_label));
        }
        if let Some(a) = &e.alt_label {
            if !valid_label(a) || crate::skin::is_reserved_nested_label(a) {
                return Err(format!("{at}: altLabel {a} is not an ordinary label"));
            }
        }
        if crate::deploy::app_id_problem(&e.install_id).is_some() || e.install_id.len() > 28 || e.install_id.contains("--") {
            return Err(format!("{at}: installId {} must be a free App id of at most 28 characters", e.install_id));
        }
        if !ids.insert(e.install_id.clone()) {
            return Err(format!("{at}: installId {} is another entry's", e.install_id));
        }
        if e.explain.is_empty() {
            return Err(format!("{at}: explain says what it will do"));
        }
        let mut versions = BTreeSet::new();
        for v in &e.versions {
            let vat = format!("{at} {}", v.version);
            semver::Version::parse(&v.version).map_err(|_| format!("{vat}: not an x.y.z version"))?;
            if !versions.insert(v.version.clone()) {
                return Err(format!("{vat}: listed twice"));
            }
            if !plain_tag(&v.tag) || !hex40(&v.commit) {
                return Err(format!("{vat}: tag or commit"));
            }
            if v.variants.is_empty() {
                return Err(format!("{vat}: no variant"));
            }
            for (name, p) in &v.variants {
                if !VARIANTS.contains(&name.as_str()) {
                    return Err(format!("{vat}: variant {name}"));
                }
                if !plain_file(&p.file) || !hex64(&p.sha256) || !hex64(&p.seed_hash) || p.size == 0 || p.size > MAX_TARBALL {
                    return Err(format!("{vat} {name}: file, sha256, seedHash or size"));
                }
            }
            if !plain_file(&v.source.file) || !hex64(&v.source.sha256) || v.source.size == 0 || v.source.size > MAX_TARBALL {
                return Err(format!("{vat}: source file, sha256 or size"));
            }
            if !v.health.live.starts_with('/') || !v.health.ready.starts_with('/') {
                return Err(format!("{vat}: health paths"));
            }
            for s in &v.setup {
                if !SETUP_STEPS.contains(&s.as_str()) {
                    return Err(format!("{}: {vat}: setup step {s} is not one this {PRODUCT_NAME} knows", codes::SETUP_UNKNOWN));
                }
            }
        }
        if let Some(st) = &e.stable {
            if !versions.contains(st) {
                return Err(format!("{at}: stable {st} is not a listed version"));
            }
        }
    }
    Ok(())
}

pub fn parse(text: &str) -> Result<Catalog, String> {
    let c: Catalog = serde_json::from_str(text).map_err(|e| format!("{}: {e}", codes::CATALOG_INVALID))?;
    check(&c)?;
    Ok(c)
}

/// The compiled-in catalog. A malformed file (a test refuses it) is an
/// empty catalog plus its error, never a partial one.
pub fn catalog() -> Result<&'static Catalog, String> {
    static C: OnceLock<Result<Catalog, String>> = OnceLock::new();
    C.get_or_init(|| parse(CATALOG_JSON)).as_ref().map_err(Clone::clone)
}

impl Catalog {
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        let id = id.trim().to_ascii_lowercase();
        self.entries.iter().find(|e| e.entry == id)
    }
}

/// The badge a verified entry shows (Settings, `k2 app catalog`).
pub const VERIFIED_WORDS: &str = concat!("Verified by ", product_name!());

impl Entry {
    /// "Verified by K2" holds only when a version is pinned in this K2 and
    /// every file it would install carries a pinned sha256 and tree hash
    /// that the job checks. An entry with nothing published yet has nothing
    /// to verify, so it never shows the badge.
    pub fn verified(&self) -> bool {
        self.version(None).is_ok_and(|v| !v.variants.is_empty() && v.variants.values().all(|p| hex64(&p.sha256) && hex64(&p.seed_hash)))
    }

    /// The version to deploy: the one asked for, else `stable`, else the
    /// newest. `Err((code, hint))`.
    pub fn version(&self, asked: Option<&str>) -> Result<&Version, (&'static str, String)> {
        if self.versions.is_empty() {
            return Err((
                codes::NOT_PINNED,
                format!("no version of {} is pinned in this {PRODUCT_NAME} yet; it arrives with a {PRODUCT_NAME} update once {} publishes it", self.name, self.publisher.name),
            ));
        }
        if let Some(a) = asked.map(str::trim).filter(|a| !a.is_empty()) {
            return self.versions.iter().find(|v| v.version == a).ok_or_else(|| {
                let have: Vec<&str> = self.versions.iter().map(|v| v.version.as_str()).collect();
                (codes::VERSION_NOT_FOUND, format!("{} has no version {a} in this {PRODUCT_NAME} (pinned: {})", self.name, have.join(", ")))
            });
        }
        if let Some(st) = &self.stable {
            if let Some(v) = self.versions.iter().find(|v| &v.version == st) {
                return Ok(v);
            }
        }
        let newest = self
            .versions
            .iter()
            .max_by(|a, b| semver::Version::parse(&a.version).ok().cmp(&semver::Version::parse(&b.version).ok()))
            .expect("non-empty");
        Ok(newest)
    }

    /// The address a second App of this family prefills (R14).
    pub fn second_label(&self, install_id: &str) -> String {
        self.alt_label.clone().unwrap_or_else(|| format!("{install_id}-{}", self.family))
    }
}

/// The variant for a Sew identity (`k2` or `standalone`, from
/// `row_provider`). A forced other one is `variant_mismatch` (R27): the
/// other pairing breaks sign-in for everyone.
pub fn pick_variant<'a>(v: &'a Version, identity: &str, forced: Option<&str>) -> Result<(&'a str, &'a VariantPin), (&'static str, String)> {
    let want = match identity {
        VARIANT_K2 | VARIANT_STANDALONE => identity,
        other => return Err((codes::VARIANT_MISMATCH, format!("this box's Sew identity is '{other}', which no variant goes with"))),
    };
    if let Some(f) = forced.map(str::trim).filter(|f| !f.is_empty()) {
        if f != want {
            return Err((
                codes::VARIANT_MISMATCH,
                format!("this box's Sew signs people in with {want} identity, so only the {want} variant works here (asked: {f}); the other pairing breaks sign-in for everyone"),
            ));
        }
    }
    v.variants
        .get_key_value(want)
        .map(|(k, p)| (k.as_str(), p))
        .ok_or_else(|| (codes::VARIANT_MISMATCH, format!("version {} has no {want} variant (this box's Sew identity is {want})", v.version)))
}

// ── agreement (R26, A12) ─────────────────────────────────────────────

/// A field where the Seed and the catalog disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub field: &'static str,
    pub detail: String,
}

impl Mismatch {
    pub fn new(field: &'static str, detail: impl Into<String>) -> Self {
        Mismatch { field, detail: detail.into() }
    }
    pub fn hint(&self) -> String {
        format!("{} ({}): {}; nothing was installed", codes::CATALOG_MISMATCH, self.field, self.detail)
    }
    /// The same in plain words for people (the install job's failure text).
    pub fn text(&self) -> String {
        format!("The downloaded files aren't exactly what this {PRODUCT_NAME} release pinned ({}: {}), so nothing was installed", self.field, self.detail)
    }
}

fn floor(r: &str) -> Option<semver::Version> {
    let t = r.trim().strip_prefix(">=")?.trim();
    semver::Version::parse(t).ok()
}

/// `entry` is the same as `manifest`, or a stricter `>=x.y.z` floor.
fn stricter_or_same(entry: Option<&str>, manifest: Option<&str>) -> bool {
    match (entry.map(str::trim), manifest.map(str::trim)) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(e), Some(m)) if e == m => true,
        (Some(e), Some(m)) => matches!((floor(e), floor(m)), (Some(a), Some(b)) if a >= b),
    }
}

/// The catalog's `requires` may only be stricter than the Seed's (A12).
pub fn requires_ok(entry: &Requires, manifest: Option<&Requires>) -> Result<(), Mismatch> {
    let Some(m) = manifest else { return Ok(()) };
    if !stricter_or_same(entry.k2.as_deref(), m.k2.as_deref()) {
        return Err(Mismatch::new("requires.k2", format!("the Seed asks {:?}, the catalog {:?}", m.k2, entry.k2)));
    }
    if !stricter_or_same(entry.sew.as_deref(), m.sew.as_deref()) {
        return Err(Mismatch::new("requires.sew", format!("the Seed asks {:?}, the catalog {:?}", m.sew, entry.sew)));
    }
    if m.voice && !entry.voice {
        return Err(Mismatch::new("requires.voice", "the Seed needs voice, the catalog doesn't say so"));
    }
    if m.features.iter().any(|f| !entry.features.contains(f)) {
        return Err(Mismatch::new("requires.features", format!("the Seed asks {:?}, the catalog {:?}", m.features, entry.features)));
    }
    if m.postgres != entry.postgres {
        return Err(Mismatch::new("requires.postgres", format!("the Seed asks {:?}, the catalog {:?}", m.postgres, entry.postgres)));
    }
    Ok(())
}

fn set(v: &[String]) -> BTreeSet<&str> {
    v.iter().map(String::as_str).collect()
}

/// The unpacked Seed (`manifest`, and `build.json` when the publisher
/// ships one) against the pin: version, caps and box services exactly;
/// `requires` no looser; build.json's app, version, variant and commit.
pub fn agree(pin: &Pin, manifest: &Manifest, build_json: Option<&serde_json::Value>) -> Result<(), Mismatch> {
    if manifest.version != pin.version {
        return Err(Mismatch::new("version", format!("the Seed says {}, the catalog {}", manifest.version, pin.version)));
    }
    if set(&manifest.caps) != set(&pin.seed.caps) {
        return Err(Mismatch::new("caps", format!("the Seed asks {:?}, the catalog {:?}", manifest.caps, pin.seed.caps)));
    }
    let names: Vec<String> = manifest.box_services.iter().map(|b| b.name.clone()).collect();
    if set(&names) != set(&pin.seed.box_services) {
        return Err(Mismatch::new("boxServices", format!("the Seed asks {names:?}, the catalog {:?}", pin.seed.box_services)));
    }
    requires_ok(&pin.requires, manifest.requires.as_ref())?;
    let b = build_json.ok_or_else(|| Mismatch::new("build.json", "the Seed has no build.json"))?;
    for (k, want) in [("app", pin.app.as_str()), ("version", pin.version.as_str()), ("variant", pin.variant.as_str()), ("commit", pin.commit.as_str())] {
        let got = b.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if got != want {
            return Err(Mismatch::new("build.json", format!("its {k} is {got:?}, the catalog's {want:?}")));
        }
    }
    Ok(())
}

/// The health answer (`/readyz`) names what was installed (Sew's step 7):
/// `None` = it matches.
pub fn health_mismatch(pin: &Pin, v: &serde_json::Value) -> Option<String> {
    for (k, want) in [("app", pin.app.as_str()), ("version", pin.version.as_str()), ("variant", pin.variant.as_str()), ("commit", pin.commit.as_str())] {
        let got = v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if got != want {
            return Some(format!("{k} is {got:?}, the catalog's {want:?}"));
        }
    }
    None
}

/// The Sew floor before anything is downloaded (Sew's "how K2's installer
/// reads it", step 2): `Some(why)` when this box's Sew is older than the
/// pin's `requires.sew`. Unknown versions pass here; deploy checks again.
pub fn sew_floor_unmet(pin: &Pin, sew_version: Option<&str>) -> Option<String> {
    let want = pin.requires.sew.as_deref()?;
    let have = sew_version?;
    let req = semver::VersionReq::parse(&want.split_whitespace().collect::<Vec<_>>().join(", ")).ok()?;
    let v = semver::Version::parse(have.trim().trim_start_matches('v')).ok()?;
    (!req.matches(&v)).then(|| format!("{} {} needs Sew {want}; this box runs {have} ({CLI_NAME} hostchat upgrade)", pin.name, pin.version))
}

// ── addresses (R7, R10, R13, R14) ────────────────────────────────────

/// An address as typed: `chat`, `Chat.box.k2.dev`, `auto`. `Ok(None)` =
/// `auto` (take the default, else the suggestion).
pub fn normalize_address(raw: &str) -> Result<Option<String>, String> {
    let t = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if t == "auto" {
        return Ok(None);
    }
    let host = crate::tunnel::config::SUBDOMAIN_HOST;
    let first = if t.ends_with(&format!(".{host}")) {
        // `<label>.<box>.k2.dev`: one level under the box only (R12).
        let parts: Vec<&str> = t.split('.').collect();
        if parts.len() != 4 {
            return Err(format!("{raw} is more than one level under your box: an address is one label (chat, team-meet)"));
        }
        parts[0].to_string()
    } else {
        t
    };
    if first.contains('.') {
        return Err(format!(
            "{raw} looks like your own domain: custom domains at deploy come in a later {PRODUCT_NAME}; deploy with a {PRODUCT_NAME} address, then add it with {CLI_NAME} app domain add <app> {raw}"
        ));
    }
    if !valid_label(&first) {
        return Err(format!("{raw} is not an address label: letters, digits and inner '-', at most 40"));
    }
    Ok(Some(first))
}

/// The install id for a new App of `e` (R7): `installId`, then
/// `<installId>-2`, `-3`… `taken` says an App id is in use.
pub fn install_id_for(e: &Entry, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(&e.install_id) {
        return e.install_id.clone();
    }
    (2..1000).map(|n| format!("{}-{n}", e.install_id)).find(|id| !taken(id)).unwrap_or_else(|| format!("{}-x", e.install_id))
}

/// The suggestion for a taken family label: the entry's `altLabel` (or
/// `<installId>-<family>`), numbered only when that is taken too.
pub fn suggestion(e: &Entry, install_id: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let base = e.second_label(install_id);
    if !taken(&base) {
        return base;
    }
    (2..1000).map(|n| format!("{base}-{n}")).find(|l| !taken(l)).unwrap_or(base)
}

#[cfg(test)]
mod tests;
