//! What one inbox can do, for the caller (prd-app-mail-calendar-surfaces-v1
//! §3.1 "Capabilities (per inbox)", slice MCS1).
//!
//! Every `k2 mail inboxes` row carries a `capabilities` object, and
//! `GET /cli/mail/capabilities[?address=]` answers it on its own. It is
//! derived — never stored — from four things:
//!   1. the backend (hosted Stalwart, linked IMAP with an app password,
//!      linked Google, linked Microsoft Graph) and what K2 has built for it;
//!   2. the scopes the provider actually granted (`granted_scopes`, 0188);
//!   3. the owner's switches (the hosted calendar policy);
//!   4. the caller's access level (agent view) — the owner view reports what
//!      the inbox itself can do.
//!
//! A capability that is off names why in `missing`: `{cap, reason, fix}`,
//! with reason one of `backend_unsupported`, `scope_not_granted`, `level`,
//! `owner_disabled`, `not_built`. The masking rule stays (CAL19): this is
//! only ever computed for inboxes the caller can already see.

use crate::mail::access::{Level, Source};
use crate::mail::external::OauthMeta;
use crate::mail::oauth::{self, OauthProvider};

/// The facts about one inbox the derivation needs (all non-secret).
pub struct CapInput<'a> {
    pub source: Source,
    pub address: &'a str,
    /// Linked only: `imap` | `graph`.
    pub kind: Option<&'a str>,
    /// Linked only: `password` | `oauth`.
    pub auth_kind: Option<&'a str>,
    /// Linked OAuth only: `gmail` | `microsoft`.
    pub provider: Option<&'a str>,
    /// Linked only: the 0188 columns.
    pub meta: Option<&'a OauthMeta>,
    /// The caller's effective level (the owner view passes the source max).
    pub level: Level,
    /// The owner's hosted calendar policy (S2 DAV policy) is on.
    pub hosted_calendars_on: bool,
}

/// The backend family one inbox rides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Hosted,
    Imap,
    Google,
    Microsoft,
}

impl Backend {
    fn of(input: &CapInput) -> Backend {
        if input.source == Source::Hosted {
            return Backend::Hosted;
        }
        let oauth = input.auth_kind == Some(crate::mail::external::AUTH_OAUTH);
        match (input.kind, oauth, input.provider) {
            (Some("graph"), _, _) => Backend::Microsoft,
            (_, true, Some("gmail")) => Backend::Google,
            (_, true, Some("microsoft")) => Backend::Microsoft,
            _ => Backend::Imap,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Backend::Hosted => "hosted",
            Backend::Imap => "imap",
            Backend::Google => "google",
            Backend::Microsoft => "microsoft",
        }
    }
}


/// One capability's verdict: on, or off with a reason + fix.
struct Verdict {
    on: bool,
    reason: &'static str,
    fix: String,
}

impl Verdict {
    fn on() -> Verdict {
        Verdict {
            on: true,
            reason: "",
            fix: String::new(),
        }
    }
    fn off(reason: &'static str, fix: impl Into<String>) -> Verdict {
        Verdict {
            on: false,
            reason,
            fix: fix.into(),
        }
    }
    /// Gate an on verdict on the caller's level.
    fn needs(self, have: Level, want: Level, address: &str) -> Verdict {
        if !self.on || have >= want {
            return self;
        }
        Verdict::off(
            "level",
            format!(
                "your workspace has '{}' on {address}; this needs '{}' — the owner raises it \
                 with 'k2 mail access set-level'",
                have.as_str(),
                want.as_str()
            ),
        )
    }
}

fn upgrade_fix(address: &str, feature: &str) -> String {
    format!("k2 mail link upgrade {address} --features {feature} (owner)")
}

/// The capabilities object for one inbox (shape: PRD §3.1).
pub fn capabilities(input: &CapInput) -> serde_json::Value {
    let backend = Backend::of(input);
    let address = input.address;
    let granted = input.meta.and_then(|m| m.granted_scopes.as_deref());
    // What the grant covers. A row from before 0188 has no mirrored scopes
    // yet: it was a mail-only link with the pre-MCS1 scope set, which for
    // Microsoft never included Mail.Send or calendars.
    let (mail_scope, send_scope, calendar_scope) = match backend {
        Backend::Google => {
            let f = granted.map(|g| oauth::features_granted(OauthProvider::Gmail, g));
            (
                f.is_none_or(|f| f.mail),
                f.is_none_or(|f| f.mail),
                f.is_some_and(|f| f.calendar),
            )
        }
        Backend::Microsoft => match granted {
            Some(g) => {
                let f = oauth::features_granted(OauthProvider::Microsoft, g);
                (f.mail, oauth::scope_granted(g, "Mail.Send"), f.calendar)
            }
            None => (true, false, false),
        },
        Backend::Hosted | Backend::Imap => (true, true, false),
    };

    // ── mail ──
    let mail_read = if !mail_scope {
        Verdict::off("scope_not_granted", upgrade_fix(address, "mail"))
    } else {
        Verdict::on()
    };
    let draft_backend = match backend {
        Backend::Hosted => Verdict::off(
            "backend_unsupported",
            "drafts are saved into linked inboxes only — a hosted inbox sends (or queues for \
             approval) with 'k2 mail send'",
        ),
        _ if !mail_scope => Verdict::off("scope_not_granted", upgrade_fix(address, "mail")),
        _ => Verdict::on(),
    };
    let compose_backend = match backend {
        Backend::Hosted => Verdict::off(
            "backend_unsupported",
            "drafts are saved into linked inboxes only — a hosted inbox sends with 'k2 mail send'",
        ),
        _ if !mail_scope => Verdict::off("scope_not_granted", upgrade_fix(address, "mail")),
        _ => Verdict::on(),
    };
    // MCS2: Microsoft sends over Graph (`sendMail` / createReply → send)
    // once `Mail.Send` is granted (a pre-MCS1 link never asked for it).
    let send_backend = if send_scope {
        Verdict::on()
    } else {
        Verdict::off("scope_not_granted", upgrade_fix(address, "mail"))
    };
    let level = input.level;
    let mail = [
        ("read", mail_read.needs(level, Level::Read, address)),
        ("draft", draft_backend.needs(level, Level::Draft, address)),
        (
            "compose",
            compose_backend.needs(level, Level::Draft, address),
        ),
        ("send", send_backend.needs(level, Level::Send, address)),
    ];

    // ── calendar ──
    let cal_base = |want: Level| -> Verdict {
        let v = match backend {
            Backend::Hosted if !input.hosted_calendars_on => Verdict::off(
                "owner_disabled",
                "the owner turned calendars off for hosted inboxes — 'k2 hostmail calendar \
                 enable' (owner)",
            ),
            Backend::Hosted => Verdict::on(),
            Backend::Imap => Verdict::off(
                "backend_unsupported",
                "an IMAP inbox linked with an app password has no calendar API",
            ),
            Backend::Google | Backend::Microsoft if !calendar_scope => {
                Verdict::off("scope_not_granted", upgrade_fix(address, "calendar"))
            }
            Backend::Google | Backend::Microsoft => Verdict::off(
                "not_built",
                "calendars on linked Google / Microsoft inboxes are not available in this K2 \
                 version (the consent is already in place)",
            ),
        };
        v.needs(level, want, address)
    };
    let invite_base = |want: Level| -> Verdict {
        let v = match backend {
            Backend::Imap => Verdict::off(
                "backend_unsupported",
                "an IMAP inbox linked with an app password has no calendar API",
            ),
            Backend::Google | Backend::Microsoft if !calendar_scope => {
                Verdict::off("scope_not_granted", upgrade_fix(address, "calendar"))
            }
            _ => Verdict::off(
                "not_built",
                "calendar invites and RSVPs are not available in this K2 version",
            ),
        };
        v.needs(level, want, address)
    };
    let cal_read = cal_base(Level::Read);
    let freebusy = if cal_read.on && backend == Backend::Hosted {
        "box"
    } else {
        "none"
    };
    let calendar = [
        ("read", cal_read),
        ("write", cal_base(Level::Draft)),
        ("invite", invite_base(Level::Send)),
        ("rsvp", invite_base(Level::Send)),
    ];

    let mut missing: Vec<serde_json::Value> = Vec::new();
    let mut mail_obj = serde_json::Map::new();
    for (name, v) in mail {
        if !v.on {
            missing.push(serde_json::json!({
                "cap": format!("mail.{name}"), "reason": v.reason, "fix": v.fix,
            }));
        }
        mail_obj.insert(name.to_string(), serde_json::json!(v.on));
    }
    let mut cal_obj = serde_json::Map::new();
    for (name, v) in calendar {
        if !v.on {
            missing.push(serde_json::json!({
                "cap": format!("calendar.{name}"), "reason": v.reason, "fix": v.fix,
            }));
        }
        cal_obj.insert(name.to_string(), serde_json::json!(v.on));
    }
    cal_obj.insert("freebusy".to_string(), serde_json::json!(freebusy));
    // Invites are not built on any backend yet, so nothing schedules.
    cal_obj.insert("scheduling".to_string(), serde_json::json!("none"));

    let mut out = serde_json::json!({
        "source": backend.label(),
        "mail": mail_obj,
        "calendar": cal_obj,
        "missing": missing,
    });
    if matches!(backend, Backend::Google | Backend::Microsoft) {
        let m = input.meta;
        out["account"] = serde_json::json!({
            "kind": m.and_then(|m| m.account_kind.clone()),
            "tenant": m.and_then(|m| m.account_tenant.clone()),
            "checked": m.and_then(|m| m.account_checked_at).is_some(),
            "features": m.map(|m| m.features.clone()).unwrap_or_else(|| "mail".to_string()),
            "client": m.and_then(|m| m.oauth_client.clone()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(scopes: Option<&str>) -> OauthMeta {
        OauthMeta {
            features: "mail".to_string(),
            granted_scopes: scopes.map(str::to_string),
            account_kind: Some("work".to_string()),
            account_tenant: Some("t-1".to_string()),
            account_checked_at: Some(5),
            oauth_client: Some("baked".to_string()),
            ..Default::default()
        }
    }

    fn missing_for<'a>(v: &'a serde_json::Value, cap: &str) -> Option<&'a serde_json::Value> {
        v["missing"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["cap"] == cap)
    }

    #[test]
    fn hosted_inbox_sends_reads_calendars_and_never_drafts() {
        let v = capabilities(&CapInput {
            source: Source::Hosted,
            address: "a@k2.example",
            kind: None,
            auth_kind: None,
            provider: None,
            meta: None,
            level: Level::Send,
            hosted_calendars_on: true,
        });
        assert_eq!(v["source"], "hosted");
        assert_eq!(v["mail"]["read"], true);
        assert_eq!(v["mail"]["send"], true);
        assert_eq!(v["mail"]["draft"], false);
        assert_eq!(v["calendar"]["read"], true);
        assert_eq!(v["calendar"]["write"], true);
        assert_eq!(v["calendar"]["freebusy"], "box");
        assert_eq!(
            missing_for(&v, "calendar.invite").unwrap()["reason"],
            "not_built"
        );
        assert!(v.get("account").is_none());
    }

    #[test]
    fn owner_calendar_switch_and_the_caller_level_turn_caps_off() {
        let v = capabilities(&CapInput {
            source: Source::Hosted,
            address: "a@k2.example",
            kind: None,
            auth_kind: None,
            provider: None,
            meta: None,
            level: Level::Read,
            hosted_calendars_on: false,
        });
        assert_eq!(v["calendar"]["read"], false);
        assert_eq!(
            missing_for(&v, "calendar.read").unwrap()["reason"],
            "owner_disabled"
        );
        assert_eq!(v["calendar"]["freebusy"], "none");
        assert_eq!(v["mail"]["send"], false);
        let send = missing_for(&v, "mail.send").unwrap();
        assert_eq!(send["reason"], "level");
        assert!(send["fix"].as_str().unwrap().contains("set-level"));
    }

    #[test]
    fn microsoft_mail_only_link_names_the_calendar_upgrade() {
        let scopes = oauth::scope_for(OauthProvider::Microsoft, oauth::Features::MAIL);
        let m = meta(Some(&scopes));
        let v = capabilities(&CapInput {
            source: Source::Linked,
            address: "you@contoso.com",
            kind: Some("graph"),
            auth_kind: Some("oauth"),
            provider: Some("microsoft"),
            meta: Some(&m),
            level: Level::Send,
            hosted_calendars_on: true,
        });
        assert_eq!(v["source"], "microsoft");
        assert_eq!(v["mail"]["read"], true);
        assert_eq!(v["mail"]["draft"], true);
        let cal = missing_for(&v, "calendar.write").unwrap();
        assert_eq!(cal["reason"], "scope_not_granted");
        assert_eq!(
            cal["fix"],
            "k2 mail link upgrade you@contoso.com --features calendar (owner)"
        );
        assert_eq!(v["account"]["kind"], "work");
        assert_eq!(v["account"]["checked"], true);
        // MCS2: Mail.Send granted → Graph send + compose drafts are on.
        assert_eq!(v["mail"]["send"], true);
        assert_eq!(v["mail"]["compose"], true);
        assert!(missing_for(&v, "mail.send").is_none());
    }

    #[test]
    fn a_pre_0188_microsoft_row_lacks_send_scope_and_is_unchecked() {
        let v = capabilities(&CapInput {
            source: Source::Linked,
            address: "old@contoso.com",
            kind: Some("graph"),
            auth_kind: Some("oauth"),
            provider: Some("microsoft"),
            meta: Some(&OauthMeta {
                features: "mail".into(),
                ..Default::default()
            }),
            level: Level::Send,
            hosted_calendars_on: true,
        });
        assert_eq!(v["mail"]["send"], false);
        assert_eq!(v["account"]["checked"], false);
        assert_eq!(v["calendar"]["read"], false);
        assert_eq!(
            missing_for(&v, "calendar.read").unwrap()["reason"],
            "scope_not_granted"
        );
    }

    #[test]
    fn password_imap_has_mail_but_no_calendar_api() {
        let v = capabilities(&CapInput {
            source: Source::Linked,
            address: "me@fastmail.example",
            kind: Some("imap"),
            auth_kind: Some("password"),
            provider: None,
            meta: None,
            level: Level::Draft,
            hosted_calendars_on: true,
        });
        assert_eq!(v["source"], "imap");
        assert_eq!(v["mail"]["draft"], true);
        assert_eq!(v["mail"]["compose"], true);
        assert_eq!(missing_for(&v, "mail.send").unwrap()["reason"], "level");
        assert_eq!(
            missing_for(&v, "calendar.read").unwrap()["reason"],
            "backend_unsupported"
        );
    }

    #[test]
    fn google_calendar_grant_is_recorded_but_not_built_yet() {
        let scopes = oauth::scope_for(
            OauthProvider::Gmail,
            oauth::Features {
                mail: true,
                calendar: true,
            },
        );
        let m = meta(Some(&scopes));
        let v = capabilities(&CapInput {
            source: Source::Linked,
            address: "you@gmail.com",
            kind: Some("imap"),
            auth_kind: Some("oauth"),
            provider: Some("gmail"),
            meta: Some(&m),
            level: Level::Send,
            hosted_calendars_on: true,
        });
        assert_eq!(v["source"], "google");
        assert_eq!(v["mail"]["send"], true);
        assert_eq!(
            missing_for(&v, "calendar.read").unwrap()["reason"],
            "not_built"
        );
    }
}
