//! A small XML reader for the two machine-written documents the Android
//! lane reads: Google's SDK repository XML and `uiautomator dump`. Builds a
//! tree of elements (namespace prefixes stripped); comments, processing
//! instructions and DOCTYPE are skipped; CDATA and entities are decoded.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }
    /// Every descendant (depth-first) named `name`.
    pub fn descendants<'a>(&'a self, name: &'a str, out: &mut Vec<&'a Element>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.descendants(name, out);
        }
    }
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map(|(_, l)| l).unwrap_or(name)
}

/// Decode `&amp;` `&lt;` `&gt;` `&quot;` `&apos;` `&#N;` `&#xN;`.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn parse_attrs(s: &str) -> Result<Vec<(String, String)>, String> {
    let mut attrs = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let start = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = &s[start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            return Err(format!("attribute {name} has no value"));
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let q = *b.get(i).ok_or("attribute value missing")?;
        if q != b'"' && q != b'\'' {
            return Err(format!("attribute {name} value is not quoted"));
        }
        i += 1;
        let vstart = i;
        while i < b.len() && b[i] != q {
            i += 1;
        }
        if i >= b.len() {
            return Err(format!("attribute {name} value is not closed"));
        }
        attrs.push((local(name).to_string(), unescape(&s[vstart..i])));
        i += 1;
    }
    Ok(attrs)
}

/// Parse a document into its root element.
pub fn parse(doc: &str) -> Result<Element, String> {
    let mut stack: Vec<Element> = vec![Element { name: "#document".into(), ..Default::default() }];
    let mut rest = doc;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&unescape(rest));
            }
            break;
        };
        if lt > 0 {
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&unescape(&rest[..lt]));
            }
        }
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            let end = r.find("-->").ok_or("comment not closed")?;
            rest = &r[end + 3..];
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").ok_or("CDATA not closed")?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&r[..end]);
            }
            rest = &r[end + 3..];
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            let end = rest.find('>').ok_or("declaration not closed")?;
            rest = &rest[end + 1..];
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').ok_or("end tag not closed")?;
            let name = local(r[..end].trim());
            let el = stack.pop().ok_or("unbalanced end tag")?;
            if el.name != name {
                return Err(format!("end tag </{name}> closes <{}>", el.name));
            }
            stack.last_mut().ok_or("end tag above the root")?.children.push(el);
            rest = &r[end + 1..];
        } else {
            // A start or empty tag; '>' inside quoted values is allowed.
            let b = rest.as_bytes();
            let mut i = 1;
            let mut quote: Option<u8> = None;
            while i < b.len() {
                match (quote, b[i]) {
                    (None, b'"') | (None, b'\'') => quote = Some(b[i]),
                    (Some(q), c) if c == q => quote = None,
                    (None, b'>') => break,
                    _ => {}
                }
                i += 1;
            }
            if i >= b.len() {
                return Err("tag not closed".into());
            }
            let inner = &rest[1..i];
            let (inner, empty) = match inner.strip_suffix('/') {
                Some(s) => (s, true),
                None => (inner, false),
            };
            let name_end = inner.find(|c: char| c.is_ascii_whitespace()).unwrap_or(inner.len());
            let el = Element { name: local(&inner[..name_end]).to_string(), attrs: parse_attrs(&inner[name_end..])?, ..Default::default() };
            if empty {
                stack.last_mut().ok_or("no parent")?.children.push(el);
            } else {
                stack.push(el);
            }
            rest = &rest[i + 1..];
        }
    }
    if stack.len() != 1 {
        return Err(format!("{} element(s) not closed", stack.len() - 1));
    }
    let doc = stack.pop().ok_or("empty document")?;
    doc.children.into_iter().next().ok_or_else(|| "no root element".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespaces_entities_cdata_and_nesting() {
        let d = r#"<?xml version="1.0"?><!-- c --><sdk:repo xmlns:sdk="x"><license id="a" type="text">A &quot;b&quot; &amp; &#65;</license>
            <remotePackage path="emulator"><archives><archive><complete><size>12</size><checksum type="sha1">ab</checksum><url>e.zip</url></complete><host-os>linux</host-os></archive></archives></remotePackage>
            <node text="a &gt; b" bounds="[0,0][1,1]"/><x><![CDATA[<raw>]]></x></sdk:repo>"#;
        let r = parse(d).expect("parses");
        assert_eq!(r.name, "repo");
        assert_eq!(r.child_text("license"), Some("A \"b\" & A"));
        assert_eq!(r.child("license").and_then(|l| l.attr("id")), Some("a"));
        let mut a = Vec::new();
        r.descendants("archive", &mut a);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].child("complete").and_then(|c| c.child_text("url")), Some("e.zip"));
        assert_eq!(r.child("node").and_then(|n| n.attr("text")), Some("a > b"));
        assert_eq!(r.child_text("x"), Some("<raw>"));
    }

    #[test]
    fn broken_documents_fail_loudly() {
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a attr=x/>").is_err());
        assert!(parse("<a>").is_err());
    }
}
