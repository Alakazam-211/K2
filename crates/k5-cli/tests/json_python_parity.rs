//! `out::json` prints what Python 3 prints.
//!
//! The tables under `tests/data` were written once by
//! `tests/data/gen_float_table.py`; every expected value in them is what
//! Python's `json.dumps(json.loads(text), **options)` returned. The named
//! tests below pin each Python habit on its own; their expected strings were
//! checked against python3 too.

use k5_cli::out::json::{
    parse, reprint, to_string, Error, Options, Text, Value, MAX_DEPTH, MAX_INT_DIGITS,
};

const FLOAT_TABLE: &str = include_str!("data/float_repr.tsv");
const DOCUMENT_TABLE: &str = include_str!("data/documents.tsv");
const REJECT_TABLE: &str = include_str!("data/rejects.tsv");

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

/// Reads the options column of `documents.tsv`.
fn options_from(names: &str) -> Options {
    let mut options = Options::default();
    if names == "-" {
        return options;
    }
    for name in names.split(',') {
        match name {
            "compact" => options.compact = true,
            "raw" => options.ensure_ascii = false,
            "sort" => options.sort_keys = true,
            _ => match name.strip_prefix("indent=") {
                Some(width) => {
                    options.indent = Some(
                        width
                            .parse()
                            .unwrap_or_else(|_| panic!("bad indent in options {names:?}")),
                    )
                }
                None => panic!("unknown option {name:?} in {names:?}"),
            },
        }
    }
    options
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

fn default_print(text: &str) -> String {
    print_with(text, &Options::default())
}

fn print_with(text: &str, options: &Options) -> String {
    match reprint(text, options) {
        Ok(printed) => printed,
        Err(error) => panic!("{text:?} did not print: {error}"),
    }
}

fn compact() -> Options {
    Options {
        compact: true,
        ..Options::default()
    }
}

fn indent(width: usize) -> Options {
    Options {
        indent: Some(width),
        ..Options::default()
    }
}

fn raw() -> Options {
    Options {
        ensure_ascii: false,
        ..Options::default()
    }
}

fn sorted() -> Options {
    Options {
        sort_keys: true,
        ..Options::default()
    }
}

// ---- The tables ----

#[test]
fn every_row_of_the_float_table_prints_as_python_does() {
    let mut rows = 0;
    let mut wrong = Vec::new();
    for (number, line) in FLOAT_TABLE.lines().enumerate() {
        let (input, expected) = line
            .split_once('\t')
            .unwrap_or_else(|| panic!("float_repr.tsv row {}: no tab", number + 1));
        rows += 1;
        match reprint(input, &Options::default()) {
            Ok(printed) if printed == expected => {}
            got => wrong.push(format!(
                "row {}: {input} -> left {got:?}, right (python) {expected:?}",
                number + 1
            )),
        }
    }
    assert!(rows >= 600, "float_repr.tsv has only {rows} rows");
    report("float_repr.tsv", rows, wrong);
}

#[test]
fn every_row_of_the_document_table_prints_as_python_does() {
    let mut rows = 0;
    let mut encode_errors = 0;
    let mut wrong = Vec::new();
    for (number, line) in DOCUMENT_TABLE.lines().enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            fields.len(),
            3,
            "documents.tsv row {}: expected 3 fields",
            number + 1
        );
        rows += 1;
        let options = options_from(fields[0]);
        let input = unescape(fields[1]);
        let got = reprint(&input, &options);
        let matches = if fields[2] == "!ENCODE-ERROR" {
            encode_errors += 1;
            matches!(got, Err(Error::LoneSurrogate { .. }))
        } else {
            got.as_deref() == Ok(unescape(fields[2]).as_str())
        };
        if !matches {
            wrong.push(format!(
                "row {} [{}]: {} -> left {got:?}, right (python, escaped) {:?}",
                number + 1,
                fields[0],
                fields[1],
                fields[2]
            ));
        }
    }
    assert!(rows >= 1000, "documents.tsv has only {rows} rows");
    assert!(
        encode_errors > 0,
        "documents.tsv has no lone surrogate row with ensure_ascii off"
    );
    report("documents.tsv", rows, wrong);
}

#[test]
fn every_row_of_the_reject_table_is_refused() {
    let mut rows = 0;
    let mut wrong = Vec::new();
    for (number, line) in REJECT_TABLE.lines().enumerate() {
        let escaped = line
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or_else(|| panic!("rejects.tsv row {}: not inside < >", number + 1));
        rows += 1;
        if let Ok(value) = parse(&unescape(escaped)) {
            wrong.push(format!(
                "row {}: {escaped} was read as {value:?}",
                number + 1
            ));
        }
    }
    assert!(rows >= 100, "rejects.tsv has only {rows} rows");
    report("rejects.tsv", rows, wrong);
}

// ---- Keys ----

#[test]
fn keys_keep_the_order_they_came_in() {
    assert_eq!(
        default_print(r#"{"zeta":1,"alpha":2,"Mid":3,"10":4,"9":5}"#),
        r#"{"zeta": 1, "alpha": 2, "Mid": 3, "10": 4, "9": 5}"#
    );
}

#[test]
fn a_repeated_key_keeps_its_first_place_and_its_last_value() {
    assert_eq!(
        default_print(r#"{"a":1,"a":2,"b":3}"#),
        r#"{"a": 2, "b": 3}"#
    );
    assert_eq!(
        default_print(r#"{"b":1,"a":2,"b":3,"a":4}"#),
        r#"{"b": 3, "a": 4}"#
    );
    assert_eq!(
        parse(r#"{"a":1,"a":2,"b":3}"#),
        Ok(Value::Object(vec![
            (Text::from("a"), Value::Int("2".into())),
            (Text::from("b"), Value::Int("3".into())),
        ]))
    );
}

#[test]
fn a_repeated_key_is_found_in_a_large_object_too() {
    // 40 keys, then the 4th and the 40th again.
    let mut input = String::from("{");
    let mut expected = String::from("{");
    for n in 0..40 {
        input.push_str(&format!("\"k{n}\": {n}, "));
        let value = match n {
            3 => "\"again\"".to_owned(),
            39 => "null".to_owned(),
            _ => n.to_string(),
        };
        expected.push_str(&format!("\"k{n}\": {value}, "));
    }
    input.push_str("\"k3\": \"again\", \"k39\": null, \"last\": true}");
    expected.push_str("\"last\": true}");
    assert_eq!(default_print(&input), expected);
}

#[test]
fn sort_keys_orders_by_code_point() {
    // U+FFFF sorts before an emoji (it would not in UTF-16), upper case
    // before lower case, and a shorter key before a longer one it starts.
    assert_eq!(
        print_with(
            "{\"\u{1F600}\":1,\"\u{FFFF}\":2,\"b\":3,\"B\":4,\"ab\":5,\"a\":6,\"\u{e9}\":7,\"\":8}",
            &sorted()
        ),
        r#"{"": 8, "B": 4, "a": 6, "ab": 5, "b": 3, "\u00e9": 7, "\uffff": 2, "\ud83d\ude00": 1}"#
    );
}

#[test]
fn sort_keys_sorts_every_level_and_leaves_arrays_alone() {
    assert_eq!(
        print_with(r#"{"b":{"d":1,"c":[{"z":1,"y":2},3,1]},"a":0}"#, &sorted()),
        r#"{"a": 0, "b": {"c": [{"y": 2, "z": 1}, 3, 1], "d": 1}}"#
    );
}

#[test]
fn sort_keys_places_a_lone_surrogate_key_by_its_code_point() {
    assert_eq!(
        print_with(
            "{\"\\ud83d\":1,\"\u{E000}\":2,\"\u{D7FF}\":3,\"\u{1F600}\":4}",
            &sorted()
        ),
        r#"{"\ud7ff": 3, "\ud83d": 1, "\ue000": 2, "\ud83d\ude00": 4}"#
    );
}

// ---- Integers ----

#[test]
fn an_integer_keeps_every_digit() {
    assert_eq!(
        default_print("123456789012345678901234567890"),
        "123456789012345678901234567890"
    );
    assert_eq!(
        default_print("[-123456789012345678901234567890, 18446744073709551616]"),
        "[-123456789012345678901234567890, 18446744073709551616]"
    );
    assert_eq!(
        parse("9007199254740993"),
        Ok(Value::Int("9007199254740993".into()))
    );
}

#[test]
fn minus_zero_without_a_point_is_the_integer_zero() {
    assert_eq!(default_print("-0"), "0");
    assert_eq!(parse("-0"), Ok(Value::Int("0".into())));
    assert_eq!(default_print("[-0, -0.0, -0e0]"), "[0, -0.0, -0.0]");
}

#[test]
fn an_integer_longer_than_pythons_limit_is_refused() {
    let longest = "1".repeat(MAX_INT_DIGITS);
    assert_eq!(default_print(&longest), longest);
    assert_eq!(default_print(&format!("-{longest}")), format!("-{longest}"));

    let too_long = "1".repeat(MAX_INT_DIGITS + 1);
    assert_eq!(parse(&too_long), Err(Error::IntTooLong { at: 0 }));
    assert_eq!(
        parse(&format!("[-{too_long}]")),
        Err(Error::IntTooLong { at: 1 })
    );
    // The limit is on integers only: a float may have any number of digits.
    assert_eq!(default_print(&format!("{too_long}.5")), "Infinity");
}

// ---- Floats ----

#[test]
fn floats_print_as_pythons_repr() {
    for (input, expected) in [
        ("1.50", "1.5"),
        ("1E5", "100000.0"),
        ("1e+5", "100000.0"),
        ("1e16", "1e+16"),
        ("1e15", "1000000000000000.0"),
        ("9999999999999998.0", "9999999999999998.0"),
        ("1.0e-7", "1e-07"),
        ("1.5e-7", "1.5e-07"),
        ("0.0001", "0.0001"),
        ("0.00009999", "9.999e-05"),
        ("0.00001", "1e-05"),
        ("0.1", "0.1"),
        ("123456789.123456789", "123456789.12345679"),
        ("5e-324", "5e-324"),
        ("1.7976931348623157e308", "1.7976931348623157e+308"),
        ("2.2250738585072014e-308", "2.2250738585072014e-308"),
        ("0.30000000000000004", "0.30000000000000004"),
        ("2.0", "2.0"),
        ("2e0", "2.0"),
        ("20e-1", "2.0"),
        ("1e22", "1e+22"),
        ("1e100", "1e+100"),
        ("-2.5E-3", "-0.0025"),
        ("0.0", "0.0"),
        ("-0.0", "-0.0"),
        ("0e0", "0.0"),
    ] {
        assert_eq!(default_print(input), expected, "input {input}");
    }
}

#[test]
fn a_float_halfway_between_two_shortest_forms_takes_the_even_digit() {
    // Each of these is exactly ...25 or ...75 past the last digit printed.
    for (input, expected) in [
        ("659774685697112.25", "659774685697112.2"),
        ("659774685697112.3", "659774685697112.2"),
        ("611998825668431.25", "611998825668431.2"),
        ("562949953421312.25", "562949953421312.2"),
        ("562949953421312.75", "562949953421312.8"),
        ("1125899906842624.75", "1125899906842624.8"),
        ("-1125899906842624.25", "-1125899906842624.2"),
        ("1.00000762939453125", "1.0000076293945312"),
        ("1.00002288818359375", "1.0000228881835938"),
        // 2**-24: the even candidate (...062) is a different float, so the
        // odd one stays.
        ("5.9604644775390625e-08", "5.960464477539063e-08"),
    ] {
        assert_eq!(default_print(input), expected, "input {input}");
    }
}

#[test]
fn nan_and_the_infinities_are_read_and_printed() {
    assert_eq!(
        default_print("[NaN, Infinity, -Infinity]"),
        "[NaN, Infinity, -Infinity]"
    );
    // Too large for a float is infinity; too small is zero.
    assert_eq!(
        default_print("[1e400, -1e400, 1e-400, -1e-400]"),
        "[Infinity, -Infinity, 0.0, -0.0]"
    );
    match parse("NaN") {
        Ok(Value::Float(number)) => assert!(number.is_nan()),
        other => panic!("NaN was read as {other:?}"),
    }
}

// ---- Strings ----

#[test]
fn an_emoji_is_a_lower_case_surrogate_pair_in_ascii_mode() {
    assert_eq!(default_print("\"\u{1F600}\""), r#""\ud83d\ude00""#);
    assert_eq!(default_print(r#""\uD83D\uDE00""#), r#""\ud83d\ude00""#);
    assert_eq!(
        default_print("\"caf\u{e9} \u{4e2d}\""),
        r#""caf\u00e9 \u4e2d""#
    );
}

#[test]
fn non_ascii_goes_out_as_it_is_when_ensure_ascii_is_off() {
    assert_eq!(print_with("\"\u{1F600}\"", &raw()), "\"\u{1F600}\"");
    assert_eq!(print_with(r#""\ud83d\ude00""#, &raw()), "\"\u{1F600}\"");
    assert_eq!(print_with(r#""caf\u00e9""#, &raw()), "\"caf\u{e9}\"");
}

#[test]
fn del_is_escaped_in_ascii_mode_only() {
    assert_eq!(default_print("\"a\u{7f}b\""), r#""a\u007fb""#);
    assert_eq!(print_with("\"a\u{7f}b\"", &raw()), "\"a\u{7f}b\"");
    assert_eq!(print_with(r#""\u007f""#, &raw()), "\"\u{7f}\"");
}

#[test]
fn control_characters_are_escaped_in_both_modes() {
    let input = r#""\n\r\t\b\f\u0000\u0001\u001b\u001f""#;
    let expected = r#""\n\r\t\b\f\u0000\u0001\u001b\u001f""#;
    assert_eq!(default_print(input), expected);
    assert_eq!(print_with(input, &raw()), expected);
    // The long spellings of the five short ones come back short.
    assert_eq!(
        default_print(r#""\u000a\u000d\u0009\u0008\u000c""#),
        r#""\n\r\t\b\f""#
    );
}

#[test]
fn quotes_and_backslashes_are_escaped_and_slashes_are_not() {
    assert_eq!(
        default_print(r#""say \"hi\" \\ a\/b a/b""#),
        r#""say \"hi\" \\ a/b a/b""#
    );
    assert_eq!(
        parse(r#""a\/b""#),
        Ok(Value::Str(Text::from("a/b")))
    );
}

#[test]
fn a_lone_surrogate_is_read_and_printed_back_in_ascii_mode() {
    assert_eq!(default_print(r#""\ud83d""#), r#""\ud83d""#);
    assert_eq!(default_print(r#""\ude00""#), r#""\ude00""#);
    // A high one joins only with a low one right after it.
    assert_eq!(default_print(r#""\ud83dA""#), r#""\ud83dA""#);
    assert_eq!(default_print(r#""\ude00\ud83d""#), r#""\ude00\ud83d""#);
    assert_eq!(
        default_print(r#""\ud83d\ud83d\ude00""#),
        r#""\ud83d\ud83d\ude00""#
    );
    assert_eq!(default_print(r#""\ud83d\u00e9""#), r#""\ud83d\u00e9""#);

    match parse(r#""x\ud83d""#) {
        Ok(Value::Str(text)) => assert_eq!(text.as_str(), None),
        other => panic!("read as {other:?}"),
    }
    match parse(r#""\ud83d\ude00""#) {
        Ok(Value::Str(text)) => assert_eq!(text.as_str(), Some("\u{1F600}")),
        other => panic!("read as {other:?}"),
    }
}

#[test]
fn a_lone_surrogate_cannot_be_printed_when_ensure_ascii_is_off() {
    // Python fails here too, when it writes the text out as UTF-8.
    assert_eq!(
        reprint(r#""\ud83d""#, &raw()),
        Err(Error::LoneSurrogate { code_point: 0xD83D })
    );
    assert_eq!(
        reprint(r#"{"ok":1,"deep":[{"\udead":2}]}"#, &raw()),
        Err(Error::LoneSurrogate { code_point: 0xDEAD })
    );
}

// ---- The four switches ----

#[test]
fn the_default_separators_have_a_space_after_them() {
    assert_eq!(
        default_print(r#"{"a":[1,2,{"b":null}],"c":"d"}"#),
        r#"{"a": [1, 2, {"b": null}], "c": "d"}"#
    );
}

#[test]
fn compact_has_no_spaces() {
    assert_eq!(
        print_with(r#"{"a": [1, 2, {"b": null}], "c": "d"}"#, &compact()),
        r#"{"a":[1,2,{"b":null}],"c":"d"}"#
    );
}

#[test]
fn indent_puts_one_item_per_line() {
    let input = r#"{"b":[1,{"a":[],"c":{}}],"a":"x"}"#;
    assert_eq!(
        print_with(input, &indent(2)),
        "{\n  \"b\": [\n    1,\n    {\n      \"a\": [],\n      \"c\": {}\n    }\n  ],\n  \"a\": \"x\"\n}"
    );
    assert_eq!(
        print_with(input, &indent(1)),
        "{\n \"b\": [\n  1,\n  {\n   \"a\": [],\n   \"c\": {}\n  }\n ],\n \"a\": \"x\"\n}"
    );
    // Zero still breaks the lines.
    assert_eq!(
        print_with(r#"{"b":[1,2]}"#, &indent(0)),
        "{\n\"b\": [\n1,\n2\n]\n}"
    );
}

#[test]
fn indent_leaves_empty_containers_and_plain_values_on_one_line() {
    for text in ["{}", "[]", "1", "\"s\"", "null", "1.5"] {
        assert_eq!(print_with(text, &indent(2)), text);
    }
    assert_eq!(print_with("[[]]", &indent(2)), "[\n  []\n]");
}

#[test]
fn compact_with_indent_drops_the_space_after_the_colon() {
    let options = Options {
        compact: true,
        indent: Some(1),
        sort_keys: true,
        ..Options::default()
    };
    assert_eq!(
        print_with(r#"{"b":[1,{"a":[],"c":{}}],"a":"x"}"#, &options),
        "{\n \"a\":\"x\",\n \"b\":[\n  1,\n  {\n   \"a\":[],\n   \"c\":{}\n  }\n ]\n}"
    );
}

#[test]
fn the_default_options_are_pythons_defaults() {
    assert_eq!(
        Options::default(),
        Options {
            compact: false,
            indent: None,
            ensure_ascii: true,
            sort_keys: false,
        }
    );
}

#[test]
fn to_string_prints_a_value_built_by_hand() {
    let value = Value::Object(vec![
        (Text::from("name"), Value::Str(Text::from("Ren\u{e9}e"))),
        (
            Text::from(String::from("list")),
            Value::Array(vec![
                Value::Null,
                Value::Bool(true),
                Value::Bool(false),
                Value::Int("-7".into()),
                Value::Float(1e16),
            ]),
        ),
    ]);
    assert_eq!(
        to_string(&value, &Options::default()),
        Ok(r#"{"name": "Ren\u00e9e", "list": [null, true, false, -7, 1e+16]}"#.to_owned())
    );
}

// ---- What is not JSON ----

#[test]
fn whitespace_is_space_tab_line_feed_and_carriage_return_only() {
    assert_eq!(default_print(" \t\r\n[ 1 ,\t2\r\n]\n "), "[1, 2]");
    for text in ["\u{0c}1", "\u{0b}1", "\u{a0}1", "1\u{a0}", "\u{feff}1"] {
        assert!(parse(text).is_err(), "{text:?} was read");
    }
}

#[test]
fn a_raw_control_character_inside_a_string_is_refused() {
    for text in ["\"\t\"", "\"\n\"", "\"\r\"", "\"\u{0}\"", "\"a\u{1f}b\""] {
        assert!(
            matches!(
                parse(text),
                Err(Error::Syntax {
                    what: "control character inside a string",
                    ..
                })
            ),
            "{text:?}"
        );
    }
    // DEL is not a control character to Python's reader.
    assert_eq!(default_print("\"\u{7f}\""), r#""\u007f""#);
}

#[test]
fn trailing_commas_comments_and_single_quotes_are_refused() {
    for text in [
        "[1,]",
        r#"{"a":1,}"#,
        "[,1]",
        "[1,,2]",
        "[1 2]",
        "{'a':1}",
        "'a'",
        "{a:1}",
        "{1:2}",
        "[//c\n1]",
        "/* c */ 1",
        "# c\n1",
    ] {
        assert!(parse(text).is_err(), "{text:?} was read");
    }
}

#[test]
fn numbers_python_does_not_read_are_refused() {
    for text in [
        "01", "-01", "00", "1.", "1.e5", ".5", "-.5", "1e", "1e+", "+1", "-", "--1", "0x10",
        "1_0", "1e5x", "1.2.3", "nan", "inf", "-NaN", "+Infinity", "-Infinit", "\u{661}",
    ] {
        assert!(parse(text).is_err(), "{text:?} was read");
    }
}

#[test]
fn broken_escapes_and_unfinished_text_are_refused() {
    for text in [
        r#""\x41""#,
        r#""\U0041""#,
        r#""\a""#,
        r#""\u12g4""#,
        r#""\u+123""#,
        r#""\u00""#,
        r#""\ud83d\u12""#,
        r#""\ud83d\uzzzz""#,
        r#""abc"#,
        r#""abc\"#,
        "[1",
        r#"{"a":1"#,
        r#"{"a""#,
        "",
        "  ",
        "tru",
        "True",
        "[1] x",
        "1 2",
        "{} {}",
    ] {
        assert!(parse(text).is_err(), "{text:?} was read");
    }
}

#[test]
fn an_error_says_what_and_where() {
    assert_eq!(
        parse("[1, 2,]"),
        Err(Error::Syntax {
            what: "expecting a value",
            at: 6
        })
    );
    assert_eq!(
        parse("[1] x").map_err(|error| error.to_string()),
        Err("invalid JSON: extra data after the value at byte 4".to_owned())
    );
    assert_eq!(
        Error::LoneSurrogate { code_point: 0xD83D }.to_string(),
        "cannot write JSON as UTF-8: lone surrogate \\ud83d in a string"
    );
}

#[test]
fn nesting_is_read_up_to_the_depth_limit() {
    let deepest = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
    assert_eq!(default_print(&deepest), deepest);

    let too_deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
    assert_eq!(parse(&too_deep), Err(Error::TooDeep { at: MAX_DEPTH }));

    let objects = format!("{}1{}", "{\"a\":".repeat(MAX_DEPTH + 1), "}".repeat(MAX_DEPTH + 1));
    assert_eq!(
        parse(&objects),
        Err(Error::TooDeep {
            at: MAX_DEPTH * "{\"a\":".len()
        })
    );
}
