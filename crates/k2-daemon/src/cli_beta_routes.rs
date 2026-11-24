//! The owner's switch for the new k5 command (Beta).
//!
//! ```text
//! GET  /cli/k5-cli               {"on", "runs": "script"|"binary"|"none", "binary"?, "note"?, "fixed"?}
//! POST /cli/k5-cli {"on": bool}  flips it, re-points the commands, answers as GET plus "links"
//! ```
//!
//! `runs` is what the commands run now (`k2_core::cli_stage::current_choice`):
//! the binary only when the switch is on and this release has it at its own
//! version; `note` says why the script when the switch is on.
//!
//! A build with no shell CLI (`k2_core::cli_stage::NO_BASH`) has nothing
//! to switch to: GET says `"on": true, "fixed": true` (and `runs` is the
//! binary or `none`, never the script), and POST refuses and changes
//! nothing. Our build's answers carry no `fixed` key.
//!
//! The switch is the `k5CliBeta` setting, off by default. Only this route
//! changes it (a settings write refuses the key, `settings_routes`), because
//! a flip also re-points the `k2` and `k5` commands at once: right after the
//! write it runs the same heal the daemon runs at boot ([`relink`]). A
//! settings reset puts it back off and runs the heal too.
//!
//! A flip also changes the command agents are taught
//! (`k2_core::agent_cli`): every workspace's AGENTS.md and K2's own skills
//! are rewritten off the request's thread ([`reteach`]), and a reset that
//! turns it off does the same.
//!
//! Owner only: the owner token or an Owner login. The route table's floor
//! is Owner, so an Admin or Member login is refused there; the handler
//! refuses whatever else reaches it (an agent passport, an App pass, a dead
//! session). The agents' cell socket never serves it.

use k2_core::brand::CLI_NAME;
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
/// the one the heal reads), and what they run. `fixed`: a build with no
/// shell CLI, where the links are on by build.
fn state(fixed: bool) -> Value {
    let mut v = serde_json::to_value(k2_core::cli_stage::current_choice()).unwrap_or_else(|_| json!({}));
    v["on"] = json!(fixed || k2_core::app_settings::k5_cli_beta());
    if fixed {
        v["fixed"] = json!(true);
    }
    v
}

/// The route. `by`: the caller when it is the owner (the dispatcher's
/// `owner_role_identity`), else `None`.
/// The TCP door's one arm for the k5 command's two routes
/// (`routes/dispatcher.rs`): [`crate::cli_usage::ROUTE`], the owner's usage
/// counts, and [`PATH`], the owner's "New k5 command (Beta)" switch.
pub(crate) async fn serve_k5_tcp(path: &str, stream: &mut tokio::net::TcpStream, buf: &mut [u8], query: &str, owner_token: &str, is_post: bool) {
    if path == crate::cli_usage::ROUTE {
        crate::cli_usage::serve_tcp(stream, buf, query, owner_token).await
    } else {
        serve_tcp(stream, buf, query, owner_token, is_post).await
    }
}

/// The TCP door's arm for [`PATH`]: the body is
/// read for the owner only (else drained), [`handle`] runs off the async
/// threads, and its answer goes back.
pub(crate) async fn serve_tcp(stream: &mut tokio::net::TcpStream, buf: &mut [u8], query: &str, owner_token: &str, is_post: bool) {
    use tokio::io::AsyncReadExt;
    let by = crate::routes::http::owner_role_identity(query, owner_token);
    let body = if is_post && by.is_some() {
        crate::routes::http::read_post_body(stream, buf).await
    } else {
        let _ = stream.read(buf).await;
        Vec::new()
    };
    let r = tokio::task::spawn_blocking(move || handle(by.as_deref(), is_post, &body))
        .await
        .unwrap_or_else(CliResponse::internal_error);
    crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
}

pub fn handle(by: Option<&str>, is_post: bool, body: &[u8]) -> CliResponse {
    handle_with(
        by,
        is_post,
        body,
        k2_core::cli_stage::NO_BASH,
        relink,
        reteach,
    )
}

/// [`handle`] with the build kind (`fixed`: no shell CLI, so the switch is
/// on by build), the heal and the reteach passed in, so tests prove either
/// build's answers and never touch a real bin folder or another test's
/// workspaces.
pub(crate) fn handle_with(
    by: Option<&str>,
    is_post: bool,
    body: &[u8],
    fixed: bool,
    relink: impl FnOnce() -> Value,
    reteach: impl FnOnce(),
) -> CliResponse {
    let Some(by) = by else {
        if fixed {
            return refuse(
                "403 Forbidden",
                "owner_only",
                &format!("Only the owner of this server sees or sets how {CLI_NAME} runs."),
            );
        }
        return refuse(
            "403 Forbidden",
            "owner_only",
            "Only the owner of this server turns the new k5 command on or off.",
        );
    };
    if !is_post {
        return CliResponse::ok_json(state(fixed).to_string());
    }
    if fixed {
        return refuse(
            "403 Forbidden",
            "fixed_in_this_build",
            &format!(
                "This build has no shell CLI, so {CLI_NAME} always runs its own commands: there is nothing to switch."
            ),
        );
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
    let mut answer = state(false);
    answer["links"] = relink();
    if was != on {
        // The links changed under the kept "runs k5" answer: drop it, then
        // teach agents the command they type now.
        k2_core::agent_cli::forget();
        reteach();
    }
    CliResponse::ok_json(answer.to_string())
}

/// Rewrite every workspace's AGENTS.md and K2's own skills for the command
/// agents type now (`k2_core::workspace::skill_regen::sync_cli_name_teaching_everywhere`).
/// Off the request's thread: it recomposes every workspace. Never in a
/// test build, where it would rewrite other tests' workspaces.
pub fn reteach() {
    if cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new().name("k5-cli-reteach".into()).spawn(|| {
        let n = k2_core::workspace::skill_regen::sync_cli_name_teaching_everywhere();
        k2_core::log_debug!("[k5-cli] agents' command is {} in {n} workspace(s)", k2_core::agent_cli::name());
    });
}

/// Re-point the commands now, as the boot does: the Linux stage
/// (`k2_core::cli_stage::stage_cli_for`) and, when the links run the k5
/// binary, its PATH line (`k2_core::cli_install::boot_path_for_staged_as`),
/// and the macOS heal
/// (`k2_core::cli_install::boot_heal`). One line per thing it did. Never in
/// a test build: those run as root on CI, where a stage would write the
/// real `/usr/local/bin`.
pub fn relink() -> Value {
    if cfg!(test) {
        return json!(["skipped: test build"]);
    }
    let mut lines: Vec<String> = Vec::new();
    let choice = k2_core::cli_stage::current_choice();
    match k2_core::cli_stage::stage_cli_for(&choice) {
        Ok(Some(path)) => {
            lines.push(format!("staged {}", path.display()));
            // With the binary, a K2 script earlier on PATH (a distro
            // package's /usr/bin/k2) would hide the links: the PATH line
            // now, not at the next boot. With the script, as before.
            #[cfg(unix)]
            match choice.binary().and_then(|_| k2_core::cli_install::boot_path_for_staged_as(&path, &choice)) {
                Some(Ok(k2_core::cli_install::PathOutcome::BlockAdded { file })) => {
                    lines.push(format!("added the PATH line to {}", file.display()))
                }
                Some(Ok(k2_core::cli_install::PathOutcome::Skipped { reason })) => {
                    lines.push(format!("PATH line not added: {reason}"))
                }
                Some(Err(e)) => lines.push(format!("couldn't add the PATH line: {e}")),
                _ => {}
            }
        }
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
        k2_core::agent_cli::forget();
        let n = k2_core::workspace::skill_regen::sync_cli_name_teaching_everywhere();
        k2_core::log_debug!("[k5-cli] agents' command is {} in {n} workspace(s)", k2_core::agent_cli::name());
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
        call_counting(query, is_post, payload, heals, &std::cell::Cell::new(0))
    }

    /// [`call`], also counting the reteach runs.
    fn call_counting(
        query: &str,
        is_post: bool,
        payload: &str,
        heals: &std::cell::Cell<u32>,
        reteaches: &std::cell::Cell<u32>,
    ) -> CliResponse {
        call_in(false, query, is_post, payload, heals, reteaches)
    }

    /// [`call_counting`] in a build that carries the shell CLI or not
    /// (`fixed`).
    fn call_in(
        fixed: bool,
        query: &str,
        is_post: bool,
        payload: &str,
        heals: &std::cell::Cell<u32>,
        reteaches: &std::cell::Cell<u32>,
    ) -> CliResponse {
        let by = crate::routes::http::owner_role_identity(query, OWNER);
        handle_with(
            by.as_deref(),
            is_post,
            payload.as_bytes(),
            fixed,
            || {
                heals.set(heals.get() + 1);
                json!(["healed"])
            },
            || reteaches.set(reteaches.get() + 1),
        )
    }

    /// The live build's GET, as the route serves it: ours is byte for byte
    /// today's (no `fixed` key); one with no shell CLI says on and fixed,
    /// and never runs the script.
    #[test]
    fn get_in_this_build_has_its_shape() {
        crate::test_support::with_temp_home(|| {
            let r = handle(Some("owner-token"), false, b"");
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let v = body(&r);
            if k2_core::cli_stage::NO_BASH {
                assert_eq!(
                    (&v["on"], &v["fixed"]),
                    (&json!(true), &json!(true)),
                    "{}",
                    r.body
                );
                assert_ne!(v["runs"], "script", "{}", r.body);
            } else {
                assert_eq!(r.body, r#"{"note":null,"on":false,"runs":"script"}"#);
            }
        });
    }

    /// A build with no shell CLI: GET says on and fixed (the switch's own
    /// setting stays off), and POST refuses whatever it asks, saves
    /// nothing, audits nothing and runs no heal and no reteach.
    #[test]
    fn no_bash_get_is_fixed_on_and_post_refuses() {
        crate::test_support::with_temp_home(|| {
            let heals = std::cell::Cell::new(0);
            let reteaches = std::cell::Cell::new(0);
            let q = format!("token={OWNER}");
            let r = call_in(true, &q, false, "", &heals, &reteaches);
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert_eq!(body(&r)["on"], json!(true));
            assert_eq!(body(&r)["fixed"], json!(true));
            for payload in [r#"{"on": true}"#, r#"{"on": false}"#, "not json"] {
                let r = call_in(true, &q, true, payload, &heals, &reteaches);
                assert_eq!(r.status, "403 Forbidden", "{payload}: {}", r.body);
                assert_eq!(body(&r)["error"], "fixed_in_this_build");
                assert!(
                    body(&r)["hint"]
                        .as_str()
                        .is_some_and(|h| h.contains("nothing to switch")),
                    "{}",
                    r.body
                );
            }
            assert_eq!(heals.get(), 0, "a refusal never heals");
            assert_eq!(reteaches.get(), 0, "nor reteaches");
            assert!(
                !k2_core::app_settings::load().k5_cli_beta,
                "and never writes the setting"
            );
            let flips = k2_core::auth_audit::tail(1000).expect("audit tail");
            assert!(
                !flips.iter().any(|v| v["event"] == "k5_cli.switch"),
                "{flips:?}"
            );
            // A non-owner is still refused as before, not told it is fixed.
            let r = call_in(true, "", true, r#"{"on": true}"#, &heals, &reteaches);
            assert_eq!(body(&r)["error"], "owner_only");
            // Ours: no fixed key.
            assert_eq!(
                body(&call_in(false, &q, false, "", &heals, &reteaches)).get("fixed"),
                None
            );
        });
    }

    /// K2's ruling: a build with no shell CLI never says k5 in what this
    /// route tells the owner (the POST refusal, the owner-only refusal, the
    /// GET note); it names the brand's command. The brand's own name is
    /// taken out first, so the test holds for any brand, xy too.
    #[test]
    fn no_bash_words_name_the_brand_not_k5() {
        let says_k5 = |text: &str| {
            let rest = text.replace(CLI_NAME, "");
            rest.contains("k5") || rest.contains("K5")
        };
        crate::test_support::with_temp_home(|| {
            let heals = std::cell::Cell::new(0);
            let reteaches = std::cell::Cell::new(0);
            let q = format!("token={OWNER}");
            let post = call_in(true, &q, true, r#"{"on": false}"#, &heals, &reteaches);
            let hint = body(&post)["hint"].as_str().expect("a hint").to_string();
            assert_eq!(
                hint,
                format!("This build has no shell CLI, so {CLI_NAME} always runs its own commands: there is nothing to switch.")
            );
            for r in [
                call_in(true, "", false, "", &heals, &reteaches),
                call_in(true, "", true, r#"{"on": true}"#, &heals, &reteaches),
            ] {
                assert_eq!(body(&r)["error"], "owner_only", "{}", r.body);
                let hint = body(&r)["hint"].as_str().expect("a hint").to_string();
                assert!(hint.contains(CLI_NAME) && !says_k5(&hint), "{hint}");
            }
            let get = call_in(true, &q, false, "", &heals, &reteaches);
            assert!(!says_k5(&get.body.replace("k5-cli", "")), "{}", get.body);
        });
        // Each `runs` a no-bash GET can carry, with its note.
        let bin = std::path::PathBuf::from("/opt/globex/bin/k5-cli");
        for (runs, choice) in [
            (
                "binary",
                k2_core::cli_install::Choice::from_found_in(true, true, Ok(bin.clone())),
            ),
            (
                "none",
                k2_core::cli_install::Choice::from_found_in(
                    true,
                    true,
                    k2_core::cli_install::check_binary_in(true, &bin, Some("0.0.1"))
                        .map(|_| bin.clone()),
                ),
            ),
        ] {
            let v = serde_json::to_value(&choice).expect("a choice is JSON");
            assert_eq!(v["runs"], runs, "{v}");
            if let Some(note) = v.get("note").and_then(Value::as_str) {
                // The binary's path is a fact, not prose: the shipped program is
                // named k5-cli in every brand (machine.binary_prefix and
                // scripts/brand/shipped-bins-linux.txt equal ours by design), so
                // the path is taken out before the words are read.
                let words = note.replace(&bin.display().to_string(), "");
                assert!(
                    note.contains(CLI_NAME) && !says_k5(&words),
                    "{runs}: {note}"
                );
            }
        }
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

    /// A flip teaches agents the command they type now, once per flip; a
    /// write of the value it already has, a read or a refused caller don't.
    #[test]
    fn a_flip_reteaches_agents_and_nothing_else_does() {
        crate::test_support::with_temp_home(|| {
            let heals = std::cell::Cell::new(0);
            let reteaches = std::cell::Cell::new(0);
            let q = format!("token={OWNER}");
            call_counting("token=not-a-session", true, r#"{"on": true}"#, &heals, &reteaches);
            call_counting(&q, false, "", &heals, &reteaches);
            assert_eq!(reteaches.get(), 0, "a refusal and a read never reteach");
            call_counting(&q, true, r#"{"on": true}"#, &heals, &reteaches);
            assert_eq!(reteaches.get(), 1, "off to on");
            call_counting(&q, true, r#"{"on": true}"#, &heals, &reteaches);
            assert_eq!(reteaches.get(), 1, "on to on is not a flip");
            call_counting(&q, true, r#"{"on": false}"#, &heals, &reteaches);
            assert_eq!(reteaches.get(), 2, "on to off");
            assert_eq!(heals.get(), 3, "every owner write heals");
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
