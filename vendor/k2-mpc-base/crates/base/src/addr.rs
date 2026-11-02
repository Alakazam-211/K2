//! The kit's one address grammar (K-ADDR-1…3): how a verb names a thing inside a doc, for
//! every pack. A verb parses its address operands with [`parse`], says which forms its
//! context has ([`Forms`]), and refuses an ambiguous one with [`ambiguous`] (exit 2,
//! `MPC-E170`, `details.candidates`), never picking "the first" or "the topmost".
//!
//! | Form | Syntax | Example | Class |
//! |---|---|---|---|
//! | native ID, scoped | `@<kind>:<id>[/<kind>:<id>]…` | `@sld:256/sp:4`, `@lyid:42` | the format's own |
//! | minted key | `@k:<key>` (also `key:<key>`) | `@k:r17`, `key:c3` | durable |
//! | positional: Nth | `#N` (from 1) | `#3` | positional |
//! | positional: line | `LN` (from 1) | `L12` | positional |
//! | forced name | `name:<text>` | `name:city` | counts only when unique |
//! | bare word | anything else | `city`, `r17`, `2024` | the context's key, else a unique name |
//!
//! **The forms are syntactically distinct.** `@` starts a native ID or a key; `#` a
//! position; `L` and digits a line (where the context has lines); `name:` a name. A bare word
//! is never a position: **a number never falls back to an index** (`2024` is a key or a
//! name called "2024", never the 2024th anything). A context without native IDs (a grid)
//! reads `@<other>` as a bare word, so a header named `@timestamp` still works; `name:` forces
//! a name everywhere.
//!
//! **Positional forms are bound to a version** (K-ADDR-3): `#N` and `LN` mean "in the version
//! the caller read", so an agent's positional edit names that version (`--if-version` or
//! `--base`). A person acts on what they see; their positional addresses resolve live.
//!
//! **An address is `(scope, id)`** (P1): `@sld:256/sp:4` is shape 4 of slide 256, never shape
//! 4 of whatever slide; the kinds a pack reads, their class and scope, are its ID table.

use crate::errors::{ids, ErrorReport, Value};

/// A parsed address (see the module docs for the forms).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Addr {
    /// `@<kind>:<id>[/<kind>:<id>]…`: a native ID with its scope, outermost first.
    Native(Vec<(String, String)>),
    /// `@k:<key>` or `key:<key>`: a key the kit minted.
    Key(String),
    /// `#N`: the Nth element (1-based) of the context's order.
    Nth(usize),
    /// `LN`: rendered line N (1-based).
    Line(usize),
    /// `name:<text>`: a name, which counts only when it is unique.
    Name(String),
    /// Anything else: the context's key if it is one, else a unique name; never a position.
    Word(String),
}

/// The grammar's four classes of form (P12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// The format's own ID ([`Addr::Native`]).
    Native,
    /// A key the kit minted ([`Addr::Key`]).
    Key,
    /// A position ([`Addr::Nth`], [`Addr::Line`]): bound to a version (K-ADDR-3).
    Positional,
    /// A name or a bare word ([`Addr::Name`], [`Addr::Word`]): counts only when unique.
    Name,
}

impl Addr {
    pub fn form(&self) -> Form {
        match self {
            Addr::Native(_) => Form::Native,
            Addr::Key(_) => Form::Key,
            Addr::Nth(_) | Addr::Line(_) => Form::Positional,
            Addr::Name(_) | Addr::Word(_) => Form::Name,
        }
    }

    /// Whether it names a position (`#N`, `LN`): it means something only in one version.
    pub fn is_positional(&self) -> bool {
        self.form() == Form::Positional
    }
}

impl std::fmt::Display for Addr {
    /// The canonical spelling ([`parse`] reads it back as the same address).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Addr::Native(segs) => {
                for (i, (k, id)) in segs.iter().enumerate() {
                    write!(f, "{}{k}:{id}", if i == 0 { "@" } else { "/" })?;
                }
                Ok(())
            }
            Addr::Key(k) => write!(f, "@k:{k}"),
            Addr::Nth(n) => write!(f, "#{n}"),
            Addr::Line(n) => write!(f, "L{n}"),
            Addr::Name(n) => write!(f, "name:{n}"),
            Addr::Word(w) => f.write_str(w),
        }
    }
}

/// The forms a context has: a grid's rows take `#N` and `LN`, its columns `#N` only, and
/// neither has native IDs. Keys, names and bare words are always read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Forms {
    /// `@<kind>:<id>`: the context has native IDs (else `@<other>` is a bare word).
    pub native: bool,
    /// `#N`.
    pub nth: bool,
    /// `LN`.
    pub line: bool,
}

impl Forms {
    /// No native IDs, no positions: keys, names and words.
    pub const fn none() -> Self {
        Forms {
            native: false,
            nth: false,
            line: false,
        }
    }
    pub const fn native(mut self) -> Self {
        self.native = true;
        self
    }
    pub const fn nth(mut self) -> Self {
        self.nth = true;
        self
    }
    pub const fn line(mut self) -> Self {
        self.line = true;
        self
    }
}

/// Why an address doesn't parse (a usage error, exit 2: `MPC-E122`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AddrError {
    /// `#N` or `LN` whose N isn't a number from 1 (`what`: `#N` or `LN`).
    #[error("{what}: not a number from 1: {given:?}")]
    BadNumber { what: &'static str, given: String },
    /// A native ID or key that is malformed (an empty kind or id, a bad kind).
    #[error("not an address: {given:?} ({reason})")]
    Malformed { given: String, reason: &'static str },
    /// `""`.
    #[error("an empty address")]
    Empty,
}

fn number(s: &str, what: &'static str) -> Result<usize, AddrError> {
    s.parse::<usize>()
        .ok()
        .filter(|n| *n > 0 && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| AddrError::BadNumber {
            what,
            given: s.to_owned(),
        })
}

fn kind_ok(k: &str) -> bool {
    let mut b = k.bytes();
    b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// Parse one address in a context with `forms` (see the module docs).
pub fn parse(s: &str, forms: Forms) -> Result<Addr, AddrError> {
    if s.is_empty() {
        return Err(AddrError::Empty);
    }
    let bad = |reason| AddrError::Malformed {
        given: s.to_owned(),
        reason,
    };
    if let Some(k) = s.strip_prefix("@k:").or_else(|| s.strip_prefix("key:")) {
        return match crate::elem::Key::new(k) {
            Ok(_) => Ok(Addr::Key(k.to_owned())),
            Err(_) => Err(bad("a key is 1 to 24 of [0-9a-z._-]")),
        };
    }
    if let Some(n) = s.strip_prefix("name:") {
        return Ok(Addr::Name(n.to_owned()));
    }
    if let Some(n) = s.strip_prefix('#').filter(|_| forms.nth) {
        return Ok(Addr::Nth(number(n, "#N")?));
    }
    if let Some(n) = s
        .strip_prefix('L')
        .filter(|n| forms.line && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    {
        return Ok(Addr::Line(number(n, "LN")?));
    }
    if let Some(rest) = s.strip_prefix('@').filter(|_| forms.native) {
        let mut segs = Vec::new();
        for seg in rest.split('/') {
            let (k, id) = seg
                .split_once(':')
                .ok_or_else(|| bad("each part is <kind>:<id>"))?;
            if !kind_ok(k) || k == "k" {
                return Err(bad("a kind is lowercase letters, digits, _ or -"));
            }
            if id.is_empty() || id.chars().any(char::is_whitespace) {
                return Err(bad("an id is non-empty, without spaces"));
            }
            segs.push((k.to_owned(), id.to_owned()));
        }
        return Ok(Addr::Native(segs));
    }
    Ok(Addr::Word(s.to_owned()))
}

/// What a lookup by name or key found: nothing, one, or several (refused: never the first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unique<T> {
    None,
    One(T),
    Many(Vec<T>),
}

/// The one match in `items`, if there is exactly one (K-ADDR-2: two or more is
/// [`Unique::Many`], for [`ambiguous`]; nothing ever picks the first or the topmost).
pub fn unique<T>(items: impl IntoIterator<Item = T>) -> Unique<T> {
    let mut v: Vec<T> = items.into_iter().collect();
    match v.len() {
        0 => Unique::None,
        1 => Unique::One(v.pop().expect("one")),
        _ => Unique::Many(v),
    }
}

/// One candidate of an ambiguous (or unknown) address: its most durable address, and where
/// it is now (a position, for people; it says nothing durable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The most durable address that names it alone (`@k:c2`, `@sld:256/sp:4`).
    pub address: String,
    /// Where it is in the version the refusal looked at (`#2`, `L7`), if it has a place.
    pub position: Option<String>,
}

impl Candidate {
    pub fn new(address: impl Into<String>) -> Self {
        Candidate {
            address: address.into(),
            position: None,
        }
    }
    pub fn at(mut self, position: impl Into<String>) -> Self {
        self.position = Some(position.into());
        self
    }
    fn value(&self) -> Value {
        Value::obj([
            ("address", Value::from(self.address.as_str())),
            ("position", Value::from(self.position.as_deref())),
        ])
    }
}

/// The refusal for an address that names more than one thing (K-ADDR-2): `MPC-E170
/// ambiguous_address`, exit 2 (usage: the caller names one of the candidates; never a
/// hold), with `details.given` (the address as given), `details.what` (`row`, `column`,
/// `section`...) and `details.candidates` (each one's most durable address, and where it
/// is). `message` is the verb's own line.
pub fn ambiguous(
    message: impl Into<String>,
    what: &str,
    given: &str,
    candidates: &[Candidate],
) -> ErrorReport {
    ErrorReport::new(&ids::AMBIGUOUS_ADDRESS, message)
        .with("what", what)
        .with("given", given)
        .with(
            "candidates",
            Value::List(candidates.iter().map(Candidate::value).collect()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Forms = Forms::none().native().nth().line();

    #[test]
    fn each_form_parses_as_itself() {
        for (s, want) in [
            (
                "@sld:256/sp:4",
                Addr::Native(vec![
                    ("sld".into(), "256".into()),
                    ("sp".into(), "4".into()),
                ]),
            ),
            ("@lyid:42", Addr::Native(vec![("lyid".into(), "42".into())])),
            ("@k:r17", Addr::Key("r17".into())),
            ("key:c3", Addr::Key("c3".into())),
            ("#3", Addr::Nth(3)),
            ("L12", Addr::Line(12)),
            ("name:city", Addr::Name("city".into())),
            ("name:#3", Addr::Name("#3".into())),
            ("city", Addr::Word("city".into())),
            ("Lima", Addr::Word("Lima".into())),
        ] {
            assert_eq!(parse(s, ALL).unwrap(), want, "{s}");
        }
    }

    #[test]
    fn the_canonical_spelling_reads_back_the_same() {
        for s in ["@sld:256/sp:4", "@k:r1", "#2", "L9", "name:a b", "word"] {
            let a = parse(s, ALL).unwrap();
            assert_eq!(parse(&a.to_string(), ALL).unwrap(), a, "{s}");
        }
        assert_eq!(parse("key:c3", ALL).unwrap().to_string(), "@k:c3");
    }

    #[test]
    fn a_context_reads_only_its_forms() {
        // no lines (a grid's columns): L3 is a word; no native ids: @x:1 is a word
        let cols = Forms::none().nth();
        assert_eq!(parse("L3", cols).unwrap(), Addr::Word("L3".into()));
        assert_eq!(
            parse("@timestamp", cols).unwrap(),
            Addr::Word("@timestamp".into())
        );
        assert_eq!(parse("@k:c2", cols).unwrap(), Addr::Key("c2".into()));
        assert_eq!(parse("#3", Forms::none()).unwrap(), Addr::Word("#3".into()));
    }

    #[test]
    fn malformed_addresses_are_usage_errors() {
        for s in [
            "", "#0", "#x", "#-1", "L0", "@k:", "@:1", "@Sld:1", "@k:1/x:2", "@sld:",
        ] {
            assert!(parse(s, ALL).is_err(), "{s}");
        }
        assert_eq!(
            parse("#0", ALL).unwrap_err().to_string(),
            "#N: not a number from 1: \"0\""
        );
    }

    /// The check behind P12's "a number never falls back to an index": a bare number (any
    /// width, leading zeros) is a word in every context, never a position.
    fn numbers_are_never_positions(
        p: fn(&str, Forms) -> Result<Addr, AddrError>,
    ) -> Result<(), String> {
        for forms in [ALL, Forms::none(), Forms::none().nth()] {
            for s in ["1", "3", "42", "007", "2024", "18446744073709551616"] {
                match p(s, forms) {
                    Ok(Addr::Word(w)) if w == s => {}
                    other => return Err(format!("{s:?} parsed as {other:?}")),
                }
            }
        }
        Ok(())
    }

    #[test]
    fn a_number_never_falls_back_to_an_index() {
        numbers_are_never_positions(parse).unwrap();
    }

    /// The mutant "number falls back to index" is caught by the check above.
    #[test]
    fn mutant_number_falls_back_to_index_is_caught() {
        fn mutant(s: &str, f: Forms) -> Result<Addr, AddrError> {
            match s.parse::<usize>() {
                Ok(n) if n > 0 => Ok(Addr::Nth(n)),
                _ => parse(s, f),
            }
        }
        assert!(numbers_are_never_positions(mutant).is_err());
    }

    /// The check behind K-ADDR-2: two or more matches are refused, whatever their order.
    fn never_picks(u: fn(Vec<u32>) -> Unique<u32>) -> Result<(), String> {
        for items in [vec![1, 2], vec![2, 1], vec![7, 7], vec![1, 2, 3]] {
            if !matches!(u(items.clone()), Unique::Many(ref m) if *m == items) {
                return Err(format!("{items:?} was not refused with every candidate"));
            }
        }
        if u(vec![]) != Unique::None || u(vec![5]) != Unique::One(5) {
            return Err("none or one".into());
        }
        Ok(())
    }

    #[test]
    fn ambiguity_is_never_resolved_by_order() {
        never_picks(unique).unwrap();
    }

    /// The mutant "topmost wins" is caught by the check above.
    #[test]
    fn mutant_topmost_wins_is_caught() {
        fn topmost(v: Vec<u32>) -> Unique<u32> {
            match v.first() {
                None => Unique::None,
                Some(x) => Unique::One(*x),
            }
        }
        assert!(never_picks(topmost).is_err());
    }

    #[test]
    fn the_refusal_carries_the_candidates() {
        let r = ambiguous(
            "2 columns are named \"city\": c2, c3 (name one by key)",
            "column",
            "city",
            &[Candidate::new("@k:c2").at("#2"), Candidate::new("@k:c3")],
        );
        assert_eq!((r.id(), r.exit()), ("MPC-E170", 2));
        assert_eq!(
            r.to_value().get("details").unwrap().to_json(),
            r##"{"what":"column","given":"city","candidates":[{"address":"@k:c2","position":"#2"},{"address":"@k:c3","position":null}]}"##
        );
    }
}
