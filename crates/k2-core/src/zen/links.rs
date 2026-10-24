//! `k2.open(url)` (catalog `links.open`, cap `links:open`; 0.46.4, Baden's
//! Board via Appa 2026-10-23): the sites a person let a Garden's widgets
//! open in their browser without asking again.
//!
//! - **Who decides.** The first time a widget in a Garden opens a link to a
//!   site, K2 (the renderer, outside the sealed frame) shows its own card:
//!   Don't open / Open once / Always open <site> from this Garden. Only
//!   "Always" lands here. Widgets and agents can't add a site: the write
//!   routes take this computer's owner token only (the app), and agents may
//!   only read the list (`k2 zen links`).
//! - **A site** is a URL's origin: `https://host[:port]`, lower case, the
//!   default port dropped. Only http and https; never a URL with a user
//!   name or password in it.
//! - **Where.** `~/.k2/zen/links.json`, daemon-written (atomic), per
//!   computer: never synced, never a file a Zen route can name, never in a
//!   Garden's TOML. Deleting a Garden drops its row ([`forget_garden`]).
//!
//! Honest bar (as `garden_data`): a same-user process can edit the file;
//! this is a guardrail against a widget opening sites the person never
//! chose, not a wall.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::store::{valid_garden_id, ZenError};

/// The file, in the Zen folder.
pub const LINKS_FILE: &str = "links.json";
/// Its format version.
pub const LINKS_VERSION: u32 = 1;
/// Sites remembered per Garden; "Always" past this refuses (forget one first).
pub const MAX_SITES_PER_GARDEN: usize = 200;
/// The longest URL `k2.open` takes.
pub const MAX_URL_CHARS: usize = 2048;

/// Serialises every change to the file (routes run on worker threads).
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinksFile {
    #[serde(default)]
    pub version: u32,
    /// Garden id → the sites it may open without asking, sorted.
    #[serde(default)]
    pub gardens: BTreeMap<String, Vec<String>>,
}

/// The site (origin) of an http(s) URL, or why `k2.open` refuses it.
pub fn site_of(raw: &str) -> Result<String, ZenError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().count() > MAX_URL_CHARS {
        return Err(ZenError::BadRequest(format!("a link is 1 to {MAX_URL_CHARS} characters")));
    }
    let u = url::Url::parse(raw).map_err(|_| ZenError::BadRequest("that isn't a full http or https link".into()))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err(ZenError::BadRequest("K2 opens only http and https links".into()));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(ZenError::BadRequest("K2 never opens a link with a user name or password in it".into()));
    }
    let host = u.host_str().filter(|h| !h.is_empty()).ok_or_else(|| ZenError::BadRequest("the link has no host".into()))?;
    Ok(match u.port() {
        Some(p) => format!("{}://{}:{p}", u.scheme(), host.to_ascii_lowercase()),
        None => format!("{}://{}", u.scheme(), host.to_ascii_lowercase()),
    })
}

/// A site exactly as [`site_of`] writes it (what the write routes take).
pub fn check_site(site: &str) -> Result<String, ZenError> {
    let s = site_of(site)?;
    if s != site.trim() {
        return Err(ZenError::BadRequest(format!("a site is an origin like {s}, with no path")));
    }
    Ok(s)
}

fn check_garden(garden: &str) -> Result<(), ZenError> {
    if valid_garden_id(garden) {
        Ok(())
    } else {
        Err(ZenError::BadRequest(format!("'{garden}' isn't a Garden id")))
    }
}

/// The file as it is (empty when missing; a broken file reads as empty and
/// is replaced on the next write, so the person is simply asked again).
pub fn read(zen_dir: &Path) -> LinksFile {
    std::fs::read_to_string(zen_dir.join(LINKS_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write(zen_dir: &Path, f: &LinksFile) -> Result<(), ZenError> {
    let path = zen_dir.join(LINKS_FILE);
    let mut f = f.clone();
    f.version = LINKS_VERSION;
    f.gardens.retain(|_, sites| !sites.is_empty());
    let body = serde_json::to_string_pretty(&f).map_err(|e| ZenError::Io(e.to_string()))?;
    std::fs::create_dir_all(zen_dir).map_err(|e| ZenError::Io(format!("{}: {e}", zen_dir.display())))?;
    crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| ZenError::Io(format!("{}: {e}", path.display())))
}

/// The sites `garden` may open without asking.
pub fn sites(zen_dir: &Path, garden: &str) -> Result<Vec<String>, ZenError> {
    check_garden(garden)?;
    Ok(read(zen_dir).gardens.get(garden).cloned().unwrap_or_default())
}

/// Does `garden` open `site` without asking?
pub fn allowed(zen_dir: &Path, garden: &str, site: &str) -> Result<bool, ZenError> {
    Ok(sites(zen_dir, garden)?.iter().any(|s| s == site))
}

/// "Always open <site> from this Garden". Answers false when it was already there.
pub fn allow(zen_dir: &Path, garden: &str, site: &str) -> Result<bool, ZenError> {
    check_garden(garden)?;
    let site = check_site(site)?;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = read(zen_dir);
    let list = f.gardens.entry(garden.to_string()).or_default();
    if list.contains(&site) {
        return Ok(false);
    }
    if list.len() >= MAX_SITES_PER_GARDEN {
        return Err(ZenError::BadRequest(format!(
            "a Garden remembers at most {MAX_SITES_PER_GARDEN} sites; forget one first (k2 zen links forget)"
        )));
    }
    list.push(site);
    list.sort();
    write(zen_dir, &f)?;
    Ok(true)
}

/// Ask again for `site` in `garden`. Answers false when it wasn't there.
pub fn forget(zen_dir: &Path, garden: &str, site: &str) -> Result<bool, ZenError> {
    check_garden(garden)?;
    let site = check_site(site)?;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = read(zen_dir);
    let Some(list) = f.gardens.get_mut(garden) else { return Ok(false) };
    let before = list.len();
    list.retain(|s| *s != site);
    if list.len() == before {
        return Ok(false);
    }
    write(zen_dir, &f)?;
    Ok(true)
}

/// A deleted Garden's row goes (its sites are asked for again if it comes back).
pub fn forget_garden(zen_dir: &Path, garden: &str) -> Result<(), ZenError> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = read(zen_dir);
    if f.gardens.remove(garden).is_some() {
        write(zen_dir, &f)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let d = crate::test_env::unique_temp_path("zen-links");
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }

    #[test]
    fn site_of_is_the_origin_of_http_and_https_only() {
        assert_eq!(site_of("https://Example.com/a/b?c=1#d").expect("ok"), "https://example.com");
        assert_eq!(site_of("http://example.com:8080/x").expect("ok"), "http://example.com:8080");
        assert_eq!(site_of("https://example.com:443/x").expect("ok"), "https://example.com", "the default port is dropped");
        for bad in [
            "",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "mailto:a@b.c",
            "data:text/html,hi",
            "k2://x",
            "/relative",
            "example.com",
            "https://user:pw@example.com/",
            "https://user@example.com/",
        ] {
            match site_of(bad) {
                Err(ZenError::BadRequest(_)) => {}
                other => panic!("{bad:?}: want bad_request, got {other:?}"),
            }
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
        assert!(matches!(site_of(&long), Err(ZenError::BadRequest(_))), "too long");
    }

    #[test]
    fn allow_forget_and_garden_delete() {
        let d = dir();
        assert_eq!(sites(&d, "g-1").expect("sites"), Vec::<String>::new(), "no file: nothing allowed");
        assert!(allow(&d, "g-1", "https://example.com").expect("allow"));
        assert!(!allow(&d, "g-1", "https://example.com").expect("again"), "already there");
        assert!(allow(&d, "g-1", "http://a.test:8080").expect("allow"));
        assert!(allow(&d, "g-2", "https://example.com").expect("allow"));
        assert_eq!(sites(&d, "g-1").expect("sites"), vec!["http://a.test:8080", "https://example.com"]);
        assert!(allowed(&d, "g-1", "https://example.com").expect("allowed"));
        assert!(!allowed(&d, "g-3", "https://example.com").expect("allowed"), "per Garden");
        assert!(forget(&d, "g-1", "https://example.com").expect("forget"));
        assert!(!forget(&d, "g-1", "https://example.com").expect("forget again"));
        assert!(!allowed(&d, "g-1", "https://example.com").expect("allowed"));
        forget_garden(&d, "g-2").expect("forget garden");
        assert_eq!(sites(&d, "g-2").expect("sites"), Vec::<String>::new());
        let f = read(&d);
        assert_eq!(f.version, LINKS_VERSION);
        assert_eq!(f.gardens.keys().collect::<Vec<_>>(), vec!["g-1"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writes_check_their_inputs_and_the_cap() {
        let d = dir();
        for (g, s) in [("../x", "https://example.com"), ("g-1", "https://example.com/path"), ("g-1", "ftp://example.com")] {
            assert!(matches!(allow(&d, g, s), Err(ZenError::BadRequest(_))), "{g} {s}");
        }
        for i in 0..MAX_SITES_PER_GARDEN {
            allow(&d, "g-1", &format!("https://s{i}.test")).expect("allow");
        }
        match allow(&d, "g-1", "https://one-more.test") {
            Err(ZenError::BadRequest(m)) => assert!(m.contains("at most"), "{m}"),
            other => panic!("want the cap, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_broken_file_reads_as_empty_and_is_replaced() {
        let d = dir();
        std::fs::write(d.join(LINKS_FILE), "{not json").expect("write");
        assert_eq!(sites(&d, "g-1").expect("sites"), Vec::<String>::new());
        assert!(allow(&d, "g-1", "https://example.com").expect("allow"));
        assert_eq!(sites(&d, "g-1").expect("sites"), vec!["https://example.com"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
