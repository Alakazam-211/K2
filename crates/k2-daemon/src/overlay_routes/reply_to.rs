//! A reply to one Thread message or card (thread-reply-to-v1, 0.46.18).
//!
//! Rosson: "K2 Engine wanted me to review a card, but he had no way to
//! reference it. ... let agents REPLY to specific messages in the Thread."
//!
//! `POST /cli/thread/post` takes `replyTo`: the target's seq in the same
//! Thread, as `N` or `#N` (a card's number is its seq: `card #13664`). The
//! daemon finds the stored target and:
//!
//! - writes the visible reply the desktop and the phone already draw, from
//!   the stored message, never the caller's words: `↩ #N <from>:`, the
//!   quote as `> ` lines (at most 600 characters, then "…"), a blank line,
//!   then the words ([`crate::shared_routes::reply_text`]). An older client
//!   shows it as it shows a person's reply;
//! - stores `replyTo: {seq, cardId?}` on the doc (`cardId` when the target
//!   is a card), which a newer client reads to jump to the target.
//!
//! A `replyTo` that names no message in this Thread is refused (404
//! `reply_not_found`) and nothing is stored; so is a deleted one
//! (`reply_deleted`), and words that already start with a reply header
//! (`reply_header_not_allowed`: K2 writes the header).
//!
//! Rev 2: `replyQuote` replies to part of the target, the selected words.
//! Squashed to single spaces they must be in the target's quotable words
//! ([`quote_of`], squashed the same way), else 400
//! `reply_quote_not_in_target` and nothing is stored. The header says
//! `↩ part of #N <from>:`, the quote is the selection (trimmed), and the
//! doc's `replyTo` carries `part: true` and `quote` (the squashed words).
//!
//! A post with no `replyTo` whose words start with a reply header (the
//! desktop's and the phone's own reply text, from before this) is stored
//! as it always was.

use super::*;

/// `/boot-status` feature key: the server takes `replyTo` and stores it.
pub(crate) const FEATURE: &str = "thread-reply-to-v1";
/// The post's param.
pub(crate) const PARAM: &str = "replyTo";
/// Rev 2: reply to part of the target, these words.
pub(crate) const QUOTE_PARAM: &str = "replyQuote";
/// The refusal for a selection that isn't in the target.
pub(crate) const QUOTE_NOT_IN_TARGET: &str = "reply_quote_not_in_target";

/// `replyQuote`, as sent (None: none, or empty).
pub(crate) fn quote_param(params: &HashMap<String, String>) -> Option<String> {
    params
        .get(QUOTE_PARAM)
        .or_else(|| params.get("reply_quote"))
        .map(|q| strip_untrusted_controls(q.as_str()))
        .filter(|q| !q.trim().is_empty())
}

/// Runs of whitespace (line breaks too) as one space, trimmed.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rev 2: the selection `q` of message `seq` whose quotable words are
/// `quotable`: what the quote shows (`q` trimmed) and what `replyTo.quote`
/// keeps (`q` squashed). Refused when, squashed, it is empty or not in the
/// squashed words.
pub(crate) fn part_of(seq: i64, quotable: &str, q: &str) -> Result<(String, String), CliResponse> {
    let squashed = squash(q);
    if squashed.is_empty() || !squash(quotable).contains(&squashed) {
        return Err(error_json(
            "400 Bad Request",
            QUOTE_NOT_IN_TARGET,
            format!("replyQuote must be words from message #{seq} (as it reads in the Thread); these aren't in it"),
        ));
    }
    Ok((q.trim().to_string(), squashed))
}

/// What a reply may quote of a stored body: a reply's own words, else the
/// body ([`quote_of`] for a text message).
pub(crate) fn quotable_text(body: &str) -> String {
    words_of_reply(body).unwrap_or_else(|| body.to_string())
}

/// The seq `replyTo` names: `N` or `#N`, a whole number above 0. None: the
/// post is no reply.
pub(crate) fn param(params: &HashMap<String, String>) -> Result<Option<i64>, CliResponse> {
    let Some(raw) = params.get(PARAM).or_else(|| params.get("reply_to")) else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let digits = raw.strip_prefix('#').unwrap_or(raw);
    match digits.parse::<i64>() {
        Ok(n) if n > 0 && digits.bytes().all(|b| b.is_ascii_digit()) => Ok(Some(n)),
        _ => Err(usage(format!(
            "replyTo is a message's or card's number in this Thread (#N or N, a whole number above 0), not {:?}",
            raw.chars().take(24).collect::<String>()
        ))),
    }
}

/// What a reply quotes from its target: a message's own words (a reply's
/// words without its header and quote, as the desktop quotes it), a card's
/// question, a file-only post's `[file]`.
pub(crate) fn quote_of(doc: &overlay::OverlayDoc) -> String {
    let body = doc.body.clone().unwrap_or_default();
    let text = match doc.kind.as_str() {
        "choice" => doc.choice.as_ref().map(|c| c.prompt.clone()).filter(|p| !p.trim().is_empty()).unwrap_or(body),
        "secret" => doc
            .secret
            .as_ref()
            .map(|s| s.prompt.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| format!("Secret {}", s.name)))
            .unwrap_or(body),
        _ => words_of_reply(&body).unwrap_or(body),
    };
    if text.trim().is_empty() {
        return k2_core::overlay::attachments::placeholder(doc.attachments.len()).unwrap_or_default();
    }
    text
}

/// A reply's own words (after its `↩` header, its `> ` lines and one blank
/// line), each line's two-space lead taken off; None for any other text.
fn words_of_reply(body: &str) -> Option<String> {
    let mut lines = body.split('\n');
    let head = lines.next()?;
    if !head.starts_with(crate::shared_routes::REPLY_MARK) || !head.ends_with(':') {
        return None;
    }
    let rest: Vec<&str> = lines.collect();
    let quoted = rest.iter().take_while(|l| **l == ">" || l.starts_with("> ")).count();
    if quoted == 0 {
        return None;
    }
    let mut words = &rest[quoted..];
    if words.first() == Some(&"") {
        words = &words[1..];
    }
    Some(words.iter().map(|l| l.strip_prefix("  ").unwrap_or(l)).collect::<Vec<_>>().join("\n"))
}

/// The stored reply for `words` to message `seq` of `conversation_id`: the
/// text to store (header, quote, words) and the doc's `replyTo`. `part`:
/// reply to part of it, these words (rev 2).
pub(crate) fn build(conversation_id: &str, seq: i64, words: &str, part: Option<&str>) -> Result<(String, overlay::ReplyTo), CliResponse> {
    if words.trim_start().starts_with(crate::shared_routes::REPLY_MARK) {
        return Err(error_json(
            "400 Bad Request",
            "reply_header_not_allowed",
            concat!("send only your words with replyTo; ", k2_core::product_name!(), " writes the reply header and quote"),
        ));
    }
    let item = match overlay::find_thread_message(conversation_id, &overlay::MessageRef::Seq(seq)) {
        Ok(Some(item)) => item,
        Ok(None) => {
            return Err(error_json(
                "404 Not Found",
                "reply_not_found",
                format!("no message or card #{seq} in this Thread (a read lists them as [N])"),
            ))
        }
        Err(e) => return Err(error_json("500 Internal Server Error", "store", e)),
    };
    if item.doc.deleted {
        return Err(error_json("404 Not Found", "reply_deleted", format!("message #{seq} was deleted; there is nothing to reply to")));
    }
    let quotable = quote_of(&item.doc);
    let mut reply_to = reply_to_of(&item);
    let text = match part {
        None => crate::shared_routes::reply_text(seq, &item.doc.from, &quotable, words),
        Some(q) => {
            let (shown, squashed) = part_of(seq, &quotable, q)?;
            reply_to.part = true;
            reply_to.quote = Some(squashed);
            crate::shared_routes::reply_text_as(seq, &item.doc.from, &shown, words, true)
        }
    };
    Ok((text.trim_end().to_string(), reply_to))
}

/// The doc's `replyTo` for a reply to `item`: its seq, and its id when it
/// is a card (any kind but `text`).
pub(crate) fn reply_to_of(item: &overlay::OverlayItem) -> overlay::ReplyTo {
    overlay::ReplyTo { seq: item.seq, card_id: (item.doc.kind != "text").then(|| item.id.clone()), part: false, quote: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_s_own_words_are_what_a_reply_to_it_quotes() {
        assert_eq!(words_of_reply("↩ #3 sales:\n> ship?\n\n  yes\n  today").as_deref(), Some("yes\ntoday"));
        assert_eq!(words_of_reply("↩ #3 sales:\n> ship?").as_deref(), Some(""));
        assert_eq!(words_of_reply("plain words"), None);
        assert_eq!(words_of_reply("↩ #3 sales:\nno quote"), None);
    }

    #[test]
    fn a_part_must_be_words_of_the_target_whatever_the_spacing() {
        let (shown, kept) = part_of(4, "Ship the\n contrast   pass today?", "  contrast pass\n").unwrap_or_else(|r| panic!("in it: {}", r.body));
        assert_eq!((shown.as_str(), kept.as_str()), ("contrast pass", "contrast pass"));
        for bad in ["", "   ", "contrast fail", "Ship today"] {
            let r = part_of(4, "Ship the contrast pass today?", bad).expect_err(bad);
            assert_eq!(r.status, "400 Bad Request", "{bad}");
            assert!(r.body.contains(QUOTE_NOT_IN_TARGET), "{bad}: {}", r.body);
        }
    }

    #[test]
    fn the_param_takes_n_and_hash_n_only() {
        let p = |v: &str| param(&HashMap::from([(PARAM.to_string(), v.to_string())]));
        assert_eq!(p("12").ok().flatten(), Some(12));
        assert_eq!(p("#13664").ok().flatten(), Some(13664));
        assert_eq!(p(" ").ok().flatten(), None);
        for bad in ["0", "#0", "-3", "+3", "#", "x", "1.5", "##4"] {
            let r = p(bad).expect_err(bad);
            assert_eq!(r.status, "400 Bad Request", "{bad}: {}", r.body);
        }
        assert_eq!(param(&HashMap::new()).ok().flatten(), None);
    }
}
