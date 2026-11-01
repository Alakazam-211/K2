//! The Board core (plan-board-garden-v1 §1.4; Rosson 2026-11-01): a Beta,
//! off by default, that opens as a Garden (`CoreView::Garden`).
//!
//! - **Data** (daemon-first): while the core is off, every Garden data call
//!   on the Board widget's space (`data:k2:board`) answers `403 core_off`,
//!   from either door (`/cli/zen/call` for the widget, `/cli/zen/data/*`
//!   for `k2 store --widget k2:board`), and `k2 board` refuses the same
//!   way. The data is kept: turning the core off deletes nothing, and it is
//!   all back when the core is on again. Grants and the person's export and
//!   import still work (they move no board content into a widget).
//! - **Adding a Board Garden** (New Garden, `garden/new {template: board}`,
//!   or `garden/template`) is the opt-in: the person's request carries
//!   `turnOnCore: "board"` after New Garden's one confirm, and K2 turns the
//!   core on in the same step. Without it, the answer is `409 core_off`
//!   with `core`, so a client can ask and send again. An agent can't flip a
//!   core: it gets `403 core_off` and asks its human.

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// The Board widget's data space.
pub const BOARD_SPACE: &str = "data:k2:board";

/// The body key New Garden sends after its one confirm.
pub const TURN_ON_KEY: &str = "turnOnCore";

/// Whether the Board core is on now.
pub fn on() -> bool {
    crate::docs::cores::is_on(k2_core::docs_cores::BOARD)
}

/// The refusal for a Board data call while the core is off.
pub fn data_refusal() -> k2_core::garden_data::DataError {
    k2_core::garden_data::DataError::Refused {
        code: "core_off",
        status: 403,
        hint: "the Board core is off on this computer, so Board's data is closed (it is kept). The person turns it on in Settings → Cores → Multiplayer (k2 doc cores on board), or by adding a Board Garden. Agents: ask your human (k2 thread).".into(),
    }
}

/// Garden data on `space` is reachable now: always, except the Board's
/// while its core is off.
pub fn check_space(space: &str) -> Result<(), k2_core::garden_data::DataError> {
    if space == BOARD_SPACE && !on() {
        Err(data_refusal())
    } else {
        Ok(())
    }
}

fn refusal(status: &'static str, core: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": { "code": "core_off", "core": core, "hint": hint, "exit": 3 },
        })
        .to_string(),
    }
}

/// `garden/new` and `garden/template`: when the template needs a core
/// that's off, turn it on for the person who asked for it (`turnOnCore`),
/// else refuse. Answers the body to pass on (with `turnOnCore` taken out,
/// so the strict body types never see it), or the refusal.
pub fn gate_template(owner_token: bool, caller: &Caller, body: &[u8]) -> Result<Option<Vec<u8>>, CliResponse> {
    let Ok(Value::Object(mut obj)) = serde_json::from_slice::<Value>(body) else { return Ok(None) };
    let asked = obj.remove(TURN_ON_KEY);
    let template = obj.get("template").and_then(Value::as_str).unwrap_or("");
    let Some(core) = k2_core::zen::garden_catalog::core_for_template(template) else {
        // No core needed: a stray `turnOnCore` is refused, not ignored.
        if asked.is_some() {
            return Err(refusal(
                "400 Bad Request",
                "",
                format!("{TURN_ON_KEY} goes only with a template that needs a core ('{template}' needs none)"),
            ));
        }
        return Ok(None);
    };
    let stripped = || serde_json::to_vec(&Value::Object(obj.clone())).map(Some).unwrap_or(None);
    if crate::docs::cores::is_on(core) {
        return Ok(stripped());
    }
    let name = k2_core::docs_cores::by_id(core).map(|c| c.name).unwrap_or(core);
    if !owner_token {
        let who = match caller {
            Caller::Agent(_) | Caller::Shell { .. } => "an agent can't turn a core on: ask your human (k2 thread)",
            _ => "only the person at this computer turns a core on",
        };
        return Err(refusal(
            "403 Forbidden",
            core,
            format!("{name} is a Beta that is off on this computer; {who}. They add the Garden in New Garden (it turns the core on) or run k2 doc cores on {core}."),
        ));
    }
    if asked.as_ref().and_then(Value::as_str) != Some(core) {
        return Err(refusal(
            "409 Conflict",
            core,
            format!("{name} is a Beta that is off. Adding this Garden turns it on: send again with \"{TURN_ON_KEY}\": \"{core}\" after the person confirms."),
        ));
    }
    match crate::docs::cores::turn_on(core, "owner-token") {
        Ok(_) => Ok(stripped()),
        Err(e) => Err(refusal("500 Internal Server Error", core, format!("couldn't turn {name} on: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(r: &CliResponse) -> (String, String) {
        let v: Value = serde_json::from_str(&r.body).expect("json");
        (r.status.to_string(), v["error"]["code"].as_str().unwrap_or("").to_string())
    }

    /// Off: the person's request without the confirm is 409, an agent's is
    /// 403; with the confirm the core turns on and the key is taken out of
    /// the body; a template that needs no core passes untouched and a
    /// stray `turnOnCore` is refused.
    #[test]
    fn adding_a_board_garden_is_the_opt_in() {
        let _m = crate::docs::cores::tests::mirror_lock();
        crate::test_support::with_temp_home(|| {
            crate::docs::cores::tests::all_off();
            let body = br#"{"name":"Ideas","template":"board"}"#;
            let r = gate_template(true, &Caller::Owner, body).expect_err("off, not confirmed");
            assert_eq!(code(&r), ("409 Conflict".into(), "core_off".into()));
            assert!(!on());
            let agent = crate::sidecar_routes::Caller::Shell { project_id: "w-1".into() };
            let r = gate_template(false, &agent, br#"{"name":"Ideas","template":"k2.board@1","turnOnCore":"board"}"#)
                .expect_err("an agent never flips a core");
            assert_eq!(code(&r), ("403 Forbidden".into(), "core_off".into()));
            assert!(!on());
            let out = gate_template(true, &Caller::Owner, br#"{"name":"Ideas","template":"board","turnOnCore":"board"}"#)
                .expect("confirmed")
                .expect("a body without the key");
            let v: Value = serde_json::from_slice(&out).expect("json");
            assert_eq!(v, json!({"name": "Ideas", "template": "board"}));
            assert!(on(), "the core is on in the same step");
            assert!(k2_core::docs_cores::is_on(&k2_core::app_settings::load().multiplayer_cores, "board"), "and stored");
            // On: passes, no confirm needed.
            assert!(gate_template(true, &Caller::Owner, body).expect("on").is_some());
            assert!(gate_template(false, &agent, body).expect("on: an agent may add it").is_some());
            // No core needed.
            assert_eq!(gate_template(true, &Caller::Owner, br#"{"name":"K","template":"kanban"}"#).expect("ok"), None);
            let r = gate_template(true, &Caller::Owner, br#"{"name":"K","template":"kanban","turnOnCore":"board"}"#)
                .expect_err("stray key");
            assert_eq!(code(&r).0, "400 Bad Request");
            crate::docs::cores::tests::all_off();
        });
    }

    #[test]
    fn board_data_is_closed_while_the_core_is_off_and_others_never_are() {
        let _m = crate::docs::cores::tests::mirror_lock();
        crate::docs::cores::tests::all_off();
        let e = check_space(BOARD_SPACE).expect_err("off");
        assert_eq!(e.code(), "core_off");
        check_space("data:k2:kanban").expect("other spaces are never gated");
        crate::docs::cores::set_for_test("board", true);
        check_space(BOARD_SPACE).expect("on");
        crate::docs::cores::tests::all_off();
    }
}
