//! Heading slugs from the shared data file, `tests/data/heading-slugs.json`: the single source
//! of the slug cases, which a host's own slug code runs too. Each entry is a heading and its
//! slug (`{heading, slug}`: `section::slug` on the heading's text), or a document's headings in
//! order and their numbered slugs (`{headings, slugs}`: `section::addresses` on a document of
//! those headings). A floor on the counts keeps the file from emptying silently.

use k2_mpc_markdown::section::{addresses, slug};

const DATA: &str = include_str!("data/heading-slugs.json");

/// The JSON this file uses: arrays, objects and strings (no numbers, booleans or null).
#[derive(Debug)]
enum Json {
    Str(String),
    List(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            other => panic!("not a string: {other:?}"),
        }
    }
    fn strs(&self) -> Vec<&str> {
        match self {
            Json::List(items) => items.iter().map(Json::str).collect(),
            other => panic!("not a list: {other:?}"),
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.at).is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }
    fn eat(&mut self, b: u8) {
        self.ws();
        let got = self.s.get(self.at);
        assert_eq!(got, Some(&b), "expected {:?} at {}", b as char, self.at);
        self.at += 1;
    }
    fn peek(&mut self) -> u8 {
        self.ws();
        *self
            .s
            .get(self.at)
            .expect("unexpected end of the data file")
    }
    fn value(&mut self) -> Json {
        match self.peek() {
            b'"' => Json::Str(self.string()),
            b'[' => {
                self.eat(b'[');
                let mut items = Vec::new();
                while self.peek() != b']' {
                    items.push(self.value());
                    if self.peek() == b',' {
                        self.eat(b',');
                    }
                }
                self.eat(b']');
                Json::List(items)
            }
            b'{' => {
                self.eat(b'{');
                let mut fields = Vec::new();
                while self.peek() != b'}' {
                    let k = self.string();
                    self.eat(b':');
                    fields.push((k, self.value()));
                    if self.peek() == b',' {
                        self.eat(b',');
                    }
                }
                self.eat(b'}');
                Json::Obj(fields)
            }
            b => panic!("unsupported JSON at {}: {:?}", self.at, b as char),
        }
    }
    fn hex4(&mut self) -> u32 {
        let h = std::str::from_utf8(&self.s[self.at..self.at + 4]).unwrap();
        self.at += 4;
        u32::from_str_radix(h, 16).expect("four hex digits")
    }
    fn string(&mut self) -> String {
        self.eat(b'"');
        let mut out = String::new();
        loop {
            let rest = std::str::from_utf8(&self.s[self.at..]).unwrap();
            let c = rest.chars().next().expect("an unterminated string");
            self.at += c.len_utf8();
            match c {
                '"' => return out,
                '\\' => {
                    let e = self.s[self.at];
                    self.at += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'u' => {
                            let mut u = self.hex4();
                            if (0xD800..0xDC00).contains(&u) {
                                let next = &self.s[self.at..self.at + 2];
                                assert_eq!(next, b"\\u", "a lone surrogate");
                                self.at += 2;
                                let lo = self.hex4();
                                u = 0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00);
                            }
                            out.push(char::from_u32(u).expect("a scalar value"));
                        }
                        other => panic!("unsupported escape \\{}", other as char),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

fn data() -> Vec<Json> {
    let mut p = Parser {
        s: DATA.as_bytes(),
        at: 0,
    };
    let Json::List(entries) = p.value() else {
        panic!("the data file is an array")
    };
    p.ws();
    assert_eq!(p.at, DATA.len(), "nothing after the array");
    entries
}

#[test]
fn every_heading_slug_in_the_data_file() {
    let entries = data();
    let mut n = 0;
    for e in &entries {
        let (Some(h), Some(want)) = (e.get("heading"), e.get("slug")) else {
            continue;
        };
        assert_eq!(slug(h.str()), want.str(), "heading {:?}", h.str());
        n += 1;
    }
    assert!(
        n >= 20,
        "only {n} heading rows: the data file lost its cases"
    );
}

#[test]
fn every_documents_numbered_slugs_in_the_data_file() {
    let entries = data();
    let mut n = 0;
    for e in &entries {
        let (Some(hs), Some(want)) = (e.get("headings"), e.get("slugs")) else {
            continue;
        };
        let (hs, want) = (hs.strs(), want.strs());
        assert_eq!(hs.len(), want.len(), "{hs:?}: one slug a heading");
        let doc: String = hs.iter().map(|h| format!("## {h}\n\npara\n\n")).collect();
        let got: Vec<String> = addresses(&doc).into_iter().map(|a| a.slug).collect();
        assert_eq!(got, want, "{hs:?}");
        n += 1;
    }
    assert!(n >= 3, "only {n} documents: the data file lost its cases");
    // each entry is one of the two shapes
    assert!(entries
        .iter()
        .all(|e| e.get("heading").is_some() != e.get("headings").is_some()));
}
