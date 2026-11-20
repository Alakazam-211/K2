//! `cmd_hostchat`'s own verbs, after the flags: status, preflight,
//! enable (and its wait), disable, rollback, doctor, reconfigure, restart,
//! adopt, backup, backups, job, upgrade (and its wait), restore,
//! rotate-master, recovery-key, config, uninstall, app-url, ship, mirror,
//! open --print and purge.

use super::print::{self, g, is, load, or, st, sub, sv, t, Py, R};
use super::{cj, usage, Ctx, Main};
use crate::confirm::{self, Confirm};
use crate::exit::Exit;
use crate::families::voice::sew::{self, Field};
use crate::families::{chat, raised, Raise, Shown};
use crate::out::json::Value;
use crate::out::{self, py};
use crate::transport::encode::{quote, quote_bytes};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use std::time::Duration;

fn s(t: &str) -> Field {
    Field::S(t.to_string())
}

fn b(on: bool) -> Field {
    Field::B(if on { "1" } else { "0" }.to_string())
}

/// A usage line on stderr, exit 2.
fn bad(hint: &str) -> Exit {
    out::stderr_line(&usage(hint));
    Exit::Usage
}

/// A fixed line on stderr and its exit.
fn line(text: &str, exit: Exit) -> Exit {
    out::stderr_line(text);
    exit
}

/// The shell's `abs`: a relative path under `$PWD`.
pub(super) fn abs(p: &str) -> String {
    if p.is_empty() || p.starts_with('/') {
        return p.to_string();
    }
    let cwd = crate::daemon::logical_cwd(std::env::var_os("PWD")).unwrap_or_default();
    format!("{}/{p}", cwd.display())
}

/// `_hostchat_print <verb> <as_json>` on the body.
pub(super) fn show(body: &str, verb: &str, json: bool) -> Exit {
    if json {
        return match out::stdout(&format!("{body}\n")) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    print::print(body, verb).print()
}

/// `$(_hostchat_field <k>)`: a top-level string field ("" otherwise), or
/// where its Python raised (the traceback is printed on the way).
pub(super) fn field(body: &str, k: &str) -> Result<String, Exit> {
    let r = load(body, 1).and_then(|v| g(&v, k, 1));
    match r {
        Ok(Value::Str(t)) => {
            let mut v = py::str_of(&Value::Str(t));
            while v.ends_with('\n') {
                v.pop();
            }
            Ok(v)
        }
        Ok(_) => Ok(String::new()),
        Err(e) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
            Err(Exit::Error)
        }
    }
}

/// `_hostchat_person`: a session refuses a person's step.
fn person(ctx: &Ctx) -> Result<(), Exit> {
    if ctx.in_session {
        out::stderr_line(&format!("{{\"error\":\"person_only\",\"hint\":\"a person does this from a terminal outside {PRODUCT_NAME} (Touch ID or the vault password), never an agent\"}}"));
        return Err(Exit::Refused);
    }
    Ok(())
}

/// `id="$(_hostchat_install_id)"`: the install a person's confirm is
/// bound to (`none` when there is none).
pub(super) fn install_id(ctx: &mut Ctx) -> Result<String, Exit> {
    let body = ctx.call(false, "/cli/sew/status", "", false)?;
    let r = load(&body, 1).and_then(|v| g(&v, "installId", 1));
    match r {
        Ok(v) => {
            let mut id = sv(&or(v, st("none")));
            while id.ends_with('\n') {
                id.pop();
            }
            Ok(id)
        }
        Err(e) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
            Err(Exit::Error)
        }
    }
}

/// `tok="$(_vault_confirm_token <action> <id> <label>)" || exit 3`.
pub(super) fn token(ctx: &mut Ctx, action: &str, id: &str, label: &str) -> Result<String, Exit> {
    let passport = ctx.client.passport().is_some();
    let mut c = Confirm {
        client: &mut ctx.client,
        sock: &ctx.sock,
        closing: &mut ctx.closing,
        passport,
    };
    confirm::confirm_token(&mut c, action, id, label).ok_or(Exit::Refused)
}

/// Each verb, as `cmd_hostchat`'s `case "$sub"` answers it.
pub fn run(ctx: &mut Ctx, m: &Main) -> Exit {
    match verb(ctx, m) {
        Ok(e) | Err(e) => e,
    }
}

fn verb(ctx: &mut Ctx, m: &Main) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    let json = m.json;
    let pos = |n: usize| m.pos.get(n).map_or("", String::as_str);
    let post0 = |ctx: &mut Ctx, route: &str, verb: &str| -> Result<Exit, Exit> {
        let body = ctx.call(true, route, "{}", false)?;
        Ok(show(&body, verb, json))
    };
    match m.sub.as_str() {
        "status" => {
            let q = if m.health { "health=1" } else { "" };
            let body = ctx.call(false, "/cli/sew/status", q, false)?;
            Ok(show(&body, "status", json))
        }
        "preflight" => {
            let q = if m.tarball.is_empty() {
                String::new()
            } else {
                format!("tarballPath={}", quote_bytes(abs(&m.tarball).as_bytes()))
            };
            let body = ctx.call(false, "/cli/sew/preflight", &q, false)?;
            Ok(show(&body, "preflight", json))
        }
        "enable" => {
            if m.identity == "standalone" && !json {
                out::stderr_line("Identity: standalone (Sew's own password logins). This is fixed for this install; switching later means a fresh install.");
            }
            let vj = if m.voice {
                cj(&[
                    ("publicUrl", s(&m.voice_url)),
                    ("nodeIp", s(&m.node_ip)),
                    ("mediaTarballPath", s(&abs(&m.media_tarball))),
                ])
            } else {
                String::new()
            };
            let payload = cj(&[
                ("tarballPath", s(&abs(&m.tarball))),
                ("recoveryKeyFile", s(&abs(&m.recovery))),
                ("identity", s(&m.identity)),
                ("voice", Field::J(vj)),
            ]);
            let body = ctx.call(true, "/cli/sew/server/enable", &payload, false)?;
            if m.wait && field(&body, "state").unwrap_or_default() == "installing" {
                return Ok(wait_enable(ctx, m.voice, json));
            }
            Ok(show(&body, "enable", json))
        }
        "disable" => post0(ctx, "/cli/sew/server/disable", "disable"),
        "rollback" => {
            if m.accept_loss {
                return rollback_data_loss(ctx, &abs(&m.media_tarball), json);
            }
            let payload = cj(&[("mediaTarballPath", s(&abs(&m.media_tarball)))]);
            let body = ctx.call(true, "/cli/sew/server/rollback", &payload, false)?;
            let e = show(&body, "rollback", json);
            if e != Exit::Ok {
                return Ok(e);
            }
            Ok(rollback_check(&body))
        }
        "doctor" => post0(ctx, "/cli/sew/server/doctor", "doctor"),
        "reconfigure" => post0(ctx, "/cli/sew/server/reconfigure", "reconfigure"),
        "restart" => post0(ctx, "/cli/sew/server/restart", "restart"),
        "adopt" => post0(ctx, "/cli/sew/server/adopt", "adopt"),
        "backup" => {
            ctx.client.compute_post_max_time = Some(Duration::from_secs(1900));
            let body = ctx.call(true, "/cli/sew/server/backup", "{}", false);
            ctx.client.compute_post_max_time = None;
            Ok(show(&body?, "backup", json))
        }
        "backups" => {
            let body = ctx.call(false, "/cli/sew/backups", "", false)?;
            Ok(show(&body, "backups", json))
        }
        "job" => {
            if pos(0).is_empty() {
                return Ok(bad(&format!("{c} hostchat job <id>")));
            }
            let q = format!("id={}", quote(pos(0)));
            let body = ctx.call(false, "/cli/sew/job", &q, false)?;
            Ok(show(&body, "job", json))
        }
        "upgrade" => upgrade(ctx, m),
        "restore" => {
            if pos(0).is_empty() {
                return Ok(bad(&format!("{c} hostchat restore <bundle-id> --confirm")));
            }
            let payload = cj(&[("bundleId", s(pos(0))), ("confirm", b(m.confirm))]);
            let body = ctx.call(true, "/cli/sew/server/restore", &payload, false)?;
            Ok(show(&body, "restore", json))
        }
        "rotate-master" => {
            let body = if m.drop_prev {
                person(ctx)?;
                let id = install_id(ctx)?;
                let tok = token(ctx, "sew-drop-prev", &id, "Drop chat's old master key")?;
                let payload = cj(&[("phase", s("drop-prev")), ("confirmToken", s(&tok))]);
                ctx.call(true, "/cli/sew/server/rotate-master", &payload, true)?
            } else {
                let payload = cj(&[("phase", s("start"))]);
                ctx.call(true, "/cli/sew/server/rotate-master", &payload, false)?
            };
            Ok(show(&body, "rotate-master", json))
        }
        "recovery-key" => {
            person(ctx)?;
            let id = install_id(ctx)?;
            let tok = token(ctx, "sew-recovery-key", &id, "Show the chat recovery key")?;
            let payload = cj(&[("confirmToken", s(&tok))]);
            let body = ctx.call(true, "/cli/sew/server/recovery-key", &payload, true)?;
            Ok(show(&body, "recovery-key", json))
        }
        "config" => config(ctx, m),
        "uninstall" => {
            let payload = cj(&[("dropGrants", b(m.drop_grants))]);
            let body = ctx.call(true, "/cli/sew/server/uninstall", &payload, false)?;
            Ok(show(&body, "uninstall", json))
        }
        "app-url" => app_url(ctx, m),
        "ship" | "mirror" => {
            let sub = m.sub.as_str();
            let route = format!("/cli/sew/server/{sub}");
            let kinds = if sub == "ship" {
                "dir <path> | box-backup | none"
            } else {
                "dir <path> | none"
            };
            let payload = match pos(0) {
                "none" => r#"{"kind":"none"}"#.to_string(),
                "dir" => {
                    if pos(1).is_empty() {
                        return Ok(bad(&format!(
                            "{c} hostchat {sub} dir <path> (a folder on another disk)"
                        )));
                    }
                    cj(&[("kind", s("dir")), ("path", s(&abs(pos(1))))])
                }
                "box-backup" => {
                    if sub != "ship" {
                        return Ok(bad(&format!("box-backup is for {c} hostchat ship; the keyring mirror is {c} hostchat mirror dir <path> | none")));
                    }
                    r#"{"kind":"box-backup"}"#.to_string()
                }
                _ => return Ok(bad(&format!("{c} hostchat {sub} {kinds}"))),
            };
            let body = ctx.call(true, &route, &payload, false)?;
            Ok(show(&body, sub, json))
        }
        "open" => open_print(ctx),
        "purge" => {
            if m.host.is_empty() {
                return Ok(bad(&format!(
                    "{c} hostchat purge --confirm-hostname HOST ({c} hostchat status shows it)"
                )));
            }
            person(ctx)?;
            let id = install_id(ctx)?;
            let tok = token(
                ctx,
                "sew-purge",
                &id,
                "Delete all chat and voice data on this box",
            )?;
            let payload = cj(&[
                ("purge", b(true)),
                ("confirmHostname", s(&m.host)),
                ("keepBackups", b(m.keep_backups)),
                ("confirmToken", s(&tok)),
            ]);
            let body = ctx.call(true, "/cli/sew/server/uninstall", &payload, true)?;
            Ok(show(&body, "purge", json))
        }
        other => unreachable!("parse sends no other verb here: {other}"),
    }
}

/// `rollback`'s check after the lines: an answer with `ok` false exits 3
/// with its code (and Sew's command, when there is one).
fn rollback_check(body: &str) -> Exit {
    let r = (|| -> R<Option<Value>> {
        let v = load(body, 3)?;
        if !matches!(g(&v, "ok", 4)?, Value::Bool(false)) {
            return Ok(None);
        }
        let mut pairs = vec![
            ("error", or(g(&v, "code", 5)?, st("rollback_incomplete"))),
            ("hint", or(g(&v, "error", 5)?, st("see the answer above"))),
        ];
        if t(&g(&v, "mediaCommand", 6)?) {
            pairs.push(("command", print::ix(&v, "mediaCommand", 7)?));
        }
        Ok(Some(confirm::object(pairs)))
    })();
    match r {
        Ok(None) => Exit::Ok,
        Ok(Some(v)) => line(&sew::dumps_default(&v), Exit::Refused),
        Err(e) => raised(String::new(), String::new(), &e).print(),
    }
}

fn upgrade(ctx: &mut Ctx, m: &Main) -> Result<Exit, Exit> {
    let json = m.json;
    let tarball = abs(&m.tarball);
    let media = abs(&m.media_tarball);
    let dry = b(m.dry);
    let body = if m.accept_binary.is_empty() {
        let payload = cj(&[
            ("tarballPath", s(&tarball)),
            ("mediaTarballPath", s(&media)),
            ("dryRun", dry),
        ]);
        ctx.call(true, "/cli/sew/server/upgrade", &payload, false)?
    } else {
        let c = CLI_NAME;
        // `printf '%s' … | tr 'A-F' 'a-f'` in `$(…)`.
        let mut sha: String = m
            .accept_binary
            .chars()
            .map(|ch| {
                if ch.is_ascii_uppercase() && ch <= 'F' {
                    ch.to_ascii_lowercase()
                } else {
                    ch
                }
            })
            .collect();
        while sha.ends_with('\n') {
            sha.pop();
        }
        if sha.chars().any(|ch| !matches!(ch, '0'..='9' | 'a'..='f')) {
            return Ok(bad(&format!("--accept-binary is the binary's sha256: 64 hex characters ({c} hostchat upgrade names it when it refuses)")));
        }
        if sha.chars().count() != 64 {
            return Ok(bad(
                "--accept-binary is the binary's sha256: 64 hex characters",
            ));
        }
        person(ctx)?;
        let id = install_id(ctx)?;
        let tok = token(
            ctx,
            "sew-accept-binary",
            &format!("{id}:{sha}"),
            &format!("Update chat from a build {PRODUCT_NAME} doesn't know (sha256 {sha})"),
        )?;
        let payload = cj(&[
            ("tarballPath", s(&tarball)),
            ("mediaTarballPath", s(&media)),
            ("dryRun", dry),
            ("acceptBinary", s(&sha)),
            ("confirmToken", s(&tok)),
        ]);
        ctx.call(true, "/cli/sew/server/upgrade", &payload, true)?
    };
    if m.wait && field(&body, "state").unwrap_or_default() == "upgrading" {
        return Ok(wait_upgrade(ctx, json));
    }
    Ok(show(&body, "upgrade", json))
}

fn config(ctx: &mut Ctx, m: &Main) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    let json = m.json;
    let pos = |n: usize| m.pos.get(n).map_or("", String::as_str);
    let mut public_url = m.public_url.clone();
    if pos(0) == "set" {
        let policy = |ctx: &mut Ctx, payload: String| -> Result<Exit, Exit> {
            let body = chat::call(
                &mut ctx.client,
                &mut ctx.closing,
                true,
                "/cli/sew/approvals/policy/set",
                &payload,
                false,
            )?;
            let text = String::from_utf8_lossy(&body).into_owned();
            Ok(if json {
                show(&text, "config", true)
            } else {
                chat::print(&text, "config").print()
            })
        };
        match pos(1) {
            "approvals.by" | "approvals_by" => {
                if !matches!(
                    pos(2),
                    "people" | "people+agents" | "person_only" | "agents_allowed"
                ) {
                    return Ok(bad(&format!(
                        "{c} hostchat config set approvals.by people|people+agents"
                    )));
                }
                return policy(ctx, cj(&[("approvalsBy", s(pos(2)))]));
            }
            "default_space" | "default-space" | "chat.default_space" => {
                if pos(2).is_empty() {
                    return Ok(bad(&format!(
                        "{c} hostchat config set default_space <space-slug>|none"
                    )));
                }
                return policy(ctx, cj(&[("defaultSpace", s(pos(2)))]));
            }
            "server.public_url" | "public_url" => {
                if pos(2).is_empty() {
                    return Ok(bad(&format!(
                        "{c} hostchat config set server.public_url https://HOST"
                    )));
                }
                public_url = pos(2).to_string();
            }
            _ => {
                return Ok(bad(&format!("settable keys: approvals.by people|people+agents, default_space <space-slug>|none, server.public_url https://HOST ({c} hostchat config lists them)")));
            }
        }
    }
    if public_url.is_empty() {
        if !pos(0).is_empty() {
            return Ok(bad(&format!(
                "{c} hostchat config [set <key> <value>] | config --public-url https://HOST"
            )));
        }
        let body = chat::call(
            &mut ctx.client,
            &mut ctx.closing,
            false,
            "/cli/sew/approvals/policy",
            "",
            false,
        )?;
        let text = String::from_utf8_lossy(&body).into_owned();
        if json {
            return Ok(show(&text, "config", true));
        }
        let mut p = Py::default();
        let r = (|| -> R<()> {
            let v = load(&text, 3)?;
            let row = |a: &str, b: &str, c: &str| {
                format!(
                    "{} {} {}",
                    crate::families::ljust(a, 20),
                    crate::families::ljust(b, 14),
                    c
                )
            };
            p.print(&row("key", "value", "allowed"));
            let by = sv(&g(&v, "approvalsBy", 5)?);
            let allowed = or(
                g(&v, "allowed", 5)?,
                Value::Array(vec![st("people"), st("people+agents")]),
            );
            let allowed = crate::out::pylib::ops::join(&allowed, "|", 5)?;
            p.print(&row("approvals.by", &by, &allowed));
            let ds = sv(&or(g(&v, "defaultSpace", 6)?, st("(none)")));
            p.print(&row("default_space", &ds, "a Space slug: its access governs DMs outside every Space; none clears (fixed to k5-admin while voice rooms are on)"));
            p.print(&row(
                "server.public_url",
                "(see status)",
                &format!("https://HOST on this box ({c} hostchat config --public-url)"),
            ));
            Ok(())
        })();
        return Ok(match r {
            Ok(()) => Shown::out(p.out).print(),
            Err(e) => raised(p.out, p.err, &e).print(),
        });
    }
    let payload = cj(&[("publicUrl", s(&public_url))]);
    let body = ctx.call(true, "/cli/sew/server/config", &payload, false)?;
    Ok(show(&body, "config", json))
}

fn app_url(ctx: &mut Ctx, m: &Main) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    let json = m.json;
    let pos = |n: usize| m.pos.get(n).map_or("", String::as_str);
    match m.pos.first().map_or("show", String::as_str) {
        "show" => {
            let body = ctx.call(false, "/cli/sew/status", "", false)?;
            if json {
                let r = load(&body, 1).and_then(|v| g(&v, "appUrl", 1));
                return Ok(match r {
                    Ok(u) => {
                        let o = confirm::object(vec![("appUrl", u)]);
                        Shown::out(format!("{}\n", sew::dumps_default(&o))).print()
                    }
                    Err(e) => raised(String::new(), String::new(), &e).print(),
                });
            }
            let u = field(&body, "appUrl")?;
            let u = if u.is_empty() { "(not set)".into() } else { u };
            Ok(Shown::out(format!("{u}\n")).print())
        }
        "set" => {
            if pos(1).is_empty() {
                return Ok(bad(&format!(
                    "{c} hostchat app-url set https://HOST[/path]"
                )));
            }
            let payload = cj(&[("url", s(pos(1)))]);
            let body = ctx.call(true, "/cli/sew/server/app-url", &payload, false)?;
            Ok(show(&body, "app-url", json))
        }
        "clear" => {
            let body = ctx.call(true, "/cli/sew/server/app-url", r#"{"url":""}"#, false)?;
            Ok(show(&body, "app-url", json))
        }
        _ => Ok(bad(&format!(
            "{c} hostchat app-url set <url> | clear | show"
        ))),
    }
}

/// `open --print`: the owner's one-use link into chat, on stdout.
fn open_print(ctx: &mut Ctx) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    person(ctx)?;
    let body = ctx.call(false, "/cli/sew/status", "", false)?;
    let app_url = field(&body, "appUrl")?;
    if app_url.is_empty() {
        return Ok(line(&format!("{{\"error\":\"app_url_unset\",\"hint\":\"set where the Sew web app lives first: {c} hostchat app-url set https://HOST\"}}"), Exit::Refused));
    }
    let tok = token(ctx, "chat-owner-handoff", "", "Open chat as its owner")?;
    let payload = cj(&[("confirmToken", s(&tok))]);
    drop(tok);
    let body = ctx.call(true, "/cli/sew/owner-handoff", &payload, true)?;
    let link = (|| -> R<Option<String>> {
        let v = load(&body, 3)?;
        let code = or(g(&v, "code", 3)?, st(""));
        let Value::Str(_) = code else {
            return Err(Raise::no_attribute(4, &code, "startswith"));
        };
        let code = sv(&code);
        if !code.starts_with("k2sho_") {
            return Ok(None);
        }
        Ok(Some(format!(
            "{}/k2/owner-handoff#code={code}",
            app_url.trim_end_matches('/')
        )))
    })();
    let link = match link {
        Ok(Some(l)) => l,
        Ok(None) => {
            return Ok(line(
                &format!("{{\"error\":\"no_code\",\"hint\":\"{PRODUCT_NAME} did not return a handoff code\"}}"),
                Exit::Refused,
            ))
        }
        Err(e) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
            return Ok(line(
                &format!("{{\"error\":\"no_code\",\"hint\":\"{PRODUCT_NAME} did not return a handoff code\"}}"),
                Exit::Refused,
            ));
        }
    };
    let mut link = link;
    while link.ends_with('\n') {
        link.pop();
    }
    Ok(Shown::out(format!("{link}\n")).print())
}

/// `_hostchat_rollback_data_loss <media> <as_json>`.
fn rollback_data_loss(ctx: &mut Ctx, media: &str, json: bool) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    let route = "/cli/sew/server/rollback";
    if ctx.in_session {
        ctx.call(true, route, &cj(&[("acceptDataLoss", b(true))]), false)?;
        return Ok(Exit::Ok);
    }
    let plan = ctx.call(
        true,
        route,
        &cj(&[("acceptDataLoss", b(true)), ("plan", b(true))]),
        false,
    )?;
    let get = |k: &str| -> Result<String, Exit> {
        match load(&plan, 1).and_then(|v| g(&v, k, 1)) {
            Ok(x) => {
                let mut x = sv(&or(x, st("")));
                while x.ends_with('\n') {
                    x.pop();
                }
                Ok(x)
            }
            Err(e) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
                Err(Exit::Error)
            }
        }
    };
    let bundle = get("bundle")?;
    let to = get("to")?;
    let from = get("from")?;
    let since = get("since")?;
    if bundle.is_empty() {
        return Ok(line(&format!("{{\"error\":\"no_rollback_point\",\"hint\":\"{PRODUCT_NAME} found no backup to roll back to ({c} hostchat backups)\"}}"), Exit::Refused));
    }
    let lines = [
        "Nothing has changed yet. Confirming will:".to_string(),
        format!("  roll this server's chat (Sew) back from {from} to {to}, for everyone on it."),
        format!("  Chat written since {since} will be lost from the live chat; a backup of it is kept."),
        format!("  {PRODUCT_NAME} first backs up the chat on Sew {from}, then rolls back to backup {bundle} (taken just before the upgrade)."),
        "  Chat is offline for a few minutes while it runs.".to_string(),
        format!("Backups are listed by {c} hostchat backups. To bring the newer chat back later:"),
        format!("  {c} hostchat upgrade, then {c} hostchat restore <kept backup> --confirm."),
    ];
    for l in &lines {
        out::stderr_line(l);
    }
    let id = install_id(ctx)?;
    let tok = token(
        ctx,
        "sew-rollback-data-loss",
        &format!("{id}:{bundle}"),
        &format!("Roll chat back to {to}: chat written since {since} leaves the live chat (a backup of it is kept)"),
    )?;
    let payload = cj(&[
        ("acceptDataLoss", b(true)),
        ("bundleId", s(&bundle)),
        ("confirmToken", s(&tok)),
        ("mediaTarballPath", s(media)),
    ]);
    drop(tok);
    ctx.client.compute_post_max_time = Some(Duration::from_secs(3600));
    let body = ctx.call(true, route, &payload, true);
    ctx.client.compute_post_max_time = None;
    let body = body?;
    if json {
        if let Err(e) = out::stdout(&format!("{body}\n")) {
            return Ok(out::after_stdout_error(&e));
        }
    }
    let mut p = Py::default();
    let r = (|| -> R<bool> {
        let v = load(&body, 3)?;
        let r = g(&v, "result", 4)?;
        if !json {
            let head = or(
                or(g(&v, "message", 6)?, r.clone()),
                st(&sew::dumps_default(&v)),
            );
            p.print(&sv(&head));
            for (k, lbl) in [
                ("keptBackup", "kept backup"),
                ("bundle", "rolled back to"),
                ("version", "Sew now"),
            ] {
                if t(&g(&v, k, 8)?) {
                    p.print(&format!(
                        "{} {}",
                        crate::families::ljust(&format!("{lbl}:"), 16),
                        sv(&print::ix(&v, k, 9)?)
                    ));
                }
            }
            if t(&g(&v, "doctorFails", 10)?) {
                let fails =
                    crate::out::pylib::ops::join(&print::ix(&v, "doctorFails", 11)?, ", ", 11)?;
                p.print(&format!("doctor fails:    {fails}"));
            }
        }
        if matches!(r, Value::Array(_) | Value::Object(_)) {
            return Err(Raise::at(
                13,
                format!("TypeError: unhashable type: '{}'", py::type_name(&r)),
            ));
        }
        if is(&r, "rolled_back") {
            return Ok(true);
        }
        let err = confirm::object(vec![
            ("error", or(r, st("rollback_incomplete"))),
            (
                "hint",
                or(g(&v, "message", 14)?, st("see the answer above")),
            ),
        ]);
        p.eprint(&sew::dumps_default(&err));
        Ok(false)
    })();
    Ok(match r {
        Ok(true) => Shown::out(p.out).print(),
        Ok(false) => Shown {
            stdout: p.out,
            stderr: p.err,
            exit: Exit::Refused,
        }
        .print(),
        Err(e) => raised(p.out, p.err, &e).print(),
    })
}

/// `"%.1f" % (n / 1e6)`.
fn mb(n: &Value, line: u32) -> R<String> {
    let f = match n {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::INFINITY),
        Value::Float(f) => *f,
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: unsupported operand type(s) for /: '{}' and 'float'",
                    py::type_name(other)
                ),
            ))
        }
    };
    Ok(format!("{:.1}", f / 1e6))
}

/// The wait's Python: `busy <state>/<step>` or `done <state>/<step>`.
fn enable_now(body: &str, voice: bool) -> R<String> {
    let v = load(body, 3)?;
    let st_ = g(&v, "state", 4)?;
    let vo = sub(&v, "voice", 5)?;
    let p = sub(&vo, "progress", 6)?;
    let installing = is(&st_, "installing");
    let mut busy = installing || (voice && is(&g(&p, "result", 7)?, "running"));
    let ep = sub(&v, "enableProgress", 8)?;
    let mut step = if installing {
        g(&ep, "current", 8)?
    } else {
        g(&p, "step", 8)?
    };
    let from = if installing { ep } else { p };
    let dl = or(g(&from, "download", 10)?, print::dict());
    if is(&step, "stage")
        && is(&g(&dl, "source", 11)?, "download")
        && !matches!(g(&dl, "bytes", 11)?, Value::Null)
    {
        let label = or(g(&dl, "label", 13)?, st("Sew"));
        let total = g(&dl, "total", 13)?;
        let of = if t(&total) {
            format!(" of {}", mb(&print::ix(&dl, "total", 13)?, 13)?)
        } else {
            String::new()
        };
        step = st(&format!(
            "stage (downloading {}: {}{} MB)",
            sv(&label),
            mb(&print::ix(&dl, "bytes", 13)?, 13)?,
            of
        ));
    } else if is(&step, "stage") && is(&g(&dl, "source", 14)?, "cache") {
        let label = or(g(&dl, "label", 15)?, st("Sew"));
        step = st(&format!(
            "stage ({} from the {PRODUCT_NAME} download cache)",
            sv(&label)
        ));
    }
    let h = sub(&v, "apiHealth", 17)?;
    if !busy && is(&g(&h, "reason", 18)?, "upgrading") {
        busy = true;
        step = st("sew-upgrading");
    }
    Ok(format!(
        "{}{}/{}",
        if busy { "busy " } else { "done " },
        sv(&st_),
        sv(&or(step, st("-")))
    ))
}

/// The end of the enable wait: exit 3 when the install or voice failed,
/// or Sew is stopping.
fn enable_end(body: &str) -> Exit {
    let mut p = Py::default();
    let r = (|| -> R<bool> {
        let v = load(body, 3)?;
        let vo = sub(&v, "voice", 4)?;
        let h = sub(&v, "apiHealth", 5)?;
        let mut bad = is(&g(&v, "state", 6)?, "error")
            || is(&g(&sub(&vo, "progress", 6)?, "result", 6)?, "error");
        if is(&g(&h, "reason", 7)?, "stopping") {
            p.eprint(&format!("Sew is stopping (a stop, disable or restart is under way); not waiting. Check {CLI_NAME} hostchat status, or {CLI_NAME} hostchat restart."));
            bad = true;
        }
        Ok(bad)
    })();
    match r {
        Ok(bad) => Shown {
            stdout: p.out,
            stderr: p.err,
            exit: if bad { Exit::Refused } else { Exit::Ok },
        }
        .print(),
        Err(e) => raised(p.out, p.err, &e).print(),
    }
}

/// The text after the first `prefix`, as `${now#prefix}`.
fn after<'a>(s: &'a str, prefix: &str) -> &'a str {
    s.strip_prefix(prefix).unwrap_or(s)
}

/// `_hostchat_wait <voice> <as_json>`.
pub(super) fn wait_enable(ctx: &mut Ctx, voice: bool, json: bool) -> Exit {
    let mut last = String::new();
    for _ in 0..600 {
        let body = match ctx.call(false, "/cli/sew/status", "", false) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let now = match enable_now(&body, voice) {
            Ok(n) => n,
            Err(e) => return raised(String::new(), String::new(), &e).print(),
        };
        if now.starts_with("done") {
            if !json {
                // `echo "${now#done }" | sed 's#/-$##'`
                let shown = after(&now, "done ");
                let shown = shown.strip_suffix("/-").unwrap_or(shown);
                out::stderr_line(shown);
            }
            let e = show(&body, "status", json);
            if e != Exit::Ok {
                return e;
            }
            return enable_end(&body);
        }
        if now != last && !json {
            out::stderr_line(&format!("\u{2026} {}", after(&now, "busy ")));
            last = now;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    line(
        &format!("{{\"error\":\"timeout\",\"hint\":\"still installing after 20 minutes: {CLI_NAME} hostchat status\"}}"),
        Exit::Refused,
    )
}

/// One step's sort key in the upgrade wait: `(seq or 0, at or 0, its
/// place in the machine's order)`.
#[derive(Clone, PartialEq)]
enum Key {
    Num(f64),
    Str(String),
}

/// Python's `<`: numbers with numbers, strings with strings; anything
/// else raises (`None` here).
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Key) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Key::Num(a), Key::Num(b)) => a.partial_cmp(b),
            (Key::Str(a), Key::Str(b)) => {
                let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
                Some(a.cmp(&b))
            }
            _ => None,
        }
    }
}

fn key_of(v: &Value) -> Result<Key, ()> {
    match v {
        _ if !t(v) => Ok(Key::Num(0.0)),
        Value::Bool(b) => Ok(Key::Num(f64::from(u8::from(*b)))),
        Value::Int(d) => Ok(Key::Num(d.parse().unwrap_or(f64::INFINITY))),
        Value::Float(f) => Ok(Key::Num(*f)),
        Value::Str(_) => Ok(Key::Str(sv(v))),
        _ => Err(()),
    }
}

/// The upgrade wait's two Pythons: `busy|done <current>` and the steps
/// done so far, in the order they ran.
fn upgrade_now(body: &str) -> R<(String, Vec<String>)> {
    let v = load(body, 3)?;
    let p = sub(&v, "upgradeProgress", 4)?;
    let busy = is(&g(&v, "state", 5)?, "upgrading")
        || (matches!(g(&p, "result", 5)?, Value::Null)
            && !matches!(g(&p, "current", 5)?, Value::Null));
    let now = format!(
        "{}{}",
        if busy { "busy " } else { "done " },
        sv(&or(g(&p, "current", 6)?, st("-")))
    );
    let steps = or(g(&p, "steps", 4)?, print::dict());
    let order = [
        "stage",
        "upgrade",
        "health",
        "rollback",
        "media",
        "tunnel-bind",
    ];
    let mut keyed: Vec<((Key, Key, usize), String)> = Vec::new();
    let pairs: Vec<(String, Value)> = match &steps {
        Value::Object(pairs) => pairs
            .iter()
            .map(|(k, x)| (sv(&Value::Str(k.clone())), x.clone()))
            .collect(),
        other => return Err(Raise::no_attribute(11, other, "items")),
    };
    for (k, x) in pairs {
        let x = or(x, print::dict());
        let seq = g(&x, "seq", 9)?;
        let at = g(&x, "at", 9)?;
        let ty = || Raise::at(11, "TypeError: '<' not supported between instances");
        let seq = key_of(&seq).map_err(|_| ty())?;
        let at = key_of(&at).map_err(|_| ty())?;
        let place = order.iter().position(|o| *o == k).unwrap_or(order.len());
        keyed.push(((seq, at, place), k));
    }
    // Python's sort compares the tuples; a number against a string raises.
    let mut err = false;
    keyed.sort_by(|a, b| {
        a.0.partial_cmp(&b.0).unwrap_or_else(|| {
            err = true;
            std::cmp::Ordering::Equal
        })
    });
    if err {
        return Err(Raise::at(
            11,
            "TypeError: '<' not supported between instances of 'str' and 'int'",
        ));
    }
    let line = keyed
        .into_iter()
        .map(|(_, k)| k)
        .collect::<Vec<_>>()
        .join(" ");
    // `for s in $steps`: split on blanks.
    let steps = line
        .split([' ', '\t', '\n'])
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    Ok((now, steps))
}

/// The end of the upgrade wait: `upgraded: <from> -> <to>`, or the
/// reason it stopped, exit 3.
fn upgrade_end(body: &str, json: bool) -> Exit {
    let mut p = Py::default();
    let r = (|| -> R<bool> {
        let v = load(body, 3)?;
        let pr = sub(&v, "upgradeProgress", 4)?;
        let res = g(&pr, "result", 5)?;
        let mut why = or(g(&pr, "lastError", 6)?, g(&v, "lastError", 6)?);
        if is(&res, "done") {
            if !json {
                p.print(&format!(
                    "upgraded: {} -> {}",
                    sv(&g(&pr, "from", 10)?),
                    sv(&g(&pr, "to", 10)?)
                ));
            }
            return Ok(true);
        }
        if !t(&why) {
            why = st(&format!(
                "the daemon recorded no reason (a {PRODUCT_NAME} bug: send the k2-daemon log)"
            ));
        }
        let o = confirm::object(vec![
            ("error", st(&format!("upgrade_{}", sv(&res)))),
            ("result", res),
            ("from", g(&pr, "from", 15)?),
            ("to", g(&pr, "to", 15)?),
            ("hint", why),
        ]);
        p.eprint(&sew::dumps_default(&o));
        Ok(false)
    })();
    match r {
        Ok(ok) => Shown {
            stdout: p.out,
            stderr: p.err,
            exit: if ok { Exit::Ok } else { Exit::Refused },
        }
        .print(),
        Err(e) => raised(p.out, p.err, &e).print(),
    }
}

/// `_hostchat_upgrade_wait <as_json>`.
fn wait_upgrade(ctx: &mut Ctx, json: bool) -> Exit {
    let mut shown: Vec<String> = Vec::new();
    for _ in 0..1800 {
        let body = match ctx.call(false, "/cli/sew/status", "", false) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let (now, steps) = match upgrade_now(&body) {
            Ok(x) => x,
            Err(e) => return raised(String::new(), String::new(), &e).print(),
        };
        if !json {
            for s in steps {
                // `case " $shown " in *" $s "*)`.
                if !shown.contains(&s) {
                    out::stderr_line(&format!("\u{2026} {s}: done"));
                    shown.push(s);
                }
            }
        }
        if now.starts_with("done") {
            if json {
                if let Err(e) = out::stdout(&format!("{body}\n")) {
                    return out::after_stdout_error(&e);
                }
            }
            return upgrade_end(&body, json);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    line(
        &format!("{{\"error\":\"timeout\",\"hint\":\"still upgrading after 30 minutes: {CLI_NAME} hostchat status\"}}"),
        Exit::Refused,
    )
}
