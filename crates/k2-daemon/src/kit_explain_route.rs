//! `GET /cli/kit/explain?id=<id>[&json=1]`: the Markdown kit's own page for
//! one of its error ids (MPC-Ennn, MPCE-Ennn), for `k2 explain <kit id>`.
//!
//! The answer is the one `POST /cli/board/file/verb {"argv":["explain",<id>]}`
//! gives with no path (`board_file::routes::meta_verb`), byte for byte, but it
//! doesn't wait for the Board files preview: explaining an error id is
//! documentation and reads no file. So it takes no `path` or `file` and
//! refuses a request that sends one.
//!
//! A read for the owner, a login (the Member floor) and any agent's passport,
//! on TCP and on the cell socket. Never an App pass or an API key.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::docs::caller::refusal;

pub const ROUTE: &str = "/cli/kit/explain";

/// The fields that would name a file. Explain reads none.
const FILE_FIELDS: &[&str] = &["path", "file"];

/// `GET /cli/kit/explain?id=<id>[&json=1]`.
/// The TCP door's arm for [`ROUTE`] (`routes/dispatcher.rs`): a read for
/// any credential but an App pass or an API key; it answers whether or not
/// Board files are on (it reads no file). A POST gets 405 and `true`: the
/// dispatcher ends the connection.
pub(crate) async fn serve_tcp(
    stream: &mut tokio::net::TcpStream,
    buf: &mut [u8],
    is_post: bool,
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) -> bool {
    use tokio::io::AsyncReadExt;
    let _ = stream.read(buf).await;
    if is_post {
        let r = CliResponse::method_not_allowed();
        crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
        return true;
    }
    let who = crate::compute_routes::who_from_tcp(query, bearer, owner_token);
    let params = crate::routes::http::parse_params(path, query);
    let r = handle(&who, &params);
    crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
    false
}

pub fn handle(who: &Who, params: &HashMap<String, String>) -> CliResponse {
    match who {
        Who::Owner | Who::Login { .. } | Who::Agent { .. } | Who::Shell { .. } => {}
        Who::Wrong => {
            return refusal(
                "403 Forbidden",
                k2_core::deploy::codes::WRONG_CREDENTIAL,
                "the kit's explain is for the owner, a login or an agent's passport, never an App pass or an API key",
            )
        }
        Who::Invalid => return CliResponse::forbidden(),
    }
    if let Some(field) = FILE_FIELDS.iter().find(|f| params.contains_key(**f)) {
        return refusal(
            "400 Bad Request",
            "bad_request",
            format!("the kit's explain reads no file: send id (and json), never {field}"),
        );
    }
    let Some(id) = params.get("id").filter(|id| !id.is_empty()) else {
        return refusal("400 Bad Request", "bad_request", "id is required: one of the kit's error ids (MPC-E160)");
    };
    let json_out = matches!(params.get("json").map(String::as_str), Some("1" | "true"));
    let argv = ["explain".to_string(), id.clone()];
    crate::board_file::routes::meta_verb(&argv, json_out, "")
        .unwrap_or_else(|| CliResponse::internal_error("explain is no longer one of the kit's meta verbs"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// A kit id that exists: the kit's own "unknown error id" row.
    const KNOWN: &str = "MPC-E160";

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    fn agent() -> Who {
        Who::Agent { project_id: "ws-acme".into(), session_id: "sid-acme".into() }
    }

    fn json_of(r: &CliResponse) -> Value {
        serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body))
    }

    /// The board route's answer for the same explain, with no path, as
    /// `k2 explain` sent it before this route.
    fn board_verb(id: &str, json_out: bool) -> CliResponse {
        let body = serde_json::json!({ "argv": ["explain", id], "json": json_out }).to_string();
        crate::board_file::routes::handle(
            "POST",
            crate::board_file::routes::VERB,
            &HashMap::new(),
            body.as_bytes(),
            &crate::docs::caller::owner(),
        )
    }

    fn same(a: &CliResponse, b: &CliResponse, what: &str) {
        assert_eq!(a.status, b.status, "{what}: {} vs {}", a.body, b.body);
        assert_eq!(a.content_type, b.content_type, "{what}");
        assert_eq!(a.body, b.body, "{what}");
    }

    #[test]
    fn it_gives_the_board_verbs_text_for_a_known_and_an_unknown_id() {
        let _home = crate::test_support::TempHome::new();
        k2_core::board_preview::set_enabled(true);
        for id in [KNOWN, "MPC-E999", "mpce-E9"] {
            for json_out in [false, true] {
                let mut q = params(&[("id", id)]);
                if json_out {
                    q.insert("json".into(), "1".into());
                }
                let kit = handle(&agent(), &q);
                same(&kit, &board_verb(id, json_out), &format!("{id} json={json_out}"));
            }
        }
        k2_core::board_preview::set_enabled(false);
    }

    #[test]
    fn it_answers_with_board_files_off() {
        let _home = crate::test_support::TempHome::new();
        k2_core::board_preview::set_enabled(false);
        assert!(!k2_core::board_preview::enabled(), "this test needs {} unset", k2_core::board_preview::ENV);
        let board = board_verb(KNOWN, false);
        assert_eq!(board.status, "404 Not Found", "{}", board.body);
        assert_eq!(json_of(&board)["error"]["code"], "not_enabled", "{}", board.body);

        let kit = handle(&agent(), &params(&[("id", KNOWN)]));
        assert_eq!(kit.status, "200 OK", "{}", kit.body);
        let v = json_of(&kit);
        assert_eq!(v["exit"], 0, "{v}");
        let page = v["stdout"].as_str().unwrap_or_else(|| panic!("stdout is the page: {v}"));
        assert!(page.contains(KNOWN), "the page names its id: {page}");

        let kit = handle(&Who::Owner, &params(&[("id", KNOWN), ("json", "1")]));
        assert_eq!(kit.status, "200 OK", "{}", kit.body);
        let v = json_of(&kit);
        let page: Value = serde_json::from_str(v["stdout"].as_str().unwrap_or_else(|| panic!("stdout: {v}")))
            .unwrap_or_else(|e| panic!("json=1 prints the kit's JSON: {e}: {v}"));
        assert_eq!(page["id"], KNOWN, "{page}");
    }

    #[test]
    fn an_unknown_id_is_the_kits_row() {
        let kit = handle(&agent(), &params(&[("id", "MPC-E999")]));
        assert_eq!(kit.status, "422 Unprocessable Entity", "{}", kit.body);
        let v = json_of(&kit);
        assert_eq!(v["id"], KNOWN, "the kit's unknown id row: {v}");
        assert_eq!(v["code"], "unknown_error_id", "{v}");
        assert_eq!(v["exit"], 4, "{v}");
        assert!(v["message"].as_str().is_some_and(|m| m.contains("MPC-E999")), "names the id asked for: {v}");
        assert_eq!(v["on"]["path"], "", "no file: {v}");
    }

    #[test]
    fn a_path_or_file_field_is_refused() {
        for field in ["path", "file"] {
            for value in ["MPC-E160", "notes.md", ""] {
                let r = handle(&agent(), &params(&[("id", KNOWN), (field, value)]));
                assert_eq!(r.status, "400 Bad Request", "{field}={value}: {}", r.body);
                let v = json_of(&r);
                assert_eq!(v["error"]["code"], "bad_request", "{v}");
                assert!(v["error"]["hint"].as_str().is_some_and(|h| h.contains(field)), "names {field}: {v}");
            }
        }
    }

    #[test]
    fn an_id_is_required() {
        for q in [params(&[]), params(&[("id", "")]), params(&[("json", "1")])] {
            let r = handle(&agent(), &q);
            assert_eq!(r.status, "400 Bad Request", "{q:?}: {}", r.body);
            assert_eq!(json_of(&r)["error"]["code"], "bad_request", "{}", r.body);
        }
    }

    #[test]
    fn an_app_pass_and_no_credential_are_refused() {
        let q = params(&[("id", KNOWN)]);
        let r = handle(&Who::Wrong, &q);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(json_of(&r)["error"]["code"], k2_core::deploy::codes::WRONG_CREDENTIAL, "{}", r.body);
        assert_eq!(handle(&Who::Invalid, &q).status, "403 Forbidden");
        let shell = Who::Shell { project_id: "ws-acme".into(), session_id: "sid-shell".into() };
        let login = Who::Login { username: "acme".into(), role: k2_core::connect_users::Role::Member };
        for who in [Who::Owner, login, agent(), shell] {
            assert_eq!(handle(&who, &q).status, "200 OK", "{}", who.label());
        }
    }

    #[test]
    fn its_policy_row_is_a_member_read_and_an_agent_verb() {
        let row = crate::routes::route_policy::lookup(ROUTE).expect("a route policy row");
        assert_eq!(row.get, Some(crate::routes::route_policy::Floor::Member));
        assert_eq!(row.post, None, "a read: GET only");
        assert!(!crate::routes::route_policy::post_allowed(ROUTE));
        assert!(crate::session_token::is_agent_verb(ROUTE), "any agent may read it on the cell socket");
    }
}
