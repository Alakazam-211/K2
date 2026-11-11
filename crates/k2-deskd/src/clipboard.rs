//! The host clipboard, text only (PRD remote desktop §6: Send my clipboard
//! / Copy theirs). Platform-neutral policy; the OS end is the [`Host`]'s
//! `clipboard_get` / `clipboard_set` (macOS: NSPasteboard, `mac::clipboard`).
//!
//! - Text only, UTF-8, at most [`CLIPBOARD_MAX_BYTES`] (1 MiB) either way.
//! - Who may use it is the daemon's call (the controller only, D6); deskd
//!   answers whoever the daemon forwards.
//! - **The text is never logged** and never put in an error: replies and
//!   refusals say only why (sizes in bytes at most). The access log line
//!   (`clipboard_read` / `clipboard_write`) is the daemon's and has no
//!   field for content.
//!
//! [`Host`]: crate::server::Host

use serde_json::{json, Value as J};

use k2_deskd_proto::msg::{clip, codes, CLIPBOARD_MAX_BYTES};

/// What the host's clipboard holds, as the OS end read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipRead {
    Text(String),
    Empty,
    /// Something that isn't text (an image, files).
    NotText,
    /// A password manager's copy (`org.nspasteboard.ConcealedType`).
    Concealed,
    /// Text over the cap; only its size was read.
    TooLarge { bytes: usize },
    /// The person at the machine denied K2 Screen paste access.
    Denied,
    /// No clipboard on this host yet.
    Unsupported,
}

/// The `clipboard_get` reply: `{text}` or `{text: null, reason}`.
pub fn get_reply(r: ClipRead) -> J {
    let reason = match r {
        ClipRead::Text(t) if t.len() <= CLIPBOARD_MAX_BYTES => return json!({"text": t}),
        ClipRead::Text(_) | ClipRead::TooLarge { .. } => clip::TOO_LARGE,
        ClipRead::Empty => clip::EMPTY,
        ClipRead::NotText => clip::NOT_TEXT,
        ClipRead::Concealed => clip::CONCEALED,
        ClipRead::Denied => clip::DENIED,
        ClipRead::Unsupported => clip::UNSUPPORTED,
    };
    json!({"text": null, "reason": reason})
}

/// Checks a `clipboard_set` before it reaches the OS. The error names the
/// size, never the text.
pub fn check_set(text: &str) -> Result<(), (&'static str, String)> {
    if text.is_empty() {
        return Err((codes::EMPTY, "There's no text to put on the clipboard.".into()));
    }
    if text.len() > CLIPBOARD_MAX_BYTES {
        return Err((codes::TOO_LARGE, format!("Clipboard text is {} bytes; K2 Screen takes at most 1 MiB.", text.len())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reply_with_text_or_a_reason() {
        assert_eq!(get_reply(ClipRead::Text("héllo".into())), json!({"text": "héllo"}));
        for (r, why) in [
            (ClipRead::Empty, "empty"),
            (ClipRead::NotText, "not_text"),
            (ClipRead::Concealed, "concealed"),
            (ClipRead::TooLarge { bytes: 5 << 20 }, "too_large"),
            (ClipRead::Denied, "denied"),
            (ClipRead::Unsupported, "unsupported"),
        ] {
            assert_eq!(get_reply(r), json!({"text": null, "reason": why}));
        }
        // An OS end that read too much is still capped here.
        let big = "x".repeat(CLIPBOARD_MAX_BYTES + 1);
        assert_eq!(get_reply(ClipRead::Text(big)), json!({"text": null, "reason": "too_large"}));
        assert_eq!(get_reply(ClipRead::Text("y".repeat(CLIPBOARD_MAX_BYTES))).get("text").and_then(J::as_str).map(str::len), Some(CLIPBOARD_MAX_BYTES), "exactly the cap passes");
    }

    #[test]
    fn writes_are_text_up_to_the_cap_and_refusals_never_echo_it() {
        assert_eq!(check_set("pässword\n"), Ok(()));
        assert_eq!(check_set("").unwrap_err().0, codes::EMPTY);
        let secret = format!("hunter2-{}", "z".repeat(CLIPBOARD_MAX_BYTES));
        let (code, msg) = check_set(&secret).unwrap_err();
        assert_eq!(code, codes::TOO_LARGE);
        assert!(!msg.contains("hunter2") && msg.len() < 100, "{msg}");
        // The cap counts UTF-8 bytes, not characters.
        assert!(check_set(&"é".repeat(CLIPBOARD_MAX_BYTES / 2 + 1)).is_err());
    }
}
