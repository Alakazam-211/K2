//! The verbs `cmd_vault`'s own flag parser reads (`cli/src/migrate.sh`):
//! [`parse`] is the parser and each verb's flag check, then [`run`]
//! answers the verbs in [`NATIVE`] as the shell does, request and printer
//! (`cli/src/migrate.sh`, `cli/src/vault.sh`). The verbs not in it are
//! the shell's still ([`native`]).

use super::py::{
    self, cat, get, get_or, idx, is, items, left, or_list, or_obj, render, s, t, Lines, R,
};
use super::Ctx;
use crate::confirm::{self, dumps, Split};
use crate::exit::Exit;
use crate::families::Shown;
use crate::out;
use crate::out::json::{Text, Value};
use crate::prompt;
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};

/// The verbs answered here; every other verb of this parser is the
/// shell's.
pub const NATIVE: &[&str] = &[
    "status",
    "list",
    "ls",
    "show",
    "set",
    "rotate",
    "hosts",
    "sign",
    "public-key",
    "agent-limit",
    "delivery",
    "revoke",
    "open",
    "disable",
    "enable",
    "destroy",
    "get",
    "audit",
    "requests",
    "presets",
    "doctor",
    "proxy-url",
    "recipient",
    "file",
    "env-file",
    "grant",
    "service",
];

/// Whether the binary answers `vault <sub> <rest>` (no I/O).
pub fn native(sub: &str, _rest: &[&str]) -> bool {
    NATIVE.contains(&sub)
}

/// `cmd_vault`'s locals after its loop.
#[derive(Debug, Default, Clone)]
pub struct Opts {
    pub json: bool,
    pub system: bool,
    pub yes: bool,
    pub remove_original: bool,
    pub own_box_move: bool,
    pub transcripts: bool,
    pub dry_run: bool,
    pub replace: bool,
    pub apply: bool,
    pub accept_agent: bool,
    pub allow_fwd: bool,
    pub no_fwd: bool,
    pub burst: String,
    pub per_minute: String,
    pub recipient: String,
    pub target: String,
    pub target_ws: String,
    pub workspaces: Vec<String>,
    pub servers: Vec<String>,
    pub out_file: String,
    pub holder: String,
    pub secrets: Vec<String>,
    pub sign_scheme: String,
    pub sign_header: String,
    pub sign_encoding: String,
    pub sign_ts_header: String,
    pub oauth_token_url: String,
    pub oauth_client_auth: String,
    pub sign_prefix: Option<String>,
    pub sign_template: Option<String>,
    pub sign_value: Option<String>,
    pub env_file: String,
    pub wait: String,
    pub done: String,
    pub scope: String,
    pub kind: String,
    pub hosts: String,
    pub preset: String,
    pub delivery: String,
    pub ceiling: String,
    pub describe: String,
    pub rotate_every: String,
    pub ttl: String,
    pub inject_header: String,
    pub inject_format: String,
    pub inject_dst: String,
    pub to: String,
    pub to_version: String,
    pub from: String,
    pub as_d: String,
    pub env_name: String,
    pub file_name: String,
    pub grant: String,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub since: String,
    pub pos: Vec<String>,
    pub seen: Vec<String>,
}

/// One `usage:` (or other) line and exit 2.
fn usage(line: String) -> Exit {
    out::stderr_line(&line);
    Exit::Usage
}

/// `cmd_vault`'s loop and each verb's flag check: the options, or the
/// line the shell ends with (exit 2, printed).
pub fn parse(sub: &str, rest: &[&str]) -> Result<Opts, Exit> {
    let mut o = Opts::default();
    let mut i = 0;
    let value = |i: usize| rest.get(i + 1).copied().unwrap_or("");
    while i < rest.len() {
        let w = rest[i];
        let mut seen = true;
        let mut step = 1;
        match w {
            "--json" => o.json = true,
            "--system" => {
                if !matches!(sub, "list" | "ls") {
                    return Err(usage(format!(
                        "usage: only {CLI} vault list takes --system"
                    )));
                }
                o.system = true;
            }
            "--yes" => o.yes = true,
            "--remove-original" => o.remove_original = true,
            "--own-box-move" => o.own_box_move = true,
            "--transcripts" => {
                if sub != "doctor" {
                    return Err(usage(format!(
                        "usage: --transcripts goes with {CLI} vault doctor"
                    )));
                }
                o.transcripts = true;
            }
            "--dry-run" => o.dry_run = true,
            "--replace" => o.replace = true,
            "--apply" => o.apply = true,
            "--accept-agent-value" => o.accept_agent = true,
            "--allow-forwarded" => o.allow_fwd = true,
            "--no-forwarded" => o.no_fwd = true,
            "--burst" | "--per-minute" => {
                let v = value(i);
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(usage(format!("usage: {w} needs a number")));
                }
                if w == "--burst" {
                    o.burst = v.into();
                } else {
                    o.per_minute = v.into();
                }
                step = 2;
            }
            "--recipient"
            | "--target"
            | "--target-workspace"
            | "--workspace"
            | "--server"
            | "--out"
            | "--holder"
            | "--sign"
            | "--sign-header"
            | "--sign-encoding"
            | "--sign-ts-header"
            | "--oauth-token-url"
            | "--oauth-client-auth"
            | "--scope"
            | "--kind"
            | "--for"
            | "--preset"
            | "--delivery"
            | "--ceiling"
            | "--describe"
            | "--rotate-every"
            | "--ttl"
            | "--inject-header"
            | "--inject-format"
            | "--inject-dst"
            | "--to"
            | "--from"
            | "--as"
            | "--env-name"
            | "--file-name"
            | "--grant"
            | "--add"
            | "--remove"
            | "--since" => {
                let v = value(i).to_string();
                if v.is_empty() {
                    return Err(usage(format!("usage: {w} needs a value")));
                }
                match w {
                    "--recipient" => o.recipient = v,
                    "--target" => o.target = v,
                    "--target-workspace" => o.target_ws = v,
                    "--workspace" => o.workspaces.push(v),
                    "--server" => o.servers.push(v),
                    "--out" => o.out_file = v,
                    "--holder" => o.holder = v,
                    "--sign" => o.sign_scheme = v,
                    "--sign-header" => o.sign_header = v,
                    "--sign-encoding" => o.sign_encoding = v,
                    "--sign-ts-header" => o.sign_ts_header = v,
                    "--oauth-token-url" => o.oauth_token_url = v,
                    "--oauth-client-auth" => o.oauth_client_auth = v,
                    "--scope" => o.scope = v,
                    "--kind" => o.kind = v,
                    "--for" => o.hosts = v,
                    "--preset" => o.preset = v,
                    "--delivery" => o.delivery = v,
                    "--ceiling" => o.ceiling = v,
                    "--describe" => o.describe = v,
                    "--rotate-every" => o.rotate_every = v,
                    "--ttl" => o.ttl = v,
                    "--inject-header" => o.inject_header = v,
                    "--inject-format" => o.inject_format = v,
                    "--inject-dst" => o.inject_dst = v,
                    "--to" if sub == "rollback" => o.to_version = v,
                    "--to" => o.to = v,
                    "--from" => o.from = v,
                    "--as" => o.as_d = v,
                    "--env-name" => o.env_name = v,
                    "--file-name" => o.file_name = v,
                    "--grant" => o.grant = v,
                    "--add" => o.add.push(v),
                    "--remove" => o.remove.push(v),
                    _ => o.since = v,
                }
                step = 2;
            }
            "--secret" => {
                let v = value(i);
                if v.is_empty() {
                    return Err(usage("usage: --secret NAME".into()));
                }
                o.secrets.push(v.into());
                step = 2;
            }
            "--sign-prefix" | "--sign-template" | "--sign-value" => {
                // May be empty: only the count is checked.
                let Some(v) = rest.get(i + 1) else {
                    return Err(usage(format!("usage: {w} needs a value")));
                };
                let v = Some(v.to_string());
                match w {
                    "--sign-prefix" => o.sign_prefix = v,
                    "--sign-template" => o.sign_template = v,
                    _ => o.sign_value = v,
                }
                step = 2;
            }
            "--env-file" | "--wait" | "--done" => {
                let v = value(i).to_string();
                if v.is_empty() {
                    return Err(usage(match w {
                        "--env-file" => "usage: --env-file <file>".into(),
                        "--wait" => "usage: --wait <duration> (10m, 2h)".into(),
                        _ => "usage: --done <path>".into(),
                    }));
                }
                match w {
                    "--env-file" => o.env_file = v,
                    "--wait" => o.wait = v,
                    _ => o.done = v,
                }
                step = 2;
            }
            _ if w == "--value" || w.starts_with("--value=") || w.starts_with("--secret-value") => {
                return Err(usage(format!(
                    "value_on_argv: values are read from stdin, never an argument ({CLI} vault set NAME < file)"
                )));
            }
            _ if w.starts_with("--") => {
                return Err(usage(format!(
                    "usage: unknown flag '{w}' ({CLI} vault --help)"
                )));
            }
            _ => {
                o.pos.push(w.into());
                seen = false;
            }
        }
        if seen {
            o.seen.push(w.into());
        }
        i += step;
    }
    let (takes, also, words) = takes(sub, o.pos.first().map(String::as_str));
    for f in &o.seen {
        let all = format!(" {takes} {also} ");
        if all.contains(&format!(" {f} ")) {
            continue;
        }
        return Err(usage(if takes.is_empty() {
            format!("usage: {CLI} vault {words} takes no flags, not {f} ({CLI} vault {sub} --help)")
        } else {
            format!(
                "usage: {CLI} vault {words} doesn't take {f} (it takes: {}; {CLI} vault {sub} --help)",
                takes.replace(' ', ", ")
            )
        }));
    }
    Ok(o)
}

/// Each verb's `takes`, its `also` and the words its line names it by.
pub fn takes(sub: &str, first: Option<&str>) -> (&'static str, &'static str, String) {
    let set = "--scope --kind --for --preset --delivery --ceiling --describe --rotate-every --ttl --inject-dst --inject-header --inject-format --sign --sign-header --sign-encoding --sign-prefix --sign-ts-header --sign-template --sign-value --oauth-token-url --oauth-client-auth --json";
    let sign = "--sign-header --sign-encoding --sign-prefix --sign-ts-header --sign-template --sign-value --oauth-token-url --oauth-client-auth --scope --json";
    let (t, a) = match sub {
        "status" | "open" | "recipient" | "requests" | "presets" => ("--json", ""),
        "list" | "ls" => ("--scope --system --json", ""),
        "show" | "rotate" | "public-key" | "disable" | "enable" | "get" => ("--scope --json", ""),
        "set" => (set, ""),
        "hosts" => ("--add --remove --scope --json", ""),
        "sign" => (sign, "--sign"),
        "agent-limit" => (
            "--burst --per-minute --allow-forwarded --no-forwarded --scope --json",
            "",
        ),
        "delivery" => ("--ceiling --scope --json", "--env-name --file-name"),
        "grant" => ("--to --as --env-name --file-name --ttl --scope --json", ""),
        "revoke" => ("--from --grant --scope --json", ""),
        "destroy" => ("--yes --scope --json", ""),
        "audit" => ("--since --scope --json", ""),
        "doctor" => ("--transcripts --json", ""),
        "env-file" | "file" => ("--secret --scope --ttl --wait --done --json", ""),
        "proxy-url" => ("--scope --ttl --json", "--secret"),
        "import" => ("--scope --delivery --remove-original --json", ""),
        "service" => ("--env-file --json", ""),
        "reveal" => ("--scope", ""),
        "rollback" => ("--to --scope --accept-agent-value", ""),
        "restore" => ("--from --holder --dry-run --replace", ""),
        "carry" => match first {
            Some("export") => (
                "--recipient --target --workspace --target-workspace --server --own-box-move --out",
                "",
            ),
            Some("import") => ("--workspace --apply --json", ""),
            _ => ("", ""),
        },
        _ => ("", ""),
    };
    let words = match (sub, first) {
        ("carry", Some(w @ ("export" | "import"))) => format!("carry {w}"),
        _ => sub.to_string(),
    };
    (t, a, words)
}

/// `_vault_scope_json`: the scope's fields, or its usage line (exit 2).
pub fn scope_json(s: &str) -> Result<Vec<(Text, Value)>, Exit> {
    let f = |k: &str, v: &str| (Text::from(k), py::text(v));
    if let Some(ws) = s
        .strip_prefix("ws:")
        .or_else(|| s.strip_prefix("workspace:"))
    {
        Ok(vec![f("scope", "workspace"), f("workspace", ws)])
    } else if matches!(s, "server" | "session" | "workspace") {
        Ok(vec![f("scope", s)])
    } else if let Some(id) = s.strip_prefix("session:") {
        Ok(vec![f("scope", "session"), f("session", id)])
    } else if s.starts_with("engine:") {
        Ok(vec![f("scope", s)])
    } else if s.is_empty() {
        Ok(Vec::new())
    } else {
        Err(usage(
            "usage: --scope ws:<name>|server|session|engine:<engine>".into(),
        ))
    }
}

/// `_vault_merge a b …`: `o.update(x)` for each, in order.
pub fn merge(parts: &[&[(Text, Value)]]) -> Vec<(Text, Value)> {
    let mut o: Vec<(Text, Value)> = Vec::new();
    for p in parts {
        for (k, v) in p.iter() {
            match o.iter_mut().find(|(ok, _)| ok == k) {
                Some((_, ov)) => *ov = v.clone(),
                None => o.push((k.clone(), v.clone())),
            }
        }
    }
    o
}

fn obj(o: Vec<(Text, Value)>) -> String {
    dumps(&Value::Object(o))
}

fn kv(k: &str, v: Value) -> (Text, Value) {
    (Text::from(k), v)
}

/// `printf '%s\n' "$_SC_BODY"`, exit 0.
fn print_body(s: &Split) -> Exit {
    let mut b = s.body.clone();
    b.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &b) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `echo "<line>"`.
fn echo(line: &str) -> Exit {
    Shown::out(format!("{line}\n")).print()
}

/// The verb. Called only for a form [`native`] takes.
pub fn run(ctx: &mut Ctx, sub: &str, rest: &[&str]) -> Exit {
    match verb(ctx, sub, rest) {
        Ok(e) | Err(e) => e,
    }
}

fn verb(ctx: &mut Ctx, sub: &str, rest: &[&str]) -> Result<Exit, Exit> {
    let o = parse(sub, rest)?;
    let sj = scope_json(&o.scope)?;
    let name = o.pos.first().cloned().unwrap_or_default();
    let nj: Vec<(Text, Value)> = if name.is_empty() {
        Vec::new()
    } else {
        vec![kv("name", py::text(&name))]
    };
    let need = |ok: bool, line: String| if ok { Ok(()) } else { Err(usage(line)) };
    let shown = |sp: &Split, f: fn(&Value, &mut Lines) -> R<i32>| -> Exit {
        if o.json {
            print_body(sp)
        } else {
            render(&sp.body, f).print()
        }
    };
    Ok(match sub {
        "status" => {
            let r = ctx.call(false, "status", "")?;
            shown(&r, status)
        }
        "list" | "ls" => {
            let mut q = String::new();
            if o.system {
                q.push_str("system=1");
            }
            if !o.scope.is_empty() {
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str(&py::urlencode(&sj));
            }
            let r = ctx.call(false, "list", &q)?;
            shown(&r, list)
        }
        "show" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault show NAME [--scope <s>]"),
            )?;
            let r = ctx.call(false, "show", &py::urlencode(&merge(&[&sj, &nj])))?;
            shown(&r, show)
        }
        "set" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault set NAME [options] < value"),
            )?;
            need(
                o.pos.len() == 1,
                format!("value_on_argv: values are read from stdin, never an argument ({CLI} vault set NAME < file)"),
            )?;
            let sign = sign_json(
                &o.sign_scheme,
                &o.sign_header,
                &o.sign_encoding,
                &o,
                &o.oauth_token_url,
                &o.oauth_client_auth,
            )?;
            let opts = set_opts(&o, sign)?;
            let base = merge(&[&sj, &nj, &opts]);
            let body = body_with_value(base)?;
            let r = ctx.call(true, "set", body.as_str())?;
            drop(body);
            shown(&r, set_printer)
        }
        "rotate" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault rotate NAME < value"),
            )?;
            need(
                o.pos.len() == 1,
                "value_on_argv: values are read from stdin, never an argument".into(),
            )?;
            let body = body_with_value(merge(&[&sj, &nj]))?;
            let r = ctx.call(true, "rotate", body.as_str())?;
            drop(body);
            shown(&r, rotate_printer)
        }
        "hosts" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault hosts NAME --add <host> | --remove <host>"),
            )?;
            // `printf '%s\n' "${hosts[@]}"`, split on newlines, empties out.
            let list = |v: &[String]| {
                Value::Array(
                    v.iter()
                        .flat_map(|x| x.split('\n'))
                        .filter(|x| !x.is_empty())
                        .map(py::text)
                        .collect(),
                )
            };
            let hj = vec![
                kv("addHosts", list(&o.add)),
                kv("removeHosts", list(&o.remove)),
            ];
            let r = ctx.call(true, "policy", &obj(merge(&[&sj, &nj, &hj])))?;
            shown(&r, |v, l| {
                let m = or_obj(get(v, "secret")?);
                l.p(format!(
                    "{} sites: {}",
                    s(&get(&m, "name")?),
                    joined_or(&get(&m, "hosts")?, ", ", "none")?
                ));
                Ok(0)
            })
        }
        "sign" => {
            let scheme = o.pos.get(1).cloned().unwrap_or_default();
            need(
                !name.is_empty() && !scheme.is_empty(),
                format!("usage: {CLI} vault sign NAME ovh|hmac-sha256|hmac-sha256-ts|oauth-refresh|none [--sign-header …] ({CLI} vault sign --help)"),
            )?;
            need(
                o.sign_scheme.is_empty(),
                format!(
                    "usage: the scheme is the second word: {CLI} vault sign NAME {}",
                    o.sign_scheme
                ),
            )?;
            let gj = if scheme == "none" {
                let any = !(o.sign_header.is_empty()
                    && o.sign_encoding.is_empty()
                    && o.sign_ts_header.is_empty()
                    && o.oauth_token_url.is_empty()
                    && o.oauth_client_auth.is_empty())
                    || o.sign_prefix.is_some()
                    || o.sign_template.is_some()
                    || o.sign_value.is_some();
                need(
                    !any,
                    format!("usage: {CLI} vault sign NAME none takes no --sign-* options"),
                )?;
                vec![kv("sign", py::text("none"))]
            } else {
                let sg = sign_json(
                    &scheme,
                    &o.sign_header,
                    &o.sign_encoding,
                    &o,
                    &o.oauth_token_url,
                    &o.oauth_client_auth,
                )?
                .unwrap_or(Value::Null);
                vec![kv("sign", sg)]
            };
            let r = ctx.call(true, "policy", &obj(merge(&[&sj, &nj, &gj])))?;
            shown(&r, |v, l| {
                let m = or_obj(get(v, "secret")?);
                let sg = get(&or_obj(get(&m, "inject")?), "sign")?;
                let what = if t(&sg) {
                    cat("signs requests: ", &get(&sg, "scheme")?)?
                } else {
                    "no signer (fetch adds the value as set)".into()
                };
                l.p(format!("{}: {what}", s(&get(&m, "name")?)));
                Ok(0)
            })
        }
        "public-key" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault public-key NAME [--scope <s>]"),
            )?;
            let r = ctx.call(false, "public-key", &py::urlencode(&merge(&[&sj, &nj])))?;
            shown(&r, |v, l| {
                l.p(s(&idx(v, "publicKey")?));
                l.e(format!("# {}", s(&get_or(v, "fingerprint", py::text(""))?)));
                Ok(0)
            })
        }
        "agent-limit" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault agent-limit NAME [--burst N] [--per-minute M] [--allow-forwarded|--no-forwarded]"),
            )?;
            need(
                !(o.allow_fwd && o.no_fwd),
                "usage: give --allow-forwarded or --no-forwarded, not both".into(),
            )?;
            let mut aj = Vec::new();
            if !o.burst.is_empty() {
                aj.push(kv("burst", Value::Int(int_text(&o.burst))));
            }
            if !o.per_minute.is_empty() {
                aj.push(kv("perMinute", Value::Int(int_text(&o.per_minute))));
            }
            if o.allow_fwd {
                aj.push(kv("allowForwarded", Value::Bool(true)));
            }
            if o.no_fwd {
                aj.push(kv("allowForwarded", Value::Bool(false)));
            }
            let r = ctx.call(true, "agent-policy", &obj(merge(&[&sj, &nj, &aj])))?;
            shown(&r, |v, l| {
                let m = or_obj(get(v, "secret")?);
                let p = or_obj(get(v, "agentPolicy")?);
                l.p(format!(
                    "{}: at most {} signatures a second, {} a minute per session; forwarded agents (ssh -A) {}",
                    s(&get(&m, "name")?),
                    s(&get(&p, "burst")?),
                    s(&get(&p, "perMinute")?),
                    if t(&get(&p, "allowForwarded")?) { "allowed" } else { "refused" }
                ));
                Ok(0)
            })
        }
        "delivery" => {
            let d = o.pos.get(1).cloned().unwrap_or_default();
            need(
                !name.is_empty() && !d.is_empty(),
                format!("usage: {CLI} vault delivery NAME fetch|agent|proxy|file|env|api [--ceiling <d>]"),
            )?;
            if !o.env_name.is_empty() || !o.file_name.is_empty() {
                let (flag, val, as_) = if o.env_name.is_empty() {
                    ("--file-name", &o.file_name, "file")
                } else {
                    ("--env-name", &o.env_name, "env")
                };
                out::stderr_line(&format!(
                    "usage: {CLI} vault delivery doesn't take {flag}; a secret has no {as_} name of its own. Name it on a grant:"
                ));
                return Err(usage(format!(
                    "  {CLI} vault grant {name} --to ws:<name> --as {as_} {flag} {val}"
                )));
            }
            let mut dj = vec![kv("delivery", py::text(&d))];
            if !o.ceiling.is_empty() {
                dj.push(kv("ceiling", py::text(&o.ceiling)));
            }
            let r = ctx.call(true, "policy", &obj(merge(&[&sj, &nj, &dj])))?;
            shown(&r, |v, l| {
                let m = or_obj(get(v, "secret")?);
                l.p(format!(
                    "{}: {} (at most {})",
                    s(&get(&m, "name")?),
                    s(&get(&m, "defaultDelivery")?),
                    s(&get(&m, "deliveryCeiling")?)
                ));
                Ok(0)
            })
        }
        "revoke" => {
            let mut rj = Vec::new();
            if !o.grant.is_empty() {
                rj.push(kv("grantId", py::text(&o.grant)));
            }
            if !o.from.is_empty() {
                rj.push(kv("from", py::text(&o.from)));
            }
            let r = ctx.call(true, "revoke", &obj(merge(&[&sj, &nj, &rj])))?;
            if o.json {
                print_body(&r)
            } else {
                let first = if o.grant.is_empty() {
                    format!(
                        "{name}: no longer shared with {}; the secret itself is still in the vault",
                        o.from
                    )
                } else {
                    format!(
                        "share {} removed; the secret itself is still in the vault",
                        o.grant
                    )
                };
                let e = echo(&first);
                if e != Exit::Ok {
                    return Ok(e);
                }
                let e = recall(&r);
                if e != Exit::Ok {
                    return Ok(e);
                }
                render(&r.body, |v, l| {
                    for j in items(&or_list(get(v, "stillRunning")?))? {
                        let sh = s(&idx(&j, "short")?);
                        l.p(format!(
                            "still running with it: job {sh} on {} ({}, {}); revoking stops its next attempt, not this one ({CLI} compute cancel {sh})",
                            s(&idx(&j, "node")?),
                            s(&idx(&j, "env")?),
                            s(&idx(&j, "state")?)
                        ));
                    }
                    Ok(0)
                })
                .print()
            }
        }
        "open" => {
            let r = ctx.call(true, "open", "{}")?;
            if o.json {
                print_body(&r)
            } else {
                echo("the vault is open")
            }
        }
        "disable" | "enable" => {
            need(!name.is_empty(), format!("usage: {CLI} vault {sub} NAME"))?;
            let r = ctx.call(true, sub, &obj(merge(&[&sj, &nj])))?;
            if o.json {
                print_body(&r)
            } else {
                let e = echo(&format!("{name}: {sub}d"));
                if e != Exit::Ok {
                    return Ok(e);
                }
                recall(&r)
            }
        }
        "destroy" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault destroy NAME --yes"),
            )?;
            let yj = if o.yes {
                vec![kv("yes", Value::Bool(true))]
            } else {
                Vec::new()
            };
            let r = ctx.call(true, "destroy", &obj(merge(&[&sj, &nj, &yj])))?;
            if o.json {
                print_body(&r)
            } else {
                let e = echo(&format!(
                    "{name}: destroyed on this box (backups made earlier keep it until they expire)"
                ));
                if e != Exit::Ok {
                    return Ok(e);
                }
                recall(&r)
            }
        }
        "get" => {
            need(!name.is_empty(), format!("usage: {CLI} vault get NAME"))?;
            let r = ctx.call(true, "get", &obj(merge(&[&sj, &nj])))?;
            if o.json {
                print_body(&r)
            } else {
                value_out(&r.body, false)
            }
        }
        "audit" => {
            let mut q = String::new();
            if !name.is_empty() {
                q = py::urlencode(&merge(&[&sj, &nj]));
            }
            if !o.since.is_empty() {
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str("since=");
                q.push_str(&crate::transport::encode::quote(&o.since));
            }
            let r = ctx.call(false, "audit", &q)?;
            shown(&r, |v, l| {
                for a in items(&or_list(get(v, "audit")?))? {
                    let th = get(&a, "targetHost")?;
                    l.p(format!(
                        "{} {} {} {} {}{}",
                        s(&get(&a, "ts")?),
                        left(&get(&a, "action")?, 8),
                        left(&get(&a, "outcome")?, 28),
                        left(&get(&a, "actor")?, 22),
                        if t(&th) { s(&th) } else { String::new() },
                        times(&a)?
                    ));
                }
                Ok(0)
            })
        }
        "requests" => {
            let r = ctx.call(false, "requests", "")?;
            shown(&r, |v, l| {
                let r = items(&or_list(get(v, "requests")?))?;
                if r.is_empty() {
                    l.p("no secret cards yet");
                }
                for x in r {
                    let i = or_obj(get(&x, "inject")?);
                    let how = if t(&i) {
                        format!("  sends {} {}", s(&get(&i, "dst")?), s(&get(&i, "fmt")?))
                    } else {
                        String::new()
                    };
                    l.p(format!(
                        "{} {} {} asked by {}{how}",
                        s(&get(&x, "createdAt")?),
                        left(&get(&x, "status")?, 8),
                        left(&get(&x, "name")?, 24),
                        s(&get(&x, "requestedBy")?)
                    ));
                }
                Ok(0)
            })
        }
        "presets" => {
            let r = ctx.call(false, "presets", "")?;
            shown(&r, |v, l| {
                for p in items(&or_list(get(v, "presets")?))? {
                    let sign = get(&p, "sign")?;
                    let how = if t(&sign) {
                        cat("signs: ", &idx(&p, "sign")?)?
                    } else {
                        let dst = idx(&p, "dst")?;
                        let fmt = idx(&p, "fmt")?;
                        let a = cat("", &dst)?;
                        let b = cat(" ", &fmt)?;
                        a + &b
                    };
                    let hosts = joined(&idx(&p, "hosts")?, ",")?;
                    let hosts = if hosts.is_empty() {
                        "(your project host)".to_string()
                    } else {
                        hosts
                    };
                    l.p(format!(
                        "{} {} {} {how}",
                        left(&idx(&p, "id")?, 10),
                        left(&idx(&p, "label")?, 16),
                        left(&py::text(&hosts), 48)
                    ));
                }
                Ok(0)
            })
        }
        "doctor" => {
            let q = if o.transcripts { "transcripts=1" } else { "" };
            let r = ctx.call(false, "doctor", q)?;
            shown(&r, doctor)
        }
        "proxy-url" => {
            let psecret = o.secrets.first().cloned().unwrap_or_else(|| name.clone());
            need(
                o.secrets.len() <= 1 && !psecret.is_empty(),
                format!("usage: {CLI} vault proxy-url NAME [--ttl 2h]  (one secret)"),
            )?;
            let mut pj = vec![kv("secret", py::text(&psecret))];
            if !o.ttl.is_empty() {
                pj.push(kv("ttl", py::text(&o.ttl)));
            }
            let r = ctx.call(true, "proxy-url", &obj(merge(&[&pj, &sj])))?;
            shown(&r, |v, l| {
                l.p(format!(
                    "{}={}",
                    s(&idx(v, "envName")?),
                    s(&idx(v, "placeholder")?)
                ));
                l.p(format!(
                    "{}={}",
                    s(&idx(v, "baseUrlEnv")?),
                    s(&idx(v, "baseUrl")?)
                ));
                Ok(0)
            })
        }
        "recipient" => {
            let r = ctx.call(false, "recipient", "")?;
            shown(&r, |v, l| {
                let r = s(&idx(v, "recipient")?);
                let id = s(&idx(v, "vaultId")?);
                l.p(format!("recipient: {r}"));
                l.p(format!("vault id:  {id}"));
                l.p(format!(
                    "On the old box: {CLI} vault carry export --recipient {r} --target {id} --workspace <ws> --out carry.age"
                ));
                Ok(0)
            })
        }
        "file" | "env-file" => {
            let mut j = Vec::new();
            if sub == "env-file" {
                need(
                    o.wait.is_empty() || o.done.is_empty(),
                    "usage: --wait doesn't go with --done".into(),
                )?;
                let efs: Vec<&String> = o.pos.iter().chain(o.secrets.iter()).collect();
                need(
                    !efs.is_empty() || !o.done.is_empty(),
                    format!(
                        "usage: {CLI} vault env-file NAME [NAME2 …] [--ttl 90s] | --done <path>"
                    ),
                )?;
                if !o.done.is_empty() {
                    j.push(kv("done", py::text(&o.done)));
                }
                if !o.ttl.is_empty() {
                    j.push(kv("ttl", py::text(&o.ttl)));
                }
                if !o.wait.is_empty() {
                    j.push(kv("wait", py::text(&o.wait)));
                }
                j.push(kv(
                    "secrets",
                    Value::Array(
                        efs.iter()
                            .filter(|x| !x.is_empty())
                            .map(|x| py::text(x))
                            .collect(),
                    ),
                ));
            } else {
                let fsecret = o.secrets.first().cloned().unwrap_or_else(|| name.clone());
                need(
                    o.secrets.len() <= 1,
                    format!("usage: {CLI} vault file NAME [--ttl 90s]  (one secret per file)"),
                )?;
                need(
                    !fsecret.is_empty() || !o.done.is_empty(),
                    format!("usage: {CLI} vault file NAME [--ttl 90s] | --done <path>"),
                )?;
                need(
                    o.wait.is_empty() || o.done.is_empty(),
                    "usage: --wait doesn't go with --done".into(),
                )?;
                if !o.done.is_empty() {
                    j.push(kv("done", py::text(&o.done)));
                }
                if !o.ttl.is_empty() {
                    j.push(kv("ttl", py::text(&o.ttl)));
                }
                if !fsecret.is_empty() && o.done.is_empty() {
                    j.push(kv("secret", py::text(&fsecret)));
                }
                if !o.wait.is_empty() {
                    j.push(kv("wait", py::text(&o.wait)));
                }
            }
            let r = wait_call(ctx, sub, merge(&[&j, &sj]))?;
            shown(&r, |v, l| {
                let p = get(v, "path")?;
                l.p(if t(&p) {
                    s(&p)
                } else {
                    format!("removed {}", s(&get(v, "removed")?))
                });
                Ok(0)
            })
        }
        "grant" => grant(ctx, &o, &name, &sj, &nj)?,
        "service" => {
            need(
                !name.is_empty(),
                format!("usage: {CLI} vault service <published service> [clean|legacy] [--env-file .env.k2]"),
            )?;
            let mode = o.pos.get(1).cloned().unwrap_or_default();
            let r = if !mode.is_empty() || !o.env_file.is_empty() {
                let mut j = vec![kv("service", py::text(&name))];
                if !mode.is_empty() {
                    j.push(kv("mode", py::text(&mode)));
                }
                if !o.env_file.is_empty() {
                    j.push(kv("envFile", py::text(&o.env_file)));
                }
                ctx.call(true, "service", &obj(j))?
            } else {
                ctx.call(
                    false,
                    "service",
                    &format!("service={}", crate::transport::encode::quote(&name)),
                )?
            };
            shown(&r, service)
        }
        _ => Exit::Error,
    })
}

/// `_vault_wait_call <route> <body>`: one blocking call, then again until
/// the file is there, a person answered, or this command's own deadline.
fn wait_call(ctx: &mut Ctx, route: &str, first: Vec<(Text, Value)>) -> Result<Split, Exit> {
    let start = std::time::Instant::now();
    let secs = || start.elapsed().as_secs() as i64;
    let mut deadline: Option<i64> = None;
    let mut body = first;
    loop {
        let r = ctx.call(true, route, &obj(body.clone()))?;
        // `IFS=$'\t' read -r waiting card wsecs` of the Python's
        // "%d\t%s\t%d": tabs are IFS white space, so an empty card leaves
        // the seconds in `card` and none in `wsecs`.
        let fields = match wait_fields(&r.body) {
            Ok(f) => f,
            Err(e) => {
                out::stderr_line(e.text().trim_end_matches('\n'));
                [String::new(), String::new(), String::new()]
            }
        };
        let line = fields_line(&fields);
        let words: Vec<&str> = line.split('\t').filter(|w| !w.is_empty()).collect();
        let waiting = words.first().copied().unwrap_or("");
        let card = words.get(1).copied().unwrap_or("").to_string();
        let wsecs: i64 = words.get(2).and_then(|w| w.parse().ok()).unwrap_or(0);
        if waiting != "1" {
            return Ok(r);
        }
        let dl = match deadline {
            Some(d) => d,
            None => {
                out::stderr_line(&format!(
                    "{CLI} vault: waiting for a person to save the card ({card})…"
                ));
                deadline = Some(wsecs);
                wsecs
            }
        };
        let left = dl - secs();
        if left <= 0 {
            out::stderr_line(&format!(
                "no_answer: no answer yet; the card is still open (card {card})"
            ));
            return Err(Exit::Refused);
        }
        body = merge(&[
            &body,
            &[
                kv("wait", Value::Int(left.to_string())),
                kv("waitCard", py::text(&card)),
            ],
        ]);
    }
}

/// The wait's Python: `waiting` as 1 or 0, `card` or "", `waitSecs` or 0.
fn wait_fields(body: &[u8]) -> R<[String; 3]> {
    let v = confirm::load(body)?;
    let waiting = if t(&get(&v, "waiting")?) { "1" } else { "0" };
    let c = get(&v, "card")?;
    let c = if t(&c) { s(&c) } else { String::new() };
    let w = get(&v, "waitSecs")?;
    let w = if t(&w) { py::d(&w)? } else { "0".into() };
    Ok([waiting.into(), c, w])
}

fn fields_line(f: &[String; 3]) -> String {
    format!("{}\t{}\t{}", f[0], f[1], f[2])
}

/// `k2 vault grant NAME --to …`, with the person's confirm over the vault
/// socket when the daemon asks for one from a terminal outside K2.
fn grant(
    ctx: &mut Ctx,
    o: &Opts,
    name: &str,
    sj: &[(Text, Value)],
    nj: &[(Text, Value)],
) -> Result<Exit, Exit> {
    if name.is_empty() || o.to.is_empty() {
        return Err(usage(format!(
            "usage: {CLI} vault grant NAME --to ws:<name>|ws:*|session:<id>|service:<label>|job:<name>|compute:<ws>/<node> [--as <d>] [--ttl 2h]"
        )));
    }
    let mut gj = vec![kv("to", py::text(&o.to))];
    for (k, v) in [
        ("as", &o.as_d),
        ("envName", &o.env_name),
        ("fileName", &o.file_name),
        ("ttl", &o.ttl),
    ] {
        if !v.is_empty() {
            gj.push(kv(k, py::text(v)));
        }
    }
    let body = obj(merge(&[sj, nj, &gj]));
    let mut r = ctx.send(true, "/cli/vault/grant", &body)?;
    let needs_confirm = r.status == "403"
        && r.body
            .windows(24)
            .any(|w| w == b"\"vault_confirm_required\"")
        && !confirm::in_k2_terminal()
        && crate::families::live_socket(Some(&ctx.sock));
    if needs_confirm {
        let shown_q = py::urlencode(&merge(&[sj, nj]));
        let sh = ctx.sock(false, &format!("show?{shown_q}"), b"");
        if !sh.ok() {
            return Err(ctx.fail(&sh));
        }
        let gsid = match confirm::load(&sh.body).and_then(|v| idx(&idx(&v, "secret")?, "id")) {
            Ok(id) => s(&id),
            Err(e) => {
                out::stderr_line(e.text().trim_end_matches('\n'));
                return Err(Exit::Error);
            }
        };
        out::stderr_line(&format!(
            "Sharing {name} this way lets {} read the value: confirm to go ahead (nothing is shared if you cancel).",
            o.to
        ));
        let label = format!("share {name} with {}", o.to);
        let Some(tok) = confirm::confirm_token(&mut ctx.confirm(), "grant-read", &gsid, &label)
        else {
            return Err(Exit::Refused);
        };
        let Some(b) = confirm::with_confirm_token(&body, &tok, None) else {
            return Err(Exit::Error);
        };
        r = ctx.sock(true, "grant", b.as_bytes());
    }
    if !r.ok() {
        return Err(ctx.fail(&r));
    }
    if !o.ttl.is_empty() {
        let ends = confirm::load(&r.body)
            .and_then(|v| get(&or_obj(get(&v, "grant")?), "expiresAt"))
            .map(|x| t(&x));
        if !matches!(ends, Ok(true)) {
            if let Err(e) = &ends {
                out::stderr_line(e.text().trim_end_matches('\n'));
            }
            let id =
                match confirm::load(&r.body).and_then(|v| get(&or_obj(get(&v, "grant")?), "id")) {
                    Ok(x) if t(&x) => s(&x),
                    Ok(_) => String::new(),
                    Err(e) => {
                        out::stderr_line(e.text().trim_end_matches('\n'));
                        String::new()
                    }
                };
            out::stderr_line(&format!(
                "grant_ttl_ignored: this {PRODUCT} server made the share WITHOUT an end time (it is older than 0.46.11 and drops --ttl). Take it back when you're done: {CLI} vault revoke --grant {id}"
            ));
            return Ok(Exit::Error);
        }
    }
    if o.json {
        return Ok(print_body(&r));
    }
    let (n, to) = (name.to_string(), o.to.clone());
    let e = render(&r.body, |v, l| {
        let g = or_obj(get(v, "grant")?);
        let until = if t(&get(&g, "expiresAt")?) {
            cat(" until ", &idx(&g, "expiresAt")?)?
        } else {
            String::new()
        };
        l.p(format!("shared {n} with {to}{until}"));
        Ok(0)
    })
    .print();
    if e != Exit::Ok {
        return Ok(e);
    }
    let e = render(&r.body, |v, l| {
        let x = get(v, "reach")?;
        if t(&x) {
            l.p(format!("note: {}", s(&x)));
        }
        Ok(0)
    })
    .print();
    if e != Exit::Ok {
        return Ok(e);
    }
    Ok(render(&r.body, |v, l| {
        let w = get(v, "warning")?;
        if t(&w) {
            l.err.push_str(&cat("", &w)?);
            l.err.push('\n');
        }
        Ok(0)
    })
    .print())
}

fn service(v: &Value, l: &mut Lines) -> R<i32> {
    let ef = get(v, "envFile")?;
    let ef = if t(&ef) {
        cat(", .env.k2 = ", &idx(v, "envFile")?)?
    } else {
        String::new()
    };
    l.p(format!(
        "{}: environment {}{ef}",
        s(&get(v, "service")?),
        s(&get(v, "mode")?)
    ));
    for x in items(&or_list(get(v, "secrets")?))? {
        let en = if t(&get(&x, "envName")?) {
            format!("{})", cat(" (", &idx(&x, "envName")?)?)
        } else {
            String::new()
        };
        l.p(format!(
            "  {} as {}{en}",
            s(&get(&x, "name")?),
            s(&get(&x, "delivery")?)
        ));
    }
    if t(&get(v, "inheritedNames")?) {
        l.p(format!(
            "  inherits the daemon environment: {}",
            joined(&idx(v, "inheritedNames")?, ", ")?
        ));
    }
    if is(&get(v, "mode")?, "legacy") {
        let ends = get(v, "envRemovedIn")?;
        if t(&ends) {
            let when = if t(&get(v, "cutoverNow")?) {
                format!("with this release ({PRODUCT} {})", s(&ends))
            } else {
                format!("in {PRODUCT} {}", s(&ends))
            };
            l.p(format!(
                "  deprecated: inheriting the daemon environment ends {when} — switch with: {CLI} publish env {} clean",
                s(&get(v, "service")?)
            ));
        } else {
            l.p(format!(
                "  inherits the daemon environment (it keeps it as a --cmd service) — start it clean with: {CLI} publish env {} clean",
                s(&get(v, "service")?)
            ));
        }
    }
    if t(&get(v, "restart")?) {
        l.p(s(&idx(v, "restart")?));
    }
    Ok(0)
}

/// `int(x)` of digits the parser checked: the digits with leading zeros
/// gone, as Python's `int` reads them.
fn int_text(digits: &str) -> String {
    let t = digits.trim_start_matches('0');
    if t.is_empty() {
        "0".into()
    } else {
        t.into()
    }
}

/// `", ".join(x or [])`, or `none_text` when that is empty.
fn joined_or(v: &Value, sep: &str, none_text: &str) -> R<String> {
    let j = joined(&or_list(v.clone()), sep)?;
    Ok(if j.is_empty() { none_text.into() } else { j })
}

/// `sep.join(v)`: strings only, as Python's `join`.
fn joined(v: &Value, sep: &str) -> R<String> {
    let mut parts = Vec::new();
    for (i, x) in items(v)?.iter().enumerate() {
        match x {
            Value::Str(_) => parts.push(s(x)),
            other => {
                return Err(crate::families::Raise::at(
                    1,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        crate::out::py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(parts.join(sep))
}

/// `(" x%d" % a["count"]) if a.get("count",1) > 1 else ""`.
fn times(a: &Value) -> R<String> {
    let c = get_or(a, "count", Value::Int("1".into()))?;
    let more = match &c {
        Value::Int(n) => n.parse::<i128>().map_or(!n.starts_with('-'), |n| n > 1),
        Value::Float(f) => *f > 1.0,
        Value::Bool(_) => false,
        other => {
            return Err(crate::families::Raise::at(
                1,
                format!(
                    "TypeError: '>' not supported between instances of '{}' and 'int'",
                    crate::out::py::type_name(other)
                ),
            ))
        }
    };
    Ok(if more {
        format!(" x{}", py::d(&idx(a, "count")?)?)
    } else {
        String::new()
    })
}

/// `_vault_print_recall`: the answer's `recall` words, when there are any.
fn recall(r: &Split) -> Exit {
    render(&r.body, |v, l| {
        let w = get(v, "recall")?;
        if t(&w) {
            l.p(py::s(&w));
        }
        Ok(0)
    })
    .print()
}

/// `sys.stdout.buffer.write(base64.b64decode(json.load(sys.stdin)["valueB64"]))`,
/// and a newline after it when `newline` (`reveal`).
pub fn value_out(body: &[u8], newline: bool) -> Exit {
    let v = match confirm::load(body) {
        Ok(v) => v,
        Err(r) => return crate::families::raised(String::new(), String::new(), &r).print(),
    };
    let b64 = match idx(&v, "valueB64") {
        Ok(Value::Str(t)) => t.as_str().unwrap_or("").to_string(),
        Ok(other) => {
            let r = crate::families::Raise::at(
                1,
                format!(
                    "TypeError: argument should be a bytes-like object or ASCII string, not '{}'",
                    crate::out::py::type_name(&other)
                ),
            );
            return crate::families::raised(String::new(), String::new(), &r).print();
        }
        Err(r) => return crate::families::raised(String::new(), String::new(), &r).print(),
    };
    let Some(mut bytes) = b64_decode(&b64) else {
        let r = crate::families::Raise::at(1, "binascii.Error: Incorrect padding");
        return crate::families::raised(String::new(), String::new(), &r).print();
    };
    if newline {
        bytes.push(b'\n');
    }
    let w = out::write_flushed(&mut std::io::stdout().lock(), &bytes);
    for b in bytes.iter_mut() {
        *b = 0;
    }
    match w {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// Python's `base64.b64decode(s)` (not validating): characters outside
/// the alphabet are dropped, then the padding must come out right.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut quad = Vec::new();
    let mut pads = 0usize;
    for c in s.bytes() {
        if c == b'=' {
            pads += 1;
            quad.push(c);
            continue;
        }
        if val(c).is_some() {
            if pads > 0 {
                // Data after padding: binascii stops at the padded group.
                break;
            }
            quad.push(c);
        }
    }
    if quad.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::new();
    for chunk in quad.chunks(4) {
        let mut n = 0u32;
        let mut k = 0;
        for &c in chunk {
            if c == b'=' {
                continue;
            }
            n = (n << 6) | val(c)?;
            k += 1;
        }
        match k {
            4 => out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]),
            3 => {
                n <<= 6;
                out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8]);
            }
            2 => {
                n <<= 12;
                out.push((n >> 16) as u8);
            }
            0 => {}
            _ => return None,
        }
    }
    Some(out)
}

fn b64_encode(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `_vault_body_with_value <base>`: the value from stdin as `valueB64`.
/// On a terminal, `Value (not shown): ` and a no-echo line; else all of
/// stdin, one trailing newline dropped when it is the only one. No value
/// is `usage: no value on stdin`, exit 2. The value never leaves this
/// process but in the request body.
pub fn body_with_value(mut base: Vec<(Text, Value)>) -> Result<prompt::Secret, Exit> {
    let v = if prompt::stdin_is_tty() {
        let v = prompt::ask_hidden("Value (not shown): ");
        if v.is_empty() {
            return Err(usage("usage: no value on stdin".into()));
        }
        v
    } else {
        let mut all = Vec::new();
        let mut buf = [0u8; 8192];
        use std::io::Read;
        let mut stdin = std::io::stdin().lock();
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => all.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        buf.fill(0);
        if all.last() == Some(&b'\n') && all.iter().filter(|b| **b == b'\n').count() == 1 {
            all.pop();
        }
        if all.is_empty() {
            return Err(usage("usage: no value on stdin".into()));
        }
        prompt::Secret::new(all)
    };
    let enc = prompt::Secret::new(b64_encode(v.expose()).into_bytes());
    drop(v);
    let field = Value::Str(Text::from(
        String::from_utf8_lossy(enc.expose()).into_owned(),
    ));
    match base
        .iter_mut()
        .find(|(k, _)| k.as_str() == Some("valueB64"))
    {
        Some((_, old)) => *old = field,
        None => base.push(kv("valueB64", field)),
    }
    let text = dumps(&Value::Object(base));
    Ok(prompt::Secret::new(text.into_bytes()))
}

trait AsStr {
    fn as_str(&self) -> &str;
}

impl AsStr for prompt::Secret {
    fn as_str(&self) -> &str {
        std::str::from_utf8(self.expose()).unwrap_or("")
    }
}

/// `_vault_sign_json scheme header encoding prefix … `: the signer, `None`
/// for no scheme, or its usage line (exit 2).
fn sign_json(
    scheme: &str,
    header: &str,
    enc: &str,
    o: &Opts,
    token_url: &str,
    client_auth: &str,
) -> Result<Option<Value>, Exit> {
    let tsh = &o.sign_ts_header;
    if scheme.is_empty() {
        if !header.is_empty()
            || !enc.is_empty()
            || !tsh.is_empty()
            || o.sign_prefix.is_some()
            || o.sign_template.is_some()
            || o.sign_value.is_some()
            || !token_url.is_empty()
            || !client_auth.is_empty()
        {
            return Err(usage("usage: --sign-* and --oauth-* options need --sign ovh|hmac-sha256|hmac-sha256-ts|oauth-refresh".into()));
        }
        return Ok(None);
    }
    let mut v = vec![kv("scheme", py::text(scheme))];
    let mut add = |k: &str, x: &str| v.push(kv(k, py::text(x)));
    if !token_url.is_empty() {
        add("tokenUrl", token_url);
    }
    if !client_auth.is_empty() {
        add("clientAuth", client_auth);
    }
    if !header.is_empty() {
        add("header", header);
    }
    if !enc.is_empty() {
        add("encoding", enc);
    }
    if let Some(p) = &o.sign_prefix {
        add("prefix", p);
    }
    if !tsh.is_empty() {
        add("tsHeader", tsh);
    }
    if let Some(p) = &o.sign_template {
        add("template", p);
    }
    if let Some(p) = &o.sign_value {
        add("valueFmt", p);
    }
    Ok(Some(Value::Object(v)))
}

/// `set`'s options (its Python before the value).
fn set_opts(o: &Opts, sign: Option<Value>) -> Result<Vec<(Text, Value)>, Exit> {
    let (ih, iformat, idst) = (&o.inject_header, &o.inject_format, &o.inject_dst);
    if sign.is_some() && !(ih.is_empty() && iformat.is_empty() && idst.is_empty()) {
        return Err(usage(format!(
            "usage: a signed secret has no --inject-*: {PRODUCT} sets the signer's own headers"
        )));
    }
    if !ih.is_empty() && !idst.is_empty() {
        return Err(usage(
            "usage: --inject-header X is --inject-dst header:X; give one of them".into(),
        ));
    }
    let mut v = Vec::new();
    let mut add = |k: &str, x: Value| v.push(kv(k, x));
    if !o.kind.is_empty() {
        add("kind", py::text(&o.kind));
    }
    if !o.hosts.is_empty() {
        add(
            "hosts",
            Value::Array(
                o.hosts
                    .split(',')
                    .map(|h| h.trim_matches(crate::families::py_space))
                    .filter(|h| !h.is_empty())
                    .map(py::text)
                    .collect(),
            ),
        );
    }
    if !o.preset.is_empty() {
        add("preset", py::text(&o.preset));
    }
    if !o.delivery.is_empty() {
        add("delivery", py::text(&o.delivery));
    }
    if !o.ceiling.is_empty() {
        add("ceiling", py::text(&o.ceiling));
    }
    if !o.describe.is_empty() {
        add("description", py::text(&o.describe));
    }
    if !o.rotate_every.is_empty() {
        match py_int(&o.rotate_every) {
            Some(n) => add("rotateEveryDays", Value::Int(n)),
            None => {
                // int() raises: the shell's Python prints its traceback and
                // the command ends with 2 (`|| exit 2`).
                let r = crate::families::Raise::at(
                    10,
                    format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        crate::out::py::repr(&py::text(&o.rotate_every))
                    ),
                );
                out::stderr_line(r.text().trim_end_matches('\n'));
                return Err(Exit::Usage);
            }
        }
    }
    if !o.ttl.is_empty() {
        add("ttl", py::text(&o.ttl));
    }
    if !(ih.is_empty() && iformat.is_empty() && idst.is_empty()) {
        let dst = if idst.is_empty() {
            format!(
                "header:{}",
                if ih.is_empty() { "Authorization" } else { ih }
            )
        } else {
            idst.clone()
        };
        let fmt = if iformat.is_empty() { "{}" } else { iformat };
        add(
            "inject",
            Value::Object(vec![kv("dst", py::text(&dst)), kv("fmt", py::text(fmt))]),
        );
    }
    if let Some(sg) = sign {
        add("inject", Value::Object(vec![kv("sign", sg)]));
    }
    // `o["inject"]` set twice keeps one key: the last.
    let mut dedup: Vec<(Text, Value)> = Vec::new();
    for (k, x) in v {
        match dedup.iter_mut().find(|(dk, _)| *dk == k) {
            Some((_, dx)) => *dx = x,
            None => dedup.push((k, x)),
        }
    }
    Ok(dedup)
}

/// Python's `int(s)` of a command-line word: optional spaces and sign,
/// digits with single underscores between them.
fn py_int(s: &str) -> Option<String> {
    let t = s.trim_matches(crate::families::py_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let d: String = digits.chars().filter(|c| *c != '_').collect();
    let d = d.trim_start_matches('0');
    Some(match (neg, d.is_empty()) {
        (_, true) => "0".into(),
        (true, false) => format!("-{d}"),
        (false, false) => d.into(),
    })
}

fn status(v: &Value, l: &mut Lines) -> R<i32> {
    if !t(&get_or(v, "open", Value::Bool(true))?) && t(&get(v, "error")?) {
        let e = idx(v, "error")?;
        l.p(format!(
            "vault: closed — {}: {}",
            s(&get(&e, "code")?),
            s(&get(&e, "hint")?)
        ));
        return Ok(0);
    }
    l.p("vault:          open");
    l.p(format!(
        "key:            {} ({})",
        s(&get(v, "keyHomeLabel")?),
        s(&get(v, "keyHome")?)
    ));
    l.p(format!("rollback guard: {}", s(&get(v, "rollbackGuard")?)));
    l.p(format!("recovery key:   {}", s(&get(v, "recovery")?)));
    l.p(format!(
        "vault password: {}",
        if t(&get(v, "confirmPassword")?) {
            "set"
        } else {
            "not set"
        }
    ));
    let px = or_obj(get(v, "proxy")?);
    let proxy = if t(&get(&px, "port")?) {
        let port = idx(&px, "port")?;
        let moved = if t(&get(&px, "moved")?) {
            format!("  ({})", s(&idx(&px, "hint")?))
        } else {
            String::new()
        };
        format!("127.0.0.1:{}{moved}", s(&port))
    } else {
        let h = get(&px, "hint")?;
        if t(&h) {
            s(&h)
        } else {
            "not listening".into()
        }
    };
    l.p(format!("proxy:          {proxy}"));
    let hs = items(&or_list(get(v, "holders")?))?;
    if !hs.is_empty() {
        l.p("key holders:    (who can open the vault)");
        for h in hs {
            let kind = get(&h, "kind")?;
            let label = match kind.clone() {
                Value::Str(k) if k.as_str() == Some("box") => "box key (this machine)".into(),
                Value::Str(k) if k.as_str() == Some("recovery") => "recovery key".into(),
                Value::Str(k) if k.as_str() == Some("backup") => "backup key".into(),
                Value::Array(_) | Value::Object(_) => {
                    return Err(crate::families::Raise::at(
                        1,
                        format!(
                            "TypeError: unhashable type: '{}'",
                            crate::out::py::type_name(&kind)
                        ),
                    ))
                }
                k => format!("{} key", s(&k)),
            };
            let fp = get(&h, "fingerprint")?;
            let fp = if t(&fp) { s(&fp) } else { String::new() };
            let added = if t(&get(&h, "createdAt")?) {
                format!("  added {}", s(&idx(&h, "createdAt")?))
            } else {
                String::new()
            };
            l.p(format!("  {} {fp}{added}", left(&py::text(&label), 22)));
        }
    }
    let c = or_obj(get(v, "counts")?);
    l.p(format!(
        "secrets:        {} yours, {} {PRODUCT} system, {} shares, {} pending cards",
        s(&get(&c, "user")?),
        s(&get(&c, "system")?),
        s(&get(&c, "grants")?),
        s(&get(&c, "pendingRequests")?)
    ));
    if !is(&get(v, "tamper")?, "ok") {
        l.p(format!("PROBLEM:        {}", s(&get(v, "tamper")?)));
    }
    if t(&get(v, "newerRows")?) {
        l.p(format!("NEEDS UPDATE:   {}", s(&get(v, "newerRows")?)));
    }
    if t(&get(v, "rollback")?) {
        l.p("PROBLEM:        vault.db looks older than expected; the owner confirms in Settings → Vault");
    }
    if t(&get(v, "movedOut")?) {
        let m = idx(v, "movedOut")?;
        l.p(format!(
            "FROZEN:         exported for a box move at {} (move key {}): reads work, writes wait. Called off? {CLI} migrate export-secrets --unfreeze",
            s(&get(&m, "at")?),
            s(&get(&m, "moveKey")?)
        ));
    }
    l.p("");
    l.p(s(&get_or(v, "honest", py::text(""))?));
    if t(&get(v, "recoveryHint")?) {
        l.p("");
        l.p(s(&idx(v, "recoveryHint")?));
    }
    Ok(0)
}

fn list(v: &Value, l: &mut Lines) -> R<i32> {
    let its = items(&or_list(get(v, "secrets")?))?;
    let hidden = get(v, "systemHidden")?;
    if its.is_empty() {
        l.p(format!(
            "{} ({CLI} vault set NAME < value, or ask a person with {CLI} thread secret)",
            if t(&hidden) {
                "none of yours to show"
            } else {
                "no secrets yet"
            }
        ));
    }
    for m in its {
        let sc = or_obj(get(&m, "scope")?);
        let kind = get(&sc, "kind")?;
        let where_ = if is(&kind, "server") {
            "server".to_string()
        } else if is(&kind, "workspace") {
            let w = get(&m, "workspace")?;
            let w = if t(&w) {
                w
            } else {
                get_or(&sc, "id", py::text(""))?
            };
            cat("ws:", &w)?
        } else {
            let k = get_or(&sc, "kind", py::text(""))?;
            let id = get_or(&sc, "id", py::text(""))?;
            let id8 = match &id {
                Value::Str(x) => x
                    .code_points()
                    .into_iter()
                    .take(8)
                    .filter_map(char::from_u32)
                    .collect::<String>(),
                other => {
                    return Err(crate::families::Raise::at(
                        1,
                        format!(
                            "TypeError: '{}' object is not subscriptable",
                            crate::out::py::type_name(other)
                        ),
                    ))
                }
            };
            let a = cat("", &k)?;
            format!("{a}:{id8}")
        };
        let mut hosts = Vec::new();
        for h in items(&or_list(get(&m, "hosts")?))? {
            let h = match &h {
                Value::Str(_) => s(&h),
                other => {
                    return Err(crate::families::Raise::at(
                        1,
                        format!(
                            "AttributeError: '{}' object has no attribute 'split'",
                            crate::out::py::type_name(other)
                        ),
                    ))
                }
            };
            let after = h.split_once("://").map_or(h.as_str(), |(_, b)| b);
            let host = after.rsplit_once(':').map_or(after, |(a, _)| a);
            hosts.push(host.to_string());
        }
        let hosts = if hosts.is_empty() {
            "-".to_string()
        } else {
            hosts.join(",")
        };
        let see = if t(&get(&m, "agentCanSee")?) {
            " (agent can see it)"
        } else {
            ""
        };
        let ns = if is(&get(&m, "namespace")?, "system") {
            " [system]"
        } else {
            ""
        };
        let mut st = if is(&get(&m, "status")?, "active") {
            String::new()
        } else {
            let a = cat(" [", &get_or(&m, "status", py::text(""))?)?;
            a + "]"
        };
        let sg = get(&or_obj(get(&m, "inject")?), "sign")?;
        if t(&sg) {
            st += &(cat(" [signs: ", &get_or(&sg, "scheme", py::text(""))?)? + "]");
        }
        if is(&get(&or_obj(get(&m, "oauth")?), "state")?, "expired") {
            st += " [expired: sign in again]";
        }
        l.p(format!(
            "{} {} {} {} v{} {hosts}{see}{ns}{st}",
            left(&get(&m, "name")?, 28),
            left(&py::text(&where_), 18),
            left(&get(&m, "kind")?, 10),
            left(&get(&m, "defaultDelivery")?, 8),
            left(&get(&m, "latestVersion")?, 3)
        ));
    }
    if t(&hidden) {
        l.p(format!(
            "{} {PRODUCT} system secrets hidden from you: {}",
            s(&get(&hidden, "count")?),
            s(&get(&hidden, "why")?)
        ));
    }
    Ok(0)
}

fn show(v: &Value, l: &mut Lines) -> R<i32> {
    let m = or_obj(get(v, "secret")?);
    let r = get(&m, "ref")?;
    l.p(format!(
        "{}  ({}, {}, v{}, {})",
        s(&get(&m, "name")?),
        s(&get(&m, "kind")?),
        s(&get(&m, "status")?),
        s(&get(&m, "latestVersion")?),
        if t(&r) { s(&r) } else { "system".into() }
    ));
    l.p(format!(
        "sites:     {}",
        joined_or(
            &get(&m, "hosts")?,
            ", ",
            "none (fetch is refused until a site is added)"
        )?
    ));
    let inj = or_obj(get(&m, "inject")?);
    let sg = get(&inj, "sign")?;
    if t(&sg) {
        let sch = get(&sg, "scheme")?;
        if is(&sch, "ovh") {
            l.p("signs:     ovh (X-Ovh-Application, X-Ovh-Consumer, X-Ovh-Timestamp, X-Ovh-Signature; OVH clock)");
        } else if is(&sch, "oauth-refresh") {
            l.p(format!(
                "signs:     oauth-refresh ({PRODUCT} refreshes the token at {}; client auth {})",
                s(&get(&sg, "tokenUrl")?),
                s(&get_or(&sg, "clientAuth", py::text("post"))?)
            ));
            let oa = or_obj(get(&m, "oauth")?);
            let msg = get(&oa, "message")?;
            let line = if t(&msg) {
                s(&msg)
            } else if t(&get(&oa, "expiresAt")?) {
                cat("ok, token good until ", &idx(&oa, "expiresAt")?)?
            } else {
                "ok".into()
            };
            l.p(format!("sign-in:   {line}"));
        } else if is(&sch, "hmac-sha256") {
            l.p(format!(
                "signs:     hmac-sha256 over the body → {}: {}<{}>",
                s(&get(&sg, "header")?),
                s(&get_or(&sg, "prefix", py::text(""))?),
                s(&get_or(&sg, "encoding", py::text("hex"))?)
            ));
        } else {
            let tsh = get(&sg, "tsHeader")?;
            let ts = if t(&tsh) {
                format!("{}: {{ts}}", cat(", ", &idx(&sg, "tsHeader")?)?)
            } else {
                String::new()
            };
            l.p(format!(
                "signs:     hmac-sha256-ts over {} → {}: {} ({}){ts}",
                s(&get(&sg, "template")?),
                s(&get(&sg, "header")?),
                s(&get(&sg, "valueFmt")?),
                s(&get_or(&sg, "encoding", py::text("hex"))?)
            ));
        }
    } else if t(&inj) {
        l.p(format!(
            "goes in:   {} as {}",
            s(&get(&inj, "dst")?),
            s(&get(&inj, "fmt")?)
        ));
    }
    l.p(format!(
        "delivery:  {} (at most {}){}",
        s(&get(&m, "defaultDelivery")?),
        s(&get(&m, "deliveryCeiling")?),
        if t(&get(&m, "agentCanSee")?) {
            " — agents can see it"
        } else {
            ""
        }
    ));
    l.p(format!(
        "given by:  {}{}",
        s(&get(&m, "createdBy")?),
        if t(&get(&m, "humanGiven")?) {
            " (a person)"
        } else {
            ""
        }
    ));
    let ho = or_obj(get(&m, "handoff")?);
    if t(&ho) {
        let acc = if t(&get(&ho, "acceptedCeiling")?) {
            format!(
                ", accepted by {} up to {}",
                s(&get(&ho, "decidedBy")?),
                s(&idx(&ho, "acceptedCeiling")?)
            )
        } else {
            String::new()
        };
        l.p(format!(
            "sent by:   {} ({}){acc}",
            s(&get(&ho, "from")?),
            s(&get(&ho, "deliveryId")?)
        ));
    }
    if t(&get(v, "use")?) {
        l.p(format!("use:       {}", s(&idx(v, "use")?)));
    }
    if t(&get(v, "canTake")?) {
        l.p(format!("more:      {}", s(&idx(v, "canTake")?)));
    }
    let ap = get(v, "agentPolicy")?;
    if t(&ap) {
        l.p(format!(
            "agent:     at most {} signatures a second, {} a minute per session; forwarded agents {}",
            s(&get(&ap, "burst")?),
            s(&get(&ap, "perMinute")?),
            if t(&get(&ap, "allowForwarded")?) { "allowed" } else { "refused" }
        ));
    }
    for g in items(&or_list(get(v, "grants")?))? {
        let env = if t(&get(&g, "envName")?) {
            cat(" env ", &idx(&g, "envName")?)?
        } else {
            String::new()
        };
        let until = if t(&get(&g, "expiresAt")?) {
            cat(" until ", &idx(&g, "expiresAt")?)?
        } else {
            String::new()
        };
        l.p(format!(
            "shared:    {} as {}{env}{until}",
            s(&get(&g, "consumerLabel")?),
            s(&get(&g, "delivery")?)
        ));
    }
    for p in items(&or_list(get(v, "placements")?))? {
        let where_ = if is(&get(&p, "target")?, "systemd_cred") {
            format!(
                "systemd credential {} on {}",
                s(&get(&p, "credName")?),
                s(&get(&p, "unit")?)
            )
        } else {
            let w = get(&p, "where")?;
            if t(&w) {
                s(&w)
            } else {
                String::new()
            }
        };
        let st = get(&p, "state")?;
        let tail = if is(&st, "due") {
            format!(
                " — due: put v{} there, then {CLI} vault placement done {}",
                s(&get(&p, "latestVersion")?),
                s(&get(&p, "id")?)
            )
        } else if is(&st, "stale") {
            format!(
                " — stale: {CLI} vault systemd-cred {} --unit {}",
                s(&get(&m, "name")?),
                s(&get(&p, "unit")?)
            )
        } else if is(&st, "inactive") {
            format!(
                " — the secret is {}; take it off",
                s(&get(&p, "secretStatus")?)
            )
        } else if matches!(st, Value::Array(_) | Value::Object(_)) {
            return Err(crate::families::Raise::at(
                1,
                format!(
                    "TypeError: unhashable type: '{}'",
                    crate::out::py::type_name(&st)
                ),
            ));
        } else {
            String::new()
        };
        l.p(format!(
            "placed:    {where_} (v{}){tail}",
            s(&get(&p, "version")?)
        ));
    }
    let audit = or_list(get(v, "audit")?);
    let rows = match &audit {
        Value::Array(a) => a.iter().take(10).cloned().collect::<Vec<_>>(),
        Value::Str(_) => items(&audit)?.into_iter().take(10).collect(),
        other => {
            return Err(crate::families::Raise::at(
                1,
                format!(
                    "TypeError: '{}' object is not subscriptable",
                    crate::out::py::type_name(other)
                ),
            ))
        }
    };
    for a in rows {
        l.p(format!(
            "  {} {} {} {}{}",
            s(&get(&a, "ts")?),
            s(&get(&a, "action")?),
            s(&get(&a, "outcome")?),
            s(&get(&a, "actor")?),
            times(&a)?
        ));
    }
    Ok(0)
}

fn set_printer(v: &Value, l: &mut Lines) -> R<i32> {
    let m = or_obj(get(v, "secret")?);
    l.p(format!(
        "{} {} → {} (v{})",
        if t(&get(v, "created")?) {
            "saved"
        } else {
            "new version of"
        },
        s(&get(&m, "name")?),
        s(&get(v, "ref")?),
        s(&get(&m, "latestVersion")?)
    ));
    let sg = get(&or_obj(get(&m, "inject")?), "sign")?;
    if t(&sg) {
        l.p(format!(
            "signs requests: {} ({CLI} vault show {})",
            s(&get(&sg, "scheme")?),
            s(&get(&m, "name")?)
        ));
    }
    if t(&get(v, "trimmed")?) {
        l.p("(trimmed spaces/newlines around the value)");
    }
    let u = get(v, "use")?;
    l.p(cat("use: ", &if t(&u) { u } else { py::text("") })?);
    if t(&get(v, "ttlCut")?) {
        let n = get(v, "ttlNote")?;
        l.p(if t(&n) {
            s(&n)
        } else {
            "kept for 7 d (the most for one session)".into()
        });
    }
    Ok(0)
}

fn rotate_printer(v: &Value, l: &mut Lines) -> R<i32> {
    l.p(format!(
        "{} is now v{}",
        s(&get(&or_obj(get(v, "secret")?), "name")?),
        s(&get(v, "version")?)
    ));
    for r in items(&or_list(get(v, "replaced")?))? {
        if t(&get(&r, "ok")?) {
            l.p(format!(
                "re-placed into {} (systemd credential {}){}",
                s(&get(&r, "unit")?),
                s(&get(&r, "credName")?),
                if t(&get(&r, "restarted")?) {
                    ", restart queued"
                } else {
                    ""
                }
            ));
        } else {
            let e = or_obj(get(&r, "error")?);
            l.p(format!(
                "NOT re-placed into {}: {}",
                s(&get(&r, "unit")?),
                s(&get(&e, "hint")?)
            ));
        }
    }
    Ok(0)
}

fn doctor(v: &Value, l: &mut Lines) -> R<i32> {
    let f = items(&or_list(get(v, "findings")?))?;
    if f.is_empty() {
        l.p("vault: nothing to fix");
    }
    for x in f {
        l.p(format!(
            "{} {}",
            left(&get(&x, "level")?, 7),
            s(&get(&x, "message")?)
        ));
        let paths = or_list(get(&x, "files")?);
        let all = items(&paths)?;
        for p in all.iter().take(5) {
            l.p(format!("        {}", s(p)));
        }
        let n = all.len() as i128;
        let fc = get(&x, "fileCount")?;
        let total = if t(&fc) {
            match &fc {
                Value::Int(i) => i.parse::<i128>().unwrap_or(0),
                Value::Bool(b) => i128::from(*b),
                other => {
                    return Err(crate::families::Raise::at(
                        1,
                        format!(
                            "TypeError: unsupported operand type(s) for -: '{}' and 'int'",
                            crate::out::py::type_name(other)
                        ),
                    ))
                }
            }
        } else {
            n
        };
        let more = total - n.min(5);
        if more > 0 {
            l.p(format!(
                "        ... and {more} more (--json lists up to {})",
                if n > 0 { n } else { 5 }
            ));
        }
    }
    if t(&get(v, "checkedNames")?) {
        l.p(format!(".env {}", s(&idx(v, "checkedNames")?)));
    }
    if t(&get(v, "summary")?) {
        l.p(s(&idx(v, "summary")?));
    }
    Ok(if t(&get(v, "ok")?) { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(sub: &str, rest: &[&str]) -> Result<Opts, Exit> {
        parse(sub, rest)
    }

    #[test]
    fn the_parser_reads_the_shell_s_flags() {
        let o = p(
            "set",
            &["A", "--for", "a.com,b.com", "--sign-prefix", "", "--json"],
        )
        .unwrap();
        assert_eq!(o.pos, ["A"]);
        assert_eq!(o.hosts, "a.com,b.com");
        assert_eq!(o.sign_prefix.as_deref(), Some(""));
        assert!(o.json);
        assert_eq!(p("show", &["A", "--kind", "x"]).unwrap_err(), Exit::Usage);
        assert_eq!(p("show", &["A", "--scope"]).unwrap_err(), Exit::Usage);
        assert_eq!(p("show", &["--value=x"]).unwrap_err(), Exit::Usage);
        assert_eq!(p("show", &["--nope"]).unwrap_err(), Exit::Usage);
        assert_eq!(p("recovery", &["--json"]).unwrap_err(), Exit::Usage);
        assert_eq!(
            p("agent-limit", &["A", "--burst", "1x"]).unwrap_err(),
            Exit::Usage
        );
        // A single dash is a word.
        assert_eq!(p("show", &["-y"]).unwrap().pos, ["-y"]);
        // rollback's --to is the version.
        assert_eq!(p("rollback", &["A", "--to", "3"]).unwrap().to_version, "3");
    }

    #[test]
    fn takes_names_the_carry_verb() {
        assert_eq!(takes("carry", Some("export")).2, "carry export");
        assert_eq!(takes("carry", Some("x")).0, "");
        assert_eq!(takes("delivery", None).1, "--env-name --file-name");
    }

    #[test]
    fn the_scope_s_fields() {
        let j = |s: &str| obj(scope_json(s).unwrap());
        assert_eq!(j("ws:a b"), r#"{"scope": "workspace", "workspace": "a b"}"#);
        assert_eq!(
            j("workspace:"),
            r#"{"scope": "workspace", "workspace": ""}"#
        );
        assert_eq!(j("server"), r#"{"scope": "server"}"#);
        assert_eq!(j("session:x"), r#"{"scope": "session", "session": "x"}"#);
        assert_eq!(j("engine:sew"), r#"{"scope": "engine:sew"}"#);
        assert_eq!(j(""), "{}");
        assert_eq!(scope_json("nope").unwrap_err(), Exit::Usage);
    }

    #[test]
    fn merge_updates_in_order() {
        let a = vec![
            kv("a", Value::Int("1".into())),
            kv("b", Value::Int("2".into())),
        ];
        let b = vec![kv("a", Value::Int("3".into())), kv("c", Value::Null)];
        assert_eq!(obj(merge(&[&a, &b])), r#"{"a": 3, "b": 2, "c": null}"#);
    }

    #[test]
    fn base64_both_ways() {
        for v in [&b""[..], b"a", b"ab", b"abc", b"\x00\xffhello world"] {
            assert_eq!(b64_decode(&b64_encode(v)).unwrap(), v);
        }
        assert_eq!(b64_encode(b"hi"), "aGk=");
        assert_eq!(b64_decode("aGk"), None);
        assert_eq!(b64_decode("a G\nk=").unwrap(), b"hi");
    }

    #[test]
    fn int_as_python_reads_it() {
        assert_eq!(py_int("007").as_deref(), Some("7"));
        assert_eq!(py_int(" 1_0 ").as_deref(), Some("10"));
        assert_eq!(py_int("-0").as_deref(), Some("0"));
        assert_eq!(py_int("x"), None);
        assert_eq!(py_int("1__0"), None);
    }

    #[test]
    fn the_status_page() {
        let v = crate::out::json::parse(r#"{"keyHomeLabel":"Keychain","keyHome":"kc","rollbackGuard":"on","recovery":"confirmed","confirmPassword":true,"proxy":{"port":8899},"holders":[{"kind":"box","fingerprint":"ab"},{"kind":"x","createdAt":"t"}],"counts":{"user":1,"system":2,"grants":0,"pendingRequests":0},"tamper":"ok","honest":"h"}"#).unwrap();
        let mut l = Lines::default();
        status(&v, &mut l).unwrap();
        assert!(l.out.contains("proxy:          127.0.0.1:8899\n"));
        assert!(l.out.contains("  box key (this machine) ab\n"));
        assert!(l
            .out
            .contains(&format!("  x key{}   added t\n", " ".repeat(17))));
        assert!(l.out.ends_with("\nh\n"));
    }
}
