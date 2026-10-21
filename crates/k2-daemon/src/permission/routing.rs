//! A2 — Who is pushed: a Thread push with `device_usernames` = the eligible
//! logins, `ThreadPush.budget = Permission`, never an empty list;
//! `file_permission_notice` when nobody eligible was visible in 10 minutes
//! (PRD rev 3.1 §6.7, §17.7).
//!
//! P0: [`push_card`] is the hook `overlay_routes::present_action_card` calls
//! for `CardAudience::Logins` (frozen signature, vs-live C11). It sends
//! nothing yet; lane A2 fills it.

/// Push a just-posted permission card to `logins` (the eligible approvers)
/// only. An empty list sends nothing.
pub(crate) fn push_card(conversation_id: &str, project_id: &str, addr: &str, card_id: &str, seq: i64, logins: &[String]) {
    let _ = (conversation_id, project_id, addr, card_id, seq, logins);
}
