//! `k5 hostchat recording …` (`_hostchat_recording` and
//! `_hostchat_recording_policy`, `cli/src/hostchat-voice.sh`): Sew's
//! recorder (status, preflight, install, enable, disable, uninstall) and
//! the server's recording policies. Enabling the recorder and a policy
//! change the daemon holds for a confirm are a person's, with a confirm
//! token over the vault socket; the policy's confirm is
//! `_rec_needs_confirm` and `_rec_send_confirmed`
//! (`cli/src/app-recording.sh`), written here until the app lane's
//! recording verbs share them.

use super::main_verbs::{field, install_id, token};
use super::print::{self, g, load, or, st, sub, sv, t, Py, R};
use super::{cj, usage, Ctx, Plan};
use crate::confirm;
use crate::exit::Exit;
use crate::families::app::py::local_minutes;
use crate::families::voice::sew::{self, Field};
use crate::families::voice::SPACE;
use crate::families::{ljust, py_iter, raised, Shown};
use crate::out::json::{Text, Value};
use crate::out;
use crate::prompt::{self, AtEof, Echo};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use std::time::Duration;

/// The help page: `_hostchat_recording_usage`.
pub const HELP: &str = "hostchat/recording";

/// A recorder verb, or the policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rec {
    Verb {
        sub: String,
        json: bool,
        yes: bool,
        wait: bool,
        rec: String,
        tra: String,
        model: String,
    },
    /// `policy`: the read (`changes` empty), or the change.
    Policy { json: bool, changes: Option<String> },
}

/// `"$PWD/$f"` for a relative `f` (an empty one too).
fn under_pwd(f: &str) -> String {
    if f.starts_with('/') {
        return f.to_string();
    }
    let cwd = crate::daemon::logical_cwd(std::env::var_os("PWD")).unwrap_or_default();
    format!("{}/{f}", cwd.display())
}

/// The words after `recording`.
pub fn parse(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&s, r)) => (s, r),
        None => ("", &[]),
    };
    if matches!(sub, "policy" | "policies") {
        return policy(rest);
    }
    let (mut json, mut yes, mut wait) = (false, false, true);
    let (mut rec, mut tra, mut model) = (String::new(), String::new(), String::new());
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--json" => json = true,
            "--yes" | "-y" => {
                if sub != "uninstall" {
                    return Plan::Bad(usage(&format!(
                        "{w} only applies to {c} hostchat recording uninstall"
                    )));
                }
                yes = true;
            }
            "--no-wait" => wait = false,
            "--recorder-tarball" | "--transcriber-tarball" | "--model-file" => {
                if !matches!(sub, "enable" | "install") {
                    return Plan::Bad(usage(&format!(
                        "{w} only applies to {c} hostchat recording install and enable"
                    )));
                }
                let Some(f) = rest.get(i + 1) else {
                    return Plan::Bad(usage(&format!("{w} FILE")));
                };
                let f = under_pwd(f);
                match w {
                    "--recorder-tarball" => rec = f,
                    "--transcriber-tarball" => tra = f,
                    _ => model = f,
                }
                i += 1;
            }
            "-h" | "--help" => return Plan::Text(HELP),
            w if w.starts_with('-') => {
                return Plan::Bad(usage(&format!(
                    "unknown flag {w} ({c} hostchat recording --help)"
                )))
            }
            w => {
                return Plan::Bad(usage(&format!(
                    "unexpected '{w}' ({c} hostchat recording --help)"
                )))
            }
        }
        i += 1;
    }
    match sub {
        "status" | "preflight" | "install" | "enable" | "disable" | "uninstall" => {
            Plan::Rec(Rec::Verb {
                sub: sub.to_string(),
                json,
                yes,
                wait,
                rec,
                tra,
                model,
            })
        }
        "" | "help" => Plan::Text(HELP),
        _ => Plan::Bad(usage(&format!(
            "unknown recording command '{sub}' ({c} hostchat recording --help)"
        ))),
    }
}

/// The keys `policy set` takes, as `recording.<name>`.
const BOOLS: &[&str] = &[
    "recording.enabled",
    "recording.dm_enabled",
    "recording.require_consent_to_join",
];

/// `_hostchat_recording_policy`'s words, and its `changes` Python.
fn policy(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (mut json, mut set_mode) = (false, false);
    let mut pos: Vec<&str> = Vec::new();
    for &w in words {
        match w {
            "--json" => json = true,
            "-h" | "--help" => return Plan::Text(HELP),
            "set" if !set_mode && pos.is_empty() => set_mode = true,
            w if w.starts_with('-') => {
                return Plan::Bad(usage(&format!(
                    "unknown flag {w} ({c} hostchat recording policy [set KEY VALUE])"
                )))
            }
            w => pos.push(w),
        }
    }
    if !set_mode {
        if !pos.is_empty() {
            return Plan::Bad(usage(&format!(
                "{c} hostchat recording policy [--json] | {c} hostchat recording policy set KEY VALUE"
            )));
        }
        return Plan::Rec(Rec::Policy {
            json,
            changes: None,
        });
    }
    if pos.is_empty() || pos.len() % 2 != 0 {
        return Plan::Bad(usage(&format!("{c} hostchat recording policy set KEY VALUE [KEY VALUE \u{2026}] ({c} hostchat recording --help lists the keys)")));
    }
    let py_bad = |h: String| {
        Plan::Bad(sew::dumps_default(&confirm::object(vec![
            ("error", st("usage")),
            ("hint", st(&h)),
        ])))
    };
    let mut out: Vec<(String, Value)> = Vec::new();
    for pair in pos.chunks(2) {
        let (mut k, v) = (pair[0].to_string(), pair[1]);
        if !k.starts_with("recording.") {
            k = format!("recording.{k}");
        }
        let x = if BOOLS.contains(&k.as_str()) {
            match v {
                "on" | "true" | "yes" => Value::Bool(true),
                "off" | "false" | "no" => Value::Bool(false),
                _ => return py_bad(format!("{k} is on or off")),
            }
        } else if k == "recording.required" {
            if !matches!(v, "off" | "multi_person") {
                return py_bad("recording.required is off or multi_person".into());
            }
            st(v)
        } else if k == "recording.audio_max_days" {
            let d = v.strip_suffix('d').unwrap_or(v);
            let digits = d.trim_start_matches('0');
            let ok = !d.is_empty()
                && d.bytes().all(|b| b.is_ascii_digit())
                && digits.len() <= 4
                && (1..=3650).contains(&digits.parse::<u32>().unwrap_or(0));
            if !ok {
                return py_bad("recording.audio_max_days is a number of days, 1 to 3650".into());
            }
            Value::Int(digits.to_string())
        } else {
            return py_bad(format!("unknown policy {k}: recording.enabled, recording.dm_enabled, recording.require_consent_to_join, recording.required, recording.audio_max_days"));
        };
        match out.iter_mut().find(|(k2, _)| *k2 == k) {
            Some(slot) => slot.1 = x,
            None => out.push((k, x)),
        }
    }
    let changes = Value::Object(
        out.into_iter()
            .map(|(k, v)| (Text::from(k.as_str()), v))
            .collect(),
    );
    let body = confirm::object(vec![("changes", changes)]);
    Plan::Rec(Rec::Policy {
        json,
        changes: Some(sew::dumps_default(&body)),
    })
}

fn show(body: &str, verb: &str, json: bool) -> Exit {
    if json {
        return match out::stdout(&format!("{body}\n")) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    print::print(body, verb).print()
}

pub fn run(ctx: &mut Ctx, r: &Rec) -> Exit {
    let r = match r {
        Rec::Verb {
            sub,
            json,
            yes,
            wait,
            rec,
            tra,
            model,
        } => verb(ctx, sub, *json, *yes, *wait, [rec, tra, model]),
        Rec::Policy { json, changes } => run_policy(ctx, *json, changes.as_deref()),
    };
    match r {
        Ok(e) | Err(e) => e,
    }
}

fn files(f: [&String; 3]) -> [(&'static str, Field); 3] {
    [
        ("recorderTarballPath", Field::S(f[0].clone())),
        ("transcriberTarballPath", Field::S(f[1].clone())),
        ("modelFilePath", Field::S(f[2].clone())),
    ]
}

fn verb(
    ctx: &mut Ctx,
    which: &str,
    json: bool,
    yes: bool,
    wait: bool,
    f: [&String; 3],
) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    match which {
        "status" => {
            let body = ctx.call(false, "/cli/sew/recording", "", false)?;
            Ok(show(&body, "recording-status", json))
        }
        "preflight" => {
            let body = ctx.call(false, "/cli/sew/recording/preflight", "", false)?;
            Ok(show(&body, "recording-preflight", json))
        }
        "install" => {
            let body = ctx.call(true, "/cli/sew/recording/install", &cj(&files(f)), false)?;
            if wait && field(&body, "state").unwrap_or_default() == "installing" {
                return wait_install(ctx, json);
            }
            Ok(show(&body, "recording-install", json))
        }
        "enable" => {
            if ctx.in_session {
                let body = ctx.call(true, "/cli/sew/recording/enable", "{}", false)?;
                return Ok(show(&body, "recording-enable", json));
            }
            let body = ctx.call(false, "/cli/sew/recording", "", false)?;
            let notready = (|| -> R<Option<String>> {
                let v = load(&body, 3)?;
                let r = sub(&v, "ready", 4)?;
                if !matches!(g(&r, "ok", 5)?, Value::Bool(false)) {
                    return Ok(None);
                }
                let o = confirm::object(vec![
                    ("error", or(g(&r, "code", 6)?, st("not_ready"))),
                    ("hint", or(g(&r, "hint", 6)?, st(""))),
                ]);
                Ok(Some(sew::dumps_default(&o)))
            })();
            match notready {
                Ok(None) => {}
                Ok(Some(line)) => {
                    out::stderr_line(&line);
                    return Err(Exit::Refused);
                }
                Err(e) => {
                    let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
                    return Err(Exit::Error);
                }
            }
            let id = install_id(ctx)?;
            let tok = token(
                ctx,
                "sew-recording-enable",
                &id,
                "Install the chat recorder and turn recording on",
            )?;
            let [a, b, d] = files(f);
            let payload = cj(&[("confirmToken", Field::S(tok)), a, b, d]);
            let body = ctx.call(true, "/cli/sew/recording/enable", &payload, true)?;
            if wait && field(&body, "state").unwrap_or_default() == "installing" {
                return wait_install(ctx, json);
            }
            Ok(show(&body, "recording-enable", json))
        }
        "disable" => {
            let body = ctx.call(true, "/cli/sew/recording/disable", "{}", false)?;
            Ok(show(&body, "recording-disable", json))
        }
        _ => {
            if !yes {
                if !prompt::stdin_is_tty() {
                    out::stderr_line(&format!("{{\"error\":\"confirm_required\",\"hint\":\"Remove the recorder? Live recordings stop; every recording is kept. There is no terminal to ask on: pass --yes ({c} hostchat recording uninstall --yes)\"}}"));
                    return Err(Exit::Usage);
                }
                prompt::prompt("Remove the recorder? Live recordings stop; every recording is kept. [y/N] ");
                let ans = prompt::read_line(Echo::Shown, AtEof::Drop);
                if !matches!(ans.expose(), b"y" | b"Y" | b"yes" | b"YES" | b"Yes") {
                    out::stderr_line("Recorder not removed.");
                    return Err(Exit::Error);
                }
            }
            let body = ctx.call(true, "/cli/sew/recording/uninstall", r#"{"confirm": true}"#, false)?;
            Ok(show(&body, "recording-uninstall", json))
        }
    }
}

/// The install's wait: `GET /cli/sew/recording` every 2 s while its
/// progress runs, then its status lines; exit 3 unless it is done.
fn wait_install(ctx: &mut Ctx, json: bool) -> Result<Exit, Exit> {
    let mut res = String::new();
    let mut body = String::new();
    for i in 0..900 {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        body = ctx.call(false, "/cli/sew/recording", "", false)?;
        let r = load(&body, 1)
            .and_then(|v| sub(&v, "progress", 1))
            .and_then(|p| g(&p, "result", 1));
        res = match r {
            Ok(x) => {
                let mut s = sv(&or(x, st("")));
                while s.ends_with('\n') {
                    s.pop();
                }
                s
            }
            Err(e) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
                return Err(Exit::Error);
            }
        };
        if res != "running" {
            break;
        }
    }
    let e = show(&body, "recording-status", json);
    if e != Exit::Ok {
        return Ok(e);
    }
    Ok(if res == "done" { Exit::Ok } else { Exit::Refused })
}

fn run_policy(ctx: &mut Ctx, json: bool, changes: Option<&str>) -> Result<Exit, Exit> {
    let route = "/cli/sew/recording/policy";
    let body = match changes {
        None => ctx.call(false, "/cli/sew/recording/policies", "", false)?,
        Some(changes) => {
            let mut sent = sew::send(&mut ctx.client, true, route, changes, false)?;
            sew::note(&mut ctx.closing, &sent.body, ctx.client.passport().is_some());
            let held = sent.status == "403"
                && [&b"\"vault_confirm_required\""[..], b"\"owner_confirm_required\""]
                    .iter()
                    .any(|w| sent.body.windows(w.len()).any(|x| x == *w))
                && !ctx.in_session;
            if held {
                let id = install_id(ctx)?;
                let tok = token(
                    ctx,
                    "recording-policy",
                    &id,
                    "Change this server's recording policies",
                )?;
                let b = confirm::with_confirm_token(changes, &tok, None).unwrap_or_default();
                drop(tok);
                sent = sew::send(&mut ctx.client, true, route, &b, true)?;
                sew::note(&mut ctx.closing, &sent.body, ctx.client.passport().is_some());
            }
            if sent.status != "200" {
                let (line, exit) = sew::fail_line(&sent.status, &sent.body);
                out::stderr_line(&line);
                return Err(exit);
            }
            String::from_utf8_lossy(&sent.body).into_owned()
        }
    };
    if json {
        return Ok(show(&body, "", true));
    }
    let set = changes.is_some();
    let mut p = Py::default();
    let r = (|| -> R<()> {
        let v = load(&body, 3)?;
        let shown = |x: &Value| match x {
            Value::Null => "-".to_string(),
            Value::Bool(true) => "on".to_string(),
            Value::Bool(false) => "off".to_string(),
            x => sv(x),
        };
        if set {
            let ch = or(g(&v, "changed", 6)?, Value::Array(Vec::new()));
            if t(&ch) {
                p.print(&format!(
                    "saved: {}",
                    crate::out::pylib::ops::join(&ch, ", ", 7)?
                ));
            } else {
                p.print("nothing changed (already so)");
            }
        }
        let f = g(&v, "flags", 8)?;
        if let Value::Object(_) = f {
            let yn = |k: &str| -> R<&'static str> {
                Ok(if t(&g(&f, k, 11)?) { "yes" } else { "no" })
            };
            p.print(&format!(
                "Sew flags: record in Apps {} \u{b7} voice:record cap {} \u{b7} required {} \u{b7} server reads {}",
                yn("appRecording")?,
                yn("voiceRecordCap")?,
                yn("recordingRequired")?,
                yn("mgmtRecordingReads")?
            ));
            if !t(&g(&f, "appRecording", 12)?) {
                p.print(&format!("  This server's chat can't record in Apps yet (it needs Sew 0.9.0: {CLI_NAME} hostchat upgrade)."));
            }
        }
        let row = |a: &str, b: &str, c: &str, d: &str| {
            format!("{} {} {} {d}", ljust(a, 36), ljust(b, 14), ljust(c, 14))
        };
        p.print(&row("policy", &format!("{PRODUCT_NAME} asked"), "Sew shows", ""));
        for pol in py_iter(&or(g(&v, "policies", 15)?, Value::Array(Vec::new())), 15)? {
            let want = g(&pol, "want", 17)?;
            let mut note = String::new();
            if !matches!(want, Value::Null) {
                note = if t(&g(&pol, "delivered", 18)?) {
                    "delivered".into()
                } else {
                    "not yet delivered".into()
                };
                if t(&g(&pol, "setBy", 19)?) {
                    note.push_str(&format!(" (by {})", sv(&print::ix(&pol, "setBy", 20)?)));
                }
            }
            p.print(&row(
                &sv(&g(&pol, "key", 21)?),
                &shown(&want),
                &shown(&g(&pol, "seen", 21)?),
                &note,
            ));
        }
        let locks = g(&v, "locks", 22)?;
        if let Value::Array(items) = &locks {
            if items.is_empty() {
                p.print("locked Spaces: none");
            }
            for l in items {
                let at = or(g(l, "lockedAt", 27)?, Value::Int("0".into()));
                p.print(&format!(
                    "locked Space {} (no recording, CUI/FCI) by {}, {}",
                    sv(&g(l, "spaceId", 27)?),
                    sv(&g(l, "lockedBy", 27)?),
                    local_minutes(&at, 27)?
                ));
            }
        }
        if t(&g(&v, "k5SpaceId", 28)?) {
            p.print(&format!(
                "{SPACE} rooms Space: {} ({CLI_NAME} voice settings)",
                sv(&print::ix(&v, "k5SpaceId", 29)?)
            ));
        }
        Ok(())
    })();
    Ok(match r {
        Ok(()) => Shown::out(p.out).print(),
        Err(e) => raised(p.out, p.err, &e).print(),
    })
}
