use super::*;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/sidecar.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The Python that starts after the `n`th `python3 -c '` following
/// `after`, its lines as `python3 -c` numbers them: line 1 is the empty
/// one after `-c '`.
fn python_after(after: &str) -> Vec<String> {
    let text = source();
    let at = text.find(after).unwrap_or_else(|| panic!("no {after:?}"));
    let body = &text[at..];
    let start = body.find("python3 -c '").expect("its python3") + "python3 -c '".len();
    let body = &body[start..];
    let end = body.find("\n'").expect("its end");
    body[..end].split('\n').map(str::to_string).collect()
}

fn line_of(py: &[String], line: u32) -> &str {
    py[line as usize - 1].trim()
}

fn w(list: &[&str]) -> Vec<String> {
    list.iter().map(|x| x.to_string()).collect()
}

/// Each line the words and the calls follow, as the shell has them.
#[test]
fn the_shell_s_lines() {
    let text = source();
    for shell in [
        r#"        ""|help|--help|-h) _sidecar_usage; return 0 ;;"#,
        r#"    if _wants_help "$@"; then"#,
        r#"            --base) base="${2:-}"; shift 2 ;;"#,
        r#"            --harness) harness="${2:-}"; shift 2 ;;"#,
        r#"                if [ -z "$name" ] && [ $# -gt 0 ] && [[ "$1" != -* ]]; then"#,
        r#"            --) shift; break ;;"#,
        r#"            -*) echo "usage: unknown flag '$1' (k2 sidecar --help)" >&2; exit 2 ;;"#,
        r#"                    echo "usage: one name only; quote names with spaces (k2 sidecar --help)" >&2"#,
        r#"        echo "usage: k2 sidecar new <name> --harness <preset> [--brief <file>|-] [--brief-text \"…\"] [--worktree [--base <ref>]]" >&2"#,
        r#"        echo "usage: --base goes with --worktree (k2 sidecar --help)" >&2"#,
        r#"        echo "usage: pass --brief or --brief-text, not both" >&2"#,
        r#"            echo "usage: brief file not found: $brief_file" >&2"#,
        r#"    [ "$rc" -ne 0 ] && exit 2"#,
        r#"    raw="$(_sidecar_send POST /cli/sidecar/new "$body")" || true"#,
        r#"            *) echo "usage: k2 sidecar list [--workspace <ws>] [--json]" >&2; exit 2 ;;"#,
        r#"    raw="$(_sidecar_send GET /cli/sidecar/list "workspace=$(urlencode "$workspace")")" || true"#,
        r#"    local usage="usage: k2 sidecar archive <name>|--all-stopped [--workspace <ws>] [--json]""#,
        r#"    if { [ -z "$name" ] && [ "$all" = 0 ]; } || { [ -n "$name" ] && [ "$all" = 1 ]; }; then"#,
        r#"                    echo "usage: k2 sidecar stop <name> [--workspace <ws>] [--force] [--json]" >&2"#,
        r#"            --keep|--discard|--keep-branch) _SCW_FLAGS="${_SCW_FLAGS}$1 "; shift ;;"#,
        r#"            -*) echo "usage: unknown flag '$1' ($usage)" >&2; exit 2 ;;"#,
        r#"    if ! { [ -t 0 ] && [ -t 2 ]; }; then"#,
        r#"        echo "not done: no terminal to ask on; read the above, then run it again with --yes" >&2"#,
        r#"    printf '%s [y/N] ' "$question" >&2"#,
        r#"    case "$ans" in y|Y|yes|YES) return 0 ;; esac"#,
        r#"    _sidecar_name_args "k2 sidecar interrupt <name> [--workspace <ws>] [--json]" "$@""#,
        r#"    _sidecar_name_args "k2 sidecar review <name> [--workspace <ws>] [--json]" "$@""#,
        r#"    _sidecar_name_args "k2 sidecar land <name> [--keep] [--yes] [--workspace <ws>] [--json]" "$@""#,
        r#"    _sidecar_name_args "k2 sidecar retire <name> [--discard] [--keep-branch] [--yes] [--workspace <ws>] [--json]" "$@""#,
        r#"        _sidecar_print_review "$_SC_BODY" >&2"#,
        r#"    if [ "$blocked" = "already_landed" ] || [ "$blocked" = "sidecar_retired" ]; then"#,
        r#"    _sidecar_confirm "Land these on the base now?""#,
        r#"        _sidecar_confirm "Discard this work (commits deleted with the branch, files to the Trash)?""#,
        r#"            *) echo "usage: k2 sidecar access on|off|status [--workspace <ws>]" >&2; exit 2 ;;"#,
        r#"            raw="$(_sidecar_send POST /cli/agent-access/set "$body")" || true"#,
        r#"            _sidecar_render access 0 "$raw""#,
        r#"            echo "usage: unknown sidecar verb '$sub' (new|list|stop|archive|interrupt|review|land|retire|access — k2 sidecar --help)" >&2"#,
    ] {
        assert!(text.contains(shell), "sidecar.sh no longer has: {shell}");
    }
}

/// The readers' lines where this says Python raises.
#[test]
fn the_readers_lines() {
    let new = python_after("        new)\n            printf");
    assert_eq!(line_of(&new, 3), "v = json.load(sys.stdin)");
    assert_eq!(line_of(&new, 4), "print(v[\"address\"])");
    assert!(line_of(&new, 7).starts_with("extra = (\" with brief %s (%d bytes)\""));
    assert!(line_of(&new, 12).contains("(wt.get(\"baseSha\") or \"\")[:10]"));
    assert!(line_of(&new, 13).starts_with("sys.stderr.write("));
    let list = python_after("        list)\n            printf");
    assert_eq!(line_of(&list, 4), "rows = v.get(\"sidecars\", [])");
    assert!(line_of(&list, 12).starts_with("when = time.strftime(\"%Y-%m-%d %H:%M\""));
    assert_eq!(line_of(&list, 18), "task += \" +%d\" % wt.get(\"ahead\")");
    assert!(line_of(&list, 21).contains("(r.get(\"name\") or \"-\")[:20]"));
    assert!(line_of(&list, 22).contains("(r.get(\"createdBy\") or \"app\")[:14]"));
    assert!(line_of(&list, 25).starts_with("print(\"worktree sidecars: %d of %d"));
    let archive = python_after("        archive)\n            printf");
    assert!(line_of(&archive, 5).contains("v[\"address\"].split(\"/\")[-1]"));
    assert!(line_of(&archive, 8).contains("\", \".join(v.get(\"archived\") or [])"));
    assert!(line_of(&archive, 10).starts_with("print(\"not archived %s: %s\""));
    let review = python_after("_sidecar_print_review() {");
    assert!(line_of(&review, 8).starts_with("print(\"%d commit%s to land on %s (at %s):\""));
    assert!(line_of(&review, 10).contains("c[\"sha\"][:10]"));
    assert!(line_of(&review, 18).contains("\", \".join(dirty)"));
    assert!(line_of(&review, 21).contains("\", \".join(v.get(\"mainDirty\") or [])"));
    assert!(line_of(&review, 28).contains("why.get(b, b)"));
    let land = python_after("        land)\n            printf");
    assert!(line_of(&land, 5).starts_with("print(\"landed %d commit%s from %s on %s (%s..%s)\""));
    assert!(line_of(&land, 12).starts_with("print(\"retire it later: k2 sidecar retire %s\""));
    let retire = python_after("        retire)\n            printf");
    assert!(line_of(&retire, 7).contains("dc[-1][\"sha\"][:10]"));
    let body = python_after("K2_SC_BRIEF_FILE=\"$brief_src\"");
    assert_eq!(line_of(&body, 13), "raw = open(path, \"rb\").read()");
    let words = python_after("read -r tip onto blocked");
    assert!(line_of(&words, 4).starts_with("print(v.get(\"tip\") or \"-\""));
}

#[test]
fn the_help_and_the_verb() {
    for help in [
        &[][..],
        &["help"],
        &["--help"],
        &["-h"],
        &["new", "x", "--harness", "-h"],
        &["list", "--help"],
        &["bogus", "-h"],
    ] {
        assert_eq!(parse(help), Parsed::Help, "{help:?}");
    }
    assert_eq!(
        parse(&["ls", "--json"]),
        Parsed::Verb("ls".into(), w(&["--json"]))
    );
    assert_eq!(
        unknown_verb("x"),
        format!("usage: unknown sidecar verb 'x' (new|list|stop|archive|interrupt|review|land|retire|access \u{2014} {CLI} sidecar --help)")
    );
}

#[test]
fn new_s_words() {
    let n = parse_new(&w(&["a", "--harness", "claude", "--model", "m", "--json"])).unwrap();
    assert_eq!(
        (n.name.as_str(), n.harness.as_str(), n.model.as_str()),
        ("a", "claude", "m")
    );
    assert!(n.json);
    // `--worktree <name>` names it; a word with a dash is not taken.
    let n = parse_new(&w(&["--worktree", "b", "--harness", "h"])).unwrap();
    assert_eq!(n.name, "b");
    assert!(n.worktree);
    assert_eq!(
        parse_new(&w(&["--worktree", "-x", "--harness", "h"])),
        Err(Stop::Usage(unknown_flag("-x")))
    );
    assert_eq!(
        parse_new(&w(&["a", "--worktree", "b", "--harness", "h"])),
        Err(Stop::Usage(format!(
            "usage: one name only; quote names with spaces ({CLI} sidecar --help)"
        )))
    );
    // Words after `--` are dropped.
    let n = parse_new(&w(&["a", "--harness", "h", "--", "--bogus", "x"])).unwrap();
    assert_eq!(n.name, "a");
    assert_eq!(parse_new(&w(&["a", "--harness"])), Err(Stop::Shift("--harness".into())));
    assert_eq!(parse_new(&w(&["a", "--brief-text"])), Err(Stop::Shift("--brief-text".into())));
    assert_eq!(parse_new(&w(&["a"])), Err(Stop::Usage(new_usage())));
    assert_eq!(
        parse_new(&w(&["--harness", "h"])),
        Err(Stop::Usage(new_usage()))
    );
    assert_eq!(
        parse_new(&w(&["a", "--harness", "h", "--base", "main"])),
        Err(Stop::Usage(format!(
            "usage: --base goes with --worktree ({CLI} sidecar --help)"
        )))
    );
    assert_eq!(
        parse_new(&w(&[
            "a",
            "--harness",
            "h",
            "--brief",
            "f",
            "--brief-text",
            ""
        ])),
        Err(Stop::Usage(
            "usage: pass --brief or --brief-text, not both".into()
        ))
    );
    assert_eq!(parse_new(&w(&["-"])), Err(Stop::Usage(unknown_flag("-"))));
}

#[test]
fn new_s_body() {
    let n = parse_new(&w(&[
        "a é",
        "--harness",
        "h",
        "--worktree",
        "--base",
        "main",
        "--model",
        "m",
    ]))
    .unwrap();
    assert_eq!(
        n.body("acme", None).unwrap(),
        r#"{"workspace": "acme", "name": "a \u00e9", "harness": "h", "model": "m", "worktree": true, "base": "main"}"#
    );
    let n = parse_new(&w(&["a", "--harness", "h", "--brief-text", ""])).unwrap();
    assert_eq!(
        n.body("w", None).unwrap(),
        r#"{"workspace": "w", "name": "a", "harness": "h", "brief": ""}"#
    );
    let n = parse_new(&w(&["a", "--harness", "h", "--brief", "-"])).unwrap();
    assert_eq!(
        n.body("w", Some(Ok(b"line\n".to_vec()))).unwrap(),
        r#"{"workspace": "w", "name": "a", "harness": "h", "brief": "line\n"}"#
    );
    assert_eq!(
        n.body("w", Some(Ok(vec![0xff]))),
        Err("brief_not_text: the brief is not UTF-8 text\n".into())
    );
}

#[test]
fn the_other_verbs_words() {
    assert_eq!(parse_simple("list", &w(&["--workspace"])), Err(Stop::Shift("--workspace".into())));
    assert_eq!(
        parse_simple("list", &w(&["x"])),
        Err(Stop::Usage(list_usage()))
    );
    assert_eq!(
        parse_simple("stop", &w(&[])),
        Err(Stop::Usage(stop_usage()))
    );
    assert_eq!(
        parse_simple("stop", &w(&["a", "--force", "--bad"])),
        Err(Stop::Usage(unknown_flag("--bad")))
    );
    assert_eq!(
        parse_simple("archive", &w(&["a", "--all-stopped"])),
        Err(Stop::Usage(archive_usage()))
    );
    assert!(parse_simple("archive", &w(&["--all-stopped"])).is_ok());
    assert_eq!(
        parse_access(&w(&["on", "--json"])),
        Err(Stop::Usage(access_usage()))
    );
    assert_eq!(parse_access(&w(&["on", "--workspace"])), Err(Stop::Shift("--workspace".into())));
    assert_eq!(
        parse_access(&w(&["status", "--workspace", "x"])),
        Ok(("status".into(), "x".into()))
    );
    let u = verb_usage("land");
    assert_eq!(
        u,
        format!("{CLI} sidecar land <name> [--keep] [--yes] [--workspace <ws>] [--json]")
    );
    assert_eq!(
        name_args(&u, &w(&["a", "-q"])),
        Err(Stop::Usage(format!("usage: unknown flag '-q' ({u})")))
    );
    // Any verb takes any of the Workbench switches.
    let n = name_args(&verb_usage("review"), &w(&["a", "--discard", "-y"])).unwrap();
    assert!(n.discard && n.yes);
}

#[test]
fn read_s_three_words() {
    assert_eq!(read_three("a b c"), ("a".into(), "b".into(), "c".into()));
    assert_eq!(
        read_three("  a\t b  c d  "),
        ("a".into(), "b".into(), "c d".into())
    );
    assert_eq!(read_three("a"), ("a".into(), String::new(), String::new()));
    let v = crate::out::json::parse(r#"{"tip":"t 1","blocked":"x\ny"}"#).unwrap();
    assert_eq!(land_words(&v), ("t".into(), "1".into(), "- x".into()));
}

fn shown(reader: Reader, body: &str) -> Shown {
    lay_out(reader, body.as_bytes())
}

#[test]
fn the_readers() {
    let o = shown(
        Reader::New,
        r#"{"address":"ws/a","harness":"h","brief":{"path":"/b","bytes":3},"worktree":{"dir":"/d","branch":"b","base":"main","baseSha":"0123456789abc"}}"#,
    );
    assert_eq!(o.stdout, "ws/a\n");
    assert_eq!(
        o.stderr,
        format!("opened ws/a (h) with brief /b (3 bytes) in /d on branch b (from main 0123456789). Message it: {CLI} msg ws/a \"...\"\n")
    );
    let o = shown(Reader::Stop, r#"{"address":"a"}"#);
    assert_eq!(o.stdout, "a was already asleep\n");
    let o = shown(
        Reader::Archive,
        r#"{"count":2,"archived":["x","y"],"skipped":[{"address":"z","hint":"live"}]}"#,
    );
    assert_eq!(
        o.stdout,
        "archived 2 stopped sidecars: x, y\nnot archived z: live\n"
    );
    let o = shown(Reader::Archive, r#"{"address":"ws/n"}"#);
    assert_eq!(
        o.stdout,
        format!("archived ws/n. Its conversation stays: {CLI} sidecar new n --harness <the same harness> resumes it.\n")
    );
    let o = shown(
        Reader::List,
        r#"{"workspace":"w","live":1,"cap":4,"sidecars":[]}"#,
    );
    assert_eq!(
        o.stdout,
        "w: 1 live of 0 (cap 4); its agent may open sidecars: off\n(no sidecars)\n"
    );
    let o = shown(
        Reader::Retire,
        r#"{"address":"a","how":"h","discardedCommits":[{"sha":"0123456789abcdef"}]}"#,
    );
    assert_eq!(
        o.stdout,
        "retired a (h): worktree in the Trash, branch kept\ndiscarded 1 commit(s); until git cleans up, git branch <name> 0123456789 brings them back\n"
    );
    let o = shown(
        Reader::Review,
        r#"{"address":"a","blocked":"wrong_branch","mainBranch":"dev","onto":"main"}"#,
    );
    assert_eq!(
        o.stdout,
        "a  None in None  (None)\n0 commits to land on main (at -):\nland: not now (wrong_branch): the main checkout is on dev, not main\n"
    );
}

#[test]
fn where_the_readers_raise() {
    let o = shown(Reader::Stop, "nope");
    assert_eq!(o.exit, Exit::Error);
    assert!(
        o.stderr
            .ends_with("json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)\n"),
        "{}",
        o.stderr
    );
    let o = shown(Reader::New, r#"{"harness":"h"}"#);
    assert!(
        o.stderr
            .contains("line 4, in <module>\nKeyError: 'address'\n"),
        "{}",
        o.stderr
    );
    let o = shown(Reader::List, r#"{"sidecars":null}"#);
    assert!(
        o.stderr
            .ends_with("line 5, in <module>\nTypeError: object of type 'NoneType' has no len()\n"),
        "{}",
        o.stderr
    );
    let o = shown(Reader::List, r#"{"sidecars":[{"name":5}]}"#);
    assert!(
        o.stderr
            .ends_with("line 21, in <module>\nTypeError: 'int' object is not subscriptable\n"),
        "{}",
        o.stderr
    );
    let o = shown(Reader::Archive, r#"{"address":7}"#);
    assert!(
        o.stderr
            .ends_with("AttributeError: 'int' object has no attribute 'split'\n"),
        "{}",
        o.stderr
    );
    let o = shown(Reader::Review, r#"{"address":"a","blocked":["x"]}"#);
    assert!(
        o.stderr
            .ends_with("line 28, in <module>\nTypeError: unhashable type: 'list'\n"),
        "{}",
        o.stderr
    );
    let o = shown(
        Reader::Retire,
        r#"{"address":"a","discardedCommits":{"x":1}}"#,
    );
    assert!(
        o.stderr.ends_with("line 7, in <module>\nKeyError: -1\n"),
        "{}",
        o.stderr
    );
}

#[test]
fn python_s_slice() {
    assert_eq!(py_slice(&s("héllo"), 2, 1), Ok(s("hé")));
    assert_eq!(
        py_slice(&crate::out::json::parse("[1,2,3]").unwrap(), 2, 1),
        Ok(crate::out::json::parse("[1,2]").unwrap())
    );
    assert_eq!(
        py_slice(&Value::Object(Vec::new()), 2, 9),
        Err(Raise::at(9, "TypeError: unhashable type: 'slice'"))
    );
    assert_eq!(
        py_slice(&Value::Null, 2, 9),
        Err(Raise::at(
            9,
            "TypeError: 'NoneType' object is not subscriptable"
        ))
    );
}
