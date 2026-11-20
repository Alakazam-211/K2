use super::*;
use k2_client::brand::CLI_NAME;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/permission.sh");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    // The shell's command name, as this build names it.
    text.replace("k2 ", &format!("{CLI_NAME} "))
}

/// `_ORG_PY`'s lines as `python3 -c` numbers them (line 1 is the rest of
/// the line after the opening quote).
fn python() -> Vec<String> {
    let text = source();
    let body = text.split_once("\n_ORG_PY='").expect("_ORG_PY").1;
    let end = body.find("\n'\n").expect("the end of _ORG_PY");
    body[..end].split('\n').map(str::to_string).collect()
}

/// `cmd_organization`'s body.
fn command() -> String {
    let text = source();
    let start = text
        .find("\ncmd_organization() {\n")
        .expect("cmd_organization");
    let body = &text[start + 1..];
    body[..body.find("\n}\n").expect("its end") + 2].to_string()
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| (*w).to_string()).collect()
}

#[test]
fn the_python_lines_a_traceback_names() {
    let py = python();
    let at = |n: u32| py[(n - 1) as usize].trim().to_string();
    for (n, text) in [
        (line::WRITE_TEXT, r#"w(v.get("text", ""))"#),
        (line::LIST_ORGS, "for o in orgs:"),
        (line::LIST_LINES, "org_lines(o)"),
        (line::SHOW_LINES, "org_lines(o)"),
        (line::SHOW_FOR, r#"for l in o.get("lines") or []:"#),
        (
            line::SHOW_KIND,
            r#"word = "above" if l.get("kind") == "below" else "beside""#,
        ),
        (
            line::SHOW_LINE,
            r#"w("  Line: %s %s %s\n" % (l["a"]["name"], word, l["b"]["name"]))"#,
        ),
        (
            line::MINE_NEXT,
            r#"w(v.get("next", "You are in no org.") + "\n")"#,
        ),
        (
            line::MINE_HEAD,
            r#"w("You are %s head.\n" % (o["name"] + chr(39) + "s"))"#,
        ),
        (
            line::MINE_NO_HEAD,
            r#"w("You are in %s. It has no head yet.\n" % o["name"])"#,
        ),
        (line::MINE_LINES, "org_lines(o)"),
        (line::REACH, r#"w((v.get("text") or "") + "\n")"#),
        (line::NIGHT_MACHINE, r#"mc = g.get("machine") or {}"#),
        (line::NIGHT_FREE, r#"free = mc.get("availableBytes")"#),
        (
            line::NIGHT_COUNTS,
            r#"c.get("queued", 0), c.get("running", 0), c.get("done", 0), c.get("skipped", 0),"#,
        ),
        (line::NIGHT_ORGS, r#"for o in v.get("orgs") or []:"#),
        (line::NIGHT_STATE, r#"state = o.get("state")"#),
        (line::UPDATES_FOR, "for u in us:"),
        (
            line::UPDATES_ROW,
            r#"w("%s: %s\n" % (u["from"]["handle"], u["body"]))"#,
        ),
        (line::ASK_FOR, r#"for n in v.get("notDelivered") or []:"#),
        (
            line::UPDATE_HEAD,
            r#"h = (v.get("head") or {}).get("handle", "the head")"#,
        ),
        (line::SET_LEN, r#"n = len(v.get("lines") or [])"#),
        (
            line::LINK_KIND,
            r#"word = "above" if l.get("kind") == "below" else "beside""#,
        ),
        (
            line::DELETE,
            r#"w("Deleted %s; its members left (the agents stay).\n" % v["deleted"]["name"])"#,
        ),
        (line::CHANGED_LINES, r#"org_lines(v.get("org") or {})"#),
        (line::ORG_LINES, r#"org_lines(v.get("org") or {})"#),
        (line::OL_HEAD, r#"head = o.get("head")"#),
        (line::OL_MEMBERS, r#"ms = o.get("members") or []"#),
        (line::OL_FOR_ROLE, "for m in ms:"),
        (line::OL_ROLE, r#"if m.get("role"):"#),
        (
            line::OL_ROLE_ROW,
            r#"w("    %s: %s\n" % (m["handle"], m["role"]))"#,
        ),
        (line::OL_BULLETIN, r#"bl = o.get("bulletin")"#),
        (line::OL_LINES, r#"if bl and bl.get("lines"):"#),
        (line::OL_FOR_LINES, r#"for l in bl["lines"]:"#),
    ] {
        assert_eq!(at(n), text, "line {n} of _ORG_PY moved");
    }
    for (n, starts) in [
        (line::MINE_CONNECTED, "w(\"You are in %s. Head: %s (connected:"),
        (line::MINE_NOT_CONNECTED, "w(\"You are in %s. Head: %s (not connected:"),
        (line::NIGHT_FREE_GIB, "\", %.1f GiB free\" % (free / 1073741824.0)"),
        (line::NIGHT_LOAD, "\", load %.1f\" % load"),
        (line::NIGHT_ROW, "w(\"  %s  %s · head: %s · %s\\n\" % (o.get(\"due\"), o[\"org\"][\"name\"]"),
        (line::UPDATES_NONE, "w(\"No updates for %s since last night.\\n\""),
        (line::ASK_ASKED, "w(\"Asked %s now.\\n\" % (\", \".join(v.get(\"asked\") or [])"),
        (line::ASK_QUEUED, "w(\"Queued: %s ("),
        (line::ASK_ROW, "w(\"Not delivered to %s (%s); the ask is stored.\\n\""),
        (line::SET_ORG, "w(\"%s bulletin now has %d line%s; every agent sees it at once.\\n\" % (v[\"org\"][\"name\"]"),
        (line::LINK_ROW, "w(\"Linked: %s %s %s.\\n\""),
        (line::OL_PURPOSE, "if o.get(\"purpose\"):"),
        (line::OL_PURPOSE_ROW, "w(\"  Purpose: %s\\n\" % o[\"purpose\"])"),
        (line::OL_JOIN, "w(\"  Members: %s\\n\" % \", \".join("),
    ] {
        assert!(at(n).starts_with(starts), "line {n} of _ORG_PY moved: {:?}", at(n));
    }
    assert_eq!(py[0], "", "line 1 is the empty rest of the opening line");
    assert_eq!(py.len(), 156, "_ORG_PY changed length");
}

/// The texts the binary prints, as the shell has them.
#[test]
fn the_shell_s_texts() {
    let body = command();
    let all = source();
    let no_answer = NO_ANSWER.replace(k2_client::brand::PRODUCT_NAME, "K2");
    assert!(all.contains(&format!("echo \"{no_answer}\" >&2")));
    assert!(
        all.contains("    echo \"Error: $1\" >&2\n    echo \"See: ")
            && all.contains(&format!("echo \"{SEE_HELP}\" >&2"))
    );
    assert!(body.contains("echo \"Unknown organization command: $verb\" >&2\n            cmd_help_organization >&2\n            exit 2"));
    // Each usage line the plan can stop with is the shell's.
    let f = |list: &[&str]| Flags {
        args: words(list),
        ..Flags::default()
    };
    let none: FileText = None;
    for (verb, flags) in [
        ("bulletin", f(&["show"])),
        ("bulletin", f(&["set"])),
        ("bulletin", f(&["nope"])),
        ("show", f(&[])),
        ("reach", f(&[])),
        ("ask", f(&[])),
        ("update", f(&[])),
        ("create", f(&[])),
        ("rename", f(&["a"])),
        ("purpose", f(&[])),
        ("delete", f(&[])),
        ("add", f(&["a"])),
        ("move", f(&["a"])),
        ("remove", f(&["a"])),
        ("head", f(&["a"])),
        ("link", f(&["a"])),
        ("link", f(&["a", "over", "b"])),
        ("unlink", f(&["a"])),
        ("position", f(&[])),
        ("arrange", f(&[])),
    ] {
        let Err(Stop::Usage(why)) = plan(verb, &flags, &none) else {
            panic!("{verb} {flags:?} is no usage stop");
        };
        let shell = format!("_org_usage_err \"{}\"", why.replace('"', "\\\""));
        assert!(body.contains(&shell), "{verb}: the shell has no {shell:?}");
    }
    let Err(Stop::Usage(why)) = plan("reach", &f(&["acme"]), &none) else {
        panic!("reach with no --why");
    };
    assert_eq!(
        why,
        format!("say why in one line: {CLI_NAME} organization reach acme --why \"<one line>\"")
    );
    assert!(body.contains(&format!(
        "_org_usage_err \"say why in one line: {CLI_NAME} organization reach ${{args[0]}} --why \\\"<one line>\\\"\""
    )));
    let file = Flags {
        file: "x.txt".into(),
        args: words(&["set"]),
        ..Flags::default()
    };
    assert_eq!(
        plan("bulletin", &file, &Some(None)),
        Err(Stop::Usage("no file 'x.txt'".into()))
    );
    assert!(body.contains("_org_usage_err \"no file '$file'\""));
}

#[test]
fn the_flags() {
    assert_eq!(flags(&[]), Ok(Flags::default()));
    let f = flags(&[
        "a",
        "--json",
        "-x",
        "--peek",
        "--why",
        "--org",
        "--org=o",
        "--member=",
        "b",
    ])
    .unwrap();
    assert_eq!(f.args, words(&["a", "-x", "b"]));
    assert!(f.json && f.peek);
    assert_eq!(
        (f.why.as_str(), f.org.as_str(), f.member.as_str()),
        ("--org", "o", "")
    );
    let f = flags(&[
        "--file=a=b",
        "--purpose",
        "",
        "--since",
        "5",
        "--",
        "--json",
        "--x",
    ])
    .unwrap();
    assert_eq!(
        (f.file.as_str(), f.purpose.as_str(), f.since.as_str()),
        ("a=b", "", "5")
    );
    assert_eq!(f.args, words(&["--json", "--x"]));
    assert!(!f.json);
    for (list, why) in [
        (&["--file"][..], "--file needs a path"),
        (&["--why"], "--why needs one line"),
        (&["--purpose"], "--purpose needs text"),
        (&["--member"], "--member needs an agent"),
        (&["--org"], "--org needs an org"),
        (&["--since"], "--since needs a unix time"),
        (&["--since=5"], "unknown flag '--since=5'"),
        (&["--bogus", "--why"], "unknown flag '--bogus'"),
        (&["x", "--"], ""),
    ] {
        if why.is_empty() {
            assert!(flags(list).is_ok(), "{list:?}");
        } else {
            assert_eq!(flags(list), Err(why.to_string()), "{list:?}");
        }
    }
    // Each stop is one the shell has.
    let body = command();
    for why in [
        "--file needs a path",
        "--why needs one line",
        "--purpose needs text",
        "--member needs an agent",
        "--org needs an org",
        "--since needs a unix time",
    ] {
        assert!(body.contains(&format!("_org_usage_err \"{why}\"")), "{why}");
    }
    assert!(body.contains(r#"--*) _org_usage_err "unknown flag '$a'" ;;"#));
}

#[test]
fn the_json_bodies() {
    assert_eq!(org_json(&[]), "{}");
    assert_eq!(
        org_json(&[("org", "a b"), ("why", "é \"x\"\n\u{7f}")]),
        r#"{"org": "a b", "why": "\u00e9 \"x\"\n\u007f"}"#
    );
    assert_eq!(
        org_json(&[("lines[]", "one"), ("lines[]", ""), ("org", "o")]),
        r#"{"lines": ["one", ""], "org": "o"}"#
    );
    assert_eq!(org_json(&[("a", "1"), ("a", "2")]), r#"{"a": "2"}"#);
    assert_eq!(org_json(&[("x", "😀")]), r#"{"x": "\ud83d\ude00"}"#);
}

fn fl(list: &[&str]) -> Flags {
    flags(list).unwrap()
}

fn get_of(p: &Plan) -> (&str, bool, &str, &str, bool) {
    (p.verb, p.post, p.path.as_str(), p.payload.as_str(), p.json)
}

#[test]
fn the_requests() {
    let none: FileText = None;
    let run = |verb: &str, list: &[&str]| plan(verb, &fl(list), &none).unwrap();
    let p = run("bulletin", &["--peek"]);
    assert_eq!(
        get_of(&p),
        (
            "bulletin",
            false,
            "/cli/organization/bulletin",
            "peek=1",
            false
        )
    );
    let p = run("bulletin", &[]);
    assert_eq!(
        get_of(&p),
        ("bulletin", false, "/cli/organization/bulletin", "", false)
    );
    let p = run("bulletin", &["read", "x"]);
    assert_eq!(
        get_of(&p),
        (
            "bulletin-read",
            true,
            "/cli/organization/bulletin/read",
            "{}",
            false
        )
    );
    let p = run("bulletin", &["show", "", "--org", "a&b"]);
    assert_eq!(
        get_of(&p),
        ("show", false, "/cli/organization/show", "org=a%26b", false)
    );
    let p = run("bulletin", &["show", "x y", "--org", "o"]);
    assert_eq!(p.payload, "org=x%20y");
    let p = run("bulletin", &["set", "one", "two", "--org", "o"]);
    assert_eq!(
        get_of(&p),
        (
            "bulletin-set",
            true,
            "/cli/organization/bulletin/set",
            r#"{"lines": ["one", "two"], "org": "o"}"#,
            false
        )
    );
    let file = Flags {
        file: "f".into(),
        args: words(&["set", "three"]),
        ..Flags::default()
    };
    let p = plan("bulletin", &file, &Some(Some("a\nb".into()))).unwrap();
    assert_eq!(p.payload, r#"{"lines": ["a\nb", "three"]}"#);
    assert_eq!(
        get_of(&run("status", &["x"])),
        (
            "status",
            false,
            "/cli/organization/bulletin/status",
            "",
            false
        )
    );
    assert!(run("list", &["--json"]).json);
    assert_eq!(
        get_of(&run("show", &["é", "x"])),
        ("show", false, "/cli/organization/show", "org=%C3%A9", false)
    );
    assert_eq!(run("mine", &[]).path, "/cli/organization/mine");
    assert_eq!(run("night", &[]).path, "/cli/organization/night");
    assert_eq!(
        get_of(&run("chart", &[])),
        ("chart", false, "/cli/organization/chart", "", true)
    );
    assert_eq!(
        run("reach", &["acme", "extra", "--why", "need it"]).payload,
        r#"{"org": "acme", "why": "need it"}"#
    );
    assert_eq!(run("updates", &[]).payload, "");
    assert_eq!(run("updates", &["--since", "1 2"]).payload, "since=1%202");
    assert_eq!(
        run("updates", &["--org", "o", "--since", "5"]).payload,
        "org=o&since=5"
    );
    assert_eq!(
        run("ask", &["how", "goes", "--member", "m"]).payload,
        r#"{"question": "how goes", "member": "m"}"#
    );
    assert_eq!(
        run("update", &["news", "here", "--org", "o"]).payload,
        r#"{"text": "news here", "org": "o"}"#
    );
    assert_eq!(
        run("create", &["Big", "Team"]).payload,
        r#"{"name": "Big Team", "purpose": ""}"#
    );
    assert_eq!(
        run("rename", &["o", "New", "Name"]).payload,
        r#"{"org": "o", "name": "New Name"}"#
    );
    assert_eq!(
        run("purpose", &["o", "a", "b"]).payload,
        r#"{"org": "o", "purpose": "a b"}"#
    );
    assert_eq!(
        run("purpose", &["o", "a", "--purpose", "p"]).payload,
        r#"{"org": "o", "purpose": "p"}"#
    );
    assert_eq!(
        run("purpose", &["o"]).payload,
        r#"{"org": "o", "purpose": ""}"#
    );
    assert_eq!(run("delete", &["o"]).path, "/cli/organization/delete");
    assert_eq!(
        get_of(&run("add", &["o", "a"])),
        (
            "add",
            true,
            "/cli/organization/member/add",
            r#"{"org": "o", "agent": "a"}"#,
            false
        )
    );
    assert_eq!(
        run("move", &["a", "o"]).payload,
        r#"{"agent": "a", "org": "o"}"#
    );
    assert_eq!(
        run("remove", &["o", "a"]).path,
        "/cli/organization/member/remove"
    );
    assert_eq!(run("head", &["o", "none"]).path, "/cli/organization/head");
    for (word, kind) in [("above", "below"), ("below", "under"), ("beside", "beside")] {
        assert_eq!(
            run("link", &["a", word, "b"]).payload,
            format!(r#"{{"a": "a", "kind": "{kind}", "b": "b"}}"#)
        );
    }
    assert_eq!(
        get_of(&run("unlink", &["a", "b"])),
        (
            "unlink",
            true,
            "/cli/organization/line/remove",
            r#"{"a": "a", "b": "b"}"#,
            false
        )
    );
    assert_eq!(
        plan("--json", &fl(&[]), &none),
        Err(Stop::Unknown("--json".into()))
    );
    assert_eq!(
        plan("nope", &fl(&[]), &none),
        Err(Stop::Unknown("nope".into()))
    );
    // The shell's paths.
    let body = command();
    for path in [
        "\"$P/bulletin\" \"peek=1\"",
        "_org_call bulletin-read POST \"$P/bulletin/read\" \"{}\"",
        "_org_call bulletin-set POST \"$P/bulletin/set\"",
        "_org_call status GET \"$P/bulletin/status\" \"\"",
        "_org_call chart GET \"$P/chart\" \"\"",
        "_org_call link POST \"$P/line/add\"",
        "_org_call unlink POST \"$P/line/remove\"",
        "_org_call move POST \"$P/member/move\" \"$(_org_json \"agent=${args[0]}\" \"org=${args[1]}\")\"",
    ] {
        assert!(body.contains(path), "{path}");
    }
}

fn shown(verb: &str, status: u16, body: &str, json: bool) -> Shown {
    render(verb, status, body.as_bytes(), json)
}

fn out(text: &str) -> Shown {
    Shown::out(text)
}

const ORG: &str = r#"{"name":"Ops","head":{"handle":"ann"},"purpose":"Keep it up","members":[{"handle":"ann","isHead":true,"role":"lead"},{"handle":"bob"}],"bulletin":{"lines":["one","two"]}}"#;
const ORG_TEXT: &str = "Ops · head: ann\n  Purpose: Keep it up\n  Members: ann (head), bob\n    ann: lead\n  Bulletin:\n    one\n    two\n";

#[test]
fn the_answers_as_text() {
    assert_eq!(
        shown("bulletin", 200, r#"{"text":"Hi\nthere"}"#, false),
        out("Hi\nthere")
    );
    assert_eq!(shown("bulletin", 200, r#"{}"#, false), out(""));
    assert_eq!(
        shown("bulletin-read", 200, r#"{"ok":true}"#, false),
        out("")
    );
    assert_eq!(
        shown(
            "status",
            200,
            r#"{"day":"2026-11-18","unread":true}"#,
            false
        ),
        out(&format!(
            "2026-11-18: unread (read it: {CLI_NAME} organization bulletin)\n"
        ))
    );
    assert_eq!(shown("status", 200, r#"{}"#, false), out("None: read\n"));
    assert_eq!(
        shown("list", 200, r#"{"orgs":[]}"#, false),
        out(&format!(
            "No orgs yet. An Owner or Admin makes one: {CLI_NAME} organization create <name>\n"
        ))
    );
    assert_eq!(
        shown(
            "list",
            200,
            &format!(r#"{{"orgs":[{ORG},{{"name":"B"}}]}}"#),
            false
        ),
        out(&format!(
            "{ORG_TEXT}B · head: none yet\n  Members: none yet\n  Bulletin: none yet\n"
        ))
    );
    assert_eq!(
        shown(
            "show",
            200,
            r#"{"org":{"name":"A","head":{},"lines":[{"kind":"below","a":{"name":"A"},"b":{"name":"B"}},{"kind":"beside","a":{"name":"A"},"b":{"name":"C"}}]}}"#,
            false
        ),
        out("A · head: none yet\n  Members: none yet\n  Bulletin: none yet\n  Line: A above B\n  Line: A beside C\n")
    );
    assert_eq!(
        shown("show", 200, r#"{"org":{"name":"A","head":{"x":1}}}"#, false).stdout,
        "A · head: None\n  Members: none yet\n  Bulletin: none yet\n"
    );
    assert_eq!(
        shown("mine", 200, r#"{}"#, false),
        out("You are in no org.\n")
    );
    assert_eq!(
        shown("mine", 200, r#"{"next":"Ask."}"#, false),
        out("Ask.\n")
    );
    assert_eq!(
        shown("mine", 200, r#"{"org":{"name":"A"},"isHead":true}"#, false).stdout,
        "You are A's head.\nA · head: none yet\n  Members: none yet\n  Bulletin: none yet\n"
    );
    assert!(shown("mine", 200, r#"{"org":{"name":"A"}}"#, false)
        .stdout
        .starts_with("You are in A. It has no head yet.\n"));
    assert!(shown(
        "mine",
        200,
        r#"{"org":{"name":"A"},"head":{"handle":"h"},"connectedToHead":1}"#,
        false
    )
    .stdout
    .starts_with(&format!(
        "You are in A. Head: h (connected: {CLI_NAME} msg h \"...\").\n"
    )));
    assert!(shown(
        "mine",
        200,
        r#"{"org":{"name":"A"},"head":{"handle":"h"}}"#,
        false
    )
    .stdout
    .starts_with(&format!(
        "You are in A. Head: h (not connected: {CLI_NAME} organization reach A --why \"...\").\n"
    )));
    assert_eq!(
        shown("reach", 200, r#"{"text":"Sent.","status":"sent"}"#, false),
        out("Sent.\n")
    );
    let held = shown("reach", 200, r#"{"text":"A card.","status":"card"}"#, false);
    assert_eq!(
        (held.stdout.as_str(), held.exit),
        ("A card.\n", Exit::Usage)
    );
    assert_eq!(
        shown("reach", 200, r#"{"status":"open"}"#, true).exit,
        Exit::Usage
    );
    assert_eq!(shown("reach", 200, r#"{"text":null}"#, false), out("\n"));
    assert_eq!(
        shown(
            "updates",
            200,
            r#"{"updates":[],"org":{"name":"Ops"}}"#,
            false
        ),
        out("No updates for Ops since last night.\n")
    );
    assert_eq!(
        shown("updates", 200, r#"{}"#, false),
        out("No updates for your org since last night.\n")
    );
    assert_eq!(
        shown(
            "updates",
            200,
            r#"{"updates":[{"from":{"handle":"a"},"body":"b"}]}"#,
            false
        ),
        out("a: b\n")
    );
    assert_eq!(
        shown("ask", 200, r#"{"asked":["a","b"],"queued":["c"],"notDelivered":[{"member":"d","why":"off"}]}"#, false),
        out(&format!(
            "Asked a, b now.\nQueued: c ({} asks one at a time).\nTheir answers: {CLI_NAME} organization updates\nNot delivered to d (off); the ask is stored.\n",
            k2_client::brand::PRODUCT_NAME
        ))
    );
    assert!(shown("ask", 200, r#"{}"#, false)
        .stdout
        .starts_with("Asked nobody now.\n"));
    assert_eq!(
        shown(
            "update",
            200,
            r#"{"deliveredNow":true,"head":{"handle":"h"}}"#,
            false
        ),
        out("Sent to h.\n")
    );
    assert_eq!(
        shown("update", 200, r#"{}"#, false),
        out(&format!(
            "Stored for the head (heads read updates at night: {CLI_NAME} organization updates).\n"
        ))
    );
    assert_eq!(
        shown(
            "bulletin-set",
            200,
            r#"{"org":{"name":"Ops"},"lines":["x"]}"#,
            false
        ),
        out("Ops's bulletin now has 1 line; every agent sees it at once.\n")
    );
    assert_eq!(
        shown("bulletin-set", 200, r#"{"org":{"name":"Ops"}}"#, false),
        out("Ops's bulletin now has 0 lines; every agent sees it at once.\n")
    );
    assert_eq!(
        shown(
            "link",
            200,
            r#"{"line":{"kind":"below","a":{"name":"A"},"b":{"name":"B"}}}"#,
            false
        ),
        out("Linked: A above B.\n")
    );
    assert_eq!(
        shown("link", 200, r#"{}"#, false),
        out("Linked: None beside None.\n")
    );
    assert_eq!(shown("unlink", 200, r#"{}"#, false), out("Unlinked.\n"));
    assert_eq!(
        shown("delete", 200, r#"{"deleted":{"name":"Ops"}}"#, false),
        out("Deleted Ops; its members left (the agents stay).\n")
    );
    assert_eq!(
        shown("add", 200, r#"{"changed":false,"org":{"name":"A"}}"#, false).stdout,
        "No change.\nA · head: none yet\n  Members: none yet\n  Bulletin: none yet\n"
    );
    assert_eq!(
        shown("add", 200, r#"{"changed":0,"org":{"name":"A"}}"#, false).stdout,
        "A · head: none yet\n  Members: none yet\n  Bulletin: none yet\n"
    );
    for verb in ["create", "rename", "purpose", "move", "head"] {
        assert_eq!(
            shown(verb, 200, &format!(r#"{{"org":{ORG}}}"#), false),
            out(ORG_TEXT)
        );
    }
}

#[test]
fn the_night() {
    let v = r#"{"tonight":true,"night":"2026-11-18","zone":"UTC","runAt":"01:00","spreadEndsAt":"03:00","lastCallAt":"04:00","dayStartsAt":"06:00",
      "governor":{"slots":2,"holding":"low memory","machine":{"cores":8,"availableBytes":268435456,"load1":1.25}},
      "counts":{"queued":1,"running":0,"done":3,"skipped":1,"failed":2},
      "orgs":[{"due":"01:00","org":{"name":"A"},"head":{"handle":"h"},"state":"done","wrote":true},
              {"due":"01:10","org":{"name":"B"},"head":{"handle":"i"},"state":"skipped"},
              {"due":"01:20","org":{"name":"C"},"head":{"handle":"j"},"state":"queued"}]}"#;
    assert_eq!(
        shown("night", 200, v, false),
        out("Tonight (night of 2026-11-18, UTC): runs spread 01:00-03:00, last call 04:00, new day 06:00.\n\
This computer: 2 at a time (8 cores, 0.2 GiB free, load 1.2).\n\
Holding new starts: low memory.\n\
Queued 1 · running 0 · done 3 · skipped 1 · failed 2\n\
\x20 01:00  A · head: h · done (wrote its bulletin)\n\
\x20 01:10  B · head: i · skipped (the bulletin stays as it was)\n\
\x20 01:20  C · head: j · queued\n")
    );
    assert_eq!(
        shown(
            "night",
            200,
            r#"{"governor":{"machine":{"availableBytes":true,"load1":"x"}}}"#,
            false
        )
        .stdout,
        "Last night (night of None, None): runs spread None-None, last call None, new day None.\n\
This computer: None at a time (None cores, 0.0 GiB free).\n\
Queued 0 · running 0 · done 0 · skipped 0\n"
    );
    let big = shown(
        "night",
        200,
        &format!(
            r#"{{"governor":{{"machine":{{"availableBytes":1{}}}}}}}"#,
            "0".repeat(400)
        ),
        false,
    );
    assert_eq!(big.exit, Exit::Error);
    assert!(big.stderr.ends_with(&format!(
        "line {}, in <module>\nOverflowError: int too large to convert to float\n",
        line::NIGHT_FREE_GIB
    )));
}

#[test]
fn refusals_and_odd_answers() {
    assert_eq!(
        shown(
            "list",
            403,
            r#"{"error":{"code":"owner_only","hint":"Ask the owner."}}"#,
            false
        ),
        Shown::err("Error [owner_only]: Ask the owner.\n", Exit::Refused)
    );
    assert_eq!(
        shown("list", 401, r#"{"error":{"message":"m"}}"#, false),
        Shown::err("Error [error]: m\n", Exit::Refused)
    );
    assert_eq!(
        shown(
            "list",
            404,
            r#"{"error":"not_found","message":"No such org."}"#,
            false
        ),
        Shown::err("Error: No such org.\n", Exit::Usage)
    );
    assert_eq!(
        shown("list", 409, r#"{"error":"conflict"}"#, false),
        Shown::err("Error: conflict\n", Exit::Usage)
    );
    assert_eq!(
        shown("list", 500, r#"{"x":1}"#, false),
        Shown::err("Error: {\"x\":1}\n", Exit::Error)
    );
    assert_eq!(
        shown("list", 302, r#"{"error":{"code":null}}"#, false),
        Shown::err("Error [None]: \n", Exit::Error)
    );
    let product = k2_client::brand::PRODUCT_NAME;
    assert_eq!(
        shown("list", 200, "not json", false),
        Shown::err(
            format!("Error: unexpected answer from the {product} daemon (HTTP 200): not json\n"),
            Exit::Error
        )
    );
    assert_eq!(
        shown("list", 200, "", false),
        Shown::err(
            format!("Error: unexpected answer from the {product} daemon (HTTP 200): \n"),
            Exit::Error
        )
    );
    assert_eq!(
        shown("list", 403, "[1]", false),
        Shown::err(
            format!("Error: unexpected answer from the {product} daemon (HTTP 403): [1]\n"),
            Exit::Refused
        )
    );
    let long = "x".repeat(400);
    assert_eq!(
        shown("list", 500, &long, false).stderr,
        format!(
            "Error: unexpected answer from the {product} daemon (HTTP 500): {}\n",
            "x".repeat(300)
        )
    );
    // The bulletin's mark is no part of the answer.
    assert_eq!(
        shown(
            "list",
            200,
            r#"{"orgs":[],"bulletin":{"day":"d","unread":true,"key":"k"},"z":1}"#,
            true
        ),
        out("{\"orgs\": [], \"z\": 1}\n")
    );
    assert_eq!(
        shown("list", 200, r#"{"bulletin":{"day":"d"},"z":"é"}"#, true),
        out("{\"bulletin\": {\"day\": \"d\"}, \"z\": \"\\u00e9\"}\n")
    );
}

fn raises(verb: &str, body: &str, frames: &[(u32, &str)], exception: &str, stdout: &str) {
    let s = shown(verb, 200, body, false);
    let want = traceback(frames, exception);
    assert_eq!(
        (s.stdout.as_str(), s.stderr.as_str(), s.exit),
        (stdout, want.as_str(), Exit::Error),
        "{verb} {body}"
    );
}

use super::super::traceback;

#[test]
fn where_the_python_raises() {
    let m = |n: u32| vec![(n, "<module>")];
    raises(
        "bulletin",
        r#"{"text":5}"#,
        &m(line::WRITE_TEXT),
        "TypeError: write() argument must be str, not int",
        "",
    );
    raises(
        "bulletin",
        r#"{"text":null}"#,
        &m(line::WRITE_TEXT),
        "TypeError: write() argument must be str, not NoneType",
        "",
    );
    raises(
        "list",
        r#"{"orgs":3}"#,
        &m(line::LIST_ORGS),
        "TypeError: 'int' object is not iterable",
        "",
    );
    raises(
        "list",
        r#"{"orgs":["x"]}"#,
        &[(line::LIST_LINES, "<module>"), (line::OL_HEAD, "org_lines")],
        "AttributeError: 'str' object has no attribute 'get'",
        "",
    );
    raises(
        "list",
        r#"{"orgs":[{"name":"A","members":[{"isHead":true}]}]}"#,
        &[
            (line::LIST_LINES, "<module>"),
            (line::OL_JOIN, "org_lines"),
            (line::OL_JOIN, "<genexpr>"),
        ],
        "KeyError: 'handle'",
        "A · head: none yet\n",
    );
    raises(
        "list",
        r#"{"orgs":[{"name":"A","members":[{"handle":1}]}]}"#,
        &[(line::LIST_LINES, "<module>"), (line::OL_JOIN, "org_lines")],
        "TypeError: sequence item 0: expected str instance, int found",
        "A · head: none yet\n",
    );
    raises(
        "list",
        r#"{"orgs":[{"name":"A","bulletin":["x"]}]}"#,
        &[
            (line::LIST_LINES, "<module>"),
            (line::OL_LINES, "org_lines"),
        ],
        "AttributeError: 'list' object has no attribute 'get'",
        "A · head: none yet\n  Members: none yet\n",
    );
    raises(
        "show",
        r#"{"org":{"name":"A","lines":[{"a":{}}]}}"#,
        &m(line::SHOW_LINE),
        "KeyError: 'name'",
        "A · head: none yet\n  Members: none yet\n  Bulletin: none yet\n",
    );
    raises(
        "mine",
        r#"{"next":5}"#,
        &m(line::MINE_NEXT),
        "TypeError: unsupported operand type(s) for +: 'int' and 'str'",
        "",
    );
    raises(
        "mine",
        r#"{"next":[1]}"#,
        &m(line::MINE_NEXT),
        "TypeError: can only concatenate list (not \"str\") to list",
        "",
    );
    raises(
        "mine",
        r#"{"org":[1],"isHead":true}"#,
        &m(line::MINE_HEAD),
        "TypeError: list indices must be integers or slices, not str",
        "",
    );
    raises(
        "mine",
        r#"{"org":"o"}"#,
        &m(line::MINE_NO_HEAD),
        "TypeError: string indices must be integers, not 'str'",
        "",
    );
    raises(
        "mine",
        r#"{"org":{"x":1},"head":1}"#,
        &m(line::MINE_NOT_CONNECTED),
        "KeyError: 'name'",
        "",
    );
    raises(
        "reach",
        r#"{"text":{"a":1}}"#,
        &m(line::REACH),
        "TypeError: unsupported operand type(s) for +: 'dict' and 'str'",
        "",
    );
    raises(
        "night",
        r#"{"governor":[1]}"#,
        &m(line::NIGHT_MACHINE),
        "AttributeError: 'list' object has no attribute 'get'",
        "",
    );
    raises(
        "updates",
        r#"{"updates":[{"from":"x"}]}"#,
        &m(line::UPDATES_ROW),
        "TypeError: string indices must be integers, not 'str'",
        "",
    );
    raises(
        "ask",
        r#"{"asked":5}"#,
        &m(line::ASK_ASKED),
        "TypeError: can only join an iterable",
        "",
    );
    raises(
        "ask",
        r#"{"asked":[1]}"#,
        &m(line::ASK_ASKED),
        "TypeError: sequence item 0: expected str instance, int found",
        "",
    );
    raises(
        "update",
        r#"{"head":"h"}"#,
        &m(line::UPDATE_HEAD),
        "AttributeError: 'str' object has no attribute 'get'",
        "",
    );
    raises(
        "bulletin-set",
        r#"{"lines":4}"#,
        &m(line::SET_LEN),
        "TypeError: object of type 'int' has no len()",
        "",
    );
    raises(
        "bulletin-set",
        r#"{}"#,
        &m(line::SET_ORG),
        "KeyError: 'org'",
        "",
    );
    raises(
        "link",
        r#"{"line":{"a":null}}"#,
        &m(line::LINK_ROW),
        "AttributeError: 'NoneType' object has no attribute 'get'",
        "",
    );
    raises(
        "delete",
        r#"{"deleted":null}"#,
        &m(line::DELETE),
        "TypeError: 'NoneType' object is not subscriptable",
        "",
    );
}

#[test]
fn the_help_and_the_odd_forms_are_the_shell_s() {
    // The help is a page or the shell's: `run` hands it on before it
    // reads anything.
    for list in [
        &["organization"][..],
        &["organization", ""],
        &["organization", "help"],
        &["organization", "list", "-h"],
        &["organization", "list", "--help", "x"],
    ] {
        let args: Vec<OsString> = list.iter().map(OsString::from).collect();
        assert_eq!(run(&args), None, "{list:?}");
    }
    let body = command();
    assert!(body.contains("''|--help|-h|help) cmd_help_organization; exit 0 ;;"));
    assert!(body.contains("if _wants_help \"$@\"; then cmd_help_organization; exit 0; fi"));
}

#[test]
fn a_file_for_the_bulletin() {
    let dir = std::env::temp_dir().join(format!("k5-org-file-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = |file: &std::path::Path| Flags {
        file: file.to_string_lossy().into_owned(),
        args: words(&["set"]),
        ..Flags::default()
    };
    let good = dir.join("good.txt");
    std::fs::write(&good, "a\nb\n\n").unwrap();
    assert_eq!(
        file_text("bulletin", &f(&good)),
        Some(Some(Some("a\nb".into())))
    );
    assert_eq!(
        file_text("bulletin", &f(&dir)),
        Some(Some(None)),
        "a folder is no file"
    );
    assert_eq!(
        file_text("bulletin", &f(&dir.join("none"))),
        Some(Some(None))
    );
    let nul = dir.join("nul.txt");
    std::fs::write(&nul, b"a\0b").unwrap();
    assert_eq!(file_text("bulletin", &f(&nul)), None);
    let bad = dir.join("bad.txt");
    std::fs::write(&bad, b"a\xffb").unwrap();
    assert_eq!(file_text("bulletin", &f(&bad)), None);
    assert_eq!(file_text("list", &f(&nul)), Some(None));
    assert_eq!(
        file_text(
            "bulletin",
            &Flags {
                args: words(&["set"]),
                ..Flags::default()
            }
        ),
        Some(None)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
