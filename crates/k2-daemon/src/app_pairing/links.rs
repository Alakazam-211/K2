//! The link files on a pairing App's hosts (`prd-app-phone-pairing-v1`
//! §6, vs-live V2–V10; S4).
//!
//! While an App's pairing is on, the daemon itself answers three paths on
//! the App's pairing hosts (its own k2.dev label, its taken and attached
//! labels; R1), before the App sees the request:
//! - `GET|HEAD /.well-known/apple-app-site-association`: the iOS file
//!   naming the owner's iOS app ids for `/.well-known/k2-pair` plus the
//!   App's own `linkPaths` (only when the owner named an iOS app);
//! - `GET|HEAD /.well-known/assetlinks.json`: one statement per Android
//!   app (only when the owner named one);
//! - `GET|HEAD /.well-known/k2-pair`: the landing page for a browser that
//!   opened the QR's link anyway (the code stays in the fragment; the page
//!   makes no network call).
//!
//! Any query is ignored. Every other path, every other host, a request
//! with a body and every App whose pairing is off reach the App untouched
//! (R2: the App's own files pass through while pairing is off).
//!
//! **Where.** `client_ip::forward` answers them at any position on a
//! keep-alive connection (`Expect::Own`, then `Connection: close`, V4), and
//! the App's front port and the E2E App arm answer a connection's first
//! request before dialing the App, so the files work while the App is down
//! (V5). The App is chosen by K2's routing (the backend / front listener),
//! never by the request; the `Host` only picks whether this is one of the
//! App's pairing hosts (V2, V3). The answer is read from an in-memory
//! snapshot ([`set`]) swapped on `set` / `off` / boot / deploy: no skin.db
//! read per request.
//!
//! **Not covered (V6, V7):** raw Apps (`host.clientIp: false`, `--cmd`
//! without the label opt-in) and `--skin` services (the browser helper is
//! another process): pairing setup refuses them (`pairing_needs_k2_front`).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use k2_core::skin::app_pairing::{AndroidApp, PAIR_PATH};
use serde_json::json;

pub const AASA: &str = "/.well-known/apple-app-site-association";
pub const ASSETLINKS: &str = "/.well-known/assetlinks.json";

/// What one pairing App serves (built from its config and its hosts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSet {
    pub service_id: String,
    /// The App's name, for the landing page.
    pub app_name: String,
    pub ios: Vec<String>,
    pub android: Vec<AndroidApp>,
    pub link_paths: Vec<String>,
    /// The hosts it answers on (lowercase, no port).
    pub hosts: Vec<String>,
    pub store_ios: Option<String>,
    pub store_android: Option<String>,
}

type Snapshot = HashMap<String, Arc<LinkSet>>;

fn snapshot() -> &'static ArcSwap<Snapshot> {
    static S: OnceLock<ArcSwap<Snapshot>> = OnceLock::new();
    S.get_or_init(|| ArcSwap::from_pointee(HashMap::new()))
}

fn write_lock() -> &'static std::sync::Mutex<()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &L
}

/// Set (or clear) the runtime App `app_id`'s link files.
pub fn set(app_id: &str, links: Option<LinkSet>) {
    let _g = write_lock().lock().unwrap_or_else(|e| e.into_inner());
    let cur = snapshot().load_full();
    let mut next: Snapshot = (*cur).clone();
    match links {
        Some(l) => {
            next.insert(app_id.to_string(), Arc::new(l));
        }
        None => {
            next.remove(app_id);
        }
    }
    snapshot().store(Arc::new(next));
}

/// The runtime App `app_id`'s link files, when its pairing is on (one
/// atomic load).
pub fn get(app_id: &str) -> Option<Arc<LinkSet>> {
    snapshot().load().get(app_id).cloned()
}

/// Is `path` (query ignored) one of the three paths? One string compare
/// for the common miss.
pub fn is_link_path(path_and_query: &str) -> bool {
    let p = path_and_query.split('?').next().unwrap_or(path_and_query);
    p.starts_with("/.well-known/") && (p == AASA || p == ASSETLINKS || p == PAIR_PATH)
}

/// `Host` as compared: lowercase, no port, no trailing dot.
pub fn host_name(raw: &str) -> String {
    let h = raw.trim();
    let name = match h.rsplit_once(':') {
        Some((n, p)) if !n.contains(']') && p.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => h,
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// The AASA body: per iOS app id, the classic `appID` + `paths` form and
/// the iOS 13+ `appIDs` + `components` form, for [`PAIR_PATH`] and the
/// App's own `linkPaths`. Nothing about the box, a person or another App.
pub fn aasa_body(l: &LinkSet) -> String {
    let mut paths: Vec<String> = vec![PAIR_PATH.to_string()];
    paths.extend(l.link_paths.iter().cloned());
    let components: Vec<serde_json::Value> = paths.iter().map(|p| json!({ "/": p })).collect();
    let details: Vec<serde_json::Value> =
        l.ios.iter().map(|id| json!({ "appID": id, "paths": paths, "appIDs": [id], "components": components })).collect();
    json!({ "applinks": { "apps": [], "details": details } }).to_string()
}

/// The assetlinks.json body: one statement per Android app.
pub fn assetlinks_body(l: &LinkSet) -> String {
    let statements: Vec<serde_json::Value> = l
        .android
        .iter()
        .map(|a| {
            json!({
                "relation": ["delegate_permission/common.handle_all_urls"],
                "target": { "namespace": "android_app", "package_name": a.package, "sha256_cert_fingerprints": a.sha256 },
            })
        })
        .collect();
    serde_json::Value::Array(statements).to_string()
}

/// HTML-escape for text and attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The landing page's Content-Security-Policy for `nonce` (the Companion
/// pair page's: nothing loads, nothing connects).
pub fn page_csp(nonce: &str) -> String {
    crate::pair_page::pair_csp(nonce)
}

/// The landing page for `l` with `nonce`: the App's name (escaped), the
/// code from `location.hash` to copy, the owner's store links (checked at
/// set), and a warning never to share the code. No "open in" scheme
/// button (Q4), no network call.
pub fn page_html(l: &LinkSet, nonce: &str) -> String {
    let name = escape(&l.app_name);
    let mut stores = String::new();
    if let Some(u) = &l.store_ios {
        stores.push_str(&format!("  <a class=\"button\" href=\"{}\" rel=\"noreferrer\">Get it on the App Store</a>\n", escape(u)));
    }
    if let Some(u) = &l.store_android {
        stores.push_str(&format!("  <a class=\"button\" href=\"{}\" rel=\"noreferrer\">Get it on Google Play</a>\n", escape(u)));
    }
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Open this in the {name} app</title>
<style nonce="{nonce}">
:root {{ color-scheme: light dark; --bg:#f6f6f4; --fg:#1d1d1b; --muted:#6b6b66; --card:#ffffff; --line:#dcdcd6; --accent:#2f5bd3; --warn:#9a4b00; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#111214; --fg:#ececea; --muted:#9a9a94; --card:#1b1c1f; --line:#2c2d31; --accent:#7c9cff; --warn:#f0a35c; }} }}
* {{ box-sizing: border-box; }}
body {{ margin:0; min-height:100vh; display:grid; place-items:center; padding:16px; background:var(--bg); color:var(--fg);
  font: 16px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
main {{ width:min(420px, 100%); background:var(--card); border:1px solid var(--line); border-radius:14px; padding:24px; }}
h1 {{ font-size:20px; margin:0 0 8px; }}
p {{ margin:0 0 16px; color:var(--muted); }}
p.warn {{ color:var(--warn); font-size:14px; }}
code {{ display:block; font: 600 18px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing:.04em;
  padding:12px; border:1px solid var(--line); border-radius:10px; word-break:break-all; margin:0 0 16px; }}
a.button, button {{ display:block; width:100%; text-align:center; font:600 16px/1 inherit; padding:14px; border-radius:10px;
  border:1px solid var(--accent); text-decoration:none; cursor:pointer; margin:0 0 10px; background:transparent; color:var(--accent); }}
[hidden] {{ display:none !important; }}
#status {{ min-height:1.5em; font-size:14px; }}
</style>
</head>
<body>
<main>
  <h1>Open this in the {name} app</h1>
  <p id="lead">This link signs your phone in to {name}. Open it with the {name} app on your phone, or copy the code and paste it in the app.</p>
  <code id="code">…</code>
  <button type="button" id="copy" hidden>Copy code</button>
  <p class="warn">Never share this code or read it to anyone. K2 and {name} will never ask you for it.</p>
{stores}  <p id="status" role="status"></p>
</main>
<script nonce="{nonce}">
(function () {{
  var raw = (window.location.hash || "").replace(/^#/, "");
  try {{ raw = decodeURIComponent(raw); }} catch (e) {{ raw = ""; }}
  var code = document.getElementById("code");
  var copy = document.getElementById("copy");
  var status = document.getElementById("status");
  if (!/^[0-9A-Za-z-]{{20,40}}$/.test(raw)) {{
    code.textContent = "No sign-in code in this link.";
    document.getElementById("lead").textContent = "Start again from the web app to get a new QR code.";
    return;
  }}
  code.textContent = raw;
  copy.hidden = false;
  copy.addEventListener("click", function () {{
    function done() {{ status.textContent = "Copied. Paste it in the app."; }}
    function manual() {{
      var range = document.createRange();
      range.selectNodeContents(code);
      var sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      status.textContent = "Selected. Copy it, then paste it in the app.";
    }}
    if (navigator.clipboard && navigator.clipboard.writeText) {{
      navigator.clipboard.writeText(raw).then(done, manual);
    }} else {{
      manual();
    }}
  }});
}})();
</script>
</body>
</html>
"##
    )
}

fn nonce() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("getrandom for the pairing page nonce");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every pairing link file and the landing page refuse every frame
/// (`X-Frame-Options: DENY`, stricter than the daemon's SAMEORIGIN; the
/// frame-header walk reads it here).
fn response(head_only: bool, ct: &str, extra: &str, body: &str) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    if !head_only {
        out.extend_from_slice(body.as_bytes());
    }
    out
}

/// The whole response K2 gives for `method path_and_query` with `host` on
/// the runtime App `app_id`, or `None` (the App answers). Only `GET` and
/// `HEAD` of the three paths, on one of the App's pairing hosts, while its
/// pairing is on and the box switch allows it.
pub fn answer(app_id: &str, method: &str, path_and_query: &str, host: Option<&str>) -> Option<Vec<u8>> {
    if !is_link_path(path_and_query) {
        return None;
    }
    answer_on(super::box_enabled(), app_id, method, path_and_query, host)
}

/// [`answer`] with the box switch given (`K2_APP_PAIRING=off` = `false`:
/// every App's files pass through).
fn answer_on(box_on: bool, app_id: &str, method: &str, path_and_query: &str, host: Option<&str>) -> Option<Vec<u8>> {
    if !is_link_path(path_and_query) {
        return None;
    }
    let head_only = method.eq_ignore_ascii_case("HEAD");
    if !head_only && !method.eq_ignore_ascii_case("GET") {
        return None;
    }
    if !box_on {
        return None;
    }
    let l = get(app_id)?;
    let host = host_name(host?);
    if !l.hosts.iter().any(|h| *h == host) {
        return None;
    }
    let path = path_and_query.split('?').next().unwrap_or(path_and_query);
    let static_headers = "Cache-Control: public, max-age=300\r\n";
    Some(match path {
        AASA if !l.ios.is_empty() => response(head_only, "application/json", static_headers, &aasa_body(&l)),
        ASSETLINKS if !l.android.is_empty() => response(head_only, "application/json", static_headers, &assetlinks_body(&l)),
        PAIR_PATH => {
            let n = nonce();
            let extra = format!(
                "Content-Security-Policy: {}\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n",
                page_csp(&n)
            );
            response(head_only, "text/html; charset=utf-8", &extra, &page_html(&l, &n))
        }
        _ => return None,
    })
}

/// A [`crate::client_ip::LocalAnswer`] for the runtime App `app_id` when
/// its pairing is on now (`None`: no peek, no local answers).
pub fn local_for(app_id: &str) -> Option<crate::client_ip::LocalAnswer> {
    get(app_id)?;
    let id = app_id.to_string();
    Some(Arc::new(move |method: &str, path: &str, host: Option<&str>| answer(&id, method, path, host)))
}

// ── the owner's network check (§6.4, V10) ────────────────────────────

/// What one public fetch saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Ok { status: u16, body: String },
    Tls,
    Unreachable,
}

/// The public fetch `k2 app pairing check` makes (a trait so tests never
/// touch the internet).
pub trait Fetcher {
    fn get(&self, url: &str) -> Fetched;
}

/// One host's link-file state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCheck {
    pub host: String,
    /// `ok` | `not_served` | `stale` | `tls_error`, per file.
    pub aasa: &'static str,
    pub assetlinks: &'static str,
    /// Apple's CDN copy of the AASA: `ok` | `stale` | `not_served`
    /// (unreachable or not cached yet) | `none` (no iOS app).
    pub apple_cdn: &'static str,
}

fn json_eq(a: &str, b: &str) -> bool {
    match (serde_json::from_str::<serde_json::Value>(a), serde_json::from_str::<serde_json::Value>(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn verdict(f: Fetched, want: &str) -> &'static str {
    match f {
        Fetched::Tls => "tls_error",
        Fetched::Unreachable => "not_served",
        Fetched::Ok { status, body } if status == 200 && json_eq(&body, want) => "ok",
        // Another server answers this host (DNS elsewhere, a proxy), or
        // our own answer isn't what we serve now.
        Fetched::Ok { status: 200, .. } => "stale",
        Fetched::Ok { .. } => "not_served",
    }
}

/// Fetch both files from each host over the public internet (and Apple's
/// CDN copy of the AASA) and compare them with what K2 serves now.
pub fn check_hosts(fetcher: &dyn Fetcher, l: &LinkSet) -> Vec<HostCheck> {
    let aasa = aasa_body(l);
    let assets = assetlinks_body(l);
    l.hosts
        .iter()
        .map(|h| {
            let (a, cdn) = if l.ios.is_empty() {
                ("none", "none")
            } else {
                let a = verdict(fetcher.get(&format!("https://{h}{AASA}")), &aasa);
                let cdn = match verdict(fetcher.get(&format!("https://app-site-association.cdn-apple.com/a/v1/{h}")), &aasa) {
                    "ok" => "ok",
                    "stale" => "stale",
                    _ => "not_served",
                };
                (a, cdn)
            };
            let al = if l.android.is_empty() { "none" } else { verdict(fetcher.get(&format!("https://{h}{ASSETLINKS}")), &assets) };
            HostCheck { host: h.clone(), aasa: a, assetlinks: al, apple_cdn: cdn }
        })
        .collect()
}

/// The real fetcher: HTTPS GET, no redirects, short timeouts.
pub struct NetFetcher;

impl Fetcher for NetFetcher {
    fn get(&self, url: &str) -> Fetched {
        let client = match reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(_) => return Fetched::Unreachable,
        };
        match client.get(url).send() {
            Ok(r) => {
                let status = r.status().as_u16();
                let body = r.text().unwrap_or_default();
                Fetched::Ok { status, body: body.chars().take(256 * 1024).collect() }
            }
            Err(e) if is_tls(&e) => Fetched::Tls,
            Err(_) => Fetched::Unreachable,
        }
    }
}

fn is_tls(e: &reqwest::Error) -> bool {
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(s) = src {
        let t = s.to_string().to_ascii_lowercase();
        if t.contains("certificate") || t.contains("tls") || t.contains("handshake") {
            return true;
        }
        src = s.source();
    }
    false
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;
