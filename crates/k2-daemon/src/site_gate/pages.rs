//! K2's own answers on a gated site (`/_k2/site/…` and the refusals). They
//! load nothing from the site or anywhere else, and end the connection, so
//! the next request starts over at the gate.

/// One K2 answer. `dev` marks a dev site's answers (DM-4); `head` drops the
/// body of a `HEAD`.
pub struct Page<'a> {
    pub status: &'a str,
    pub dev: bool,
    pub head: bool,
    /// Complete `\r\n` header lines of this answer (cookies, `Location`, …).
    pub extra: String,
    pub html: String,
}

impl Page<'_> {
    pub fn bytes(&self) -> Vec<u8> {
        let dev = if self.dev { "X-K2-Mode: dev\r\n" } else { "" };
        let mut out = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\n\
             X-Robots-Tag: noindex\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
             Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'\r\n\
             {dev}{extra}Content-Length: {len}\r\n{frame}Connection: close\r\n\r\n",
            status = self.status,
            extra = self.extra,
            len = self.html.len(),
            frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
        )
        .into_bytes();
        if !self.head {
            out.extend_from_slice(self.html.as_bytes());
        }
        out
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#111;color:#eee;padding:16px;box-sizing:border-box}main{max-width:26rem}h1{font-size:1.25rem;margin:0 0 .5rem}p{margin:.5rem 0}code{background:#222;padding:.1rem .3rem;border-radius:3px}input{font:inherit;padding:.5rem;width:100%;box-sizing:border-box;margin:.5rem 0;background:#222;color:#eee;border:1px solid #555;border-radius:4px}button,a.b{font:inherit;padding:.5rem 1rem;background:#eee;color:#111;border:0;border-radius:4px;text-decoration:none;display:inline-block}.e{color:#f99}";

fn doc(title: &str, dev: bool, body: &str) -> String {
    let title = if dev { format!("Dev server: {title}") } else { title.to_string() };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>\n",
        esc(&title)
    )
}

/// PV-6: the "private" page. Its bytes are pinned by a test.
pub fn private_page(label: &str, dev: bool, head: bool) -> Vec<u8> {
    let l = esc(label);
    let body = format!(
        "<h1>This site is private.</h1><p>Open it from K2: Published → Open, or <code>k2 publish open {l}</code>.</p>"
    );
    Page { status: "401 Unauthorized", dev, head, extra: String::new(), html: doc("Private site", dev, &body) }.bytes()
}

/// A one-time link that was used, expired, or is another site's.
pub fn link_dead_page(label: &str, dev: bool, head: bool) -> Vec<u8> {
    let l = esc(label);
    let body = format!(
        "<h1>This link has expired or was already used.</h1><p>A link works once, for a minute. Get a new one from K2: Published → Open, or <code>k2 publish open {l}</code>. If someone shared it with you, ask them.</p>"
    );
    Page { status: "401 Unauthorized", dev, head, extra: String::new(), html: doc("Private site", dev, &body) }.bytes()
}

/// PV-8: the PIN page. `next` is where a right PIN goes (already checked).
pub fn pin_page(next: &str, error: Option<&str>, dev: bool, head: bool) -> Vec<u8> {
    let err = error.map(|e| format!("<p class=\"e\">{}</p>", esc(e))).unwrap_or_default();
    let body = format!(
        "<h1>This site needs a PIN.</h1>{err}<form method=\"post\" action=\"/_k2/site/pin\"><input type=\"hidden\" name=\"next\" value=\"{}\"><input name=\"pin\" type=\"password\" autocomplete=\"off\" autofocus aria-label=\"PIN\" required><button type=\"submit\">Continue</button></form>",
        esc(next)
    );
    Page { status: "401 Unauthorized", dev, head, extra: String::new(), html: doc("PIN", dev, &body) }.bytes()
}

/// PV-8: too many wrong tries.
pub fn pin_locked_page(retry_after: i64, dev: bool) -> Vec<u8> {
    let mins = (retry_after + 59) / 60;
    let body = format!("<h1>Too many wrong PINs.</h1><p>Try again in about {mins} minute{}.</p>", if mins == 1 { "" } else { "s" });
    Page {
        status: "429 Too Many Requests",
        dev,
        head: false,
        extra: format!("Retry-After: {retry_after}\r\n"),
        html: doc("PIN", dev, &body),
    }
    .bytes()
}

/// DM-4: the one-screen notice, once per PIN session of a dev site.
pub fn dev_notice_page(label: &str, next: &str, head: bool) -> Vec<u8> {
    let body = format!(
        "<h1>This is a dev server for {}.</h1><p>It may show code and errors.</p><p><a class=\"b\" href=\"/_k2/site/continue?next={}\">Continue</a></p>",
        esc(label),
        esc(&urlencode(next))
    );
    Page { status: "200 OK", dev: true, head, extra: String::new(), html: doc("Notice", true, &body) }.bytes()
}

/// DM-6: the dev server is on its way. The page reloads itself.
pub fn starting_page(head: bool) -> Vec<u8> {
    let body = "<h1>Starting the dev server…</h1><p>This page reloads when it's ready.</p>";
    Page {
        status: "503 Service Unavailable",
        dev: true,
        head,
        extra: "Retry-After: 2\r\nRefresh: 2\r\n".to_string(),
        html: doc("Starting", true, body),
    }
    .bytes()
}

/// A dev server that couldn't start. Why stays in the log and `k2 publish ps`.
pub fn dev_failed_page(head: bool) -> Vec<u8> {
    let body = "<h1>The dev server didn't start.</h1><p>Ask the site's owner to check <code>k2 publish logs</code> on the server.</p>";
    Page { status: "502 Bad Gateway", dev: true, head, extra: String::new(), html: doc("Starting", true, body) }.bytes()
}

/// A redirect that sets (or doesn't set) K2's session cookie.
pub fn redirect(location: &str, cookie: Option<(&str, &str, i64)>, dev: bool) -> Vec<u8> {
    let mut extra = format!("Location: {location}\r\n");
    if let Some((name, value, max_age)) = cookie {
        extra.push_str(&format!("Set-Cookie: {name}={value}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={max_age}\r\n"));
    }
    Page { status: "303 See Other", dev, head: false, extra, html: String::new() }.bytes()
}

/// Send a `POST` to K2's page again on a new connection (it arrived on one
/// that already passed the gate, with a body K2 doesn't read there).
pub fn reconnect(path: &str, dev: bool) -> Vec<u8> {
    Page { status: "307 Temporary Redirect", dev, head: false, extra: format!("Location: {path}\r\n"), html: String::new() }.bytes()
}

/// K2's refusal of a request (a plain line, no form).
pub fn plain(status: &str, text: &str, dev: bool, head: bool) -> Vec<u8> {
    Page { status, dev, head, extra: String::new(), html: doc("K2", dev, &format!("<p>{}</p>", esc(text))) }.bytes()
}

/// Percent-encode for a query value (only what is safe stays).
pub fn urlencode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}
