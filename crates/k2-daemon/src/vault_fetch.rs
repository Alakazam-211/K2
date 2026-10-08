//! The vault fetch engine (`prd-k2-vault-v1.md` §9.2, §23.2 VS3–VS6):
//! the I/O half of `k2 vault fetch`. Policy (host binding, sealed
//! injection, header rules, redirect rule, error templates) lives in
//! `k2_vault::fetch`; this module makes the call through [`crate::egress`].
//!
//! - Every secret's allowed sites must match the URL's origin before any
//!   lookup or connection.
//! - Each value goes only where its sealed `dst`/`fmt` say. An agent header
//!   with the same name as an injected one is refused.
//! - The host is resolved once, vetted and pinned (egress). Redirects are
//!   followed (≤ 5) only to the same origin, re-vetted each hop; anything
//!   else comes back as the 3xx itself and the secret isn't re-sent.
//! - The response is capped at `max_bytes`, decoded if the server
//!   compressed it anyway, and redacted (all headers and the body) with the
//!   caller's redactor plus one built from the injected values.
//! - Errors are fixed templates naming only the host.
//!
//! Blocking: callers run it in `spawn_blocking`.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::time::{Duration, Instant};

use k2_vault::fetch::{
    check_host, inject_body, inject_edit, redirect_allowed, upstream_error, validate_agent_header, validate_method,
    InjectEdit, DEFAULT_MAX_RESPONSE, MAX_REDIRECTS, MAX_RESPONSE, RESPONSE_HEADER_ALLOWLIST,
};
use k2_vault::hosts::{HostKind, Origin, OriginPattern};
use k2_vault::inject::Dst;
use k2_vault::redact::{Redactor, StreamRedactor};
use k2_vault::{SecretBytes, VaultError};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::egress::{self, Destination, Purpose};

/// Used when the caller passes a zero timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest a single fetch (all hops and the body) may take.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(300);

/// One outgoing request, as the agent shaped it (no secret in it).
#[derive(Debug, Clone)]
pub struct FetchSpec {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// 0 = [`DEFAULT_MAX_RESPONSE`]; more than [`MAX_RESPONSE`] is refused.
    pub max_bytes: usize,
    /// Zero = [`DEFAULT_TIMEOUT`]; capped at [`MAX_TIMEOUT`].
    pub timeout: Duration,
    /// Return every (redacted) response header, not just the allowlist.
    pub include_headers: bool,
}

/// One secret to add to the request, with its sealed policy.
pub struct Injection {
    /// The secret's name; redacted echoes read `[k2v:<name>]`.
    pub name: String,
    pub value: SecretBytes,
    pub dst: Dst,
    pub fmt: String,
    /// The secret's allowed sites, already narrowed by the grant.
    pub allowed: Vec<OriginPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResult {
    pub status: u16,
    /// Lowercase names, upstream order, redacted.
    pub headers: Vec<(String, String)>,
    /// Decoded and redacted; at most `max_bytes`.
    pub body: Vec<u8>,
    pub truncated: bool,
    /// At least one same-origin redirect was followed.
    pub redirected: bool,
}

/// Run one fetch. Every refusal happens before the first connection.
pub fn execute(spec: FetchSpec, injections: Vec<Injection>, redactor: &Redactor) -> Result<FetchResult, VaultError> {
    let mut method = reqwest::Method::from_bytes(validate_method(&spec.method)?.as_bytes())
        .map_err(|_| VaultError::bad_request("bad_method", "method isn't supported"))?;
    let max_bytes = match spec.max_bytes {
        0 => DEFAULT_MAX_RESPONSE,
        n if n > MAX_RESPONSE => {
            return Err(VaultError::bad_request(
                "bad_max_bytes",
                format!("max bytes is at most {MAX_RESPONSE}"),
            ))
        }
        n => n,
    };
    let timeout = if spec.timeout.is_zero() { DEFAULT_TIMEOUT } else { spec.timeout.min(MAX_TIMEOUT) };
    let deadline = Instant::now() + timeout;

    let (mut url, origin) = Origin::parse_url(&spec.url)?;
    if injections.is_empty() {
        return Err(VaultError::bad_request("no_secret", "name the secret to use"));
    }
    for inj in &injections {
        check_host(&origin, &inj.allowed)?;
    }

    // Sealed injection: build each secret's edit. `wire` keeps each value
    // as it goes out (e.g. `Basic <base64>`) for redaction.
    let mut header_edits: Vec<(String, SecretBytes)> = Vec::new();
    let mut query_edits: Vec<(String, SecretBytes)> = Vec::new();
    let mut wire: Vec<(usize, SecretBytes)> = Vec::new();
    let mut body = spec.body.map(SecretBytes::new);
    for (i, inj) in injections.iter().enumerate() {
        match inject_edit(&inj.dst, &inj.fmt, &inj.value)? {
            InjectEdit::Header { name, value } => {
                if header_edits.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                    return Err(VaultError::bad_request("inject_conflict", format!("two secrets go into {name}")));
                }
                wire.push((i, value.duplicate()));
                header_edits.push((name, value));
            }
            InjectEdit::Query { param, value } => {
                if query_edits.iter().any(|(p, _)| *p == param) {
                    return Err(VaultError::bad_request("inject_conflict", format!("two secrets go into ?{param}")));
                }
                if value.expose_str().is_none() {
                    return Err(VaultError::bad_request("value_not_query_safe", "this value isn't text"));
                }
                wire.push((i, value.duplicate()));
                query_edits.push((param, value));
            }
            InjectEdit::Body => {
                let current = body.as_ref().ok_or_else(|| {
                    VaultError::bad_request("body_marker_missing", "this secret goes in the request body; send a body")
                })?;
                body = Some(SecretBytes::new(inject_body(current.expose(), &inj.fmt, &inj.value)?));
            }
        }
    }

    // Agent headers: RFC 9110 names, no routing/framing headers, no line
    // breaks, and none that an injected secret owns.
    let mut agent_headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
    for (n, v) in &spec.headers {
        validate_agent_header(n, v)?;
        if header_edits.iter().any(|(h, _)| h.eq_ignore_ascii_case(n)) {
            return Err(VaultError::bad_request(
                "header_not_allowed",
                format!("{n} is set from the secret; remove it"),
            ));
        }
        let name = HeaderName::from_bytes(n.as_bytes())
            .map_err(|_| VaultError::bad_request("header_not_allowed", format!("bad header name: {n}")))?;
        let value = HeaderValue::from_str(v)
            .map_err(|_| VaultError::bad_request("header_not_allowed", format!("{n} has a control character")))?;
        agent_headers.push((name, value));
    }
    let mut injected_headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
    for (n, v) in &header_edits {
        let name = HeaderName::from_bytes(n.as_bytes())
            .map_err(|_| VaultError::bad_request("bad_inject", format!("bad header name: {n}")))?;
        let mut value = HeaderValue::from_bytes(v.expose())
            .map_err(|_| VaultError::bad_request("value_not_header_safe", "this value has a control character"))?;
        value.set_sensitive(true);
        injected_headers.push((name, value));
    }

    // Redact the injected values themselves (no length floor), raw and as
    // they went on the wire.
    let mut entries: Vec<(&str, &[u8])> = Vec::new();
    for inj in &injections {
        entries.push((inj.name.as_str(), inj.value.expose()));
    }
    for (i, v) in &wire {
        entries.push((injections[*i].name.as_str(), v.expose()));
    }
    let own = Redactor::new(entries);

    let exact_ip = matches!(origin.host, HostKind::Ip(_));
    let mut redirected = false;
    let mut hops = 0usize;
    loop {
        for (param, value) in &query_edits {
            set_query_param(&mut url, param, value.expose_str().expect("checked above"));
        }
        let dest = Destination::vet(&url, exact_ip)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let client = egress::client(Purpose::VaultFetch, &dest, remaining)?;

        let mut headers = HeaderMap::new();
        for (n, v) in &agent_headers {
            headers.append(n.clone(), v.clone());
        }
        for (n, v) in &injected_headers {
            headers.insert(n.clone(), v.clone());
        }
        headers.insert(reqwest::header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));

        let mut req = client.request(method.clone(), url.clone()).headers(headers);
        if let Some(b) = &body {
            req = req.body(b.expose().to_vec());
        }
        let resp = req.send().map_err(|e| upstream_error(send_kind(&e.without_url()), &origin))?;
        let status = resp.status();

        if status.is_redirection() && hops < MAX_REDIRECTS {
            if let Some(next) = follow_target(&url, &resp, &origin) {
                drop(resp);
                hops += 1;
                redirected = true;
                let code = status.as_u16();
                if (code == 303 && method != reqwest::Method::HEAD)
                    || ((code == 301 || code == 302) && method == reqwest::Method::POST)
                {
                    method = reqwest::Method::GET;
                    body = None;
                }
                url = next;
                continue;
            }
        }

        let mut result = read_response(resp, &method, max_bytes, deadline, &own, redactor, &origin)?;
        result.redirected = redirected;
        if !spec.include_headers {
            result.headers.retain(|(n, _)| RESPONSE_HEADER_ALLOWLIST.contains(&n.as_str()));
        }
        return Ok(result);
    }
}

/// A same-origin `Location` to follow, or `None` (return the 3xx as-is).
fn follow_target(current: &url::Url, resp: &reqwest::blocking::Response, origin: &Origin) -> Option<url::Url> {
    let loc = resp.headers().get(reqwest::header::LOCATION)?.to_str().ok()?;
    let next = current.join(loc).ok()?;
    let to = Origin::from_url(&next).ok()?;
    redirect_allowed(origin, &to).then_some(next)
}

/// Set `param` to `value`, replacing any agent-supplied value.
fn set_query_param(url: &mut url::Url, param: &str, value: &str) {
    if url.query_pairs().any(|(k, _)| k == param) {
        let kept: Vec<(String, String)> =
            url.query_pairs().filter(|(k, _)| k != param).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        url.set_query(None);
        let mut q = url.query_pairs_mut();
        for (k, v) in &kept {
            q.append_pair(k, v);
        }
        q.append_pair(param, value);
    } else {
        url.query_pairs_mut().append_pair(param, value);
    }
}

fn send_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "couldn't connect"
    } else {
        "request failed"
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Coding {
    Identity,
    Gzip,
    Deflate,
}

fn coding(headers: &HeaderMap) -> Option<Coding> {
    let mut out = Coding::Identity;
    for v in headers.get_all(reqwest::header::CONTENT_ENCODING) {
        for part in v.to_str().ok()?.split(',') {
            let c = match part.trim().to_ascii_lowercase().as_str() {
                "" | "identity" => continue,
                "gzip" | "x-gzip" => Coding::Gzip,
                "deflate" => Coding::Deflate,
                _ => return None,
            };
            if out != Coding::Identity {
                return None; // stacked codings
            }
            out = c;
        }
    }
    Some(out)
}

/// Two redactors in a row over a stream: the injected values first, then
/// the caller's.
struct Chain<'a> {
    own: StreamRedactor<'a>,
    caller: StreamRedactor<'a>,
}

impl Chain<'_> {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        let a = self.own.push(chunk);
        self.caller.push(&a)
    }

    fn finish(&mut self) -> Vec<u8> {
        let a = self.own.finish();
        let mut b = self.caller.push(&a);
        b.extend(self.caller.finish());
        b
    }
}

fn redact_both(own: &Redactor, caller: &Redactor, v: &[u8]) -> String {
    String::from_utf8_lossy(&caller.redact(&own.redact(v))).into_owned()
}

fn read_response(
    resp: reqwest::blocking::Response,
    method: &reqwest::Method,
    max_bytes: usize,
    deadline: Instant,
    own: &Redactor,
    caller: &Redactor,
    origin: &Origin,
) -> Result<FetchResult, VaultError> {
    let status = resp.status();
    let raw_headers = resp.headers().clone();
    let no_body = *method == reqwest::Method::HEAD || status.as_u16() == 204 || status.as_u16() == 304;

    let mut body = Vec::new();
    let mut truncated = false;
    if !no_body {
        let coding = coding(&raw_headers)
            .ok_or_else(|| upstream_error("the response used a compression K2 can't read", origin))?;
        let mut reader: Box<dyn Read> = match coding {
            Coding::Identity => Box::new(resp),
            Coding::Gzip => Box::new(flate2::read::GzDecoder::new(resp)),
            Coding::Deflate => Box::new(flate2::read::ZlibDecoder::new(resp)),
        };
        let mut chain = Chain { own: own.stream(), caller: caller.stream() };
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            if Instant::now() > deadline {
                return Err(upstream_error("timed out", origin));
            }
            let n = match reader.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(upstream_error(read_kind(&e, coding), origin)),
            };
            if n == 0 {
                body.extend(chain.finish());
                break;
            }
            body.extend(chain.push(&buf[..n]));
            if body.len() > max_bytes {
                // Stop reading. The redactors' held-back tail is dropped,
                // never emitted, so a value cut at the limit can't leak.
                break;
            }
        }
        if body.len() > max_bytes {
            body.truncate(max_bytes);
            truncated = true;
        }
    }

    let mut headers = Vec::new();
    for (name, value) in raw_headers.iter() {
        if name == reqwest::header::CONTENT_ENCODING || name == reqwest::header::TRANSFER_ENCODING {
            continue;
        }
        if name == reqwest::header::CONTENT_LENGTH && !no_body {
            continue;
        }
        headers.push((redact_both(own, caller, name.as_str().as_bytes()), redact_both(own, caller, value.as_bytes())));
    }
    if !no_body {
        headers.push(("content-length".to_string(), body.len().to_string()));
    }
    Ok(FetchResult { status: status.as_u16(), headers, body, truncated, redirected: false })
}

fn read_kind(e: &std::io::Error, coding: Coding) -> &'static str {
    let inner = e.get_ref().and_then(|x| x.downcast_ref::<reqwest::Error>());
    if inner.is_some_and(|r| r.is_timeout()) || e.kind() == std::io::ErrorKind::TimedOut {
        "timed out"
    } else if coding != Coding::Identity && inner.is_none() {
        "couldn't decode the response"
    } else {
        "the response was cut off"
    }
}

/// Per-key fetch limits (§9.2): a burst cap per second and a cap per
/// minute. In memory; the caller picks the key (one per session).
pub struct RateLimiter {
    per_second: usize,
    per_minute: usize,
    hits: parking_lot::Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(per_second: usize, per_minute: usize) -> RateLimiter {
        RateLimiter { per_second, per_minute, hits: parking_lot::Mutex::new(HashMap::new()) }
    }

    /// 30 a second, 600 a minute.
    pub fn for_fetch() -> RateLimiter {
        RateLimiter::new(30, 600)
    }

    pub fn check(&self, key: &str) -> Result<(), VaultError> {
        self.check_at(key, Instant::now())
    }

    pub fn check_at(&self, key: &str, now: Instant) -> Result<(), VaultError> {
        let minute = Duration::from_secs(60);
        let second = Duration::from_secs(1);
        let mut map = self.hits.lock();
        if map.len() > 4096 {
            map.retain(|_, q| q.back().is_some_and(|t| now.saturating_duration_since(*t) < minute));
        }
        let q = map.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now.saturating_duration_since(*t) >= minute) {
            q.pop_front();
        }
        let last_second = q.iter().rev().take_while(|t| now.saturating_duration_since(**t) < second).count();
        if q.len() >= self.per_minute || last_second >= self.per_second {
            return Err(VaultError::new("rate_limited", 429, "too many vault fetches; wait a moment and try again"));
        }
        q.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::test_util::{self, AirgapEnv, Roots};
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    const KEY: &str = "k2test_fake_key_4eC39HqLyjWDarjt";

    #[derive(Clone, Debug)]
    struct Seen {
        head: String,
        body: Vec<u8>,
    }

    /// A plain-HTTP upstream on 127.0.0.1 that records what it received.
    struct Upstream {
        port: u16,
        conns: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    type Responder = dyn Fn(&Seen) -> Vec<Vec<u8>> + Send + Sync;

    impl Upstream {
        /// `respond` returns the response in pieces, written with a pause
        /// between them so the client reads them separately.
        fn start(respond: impl Fn(&Seen) -> Vec<Vec<u8>> + Send + Sync + 'static) -> Upstream {
            let l = TcpListener::bind("127.0.0.1:0").expect("bind upstream");
            let port = l.local_addr().expect("addr").port();
            let conns = Arc::new(AtomicUsize::new(0));
            let seen = Arc::new(Mutex::new(Vec::new()));
            let respond: Arc<Responder> = Arc::new(respond);
            let (c2, s2) = (conns.clone(), seen.clone());
            std::thread::spawn(move || {
                for s in l.incoming() {
                    let Ok(mut s) = s else { return };
                    c2.fetch_add(1, Ordering::SeqCst);
                    let (s3, r3) = (s2.clone(), respond.clone());
                    std::thread::spawn(move || {
                        let Some(req) = read_request(&mut s) else { return };
                        s3.lock().expect("seen lock").push(req.clone());
                        for (i, piece) in r3(&req).iter().enumerate() {
                            if i > 0 {
                                std::thread::sleep(Duration::from_millis(30));
                            }
                            if s.write_all(piece).is_err() {
                                return;
                            }
                            let _ = s.flush();
                        }
                    });
                }
            });
            Upstream { port, conns, seen }
        }

        fn base(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn pattern(&self) -> OriginPattern {
            OriginPattern::parse(&self.base(), false).expect("exact ip pattern")
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().expect("seen lock").clone()
        }

        fn conns(&self) -> usize {
            self.conns.load(Ordering::SeqCst)
        }
    }

    fn read_request(s: &mut std::net::TcpStream) -> Option<Seen> {
        s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let head_end = loop {
            if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            let n = s.read(&mut buf).ok()?;
            if n == 0 {
                return None;
            }
            data.extend_from_slice(&buf[..n]);
        };
        let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
        let len = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        let mut body = data[head_end..].to_vec();
        while body.len() < len {
            let n = s.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
        Some(Seen { head, body })
    }

    fn resp(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut r = format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in headers {
            r.push_str(&format!("{k}: {v}\r\n"));
        }
        r.push_str("\r\n");
        let mut r = r.into_bytes();
        r.extend_from_slice(body);
        r
    }

    fn ok(body: &str) -> Vec<Vec<u8>> {
        vec![resp("200 OK", &[("Content-Type", "text/plain")], body.as_bytes())]
    }

    fn spec(url: &str) -> FetchSpec {
        FetchSpec {
            method: "GET".into(),
            url: url.into(),
            headers: vec![],
            body: None,
            max_bytes: 0,
            timeout: Duration::from_secs(10),
            include_headers: false,
        }
    }

    fn bearer(allowed: Vec<OriginPattern>) -> Injection {
        Injection {
            name: "STRIPE_KEY".into(),
            value: SecretBytes::from_slice(KEY.as_bytes()),
            dst: Dst::Header("Authorization".into()),
            fmt: "Bearer {}".into(),
            allowed,
        }
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    fn b64(v: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(v)
    }

    #[test]
    fn header_injection_lands_only_in_the_sealed_header() {
        let _a = AirgapEnv::set(false);
        let up = Upstream::start(|_| ok("fine"));
        let mut s = spec(&format!("{}/v1/charges?limit=3", up.base()));
        s.headers = vec![("X-Trace".into(), "abc".into())];
        let r = execute(s, vec![bearer(vec![up.pattern()])], &Redactor::empty()).expect("fetch");
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"fine");
        assert!(!r.redirected && !r.truncated);
        let seen = up.seen();
        assert_eq!(seen.len(), 1);
        let head = &seen[0].head;
        assert!(head.starts_with("GET /v1/charges?limit=3 HTTP/1.1\r\n"), "{head}");
        assert!(head.contains(&format!("authorization: Bearer {KEY}\r\n")), "{head}");
        assert_eq!(count(head.as_bytes(), KEY.as_bytes()), 1, "value appears once, in the sealed header");
        assert!(head.contains("x-trace: abc\r\n"), "{head}");
        assert!(head.contains("accept-encoding: identity\r\n"), "{head}");
        assert!(head.contains(&format!("host: 127.0.0.1:{}\r\n", up.port)), "{head}");
    }

    #[test]
    fn query_injection_replaces_an_agent_value() {
        let _a = AirgapEnv::set(false);
        let up = Upstream::start(|_| ok("fine"));
        let inj = Injection { dst: Dst::Query("api_key".into()), fmt: "{}".into(), ..bearer(vec![up.pattern()]) };
        let s = spec(&format!("{}/q?api_key=agent-chosen&x=1", up.base()));
        execute(s, vec![inj], &Redactor::empty()).expect("fetch");
        let head = &up.seen()[0].head;
        let line = head.lines().next().expect("request line");
        assert!(line.contains(&format!("api_key={KEY}")), "{line}");
        assert!(line.contains("x=1"), "{line}");
        assert!(!line.contains("agent-chosen"), "{line}");
        assert!(!head.to_ascii_lowercase().contains("authorization"), "{head}");
    }

    #[test]
    fn body_injection_fills_the_marker_only() {
        let _a = AirgapEnv::set(false);
        let up = Upstream::start(|_| ok("fine"));
        let inj = Injection { dst: Dst::Body, fmt: "{}".into(), ..bearer(vec![up.pattern()]) };
        let mut s = spec(&format!("{}/token", up.base()));
        s.method = "POST".into();
        s.headers = vec![("Content-Type".into(), "application/x-www-form-urlencoded".into())];
        s.body = Some(b"grant_type=client_credentials&client_secret={{secret}}".to_vec());
        execute(s, vec![inj], &Redactor::empty()).expect("fetch");
        let seen = &up.seen()[0];
        assert_eq!(seen.body, format!("grant_type=client_credentials&client_secret={KEY}").into_bytes());
        assert_eq!(count(seen.head.as_bytes(), KEY.as_bytes()), 0, "not in headers");

        let inj = Injection { dst: Dst::Body, fmt: "{}".into(), ..bearer(vec![up.pattern()]) };
        let mut s = spec(&format!("{}/token", up.base()));
        s.method = "POST".into();
        let e = execute(s, vec![inj], &Redactor::empty()).expect_err("no body");
        assert_eq!(e.code, "body_marker_missing");
    }

    #[test]
    fn agent_cannot_set_routing_framing_or_injected_headers() {
        let _a = AirgapEnv::set(false);
        let up = Upstream::start(|_| ok("fine"));
        for (n, v) in [
            ("Host", "other.example"),
            ("Transfer-Encoding", "chunked"),
            ("Content-Length", "0"),
            ("Connection", "close"),
            ("X-A", "a\r\nX-B: b"),
            ("X-A", "a\nb"),
            ("Authorization", "Bearer agent"),
            ("authorization", "Bearer agent"),
        ] {
            let mut s = spec(&format!("{}/", up.base()));
            s.headers = vec![(n.into(), v.into())];
            let e = execute(s, vec![bearer(vec![up.pattern()])], &Redactor::empty()).expect_err(n);
            assert_eq!(e.code, "header_not_allowed", "{n}");
        }
        assert_eq!(up.conns(), 0, "no connection for a refused request");
    }

    #[test]
    fn host_mismatch_refuses_before_any_connection() {
        let _a = AirgapEnv::set(false);
        let up = Upstream::start(|_| ok("fine"));
        let elsewhere = OriginPattern::parse("api.stripe.com", false).expect("pattern");
        let e = execute(spec(&format!("{}/", up.base())), vec![bearer(vec![elsewhere])], &Redactor::empty())
            .expect_err("mismatch");
        assert_eq!(e.code, "host_not_allowed");
        // Two secrets: both must allow the origin.
        let other = Injection {
            name: "OTHER".into(),
            value: SecretBytes::from_slice(b"other-value"),
            dst: Dst::Header("X-Other".into()),
            fmt: "{}".into(),
            allowed: vec![OriginPattern::parse("http://127.0.0.1:1", false).expect("pattern")],
        };
        let e = execute(spec(&format!("{}/", up.base())), vec![bearer(vec![up.pattern()]), other], &Redactor::empty())
            .expect_err("second secret not allowed");
        assert_eq!(e.code, "host_not_allowed");
        let e = execute(spec(&format!("{}/", up.base())), vec![], &Redactor::empty()).expect_err("no secret");
        assert_eq!(e.code, "no_secret");
        assert_eq!(up.conns(), 0);
    }

    #[test]
    fn host_name_resolving_to_a_blocked_address_is_refused() {
        let _a = AirgapEnv::set(false);
        let up = Upstream::start(|_| ok("fine"));
        let pat = OriginPattern::parse(&format!("http://localhost:{}", up.port), false).expect("pattern");
        assert!(!pat.is_exact_ip());
        let e = execute(spec(&format!("http://localhost:{}/", up.port)), vec![bearer(vec![pat])], &Redactor::empty())
            .expect_err("loopback by name");
        assert_eq!(e.code, "address_not_allowed");
        assert_eq!(up.conns(), 0);
    }

    #[test]
    fn same_origin_redirect_is_followed_with_the_secret() {
        let _a = AirgapEnv::set(false);
        let up = Upstream::start(|req| {
            if req.head.starts_with("GET /start") {
                vec![resp("302 Found", &[("Location", "/next")], b"")]
            } else {
                ok("arrived")
            }
        });
        let r = execute(spec(&format!("{}/start", up.base())), vec![bearer(vec![up.pattern()])], &Redactor::empty())
            .expect("fetch");
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"arrived");
        assert!(r.redirected);
        let seen = up.seen();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].head.starts_with("GET /next "), "{}", seen[1].head);
        assert!(seen[1].head.contains(&format!("authorization: Bearer {KEY}")));
    }

    #[test]
    fn off_origin_redirect_is_returned_and_the_secret_not_resent() {
        let _a = AirgapEnv::set(false);
        let other = Upstream::start(|_| ok("other"));
        let target = format!("{}/collect?k={KEY}", other.base());
        let up = Upstream::start(move |_| vec![resp("302 Found", &[("Location", &target)], b"moved")]);
        // Even with the other origin also allowed, a redirect to it isn't
        // followed.
        let r = execute(
            spec(&format!("{}/start", up.base())),
            vec![bearer(vec![up.pattern(), other.pattern()])],
            &Redactor::empty(),
        )
        .expect("3xx comes back");
        assert_eq!(r.status, 302);
        assert!(!r.redirected);
        assert_eq!(r.body, b"moved");
        let loc = r.headers.iter().find(|(n, _)| n == "location").expect("location kept").1.clone();
        assert_eq!(loc, format!("{}/collect?k=[k2v:STRIPE_KEY]", other.base()));
        assert_eq!(other.conns(), 0, "redirect target never contacted");
        assert_eq!(up.seen().len(), 1);
    }

    #[test]
    fn https_to_http_redirect_is_not_followed() {
        let _a = AirgapEnv::set(false);
        let plain = Upstream::start(|_| ok("plain"));
        let ca = test_util::ca();
        let (cert, key) = test_util::leaf(&ca, &["127.0.0.1"]);
        let resp_bytes: &'static [u8] = Box::leak(
            resp("301 Moved Permanently", &[("Location", &format!("{}/", plain.base()))], b"").into_boxed_slice(),
        );
        let (tls_port, rx) = test_util::serve(cert, key, resp_bytes);
        let _roots = Roots::set(test_util::root(&ca));
        let tls_pat = OriginPattern::parse(&format!("https://127.0.0.1:{tls_port}"), false).expect("pattern");
        let r = execute(
            spec(&format!("https://127.0.0.1:{tls_port}/start")),
            vec![bearer(vec![tls_pat, plain.pattern()])],
            &Redactor::empty(),
        )
        .expect("3xx comes back");
        assert_eq!(r.status, 301);
        assert!(!r.redirected);
        let head = rx.recv_timeout(Duration::from_secs(5)).expect("tls upstream saw the request");
        assert!(String::from_utf8_lossy(&head).contains(&format!("authorization: Bearer {KEY}")));
        assert_eq!(plain.conns(), 0, "plain-http target never contacted");
    }

    #[test]
    fn echoed_secret_is_redacted_in_body_and_headers() {
        let _a = AirgapEnv::set(false);
        const SPECIAL: &str = "pk/live+Ab9=zz~Q";
        let special_pct = "pk%2Flive%2BAb9%3Dzz~Q";
        let body = format!(
            "raw={KEY}\nb64={}\nbasic={}\nsraw={SPECIAL}\nspct={special_pct}\nsb64={}\nother=hunter2-other-value\n",
            b64(KEY.as_bytes()),
            b64(format!("user:{KEY}").as_bytes()),
            b64(SPECIAL.as_bytes()),
        );
        let echo_hdr = format!("Bearer {KEY}");
        let cookie = format!("s={}; Path=/", b64(KEY.as_bytes()));
        let www = format!("Bearer realm=\"{special_pct}\"");
        let loc = format!("/x?k={KEY}");
        let up = Upstream::start(move |_| {
            vec![resp(
                "200 OK",
                &[
                    ("Content-Type", "text/plain"),
                    ("X-Echo", &echo_hdr),
                    ("Set-Cookie", &cookie),
                    ("WWW-Authenticate", &www),
                    ("Location", &loc),
                ],
                body.as_bytes(),
            )]
        });
        let special = Injection {
            name: "SPECIAL".into(),
            value: SecretBytes::from_slice(SPECIAL.as_bytes()),
            dst: Dst::Header("X-Api-Key".into()),
            fmt: "{}".into(),
            allowed: vec![up.pattern()],
        };
        let caller = Redactor::new([("OTHER", b"hunter2-other-value".as_slice())]);
        let mut s = spec(&format!("{}/", up.base()));
        s.include_headers = true;
        let r = execute(s, vec![bearer(vec![up.pattern()]), special], &caller).expect("fetch");
        let text = String::from_utf8(r.body.clone()).expect("utf8");
        let b64_key = b64(KEY.as_bytes());
        for leaked in [KEY, b64_key.as_str(), SPECIAL, special_pct] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
        assert_eq!(text.matches("[k2v:STRIPE_KEY]").count(), 3, "{text}");
        assert_eq!(text.matches("[k2v:SPECIAL]").count(), 3, "{text}");
        assert!(text.contains("other=[k2v:OTHER]"), "{text}");
        for (n, v) in &r.headers {
            assert!(!v.contains(KEY) && !v.contains(&b64_key) && !v.contains(special_pct), "{n}: {v}");
        }
        let get = |n: &str| r.headers.iter().find(|(k, _)| k == n).unwrap_or_else(|| panic!("{n} missing")).1.clone();
        // The whole header value as sent is one pattern.
        assert_eq!(get("x-echo"), "[k2v:STRIPE_KEY]");
        // Base64 padding past the value's last whole sextet stays.
        let cookie = get("set-cookie");
        assert!(cookie.starts_with("s=[k2v:STRIPE_KEY]") && cookie.ends_with("; Path=/"), "{cookie}");
        assert_eq!(get("www-authenticate"), "Bearer realm=\"[k2v:SPECIAL]\"");
        assert_eq!(get("location"), "/x?k=[k2v:STRIPE_KEY]");

        // Without --include-headers only the allowlist comes back.
        let r = execute(spec(&format!("{}/", up.base())), vec![bearer(vec![up.pattern()])], &caller).expect("fetch");
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"content-type") && names.contains(&"location"), "{names:?}");
        assert!(!names.contains(&"x-echo") && !names.contains(&"set-cookie"), "{names:?}");
    }

    #[test]
    fn compressed_echo_is_decoded_then_redacted() {
        let _a = AirgapEnv::set(false);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(format!("{{\"key\":\"{KEY}\"}}").as_bytes()).expect("gzip");
        let gz = gz.finish().expect("gzip finish");
        let up = Upstream::start(move |req| {
            if req.head.starts_with("GET /br") {
                vec![resp("200 OK", &[("Content-Encoding", "br")], b"\x0b\x02\x80")]
            } else {
                vec![resp("200 OK", &[("Content-Encoding", "gzip"), ("Content-Type", "application/json")], &gz)]
            }
        });
        let mut s = spec(&format!("{}/gz", up.base()));
        s.include_headers = true;
        let r = execute(s, vec![bearer(vec![up.pattern()])], &Redactor::empty()).expect("fetch");
        assert_eq!(String::from_utf8(r.body.clone()).expect("utf8"), "{\"key\":\"[k2v:STRIPE_KEY]\"}");
        assert!(r.headers.iter().all(|(n, _)| n != "content-encoding"), "{:?}", r.headers);
        let len = r.headers.iter().find(|(n, _)| n == "content-length").expect("length").1.clone();
        assert_eq!(len, r.body.len().to_string());
        assert!(up.seen()[0].head.contains("accept-encoding: identity\r\n"));

        let e = execute(spec(&format!("{}/br", up.base())), vec![bearer(vec![up.pattern()])], &Redactor::empty())
            .expect_err("unreadable encoding");
        assert_eq!(e.code, "upstream_error");
    }

    #[test]
    fn split_and_large_bodies_are_redacted_and_capped() {
        let _a = AirgapEnv::set(false);
        // A value split across two writes is still caught.
        let (a, b) = KEY.split_at(10);
        let (a, b) = (a.to_string(), b.to_string());
        let total = 6 + KEY.len() + 4;
        let up = Upstream::start(move |req| {
            if req.head.starts_with("GET /split") {
                let head = format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {total}\r\n\r\nhello {a}");
                vec![head.into_bytes(), format!("{b} end").into_bytes()]
            } else {
                // 64 KiB of filler with the value straddling byte 1000.
                let mut body = vec![b'x'; 64 * 1024];
                body[995..995 + KEY.len()].copy_from_slice(KEY.as_bytes());
                vec![resp("200 OK", &[], &body)]
            }
        });
        let r = execute(spec(&format!("{}/split", up.base())), vec![bearer(vec![up.pattern()])], &Redactor::empty())
            .expect("fetch");
        assert_eq!(String::from_utf8(r.body).expect("utf8"), "hello [k2v:STRIPE_KEY] end");
        assert!(!r.truncated);

        let mut s = spec(&format!("{}/big", up.base()));
        s.max_bytes = 1000;
        let r = execute(s, vec![bearer(vec![up.pattern()])], &Redactor::empty()).expect("fetch");
        assert!(r.truncated);
        assert_eq!(r.body.len(), 1000);
        assert!(r.body[..995].iter().all(|&c| c == b'x'));
        assert_eq!(&r.body[995..], b"[k2v:", "no part of the value at the cut");

        let mut s = spec(&format!("{}/big", up.base()));
        s.max_bytes = MAX_RESPONSE + 1;
        assert_eq!(
            execute(s, vec![bearer(vec![up.pattern()])], &Redactor::empty()).expect_err("too big").code,
            "bad_max_bytes"
        );
    }

    #[test]
    fn errors_never_carry_the_url_query_or_path() {
        let _a = AirgapEnv::set(false);
        // A port with nothing listening.
        let dead = TcpListener::bind("127.0.0.1:0").expect("bind");
        let dead_port = dead.local_addr().expect("addr").port();
        drop(dead);
        let pat = OriginPattern::parse(&format!("http://127.0.0.1:{dead_port}"), false).expect("pattern");
        let inj = Injection { dst: Dst::Query("key".into()), fmt: "{}".into(), ..bearer(vec![pat]) };
        let e = execute(
            spec(&format!("http://127.0.0.1:{dead_port}/private/path?marker=query-marker-123")),
            vec![inj],
            &Redactor::empty(),
        )
        .expect_err("refused connection");
        assert_eq!(e.code, "upstream_error");
        let shown = format!("{e} {e:?}");
        for bad in ["query-marker-123", "/private/path", KEY, "marker="] {
            assert!(!shown.contains(bad), "{bad} in {shown}");
        }

        // A silent upstream: the timeout error is fixed text too.
        let silent = TcpListener::bind("127.0.0.1:0").expect("bind");
        let silent_port = silent.local_addr().expect("addr").port();
        let hold = std::thread::spawn(move || {
            let c = silent.accept().expect("accept");
            std::thread::sleep(Duration::from_secs(3));
            drop(c);
        });
        let pat = OriginPattern::parse(&format!("http://127.0.0.1:{silent_port}"), false).expect("pattern");
        let mut s = spec(&format!("http://127.0.0.1:{silent_port}/p?marker=query-marker-456"));
        s.timeout = Duration::from_secs(1);
        let e = execute(s, vec![bearer(vec![pat])], &Redactor::empty()).expect_err("timeout");
        assert_eq!(e.code, "upstream_error");
        assert!(e.hint.contains("timed out"), "{}", e.hint);
        let shown = format!("{e} {e:?}");
        assert!(!shown.contains("query-marker-456") && !shown.contains(KEY), "{shown}");
        hold.join().expect("silent upstream thread");
    }

    #[test]
    fn airgap_refuses_external_hosts_but_reaches_exact_lan_ips() {
        let _a = AirgapEnv::set(true);
        let stripe = OriginPattern::parse("api.stripe.com", false).expect("pattern");
        let e = execute(spec("https://api.stripe.com/v1/charges"), vec![bearer(vec![stripe])], &Redactor::empty())
            .expect_err("external under air-gap");
        assert_eq!(e.code, "airgap");
        assert_eq!(e.status, 403);

        let up = Upstream::start(|_| ok("lan"));
        let r = execute(spec(&format!("{}/", up.base())), vec![bearer(vec![up.pattern()])], &Redactor::empty())
            .expect("exact LAN ip under air-gap");
        assert_eq!(r.body, b"lan");
    }

    #[test]
    fn rate_limiter_caps_bursts_and_minutes_per_key() {
        let rl = RateLimiter::for_fetch();
        let t0 = Instant::now();
        for _ in 0..30 {
            rl.check_at("s1", t0).expect("within burst");
        }
        let e = rl.check_at("s1", t0).expect_err("burst over");
        assert_eq!((e.code, e.status), ("rate_limited", 429));
        rl.check_at("s2", t0).expect("other key unaffected");
        // Twenty more seconds at 30/s reaches 600 in the minute.
        for s in 1..20u64 {
            for _ in 0..30 {
                rl.check_at("s1", t0 + Duration::from_secs(s)).expect("within limits");
            }
        }
        let e = rl.check_at("s1", t0 + Duration::from_secs(20)).expect_err("minute over");
        assert_eq!(e.code, "rate_limited");
        rl.check_at("s1", t0 + Duration::from_secs(61)).expect("window moved on");
    }
}
