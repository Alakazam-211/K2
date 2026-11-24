//! thread-context-v1: the line K2 writes before a Garden widget's post,
//! and its label rule. Moved out of overlay_routes.rs as it was (0.46.18
//! queue, ratchet).

use k2_core::overlay;

/// A context label (placement, folder, user) as K2's own line shows it:
/// letters, digits and `-_.@:` only, anything else `_`, so an author-chosen
/// name can't close the bracket or start a stamp.
pub(super) fn context_label(s: &str) -> String {
    let out: String = s
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':') { c } else { '_' })
        .collect();
    if out.is_empty() { "_".to_string() } else { out }
}

/// thread-context-v1 (CA7): K2's own line for a Board context, in fixed
/// words, written BEFORE the person's `[from <user>] [thread:<addr>]`
/// stamp line and never inside it. The widget's text is always a quoted,
/// JSON-escaped string; the placement and the (author-chosen) folder are
/// labelled as such.
pub(super) fn thread_context_line(ctx: &overlay::ThreadContext, from: &str) -> String {
    let quoted = serde_json::to_string(&ctx.text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "[context from Garden widget {} · folder {}, not typed by {}] {quoted}",
        context_label(&ctx.placement),
        context_label(&ctx.widget),
        context_label(&crate::workspace_msg::humanize_from(from)),
    )
}
