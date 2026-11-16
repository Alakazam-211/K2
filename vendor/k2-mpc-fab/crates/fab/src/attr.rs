//! Attributes, shared by both readers: the photoplot specification's attribute commands
//! (chapter 5), which XNC drill files carry in standard comments (XNC 4.1). One dictionary per
//! read; an aperture (a tool) and an object (a hole) take a snapshot of it when created.

use std::collections::BTreeMap;

use crate::error::Code;

/// What an attribute attaches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Domain {
    /// The file (`TF`).
    File,
    /// Apertures and regions, or drill tools (`TA`).
    Aperture,
    /// Graphical objects, or drill holes (`TO`).
    Object,
}

/// One attribute: its name (a dot starts a standard one) and its fields, escapes decoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Attr {
    pub name: String,
    pub fields: Vec<String>,
}

impl Attr {
    /// The fields joined by commas, as the value reads.
    pub fn value(&self) -> String {
        self.fields.join(",")
    }
}

/// The standard file attribute names (photoplot 3.5, 5.6).
pub const FILE_NAMES: &[&str] = &[
    ".Part",
    ".FileFunction",
    ".FilePolarity",
    ".SameCoordinates",
    ".CreationDate",
    ".GenerationSoftware",
    ".ProjectId",
    ".MD5",
];
/// The standard aperture attribute names.
pub const APERTURE_NAMES: &[&str] = &[".AperFunction", ".DrillTolerance", ".FlashText"];
/// The standard object attribute names.
pub const OBJECT_NAMES: &[&str] = &[
    ".N", ".P", ".C", ".CRot", ".CMfr", ".CMPN", ".CVal", ".CMnt", ".CFtp", ".CPgN", ".CPgD",
    ".CHgt", ".CLbN", ".CLbD", ".CSup",
];
/// Aperture function values the deprecated section lists (photoplot 8.4).
pub const APER_FUNCTION_DEPRECATED: &[&str] = &["Drawing", "CutOut", "Slot", "Cavity"];

/// Whether `s` is a user name: a letter, `_` or `$`, then letters, digits or `_` (photoplot
/// 3.4.5).
pub fn is_user_name(s: &[u8]) -> bool {
    match s.split_first() {
        Some((&f, rest)) => {
            (f.is_ascii_alphabetic() || f == b'_' || f == b'$')
                && rest.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
        }
        None => false,
    }
}

/// Whether `s` is a name: a user name, or a dot and a user name (photoplot 3.4.5).
pub fn is_name(s: &[u8]) -> bool {
    match s.strip_prefix(b".") {
        Some(rest) => is_user_name(rest),
        None => is_user_name(s),
    }
}

/// Whether `name` may be used for `domain`: a user name, or one of the domain's standard ones.
pub fn name_ok(name: &str, domain: Domain) -> bool {
    if !name.starts_with('.') {
        return is_user_name(name.as_bytes());
    }
    let list = match domain {
        Domain::File => FILE_NAMES,
        Domain::Aperture => APERTURE_NAMES,
        Domain::Object => OBJECT_NAMES,
    };
    list.contains(&name)
}

/// Whether `name` is a standard name of any domain, or a user name (what `TD` may name).
pub fn any_name_ok(name: &str) -> bool {
    name_ok(name, Domain::File) || name_ok(name, Domain::Aperture) || name_ok(name, Domain::Object)
}

/// Decodes a field's escapes (`\uXXXX`, `\UXXXXXXXX`; photoplot 3.4.3). A lone backslash, a
/// bad hex digit or a code point that isn't a character is an error.
pub fn unescape(s: &str) -> Result<String, Code> {
    if !s.contains('\\') {
        return Ok(s.to_owned());
    }
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            // copy one UTF-8 character
            let ch = s[i..].chars().next().ok_or(Code::Escape)?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let n = match b.get(i + 1) {
            Some(b'u') => 4,
            Some(b'U') => 8,
            _ => return Err(Code::Escape),
        };
        let hex = b.get(i + 2..i + 2 + n).ok_or(Code::Escape)?;
        if !hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(Code::Escape);
        }
        let v = u32::from_str_radix(std::str::from_utf8(hex).map_err(|_| Code::Escape)?, 16)
            .map_err(|_| Code::Escape)?;
        out.push(char::from_u32(v).ok_or(Code::Escape)?);
        i += 2 + n;
    }
    Ok(out)
}

/// A parsed attribute command: `TF`, `TA`, `TO` or `TD` and what follows (without `%` and `*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrCommand {
    Set(Domain, Attr),
    /// `TD` with a name, or `None` for all aperture and object attributes.
    Delete(Option<String>),
}

/// Parses an attribute command's text: `TF.FileFunction,Copper,L1,Top`, `TD`, `TD.N`.
/// Returns `Code::Syntax` for a malformed one, `Code::AttributeName` for a name its domain
/// doesn't define, `Code::Escape` for a bad escape.
pub fn parse_command(text: &str) -> Result<AttrCommand, Code> {
    let b = text.as_bytes();
    if b.len() < 2 || b[0] != b'T' {
        return Err(Code::Syntax);
    }
    let domain = match b[1] {
        b'F' => Some(Domain::File),
        b'A' => Some(Domain::Aperture),
        b'O' => Some(Domain::Object),
        b'D' => None,
        _ => return Err(Code::Syntax),
    };
    let rest = &text[2..];
    let (name, value) = match rest.find(',') {
        Some(p) => (&rest[..p], Some(&rest[p + 1..])),
        None => (rest, None),
    };
    let Some(domain) = domain else {
        if value.is_some() {
            return Err(Code::Syntax);
        }
        if name.is_empty() {
            return Ok(AttrCommand::Delete(None));
        }
        if !is_name(name.as_bytes()) {
            return Err(Code::Syntax);
        }
        if !any_name_ok(name) {
            return Err(Code::AttributeName);
        }
        return Ok(AttrCommand::Delete(Some(name.to_owned())));
    };
    if !is_name(name.as_bytes()) {
        return Err(Code::Syntax);
    }
    if !name_ok(name, domain) {
        return Err(Code::AttributeName);
    }
    let mut fields = Vec::new();
    if let Some(v) = value {
        for f in v.split(',') {
            fields.push(unescape(f)?);
        }
    }
    Ok(AttrCommand::Set(
        domain,
        Attr {
            name: name.to_owned(),
            fields,
        },
    ))
}

/// The attribute dictionary of one read (photoplot 5.1).
#[derive(Debug, Clone, Default)]
pub struct Dict {
    entries: BTreeMap<String, (Domain, Vec<String>)>,
    /// Bumped on every change of the aperture or object entries: snapshots compare by it.
    aperture_gen: u64,
    object_gen: u64,
}

/// What a dictionary refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictError {
    /// A file attribute set again, or deleted (photoplot 5.2).
    FileAttribute,
    /// A name already in another domain (photoplot 5.1).
    DomainClash,
}

impl Dict {
    /// Applies one command.
    pub fn apply(&mut self, cmd: AttrCommand) -> Result<(), DictError> {
        match cmd {
            AttrCommand::Set(domain, attr) => {
                match self.entries.get(&attr.name) {
                    Some((Domain::File, _)) => return Err(DictError::FileAttribute),
                    Some((d, _)) if *d != domain => return Err(DictError::DomainClash),
                    _ => {}
                }
                self.bump(domain);
                self.entries.insert(attr.name, (domain, attr.fields));
            }
            AttrCommand::Delete(Some(name)) => match self.entries.get(&name) {
                Some((Domain::File, _)) => return Err(DictError::FileAttribute),
                Some((d, _)) => {
                    let d = *d;
                    self.entries.remove(&name);
                    self.bump(d);
                }
                None => {}
            },
            AttrCommand::Delete(None) => {
                let before = self.entries.len();
                self.entries.retain(|_, (d, _)| *d == Domain::File);
                if self.entries.len() != before {
                    self.bump(Domain::Aperture);
                    self.bump(Domain::Object);
                }
            }
        }
        Ok(())
    }

    fn bump(&mut self, d: Domain) {
        match d {
            Domain::Aperture => self.aperture_gen += 1,
            Domain::Object => self.object_gen += 1,
            Domain::File => {}
        }
    }

    /// The entries of `domain`, by name.
    pub fn snapshot(&self, domain: Domain) -> Vec<Attr> {
        self.entries
            .iter()
            .filter(|(_, (d, _))| *d == domain)
            .map(|(n, (_, f))| Attr {
                name: n.clone(),
                fields: f.clone(),
            })
            .collect()
    }

    /// A number that changes whenever `domain`'s entries change.
    pub fn generation(&self, domain: Domain) -> u64 {
        match domain {
            Domain::Aperture => self.aperture_gen,
            Domain::Object => self.object_gen,
            Domain::File => 0,
        }
    }
}

/// Attribute sets, each stored once: objects hold an index, `0` the empty set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrSets {
    pub sets: Vec<Vec<Attr>>,
    /// The dictionary generation last asked for, and its set.
    last: Option<(u64, u32)>,
}

impl Default for AttrSets {
    fn default() -> Self {
        AttrSets {
            sets: vec![Vec::new()],
            last: None,
        }
    }
}

impl AttrSets {
    /// The index of the dictionary's current `domain` set, adding it when it changed since the
    /// last call. `None` when that would make more than `max` sets.
    pub fn current(&mut self, dict: &Dict, domain: Domain, max: usize) -> Option<u32> {
        let g = dict.generation(domain);
        if let Some((g0, i)) = self.last {
            if g0 == g {
                return Some(i);
            }
        }
        let snap = dict.snapshot(domain);
        let i = if snap.is_empty() {
            0
        } else if self.sets.last() == Some(&snap) {
            (self.sets.len() - 1) as u32
        } else {
            if self.sets.len() >= max {
                return None;
            }
            self.sets.push(snap);
            (self.sets.len() - 1) as u32
        };
        self.last = Some((g, i));
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_escapes() {
        assert!(name_ok(".N", Domain::Object));
        assert!(!name_ok(".N", Domain::File));
        assert!(!name_ok(".Nope", Domain::Object));
        assert!(name_ok("$user_1", Domain::File));
        assert!(!name_ok("1user", Domain::File));
        assert_eq!(unescape("a\\u002Cb").unwrap(), "a,b");
        assert_eq!(unescape("\\U000000A9").unwrap(), "\u{a9}");
        assert_eq!(unescape("a\\b"), Err(Code::Escape));
        assert_eq!(unescape("a\\u00"), Err(Code::Escape));
        assert_eq!(
            parse_command("TF.FileFunction,Copper,L1,Top").unwrap(),
            AttrCommand::Set(
                Domain::File,
                Attr {
                    name: ".FileFunction".into(),
                    fields: vec!["Copper".into(), "L1".into(), "Top".into()],
                }
            )
        );
        assert_eq!(parse_command("TD").unwrap(), AttrCommand::Delete(None));
        assert_eq!(parse_command("TF.N,x"), Err(Code::AttributeName));
    }

    #[test]
    fn the_dictionary_keeps_file_attributes_and_domains_apart() {
        let mut d = Dict::default();
        d.apply(parse_command("TF.Part,Single").unwrap()).unwrap();
        assert_eq!(
            d.apply(parse_command("TF.Part,Array").unwrap()),
            Err(DictError::FileAttribute)
        );
        assert_eq!(
            d.apply(parse_command("TD.Part").unwrap()),
            Err(DictError::FileAttribute)
        );
        d.apply(parse_command("TAmine,1").unwrap()).unwrap();
        assert_eq!(
            d.apply(parse_command("TOmine,2").unwrap()),
            Err(DictError::DomainClash)
        );
        d.apply(parse_command("TD").unwrap()).unwrap();
        assert!(d.snapshot(Domain::Aperture).is_empty());
        assert_eq!(d.snapshot(Domain::File).len(), 1);
    }
}
