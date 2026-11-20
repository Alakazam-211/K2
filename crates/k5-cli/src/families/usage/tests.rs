use super::*;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/usage.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The readers' Python blocks, in the file's order, each as `python3 -c`
/// numbers its lines (line 1 is the empty one after `-c '`).
fn readers() -> Vec<Vec<String>> {
    let text = source();
    let start = text.find("cmd_usage() {").expect("cmd_usage");
    let end = text.find("\ncmd_llm() {").expect("its end");
    let mut out = Vec::new();
    let mut rest = &text[start..end];
    while let Some(at) = rest.find("| python3 -c '") {
        let body = &rest[at + "| python3 -c '".len()..];
        let stop = body.find("\n'").expect("a reader's end");
        out.push(
            std::iter::once(String::new())
                .chain(body[..stop].split('\n').map(str::to_string))
                .collect(),
        );
        rest = &body[stop..];
    }
    out
}

fn ask(list: &[&str]) -> Ask {
    match parse(list) {
        Parsed::Call(a) => *a,
        other => panic!("{list:?}: {other:?}"),
    }
}

fn req(list: &[&str]) -> Result<Request, String> {
    check(&ask(list), || "/w/acme".into(), || "Europe/Oslo".into())
}

#[test]
fn the_shell_s_lines() {
    let text = source();
    for shell in [
        r#"        ""|help|--help|-h) _usage_cli_usage; return 0 ;;"#,
        r#"    if _wants_help "$@"; then"#,
        r#"            --workspace|--harness|--login|--model|--since|--until|--by|--tz|--limit|--before|--tool)"#,
        r#"                [ -n "${2:-}" ] || { echo "usage: $1 needs a value" >&2; exit 2; }"#,
        r#"            --*) echo "usage: unknown flag '$1' (k2 usage --help)" >&2; exit 2 ;;"#,
        r#"            *) echo "usage: unexpected argument '$1' (k2 usage --help)" >&2; exit 2 ;;"#,
        r#"        ws="${K2_PROJECT_PATH:-$PWD}""#,
        r#"    case "$limit" in ""|[1-9]|[1-9][0-9]|[1-9][0-9][0-9]) ;; *) echo "usage: --limit must be 1-200" >&2; exit 2 ;; esac"#,
        r#"    _uq() { [ -n "$2" ] && q="${q}${q:+&}$1=$(urlencode "$2")"; return 0; }"#,
        r#"            _uq since "$since"; _uq until "$until"; _uq group_by "$by"; _uq tz "$tz""#,
        r#"            _uq limit "$limit"; _uq before "$before""#,
        r#"                _sidecar_split "$(_sidecar_send POST "/cli/usage/limits/refresh" "$body")""#,
        r#"    [ "$_SC_STATUS" = "200" ] || _sidecar_fail "$_SC_STATUS" "$_SC_BODY""#,
        r#"            echo "usage: unknown usage verb '$sub' (k2 usage --help)" >&2"#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "cmd_usage no longer has {shell:?}"
        );
    }
    let r = readers();
    assert_eq!(r.len(), 3, "three readers");
    for (block, at, has) in [
        (0, line::LOAD, "r = json.load(sys.stdin)"),
        (0, line::T_N, "x = int(x or 0)"),
        (0, line::T_USD, r#"s = "$%.2f" % c.get("usd", 0)"#),
        (
            0,
            line::T_SCOPE,
            r#"scope = r.get("workspace") or "this server""#,
        ),
        (0, line::T_TOTAL, r#"print(line("Total", t))"#),
        (0, line::T_HARN, r#"{"label": h["harness"], **h}"#),
        (
            0,
            line::T_GROUP,
            r#"print(line(g.get("label") or g.get("key") or "-", g))"#,
        ),
        (0, line::T_DAY, r#"print(line(d["day"], d))"#),
        (1, line::U_ROWS, r#"rows = p.get("rows") or []"#),
        (1, line::U_WHEN, "time.localtime(ms / 1000)"),
        (1, line::U_WS, r#".rstrip("/").split("/")[-1] or "-""#),
        (1, line::U_NEXT, r#"if p.get("next_cursor"):"#),
        (
            2,
            line::PARSE,
            r#"fromisoformat((s or "").replace("Z", "+00:00"))"#,
        ),
        (2, line::AGO_M, "m = int((now - t).total_seconds() // 60)"),
        (2, line::UNTIL_M, "m = int((t - now).total_seconds() // 60)"),
        (2, line::L_TOOLS, r#"for t in d.get("tools") or []:"#),
        (
            2,
            line::L_LOGINS,
            r#"logins = [l for l in t.get("logins") or []"#,
        ),
        (2, line::L_ROUND, "round(pct)"),
        (2, line::L_LABEL, r#"(l.get("label") or "-")[:22]"#),
        (2, line::L_AGO, r#"ago(l.get("checkedAt"))"#),
    ] {
        assert!(
            r[block][at as usize].contains(has),
            "reader {block} line {at}: {:?}",
            r[block][at as usize]
        );
    }
}

#[test]
fn the_words_and_the_checks() {
    assert_eq!(parse(&[]), Parsed::Help);
    assert_eq!(parse(&["tokens", "--json", "-h"]), Parsed::Help);
    assert_eq!(
        parse(&["tokens", "--tz"]),
        Parsed::Usage("usage: --tz needs a value".into())
    );
    assert!(
        matches!(parse(&["tokens", "x"]), Parsed::Usage(l) if l.contains("unexpected argument 'x'"))
    );
    assert!(
        matches!(parse(&["tokens", "--x"]), Parsed::Usage(l) if l.contains("unknown flag '--x'"))
    );
    assert_eq!(
        req(&["tokens", "--here", "--by", "model", "--since", "a b"]),
        Ok(Request::Get(
            "tokens",
            "workspace=%2Fw%2Facme&since=a%20b&group_by=model&tz=Europe%2FOslo".into()
        ))
    );
    assert_eq!(
        req(&["tokens", "--outside", "--tz", "UTC"]),
        Ok(Request::Get("tokens", "tz=UTC&outside=1".into()))
    );
    assert_eq!(
        req(&["turns", "--limit", "200", "--login", "x"]),
        Ok(Request::Get("turns", "account=x&limit=200".into()))
    );
    assert_eq!(req(&["limits"]), Ok(Request::Get("limits", String::new())));
    assert_eq!(
        req(&["limits", "--refresh", "--tool", "codex", "--login", "é"]),
        Ok(Request::Refresh(
            r#"{"tool": "codex", "accountId": "\u00e9"}"#.into()
        ))
    );
    assert_eq!(
        req(&["limits", "--refresh"]),
        Ok(Request::Refresh("{}".into()))
    );
    for (words, has) in [
        (&["tokens", "--here", "--workspace", "w"][..], "pick one"),
        (
            &["tokens", "--workspace", "w", "--outside"],
            "cannot both be set",
        ),
        (&["tokens", "--harness", "x"], "--harness must be"),
        (&["tokens", "--tool", "x"], "--tool must be"),
        (&["tokens", "--by", "x"], "--by must be"),
        (&["turns", "--limit", "0"], "--limit must be 1-200"),
        (&["turns", "--limit", "1000"], "--limit must be 1-200"),
        (&["tokens", "--limit", "5"], "takes no --refresh"),
        (&["turns", "--tz", "UTC"], "usage turns [--limit N]"),
        (&["limits", "--since", "x"], "usage limits [--refresh]"),
        (&["nope"], "unknown usage verb 'nope'"),
    ] {
        match req(words) {
            Err(line) => assert!(line.contains(has), "{words:?}: {line}"),
            Ok(r) => panic!("{words:?}: {r:?}"),
        }
    }
}

#[test]
fn this_computer_s_zone() {
    assert_eq!(
        local_tz(Some(":America/Denver"), None, None),
        "America/Denver"
    );
    assert_eq!(
        local_tz(
            None,
            Some("/var/db/timezone/zoneinfo/Asia/Tokyo".into()),
            None
        ),
        "Asia/Tokyo"
    );
    assert_eq!(local_tz(None, Some("/etc/other".into()), None), "UTC");
    assert_eq!(
        local_tz(None, None, Some("Europe/Oslo\nx\n".into())),
        "Europe/Oslo"
    );
    assert_eq!(local_tz(None, None, Some("Bad Zone\n".into())), "UTC");
    assert_eq!(local_tz(Some(""), None, None), "UTC");
}

#[test]
fn iso_times() {
    let base = iso_time("2026-11-19T12:00:30+00:00").unwrap();
    assert_eq!(base, (1_795_089_630_000_000, true));
    assert_eq!(iso_time("2026-11-19T13:00:30+01:00").unwrap(), base);
    assert_eq!(
        iso_time("2026-11-19 12:00:30.5+00:00").unwrap().0,
        base.0 + 500_000
    );
    assert!(!iso_time("2026-11-19T12:00:30").unwrap().1);
    assert!(!iso_time("2026-11-19").unwrap().1);
    for bad in [
        "",
        "2026-13-01",
        "2026-02-30",
        "x",
        "2026-11-19T25:00",
        "2026-11-19T12:00:30+0",
    ] {
        assert_eq!(iso_time(bad), None, "{bad}");
    }
}

fn shown(list: &[&str], body: &str) -> Shown {
    render(&ask(list), body.as_bytes(), 1_795_089_630_000_000)
}

/// What the shell's Python printed for these bodies (Python 3.12; the
/// limits reader with its `now` set to the time above).
#[test]
fn the_readers() {
    let tokens = r#"{"workspace":"acme","tz":"America/Denver","total":{"requests":1234,"input_tokens":2500000000,"output_tokens":999,"cache_read_tokens":"1500","cache_write_tokens":true,"cost":{"usd":12.345,"unpriced_requests":3}},"harnesses":[{"harness":"claude","requests":5,"label":"Claude Code"},{"harness":"codex","input_tokens":1250}],"days":[]}"#;
    assert_eq!(
        shown(&["tokens"], tokens).stdout,
        concat!(
            "Usage on acme (days in America/Denver)\n",
            "  Total                                1.2k req  in    2.5B  out     999  cache r    1.5k  w       1  $12.35 (+3 unpriced)\n",
            "  Estimated API-equivalent cost: $12.35 \u{2014} an estimate, not your bill.\n",
            "By harness:\n",
            "  Claude Code                             5 req  in       0  out       0  cache r       0  w       0  \n",
            "  codex                                   0 req  in    1.2k  out       0  cache r       0  w       0  \n",
            "Input is uncached; output includes reasoning.\n",
        )
    );
    let limits = r#"{"refresh":{"retryAfterSecs":40,"outcome":"too_soon"},"tools":[
 {"tool":"claude","display":"Claude","supported":true,"logins":[
   {"label":"me@example.com and a long tail","isDefault":true,"accountId":"a1","state":"ok","source":"live-probe","checkedAt":"2026-11-19T11:30:00Z",
    "windows":[{"label":"5h","usedPercent":42.5,"resetsAt":"2026-11-19T13:30:00Z"},{"label":"week","used":0.125,"resetsAt":"2026-11-23T12:00:00+00:00"},{"label":"x","usedPercent":3,"resetsAt":"2026-11-19T11:00:00Z"}]},
   {"label":"b","state":"probe_failed","error":"boom","pinned":true,"checkedAt":"2026-11-19T07:00:00.5+00:00"}]},
 {"tool":"codex","display":"Codex","supported":false,"logins":[{"label":"c"}]},
 {"tool":"grok","display":"Grok","supported":true,"logins":[]}]}"#;
    assert_eq!(
        shown(&["limits"], limits).stdout,
        concat!(
            "Not re-read: read less than a minute ago (try in 40s).\n",
            "Claude:\n",
            "* me@example.com and a l 5h 42% (resets in 89m) \u{b7} week 12% (resets in 3d) \u{b7} x 3% (reset \u{2014} re-checking)\n",
            "    updated 30m ago \u{b7} live\n",
            "  b                      - [pinned]\n",
            "    updated 5h ago \u{b7} last check failed: boom\n",
            "Codex: no limits API\n",
            "Grok:\n",
            "  (not signed in)\n",
            "* = the server default login.\n",
        )
    );
    let r = shown(
        &["limits"],
        r#"{"tools":[{"supported":true,"logins":[{"checkedAt":"2026-11-19T11:00:00"}]}]}"#,
    );
    assert!(
        r.stderr
            .ends_with("TypeError: can't subtract offset-naive and offset-aware datetimes\n"),
        "{}",
        r.stderr
    );
    let r = shown(&["tokens"], r#"{"total":{"requests":"x1"}}"#);
    assert!(
        r.stderr
            .ends_with("ValueError: invalid literal for int() with base 10: 'x1'\n"),
        "{}",
        r.stderr
    );
    let r = shown(&["turns"], "[]");
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    assert_eq!(shown(&["turns"], "{}").stdout, "(no requests)\n");
}
