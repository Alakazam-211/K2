use super::*;

/// Security review M4: a Voice & Chat only line is held without
/// bracketed paste; other lines (an App guest's, a person's) aren't.
#[test]
fn a_voice_chat_line_needs_bracketed_paste() {
    let vc = format_sender(Sender::Guest(&guest_label("rev (Voice & Chat only)")), "[thread:vc:demo:rev]\n> hi", "");
    assert!(is_voice_chat_line(&vc), "{vc}");
    assert!(voice_chat_paste_hold(&vc, false));
    assert!(!voice_chat_paste_hold(&vc, true));
    let guest = format_sender(Sender::Guest(&guest_label("ana")), "[thread:sales] hi", "");
    assert!(!voice_chat_paste_hold(&guest, false));
    let person = format_message("mo", "[thread:sales] hi (Voice & Chat only)]", "");
    assert!(!voice_chat_paste_hold(&person, false));
}
