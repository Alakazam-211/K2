//! TC-2: the static file server's jail, MIME, ETag/304, cache rule,
//! pre-compressed pick, SPA fallback, 404 file, HEAD and 405.

use super::*;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A temp site folder, removed on drop.
struct SiteDir(PathBuf);

impl SiteDir {
    fn new() -> Self {
        let p = k2_core::test_env::unique_temp_path("static-files");
        std::fs::create_dir_all(p.join("site/assets")).unwrap();
        std::fs::create_dir_all(p.join("site/docs")).unwrap();
        std::fs::create_dir_all(p.join("site/.well-known")).unwrap();
        std::fs::create_dir_all(p.join("outside")).unwrap();
        let w = |rel: &str, body: &[u8]| std::fs::write(p.join("site").join(rel), body).unwrap();
        w("index.html", b"<!doctype html><div id=app></div>");
        w("404.html", b"<p>gone</p>");
        w("assets/index-BX4f1a2c.js", b"console.log(1)");
        w("assets/index-BX4f1a2c.js.br", b"BR");
        w("assets/index-BX4f1a2c.js.gz", b"GZ");
        w("app.wasm", b"\0asm");
        w("docs/index.html", b"<h1>docs</h1>");
        w(".env", b"SECRET=1");
        w(".well-known/assetlinks.json", b"[]");
        std::fs::write(p.join("outside/secret.txt"), b"nope").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(p.join("outside"), p.join("site/out")).unwrap();
        SiteDir(p)
    }

    fn served(&self, spa: bool, not_found: Option<&str>) -> Served {
        let site = Site {
            root: ".".into(),
            spa,
            not_found: not_found.map(str::to_string),
            headers: vec![("Referrer-Policy".into(), "no-referrer".into())],
            immutable: vec![],
        };
        Served::new(&self.0.join("site"), site, "abcdef0123456789zz", "X-Frame-Options: SAMEORIGIN\r\n").unwrap()
    }
}

impl Drop for SiteDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Send `raw` requests on one connection and read until it closes.
async fn exchange(s: &Served, raw: &str) -> String {
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    let served = s.clone();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        serve_conn(&mut server, &served, rx).await;
    });
    client.write_all(raw.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut out)).await.expect("the server closed the connection").unwrap();
    task.await.unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

async fn get(s: &Served, path: &str, extra: &str) -> String {
    exchange(s, &format!("GET {path} HTTP/1.1\r\nHost: docs.box.k2.dev\r\n{extra}Connection: close\r\n\r\n")).await
}

fn status(resp: &str) -> u16 {
    resp.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("no status line: {resp}"))
}

fn header<'a>(resp: &'a str, name: &str) -> Option<&'a str> {
    let head = resp.split("\r\n\r\n").next().unwrap();
    head.lines().skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()))
}

fn body(resp: &str) -> &str {
    resp.split_once("\r\n\r\n").map(|(_, b)| b).unwrap()
}

#[tokio::test]
async fn the_jail_holds() {
    let d = SiteDir::new();
    let s = d.served(false, None);
    for p in ["/../outside/secret.txt", "/%2e%2e/outside/secret.txt", "/a%2f..%2f..%2foutside/secret.txt", "/index.html%00.js", "/.env", "/assets/../../outside/secret.txt"] {
        let r = get(&s, p, "").await;
        assert_eq!(status(&r), 404, "{p}: {r}");
        assert!(!r.contains("nope") && !r.contains("SECRET"), "{p} leaked: {r}");
    }
    #[cfg(unix)]
    {
        let r = get(&s, "/out/secret.txt", "").await;
        assert_eq!(status(&r), 404, "a symlink out of the folder is never followed: {r}");
    }
    let r = get(&s, "/.well-known/assetlinks.json", "").await;
    assert_eq!(status(&r), 200, ".well-known is served: {r}");
}

#[tokio::test]
async fn files_have_their_types_and_every_answer_its_headers() {
    let d = SiteDir::new();
    let s = d.served(false, None);
    let r = get(&s, "/", "").await;
    assert_eq!(status(&r), 200);
    assert_eq!(header(&r, "content-type"), Some("text/html; charset=utf-8"));
    assert_eq!(body(&r), "<!doctype html><div id=app></div>");
    assert_eq!(header(&r, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&r, "x-frame-options"), Some("SAMEORIGIN"), "the caller's frame lines");
    assert_eq!(header(&r, "referrer-policy"), Some("no-referrer"), "the App's allowed headers");
    assert_eq!(header(&r, "cache-control"), Some("no-cache"), "HTML is revalidated");
    let r = get(&s, "/app.wasm", "").await;
    assert_eq!(header(&r, "content-type"), Some("application/wasm"));
    let r = get(&s, "/nope.js", "").await;
    assert_eq!(status(&r), 404);
    assert_eq!(header(&r, "x-content-type-options"), Some("nosniff"), "a 404 carries it too");
    assert_eq!(header(&r, "x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(content_type(Some("mjs")), "text/javascript; charset=utf-8");
    assert_eq!(content_type(Some("bin")), "application/octet-stream");
}

#[tokio::test]
async fn etags_give_304_and_hashed_assets_are_immutable() {
    let d = SiteDir::new();
    let s = d.served(false, None);
    let r = get(&s, "/assets/index-BX4f1a2c.js", "").await;
    assert_eq!(status(&r), 200);
    assert_eq!(header(&r, "cache-control"), Some(static_site::CACHE_IMMUTABLE));
    let tag = header(&r, "etag").expect("an ETag").to_string();
    assert!(tag.starts_with("W/\"abcdef0123456789-"), "weak, from the Seed hash: {tag}");
    let r2 = get(&s, "/assets/index-BX4f1a2c.js", &format!("If-None-Match: {tag}\r\n")).await;
    assert_eq!(status(&r2), 304, "{r2}");
    assert_eq!(body(&r2), "", "a 304 has no body");
    let r3 = get(&s, "/assets/index-BX4f1a2c.js", "If-None-Match: \"other\"\r\n").await;
    assert_eq!(status(&r3), 200);
    assert_ne!(etag("abc", "a.js", None), etag("abc", "b.js", None));
    assert_ne!(etag("abc", "a.js", None), etag("abc", "a.js", Some("br")), "each coding is its own entity");
    assert!(not_modified(Some("W/\"x\", *"), "W/\"y\""));
    assert!(!not_modified(None, "W/\"y\""));
}

#[tokio::test]
async fn pre_compressed_siblings_follow_accept_encoding() {
    let d = SiteDir::new();
    let s = d.served(false, None);
    let r = get(&s, "/assets/index-BX4f1a2c.js", "Accept-Encoding: gzip, br\r\n").await;
    assert_eq!(header(&r, "content-encoding"), Some("br"), "br first");
    assert_eq!(body(&r), "BR");
    assert_eq!(header(&r, "content-type"), Some("text/javascript; charset=utf-8"), "the type is the file's own");
    assert_eq!(header(&r, "vary"), Some("Accept-Encoding"));
    let r = get(&s, "/assets/index-BX4f1a2c.js", "Accept-Encoding: gzip, br;q=0\r\n").await;
    assert_eq!(header(&r, "content-encoding"), Some("gzip"), "q=0 refuses br");
    assert_eq!(body(&r), "GZ");
    let r = get(&s, "/assets/index-BX4f1a2c.js", "").await;
    assert_eq!(header(&r, "content-encoding"), None);
    assert_eq!(body(&r), "console.log(1)");
    let r = get(&s, "/", "Accept-Encoding: br\r\n").await;
    assert_eq!(header(&r, "content-encoding"), None, "no sibling, no coding");
}

#[tokio::test]
async fn spa_fallback_and_the_404_file() {
    let d = SiteDir::new();
    let spa = d.served(true, Some("404.html"));
    let r = get(&spa, "/settings/profile", "").await;
    assert_eq!(status(&r), 200, "an extension-less unknown path answers index.html");
    assert_eq!(body(&r), "<!doctype html><div id=app></div>");
    let r = get(&spa, "/missing.png", "").await;
    assert_eq!(status(&r), 404, "a missing file with an extension is a 404");
    assert_eq!(body(&r), "<p>gone</p>", "the 404 file");
    assert_eq!(header(&r, "content-type"), Some("text/html; charset=utf-8"));
    let plain = d.served(false, None);
    let r = get(&plain, "/settings/profile", "").await;
    assert_eq!(status(&r), 404);
    assert_eq!(body(&r), "Not found.\r\n", "K2's plain 404");
}

#[tokio::test]
async fn folders_redirect_to_their_slash() {
    let d = SiteDir::new();
    let s = d.served(false, None);
    let r = get(&s, "/docs?x=1", "").await;
    assert_eq!(status(&r), 301);
    assert_eq!(header(&r, "location"), Some("/docs/?x=1"));
    let r = get(&s, "/docs/", "").await;
    assert_eq!(body(&r), "<h1>docs</h1>");
    let r = get(&s, "//docs", "").await;
    assert_eq!(header(&r, "location"), Some("/docs/"), "never a protocol-relative redirect");
}

#[tokio::test]
async fn head_405_and_keep_alive() {
    let d = SiteDir::new();
    let s = d.served(false, None);
    let r = exchange(&s, "HEAD /index.html HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await;
    assert_eq!(status(&r), 200);
    assert_eq!(header(&r, "content-length"), Some("33"));
    assert_eq!(body(&r), "", "HEAD has no body");
    let r = exchange(&s, "POST /index.html HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\nhi").await;
    assert_eq!(status(&r), 405);
    assert_eq!(header(&r, "allow"), Some("GET, HEAD"));
    assert_eq!(header(&r, "connection"), Some("close"));
    // Two requests on one keep-alive connection, the second closing it.
    let r = exchange(&s, "GET /app.wasm HTTP/1.1\r\nHost: x\r\n\r\nGET /index.html HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await;
    assert_eq!(r.matches("HTTP/1.1 200 OK").count(), 2, "{r}");
    assert!(r.ends_with("<!doctype html><div id=app></div>"), "{r}");
    let r = exchange(&s, "NOT HTTP\r\n\r\n").await;
    assert_eq!(status(&r), 400);
    assert_eq!(header(&r, "x-frame-options"), Some("SAMEORIGIN"), "a 400 carries the frame lines");
    let r = exchange(&s, &format!("GET / HTTP/1.1\r\nX-Big: {}\r\n", "a".repeat(20_000))).await;
    assert_eq!(status(&r), 431, "{}", &r[..r.len().min(200)]);
    assert_eq!(header(&r, "x-frame-options"), Some("SAMEORIGIN"), "a 431 carries the frame lines");
}

#[test]
fn heads_parse() {
    let r = parse_head(b"GET /a/b?x=1 HTTP/1.0\r\nIf-None-Match: W/\"t\"\r\nAccept-Encoding: br\r\n\r\n").unwrap();
    assert_eq!((r.path.as_str(), r.query.as_str()), ("/a/b", "?x=1"));
    assert!(!r.keep_alive, "HTTP/1.0 closes by default");
    assert_eq!(r.if_none_match.as_deref(), Some("W/\"t\""));
    let r = parse_head(b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
    assert!(r.keep_alive && r.has_body);
    assert!(accepts(Some("gzip;q=0.5, br"), "br") && !accepts(Some("br;q=0"), "br") && !accepts(None, "br"));
}
