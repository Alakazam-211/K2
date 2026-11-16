//! `transport::encode` escapes as Python 3 does.
//!
//! The tables under `tests/data` were written once by
//! `tests/data/gen_quote_table.py`; every expected value in them is what
//! Python's `urllib.parse.quote(value, safe='')` returned. The named tests
//! below pin each rule on its own; their expected strings were checked
//! against python3 too.

use k5_cli::transport::encode::{quote, quote_bytes};

const QUOTE_TABLE: &str = include_str!("data/quote.tsv");
const QUOTE_BYTES_TABLE: &str = include_str!("data/quote_bytes.tsv");

/// Undoes the escaping the generator documents: `\\`, `\n`, `\t`, `\r` and
/// `\xNN`.
fn unescape(field: &str) -> String {
    let mut out = String::new();
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                let byte = u8::from_str_radix(&hex, 16)
                    .unwrap_or_else(|_| panic!("bad \\x escape in table field {field:?}"));
                out.push(char::from(byte));
            }
            other => panic!("bad escape {other:?} in table field {field:?}"),
        }
    }
    out
}

/// Reads the lower-case hex input column of `quote_bytes.tsv`.
fn from_hex(field: &str) -> Vec<u8> {
    assert!(field.len().is_multiple_of(2), "odd hex field {field:?}");
    (0..field.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&field[at..at + 2], 16)
                .unwrap_or_else(|_| panic!("bad hex field {field:?}"))
        })
        .collect()
}

/// Fails with every wrong row listed (the first forty in full).
fn report(table: &str, rows: usize, wrong: Vec<String>) {
    assert!(
        wrong.is_empty(),
        "{table}: {} of {rows} rows differ from Python\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ---- The tables ----

#[test]
fn every_row_of_the_quote_table_escapes_as_python_does() {
    let mut rows = 0;
    let mut wrong = Vec::new();
    for (number, line) in QUOTE_TABLE.lines().enumerate() {
        let (escaped, expected) = line
            .split_once('\t')
            .unwrap_or_else(|| panic!("quote.tsv row {}: no tab", number + 1));
        rows += 1;
        let got = quote(&unescape(escaped));
        if got != expected {
            wrong.push(format!(
                "row {}: {escaped} -> left {got:?}, right (python) {expected:?}",
                number + 1
            ));
        }
    }
    assert!(rows >= 1000, "quote.tsv has only {rows} rows");
    report("quote.tsv", rows, wrong);
}

#[test]
fn every_row_of_the_bytes_table_escapes_as_python_does() {
    let mut rows = 0;
    let mut not_utf8 = 0;
    let mut wrong = Vec::new();
    for (number, line) in QUOTE_BYTES_TABLE.lines().enumerate() {
        let (hex, expected) = line
            .split_once('\t')
            .unwrap_or_else(|| panic!("quote_bytes.tsv row {}: no tab", number + 1));
        rows += 1;
        let input = from_hex(hex);
        if std::str::from_utf8(&input).is_err() {
            not_utf8 += 1;
        }
        let got = quote_bytes(&input);
        if got != expected {
            wrong.push(format!(
                "row {}: {hex} -> left {got:?}, right (python) {expected:?}",
                number + 1
            ));
        }
    }
    assert!(rows >= 500, "quote_bytes.tsv has only {rows} rows");
    assert!(
        not_utf8 >= 100,
        "quote_bytes.tsv has only {not_utf8} rows that are not UTF-8"
    );
    report("quote_bytes.tsv", rows, wrong);
}

// ---- The rules one by one ----

#[test]
fn only_ascii_letters_digits_and_four_marks_stay_as_they_are() {
    let kept: String = (0u8..=255)
        .filter(|&byte| quote_bytes(&[byte]) == char::from(byte).to_string())
        .map(char::from)
        .collect();
    assert_eq!(
        kept,
        "-.0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz~"
    );
    assert_eq!(quote("_.-~azAZ09"), "_.-~azAZ09");
}

#[test]
fn a_slash_is_escaped() {
    assert_eq!(quote("/a/b"), "%2Fa%2Fb");
    assert_eq!(quote("/"), "%2F");
}

#[test]
fn a_space_is_percent_20_and_a_plus_is_escaped() {
    assert_eq!(quote("a b"), "a%20b");
    assert_eq!(quote("a+b c"), "a%2Bb%20c");
}

#[test]
fn query_string_marks_and_the_percent_sign_are_escaped() {
    assert_eq!(quote("a=1&b=2?#%"), "a%3D1%26b%3D2%3F%23%25");
}

#[test]
fn hex_digits_are_upper_case() {
    assert_eq!(quote("\u{7f}\u{ff}"), "%7F%C3%BF");
    assert_eq!(quote("caf\u{e9} \u{4e2d}"), "caf%C3%A9%20%E4%B8%AD");
}

#[test]
fn an_emoji_is_four_escaped_utf8_bytes() {
    assert_eq!(quote("\u{1F600}"), "%F0%9F%98%80");
}

#[test]
fn the_empty_string_stays_empty() {
    assert_eq!(quote(""), "");
    assert_eq!(quote_bytes(b""), "");
}

#[test]
fn bytes_that_are_not_utf8_are_escaped_one_by_one() {
    assert_eq!(quote_bytes(b"\xff\x80a\xed\xa0\x80"), "%FF%80a%ED%A0%80");
    assert_eq!(quote_bytes(b"\x00\x7f"), "%00%7F");
}

#[test]
fn quote_is_quote_bytes_of_the_utf8() {
    let text = "Ren\u{e9}e /x?y=1&z=\u{1F331}";
    assert_eq!(quote(text), quote_bytes(text.as_bytes()));
}
