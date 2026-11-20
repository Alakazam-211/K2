//! The writes of an App's phone and mail doors, as the shell CLI's
//! `cli/src/app-links.sh` and `cli/src/app-auth.sh` make them: `app links
//! set|off|check`, `app pairing setup|set|off|check`, `app push
//! setup|creds rm|check|on|off|set|send|test|devices rm` and `app email
//! on|off|test|send`. Each is the shell's word loop and checks, the body
//! its Python builds, the request, and the answer printed by the read's
//! printer ([`super::phone`]).
//!
//! The confirm steps: a 403 whose body names `"vault_confirm_required"`,
//! outside a session, asks for a confirm token over the vault socket
//! ([`crate::confirm::confirm_token`]) and sends the body again with it;
//! the token goes from one body into the next and nowhere else. Pairing
//! and the push keys and switches ask once (`_app_push_keys_post`,
//! `_app_push_on_post`); email asks up to three times (`_email_post`).
//!
//! What the user names (a phone app, a key file's text, a message) goes
//! in the body only: nothing reaches a child's argv or environment.

use super::call::{Cj, Ctx, Split};
use super::phone::{self, usage as read_usage};
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{get, py_space, raised, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib;
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const LINKS_SET: &str = concat!(
        "name at least one phone app: --ios and/or --android (",
        cli_name!(),
        " app links --help)"
    );
    pub const ANDROID: &str = "--android is <package>:<SHA-256 fingerprint>[,<fingerprint>\u{2026}]";
    pub const PAIRING_SETUP: &str = concat!(
        "usage: ",
        cli_name!(),
        " app pairing setup <app> ([--ios TEAMID.com.you.app]\u{2026} [--android com.you.app:AB:CD:\u{2026}]\u{2026} | --desktop-only) [--host h]\u{2026} [--native-origin o]\u{2026} [--link-path p]\u{2026} [--app-store URL] [--play-store URL]"
    );
    pub const PAIRING_DESKTOP: &str =
        "--desktop-only names no phone app: leave out --ios and --android";
    pub const PAIRING_NONE: &str = "name at least one phone app (--ios and/or --android), or --desktop-only for an App whose only native app is a desktop app";
    pub const PAIRING_OFF: &str = concat!("usage: ", cli_name!(), " app pairing off <app>");
    pub const PAIRING_CHECK: &str =
        concat!("usage: ", cli_name!(), " app pairing check <app> [--json]");
    pub const PUSH_SETUP: &str = concat!(
        "usage: ",
        cli_name!(),
        " app push setup <app> [--apns-key FILE.p8 --key-id ID --team-id ID --bundle-id com.you.app] [--fcm-service-account sa.json]"
    );
    pub const PUSH_APNS: &str = "--apns-key needs --key-id, --team-id and --bundle-id";
    pub const PUSH_CREDS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app push creds rm <app> --apns|--fcm|--all"
    );
    pub const PUSH_CHECK: &str = concat!("usage: ", cli_name!(), " app push check <app> [--json]");
    pub const PUSH_ONOFF: &str = concat!("usage: ", cli_name!(), " app push on|off <app>");
    pub const PUSH_SET: &str = concat!(
        "usage: ",
        cli_name!(),
        " app push set <app> [--agent-replies on|off] [--chat on|off] [--previews on|off] [--max-devices N] [--register server|any]"
    );
    pub const PUSH_TEST: &str = concat!("usage: ", cli_name!(), " app push test <app> --to <user>");
    pub const PUSH_SEND: &str = concat!(
        "usage: ",
        cli_name!(),
        " app push send <app> (--to <user>\u{2026} | --room <room>) --title T --body B"
    );
    pub const PUSH_SEND_TO: &str = "name --to <user> (one or more) or --room <room>";
    pub const PUSH_SEND_BOTH: &str = "--to or --room, not both";
    pub const DEVICES_RM: &str = concat!(
        "usage: ",
        cli_name!(),
        " app push devices rm <app> <apd_\u{2026}>"
    );
    pub const EMAIL_ON: &str = concat!(
        "usage: ",
        cli_name!(),
        " app email on <app> --sender <address|id> [--scope users|any]"
    );
    pub const EMAIL_SCOPE: &str = "--scope is users or any";
    pub const EMAIL_MODE: &str = concat!("usage: ", cli_name!(), " app email ");
    pub const EMAIL_SEND: &str = concat!(
        "usage: ",
        cli_name!(),
        " app email send <app> --to <addr> --subject S (--text T | --text-file F) [--html-file F]"
    );
}

/// `Next: k2 app pairing check <app> (…)`.
const PAIRING_NEXT: &str = concat!("Next: ", cli_name!(), " app pairing check ");
/// `Next: k2 app push check <app>`.
const PUSH_NEXT: &str = concat!("Next: ", cli_name!(), " app push check ");
/// `  counts: k2 app push log <app>`.
const PUSH_COUNTS: &str = concat!("  counts: ", cli_name!(), " app push log ");
/// The held email's note.
const EMAIL_HELD: &str = concat!(
    " \u{2014} held for the owner (",
    cli_name!(),
    " mail approvals)"
);

/// Which write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    LinksSet,
    LinksOff,
    LinksCheck,
    PairingSet,
    PairingOff,
    PairingCheck,
    PushSetup,
    PushCredsRm,
    PushCheck,
    PushOnOff,
    PushSet,
    PushSend,
    PushTest,
    DevicesRm,
    EmailOn,
    EmailOff,
    EmailTest,
    EmailSend,
}

/// What the words ask for: one write, its App, its body and `--json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The family's page, on stdout (`push creds` alone).
    Page(&'static str),
    Post {
        kind: Kind,
        app: String,
        body: String,
        json: bool,
        /// `devices rm`'s id.
        id: String,
    },
}

/// The words after the verb. `None`: a help form or a read (the reads
/// are [`super::phone`]'s).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = *words.first()?;
    let rest = &words[1..];
    Some(match (verb, sub) {
        ("links", "set" | "off" | "check") => links(sub, rest),
        ("pairing", "setup" | "set") => pairing_setup(rest),
        ("pairing", "off") => simple(rest, Kind::PairingOff, usage::PAIRING_OFF),
        ("pairing", "check") => simple(rest, Kind::PairingCheck, usage::PAIRING_CHECK),
        ("push", "setup") => push_setup(rest),
        ("push", "creds" | "credentials") => push_creds(rest),
        ("push", "check") => simple(rest, Kind::PushCheck, usage::PUSH_CHECK),
        ("push", "on" | "off") => simple(rest, Kind::PushOnOff, usage::PUSH_ONOFF).map(|a| {
            on_body(a, sub == "on")
        }),
        ("push", "set") => push_set(rest),
        ("push", "send" | "test") => push_send(sub == "test", rest),
        ("push", "devices") if matches!(rest.first(), Some(&"rm" | &"remove")) => {
            devices_rm(&rest[1..])
        }
        ("email", "on") => email_on(rest),
        ("email", "off" | "test") => {
            let line = format!("{}{sub} <app>", usage::EMAIL_MODE);
            let kind = if sub == "off" {
                Kind::EmailOff
            } else {
                Kind::EmailTest
            };
            email_mode(rest, kind, line)
        }
        ("email", "send") => email_send(rest),
        _ => return None,
    })
}

fn unknown_flag(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}"))
}

/// `x="${2:-}"; shift 2`: with one word left, set -e ends the command.
fn value<'a>(rest: &[&'a str]) -> Result<&'a str, Stop> {
    rest.get(1)
        .copied()
        .ok_or_else(|| Stop::silent(Exit::Error))
}

fn post(kind: Kind, app: &str, body: String, json: bool) -> Ask {
    Ask::Post {
        kind,
        app: app.to_string(),
        body,
        json,
        id: String::new(),
    }
}

/// `--json`, `-*` unknown, any other word the App (the last wins); the
/// body `{"app": …}`.
fn simple(words: &[&str], kind: Kind, line: &'static str) -> Result<Ask, Stop> {
    let (mut app, mut json) = (String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(line));
    }
    let body = Cj::new().s("app", &app).body();
    Ok(post(kind, &app, body, json))
}

/// `push on|off`'s body: `{"app": …, "enabled": true|false}`.
fn on_body(a: Ask, on: bool) -> Ask {
    match a {
        Ask::Post { app, json, .. } => {
            let body = Cj::new()
                .s("app", &app)
                .b("enabled", if on { "1" } else { "0" })
                .body();
            post(Kind::PushOnOff, &app, body, json)
        }
        other => other,
    }
}

/// `[x.strip() for x in v.split("\n") if x.strip()]`, as a JSON list.
fn lines(acc: &str) -> Vec<Value> {
    acc.split('\n')
        .map(|x| x.trim_matches(py_space))
        .filter(|x| !x.is_empty())
        .map(text)
        .collect()
}

fn text(t: &str) -> Value {
    Value::Str(Text::from(t))
}

fn dumps(pairs: Vec<(Text, Value)>) -> String {
    json::to_string(&Value::Object(pairs), &Options::default()).unwrap_or_default()
}

/// `b[k] = v`: a key already there keeps its place.
fn put(pairs: &mut Vec<(Text, Value)>, key: &str, v: Value) {
    let key = Text::from(key);
    match pairs.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = v,
        None => pairs.push((key, v)),
    }
}

/// The phone apps' `android` list, or the line the Python exits 2 with.
fn android(acc: &str) -> Result<Vec<Value>, Stop> {
    let mut out = Vec::new();
    for spec in lines(acc) {
        let spec = py::s(&spec);
        let Some((pkg, fprs)) = spec.split_once(':') else {
            return Err(Stop::usage(usage::ANDROID));
        };
        let sha: Vec<Value> = fprs
            .split(',')
            .map(|f| f.trim_matches(py_space))
            .filter(|f| !f.is_empty())
            .map(text)
            .collect();
        out.push(Value::Object(vec![
            (Text::from("package"), text(pkg)),
            (Text::from("sha256"), Value::Array(sha)),
        ]));
    }
    Ok(out)
}

/// `cmd_app_links set|off|check`.
fn links(sub: &str, words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json) = (String::new(), false);
    let (mut ios, mut andr, mut paths, mut drops) =
        (String::new(), String::new(), String::new(), String::new());
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let acc = match w {
            "--ios" => Some(&mut ios),
            "--android" => Some(&mut andr),
            "--path" => Some(&mut paths),
            "--drop" => Some(&mut drops),
            _ => None,
        };
        if let Some(acc) = acc {
            if sub != "set" {
                return Err(Stop::usage(format!("{w}{}", read_usage::LINKS_SET_ONLY)));
            }
            acc.push_str(rest.get(1).copied().unwrap_or(""));
            acc.push('\n');
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    read_usage::LINKS_FLAG_TAIL
                )))
            }
            _ if app.is_empty() => app = w.to_string(),
            _ => {
                return Err(Stop::usage(format!(
                    "Unexpected argument: {w}{}",
                    read_usage::LINKS_FLAG_TAIL
                )))
            }
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(format!(
            "usage: {} app links {sub} <app>{}",
            cli_name!(),
            read_usage::LINKS_FLAG_TAIL
        )));
    }
    let kind = match sub {
        "set" => Kind::LinksSet,
        "off" => Kind::LinksOff,
        _ => Kind::LinksCheck,
    };
    if kind != Kind::LinksSet {
        let body = Cj::new().s("app", &app).body();
        return Ok(post(kind, &app, body, json));
    }
    if ios.is_empty() && andr.is_empty() {
        return Err(Stop::usage(usage::LINKS_SET));
    }
    let mut b = vec![
        (Text::from("app"), text(&app)),
        (Text::from("paths"), Value::Array(lines(&paths))),
    ];
    let ios = lines(&ios);
    if !ios.is_empty() {
        b.push((Text::from("ios"), Value::Array(ios)));
    }
    let andr = android(&andr)?;
    if !andr.is_empty() {
        b.push((Text::from("android"), Value::Array(andr)));
    }
    let drops = lines(&drops);
    if !drops.is_empty() {
        b.push((Text::from("drop"), Value::Array(drops)));
    }
    Ok(post(kind, &app, dumps(b), json))
}

/// `cmd_app_pairing_setup`.
fn pairing_setup(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json, mut desktop) = (String::new(), false, false);
    let mut acc: [String; 5] = Default::default();
    let (mut appstore, mut playstore) = (String::new(), String::new());
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = ["--ios", "--android", "--host", "--native-origin", "--link-path"]
            .iter()
            .position(|f| *f == w);
        if let Some(i) = slot {
            acc[i].push_str(rest.get(1).copied().unwrap_or(""));
            acc[i].push('\n');
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        if w == "--app-store" || w == "--play-store" {
            let v = rest.get(1).copied().unwrap_or("").to_string();
            if w == "--app-store" {
                appstore = v;
            } else {
                playstore = v;
            }
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        match w {
            "--desktop-only" => desktop = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::PAIRING_SETUP));
    }
    let [ios, andr, hosts, origins, paths] = acc;
    let phones = !ios.is_empty() || !andr.is_empty();
    if desktop && phones {
        return Err(Stop::usage(format!(
            "{}\n{}",
            usage::PAIRING_SETUP,
            usage::PAIRING_DESKTOP
        )));
    }
    if !desktop && !phones {
        return Err(Stop::usage(format!(
            "{}\n{}",
            usage::PAIRING_SETUP,
            usage::PAIRING_NONE
        )));
    }
    let mut b = vec![(Text::from("app"), text(&app))];
    if desktop {
        b.push((Text::from("desktopOnly"), Value::Bool(true)));
    }
    let ios = lines(&ios);
    if !ios.is_empty() {
        b.push((Text::from("ios"), Value::Array(ios)));
    }
    let andr = android(&andr)?;
    if !andr.is_empty() {
        b.push((Text::from("android"), Value::Array(andr)));
    }
    for (key, acc) in [("hosts", hosts), ("nativeOrigins", origins), ("linkPaths", paths)] {
        let l = lines(&acc);
        if !l.is_empty() {
            b.push((Text::from(key), Value::Array(l)));
        }
    }
    let mut store = Vec::new();
    if !appstore.is_empty() {
        store.push((Text::from("ios"), text(&appstore)));
    }
    if !playstore.is_empty() {
        store.push((Text::from("android"), text(&playstore)));
    }
    if !store.is_empty() {
        b.push((Text::from("store"), Value::Object(store)));
    }
    Ok(post(Kind::PairingSet, &app, dumps(b), json))
}

/// `[ -r "$path" ]`.
fn readable(path: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(path) else {
        return false;
    };
    // SAFETY: `c` is a NUL-ended path that lives for the call.
    unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
}

/// `open(path, encoding="utf-8").read()` at `line`, or the traceback.
fn read_file(path: &str, line: u32) -> Result<String, Raise> {
    pylib::json::read_text_file(path).map_err(|e| Raise::at(line, e.last_line()))
}

/// `cmd_app_push_setup`.
fn push_setup(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json) = (String::new(), false);
    let mut v: [String; 5] = Default::default();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = match w {
            "--apns-key" => Some(0),
            "--key-id" => Some(1),
            "--team-id" => Some(2),
            "--bundle-id" => Some(3),
            "--fcm-service-account" | "--fcm" => Some(4),
            _ => None,
        };
        if let Some(i) = slot {
            v[i] = rest.get(1).copied().unwrap_or("").to_string();
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    let [p8, kid, team, bundle, sa] = v;
    if app.is_empty() || (p8.is_empty() && sa.is_empty()) {
        return Err(Stop::usage(usage::PUSH_SETUP));
    }
    if !p8.is_empty() && (kid.is_empty() || team.is_empty() || bundle.is_empty()) {
        return Err(Stop::usage(usage::PUSH_APNS));
    }
    for f in [&p8, &sa] {
        if !f.is_empty() && !readable(f) {
            return Err(Stop::usage(format!("can't read {f}")));
        }
    }
    let unread = |e: Raise| {
        let mut line = e.text();
        line.push_str("couldn't read the key file(s)");
        Stop::usage(line)
    };
    let mut b = vec![(Text::from("app"), text(&app))];
    if !p8.is_empty() {
        let key = read_file(&p8, 6).map_err(unread)?;
        b.push((
            Text::from("apns"),
            Value::Object(vec![
                (Text::from("p8"), text(&key)),
                (Text::from("keyId"), text(&kid)),
                (Text::from("teamId"), text(&team)),
                (Text::from("bundleId"), text(&bundle)),
            ]),
        ));
    }
    if !sa.is_empty() {
        let json_text = read_file(&sa, 9).map_err(unread)?;
        b.push((
            Text::from("fcm"),
            Value::Object(vec![(Text::from("serviceAccountJson"), text(&json_text))]),
        ));
    }
    Ok(post(Kind::PushSetup, &app, dumps(b), json))
}

/// `cmd_app_push_creds`.
fn push_creds(words: &[&str]) -> Result<Ask, Stop> {
    match words.first().copied() {
        None | Some("") => return Ok(Ask::Page("app/push")),
        Some("rm" | "remove") => {}
        Some(_) => return Err(Stop::usage(usage::PUSH_CREDS)),
    }
    let (mut app, mut platform, mut json) = (String::new(), "", false);
    for w in &words[1..] {
        match *w {
            "--apns" => platform = "apns",
            "--fcm" => platform = "fcm",
            "--all" => platform = "all",
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() || platform.is_empty() {
        return Err(Stop::usage(usage::PUSH_CREDS));
    }
    let body = Cj::new().s("app", &app).s("platform", platform).body();
    Ok(post(Kind::PushCredsRm, &app, body, json))
}

/// `on` 1, `off` 0, nothing empty; anything else is the usage line.
fn on_off(v: &str) -> Result<&'static str, Stop> {
    match v {
        "on" => Ok("1"),
        "off" => Ok("0"),
        "" => Ok(""),
        _ => Err(Stop::usage(usage::PUSH_SET)),
    }
}

/// `cmd_app_push_set`.
fn push_set(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json) = (String::new(), false);
    let mut v: [String; 5] = Default::default();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = match w {
            "--agent-replies" => Some(0),
            "--chat" => Some(1),
            "--previews" => Some(2),
            "--max-devices" => Some(3),
            "--register" => Some(4),
            _ => None,
        };
        if let Some(i) = slot {
            v[i] = rest.get(1).copied().unwrap_or("").to_string();
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::PUSH_SET));
    }
    let [replies, chat, previews, maxd, register] = v;
    let s_b = match register.as_str() {
        "server" => "1",
        "any" => "0",
        "" => "",
        _ => return Err(Stop::usage(usage::PUSH_SET)),
    };
    let r_b = on_off(&replies)?;
    let c_b = on_off(&chat)?;
    let p_b = on_off(&previews)?;
    if !maxd.is_empty() && !maxd.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Stop::usage(usage::PUSH_SET));
    }
    if [r_b, c_b, p_b, maxd.as_str(), s_b].iter().all(|x| x.is_empty()) {
        return Err(Stop::usage(usage::PUSH_SET));
    }
    let body = Cj::new()
        .s("app", &app)
        .b("agentReplies", r_b)
        .b("chatPushes", c_b)
        .b("previews", p_b)
        .n("maxDevices", &maxd)
        .b("serverOnly", s_b)
        .body();
    Ok(post(Kind::PushSet, &app, body, json))
}

/// Python's `int(word)` for ASCII digits: the value, or `None` where it
/// raises `ValueError`.
fn py_int(word: &str) -> Option<Value> {
    let w = word.trim_matches(py_space);
    let (neg, body) = match w.as_bytes().first() {
        Some(b'-') => (true, &w[1..]),
        Some(b'+') => (false, &w[1..]),
        _ => (false, w),
    };
    let b = body.as_bytes();
    if b.is_empty() || b[0] == b'_' || b[b.len() - 1] == b'_' || body.contains("__") {
        return None;
    }
    if !b.iter().all(|c| c.is_ascii_digit() || *c == b'_') {
        return None;
    }
    let digits: String = body.chars().filter(|c| *c != '_').collect();
    let digits = digits.trim_start_matches('0');
    Some(Value::Int(match (neg, digits.is_empty()) {
        (_, true) => "0".to_string(),
        (true, false) => format!("-{digits}"),
        (false, false) => digits.to_string(),
    }))
}

/// `cmd_app_push_send send|test`.
fn push_send(test: bool, words: &[&str]) -> Result<Ask, Stop> {
    // title, body, topic, collapse, thread, sound, badge, priority, ttl, channel
    const FLAGS: [&str; 10] = [
        "--title",
        "--body",
        "--topic",
        "--collapse-id",
        "--thread-id",
        "--sound",
        "--badge",
        "--priority",
        "--ttl",
        "--android-channel",
    ];
    let (mut app, mut room, mut json, mut silent) = (String::new(), String::new(), false, false);
    let mut v: [String; 10] = Default::default();
    let (mut tos, mut datas): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let given = rest.get(1).copied().unwrap_or("").to_string();
        let took = if w == "--to" {
            tos.push(given);
            true
        } else if w == "--data" {
            datas.push(given);
            true
        } else if w == "--room" {
            room = given;
            true
        } else if let Some(i) = FLAGS.iter().position(|f| *f == w) {
            v[i] = given;
            true
        } else {
            false
        };
        if took {
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        match w {
            "--silent" => silent = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if test {
        if app.is_empty() || tos.is_empty() {
            return Err(Stop::usage(usage::PUSH_TEST));
        }
    } else {
        if app.is_empty() {
            return Err(Stop::usage(usage::PUSH_SEND));
        }
        if tos.is_empty() && room.is_empty() {
            return Err(Stop::usage(usage::PUSH_SEND_TO));
        }
        if !tos.is_empty() && !room.is_empty() {
            return Err(Stop::usage(usage::PUSH_SEND_BOTH));
        }
    }
    // `sys.argv[1:]` is the --to values, `--`, the --data values; the
    // split is at the first `--`.
    let mut args = tos.clone();
    args.push("--".to_string());
    args.extend(datas.iter().cloned());
    let sep = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (tos, datas) = (&args[..sep], &args[sep + 1..]);
    let mut b = vec![(Text::from("app"), text(&app))];
    let to = if !room.is_empty() {
        vec![(Text::from("room"), text(&room))]
    } else if tos.len() == 1 {
        vec![(Text::from("guest"), text(&tos[0]))]
    } else {
        vec![(
            Text::from("guests"),
            Value::Array(tos.iter().map(|t| text(t)).collect()),
        )]
    };
    b.push((Text::from("to"), Value::Object(to)));
    if !test {
        let names = [
            (0, "title"),
            (1, "body"),
            (2, "topic"),
            (3, "collapseId"),
            (4, "threadId"),
            (5, "sound"),
            (7, "priority"),
            (9, "androidChannel"),
        ];
        for (i, name) in names {
            if !v[i].is_empty() {
                b.push((Text::from(name), text(&v[i])));
            }
        }
        for (i, name) in [(6, "badge"), (8, "ttlSecs")] {
            if !v[i].is_empty() {
                let Some(n) = py_int(&v[i]) else {
                    return Err(Stop::usage(format!(
                        "--{} is a number",
                        name.to_lowercase()
                    )));
                };
                b.push((Text::from(name), n));
            }
        }
        if silent {
            b.push((Text::from("silent"), Value::Bool(true)));
        }
        if !datas.is_empty() {
            let mut d = Vec::new();
            for kv in datas {
                let Some((k, val)) = kv.split_once('=') else {
                    return Err(Stop::usage("--data is key=value"));
                };
                put(&mut d, k, text(val));
            }
            b.push((Text::from("data"), Value::Object(d)));
        }
    }
    let kind = if test { Kind::PushTest } else { Kind::PushSend };
    Ok(post(kind, &app, dumps(b), json))
}

/// `cmd_app_push_devices rm <app> <id>`: two plain words, flags or not.
fn devices_rm(words: &[&str]) -> Result<Ask, Stop> {
    let app = words.first().copied().unwrap_or("");
    let id = words.get(1).copied().unwrap_or("");
    if app.is_empty() || id.is_empty() {
        return Err(Stop::usage(usage::DEVICES_RM));
    }
    let body = Cj::new().s("app", app).s("id", id).body();
    Ok(Ask::Post {
        kind: Kind::DevicesRm,
        app: app.to_string(),
        body,
        json: false,
        id: id.to_string(),
    })
}

/// `cmd_app_email_on`.
fn email_on(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut sender, mut scope, mut json) =
        (String::new(), String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        if matches!(w, "--sender" | "--from" | "--scope") {
            let v = rest.get(1).copied().unwrap_or("").to_string();
            if w == "--scope" {
                scope = v;
            } else {
                sender = v;
            }
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() || sender.is_empty() {
        return Err(Stop::usage(usage::EMAIL_ON));
    }
    if !matches!(scope.as_str(), "" | "users" | "any") {
        return Err(Stop::usage(usage::EMAIL_SCOPE));
    }
    let body = Cj::new()
        .s("app", &app)
        .s("sender", &sender)
        .s("scope", &scope)
        .body();
    Ok(post(Kind::EmailOn, &app, body, json))
}

/// `cmd_app_email_onoff off|test`.
fn email_mode(words: &[&str], kind: Kind, line: String) -> Result<Ask, Stop> {
    let (mut app, mut json) = (String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(line));
    }
    let body = Cj::new().s("app", &app).body();
    Ok(post(kind, &app, body, json))
}

/// `cmd_app_email_send`.
fn email_send(words: &[&str]) -> Result<Ask, Stop> {
    const LISTS: [(&str, &str); 6] = [
        ("--to", "to"),
        ("--cc", "cc"),
        ("--bcc", "bcc"),
        ("--reply-to", "replyTo"),
        ("--header", "headers"),
        ("--tag", "tags"),
    ];
    let (mut app, mut json) = (String::new(), false);
    let mut lists: [Vec<String>; 6] = Default::default();
    // subject, text, text-file, html-file, idempotency key
    let mut v: [String; 5] = Default::default();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let given = rest.get(1).copied().unwrap_or("").to_string();
        let took = if let Some(i) = LISTS.iter().position(|(f, _)| *f == w) {
            lists[i].push(given);
            true
        } else if let Some(i) = [
            "--subject",
            "--text",
            "--text-file",
            "--html-file",
            "--idempotency-key",
        ]
        .iter()
        .position(|f| *f == w)
        {
            v[i] = given;
            true
        } else {
            false
        };
        if took {
            value(rest)?;
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    let [subject, txt, text_file, html_file, key] = v;
    if app.is_empty() || lists[0].is_empty() || subject.is_empty() {
        return Err(Stop::usage(usage::EMAIL_SEND));
    }
    if txt.is_empty() && text_file.is_empty() && html_file.is_empty() {
        return Err(Stop::usage(usage::EMAIL_SEND));
    }
    for f in [&text_file, &html_file] {
        if !f.is_empty() && !readable(f) {
            return Err(Stop::usage(format!("can't read {f}")));
        }
    }
    // The Python's argv: each list's name, then its values; a value that
    // is `--<name>` of a list starts that list.
    let mut parsed: [Vec<String>; 6] = Default::default();
    let mut cur = 0usize;
    for (i, (_, name)) in LISTS.iter().enumerate() {
        for a in std::iter::once(format!("--{name}")).chain(lists[i].iter().cloned()) {
            if let Some(j) = a
                .strip_prefix("--")
                .and_then(|n| LISTS.iter().position(|(_, l)| *l == n))
            {
                cur = j;
                continue;
            }
            parsed[cur].push(a);
        }
    }
    let list = |l: &[String]| Value::Array(l.iter().map(|x| text(x)).collect());
    let mut b = vec![
        (Text::from("app"), text(&app)),
        (Text::from("subject"), text(&subject)),
        (Text::from("to"), list(&parsed[0])),
    ];
    for i in [1, 2, 3, 5] {
        if !parsed[i].is_empty() {
            b.push((Text::from(LISTS[i].1), list(&parsed[i])));
        }
    }
    if !parsed[4].is_empty() {
        let mut h = Vec::new();
        for kv in &parsed[4] {
            let Some((k, val)) = kv.split_once('=') else {
                return Err(Stop::usage("--header is X-Name=value"));
            };
            put(&mut h, k, text(val));
        }
        b.push((Text::from("headers"), Value::Object(h)));
    }
    let traceback = |e: Raise| {
        let mut t = e.text();
        while t.ends_with('\n') {
            t.pop();
        }
        Stop::usage(t)
    };
    if !text_file.is_empty() {
        b.push((Text::from("text"), text(&read_file(&text_file, 26).map_err(traceback)?)));
    } else if !txt.is_empty() {
        b.push((Text::from("text"), text(&txt)));
    }
    if !html_file.is_empty() {
        b.push((Text::from("html"), text(&read_file(&html_file, 31).map_err(traceback)?)));
    }
    if !key.is_empty() {
        b.push((Text::from("idempotencyKey"), text(&key)));
    }
    Ok(post(Kind::EmailSend, &app, dumps(b), json))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let (kind, app, body, json, id) = match ask {
        Ask::Page(p) => return Ok(Shown::out(crate::text::help(p).unwrap_or_default())),
        Ask::Post {
            kind,
            app,
            body,
            json,
            id,
        } => (*kind, app.as_str(), body.clone(), *json, id.as_str()),
    };
    let answer = match kind {
        Kind::LinksSet => {
            let s = ctx.send(true, "/cli/app/links/set", &body, false)?;
            ok_200(ctx, s)?
        }
        Kind::LinksOff => ctx.call(true, "/cli/app/links/off", &body)?,
        Kind::LinksCheck => ctx.call_on(true, "/cli/app/links/check", &body, true)?,
        Kind::PairingSet => {
            confirming(ctx, "/cli/app/pairing/set", body, "app-pairing-apps", "")?
        }
        Kind::PairingOff => ctx.call(true, "/cli/app/pairing/off", &body)?,
        Kind::PairingCheck => ctx.call_on(true, "/cli/app/pairing/check", &body, true)?,
        Kind::PushSetup => confirming(
            ctx,
            "/cli/app/push/credentials",
            body,
            "app-push-keys",
            &format!("Push keys for {app}"),
        )?,
        Kind::PushCredsRm => confirming(
            ctx,
            "/cli/app/push/credentials/remove",
            body,
            "app-push-keys",
            &format!("Remove push keys for {app}"),
        )?,
        Kind::PushCheck => ctx.call_on(true, "/cli/app/push/check", &body, true)?,
        Kind::PushOnOff | Kind::PushSet => {
            confirming(ctx, "/cli/app/push/set", body, "app-push-on", "")?
        }
        Kind::PushSend | Kind::PushTest => {
            let path = if kind == Kind::PushSend {
                "/cli/app/push/send"
            } else {
                "/cli/app/push/test"
            };
            let s = ctx.send(true, path, &body, false)?;
            ok_200_202(ctx, s)?
        }
        Kind::DevicesRm => {
            ctx.call(true, "/cli/app/push/devices/remove", &body)?;
            return Ok(Shown::out(format!("removed {id}\n")));
        }
        Kind::EmailOn => email_post(ctx, "/cli/app/email/on", body)?,
        Kind::EmailOff => email_post(ctx, "/cli/app/email/off", body)?,
        Kind::EmailTest => email_post(ctx, "/cli/app/email/test", body)?,
        Kind::EmailSend => email_post(ctx, "/cli/app/email/send", body)?,
    };
    if json {
        return Ok(super::wallet::raw(&answer));
    }
    let shown = match kind {
        Kind::LinksSet | Kind::LinksOff => print(&answer, phone::links_lines, None),
        Kind::LinksCheck | Kind::PairingCheck => print(&answer, check_lines, None),
        Kind::PairingSet => print(
            &answer,
            phone::pairing_lines,
            Some(format!(
                "{PAIRING_NEXT}{app} (Apple's CDN can take up to a day to fetch a new host)"
            )),
        ),
        Kind::PairingOff => print(&answer, phone::pairing_lines, None),
        Kind::PushSetup => print(
            &answer,
            phone::push_lines,
            Some(format!("{PUSH_NEXT}{app}")),
        ),
        Kind::PushCredsRm | Kind::PushOnOff | Kind::PushSet => {
            print(&answer, phone::push_lines, None)
        }
        Kind::PushCheck => print(&answer, push_check_lines, None),
        Kind::PushSend | Kind::PushTest => {
            let mut o = Out::default();
            let r = push_sent_lines(&answer, app, &mut o);
            o.done(r)
        }
        Kind::EmailOn | Kind::EmailOff => print(&answer, phone::email_lines, None),
        Kind::EmailTest => print(&answer, email_test_lines, None),
        Kind::EmailSend => print(&answer, email_sent_lines, None),
        Kind::DevicesRm => unreachable!("printed above"),
    };
    Ok(shown)
}

/// The printer, then `extra` when it ended well.
fn print(
    body: &[u8],
    lines: fn(&[u8], &mut Out) -> Result<Exit, Raise>,
    extra: Option<String>,
) -> Shown {
    let mut o = Out::default();
    let r = lines(body, &mut o);
    if let (Ok(Exit::Ok), Some(line)) = (&r, extra) {
        o.line(&line);
    }
    o.done(r)
}

/// `[ "$_CP_STATUS" = "200" ] || _compute_fail …`.
fn ok_200(ctx: &Ctx, s: Split) -> Result<Vec<u8>, Exit> {
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

/// `case "$_CP_STATUS" in 200|202) ;; *) _compute_fail …`.
fn ok_200_202(ctx: &Ctx, s: Split) -> Result<Vec<u8>, Exit> {
    if s.status == "200" || s.status == "202" {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

/// Whether an answer asks for a confirm the shell would ask for here.
fn wants_confirm(ctx: &Ctx, s: &Split) -> bool {
    s.status == "403"
        && s.body
            .windows(b"\"vault_confirm_required\"".len())
            .any(|w| w == b"\"vault_confirm_required\"")
        && !ctx.in_session()
}

/// `print(… (json.load(sys.stdin).get("error") or {}).get(k) or …)`, as
/// `$(…)` holds it: the first of `keys` that is there and true. A raise is
/// its traceback, and set -e ends the command (exit 1).
fn error_field(body: &[u8], keys: &[&str]) -> Result<String, Exit> {
    let r = (|| {
        let v = py::load(body, 1)?;
        let e = py::or(py::dget(&v, "error", 1)?, Value::Object(Vec::new()));
        let mut out = Value::Null;
        for k in keys {
            out = py::dget(&e, k, 1)?;
            if py::truthy(&out) {
                break;
            }
        }
        Ok(py::s(&py::or(out, py::text(""))))
    })();
    match r {
        Ok(mut t) => {
            while t.ends_with('\n') {
                t.pop();
            }
            Ok(t)
        }
        Err(e) => Err(raised(String::new(), String::new(), &e).print()),
    }
}

/// `_app_push_keys_post`, `_app_push_on_post` and the pairing set's own
/// copy: one confirm when the daemon asks for one, then 200 or
/// `_compute_fail`.
fn confirming(
    ctx: &mut Ctx,
    path: &str,
    body: String,
    action: &str,
    label: &str,
) -> Result<Vec<u8>, Exit> {
    let mut s = ctx.send(true, path, &body, false)?;
    if wants_confirm(ctx, &s) {
        let svc = error_field(&s.body, &["serviceId"])?;
        if svc.is_empty() {
            return Err(ctx.fail(&s));
        }
        let Some(tok) = crate::confirm::confirm_token(&mut ctx.confirm(), action, &svc, label)
        else {
            return Err(Exit::Refused);
        };
        let Some(next) = crate::confirm::with_confirm_token(&body, &tok, None) else {
            return Err(Exit::Error);
        };
        drop(tok);
        s = ctx.send(true, path, &next, false)?;
    }
    ok_200(ctx, s)
}

/// `_email_post`: up to three confirms, then 200 or 202, else
/// `_compute_fail`.
fn email_post(ctx: &mut Ctx, path: &str, mut body: String) -> Result<Vec<u8>, Exit> {
    let mut tries = 0;
    loop {
        let s = ctx.send(true, path, &body, false)?;
        if !(wants_confirm(ctx, &s) && tries < 3) {
            return ok_200_202(ctx, s);
        }
        let action = error_field(&s.body, &["action"])?;
        let sid = error_field(&s.body, &["secretId", "serviceId"])?;
        if action.is_empty() || sid.is_empty() {
            return Err(ctx.fail(&s));
        }
        let Some(tok) = crate::confirm::confirm_token(&mut ctx.confirm(), &action, &sid, "")
        else {
            return Err(Exit::Refused);
        };
        let field = if action == "app-email-any" {
            "confirmAnyToken"
        } else {
            "confirmToken"
        };
        let Some(next) = crate::confirm::with_confirm_token(&body, &tok, Some(field)) else {
            return Err(Exit::Error);
        };
        body = next;
        tries += 1;
    }
}

/// The words a link check prints for a file's state.
const CHECK_WORDS: &[(&str, &str)] = &[
    ("ok", "ok"),
    ("none", "not used"),
    (
        "stale",
        "another file is served there (DNS or a proxy elsewhere, or a cached copy)",
    ),
    ("not_served", "not served"),
    ("tls_error", "TLS error"),
];

/// `cmd_app_links check` and `cmd_app_pairing_check`'s Python: exit 1
/// when a host's iOS or Android file isn't right.
pub fn check_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let mut bad = false;
    let hosts = py::or(py::dget(&v, "hosts", 7)?, py::empty_list());
    for h in py::iter(&hosts, 7)? {
        o.line(&py::s(&py::dget(&h, "host", 8)?));
        for (k, n) in [
            ("aasa", "iOS file"),
            ("assetlinks", "Android file"),
            ("appleCdn", "Apple CDN copy"),
        ] {
            let s = py::dget(&h, k, 10)?;
            if s != py::text("ok") && s != py::text("none") && k != "appleCdn" {
                bad = true;
            }
            let word = py::word_for(CHECK_WORDS, &s, s.clone(), 13)?;
            o.line(&format!(
                "  {} {}",
                crate::families::ljust(&format!("{n}:"), 15),
                py::s(&word)
            ));
        }
    }
    Ok(if bad { Exit::Error } else { Exit::Ok })
}

/// `cmd_app_push_check`'s Python: exit 1 when a key is refused.
pub fn push_check_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let mut bad = false;
    for (p, name) in [("apns", "Apple"), ("fcm", "Firebase")] {
        let results = py::or(py::dget(&v, "results", 6)?, Value::Object(Vec::new()));
        let r = py::or(py::dget(&results, p, 6)?, Value::Object(Vec::new()));
        let name_9 = crate::families::ljust(name, 9);
        if !py::truthy(&py::dget(&r, "configured", 7)?) {
            o.line(&format!("{name_9} no key"));
        } else if py::truthy(&py::dget(&r, "ok", 9)?) {
            o.line(&format!("{name_9} key works"));
        } else {
            bad = true;
            o.line(&format!(
                "{name_9} refused: {}",
                py::s(&py::dget(&r, "code", 13)?)
            ));
        }
        if py::truthy(&py::dget(&r, "limits", 14)?) {
            let limits = py::idx(&r, "limits", 15)?;
            o.line(&py::concat("          ", &limits, 15)?);
        }
    }
    Ok(if bad { Exit::Error } else { Exit::Ok })
}

/// `cmd_app_push_send`'s Python.
fn push_sent_lines(body: &[u8], app: &str, o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "skipped", 4)?, Value::Object(Vec::new()));
    let Value::Object(pairs) = &s else {
        return Err(Raise::no_attribute(5, &s, "items"));
    };
    let mut parts = Vec::new();
    for (k, n) in pairs {
        if py::truthy(n) {
            parts.push(format!(
                "{} {}",
                py::s(n),
                py::s(&Value::Str(k.clone())).replace('_', " ")
            ));
        }
    }
    let skipped = parts.join(", ");
    let tail = if skipped.is_empty() {
        String::new()
    } else {
        format!("; skipped: {skipped}")
    };
    o.line(&format!(
        "queued {} device send(s), send {}{tail}",
        py::s(&py::dget(&v, "queued", 6)?),
        py::s(&py::dget(&v, "sendId", 6)?)
    ));
    o.line(&format!("{PUSH_COUNTS}{app}"));
    Ok(Exit::Ok)
}

/// `cmd_app_email_onoff test`'s Python.
fn email_test_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 1)?;
    o.line(&format!(
        "test email {}: {} (to {})",
        py::s(&py::dget(&v, "id", 1)?),
        py::s(&py::dget(&v, "result", 1)?),
        py::s(&py::dget(&v, "to", 1)?)
    ));
    Ok(Exit::Ok)
}

/// `cmd_app_email_send`'s Python, then its held check: exit 2 when the
/// email waits for the owner.
fn email_sent_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let recipients = py::dget(&v, "recipients", 4)?;
    let plural = if pylib::ops::py_eq(&recipients, &Value::Int("1".into())) {
        ""
    } else {
        "s"
    };
    let mut line = format!(
        "email {}: {} ({} recipient{plural})",
        py::s(&py::dget(&v, "id", 4)?),
        py::s(&py::dget(&v, "status", 4)?),
        py::s(&recipients)
    );
    let held = py::dget(&v, "status", 5)? == py::text("held");
    if held {
        line.push_str(EMAIL_HELD);
    }
    o.line(&line);
    Ok(if held { Exit::Usage } else { Exit::Ok })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(words: &[&str], verb: &str) -> String {
        match parse(verb, words) {
            Some(Ok(Ask::Post { body, .. })) => body,
            other => panic!("{other:?}"),
        }
    }

    fn stop(words: &[&str], verb: &str) -> Stop {
        match parse(verb, words) {
            Some(Err(s)) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bodies_are_what_the_python_builds() {
        assert_eq!(
            body(
                &["set", "acme", "--ios", " T.app \n", "--android", "com.a: AB , CD", "--drop", "/x"],
                "links"
            ),
            r#"{"app": "acme", "paths": [], "ios": ["T.app"], "android": [{"package": "com.a", "sha256": ["AB", "CD"]}], "drop": ["/x"]}"#
        );
        assert_eq!(
            body(&["setup", "a", "--desktop-only", "--app-store", "u"], "pairing"),
            r#"{"app": "a", "desktopOnly": true, "store": {"ios": "u"}}"#
        );
        assert_eq!(
            body(&["set", "a", "--chat", "on", "--max-devices", "007"], "push"),
            r#"{"app": "a", "chatPushes": true, "maxDevices": 7}"#
        );
        assert_eq!(
            body(
                &["send", "a", "--to", "u", "--badge", " +1_0 ", "--data", "k=v=w", "--data", "k=z"],
                "push"
            ),
            r#"{"app": "a", "to": {"guest": "u"}, "badge": 10, "data": {"k": "z"}}"#
        );
    }

    #[test]
    fn a_to_of_two_dashes_moves_the_split() {
        // `args.index("--")` finds the --to value first: "b" is data.
        assert_eq!(
            stop(&["send", "a", "--to", "--", "--to", "b", "--data", "k=v"], "push").line,
            "--data is key=value"
        );
    }

    #[test]
    fn the_shells_mistakes_are_copied() {
        assert_eq!(stop(&["off", "a", "--ios", "x"], "links").exit, Exit::Usage);
        assert_eq!(stop(&["set", "a", "--ios"], "links"), Stop::silent(Exit::Error));
        assert_eq!(
            stop(&["send", "a", "--to", "u", "--ttl", "x"], "push").line,
            "--ttlsecs is a number"
        );
        assert_eq!(stop(&["set", "a"], "push").line, usage::PUSH_SET);
        assert!(parse("push", &["devices", "acme"]).is_none());
        assert!(parse("email", &["show", "acme"]).is_none());
    }
}
