//! Insert keys into a JSON file's text, keeping every other byte (its key
//! order, spacing and line breaks): `serde_json` would re-sort and re-space
//! a person's `tauri.conf.json`. A small scanner finds the object to add
//! to; the result is parsed again and must equal the original plus exactly
//! the added keys, or nothing is written (the caller prints a snippet).

use serde_json::Value as J;

/// What [`wire_updater`] made.
pub struct Wired {
    /// The new text, or `None` when both keys were already there.
    pub text: Option<String>,
    /// The keys it added (`plugins.updater`, `bundle.createUpdaterArtifacts`).
    pub added: Vec<&'static str>,
}

/// Add `plugins.updater` (= `updater`) and `bundle.createUpdaterArtifacts:
/// true` to a Tauri config's text where they are missing. `Err(why)`: the
/// text isn't an object K2 can edit safely.
pub fn wire_updater(text: &str, updater: &J) -> Result<Wired, String> {
    let root: J = serde_json::from_str(text).map_err(|e| format!("it doesn't parse as JSON: {e}"))?;
    if !root.is_object() {
        return Err("it isn't a JSON object".into());
    }
    let need_artifacts = root.get("bundle").and_then(|b| b.get("createUpdaterArtifacts")).is_none();
    let need_updater = root.get("plugins").and_then(|p| p.get("updater")).is_none();
    if !need_artifacts && !need_updater {
        return Ok(Wired { text: None, added: Vec::new() });
    }
    for (k, want) in [("bundle", need_artifacts), ("plugins", need_updater)] {
        if want && root.get(k).is_some_and(|v| !v.is_object()) {
            return Err(format!("its {k} isn't an object"));
        }
    }
    let unit = indent_unit(text);
    let mut t = text.to_string();
    let mut added = Vec::new();
    if need_artifacts {
        t = match object_at(&t, &["bundle"]) {
            Some(open) => insert_member(&t, open, "createUpdaterArtifacts", &J::Bool(true), &unit),
            None => insert_member(&t, object_at(&t, &[]).ok_or("no top-level object")?, "bundle", &serde_json::json!({ "createUpdaterArtifacts": true }), &unit),
        }
        .ok_or("the bundle object couldn't be found in the text")?;
        added.push("bundle.createUpdaterArtifacts");
    }
    if need_updater {
        t = match object_at(&t, &["plugins"]) {
            Some(open) => insert_member(&t, open, "updater", updater, &unit),
            None => insert_member(&t, object_at(&t, &[]).ok_or("no top-level object")?, "plugins", &serde_json::json!({ "updater": updater }), &unit),
        }
        .ok_or("the plugins object couldn't be found in the text")?;
        added.push("plugins.updater");
    }
    // The proof: the new text is the old value plus exactly these keys.
    let after: J = serde_json::from_str(&t).map_err(|e| format!("the edit didn't parse ({e})"))?;
    let mut expect = root.clone();
    if need_artifacts {
        let b = expect.as_object_mut().ok_or("not an object")?.entry("bundle").or_insert_with(|| J::Object(Default::default()));
        b.as_object_mut().ok_or("bundle")?.insert("createUpdaterArtifacts".into(), J::Bool(true));
    }
    if need_updater {
        let p = expect.as_object_mut().ok_or("not an object")?.entry("plugins").or_insert_with(|| J::Object(Default::default()));
        p.as_object_mut().ok_or("plugins")?.insert("updater".into(), updater.clone());
    }
    if after != expect {
        return Err("the edit didn't come out as planned".into());
    }
    Ok(Wired { text: Some(t), added })
}

/// The file's indent step: the leading whitespace of the first member of
/// the top-level object (two spaces when it can't tell).
fn indent_unit(text: &str) -> String {
    text.lines()
        .skip_while(|l| !l.trim_start().starts_with('{'))
        .nth(1)
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "  ".into())
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// `i` at `"`: the index just past the closing quote.
fn string_end(b: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// `i` at a value's first byte: the index just past it.
fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'"' => string_end(b, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_end(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace() {
                j += 1;
            }
            Some(j)
        }
    }
}

/// One member of an object: (key start, key end, value start, value end).
type Member = (usize, usize, usize, usize);

/// The members of the object whose `{` is at `open`.
fn members(b: &[u8], open: usize) -> Option<Vec<Member>> {
    let mut out = Vec::new();
    let mut i = skip_ws(b, open + 1);
    if b.get(i) == Some(&b'}') {
        return Some(out);
    }
    loop {
        if b.get(i) != Some(&b'"') {
            return None;
        }
        let ke = string_end(b, i)?;
        let colon = skip_ws(b, ke);
        if b.get(colon) != Some(&b':') {
            return None;
        }
        let vs = skip_ws(b, colon + 1);
        let ve = value_end(b, vs)?;
        out.push((i, ke, vs, ve));
        let next = skip_ws(b, ve);
        match b.get(next)? {
            b',' => i = skip_ws(b, next + 1),
            b'}' => return Some(out),
            _ => return None,
        }
    }
}

/// The `{` of the object at `path` (`[]` = the top level).
fn object_at(t: &str, path: &[&str]) -> Option<usize> {
    let b = t.as_bytes();
    let mut open = skip_ws(b, 0);
    if b.get(open) != Some(&b'{') {
        return None;
    }
    for key in path {
        let quoted = format!("\"{key}\"");
        let m = members(b, open)?.into_iter().find(|(ks, ke, _, _)| &t[*ks..*ke] == quoted)?;
        if b.get(m.2) != Some(&b'{') {
            return None;
        }
        open = m.2;
    }
    Some(open)
}

/// The leading whitespace of the line holding byte `i`.
fn line_indent(t: &str, i: usize) -> String {
    let start = t[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
    t[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// `value` as JSON text indented by `unit`, its later lines prefixed with
/// `indent`.
fn render(value: &J, unit: &str, indent: &str) -> Option<String> {
    use serde::Serialize;
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    value.serialize(&mut ser).ok()?;
    let s = String::from_utf8(buf).ok()?;
    Some(s.replace('\n', &format!("\n{indent}")))
}

/// Insert `"key": value` as the last member of the object at `open`.
fn insert_member(t: &str, open: usize, key: &str, value: &J, unit: &str) -> Option<String> {
    let b = t.as_bytes();
    let ms = members(b, open)?;
    let close = value_end(b, open)? - 1;
    let key_json = serde_json::to_string(key).ok()?;
    match ms.last() {
        Some(&(_, _, _, ve)) => {
            let indent = line_indent(t, ms[0].0);
            let v = render(value, unit, &indent)?;
            Some(format!("{},\n{indent}{key_json}: {v}{}", &t[..ve], &t[ve..]))
        }
        None => {
            let outer = line_indent(t, open);
            let indent = format!("{outer}{unit}");
            let v = render(value, unit, &indent)?;
            Some(format!("{}\n{indent}{key_json}: {v}\n{outer}{}", &t[..open + 1], &t[close..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CONF: &str = r#"{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "My App",
  "identifier": "com.you.app",
  "build": { "frontendDist": "../dist" },
  "bundle": {
    "active": true,
    "icon": ["icons/a.png", "icons/{b}.png"]
  },
  "plugins": {}
}
"#;

    fn updater() -> J {
        json!({ "endpoints": ["https://x/_k2/updates/stable/{{target}}/{{arch}}/{{current_version}}"], "pubkey": "" })
    }

    #[test]
    fn keys_go_in_and_every_other_byte_stays() {
        let w = wire_updater(CONF, &updater()).expect("edit");
        let t = w.text.expect("changed");
        assert_eq!(w.added, ["bundle.createUpdaterArtifacts", "plugins.updater"]);
        assert!(t.starts_with("{\n  \"$schema\": \"https://schema.tauri.app/config/2\",\n  \"productName\": \"My App\","), "order kept:\n{t}");
        assert!(t.contains("    \"icon\": [\"icons/a.png\", \"icons/{b}.png\"],\n    \"createUpdaterArtifacts\": true\n  },"), "{t}");
        assert!(t.contains("  \"plugins\": {\n    \"updater\": {\n      \"endpoints\": ["), "{t}");
        assert!(t.ends_with("}\n"), "the trailing newline stays");
        let v: J = serde_json::from_str(&t).unwrap();
        assert_eq!(v["plugins"]["updater"], updater());
        // Twice: nothing more to add.
        assert!(wire_updater(&t, &updater()).unwrap().text.is_none());
    }

    #[test]
    fn missing_objects_are_made_and_odd_files_are_refused() {
        let t = wire_updater("{\n    \"identifier\": \"a.b\"\n}", &updater()).unwrap().text.unwrap();
        assert!(t.contains("\n    \"bundle\": {\n        \"createUpdaterArtifacts\": true\n    }"), "4-space files stay 4-space:\n{t}");
        let v: J = serde_json::from_str(&t).unwrap();
        assert_eq!(v["identifier"], "a.b");
        assert_eq!(v["bundle"]["createUpdaterArtifacts"], true);
        // Only the missing key is added.
        let w = wire_updater(r#"{"bundle": {"createUpdaterArtifacts": false}}"#, &updater()).unwrap();
        assert_eq!(w.added, ["plugins.updater"], "an explicit false is the person's choice");
        assert!(wire_updater("[1]", &updater()).is_err());
        assert!(wire_updater("{\"bundle\": 3}", &updater()).is_err());
        assert!(wire_updater("{ // a comment\n}", &updater()).is_err(), "JSON5 isn't edited");
    }
}
