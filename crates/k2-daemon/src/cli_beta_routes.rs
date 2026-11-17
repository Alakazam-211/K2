//! The owner's switch for the new k5 command (Beta).
//!
//! ```text
//! GET  /cli/k5-cli               {"on", "runs": "script"|"binary", "binary"?, "note"?}
//! POST /cli/k5-cli {"on": bool}  flips it, re-points the commands, answers as GET plus "links"
//! ```
//!
//! `runs` is what the commands run now (`k2_core::cli_stage::current_choice`):
//! the binary only when the switch is on and this release has it at its own
//! version; `note` says why the script when the switch is on.
//!
//! The switch is the `k5CliBeta` setting, off by default. Only this route
//! changes it (a settings write refuses the key, `settings_routes`), because
//! a flip also re-points the `k2` and `k5` commands at once: right after the
//! write it runs the same heal the daemon runs at boot ([`relink`]). A
//! settings reset puts it back off and runs the heal too.
//!
//! Owner only: the owner token or an Owner login. The route table's floor
//! is Owner, so an Admin or Member login is refused there; the handler
//! refuses whatever else reaches it (an agent passport, an App pass, a dead
//! session). The agents' cell socket never serves it.

use serde_json::{json, Value};

use crate::cli_response::CliResponse;

pub const PATH: &str = "/cli/k5-cli";
/// The setting's wire name.
pub const SETTING_KEY: &str = "k5CliBeta";

fn refuse(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "error": code, "hint": hint }).to_string(),
    }
}

/// The switch as the commands see it (the client view of `settings.json`,
/// the one the heal reads), and what they run.
fn state() -> Value {
    let mut v = serde_json::to_value(k2_core::cli_stage::current_choice()).unwrap_or_else(|_| json!({}));
    v["on"] = json!(k2_core::app_settings::k5_cli_beta());
    v
}

/// The route. `by`: the caller when it is the owner (the dispatcher's
/// `owner_role_identity`), else `None`.
pub fn handle(by: Option<&str>, is_post: bool, body: &[u8]) -> CliResponse {
    handle_with(by, is_post, body, relink)
}

/// [`handle`] with the heal passed in, so tests never touch a real bin folder.
pub(crate) fn handle_with(by: Option<&str>, is_post: bool, body: &[u8], relink: impl FnOnce() -> Value) -> CliResponse {
    let Some(by) = by else {
        return refuse(
            "403 Forbidden",
            "owner_only",
            "Only the owner of this server turns the new k5 command on or off.",
        );
    };
    if !is_post {
        return CliResponse::ok_json(state().to_string());
    }
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(on) = parsed.get("on").and_then(Value::as_bool) else {
        return CliResponse::bad_request("the body is {\"on\": true} or {\"on\": false}");
    };
    let was = k2_core::app_settings::k5_cli_beta();
    if let Err(e) = k2_core::app_settings::update(json!({ SETTING_KEY: on })) {
        return CliResponse::internal_error(format!("save the setting: {e}"));
    }
    if was != on {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "k5_cli.switch",
            by,
            if on { "on" } else { "off" },
            "-",
            "-",
            "cli",
        ));
    }
    let mut answer = state();
    answer["links"] = relink();
    CliResponse::ok_json(answer.to_string())
}

/// Re-point the commands now, as the boot does: the Linux stage
/// (`k2_core::cli_stage::stage_cli`) and the macOS heal
/// (`k2_core::cli_install::boot_heal`). One line per thing it did. Never in
/// a test build: those run as root on CI, where a stage would write the
/// real `/usr/local/bin`.
pub fn relink() -> Value {
    if cfg!(test) {
        return json!(["skipped: test build"]);
    }
    let mut lines: Vec<String> = Vec::new();
    match k2_core::cli_stage::stage_cli() {
        Ok(Some(path)) => lines.push(format!("staged {}", path.display())),
        Ok(None) => {}
        Err(e) => lines.push(format!("couldn't stage the command: {e}")),
    }
    #[cfg(unix)]
    if let Ok(exe) = std::env::current_exe() {
        match k2_core::cli_install::boot_heal(&exe) {
            Some(Ok(report)) => lines.extend(report.log_lines()),
            Some(Err(e)) => lines.push(format!("couldn't link the command: {e}")),
            None => {}
        }
    }
    json!(lines)
}

/// After a settings reset: the switch is back off, so a heal puts the
/// script back when it was on. Off the request's thread.
pub fn after_reset(was_on: bool) {
    if !was_on || cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new().name("k5-cli-relink".into()).spawn(|| {
        let _ = relink();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::route_policy::{lookup, role_gate};
    use k2_core::connect_users::{self, Role};

    const OWNER: &str = "k5-cli-route-owner-token";

    fn session(name: &str, role: Role) -> String {
        connect_users::add_user(name, "password123").expect("add_user");
        if role != Role::Member {
            connect_users::set_role(name, role).expect("set_role");
        }
        connect_users::create_session(name)
    }

    fn body(r: &CliResponse) -> Value {
        serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
    }

    /// The handler as the dispatcher calls it: the caller resolved by
    /// `owner_role_identity`, a heal that only counts its runs.
    fn call(query: &str, is_post: bool, payload: &str, heals: &std::cell::Cell<u32>) -> CliResponse {
        let by = crate::routes::http::owner_role_identity(query, OWNER);
        handle_with(by.as_deref(), is_post, payload.as_bytes(), || {
            heals.set(heals.get() + 1);
            json!(["healed"])
        })
    }

    #[test]
    fn the_row_is_owner_only_for_both_methods() {
        let row = lookup(PATH).expect("/cli/k5-cli is classified");
        for (method, floor) in [("GET", row.get), ("POST", row.post)] {
            let floor = floor.unwrap_or_else(|| panic!("{method} is served"));
            assert_eq!(floor.as_wire(), "owner", "{method}");
            assert!(!floor.admits(Role::Member), "{method}");
            assert!(!floor.admits(Role::Admin), "{method}");
            assert!(floor.admits(Role::Owner), "{method}");
        }
    }

    #[test]
    fn off_by_default_and_only_the_owner_reads_or_flips_it() {
        crate::test_support::with_temp_home(|| {
            let member = session("k5_member", Role::Member);
            let admin = session("k5_admin", Role::Admin);
            let owner = session("k5_owner", Role::Owner);
            let heals = std::cell::Cell::new(0);

            // The route table refuses Member and Admin logins before the handler.
            for (who, tok) in [("member", &member), ("admin", &admin)] {
                for method in ["GET", "POST"] {
                    let r = role_gate(method, PATH, &format!("token={tok}"), OWNER)
                        .unwrap_or_else(|| panic!("a {who} login must be refused on {method}"));
                    assert_eq!(r.status, "403 Forbidden", "{who} {method}: {}", r.body);
                    assert_eq!(body(&r)["error"], "role_required");
                }
            }
            assert!(role_gate("POST", PATH, &format!("token={owner}"), OWNER).is_none());

            // The handler refuses every non-owner credential that passes the table.
            for query in [
                "token=sid-123.secret-abc".to_string(),
                "token=k2skn_abc".to_string(),
                "token=not-a-session".to_string(),
                String::new(),
                format!("token={member}"),
                format!("token={admin}"),
            ] {
                for is_post in [false, true] {
                    let r = call(&query, is_post, r#"{"on": true}"#, &heals);
                    assert_eq!(r.status, "403 Forbidden", "{query:?} post={is_post}: {}", r.body);
                    assert_eq!(body(&r)["error"], "owner_only");
                }
            }
            assert_eq!(heals.get(), 0, "a refused caller never runs the heal");
            assert!(!k2_core::app_settings::load().k5_cli_beta, "and never writes the setting");

            // Off by default, for the owner token and an Owner login.
            for q in [format!("token={OWNER}"), format!("token={owner}")] {
                let r = call(&q, false, "", &heals);
                assert_eq!(r.status, "200 OK", "{}", r.body);
                assert_eq!(body(&r)["on"], json!(false));
                assert_eq!(body(&r)["runs"], json!("script"), "{}", r.body);
                assert_eq!(body(&r)["note"], Value::Null, "off carries no note");
            }
            assert_eq!(heals.get(), 0, "a read never heals");
        });
    }

    #[test]
    fn a_flip_saves_audits_and_heals_both_ways() {
        crate::test_support::with_temp_home(|| {
            let heals = std::cell::Cell::new(0);
            let q = format!("token={OWNER}");

            let r = call(&q, true, r#"{"on": true}"#, &heals);
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert_eq!(body(&r)["on"], json!(true));
            assert_eq!(body(&r)["links"], json!(["healed"]));
            // A test build has no k5 binary beside it: the script, and why.
            assert_eq!(body(&r)["runs"], json!("script"), "{}", r.body);
            assert!(body(&r)["note"].as_str().is_some_and(|n| !n.is_empty()), "{}", r.body);
            assert!(k2_core::app_settings::load().k5_cli_beta);
            assert!(k2_core::app_settings::k5_cli_beta(), "the heal reads it on");
            assert_eq!(body(&call(&q, false, "", &heals))["on"], json!(true));

            let r = call(&q, true, r#"{"on": false}"#, &heals);
            assert_eq!(body(&r)["on"], json!(false));
            assert!(!k2_core::app_settings::k5_cli_beta());
            assert_eq!(heals.get(), 2, "each flip heals once");

            // A write of the value it already has is not a flip.
            call(&q, true, r#"{"on": false}"#, &heals);
            let flips: Vec<Value> = k2_core::auth_audit::tail(1000)
                .expect("audit tail")
                .into_iter()
                .filter(|v| v["event"] == "k5_cli.switch")
                .collect();
            assert_eq!(flips.len(), 2, "{flips:?}");
            assert_eq!((flips[0]["outcome"].as_str(), flips[0]["user"].as_str()), (Some("on"), Some("owner-token")));
            assert_eq!(flips[1]["outcome"], "off");
        });
    }

    #[test]
    fn a_bad_body_changes_nothing() {
        crate::test_support::with_temp_home(|| {
            let heals = std::cell::Cell::new(0);
            let q = format!("token={OWNER}");
            for bad in ["", "not json", "{}", r#"{"on": "yes"}"#, r#"{"on": 1}"#] {
                let r = call(&q, true, bad, &heals);
                assert_eq!(r.status, "400 Bad Request", "{bad:?}: {}", r.body);
            }
            assert_eq!(heals.get(), 0);
            assert!(!k2_core::app_settings::load().k5_cli_beta);
        });
    }

    #[test]
    fn a_settings_write_refuses_the_key_and_a_reset_turns_it_off() {
        crate::test_support::with_temp_home(|| {
            use crate::settings_routes::{handle_settings_reset, handle_settings_update, SettingsActor};
            let r = handle_settings_update(br#"{"k5CliBeta": true}"#, SettingsActor::Owner);
            assert_eq!(r.status, "403 Forbidden", "{}", r.body);
            assert!(r.body.contains(PATH), "the refusal names the route: {}", r.body);
            assert!(!k2_core::app_settings::load().k5_cli_beta);

            let heals = std::cell::Cell::new(0);
            call(&format!("token={OWNER}"), true, r#"{"on": true}"#, &heals);
            assert!(k2_core::app_settings::load().k5_cli_beta);
            let r = handle_settings_reset(SettingsActor::Owner);
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert!(!k2_core::app_settings::load().k5_cli_beta, "a reset puts it back off");
        });
    }
}
