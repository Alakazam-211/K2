//! The owner's mail verbs: `link` (and its old name `external`: add over
//! IMAP with a password, add or upgrade over OAuth with its long poll,
//! calendars, list, remove), `access` (grant, revoke, primary, level,
//! manage), `oauth-config` (show, set, clear) and `sending`, as their
//! `cmd_mail_*` functions read the words (`cli/src/mail-inboxes.sh`,
//! `mail-manage.sh`, `mail-watch.sh`) and `_mail_py`'s verbs answer
//! (`cli/src/mail.sh`). An agent's call is refused by the daemon (exit 3).
//!
//! Secrets come on stdin, never in argv: `link add --pass-stdin` and
//! `oauth-config set --secret-stdin` read one line (`IFS= read -r`).
//!
//! The OAuth link prints its sign-in line on stderr, then asks
//! `/cli/mail/link/oauth/status` every 3 seconds, for up to 15 minutes,
//! until the link is connected, denied, expired or failed.

use super::core::{self, run_env, usage, At, End, Plan, Py, Secs, Within, MODULE, R};
use super::py::{self as p, attr_get, big, iter};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::py;
use crate::prompt::{self, AtEof, Echo};
use crate::transport::urllib::Method;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use std::time::{Duration, Instant};

/// The verbs (second words) this module answers.
pub const VERBS: &[&str] = &["link", "external", "access", "oauth-config", "sending"];

/// The lines of `_mail_py` where these verbs call or raise. `tests`
/// pins each to its statement.
pub mod line {
    pub const SENDING_SET: u32 = 1498;
    pub const SENDING_GET: u32 = 1500;
    pub const SENDING_W: u32 = 1501;
    pub const SENDING_A: u32 = 1502;
    pub const SENDING_JSON: u32 = 1504;
    pub const SENDING_EFF: u32 = 1508;
    pub const SENDING_SRC: u32 = 1509;
    pub const EXT_ADD_CALL: u32 = 1720;
    pub const EXT_ADD_PRINT: u32 = 1724;
    pub const EXT_ADD_DRAFTS: u32 = 1725;
    pub const EXT_ADD_DRAFTS_PRINT: u32 = 1726;
    pub const CAL_POST: u32 = 1737;
    pub const CAL_GET: u32 = 1739;
    pub const CAL_STATE: u32 = 1743;
    pub const CAL_PRINT: u32 = 1744;
    pub const OAUTH_START: u32 = 1778;
    pub const OAUTH_FLOW: u32 = 1779;
    pub const OAUTH_LINK_ID: u32 = 1780;
    pub const OAUTH_DEVICE: u32 = 1783;
    pub const OAUTH_BROWSER: u32 = 1788;
    pub const OAUTH_STATUS: u32 = 1796;
    pub const OAUTH_ERROR: u32 = 1801;
    pub const OAUTH_STATE: u32 = 1803;
    pub const OAUTH_FINAL: u32 = 1812;
    pub const OAUTH_RELINKED: u32 = 1817;
    pub const OAUTH_UPGRADED: u32 = 1820;
    pub const OAUTH_UPGRADED_HINT: u32 = 1821;
    pub const OAUTH_CONNECTED: u32 = 1823;
    pub const OAUTH_CONNECTED_HINT: u32 = 1826;
    pub const OAUTH_HINT: u32 = 1832;
    pub const OAUTH_CONSENT: u32 = 1833;
    pub const OAUTH_CONSENT_ADD: u32 = 1834;
    pub const OAUTH_DIE: u32 = 1835;
    pub const EXT_REMOVE_CALL: u32 = 1839;
    pub const EXT_REMOVE_PRINT: u32 = 1843;
    pub const EXT_REMOVE_HINT: u32 = 1846;
    pub const EXT_REMOVE_HINT_PRINT: u32 = 1847;
    pub const OAUTH_SHOW: u32 = 1901;
    pub const OAUTH_SET: u32 = 1910;
    pub const OAUTH_CLEAR: u32 = 1912;
    pub const OAUTH_PROVS: u32 = 1916;
    pub const OAUTH_P: u32 = 1918;
    pub const OAUTH_LINE: u32 = 1923;
    pub const OAUTH_SECRET: u32 = 1928;
    pub const OAUTH_TENANT: u32 = 1929;
    pub const OAUTH_TENANT_LINE: u32 = 1930;
    pub const LINKS_CALL: u32 = 1956;
    pub const LINKS_ROWS: u32 = 1957;
    pub const LINKS_COUNT: u32 = 1959;
    pub const LINKS_W: u32 = 1963;
    pub const LINKS_PRI: u32 = 1967;
    pub const LINKS_ROW: u32 = 1971;
    pub const LINKS_GRANTS: u32 = 1975;
    pub const LINKS_ST: u32 = 1976;
    pub const LINKS_NOTE: u32 = 1979;
    pub const LINKS_GRANT: u32 = 1982;
    pub const GRANT_CALL: u32 = 2000;
    pub const GRANT_PRINT: u32 = 2004;
    pub const REVOKE_CALL: u32 = 2008;
    pub const REVOKE_PRINT: u32 = 2013;
    pub const PRIMARY_CALL: u32 = 2016;
    pub const PRIMARY_PRINT: u32 = 2021;
    pub const LEVEL_CALL: u32 = 2028;
    pub const LEVEL_PRINT: u32 = 2032;
    pub const MANAGE_CALL: u32 = 2110;
    pub const MANAGE_WHO: u32 = 2117;
    pub const MANAGE_CAPS: u32 = 2118;
    pub const MANAGE_PRINT: u32 = 2120;
}

/// The words after `mail <verb>`, as the verb's `cmd_mail_*` reads them.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    let r = match verb {
        "link" if words.is_empty() => Ok(Plan::Help("mail/link")),
        "link" | "external" => link(words),
        "access" => access(words),
        "oauth-config" => oauth_config(words),
        "sending" => sending(words),
        _ => Err(Plan::Shell),
    };
    r.unwrap_or_else(|p| p)
}

/// `IFS= read -r line || true` on stdin ([`crate::prompt::read_line`]):
/// the line, or what came before the end. `None`: not UTF-8.
fn read_line() -> Option<String> {
    let secret = prompt::read_line(Echo::Shown, AtEof::Keep);
    String::from_utf8(secret.expose().to_vec()).ok()
}

/// A value flag's value: `[ $# -ge 2 ] || { _mail_err "usage" <hint>; exit 2; }`.
fn value<'a>(words: &[&'a str], i: usize, hint: &str) -> Result<&'a str, Plan> {
    words.get(i + 1).copied().ok_or_else(|| usage(hint))
}

/// `cmd_mail_external` (`mail link …` and `mail external …`).
fn link(words: &[&str]) -> Result<Plan, Plan> {
    let first = words.first().copied().unwrap_or("");
    let (sub, rest) = match first {
        "add" | "list" | "remove" | "upgrade" | "relink" | "calendars" => (first, &words[1..]),
        "" => return Ok(Plan::Help("mail/link")),
        f if f.starts_with('-') => ("list", words),
        "grant" | "revoke" => {
            return Err(usage(format!(
                "per-workspace access moved — use '{CLI_NAME} mail access {first} <address> <workspace>'"
            )))
        }
        other => {
            return Err(usage(format!(
                "unknown link command '{other}' — usage: {CLI_NAME} mail link <add|upgrade|relink|calendars|list|remove>"
            )))
        }
    };
    let mut vals: Vec<(&'static str, String)> = Vec::new();
    let (mut address, mut pass_stdin, mut json) = ("", false, false);
    let flags: &[(&str, &'static str, &str)] = &[
        (
            "--provider",
            "provider",
            "--provider requires gmail or microsoft",
        ),
        (
            "--features",
            "features",
            "--features requires mail, calendar-read, mail,calendar-read or calendar",
        ),
        ("--level", "level", "--level requires read, draft, or send"),
        (
            "--workspace",
            "workspace",
            "--workspace requires a value (the ONE workspace to bind)",
        ),
        (
            "--host",
            "host",
            "--host requires a value (e.g. imap.gmail.com)",
        ),
        ("--port", "port", "--port requires a number"),
        ("--tls", "tls", "--tls requires implicit-tls or starttls"),
        (
            "--user",
            "user",
            "--user requires the IMAP login (default: the address)",
        ),
        ("--name", "name", "--name requires a display name"),
        (
            "--drafts-folder",
            "drafts_folder",
            "--drafts-folder requires a folder name",
        ),
        (
            "--smtp-host",
            "smtp_host",
            "--smtp-host requires a value (e.g. smtp.gmail.com)",
        ),
        ("--smtp-port", "smtp_port", "--smtp-port requires a number"),
        (
            "--smtp-tls",
            "smtp_tls",
            "--smtp-tls requires implicit-tls or starttls",
        ),
        (
            "--pass-ref",
            "pass_ref",
            "--pass-ref requires env:<VAR> or an absolute file path",
        ),
    ];
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        if let Some((_, key, hint)) = flags.iter().find(|(f, _, _)| *f == w) {
            let x = value(rest, i, hint)?;
            if *key == "port" && !core::is_uint(x) {
                return Err(usage(
                    "--port must be a number (default: 993 implicit-tls / 143 starttls)",
                ));
            }
            if *key == "smtp_port" && !core::is_uint(x) {
                return Err(usage(
                    "--smtp-port must be a number (default: 465 implicit-tls / 587 starttls)",
                ));
            }
            vals.push((*key, x.to_string()));
            i += 2;
            continue;
        }
        match w {
            "--pass-stdin" => pass_stdin = true,
            "--json" => json = true,
            f if f.starts_with('-') => {
                return Err(usage(format!(
                    "unknown flag '{f}' — see {CLI_NAME} mail link --help"
                )))
            }
            pos => {
                if address.is_empty() {
                    address = pos;
                } else {
                    return Err(usage(format!("unexpected extra argument '{pos}'")));
                }
            }
        }
        i += 1;
    }
    let val = |k: &str| -> String {
        vals.iter()
            .rev()
            .find(|(key, _)| *key == k)
            .map(|(_, x)| x.clone())
            .unwrap_or_default()
    };
    let (provider, features, workspace, host) = (
        val("provider"),
        val("features"),
        val("workspace"),
        val("host"),
    );
    let pass_ref = val("pass_ref");
    match sub {
        "add" => {
            if !provider.is_empty() {
                if !matches!(provider.as_str(), "gmail" | "microsoft") {
                    return Err(usage("--provider must be gmail or microsoft"));
                }
                if address.is_empty() || workspace.is_empty() {
                    return Err(usage(format!(
                        "link add --provider {provider} requires <address> --workspace <ws>"
                    )));
                }
                if !host.is_empty() || pass_stdin || !pass_ref.is_empty() {
                    return Err(usage("--provider is OAuth — drop --host/--pass-stdin/--pass-ref (those are the generic-IMAP path)"));
                }
                return Ok(Plan::Run(
                    run_env("link_oauth_add", json)
                        .with("M_ADDRESS", address)
                        .with("M_WORKSPACE", workspace)
                        .with("M_PROVIDER", provider)
                        .with("M_FEATURES", features),
                ));
            }
            if !features.is_empty() {
                return Err(usage(
                    "--features is for OAuth links (--provider gmail|microsoft)",
                ));
            }
            if address.is_empty() || host.is_empty() || workspace.is_empty() {
                return Err(usage("link add requires <address> --host <imap-host> --workspace <ws> (+ --pass-stdin | --pass-ref <ref>)  |  OAuth: --provider gmail|microsoft --workspace <ws>"));
            }
            let mut pass = String::new();
            if pass_stdin {
                if !pass_ref.is_empty() {
                    return Err(usage("--pass-stdin and --pass-ref are mutually exclusive"));
                }
                // Read here, after every check that can hand the line to
                // the shell: what stdin held is gone once read.
                pass = read_line().unwrap_or_else(|| "\u{fffd}".into());
                if pass.is_empty() {
                    return Err(usage(format!(
                        "--pass-stdin: no password on stdin (pipe it: printf %s \"$PW\" | {CLI_NAME} mail link add …)"
                    )));
                }
            } else if pass_ref.is_empty() {
                return Err(usage("link add needs the app-password: --pass-stdin (piped) or --pass-ref env:<VAR> / /abs/path"));
            }
            let mut e = run_env("external_add", json)
                .with("M_ADDRESS", address)
                .with("M_WORKSPACE", workspace)
                .with("M_HOST", host)
                .with("M_PASS", pass)
                .with("M_PASS_REF", pass_ref);
            for (key, env) in [
                ("port", "M_PORT"),
                ("tls", "M_TLS"),
                ("user", "M_USER"),
                ("name", "M_NAME"),
                ("drafts_folder", "M_DRAFTS_FOLDER"),
                ("smtp_host", "M_SMTP_HOST"),
                ("smtp_port", "M_SMTP_PORT"),
                ("smtp_tls", "M_SMTP_TLS"),
            ] {
                e.set(env, val(key));
            }
            Ok(Plan::Run(e))
        }
        "upgrade" => {
            if address.is_empty() || features.is_empty() {
                return Err(usage(
                    "link upgrade requires <address> --features calendar-read|mail|calendar",
                ));
            }
            if !host.is_empty() || pass_stdin || !pass_ref.is_empty() || !workspace.is_empty() {
                return Err(usage(
                    "link upgrade takes only <address> --features (the row keeps its workspace)",
                ));
            }
            Ok(Plan::Run(
                run_env("link_oauth_upgrade", json)
                    .with("M_ADDRESS", address)
                    .with("M_PROVIDER", provider)
                    .with("M_FEATURES", features),
            ))
        }
        "relink" => {
            if address.is_empty() {
                return Err(usage(format!(
                    "link relink requires the <address> ('{CLI_NAME} mail link list' marks the ones that need it)"
                )));
            }
            if !host.is_empty()
                || pass_stdin
                || !pass_ref.is_empty()
                || !workspace.is_empty()
                || !features.is_empty()
            {
                return Err(usage(format!(
                    "link relink takes only <address> [--json] (it keeps the link's features; add more with '{CLI_NAME} mail link upgrade')"
                )));
            }
            Ok(Plan::Run(
                run_env("link_oauth_upgrade", json)
                    .with("M_ADDRESS", address)
                    .with("M_PROVIDER", provider)
                    .with("M_FEATURES", "")
                    .with("M_RELINK", "true"),
            ))
        }
        "calendars" => {
            if !matches!(address, "" | "on" | "off") {
                return Err(usage(
                    "link calendars takes on or off (or nothing to show the switch)",
                ));
            }
            if !provider.is_empty()
                || !features.is_empty()
                || !host.is_empty()
                || !workspace.is_empty()
                || pass_stdin
                || !pass_ref.is_empty()
            {
                return Err(usage("link calendars takes only [on|off] [--json]"));
            }
            Ok(Plan::Run(
                run_env("link_calendars", json).with("M_LINK_CAL", address),
            ))
        }
        "list" => Ok(Plan::Run(run_env("link_list", json))),
        _ => {
            if address.is_empty() {
                return Err(usage(format!(
                    "link remove requires the <address> ('{CLI_NAME} mail link list')"
                )));
            }
            Ok(Plan::Run(
                run_env("external_remove", json).with("M_ADDRESS", address),
            ))
        }
    }
}

/// `cmd_mail_access`.
fn access(words: &[&str]) -> Result<Plan, Plan> {
    let Some((sub, rest)) = words.split_first() else {
        return Ok(Plan::Help("mail/access"));
    };
    let verb = match *sub {
        "grant" => "access_grant",
        "revoke" => "access_revoke",
        "primary" => "access_primary",
        "level" => "access_level",
        "manage" => "access_manage",
        other => {
            return Err(usage(format!(
                "unknown access command '{other}' — usage: {CLI_NAME} mail access <grant|revoke|primary|level|manage>"
            )))
        }
    };
    let (mut address, mut workspace, mut level, mut json) = ("", "", String::new(), false);
    let (mut manage_v, mut delete_v, mut cal_omit) = ("", "", "");
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--calendar-omit-description" => {
                cal_omit = match rest.get(i + 1).copied() {
                    Some("on") => "true",
                    Some("off") => "false",
                    _ => return Err(usage("--calendar-omit-description takes on or off")),
                };
                i += 2;
                continue;
            }
            "--level" => {
                level = value(rest, i, "--level requires read, draft, or send")?.to_string();
                i += 2;
                continue;
            }
            "--on" => manage_v = "true",
            "--off" => manage_v = "false",
            "--delete" => delete_v = "true",
            "--no-delete" => delete_v = "false",
            "--json" => json = true,
            f if f.starts_with('-') => {
                return Err(usage(format!(
                    "unknown flag '{f}' — see {CLI_NAME} mail access --help"
                )))
            }
            pos => {
                if address.is_empty() {
                    address = pos;
                } else if workspace.is_empty() {
                    workspace = pos;
                } else {
                    return Err(usage(format!("unexpected extra argument '{pos}'")));
                }
            }
        }
        i += 1;
    }
    if address.is_empty() || workspace.is_empty() {
        return Err(usage(format!(
            "access {sub} requires <address> <workspace>"
        )));
    }
    if !cal_omit.is_empty() && verb != "access_grant" && verb != "access_level" {
        return Err(usage(format!(
            "--calendar-omit-description goes with '{CLI_NAME} mail access grant' or 'level'"
        )));
    }
    if verb != "access_manage" && (!manage_v.is_empty() || !delete_v.is_empty()) {
        return Err(usage(format!(
            "--on/--off/--delete/--no-delete only apply to '{CLI_NAME} mail access manage'"
        )));
    }
    if verb == "access_grant" || verb == "access_level" {
        if level.is_empty() && verb == "access_grant" {
            level = "read".into();
        }
        if level.is_empty() {
            return Err(usage(format!(
                "access {sub} requires --level read|draft|send"
            )));
        }
        if !matches!(level.as_str(), "read" | "draft" | "send") {
            return Err(usage(
                "--level must be read, draft, or send (send is hosted-only)",
            ));
        }
    }
    if verb == "access_manage" {
        if manage_v.is_empty() {
            manage_v = "true";
        }
        if delete_v.is_empty() {
            delete_v = "false";
        }
        if manage_v == "false" {
            delete_v = "false";
        }
        return Ok(Plan::Run(
            run_env(verb, json)
                .with("M_ADDRESS", address)
                .with("M_WORKSPACE", workspace)
                .with("M_MANAGE", manage_v)
                .with("M_DELETE", delete_v),
        ));
    }
    Ok(Plan::Run(
        run_env(verb, json)
            .with("M_ADDRESS", address)
            .with("M_WORKSPACE", workspace)
            .with("M_LEVEL", level)
            .with("M_CAL_OMIT", cal_omit),
    ))
}

/// `cmd_mail_oauth_config`.
fn oauth_config(words: &[&str]) -> Result<Plan, Plan> {
    let first = words.first().copied().unwrap_or("");
    let (sub, rest) = match first {
        "show" | "set" | "clear" => (first, &words[1..]),
        "" => ("show", words),
        f if f.starts_with('-') => ("show", words),
        other => {
            return Err(usage(format!(
                "unknown oauth-config command '{other}' — usage: {CLI_NAME} mail oauth-config <show|set|clear>"
            )))
        }
    };
    let (mut provider, mut client_id, mut tenant) = ("", "", "");
    let (mut secret_stdin, mut json) = (false, false);
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let slot = match w {
            "--provider" => Some((&mut provider, "--provider requires gmail or microsoft")),
            "--client-id" => Some((
                &mut client_id,
                "--client-id requires the app's client (application) id",
            )),
            "--tenant" => Some((
                &mut tenant,
                "--tenant requires common|organizations|consumers|<tenant id or domain>",
            )),
            _ => None,
        };
        if let Some((slot, hint)) = slot {
            *slot = value(rest, i, hint)?;
            i += 2;
            continue;
        }
        match w {
            "--secret-stdin" => secret_stdin = true,
            "--json" => json = true,
            other => {
                return Err(usage(format!(
                    "unexpected argument '{other}' — see {CLI_NAME} mail oauth-config --help"
                )))
            }
        }
        i += 1;
    }
    if !matches!(provider, "" | "gmail" | "microsoft") {
        return Err(usage("--provider must be gmail or microsoft"));
    }
    let mut secret = String::new();
    match sub {
        "set" => {
            if provider.is_empty() {
                return Err(usage(
                    "oauth-config set requires --provider gmail|microsoft",
                ));
            }
            if client_id.is_empty() && tenant.is_empty() {
                return Err(usage(
                    "oauth-config set requires --client-id (Microsoft may pass just --tenant)",
                ));
            }
            if secret_stdin {
                if provider != "gmail" {
                    return Err(usage(
                        "--secret-stdin is for Gmail only (Microsoft is a public client)",
                    ));
                }
                secret = read_line().unwrap_or_else(|| "\u{fffd}".into());
                if secret.is_empty() {
                    return Err(usage("--secret-stdin: no secret on stdin (pipe it in)"));
                }
            }
        }
        "clear" if provider.is_empty() => {
            return Err(usage(
                "oauth-config clear requires --provider gmail|microsoft",
            ));
        }
        _ => {}
    }
    Ok(Plan::Run(
        run_env(&format!("oauth_config_{sub}"), json)
            .with("M_PROVIDER", provider)
            .with("M_CLIENT_ID", client_id)
            .with("M_TENANT", tenant)
            .with("M_OAUTH_SECRET", secret),
    ))
}

/// `cmd_mail_sending`.
fn sending(words: &[&str]) -> Result<Plan, Plan> {
    let (mut workspace, mut mode, mut json) = ("", "", false);
    for w in words {
        match *w {
            "--json" => json = true,
            f if f.starts_with('-') => {
                return Err(usage(format!(
                    "unknown flag '{f}' — see {CLI_NAME} mail sending --help"
                )))
            }
            pos => {
                if workspace.is_empty() {
                    workspace = pos;
                } else if mode.is_empty() {
                    mode = pos;
                } else {
                    return Err(usage(format!(
                        "unexpected extra argument '{pos}' — usage: {CLI_NAME} mail sending <workspace> [off|approval|on]"
                    )));
                }
            }
        }
    }
    if workspace.is_empty() {
        return Err(usage(format!(
            "missing <workspace> — usage: {CLI_NAME} mail sending <workspace> [off|approval|on]"
        )));
    }
    let mut verb = "sending_get";
    if !mode.is_empty() {
        if !matches!(mode, "off" | "approval" | "on") {
            return Err(usage(format!(
                "invalid mode '{mode}' — valid: off, approval, on"
            )));
        }
        verb = "sending_set";
    }
    Ok(Plan::Run(
        run_env(verb, json)
            .with("M_WORKSPACE", workspace)
            .with("M_AGENT_SEND", mode),
    ))
}

/// `_mail_py`'s branch for `verb`: `None` when it is not one of these.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    Some(match verb {
        "external_add" => external_add_py(py_),
        "link_calendars" => link_calendars_py(py_),
        "link_oauth_add" => oauth_py(py_, false),
        "link_oauth_upgrade" => oauth_py(py_, true),
        "external_remove" => external_remove_py(py_),
        "link_list" => link_list_py(py_),
        "access_grant" | "access_level" => grant_or_level_py(py_, verb == "access_grant"),
        "access_revoke" => revoke_py(py_),
        "access_primary" => primary_py(py_),
        "access_manage" => manage_py(py_),
        "oauth_config_show" | "oauth_config_set" | "oauth_config_clear" => {
            oauth_config_py(py_, verb)
        }
        "sending_get" | "sending_set" => sending_py(py_, verb == "sending_set"),
        _ => return None,
    })
}

fn g(d: &Value, key: &str, l: u32) -> R<Option<Value>> {
    attr_get(d, key).at(l, MODULE)
}

fn gs(d: &Value, key: &str, default: &str, l: u32) -> R<String> {
    Ok(py::str_of(
        &p::get_or(d, key, p::sv(default)).at(l, MODULE)?,
    ))
}

fn sv(t: &str) -> Value {
    p::sv(t)
}

/// `print(d.get("hint") or <text>)`, at line `l`.
fn hint_or(py_: &mut Py, d: &Value, l: u32, text: impl FnOnce() -> R<String>) -> R<()> {
    match g(d, "hint", l)?.filter(py::truthy) {
        Some(h) => py_.print_v(&h),
        None => {
            let t = text()?;
            py_.print(&t);
        }
    }
    Ok(())
}

/// `external_add`: a linked IMAP inbox with a password.
fn external_add_py(py_: &mut Py) -> R<()> {
    let e = py_.env.clone();
    let mut body = vec![
        ("project", sv(e.get("M_WORKSPACE"))),
        ("address", sv(e.get("M_ADDRESS"))),
        ("host", sv(e.get("M_HOST"))),
    ];
    for (k, key) in [
        ("M_PORT", "port"),
        ("M_TLS", "tls"),
        ("M_USER", "username"),
        ("M_NAME", "displayName"),
        ("M_DRAFTS_FOLDER", "draftsFolder"),
        ("M_SMTP_HOST", "smtpHost"),
        ("M_SMTP_PORT", "smtpPort"),
        ("M_SMTP_TLS", "smtpTls"),
    ] {
        let v = e.get(k);
        if !v.is_empty() {
            body.push((
                key,
                if key == "port" || key == "smtpPort" {
                    Value::Int(big::trim(v))
                } else {
                    sv(v)
                },
            ));
        }
    }
    if !e.get("M_PASS").is_empty() {
        body.push(("password", sv(e.get("M_PASS"))));
    } else if !e.get("M_PASS_REF").is_empty() {
        body.push(("passwordRef", sv(e.get("M_PASS_REF"))));
    }
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/external/add",
        &[],
        Some(&core::obj(body)),
        Secs::Int(90),
        None,
        line::EXT_ADD_CALL,
    )?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = gs(&d, "address", "?", line::EXT_ADD_PRINT)?;
    py_.print(&format!(
        "connected {a} -> workspace {}",
        e.get("M_WORKSPACE")
    ));
    if p::truthy(g(&d, "draftsFolder", line::EXT_ADD_DRAFTS)?.as_ref()) {
        let f = p::py_index(&d, "draftsFolder").at(line::EXT_ADD_DRAFTS_PRINT, MODULE)?;
        py_.print(&format!("drafts folder: {}", py::str_of(&f)));
    } else {
        py_.print(&format!(
            "no Drafts folder detected — set one with --drafts-folder if '{CLI_NAME} mail draft' errors"
        ));
    }
    py_.print("agents in that workspace can now READ this inbox and save drafts");
    py_.print(&format!(
        "({CLI_NAME} mail draft --to/--subject compose, or draft <id> reply);"
    ));
    py_.print(&format!(
        "sending FROM it needs level 'send' AND workspace Sending=on ({CLI_NAME} mail send/reply)."
    ));
    Ok(())
}

/// `link_calendars`: the owner's switch for calendars on linked inboxes.
fn link_calendars_py(py_: &mut Py) -> R<()> {
    let want = py_.env("M_LINK_CAL").to_string();
    let d = if !want.is_empty() {
        let body = core::obj(vec![("enabled", Value::Bool(want == "on"))]);
        py_.post("/cli/mail/link/calendars", &body, line::CAL_POST)?
    } else {
        py_.get("/cli/mail/link/calendars", &[], line::CAL_GET)?
    };
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let state = match g(&d, "linkedCalendars", line::CAL_STATE)?.filter(py::truthy) {
        Some(s) => s,
        None => sv(if p::truthy(g(&d, "enabled", line::CAL_STATE)?.as_ref()) {
            "on"
        } else {
            "off"
        }),
    };
    let tail = if want.is_empty() {
        ""
    } else if p::truthy(g(&d, "changed", line::CAL_PRINT)?.as_ref()) {
        " (changed)"
    } else {
        " (unchanged)"
    };
    py_.print(&format!(
        "Calendars on linked inboxes: {}{tail}",
        py::str_of(&state)
    ));
    if p::eq_str(&state, "on") {
        py_.print(&format!("Agents with a grant can use a linked Microsoft 365 or Google account's calendar ({CLI_NAME} calendar)."));
    } else {
        py_.print(&format!("Every linked calendar verb answers capability_missing (owner_disabled). Turn on: {CLI_NAME} mail link calendars on"));
    }
    Ok(())
}

/// How long the OAuth link waits for the person (`time.time() + 900`).
const OAUTH_WAIT: Duration = Duration::from_secs(900);
/// Between two status asks (`time.sleep(3)`).
const OAUTH_POLL: Duration = Duration::from_secs(3);

/// `link_oauth_add` and `link_oauth_upgrade`: start, then wait for the
/// person's sign-in.
fn oauth_py(py_: &mut Py, upgrade: bool) -> R<()> {
    let e = py_.env.clone();
    let mut provider = e.get("M_PROVIDER").to_string();
    let relink = e.on("M_RELINK");
    let body = if upgrade {
        let mut b = vec![("inbox", sv(e.get("M_ADDRESS")))];
        if relink {
            b.push(("relink", Value::Bool(true)));
        }
        if !e.get("M_FEATURES").is_empty() {
            b.push(("features", sv(e.get("M_FEATURES"))));
        }
        if !provider.is_empty() {
            b.push(("provider", sv(&provider)));
        }
        core::obj(b)
    } else {
        let mut b = vec![
            ("address", sv(e.get("M_ADDRESS"))),
            ("provider", sv(&provider)),
            ("workspace", sv(e.get("M_WORKSPACE"))),
        ];
        if !e.get("M_FEATURES").is_empty() {
            b.push(("features", sv(e.get("M_FEATURES"))));
        }
        core::obj(b)
    };
    let d = py_.call_full(
        Method::Post,
        "/cli/mail/link/oauth/start",
        &[],
        Some(&body),
        Secs::Int(45),
        None,
        line::OAUTH_START,
    )?;
    let flow = g(&d, "flow", line::OAUTH_FLOW)?;
    let link_id = g(&d, "linkId", line::OAUTH_LINK_ID)?.unwrap_or(Value::Null);
    let device = flow.as_ref().is_some_and(|f| p::eq_str(f, "device"));
    if provider.is_empty() {
        provider = if device { "microsoft" } else { "gmail" }.into();
    }
    let addr = e.get("M_ADDRESS");
    let lead = if device {
        let l = line::OAUTH_DEVICE;
        let features = gs(&d, "features", "mail", l)?;
        let url = gs(&d, "verificationUrl", "?", l)?;
        let code = gs(&d, "userCode", "?", l)?;
        let who = p::or(g(&d, "address", l)?, sv(addr));
        format!(
            "To {} {addr} ({features}): open {url} and enter the code {code} — sign in as {}\n",
            if relink {
                "re-link"
            } else if upgrade {
                "upgrade"
            } else {
                "connect"
            },
            py::str_of(&who)
        )
    } else {
        let features = gs(&d, "features", "mail", line::OAUTH_BROWSER)?;
        format!("A browser window opened — approve access to {addr} there ({features}).\n")
    };
    let _ = crate::out::write_flushed(
        &mut std::io::stderr().lock(),
        format!("{lead}Waiting for approval (Ctrl-C to stop; the link stays pending)…\n")
            .as_bytes(),
    );
    // `params={"linkId": link_id}`: urlencode writes `str(link_id)`.
    let params = vec![("linkId".to_string(), py::str_of(&link_id))];
    let deadline = Instant::now() + OAUTH_WAIT;
    let mut fin = None;
    while Instant::now() < deadline {
        let s = py_
            .request(
                Method::Get,
                "/cli/mail/link/oauth/status",
                &params,
                None,
                Secs::Int(30),
            )
            .within(line::OAUTH_STATUS, MODULE)?;
        let Some(s) = s else {
            return Err(py_.unreachable());
        };
        let err = g(&s, "error", line::OAUTH_ERROR)?;
        if let Some(err) = err.filter(py::truthy) {
            return Err(within_module(
                py_.die(&err, None, None),
                line::OAUTH_ERROR + 1,
            ));
        }
        let state = g(&s, "state", line::OAUTH_STATE)?;
        let done = state.as_ref().is_some_and(|st| {
            ["connected", "denied", "expired", "error"]
                .iter()
                .any(|w| p::eq_str(st, w))
        });
        if done {
            fin = Some(s);
            break;
        }
        std::thread::sleep(OAUTH_POLL);
    }
    let Some(fin) = fin else {
        return Err(py_.die_with(
            "timeout",
            &format!(
                "the link did not complete in time — run '{CLI_NAME} mail link add --provider {provider}' again"
            ),
            Some(Exit::Usage),
        ));
    };
    let state = g(&fin, "state", line::OAUTH_FINAL)?.unwrap_or(Value::Null);
    if p::eq_str(&state, "connected") {
        if py_.json {
            py_.print_json(&fin);
        } else if relink {
            let a = p::or(g(&fin, "address", line::OAUTH_RELINKED)?, sv(addr));
            py_.print(&format!("re-linked {} ({provider})", py::str_of(&a)));
            py_.print("its grants, levels, calendar features and ids are unchanged.");
        } else if upgrade {
            let a = p::or(g(&fin, "address", line::OAUTH_UPGRADED)?, sv(addr));
            py_.print(&format!("upgraded {} ({provider})", py::str_of(&a)));
            let h = p::or(
                g(&fin, "hint", line::OAUTH_UPGRADED_HINT)?,
                sv("its grants, levels and ids are unchanged."),
            );
            py_.print_v(&h);
        } else {
            let a = p::or(g(&fin, "address", line::OAUTH_CONNECTED)?, sv(addr));
            py_.print(&format!(
                "connected {} -> workspace {} ({provider})",
                py::str_of(&a),
                e.get("M_WORKSPACE")
            ));
            let h = p::or(
                g(&fin, "hint", line::OAUTH_CONNECTED_HINT)?,
                sv("agents in that workspace can now read it and save reply drafts."),
            );
            py_.print_v(&h);
        }
        return Ok(());
    }
    let mut hint = match g(&fin, "hint", line::OAUTH_HINT)?.filter(py::truthy) {
        Some(h) => h,
        None => sv(&format!("the link {}", py::str_of(&state))),
    };
    if let Some(url) = g(&fin, "adminConsentUrl", line::OAUTH_CONSENT)?.filter(py::truthy) {
        let inside = match &url {
            Value::Str(t) => p::contains(&hint, &super::py::text(t)),
            other => Err(format!(
                "TypeError: 'in <string>' requires string as left operand, not {}",
                py::type_name(other)
            )),
        }
        .at(line::OAUTH_CONSENT, MODULE)?;
        if !inside {
            let tail = format!(" Admin consent: {}", py::str_of(&url));
            hint = sv(&p::concat_str(&hint, &tail).at(line::OAUTH_CONSENT_ADD, MODULE)?);
        }
    }
    let code = match g(&fin, "code", line::OAUTH_DIE)?.filter(py::truthy) {
        Some(c) => c,
        None => sv(if p::eq_str(&state, "denied") {
            "denied"
        } else {
            "error"
        }),
    };
    Err(py_.die(
        &core::obj(vec![("code", code), ("hint", hint)]),
        Some(Exit::Error),
        None,
    ))
}

/// The module's frame at `line` on a traceback from `die`.
fn within_module(end: End, line: u32) -> End {
    match end {
        End::Raise(mut t) => {
            t.frames.insert(0, (line, MODULE));
            End::Raise(t)
        }
        other => other,
    }
}

/// `external_remove`.
fn external_remove_py(py_: &mut Py) -> R<()> {
    let body = core::obj(vec![("address", sv(py_.env("M_ADDRESS")))]);
    let d = py_.post("/cli/mail/external/remove", &body, line::EXT_REMOVE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = gs(&d, "address", "?", line::EXT_REMOVE_PRINT)?;
    py_.print(&format!(
        "removed {a} — its stored credential was deleted from the vault."
    ));
    if p::truthy(g(&d, "revokeHint", line::EXT_REMOVE_HINT)?.as_ref()) {
        let h = p::py_index(&d, "revokeHint").at(line::EXT_REMOVE_HINT_PRINT, MODULE)?;
        py_.print_v(&h);
    }
    Ok(())
}

/// `link_list`: the linked rows of the owner's catalog.
fn link_list_py(py_: &mut Py) -> R<()> {
    let d = py_.get("/cli/mail/inboxes", &[], line::LINKS_CALL)?;
    let all = p::or(
        g(&d, "inboxes", line::LINKS_ROWS)?,
        Value::Array(Vec::new()),
    );
    let mut rows = Vec::new();
    for r in iter(&all).at(line::LINKS_ROWS, MODULE)? {
        let src = attr_get(&r, "source")
            .at(line::LINKS_ROWS, "<listcomp>")
            .within(line::LINKS_ROWS, MODULE)?;
        if src.as_ref().is_some_and(|s| p::eq_str(s, "linked")) {
            rows.push(r);
        }
    }
    let cli = CLI_NAME;
    if py_.json {
        let n = rows.len();
        py_.print_json(&core::obj(vec![
            ("ok", Value::Bool(true)),
            ("count", Value::Int(n.to_string())),
            ("inboxes", Value::Array(rows)),
        ]));
        return Ok(());
    }
    if rows.is_empty() {
        py_.print(&format!("No linked accounts — add one: {cli} mail link add <address> --host <imap-host> --workspace <ws> --pass-stdin"));
        return Ok(());
    }
    let mut w = "ADDRESS".len();
    for r in &rows {
        let a = p::get_or(r, "address", sv(""))
            .at(line::LINKS_W, "<genexpr>")
            .within(line::LINKS_W, MODULE)?;
        w = w.max(
            p::len(&a)
                .at(line::LINKS_W, "<genexpr>")
                .within(line::LINKS_W, MODULE)?,
        );
    }
    let w = w + 2;
    py_.print(&format!(
        "{}{}{}HOST",
        p::ljust("ADDRESS", w),
        p::ljust("STATUS", 25),
        p::ljust("PRIMARY", 22)
    ));
    let mut notes: Vec<(Value, Option<Value>)> = Vec::new();
    let ws_of = |x: &Value, l: u32| -> R<Value> {
        match g(x, "workspace", l)?.filter(py::truthy) {
            Some(v) => Ok(v),
            None => p::get_or(x, "projectId", sv("?")).at(l, MODULE),
        }
    };
    for r in &rows {
        let pri = p::or(g(r, "primary", line::LINKS_PRI)?, Value::Object(Vec::new()));
        // `r` is a dict: its `get("source")` worked.
        let word = core::print::status_word(r);
        let status = attr_get(r, "status").ok().flatten();
        if !status.as_ref().is_some_and(|x| p::py_eq(&word, x)) {
            let a = p::get_or(r, "address", sv("")).unwrap_or_else(|_| sv(""));
            notes.push((a, attr_get(r, "relinkReason").ok().flatten()));
        }
        let l = line::LINKS_ROW;
        let a = gs(r, "address", "", l)?;
        let st = py::str_of(&word);
        let who = match g(&pri, "workspace", l)?.filter(py::truthy) {
            Some(v) => v,
            None => p::or(g(&pri, "projectId", l)?, sv("?")),
        };
        let host = gs(r, "host", "?", l)?;
        let tls = gs(r, "tls", "?", l)?;
        py_.print(&format!(
            "{}{}{}{host} ({tls})",
            p::ljust(&a, w),
            p::ljust(&st, 25),
            p::ljust(&py::str_of(&who), 22)
        ));
        let grants = p::or(
            g(r, "grants", line::LINKS_GRANTS)?,
            Value::Array(Vec::new()),
        );
        for gr in iter(&grants).at(line::LINKS_GRANTS, MODULE)? {
            let st = g(&gr, "sendState", line::LINKS_ST)?;
            let blocked = st
                .as_ref()
                .is_some_and(|s| p::eq_str(s, "blocked_off") || p::eq_str(s, "blocked_approval"));
            let note = if blocked {
                let l = line::LINKS_NOTE;
                let mode = p::or(g(&gr, "agentSend", l)?, sv("off"));
                let ws = ws_of(&gr, l)?;
                format!(
                    "   BLOCKED: agent sending is {} for this workspace — {cli} mail sending {} on",
                    py::str_of(&mode),
                    py::str_of(&ws)
                )
            } else {
                String::new()
            };
            let l = line::LINKS_GRANT;
            let ws = ws_of(&gr, l)?;
            let level = gs(&gr, "level", "?", l)?;
            py_.print(&format!(
                "    grant: {} {level}{note}",
                p::ljust(&py::str_of(&ws), 20)
            ));
        }
    }
    for (addr, why) in notes {
        let a = py::str_of(&addr);
        py_.print("");
        let why = why.unwrap_or(Value::Null);
        if p::eq_str(&why, "throttled") {
            py_.print(&format!("{a}: the provider is limiting this account (too many connections or failed logins). Wait; linking it again won't help. Run fewer mail calls at once."));
        } else if p::eq_str(&why, "mail_access_not_granted") {
            py_.print(&format!("{a}: Google's consent doesn't include the Gmail permission. A person runs '{cli} mail link relink {a}' and ticks the Gmail permission."));
        } else {
            py_.print(&format!("{a}: the provider refused the sign-in. A person runs '{cli} mail link relink {a}' (same inbox, grants and ids)."));
        }
    }
    Ok(())
}

/// `{"address": …, "project": <workspace>, …}`.
fn access_body(py_: &Py, extra: Vec<(&'static str, Value)>) -> Value {
    let mut b = vec![
        ("address", sv(py_.env("M_ADDRESS"))),
        ("project", sv(py_.env("M_WORKSPACE"))),
    ];
    b.extend(extra);
    core::obj(b)
}

/// `access_grant` and `access_level`.
fn grant_or_level_py(py_: &mut Py, grant: bool) -> R<()> {
    let mut extra = vec![("level", sv(py_.env("M_LEVEL")))];
    if !py_.env("M_CAL_OMIT").is_empty() {
        extra.push((
            "calendarOmitDescription",
            Value::Bool(py_.env.on("M_CAL_OMIT")),
        ));
    }
    let body = access_body(py_, extra);
    let (route, from) = if grant {
        ("/cli/mail/access/grant", line::GRANT_CALL)
    } else {
        ("/cli/mail/access/set-level", line::LEVEL_CALL)
    };
    let d = py_.post(route, &body, from)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let (level, ws) = (
        py_.env("M_LEVEL").to_string(),
        py_.env("M_WORKSPACE").to_string(),
    );
    if grant {
        let l = line::GRANT_PRINT;
        return hint_or(py_, &d, l, || {
            let a = gs(&d, "address", "?", l)?;
            Ok(format!("granted {level} on {a} to {ws}"))
        });
    }
    let l = line::LEVEL_PRINT;
    let who = if p::truthy(g(&d, "isPrimary", l)?.as_ref()) {
        "primary".to_string()
    } else {
        ws
    };
    let a = gs(&d, "address", "?", l + 1)?;
    py_.print(&format!("set {who} level on {a} to {level}."));
    Ok(())
}

/// `access_revoke`.
fn revoke_py(py_: &mut Py) -> R<()> {
    let body = access_body(py_, Vec::new());
    let d = py_.post("/cli/mail/access/revoke", &body, line::REVOKE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let a = gs(&d, "address", "?", line::REVOKE_PRINT)?;
    let ws = py_.env("M_WORKSPACE").to_string();
    py_.print(&format!("revoked {ws}'s access to {a}."));
    Ok(())
}

/// `access_primary`.
fn primary_py(py_: &mut Py) -> R<()> {
    let body = access_body(py_, Vec::new());
    let d = py_.post("/cli/mail/access/set-primary", &body, line::PRIMARY_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let ws = py_.env("M_WORKSPACE").to_string();
    let l = line::PRIMARY_PRINT;
    hint_or(py_, &d, l, || {
        let a = gs(&d, "address", "?", l)?;
        Ok(format!("{ws} now manages {a}"))
    })
}

/// `access_manage`.
fn manage_py(py_: &mut Py) -> R<()> {
    let body = access_body(
        py_,
        vec![
            ("canManage", Value::Bool(py_.env.on("M_MANAGE"))),
            ("canDelete", Value::Bool(py_.env.on("M_DELETE"))),
        ],
    );
    let d = py_.post("/cli/mail/access/set-manage", &body, line::MANAGE_CALL)?;
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let who = if p::truthy(g(&d, "isPrimary", line::MANAGE_WHO)?.as_ref()) {
        "primary".to_string()
    } else {
        py_.env("M_WORKSPACE").to_string()
    };
    let l = line::MANAGE_CAPS;
    let m = if p::truthy(g(&d, "canManage", l)?.as_ref()) {
        "manage"
    } else {
        "no-manage"
    };
    let del = if p::truthy(g(&d, "canDelete", l + 1)?.as_ref()) {
        "delete"
    } else {
        "no-delete"
    };
    let a = gs(&d, "address", "", line::MANAGE_PRINT)?;
    py_.print(&format!("set {who} caps on {a}: {m}, {del}"));
    Ok(())
}

/// `oauth_config_show`, `_set` and `_clear`.
fn oauth_config_py(py_: &mut Py, verb: &str) -> R<()> {
    let prov = py_.env("M_PROVIDER").to_string();
    let d = match verb {
        "oauth_config_show" => py_.get("/cli/mail/oauth-config", &[], line::OAUTH_SHOW)?,
        "oauth_config_set" => {
            let mut b = vec![("provider", sv(&prov))];
            for (k, key) in [
                ("M_CLIENT_ID", "clientId"),
                ("M_TENANT", "tenant"),
                ("M_OAUTH_SECRET", "clientSecret"),
            ] {
                if !py_.env(k).is_empty() {
                    b.push((key, sv(py_.env(k))));
                }
            }
            py_.post("/cli/mail/oauth-config/set", &core::obj(b), line::OAUTH_SET)?
        }
        _ => py_.post(
            "/cli/mail/oauth-config/clear",
            &core::obj(vec![("provider", sv(&prov))]),
            line::OAUTH_CLEAR,
        )?,
    };
    if py_.json {
        py_.print_json(&d);
        return Ok(());
    }
    let l = line::OAUTH_PROVS;
    let provs = match g(&d, "providers", l)?.filter(py::truthy) {
        Some(v) => v,
        None => {
            let key = p::get_or(&d, "provider", sv(&prov)).at(l, MODULE)?;
            let key = match key {
                Value::Str(t) => t,
                Value::Array(_) => {
                    return Err(core::raise_at(l, "TypeError: unhashable type: 'list'"))
                }
                Value::Object(_) => {
                    return Err(core::raise_at(l, "TypeError: unhashable type: 'dict'"))
                }
                // Another key never matches "gmail" or "microsoft".
                _ => crate::out::json::Text::from(""),
            };
            Value::Object(vec![(key, d.clone())])
        }
    };
    for name in ["gmail", "microsoft"] {
        let pv = attr_get(&provs, name).at(line::OAUTH_P, MODULE)?;
        let Some(pv) = pv.filter(|v| *v != Value::Null) else {
            continue;
        };
        if !prov.is_empty() && name != prov {
            continue;
        }
        let l = line::OAUTH_LINE;
        let src = attr_get(&pv, "source").at(l, MODULE)?;
        let custom = src.as_ref().is_some_and(|s| p::eq_str(s, "custom"));
        let id = p::or(attr_get(&pv, "clientId").at(l, MODULE)?, sv("?"));
        let configured = p::get_or(&pv, "configured", Value::Bool(true)).at(l, MODULE)?;
        let mut text = format!(
            "{name}: {} app {}{}",
            if custom {
                "your own".to_string()
            } else {
                format!("{PRODUCT_NAME}'s built-in")
            },
            py::str_of(&id),
            if py::truthy(&configured) {
                ""
            } else {
                "  (NOT CONFIGURED — linking will refuse)"
            }
        );
        if name == "gmail" {
            let set = p::truthy(
                attr_get(&pv, "secretSet")
                    .at(line::OAUTH_SECRET, MODULE)?
                    .as_ref(),
            );
            text.push_str(&format!("  secret {}", if set { "set" } else { "not set" }));
        }
        if name == "microsoft"
            && p::truthy(
                attr_get(&pv, "tenant")
                    .at(line::OAUTH_TENANT, MODULE)?
                    .as_ref(),
            )
        {
            let t = p::py_index(&pv, "tenant").at(line::OAUTH_TENANT_LINE, MODULE)?;
            text.push_str(&format!("  tenant {}", py::str_of(&t)));
        }
        py_.print(&text);
    }
    Ok(())
}

/// `sending_get` and `sending_set`: a workspace's agent-send mode.
fn sending_py(py_: &mut Py, set: bool) -> R<()> {
    let ws = py_.env("M_WORKSPACE").to_string();
    if set {
        let body = core::obj(vec![
            ("workspace", sv(&ws)),
            ("agentSend", sv(py_.env("M_AGENT_SEND"))),
        ]);
        py_.post("/cli/mail/config/set", &body, line::SENDING_SET)?;
    }
    let d = py_.get(
        "/cli/mail/config",
        &[("workspace".to_string(), ws.clone())],
        line::SENDING_GET,
    )?;
    let w = p::or(
        g(&d, "workspace", line::SENDING_W)?,
        Value::Object(Vec::new()),
    );
    let a = p::or(
        g(&w, "agentSend", line::SENDING_A)?,
        Value::Object(Vec::new()),
    );
    if py_.json {
        let l = line::SENDING_JSON;
        let null = |v: Option<Value>| v.unwrap_or(Value::Null);
        let out = core::obj(vec![
            ("ok", Value::Bool(true)),
            ("workspace", sv(&ws)),
            ("project", null(g(&w, "project", l)?)),
            ("agentSend", null(g(&a, "effective", l)?)),
            ("override", null(g(&a, "override", l)?)),
            ("default", null(g(&a, "default", l)?)),
            ("changed", Value::Bool(set)),
        ]);
        py_.print_json(&out);
        return Ok(());
    }
    let eff = p::or(g(&a, "effective", line::SENDING_EFF)?, sv("?"));
    let over = g(&a, "override", line::SENDING_SRC)?;
    let src = if matches!(over, None | Some(Value::Null)) {
        "server default"
    } else {
        "set for this workspace"
    };
    let lead = if set {
        format!("agent email sending for {ws} is now {}", py::str_of(&eff))
    } else {
        format!("agent email sending for {ws}: {}", py::str_of(&eff))
    };
    py_.print(&format!("{lead} ({src})"));
    if p::eq_str(&eff, "approval") {
        py_.print(&format!("note: approval queues hosted {PRODUCT_NAME} mailboxes only; a linked inbox (Gmail, Outlook, IMAP) needs 'on' to send"));
    } else if p::eq_str(&eff, "off") {
        py_.print(&format!(
            "agents in this workspace can't send email. Turn it on with: {CLI_NAME} mail sending {ws} on"
        ));
    }
    Ok(())
}
