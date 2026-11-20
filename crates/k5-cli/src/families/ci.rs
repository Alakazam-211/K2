//! `k5 ci`, as the shell CLI's `cmd_ci` answers it (`cli/src/ci.sh`): CI
//! on K2 Compute, daemon-owned. The binary answers the bare line (`ci`:
//! the help; `ci --json`: the unknown-verb line) and these verbs, each
//! with `--json`: `status`, `run [--wait]`, `release [--plan|--wait]`,
//! `stats`, `monitors`, `monitor add|mute|rm`, `tests`, `setup --plan`,
//! `watch [on|off]`, `init`, `freeze`, `cache trust`, and `signer`
//! (`signers`): `list`, `add`, `reveal`, `finish`, `remove`.
//! `evidence`, `provenance` and `logs` stay the shell's for now (they
//! write a run's files and call `compute fetch` and `compute logs`).
//!
//! What runs, in the shell's order: the settings, the two gates, `ci`'s
//! tool (`compute`, an `id` tool: a session with no passport is refused,
//! exit 3), the words ([`parse`]; each problem is one `usage:` line, exit
//! 2), the verb's requests through `_compute_send` ([`Cp`]) with
//! `_ci_call`'s refusal line and exit ([`call`]), its reader, then the
//! EXIT trap's lines.
//!
//! The shell answers these itself ([`run`] returns `None`): `-h` or
//! `--help` anywhere (the help), a word that is not UTF-8, `HOOK_SOCK` set
//! with a `CODEX_` variable exported, a port curl won't take, a `python3`
//! the binary can't tell, the shell's own internal settings in the
//! environment, a `CI_WAIT_SECS` that is not plain seconds, a duration
//! its arithmetic would read past 64 bits, and a `freeze --until` time
//! that is not one of the forms read here.

pub mod py;
pub mod render;
pub mod signer;

use super::compute::writes::{self, dumps, object, str_value, Cp};
use crate::daemon;
use crate::exit::Exit;
use crate::families::{curl_port, Shown};
use crate::out;
use crate::out::json::{self, Value};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::route::{Owner, Verb};
use crate::transport::urllib::urlencode;
use k2_client::brand::CLI_NAME as CLI;
use k2_client::product_name;
use std::ffi::OsString;

/// The router arm's words.
pub const WORDS: &[&str] = &["ci"];
/// The shell's but for [`VERBS`].
pub const OWNER: Owner = Owner::Bash;
/// The bare line and each native verb.
pub const VERBS: &[Verb] = &[
    Verb::bare(Owner::Native),
    Verb {
        words: &["status"],
        owner: Owner::Native,
    },
    Verb {
        words: &["run"],
        owner: Owner::Native,
    },
    Verb {
        words: &["release"],
        owner: Owner::Native,
    },
    Verb {
        words: &["stats"],
        owner: Owner::Native,
    },
    Verb {
        words: &["monitors"],
        owner: Owner::Native,
    },
    Verb {
        words: &["monitor"],
        owner: Owner::Native,
    },
    Verb {
        words: &["tests"],
        owner: Owner::Native,
    },
    Verb {
        words: &["setup"],
        owner: Owner::Native,
    },
    Verb {
        words: &["watch"],
        owner: Owner::Native,
    },
    Verb {
        words: &["init"],
        owner: Owner::Native,
    },
    Verb {
        words: &["freeze"],
        owner: Owner::Native,
    },
    Verb {
        words: &["cache"],
        owner: Owner::Native,
    },
    Verb {
        words: &["signer", "signers"],
        owner: Owner::Native,
    },
];

/// `ci`'s tool in the shell's policy.
pub const TOOL: &str = "compute";
/// No answer to a `_ci_call`.
pub const UNREACHABLE: &str = concat!(
    "daemon_unreachable: the ",
    product_name!(),
    " daemon did not answer"
);

/// The words, read: the flags `cmd_ci`'s loop takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    pub sub: String,
    pub json: bool,
    pub ws: String,
    pub pos: Vec<String>,
    pub argv: Vec<String>,
    pub have_cmd: bool,
    pub check: String,
    pub needs: String,
    pub os: String,
    pub mem: String,
    pub cpus: String,
    pub timeout: String,
    pub branches: String,
    pub remote: String,
    pub token: String,
    pub force: bool,
    pub refs: String,
    pub tier: String,
    pub wait: bool,
    pub wait_min: String,
    pub limit: String,
    pub failures: bool,
    pub poll: String,
    pub max_inflight: String,
    pub skip: bool,
    pub until: String,
    pub why: String,
    pub sha: String,
    pub builds: String,
    pub plan: bool,
    pub out_dir: String,
    pub no_files: bool,
    pub tests_filter: String,
    pub days: String,
    pub only_keys: String,
    pub reported: String,
    pub branch: String,
    pub mkind: String,
    pub mcount: String,
    pub mover: String,
    pub mcooldown: String,
    pub mto: String,
    pub mfor: String,
    pub tadd: String,
    pub tset: String,
    pub tremove: String,
    pub tset_given: bool,
}

/// What `cmd_ci` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_ci_usage`.
    Help,
    /// The `usage:` line, exit 2.
    Usage(String),
    /// A signer verb.
    Signer(signer::Ask),
    /// A verb with its flags.
    Call(Box<Ask>),
}

/// `_compute_secs`: plain seconds, or `<n>h`/`<n>m`/`<n>s` parts. `Err`
/// the usage line; `Ok(None)`: a number bash's arithmetic reads past 64
/// bits (the shell answers).
pub fn compute_secs(v: &str) -> Result<Option<String>, String> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let small = |s: &str| s.trim_start_matches('0').len() <= 15;
    if digits(v) {
        if !small(v) {
            return Ok(None);
        }
        let n: u64 = v.parse().unwrap_or(0);
        return Ok(Some(n.to_string()));
    }
    let bad = || format!("usage: bad duration '{v}' (use 2h, 90m, 1h30m, 300s)");
    let mut total: u64 = 0;
    let mut num = String::new();
    if v.is_empty() {
        return Err(bad());
    }
    for c in v.chars() {
        match c {
            '0'..='9' => num.push(c),
            'h' | 'm' | 's' if !num.is_empty() => {
                if !small(&num) {
                    return Ok(None);
                }
                let n: u64 = num.parse().unwrap_or(0);
                total += n * if c == 'h' {
                    3600
                } else if c == 'm' {
                    60
                } else {
                    1
                };
                num.clear();
            }
            _ => return Err(bad()),
        }
    }
    if !num.is_empty() {
        return Err(bad());
    }
    Ok(Some(total.to_string()))
}

const VALUE_FLAGS: &[&str] = &[
    "--workspace",
    "--check",
    "--checks",
    "--needs",
    "--os",
    "--mem",
    "--cpus",
    "--timeout",
    "--branches",
    "--remote",
    "--statuses-token",
    "--ref",
    "--tier",
    "--wait-min",
    "--limit",
    "--poll",
    "--max-inflight",
    "--until",
    "--why",
    "--sha",
    "--build",
    "--builds",
    "--out",
    "--only-keys",
    "--reported",
];

/// `cmd_ci`'s words after `ci`. `None`: the shell answers them.
pub fn parse(words: &[&str]) -> Option<Parsed> {
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "help" | "--help" | "-h") {
        return Some(Parsed::Help);
    }
    let rest = &words[1..];
    if sub == "signer" || sub == "signers" {
        if matches!(rest.first(), Some(&("-h" | "--help"))) {
            return Some(Parsed::Help);
        }
        return Some(signer::parse(sub, rest));
    }
    for w in rest {
        if *w == "--" {
            break;
        }
        if matches!(*w, "-h" | "--help") {
            return Some(Parsed::Help);
        }
    }
    let usage = |l: String| Some(Parsed::Usage(l));
    let mut a = Ask {
        sub: sub.to_string(),
        ..Ask::default()
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let value = rest.get(i + 1).copied().unwrap_or("");
        match w {
            "--" => {
                a.have_cmd = true;
                a.argv = rest[i + 1..].iter().map(|x| x.to_string()).collect();
                break;
            }
            "--json" => a.json = true,
            "--force" => a.force = true,
            "--plan" => a.plan = true,
            "--no-files" => a.no_files = true,
            "--wait" => a.wait = true,
            "--failures" => a.failures = true,
            "--skip-current" => a.skip = true,
            "--flaky" | "--slow" | "--failing" | "--quarantined" => {
                if !a.tests_filter.is_empty() {
                    return usage(format!(
                        "usage: one of --flaky, --slow, --failing, --quarantined ({CLI} ci tests)"
                    ));
                }
                a.tests_filter = w[2..].to_string();
            }
            "--days" => {
                if value.is_empty() {
                    return usage(format!("usage: --days needs a value ({CLI} ci --help)"));
                }
                a.days = value.strip_suffix('d').unwrap_or(value).to_string();
                i += 1;
            }
            "--branch" | "--kind" | "--count" | "--over" | "--cooldown" | "--to" | "--for" => {
                if value.is_empty() {
                    return usage(format!("usage: {w} needs a value ({CLI} ci --help)"));
                }
                let v = value.to_string();
                match w {
                    "--branch" => a.branch = v,
                    "--kind" => a.mkind = v,
                    "--count" => a.mcount = v,
                    "--over" => a.mover = v,
                    "--cooldown" => a.mcooldown = v,
                    "--to" => a.mto = v,
                    _ => a.mfor = v,
                }
                i += 1;
            }
            _ if VALUE_FLAGS.contains(&w) => {
                if value.is_empty() {
                    return usage(format!("usage: {w} needs a value ({CLI} ci --help)"));
                }
                let v = value.to_string();
                match w {
                    "--workspace" => a.ws = v,
                    "--check" | "--checks" => a.check = v,
                    "--needs" => a.needs = v,
                    "--os" => a.os = v,
                    "--mem" => {
                        let m = v.strip_suffix('G').unwrap_or(&v);
                        a.mem = m.strip_suffix('g').unwrap_or(m).to_string();
                    }
                    "--cpus" => a.cpus = v,
                    "--timeout" | "--poll" => match compute_secs(&v) {
                        Err(line) => return usage(line),
                        Ok(None) => return None,
                        Ok(Some(n)) if w == "--timeout" => a.timeout = n,
                        Ok(Some(n)) => a.poll = n,
                    },
                    "--branches" => a.branches = v,
                    "--remote" => a.remote = v,
                    "--statuses-token" => a.token = v,
                    "--ref" => a.refs = v,
                    "--tier" => a.tier = v,
                    "--wait-min" => a.wait_min = v,
                    "--limit" => a.limit = v,
                    "--max-inflight" => a.max_inflight = v,
                    "--until" => a.until = v,
                    "--why" => a.why = v,
                    "--sha" => a.sha = v,
                    "--build" | "--builds" => a.builds = v,
                    "--out" => a.out_dir = v,
                    "--only-keys" => a.only_keys = v,
                    _ => a.reported = v,
                }
                i += 1;
            }
            "--add" | "--set" | "--remove" => {
                if a.sub != "cache" {
                    return usage(format!("usage: {w} goes with {CLI} ci cache trust"));
                }
                if value.is_empty() {
                    return usage(format!(
                        "usage: {w} needs a branch ({CLI} ci cache trust --help)"
                    ));
                }
                let v = value.to_string();
                match w {
                    "--add" => a.tadd = v,
                    "--set" => {
                        a.tset = v;
                        a.tset_given = true;
                    }
                    _ => a.tremove = v,
                }
                i += 1;
            }
            _ if w.starts_with("--") => {
                return usage(format!("usage: unknown flag '{w}' ({CLI} ci --help)"))
            }
            _ => a.pos.push(w.to_string()),
        }
        i += 1;
    }
    for (name, v) in [
        ("mem", a.mem.as_str()),
        ("cpus", a.cpus.as_str()),
        ("wait-min", a.wait_min.as_str()),
        ("limit", a.limit.as_str()),
        ("max-inflight", a.max_inflight.as_str()),
        ("days", a.days.as_str()),
        ("mcount", a.mcount.as_str()),
    ] {
        if !v.is_empty() && !v.bytes().all(|b| b.is_ascii_digit()) {
            return usage(format!("usage: --{name} is a number"));
        }
    }
    if a.sub != "tests" && !a.tests_filter.is_empty() {
        return usage(format!(
            "usage: --flaky, --slow, --failing and --quarantined go with {CLI} ci tests"
        ));
    }
    if !a.days.is_empty() && a.sub != "tests" && a.sub != "stats" {
        return usage(format!(
            "usage: --days goes with {CLI} ci tests and {CLI} ci stats"
        ));
    }
    if !a.branch.is_empty() && a.sub != "stats" && a.sub != "monitor" {
        return usage(format!(
            "usage: --branch goes with {CLI} ci stats and {CLI} ci monitor add (watched branches: --branches)"
        ));
    }
    let m = [&a.mkind, &a.mcount, &a.mover, &a.mcooldown, &a.mto, &a.mfor];
    if a.sub != "monitor" && m.iter().any(|v| !v.is_empty()) {
        return usage(format!(
            "usage: --kind, --count, --over, --cooldown, --to and --for go with {CLI} ci monitor"
        ));
    }
    if !matches!(a.tier.as_str(), "" | "ci" | "standard" | "normal") {
        return usage("usage: --tier ci|standard".into());
    }
    Some(Parsed::Call(Box::new(a)))
}

/// `k5 ci ...`: `args` is the whole command line after `argv[0]`.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(n, _)| n)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let parsed = parse(&words)?;
    if let Parsed::Call(a) = &parsed {
        if !shell_free(a) {
            return None;
        }
    }
    let max_time = writes::post_max_time(std::env::var_os("K2_CP_MAX_TIME"))?;
    if writes::shell_settings(std::env::vars_os()) {
        return None;
    }
    if !python::family_native(python::here()) {
        return None;
    }
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    let a = match parsed {
        Parsed::Help => {
            return Some(Shown::out(crate::text::help("ci").unwrap_or_default()).print())
        }
        Parsed::Usage(line) => {
            out::stderr_line(&line);
            return Some(Exit::Usage);
        }
        other => other,
    };
    let vault_sock = crate::confirm::sock_path(&settings.home);
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    client.compute_post_max_time = max_time;
    let mut cp = Cp {
        client: &mut client,
        closing: Closing::default(),
        project: settings.project.clone(),
        as_json: false,
    };
    let exit = match &a {
        Parsed::Signer(s) => signer::send(&mut cp, s, &vault_sock),
        Parsed::Call(a) => {
            let ws = cp.ws(&a.ws);
            verb(&mut cp, a, &ws)
        }
        _ => Exit::Error,
    };
    let closing = std::mem::take(&mut cp.closing);
    closing.flush(&mut client, exit);
    Some(exit)
}

/// What the shell must answer before anything is sent.
fn shell_free(a: &Ask) -> bool {
    if a.wait && matches!(a.sub.as_str(), "run" | "release") && wait_secs().is_none() {
        return false;
    }
    true
}

/// `$(_envn CI_WAIT_SECS 10)` as `sleep` reads it: plain seconds (a
/// fraction too). `None`: anything else.
fn wait_secs() -> Option<f64> {
    let names = k2_client::env_names::CI_WAIT_SECS;
    let v = k2_client::env_names::first_nonempty_os_in(names, |n| std::env::var_os(n))
        .map(|(_, v)| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "10".into());
    let (whole, frac) = v.split_once('.').unwrap_or((&v, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !digits(frac) || (whole.is_empty() && frac.is_empty()) {
        return None;
    }
    format!("0{whole}.{frac}0").parse().ok()
}

/// `_ci_until`: `<n>d|h|m` from now, epoch seconds (9 to 11 digits), or
/// a time `datetime.fromisoformat` reads, with its zone. `Err`: the line
/// its `sys.exit` prints (then `|| exit 2`).
pub fn until_secs(v: &str, now: i64) -> Result<i64, &'static str> {
    const BAD: &str = "usage: --until 3d | 12h | 2026-11-20T00:00:00Z";
    let s = v.trim_matches(crate::families::py_space);
    if let Some(unit) = s.chars().last() {
        let n = &s[..s.len() - unit.len_utf8()];
        if matches!(unit, 'd' | 'h' | 'm') && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
        {
            let m: i128 = match unit {
                'd' => 86_400,
                'h' => 3600,
                _ => 60,
            };
            let n: i128 = n.parse().map_err(|_| BAD)?;
            return i64::try_from(i128::from(now) + n * m).map_err(|_| BAD);
        }
    }
    if (9..=11).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().map_err(|_| BAD);
    }
    let dt =
        crate::families::heartbeat::when::fromisoformat(&s.replace('Z', "+00:00")).ok_or(BAD)?;
    let off = dt
        .offset
        .ok_or("usage: --until needs a time zone (2026-11-20T00:00:00Z)")?;
    let days = crate::families::heartbeat::when::days_from_civil(dt.year, dt.month, dt.day);
    Ok(
        days * 86_400
            + i64::from(dt.hour) * 3600
            + i64::from(dt.minute) * 60
            + i64::from(dt.second)
            - off,
    )
}

/// `_ci_call GET|POST <route> <payload>`: the body of a 200; else the
/// refusal's line, the permission note, and `_ci_exit_for`'s exit.
pub fn call(cp: &mut Cp<'_>, post: bool, route: &str, payload: &str) -> Result<Vec<u8>, Exit> {
    let s = cp.send(post, &format!("/cli/ci/{route}"), payload, false)?;
    if s.status == "200" {
        return Ok(s.body);
    }
    if s.status == "000" {
        out::stderr_line(UNREACHABLE);
        return Err(Exit::Error);
    }
    let _ = out::write_flushed(
        &mut std::io::stderr().lock(),
        fail_lines(&s.body).as_bytes(),
    );
    Err(match s.status.as_str() {
        "400" | "413" => Exit::Usage,
        "404" => Exit::NotFound,
        _ => Exit::Refused,
    })
}

/// `_ci_call`'s Python on a refusal's body.
pub fn fail_lines(body: &[u8]) -> String {
    let raw = String::from_utf8_lossy(body);
    let head = |r: &str| r.chars().take(300).collect::<String>();
    let fallback = || {
        let h = head(&raw);
        format!(
            "{}\n",
            if h.is_empty() {
                "the daemon did not answer".to_string()
            } else {
                h
            }
        )
    };
    let Ok(v) = crate::out::pylib::json::loads(&raw) else {
        return fallback();
    };
    let Value::Object(_) = &v else {
        return fallback();
    };
    let e = py::g(&v, "error", 0).unwrap_or(Value::Null);
    match &e {
        Value::Object(_) => {
            let code = py::g_or(&e, "code", py::t("error"), 0).unwrap_or(Value::Null);
            let hint = py::g_or(&e, "hint", py::t(""), 0).unwrap_or(Value::Null);
            format!("{}: {}\n", py::s(&code), py::s(&hint))
        }
        Value::Str(_) => {
            let m = py::g_or(&v, "message", py::t(""), 0).unwrap_or(Value::Null);
            format!("{}: {}\n", py::s(&e), py::s(&m))
        }
        _ => format!("{}\n", head(&raw)),
    }
}

/// `printf '%s\n' "$_CI_BODY"`.
fn print_body(body: &[u8]) -> Exit {
    writes::print_body(body)
}

/// `[ "$as_json" = 1 ] && printf … || _ci_render <verb>`.
fn show(a: &Ask, verb: &str, body: &[u8]) -> Exit {
    if a.json {
        print_body(body)
    } else {
        render::render(verb, body).print()
    }
}

fn split_list(v: &str) -> Value {
    Value::Array(
        v.split(',')
            .map(|x| x.trim_matches(crate::families::py_space))
            .filter(|x| !x.is_empty())
            .map(str_value)
            .collect(),
    )
}

/// `int(word)` of digits, as `json.dumps` writes it.
fn int_value(digits: &str) -> Value {
    let t = digits.trim_start_matches('0');
    Value::Int(if t.is_empty() { "0".into() } else { t.into() })
}

/// `git rev-parse --show-toplevel` here when git may run.
fn git_top() -> String {
    let Some(git) = daemon::usable_git() else {
        return String::new();
    };
    let Ok(o) = std::process::Command::new(git)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return String::new();
    };
    let mut t = String::from_utf8_lossy(&o.stdout).into_owned();
    while t.ends_with('\n') {
        t.pop();
    }
    t
}

fn verb(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let p0 = a.pos.first().map_or("", String::as_str);
    let r = |pairs: Vec<(&str, &str)>| {
        urlencode(
            &pairs
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .collect::<Vec<_>>(),
        )
    };
    match a.sub.as_str() {
        "status" => {
            let q = r(vec![
                ("workspace", ws),
                ("run", p0),
                ("limit", a.limit.as_str()),
            ]);
            match call(cp, false, "status", &q) {
                Ok(b) => show(a, "status", &b),
                Err(e) => e,
            }
        }
        "run" => {
            let dir = git_top();
            let mut o: Vec<(&str, Value)> = [
                ("workspace", ws),
                ("rev", p0),
                ("ref", a.refs.as_str()),
                ("tier", a.tier.as_str()),
                ("dir", dir.as_str()),
            ]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k, str_value(v)))
            .collect();
            if !a.check.is_empty() {
                o.push(("checks", split_list(&a.check)));
            }
            match call(cp, true, "run", &dumps(&object(o))) {
                Ok(b) if a.wait => wait_run(cp, a, ws, b, 60, false),
                Ok(b) => show(a, "run", &b),
                Err(e) => e,
            }
        }
        "release" => release(cp, a, ws),
        "stats" => {
            if !p0.is_empty() {
                return usage(format!(
                    "usage: {CLI} ci stats [--check NAME] [--branch B] [--days 14] [--json]"
                ));
            }
            let q = r(vec![
                ("workspace", ws),
                ("check", a.check.as_str()),
                ("branch", a.branch.as_str()),
                ("days", a.days.as_str()),
            ]);
            match call(cp, false, "stats", &q) {
                Ok(b) => show(a, "stats", &b),
                Err(e) => e,
            }
        }
        "monitors" => {
            if !p0.is_empty() {
                return usage(format!("usage: {CLI} ci monitors [--json]"));
            }
            match call(cp, false, "monitors", &r(vec![("workspace", ws)])) {
                Ok(b) => show(a, "monitors", &b),
                Err(e) => e,
            }
        }
        "monitor" => monitor(cp, a, ws),
        "tests" => {
            if !p0.is_empty() {
                return usage(format!(
                    "usage: {CLI} ci tests [--check NAME] [--flaky|--slow|--failing|--quarantined] [--days 30] [--limit 50] [--json]"
                ));
            }
            let q = r(vec![
                ("workspace", ws),
                ("check", a.check.as_str()),
                ("filter", a.tests_filter.as_str()),
                ("days", a.days.as_str()),
                ("limit", a.limit.as_str()),
            ]);
            match call(cp, false, "tests", &q) {
                Ok(b) => show(a, "tests", &b),
                Err(e) => e,
            }
        }
        "setup" => setup(cp, a, ws),
        "watch" => watch(cp, a, ws),
        "init" => init(cp, a, ws),
        "freeze" => freeze(cp, a, ws),
        "cache" => cache(cp, a, ws),
        sub => usage(format!(
            "usage: unknown {CLI} ci verb '{sub}' ({CLI} ci --help)"
        )),
    }
}

fn usage(line: String) -> Exit {
    out::stderr_line(&line);
    Exit::Usage
}

fn release(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let version = a.pos.first().map_or("", String::as_str);
    let mut rev = a.pos.get(1).map_or("", String::as_str);
    if version.is_empty() {
        return usage(format!(
            "usage: {CLI} ci release <version> [<rev> | --sha <commit>] [--build a,b] [--tier ci|standard] [--plan] [--wait [--wait-min 180]] [--json]"
        ));
    }
    if !a.sha.is_empty() && !rev.is_empty() {
        return usage(format!(
            "usage: {CLI} ci release <version> takes <rev> or --sha, not both"
        ));
    }
    if !a.sha.is_empty() {
        rev = &a.sha;
    }
    let dir = git_top();
    let mut o: Vec<(&str, Value)> = [
        ("workspace", ws),
        ("version", version),
        ("rev", rev),
        ("tier", a.tier.as_str()),
        ("dir", dir.as_str()),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| (k, str_value(v)))
    .collect();
    if !a.builds.is_empty() {
        o.push(("builds", split_list(&a.builds)));
    }
    if a.plan {
        o.push(("plan", Value::Bool(true)));
    }
    if !a.only_keys.is_empty() {
        o.push(("onlyKeys", split_list(&a.only_keys)));
    }
    let b = match call(cp, true, "release", &dumps(&object(o))) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if a.plan {
        return show(a, "release-plan", &b);
    }
    if a.wait {
        return wait_run(cp, a, ws, b, 180, true);
    }
    show(a, "run", &b)
}

/// `x="$(… json.load(sys.stdin)["run"][key] …)"`: the text; under
/// `set -e` a raise is its traceback and the end of the command (exit 1).
fn run_field(body: &[u8], key: &str) -> Result<String, Exit> {
    render::run_field(body, key).map_err(|tb| {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), tb.as_bytes());
        Exit::Error
    })
}

/// `run --wait` and `release --wait`: ask the run's status every
/// `CI_WAIT_SECS` until it ends, up to `--wait-min` minutes.
fn wait_run(
    cp: &mut Cp<'_>,
    a: &Ask,
    ws: &str,
    first: Vec<u8>,
    default_min: u64,
    release: bool,
) -> Exit {
    let mut body = first;
    let id = match run_field(&body, "id") {
        Ok(t) => t,
        Err(e) => return e,
    };
    let mins: u64 = if a.wait_min.is_empty() {
        default_min
    } else {
        a.wait_min.parse().unwrap_or(u64::MAX / 120)
    };
    let deadline =
        py::now().saturating_add(i64::try_from(mins.saturating_mul(60)).unwrap_or(i64::MAX));
    let secs = wait_secs().unwrap_or(10.0);
    let state = loop {
        let state = match run_field(&body, "state") {
            Ok(t) => t,
            Err(e) => return e,
        };
        if state != "running" {
            break state;
        }
        if py::now() >= deadline {
            let _ = show(a, "run", &body);
            let short: String = id.chars().take(8).collect();
            out::stderr_line(&format!(
                "still running after {} min: {CLI} ci status {short}",
                if a.wait_min.is_empty() {
                    default_min.to_string()
                } else {
                    a.wait_min.clone()
                }
            ));
            return Exit::Error;
        }
        std::thread::sleep(std::time::Duration::from_secs_f64(secs));
        let q = urlencode(
            &[("workspace", ws), ("run", id.as_str())]
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .collect::<Vec<_>>(),
        );
        body = match call(cp, false, "status", &q) {
            Ok(b) => b,
            Err(e) => return e,
        };
    };
    let _ = show(a, "run", &body);
    let ok = if release {
        state == "passed"
    } else {
        matches!(state.as_str(), "passed" | "skipped")
    };
    if ok {
        Exit::Ok
    } else {
        Exit::Error
    }
}

/// `_compute_json`: secrets (`_s`, `Token`) first, then the rest, each in
/// the order given, an empty value left out; `_n` is a number.
fn compute_json(pairs: &[(&str, &str)]) -> String {
    let mut o: Vec<(&str, Value)> = Vec::new();
    for secret in [true, false] {
        for (k, v) in pairs {
            let is_secret = k.ends_with("_s") || k.ends_with("Token");
            if is_secret != secret || v.is_empty() {
                continue;
            }
            if let Some(n) = k.strip_suffix("_n") {
                o.push((n, int_value(v)));
            } else if let Some(n) = k.strip_suffix("_s") {
                o.push((n, str_value(v)));
            } else {
                o.push((k, str_value(v)));
            }
        }
    }
    dumps(&object(o))
}

fn monitor(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let mact = a.pos.first().map_or("", String::as_str);
    let mname = a.pos.get(1).map_or("", String::as_str);
    let musage = format!(
        "usage: {CLI} ci monitor add <name> --kind fail-streak|slow|queue-wait|new-flaky [--branch B] [--check C] [--count 3] [--over 1.5x|10m] [--cooldown 6h] [--to <workspace>|thread[:<addr>]]  |  {CLI} ci monitor mute <name> --for 1d|off  |  {CLI} ci monitor rm <name>"
    );
    if !matches!(mact, "add" | "mute" | "rm" | "remove") {
        return usage(musage);
    }
    if mname.is_empty() || a.pos.len() > 2 && !a.pos[2].is_empty() {
        return usage(musage);
    }
    if mact == "add" && a.mkind.is_empty() {
        return usage(format!(
            "usage: {CLI} ci monitor add needs --kind fail-streak|slow|queue-wait|new-flaky"
        ));
    }
    if mact == "mute" && a.mfor.is_empty() {
        return usage(format!(
            "usage: {CLI} ci monitor mute <name> --for 1d (or --for off)"
        ));
    }
    let mact = if mact == "remove" { "rm" } else { mact };
    let body = compute_json(&[
        ("workspace", ws),
        ("action", mact),
        ("name", mname),
        ("kind", a.mkind.as_str()),
        ("check", a.check.as_str()),
        ("branch", a.branch.as_str()),
        ("count_n", a.mcount.as_str()),
        ("over", a.mover.as_str()),
        ("cooldown", a.mcooldown.as_str()),
        ("to", a.mto.as_str()),
        ("for", a.mfor.as_str()),
    ]);
    match call(cp, true, "monitor", &body) {
        Ok(b) => show(a, "monitor", &b),
        Err(e) => e,
    }
}

fn watch(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let on = a.pos.first().map_or("", String::as_str);
    if on.is_empty() {
        let mut q = vec![("limit", "1")];
        if !ws.is_empty() {
            q.push(("workspace", ws));
        }
        let b = match call(cp, false, "status", &urlencode(&q)) {
            Ok(b) => b,
            Err(e) => return e,
        };
        return render::watch_alone(&b, a.json).print();
    }
    if on != "on" && on != "off" {
        return usage(format!("usage: {CLI} ci watch on|off"));
    }
    let mut o: Vec<(&str, Value)> = vec![("on", Value::Bool(on == "on"))];
    if !ws.is_empty() {
        o.push(("workspace", str_value(ws)));
    }
    if !a.remote.is_empty() {
        o.push(("remote", str_value(&a.remote)));
    }
    if !a.branches.is_empty() {
        o.push(("branches", split_list(&a.branches)));
    }
    if !a.poll.is_empty() {
        o.push(("pollSecs", int_value(&a.poll)));
    }
    if !a.tier.is_empty() {
        o.push(("tier", str_value(&a.tier)));
    }
    if !a.max_inflight.is_empty() {
        o.push(("maxInflight", int_value(&a.max_inflight)));
    }
    if a.skip {
        o.push(("skipCurrent", Value::Bool(true)));
    }
    match call(cp, true, "watch", &dumps(&object(o))) {
        Ok(b) => show(a, "watch", &b),
        Err(e) => e,
    }
}

fn init(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let mut o: Vec<(&str, Value)> = [
        ("workspace", ws),
        ("check", a.check.as_str()),
        ("os", a.os.as_str()),
        ("remote", a.remote.as_str()),
        ("statusesToken", a.token.as_str()),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| (k, str_value(v)))
    .collect();
    for (k, v) in [
        ("memGb", a.mem.as_str()),
        ("cpus", a.cpus.as_str()),
        ("timeoutSecs", a.timeout.as_str()),
    ] {
        if !v.is_empty() {
            o.push((k, int_value(v)));
        }
    }
    if !a.needs.is_empty() {
        o.push(("needs", split_list(&a.needs)));
    }
    if !a.branches.is_empty() {
        o.push(("branches", split_list(&a.branches)));
    }
    if a.force {
        o.push(("force", Value::Bool(true)));
    }
    if a.have_cmd {
        o.push((
            "argv",
            Value::Array(a.argv.iter().map(|x| str_value(x)).collect()),
        ));
    }
    if a.have_cmd && a.argv.is_empty() {
        return usage(format!(
            "usage: {CLI} ci init \u{2026} -- <cmd> [args\u{2026}] (the check's command after --)"
        ));
    }
    match call(cp, true, "init", &dumps(&object(o))) {
        Ok(b) => show(a, "init", &b),
        Err(e) => e,
    }
}

fn freeze(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let body = if a.pos.first().map(String::as_str) == Some("off") {
        let mut o = vec![("off", Value::Bool(true))];
        if !ws.is_empty() {
            o.push(("workspace", str_value(ws)));
        }
        dumps(&object(o))
    } else {
        if a.until.is_empty() {
            return usage(format!(
                "usage: {CLI} ci freeze --until <3d|12h|2026-11-20T00:00:00Z> [--why \"\u{2026}\"] | {CLI} ci freeze off"
            ));
        }
        let secs = match until_secs(&a.until, py::now()) {
            Ok(s) => s,
            Err(line) => return usage(line.to_string()),
        };
        let mut o = vec![("until", Value::Int(secs.to_string()))];
        if !ws.is_empty() {
            o.push(("workspace", str_value(ws)));
        }
        if !a.why.is_empty() {
            o.push(("why", str_value(&a.why)));
        }
        dumps(&object(o))
    };
    match call(cp, true, "freeze", &body) {
        Ok(b) => show(a, "freeze", &b),
        Err(e) => e,
    }
}

fn cache(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    if a.pos.first().map(String::as_str) != Some("trust") {
        return usage(format!(
            "usage: {CLI} ci cache trust [--add B | --set a,b | --remove B] [--json]"
        ));
    }
    let answer = if a.tadd.is_empty() && a.tremove.is_empty() && !a.tset_given {
        let q = if ws.is_empty() {
            String::new()
        } else {
            urlencode(&[("workspace", ws)])
        };
        call(cp, false, "cache/trust", &q)
    } else {
        let mut o: Vec<(&str, Value)> = Vec::new();
        if !ws.is_empty() {
            o.push(("workspace", str_value(ws)));
        }
        if !a.tadd.is_empty() {
            o.push(("add", split_list(&a.tadd)));
        }
        if a.tset_given {
            o.push(("set", split_list(&a.tset)));
        }
        if !a.tremove.is_empty() {
            o.push(("remove", split_list(&a.tremove)));
        }
        call(cp, true, "cache/trust/set", &dumps(&object(o)))
    };
    match answer {
        Ok(b) => show(a, "cache", &b),
        Err(e) => e,
    }
}

fn setup(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    if !a.plan || a.out_dir.is_empty() {
        return usage(format!(
            "usage: {CLI} ci setup --plan --out <file> [<rev>] [--json]"
        ));
    }
    let p0 = a.pos.first().map_or("", String::as_str);
    let o: Vec<(&str, Value)> = [("workspace", ws), ("rev", p0)]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k, str_value(v)))
        .collect();
    let b = match call(cp, true, "setup", &dumps(&object(o))) {
        Ok(b) => b,
        Err(e) => return e,
    };
    render::setup(&b, &a.out_dir, a.json)
}

/// The JSON text of a value Python's `json.dumps` writes.
pub fn dumps_value(v: &Value) -> String {
    json::to_string(v, &json::Options::default()).unwrap_or_default()
}

#[cfg(test)]
mod tests;
