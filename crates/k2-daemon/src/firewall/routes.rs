//! `/cli/firewall/*` and `/cli/front/passthrough*` (0.46.6, PRD §4.3).
//!
//! ```text
//! GET  /cli/firewall                    status: the view, guard, who applies it   (?format=text)
//! GET  /cli/firewall/ports              the contract (K2 Web's E0 collector)
//! GET  /cli/firewall/rules?format=nft|ufw   ready-made rules (text/plain)
//! POST /cli/firewall/ssh       {any:true} | {allow:[cidr]} | {port:n}
//! POST /cli/firewall/open      {port, proto, label, from:[cidr]}
//! POST /cli/firewall/close     {port, proto}
//! POST /cli/firewall/guard     {app?, on?, connsPerIp?, writesPerMin?, reset?}
//! POST /cli/firewall/upstream  {hash, provider, appliedAt} | {managedBy}   (K2 Web's applied-ack)
//! GET  /cli/front/passthrough
//! POST /cli/front/passthrough/add  {host, to, proxyV2?, label?}
//! POST /cli/front/passthrough/rm   {host}
//! ```
//!
//! **Who.** Reads: the owner, an Admin login, a shell tab, any agent
//! passport (ports, not secrets). Every POST: the owner, an Admin login or
//! a shell tab (a person); `upstream` is the owner's (Owner login or the
//! owner token: K2 Web runs it as the daemon user). **An agent never
//! loosens the box's own firewall** (PA-A12): its POST is `403 owner_only`
//! with the owner's command, and the permission-card wrapper ends it with
//! "It can't be requested." (box-level, `NeverReason::BoxLevel`). The one
//! exception: the App's own agent with "Build and run Apps" may *tighten*
//! its App's guard (turn it on, lower its write limit).
//!
//! Every POST is method-guarded (405 on GET) and audited
//! (`auth-audit.jsonl`, `firewall-<act>`).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::firewall::{self as fw, AppGuard, OpenPort, Proto, RulesFormat, UpstreamAck};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const ROUTES: &[&str] = &[
    "/cli/firewall",
    "/cli/firewall/ports",
    "/cli/firewall/rules",
    "/cli/firewall/ssh",
    "/cli/firewall/open",
    "/cli/firewall/close",
    "/cli/firewall/guard",
    "/cli/firewall/upstream",
    "/cli/front/passthrough",
    "/cli/front/passthrough/add",
    "/cli/front/passthrough/rm",
];

pub const POST_ROUTES: &[&str] = &[
    "/cli/firewall/ssh",
    "/cli/firewall/open",
    "/cli/firewall/close",
    "/cli/firewall/guard",
    "/cli/firewall/upstream",
    "/cli/front/passthrough/add",
    "/cli/front/passthrough/rm",
];

/// Request bodies here are small.
pub const MAX_BODY: usize = 16 * 1024;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        _ => "500 Internal Server Error",
    }
}

fn refuse(status: u16, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status: status_line(status),
        content_type: "application/json",
        body: json!({"ok": false, "error": {"code": code, "hint": hint.into()}}).to_string(),
    }
}

pub fn too_large() -> CliResponse {
    refuse(413, "too_large", format!("request bodies here are at most {} KiB", MAX_BODY / 1024))
}

/// A person (owner, Admin login, shell tab) or an agent.
enum Caller {
    Person { actor: String, owner: bool },
    Agent,
}

fn caller(who: &Who) -> Result<Caller, CliResponse> {
    match who {
        Who::Owner => Ok(Caller::Person { actor: "owner".into(), owner: true }),
        Who::Shell { project_id, .. } => Ok(Caller::Person { actor: format!("shell:{}", crate::apps::routes::ws_handle(project_id)), owner: true }),
        Who::Login { username, role } if *role >= Role::Admin => {
            Ok(Caller::Person { actor: format!("user:{username}"), owner: *role >= Role::Owner })
        }
        Who::Login { role, .. } => Err(refuse(
            403,
            "role_required",
            format!("the firewall is for an owner or admin login; you are signed in as {}", role.as_wire()),
        )),
        Who::Agent { .. } => Ok(Caller::Agent),
        Who::Wrong => Err(refuse(403, "wrong_credential", "App passes, API keys and API sessions can't use the firewall")),
        Who::Invalid => Err(refuse(403, "forbidden", "invalid or missing token")),
    }
}

fn owner_only(cmd: &str) -> CliResponse {
    refuse(
        403,
        "owner_only",
        format!(
            "the box's firewall is the owner's: no agent opens or narrows it (box-level). The owner runs: {cmd} \
             (or Settings → Network → Custom Domains → Firewall)"
        ),
    )
}

fn audit(act: &str, actor: &str, detail: &str) {
    k2_core::auth_audit::record(
        &k2_core::auth_audit::AuditEvent::new(&format!("firewall-{act}"), actor, detail.to_string(), "loopback", "-", "cli")
            .with_actor(actor),
    );
}

fn body_json(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|e| refuse(400, "bad_json", format!("the body is not JSON: {e}")))
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn save(cfg: &mut fw::FirewallConfig) -> Result<(), CliResponse> {
    cfg.changed_at = Some(chrono::Utc::now().timestamp());
    fw::save(cfg).map_err(|e| refuse(500, "config_write_failed", e))?;
    crate::apps::guard::reload();
    Ok(())
}

fn load() -> Result<fw::FirewallConfig, CliResponse> {
    fw::load_checked().map_err(|e| refuse(500, "firewall_config_unreadable", format!("{e} (fix or remove the file; K2 won't guess)")))
}

fn json_ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn view_or_500(port: u16) -> Result<super::View, CliResponse> {
    super::view(port).map_err(|e| refuse(500, "firewall_config_unreadable", e))
}

/// One route. Blocking (it may read /proc, run systemctl, write files).
pub fn handle(path: &str, who: &Who, is_post: bool, body: &[u8], params: &HashMap<String, String>, port: u16) -> CliResponse {
    if is_post_route(path) && !is_post {
        return refuse(405, "method_not_allowed", "POST required");
    }
    if !is_post_route(path) && is_post {
        return refuse(405, "method_not_allowed", "GET only");
    }
    if body.len() > MAX_BODY {
        return too_large();
    }
    let c = match caller(who) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let r = match path {
        "/cli/firewall" => status(params, port),
        "/cli/firewall/ports" => view_or_500(port).map(|v| json_ok(serde_json::to_value(v).expect("plain struct"))),
        "/cli/firewall/rules" => rules(params, port),
        "/cli/front/passthrough" => crate::front443::passthrough_json().map(json_ok).map_err(front_refusal),
        _ => body_json(body).and_then(|b| post(path, &c, who, &b, port)),
    };
    r.unwrap_or_else(|e| e)
}

fn front_refusal((code, err, msg): crate::front443::Refusal) -> CliResponse {
    refuse(code, err, msg)
}

fn status(params: &HashMap<String, String>, port: u16) -> Result<CliResponse, CliResponse> {
    let s = super::status_json(port).map_err(|e| refuse(500, "firewall_config_unreadable", e))?;
    if params.get("format").map(String::as_str) == Some("text") {
        return Ok(CliResponse::ok_text(super::status_text(&s)));
    }
    Ok(json_ok(s))
}

fn rules(params: &HashMap<String, String>, port: u16) -> Result<CliResponse, CliResponse> {
    let f = params.get("format").map(String::as_str).unwrap_or("nft");
    let fmt = RulesFormat::parse(f).ok_or_else(|| refuse(400, "bad_format", format!("--format nft or ufw (got '{f}')")))?;
    let v = view_or_500(port)?;
    let label = v.box_info.label.clone().map(|l| format!("{l}.k2.dev")).unwrap_or_else(|| "this box".into());
    Ok(CliResponse::ok_text(fw::rules(&v.public, fmt, &label, &v.hash)))
}

fn post(path: &str, c: &Caller, who: &Who, b: &Value, port: u16) -> Result<CliResponse, CliResponse> {
    match path {
        "/cli/firewall/ssh" => ssh(c, b, port),
        "/cli/firewall/open" => open(c, b, port),
        "/cli/firewall/close" => close(c, b, port),
        "/cli/firewall/guard" => guard(c, who, b),
        "/cli/firewall/upstream" => upstream(c, b, port),
        "/cli/front/passthrough/add" => {
            let Caller::Person { actor, .. } = c else {
                return Err(owner_only("k2 front passthrough add <host> --to 127.0.0.1:<port>"));
            };
            let host = b["host"].as_str().unwrap_or("");
            let to = b["to"].as_str().unwrap_or("");
            let r = crate::front443::passthrough_add(host, to, b["proxyV2"] == true, b["label"].as_str().unwrap_or(""), actor, port)
                .map_err(front_refusal)?;
            audit("passthrough-add", actor, &format!("{host} -> {to}"));
            Ok(json_ok(r))
        }
        "/cli/front/passthrough/rm" => {
            let Caller::Person { actor, .. } = c else {
                return Err(owner_only("k2 front passthrough rm <host>"));
            };
            let host = b["host"].as_str().unwrap_or("");
            let r = crate::front443::passthrough_rm(host).map_err(front_refusal)?;
            audit("passthrough-rm", actor, host);
            Ok(json_ok(r))
        }
        _ => Err(refuse(404, "not_found", "no such firewall route")),
    }
}

fn person<'a>(c: &'a Caller, cmd: &str) -> Result<&'a str, CliResponse> {
    match c {
        Caller::Person { actor, .. } => Ok(actor),
        Caller::Agent => Err(owner_only(cmd)),
    }
}

fn changed(port: u16, extra: Value) -> Result<CliResponse, CliResponse> {
    let v = view_or_500(port)?;
    let mut out = json!({"ok": true, "hash": v.hash, "public": v.public});
    if let (Some(o), Value::Object(e)) = (out.as_object_mut(), extra) {
        o.extend(e);
    }
    Ok(json_ok(out))
}

/// `k2 firewall ssh any | allow <cidr>... | port <n>`.
fn ssh(c: &Caller, b: &Value, port: u16) -> Result<CliResponse, CliResponse> {
    let actor = person(c, "k2 firewall ssh any|allow <cidr>...|port <n>")?;
    let mut cfg = load()?;
    let what;
    if b["any"] == true {
        cfg.ssh.allow_from.clear();
        what = "any".to_string();
    } else if b.get("allow").is_some() {
        let list = fw::normalize_allow_from(&str_list(b, "allow")).map_err(|e| refuse(400, "bad_cidr", e))?;
        if list.is_empty() {
            return Err(refuse(
                400,
                "ssh_needs_an_address",
                "name at least one address (k2 firewall ssh allow 198.51.100.0/24); SSH is never closed. For anyone: k2 firewall ssh any",
            ));
        }
        what = format!("allow {}", list.join(","));
        cfg.ssh.allow_from = list;
    } else if let Some(p) = b["port"].as_u64() {
        let p = u16::try_from(p).ok().filter(|p| *p > 0).ok_or_else(|| refuse(400, "bad_port", "1–65535"))?;
        if cfg.open.iter().any(|o| o.port == p && o.proto == Proto::Tcp) {
            return Err(refuse(409, "port_in_use", format!("{p}/tcp is an owner port already (k2 firewall close {p}/tcp first)")));
        }
        cfg.ssh.port = p;
        what = format!("port {p}");
    } else {
        return Err(refuse(400, "bad_request", "one of: {\"any\":true}, {\"allow\":[cidr…]}, {\"port\":n}"));
    }
    save(&mut cfg)?;
    audit("ssh", actor, &what);
    changed(port, json!({"ssh": cfg.ssh, "note": "K2 records the SSH port; it doesn't move sshd"}))
}

/// The K2 sources that own a port in the view now (their rows are
/// read-only: `close` refuses them).
fn k2_owned(port: u16, p: u16, proto: Proto) -> Result<Option<String>, CliResponse> {
    let v = view_or_500(port)?;
    Ok(v.public.iter().chain(v.known_not_public.iter()).find(|r| r.port == p && r.proto == proto && r.source != "owner").map(|r| r.source.clone()))
}

fn port_proto(b: &Value) -> Result<(u16, Proto), CliResponse> {
    let raw = match (&b["port"], b["proto"].as_str()) {
        (Value::Number(n), proto) => format!("{n}/{}", proto.unwrap_or("tcp")),
        (Value::String(s), _) => s.clone(),
        _ => return Err(refuse(400, "bad_port", "name the port: 6000/udp")),
    };
    fw::parse_port_proto(&raw).map_err(|e| refuse(400, "bad_port", e))
}

/// `k2 firewall open <port>/<proto> --label … [--from …]`.
fn open(c: &Caller, b: &Value, port: u16) -> Result<CliResponse, CliResponse> {
    let actor = person(c, "k2 firewall open <port>/<tcp|udp> --label \"<why>\" [--from <cidr>...]")?;
    let (p, proto) = port_proto(b)?;
    let label: String = b["label"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(fw::MAX_LABEL).collect();
    if label.trim().is_empty() {
        return Err(refuse(400, "label_required", "say what it's for: --label \"game server\""));
    }
    let from = fw::normalize_allow_from(&str_list(b, "from")).map_err(|e| refuse(400, "bad_cidr", e))?;
    let mut cfg = load()?;
    if p == cfg.ssh.port && proto == Proto::Tcp {
        return Err(refuse(409, "is_ssh", "that's SSH: use k2 firewall ssh allow|any"));
    }
    if let Some(src) = k2_owned(port, p, proto)? {
        return Err(refuse(409, "k2_owned", format!("{p}/{} is K2's ({src}): K2 opens and closes it with that feature", proto.as_str())));
    }
    cfg.open.retain(|o| !(o.port == p && o.proto == proto));
    if cfg.open.len() >= fw::MAX_OPEN {
        return Err(refuse(409, "too_many", format!("at most {} owner ports", fw::MAX_OPEN)));
    }
    cfg.open.push(OpenPort { port: p, proto, label: label.trim().to_string(), allow_from: from, added_at: chrono::Utc::now().timestamp(), added_by: actor.to_string() });
    cfg.open.sort_by_key(|o| (o.port, o.proto));
    save(&mut cfg)?;
    audit("open", actor, &format!("{p}/{} {label}", proto.as_str()));
    changed(port, json!({"opened": format!("{p}/{}", proto.as_str())}))
}

/// `k2 firewall close <port>/<proto>`: owner rows only (never SSH, never K2's).
fn close(c: &Caller, b: &Value, port: u16) -> Result<CliResponse, CliResponse> {
    let actor = person(c, "k2 firewall close <port>/<tcp|udp>")?;
    let (p, proto) = port_proto(b)?;
    let mut cfg = load()?;
    if p == cfg.ssh.port && proto == Proto::Tcp {
        return Err(refuse(
            409,
            "ssh_stays_open",
            "SSH is never closed (the lockout guard). Narrow it instead: k2 firewall ssh allow <cidr>",
        ));
    }
    let before = cfg.open.len();
    cfg.open.retain(|o| !(o.port == p && o.proto == proto));
    if cfg.open.len() == before {
        if let Some(src) = k2_owned(port, p, proto)? {
            return Err(refuse(409, "k2_owned", format!("{p}/{} is K2's ({src}): turn that feature off to close it", proto.as_str())));
        }
        return Err(refuse(404, "not_open", format!("{p}/{} is not an owner port (k2 firewall)", proto.as_str())));
    }
    save(&mut cfg)?;
    audit("close", actor, &format!("{p}/{}", proto.as_str()));
    changed(port, json!({"closed": format!("{p}/{}", proto.as_str())}))
}

fn limit(b: &Value, key: &str, max: u32) -> Result<Option<u32>, CliResponse> {
    match b.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| refuse(400, "bad_limit", format!("{key} is a number")))?;
            if n < u64::from(fw::MIN_LIMIT) || n > u64::from(max) {
                return Err(refuse(400, "bad_limit", format!("{key}: {}–{max} (to switch it off: off)", fw::MIN_LIMIT)));
            }
            Ok(Some(n as u32))
        }
    }
}

/// `k2 firewall guard [<app>] [on|off] [--conns-per-ip N] [--writes-per-min N] [--reset]`.
fn guard(c: &Caller, who: &Who, b: &Value) -> Result<CliResponse, CliResponse> {
    let on = b.get("on").and_then(Value::as_bool);
    let conns = limit(b, "connsPerIp", fw::MAX_CONNS_PER_IP)?;
    let writes = limit(b, "writesPerMin", fw::MAX_WRITES_PER_MIN)?;
    let reset = b["reset"] == true;
    let mut cfg = load()?;
    let actor: String;
    match b["app"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        None => {
            actor = person(c, "k2 firewall guard [on|off] [--conns-per-ip N] [--writes-per-min N]")?.to_string();
            if reset {
                cfg.guard = fw::GuardConfig { apps: std::mem::take(&mut cfg.guard.apps), ..Default::default() };
            }
            if let Some(o) = on {
                cfg.guard.enabled = o;
            }
            if let Some(n) = conns {
                cfg.guard.conns_per_ip = n;
            }
            if let Some(n) = writes {
                cfg.guard.writes_per_min = n;
            }
        }
        Some(app) => {
            if conns.is_some() {
                return Err(refuse(400, "box_wide", "connections per visitor are box-wide (K2 counts them before it knows the App): leave out the App"));
            }
            let row = crate::apps::routes::app_for(who, Some(app)).map_err(|r| refuse(r.status, r.code, r.hint))?;
            let id = row.app_id.clone();
            let cur = cfg.guard.apps.get(&id).cloned().unwrap_or_default();
            let next = if reset { AppGuard::default() } else { AppGuard { enabled: on.or(cur.enabled), writes_per_min: writes.or(cur.writes_per_min) } };
            match c {
                Caller::Person { actor: a, .. } => actor = a.clone(),
                Caller::Agent => {
                    // The App's own agent may tighten (on, a lower limit),
                    // never loosen (off, a higher limit, a reset to looser).
                    crate::apps::routes::need_build(who).map_err(|r| refuse(r.status, r.code, r.hint))?;
                    let mut probe = cfg.guard.clone();
                    let before = probe.writes_for(&id);
                    probe.apps.insert(id.clone(), next.clone());
                    let after = probe.writes_for(&id);
                    let tighter = match (before, after) {
                        (_, None) => before.is_none(),
                        (None, Some(_)) => true,
                        (Some(x), Some(y)) => y <= x,
                    };
                    if !tighter {
                        return Err(owner_only(&format!("k2 firewall guard {app} …")));
                    }
                    actor = crate::apps::routes::actor(who);
                }
            }
            cfg.guard.apps.insert(id, next);
            cfg.guard.apps.retain(|_, g| g.enabled.is_some() || g.writes_per_min.is_some());
        }
    }
    save(&mut cfg)?;
    audit("guard", &actor, &serde_json::to_string(b).unwrap_or_default());
    Ok(json_ok(json!({"ok": true, "guard": cfg.guard, "counters": crate::apps::guard::counters_json()})))
}

/// K2 Web's applied-ack (KW-Q3) and the `managedBy` record (KW-Q2). The
/// owner's: the owner token (K2 Web runs it as the daemon user over SSH),
/// a shell tab, or an Owner login.
fn upstream(c: &Caller, b: &Value, port: u16) -> Result<CliResponse, CliResponse> {
    let actor = match c {
        Caller::Person { actor, owner: true } => actor.clone(),
        Caller::Person { .. } => return Err(refuse(403, "owner_only", "the applied record is the owner's (K2 Web runs it as the box's daemon user)")),
        Caller::Agent => return Err(owner_only("k2 firewall upstream --hash <hash> --provider <name> --applied-at <unix>")),
    };
    let mut cfg = load()?;
    let mut did: Vec<String> = Vec::new();
    if let Some(m) = b.get("managedBy").filter(|v| !v.is_null()) {
        match m.as_str() {
            Some("k2") | Some("owner") => cfg.managed_by = m.as_str().map(str::to_string),
            Some("auto") => cfg.managed_by = None,
            _ => return Err(refuse(400, "bad_managed_by", "managedBy: k2, owner or auto")),
        }
        did.push(format!("managedBy {}", m.as_str().unwrap_or("")));
    }
    if b.get("hash").is_some() {
        let hash = b["hash"].as_str().unwrap_or("").trim().to_string();
        if !(hash.starts_with("sha256:") && hash.len() == 71 && hash[7..].bytes().all(|c| c.is_ascii_hexdigit())) {
            return Err(refuse(400, "bad_hash", "hash: the view's sha256:<64 hex>"));
        }
        let provider: String = b["provider"].as_str().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || "-_. ".contains(*c)).take(40).collect();
        if provider.trim().is_empty() {
            return Err(refuse(400, "provider_required", "provider: where it was applied (e.g. hetzner)"));
        }
        let now = chrono::Utc::now().timestamp();
        let applied_at = b["appliedAt"].as_i64().unwrap_or(now);
        if applied_at <= 0 || applied_at > now + 300 {
            return Err(refuse(400, "bad_applied_at", "appliedAt: a unix time, not in the future"));
        }
        did.push(format!("applied {hash} at {provider}"));
        cfg.upstream = Some(UpstreamAck { hash, provider: provider.trim().to_string(), applied_at, recorded_at: now, by: actor.clone() });
    }
    if did.is_empty() {
        return Err(refuse(400, "bad_request", "{hash, provider, appliedAt} and/or {managedBy}"));
    }
    save(&mut cfg)?;
    audit("upstream", &actor, &did.join("; "));
    let v = view_or_500(port)?;
    Ok(json_ok(json!({"ok": true, "upstream": v.upstream, "hash": v.hash})))
}
