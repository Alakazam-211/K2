//! The Android SDK on first use (M-A23): read Google's repository XML,
//! download exactly three packages (platform-tools, emulator, one
//! `google_apis` system image) into K2's own folder, verify each archive's
//! sha1 and size against the XML (Google publishes sha1 only; S0), and
//! extract with unix modes and symlinks. No sdkmanager, no Java, never
//! bundled (the licence forbids redistribution).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::lane::{LaneError, LicenceText};
use super::xml;

pub const REPO_BASE: &str = "https://dl.google.com/android/repository/";
pub const SYS_BASE: &str = "https://dl.google.com/android/repository/sys-img/google_apis/";
pub const REPO_XML: &str = "repository2-3.xml";
pub const SYS_XML: &str = "sys-img2-3.xml";
pub const LICENCE_ID: &str = "android-sdk-license";
pub const LICENCE_TITLE: &str = "Android Software Development Kit License Agreement";
pub const LICENCE_URL: &str = "https://developer.android.com/studio/terms";
/// The system image API level (S0: android-35 google_apis boots in
/// 22–37 s cold, 4.3 s from Quick Boot on KVM + swiftshader).
pub const API: u32 = 35;
/// Re-read the repository XML at most this often.
const XML_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// The image ABI for this host.
pub fn host_abi() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64-v8a"
    } else {
        "x86_64"
    }
}

fn host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macosx"
    } else {
        "linux"
    }
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    }
}

/// Paths under `<root>/sdk`.
pub struct Sdk {
    pub root: PathBuf,
}

impl Sdk {
    pub fn new(root: PathBuf) -> Sdk {
        Sdk { root }
    }
    pub fn emulator(&self) -> PathBuf {
        self.root.join("emulator").join("emulator")
    }
    pub fn adb(&self) -> PathBuf {
        self.root.join("platform-tools").join("adb")
    }
    pub fn image_rel(&self) -> String {
        format!("system-images/android-{API}/google_apis/{}/", host_abi())
    }
    pub fn image_dir(&self) -> PathBuf {
        self.root.join(self.image_rel())
    }
    /// Every piece is in place.
    pub fn installed(&self) -> bool {
        self.emulator().is_file() && self.adb().is_file() && self.image_dir().join("system.img").is_file()
    }
    /// What's missing, for the probe.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if !self.adb().is_file() {
            v.push("platform-tools");
        }
        if !self.emulator().is_file() {
            v.push("emulator");
        }
        if !self.image_dir().join("system.img").is_file() {
            v.push("system image");
        }
        v
    }
    fn cache(&self) -> PathBuf {
        self.root.join(".k2-cache")
    }
}

/// One downloadable archive from the XML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub path: String,
    pub revision: String,
    pub licence: Option<String>,
    pub url: String,
    pub size: u64,
    pub sha1: String,
}

/// The parsed repository: packages and licence texts.
#[derive(Debug, Clone, Default)]
pub struct Repo {
    pub packages: Vec<Package>,
    pub licences: Vec<(String, String)>,
}

fn revision(p: &xml::Element) -> String {
    let Some(r) = p.child("revision") else { return "?".into() };
    ["major", "minor", "micro"].iter().map(|k| r.child_text(k).unwrap_or("0").to_string()).collect::<Vec<_>>().join(".")
}

/// Parse one repository document; `base` resolves relative archive URLs.
pub fn parse_repo(doc: &str, base: &str) -> Result<Repo, String> {
    let root = xml::parse(doc)?;
    let mut repo = Repo::default();
    for l in root.children_named("license") {
        if let Some(id) = l.attr("id") {
            repo.licences.push((id.to_string(), l.text.clone()));
        }
    }
    for p in root.children_named("remotePackage") {
        let Some(path) = p.attr("path") else { continue };
        let stable = p.child("channelRef").and_then(|c| c.attr("ref")).map(|r| r == "channel-0").unwrap_or(true);
        if !stable {
            continue;
        }
        let licence = p.child("uses-license").and_then(|u| u.attr("ref")).map(str::to_string);
        let mut archives = Vec::new();
        p.descendants("archive", &mut archives);
        for a in archives {
            let os = a.child_text("host-os");
            let arch = a.child_text("host-arch");
            if os.is_some_and(|o| o != host_os()) || arch.is_some_and(|x| x != host_arch()) {
                continue;
            }
            let Some(c) = a.child("complete") else { continue };
            let (Some(url), Some(size), Some(sum)) = (c.child_text("url"), c.child_text("size"), c.child("checksum")) else { continue };
            if sum.attr("type").is_some_and(|t| t != "sha1") {
                continue;
            }
            let url = if url.starts_with("https://") { url.to_string() } else { format!("{base}{url}") };
            repo.packages.push(Package {
                path: path.to_string(),
                revision: revision(p),
                licence: licence.clone(),
                url,
                size: size.parse().map_err(|_| format!("bad size for {path}"))?,
                sha1: sum.text.trim().to_ascii_lowercase(),
            });
            break;
        }
    }
    Ok(repo)
}

fn http() -> Result<reqwest::blocking::Client, LaneError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(None)
        .user_agent(concat!("K2/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| LaneError::tool(format!("http client: {e}")))
}

/// Read (or re-use a fresh cached copy of) one repository XML.
fn fetch_xml(sdk: &Sdk, base: &str, name: &str) -> Result<String, LaneError> {
    let cache = sdk.cache().join(name);
    if let Ok(m) = std::fs::metadata(&cache) {
        if m.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < XML_MAX_AGE) {
            if let Ok(s) = std::fs::read_to_string(&cache) {
                return Ok(s);
            }
        }
    }
    let url = format!("{base}{name}");
    let r = http()?
        .get(&url)
        .timeout(Duration::from_secs(60))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| LaneError::tool(format!("download {url}: {e}")))?;
    let body = r.text().map_err(|e| LaneError::tool(format!("read {url}: {e}")))?;
    if body.len() > 16 * 1024 * 1024 {
        return Err(LaneError::tool(format!("{url} is over 16 MB")));
    }
    let _ = std::fs::create_dir_all(sdk.cache());
    let _ = crate::fs_atomic::atomic_write(&cache, body.as_bytes());
    Ok(body)
}

fn merge(mut a: Repo, b: Repo) -> Repo {
    a.packages.extend(b.packages);
    for l in b.licences {
        if !a.licences.iter().any(|(id, _)| *id == l.0) {
            a.licences.push(l);
        }
    }
    a
}

/// Both repositories, merged.
pub fn repo(sdk: &Sdk) -> Result<Repo, LaneError> {
    let a = parse_repo(&fetch_xml(sdk, REPO_BASE, REPO_XML)?, REPO_BASE).map_err(|e| LaneError::tool(format!("{REPO_XML}: {e}")))?;
    let b = parse_repo(&fetch_xml(sdk, SYS_BASE, SYS_XML)?, SYS_BASE).map_err(|e| LaneError::tool(format!("{SYS_XML}: {e}")))?;
    Ok(merge(a, b))
}

/// The repositories as last downloaded (whatever their age), with no
/// network: the probe's view. `None` before the first download.
pub fn cached_repo(sdk: &Sdk) -> Option<Repo> {
    let read = |name: &str| std::fs::read_to_string(sdk.cache().join(name)).ok();
    let a = parse_repo(&read(REPO_XML)?, REPO_BASE).ok()?;
    let b = parse_repo(&read(SYS_XML)?, SYS_BASE).ok()?;
    Some(merge(a, b))
}

/// What people read for a licence id from Google's metadata. The ids
/// always come from the metadata; one K2 has no title for shows as itself.
pub fn licence_title(id: &str) -> String {
    match id {
        LICENCE_ID => LICENCE_TITLE.to_string(),
        "android-sdk-preview-license" => "Android SDK Preview License Agreement".to_string(),
        other => format!("Android SDK licence '{other}'"),
    }
}

fn licence_url(id: &str) -> Option<String> {
    (id == LICENCE_ID).then(|| LICENCE_URL.to_string())
}

/// The packages K2 still has to download (no install marker yet).
fn todo(sdk: &Sdk, repo: &Repo) -> Result<Vec<(Package, PathBuf)>, LaneError> {
    Ok(wanted(repo)?.into_iter().filter(|(_, rel)| !sdk.root.join(rel).join(".k2-package.json").is_file()).collect())
}

/// Every licence id the packages still to download use (each package's
/// `uses-license` in the repository metadata, for this host's archives and
/// image ABI), in package order, each once. Installed packages need none.
pub fn licences_needed(sdk: &Sdk, repo: &Repo) -> Result<Vec<String>, LaneError> {
    let mut ids: Vec<String> = Vec::new();
    for (p, _) in todo(sdk, repo)? {
        if let Some(l) = p.licence {
            if !ids.contains(&l) {
                ids.push(l);
            }
        }
    }
    Ok(ids)
}

/// Every licence the setup still needs that isn't in `accepted`, with its
/// text from the metadata (one owner action covers them all).
pub fn pending_licences(sdk: &Sdk, accepted: &[String]) -> Result<Vec<LicenceText>, LaneError> {
    let r = repo(sdk)?;
    licences_needed(sdk, &r)?.into_iter().filter(|id| !accepted.contains(id)).map(|id| licence_text(&r, &id)).collect()
}

fn licence_text(r: &Repo, id: &str) -> Result<LicenceText, LaneError> {
    let text = r
        .licences
        .iter()
        .find(|(lid, _)| lid == id)
        .map(|(_, t)| t.trim().to_string())
        .ok_or_else(|| LaneError::invalid(format!("no licence '{id}' in Google's repository")))?;
    Ok(LicenceText { id: id.to_string(), title: licence_title(id), sha1: crate::sim::licence::text_sha1(&text), text, url: licence_url(id) })
}

/// The three packages K2 installs, from the repository.
pub fn wanted(repo: &Repo) -> Result<Vec<(Package, PathBuf)>, LaneError> {
    let pick = |path: &str| {
        repo.packages
            .iter()
            .find(|p| p.path == path)
            .cloned()
            .ok_or_else(|| LaneError::tool(format!("Google's repository has no {path} for this host")))
    };
    Ok(vec![
        (pick("platform-tools")?, PathBuf::from("platform-tools")),
        (pick("emulator")?, PathBuf::from("emulator")),
        (pick(&format!("system-images;android-{API};google_apis;{}", host_abi()))?, PathBuf::from(format!("system-images/android-{API}/google_apis/{}", host_abi()))),
    ])
}

/// The licence text for the card.
pub fn licence(sdk: &Sdk, id: &str) -> Result<LicenceText, LaneError> {
    licence_text(&repo(sdk)?, id)
}

/// Download, verify and extract everything that is missing. `accepted`
/// are licence ids the box owner accepted; while any package still to
/// download uses another licence, nothing downloads and the error names
/// every licence missing (never one at a time).
pub fn install(sdk: &Sdk, accepted: &[String], progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), LaneError> {
    let r = repo(sdk)?;
    let todo = todo(sdk, &r)?;
    let missing: Vec<String> = licences_needed(sdk, &r)?.into_iter().filter(|l| !accepted.contains(l)).collect();
    if !missing.is_empty() {
        let lines: Vec<String> = missing
            .iter()
            .map(|l| {
                let pkgs: Vec<&str> = todo.iter().filter(|(p, _)| p.licence.as_deref() == Some(l.as_str())).map(|(p, _)| p.path.as_str()).collect();
                format!("{} {} the licence '{l}'", pkgs.join(", "), if pkgs.len() == 1 { "needs" } else { "need" })
            })
            .collect();
        return Err(LaneError::NeedsSetup {
            message: format!("{}, which the owner hasn't accepted", lines.join("; ")),
            needs: missing.iter().map(|l| crate::sim::lane::LaneNeed::Licence { id: l.clone(), title: licence_title(l) }).collect(),
        });
    }
    let total: u64 = todo.iter().map(|(p, _)| p.size).sum();
    let mut done = 0u64;
    progress(0, total);
    let dl = sdk.cache().join("downloads");
    std::fs::create_dir_all(&dl).map_err(|e| LaneError::tool(format!("mkdir {}: {e}", dl.display())))?;
    for (p, rel) in todo {
        let zip = dl.join(p.url.rsplit('/').next().unwrap_or("pkg.zip"));
        download(&p, &zip, &|n| progress(done + n, total))?;
        done += p.size;
        let dest = sdk.root.join(&rel);
        let partial = sdk.root.join(format!("{}.partial", rel.display()));
        let _ = std::fs::remove_dir_all(&partial);
        extract(&zip, &partial)?;
        let _ = std::fs::remove_file(&zip);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| LaneError::tool(format!("remove {}: {e}", dest.display())))?;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LaneError::tool(format!("mkdir {}: {e}", parent.display())))?;
        }
        std::fs::rename(&partial, &dest).map_err(|e| LaneError::tool(format!("move into {}: {e}", dest.display())))?;
        let marker = serde_json::json!({ "path": p.path, "revision": p.revision, "sha1": p.sha1, "licence": p.licence, "url": p.url });
        crate::fs_atomic::atomic_write(&dest.join(".k2-package.json"), marker.to_string().as_bytes())
            .map_err(|e| LaneError::tool(format!("write marker: {e}")))?;
        progress(done, total);
    }
    // sdkmanager's own licence files, so the SDK's tools agree each one the
    // owner accepted for these packages was accepted.
    let used: Vec<String> = wanted(&r)?.into_iter().filter_map(|(p, _)| p.licence).collect();
    let plain = |id: &str| !id.is_empty() && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    for id in accepted.iter().filter(|a| used.contains(a) && plain(a)) {
        if let Ok(t) = licence_text(&r, id) {
            let dir = sdk.root.join("licenses");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(id), format!("\n{}", t.sha1));
        }
    }
    Ok(())
}

/// Stream one archive to `to`, checking size and sha1; a bad archive is
/// deleted and fails loudly.
fn download(p: &Package, to: &Path, progress: &dyn Fn(u64)) -> Result<(), LaneError> {
    use sha1::{Digest, Sha1};
    let mut resp = http()?
        .get(&p.url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| LaneError::tool(format!("download {}: {e}", p.url)))?;
    let mut f = std::fs::File::create(to).map_err(|e| LaneError::tool(format!("create {}: {e}", to.display())))?;
    let mut h = Sha1::new();
    let mut n = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let k = resp.read(&mut buf).map_err(|e| LaneError::tool(format!("download {}: {e}", p.url)))?;
        if k == 0 {
            break;
        }
        n += k as u64;
        if n > p.size {
            let _ = std::fs::remove_file(to);
            return Err(LaneError::tool(format!("{} is bigger than the repository says ({} bytes)", p.url, p.size)));
        }
        h.update(&buf[..k]);
        f.write_all(&buf[..k]).map_err(|e| LaneError::tool(format!("write {}: {e}", to.display())))?;
        progress(n);
    }
    f.flush().map_err(|e| LaneError::tool(e.to_string()))?;
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if n != p.size || got != p.sha1 {
        let _ = std::fs::remove_file(to);
        return Err(LaneError::tool(format!("{} failed its check: {n} bytes sha1 {got}, expected {} bytes sha1 {}", p.url, p.size, p.sha1)));
    }
    Ok(())
}

/// Extract a package zip into `dest`, dropping its single top folder
/// (`emulator/…`, `platform-tools/…`, `x86_64/…`). Keeps unix modes and
/// symlinks; refuses any entry that would land outside `dest`.
pub fn extract(zip_path: &Path, dest: &Path) -> Result<(), LaneError> {
    let f = std::fs::File::open(zip_path).map_err(|e| LaneError::tool(format!("open {}: {e}", zip_path.display())))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| LaneError::tool(format!("read {}: {e}", zip_path.display())))?;
    std::fs::create_dir_all(dest).map_err(|e| LaneError::tool(format!("mkdir {}: {e}", dest.display())))?;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| LaneError::tool(format!("zip entry {i}: {e}")))?;
        let Some(name) = e.enclosed_name() else {
            return Err(LaneError::tool(format!("zip entry '{}' escapes its folder", e.name())));
        };
        let mut comps = name.components();
        comps.next(); // the top folder
        let rel: PathBuf = comps.collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&rel);
        let mode = e.unix_mode().unwrap_or(0);
        const S_IFMT: u32 = 0o170000;
        const S_IFLNK: u32 = 0o120000;
        if e.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| LaneError::tool(format!("mkdir {}: {e}", out.display())))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LaneError::tool(format!("mkdir {}: {e}", parent.display())))?;
        }
        if mode & S_IFMT == S_IFLNK {
            let mut target = String::new();
            e.read_to_string(&mut target).map_err(|e| LaneError::tool(format!("symlink target: {e}")))?;
            let t = Path::new(&target);
            if t.is_absolute() || out.parent().map(|p| p.join(t)).is_some_and(|j| !normalized(&j).starts_with(dest)) {
                return Err(LaneError::tool(format!("zip symlink {} → {target} escapes its folder", rel.display())));
            }
            let _ = std::fs::remove_file(&out);
            #[cfg(unix)]
            std::os::unix::fs::symlink(t, &out).map_err(|e| LaneError::tool(format!("symlink {}: {e}", out.display())))?;
            continue;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| LaneError::tool(format!("create {}: {e}", out.display())))?;
        std::io::copy(&mut e, &mut w).map_err(|e| LaneError::tool(format!("extract {}: {e}", out.display())))?;
        #[cfg(unix)]
        if mode & 0o777 != 0 {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777));
        }
    }
    Ok(())
}

/// Lexically resolve `..` and `.` (no filesystem access).
fn normalized(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &str = r#"<?xml version="1.0"?>
<sdk:sdk-repository xmlns:sdk="http://schemas.android.com/sdk/android/repo/repository2/03">
  <license id="android-sdk-license" type="text">Terms &amp; conditions.</license>
  <channel id="channel-0">stable</channel>
  <remotePackage path="emulator">
    <revision><major>37</major><minor>2</minor><micro>12</micro></revision>
    <uses-license ref="android-sdk-license"/>
    <channelRef ref="channel-0"/>
    <archives>
      <archive><complete><size>10</size><checksum type="sha1">AAAA</checksum><url>emulator-darwin.zip</url></complete><host-os>macosx</host-os><host-arch>aarch64</host-arch></archive>
      <archive><complete><size>11</size><checksum type="sha1">bbbb</checksum><url>emulator-linux_x64.zip</url></complete><host-os>linux</host-os><host-arch>x64</host-arch></archive>
    </archives>
  </remotePackage>
  <remotePackage path="emulator">
    <revision><major>38</major></revision>
    <channelRef ref="channel-3"/>
    <archives><archive><complete><size>1</size><checksum type="sha1">cc</checksum><url>canary.zip</url></complete></archive></archives>
  </remotePackage>
</sdk:sdk-repository>"#;

    #[test]
    fn picks_this_hosts_stable_archive_and_licence() {
        let r = parse_repo(REPO, REPO_BASE).expect("parses");
        assert_eq!(r.licences, vec![("android-sdk-license".to_string(), "Terms & conditions.".to_string())]);
        assert_eq!(r.packages.len(), 1, "the canary channel is skipped: {:?}", r.packages);
        let p = &r.packages[0];
        assert_eq!(p.revision, "37.2.12");
        assert_eq!(p.licence.as_deref(), Some("android-sdk-license"));
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!((p.url.as_str(), p.size, p.sha1.as_str()), ("https://dl.google.com/android/repository/emulator-linux_x64.zip", 11, "bbbb"));
        }
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(p.sha1, "aaaa", "sha1 is lower-cased");
        }
    }

    /// A repository pair shaped like Google's: the tools under the SDK
    /// licence, this host's system image under a second one (the arm64
    /// image's `android-sdk-arm-dbt-license` on Apple silicon).
    fn fixture_xml() -> (String, String) {
        let host = format!("<host-os>{}</host-os><host-arch>{}</host-arch>", host_os(), host_arch());
        let repo = format!(
            r#"<?xml version="1.0"?>
<sdk:sdk-repository xmlns:sdk="http://schemas.android.com/sdk/android/repo/repository2/03">
  <license id="android-sdk-license" type="text">SDK terms.</license>
  <remotePackage path="platform-tools"><revision><major>36</major></revision><uses-license ref="android-sdk-license"/><channelRef ref="channel-0"/>
    <archives><archive><complete><size>1</size><checksum type="sha1">aa</checksum><url>pt.zip</url></complete>{host}</archive></archives>
  </remotePackage>
  <remotePackage path="emulator"><revision><major>37</major></revision><uses-license ref="android-sdk-license"/><channelRef ref="channel-0"/>
    <archives><archive><complete><size>1</size><checksum type="sha1">bb</checksum><url>emu.zip</url></complete>{host}</archive></archives>
  </remotePackage>
</sdk:sdk-repository>"#
        );
        let sys = format!(
            r#"<?xml version="1.0"?>
<sdk:sdk-sys-img xmlns:sdk="http://schemas.android.com/sdk/android/repo/sys-img2/03">
  <license id="android-sdk-arm-dbt-license" type="text">ARM DBT terms.</license>
  <remotePackage path="system-images;android-{API};google_apis;{abi}"><revision><major>9</major></revision><uses-license ref="android-sdk-arm-dbt-license"/>
    <archives><archive><complete><size>1</size><checksum type="sha1">cc</checksum><url>img.zip</url></complete></archive></archives>
  </remotePackage>
</sdk:sdk-sys-img>"#,
            abi = host_abi()
        );
        (repo, sys)
    }

    fn need_ids(needs: &[crate::sim::lane::LaneNeed]) -> Vec<String> {
        needs.iter().filter_map(|n| if let crate::sim::lane::LaneNeed::Licence { id, .. } = n { Some(id.clone()) } else { None }).collect()
    }

    /// Every licence the setup needs comes from the metadata, up front: the
    /// probe's cached view, the card's texts and the install's refusal (which
    /// names them all and downloads nothing). No network: the cache is fresh.
    #[test]
    fn every_licence_the_missing_packages_use_comes_from_the_metadata() {
        let tmp = std::env::temp_dir().join(format!("k2-sdk-licences-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sdk = Sdk::new(tmp.join("sdk"));
        assert!(cached_repo(&sdk).is_none(), "no metadata before the first download");
        let (repo_xml, sys_xml) = fixture_xml();
        std::fs::create_dir_all(sdk.cache()).expect("cache dir");
        std::fs::write(sdk.cache().join(REPO_XML), &repo_xml).expect("repo xml");
        std::fs::write(sdk.cache().join(SYS_XML), &sys_xml).expect("sys xml");

        let r = cached_repo(&sdk).expect("the cached metadata parses");
        assert_eq!(licences_needed(&sdk, &r).expect("needed"), vec!["android-sdk-license", "android-sdk-arm-dbt-license"]);

        let p = pending_licences(&sdk, &[]).expect("pending");
        let got: Vec<(&str, &str, Option<&str>)> = p.iter().map(|t| (t.id.as_str(), t.title.as_str(), t.url.as_deref())).collect();
        assert_eq!(
            got,
            vec![
                ("android-sdk-license", LICENCE_TITLE, Some(LICENCE_URL)),
                ("android-sdk-arm-dbt-license", "Android SDK licence 'android-sdk-arm-dbt-license'", None)
            ]
        );
        assert_eq!((p[1].text.as_str(), p[1].sha1.clone()), ("ARM DBT terms.", crate::sim::licence::text_sha1("ARM DBT terms.")));
        let p = pending_licences(&sdk, &["android-sdk-license".to_string()]).expect("pending");
        assert_eq!(p.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["android-sdk-arm-dbt-license"]);

        let no_progress = |_: u64, _: u64| {};
        match install(&sdk, &[], &no_progress) {
            Err(LaneError::NeedsSetup { message, needs }) => {
                assert_eq!(need_ids(&needs), vec!["android-sdk-license", "android-sdk-arm-dbt-license"], "{message}");
                assert!(message.contains("platform-tools, emulator need the licence 'android-sdk-license'"), "{message}");
            }
            other => panic!("expected NeedsSetup for both licences, got {other:?}"),
        }
        match install(&sdk, &["android-sdk-license".to_string()], &no_progress) {
            Err(LaneError::NeedsSetup { message, needs }) => {
                assert_eq!(need_ids(&needs), vec!["android-sdk-arm-dbt-license"]);
                assert_eq!(
                    message,
                    format!("system-images;android-{API};google_apis;{} needs the licence 'android-sdk-arm-dbt-license', which the owner hasn't accepted", host_abi())
                );
            }
            other => panic!("expected NeedsSetup for the image's licence, got {other:?}"),
        }
        assert!(!sdk.root.join(".k2-cache").join("downloads").exists(), "nothing downloaded");

        // An installed image needs no licence any more.
        std::fs::create_dir_all(sdk.image_dir()).expect("image dir");
        std::fs::write(sdk.image_dir().join(".k2-package.json"), "{}").expect("marker");
        assert_eq!(licences_needed(&sdk, &r).expect("needed"), vec!["android-sdk-license"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extract_drops_the_top_folder_keeps_modes_and_refuses_escapes() {
        use std::io::Write as _;
        let tmp = std::env::temp_dir().join(format!("k2-sdk-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("tmp");
        let zp = tmp.join("p.zip");
        {
            let f = std::fs::File::create(&zp).expect("zip");
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            w.start_file("emulator/emulator", opts).expect("file");
            w.write_all(b"#!/bin/sh\n").expect("write");
            w.add_symlink("emulator/lib64/libx.so", "libx.so.1", opts).expect("link");
            w.finish().expect("finish");
        }
        let dest = tmp.join("out");
        extract(&zp, &dest).expect("extracts");
        assert!(dest.join("emulator").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dest.join("emulator")).expect("meta").permissions().mode() & 0o777, 0o755);
            assert_eq!(std::fs::read_link(dest.join("lib64/libx.so")).expect("link"), PathBuf::from("libx.so.1"));
        }
        let bad = tmp.join("bad.zip");
        {
            let f = std::fs::File::create(&bad).expect("zip");
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            w.add_symlink("top/evil", "../../../etc/passwd", opts).expect("link");
            w.finish().expect("finish");
        }
        assert!(extract(&bad, &tmp.join("out2")).is_err(), "a symlink out of the folder is refused");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
