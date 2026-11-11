//! `{{placeholder}}` scanning and rendering (prd-agent-templates-v1 §2.3).
//!
//! A placeholder is `{{` + a lowercase identifier + `}}`. Rendering is one
//! pass over the template's own text, so a value is inserted literally and
//! never expanded again. A line holding a placeholder whose value is empty
//! is dropped, so an optional param (`Focus: {{about}}`) leaves no stub.

use std::collections::BTreeMap;

/// Every placeholder in `text`, in order. A malformed one (`{{ x }}`,
/// `{{X}}`, an unclosed `{{`) is an error.
pub fn placeholders(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| "an unclosed {{".to_string())?;
        let inner = &after[..end];
        let ok = !inner.is_empty()
            && inner.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && inner.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            return Err(format!("bad placeholder {{{{{inner}}}}} (use {{{{name}}}}: lowercase letters, digits, _)"));
        }
        out.push(inner.to_string());
        rest = &after[end + 2..];
    }
    Ok(out)
}

/// Render `text` with `vars`. Lines whose placeholders have an empty (or
/// missing) value are dropped. `text` must have passed [`placeholders`].
pub fn render(text: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let names = placeholders(line).unwrap_or_default();
        if names.iter().any(|n| vars.get(n).is_none_or(|v| v.trim().is_empty())) {
            continue;
        }
        // One left-to-right pass over the template's own text: a value is
        // copied out and never scanned again.
        let mut rest = line;
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            out.push_str(&rest[..start]);
            out.push_str(vars.get(&after[..end]).map(|v| v.trim()).unwrap_or_default());
            rest = &after[end + 2..];
        }
        out.push_str(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn template_placeholders_parse_and_refuse_bad_ones() {
        assert_eq!(placeholders("a {{name}} b {{about_it}}").unwrap(), vec!["name", "about_it"]);
        assert!(placeholders("{{ name }}").is_err());
        assert!(placeholders("{{Name}}").is_err());
        assert!(placeholders("x {{name").is_err());
        assert!(placeholders("{{}}").is_err());
        assert_eq!(placeholders("no braces { here }").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn template_render_fills_and_drops_empty_lines() {
        let v = vars(&[("name", "Max"), ("person", "Sam Lee"), ("about", "")]);
        let out = render("You are {{name}}, for {{person}}.\nFocus: {{about}}\nEnd.\n", &v);
        assert_eq!(out, "You are Max, for Sam Lee.\nEnd.\n");
        // A value is inserted literally, never expanded again.
        let v = vars(&[("name", "{{person}}"), ("person", "Sam")]);
        assert_eq!(render("{{name}} / {{person}}", &v), "{{person}} / Sam");
    }
}
