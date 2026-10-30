//! Sew release auto-fetch (0.46.7) against a loopback HTTP server: the pin
//! is the only check, a mismatch deletes the file, the network policy
//! holds, nothing secret is sent.

use std::fs;

use super::test_server::{Reply, Scratch, Server};
use super::*;

const PAYLOAD: &[u8] = b"pretend this is a Sew release tarball\n";

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn file(bytes: &[u8]) -> ReleaseFile {
    ReleaseFile {
        tag: "0.0.2".into(),
        name: "sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz".into(),
        sha256: sha(bytes),
        cap: 1 << 20,
        label: "Sew 0.0.2".into(),
    }
}

fn path_of(f: &ReleaseFile) -> String {
    format!("/v{}/{}", f.tag, f.name)
}

fn source(s: &Server, dir: &Scratch) -> Source {
    Source { origin: s.origin(), cache_root: Some(dir.0.join("cache").join("sew")) }
}

fn leftovers(src: &Source, f: &ReleaseFile) -> Vec<String> {
    let d = cache_dir(src.cache_root.as_ref().unwrap(), f);
    match fs::read_dir(&d) {
        Ok(rd) => rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect(),
        Err(_) => vec![],
    }
}

#[test]
fn download_verifies_against_the_pin_and_keeps_a_private_cache() {
    let s = Server::start();
    let dir = Scratch::new("ok");
    let f = file(PAYLOAD);
    s.serve(&path_of(&f), Reply::Bytes(PAYLOAD.to_vec()));
    let src = source(&s, &dir);

    let seen = std::cell::RefCell::new(vec![]);
    let got = obtain(&src, &f, &|n, t| seen.borrow_mut().push((n, t))).expect("download");
    assert_eq!(got.how, "download");
    assert_eq!(got.path, cache_dir(src.cache_root.as_ref().unwrap(), &f).join(&f.name));
    assert_eq!(fs::read(&got.path).unwrap(), PAYLOAD);
    assert_eq!(seen.borrow().last().copied(), Some((PAYLOAD.len() as u64, Some(PAYLOAD.len() as u64))));
    assert_eq!(leftovers(&src, &f), vec![f.name.clone()], "no .part left behind");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(src.cache_root.as_ref().unwrap()), 0o700, "cache root");
        assert_eq!(mode(got.path.parent().unwrap()), 0o700, "version dir");
        assert_eq!(mode(&got.path), 0o600, "the file");
    }

    // Second time: the cache, no request.
    let before = s.seen.lock().unwrap().len();
    let again = obtain(&src, &f, &|_, _| panic!("no progress from the cache")).expect("cache");
    assert_eq!(again.how, "cache");
    assert_eq!(s.seen.lock().unwrap().len(), before, "the cache hit made no request");
}

#[test]
fn a_mismatch_is_refused_and_the_file_deleted() {
    let s = Server::start();
    let dir = Scratch::new("mismatch");
    let f = file(PAYLOAD);
    // Same name, other bytes (and a matching-looking .sha256 beside it,
    // which K2 never reads).
    s.serve(&path_of(&f), Reply::Bytes(b"tampered".to_vec()));
    s.serve(&format!("{}.sha256", path_of(&f)), Reply::Bytes(sha(b"tampered").into_bytes()));
    let src = source(&s, &dir);
    let e = download(&src, &f, &|_, _| {}).expect_err("a mismatch must be refused");
    let FetchError::Mismatch(m) = &e else { panic!("not a mismatch: {e:?}") };
    assert!(m.contains(&f.sha256), "names the pin: {m}");
    assert!(m.contains(&sha(b"tampered")), "names what arrived: {m}");
    assert!(m.contains(&f.url()), "names the public URL: {m}");
    assert!(m.contains("deleted"), "{m}");
    assert!(leftovers(&src, &f).is_empty(), "nothing kept: {:?}", leftovers(&src, &f));
    assert!(!s.seen.lock().unwrap().iter().any(|l| l.contains(".sha256")), "the .sha256 file is never fetched");
}

#[test]
fn a_bad_cached_file_is_removed_and_downloaded_again() {
    let s = Server::start();
    let dir = Scratch::new("stale");
    let f = file(PAYLOAD);
    s.serve(&path_of(&f), Reply::Bytes(PAYLOAD.to_vec()));
    let src = source(&s, &dir);
    let p = cache_dir(src.cache_root.as_ref().unwrap(), &f).join(&f.name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, b"truncated").unwrap();
    assert_eq!(cached(&src, &f), None);
    assert!(!p.exists(), "the bad cached file is gone");
    let got = obtain(&src, &f, &|_, _| {}).unwrap();
    assert_eq!(got.how, "download");
    assert_eq!(fs::read(&got.path).unwrap(), PAYLOAD);
}

#[test]
fn size_caps_hold() {
    let s = Server::start();
    let dir = Scratch::new("cap");
    let mut f = file(PAYLOAD);
    f.cap = 8;
    s.serve(&path_of(&f), Reply::Bytes(PAYLOAD.to_vec()));
    let src = source(&s, &dir);
    let e = download(&src, &f, &|_, _| {}).unwrap_err();
    assert!(matches!(e, FetchError::Refused(ref m) if m.contains("larger than K2 accepts")), "{e:?}");
    assert!(matches!(probe(&src, &f), Err(FetchError::Refused(_))), "the probe refuses it too");
    assert!(leftovers(&src, &f).is_empty());
}

#[test]
fn a_short_body_is_an_error_with_nothing_kept() {
    let s = Server::start();
    let dir = Scratch::new("short");
    let f = file(PAYLOAD);
    s.serve(&path_of(&f), Reply::Lying(PAYLOAD.len() as u64 + 100, PAYLOAD.to_vec()));
    let src = source(&s, &dir);
    let e = download(&src, &f, &|_, _| {}).unwrap_err();
    assert!(e.unreachable(), "{e:?}");
    assert!(leftovers(&src, &f).is_empty(), "{:?}", leftovers(&src, &f));
}

#[test]
fn redirects_stay_on_the_allowed_hosts() {
    let s = Server::start();
    let dir = Scratch::new("redirect");
    let f = file(PAYLOAD);
    // `localhost` is not on the loopback origin's host list (127.0.0.1).
    s.serve(&path_of(&f), Reply::Redirect(format!("http://localhost:{}/elsewhere", s.port)));
    s.serve("/elsewhere", Reply::Bytes(PAYLOAD.to_vec()));
    let src = source(&s, &dir);
    let e = download(&src, &f, &|_, _| {}).unwrap_err();
    assert!(matches!(e, FetchError::Refused(ref m) if m.contains("localhost")), "{e:?}");
    assert!(!s.seen.lock().unwrap().iter().any(|l| l.contains("/elsewhere")), "never followed");
    // An allowed redirect is followed.
    s.serve(&path_of(&f), Reply::Redirect(format!("http://127.0.0.1:{}/elsewhere", s.port)));
    assert_eq!(fs::read(download(&src, &f, &|_, _| {}).unwrap()).unwrap(), PAYLOAD);
}

#[test]
fn no_token_or_cookie_is_sent() {
    let s = Server::start();
    let dir = Scratch::new("creds");
    let f = file(PAYLOAD);
    s.serve(&path_of(&f), Reply::Bytes(PAYLOAD.to_vec()));
    let src = source(&s, &dir);
    probe(&src, &f).unwrap();
    download(&src, &f, &|_, _| {}).unwrap();
    let seen = s.seen.lock().unwrap().clone();
    assert_eq!(seen, vec![format!("HEAD {}", path_of(&f)), format!("GET {}", path_of(&f))]);
}

#[test]
fn probe_reports_size_and_404s_name_the_public_url() {
    let s = Server::start();
    let dir = Scratch::new("probe");
    let f = file(PAYLOAD);
    let src = source(&s, &dir);
    let e = probe(&src, &f).unwrap_err();
    assert!(matches!(e, FetchError::Unreachable(ref m) if m.contains("HTTP 404") && m.contains(&f.url())), "{e:?}");
    s.serve(&path_of(&f), Reply::Bytes(PAYLOAD.to_vec()));
    assert_eq!(probe(&src, &f).unwrap(), Some(PAYLOAD.len() as u64));
    assert!(leftovers(&src, &f).is_empty(), "a probe writes nothing");
}

#[test]
fn offline_reaches_nothing() {
    let f = file(PAYLOAD);
    assert!(probe(&Source::offline(), &f).unwrap_err().unreachable());
    assert!(download(&Source::offline(), &f, &|_, _| {}).unwrap_err().unreachable());
    assert_eq!(cached(&Source::offline(), &f), None);
}

#[test]
fn the_github_origin_is_https_and_github_only() {
    let o = Origin::github();
    let ok = |u: &str| o.allows(&reqwest::Url::parse(u).unwrap());
    assert!(ok("https://github.com/Alakazam-211/sew-releases/releases/download/v0.5.0/x.tar.gz").is_ok());
    assert!(ok("https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sig=x").is_ok());
    assert!(ok("https://objects.githubusercontent.com/a").is_ok());
    assert!(ok("http://github.com/a").unwrap_err().contains("not https"));
    assert!(ok("https://evil.example/a").unwrap_err().contains("evil.example"));
    assert!(ok("https://github.com.evil.example/a").is_err());
    assert!(ok("https://user:pw@github.com/a").unwrap_err().contains("credentials"));
    assert!(ok("ftp://github.com/a").is_err());
}

#[cfg(unix)]
#[test]
fn a_loose_cache_dir_is_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("perm");
    let d = dir.0.join("cache").join("sew");
    fs::create_dir_all(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    private_dir(&d).unwrap();
    assert_eq!(fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
    // A symlink in the way is refused, not followed.
    let link = dir.0.join("cache").join("link");
    std::os::unix::fs::symlink(&d, &link).unwrap();
    assert!(matches!(private_dir(&link), Err(FetchError::Cache(ref m)) if m.contains("not a directory")));
}

/// The real pin (0.5.0, the release table, not the test fixtures): the URL
/// a box downloads and the words an offline box gets.
#[test]
fn release_files_name_the_published_assets() {
    let rel = k2_root_proto::sew::SEW_RELEASES.last().unwrap();
    assert_eq!(rel.version, "0.5.0");
    let a = rel.artifacts.iter().find(|a| a.arch == "x86_64").unwrap();
    let core = ReleaseFile::core(rel.version, a);
    assert_eq!(
        core.url(),
        "https://github.com/Alakazam-211/sew-releases/releases/download/v0.5.0/sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(core.sha256, "5dd2c9d2437045315e3b95709bfe34bbaebcee22a6389ac8046431c30a533461");
    let m = rel.media.as_ref().unwrap();
    let media = ReleaseFile::media(rel.version, m.version, m.artifacts.iter().find(|a| a.arch == "x86_64").unwrap());
    assert_eq!(
        media.url(),
        "https://github.com/Alakazam-211/sew-releases/releases/download/v0.5.0/sew-media-0.5.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    let kit = ReleaseFile::app_kit(rel.version, &k2_root_proto::sew::SEW_APP_KITS[0]);
    assert_eq!(kit.url(), "https://github.com/Alakazam-211/sew-releases/releases/download/v0.5.0/sew-app-kit-0.5.0.tar.gz");
    let words = core.by_hand("k2 hostchat enable --tarball");
    assert_eq!(
        words,
        "On a box without internet: download \
         https://github.com/Alakazam-211/sew-releases/releases/download/v0.5.0/sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz \
         (sha256 5dd2c9d2437045315e3b95709bfe34bbaebcee22a6389ac8046431c30a533461) on another machine, copy it here, \
         then run k2 hostchat enable --tarball /path/to/sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(size_words(24_667_386), "24.7 MB");
}
