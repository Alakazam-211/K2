//! `--options` comma split. `\,` is a literal comma (copy tickets).
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

/// JSON array, else escaped-comma list.
pub fn parse_options_value(raw: &str) -> Result<Vec<String>, String> {
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
        return Ok(cleaned);
    }
    let cleaned = split_options_csv(trimmed);
    if cleaned.is_empty() {
        return Err("options must not be empty".to_string());
    }
    Ok(cleaned)
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
