//! TEST ONLY: a stand-in download host that follows the contract
//! (`dl-k2-dev-upload-api-contract-v1`, k2dl 1.3.0) on a loopback port.
//! k2-core's and the daemon's tests point `K2_DL_BASE` (or a client) at it.
//! It never touches the network beyond 127.0.0.1.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value as J};
use sha2::Digest as _;

#[derive(Default)]
pub struct State {
    /// id → its App token.
    pub tokens: HashMap<String, String>,
    /// (id, version, filename) → (sha256, content type, bytes)
    pub files: HashMap<(String, String, String), (String, String, Vec<u8>)>,
    /// (id, channel) → the stored manifest
    pub manifests: HashMap<(String, String), J>,
    /// Statuses to answer the next requests with (then normal service).
    pub fail_next: VecDeque<u16>,
    /// Per-file cap (bytes).
    pub max_file: u64,
    /// id → the stats answer to give.
    pub stats: HashMap<String, J>,
    /// Every request as `METHOD path` (no headers, no query secrets).
    pub log: Vec<String>,
    /// Upload bodies received (count), new or not.
    pub uploads: u32,
    /// Lie about the stored sha256 in upload answers (the client must catch it).
    pub lie_sha: bool,
    /// Answer this many upload PUTs with 503 (other requests are served).
    pub fail_uploads: u32,
}

pub struct FakeHost {
    pub base: String,
    pub state: Arc<Mutex<State>>,
}

impl FakeHost {
    /// Start one on a free loopback port; `tokens` = `(id, k2dl_ token)`.
    pub fn start(tokens: &[(&str, &str)]) -> FakeHost {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind the fake download host");
        let base = format!("http://{}", l.local_addr().expect("addr"));
        let state = Arc::new(Mutex::new(State {
            tokens: tokens.iter().map(|(a, t)| (a.to_string(), t.to_string())).collect(),
            max_file: 200 * 1024 * 1024,
            ..Default::default()
        }));
        let (st, b) = (state.clone(), base.clone());
        std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(conn) = conn else { continue };
                let (st, b) = (st.clone(), b.clone());
                std::thread::spawn(move || serve(conn, &st, &b));
            }
        });
        FakeHost { base, state }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn respond(s: &mut TcpStream, status: u16, extra: &str, ct: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}

fn err(s: &mut TcpStream, status: u16, code: &str) {
    respond(s, status, "", "application/json", json!({ "error": code, "message": code }).to_string().as_bytes());
}

fn serve(mut s: TcpStream, st: &Arc<Mutex<State>>, base: &str) {
    let mut r = BufReader::new(s.try_clone().expect("clone"));
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 && r.read_exact(&mut body).is_err() {
        return;
    }
    let path = target.split('?').next().unwrap_or("").to_string();
    let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
    g.log.push(format!("{method} {path}"));
    if let Some(code) = g.fail_next.pop_front() {
        drop(g);
        respond(&mut s, code, "Retry-After: 0\r\n", "application/json", br#"{"error":"busy","message":"busy"}"#);
        return;
    }
    let bearer = headers.get("authorization").and_then(|a| a.strip_prefix("Bearer ")).map(str::to_string);
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let auth = |g: &State, id: &str| -> Result<(), (u16, &'static str)> {
        match &bearer {
            None => Err((401, "unauthorized")),
            Some(t) if g.tokens.get(id) == Some(t) => Ok(()),
            Some(t) if g.tokens.values().any(|x| x == t) => Err((403, "wrong_app")),
            Some(_) => Err((401, "unauthorized")),
        }
    };
    match (method.as_str(), segs.as_slice()) {
        ("GET", ["healthz"]) => {
            drop(g);
            respond(&mut s, 200, "", "application/json", br#"{"ok":true,"version":"fake"}"#);
        }
        ("PUT", ["api", "v1", "apps", id, "versions", version, "files", file]) => {
            if let Err((c, e)) = auth(&g, id) {
                drop(g);
                return err(&mut s, c, e);
            }
            g.uploads += 1;
            if g.fail_uploads > 0 {
                g.fail_uploads -= 1;
                drop(g);
                return respond(&mut s, 503, "Retry-After: 0\r\n", "application/json", br#"{"error":"busy","message":"busy"}"#);
            }
            if semver::Version::parse(version).is_err() {
                drop(g);
                return err(&mut s, 400, "bad_name");
            }
            if body.len() as u64 > g.max_file {
                drop(g);
                return err(&mut s, 413, "too_large");
            }
            let sha: String = sha2::Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
            if headers.get("x-content-sha256") != Some(&sha) {
                drop(g);
                return err(&mut s, 422, "sha_mismatch");
            }
            let key = (id.to_string(), version.to_string(), file.to_string());
            let status = match g.files.get(&key) {
                Some((old, _, _)) if *old != sha => {
                    drop(g);
                    return err(&mut s, 409, "immutable");
                }
                Some(_) => 200,
                None => 201,
            };
            let ct = headers.get("content-type").cloned().unwrap_or_default();
            let size = body.len();
            g.files.insert(key, (sha.clone(), ct, body));
            let shown = if g.lie_sha { "0".repeat(64) } else { sha.clone() };
            drop(g);
            let url = format!("{base}/{id}/{version}/{sha}/{file}");
            respond(&mut s, status, "", "application/json", json!({ "url": url, "sha256": shown, "size": size }).to_string().as_bytes());
        }
        ("PUT", ["api", "v1", "apps", id, "channels", channel, "latest.json"]) => {
            if let Err((c, e)) = auth(&g, id) {
                drop(g);
                return err(&mut s, c, e);
            }
            let Ok(m) = serde_json::from_slice::<J>(&body) else {
                drop(g);
                return err(&mut s, 422, "bad_manifest");
            };
            let prefix = format!("{base}/{id}/");
            let ok = m.get("platforms").and_then(J::as_object).is_some_and(|p| {
                !p.is_empty()
                    && p.values().all(|v| {
                        let url = v.get("url").and_then(J::as_str).unwrap_or("");
                        let sig_ok = v.get("signature").and_then(J::as_str).is_some_and(|x| !x.is_empty());
                        url.strip_prefix(&prefix).is_some_and(|rest| {
                            let r: Vec<&str> = rest.split('/').collect();
                            r.len() == 3
                                && Some(r[0]) == m.get("version").and_then(J::as_str)
                                && g.files.get(&(id.to_string(), r[0].to_string(), r[2].to_string())).is_some_and(|f| f.0 == r[1])
                        }) && sig_ok
                    })
            });
            if !ok {
                drop(g);
                return err(&mut s, 422, "bad_manifest");
            }
            g.manifests.insert((id.to_string(), channel.to_string()), m);
            drop(g);
            respond(&mut s, 200, "", "application/json", json!({ "url": format!("{base}/{id}/{channel}/latest.json") }).to_string().as_bytes());
        }
        ("GET", ["api", "v1", "apps", id, "stats"]) => {
            if let Err((c, e)) = auth(&g, id) {
                drop(g);
                return err(&mut s, c, e);
            }
            let v = g.stats.get(*id).cloned().unwrap_or_else(|| json!({ "app": id, "versions": [], "manifest_checks": [] }));
            drop(g);
            respond(&mut s, 200, "", "application/json", v.to_string().as_bytes());
        }
        ("GET" | "HEAD", [id, version, sha, file]) => {
            let found = g.files.get(&(id.to_string(), version.to_string(), file.to_string())).filter(|f| f.0 == *sha).cloned();
            drop(g);
            match found {
                Some((_, ct, bytes)) => {
                    let b: &[u8] = if method == "HEAD" { &[] } else { &bytes };
                    respond(&mut s, 200, "Cache-Control: public, max-age=31536000, immutable\r\n", &ct, b)
                }
                None => err(&mut s, 404, "not_found"),
            }
        }
        _ => {
            drop(g);
            err(&mut s, 404, "not_found")
        }
    }
}
