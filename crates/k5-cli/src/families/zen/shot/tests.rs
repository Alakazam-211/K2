use super::*;

/// The Python of the `python3 -c '…'` that follows `marker` in
/// `_zen_widget_shot`, by program line (line 1 is the empty line after
/// the quote).
fn program(marker: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src/zen-widget-dev.sh");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let body = text
        .split_once("\n_zen_widget_shot() {\n")
        .expect("_zen_widget_shot")
        .1;
    let body = &body[body.find(marker).unwrap_or_else(|| panic!("no {marker:?}"))..];
    let start = body.find("python3 -c '\n").expect("python3 -c '") + "python3 -c '".len();
    body[start..]
        .split('\n')
        .map(str::to_string)
        .collect()
}

#[test]
fn the_raise_lines_are_the_shell_s() {
    let extra = program("extra=$(ZW=");
    let at = |n: u32| extra[n as usize - 1].as_str();
    assert!(at(line::WIDTH).contains(r#"b["width"] = int(e["ZWI"])"#));
    assert!(at(line::HEIGHT).contains(r#"b["height"] = int(e["ZH"])"#));
    assert!(at(line::LISTDIR).contains("os.listdir(bdir)"));
    assert!(at(line::READ_BLOB).contains(r#"raw = open(p, "rb").read()"#));
    assert!(at(line::GUESS).contains("mimetypes.guess_type(fn)[0]"));
    let check = program(r#"ZA="$answer""#);
    let at = |n: u32| check[n as usize - 1].as_str();
    assert!(at(line::READ_ANSWER).contains(r#"raw = open(os.environ["ZA"]).read()"#));
    assert!(at(line::ERROR).contains(r#"err = d.get("error")"#));
    assert!(at(line::ERRORS).contains(r#"for e in d.get("errors") or []:"#));
    assert!(at(line::ERROR_LINE).contains(r#"e.get("file")"#));
    let report = program(r#"ZO="$out""#);
    let at = |n: u32| report[n as usize - 1].as_str();
    assert!(at(line::LOAD).contains(r#"json.loads(os.environ["ZO"])"#));
    assert!(at(line::SHOT).contains(r#"(d["png"], d["scheme"], d["width"], d["height"], d["ms"])"#));
    assert!(at(line::READY).contains(r#"if not d["ready"]:"#));
    assert!(at(line::EACH_ERROR).contains(r#"for e in d["errors"]:"#));
    assert!(at(line::REFUSALS).contains(r#"if d["refusals"]:"#));
    assert!(at(line::JOIN).contains(r#"", ".join(d["refusals"])"#));
}

fn shown(stdout: &str, stderr: &str, exit: Exit) -> Shown {
    Shown {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit,
    }
}

fn tb(line: u32, exception: &str) -> String {
    format!("Traceback (most recent call last):\n  File \"<string>\", line {line}, in <module>\n{exception}\n")
}

#[test]
fn the_answer_is_checked_as_the_shell_does() {
    let no = concat!(
        "Error: no response from the ",
        product_name!(),
        " daemon on this computer\n"
    );
    assert_eq!(checked(false, b""), Some(shown("", no, Exit::Error)));
    assert_eq!(checked(false, b" \n\r\n"), Some(shown("", no, Exit::Error)));
    assert_eq!(
        checked(false, b"<html>\r\nno"),
        Some(shown(
            "",
            "Error: the daemon answered something that is not JSON: <html>\nno\n",
            Exit::Error
        ))
    );
    assert_eq!(
        checked(false, br#"{"error":"person_only","message":"a person"}"#),
        Some(shown("", "Error: person_only: a person\n", Exit::Refused))
    );
    assert_eq!(
        checked(true, br#"{"error":"zen_off"}"#),
        Some(shown("{\n  \"error\": \"zen_off\"\n}\n", "", Exit::Error))
    );
    assert_eq!(
        checked(false, br#"{"error":["owner_only"],"message":""}"#),
        Some(shown("", "Error: ['owner_only']: \n", Exit::Error))
    );
    assert_eq!(
        checked(
            false,
            br#"{"ok":false,"errors":[{"file":"a.js","line":3,"col":1,"message":"bad"},{"file":"b"}]}"#
        ),
        Some(shown(
            "",
            "The folder has errors; nothing was shot:\na.js:3:1: bad\nb:None:None: None\n",
            Exit::Error
        ))
    );
    // `ok` must be true itself, not 1.
    assert_eq!(
        checked(false, br#"{"ok":1}"#),
        Some(shown("", "The folder has errors; nothing was shot:\n", Exit::Error))
    );
    assert_eq!(
        checked(false, br#"[1]"#),
        Some(shown(
            "",
            &tb(10, "AttributeError: 'list' object has no attribute 'get'"),
            Exit::Error
        ))
    );
    assert_eq!(
        checked(false, br#"{"ok":false,"errors":5}"#),
        Some(shown(
            "",
            &format!(
                "The folder has errors; nothing was shot:\n{}",
                tb(19, "TypeError: 'int' object is not iterable")
            ),
            Exit::Error
        ))
    );
    assert_eq!(
        checked(false, br#"{"ok":false,"errors":["x"]}"#),
        Some(shown(
            "",
            &format!(
                "The folder has errors; nothing was shot:\n{}",
                tb(20, "AttributeError: 'str' object has no attribute 'get'")
            ),
            Exit::Error
        ))
    );
    assert_eq!(
        checked(false, b"\xff"),
        Some(shown(
            "",
            &tb(
                3,
                "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"
            ),
            Exit::Error
        ))
    );
    assert_eq!(checked(false, br#"{"ok":true,"host":"<p>"}"#), None);
}

const TAIL: &str = concat!(
    "The data is the ",
    product_name!(),
    " samples (alpha, bravo, charlie; a sample agent and Thread) unless --data <file.json> gave your own.\n"
);

#[test]
fn the_runner_s_report_is_printed_as_the_shell_does() {
    let r = report(
        r#"{"png":"out.png","scheme":"dark","width":800,"height":600.5,"ms":12,"ready":true,"errors":[],"refusals":[]}"#,
    );
    assert_eq!(
        r,
        shown(
            &format!("Shot out.png (dark, 800x600.5) in 12 ms\n{TAIL}"),
            "",
            Exit::Ok
        )
    );
    let r = report(
        r#"{"png":"o.png","scheme":"light","width":1,"height":2,"ms":3,"ready":false,"errors":["boom",{"a":1}],"refusals":["store.get","agent.send"]}"#,
    );
    assert_eq!(
        r.stdout,
        format!("Shot o.png (light, 1x2) in 3 ms\nThe widget never said ready (k2.ready()): in Zen it would be stopped as not started.\nerror: boom\nerror: {{'a': 1}}\nrefused: store.get, agent.send\n{TAIL}")
    );
    assert_eq!(r.exit, Exit::Ok);
    assert_eq!(
        report(r#"{"png":"o.png"}"#),
        shown("", &tb(4, "KeyError: 'scheme'"), Exit::Error)
    );
    let r = report(
        r#"{"png":"o.png","scheme":"dark","width":1,"height":2,"ms":3,"ready":true,"errors":[],"refusals":[1]}"#,
    );
    assert_eq!(r.stdout, "Shot o.png (dark, 1x2) in 3 ms\n");
    assert_eq!(
        r.stderr,
        tb(10, "TypeError: sequence item 0: expected str instance, int found")
    );
    assert_eq!(r.exit, Exit::Error);
    let r = report("not json");
    assert_eq!(
        r.stderr,
        tb(
            3,
            "json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)"
        )
    );
}

#[test]
fn a_size_is_python_s_int() {
    assert_eq!(int_of("0800", 8).unwrap(), Value::Int("800".into()));
    assert_eq!(int_of("000", 8).unwrap(), Value::Int("0".into()));
    let long = "1".repeat(4301);
    let r = int_of(&long, 9).unwrap_err();
    assert_eq!(
        r.text(),
        tb(9, "ValueError: Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit")
    );
    assert!(int_of(&"1".repeat(4300), 9).is_ok());
}

#[test]
fn a_key_set_twice_keeps_its_first_place() {
    let mut d = vec![
        (Text::from("a"), Value::Null),
        (Text::from("blobs"), Value::Null),
        (Text::from("z"), Value::Null),
    ];
    set(&mut d, "blobs", Value::Bool(true));
    set(&mut d, "new", Value::Bool(false));
    let keys: Vec<String> = d.iter().map(|(k, _)| k.as_str().unwrap().to_string()).collect();
    assert_eq!(keys, ["a", "blobs", "z", "new"]);
    assert_eq!(d[1].1, Value::Bool(true));
}

/// The fixture's `blobs/` folder: each file by its sha256 ref in name
/// order (hidden files and folders skipped, the same bytes twice one
/// ref, the later name's), with Python's type for its name.
#[test]
fn the_blobs_folder_is_read_as_the_shell_does() {
    let dir = std::env::temp_dir().join(format!("k5-zen-shot-blobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let b = dir.join("blobs");
    std::fs::create_dir_all(b.join("sub")).unwrap();
    std::fs::write(b.join("b.png"), [1u8, 2, 3]).unwrap();
    std::fs::write(b.join("a.noext-here"), "hello").unwrap();
    std::fs::write(b.join("c.png"), "hello").unwrap();
    std::fs::write(b.join(".hidden.png"), "x").unwrap();
    let got = blobs(&b.to_string_lossy()).unwrap();
    let hello = format!(
        "k2blob:sha256-{}",
        k2_client::release_sums::sha256_hex(b"hello")
    );
    let bytes = format!(
        "k2blob:sha256-{}",
        k2_client::release_sums::sha256_hex(&[1u8, 2, 3])
    );
    let keys: Vec<String> = got.iter().map(|(k, _)| k.as_str().unwrap().to_string()).collect();
    assert_eq!(keys, [hello.clone(), bytes]);
    let first = &got[0].1;
    assert_eq!(print::g(first, "name"), Some(&jstr("c.png")));
    assert_eq!(print::g(first, "mime"), Some(&jstr("image/png")));
    assert_eq!(print::g(first, "base64"), Some(&jstr("aGVsbG8=")));
    assert_eq!(
        print::g(&got[1].1, "base64"),
        Some(&jstr("AQID"))
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_flags_are_there_when_any_is_set() {
    assert!(!Flags::default().any());
    let f = Flags {
        wait_ms: "5".into(),
        ..Flags::default()
    };
    assert!(f.any());
}
