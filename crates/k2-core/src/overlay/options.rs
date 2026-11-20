//! `--options` comma split. `\,` is a literal comma (copy tickets).
//! A choice ask's list also keeps commas inside balanced (), [], {} and
//! "…" together ([`split_ask_options`]).
//! Plus the size caps on a choice ask (prd-agent-settings-cards-v1 §6.10).

/// Most options one choice card offers (§6.10).
pub const MAX_ASK_OPTIONS: usize = 8;
/// Longest option label, in characters (§6.10).
pub const MAX_ASK_OPTION_CHARS: usize = 80;
/// Longest choice prompt, in characters (§6.10).
pub const MAX_ASK_PROMPT_CHARS: usize = 1000;

/// The Thread card hygiene caps on a choice ask (§6.10, unswitched): at
/// most [`MAX_ASK_OPTIONS`] options of [`MAX_ASK_OPTION_CHARS`] characters
/// each, and a prompt of [`MAX_ASK_PROMPT_CHARS`]. Counted in characters
/// after trimming, as stored. K2's own cards (settings and permission
/// cards) are written by the daemon and don't come through an ask.
pub fn check_ask_caps(prompt: &str, options: &[String]) -> Result<(), String> {
    let prompt_chars = prompt.trim().chars().count();
    if prompt_chars > MAX_ASK_PROMPT_CHARS {
        return Err(format!(
            "the prompt is {prompt_chars} characters; a card takes at most {MAX_ASK_PROMPT_CHARS} (post the detail as a message, then ask)"
        ));
    }
    if options.len() > MAX_ASK_OPTIONS {
        return Err(format!("{} options; a card takes at most {MAX_ASK_OPTIONS}", options.len()));
    }
    if let Some(long) = options.iter().find(|o| o.trim().chars().count() > MAX_ASK_OPTION_CHARS) {
        return Err(format!(
            "option \"{}…\" is {} characters; each option takes at most {MAX_ASK_OPTION_CHARS}",
            long.trim().chars().take(24).collect::<String>(),
            long.trim().chars().count()
        ));
    }
    Ok(())
}

/// Split `a,b,c` on unescaped commas. `\,` → `,`. Trim; skip empty.
pub fn split_options_csv(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&',') {
            chars.next();
            cur.push(',');
            continue;
        }
        if c == ',' {
            let t = cur.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
            cur.clear();
            continue;
        }
        cur.push(c);
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

/// A choice ask's `--options` list, split, plus a hint when a splitting
/// comma reads like prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOptions {
    pub options: Vec<String>,
    /// One line for the asker: a comma that split the list is followed by
    /// a space and a lowercase letter or digit (`Yes, node-2 …`), so the
    /// two halves may have been one option. The list still splits there.
    pub hint: Option<String>,
}

/// The closer for an opening bracket or double quote.
fn closer_of(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\u{201C}' => Some('\u{201D}'),
        _ => None,
    }
}

fn is_quote_closer(c: char) -> bool {
    c == '"' || c == '\u{201D}'
}

/// True when every (), [], {} and double-quoted span in `raw` opens and
/// closes in order. `\,` is skipped; an apostrophe is never a quote.
fn brackets_balance(raw: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&',') {
            chars.next();
            continue;
        }
        if let Some(&want) = stack.last() {
            if is_quote_closer(want) {
                if c == want {
                    stack.pop();
                }
                continue;
            }
        }
        if let Some(close) = closer_of(c) {
            stack.push(close);
        } else if matches!(c, ')' | ']' | '}' | '\u{201D}') && stack.pop() != Some(c) {
            return false;
        }
    }
    stack.is_empty()
}

/// Shorten one option for the hint: a parenthetical becomes `(…)`, and
/// the label stops at 32 characters.
fn hint_label(label: &str) -> String {
    let short = match label.find(" (") {
        Some(i) => format!("{} (…)", &label[..i]),
        None => label.to_string(),
    };
    if short.chars().count() > 32 {
        format!("{}…", short.chars().take(31).collect::<String>())
    } else {
        short
    }
}

/// Split a choice ask's `--options` list (the CLI sends it as typed).
///
/// - A comma splits the list unless it is inside balanced (), [], {} or
///   double quotes (straight or curly). `\,` is always a literal comma.
/// - When the list's brackets or quotes don't balance, every unescaped
///   comma splits (the plain rule), so a stray `(` never swallows the rest.
/// - A splitting comma followed by a space and then a lowercase letter or
///   a digit reads like prose (`Yes, node-2 …`): the list still splits
///   there, and [`AskOptions::hint`] says to pass each label with `--option`.
///
/// Labels are trimmed; empty ones are skipped.
pub fn split_ask_options(raw: &str) -> AskOptions {
    let nest = brackets_balance(raw);
    let chars: Vec<char> = raw.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut stack: Vec<char> = Vec::new();
    // The first prose-like split: (the option before it, index of the one after).
    let mut prose: Option<(String, usize)> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c == '\\' && chars.get(i) == Some(&',') {
            i += 1;
            cur.push(',');
            continue;
        }
        if nest {
            if stack.last().is_some_and(|w| is_quote_closer(*w)) {
                if stack.last() == Some(&c) {
                    stack.pop();
                }
                cur.push(c);
                continue;
            }
            if let Some(close) = closer_of(c) {
                stack.push(close);
            } else if matches!(c, ')' | ']' | '}') {
                stack.pop();
            }
        }
        if c == ',' && stack.is_empty() {
            let t = cur.trim().to_string();
            cur.clear();
            if t.is_empty() {
                continue;
            }
            out.push(t.clone());
            if prose.is_none() {
                let mut j = i;
                while chars.get(j) == Some(&' ') {
                    j += 1;
                }
                if j > i && chars.get(j).is_some_and(|n| n.is_lowercase() || n.is_ascii_digit()) {
                    prose = Some((t, out.len()));
                }
            }
            continue;
        }
        cur.push(c);
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    let hint = prose.and_then(|(before, next)| {
        out.get(next).map(|after| {
            format!(
                "an option looks like it continues after a comma (\"{}\" / \"{}\"); to keep commas inside an option, pass each one with --option \"…\"",
                hint_label(&before),
                hint_label(after)
            )
        })
    });
    AskOptions { options: out, hint }
}

/// JSON array, else a choice ask's comma list ([`split_ask_options`]).
pub fn parse_options_value(raw: &str) -> Result<Vec<String>, String> {
    parse_ask_options(raw).map(|a| a.options)
}

/// [`parse_options_value`] plus the prose hint. A JSON array never hints.
pub fn parse_ask_options(raw: &str) -> Result<AskOptions, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("options must not be empty".to_string());
    }
    if trimmed.starts_with('[') {
        let parsed: Vec<String> = serde_json::from_str(trimmed)
            .map_err(|e| format!("options JSON: {e}"))?;
        let cleaned: Vec<String> = parsed
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if cleaned.is_empty() {
            return Err("options must not be empty".to_string());
        }
        return Ok(AskOptions { options: cleaned, hint: None });
    }
    let split = split_ask_options(trimmed);
    if split.options.is_empty() {
        return Err("options must not be empty".to_string());
    }
    Ok(split)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_comma_is_literal() {
        let opts = split_options_csv(r#"Local 8B (slow\, private),Hosted,Hybrid"#);
        assert_eq!(
            opts,
            vec![
                "Local 8B (slow, private)".to_string(),
                "Hosted".to_string(),
                "Hybrid".to_string()
            ],
            "{opts:?}"
        );
    }

    #[test]
    fn ask_options_keep_commas_inside_brackets_and_quotes() {
        let a = split_ask_options(r#"Go (fast, risky),Hold [until Monday, maybe],"Ship, then tell",Stop"#);
        assert_eq!(a.options, vec!["Go (fast, risky)", "Hold [until Monday, maybe]", r#""Ship, then tell""#, "Stop"]);
        assert_eq!(a.hint, None);
        let curly = split_ask_options("\u{201C}Yes, now\u{201D},No");
        assert_eq!(curly.options, vec!["\u{201C}Yes, now\u{201D}", "No"]);
        // An apostrophe is not a quote.
        assert_eq!(split_ask_options("Don't,Do").options, vec!["Don't", "Do"]);
    }

    #[test]
    fn ask_options_plain_list_is_unchanged() {
        let a = split_ask_options("a,b,c");
        assert_eq!(a.options, vec!["a", "b", "c"]);
        assert_eq!(a.hint, None);
        // Capitalised labels after ", " read as a list, not prose.
        let b = split_ask_options("Go, Stop, Wait");
        assert_eq!(b.options, vec!["Go", "Stop", "Wait"]);
        assert_eq!(b.hint, None);
        // `\,` stays a literal comma.
        assert_eq!(split_ask_options(r"Local 8B (slow\, private),Hosted").options, vec!["Local 8B (slow, private)", "Hosted"]);
        assert_eq!(split_ask_options("  Go , Stop  ,,").options, vec!["Go", "Stop"]);
    }

    #[test]
    fn ask_options_prose_comma_splits_and_hints() {
        let a = split_ask_options("Yes, node-2 (an M1; Sew keeps the M4 and node-3),No, wait");
        assert_eq!(a.options, vec!["Yes", "node-2 (an M1; Sew keeps the M4 and node-3)", "No", "wait"]);
        assert_eq!(
            a.hint.as_deref(),
            Some(r#"an option looks like it continues after a comma ("Yes" / "node-2 (…)"); to keep commas inside an option, pass each one with --option "…""#)
        );
        // A digit after ", " reads like prose too; no space does not.
        assert!(split_ask_options("Ship, 2 boxes,Hold").hint.is_some());
        assert_eq!(split_ask_options("yes,no").hint, None);
        // A JSON array (the CLI's --option form) never hints.
        let j = parse_ask_options(r#"["Yes, node-2 (an M1)","No"]"#).expect("json");
        assert_eq!(j.options, vec!["Yes, node-2 (an M1)", "No"]);
        assert_eq!(j.hint, None);
    }

    #[test]
    fn ask_options_unbalanced_brackets_split_on_every_comma() {
        assert_eq!(split_ask_options("Go (fast, risky,Stop").options, vec!["Go (fast", "risky", "Stop"]);
        assert_eq!(split_ask_options("Go) now, later,Stop").options, vec!["Go) now", "later", "Stop"]);
        assert_eq!(split_ask_options(r#""Go, now,Stop"#).options, vec![r#""Go"#, "now", "Stop"]);
    }

    #[test]
    fn trims_and_skips_empty() {
        assert_eq!(split_options_csv("  Go , Stop  ,,"), vec!["Go", "Stop"]);
    }

    #[test]
    fn ask_caps_are_eight_options_of_eighty_and_a_thousand_char_prompt() {
        let opts = |n: usize, len: usize| -> Vec<String> { (0..n).map(|i| format!("{i}{}", "x".repeat(len - 1))).collect() };
        assert_eq!(check_ask_caps("Pick one", &opts(8, 80)), Ok(()));
        assert_eq!(check_ask_caps(&"p".repeat(1000), &opts(1, 1)), Ok(()));
        // Characters, not bytes: 80 two-byte letters pass.
        assert_eq!(check_ask_caps("Pick", &["é".repeat(80)]), Ok(()));
        let e = check_ask_caps("Pick", &opts(9, 3)).expect_err("9 options");
        assert!(e.contains("at most 8"), "{e}");
        let e = check_ask_caps("Pick", &opts(2, 81)).expect_err("81-char option");
        assert!(e.contains("at most 80"), "{e}");
        let e = check_ask_caps(&"p".repeat(1001), &opts(1, 1)).expect_err("1001-char prompt");
        assert!(e.contains("at most 1000"), "{e}");
        // Trimmed, as stored.
        assert_eq!(check_ask_caps(&format!("  {}  ", "p".repeat(1000)), &[format!(" {} ", "o".repeat(80))]), Ok(()));
    }
}
