use super::read::*;
use super::*;
use crate::families::Shown;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/wiki.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `python3 -c '…'` blocks of the function `name` in `wiki.sh`, each
/// as `python3 -c` numbers its lines (line 1 is the empty one after `'`).
fn python(name: &str) -> Vec<Vec<String>> {
    let text = source();
    let start = text
        .find(&format!("\n{name}() {{\n"))
        .unwrap_or_else(|| panic!("no {name}"));
    let body = &text[start + 1..];
    // The function ends where the next one starts.
    let first = body.find('\n').expect("the function's first line") + 1;
    let next = body[first..].find("() {\n").map_or(body.len(), |at| {
        body[..first + at].rfind('\n').expect("a line before it")
    });
    let body = &body[..next];
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find("python3 -c '\n") {
        let py = &rest[at + "python3 -c '".len()..];
        let end = py.find("\n'").expect("the end of the Python");
        out.push(py[..end].split('\n').map(str::to_string).collect());
        rest = &py[end..];
    }
    out
}

fn line(block: &[String], n: u32) -> &str {
    block[n as usize - 1].trim()
}

/// Each line a reader raises at or branches on, as the shell has it.
#[test]
fn the_readers_lines() {
    let st = &python("cmd_wiki_status")[0];
    assert_eq!(st.len(), 76);
    assert_eq!(line(st, 30), r#"note_count = idx.get("noteCount")"#);
    assert_eq!(
        line(st, 32),
        r#"note_count = sum(1 for n in idx["nodes"] if n.get("exists", True))"#
    );
    assert_eq!(
        line(st, 33),
        r#"wiki_rel = idx.get("wikiRel") or ".k2/wiki""#
    );
    assert_eq!(
        line(st, 34),
        r#"ws_path = idx.get("workspacePath") or project"#
    );
    assert_eq!(
        line(st, 35),
        r#"enabled = bool(d.get("enabled") or d.get("serving"))"#
    );
    assert_eq!(
        line(st, 38),
        r#"pub = d.get("publishedLabel") or d.get("published_label") or d.get("label")"#
    );
    assert_eq!(
        line(st, 39),
        r#"generated = idx.get("generatedAt") or d.get("generatedAt")"#
    );
    assert_eq!(line(st, 67), r#"if d.get("publicChatEnabled"):"#);
    let ix = python("cmd_wiki_index");
    assert_eq!(ix.len(), 2);
    assert_eq!(ix[0].len(), 12);
    let t = &ix[1];
    assert_eq!(t.len(), 31);
    assert_eq!(line(t, 13), r#"nodes = d.get("nodes") or []"#);
    assert_eq!(line(t, 14), r#"links = d.get("links") or []"#);
    assert_eq!(
        line(t, 15),
        r#"exists = [n for n in nodes if n.get("exists", True)]"#
    );
    assert_eq!(
        line(t, 16),
        r#"missing = [n for n in nodes if not n.get("exists", True)]"#
    );
    assert_eq!(line(t, 20), r#"print("  Links:      %s" % len(links))"#);
    assert_eq!(
        line(t, 28),
        r#"for n in sorted(exists, key=lambda x: (x.get("title") or x.get("id") or "").lower())[:50]:"#
    );
    assert_eq!(
        line(t, 29),
        r#"print("  %-28s  %s" % ((n.get("id") or "")[:28], n.get("title") or ""))"#
    );
    let nt = &python("cmd_wiki_note")[1];
    assert_eq!(nt.len(), 22);
    assert_eq!(
        line(nt, 13),
        r#"title = d.get("title") or d.get("id") or "?""#
    );
    assert_eq!(line(nt, 15), r#"tags = d.get("tags") or []"#);
    assert_eq!(line(nt, 17), r#"print("tags: %s" % ", ".join(tags))"#);
    assert_eq!(line(nt, 19), r#"body = d.get("body")"#);
    assert_eq!(
        line(nt, 21),
        r#"body = d.get("content") or d.get("markdown") or """#
    );
    assert_eq!(
        line(nt, 22),
        r#"sys.stdout.write(body if body.endswith("\n") else body + "\n")"#
    );
    let sd = &python("cmd_wiki_seed")[0];
    assert_eq!(sd.len(), 24);
    assert_eq!(
        line(sd, 13),
        r#"created = d.get("created") or d.get("files") or []"#
    );
    assert_eq!(line(sd, 18), "for f in created:");
    assert_eq!(
        line(sd, 22),
        r#"wiki = d.get("wikiRel") or d.get("path") or ".k2/wiki""#
    );
    let sv = &python("cmd_wiki_serve")[0];
    assert_eq!(sv.len(), 25);
    assert_eq!(
        line(sv, 14),
        r#"enabled = bool(d.get("enabled") if "enabled" in d else d.get("serving", want))"#
    );
    assert_eq!(line(sv, 15), r#"port = d.get("port")"#);
    assert_eq!(
        line(sv, 16),
        r#"url = d.get("url") or (("http://127.0.0.1:%s" % port) if port else None)"#
    );
    assert_eq!(
        line(sv, 23),
        r#"if d.get("publishedLabel") or d.get("published_label"):"#
    );
    let ch = &python("cmd_wiki_chat")[0];
    assert_eq!(ch.len(), 17);
    assert_eq!(line(ch, 13), r#"if d.get("publicChatEnabled"):"#);
    let un = python("cmd_wiki_unpublish");
    assert_eq!(
        line(&un[0], 5),
        r#"print(d.get("publishedLabel") or d.get("published_label") or d.get("label") or "")"#
    );
    assert_eq!(line(&un[1], 15), r#"print("Wiki serve OFF")"#);
    let ap = &python("_wiki_apply_project")[0];
    assert_eq!(
        line(ap, 8),
        r#"print(d.get("path") or d.get("projectPath") or d.get("project_path") or "")"#
    );
}

/// The shell's arms and lines the binary copies.
#[test]
fn the_shell_s_lines() {
    let text = source();
    for shell in [
        r#"        AS_JSON=true python3 -c '"#,
        r#"' <<<"$raw""#,
        r#"    raw=$(cli_request "/cli/wiki/serve/status")"#,
        r#"    idx_raw=$(cli_request "/cli/wiki/index" 2>/dev/null || true)"#,
        r#"    raw=$(cli_request "/cli/wiki/note" "id=$(urlencode "$note_id")")"#,
        r#"    raw=$(cli_post_form "/cli/wiki/seed")"#,
        r#"        raw=$(cli_post_form "/cli/wiki/serve" "enabled=${enabled}" "port=$(urlencode "$port")")"#,
        r#"    raw=$(cli_post_form "/cli/wiki/chat" "enabled=false")"#,
        r#"    status_raw=$(cli_request "/cli/wiki/serve/status" 2>/dev/null || true)"#,
        r#"    raw=$(cli_post_form "/cli/wiki/serve" "enabled=false")"#,
        r#"            echo "Unknown 'k2 wiki' subcommand: $sub" >&2"#,
        r#"                _wiki_err "usage" "unknown flag '$1' — usage: k2 wiki note <id> [--json] [project]""#,
        r#"                    _wiki_err "usage" "$1 requires a workspace path/name""#,
        r#"            _wiki_err "public_chat_unavailable" "Public wiki chat is coming back with the K2 runtime. It is off on every workspace for now; hosting is being rebuilt on the runtime.""#,
        r#"            [ -z "$mode" ] && exit 2 || exit 0"#,
        r#"        echo "  k2 publish subdomain unclaim $pub_label   # drop workspace attribution""#,
        r#"    echo "Notes under .k2/wiki/ are kept (unpublish does not delete the vault).""#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "wiki.sh no longer has {shell:?}"
        );
    }
    let policy = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh"),
    )
    .unwrap();
    assert!(policy.contains("        help|schema|whats-new|checkin|activity|read|skills|wiki|"));
    assert!(policy.contains(
        "        [ \"${2:-}\" = \"chat\" ] && case \"${3:-}\" in on|enable|true|1) _skip_conn_gate=1 ;; esac"
    ));
}

fn ask(verb: Verb, json: bool, ws: &str, port: &str, label: &str) -> Form {
    Form::Ask(
        verb,
        Opts {
            json,
            workspace: ws.into(),
            port: port.into(),
            label: label.into(),
        },
    )
}

#[test]
fn the_words() {
    assert_eq!(parse(&[]), Form::Help);
    assert_eq!(parse(&["help"]), Form::Help);
    assert_eq!(parse(&["x"]), Form::Unknown("x".into()));
    assert_eq!(parse(&["status"]), ask(Verb::Status, false, "", "", ""));
    assert_eq!(
        parse(&["status", "--json", "a", "b"]),
        ask(Verb::Status, true, "a", "", "")
    );
    assert_eq!(
        parse(&["index", "--workspace=w", "a"]),
        ask(Verb::Index, false, "w", "", "")
    );
    assert_eq!(
        parse(&["index", "--", "--json"]),
        ask(Verb::Index, false, "--json", "", "")
    );
    assert_eq!(
        parse(&["seed", "--json", "--port"]),
        Form::Usage {
            json: true,
            hint: "--port requires a value".into()
        }
    );
    assert_eq!(
        parse(&["seed", "--project", ""]),
        Form::Usage {
            json: false,
            hint: "--project requires a workspace path/name".into()
        }
    );
    assert_eq!(
        parse(&["seed", "-x", "--json"]),
        Form::Usage {
            json: false,
            hint: "unknown flag '-x'".into()
        }
    );
    assert_eq!(
        parse(&["serve", "on", "--port=", "p"]),
        ask(Verb::Serve(true), false, "p", "", "")
    );
    assert_eq!(
        parse(&["serve", "stop", "--port", "9"]),
        ask(Verb::Serve(false), false, "", "9", "")
    );
    assert_eq!(parse(&["serve"]), Form::Page("wiki/serve", Exit::Usage));
    assert_eq!(
        parse(&["serve", "help"]),
        Form::Page("wiki/serve", Exit::Ok)
    );
    assert!(matches!(parse(&["serve", "up"]), Form::Usage { .. }));
    assert_eq!(parse(&["chat", "1", "--json"]), Form::ChatOn { json: true });
    assert!(matches!(
        parse(&["chat", "on", "--bad"]),
        Form::Usage { json: false, .. }
    ));
    assert_eq!(
        parse(&["chat", "off", "w"]),
        ask(Verb::ChatOff, false, "w", "", "")
    );
    assert_eq!(parse(&["chat"]), Form::Page("wiki/chat", Exit::Usage));
    assert_eq!(parse(&["publish", "--label", "x"]), Form::Publish);
    assert_eq!(
        parse(&["unpublish", "--label", "l"]),
        ask(Verb::Unpublish, false, "", "", "l")
    );
    assert_eq!(
        parse(&["note", "", "Home.md", "w"]),
        ask(Verb::Note("Home.md".into()), false, "w", "", "")
    );
    assert_eq!(
        parse(&["note", "a", "--workspace", "w", "b"]),
        Form::Usage {
            json: false,
            hint: "unexpected argument 'b'".into()
        }
    );
    assert_eq!(
        parse(&["note", "--json"]),
        Form::Usage {
            json: true,
            hint: k("usage: k2 wiki note <id> [--json] [project]")
        }
    );
    assert_eq!(
        parse(&["note", "a", "--"]),
        Form::Usage {
            json: false,
            hint: k("unknown flag '--' \u{2014} usage: k2 wiki note <id> [--json] [project]")
        }
    );
}

/// The text as the brand prints it: the command and the product's names.
fn k(s: &str) -> String {
    s.replace("k2 ", &format!("{} ", cli_name!())).replace(
        "the K2 runtime",
        concat!("the ", k2_client::product_name!(), " runtime"),
    )
}

fn out(s: &str) -> Shown {
    Shown::out(k(s))
}

fn err(s: &str) -> Shown {
    Shown::err(k(s), Exit::Error)
}

#[test]
fn the_status_reader() {
    let f = |raw: &str, idx: &str, json: bool| status(raw.as_bytes(), idx.as_bytes(), "/p", json);
    assert_eq!(
        f("", "", false),
        err("Error: no response from daemon (wiki routes missing?)\n")
    );
    assert_eq!(
        f(r#"{"error":{"message":"m"}}"#, "", false),
        err("Error: m\n")
    );
    assert_eq!(
        f(r#"{"error":"x"}"#, "", true),
        Shown {
            stdout: "{\"error\":{\"code\":\"daemon\",\"hint\":\"x\"}}\n".into(),
            stderr: String::new(),
            exit: Exit::Error
        }
    );
    assert_eq!(
        f(
            r#"{"enabled":true,"port":7,"publishedLabel":"lab","extra":1}"#,
            r#"{"nodes":[{"id":"a"},{"exists":false}],"generatedAt":"g"}"#,
            false
        ),
        out("Wiki  /p/.k2/wiki\n  Notes:      1\n  Serving:    yes  http://127.0.0.1:7\n  Public chat: no  (unavailable \u{2014} coming back with the K2 runtime)\n  Published:  lab  (nested subdomain label)\n  Indexed:    g\n")
    );
    assert_eq!(
        f("nope", r#"{"error":1}"#, true),
        out("{\"workspacePath\":\"/p\",\"wikiRel\":\".k2/wiki\",\"enabled\":false,\"serving\":false,\"port\":null,\"url\":null,\"publishedLabel\":null,\"noteCount\":null,\"generatedAt\":null}\n")
    );
    assert_eq!(
        f(r#"{"label":"","generatedAt":0,"port":0}"#, "", true),
        out("{\"workspacePath\":\"/p\",\"wikiRel\":\".k2/wiki\",\"enabled\":false,\"serving\":false,\"port\":0,\"url\":null,\"publishedLabel\":\"\",\"noteCount\":null,\"generatedAt\":0,\"label\":\"\"}\n")
    );
    let r = f("{}", r#"{"nodes":[1]}"#, false);
    assert!(r
        .stderr
        .ends_with("line 32, in <genexpr>\nAttributeError: 'int' object has no attribute 'get'\n"));
    let r = f("[]", "{}", false);
    assert!(r
        .stderr
        .ends_with("line 35, in <module>\nAttributeError: 'list' object has no attribute 'get'\n"));
}

#[test]
fn the_index_readers() {
    assert_eq!(
        as_json(b"\n"),
        Shown::err(
            "{\"error\": {\"code\": \"no_response\", \"hint\": \"no response from daemon\"}}\n",
            Exit::Error
        )
    );
    assert_eq!(
        as_json(b"x\n"),
        Shown::err(
            "{\"error\": {\"code\": \"bad_json\", \"hint\": \"x\\n\"}}\n",
            Exit::Error
        )
    );
    assert_eq!(
        as_json(b"{\"a\": \"\xc3\xa9\"}\n"),
        out("{\"a\":\"\\u00e9\"}\n")
    );
    assert_eq!(
        index(b"x"),
        err("Error: Expecting value: line 1 column 1 (char 0)\n")
    );
    assert_eq!(
        index(br#"{"nodes":[],"links":[1]}"#),
        out("Wiki index  \n  Generated:  \u{2014}\n  Notes:      0  (+ 0 missing stubs)\n  Links:      1\n\n  Empty vault. Seed with: k2 wiki seed\n  Or drop Markdown under .k2/wiki/\n")
    );
    let r = index(br#"{"nodes":[{"id":"b","title":"B"},{"id":"a"},{"id":5,"title":"c"},{"id":"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"}]}"#);
    assert_eq!(r.stdout, format!("Wiki index  \n  Generated:  \u{2014}\n  Notes:      4  (+ 0 missing stubs)\n  Links:      0\n\n  {:28}  TITLE\n  {:28}  \n  {:28}  B\n", "ID", "a", "b"));
    assert!(r
        .stderr
        .ends_with("line 29, in <module>\nTypeError: 'int' object is not subscriptable\n"));
    let r = index(br#"{"nodes":[{"title":3}]}"#);
    assert!(r.stderr.ends_with(
        "line 28, in <lambda>\nAttributeError: 'int' object has no attribute 'lower'\n"
    ));
    let r = index(br#"{"nodes":5}"#);
    assert!(r
        .stderr
        .ends_with("line 15, in <module>\nTypeError: 'int' object is not iterable\n"));
    let many: Vec<String> = (0..60).map(|i| format!(r#"{{"id":"n{i:02}"}}"#)).collect();
    let r = index(format!(r#"{{"nodes":[{}]}}"#, many.join(",")).as_bytes());
    assert!(r.stdout.ends_with(&format!(
        "  {:28}  \n  \u{2026} 10 more (use --json)\n",
        "n49"
    )));
}

#[test]
fn the_other_readers() {
    assert_eq!(
        note(br#"{"id":"Home.md","tags":["a","b"],"body":null,"markdown":"x\n"}"#),
        out("# Home.md\ntags: a, b\n\nx\n")
    );
    assert!(note(br#"{"body":false}"#)
        .stderr
        .ends_with("AttributeError: 'bool' object has no attribute 'endswith'\n"));
    assert_eq!(
        seed(br#"{"created":"Home.md"}"#),
        out("Seeded wiki:\n  + Home.md\n  Vault: .k2/wiki\n  View:  k2 wiki index   or open View Wiki in the app\n")
    );
    assert_eq!(
        serve(br#"{"serving":false,"publishedLabel":"l"}"#, true),
        out("Wiki serve OFF\n  Note: nested subdomain may still point here \u{2014} run k2 wiki unpublish or\n  k2 publish subdomain rm l\n")
    );
    assert_eq!(
        serve(b"{}", true).stdout.lines().next(),
        Some("Wiki serve ON  port ?")
    );
    assert!(serve(b"null", true).stderr.ends_with(
        "line 14, in <module>\nTypeError: argument of type 'NoneType' is not iterable\n"
    ));
    assert_eq!(
        chat_off(br#"{"publicChatEnabled":1}"#),
        err("Error: the daemon still reports public chat on (older daemon?) \u{2014} update it\n")
    );
    assert_eq!(published_label(br#"{"label":5}"#), "5");
    assert_eq!(published_label(b"[1]"), "");
    assert_eq!(resolved(br#"{"projectPath":"/x\n"}"#), "/x");
    assert_eq!(resolved(b"\"s\""), "");
    assert_eq!(
        serve_off(br#"{"error":{"message":"m"}}"#),
        Shown::err("Warning: serve off failed: {'message': 'm'}\n", Exit::Ok)
    );
    assert_eq!(
        unpublished_json(""),
        k("{\"ok\":true,\"serving\":false,\"publishedLabel\":null,\"hint\":\"k2 publish subdomain list\"}\n")
    );
}
