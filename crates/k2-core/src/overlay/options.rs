//! A choice card's `--options` list arrives as one string and splits on
//! `;` only ([`split_ask_options`]); a comma is part of a label. The
//! repeatable `--option` form arrives as a JSON array and never splits.
//! [`split_options_csv`] is the comma list of a permission request's values
//! (`nodes=a,b`), not a card's options.
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

/// A permission request's list value (`nodes=a,b`): split on unescaped
/// commas, `\,` is a literal comma. Trim; skip empty. Never a card's options.
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

/// The one line the CLI prints, and the ask's answer carries as `hint`,
/// when an `--options` value has a comma and no `;`: it became one option.
pub const OPTIONS_ONE_OPTION_NOTE: &str =
    "--options splits on ';'; this became one option. Use ';' between options, or repeat --option";

/// A choice ask's `--options` list, split, plus the note for a value that
/// has commas and no `;`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOptions {
    pub options: Vec<String>,
    /// [`OPTIONS_ONE_OPTION_NOTE`] when the value has a comma and no `;`
    /// (it stayed one option); else None.
    pub hint: Option<String>,
}

/// Split a choice ask's `--options` value on `;` only. Labels are
/// trimmed; empty ones are skipped. A comma never splits: a value with
/// commas and no `;` is one option, and [`AskOptions::hint`] says so.
pub fn split_ask_options(raw: &str) -> AskOptions {
    let options: Vec<String> = raw
        .split(';')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let hint = (raw.contains(',') && !raw.contains(';')).then(|| OPTIONS_ONE_OPTION_NOTE.to_string());
    AskOptions { options, hint }
}

/// JSON array, else a choice ask's `;` list ([`split_ask_options`]).
pub fn parse_options_value(raw: &str) -> Result<Vec<String>, String> {
    parse_ask_options(raw).map(|a| a.options)
}

/// [`parse_options_value`] plus the one-option note. A JSON array never
/// has one.
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
    fn ask_options_split_on_semicolons_only() {
        let a = split_ask_options("No, the forward clean-up is enough; Yes, rewrite the history too");
        assert_eq!(a.options, vec!["No, the forward clean-up is enough", "Yes, rewrite the history too"]);
        assert_eq!(a.hint, None);
        assert_eq!(split_ask_options("a; b").options, vec!["a", "b"]);
        assert_eq!(split_ask_options("  Go ; Stop  ;;").options, vec!["Go", "Stop"]);
        // No comma and no `;`: one option, no note.
        let one = split_ask_options("Done");
        assert_eq!(one.options, vec!["Done"]);
        assert_eq!(one.hint, None);
    }

    #[test]
    fn ask_options_with_commas_and_no_semicolon_stay_one_option_with_the_note() {
        let a = split_ask_options("No, the forward clean-up is enough, Yes, rewrite the history too");
        assert_eq!(a.options, vec!["No, the forward clean-up is enough, Yes, rewrite the history too"]);
        assert_eq!(a.hint.as_deref(), Some(OPTIONS_ONE_OPTION_NOTE));
        assert_eq!(
            OPTIONS_ONE_OPTION_NOTE,
            "--options splits on ';'; this became one option. Use ';' between options, or repeat --option"
        );
        // `\,` is not an escape any more: the backslash stays.
        assert_eq!(split_ask_options(r"a\,b").options, vec![r"a\,b"]);
        // A JSON array (the CLI's --option form) never splits and never has the note.
        let j = parse_ask_options(r#"["Yes, node-2 (an M1)","No"]"#).expect("json");
        assert_eq!(j.options, vec!["Yes, node-2 (an M1)", "No"]);
        assert_eq!(j.hint, None);
        // Only `;` and spaces: refused as empty.
        assert!(parse_ask_options(" ; ;").is_err());
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
