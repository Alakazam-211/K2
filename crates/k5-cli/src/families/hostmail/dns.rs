//! A hosted domain's DNS-facing settings and the Apple profile:
//! `hostmail dkim` (show, rotate, retire), `hostmail dmarc` (show,
//! report-to), `hostmail ptr` (show, set) and `hostmail profile`, as their
//! `cmd_hostmail_*` functions read the words
//! (`cli/src/hostmail-dkim-dmarc.sh`, `cli/src/hostmail-bans-allowlist.sh`)
//! and `_mail_py`'s verbs answer (`cli/src/mail.sh`).
//!
//! `profile --out <file>` writes the file here, as the Python does: made
//! with mode 0600, refused when it exists unless `--force`.

use super::pyx::{self, g, items, or_dict, or_env, or_s, s_of, sv};
use super::words::{scan, wants_help, Spec};
use crate::families::mail::core::{obj, raise_at, run_env, usage, End, Plan, Py, Secs, R};
use crate::families::mail::py as p;
use crate::out::json::{Text, Value};
use crate::out::{self, py};
use crate::transport::urllib::Method;
use k2_client::brand::CLI_NAME;
use k2_client::hosts::ZONE;
use std::path::Path;

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["dkim", "dmarc", "ptr", "profile"];

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const DKIM_SHOW_CALL: u32 = 2671;
    pub const DKIM_SHOW_FOR: u32 = 2676;
    pub const DKIM_SHOW_ROW: u32 = 2678;
    pub const DKIM_SHOW_EXPECTED: u32 = 2682;
    pub const DKIM_SHOW_NAME: u32 = 2683;
    pub const DKIM_ROTATE_CALL: u32 = 2687;
    pub const DKIM_ROTATE_FOR: u32 = 2694;
    pub const DKIM_ROTATE_ROW: u32 = 2695;
    pub const DKIM_RETIRE_CALL: u32 = 2700;
    pub const DMARC_SHOW_CALL: u32 = 2710;
    pub const DMARC_SHOW_REC: u32 = 2716;
    pub const DMARC_SHOW_MATCH: u32 = 2717;
    pub const DMARC_REPORT_CALL: u32 = 2726;
    pub const PTR_SHOW_CALL: u32 = 2737;
    pub const PTR_SHOW_A: u32 = 2745;
    pub const PTR_SHOW_RELAY: u32 = 2749;
    pub const PTR_SHOW_UNRESOLVED: u32 = 2751;
    pub const PTR_SET_CALL: u32 = 2754;
    pub const PTR_SET_SOURCE: u32 = 2763;
    pub const PTR_SET_WARNING: u32 = 2771;
    pub const PTR_SET_DC: u32 = 2774;
    pub const PTR_SET_DC_WARNING: u32 = 2776;
    pub const PROFILE_CALL: u32 = 3241;
    pub const PROFILE_WRITE: u32 = 3252;
    pub const PROFILE_INCLUDES: u32 = 3267;
    pub const PROFILE_NOTES: u32 = 3270;
    pub const PROFILE_STDOUT: u32 = 3275;
    pub const PROFILE_STDERR_NOTES: u32 = 3278;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "dkim" => dkim(words),
        "dmarc" => dmarc(words),
        "ptr" => ptr(words),
        "profile" => profile(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// `<domain>` (or `--domain`), a usage line naming `op`.
fn one_domain(words: &[&str], noun: &str, op: &str, verb: &str) -> Result<Plan, Plan> {
    let g = scan(
        words,
        &Spec {
            slots: &["domain"],
            values: &[("--domain", "domain", "--domain requires a value")],
            switches: &[],
            unknown: &format!(
                "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail {noun} {op} <domain> [--json]"
            ),
            extra: "unexpected extra argument '{w}' — one domain only",
        },
    )?;
    if g.get("domain").is_empty() {
        return Err(usage(format!("{CLI_NAME} hostmail {noun} {op} <domain>")));
    }
    Ok(Plan::Run(
        run_env(verb, g.json).with("M_DOMAIN", g.get("domain")),
    ))
}

/// `cmd_hostmail_dkim`.
fn dkim(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/dkim"));
    };
    match *sub {
        "show" | "rotate" => {
            if wants_help(rest) {
                return Ok(Plan::Help(if *sub == "show" {
                    "hostmail/dkim/show"
                } else {
                    "hostmail/dkim/rotate"
                }));
            }
            one_domain(rest, "dkim", sub, &format!("dkim_{sub}"))
        }
        "retire" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/dkim/retire"));
            }
            let g = scan(
                rest,
                &Spec {
                    slots: &["domain", "selector"],
                    values: &[
                        ("--domain", "domain", "--domain requires a value"),
                        ("--selector", "selector", "--selector requires a value"),
                    ],
                    switches: &[],
                    unknown: &format!("unknown flag '{{w}}' — usage: {CLI_NAME} hostmail dkim retire <domain> <selector> [--json]"),
                    extra: "unexpected extra argument '{w}'",
                },
            )?;
            if g.get("domain").is_empty() || g.get("selector").is_empty() {
                return Err(usage(format!(
                    "{CLI_NAME} hostmail dkim retire <domain> <selector>"
                )));
            }
            Ok(Plan::Run(
                run_env("dkim_retire", g.json)
                    .with("M_DOMAIN", g.get("domain"))
                    .with("M_SELECTOR", g.get("selector")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/dkim")),
        other => Err(usage(format!(
            "unknown dkim command '{other}' — usage: {CLI_NAME} hostmail dkim <show|rotate|retire>"
        ))),
    }
}

/// `cmd_hostmail_dmarc`.
fn dmarc(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/dmarc"));
    };
    match *sub {
        "show" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/dmarc/show"));
            }
            one_domain(rest, "dmarc", "show", "dmarc_show")
        }
        "report-to" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/dmarc/report-to"));
            }
            let short = format!("{CLI_NAME} hostmail dmarc report-to <domain> --address <minted>");
            let g = scan(
                rest,
                &Spec {
                    slots: &["domain"],
                    values: &[
                        ("--domain", "domain", "--domain requires a value"),
                        ("--address", "address", "--address requires a value"),
                    ],
                    switches: &[],
                    unknown: &format!("unknown flag '{{w}}' — usage: {short} [--json]"),
                    extra: "unexpected extra argument '{w}'",
                },
            )?;
            if g.get("domain").is_empty() || g.get("address").is_empty() {
                return Err(usage(short));
            }
            Ok(Plan::Run(
                run_env("dmarc_report_to", g.json)
                    .with("M_DOMAIN", g.get("domain"))
                    .with("M_ADDRESS", g.get("address")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/dmarc")),
        other => Err(usage(format!(
            "unknown dmarc command '{other}' — usage: {CLI_NAME} hostmail dmarc <show|report-to>"
        ))),
    }
}

/// `cmd_hostmail_ptr`.
fn ptr(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/ptr"));
    };
    match *sub {
        "show" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/ptr/show"));
            }
            let use_ = format!("usage: {CLI_NAME} hostmail ptr show [--json]");
            let g = scan(
                rest,
                &Spec {
                    slots: &[],
                    values: &[],
                    switches: &[],
                    unknown: &format!("unknown flag '{{w}}' — {use_}"),
                    extra: &format!("unexpected argument '{{w}}' — {use_}"),
                },
            )?;
            Ok(Plan::Run(run_env("ptr_show", g.json)))
        }
        "set" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/ptr/set"));
            }
            let g = scan(
                rest,
                &Spec {
                    slots: &["hostname"],
                    values: &[("--hostname", "hostname", "--hostname requires a value")],
                    switches: &[],
                    unknown: &format!(
                        "unknown flag '{{w}}' — usage: {CLI_NAME} hostmail ptr set <hostname> [--json]"
                    ),
                    extra: "unexpected extra argument '{w}' — one hostname only",
                },
            )?;
            if g.get("hostname").is_empty() {
                return Err(usage(format!("{CLI_NAME} hostmail ptr set <hostname>")));
            }
            Ok(Plan::Run(
                run_env("ptr_set", g.json).with("M_HOSTNAME", g.get("hostname")),
            ))
        }
        "--help" | "-h" => Ok(Plan::Help("hostmail/ptr")),
        other => Err(usage(format!(
            "unknown ptr command '{other}' — usage: {CLI_NAME} hostmail ptr <show|set>"
        ))),
    }
}

/// `dirname -- "$p"`.
fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/".into();
    }
    match t.rfind('/') {
        None => ".".into(),
        Some(i) => {
            let d = t[..i].trim_end_matches('/');
            if d.is_empty() {
                "/".into()
            } else {
                d.into()
            }
        }
    }
}

/// `cmd_hostmail_profile`: the file checks run here, before anything is
/// sent, as bash runs them.
fn profile(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/profile"));
    }
    let use_ = format!("{CLI_NAME} hostmail profile <addr> --apple [--out <file> [--force]] [--json]");
    let (mut address, mut apple, mut out, mut force, mut json) =
        (String::new(), false, String::new(), false, false);
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--apple" => apple = true,
            "--json" => json = true,
            "--force" => force = true,
            "--out" => match words.get(i + 1) {
                Some(v) if !v.is_empty() => {
                    out = v.to_string();
                    i += 1;
                }
                _ => {
                    return Err(usage(format!(
                        "--out requires a file path — usage: {use_}"
                    )))
                }
            },
            f if f.starts_with('-') => {
                return Err(usage(format!("unknown flag '{f}' — usage: {use_}")));
            }
            w => {
                if address.is_empty() {
                    address = w.into();
                } else {
                    return Err(usage(format!(
                        "unexpected extra argument '{w}' — one <addr> only"
                    )));
                }
            }
        }
        i += 1;
    }
    if address.is_empty() {
        return Err(usage(use_));
    }
    if !apple {
        return Err(usage(format!(
            "pass --apple (the Apple .mobileconfig is the only profile format) — usage: {use_}"
        )));
    }
    if force && out.is_empty() {
        return Err(usage("--force only goes with --out <file>"));
    }
    if !out.is_empty() {
        let path = Path::new(&out);
        if path.is_dir() {
            return Err(usage(format!(
                "--out {out} is a directory — pass a file path (e.g. {out}/{address}.mobileconfig)"
            )));
        }
        if path.symlink_metadata().is_ok() && !force {
            return Err(usage(format!(
                "{out} already exists — pass --force to overwrite (nothing was minted)"
            )));
        }
        let dir = dirname(&out);
        if !Path::new(&dir).is_dir() {
            return Err(usage(format!("--out directory {dir} does not exist")));
        }
    }
    Ok(Plan::Run(
        run_env("profile", json)
            .with("M_ADDRESS", address)
            .with("M_OUT", out)
            .with("M_FORCE", if force { "true" } else { "false" }),
    ))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "dkim_show" => dkim_show_py(py_),
        "dkim_rotate" => dkim_rotate_py(py_),
        "dkim_retire" => dkim_retire_py(py_),
        "dmarc_show" => dmarc_show_py(py_),
        "dmarc_report_to" => dmarc_report_py(py_),
        "ptr_show" => ptr_show_py(py_),
        "ptr_set" => ptr_set_py(py_),
        "profile" => profile_py(py_),
        _ => return None,
    })
}

fn domain_params(py_: &Py) -> Vec<(String, String)> {
    vec![("domain".to_string(), py_.env("M_DOMAIN").to_string())]
}

fn dkim_show_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/dkim", &domain_params(py_), line::DKIM_SHOW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let dom = or_env(py_, &d, "domain", "M_DOMAIN", line::DKIM_SHOW_FOR - 1)?;
    py_.print(&format!("dkim  {dom}"));
    for s in items(g(&d, "signatures", line::DKIM_SHOW_FOR)?, line::DKIM_SHOW_FOR)? {
        let l = line::DKIM_SHOW_ROW;
        let sel = or_s(g(&s, "selector", l)?, "?");
        let stage = or_s(g(&s, "stage", l + 1)?, "?");
        let typ = or_s(g(&s, "type", l + 2)?, "?");
        let m = or_s(g(&s, "match", l + 3)?, "?");
        py_.print(&format!(
            "  {}  {}  {}  {m}",
            pyx::ljust(&sel, 28),
            pyx::ljust(&stage, 8),
            pyx::ljust(&typ, 22)
        ));
        if let Some(e) = g(&s, "expected", line::DKIM_SHOW_EXPECTED)?.filter(py::truthy) {
            py_.print(&format!(
                "    TXT {}",
                or_s(g(&s, "name", line::DKIM_SHOW_NAME)?, "")
            ));
            py_.print(&format!("    {}", py::str_of(&e)));
        }
    }
    Ok(())
}

fn dkim_rotate_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("domain", pyx::env_v(py_, "M_DOMAIN"))]);
    let d = py_.post("/cli/mail/dkim", &body, line::DKIM_ROTATE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let dom = or_env(py_, &d, "domain", "M_DOMAIN", line::DKIM_ROTATE_CALL + 5)?;
    let planted = g(&d, "planted", line::DKIM_ROTATE_CALL + 6)?;
    py_.print(&format!(
        "rotated dkim on {dom}  planted={}",
        s_of(planted.clone())
    ));
    for rec in items(g(&d, "records", line::DKIM_ROTATE_FOR)?, line::DKIM_ROTATE_FOR)? {
        let l = line::DKIM_ROTATE_ROW;
        let name = or_s(g(&rec, "name", l)?, "?");
        let value = match g(&rec, "value", l)?.filter(py::truthy) {
            Some(v) => py::str_of(&v),
            None => or_s(g(&rec, "expectedDisplay", l)?, ""),
        };
        py_.print(&format!("{name}  TXT  {value}"));
    }
    if !p::truthy(planted.as_ref()) {
        py_.print("plant this TXT at your DNS host (this box is not dnsWrite)");
    }
    Ok(())
}

fn dkim_retire_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("domain", pyx::env_v(py_, "M_DOMAIN")),
        ("selector", pyx::env_v(py_, "M_SELECTOR")),
    ]);
    let d = py_.post("/cli/mail/dkim/retire", &body, line::DKIM_RETIRE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DKIM_RETIRE_CALL + 6;
    let sel = or_env(py_, &d, "selector", "M_SELECTOR", l)?;
    let dom = or_env(py_, &d, "domain", "M_DOMAIN", l + 1)?;
    py_.print(&format!("retired selector {sel} on {dom}"));
    Ok(())
}

fn dmarc_show_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/dmarc", &domain_params(py_), line::DMARC_SHOW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DMARC_SHOW_CALL + 4;
    py_.print(&format!(
        "dmarc {}",
        or_env(py_, &d, "domain", "M_DOMAIN", l)?
    ));
    let rec = or_dict(g(&d, "dmarc", l + 1)?);
    py_.print(&format!(
        "  txt     : {}",
        or_s(g(&rec, "expected", line::DMARC_SHOW_REC)?, "?")
    ));
    py_.print(&format!(
        "  live    : {}",
        or_s(g(&rec, "match", line::DMARC_SHOW_MATCH)?, "?")
    ));
    let uri = or_s(g(&d, "reportAddressUri", l + 5)?, "mailto:postmaster");
    let minted = if p::truthy(g(&d, "reportAddressMinted", l + 6)?.as_ref()) {
        ""
    } else {
        "  (not a minted inbox)"
    };
    py_.print(&format!("  report  : {uri}{minted}"));
    py_.print(&format!(
        "  plant   : {}",
        or_s(g(&d, "suggestedRua", l + 7)?, "")
    ));
    if let Some(n) = g(&d, "note", l + 8)?.filter(py::truthy) {
        py_.print(&format!("  note    : {}", py::str_of(&n)));
    }
    Ok(())
}

fn dmarc_report_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![
        ("domain", pyx::env_v(py_, "M_DOMAIN")),
        ("address", pyx::env_v(py_, "M_ADDRESS")),
    ]);
    let d = py_.post("/cli/mail/dmarc", &body, line::DMARC_REPORT_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::DMARC_REPORT_CALL + 6;
    let dom = or_env(py_, &d, "domain", "M_DOMAIN", l)?;
    let uri = or_env(py_, &d, "reportAddressUri", "M_ADDRESS", l + 1)?;
    py_.print(&format!("dmarc report-to {dom} → {uri}"));
    py_.print(&format!(
        "plant DNS: {}",
        or_s(g(&d, "suggestedRua", l + 2)?, "")
    ));
    Ok(())
}

fn ptr_show_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/ptr", &[], line::PTR_SHOW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::PTR_SHOW_CALL + 4;
    py_.print(&format!(
        "origin : {}",
        or_s(g(&d, "originIpv4", l)?, "?")
    ));
    py_.print(&format!("ptr    : {}", or_s(g(&d, "ptr", l + 1)?, "(none)")));
    let helo = match g(&d, "helo", l + 2)?.filter(py::truthy) {
        Some(h) => py::str_of(&h),
        None => or_s(g(&d, "hostname", l + 2)?, "(none)"),
    };
    py_.print(&format!("helo   : {helo}"));
    let a = p::or(g(&d, "aRecords", l + 3)?, Value::Array(vec![]));
    let shown = if py::truthy(&a) {
        pyx::join(", ", &a, line::PTR_SHOW_A)?
    } else {
        "(none)".into()
    };
    py_.print(&format!("A      : {shown}"));
    let lm = line::PTR_SHOW_A + 2;
    let m1 = s_of(g(&d, "ptrMatchesHelo", lm)?);
    let m2 = s_of(g(&d, "aMatchesOrigin", lm)?);
    let m3 = s_of(g(&d, "aligned", lm)?);
    py_.print(&format!(
        "match  : ptr==helo={m1}  A==origin={m2}  aligned={m3}"
    ));
    let rc = or_dict(g(&d, "relayCheck", line::PTR_SHOW_RELAY)?);
    let un = p::or(
        g(&rc, "unresolved", line::PTR_SHOW_RELAY)?,
        Value::Array(vec![]),
    );
    if py::truthy(&un) {
        let j = pyx::join(", ", &un, line::PTR_SHOW_UNRESOLVED)?;
        py_.print(&format!(
            "relays : could not resolve {j} (ptr set refuses until it does; retry)"
        ));
    }
    Ok(())
}

fn ptr_set_py(py_: &mut Py) -> R<()> {
    let body = obj(vec![("hostname", pyx::env_v(py_, "M_HOSTNAME"))]);
    let d = py_.post("/cli/mail/ptr/set", &body, line::PTR_SET_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::PTR_SET_CALL + 5;
    let host = or_env(py_, &d, "hostname", "M_HOSTNAME", l)?;
    let helo = or_s(g(&d, "helo", l + 1)?, "?");
    let origin = or_s(g(&d, "originIpv4", l + 2)?, "?");
    let aligned = s_of(g(&d, "aligned", l + 3)?);
    py_.print(&format!(
        "ptr set  hostname={host}  helo={helo}  origin={origin}  aligned={aligned}"
    ));
    let ls = line::PTR_SET_SOURCE;
    if g(&d, "ptrSource", ls)?.is_some_and(|v| p::eq_str(&v, "authoritative")) {
        py_.print(&format!(
            "published ptr: {}  ({ZONE}, authoritative nameservers)",
            s_of(g(&d, "ptr", ls + 1)?)
        ));
        py_.print(&format!(
            "local ptr    : {}",
            or_s(g(&d, "ptrLocal", ls + 2)?, "(none)")
        ));
    } else if let Some(ptr) = g(&d, "ptr", ls + 3)?.filter(py::truthy) {
        py_.print(&format!("live ptr: {}", py::str_of(&ptr)));
    }
    if let Some(n) = g(&d, "resolverNote", ls + 5)?.filter(py::truthy) {
        py_.print(&format!("note: {}", py::str_of(&n)));
    }
    let settings = or_dict(g(&d, "settings", line::PTR_SET_WARNING - 1)?);
    if let Some(w) = g(&settings, "warning", line::PTR_SET_WARNING)?.filter(py::truthy) {
        py_.print(&format!("warning: {}", py::str_of(&w)));
    }
    let dc = or_dict(g(&d, "defaultCertificate", line::PTR_SET_DC - 1)?);
    if g(&dc, "state", line::PTR_SET_DC)?.is_some_and(|v| p::eq_str(&v, "moved")) {
        py_.print(&format!(
            "default certificate: moved to this name's newest ({})",
            s_of(g(&dc, "certificateId", line::PTR_SET_DC + 1)?)
        ));
    } else if let Some(w) = g(&dc, "warning", line::PTR_SET_DC_WARNING)?.filter(py::truthy) {
        py_.print(&format!("warning: {}", py::str_of(&w)));
    }
    Ok(())
}

/// `os.open(out, flags, 0o600)`, `fchmod` and the write of `xml`; the
/// error's text as `e.strerror` gives it.
fn write_profile(out: &str, force: bool, xml: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut o = std::fs::OpenOptions::new();
    o.write(true).mode(0o600);
    if force {
        o.create(true).truncate(true);
    } else {
        o.create_new(true);
    }
    let strerror = |e: std::io::Error| strerror(&e);
    let mut f = o.open(out).map_err(strerror)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(strerror)?;
    f.write_all(xml).map_err(strerror)
}

/// `e.strerror`: the C library's text for the error, without Rust's
/// ` (os error N)`.
fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.rfind(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

fn profile_py(py_: &mut Py) -> R<()> {
    let out_path = py_.env("M_OUT").to_string();
    let force = py_.env("M_FORCE") == "true";
    if !out_path.is_empty() && Path::new(&out_path).symlink_metadata().is_ok() && !force {
        return Err(py_.die_with(
            "usage",
            &format!("{out_path} already exists — pass --force to overwrite (nothing was minted)"),
            None,
        ));
    }
    let body = obj(vec![
        ("address", pyx::env_v(py_, "M_ADDRESS")),
        ("apple", Value::Bool(true)),
    ]);
    let mut d = py_.call_full(
        Method::Post,
        "/cli/mail/profile",
        &[],
        Some(&body),
        Secs::Int(60),
        None,
        line::PROFILE_CALL,
    )?;
    let xml = p::or(g(&d, "profile", line::PROFILE_CALL + 1)?, sv(""));
    if !py::truthy(&xml) {
        return Err(py_.die_with("error", "daemon returned no profile", None));
    }
    let lr = line::PROFILE_CALL + 4;
    let revoke = match g(&d, "revoke", lr)?.filter(py::truthy) {
        Some(r) => py::str_of(&r),
        None => format!(
            "{CLI_NAME} hostmail app-password revoke {} {}",
            py_.env("M_ADDRESS"),
            or_s(g(&d, "appPasswordId", lr)?, "<id>")
        ),
    };
    if !out_path.is_empty() {
        let text = match &xml {
            Value::Str(t) => Some(p::text(t)),
            _ => None,
        };
        let bytes = text.as_deref().map(str::as_bytes).unwrap_or(&[]);
        if let Err(e) = write_profile(&out_path, force, bytes) {
            return Err(py_.die_with(
                "error",
                &format!("could not write {out_path} ({e}) — an app password was minted anyway; revoke it: {revoke}"),
                None,
            ));
        }
        if text.is_none() {
            return Err(raise_at(
                line::PROFILE_WRITE,
                format!(
                    "AttributeError: '{}' object has no attribute 'encode'",
                    py::type_name(&xml)
                ),
            ));
        }
    }
    if py_.json {
        if !out_path.is_empty() {
            if let Value::Object(pairs) = &mut d {
                pairs.retain(|(k, _)| p::text(k) != "profile");
                match pairs.iter_mut().find(|(k, _)| p::text(k) == "out") {
                    Some(slot) => slot.1 = sv(&out_path),
                    None => pairs.push((Text::from("out"), sv(&out_path))),
                }
            }
        }
        py_.print_json(&d);
    } else if !out_path.is_empty() {
        let l = line::PROFILE_INCLUDES;
        py_.print(&format!(
            "wrote    : {out_path}  (mode 0600, unsigned — iOS/macOS show \"Unverified\")"
        ));
        let a = match g(&d, "address", l - 2)?.filter(py::truthy) {
            Some(v) => py::str_of(&v),
            None => py_.env("M_ADDRESS").to_string(),
        };
        py_.print(&format!("address  : {a}"));
        py_.print(&format!(
            "server   : {}",
            or_s(g(&d, "mailHost", l - 1)?, "?")
        ));
        let inc = p::or(g(&d, "includes", l)?, Value::Array(vec![sv("mail")]));
        py_.print(&format!("includes : {}", pyx::join(", ", &inc, l)?));
        let id = or_s(g(&d, "appPasswordId", l + 1)?, "?");
        let desc = or_s(g(&d, "description", l + 1)?, "?");
        py_.print(&format!(
            "password : app password {id} ({desc}) — inside the file only"
        ));
        py_.print(&format!("revoke   : {revoke}"));
        for n in items(g(&d, "notes", line::PROFILE_NOTES)?, line::PROFILE_NOTES)? {
            py_.print(&format!("note     : {}", py::str_of(&n)));
        }
        py_.print("next     : AirDrop or email the file to the Mac/iPhone, then Settings → Profile Downloaded → Install. Delete the file afterwards.");
    } else {
        let Value::Str(t) = &xml else {
            return Err(raise_at(
                line::PROFILE_STDOUT,
                format!(
                    "TypeError: write() argument must be str, not {}",
                    py::type_name(&xml)
                ),
            ));
        };
        py_.stdout.extend_from_slice(p::text(t).as_bytes());
        let l = line::PROFILE_STDOUT + 2;
        let id = or_s(g(&d, "appPasswordId", l)?, "?");
        let desc = or_s(g(&d, "description", l)?, "?");
        let mut err = format!("app password {id} ({desc}) is inside this profile; revoke: {revoke}\n");
        // The summary goes out as it is written; stdout waits for the end.
        let notes = items(
            g(&d, "notes", line::PROFILE_STDERR_NOTES)?,
            line::PROFILE_STDERR_NOTES,
        );
        let (notes, raised): (Vec<Value>, Option<End>) = match notes {
            Ok(n) => (n, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        for n in notes {
            err.push_str(&format!("note: {}\n", py::str_of(&n)));
        }
        let _ = out::write_flushed(&mut std::io::stderr().lock(), err.as_bytes());
        if let Some(e) = raised {
            return Err(e);
        }
    }
    Ok(())
}
