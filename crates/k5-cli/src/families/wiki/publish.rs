//! `wiki publish`'s readers and lines (`cmd_wiki_publish` in
//! `cli/src/wiki.sh`, and the Connect helpers it calls from
//! `cli/src/connect.sh`): what each Python block or `echo` prints for what
//! it reads. The requests are [`super::Run::publish`]'s.

use super::super::{get, py_space, s, Raise};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::json::loads;
use k2_client::hosts::{ZONE, ZONE_SUFFIX};
use k2_client::{cli_name, product_name};

fn text(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// `$(…)`: the trailing newlines dropped.
fn held(mut t: String) -> String {
    while t.ends_with('\n') {
        t.pop();
    }
    t
}

fn dumps(v: &Value, compact: bool) -> String {
    json::to_string(
        v,
        &Options {
            compact,
            ..Options::default()
        },
    )
    .expect("a value from JSON always prints")
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `basename "$PROJECT"`, as `$(…)` holds it.
fn basename(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    held(base.to_string())
}

/// `_wiki_default_label`: the workspace folder's name as a slug, at most
/// 48 bytes, else `brain`.
pub fn default_label(project: &str) -> String {
    let base = basename(project);
    let lower = base.trim_matches(py_space).to_lowercase();
    let mut slug = String::new();
    let mut gap = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !slug.is_empty() {
                slug.push('-');
                gap = false;
            }
            slug.push(c);
        } else {
            gap = true;
        }
    }
    // `re.sub(...).strip("-")`: a run at either end goes.
    if slug.is_empty() {
        return "brain".to_string();
    }
    slug.truncate(48);
    slug
}

/// `_refuse_reserved_nested_label`: the refusal line for a box door's
/// label (`tr` lowers ASCII and drops ASCII spaces first), then exit 1.
pub fn reserved_refusal(label: &str) -> Option<String> {
    let norm: String = label
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let cli = cli_name!();
    let product = product_name!();
    let line = match norm.as_str() {
        "skin" => format!(
            "Error: reserved_label: 'skin' is reserved. Pick another nested label for your UI ({cli} study apps; leftover: {cli} study skins)."
        ),
        "meet" => format!(
            "Error: reserved_label: 'meet' is reserved for {product} Meet (meet.<box>.{ZONE}). Pick another nested label."
        ),
        "chat" => format!(
            "Error: reserved_label: 'chat' is reserved for {product} chat (chat.<box>.{ZONE}). Pick another nested label."
        ),
        "mail" => format!(
            "Error: reserved_label: 'mail' is reserved for {product} webmail (mail.<box>.{ZONE}). Pick another nested label."
        ),
        _ => return None,
    };
    Some(format!("{line}\n"))
}

/// `load(s)`: the answer, `{}` when it is blank or not JSON.
fn load(raw: &str) -> Value {
    if raw.chars().all(py_space) {
        return Value::Object(Vec::new());
    }
    loads(raw).unwrap_or_else(|_| Value::Object(Vec::new()))
}

/// One of the two port-and-url readers: what it prints (as `$(…)` holds
/// it), or its traceback. `url`: the url reader.
fn pick(sources: &[Value], url: bool) -> Result<String, Raise> {
    for d in sources {
        let Value::Object(pairs) = d else {
            return Err(Raise::no_attribute(9, d, "get"));
        };
        if get(pairs, "error").is_some_and(py::truthy) {
            continue;
        }
        let port = get(pairs, "port").filter(|p| **p != Value::Null);
        if url {
            if let Some(u) = get(pairs, "url").filter(|u| py::truthy(u)) {
                return Ok(held(py::str_of(u)));
            }
            if let Some(p) = port {
                return Ok(held(format!("http://127.0.0.1:{}", py::str_of(p))));
            }
        } else if let Some(p) = port {
            return Ok(held(py::str_of(p)));
        }
    }
    Ok(String::new())
}

/// The port and the url publish reads from the serve answer, then the
/// serve status (its Python gets them in environment variables, after an
/// empty `PUB`). Each is what its Python printed, or the traceback it
/// wrote on stderr (and printed nothing).
pub fn port_and_url(serve: &[u8], status: &[u8]) -> (Result<String, Raise>, Result<String, Raise>) {
    let sources = [
        Value::Object(Vec::new()),
        load(&text(serve)),
        load(&text(status)),
    ];
    (pick(&sources, false), pick(&sources, true))
}

/// `_subdomain_label_preflight`'s reader of the claim check: `Ok` when
/// the daemon said `success`, else its lines on stderr.
pub fn preflight(resp: &[u8]) -> Result<(), String> {
    let raw = text(resp);
    let raw = if raw.is_empty() { "{}".to_string() } else { raw };
    let d = loads(&raw).unwrap_or_else(|_| Value::Object(Vec::new()));
    let pairs = match &d {
        Value::Object(p) => Some(p),
        _ => None,
    };
    if pairs.is_some_and(|p| get(p, "success").is_some_and(py::truthy)) {
        return Ok(());
    }
    let err = pairs
        .and_then(|p| get(p, "error"))
        .filter(|e| py::truthy(e))
        .map_or_else(|| "no response from the daemon".to_string(), py::str_of);
    let mut lines = format!("Error: {err}\n");
    if let Some(h) = pairs.and_then(|p| get(p, "hint")).filter(|h| py::truthy(h)) {
        lines.push_str(&format!("  {}\n", py::str_of(h)));
    }
    Err(lines)
}

/// `_stamp_subdomain_attrib claim <label>`'s readers: `Ok` on `success`,
/// else its two lines on stderr.
pub fn stamp(resp: &[u8], label: &str) -> Result<(), String> {
    let raw = text(resp);
    let raw = if raw.is_empty() { "{}".to_string() } else { raw };
    let parsed = loads(&raw);
    // `(json.loads(…) or {}).get("success")`, errors hidden: a falsy
    // value is `{}`, a truthy one that is no dict raises.
    let ok = match &parsed {
        Ok(Value::Object(p)) => get(p, "success").is_some_and(py::truthy),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    let err = match &parsed {
        Ok(Value::Object(p)) => get(p, "error")
            .filter(|e| py::truthy(e))
            .map(|e| held(py::str_of(e))),
        _ => None,
    }
    .unwrap_or_else(|| "no response from the daemon".to_string());
    Err(format!(
        "Error: workspace attribution was not recorded on the daemon: {err}\n  Adopt it later from the owning workspace: {} publish subdomain claim {label}\n",
        cli_name!()
    ))
}

/// `json.dumps({"label": label, "target": target})`.
pub fn create_body(label: &str, target: &str) -> String {
    dumps(&obj(vec![("label", s(label)), ("target", s(target))]), false)
}

/// `json.dumps({"target": target})`.
pub fn point_body(target: &str) -> String {
    dumps(&obj(vec![("target", s(target))]), false)
}

/// `_connect_curl_daemon`'s request body: `{"method", "endpoint"[,
/// "body"]}` as `json.dumps` writes it.
pub fn proxy_request(method: &str, endpoint: &str, body: &str) -> String {
    let mut pairs = vec![("method", s(method)), ("endpoint", s(endpoint))];
    if !body.is_empty() {
        pairs.push(("body", s(body)));
    }
    dumps(&obj(pairs), false)
}

/// Python's `int(x)` for a JSON value: the digits, or `None` where it
/// raises.
fn py_int(v: &Value) -> Option<String> {
    match v {
        Value::Int(d) => Some(d.clone()),
        Value::Bool(b) => Some(if *b { "1" } else { "0" }.to_string()),
        Value::Float(f) if f.is_finite() => {
            let t = f.trunc();
            Some(if t == 0.0 {
                "0".to_string()
            } else {
                format!("{t}")
            })
        }
        Value::Str(t) => {
            let st = t.as_str()?;
            let st = st.trim_matches(py_space);
            let (neg, digits) = match st.as_bytes().first() {
                Some(b'-') => (true, &st[1..]),
                Some(b'+') => (false, &st[1..]),
                _ => (false, st),
            };
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
            if !ok {
                return None;
            }
            let plain: String = digits.chars().filter(|c| *c != '_').collect();
            let plain = plain.trim_start_matches('0');
            Some(match (plain.is_empty(), neg) {
                (true, _) => "0".to_string(),
                (false, true) => format!("-{plain}"),
                (false, false) => plain.to_string(),
            })
        }
        _ => None,
    }
}

/// What `_connect_curl_daemon` makes of the daemon's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proxied {
    /// Status 200: `"%d\n%s" % (status, body)` of the control plane's
    /// answer it relayed, as `$(…)` holds it.
    Relayed(String),
    /// 404 or 405, a daemon from before the route: the caller may call the
    /// control plane itself.
    NoDaemon,
    /// Any other answer, or a 200 its Python can't read: it says why (on a
    /// stderr publish hides) and fails.
    Failed,
}

/// `_connect_curl_daemon`'s reading of one answer.
pub fn proxied(status: u16, body: &[u8]) -> Proxied {
    match status {
        200 => {}
        404 | 405 => return Proxied::NoDaemon,
        _ => return Proxied::Failed,
    }
    let raw = text(&super::super::without_nuls(body));
    let Ok(Value::Object(pairs)) = loads(&raw) else {
        return Proxied::Failed;
    };
    let Some(code) = get(&pairs, "status").and_then(py_int) else {
        return Proxied::Failed;
    };
    let Some(b) = get(&pairs, "body") else {
        return Proxied::Failed;
    };
    Proxied::Relayed(held(format!("{code}\n{}", py::str_of(b))))
}

/// `head -1 | tr -d '[:space:]'` of the relayed answer is 200 or 201.
pub fn made(cp_raw: &str) -> bool {
    let first: String = cp_raw
        .split('\n')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '\u{b}')
        .collect();
    first == "200" || first == "201"
}

/// `_connect_subdomain_render`, its stderr hidden: what the 2xx answer
/// prints, `None` where it fails (nothing printed).
fn render(cp_raw: &str, pretty: impl Fn(&[(Text, Value)]) -> String) -> Option<String> {
    let (status, body) = cp_raw.split_once('\n')?;
    let code: i64 = status.trim_matches(py_space).parse().ok()?;
    if !(200..300).contains(&code) {
        return None;
    }
    let d = if body.chars().all(py_space) {
        Value::Object(Vec::new())
    } else {
        loads(body).unwrap_or_else(|_| Value::Object(Vec::new()))
    };
    let Value::Object(pairs) = d else {
        return None;
    };
    Some(pretty(&pairs))
}

fn host(d: &[(Text, Value)]) -> String {
    match get(d, "host").filter(|h| py::truthy(h)) {
        Some(h) => py::str_of(h),
        None => format!(
            "{}{ZONE_SUFFIX}",
            get(d, "label").map_or_else(|| "?".to_string(), py::str_of)
        ),
    }
}

fn or_q(d: &[(Text, Value)], key: &str) -> String {
    get(d, key).map_or_else(|| "?".to_string(), py::str_of)
}

/// The create's lines.
pub fn created(cp_raw: &str) -> Option<String> {
    render(cp_raw, |d| {
        format!(
            "Published {}\n  target:    {}\n  status:    {}\n  effective: {}\n",
            host(d),
            or_q(d, "target"),
            or_q(d, "status"),
            or_q(d, "effective")
        )
    })
}

/// The point's line.
pub fn pointed(cp_raw: &str) -> Option<String> {
    render(cp_raw, |d| {
        format!("Repointed {} -> {}\n", host(d), or_q(d, "target"))
    })
}

/// The lines after the local site's, made or not.
pub fn closing_lines(label: &str, target: &str, made: bool) -> String {
    let cli = cli_name!();
    if made {
        return format!(
            "\nLive map is on the tunnel target above. Unpublish: {cli} wiki unpublish\n"
        );
    }
    format!(
        "To expose this brain on your {product} Connect host (Pro nested subdomain):\n\n  {cli} publish subdomain create {label} --target {target}\n\nIf the label already exists:\n  {cli} publish subdomain point {label} --target {target}\n\nRequires: tunnel up ({cli} tunnel start) + Pro plan for nested subdomains.\nUnpublish later: {cli} wiki unpublish\n",
        product = product_name!()
    )
}

/// `--json`'s line.
pub fn json_line(label: &str, target: &str, url: &str, port: &str, made: bool) -> String {
    let port = if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
        let p = port.trim_start_matches('0');
        Value::Int(if p.is_empty() { "0" } else { p }.to_string())
    } else {
        s(port)
    };
    let v = obj(vec![
        ("ok", Value::Bool(true)),
        ("label", s(label)),
        ("target", s(target)),
        ("url", if url.is_empty() { Value::Null } else { s(url) }),
        ("port", port),
        ("subdomainCreated", Value::Bool(made)),
    ]);
    format!("{}\n", dumps(&v, true))
}

/// `_connect_bearer_token`: `tunnel.json`'s token, stripped, as its Python
/// reads it (anything it can't read is none). Only whether there is one
/// is used here; the token itself is never printed or sent by this module.
pub fn has_tunnel_token(path: &std::path::Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(t) = String::from_utf8(bytes) else {
        return false;
    };
    let Ok(Value::Object(pairs)) = loads(&t) else {
        return false;
    };
    match get(&pairs, "token") {
        Some(Value::Str(tok)) => tok
            .as_str()
            .is_some_and(|x| !x.trim_matches(py_space).is_empty()),
        _ => false,
    }
}
