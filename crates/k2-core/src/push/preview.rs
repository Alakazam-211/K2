//! Message previews for Thread pushes (0.46.1, Rosson 2026-10-17).
//!
//! A Thread push used to say only "New message in Thread". It now shows
//! the start of the agent's message: markdown stripped to plain text, the
//! first [`PREVIEW_SENTENCES`] sentences, at most [`PREVIEW_MAX_CHARS`]
//! characters.
//!
//! **Privacy.** A preview is message text, and a push body leaves this
//! server: it passes through K2's push relay and then Apple (APNs) or
//! Google (FCM) on its way to the phone's lock screen. That is why:
//! - a secret card NEVER carries content (its body stays "Asks for a
//!   secret", whatever the caller passes);
//! - each phone opts in or out (`previews` on `POST
//!   /cli/push/register-device`, default on);
//! - the owner has a server-wide master switch (`pushMessagePreviews`,
//!   default on). Off = every phone gets the generic line.
//!
//! Lengths are counted in Unicode scalar values (`char`s) and a cut is
//! only ever made on a grapheme-cluster boundary, so an emoji (even a
//! ZWJ family or a flag) or a combining sequence is never split.

use unicode_segmentation::UnicodeSegmentation;

/// The longest push body a preview may make, "…" included.
pub const PREVIEW_MAX_CHARS: usize = 140;
/// How many sentences a preview keeps.
pub const PREVIEW_SENTENCES: usize = 2;
/// What a cut preview ends with.
pub const ELLIPSIS: char = '…';
/// What a fenced code block becomes.
pub const CODE_PLACEHOLDER: &str = "[code]";
/// What an image becomes.
pub const IMAGE_PLACEHOLDER: &str = "[image]";

/// Marks a hard sentence boundary in the stripped text (a heading's end, a
/// paragraph break). Never in the output.
const BREAK: char = '\u{0}';

/// The preview of one message: plain text, the first two sentences, at
/// most [`PREVIEW_MAX_CHARS`] characters with "…" when cut. `""` when
/// nothing readable is left (the caller then uses the generic body).
pub fn message_preview(text: &str) -> String {
    let plain = strip_markdown(text);
    let first = first_sentences(&plain, PREVIEW_SENTENCES);
    cap_chars(&first, PREVIEW_MAX_CHARS)
}

/// Cut `s` to at most `max` characters on a grapheme boundary, ending in
/// "…" when anything was dropped. A string that already fits is returned
/// as is. A cut backs off to the last space when one is near, so a word is
/// not chopped in half (scripts without spaces are cut anywhere).
pub fn cap_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let room = max - 1; // the ellipsis
    let mut out = String::new();
    let mut used = 0usize;
    let mut cut_mid_word = false;
    let mut graphemes = s.graphemes(true).peekable();
    while let Some(g) = graphemes.next() {
        let n = g.chars().count();
        if used + n > room {
            cut_mid_word = !g.chars().all(char::is_whitespace)
                && !out.ends_with(char::is_whitespace);
            break;
        }
        out.push_str(g);
        used += n;
    }
    if cut_mid_word {
        // Back off to a space in the last 24 characters, if there is one.
        if let Some(i) = out.rfind(char::is_whitespace) {
            if out[i..].chars().count() <= 24 && out[..i].trim_end().chars().count() > 0 {
                out.truncate(i);
            }
        }
    }
    let mut out = out.trim_end().trim_end_matches([',', ';', ':']).trim_end().to_string();
    // Never "foo.…": a cut right after a full stop reads oddly.
    while out.ends_with('.') {
        out.pop();
    }
    out.push(ELLIPSIS);
    out
}

/// Markdown → plain text: headings, emphasis, blockquotes, list markers and
/// rules go; links keep their text; inline code keeps its text; a fenced
/// code block becomes "[code]"; an image becomes "[image]". Whitespace is
/// collapsed. Headings and paragraph breaks end a sentence.
pub fn strip_markdown(text: &str) -> String {
    let mut out = String::new();
    let mut fence: Option<(char, usize)> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some((ch, len)) = fence {
            if is_fence_close(line, ch, len) {
                fence = None;
            }
            continue;
        }
        if let Some((ch, len)) = fence_open(line) {
            fence = Some((ch, len));
            push_word(&mut out, CODE_PLACEHOLDER);
            continue;
        }
        if line.is_empty() {
            out.push(BREAK);
            continue;
        }
        if is_table_divider(line) && !is_rule(line) {
            continue;
        }
        if is_rule(line) {
            out.push(BREAK);
            continue;
        }
        let (body, heading) = strip_block_prefix(line);
        let inline = strip_inline(&body);
        let inline = inline.trim();
        if !inline.is_empty() {
            push_word(&mut out, inline);
        }
        if heading {
            out.push(BREAK);
        }
    }
    collapse(&out)
}

fn push_word(out: &mut String, s: &str) {
    if !out.is_empty() && !out.ends_with(char::is_whitespace) && !out.ends_with(BREAK) {
        out.push(' ');
    }
    out.push_str(s);
}

/// Collapse runs of whitespace to one space; runs of breaks to one break;
/// trim. A break absorbs the spaces around it.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    let mut pending_break = false;
    for c in s.chars() {
        if c == BREAK {
            pending_break = true;
            pending_space = false;
        } else if c.is_whitespace() {
            if !pending_break {
                pending_space = true;
            }
        } else {
            if !out.is_empty() {
                if pending_break {
                    out.push(BREAK);
                } else if pending_space {
                    out.push(' ');
                }
            }
            pending_space = false;
            pending_break = false;
            out.push(c);
        }
    }
    out
}

/// "```lang" / "~~~": the fence char and its run length.
fn fence_open(line: &str) -> Option<(char, usize)> {
    for ch in ['`', '~'] {
        let len = line.chars().take_while(|c| *c == ch).count();
        if len >= 3 {
            // A backtick fence's info string can't hold a backtick.
            if ch == '`' && line[len..].contains('`') {
                return None;
            }
            return Some((ch, len));
        }
    }
    None
}

fn is_fence_close(line: &str, ch: char, len: usize) -> bool {
    let run = line.chars().take_while(|c| *c == ch).count();
    run >= len && line.chars().skip(run).all(char::is_whitespace)
}

/// `---`, `***`, `___` (spaces allowed between).
fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_'].iter().any(|r| compact.chars().all(|c| c == *r))
}

/// `|---|:--:|` and friends.
fn is_table_divider(line: &str) -> bool {
    line.contains('-')
        && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// Strip a line's block prefix: blockquote `>`s, a heading's `#`s (returns
/// `heading = true`), a list marker (`-`, `*`, `+`, `1.`, `1)`) and a task
/// box (`[ ]`, `[x]`). Table pipes become spaces.
fn strip_block_prefix(line: &str) -> (String, bool) {
    let mut s = line;
    loop {
        let t = s.trim_start();
        if let Some(rest) = t.strip_prefix('>') {
            s = rest;
        } else {
            s = t;
            break;
        }
    }
    let mut heading = false;
    let hashes = s.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &s[hashes..];
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            heading = true;
            // A closing run of #s is decoration too.
            s = rest.trim().trim_end_matches('#').trim_end();
        }
    }
    if !heading {
        s = strip_list_marker(s);
    }
    let s = if s.starts_with('|') || s.ends_with('|') {
        s.replace('|', " ")
    } else {
        s.to_string()
    };
    (s, heading)
}

fn strip_list_marker(s: &str) -> &str {
    let after = if let Some(rest) = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
    {
        rest
    } else {
        let digits = s.chars().take_while(char::is_ascii_digit).count();
        let rest = &s[digits..];
        match (digits, rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))) {
            (1..=9, Some(r)) => r,
            _ => return s,
        }
    };
    let after = after.trim_start();
    for boxed in ["[ ] ", "[x] ", "[X] "] {
        if let Some(r) = after.strip_prefix(boxed) {
            return r;
        }
    }
    after
}

/// Inline markdown → text: images, links, autolinks, inline code,
/// emphasis and strikethrough, backslash escapes.
fn strip_inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                // Inline code: a run of n backticks up to the next run of n.
                let run = count_run(&chars, i, '`');
                if let Some(end) = find_run(&chars, i + run, '`', run) {
                    let inner: String = chars[i + run..end].iter().collect();
                    out.push_str(inner.trim());
                    i = end + run;
                } else {
                    i += run; // an unmatched backtick is dropped
                }
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                if let Some((_, next)) = parse_link(&chars, i + 1) {
                    out.push_str(IMAGE_PLACEHOLDER);
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            '[' => {
                if let Some((text, next)) = parse_link(&chars, i) {
                    out.push_str(&strip_inline(&text));
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            '<' => {
                // Autolink <https://…> / <a@b.c>: keep the address.
                if let Some(end) = chars[i + 1..].iter().position(|c| *c == '>') {
                    let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                    if !inner.contains(char::is_whitespace)
                        && (inner.contains("://") || inner.contains('@'))
                    {
                        out.push_str(&inner);
                        i += end + 2;
                        continue;
                    }
                }
                out.push(c);
                i += 1;
            }
            '*' | '_' | '~' => {
                let run = count_run(&chars, i, c);
                let prev = if i == 0 { None } else { Some(chars[i - 1]) };
                let next = chars.get(i + run).copied();
                let is_delim = if c == '~' {
                    run == 2
                } else {
                    emphasis_delimiter(c, prev, next)
                };
                if !is_delim {
                    for _ in 0..run {
                        out.push(c);
                    }
                }
                i += run;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn count_run(chars: &[char], from: usize, ch: char) -> usize {
    chars[from..].iter().take_while(|c| **c == ch).count()
}

/// The start of the next run of exactly `len` `ch`s at or after `from`.
fn find_run(chars: &[char], from: usize, ch: char, len: usize) -> Option<usize> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == ch {
            let run = count_run(chars, j, ch);
            if run == len {
                return Some(j);
            }
            j += run;
        } else {
            j += 1;
        }
    }
    None
}

/// `[text](url)`, `[text][ref]` or `[text]` followed by `(`/`[`, starting
/// at the `[`: the link text and the index after the link.
fn parse_link(chars: &[char], open: usize) -> Option<(String, usize)> {
    let mut depth = 0usize;
    let mut close = None;
    for (j, c) in chars.iter().enumerate().skip(open) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let text: String = chars[open + 1..close].iter().collect();
    let (opener, closer) = match chars.get(close + 1) {
        Some('(') => ('(', ')'),
        Some('[') => ('[', ']'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (j, c) in chars.iter().enumerate().skip(close + 1) {
        if *c == opener {
            depth += 1;
        } else if *c == closer {
            depth -= 1;
            if depth == 0 {
                return Some((text, j + 1));
            }
        }
    }
    None
}

/// Is a `*`/`_` run emphasis (strip) or literal (keep)? A run opens when
/// the next char is not a space, closes when the previous one is not a
/// space; `_` inside a word (`snake_case`) and `*` between spaces
/// (`2 * 3`) stay.
fn emphasis_delimiter(ch: char, prev: Option<char>, next: Option<char>) -> bool {
    let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
    let opens = !space(next);
    let closes = !space(prev);
    if !opens && !closes {
        return false;
    }
    if ch == '_' {
        let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        if word(prev) && word(next) {
            return false;
        }
    }
    true
}

/// The first `n` sentences of collapsed plain text, joined by a space. A
/// sentence ends at `.`/`!`/`?`/`…` followed by a space (closing quotes and
/// brackets stay with it), at a CJK full stop (`。！？`), or at a hard break.
pub fn first_sentences(plain: &str, n: usize) -> String {
    let chars: Vec<char> = plain.chars().collect();
    let mut sentences: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    let finish = |cur: &mut String, sentences: &mut Vec<String>| {
        let t = cur.trim();
        if !t.is_empty() {
            sentences.push(t.to_string());
        }
        cur.clear();
    };
    while i < chars.len() && sentences.len() < n {
        let c = chars[i];
        if c == BREAK {
            finish(&mut cur, &mut sentences);
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
        let ends = match c {
            '。' | '！' | '？' => true,
            '.' | '!' | '?' | '…' => {
                // Swallow closing quotes/brackets and more terminators.
                while i < chars.len()
                    && matches!(chars[i], '"' | '\'' | ')' | ']' | '”' | '’' | '.' | '!' | '?')
                {
                    cur.push(chars[i]);
                    i += 1;
                }
                i >= chars.len() || chars[i] == ' ' || chars[i] == BREAK
            }
            _ => false,
        };
        if ends {
            finish(&mut cur, &mut sentences);
        }
    }
    if sentences.len() < n {
        finish(&mut cur, &mut sentences);
    }
    // A sentence that ended at a break (a heading, a paragraph with no full
    // stop) is joined with a dash so the two don't run together.
    let mut out = String::new();
    for s in sentences {
        if !out.is_empty() {
            let sep = if out.ends_with(['。', '！', '？']) {
                ""
            } else if out.ends_with(['.', '!', '?', '…', '"', '\'', ')', ']', '”', '’']) {
                " "
            } else {
                " — "
            };
            out.push_str(sep);
        }
        out.push_str(&s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_keeps_the_first_two_sentences() {
        assert_eq!(
            message_preview("Done. The build is green. I also pushed the branch."),
            "Done. The build is green."
        );
        assert_eq!(message_preview("Ready? Yes! Next."), "Ready? Yes!");
        assert_eq!(message_preview("One sentence only"), "One sentence only");
        // A decimal or a version is not a sentence end.
        assert_eq!(
            message_preview("Bumped to 0.46.1 today. Tests pass. More later."),
            "Bumped to 0.46.1 today. Tests pass."
        );
        // Closing quotes stay with their sentence.
        assert_eq!(
            message_preview("He said \"ship it.\" So I did. Then lunch."),
            "He said \"ship it.\" So I did."
        );
    }

    #[test]
    fn markdown_is_stripped() {
        let md = "## Summary\n\nI **fixed** the _login_ bug in `auth.rs` and [opened a PR](https://github.com/x/y/pull/1). Next up: ~~nothing~~ tests.";
        // A heading is a sentence of its own, joined with a dash.
        assert_eq!(message_preview(md), "Summary — I fixed the login bug in auth.rs and opened a PR.");
        assert_eq!(
            message_preview("I **fixed** it. Next up: ~~nothing~~ tests. Bye."),
            "I fixed it. Next up: nothing tests."
        );
        // The heading is its own sentence: two sentences = heading + first.
        assert_eq!(
            strip_markdown("# Title\nBody here."),
            format!("Title{BREAK}Body here.")
        );
        assert_eq!(
            message_preview("- first item\n- second item\n1. third\n2) fourth\n- [x] done"),
            "first item second item third fourth done"
        );
        assert_eq!(message_preview("> quoted **text**. Then more. And more."), "quoted text. Then more.");
        assert_eq!(message_preview("See ![diagram](a.png) above."), "See [image] above.");
        assert_eq!(message_preview("Link: <https://k2.dev>."), "Link: https://k2.dev.");
        assert_eq!(message_preview("A [ref link][1] here."), "A ref link here.");
        assert_eq!(message_preview("Use ``a `tick` b`` now."), "Use a `tick` b now.");
        assert_eq!(message_preview("Escaped \\*stars\\* stay."), "Escaped *stars* stay.");
        // Literal stars and underscores survive.
        assert_eq!(message_preview("2 * 3 = 6 and my_var_name works."), "2 * 3 = 6 and my_var_name works.");
        assert_eq!(message_preview("---\n\n***\nAfter the rules."), "After the rules.");
        assert_eq!(message_preview("| a | b |\n|---|---|\n| 1 | 2 |"), "a b 1 2");
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(message_preview("  lots\tof \n  space   here  "), "lots of space here");
    }

    #[test]
    fn code_fences_become_a_placeholder() {
        assert_eq!(
            message_preview("Run this:\n```bash\ncargo test -p k2-core\n```\nThen restart."),
            "Run this: [code] Then restart."
        );
        // A code-only message is just the placeholder, never the code.
        assert_eq!(message_preview("```rust\nfn secret() {}\n```"), "[code]");
        assert_eq!(message_preview("~~~\nx\n~~~"), "[code]");
        // An unclosed fence hides the rest.
        assert_eq!(message_preview("Look:\n```\nlet key = 1;\nmore"), "Look: [code]");
    }

    #[test]
    fn empty_after_stripping_is_empty() {
        assert_eq!(message_preview(""), "");
        assert_eq!(message_preview("   \n\n  "), "");
        assert_eq!(message_preview("---\n***"), "");
        assert_eq!(message_preview("#"), "");
        assert_eq!(message_preview("![](x.png)"), "[image]");
    }

    #[test]
    fn long_text_is_capped_at_140_with_an_ellipsis() {
        let long = "word ".repeat(60);
        let p = message_preview(&long);
        assert!(p.chars().count() <= PREVIEW_MAX_CHARS, "{} chars: {p}", p.chars().count());
        assert!(p.ends_with(ELLIPSIS), "{p}");
        assert!(!p.contains("wor…"), "cut on a word boundary: {p}");
        // Exactly 140 fits untouched.
        let exact = "a".repeat(140);
        assert_eq!(message_preview(&exact), exact);
        let over = "a".repeat(141);
        let p = message_preview(&over);
        assert_eq!(p.chars().count(), 140);
        assert!(p.ends_with(ELLIPSIS));
        // Two sentences that together run long are cut too.
        let two = format!("{}. {}.", "x".repeat(100), "y".repeat(100));
        let p = message_preview(&two);
        assert_eq!(p.chars().count(), 140, "{p}");
        assert!(p.starts_with(&"x".repeat(100)));
    }

    #[test]
    fn cuts_never_split_an_emoji_or_a_grapheme() {
        // A ZWJ family is 7 chars, one grapheme.
        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
        assert_eq!(family.chars().count(), 7);
        let s = format!("{}{}", "a".repeat(135), family.repeat(3));
        let p = cap_chars(&s, PREVIEW_MAX_CHARS);
        assert!(p.chars().count() <= PREVIEW_MAX_CHARS);
        assert_eq!(p, format!("{}…", "a".repeat(135)), "the family is dropped whole, never split");
        // Flags (regional indicator pairs) and combining marks too.
        let flags = "🇯🇵".repeat(80);
        let p = cap_chars(&flags, 11);
        assert_eq!(p, format!("{}…", "🇯🇵".repeat(5)));
        let combining = "e\u{301}".repeat(80);
        let p = cap_chars(&combining, 10);
        assert_eq!(p, format!("{}…", "e\u{301}".repeat(4)));
        for g in p.trim_end_matches(ELLIPSIS).graphemes(true) {
            assert_eq!(g, "e\u{301}");
        }
    }

    #[test]
    fn cjk_sentences_and_caps() {
        assert_eq!(message_preview("ビルドは成功しました。テストも通りました。次はリリースです。"), "ビルドは成功しました。テストも通りました。");
        let long = "这是一个很长的句子没有标点".repeat(20);
        let p = message_preview(&long);
        assert_eq!(p.chars().count(), PREVIEW_MAX_CHARS, "no spaces: cut anywhere on a char boundary");
        assert!(p.ends_with(ELLIPSIS));
        assert!(long.starts_with(p.trim_end_matches(ELLIPSIS)));
    }

    #[test]
    fn cap_chars_respects_smaller_caps() {
        assert_eq!(cap_chars("short", 10), "short");
        assert_eq!(cap_chars("hello brave new world", 12), "hello brave…");
        assert_eq!(cap_chars("abcdefghij", 5), "abcd…");
        assert_eq!(cap_chars("Done. More text here", 6), "Done…");
        assert_eq!(cap_chars("anything", 0), "");
    }
}
