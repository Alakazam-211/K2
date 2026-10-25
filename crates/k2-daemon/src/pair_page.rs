//! The phone pairing link's landing page and the iOS app-link file
//! (phone-web-off, Companion asks 2026-10-24).
//!
//! A pairing code is shared as `https://<label>.k2.dev/pair#<CODE>` (the
//! QR in Settings and `k2 users pair-phone`). The code rides in the URL
//! fragment, which a browser never sends to a server, so neither the edge
//! nor this daemon ever sees or logs it.
//!
//! - `GET /.well-known/apple-app-site-association`: the iOS universal-link
//!   file, so the Camera app opens the Companion on its pair screen. JSON,
//!   no auth, no redirect, nothing about this server in it.
//!   Android `/.well-known/assetlinks.json` is not served yet. TODO(phone-web-off):
//!   add it for package `com.alakazamlabs.k2so.companion` once the Play App
//!   Signing SHA-256 fingerprint is known (it comes from the Play Console).
//! - `GET /pair`: a static page for a browser that opened the link anyway.
//!   It reads the code from `location.hash` in the page, shows it to copy,
//!   and offers an "Open in K2" button to the custom scheme
//!   `k2companion://pair?server=<host>#<CODE>` (a universal link tapped
//!   from a page on the same domain stays in Safari, so the button must
//!   use the scheme). Strict CSP with a per-response nonce: no external
//!   loads, no network calls, no referrer.
//!
//! Neither is under `/cli/`, so the web-access wall never applies, and
//! neither reads a credential. Nothing else in the daemon answers under
//! `/.well-known/`: ACME HTTP-01 is served by its own short-lived `:80`
//! listener (`domains::acme::Http01Guard`), not this dispatcher.

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

pub const PAIR_PAGE: &str = "/pair";
pub const AASA: &str = "/.well-known/apple-app-site-association";

/// The Companion's App ID (Team ID + bundle id).
pub const COMPANION_APP_ID: &str = "36B8R93HXV.com.alakazamlabs.k2so.companion";

/// The AASA body: the classic `appID` + `paths` form and the iOS 13+
/// `appIDs` + `components` form, both for `/pair` only.
pub fn aasa_body() -> String {
    serde_json::json!({
        "applinks": {
            "apps": [],
            "details": [{
                "appID": COMPANION_APP_ID,
                "paths": ["/pair"],
                "appIDs": [COMPANION_APP_ID],
                "components": [{ "/": "/pair" }],
            }],
        }
    })
    .to_string()
}

/// `GET /.well-known/apple-app-site-association`.
pub async fn send_aasa(stream: &mut TcpStream) {
    let body = aasa_body();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Cache-Control: public, max-age=300\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

fn nonce() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("getrandom for the pair page nonce");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The page's Content-Security-Policy for `nonce`.
pub fn pair_csp(nonce: &str) -> String {
    format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; img-src 'none'; \
         connect-src 'none'; font-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    )
}

/// The page HTML for `nonce`.
pub fn pair_html(nonce: &str) -> String {
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Pair a phone with K2</title>
<style nonce="{nonce}">
:root {{ color-scheme: light dark; --bg:#f6f6f4; --fg:#1d1d1b; --muted:#6b6b66; --card:#ffffff; --line:#dcdcd6; --accent:#2f5bd3; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#111214; --fg:#ececea; --muted:#9a9a94; --card:#1b1c1f; --line:#2c2d31; --accent:#7c9cff; }} }}
* {{ box-sizing: border-box; }}
body {{ margin:0; min-height:100vh; display:grid; place-items:center; padding:16px; background:var(--bg); color:var(--fg);
  font: 16px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
main {{ width:min(420px, 100%); background:var(--card); border:1px solid var(--line); border-radius:14px; padding:24px; }}
h1 {{ font-size:20px; margin:0 0 8px; }}
p {{ margin:0 0 16px; color:var(--muted); }}
code {{ display:block; font: 600 18px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing:.04em;
  padding:12px; border:1px solid var(--line); border-radius:10px; word-break:break-all; margin:0 0 16px; }}
a.button, button {{ display:block; width:100%; text-align:center; font:600 16px/1 inherit; padding:14px; border-radius:10px;
  border:1px solid var(--accent); text-decoration:none; cursor:pointer; margin:0 0 10px; }}
a.button {{ background:var(--accent); color:#fff; }}
button {{ background:transparent; color:var(--accent); }}
[hidden] {{ display:none !important; }}
#status {{ min-height:1.5em; font-size:14px; }}
</style>
</head>
<body>
<main>
  <h1>Open this in the K2 app</h1>
  <p id="lead">This link pairs your phone with a K2 server. Open it in the K2 Companion app, or copy the code and paste it on the app's Pair screen.</p>
  <code id="code">…</code>
  <a class="button" id="open" href="#" hidden>Open in K2</a>
  <button type="button" id="copy" hidden>Copy code</button>
  <p id="status" role="status"></p>
</main>
<script nonce="{nonce}">
(function () {{
  var raw = (window.location.hash || "").replace(/^#/, "");
  try {{ raw = decodeURIComponent(raw); }} catch (e) {{ raw = ""; }}
  var code = document.getElementById("code");
  var open = document.getElementById("open");
  var copy = document.getElementById("copy");
  var status = document.getElementById("status");
  if (!/^[0-9A-Za-z-]{{20,40}}$/.test(raw)) {{
    code.textContent = "No pairing code in this link.";
    document.getElementById("lead").textContent = "Ask for a new pairing link or QR code.";
    return;
  }}
  code.textContent = raw;
  open.href = "k2companion://pair?server=" + encodeURIComponent(window.location.host) + "#" + encodeURIComponent(raw);
  open.hidden = false;
  copy.hidden = false;
  copy.addEventListener("click", function () {{
    function done() {{ status.textContent = "Copied. Paste it on the K2 app's Pair screen."; }}
    function manual() {{
      var range = document.createRange();
      range.selectNodeContents(code);
      var sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      status.textContent = "Selected. Copy it, then paste it in the K2 app.";
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

/// `GET /pair`.
pub async fn send_pair_page(stream: &mut TcpStream) {
    let n = nonce();
    let body = pair_html(&n);
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Content-Security-Policy: {}\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\r\n{body}",
        body.len(),
        pair_csp(&n)
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_aasa_names_only_the_companion_and_pair() {
        let v: serde_json::Value = serde_json::from_str(&aasa_body()).expect("JSON");
        let d = &v["applinks"]["details"][0];
        assert_eq!(v["applinks"]["apps"], serde_json::json!([]));
        assert_eq!(d["appID"], COMPANION_APP_ID);
        assert_eq!(d["paths"], serde_json::json!(["/pair"]));
        assert_eq!(d["appIDs"], serde_json::json!([COMPANION_APP_ID]));
        assert_eq!(d["components"], serde_json::json!([{ "/": "/pair" }]));
    }

    #[test]
    fn the_page_loads_nothing_and_uses_its_nonce() {
        let html = pair_html("abc123");
        assert!(html.contains("<script nonce=\"abc123\">"));
        assert!(html.contains("<style nonce=\"abc123\">"));
        assert!(html.contains("k2companion://pair?server="));
        assert!(html.contains("location.hash"));
        for banned in ["src=\"http", "href=\"http", "fetch(", "XMLHttpRequest", "sendBeacon", "<img", "<link"] {
            assert!(!html.contains(banned), "{banned}");
        }
        let csp = pair_csp("abc123");
        assert!(csp.starts_with("default-src 'none';"));
        assert!(csp.contains("connect-src 'none'"));
        assert!(csp.contains("frame-ancestors 'none'"));
    }
}
