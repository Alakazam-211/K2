//! Every conformance vector, as data. `examples/gen.rs` writes them to `vectors/`, and
//! `tests/vectors.rs` checks the tree still matches. The widget conversion pairs describe Board's
//! widget store shapes: portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT (see
//! `THIRD-PARTY-NOTICES.md`).
#![allow(dead_code, clippy::vec_init_then_push)]

use crate::support::forge::*;
use k2_mpc_board::{apply_group, schema, verbs, Board, Limits};

pub struct Case {
    pub layer: &'static str,
    pub name: String,
    pub input: Vec<u8>,
    pub canonical: Option<Vec<u8>>,
    pub refusal: Option<&'static str>,
    pub guard: Option<String>,
    pub stable_guard: bool,
    pub limits: Vec<(&'static str, u64)>,
    pub revs: Vec<(&'static str, String)>,
    pub settings_rev: Option<String>,
    pub missing: Vec<String>,
    pub cleared: Option<Vec<u8>>,
    pub bad_ids: Option<Vec<u8>>,
}

impl Case {
    fn new(layer: &'static str, name: &str, input: Vec<u8>) -> Case {
        Case {
            layer,
            name: name.to_string(),
            input,
            canonical: None,
            refusal: None,
            guard: None,
            stable_guard: false,
            limits: Vec::new(),
            revs: Vec::new(),
            settings_rev: None,
            missing: Vec::new(),
            cleared: None,
            bad_ids: None,
        }
    }
    pub fn extension(&self) -> &'static str {
        if self.layer == "zip" {
            "k5board"
        } else {
            "json"
        }
    }
}

fn ok(layer: &'static str, name: &str, input: &[u8], canonical: &str) -> Case {
    let mut c = Case::new(layer, name, input.to_vec());
    c.canonical = Some(canonical.as_bytes().to_vec());
    c
}

fn no(layer: &'static str, name: &str, input: &[u8], code: &'static str) -> Case {
    let mut c = Case::new(layer, name, input.to_vec());
    c.refusal = Some(code);
    c
}

fn limited(mut c: Case, limits: &[(&'static str, u64)]) -> Case {
    c.limits = limits.to_vec();
    c
}

fn canon_cases() -> Vec<Case> {
    let mut v = Vec::new();
    v.push(ok(
        "canon",
        "numbers",
        b"[0.1,1.0,1E2,-0,-0.0,1e-7,0.000001,1.5e3,9007199254740992,-9007199254740992,-1.5,1e0,0e5,100,0.30000000000000004,5e-324,4.50,2e-3,0.000000000000000000000000001]",
        "[0.1,1,100,0,0,1e-7,0.000001,1500,9007199254740992,-9007199254740992,-1.5,1,0,100,0.30000000000000004,5e-324,4.5,0.002,1e-27]",
    ));
    v.push(ok(
        "canon",
        "number-spellings",
        b"[0.10000000000000001,1.0000000000000001,333333333.33333329,1.00000000000000000000,1000e-3,0.9007199254740992e16,3e-324,0e99999999999999999999,-1.0000000000000000e0]",
        "[0.1,1,333333333.3333333,1,1,9007199254740992,5e-324,0,-1]",
    ));
    v.push(ok(
        "canon",
        "strings",
        "{\"b\":\"\u{e9}\",\"a\":\"\\u00e9\",\"c\":\"\\u20ac\\ud83d\\ude00\",\"d\":\"\\/\\b\\f\\n\\r\\t\\\"\\\\\\u0001\\u007f\\u2028\"}".as_bytes(),
        "{\"a\":\"\u{e9}\",\"b\":\"\u{e9}\",\"c\":\"\u{20ac}\u{1F600}\",\"d\":\"/\\b\\f\\n\\r\\t\\\"\\\\\\u0001\u{7f}\u{2028}\"}",
    ));
    v.push(ok(
        "canon",
        "key-order",
        "{\"\\uff5e\":2,\"\\ud83d\\ude00\":1,\"\u{e9}\":3,\"a\":4,\"B\":5}".as_bytes(),
        "{\"B\":5,\"a\":4,\"\u{e9}\":3,\"\u{1F600}\":1,\"\u{ff5e}\":2}",
    ));
    v.push(ok(
        "canon",
        "rfc8785",
        "{\"numbers\":[4.50,2e-3,0.000000000000000000000000001],\"string\":\"\\u20ac$\\u000F\\u000aA'\\u0042\\u0022\\u005c\\\\\\\"\\/\",\"literals\":[null,true,false]}".as_bytes(),
        "{\"literals\":[null,true,false],\"numbers\":[4.5,0.002,1e-27],\"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}",
    ));
    v.push(ok(
        "canon",
        "spaces",
        b" \n\t{ \"a\" : [ 1 , 2 ] , \"b\" : { } }\r\n",
        "{\"a\":[1,2],\"b\":{}}",
    ));
    v.push(ok(
        "canon",
        "guard-shape",
        b"{\"version\":1,\"parts\":[],\"min_reader\":1,\"lineage\":\"0123456789abcdef0123456789abcdef\",\"format\":\"k5board\"}",
        "{\"format\":\"k5board\",\"lineage\":\"0123456789abcdef0123456789abcdef\",\"min_reader\":1,\"parts\":[],\"version\":1}",
    ));
    let mut deep_ok = Vec::new();
    deep_ok.extend(std::iter::repeat(b'[').take(64));
    deep_ok.extend(std::iter::repeat(b']').take(64));
    v.push(ok(
        "canon",
        "depth-64",
        &deep_ok,
        std::str::from_utf8(&deep_ok).unwrap_or(""),
    ));
    let mut deep = Vec::new();
    deep.extend(std::iter::repeat(b'[').take(65));
    deep.extend(std::iter::repeat(b']').take(65));
    v.push(no("canon", "depth-65", &deep, "limit_depth"));
    v.push(limited(
        no("canon", "depth-limit", b"[[[1]]]", "limit_depth"),
        &[("depth", 2)],
    ));
    v.push(limited(
        no("canon", "values-3", b"[1,2]", "limit_values"),
        &[("values", 2)],
    ));

    for (name, input, code) in [
        (
            "num-long-tail",
            &b"[0.1000000000000000055]"[..],
            "json_number_inexact",
        ),
        ("num-2p53p1", b"[9007199254740993]", "json_number_range"),
        (
            "num-2p53-half",
            b"[9007199254740992.5]",
            "json_number_range",
        ),
        (
            "num-2p53p1-exp",
            b"[9007199254740993e0]",
            "json_number_range",
        ),
        (
            "num-2p53p1-neg",
            b"[-9007199254740993]",
            "json_number_range",
        ),
        (
            "num-18-digits",
            b"[0.100000000000000001]",
            "json_number_inexact",
        ),
        (
            "num-big-exp",
            b"[1e99999999999999999999]",
            "json_number_range",
        ),
        (
            "num-small-exp",
            b"[1e-99999999999999999999]",
            "json_number_inexact",
        ),
        ("num-neg-1e-400", b"[-1e-400]", "json_number_inexact"),
        ("num-2p53p2", b"[9007199254740994]", "json_number_range"),
        ("num-neg-big", b"[-9007199254740994]", "json_number_range"),
        ("num-1e400", b"[1e400]", "json_number_range"),
        ("num-1e22", b"[1e22]", "json_number_range"),
        ("num-1e-400", b"[1e-400]", "json_number_inexact"),
        ("num-plus", b"[+1]", "json_syntax"),
        ("num-lead-zero", b"[01]", "json_syntax"),
        ("num-bare-dot", b"[1.]", "json_syntax"),
        ("num-dot-first", b"[.5]", "json_syntax"),
        ("num-no-exp", b"[1e]", "json_syntax"),
        ("num-nan", b"[NaN]", "json_syntax"),
        ("num-inf", b"[Infinity]", "json_syntax"),
        ("dup-key", b"{\"a\":1,\"a\":2}", "json_duplicate_key"),
        (
            "dup-escaped",
            b"{\"a\":1,\"\\u0061\":2}",
            "json_duplicate_key",
        ),
        ("lone-high", b"[\"\\ud800\"]", "json_surrogate"),
        ("lone-low", b"[\"\\udc00\"]", "json_surrogate"),
        ("high-then-a", b"[\"\\ud800\\u0041\"]", "json_surrogate"),
        ("bad-utf8", b"[\"\xff\"]", "json_utf8"),
        ("bom", b"\xef\xbb\xbf[]", "json_syntax"),
        ("comma-end", b"[1,]", "json_syntax"),
        ("after-doc", b"[] []", "json_syntax"),
        ("ctrl-in-str", b"[\"a\nb\"]", "json_syntax"),
        ("bad-escape", b"[\"\\x\"]", "json_syntax"),
        ("short-hex", b"[\"\\u12\"]", "json_syntax"),
        ("open-str", b"[\"abc", "json_syntax"),
        ("key-not-str", b"{1:2}", "json_syntax"),
        ("empty-doc", b"", "json_syntax"),
    ] {
        v.push(no("canon", name, input, code));
    }
    let mut long = String::from("[0.");
    long.push_str(&"0".repeat(62));
    long.push_str("1]");
    v.push(no(
        "canon",
        "num-65-bytes",
        long.as_bytes(),
        "limit_number_literal",
    ));
    v
}

const EMPTY: &str = "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{}}";
const P1: &str = "p-0123456789abcdef";
const P2: &str = "p-1123456789abcdef";
const HASH: &str = "sha256-9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

/// A board around these canonical object bodies, with two people.
fn board(objects: &[&str]) -> String {
    format!(
        "{{\"frames\":[],\"objects\":[{}],\"people\":{{\"{P1}\":{{\"name\":\"Ann\"}},\"{P2}\":{{}}}},\"settings\":{{}}}}",
        objects.join(",")
    )
}

fn sticky(fields: &str) -> String {
    format!("{{{fields}\"id\":\"s-1\",\"type\":\"sticky\"}}")
}

/// One field on a sticky, sorted before `id`.
fn field_case(name: &str, field: &str, code: &'static str) -> Case {
    no("json", name, board(&[&sticky(field)]).as_bytes(), code)
}

fn typed(kind: &str, fields: &str) -> String {
    format!("{{{fields}\"id\":\"o-1\",\"type\":\"{kind}\"}}")
}

fn type_case(name: &str, kind: &str, fields: &str, code: &'static str) -> Case {
    no(
        "json",
        name,
        board(&[&typed(kind, fields)]).as_bytes(),
        code,
    )
}

fn json_ok(name: &str, text: &str) -> Case {
    ok("json", name, text.as_bytes(), text)
}

fn all_types_board() -> String {
    let at = "2026-11-03T10:00:00.000Z";
    let objects = [
        format!("{{\"color\":\"yellow\",\"created\":{{\"at\":\"{at}\",\"by\":\"{P1}\"}},\"frame\":\"f-1\",\"group\":\"g-1\",\"id\":\"s-1\",\"link\":\"https://example.com/a\",\"locked\":true,\"reactions\":{{\"\u{1F44D}\":[\"{P1}\",\"{P2}\"]}},\"rot\":12.5,\"tags\":[\"a\",\"b\"],\"text\":\"hi\",\"type\":\"sticky\",\"votes\":{{\"{P1}\":2}},\"x\":-1.5,\"y\":0.1}}"),
        "{\"checks\":[{\"done\":false,\"id\":\"c1\",\"text\":\"one\"}],\"id\":\"t-1\",\"text\":\"body\",\"type\":\"text\"}".to_string(),
        "{\"fill\":\"#aabbcc\",\"id\":\"h-1\",\"shape\":\"cloud\",\"type\":\"shape\"}".to_string(),
        "{\"from\":{\"anchor\":\"top\",\"hint\":{\"x\":1,\"y\":2},\"id\":\"s-1\"},\"id\":\"c-1\",\"label\":\"go\",\"to\":{\"x\":5,\"y\":6},\"type\":\"connector\"}".to_string(),
        "{\"id\":\"p-1\",\"points\":[[0,0],[1.5,2.5]],\"type\":\"pen\",\"x\":3,\"y\":4}".to_string(),
        "{\"id\":\"f-1\",\"title\":\"Part one\",\"type\":\"frame\"}".to_string(),
        "{\"assignee\":\"Sam\",\"id\":\"k-1\",\"status\":\"todo\",\"title\":\"Do it\",\"type\":\"card\"}".to_string(),
        "{\"columns\":[{\"id\":\"a\",\"width\":40},{\"id\":\"b\"}],\"id\":\"b-1\",\"rows\":[{\"cells\":{\"a\":\"x\"},\"id\":\"r1\"},{\"id\":\"r2\"}],\"type\":\"table\"}".to_string(),
        "{\"id\":\"m-1\",\"parent\":\"m-2\",\"text\":\"Idea\",\"type\":\"mind\"}".to_string(),
        "{\"id\":\"m-2\",\"text\":\"Root\",\"type\":\"mind\"}".to_string(),
        format!("{{\"id\":\"q-1\",\"replies\":[{{\"at\":\"{at}\",\"by\":\"{P2}\",\"id\":\"r1\",\"text\":\"yes\"}}],\"resolved\":true,\"target\":\"s-1\",\"text\":\"why\",\"type\":\"comment\"}}"),
        format!("{{\"alt\":\"a picture\",\"crop\":{{\"h\":0.5,\"w\":0.5,\"x\":0.25,\"y\":0.25}},\"id\":\"i-1\",\"src\":{{\"blob\":\"{HASH}\"}},\"type\":\"image\"}}"),
    ];
    let joined: Vec<&str> = objects.iter().map(String::as_str).collect();
    format!(
        "{{\"frames\":[\"f-1\"],\"objects\":[{}],\"people\":{{\"{P1}\":{{\"name\":\"Ann\"}},\"{P2}\":{{}}}},\"settings\":{{\"title\":\"Plan\",\"voting\":{{\"active\":true,\"perPerson\":5}}}}}}",
        joined.join(",")
    )
}

fn json_cases() -> Vec<Case> {
    let mut v = vec![
        json_ok("empty", EMPTY),
        json_ok("all-types", &all_types_board()),
        ok(
            "json",
            "any-spelling",
            b"{ \"settings\" : {\"title\":\"A\"},\n \"objects\":[{\"type\":\"sticky\",\"x\":1.0,\"id\":\"s-1\"}], \"frames\":[], \"people\":{} }",
            "{\"frames\":[],\"objects\":[{\"id\":\"s-1\",\"type\":\"sticky\",\"x\":1}],\"people\":{},\"settings\":{\"title\":\"A\"}}",
        ),
        ok(
            "json",
            "number-spelling",
            b"{\"frames\":[],\"objects\":[{\"id\":\"s-1\",\"later\":[0.10000000000000001],\"type\":\"sticky\",\"x\":0.30000000000000004,\"y\":1.0000000000000001}],\"people\":{},\"settings\":{}}",
            "{\"frames\":[],\"objects\":[{\"id\":\"s-1\",\"later\":[0.1],\"type\":\"sticky\",\"x\":0.30000000000000004,\"y\":1}],\"people\":{},\"settings\":{}}",
        ),
        json_ok(
            "unknown-kept",
            "{\"frames\":[],\"objects\":[{\"id\":\"s-1\",\"newField\":{\"a\":[1,null,{\"b\":\"c\"}]},\"type\":\"sticky\"}],\"people\":{\"p-1\":{\"future\":true,\"name\":\"Ann\"}},\"settings\":{\"timer\":{\"state\":\"off\"}},\"zUnknown\":[1,2]}",
        ),
        json_ok(
            "unknown-type",
            "{\"frames\":[],\"objects\":[{\"id\":\"x-1\",\"size\":3,\"type\":\"video-clip\",\"x\":10}],\"people\":{},\"settings\":{}}",
        ),
        json_ok(
            "open-names",
            "{\"frames\":[],\"objects\":[{\"dash\":\"wavy\",\"fill\":\"sunset\",\"id\":\"h-1\",\"shape\":\"new-shape-2\",\"type\":\"shape\"}],\"people\":{},\"settings\":{}}",
        ),
        json_ok(
            "orphan-refs",
            "{\"frames\":[],\"objects\":[{\"from\":{\"hint\":{\"x\":1,\"y\":2},\"id\":\"gone\"},\"id\":\"c-1\",\"type\":\"connector\"},{\"id\":\"q-1\",\"target\":\"gone\",\"type\":\"comment\"},{\"id\":\"m-1\",\"parent\":\"gone\",\"type\":\"mind\"},{\"frame\":\"gone\",\"id\":\"s-1\",\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}",
        ),
        json_ok(
            "unnamed-person",
            "{\"frames\":[],\"objects\":[{\"id\":\"s-1\",\"reactions\":{\"x\":[\"p-1\"]},\"type\":\"sticky\"}],\"people\":{\"p-1\":{}},\"settings\":{}}",
        ),
        json_ok(
            "sub-ids-per-parent",
            "{\"frames\":[],\"objects\":[{\"checks\":[{\"done\":true,\"id\":\"a\",\"text\":\"x\"}],\"id\":\"a\",\"type\":\"card\"},{\"checks\":[{\"done\":false,\"id\":\"a\",\"text\":\"y\"}],\"id\":\"b\",\"type\":\"card\"}],\"people\":{},\"settings\":{}}",
        ),
        json_ok(
            "z-order-kept",
            "{\"frames\":[\"f-2\",\"f-1\"],\"objects\":[{\"id\":\"z-9\",\"type\":\"sticky\"},{\"id\":\"f-1\",\"type\":\"frame\"},{\"id\":\"a-1\",\"type\":\"sticky\"},{\"id\":\"f-2\",\"type\":\"frame\"}],\"people\":{},\"settings\":{}}",
        ),
        json_ok(
            "text-at-limit",
            &board(&[&format!(
                "{{\"id\":\"s-1\",\"text\":\"{}\",\"type\":\"sticky\"}}",
                "\u{1F600}".repeat(5000)
            )]),
        ),
        json_ok("id-at-limit", &board(&[&format!("{{\"id\":\"{}\",\"type\":\"sticky\"}}", "a.b_c-".repeat(4))])),
        no("json", "top-array", b"[]", "board_shape"),
        no("json", "no-settings", b"{\"frames\":[],\"objects\":[],\"people\":{}}", "board_shape"),
        no("json", "no-people", b"{\"frames\":[],\"objects\":[],\"settings\":{}}", "board_shape"),
        no("json", "no-objects", b"{\"frames\":[],\"people\":{},\"settings\":{}}", "board_shape"),
        no("json", "no-frames", b"{\"objects\":[],\"people\":{},\"settings\":{}}", "board_shape"),
        no("json", "objects-kind", b"{\"frames\":[],\"objects\":{},\"people\":{},\"settings\":{}}", "board_shape"),
        no("json", "object-kind", b"{\"frames\":[],\"objects\":[1],\"people\":{},\"settings\":{}}", "board_shape"),
        no("json", "frame-entry-kind", b"{\"frames\":[1],\"objects\":[],\"people\":{},\"settings\":{}}", "board_shape"),
        no("json", "person-kind", b"{\"frames\":[],\"objects\":[],\"people\":{\"p-1\":1},\"settings\":{}}", "board_shape"),
        no("json", "person-token", b"{\"frames\":[],\"objects\":[],\"people\":{\"P1\":{}},\"settings\":{}}", "id_invalid"),
        no("json", "person-name-kind", b"{\"frames\":[],\"objects\":[],\"people\":{\"p-1\":{\"name\":1}},\"settings\":{}}", "field_kind"),
        no(
            "json",
            "person-name-long",
            format!("{{\"frames\":[],\"objects\":[],\"people\":{{\"p-1\":{{\"name\":\"{}\"}}}},\"settings\":{{}}}}", "a".repeat(81)).as_bytes(),
            "limit_field",
        ),
        no("json", "id-upper", board(&["{\"id\":\"S-1\",\"type\":\"sticky\"}"]).as_bytes(), "id_invalid"),
        no("json", "id-long", board(&[&format!("{{\"id\":\"{}\",\"type\":\"sticky\"}}", "a".repeat(25))]).as_bytes(), "id_invalid"),
        no("json", "id-empty", board(&["{\"id\":\"\",\"type\":\"sticky\"}"]).as_bytes(), "id_invalid"),
        no("json", "id-absent", board(&["{\"type\":\"sticky\"}"]).as_bytes(), "id_invalid"),
        no("json", "id-kind", board(&["{\"id\":1,\"type\":\"sticky\"}"]).as_bytes(), "id_invalid"),
        no(
            "json",
            "id-twice",
            board(&["{\"id\":\"a\",\"type\":\"sticky\"}", "{\"id\":\"a\",\"type\":\"text\"}"]).as_bytes(),
            "id_duplicate",
        ),
        no("json", "type-absent", board(&["{\"id\":\"a\"}"]).as_bytes(), "type_invalid"),
        no("json", "type-bad", board(&["{\"id\":\"a\",\"type\":\"Big Type\"}"]).as_bytes(), "type_invalid"),
        field_case("name-long", &format!("\"{}\":1,", "a".repeat(201)), "field_name_invalid"),
        field_case("name-empty", "\"\":1,", "field_name_invalid"),
        field_case("name-ctrl", "\"a\\u001fb\":1,", "field_name_invalid"),
        field_case("name-nul", "\"a\\u0000\":1,", "field_name_invalid"),
        no(
            "json",
            "settings-name",
            b"{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"a\\u001f\":1}}",
            "field_name_invalid",
        ),
        field_case("num-kind", "\"x\":\"1\",", "field_kind"),
        field_case("bool-kind", "\"locked\":1,", "field_kind"),
        field_case("text-kind", "\"text\":1,", "field_kind"),
        field_case("null-known", "\"text\":null,", "field_kind"),
        field_case("name-kind", "\"align\":1,", "field_kind"),
        field_case("name-bad", "\"align\":\"Left Side\",", "field_value"),
        field_case("colour-bad", "\"color\":\"#12\",", "field_value"),
        field_case("colour-hex-bad", "\"color\":\"#gggggg\",", "field_value"),
        field_case("ref-bad", "\"frame\":\"NOT AN ID\",", "id_invalid"),
        field_case("group-bad", "\"group\":\"\",", "id_invalid"),
        field_case("text-long", &format!("\"text\":\"{}\",", "a".repeat(5001)), "limit_field"),
        field_case("text-astral", &format!("\"text\":\"{}\",", "\u{1F600}".repeat(5001)), "limit_field"),
        field_case("tags-kind", "\"tags\":\"a\",", "field_kind"),
        field_case(
            "tags-many",
            &format!("\"tags\":[{}],", vec!["\"a\""; 201].join(",")),
            "limit_field",
        ),
        field_case("tag-long", &format!("\"tags\":[\"{}\"],", "a".repeat(81)), "limit_field"),
        field_case("tag-kind", "\"tags\":[1],", "field_kind"),
        type_case("point-short", "pen", "\"points\":[[1]],", "field_value"),
        type_case("point-long", "pen", "\"points\":[[1,2,3]],", "field_value"),
        type_case("point-kind", "pen", "\"points\":[1],", "field_kind"),
        type_case("point-str", "pen", "\"points\":[[\"1\",2]],", "field_kind"),
        type_case(
            "points-many",
            "pen",
            &format!("\"points\":[{}],", vec!["[0,0]"; 5001].join(",")),
            "limit_field",
        ),
        type_case("end-both", "connector", "\"from\":{\"id\":\"a\",\"x\":1,\"y\":2},", "field_value"),
        type_case("end-empty", "connector", "\"from\":{},", "field_value"),
        type_case("end-kind", "connector", "\"from\":\"a\",", "field_kind"),
        type_case("end-half", "connector", "\"to\":{\"x\":1},", "field_kind"),
        type_case("end-id-bad", "connector", "\"to\":{\"id\":\"BAD\"},", "id_invalid"),
        type_case("end-anchor", "connector", "\"to\":{\"anchor\":\"Up High\",\"id\":\"a\"},", "field_value"),
        type_case("end-hint", "connector", "\"to\":{\"hint\":{\"x\":1},\"id\":\"a\"},", "field_kind"),
        type_case("check-twice", "card", "\"checks\":[{\"done\":true,\"id\":\"a\",\"text\":\"\"},{\"done\":true,\"id\":\"a\",\"text\":\"\"}],", "id_duplicate"),
        type_case("check-no-id", "card", "\"checks\":[{\"done\":true,\"text\":\"\"}],", "field_kind"),
        type_case("check-no-done", "card", "\"checks\":[{\"id\":\"a\",\"text\":\"\"}],", "field_kind"),
        type_case("check-no-text", "card", "\"checks\":[{\"done\":true,\"id\":\"a\"}],", "field_kind"),
        type_case("check-done-kind", "card", "\"checks\":[{\"done\":\"y\",\"id\":\"a\",\"text\":\"\"}],", "field_kind"),
        type_case("check-long", "card", &format!("\"checks\":[{{\"done\":true,\"id\":\"a\",\"text\":\"{}\"}}],", "a".repeat(501)), "limit_field"),
        type_case(
            "checks-many",
            "card",
            &format!(
                "\"checks\":[{}],",
                (0..51).map(|i| format!("{{\"done\":true,\"id\":\"c{i}\",\"text\":\"\"}}")).collect::<Vec<_>>().join(",")
            ),
            "limit_field",
        ),
        type_case("col-twice", "table", "\"columns\":[{\"id\":\"a\"},{\"id\":\"a\"}],", "id_duplicate"),
        type_case("col-width", "table", "\"columns\":[{\"id\":\"a\",\"width\":-1}],", "field_value"),
        type_case(
            "cols-many",
            "table",
            &format!("\"columns\":[{}],", (0..31).map(|i| format!("{{\"id\":\"c{i}\"}}")).collect::<Vec<_>>().join(",")),
            "limit_field",
        ),
        type_case("row-twice", "table", "\"rows\":[{\"id\":\"a\"},{\"id\":\"a\"}],", "id_duplicate"),
        type_case(
            "rows-many",
            "table",
            &format!("\"rows\":[{}],", (0..101).map(|i| format!("{{\"id\":\"r{i}\"}}")).collect::<Vec<_>>().join(",")),
            "limit_field",
        ),
        type_case("cell-no-col", "table", "\"columns\":[{\"id\":\"a\"}],\"rows\":[{\"cells\":{\"b\":\"x\"},\"id\":\"r\"}],", "cell_column_unknown"),
        type_case("cell-long", "table", &format!("\"columns\":[{{\"id\":\"a\"}}],\"rows\":[{{\"cells\":{{\"a\":\"{}\"}},\"id\":\"r\"}}],", "a".repeat(2001)), "limit_field"),
        type_case("cell-kind", "table", "\"columns\":[{\"id\":\"a\"}],\"rows\":[{\"cells\":{\"a\":1},\"id\":\"r\"}],", "field_kind"),
        field_case("react-order", &format!("\"reactions\":{{\"x\":[\"{P2}\",\"{P1}\"]}},"), "field_value"),
        field_case("react-twice", &format!("\"reactions\":{{\"x\":[\"{P1}\",\"{P1}\"]}},"), "field_value"),
        field_case("react-who", "\"reactions\":{\"x\":[\"p-9\"]},", "person_unknown"),
        field_case("react-kind", "\"reactions\":{\"x\":\"p-1\"},", "field_kind"),
        field_case("react-key", &format!("\"reactions\":{{\"{}\":[]}},", "a".repeat(33)), "field_value"),
        field_case(
            "react-keys-many",
            &format!("\"reactions\":{{{}}},", (0..65).map(|i| format!("\"k{i}\":[]")).collect::<Vec<_>>().join(",")),
            "limit_field",
        ),
        field_case("votes-zero", &format!("\"votes\":{{\"{P1}\":0}},"), "field_value"),
        field_case("votes-frac", &format!("\"votes\":{{\"{P1}\":1.5}},"), "field_value"),
        field_case("votes-who", "\"votes\":{\"p-9\":1},", "person_unknown"),
        field_case("votes-kind", "\"votes\":[],", "field_kind"),
        type_case("reply-twice", "comment", &format!("\"replies\":[{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{P1}\",\"id\":\"a\",\"text\":\"\"}},{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{P1}\",\"id\":\"a\",\"text\":\"\"}}],"), "id_duplicate"),
        type_case("reply-no-by", "comment", "\"replies\":[{\"at\":\"2026-11-03T10:00:00Z\",\"id\":\"a\",\"text\":\"\"}],", "field_kind"),
        type_case("reply-who", "comment", "\"replies\":[{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"p-9\",\"id\":\"a\",\"text\":\"\"}],", "person_unknown"),
        type_case("reply-at", "comment", &format!("\"replies\":[{{\"at\":\"yesterday\",\"by\":\"{P1}\",\"id\":\"a\",\"text\":\"\"}}],"), "field_value"),
        type_case("reply-long", "comment", &format!("\"replies\":[{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{P1}\",\"id\":\"a\",\"text\":\"{}\"}}],", "a".repeat(5001)), "limit_field"),
        type_case("crop-out", "image", "\"crop\":{\"h\":0.5,\"w\":0.5,\"x\":0.75,\"y\":0},", "field_value"),
        type_case("crop-small", "image", "\"crop\":{\"h\":0.5,\"w\":0.001,\"x\":0,\"y\":0},", "field_value"),
        type_case("crop-neg", "image", "\"crop\":{\"h\":0.5,\"w\":0.5,\"x\":-0.1,\"y\":0},", "field_value"),
        type_case("crop-half", "image", "\"crop\":{\"x\":0,\"y\":0},", "field_kind"),
        field_case("link-ftp", "\"link\":\"ftp://example.com\",", "field_value"),
        field_case("link-space", "\"link\":\"https://a b\",", "field_value"),
        field_case("link-bare", "\"link\":\"https://\",", "field_value"),
        field_case("link-long", &format!("\"link\":\"https://{}\",", "a".repeat(2000)), "limit_field"),
        type_case("blob-str", "image", "\"src\":\"data:image/png;base64,AAAA\",", "field_kind"),
        type_case("blob-hash", "image", "\"src\":{\"blob\":\"sha256-XYZ\"},", "field_value"),
        type_case("blob-more", "image", &format!("\"src\":{{\"blob\":\"{HASH}\",\"x\":1}},"), "field_value"),
        field_case("created-who", "\"created\":{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"p-9\"},", "person_unknown"),
        field_case("created-at", &format!("\"created\":{{\"at\":\"2026-13-03T10:00:00Z\",\"by\":\"{P1}\"}},"), "field_value"),
        field_case("created-zone", &format!("\"created\":{{\"at\":\"2026-11-03T10:00:00\",\"by\":\"{P1}\"}},"), "field_value"),
        no(
            "json",
            "voting-kind",
            b"{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"voting\":true}}",
            "field_kind",
        ),
        no(
            "json",
            "voting-per",
            b"{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"voting\":{\"active\":true,\"perPerson\":0}}}",
            "field_value",
        ),
        no(
            "json",
            "title-long",
            format!("{{\"frames\":[],\"objects\":[],\"people\":{{}},\"settings\":{{\"title\":\"{}\"}}}}", "a".repeat(201)).as_bytes(),
            "limit_field",
        ),
        no(
            "json",
            "frames-missing",
            b"{\"frames\":[],\"objects\":[{\"id\":\"f-1\",\"type\":\"frame\"}],\"people\":{},\"settings\":{}}",
            "frames_mismatch",
        ),
        no(
            "json",
            "frames-gone",
            b"{\"frames\":[\"f-1\"],\"objects\":[],\"people\":{},\"settings\":{}}",
            "frames_mismatch",
        ),
        no(
            "json",
            "frames-twice",
            b"{\"frames\":[\"f-1\",\"f-1\"],\"objects\":[{\"id\":\"f-1\",\"type\":\"frame\"}],\"people\":{},\"settings\":{}}",
            "frames_mismatch",
        ),
        no(
            "json",
            "frames-not-frame",
            b"{\"frames\":[\"s-1\"],\"objects\":[{\"id\":\"s-1\",\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}",
            "frames_mismatch",
        ),
        limited(
            no("json", "objects-limit", board(&["{\"id\":\"a\",\"type\":\"sticky\"}", "{\"id\":\"b\",\"type\":\"sticky\"}"]).as_bytes(), "limit_objects"),
            &[("objects", 1)],
        ),
        limited(no("json", "people-limit", board(&[]).as_bytes(), "limit_people"), &[("people", 1)]),
    ];
    v.push(field_case(
        "created-no-t",
        &format!("\"created\":{{\"at\":\"2026-11-03 10:00:00Z\",\"by\":\"{P1}\"}},"),
        "field_value",
    ));
    v.push(field_case(
        "created-offset",
        &format!("\"created\":{{\"at\":\"2026-11-03T10:00:00+0200\",\"by\":\"{P1}\"}},"),
        "field_value",
    ));
    v.push(field_case(
        "name-41-bytes",
        &format!("\"align\":\"{}\",", "a".repeat(41)),
        "field_value",
    ));
    v.push(no(
        "json",
        "type-41-bytes",
        board(&[&format!("{{\"id\":\"a\",\"type\":\"{}\"}}", "a".repeat(41))]).as_bytes(),
        "type_invalid",
    ));
    v.push(json_ok(
        "person-kinds",
        "{\"frames\":[],\"objects\":[],\"people\":{\"p-1\":{\"kind\":\"agent\",\"name\":\"Bot\"},\"p-2\":{\"kind\":\"person\"},\"p-3\":{\"kind\":\"team-of-robots\"}},\"settings\":{}}",
    ));
    v.push(no(
        "json",
        "person-kind-type",
        b"{\"frames\":[],\"objects\":[],\"people\":{\"p-1\":{\"kind\":1}},\"settings\":{}}",
        "field_kind",
    ));
    v.push(no(
        "json",
        "person-kind-bad",
        b"{\"frames\":[],\"objects\":[],\"people\":{\"p-1\":{\"kind\":\"Big Kind\"}},\"settings\":{}}",
        "field_value",
    ));
    v.push(field_case(
        "created-long",
        &format!(
            "\"created\":{{\"at\":\"2026-11-03T10:00:00.{}Z\",\"by\":\"{P1}\"}},",
            "0".repeat(30)
        ),
        "field_value",
    ));
    v.push(field_case(
        "created-frac",
        &format!("\"created\":{{\"at\":\"2026-11-03T10:00:00.Z\",\"by\":\"{P1}\"}},"),
        "field_value",
    ));
    v.push(type_case(
        "crop-out-y",
        "image",
        "\"crop\":{\"h\":0.5,\"w\":0.5,\"x\":0,\"y\":0.75},",
        "field_value",
    ));
    v.push(type_case(
        "crop-small-h",
        "image",
        "\"crop\":{\"h\":0.001,\"w\":0.5,\"x\":0,\"y\":0},",
        "field_value",
    ));
    v.push(no(
        "json",
        "voting-active",
        b"{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"voting\":{\"active\":1}}}",
        "field_kind",
    ));
    v.push(no(
        "json",
        "num-inexact",
        board(&["{\"id\":\"s\",\"type\":\"sticky\",\"x\":0.1000000000000000055}"]).as_bytes(),
        "json_number_inexact",
    ));
    v.push(no(
        "json",
        "dup-field",
        board(&["{\"id\":\"s\",\"type\":\"sticky\",\"x\":1,\"x\":2}"]).as_bytes(),
        "json_duplicate_key",
    ));
    v
}

const SMALL: &str = "{\"frames\":[],\"objects\":[{\"id\":\"s-1\",\"text\":\"hello\",\"type\":\"sticky\"}],\"people\":{},\"settings\":{\"title\":\"A\"}}";

fn image_board(blob_hash: &str) -> String {
    format!("{{\"frames\":[],\"objects\":[{{\"id\":\"i-1\",\"src\":{{\"blob\":\"sha256-{blob_hash}\"}},\"type\":\"image\"}}],\"people\":{{}},\"settings\":{{}}}}")
}

fn base_entries() -> Vec<E> {
    package(SMALL, Vec::new()).0
}

fn forged(mutate: impl FnOnce(&mut Forge)) -> Vec<u8> {
    let mut f = Forge::new(base_entries());
    mutate(&mut f);
    f.build()
}

fn zip_no(name: &str, input: Vec<u8>, code: &'static str) -> Case {
    no("zip", name, &input, code)
}

/// A package with one blob named by its real hash, whose entries the closure may change.
fn with_blob(mutate: impl FnOnce(&mut Vec<E>)) -> Vec<u8> {
    let data = b"\x89PNG not really an image".to_vec();
    let hash = sha_hex(&data);
    let name: &'static str = Box::leak(format!("blobs/sha256-{hash}").into_boxed_str());
    let (mut entries, _) = package(
        &image_board(&hash),
        vec![P {
            name,
            media: "image/png",
            role: "blob",
            data,
        }],
    );
    mutate(&mut entries);
    Forge::new(entries).build()
}

/// A package whose manifest is this text, and its other entries are as usual.
fn with_manifest(text: &str) -> Vec<u8> {
    let mut entries = base_entries();
    entries[0] = E::new("manifest.json", text.as_bytes(), STORE);
    Forge::new(entries).build()
}

fn small_manifest() -> String {
    package(SMALL, Vec::new()).1
}

fn run_and_noise() -> (Vec<u8>, Vec<u8>) {
    // A long run, then bytes that don't compress: the whole stream's ratio is fine, but its start
    // is far above the limit.
    let mut data = vec![b' '; 4 * 1024 * 1024];
    let mut x = 0x9E37_79B9u32;
    for _ in 0..(64 * 1024) {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        data.push((x & 0xFF) as u8);
    }
    let packed = miniz_oxide::deflate::compress_to_vec(&data, 6);
    (data, packed)
}

fn zip_cases() -> Vec<Case> {
    let mut v = Vec::new();

    // Accepted files.
    let (entries, manifest) = package(SMALL, Vec::new());
    let mut c = ok("zip", "minimal", &Forge::new(entries).build(), SMALL);
    c.guard = Some(sha_hex(manifest.as_bytes()));
    c.stable_guard = true;
    v.push(c);

    let data = b"some image bytes".to_vec();
    let hash = sha_hex(&data);
    let board = image_board(&hash);
    let name: &'static str = Box::leak(format!("blobs/sha256-{hash}").into_boxed_str());
    let (entries, manifest) = package(
        &board,
        vec![
            P {
                name,
                media: "image/png",
                role: "blob",
                data: data.clone(),
            },
            P {
                name: "preview.png",
                media: "image/png",
                role: "preview",
                data: b"a preview".to_vec(),
            },
            P {
                name: "extra/notes.bin",
                media: "application/octet-stream",
                role: "future-role",
                data: b"kept".to_vec(),
            },
        ],
    );
    let mut c = ok("zip", "parts-kept", &Forge::new(entries).build(), &board);
    c.guard = Some(sha_hex(manifest.as_bytes()));
    c.stable_guard = true;
    v.push(c);

    // A foreign writer: spelled differently, a deflated manifest, extra fields, flag bits it may
    // set, a newer version that an older reader may still read, and an unknown manifest field.
    let spelled = " { \"settings\":{}, \"people\":{}, \"objects\":[], \"frames\":[] } ";
    let foreign_manifest = format!(
        "{{\"future\":{{\"a\":1}},\"version\":2,\"min_reader\":1,\"format\":\"k5board\",\"lineage\":\"{LINEAGE}\",\"parts\":[{{\"sha256\":\"{}\",\"role\":\"board\",\"name\":\"board.json\",\"media_type\":\"application/json\",\"bytes\":{}}}]}}",
        sha_hex(spelled.as_bytes()),
        spelled.len()
    );
    let mut m = E::new("manifest.json", foreign_manifest.as_bytes(), DEFLATE);
    m.flags = 0x0800;
    m.extra_central = vec![0x55, 0x54, 0x01, 0x00, 0x00];
    m.extra_local = vec![0x55, 0x54, 0x05, 0x00, 0, 0, 0, 0, 0];
    let mut b = E::new("board.json", spelled.as_bytes(), STORE);
    b.made_by = 0x0314;
    b.external = 0o100_644 << 16;
    let expected_guard = format!(
        "{{\"format\":\"k5board\",\"future\":{{\"a\":1}},\"lineage\":\"{LINEAGE}\",\"min_reader\":1,\"parts\":[{{\"bytes\":{},\"media_type\":\"application/json\",\"name\":\"board.json\",\"role\":\"board\",\"sha256\":\"{}\"}}],\"version\":2}}",
        spelled.len(),
        sha_hex(spelled.as_bytes())
    );
    let mut c = ok("zip", "foreign", &Forge::new(vec![m, b]).build(), EMPTY);
    c.guard = Some(sha_hex(expected_guard.as_bytes()));
    v.push(c);

    // A newer writer's file, which this reader may still read: a rewrite keeps its version.
    let newer = small_manifest().replace("\"version\":1", "\"version\":7");
    let mut entries = base_entries();
    entries[0] = E::new("manifest.json", newer.as_bytes(), STORE);
    let mut c = ok("zip", "newer-version", &Forge::new(entries).build(), SMALL);
    c.guard = Some(sha_hex(newer.as_bytes()));
    c.stable_guard = true;
    v.push(c);

    // Parts listed out of order: the guard sorts them.
    let extra = b"kept".to_vec();
    let unsorted = format!(
        "{{\"format\":\"k5board\",\"lineage\":\"{LINEAGE}\",\"min_reader\":1,\"parts\":[{{\"bytes\":4,\"media_type\":\"application/x-y\",\"name\":\"zz.bin\",\"role\":\"future\",\"sha256\":\"{}\"}},{{\"bytes\":{},\"media_type\":\"application/json\",\"name\":\"board.json\",\"role\":\"board\",\"sha256\":\"{}\"}}],\"version\":1}}",
        sha_hex(&extra),
        SMALL.len(),
        sha_hex(SMALL.as_bytes())
    );
    let sorted = manifest_text(
        None,
        &[
            (
                "board.json".to_string(),
                "application/json".to_string(),
                "board".to_string(),
                SMALL.len(),
                sha_hex(SMALL.as_bytes()),
            ),
            (
                "zz.bin".to_string(),
                "application/x-y".to_string(),
                "future".to_string(),
                4,
                sha_hex(&extra),
            ),
        ],
    );
    let mut c = ok(
        "zip",
        "parts-unsorted",
        &Forge::new(vec![
            E::new("manifest.json", unsorted.as_bytes(), STORE),
            E::new("board.json", SMALL.as_bytes(), DEFLATE),
            E::new("zz.bin", &extra, STORE),
        ])
        .build(),
        SMALL,
    );
    c.guard = Some(sha_hex(sorted.as_bytes()));
    c.stable_guard = true;
    v.push(c);

    // Bytes outside any entry.
    v.push(zip_no(
        "stray-prefix",
        forged(|f| f.prefix = b"XXXX".to_vec()),
        "zip_stray_bytes",
    ));
    v.push(zip_no(
        "stray-trailing",
        forged(|f| f.trailing = b"XXXX".to_vec()),
        "zip_stray_bytes",
    ));
    v.push(zip_no(
        "stray-comment",
        forged(|f| f.end_comment = b"note".to_vec()),
        "zip_stray_bytes",
    ));
    v.push(zip_no(
        "stray-gap",
        forged(|f| f.entries[0].gap_after = b"XXXX".to_vec()),
        "zip_stray_bytes",
    ));
    v.push(zip_no(
        "stray-last",
        forged(|f| f.entries[1].gap_after = b"XXXX".to_vec()),
        "zip_stray_bytes",
    ));
    v.push(zip_no(
        "stray-mid-end",
        forged(|f| f.before_end = b"XXXX".to_vec()),
        "zip_stray_bytes",
    ));
    v.push(zip_no(
        "no-end",
        b"not a zip at all, and long enough to hold one".to_vec(),
        "zip_no_end_record",
    ));
    v.push(zip_no("too-short", b"PK".to_vec(), "zip_no_end_record"));
    let whole = forged(|_| {});
    v.push(zip_no(
        "truncated",
        whole[..whole.len() - 30].to_vec(),
        "zip_no_end_record",
    ));

    // Entries sharing bytes: the second entry's header sits inside the first's stored data.
    let inner = E::new("preview.png", b"inner", STORE);
    let inner_local = inner.local_bytes();
    let mut outer = E::new("board.json", &inner_local, STORE);
    let manifest_entry = E::new("manifest.json", small_manifest().as_bytes(), STORE);
    let inner_offset = manifest_entry.local_bytes().len() + 30 + "board.json".len();
    let mut pointed = inner.clone();
    pointed.no_local = true;
    pointed.offset = Some(inner_offset as u32);
    outer.gap_after = Vec::new();
    v.push(zip_no(
        "overlap",
        Forge::new(vec![manifest_entry, outer, pointed]).build(),
        "zip_overlap",
    ));

    // An entry that runs into the central directory.
    let mut e = base_entries();
    e[1].csize = Some(100_000);
    e[1].local_csize = Some(100_000);
    v.push(zip_no("into-central", Forge::new(e).build(), "zip_overlap"));

    // The local header disagrees with the central directory.
    v.push(zip_no(
        "local-name",
        forged(|f| f.entries[1].local_name = Some(b"board.jsox".to_vec())),
        "zip_header_mismatch",
    ));
    v.push(zip_no(
        "local-method",
        forged(|f| f.entries[1].local_method = Some(STORE)),
        "zip_header_mismatch",
    ));
    v.push(zip_no(
        "local-crc",
        forged(|f| f.entries[1].local_crc = Some(1)),
        "zip_header_mismatch",
    ));
    v.push(zip_no(
        "local-csize",
        forged(|f| f.entries[1].local_csize = Some(1)),
        "zip_header_mismatch",
    ));
    v.push(zip_no(
        "local-usize",
        forged(|f| f.entries[1].local_usize = Some(1)),
        "zip_header_mismatch",
    ));
    v.push(zip_no(
        "local-flags",
        forged(|f| f.entries[1].local_flags = Some(0x0800)),
        "zip_header_mismatch",
    ));
    v.push(zip_no(
        "local-bad-sig",
        forged(|f| f.entries[1].offset = Some(1)),
        "zip_malformed",
    ));

    // Duplicate names.
    v.push(zip_no(
        "dup-exact",
        forged(|f| {
            let e = f.entries[1].clone();
            f.entries.push(e)
        }),
        "zip_name_duplicate",
    ));
    v.push(zip_no(
        "dup-fold",
        forged(|f| {
            let mut e = f.entries[1].clone();
            e.name = b"BOARD.json".to_vec();
            f.entries.push(e)
        }),
        "zip_name_duplicate",
    ));

    // Names.
    for (case, name) in [
        ("name-dotdot", &b"../board.json"[..]),
        ("name-dotdot-in", b"a/../b"),
        ("name-abs", b"/board.json"),
        ("name-bslash", b"blobs\\x"),
        ("name-drive", b"c:board"),
        ("name-dir", b"blobs/"),
        ("name-empty", b""),
        ("name-device", b"con.txt"),
        ("name-device-up", b"AUX"),
        ("name-com", b"x/com1.png"),
        ("name-end-dot", b"board."),
        ("name-space", b"a b"),
        ("name-utf8", "caf\u{e9}".as_bytes()),
        ("name-deep", b"a/b/c/d/e"),
    ] {
        v.push(zip_no(
            case,
            forged(|f| {
                let mut e = E::new("x", b"y", STORE);
                e.name = name.to_vec();
                f.entries.push(e)
            }),
            "zip_name_invalid",
        ));
    }
    v.push(zip_no(
        "name-long",
        forged(|f| {
            let n = format!("{}/{}/{}", "a".repeat(64), "b".repeat(64), "c");
            f.entries.push(E::new(&n, b"y", STORE))
        }),
        "zip_name_invalid",
    ));

    // Entries that aren't regular files.
    v.push(zip_no(
        "symlink",
        forged(|f| {
            f.entries[1].made_by = 0x0314;
            f.entries[1].external = 0o120_777 << 16
        }),
        "zip_not_regular",
    ));
    v.push(zip_no(
        "fifo",
        forged(|f| {
            f.entries[1].made_by = 0x0314;
            f.entries[1].external = 0o010_644 << 16
        }),
        "zip_not_regular",
    ));
    v.push(zip_no(
        "dir-attr",
        forged(|f| f.entries[1].external = 0x10),
        "zip_not_regular",
    ));
    v.push(zip_no(
        "reparse",
        forged(|f| f.entries[1].external = 0x400),
        "zip_not_regular",
    ));

    // Encryption, disks, ZIP64, flags, methods.
    v.push(zip_no(
        "encrypted",
        forged(|f| f.entries[1].flags = 1),
        "zip_encrypted",
    ));
    v.push(zip_no(
        "strong-enc",
        forged(|f| f.entries[1].flags = 1 << 6),
        "zip_encrypted",
    ));
    v.push(zip_no(
        "masked",
        forged(|f| f.entries[1].flags = 1 << 13),
        "zip_encrypted",
    ));
    v.push(zip_no(
        "descriptor",
        forged(|f| f.entries[1].flags = 1 << 3),
        "zip_flags",
    ));
    v.push(zip_no(
        "flag-unknown",
        forged(|f| f.entries[1].flags = 1 << 5),
        "zip_flags",
    ));
    v.push(zip_no("disk-end", forged(|f| f.disk = 1), "zip_multi_disk"));
    v.push(zip_no(
        "disk-entry",
        forged(|f| f.entries[1].disk_start = 1),
        "zip_multi_disk",
    ));
    v.push(zip_no(
        "zip64-count",
        forged(|f| f.count = Some(0xFFFF)),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-size",
        forged(|f| f.central_size = Some(u32::MAX)),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-at",
        forged(|f| f.central_at = Some(u32::MAX)),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-csize",
        forged(|f| f.entries[1].csize = Some(u32::MAX)),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-usize",
        forged(|f| f.entries[1].usize_ = Some(u32::MAX)),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-offset",
        forged(|f| f.entries[1].offset = Some(u32::MAX)),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-extra",
        forged(|f| f.entries[1].extra_central = vec![1, 0, 0, 0]),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-local",
        forged(|f| f.entries[1].extra_local = vec![1, 0, 0, 0]),
        "zip64",
    ));
    v.push(zip_no(
        "zip64-locator",
        forged(|f| {
            let mut l = 0x0706_4b50u32.to_le_bytes().to_vec();
            l.resize(20, 0);
            f.before_end = l
        }),
        "zip64",
    ));
    v.push(zip_no(
        "extra-broken",
        forged(|f| f.entries[1].extra_central = vec![9, 0, 9, 0, 1]),
        "zip_malformed",
    ));
    v.push(zip_no(
        "extra-short",
        forged(|f| f.entries[1].extra_local = vec![9]),
        "zip_malformed",
    ));
    v.push(zip_no(
        "method-bzip2",
        forged(|f| f.entries[1].method = 12),
        "zip_method",
    ));
    v.push(zip_no(
        "entry-comment",
        forged(|f| f.entries[1].comment = b"c".to_vec()),
        "zip_malformed",
    ));
    v.push(zip_no(
        "count-low",
        forged(|f| f.count = Some(1)),
        "zip_malformed",
    ));
    v.push(zip_no(
        "count-high",
        forged(|f| f.count = Some(3)),
        "zip_malformed",
    ));
    let mut broken = forged(|_| {});
    let central = broken
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .unwrap_or(0);
    broken[central + 3] = 9;
    v.push(zip_no("central-sig", broken, "zip_malformed"));

    v.push(zip_no(
        "central-long",
        forged(|f| f.central_size = Some(100_000)),
        "zip_malformed",
    ));
    v.push(zip_no(
        "name-start-dot",
        forged(|f| f.entries.push(E::new(".hidden", b"y", STORE))),
        "zip_name_invalid",
    ));
    v.push(zip_no(
        "name-lpt",
        forged(|f| f.entries.push(E::new("lpt1.txt", b"y", STORE))),
        "zip_name_invalid",
    ));

    v.push(zip_no(
        "name-segment",
        forged(|f| f.entries.push(E::new(&"a".repeat(81), b"y", STORE))),
        "zip_name_invalid",
    ));
    let mut no_sig = forged(|_| {});
    let second_local = no_sig
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0x50, 0x4b, 0x03, 0x04])
        .map(|(i, _)| i)
        .nth(1)
        .unwrap_or(0);
    no_sig[second_local + 3] = 9;
    v.push(zip_no("local-sig", no_sig, "zip_malformed"));

    // CRC and sizes that lie.
    v.push(zip_no(
        "crc",
        forged(|f| f.entries[1].crc = Some(0x1234_5678)),
        "zip_crc",
    ));
    v.push(zip_no(
        "crc-store",
        with_blob(|e| e[2].crc = Some(0x1234_5678)),
        "zip_crc",
    ));
    v.push(zip_no(
        "store-sizes",
        forged(|f| {
            f.entries[0].usize_ = Some(1);
            f.entries[0].local_usize = Some(1)
        }),
        "zip_size_mismatch",
    ));
    v.push(zip_no(
        "deflate-garbage",
        forged(|f| f.entries[1].body = Some(vec![0xFF; 40])),
        "zip_deflate_invalid",
    ));
    let cut = miniz_oxide::deflate::compress_to_vec(SMALL.as_bytes(), 6);
    v.push(zip_no(
        "deflate-cut",
        forged(|f| f.entries[1].body = Some(cut[..cut.len() - 4].to_vec())),
        "zip_deflate_invalid",
    ));
    let mut padded = cut.clone();
    padded.extend_from_slice(b"pad");
    v.push(zip_no(
        "deflate-pad",
        forged(|f| f.entries[1].body = Some(padded)),
        "zip_size_mismatch",
    ));

    // A file that declares a small size and inflates to more stops at the declared size.
    let lie_text = " ".repeat(2 * 1024 * 1024);
    let lie_declared = 1000usize;
    let lie_body = miniz_oxide::deflate::compress_to_vec(lie_text.as_bytes(), 6);
    let lie_rows = vec![(
        "board.json".to_string(),
        "application/json".to_string(),
        "board".to_string(),
        lie_declared,
        sha_hex(lie_text.as_bytes()),
    )];
    let lie_manifest = manifest_text(None, &lie_rows);
    let mut b = E::new("board.json", lie_text.as_bytes(), DEFLATE);
    b.usize_ = Some(lie_declared as u32);
    b.body = Some(lie_body);
    let lie_file = Forge::new(vec![
        E::new("manifest.json", lie_manifest.as_bytes(), STORE),
        b,
    ])
    .build();
    v.push(zip_no("bomb-lie", lie_file, "zip_size_mismatch"));

    // A file that declares more than it holds.
    let short_rows = vec![(
        "board.json".to_string(),
        "application/json".to_string(),
        "board".to_string(),
        SMALL.len() + 50,
        sha_hex(SMALL.as_bytes()),
    )];
    let short_manifest = manifest_text(None, &short_rows);
    let mut b = E::new("board.json", SMALL.as_bytes(), DEFLATE);
    b.usize_ = Some((SMALL.len() + 50) as u32);
    v.push(zip_no(
        "bomb-short",
        Forge::new(vec![
            E::new("manifest.json", short_manifest.as_bytes(), STORE),
            b,
        ])
        .build(),
        "zip_size_mismatch",
    ));

    // A file that declares a huge ratio.
    let bomb_rows = vec![(
        "board.json".to_string(),
        "application/json".to_string(),
        "board".to_string(),
        100 * 1024 * 1024,
        sha_hex(b""),
    )];
    let bomb_manifest = manifest_text(None, &bomb_rows);
    let mut b = E::new("board.json", b"", DEFLATE);
    b.usize_ = Some(100 * 1024 * 1024);
    b.body = Some(vec![0x03, 0x00]);
    v.push(zip_no(
        "bomb-declared",
        Forge::new(vec![
            E::new("manifest.json", bomb_manifest.as_bytes(), STORE),
            b,
        ])
        .build(),
        "limit_ratio",
    ));

    // A file whose whole ratio is fine, but whose start inflates far past the limit.
    let (data, packed) = run_and_noise();
    let stream_rows = vec![(
        "board.json".to_string(),
        "application/json".to_string(),
        "board".to_string(),
        data.len(),
        sha_hex(&data),
    )];
    let stream_manifest = manifest_text(None, &stream_rows);
    let mut b = E::new("board.json", &data, DEFLATE);
    b.body = Some(packed);
    v.push(zip_no(
        "bomb-stream",
        Forge::new(vec![
            E::new("manifest.json", stream_manifest.as_bytes(), STORE),
            b,
        ])
        .build(),
        "limit_ratio",
    ));

    // Limits, tightened so the files stay small.
    v.push(limited(
        zip_no("limit-entries", forged(|_| {}), "limit_entries"),
        &[("entries", 1)],
    ));
    v.push(limited(
        zip_no("limit-board", forged(|_| {}), "limit_part_bytes"),
        &[("board_json_bytes", 10)],
    ));
    v.push(limited(
        zip_no("limit-manifest", forged(|_| {}), "limit_part_bytes"),
        &[("manifest_bytes", 10)],
    ));
    v.push(limited(
        zip_no("limit-part", with_blob(|_| {}), "limit_part_bytes"),
        &[("part_bytes", 5)],
    ));
    v.push(limited(
        zip_no("limit-total", forged(|_| {}), "limit_total_bytes"),
        &[("total_bytes", 10)],
    ));
    v.push(limited(
        zip_no("limit-ratio", forged(|_| {}), "limit_ratio"),
        &[("ratio", 1), ("ratio_floor", 1)],
    ));

    // The manifest.
    let mut swapped = base_entries();
    swapped.swap(0, 1);
    v.push(zip_no(
        "manifest-second",
        Forge::new(swapped).build(),
        "manifest_not_first",
    ));
    // One order in the central directory, another in the file.
    let reordered = |central_board_first: bool, file_board_first: bool| {
        let m = E::new("manifest.json", small_manifest().as_bytes(), STORE);
        let b = E::new("board.json", SMALL.as_bytes(), DEFLATE);
        let (m_len, b_len) = (m.local_bytes().len() as u32, b.local_bytes().len() as u32);
        let (mut m_at, mut b_at) = (0, m_len);
        let mut file = m.local_bytes();
        file.extend_from_slice(&b.local_bytes());
        if file_board_first {
            (m_at, b_at) = (b_len, 0);
            file = b.local_bytes();
            file.extend_from_slice(&m.local_bytes());
        }
        let place = |mut e: E, at: u32| {
            e.no_local = true;
            e.offset = Some(at);
            e
        };
        let (m, b) = (place(m, m_at), place(b, b_at));
        let mut f = Forge::new(if central_board_first {
            vec![b, m]
        } else {
            vec![m, b]
        });
        f.prefix = file;
        f.build()
    };
    v.push(zip_no(
        "file-second",
        reordered(false, true),
        "manifest_not_first",
    ));
    v.push(zip_no(
        "central-second",
        reordered(true, false),
        "manifest_not_first",
    ));
    v.push(zip_no(
        "no-manifest",
        Forge::new(vec![E::new("board.json", SMALL.as_bytes(), DEFLATE)]).build(),
        "manifest_not_first",
    ));
    v.push(zip_no(
        "no-entries",
        Forge::new(Vec::new()).build(),
        "manifest_not_first",
    ));
    v.push(zip_no(
        "manifest-text",
        with_manifest("not json"),
        "json_syntax",
    ));
    v.push(zip_no(
        "manifest-array",
        with_manifest("[]"),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "manifest-dup",
        with_manifest("{\"format\":\"k5board\",\"format\":\"k5board\"}"),
        "json_duplicate_key",
    ));
    let m = small_manifest();
    v.push(zip_no(
        "format-other",
        with_manifest(&m.replace("k5board", "k9board")),
        "manifest_format",
    ));
    v.push(zip_no(
        "format-absent",
        with_manifest("{\"version\":1}"),
        "manifest_format",
    ));
    v.push(zip_no(
        "min-reader-2",
        with_manifest(
            &m.replace("\"min_reader\":1", "\"min_reader\":2")
                .replace("\"version\":1", "\"version\":2"),
        ),
        "min_reader_too_high",
    ));
    v.push(zip_no(
        "min-reader-only",
        with_manifest("{\"format\":\"k5board\",\"min_reader\":99,\"other\":\"anything\"}"),
        "min_reader_too_high",
    ));
    v.push(zip_no(
        "min-reader-kind",
        with_manifest(&m.replace("\"min_reader\":1", "\"min_reader\":\"1\"")),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "min-reader-zero",
        with_manifest(&m.replace("\"min_reader\":1", "\"min_reader\":0")),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "version-low",
        with_manifest(&m.replace("\"version\":1", "\"version\":0")),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "lineage-bad",
        with_manifest(&m.replace(LINEAGE, "XYZ")),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "lineage-upper",
        with_manifest(&m.replace(LINEAGE, &LINEAGE.to_uppercase())),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "base-bad",
        with_manifest(&m.replacen('{', "{\"base\":{\"sha256\":\"x\"},", 1)),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "base-kind",
        with_manifest(&m.replacen('{', "{\"base\":1,", 1)),
        "manifest_invalid",
    ));
    v.push(zip_no("parts-kind", with_manifest("{\"format\":\"k5board\",\"lineage\":\"0123456789abcdef0123456789abcdef\",\"min_reader\":1,\"parts\":{},\"version\":1}"), "manifest_invalid"));
    v.push(zip_no(
        "part-bytes-kind",
        with_manifest(&m.replacen("\"bytes\":", "\"bytes\":\"x\",\"z\":", 1)),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "part-hash-bad",
        with_manifest(&m.replace(&sha_hex(SMALL.as_bytes()), "abc")),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "part-media",
        with_manifest(&m.replace("application/json", "NOT A TYPE")),
        "manifest_invalid",
    ));
    v.push(zip_no(
        "part-role-bad",
        with_manifest(&m.replace("\"role\":\"board\"", "\"role\":\"Big Role\"")),
        "manifest_invalid",
    ));

    // Parts against the zip.
    let row = |name: &str, role: &str, data: &[u8]| {
        (
            name.to_string(),
            "application/octet-stream".to_string(),
            role.to_string(),
            data.len(),
            sha_hex(data),
        )
    };
    let board_row = row("board.json", "board", SMALL.as_bytes());
    let listing = |rows: Vec<(String, String, String, usize, String)>| manifest_text(None, &rows);
    v.push(zip_no(
        "part-missing",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("extra.bin", "other", b"x"),
        ])),
        "part_missing",
    ));
    v.push(zip_no(
        "part-unlisted",
        forged(|f| f.entries.push(E::new("extra.bin", b"x", STORE))),
        "part_unlisted",
    ));
    v.push(zip_no(
        "part-twice",
        with_manifest(&listing(vec![board_row.clone(), board_row.clone()])),
        "part_duplicate",
    ));
    v.push(zip_no(
        "part-twice-fold",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("EXTRA", "other", b"x"),
            row("extra", "other", b"x"),
        ])),
        "part_duplicate",
    ));
    v.push(zip_no(
        "part-size",
        with_manifest(&listing(vec![(
            "board.json".into(),
            "application/json".into(),
            "board".into(),
            SMALL.len() + 1,
            sha_hex(SMALL.as_bytes()),
        )])),
        "part_disagree",
    ));
    v.push(zip_no(
        "part-board-hash",
        with_manifest(&listing(vec![(
            "board.json".into(),
            "application/json".into(),
            "board".into(),
            SMALL.len(),
            sha_hex(b"other"),
        )])),
        "part_hash",
    ));
    v.push(zip_no(
        "name-listed-bad",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("../x", "other", b"x"),
        ])),
        "zip_name_invalid",
    ));
    v.push(zip_no(
        "no-board",
        with_manifest(&listing(vec![row("extra.bin", "other", b"x")])),
        "part_role",
    ));
    v.push(zip_no(
        "lists-itself",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("manifest.json", "other", b"x"),
        ])),
        "part_role",
    ));
    v.push(zip_no(
        "board-other-name",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("b2.json", "board", b"x"),
        ])),
        "part_role",
    ));
    v.push(zip_no(
        "board-other-role",
        with_manifest(&listing(vec![row("board.json", "other", SMALL.as_bytes())])),
        "part_role",
    ));
    v.push(zip_no(
        "preview-other-name",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("p2.png", "preview", b"x"),
        ])),
        "part_role",
    ));
    v.push(zip_no(
        "preview-other-role",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("preview.png", "other", b"x"),
        ])),
        "part_role",
    ));
    v.push(zip_no(
        "blob-outside",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("x.png", "blob", b"x"),
        ])),
        "part_role",
    ));
    v.push(zip_no(
        "blobs-other-role",
        with_manifest(&listing(vec![
            board_row.clone(),
            row("blobs/x", "other", b"x"),
        ])),
        "part_role",
    ));

    v.push(zip_no(
        "board-swapped",
        with_manifest(&listing(vec![
            row("board.json", "other", SMALL.as_bytes()),
            row("b2.json", "board", b"x"),
        ])),
        "part_role",
    ));

    // Blobs.
    let data = b"\x89PNG not really an image".to_vec();
    let hash = sha_hex(&data);
    let swapped_name: &'static str =
        Box::leak(format!("blobs/sha256-{}", sha_hex(b"other")).into_boxed_str());
    let (entries, _) = package(
        SMALL,
        vec![P {
            name: swapped_name,
            media: "image/png",
            role: "blob",
            data: data.clone(),
        }],
    );
    v.push(zip_no(
        "blob-name-hash",
        Forge::new(entries).build(),
        "part_hash",
    ));
    v.push(zip_no(
        "blob-bytes",
        with_blob(|e| e[2].data = b"\x89PNG not really an imagE".to_vec()),
        "part_hash",
    ));
    // A dangling reference opens and reports; a write refuses; the clearing op then writes.
    let dangling = image_board(&hash);
    let (entries, _) = package(&dangling, Vec::new());
    let mut c = ok("zip", "dangling", &Forge::new(entries).build(), &dangling);
    c.missing = vec![hash.clone()];
    c.cleared = Some(
        b"{\"frames\":[],\"objects\":[{\"id\":\"i-1\",\"type\":\"image\"}],\"people\":{},\"settings\":{}}"
            .to_vec(),
    );
    v.push(c);
    let deep = format!(
        "{{\"frames\":[],\"objects\":[{{\"id\":\"s-1\",\"later\":{{\"a\":{{\"blob\":\"sha256-{hash}\"}}}},\"type\":\"sticky\"}}],\"people\":{{}},\"settings\":{{}}}}"
    );
    let (entries, _) = package(&deep, Vec::new());
    let mut c = ok("zip", "dangling-deep", &Forge::new(entries).build(), &deep);
    c.missing = vec![hash.clone()];
    c.cleared = Some(
        b"{\"frames\":[],\"objects\":[{\"id\":\"s-1\",\"later\":{\"a\":null},\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}"
            .to_vec(),
    );
    v.push(c);

    // The board's own checks still apply inside a package.
    let bad = "{\"frames\":[],\"objects\":[{\"id\":\"BAD\",\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}";
    v.push(zip_no(
        "board-bad",
        Forge::new(package(bad, Vec::new()).0).build(),
        "id_invalid",
    ));

    // One board's own manifest on another board.json of the same size: everything the manifest
    // and the directory say agrees, so only the board's hash tells them apart. A header read
    // accepts it, since it reads the manifest alone; a summary or an open refuses it.
    let other = SMALL.replace("\"title\":\"A\"", "\"title\":\"B\"");
    assert_eq!(other.len(), SMALL.len());
    let mut entries = package(&other, Vec::new()).0;
    entries[0] = base_entries().swap_remove(0);
    v.push(zip_no(
        "manifest-other-board",
        Forge::new(entries).build(),
        "part_hash",
    ));
    v
}

fn rev_cases() -> Vec<Case> {
    let a = "{\"id\":\"a\",\"newField\":{\"a\":null,\"z\":[1,2]},\"text\":\"\u{1F600}\u{e9}\",\"type\":\"sticky\",\"x\":1}";
    let b = "{\"id\":\"b\",\"type\":\"future\",\"y\":2000}";
    let settings = "{\"later\":1,\"title\":\"Caf\u{e9}\"}";
    let spelled_a = "{ \"type\":\"sticky\", \"id\":\"a\", \"x\":1.0, \"text\":\"\\ud83d\\ude00\\u00e9\", \"newField\":{\"z\":[1,2],\"a\":null} }";
    let spelled_b = "{\"y\":2e3,\"id\":\"b\",\"type\":\"future\"}";
    let top = |objects: String| {
        format!("{{\"settings\":{{\"title\":\"Caf\\u00e9\",\"later\":1.0}},\"people\":{{}},\"frames\":[],\"objects\":[{objects}]}}")
    };
    let case = |name: &str, objects: String| {
        let mut c = Case::new("rev", name, top(objects).into_bytes());
        c.revs = vec![("a", sha_hex(a.as_bytes())), ("b", sha_hex(b.as_bytes()))];
        c.settings_rev = Some(sha_hex(settings.as_bytes()));
        c
    };
    vec![
        case("spelled", format!("{spelled_a},{spelled_b}")),
        case("other-position", format!("{spelled_b},{spelled_a}")),
    ]
}

/// A refusal that names every bad id: the expected list, written by hand here.
fn named(name: &str, input: String, ids: &[String], total: usize, truncated: bool) -> Case {
    let mut c = no("json", name, input.as_bytes(), "id_invalid");
    let list: Vec<String> = ids.iter().map(|i| format!("\"{i}\"")).collect();
    c.bad_ids = Some(
        format!(
            "{{\"ids\":[{}],\"total\":{total},\"truncated\":{truncated}}}",
            list.join(",")
        )
        .into_bytes(),
    );
    c
}

fn many_bad(count: usize) -> String {
    let objects: Vec<String> = (0..count)
        .map(|i| format!("{{\"id\":\"Bad{i}\",\"type\":\"sticky\"}}"))
        .collect();
    format!(
        "{{\"frames\":[],\"objects\":[{}],\"people\":{{}},\"settings\":{{}}}}",
        objects.join(",")
    )
}

fn bad_id_cases() -> Vec<Case> {
    let at = "2026-11-03T10:00:00Z";
    let everywhere = format!(
        "{{\"frames\":[\"Frame-Z\"],\"objects\":[\
{{\"checks\":[{{\"done\":false,\"id\":\"Check!\",\"text\":\"\"}}],\"id\":\"Card A\",\"type\":\"card\"}},\
{{\"from\":{{\"id\":\"End From\"}},\"id\":\"c-1\",\"to\":{{\"id\":\"End To\"}},\"type\":\"connector\"}},\
{{\"columns\":[{{\"id\":\"Col A\"}}],\"id\":\"b-1\",\"rows\":[{{\"cells\":{{\"Cell A\":\"x\"}},\"id\":\"Row A\"}}],\"type\":\"table\"}},\
{{\"id\":\"q-1\",\"replies\":[{{\"at\":\"{at}\",\"by\":\"Reply By\",\"id\":\"Reply A\",\"text\":\"\"}}],\"target\":\"Tar Get\",\"type\":\"comment\"}},\
{{\"id\":\"m-1\",\"parent\":\"Par Ent\",\"type\":\"mind\"}},\
{{\"created\":{{\"at\":\"{at}\",\"by\":\"Made By\"}},\"frame\":\"Frm A\",\"group\":\"Team A\",\"id\":\"s-1\",\"reactions\":{{\"x\":[\"React By\"]}},\"type\":\"sticky\",\"votes\":{{\"Vote By\":1}}}},\
{{\"id\":\"Card A\",\"type\":\"sticky\"}},\
{{\"id\":\"s-2\",\"later\":{{\"id\":\"Not An Id\"}},\"type\":\"sticky\"}}\
],\"people\":{{\"P 1\":{{}}}},\"settings\":{{}}}}"
    );
    let order: Vec<String> = [
        "Frame-Z", "Check!", "Card A", "End From", "End To", "Col A", "Cell A", "Row A",
        "Reply By", "Reply A", "Tar Get", "Par Ent", "Made By", "Frm A", "Team A", "React By",
        "Vote By", "P 1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let first = |n: usize| -> Vec<String> { (0..n).map(|i| format!("Bad{i}")).collect() };
    let long = "A".repeat(70);
    let long_board = format!("{{\"frames\":[],\"objects\":[{{\"id\":\"{long}\",\"type\":\"sticky\"}}],\"people\":{{}},\"settings\":{{}}}}");
    let cut = format!("{}\u{2026}", "A".repeat(64));
    vec![
        named("bad-ids-all", everywhere, &order, order.len(), false),
        named("bad-ids-at-cap", many_bad(1000), &first(1000), 1000, false),
        named("bad-ids-past-cap", many_bad(1001), &first(1000), 1001, true),
        named("bad-ids-long", long_board, std::slice::from_ref(&cut), 1, false),
        named(
            "bad-ids-long-twins",
            format!("{{\"frames\":[],\"objects\":[{{\"id\":\"{long}1\",\"type\":\"sticky\"}},{{\"id\":\"{long}2\",\"type\":\"sticky\"}}],\"people\":{{}},\"settings\":{{}}}}"),
            &[cut.clone(), cut],
            2,
            false,
        ),
        named(
            "bad-ids-text-only",
            "{\"frames\":[],\"objects\":[{\"id\":1,\"type\":\"sticky\"},{\"id\":\"Foo\",\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}".to_string(),
            &["Foo".to_string()],
            1,
            false,
        ),
    ]
}

/// The object body of a file-side board, with one person.
fn fboard(objects: &str, frames: &str) -> String {
    format!("{{\"frames\":[{frames}],\"objects\":[{objects}],\"people\":{{\"{P1}\":{{\"kind\":\"person\",\"name\":\"Ann\"}},\"{P2}\":{{\"kind\":\"agent\",\"name\":\"Bot\"}}}},\"settings\":{{}}}}")
}

const WIDGET_README: &str = "# Conversion pairs\n\nEach file here is one shape as Board's widget stores it and as this format holds it, for a host that\nconverts between them. This crate has no converter; it checks that each `file` side is a valid board,\nand that each `file_refused` pair is refused with its code.\n\nThe host's own forms are neutral here: `key-ann` and `key-bot` stand for however a host keys its\npeople, and `hostblob:` for its blob scheme. A host swaps in its own, and tests those on its side.\n\n- `widget` and `file`: the two sides. `people_map` maps the host's person keys to the file's tokens.\n- `lossy`: what doesn't survive, in words.\n- `lossy_direction`: which way the conversion and its `lossy` line hold. `widget_to_file`: converting `widget` gives `file`, or `file` with `file_back` over it. `file_to_widget`: converting `file` gives `widget`, or `widget` with `widget_back` over it. `both`: both of those. A test runs a pair only the way it names. This is test data: no file carries it.\n- `widget_back`, `file_back`: what the way back gives, where it differs. They hold the members that differ, at the level those members sit: `table-sparse`'s `file_back` is a member of the row `rw-0`, not of the object.\n- `widget_when_absent`: what the widget should show for a field the file leaves out.\n- `widget_back_refused`: why the way back can't hold it.\n- `file_refused`, with `file_text` and `widget_text`: a value the widget's store takes and this format refuses. `file_text` is the `board.json` a direct conversion would write, as text, and reading it gives the refusal code in `file_refused`. Both sides are text because the value can be one a strict JSON reader won't parse. These pairs have no `file` or `widget` member.\n\nThe shapes are Board's: portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT. See\n`THIRD-PARTY-NOTICES.md`.\n";

/// One conversion pair between Board's widget shape and the file. `back` holds what the way back
/// gives where it differs, as extra members. `lossy` says what doesn't survive, in words.
fn pair(name: &str, widget: &str, file: &str, frames: &str, more: &str, lossy: &str) -> Case {
    let extra = if more.is_empty() {
        String::new()
    } else {
        format!(",{more}")
    };
    let direction = pair_direction(name);
    let text = format!(
        "{{\"file\":{},\"lossy\":\"{lossy}\",\"lossy_direction\":\"{direction}\",\"people_map\":{{\"key-ann\":\"{P1}\",\"key-bot\":\"{P2}\"}},\"widget\":{widget}{extra}}}\n",
        fboard(file, frames)
    );
    Case::new("widget", name, text.into_bytes())
}

/// Which way a pair's conversion holds. Most hold both ways: see `vectors/widget/README.md`.
fn pair_direction(name: &str) -> &'static str {
    match name {
        "reaction-unknown" => "file_to_widget",
        "none-is-absent" => "widget_to_file",
        _ => "both",
    }
}

/// A value the widget's store takes and this format refuses. Both sides are text: the widget's
/// number can be one a strict reader won't parse at all.
fn refused_pair(name: &str, widget: &str, file: &str, code: &str, lossy: &str) -> Case {
    let quote = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
    let text = format!(
        "{{\"file_refused\":\"{code}\",\"file_text\":\"{}\",\"lossy\":\"{lossy}\",\"lossy_direction\":\"widget_to_file\",\"people_map\":{{}},\"widget_text\":\"{}\"}}\n",
        quote(file),
        quote(widget)
    );
    Case::new("widget", name, text.into_bytes())
}

fn widget_cases() -> Vec<Case> {
    let img = "sha256-9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    let thumbs_up = "\u{1F44D}";
    let heart = "\u{2764}\u{FE0F}";
    let fire = "\u{1F525}";
    vec![
        pair(
            "table",
            r#"{"id":"b-0123456789","type":"table","rows":[["a","b"],["c","d"]],"rowIds":["rw-11111111","rw-22222222"],"colIds":["cl-11111111","cl-22222222"],"colW":[40,60]}"#,
            r#"{"columns":[{"id":"cl-11111111","width":40},{"id":"cl-22222222","width":60}],"id":"b-0123456789","rows":[{"cells":{"cl-11111111":"a","cl-22222222":"b"},"id":"rw-11111111"},{"cells":{"cl-11111111":"c","cl-22222222":"d"},"id":"rw-22222222"}],"type":"table"}"#,
            "",
            "",
            "Nothing.",
        ),
        pair(
            "table-short-row",
            r#"{"id":"b-0123456789","type":"table","rows":[["a","b"],["c"],[]],"rowIds":["rw-0","rw-1","rw-2"],"colIds":["cl-0","cl-1"]}"#,
            r#"{"columns":[{"id":"cl-0"},{"id":"cl-1"}],"id":"b-0123456789","rows":[{"cells":{"cl-0":"a","cl-1":"b"},"id":"rw-0"},{"cells":{"cl-0":"c"},"id":"rw-1"},{"id":"rw-2"}],"type":"table"}"#,
            "",
            "",
            "Nothing: a cell past a short row's end is absent in the file, and an empty text stays an empty text.",
        ),
        pair(
            "table-sparse",
            r#"{"id":"b-0123456789","type":"table","rows":[["","x"]],"rowIds":["rw-0"],"colIds":["cl-0","cl-1"]}"#,
            r#"{"columns":[{"id":"cl-0"},{"id":"cl-1"}],"id":"b-0123456789","rows":[{"cells":{"cl-1":"x"},"id":"rw-0"}],"type":"table"}"#,
            "",
            r#""file_back":{"cells":{"cl-0":"","cl-1":"x"}}"#,
            "A gap before the last cell in a file row comes back as an empty text. Both read as empty.",
        ),
        pair(
            "table-extra-widths",
            r#"{"id":"b-0123456789","type":"table","rows":[["a"]],"rowIds":["rw-0"],"colIds":["cl-0"],"colW":[40,60,80]}"#,
            r#"{"columns":[{"id":"cl-0","width":40}],"id":"b-0123456789","rows":[{"cells":{"cl-0":"a"},"id":"rw-0"}],"type":"table"}"#,
            "",
            r#""widget_back":{"colW":[40]}"#,
            "Widths past the last column describe no column, and are left out. A column with no width has none.",
        ),
        pair(
            "reactions",
            &format!(r#"{{"id":"s-0123456789","type":"sticky","reactions":{{"key-ann":["{fire}","{thumbs_up}"],"key-bot":["{thumbs_up}"]}}}}"#),
            &format!(r#"{{"id":"s-0123456789","reactions":{{"{fire}":["{P1}"],"{thumbs_up}":["{P1}","{P2}"]}},"type":"sticky"}}"#),
            "",
            &format!(r#""widget_back":{{"reactions":{{"key-ann":["{thumbs_up}","{fire}"],"key-bot":["{thumbs_up}"]}}}}"#),
            "One person's order among their reactions: the file has none, and it comes back in the widget's list order.",
        ),
        pair(
            "reaction-unknown",
            &format!(r#"{{"id":"s-0123456789","type":"sticky","reactions":{{"key-ann":["{heart}"]}}}}"#),
            &format!(r#"{{"id":"s-0123456789","reactions":{{"{heart}":["{P1}"],"party-parrot":["{P2}"]}},"type":"sticky"}}"#),
            "",
            r#""widget_back_refused":"party-parrot is a valid file reaction the widget's list doesn't hold, so a reader that keeps only the widget's list must refuse this, or pass the object through untouched""#,
            "Nothing from the widget to the file. The way back can't hold a reaction outside the widget's list.",
        ),
        pair(
            "votes",
            r#"{"id":"s-0123456789","type":"sticky","votes":{"key-ann":2,"key-bot":1}}"#,
            &format!(r#"{{"id":"s-0123456789","type":"sticky","votes":{{"{P1}":2,"{P2}":1}}}}"#),
            "",
            "",
            "Nothing.",
        ),
        pair(
            "end-last-point",
            r#"{"id":"c-0123456789","type":"connector","from":{"id":"s-0123456789","anchor":"top","x":10.5,"y":20},"to":{"x":1,"y":2}}"#,
            r#"{"from":{"anchor":"top","hint":{"x":10.5,"y":20},"id":"s-0123456789"},"id":"c-0123456789","to":{"x":1,"y":2},"type":"connector"}"#,
            "",
            "",
            "Nothing. The widget's x and y beside an id are where the end was last drawn, which is what hint means.",
        ),
        pair(
            "end-no-anchor",
            r#"{"id":"c-0123456789","type":"connector","from":{"id":"s-0123456789","anchor":"auto"}}"#,
            r#"{"from":{"id":"s-0123456789"},"id":"c-0123456789","type":"connector"}"#,
            "",
            r#""file_back":{"from":{"anchor":"auto","id":"s-0123456789"}}"#,
            "An absent anchor comes back as auto, which is its meaning. A writer that passes an untouched object through changes nothing.",
        ),
        pair(
            "checks",
            r#"{"id":"k-0123456789","type":"card","checks":[{"id":"ck-11111111","text":"one","done":true},{"id":"ck-0","text":"two","done":false},{"id":"ck-0x","text":"three","done":false}]}"#,
            r#"{"checks":[{"done":true,"id":"ck-11111111","text":"one"},{"done":false,"id":"ck-0","text":"two"},{"done":false,"id":"ck-0x","text":"three"}],"id":"k-0123456789","type":"card"}"#,
            "",
            "",
            "Nothing. All three id forms fit the id rule.",
        ),
        pair(
            "replies",
            r#"{"id":"q-0123456789","type":"comment","target":"s-0123456789","replies":[{"id":"rp-11111111","by":"key-ann","at":"2026-11-03T10:00:00.000Z","text":"yes"}]}"#,
            &format!(r#"{{"id":"q-0123456789","replies":[{{"at":"2026-11-03T10:00:00.000Z","by":"{P1}","id":"rp-11111111","text":"yes"}}],"target":"s-0123456789","type":"comment"}}"#),
            "",
            "",
            "Nothing.",
        ),
        pair(
            "order",
            r#"[{"id":"s-0000000002","type":"sticky","z":7.5},{"id":"f-0000000001","type":"frame","z":2,"order":1.5},{"id":"f-0000000002","type":"frame","z":3,"order":0.5}]"#,
            r#"{"id":"f-0000000001","type":"frame"},{"id":"f-0000000002","type":"frame"},{"id":"s-0000000002","type":"sticky"}"#,
            r#""f-0000000002","f-0000000001""#,
            r#""widget_back":[{"id":"f-0000000001","z":1,"order":2},{"id":"f-0000000002","z":2,"order":1},{"id":"s-0000000002","z":3}]"#,
            "The numbers z and order: only the order they give survives, by value then id.",
        ),
        pair(
            "pen",
            r#"{"id":"p-0123456789","type":"pen","x":10,"y":20,"w":5,"h":7,"points":[[0,0],[5,7]]}"#,
            r#"{"id":"p-0123456789","points":[[0,0],[5,7]],"type":"pen","x":10,"y":20}"#,
            "",
            "",
            "Nothing: w and h are derived from the points.",
        ),
        pair(
            "image",
            &format!(r#"{{"id":"i-0123456789","type":"image","src":"hostblob:{img}","alt":"a"}}"#),
            &format!(r#"{{"alt":"a","id":"i-0123456789","src":{{"blob":"{img}"}},"type":"image"}}"#),
            "",
            "",
            "Nothing. The file's blob is named by the same hash. This file has no such blob, so it opens with it reported missing; the object's shape is the point here.",
        ),
        pair(
            "created",
            r#"{"id":"s-0123456789","type":"sticky","createdBy":"key-ann","createdAt":"2026-11-03T10:00:00.000Z","updatedBy":"key-bot","updatedAt":"2026-11-03T11:00:00.000Z"}"#,
            &format!(r#"{{"created":{{"at":"2026-11-03T10:00:00.000Z","by":"{P1}"}},"id":"s-0123456789","type":"sticky"}}"#),
            "",
            r#""widget_back":{"updatedBy":"the writer","updatedAt":"now"}"#,
            "updatedBy and updatedAt, by design: they're history, not content.",
        ),
        pair(
            "none-is-absent",
            r#"{"id":"m-0123456789","type":"mind","parent":"","group":"","text":"Idea"}"#,
            r#"{"id":"m-0123456789","text":"Idea","type":"mind"}"#,
            "",
            "",
            "Nothing: an empty group, parent or target means none, and in the file none is absent.",
        ),
        pair(
            "group-frame",
            r#"{"id":"s-0123456789","type":"sticky","group":"g-0123456789","frame":"f-0123456789"}"#,
            r#"{"frame":"f-0123456789","group":"g-0123456789","id":"s-0123456789","type":"sticky"}"#,
            "",
            "",
            "Nothing. The widget carries frame as a field it doesn't know, and keeps it.",
        ),
        pair(
            "defaults",
            r#"{"id":"t-0123456789","type":"text","text":"Text"}"#,
            r#"{"id":"t-0123456789","text":"Text","type":"text"}"#,
            "",
            r#""widget_when_absent":{"id":"t-0123456789","type":"text","text":""}"#,
            "Nothing, if the way back uses the file's default for an absent field. The widget's own default for new objects is a placeholder word, and the file's is empty.",
        ),
        refused_pair(
            "refused-per-zero",
            r#"{"meta":{"voting":{"active":true,"perPerson":0}}}"#,
            r#"{"frames":[],"objects":[],"people":{},"settings":{"voting":{"active":true,"perPerson":0}}}"#,
            "field_value",
            "The widget's store takes a limit of 0. The file's limit is a whole number from 1, so a host refuses it or asks.",
        ),
        refused_pair(
            "refused-per-half",
            r#"{"meta":{"voting":{"active":true,"perPerson":2.5}}}"#,
            r#"{"frames":[],"objects":[],"people":{},"settings":{"voting":{"active":true,"perPerson":2.5}}}"#,
            "field_value",
            "The widget's store takes a limit of 2.5. The file's limit is a whole number from 1, so a host refuses it or asks.",
        ),
        refused_pair(
            "refused-x-range",
            r#"{"id":"s-0123456789","type":"sticky","x":9007199254740994}"#,
            r#"{"frames":[],"objects":[{"id":"s-0123456789","type":"sticky","x":9007199254740994}],"people":{},"settings":{}}"#,
            "json_number_range",
            "The widget's store takes an x past 2^53, as a double. The file judges the literal, and holds nothing past 2^53.",
        ),
    ]
}

pub fn all() -> Vec<Case> {
    let mut v = canon_cases();
    v.extend(json_cases());
    v.extend(zip_cases());
    v.extend(rev_cases());
    v.extend(widget_cases());
    v.extend(bad_id_cases());
    v.extend(fidelity_cases());
    v
}

/// A corpus case: its canonical form is its expected board, and `tests/fidelity.rs` runs each through
/// a file, field by field and verb by verb.
fn fidelity(name: &str, input: &str) -> Case {
    let board = Board::from_json(input.as_bytes(), &Limits::V1)
        .unwrap_or_else(|r| panic!("the corpus case {name}: {r}"));
    let mut c = Case::new("fidelity", name, input.as_bytes().to_vec());
    c.canonical = Some(board.to_canonical_json().expect("canonical"));
    c
}

/// A value of each kind that a field of that kind takes.
fn fidelity_sample(kind: &schema::Kind) -> String {
    use schema::Kind as K;
    let sample = match kind {
        K::Num => "12.5",
        K::Bool => "true",
        K::Str(_) => "\"words\"",
        K::Open(values) | K::Color(values) => return format!("\"{}\"", values[0]),
        K::ObjectRef => "\"s-0\"",
        K::GroupId => "\"g-0\"",
        K::Tags => "[\"a\",\"b\"]",
        K::Points => "[[1,2],[3.5,4]]",
        K::End => "{\"x\":1,\"y\":2}",
        K::Checks => "[{\"done\":false,\"id\":\"i\",\"text\":\"t\"}]",
        K::Columns => "[{\"id\":\"a\",\"width\":40}]",
        K::Rows => "[{\"cells\":{\"a\":\"x\"},\"id\":\"r\"}]",
        K::Reactions => "{\"y\":[\"p-1\"]}",
        K::Votes => "{\"p-1\":2}",
        K::Replies => {
            "[{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"p-1\",\"id\":\"i\",\"text\":\"t\"}]"
        }
        K::Crop => "{\"h\":0.5,\"w\":0.5,\"x\":0,\"y\":0}",
        K::Link => "\"https://example.com/a\"",
        K::Blob => {
            "{\"blob\":\"sha256-9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"}"
        }
        K::Created => "{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"p-1\"}",
        K::Voting => "{\"active\":true,\"perPerson\":2}",
    };
    sample.to_string()
}

/// Every field of every type, set. Built from the tables, so a new field joins it.
fn every_field() -> String {
    let mut objects = vec!["{\"id\":\"s-0\",\"type\":\"sticky\"}".to_string()];
    let mut frames = Vec::new();
    for (i, (type_name, own)) in schema::TYPES.iter().enumerate() {
        let id = format!("x-{i}");
        if *type_name == "frame" {
            frames.push(format!("\"{id}\""));
        }
        let mut members = vec![
            format!("\"id\":\"{id}\""),
            format!("\"type\":\"{type_name}\""),
        ];
        for field in own.iter().chain(schema::COMMON) {
            if field.name == "locked" {
                continue; // a lock would stop the field-by-field edits, and has its own case
            }
            let value = if field.name == "frame" {
                "\"s-0\"".to_string()
            } else {
                fidelity_sample(&field.kind)
            };
            members.push(format!("\"{}\":{value}", field.name));
        }
        objects.push(format!("{{{}}}", members.join(",")));
    }
    let settings: Vec<String> = schema::SETTINGS
        .iter()
        .map(|f| format!("\"{}\":{}", f.name, fidelity_sample(&f.kind)))
        .collect();
    format!(
        "{{\"frames\":[{}],\"objects\":[{}],\"people\":{{\"p-1\":{{\"kind\":\"person\",\"name\":\"Ann\"}}}},\"settings\":{{{}}}}}",
        frames.join(","),
        objects.join(","),
        settings.join(",")
    )
}

fn fidelity_template(name: &str) -> Case {
    let mut n = 0u64;
    let mut random = move || {
        n += 1;
        n.to_le_bytes()
    };
    let mut cx = verbs::Ctx {
        group_id: "t".into(),
        by: None,
        at: None,
        random: &mut random,
    };
    let verb = verbs::Verb::Template {
        name: name.to_string(),
        x: 0.0,
        y: 0.0,
    };
    let mut board = Board::empty();
    let planned = verbs::plan(&board, &mut cx, &verb).expect("a template");
    apply_group(&mut board, &planned.group).expect("it applies");
    let text = String::from_utf8(board.to_canonical_json().expect("canonical")).expect("utf-8");
    fidelity(&format!("template-{name}"), &text)
}

fn fidelity_cases() -> Vec<Case> {
    let hash = "sha256-9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    let mut v = vec![
        fidelity("every-field", &every_field()),
        fidelity("empty", "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{}}"),
        fidelity(
            "unknowns",
            &format!(
                r#"{{"frames":["f-1"],"later":{{"a":[1,{{"b":null}}],"c":"keep"}},"objects":[
{{"id":"u-1","type":"hologram","depth":[[[1.5]]],"anything":{{"x":null,"y":false}}}},
{{"id":"s-1","type":"sticky","color":"ultraviolet","align":"justify","soon":"keep","x":1}},
{{"id":"h-1","type":"shape","shape":"hexagram-new","dash":"wavy","soon":{{"blob":"{hash}"}}}},
{{"id":"c-1","type":"connector","route":"zigzag","startCap":"fish","from":{{"id":"s-1","anchor":"middle","pull":1}},"to":{{"x":1,"y":2,"bend":3}}}},
{{"id":"f-1","type":"frame","fill":"plaid","soon":1}},
{{"id":"k-1","type":"card","checks":[{{"id":"a","text":"t","done":true,"soon":1}}]}},
{{"id":"b-1","type":"table","columns":[{{"id":"a","tint":"red"}}],"rows":[{{"id":"r","tall":true,"cells":{{"a":"x"}}}}]}},
{{"id":"q-1","type":"comment","replies":[{{"id":"a","by":"p-1","at":"2026-11-03T10:00:00Z","text":"t","pin":true}}]}},
{{"id":"i-1","type":"image","src":{{"blob":"{hash}"}},"crop":{{"x":0,"y":0,"w":1,"h":1,"soft":true}}}},
{{"id":"s-2","type":"sticky","created":{{"by":"p-1","at":"2026-11-03T10:00:00Z","via":"import"}}}}
],"people":{{"p-1":{{"name":"Ann","kind":"robot","pronouns":"keep"}}}},"settings":{{"title":"T","voting":{{"active":true,"mode":"dots"}},"soon":{{"keep":[1]}}}}}}"#
            ),
        ),
        fidelity(
            "text",
            "{\"frames\":[],\"objects\":[\
{\"id\":\"a\",\"text\":\"caf\\u00e9 cafe\u{301} \u{4f60}\u{597d} \u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{1F469}\u{200D}\u{1F4BB} \u{1F1E6}\u{1F1FA}\",\"type\":\"sticky\"},\
{\"id\":\"b\",\"text\":\"quote \\\" slash \\\\ solidus \\/ line\\nbreak tab\\t nul\\u0000 del\\u007f bom\u{FEFF} sep\u{2028}\",\"type\":\"text\"},\
{\"id\":\"c\",\"tags\":[\"\u{1F525}\",\"a b\",\"\"],\"text\":\"\",\"type\":\"sticky\"},\
{\"id\":\"d\",\"reactions\":{\"\u{2764}\u{FE0F}\":[\"p-1\"],\"party-parrot\":[\"p-1\"],\"a b\":[\"p-1\"]},\"type\":\"sticky\"},\
{\"id\":\"e\",\"\u{7a7a}\u{6c17} field\":\"a name in any language\",\"type\":\"sticky\"}\
],\"people\":{\"p-1\":{\"name\":\"\u{e9}\u{6c49}\u{1F600}\"}},\"settings\":{\"title\":\"\u{1F4CB} Plan\"}}",
        ),
        fidelity(
            "numbers",
            "{\"frames\":[],\"objects\":[\
{\"id\":\"a\",\"type\":\"sticky\",\"x\":1.0,\"y\":-0,\"w\":1e2,\"h\":1.5E+1,\"rot\":0.10000000000000001},\
{\"id\":\"b\",\"type\":\"sticky\",\"x\":9007199254740992,\"y\":-9007199254740992,\"w\":0.000001,\"h\":1e-7,\"rot\":5e-324},\
{\"id\":\"c\",\"type\":\"sticky\",\"x\":1234567890123456,\"y\":0.1,\"keep\":[1.0,2.50,3e0]},\
{\"id\":\"p\",\"points\":[[0.1,0.2],[1e1,-1e1],[3.141592653589793,2.718281828459045]],\"type\":\"pen\"}\
],\"people\":{},\"settings\":{}}",
        ),
        fidelity(
            "order",
            "{\"frames\":[\"f-b\",\"f-c\",\"f-a\"],\"objects\":[\
{\"id\":\"z\",\"type\":\"sticky\"},{\"id\":\"f-c\",\"type\":\"frame\"},{\"id\":\"a\",\"type\":\"sticky\"},\
{\"id\":\"f-a\",\"type\":\"frame\"},{\"id\":\"m\",\"type\":\"sticky\"},{\"id\":\"f-b\",\"type\":\"frame\"},\
{\"id\":\"0\",\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}",
        ),
        fidelity(
            "references",
            "{\"frames\":[\"f-1\"],\"objects\":[\
{\"id\":\"f-1\",\"type\":\"frame\",\"frame\":\"f-1\"},\
{\"id\":\"s-1\",\"type\":\"sticky\",\"frame\":\"s-2\"},\
{\"id\":\"s-2\",\"type\":\"sticky\",\"frame\":\"gone\"},\
{\"from\":{\"hint\":{\"x\":1,\"y\":2},\"id\":\"gone\"},\"id\":\"c-1\",\"to\":{\"id\":\"s-1\"},\"type\":\"connector\"},\
{\"id\":\"m-1\",\"parent\":\"m-2\",\"type\":\"mind\"},{\"id\":\"m-2\",\"parent\":\"m-1\",\"type\":\"mind\"},\
{\"id\":\"q-1\",\"target\":\"gone\",\"type\":\"comment\"}],\"people\":{},\"settings\":{}}",
        ),
        fidelity(
            "locks",
            "{\"frames\":[\"f-1\"],\"objects\":[\
{\"id\":\"f-1\",\"locked\":true,\"type\":\"frame\"},\
{\"id\":\"s-1\",\"locked\":true,\"text\":\"t\",\"type\":\"sticky\"},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"locked\":true,\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"},\
{\"id\":\"s-2\",\"locked\":false,\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}",
        ),
        fidelity(
            "spelling",
            " {\n  \"settings\" : { } ,\r\n\t\"people\":{ \"p-1\" : { } },\"objects\" : [ { \"type\" : \"sticky\" , \"\\u0074ext\" : \"a\\u00e9\\ud83d\\ude00\" , \"id\" : \"s-1\" } ] , \"frames\" : [ ] } \n",
        ),
    ];
    for (name, _) in verbs::TEMPLATES {
        v.push(fidelity_template(name));
    }
    v
}

/// `vectors/index.json`: canonical text, one case per line for readable diffs.
pub fn index_text(cases: &[Case]) -> String {
    let mut lines = Vec::new();
    for c in cases {
        let mut f = vec![
            format!("\"input\":\"{}/{}.{}\"", c.layer, c.name, c.extension()),
            format!("\"layer\":\"{}\"", c.layer),
            format!("\"name\":\"{}\"", c.name),
        ];
        if let Some(can) = &c.canonical {
            f.push(format!("\"canonical\":\"{}/{}.out.json\"", c.layer, c.name));
            f.push(format!("\"sha256\":\"{}\"", sha_hex(can)));
        }
        if let Some(r) = c.refusal {
            f.push(format!("\"refusal\":\"{r}\""));
        }
        if !c.revs.is_empty() {
            let r: Vec<String> = c
                .revs
                .iter()
                .map(|(k, v)| format!("\"{k}\":\"{v}\""))
                .collect();
            f.push(format!("\"revs\":{{{}}}", r.join(",")));
        }
        if let Some(g) = &c.settings_rev {
            f.push(format!("\"settings_rev\":\"{g}\""));
        }
        if !c.missing.is_empty() {
            let m: Vec<String> = c.missing.iter().map(|h| format!("\"{h}\"")).collect();
            f.push(format!("\"missing\":[{}]", m.join(",")));
        }
        if c.cleared.is_some() {
            f.push(format!(
                "\"cleared\":\"{}/{}.cleared.json\"",
                c.layer, c.name
            ));
        }
        if c.bad_ids.is_some() {
            f.push(format!("\"bad_ids\":\"{}/{}.bad.json\"", c.layer, c.name));
        }
        if let Some(g) = &c.guard {
            f.push(format!("\"guard\":\"{g}\""));
        }
        if c.stable_guard {
            f.push("\"stable_guard\":true".to_string());
        }
        if !c.limits.is_empty() {
            let l: Vec<String> = c
                .limits
                .iter()
                .map(|(k, v)| format!("\"{k}\":{v}"))
                .collect();
            f.push(format!("\"limits\":{{{}}}", l.join(",")));
        }
        f.sort();
        lines.push(format!("{{{}}}", f.join(",")));
    }
    format!(
        "{{\"cases\":[\n{}\n],\"format_version\":1}}\n",
        lines.join(",\n")
    )
}

/// Every generated file, as (path under `vectors/`, bytes).
pub fn files() -> Vec<(String, Vec<u8>)> {
    let cases = all();
    let mut out = vec![
        ("index.json".to_string(), index_text(&cases).into_bytes()),
        (
            "widget/README.md".to_string(),
            WIDGET_README.as_bytes().to_vec(),
        ),
    ];
    for c in &cases {
        out.push((
            format!("{}/{}.{}", c.layer, c.name, c.extension()),
            c.input.clone(),
        ));
        if let Some(can) = &c.canonical {
            out.push((format!("{}/{}.out.json", c.layer, c.name), can.clone()));
        }
        if let Some(cleared) = &c.cleared {
            out.push((
                format!("{}/{}.cleared.json", c.layer, c.name),
                cleared.clone(),
            ));
        }
        if let Some(bad) = &c.bad_ids {
            out.push((format!("{}/{}.bad.json", c.layer, c.name), bad.clone()));
        }
    }
    out
}
