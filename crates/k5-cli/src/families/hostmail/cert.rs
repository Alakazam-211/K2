//! The mail host's certificates, outbound settings and backup choice:
//! `hostmail cert` (renew, names, owner), `hostmail outbound` and
//! `hostmail backup` (status, plan, set), as their `cmd_hostmail_*`
//! functions read the words (`cli/src/hostmail-app-cert-backup.sh`) and
//! `_mail_py`'s verbs answer (`cli/src/mail.sh`).

use super::pyx::{self, g, items, or_dict, or_s, s_of, sv};
use super::words::{wants_help, Got};
use crate::exit::Exit;
use crate::families::mail::core::{
    obj, py_float, run_env, unix_ago, usage, At, End, Plan, Py, Secs, Within, MODULE, R,
};
use crate::families::mail::py::{self as p, attr_get};
use crate::out::json::Value;
use crate::out::py;
use crate::transport::urllib::Method;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["cert", "outbound", "backup"];

/// The daemon's name for itself as a certificate's owner or a setting's
/// author: a wire value, the same in every brand.
const WIRE_SELF: &str = "k2";

/// The lines of `_mail_py` where these verbs call or raise. `tests` pins
/// each to its statement.
pub mod line {
    pub const RENEW_CALL: u32 = 2297;
    pub const RENEW_PRINT: u32 = 2301;
    pub const OWNER_ACME_CALL: u32 = 2306;
    pub const OWNER_ACME_PRINT: u32 = 2310;
    pub const OWNER_CALL: u32 = 2312;
    pub const OWNER_ERROR: u32 = 2320;
    pub const OWNER_AUTO: u32 = 2322;
    pub const OWNER_NAMES: u32 = 2323;
    pub const OWNER_TYPE: u32 = 2324;
    pub const OWNER_MANUAL: u32 = 2328;
    pub const OWNER_RESTORE: u32 = 2329;
    pub const GIB_FLOAT: u32 = 2335;
    pub const GIB_D: u32 = 2342;
    pub const DUR_INT: u32 = 2345;
    pub const DUR_HOURS: u32 = 2352;
    pub const PB_MODE: u32 = 2354;
    pub const PB_BY: u32 = 2355;
    pub const PB_REASON: u32 = 2358;
    pub const PB_WINDOW: u32 = 2360;
    pub const PB_RETENTION: u32 = 2362;
    pub const PB_LOCAL: u32 = 2364;
    pub const PB_OFFSITE: u32 = 2365;
    pub const PB_CAP: u32 = 2366;
    pub const PB_KEEP: u32 = 2368;
    pub const PB_OFF_KEEP: u32 = 2370;
    pub const PB_OBSERVER: u32 = 2371;
    pub const PB_STATUS: u32 = 2372;
    pub const PB_LAST: u32 = 2373;
    pub const PB_DAYS: u32 = 2375;
    pub const PB_CHURN: u32 = 2377;
    pub const PB_CHURN_DEL: u32 = 2380;
    pub const PB_CHURN_NEW: u32 = 2381;
    pub const PB_STORE_IF: u32 = 2382;
    pub const PB_STORE: u32 = 2384;
    pub const PB_STORE_FS: u32 = 2385;
    pub const PB_ERR: u32 = 2386;
    pub const PB_CONFIG: u32 = 2388;
    pub const PB_NOTE: u32 = 2390;
    pub const STATUS_CALL: u32 = 2392;
    pub const STATUS_PRINT: u32 = 2396;
    pub const PLAN_CALL: u32 = 2404;
    pub const PLAN_STORE: u32 = 2411;
    pub const PLAN_STORE_2: u32 = 2412;
    pub const PLAN_DISK: u32 = 2414;
    pub const PLAN_DISK_2: u32 = 2415;
    pub const PLAN_CHURN: u32 = 2417;
    pub const PLAN_CHURN_2: u32 = 2418;
    pub const PLAN_LOCAL: u32 = 2420;
    pub const PLAN_OFFSITE: u32 = 2421;
    pub const PLAN_KEEP: u32 = 2423;
    pub const PLAN_CAP: u32 = 2424;
    pub const PLAN_OFF_KEEP: u32 = 2425;
    pub const PLAN_MODES: u32 = 2428;
    pub const PLAN_VERDICT: u32 = 2430;
    pub const PLAN_AVAILABLE: u32 = 2431;
    pub const PLAN_UNAVAILABLE: u32 = 2432;
    pub const PLAN_LOCAL_DISK: u32 = 2438;
    pub const PLAN_LOCAL_DISK_2: u32 = 2439;
    pub const PLAN_OFF: u32 = 2440;
    pub const PLAN_FU: u32 = 2442;
    pub const PLAN_UPLOAD: u32 = 2444;
    pub const PLAN_UPLOAD_2: u32 = 2445;
    pub const PLAN_TEMP: u32 = 2447;
    pub const PLAN_DOWN: u32 = 2448;
    pub const PLAN_PROTECTS: u32 = 2449;
    pub const PLAN_NOTES: u32 = 2451;
    pub const PLAN_HASH: u32 = 2453;
    pub const SET_INT: u32 = 2463;
    pub const SET_CALL: u32 = 2471;
    pub const SET_MODE: u32 = 2475;
    pub const SET_PRINT: u32 = 2476;
    pub const DANE_CALL: u32 = 2481;
    pub const DANE_PRINT: u32 = 2485;
    pub const OUT_CALL: u32 = 2487;
    pub const OUT_WHO: u32 = 2493;
    pub const OUT_DANE: u32 = 2494;
    pub const OUT_DANE_ROW: u32 = 2495;
    pub const OUT_DANE_WHO: u32 = 2496;
    pub const OUT_DNSSEC: u32 = 2497;
    pub const OUT_PENDING: u32 = 2498;
    pub const OUT_PENDING_ROW: u32 = 2500;
    pub const NAMES_LIST_CALL: u32 = 2508;
    pub const NAMES_CALL: u32 = 2514;
    pub const NAMES_ROWS: u32 = 2518;
    pub const NAMES_FOR: u32 = 2523;
    pub const NAMES_CERT: u32 = 2525;
    pub const NAMES_EXP: u32 = 2526;
    pub const NAMES_ROW: u32 = 2528;
    pub const NAMES_ROW_2: u32 = 2529;
    pub const NAMES_DNS: u32 = 2530;
    pub const NAMES_REASON: u32 = 2532;
    pub const NAMES_LAST: u32 = 2534;
    pub const NAMES_R_EXP: u32 = 2537;
    pub const NAMES_R_ROW: u32 = 2538;
    pub const NAMES_DETAIL: u32 = 2539;
    pub const NAMES_RELOAD: u32 = 2542;
    pub const NAMES_METHOD: u32 = 2543;
    pub const NAMES_RELOAD_ERR: u32 = 2544;
    pub const NAMES_FAILED: u32 = 2547;
}

/// The words after `hostmail <verb>`.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "cert" => cert(words),
        "outbound" => outbound(words),
        "backup" => backup(words),
        _ => return Plan::Shell,
    };
    r.unwrap_or_else(|p| p)
}

/// A loop that knows `--json` and the given switches; any other word is
/// `bad` with `{w}` as the word.
fn switches(words: &[&str], on: &[(&'static str, &'static str)], bad: &str) -> Result<Got, Plan> {
    super::words::scan(
        words,
        &super::words::Spec {
            slots: &[],
            values: &[],
            switches: on,
            unknown: bad,
            extra: bad,
        },
    )
}

/// `cmd_hostmail_cert`.
fn cert(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/cert"));
    };
    match *sub {
        "renew" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/cert/renew"));
            }
            let g = switches(
                rest,
                &[],
                &format!(
                    "unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail cert renew [--json]"
                ),
            )?;
            Ok(Plan::Run(run_env("cert_renew", g.json)))
        }
        "owner" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/cert/owner"));
            }
            let g = switches(
                rest,
                &[("--stalwart-acme", "acme")],
                &format!("unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail cert owner [--stalwart-acme] [--json]"),
            )?;
            let action = if g.on("acme") {
                "stalwart-acme"
            } else {
                "show"
            };
            Ok(Plan::Run(
                run_env("cert_owner", g.json).with("M_CERT_ACTION", action),
            ))
        }
        "names" => cert_names(rest),
        "--help" | "-h" => Ok(Plan::Help("hostmail/cert")),
        other => Err(usage(format!(
            "unknown cert command '{other}' — usage: {CLI_NAME} hostmail cert <renew|names|owner>"
        ))),
    }
}

/// `cmd_hostmail_cert_names`.
fn cert_names(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/cert/names"));
    }
    let (mut action, mut rest) = ("list", words);
    if let Some(first @ ("list" | "issue" | "renew")) = words.first().copied() {
        action = first;
        rest = &words[1..];
    }
    let use_ = format!(
        "usage: {CLI_NAME} hostmail cert names [list|issue|renew] [--name <host>] [--json]"
    );
    let (mut json, mut name) = (false, String::new());
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--json" => json = true,
            "--name" => {
                if action == "list" {
                    return Err(usage(format!("--name goes with issue|renew — {use_}")));
                }
                name = rest
                    .get(i + 1)
                    .ok_or_else(|| usage("--name requires a host (e.g. autoconfig.example.com)"))?
                    .to_string();
                i += 1;
            }
            w => return Err(usage(format!("unexpected argument '{w}' — {use_}"))),
        }
        i += 1;
    }
    Ok(Plan::Run(
        run_env("cert_names", json)
            .with("M_CERT_ACTION", action)
            .with("M_NAME", name),
    ))
}

/// `cmd_hostmail_outbound`.
fn outbound(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/outbound"));
    }
    let use_ = format!(
        "usage: {CLI_NAME} hostmail outbound [show] [--json] | {CLI_NAME} hostmail outbound dane on [--json]"
    );
    let (mut action, mut json) = ("show", false);
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--json" => json = true,
            "show" => {}
            "dane" => {
                if words.get(i + 1) != Some(&"on") {
                    return Err(usage(use_));
                }
                action = "dane-on";
                i += 1;
            }
            w => return Err(usage(format!("unexpected argument '{w}' — {use_}"))),
        }
        i += 1;
    }
    Ok(Plan::Run(
        run_env("outbound", json).with("M_OUTBOUND_ACTION", action),
    ))
}

/// `_hostmail_backup_count`: a whole number, or the usage line.
fn count(flag: &str, v: &str) -> Result<(), Plan> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(usage(format!("{flag} must be a whole number, got '{v}'")));
    }
    Ok(())
}

/// `cmd_hostmail_backup`.
fn backup(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("hostmail/backup"));
    };
    match *sub {
        "status" => {
            if wants_help(rest) {
                return Ok(Plan::Help("hostmail/backup/status"));
            }
            let g = switches(
                rest,
                &[],
                &format!(
                    "unexpected argument '{{w}}' — usage: {CLI_NAME} hostmail backup status [--json]"
                ),
            )?;
            Ok(Plan::Run(run_env("backup_status", g.json)))
        }
        "plan" => backup_plan(rest),
        "set" => backup_set(rest),
        "--help" | "-h" => Ok(Plan::Help("hostmail/backup")),
        other => Err(usage(format!(
            "unknown backup command '{other}' — usage: {CLI_NAME} hostmail backup <status|plan|set>"
        ))),
    }
}

/// `cmd_hostmail_backup_plan`.
fn backup_plan(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/backup/plan"));
    }
    let use_ = format!("usage: {CLI_NAME} hostmail backup plan [--bandwidth-mbps N] [--keep-daily N] [--keep-weekly N] [--max-local-gb G] [--json]");
    let mut env = run_env("backup_plan", false);
    let mut json = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let key = match w {
            "--json" => {
                json = true;
                i += 1;
                continue;
            }
            "--bandwidth-mbps" => "M_BK_BANDWIDTH",
            "--keep-daily" => "M_BK_KEEP_DAILY",
            "--keep-weekly" => "M_BK_KEEP_WEEKLY",
            "--max-local-gb" => "M_BK_MAX_LOCAL_GB",
            _ => return Err(usage(format!("unexpected argument '{w}' — {use_}"))),
        };
        let v = words
            .get(i + 1)
            .ok_or_else(|| usage(format!("{w} requires a value — {use_}")))?;
        if w.starts_with("--keep") {
            count(w, v)?;
        }
        env.set(key, *v);
        i += 2;
    }
    env.set("M_JSON", if json { "true" } else { "false" });
    Ok(Plan::Run(env))
}

/// `cmd_hostmail_backup_set`.
fn backup_set(words: &[&str]) -> Result<Plan, Plan> {
    if wants_help(words) {
        return Ok(Plan::Help("hostmail/backup/set"));
    }
    let use_ = format!("usage: {CLI_NAME} hostmail backup set --mode local|offsite-only|both|none [--reason <text>] [--keep-daily N --keep-weekly N] [--offsite-keep-daily N --offsite-keep-weekly N --offsite-keep-monthly N] [--window HH:MM-HH:MM] [--max-local-gb G] --confirm-plan <hash> [--json]");
    let mut env = run_env("backup_set", false);
    let mut json = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let (key, counted) = match w {
            "--json" => {
                json = true;
                i += 1;
                continue;
            }
            "--mode" => ("M_BK_MODE", false),
            "--reason" => ("M_REASON", false),
            "--keep-daily" => ("M_BK_KEEP_DAILY", true),
            "--keep-weekly" => ("M_BK_KEEP_WEEKLY", true),
            "--offsite-keep-daily" => ("M_BK_OFF_DAILY", true),
            "--offsite-keep-weekly" => ("M_BK_OFF_WEEKLY", true),
            "--offsite-keep-monthly" => ("M_BK_OFF_MONTHLY", true),
            "--window" => ("M_BK_WINDOW", false),
            "--max-local-gb" => ("M_BK_MAX_LOCAL_GB", false),
            "--confirm-plan" => ("M_BK_CONFIRM", false),
            _ => return Err(usage(format!("unexpected argument '{w}' — {use_}"))),
        };
        let v = words
            .get(i + 1)
            .ok_or_else(|| usage(format!("{w} requires a value — {use_}")))?;
        if counted {
            count(w, v)?;
        }
        env.set(key, *v);
        i += 2;
    }
    let mode = env.get("M_BK_MODE").to_string();
    match mode.as_str() {
        "local" | "offsite-only" | "both" | "none" => {}
        "" => return Err(usage(format!("--mode is required — {use_}"))),
        m => {
            return Err(usage(format!(
                "--mode must be local, offsite-only, both or none (got '{m}')"
            )))
        }
    }
    if mode == "none" && env.get("M_REASON").is_empty() {
        return Err(usage(
            "--mode none needs --reason \"<why>\" (e.g. \"provider snapshots daily\")",
        ));
    }
    if env.get("M_BK_CONFIRM").is_empty() {
        return Err(usage(format!("--confirm-plan <hash> is required — run {CLI_NAME} hostmail backup plan (same retention flags) and pass its planHash")));
    }
    env.set("M_JSON", if json { "true" } else { "false" });
    Ok(Plan::Run(env))
}

/// This module's branches of `_mail_py`. `None`: not one of them.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "cert_renew" => renew_py(py_),
        "cert_owner" => owner_py(py_),
        "backup_status" => status_py(py_),
        "backup_plan" => plan_py(py_),
        "backup_set" => set_py(py_),
        "outbound" => outbound_py(py_),
        "cert_names" => names_py(py_),
        _ => return None,
    })
}

fn renew_py(py_: &mut Py) -> R<()> {
    let d = py_.post("/cli/mail/cert/renew", &obj(vec![]), line::RENEW_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::RENEW_PRINT;
    let text = match g(&d, "hint", l)?.filter(py::truthy) {
        Some(h) => py::str_of(&h),
        None => format!(
            "ACME renewal queued for {}",
            or_s(g(&d, "hostname", l)?, "mail hostname")
        ),
    };
    py_.print(&text);
    Ok(())
}

fn owner_py(py_: &mut Py) -> R<()> {
    if py_.env("M_CERT_ACTION") == "stalwart-acme" {
        let d = py_.post(
            "/cli/mail/cert/owner",
            &obj(vec![("action", sv("stalwart-acme"))]),
            line::OWNER_ACME_CALL,
        )?;
        return pyx::hint_or(
            py_,
            &d,
            "Stalwart ACME orders the mail host only",
            line::OWNER_ACME_PRINT,
        );
    }
    let d = py_.get("/cli/mail/cert/owner", &[], line::OWNER_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let owner = p::or(g(&d, "owner", line::OWNER_CALL + 4)?, sv("?"));
    let owner = if py::truthy(&owner) { owner } else { sv("?") };
    py_.print(&format!(
        "hostname : {}",
        or_s(g(&d, "hostname", line::OWNER_CALL + 5)?, "?")
    ));
    py_.print(&format!("owner    : {}", py::str_of(&owner)));
    let acme = or_dict(g(&d, "stalwartAcme", line::OWNER_ERROR - 1)?);
    let typ = |l: u32| g(&acme, "type", l);
    if let Some(e) = g(&acme, "error", line::OWNER_ERROR)?.filter(py::truthy) {
        py_.print(&format!("stalwart : could not read ({})", py::str_of(&e)));
    } else if typ(line::OWNER_AUTO)?.is_some_and(|v| p::eq_str(&v, "Automatic")) {
        let names = p::or(g(&acme, "names", line::OWNER_NAMES)?, Value::Array(vec![]));
        let names = if py::truthy(&names) {
            names
        } else {
            Value::Array(vec![])
        };
        let j = pyx::join(", ", &names, line::OWNER_NAMES)?;
        let shown = if j.is_empty() {
            "(empty = Stalwart's wide default)".to_string()
        } else {
            j
        };
        py_.print(&format!("stalwart : ACME Automatic, names: {shown}"));
    } else if let Some(t) = typ(line::OWNER_TYPE)?.filter(py::truthy) {
        py_.print(&format!("stalwart : ACME {}", py::str_of(&t)));
    }
    if p::eq_str(&owner, WIRE_SELF) {
        py_.print(&format!(
            "           {PRODUCT_NAME} issues, installs and renews it; Stalwart ACME stays Manual."
        ));
    } else if typ(line::OWNER_MANUAL)?.is_some_and(|v| p::eq_str(&v, "Manual")) {
        let fallback = format!("{CLI_NAME} hostmail cert owner --stalwart-acme");
        let r = or_s(g(&d, "restoreCommand", line::OWNER_RESTORE)?, &fallback);
        py_.print(&format!(
            "           Stalwart ACME is off and {PRODUCT_NAME} does not own it — {r}"
        ));
    }
    Ok(())
}

/// A frame inside one of the backup helpers: `v.get(key)` at `l` of `name`.
fn gin(v: &Value, key: &str, l: u32, name: &'static str) -> R<Option<Value>> {
    attr_get(v, key).at(l, name)
}

/// `float(n)`, or `None` where Python's `float` raises `TypeError` or
/// `ValueError`; an int too big for a float is the `Err`.
fn float_of(n: &Value) -> Result<Option<f64>, String> {
    Ok(match n {
        Value::Int(d) => match d.parse::<f64>() {
            Ok(f) if f.is_finite() => Some(f),
            _ => return Err("OverflowError: int too large to convert to float".into()),
        },
        Value::Float(f) => Some(*f),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Str(t) => py_float(&p::text(t)),
        _ => None,
    })
}

/// `"%.Nf" % f`.
fn fixed(f: f64, n: usize) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        format!("{f:.n$}")
    }
}

/// `_gib(n)`, called from the frames `from` (outermost first).
fn gib(n: Option<Value>, from: &[(u32, &'static str)]) -> R<String> {
    let r = (|| -> R<String> {
        let f = match float_of(&n.unwrap_or(Value::Null)).at(line::GIB_FLOAT, "_gib")? {
            Some(f) => f,
            None => return Ok("?".into()),
        };
        let (gib, mib) = ((1u64 << 30) as f64, (1u64 << 20) as f64);
        if f.abs() >= gib {
            return Ok(format!("{} GiB", fixed(f / gib, 1)));
        }
        if f.abs() >= mib {
            return Ok(format!("{} MiB", fixed(f / mib, 0)));
        }
        let d = py::percent_d(&Value::Float(f)).at(line::GIB_D, "_gib")?;
        Ok(format!("{d} B"))
    })();
    from.iter().rev().fold(r, |r, (l, name)| r.within(*l, name))
}

/// `_dur(secs)`, called from line `from` of the module.
fn dur(secs: Option<Value>, from: u32) -> R<String> {
    let raise = |l: u32, e: &str| {
        End::Raise(crate::families::mail::core::Tb {
            frames: vec![(from, MODULE), (l, "_dur")],
            exception: e.to_string(),
        })
    };
    // `int(secs)`'s digits; TypeError and ValueError are "?".
    let digits = match secs.unwrap_or(Value::Null) {
        Value::Float(f) if f.is_infinite() => {
            return Err(raise(
                line::DUR_INT,
                "OverflowError: cannot convert float infinity to integer",
            ))
        }
        v @ (Value::Int(_) | Value::Bool(_) | Value::Float(_) | Value::Str(_)) => {
            match p::py_int(&v) {
                Ok(d) => d,
                Err(_) => return Ok("?".into()),
            }
        }
        _ => return Ok("?".into()),
    };
    let small: Option<i64> = digits.parse().ok();
    match small {
        Some(s) if s < 120 => Ok(format!("{s} s")),
        Some(s) if s < 7200 => Ok(format!("{} min", s.div_euclid(60))),
        None if digits.starts_with('-') => Ok(format!("{digits} s")),
        _ => match digits.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(format!("{} h", fixed(f / 3600.0, 1))),
            _ => Err(raise(
                line::DUR_HOURS,
                "OverflowError: int too large to convert to float",
            )),
        },
    }
}

/// `{True: yes, False: no}.get(v, default)`, raised at `l` of `name`.
fn yes_no(
    v: Option<Value>,
    yes: &str,
    no: &str,
    default: &str,
    l: u32,
    name: &'static str,
) -> R<String> {
    let truth = match &v {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::Int(d)) if d == "1" => Some(true),
        Some(Value::Int(d)) if d == "0" => Some(false),
        Some(Value::Float(f)) if *f == 1.0 => Some(true),
        Some(Value::Float(f)) if *f == 0.0 => Some(false),
        Some(other @ (Value::Array(_) | Value::Object(_))) => {
            return Err(End::Raise(crate::families::mail::core::Tb {
                frames: vec![(l, name)],
                exception: format!("TypeError: unhashable type: '{}'", py::type_name(other)),
            }))
        }
        _ => None,
    };
    Ok(match truth {
        Some(true) => yes,
        Some(false) => no,
        None => default,
    }
    .to_string())
}

/// `_print_backup(b)`, called from module line `from`.
fn print_backup(py_: &mut Py, b: &Value, from: u32) -> R<()> {
    print_backup_in(py_, b).within(from, MODULE)
}

fn print_backup_in(py_: &mut Py, b: &Value) -> R<()> {
    const F: &str = "_print_backup";
    let pb = |l: u32| -> [(u32, &'static str); 1] { [(l, F)] };
    py_.print(&format!(
        "mode     : {}",
        or_s(gin(b, "mode", line::PB_MODE, F)?, "?")
    ));
    if let Some(by) = gin(b, "configuredBy", line::PB_BY, F)?.filter(py::truthy) {
        let ws = match gin(b, "configuredWorkspace", line::PB_BY + 1, F)?.filter(py::truthy) {
            Some(w) => format!(" ({})", py::str_of(&w)),
            None => String::new(),
        };
        let at = gin(b, "configuredAt", line::PB_BY + 2, F)?;
        py_.print(&format!(
            "set by   : {}{ws}, {}",
            py::str_of(&by),
            unix_ago(at.as_ref())
        ));
    }
    if let Some(r) = gin(b, "reason", line::PB_REASON, F)?.filter(py::truthy) {
        py_.print(&format!("reason   : {}", py::str_of(&r)));
    }
    if let Some(w) = gin(b, "window", line::PB_WINDOW, F)?.filter(py::truthy) {
        py_.print(&format!("window   : {} (box time)", py::str_of(&w)));
    }
    let r = or_dict(gin(b, "retention", line::PB_RETENTION, F)?);
    if py::truthy(&r) {
        let loc = or_dict(gin(&r, "local", line::PB_LOCAL, F)?);
        let off = or_dict(gin(&r, "offsite", line::PB_OFFSITE, F)?);
        let cap = gin(&loc, "maxLocalGb", line::PB_CAP, F)?;
        let kd = s_of(gin(&loc, "keepDaily", line::PB_KEEP, F)?);
        let kw = s_of(gin(&loc, "keepWeekly", line::PB_KEEP, F)?);
        let cap = match cap.filter(py::truthy) {
            Some(c) => format!(", max {} GiB", py::str_of(&c)),
            None => String::new(),
        };
        let od = s_of(gin(&off, "keepDaily", line::PB_OFF_KEEP, F)?);
        let ow = s_of(gin(&off, "keepWeekly", line::PB_OFF_KEEP, F)?);
        let om = s_of(gin(&off, "keepMonthly", line::PB_OFF_KEEP, F)?);
        py_.print(&format!(
            "retention: local {kd} daily + {kw} weekly{cap}; offsite {od}/{ow}/{om} (d/w/m)"
        ));
    }
    let o = or_dict(gin(b, "observer", line::PB_OBSERVER, F)?);
    let mut l = format!(
        "observer : {}",
        or_s(gin(&o, "status", line::PB_STATUS, F)?, "never")
    );
    if let Some(last) = gin(&o, "lastSuccess", line::PB_LAST, F)?.filter(py::truthy) {
        l.push_str(&format!(", last {}", unix_ago(Some(&last))));
    }
    let days = gin(&o, "days", line::PB_DAYS, F)?;
    l.push_str(&format!(", {} day(s) of churn", or_s(days.clone(), "0")));
    py_.print(&l);
    let cp = or_dict(gin(&o, "churnPerDay", line::PB_CHURN, F)?);
    if p::truthy(gin(&o, "days", line::PB_CHURN + 1, F)?.as_ref()) {
        let fd = pb(line::PB_CHURN_DEL);
        let fnw = pb(line::PB_CHURN_NEW);
        let del = gib(gin(&cp, "deletedBytes", line::PB_CHURN_DEL, F)?, &fd)?;
        let worst = gib(gin(&cp, "maxDeletedBytes", line::PB_CHURN_DEL, F)?, &fd)?;
        let new = gib(gin(&cp, "createdBytes", line::PB_CHURN_NEW, F)?, &fnw)?;
        let measured = if p::truthy(gin(&cp, "measured", line::PB_CHURN_NEW, F)?.as_ref()) {
            ""
        } else {
            " — not yet measured (3 days)"
        };
        py_.print(&format!(
            "churn    : deleted {del}/day (worst {worst}), new {new}/day{measured}"
        ));
    }
    if !matches!(
        gin(&o, "storeBytes", line::PB_STORE_IF, F)?,
        None | Some(Value::Null)
    ) {
        let fs = pb(line::PB_STORE);
        let fs2 = pb(line::PB_STORE_FS);
        let store = gib(gin(&o, "storeBytes", line::PB_STORE, F)?, &fs)?;
        let mutable = gib(gin(&o, "mutableBytes", line::PB_STORE, F)?, &fs)?;
        let free = gib(gin(&o, "fsFree", line::PB_STORE, F)?, &fs)?;
        let total = gib(gin(&o, "fsTotal", line::PB_STORE_FS, F)?, &fs2)?;
        let same = yes_no(
            gin(&o, "sameFs", line::PB_STORE_FS, F)?,
            "yes",
            "NO",
            "?",
            line::PB_STORE_FS,
            F,
        )?;
        py_.print(&format!(
            "store    : {store} (mutable {mutable}); fs free {free} of {total}; view root same fs: {same}"
        ));
    }
    if let Some(e) = gin(&o, "lastError", line::PB_ERR, F)?.filter(py::truthy) {
        py_.print(&format!("obs err  : {}", py::str_of(&e)));
    }
    if let Some(e) = gin(b, "configError", line::PB_CONFIG, F)?.filter(py::truthy) {
        py_.print(&format!("config   : {}", py::str_of(&e)));
    }
    py_.print(&format!(
        "note     : {}",
        or_s(gin(b, "note", line::PB_NOTE, F)?, "")
    ));
    Ok(())
}

fn status_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/backup", &[], line::STATUS_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let b = or_dict(g(&d, "backup", line::STATUS_PRINT)?);
    print_backup(py_, &b, line::STATUS_PRINT)
}

/// `_gib` from module line `l`.
fn gm(n: Option<Value>, l: u32) -> R<String> {
    gib(n, &[(l, MODULE)])
}

fn plan_py(py_: &mut Py) -> R<()> {
    let mut params = Vec::new();
    for (k, e) in [
        ("bandwidthMbps", "M_BK_BANDWIDTH"),
        ("keepDaily", "M_BK_KEEP_DAILY"),
        ("keepWeekly", "M_BK_KEEP_WEEKLY"),
        ("maxLocalGb", "M_BK_MAX_LOCAL_GB"),
    ] {
        if !py_.env(e).is_empty() {
            params.push((k.to_string(), py_.env(e).to_string()));
        }
    }
    let d = py_.call_full(
        Method::Get,
        "/cli/mail/backup/plan",
        &params,
        None,
        Secs::Int(120),
        None,
        line::PLAN_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let st = or_dict(g(&d, "store", line::PLAN_STORE - 3)?);
    let ch = or_dict(g(&d, "churn", line::PLAN_STORE - 2)?);
    let (l1, l2) = (line::PLAN_STORE, line::PLAN_STORE_2);
    let bytes = gm(g(&st, "bytes", l1)?, l1)?;
    let path = s_of(g(&st, "path", l1)?);
    let immut = s_of(g(&st, "immutableFiles", l1)?);
    let mutable = gm(g(&st, "mutableBytes", l2)?, l2)?;
    let config = gm(g(&st, "configBytes", l2)?, l2)?;
    let half = gm(g(&st, "k2HalfBytes", l2)?, l2)?;
    py_.print(&format!("store    : {bytes} at {path} ({immut} files hard-linkable, mutable {mutable}, /etc/stalwart {config}, {PRODUCT_NAME} half {half})"));
    let (l1, l2) = (line::PLAN_DISK, line::PLAN_DISK_2);
    let free = gm(g(&st, "fsFree", l1)?, l1)?;
    let total = gm(g(&st, "fsTotal", l1)?, l1)?;
    let floor = gm(g(&st, "floorBytes", l1)?, l1)?;
    let root = s_of(g(&st, "viewRoot", l2)?);
    let same = yes_no(g(&st, "sameFs", l2)?, "yes", "NO", "unknown", l2, MODULE)?;
    py_.print(&format!(
        "disk     : free {free} of {total}; floor {floor}; {root} same filesystem: {same}"
    ));
    let (l1, l2) = (line::PLAN_CHURN, line::PLAN_CHURN_2);
    let label = s_of(g(&ch, "label", l1)?);
    let del = gm(g(&ch, "deletedPerDay", l1)?, l1)?;
    let worst = gm(g(&ch, "maxDeletedPerDay", l1)?, l1)?;
    let new = gm(g(&ch, "createdPerDay", l2)?, l2)?;
    py_.print(&format!(
        "churn    : {label} — deleted {del}/day (worst {worst}), new {new}/day"
    ));
    let r = or_dict(g(&d, "retention", line::PLAN_LOCAL - 1)?);
    let loc = or_dict(g(&r, "local", line::PLAN_LOCAL)?);
    let off = or_dict(g(&r, "offsite", line::PLAN_OFFSITE)?);
    let kd = s_of(g(&loc, "keepDaily", line::PLAN_KEEP)?);
    let kw = s_of(g(&loc, "keepWeekly", line::PLAN_KEEP)?);
    let cap = match g(&loc, "maxLocalGb", line::PLAN_CAP)?.filter(py::truthy) {
        Some(c) => format!(", max {} GiB", py::str_of(&c)),
        None => String::new(),
    };
    let lo = line::PLAN_OFF_KEEP;
    let od = s_of(g(&off, "keepDaily", lo)?);
    let ow = s_of(g(&off, "keepWeekly", lo)?);
    let om = s_of(g(&off, "keepMonthly", lo)?);
    let window = s_of(g(&d, "window", lo)?);
    py_.print(&format!("retention: local {kd} daily + {kw} weekly{cap}; offsite {od}/{ow}/{om} (d/w/m); window {window}"));
    let mbps = s_of(g(&d, "bandwidthMbps", lo + 1)?);
    for name in ["local", "offsite-only", "both", "none"] {
        let lm = line::PLAN_MODES;
        let modes = or_dict(g(&d, "modes", lm)?);
        let m = or_dict(g(&modes, name, lm)?);
        py_.print("");
        let lv = line::PLAN_VERDICT;
        let verdict = p::or(g(&m, "verdict", lv)?, sv("?"));
        let verdict = if py::truthy(&verdict) {
            verdict
        } else {
            sv("?")
        };
        let up = match &verdict {
            Value::Str(t) => p::text(t).to_uppercase(),
            other => {
                return Err(crate::families::mail::core::raise_at(
                    lv,
                    format!(
                        "AttributeError: '{}' object has no attribute 'upper'",
                        py::type_name(other)
                    ),
                ))
            }
        };
        let mut head = format!("{} verdict {up}", pyx::ljust(name, 12));
        if matches!(
            g(&m, "available", line::PLAN_AVAILABLE)?,
            Some(Value::Bool(false))
        ) {
            head.push_str(&format!(
                "  (not available yet: {})",
                or_s(g(&m, "unavailableReason", line::PLAN_UNAVAILABLE)?, "")
            ));
        }
        py_.print(&head);
        if name == "none" {
            py_.print("  nothing runs; --reason is required and recorded");
            continue;
        }
        let (l1, l2) = (line::PLAN_LOCAL_DISK, line::PLAN_LOCAL_DISK_2);
        let first = gm(g(&m, "firstNightLocalBytes", l1)?, l1)?;
        let steady = gm(g(&m, "steadyLocalBytes", l1)?, l1)?;
        let peak = gm(g(&m, "peakBytes", l2)?, l2)?;
        let after = gm(g(&m, "freeAfterBytes", l2)?, l2)?;
        py_.print(&format!(
            "  local disk : first night {first}, steady {steady}, peak {peak} (free after {after})"
        ));
        if let Some(o @ Value::Object(_)) = g(&m, "offsite", line::PLAN_OFF)? {
            let fu = or_dict(g(&o, "firstUploadBytes", line::PLAN_FU)?);
            let (l1, l2) = (line::PLAN_UPLOAD, line::PLAN_UPLOAD_2);
            let low = gm(g(&fu, "low", l1)?, l1)?;
            let high = gm(g(&fu, "high", l1)?, l1)?;
            let secs = dur(g(&o, "firstUploadSecs", l1)?, l1)?;
            let nightly = gm(g(&o, "nightlyUploadBytes", l2)?, l2)?;
            let nsecs = dur(g(&o, "nightlyUploadSecs", l2)?, l2)?;
            py_.print(&format!("  offsite    : first upload {low}–{high} (~{secs} at {mbps} Mbit/s), nightly {nightly} (~{nsecs})"));
            let lt = line::PLAN_TEMP;
            let t1 = gm(g(&o, "firstTempBytes", lt)?, lt)?;
            let t2 = gm(g(&o, "nightlyTempBytes", lt)?, lt)?;
            py_.print(&format!("  temp space : first {t1}, nightly {t2}"));
        }
        let ld = line::PLAN_DOWN;
        let down = dur(g(&m, "downtimeSecs", ld)?, ld)?;
        let label = or_s(g(&m, "downtimeLabel", ld)?, "");
        py_.print(&format!("  downtime   : ~{down} nightly ({label})"));
        py_.print(&format!(
            "  protects   : {}",
            or_s(g(&m, "protects", line::PLAN_PROTECTS)?, "")
        ));
    }
    py_.print("");
    for n in items(g(&d, "notes", line::PLAN_NOTES)?, line::PLAN_NOTES)? {
        py_.print(&format!("note     : {}", py::str_of(&n)));
    }
    let hash = s_of(g(&d, "planHash", line::PLAN_HASH)?);
    py_.print(&format!("planHash : {hash}"));
    py_.print(&format!("next     : {CLI_NAME} hostmail backup set --mode <local|none> [same retention flags] --confirm-plan {hash}"));
    Ok(())
}

fn set_py(py_: &mut Py) -> R<()> {
    let mut body = vec![
        ("mode", pyx::env_v(py_, "M_BK_MODE")),
        ("confirmPlan", pyx::env_v(py_, "M_BK_CONFIRM")),
    ];
    if !py_.env("M_REASON").is_empty() {
        body.push(("reason", pyx::env_v(py_, "M_REASON")));
    }
    for (k, e) in [
        ("keepDaily", "M_BK_KEEP_DAILY"),
        ("keepWeekly", "M_BK_KEEP_WEEKLY"),
        ("offsiteKeepDaily", "M_BK_OFF_DAILY"),
        ("offsiteKeepWeekly", "M_BK_OFF_WEEKLY"),
        ("offsiteKeepMonthly", "M_BK_OFF_MONTHLY"),
    ] {
        if !py_.env(e).is_empty() {
            let n = pyx::int(&pyx::env_v(py_, e), line::SET_INT)?;
            body.push((k, Value::Int(n)));
        }
    }
    if !py_.env("M_BK_WINDOW").is_empty() {
        body.push(("window", pyx::env_v(py_, "M_BK_WINDOW")));
    }
    let gb = py_.env("M_BK_MAX_LOCAL_GB").to_string();
    if !gb.is_empty() {
        match py_float(&gb) {
            Some(f) => body.push(("maxLocalGb", Value::Float(f))),
            None => {
                return Err(py_.die_with(
                    "usage",
                    &format!("--max-local-gb must be a number of GiB, got '{gb}'"),
                    None,
                ))
            }
        }
    }
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/backup/set",
        &[],
        Some(&obj(body)),
        Secs::Int(120),
        None,
        line::SET_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::SET_MODE;
    let from = or_s(g(&d, "from", l)?, "?");
    let b = or_dict(g(&d, "backup", l)?);
    let mode = or_s(g(&b, "mode", l)?, "?");
    py_.print(&format!("backup mode: {from} → {mode}"));
    let b = or_dict(g(&d, "backup", line::SET_PRINT)?);
    print_backup(py_, &b, line::SET_PRINT)
}

/// `who.get(v, v or "?")`, raised at `l`.
fn who(v: Option<Value>, l: u32) -> R<String> {
    match &v {
        Some(Value::Str(t)) => {
            let s = p::text(t);
            Ok(match s.as_str() {
                WIRE_SELF => format!("set by {PRODUCT_NAME}"),
                "hand" => "set by hand".into(),
                "default" => "Stalwart default".into(),
                _ => or_s(v, "?"),
            })
        }
        Some(other @ (Value::Array(_) | Value::Object(_))) => {
            Err(crate::families::mail::core::raise_at(
                l,
                format!("TypeError: unhashable type: '{}'", py::type_name(other)),
            ))
        }
        _ => Ok(or_s(v, "?")),
    }
}

fn outbound_py(py_: &mut Py) -> R<()> {
    if py_.env("M_OUTBOUND_ACTION") == "dane-on" {
        let d = py_.post(
            "/cli/mail/outbound",
            &obj(vec![("dane", sv("on"))]),
            line::DANE_CALL,
        )?;
        return pyx::hint_or(py_, &d, "DANE is optional again", line::DANE_PRINT);
    }
    let d = py_.get("/cli/mail/outbound", &[], line::OUT_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let strategy = or_s(g(&d, "ipStrategy", line::OUT_WHO - 1)?, "?");
    let by = who(g(&d, "ipStrategySetBy", line::OUT_WHO)?, line::OUT_WHO)?;
    py_.print(&format!("mx route : {strategy} ({by})"));
    let rows = items(g(&d, "dane", line::OUT_DANE)?, line::OUT_DANE)?;
    let mut any_off = false;
    for t in &rows {
        let l = line::OUT_DANE_ROW;
        let name = or_s(g(t, "name", l)?, "?");
        let dane = g(t, "dane", l)?;
        any_off |= dane.as_ref().is_some_and(|v| p::eq_str(v, "disable"));
        let by = who(g(t, "setBy", line::OUT_DANE_WHO)?, line::OUT_DANE_WHO)?;
        py_.print(&format!(
            "DANE     : {} {} ({by})",
            pyx::ljust(&name, 12),
            or_s(dane, "?")
        ));
    }
    let fallback = format!("not checked ({CLI_NAME} hostmail doctor)");
    py_.print(&format!(
        "DNSSEC   : {}",
        or_s(g(&d, "dnssec", line::OUT_DNSSEC)?, &fallback)
    ));
    if let Some(pr) = g(&d, "pendingReload", line::OUT_PENDING)?.filter(py::truthy) {
        let l = line::OUT_PENDING_ROW;
        let at = or_s(g(&pr, "at", l)?, "?");
        let err = or_s(g(&pr, "error", l)?, "?");
        py_.print(&format!(
            "WARNING  : settings saved but not live since {at} — {err}"
        ));
    }
    if any_off {
        py_.print(&format!(
            "           turn DANE back on (Stalwart 0.16.20+): {CLI_NAME} hostmail outbound dane on"
        ));
    }
    Ok(())
}

/// `_yn(v)`.
fn yn(v: Option<Value>) -> &'static str {
    match v {
        Some(Value::Bool(true)) => "yes",
        Some(Value::Bool(false)) => "no",
        _ => "?",
    }
}

fn names_py(py_: &mut Py) -> R<()> {
    let action = match py_.env("M_CERT_ACTION") {
        "" => "list".to_string(),
        a => a.to_string(),
    };
    let list = action == "list";
    let d = if list {
        py_.call_full(
            Method::Get,
            "/cli/mail/cert/names",
            &[],
            None,
            Secs::Int(120),
            None,
            line::NAMES_LIST_CALL,
        )?
    } else {
        let mut body = vec![("action", sv(&action))];
        if !py_.env("M_NAME").is_empty() {
            body.push(("name", pyx::env_v(py_, "M_NAME")));
        }
        py_.call_full(
            Method::Post,
            "/cli/mail/cert/names",
            &[],
            Some(&obj(body)),
            Secs::Int(1800),
            None,
            line::NAMES_CALL,
        )?
    };
    if py_.json {
        py_.print_json(&d);
    } else {
        let rows = p::or(g(&d, "names", line::NAMES_ROWS)?, Value::Array(vec![]));
        if !py::truthy(&rows) {
            py_.print("no extra names (no hosted domains)");
        }
        let rows = if py::truthy(&rows) {
            p::iter(&rows).at(line::NAMES_FOR, MODULE)?
        } else {
            Vec::new()
        };
        let blank = pyx::ljust("", 34);
        for r in rows {
            if list {
                let cert = or_dict(g(&r, "cert", line::NAMES_CERT)?);
                let exp = match g(&cert, "expiresAt", line::NAMES_EXP)?.filter(py::truthy) {
                    Some(e) => format!(" expires {}", py::str_of(&e)),
                    None => String::new(),
                };
                let name = or_s(g(&r, "name", line::NAMES_ROW)?, "?");
                let here = yn(g(&r, "pointsHere", line::NAMES_ROW)?);
                let state = or_s(g(&cert, "state", line::NAMES_ROW_2)?, "?");
                let issuer = or_s(g(&r, "issuer", line::NAMES_ROW_2)?, "?");
                py_.print(&format!(
                    "{} here {}  cert {state}{exp}  ({issuer})",
                    pyx::ljust(&name, 34),
                    pyx::ljust(here, 3)
                ));
                if let Some(dns) = g(&r, "dns", line::NAMES_DNS)?.filter(py::truthy) {
                    if !matches!(
                        g(&r, "pointsHere", line::NAMES_DNS)?,
                        Some(Value::Bool(true))
                    ) {
                        py_.print(&format!("{blank}   {}", py::str_of(&dns)));
                    }
                }
                if let Some(why) = g(&r, "reason", line::NAMES_REASON)?.filter(py::truthy) {
                    py_.print(&format!("{blank}   {}", py::str_of(&why)));
                }
                if let Some(e) = g(&cert, "lastError", line::NAMES_LAST)?.filter(py::truthy) {
                    py_.print(&format!("{blank}   last error: {}", py::str_of(&e)));
                }
            } else {
                let exp = match g(&r, "expiresAt", line::NAMES_R_EXP)?.filter(py::truthy) {
                    Some(e) => format!(" expires {}", py::str_of(&e)),
                    None => String::new(),
                };
                let l = line::NAMES_R_ROW;
                let name = or_s(g(&r, "name", l)?, "?");
                let result = or_s(g(&r, "result", l)?, "?");
                py_.print(&format!("{} {result}{exp}", pyx::ljust(&name, 34)));
                if let Some(det) = g(&r, "detail", line::NAMES_DETAIL)?.filter(py::truthy) {
                    py_.print(&format!("{blank}   {}", py::str_of(&det)));
                }
            }
        }
        if !list {
            let rel = or_dict(g(&d, "reload", line::NAMES_RELOAD)?);
            let mut l = format!(
                "reload   : {}",
                or_s(g(&rel, "method", line::NAMES_METHOD)?, "?")
            );
            if let Some(e) = g(&rel, "error", line::NAMES_RELOAD_ERR)?.filter(py::truthy) {
                l.push_str(&format!(" — {}", py::str_of(&e)));
            }
            py_.print(&l);
        }
    }
    if !list && p::truthy(g(&d, "failed", line::NAMES_FAILED)?.as_ref()) {
        return Err(End::Exit(Exit::Error));
    }
    Ok(())
}
