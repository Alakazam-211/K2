//! `k2 mail draft delete <d_id>`: an agent deletes a draft IT saved
//! (0.46.17). POST `/cli/mail/draft/delete {project, id}`.
//!
//! **Only its own.** The draft's record (`mail_drafts`, 0292; compose
//! drafts too since 0321) names who wrote it. A draft another workspace's
//! agent or a person wrote answers 403 `not_your_draft`, and nothing is
//! touched. The agent also still needs the `draft` level on the inbox (a
//! lost grant answers the masked `not_found`, like every mail verb).
//!
//! **Delete is the draft only.** A hosted draft is destroyed in its Drafts
//! mailbox (the mail server's own draft delete, as the reader's Discard
//! does); a linked IMAP draft is MOVED to Trash (K2 never expunges). Graph
//! (Microsoft 365) drafts are left to Outlook (409 `not_editable`), and so
//! is a draft whose server didn't say where it saved it. The record then
//! reads `discarded`, by the agent.

use crate::cli_response::CliResponse;
use crate::mail::drafts::{self, DraftRow};

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct DeleteBody {
    project: String,
    id: String,
}

/// POST `/cli/mail/draft/delete`.
pub fn handle_draft_delete(body: &[u8]) -> CliResponse {
    handle_with(body, &|row: &DraftRow, account: &str| drafts::remove_from_mailbox(row, account))
}

type Remove<'a> = &'a dyn Fn(&DraftRow, &str) -> Result<(), CliResponse>;

fn handle_with(body: &[u8], remove: Remove<'_>) -> CliResponse {
    let b: DeleteBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    let id = b.id.trim();
    if !drafts::is_draft_id(id) {
        return error_response(
            "400 Bad Request",
            "usage",
            "pass a draft id (d_ and 32 hex) — 'k2 mail drafts' lists yours",
        );
    }
    let (_path, project_id) = match crate::mail::identity::resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some(row) = drafts::load(id) else {
        return error_response(
            "404 Not Found",
            "not_found",
            &format!("no draft '{id}' — 'k2 mail drafts' lists the drafts you saved"),
        );
    };
    if row.author_kind != "agent" || row.author != project_id {
        return error_response(
            "403 Forbidden",
            "not_your_draft",
            &format!(
                "draft {id} wasn't saved by this workspace's agent, so it can't delete it; nothing was \
                 touched. A person can discard it in the Mail page ('k2 mail reader draft-discard {id}')"
            ),
        );
    }
    let ai = match crate::mail::access::can_draft(&project_id, &row.address) {
        Ok(ai) => ai,
        Err(_) => {
            return error_response(
                "404 Not Found",
                "not_found",
                &format!("no draft '{id}' in an inbox this workspace can draft in"),
            )
        }
    };
    if row.state != drafts::STATE_OPEN {
        return error_response(
            "409 Conflict",
            "not_open",
            &format!(
                "draft {id} is already {} — there is nothing left in Drafts to delete",
                drafts::status_wire(&row)
            ),
        );
    }
    if !drafts::editable(&row) {
        return drafts::not_editable(&row);
    }
    let account = ai.account_id.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| row.inbox_id.clone());
    if let Err(resp) = remove(&row, &account) {
        return resp;
    }
    let now = drafts::now_secs();
    if let Err(e) = drafts::set_state(&row.id, drafts::STATE_DISCARDED, Some(&row.author_label), None, now) {
        return error_response(
            "503 Service Unavailable",
            "not_ready",
            &format!("the draft left Drafts, but the daemon couldn't record that ({e})"),
        );
    }
    let where_to = match row.source {
        crate::mail::access::Source::Hosted => "removed from Drafts",
        crate::mail::access::Source::Linked => "moved to Trash",
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "draftId": row.id,
            "inbox": row.address,
            "draftStatus": "discarded",
            "hint": format!("draft {} deleted from {} ({where_to})", row.id, row.address),
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn project(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/mail-draft-delete-{label}-{}", &id[..8]);
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, format!("mdd-{label}-{}", &id[..8]), path],
        )
        .expect("project");
        (id, path)
    }

    /// A linked inbox owned (primary, level draft) by `project_id`.
    fn linked(project_id: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let address = format!("box-{}@ext.example", &id[..8]);
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, port, username, created_at) \
             VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100)",
            rusqlite::params![id, project_id, address],
        )
        .expect("linked inbox");
        (id, address)
    }

    fn draft(inbox_id: &str, address: &str, project_id: &str, author_kind: &str, author: &str, kind: &str) -> DraftRow {
        let row = DraftRow {
            id: drafts::new_id(),
            source: crate::mail::access::Source::Linked,
            inbox_id: inbox_id.to_string(),
            address: address.to_string(),
            project_id: project_id.to_string(),
            reply_of: String::new(),
            thread_ref: None,
            in_reply_to: None,
            refs: None,
            backend_ref: Some("uid:7:42".to_string()),
            author_kind: author_kind.to_string(),
            author: author.to_string(),
            author_label: "bot".to_string(),
            created_at: 1,
            edited_at: 1,
            edited_by: None,
            state: drafts::STATE_OPEN.to_string(),
            state_at: None,
            state_by: None,
            outbound_id: None,
            kind: kind.to_string(),
            client_key: None,
        };
        drafts::insert(&row).expect("record");
        row
    }

    fn call(path: &str, id: &str, removed: &RefCell<Vec<(String, String)>>) -> CliResponse {
        let body = serde_json::json!({ "project": path, "id": id }).to_string();
        handle_with(body.as_bytes(), &|row: &DraftRow, account: &str| {
            removed.borrow_mut().push((row.id.clone(), account.to_string()));
            Ok(())
        })
    }

    fn code(r: &CliResponse) -> String {
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        v["error"]["code"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn an_agent_deletes_its_own_draft_and_only_that() {
        let (me, my_path) = project("me");
        let (other, _) = project("other");
        let (inbox_id, address) = linked(&me);
        let mine = draft(&inbox_id, &address, &me, "agent", &me, drafts::KIND_COMPOSE);
        let theirs = draft(&inbox_id, &address, &me, "agent", &other, drafts::KIND_REPLY);
        let persons = draft(&inbox_id, &address, &me, "person", "user:ada", drafts::KIND_REPLY);
        let removed = RefCell::new(Vec::new());

        for row in [&theirs, &persons] {
            let r = call(&my_path, &row.id, &removed);
            assert_eq!(r.status, "403 Forbidden", "{}", r.body);
            assert_eq!(code(&r), "not_your_draft");
            assert!(removed.borrow().is_empty(), "nothing touched");
            assert_eq!(drafts::load(&row.id).expect("row").state, drafts::STATE_OPEN, "record untouched");
        }

        let r = call(&my_path, &mine.id, &removed);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(removed.borrow().as_slice(), &[(mine.id.clone(), inbox_id.clone())]);
        let after = drafts::load(&mine.id).expect("row");
        assert_eq!(after.state, drafts::STATE_DISCARDED);
        assert_eq!(after.state_by.as_deref(), Some("bot"));

        // Again: already gone.
        let r = call(&my_path, &mine.id, &removed);
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert_eq!(code(&r), "not_open");
        assert_eq!(removed.borrow().len(), 1, "removed once");
    }

    #[test]
    fn unknown_malformed_and_unlocated_drafts_are_refused() {
        let (me, my_path) = project("refuse");
        let (inbox_id, address) = linked(&me);
        let removed = RefCell::new(Vec::new());
        let r = call(&my_path, "m_123", &removed);
        assert_eq!(r.status, "400 Bad Request");
        let r = call(&my_path, &drafts::new_id(), &removed);
        assert_eq!(r.status, "404 Not Found");
        let unlocated = draft(&inbox_id, &address, &me, "agent", &me, drafts::KIND_REPLY);
        drafts::set_edited(&unlocated.id, None, None, 2).expect("clear ref");
        let r = call(&my_path, &unlocated.id, &removed);
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert_eq!(code(&r), "not_editable");
        assert!(removed.borrow().is_empty());
    }
}
