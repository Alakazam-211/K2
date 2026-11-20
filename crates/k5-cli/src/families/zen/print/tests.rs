use super::*;

/// The Python of a shell function's `python3 -c '…'`, by program line
/// (line 1 is the empty line after the quote).
pub fn program(file: &str, func: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let body = text
        .split_once(&format!("\n{func}() {{\n"))
        .unwrap_or_else(|| panic!("no {func} in {file}"))
        .1;
    let start = body.find("python3 -c '\n").expect("python3 -c '") + "python3 -c '".len();
    let py = &body[start..];
    let end = py.find("\n'").expect("the end of the Python");
    py[..end].split('\n').map(str::to_string).collect()
}

#[test]
fn the_raise_lines_are_the_shell_s() {
    let lines = program("zen-guide-data.sh", "_zen_print");
    let at = |n: u32| lines[n as usize - 1].as_str();
    assert!(at(line::SET_UP).contains(r#"d.get("setUp") is False"#));
    assert!(at(line::LOCKED).contains(r#"isinstance(d.get("locked"), dict)"#));
    for (verb, n) in line::VERBS {
        assert!(
            at(*n).contains("d.get("),
            "{verb}: line {n} is {:?}",
            at(*n)
        );
        let arm = (1..*n)
            .rev()
            .map(&at)
            .find(|l| l.contains("verb ==") || l.contains("verb in ("))
            .unwrap();
        assert!(arm.contains(&format!("\"{verb}\"")), "{verb}: {arm}");
    }
}

#[test]
fn human_sizes_are_k2_s_words() {
    assert_eq!(human(0), "0 bytes");
    assert_eq!(human(1023), "1023 bytes");
    assert_eq!(human(1024), "1 KB");
    assert_eq!(human(757_760), "740 KB");
    assert_eq!(human(1_048_576), "1 MB");
    assert_eq!(human(3_670_016), "3.5 MB");
}

#[test]
fn a_reset_answer_prints_its_lines() {
    let s = zen_print(
        "reset",
        false,
        br#"{"file":"zen.toml","restored":"s1","snapshot":"s2"}"#,
        Env::default(),
    );
    assert_eq!(
        s.stdout,
        concat!(
            "reset zen.toml to s1\nprevious file kept as snapshot s2 (",
            k2_client::cli_name!(),
            " zen reset --to s2 to undo)\n"
        )
    );
    assert_eq!(s.exit, Exit::Ok);
}

#[test]
fn a_list_answer_is_python_s_attribute_error() {
    let s = zen_print("doctor", false, b"[1]", Env::default());
    assert_eq!(s.exit, Exit::Error);
    assert!(s
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    assert!(s.stderr.contains("line 222, in <module>"));
}
