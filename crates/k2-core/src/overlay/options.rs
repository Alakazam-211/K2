//! `--options` comma split. `\,` is a literal comma (copy tickets).

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
}
