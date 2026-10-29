//! Tauri's two update answers, from the records (TC-9).
//!
//! - **Static** (`latest.json`): the channel's current release with every
//!   platform it carries.
//! - **Dynamic** (`/<channel>/<target>/<arch>/<current_version>`): the
//!   newest release the channel has carried **that has this platform**; an
//!   update when it is newer than the caller's version, else none (204).
//!
//! Every `url` is the download host's immutable URL. Only `live` releases
//! with every file uploaded are served; a pulled (`yanked`) release never.

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde_json::{json, Value as J};

use super::store;

/// One release as the feed serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRelease {
    pub id: String,
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: String,
    /// platform key → (url, base64 signature)
    pub platforms: BTreeMap<String, (String, String)>,
}

/// One channel: its current release first, then older live releases it has
/// carried (newest first, never above the current one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedChannel {
    pub releases: Vec<FeedRelease>,
}

impl FeedChannel {
    pub fn current(&self) -> &FeedRelease {
        &self.releases[0]
    }
}

/// An App's whole feed: channel → releases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed {
    pub channels: BTreeMap<String, FeedChannel>,
}

fn feed_release(conn: &Connection, r: &store::ReleaseRow) -> rusqlite::Result<Option<FeedRelease>> {
    if r.state != store::STATE_LIVE {
        return Ok(None);
    }
    let arts = store::artifacts(conn, &r.id)?;
    if arts.is_empty() || arts.iter().any(|a| a.upload_state != store::UP_UPLOADED || a.url.is_none()) {
        return Ok(None);
    }
    let platforms = arts.into_iter().map(|a| (a.platform, (a.url.unwrap_or_default(), a.signature))).collect();
    Ok(Some(FeedRelease { id: r.id.clone(), version: r.version.clone(), notes: r.notes.clone(), pub_date: r.pub_date.clone(), platforms }))
}

/// Read the App's feed from the records.
pub fn load(conn: &Connection, app: &str) -> rusqlite::Result<Feed> {
    let mut out = Feed::default();
    for ch in store::channels(conn, app)? {
        let Some(cur_row) = store::release(conn, &ch.release_id)? else { continue };
        let Some(cur) = feed_release(conn, &cur_row)? else { continue };
        let cur_v = semver::Version::parse(&cur.version).ok();
        let mut seen = vec![cur.id.clone()];
        let mut older: Vec<FeedRelease> = Vec::new();
        for id in store::channel_history(conn, app, &ch.channel)?.into_iter().rev() {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            let Some(row) = store::release(conn, &id)? else { continue };
            let Some(fr) = feed_release(conn, &row)? else { continue };
            let below = match (&cur_v, semver::Version::parse(&fr.version).ok()) {
                (Some(c), Some(v)) => super::cmp_versions(&v, c) == std::cmp::Ordering::Less,
                _ => false,
            };
            if below {
                older.push(fr);
            }
        }
        older.sort_by(|a, b| match (semver::Version::parse(&b.version), semver::Version::parse(&a.version)) {
            (Ok(x), Ok(y)) => super::cmp_versions(&x, &y),
            _ => std::cmp::Ordering::Equal,
        });
        let mut releases = vec![cur];
        releases.extend(older);
        out.channels.insert(ch.channel.clone(), FeedChannel { releases });
    }
    Ok(out)
}

/// Tauri's static `latest.json` for a release.
pub fn static_manifest(r: &FeedRelease) -> J {
    let platforms: serde_json::Map<String, J> =
        r.platforms.iter().map(|(k, (url, sig))| (k.clone(), json!({ "signature": sig, "url": url }))).collect();
    let mut m = json!({ "version": r.version, "pub_date": r.pub_date, "platforms": platforms });
    if let Some(n) = &r.notes {
        m["notes"] = json!(n);
    }
    m
}

/// The installer a dynamic answer prefers when the release has no plain
/// `<os>-<arch>` file (Tauri's default updater bundles first).
const INSTALLER_PREFERENCE: &[&str] = &["app", "appimage", "nsis", "msi", "deb", "rpm"];

/// The platform key of `r` that serves `target`-`arch`, if any.
pub fn platform_for<'a>(r: &'a FeedRelease, target: &str, arch: &str) -> Option<&'a str> {
    let base = format!("{target}-{arch}");
    if let Some((k, _)) = r.platforms.get_key_value(&base) {
        return Some(k.as_str());
    }
    INSTALLER_PREFERENCE.iter().find_map(|i| r.platforms.get_key_value(&format!("{base}-{i}")).map(|(k, _)| k.as_str()))
}

/// What the dynamic endpoint answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Dynamic {
    /// 200 with Tauri's dynamic JSON.
    Update { body: J, version: String, platform: String },
    /// 204: the caller is current (or no release has its platform).
    NoUpdate,
    /// 400: the target, arch or version isn't one Tauri sends.
    Bad(String),
}

/// The dynamic answer for a caller on `target`/`arch` at `current`.
pub fn dynamic(ch: &FeedChannel, target: &str, arch: &str, current: &str) -> Dynamic {
    if !super::OSES.contains(&target) || !super::ARCHES.contains(&arch) {
        return Dynamic::Bad(format!("unknown target or arch: {target}/{arch}"));
    }
    let Ok(cur) = semver::Version::parse(current.trim_start_matches('v')) else {
        return Dynamic::Bad(format!("current version '{current}' isn't SemVer"));
    };
    for r in &ch.releases {
        let Some(key) = platform_for(r, target, arch) else { continue };
        let Ok(v) = semver::Version::parse(&r.version) else { continue };
        if super::cmp_versions(&v, &cur) != std::cmp::Ordering::Greater {
            return Dynamic::NoUpdate;
        }
        let (url, sig) = &r.platforms[key];
        let mut body = json!({ "version": r.version, "url": url, "signature": sig, "pub_date": r.pub_date });
        if let Some(n) = &r.notes {
            body["notes"] = json!(n);
        }
        return Dynamic::Update { body, version: r.version.clone(), platform: key.to_string() };
    }
    Dynamic::NoUpdate
}
