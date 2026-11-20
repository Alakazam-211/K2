//! `k5 hostchat users …` (`_hostchat_users`, `cli/src/hostchat-voice.sh`):
//! Sew's own logins on this box: list, owner, invite, reset, revoke,
//! reactivate and role. A setup link prints on stdout only.

use super::main_verbs::show;
use super::{cj, usage, Ctx, Plan};
use crate::exit::Exit;
use crate::families::voice::sew::Field;
use k2_client::brand::CLI_NAME;

/// A users verb and its flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Users {
    pub sub: String,
    pub json: bool,
    pub display: String,
    pub role: String,
    pub email: String,
    pub pos: Vec<String>,
}

/// The words after `users`.
pub fn parse(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&s, r)) => (s, r),
        None => ("", &[]),
    };
    let mut u = Users {
        sub: sub.to_string(),
        ..Users::default()
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let slot = match w {
            "--json" => {
                u.json = true;
                None
            }
            "--display-name" => Some(&mut u.display),
            "--role" => Some(&mut u.role),
            "--email" => Some(&mut u.email),
            "--admin" => {
                u.role = "admin".into();
                None
            }
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => {
                return Plan::Bad(usage(&format!("unknown flag {w} ({c} hostchat --help)")))
            }
            w => {
                u.pos.push(w.to_string());
                i += 1;
                continue;
            }
        };
        match slot {
            None => i += 1,
            Some(slot) => {
                let Some(v) = rest.get(i + 1).filter(|v| !v.is_empty()) else {
                    return super::needs_value(w);
                };
                *slot = v.to_string();
                i += 2;
            }
        }
    }
    match sub {
        "list" | "" => return Plan::Users(u),
        "help" => return Plan::Help,
        _ => {}
    }
    if u.pos.is_empty() || u.pos[0].is_empty() {
        return Plan::Bad(usage(&format!("{c} hostchat users {sub} <name>")));
    }
    match sub {
        "owner" | "invite" | "reset" | "revoke" | "reactivate" => Plan::Users(u),
        "role" => {
            let r = u.pos.get(1).filter(|r| !r.is_empty()).unwrap_or(&u.role);
            if r.is_empty() {
                return Plan::Bad(usage(&format!(
                    "{c} hostchat users role <name> admin|member"
                )));
            }
            Plan::Users(u)
        }
        _ => Plan::Bad(format!(
            "{{\"error\":\"usage\",\"hint\":\"unknown users command '{sub}' ({c} hostchat --help)\"}}"
        )),
    }
}

fn s(t: &str) -> Field {
    Field::S(t.to_string())
}

pub fn run(ctx: &mut Ctx, u: &Users) -> Exit {
    let user = u.pos.first().map_or("", String::as_str);
    let (route, payload, verb) = match u.sub.as_str() {
        "list" | "" => ("/cli/sew/users", String::new(), "users-list".to_string()),
        sub => {
            let payload = match sub {
                "owner" => cj(&[("user", s(user)), ("displayName", s(&u.display))]),
                "invite" => {
                    let role = if u.role.is_empty() { "member" } else { &u.role };
                    cj(&[
                        ("user", s(user)),
                        ("displayName", s(&u.display)),
                        ("role", s(role)),
                        ("email", s(&u.email)),
                    ])
                }
                "role" => {
                    let r = u.pos.get(1).filter(|r| !r.is_empty()).unwrap_or(&u.role);
                    cj(&[("user", s(user)), ("role", s(r))])
                }
                _ => cj(&[("user", s(user))]),
            };
            (
                match sub {
                    "owner" => "/cli/sew/users/owner",
                    "invite" => "/cli/sew/users/invite",
                    "reset" => "/cli/sew/users/reset",
                    "revoke" => "/cli/sew/users/revoke",
                    "reactivate" => "/cli/sew/users/reactivate",
                    _ => "/cli/sew/users/role",
                },
                payload,
                format!("users-{sub}"),
            )
        }
    };
    let post = !route.ends_with("/users");
    match ctx.call(post, route, &payload, false) {
        Ok(body) => show(&body, &verb, u.json),
        Err(e) => e,
    }
}
