//! The line an agent is sent (and its Chat shows) when a person answers an
//! action card.
//!
//! A plain "chose Trust it" names nothing: an agent with two folder cards
//! open, or a person reading the Chat later, can't tell which folder was
//! trusted. Like every card answer (`k2_core::overlay::card_echo`), the
//! line names the card's number, and an action card that has a subject
//! says it on the same line:
//! `chose "Trust it" on card #12 for /srv/work/app (2 risky settings: .claude/a.sh, …)`.
//!
//! The subject is the card's `params.subject` (the card's owner writes it
//! when it posts the card), else one derived from the card's own params for
//! cards posted before `subject` existed. A card with neither names its
//! prompt, as a choice card's answer does.

use k2_core::overlay::card_echo;
use k2_core::overlay::CardAction;

/// At most this many characters of subject on the line.
pub const SUBJECT_MAX: usize = 300;

/// The answer line for `answer` on `action`'s card, the Thread item `seq`
/// asking `prompt` (without the `[thread:<addr>]` prefix the delivery adds).
pub fn answer_line(action: &CardAction, answer: &str, seq: i64, prompt: &str) -> String {
    match subject(action) {
        Some(s) => format!("chose \"{}\" on {} for {s}", one_line(answer, SUBJECT_MAX), card_echo::card_number(seq)),
        None => card_echo::choice_answer_line(seq, prompt, answer),
    }
}

/// What the card is about, on one line, or `None` when it doesn't say.
fn subject(action: &CardAction) -> Option<String> {
    let text = |v: &serde_json::Value| v.as_str().map(|s| one_line(s, SUBJECT_MAX)).filter(|s| !s.is_empty());
    if let Some(s) = text(&action.params["subject"]) {
        return Some(s);
    }
    match action.id.as_str() {
        crate::folder_trust::CARD_ACTION => text(&action.params["path"]),
        _ => None,
    }
}

/// `s` with control characters (newlines too) as spaces, runs of spaces
/// kept as they are, trimmed, at most `max` characters (an ellipsis says it
/// was cut). The line goes into the agent's terminal as one message, so a
/// folder name can't start a second line there.
pub fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.trim();
    if flat.chars().count() <= max {
        return flat.to_string();
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(id: &str, params: serde_json::Value) -> CardAction {
        CardAction { id: id.to_string(), params, dedupe: String::new(), result: None, view: None }
    }

    #[test]
    fn a_card_without_a_subject_names_its_number_and_prompt() {
        let want = "chose \"Go\" on card #41 (\"Refresh the chat?\")";
        assert_eq!(answer_line(&card("chat.refresh", json!({})), "Go", 41, "Refresh the chat?"), want);
        assert_eq!(answer_line(&card("chat.refresh", json!({ "subject": "  " })), "Go", 41, "Refresh the chat?"), want);
    }

    #[test]
    fn a_subject_is_named_on_the_same_line_with_the_card_number() {
        let a = card("anything", json!({ "subject": "the notes widget" }));
        assert_eq!(answer_line(&a, "Keep as mine", 12, "Whose is it?"), "chose \"Keep as mine\" on card #12 for the notes widget");
    }

    #[test]
    fn a_folder_card_from_before_subjects_names_its_folder() {
        let a = card(crate::folder_trust::CARD_ACTION, json!({ "path": "/srv/work/app", "scope": "/srv/work/app" }));
        assert_eq!(answer_line(&a, "Trust it", 7, "Trust this folder?"), "chose \"Trust it\" on card #7 for /srv/work/app");
    }

    #[test]
    fn the_subject_stays_one_line_and_short() {
        let a = card("anything", json!({ "subject": format!("/srv/a\nb\r\tc{}", "x".repeat(400)) }));
        let line = answer_line(&a, "Go", 9, "Go?");
        assert!(!line.contains('\n') && !line.contains('\r') && !line.contains('\t'), "{line}");
        assert!(line.starts_with("chose \"Go\" on card #9 for /srv/a b  c"), "{line}");
        assert!(line.ends_with('…'), "{line}");
        assert_eq!(line.chars().count(), "chose \"Go\" on card #9 for ".chars().count() + SUBJECT_MAX, "{line}");
    }
}
