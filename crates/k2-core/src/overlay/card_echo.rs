//! How the line an agent gets when a person answers a card names that card.
//!
//! A bare `chose Yes` names nothing: an agent with two cards open, both
//! offering "Yes", can't tell which one was answered. Every answer line
//! says the card's number (its Thread seq, the `[N]` a `k2 thread <addr>`
//! read shows and `k2 thread ask` prints) and what it asked:
//!
//! `chose "Yes all three" on card #11290 ("Contrast round 2: land all three…")`
//!
//! An action card that has a subject says the subject instead of its
//! prompt (`chose "Trust it" on card #12 for /srv/work/app`); a secret card
//! already names its secret (`secret API_TOKEN set on card #12 → …`). The
//! card's id rides in the JSON (`k2 thread <addr> --json`, `k2 thread ask
//! --json`).

/// At most this many characters of a card's prompt on the line.
pub const PROMPT_EXCERPT_MAX: usize = 60;

/// `card #<seq>`: the number a Thread read shows as `[<seq>]`.
pub fn card_number(seq: i64) -> String {
    format!("card #{seq}")
}

/// The start of `text` on one line: control characters and runs of
/// whitespace (newlines too) become one space, trimmed, at most `max`
/// characters (an ellipsis says it was cut).
pub fn excerpt(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect::<String>().trim_end().to_string();
    cut.push('…');
    cut
}

/// `card #<seq> ("<start of prompt>")`, or `card #<seq>` for an empty
/// prompt.
pub fn named_card(seq: i64, prompt: &str) -> String {
    match excerpt(prompt, PROMPT_EXCERPT_MAX) {
        about if about.is_empty() => card_number(seq),
        about => format!("{} (\"{about}\")", card_number(seq)),
    }
}

/// The line (after `[thread:<addr>] `) for a person's answer on a choice
/// card.
pub fn choice_answer_line(seq: i64, prompt: &str, answer: &str) -> String {
    format!("chose \"{answer}\" on {}", named_card(seq, prompt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_answer_names_its_card_and_prompt() {
        assert_eq!(
            choice_answer_line(11290, "Contrast round 2: land all three?", "Yes all three"),
            "chose \"Yes all three\" on card #11290 (\"Contrast round 2: land all three?\")"
        );
    }

    #[test]
    fn two_cards_with_the_same_option_read_differently() {
        let a = choice_answer_line(11290, "Land the contrast changes?", "Yes all three");
        let b = choice_answer_line(11302, "Land the spacing changes?", "Yes all three");
        assert_ne!(a, b);
        assert!(a.contains("card #11290") && b.contains("card #11302"), "{a} / {b}");
    }

    #[test]
    fn a_long_multiline_prompt_is_one_short_line() {
        let prompt = format!("Contrast round 2:\n\n  land all three\tvisible changes\r\n{}", "x".repeat(200));
        let line = choice_answer_line(7, &prompt, "Go");
        assert!(!line.contains('\n') && !line.contains('\r') && !line.contains('\t'), "{line}");
        let about = excerpt(&prompt, PROMPT_EXCERPT_MAX);
        assert!(about.starts_with("Contrast round 2: land all three visible changes x"), "{about}");
        assert!(about.ends_with('…'), "{about}");
        assert_eq!(about.chars().count(), PROMPT_EXCERPT_MAX, "{about}");
        assert_eq!(line, format!("chose \"Go\" on card #7 (\"{about}\")"));
    }

    #[test]
    fn a_short_prompt_is_not_cut() {
        assert_eq!(excerpt("  Ship it?  ", PROMPT_EXCERPT_MAX), "Ship it?");
        let exact = "y".repeat(PROMPT_EXCERPT_MAX);
        assert_eq!(excerpt(&exact, PROMPT_EXCERPT_MAX), exact);
    }

    #[test]
    fn an_empty_prompt_still_names_the_number() {
        assert_eq!(named_card(3, " \n "), "card #3");
        assert_eq!(choice_answer_line(3, "", "Go"), "chose \"Go\" on card #3");
    }
}
