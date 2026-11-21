use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Each usage line is the shell's, in the default brand.
#[test]
fn the_usage_lines_are_the_shell_s() {
    let all = [
        "compute-command.sh",
        "compute-jobs.sh",
        "compute-provenance.sh",
    ]
    .iter()
    .map(|f| source(f))
    .collect::<Vec<_>>()
    .join("\n");
    for u in [
        INFO_USAGE,
        QUEUE_USAGE,
        JOBS_USAGE,
        TIER_USAGE,
        GET_USAGE,
        STATUS_USAGE,
        RECEIPT_USAGE,
        PROVENANCE_OUT_USAGE,
        VERIFY_FILE_USAGE,
        VERIFY_KEY_USAGE,
        EVENTS_USAGE,
        STATS_USAGE,
        PREVIEW_USAGE,
        PLACEMENT_USAGE,
        RIGHTSIZE_USAGE,
        DAYS_USAGE,
    ] {
        let shell = u.replace(k2_client::brand::CLI_NAME, "k2");
        assert!(all.contains(&shell), "not the shell's: {shell}");
    }
    let lib = source("compute-lib.sh");
    assert!(lib.contains(&NO_TEXT.replace(k2_client::brand::CLI_NAME, "k2")));
}

/// The arms this family answers are `cmd_compute`'s.
#[test]
fn the_verbs_are_arms_of_cmd_compute() {
    let cmd = source("compute-command.sh");
    for arm in [
        "        nodes|ls)\n",
        "        info)\n",
        "        queue)\n",
        "        jobs|list)\n",
        "        get|status)\n",
        "        grants)\n",
        "        receipt|provenance|verify) cmd_compute_provenance \"$sub\" \"$@\" ;;\n",
        "        events)\n",
        "        stats) cmd_compute_stats \"$@\" ;;\n",
        "        usage)\n",
        "        preview)\n",
        "        placement)\n",
        "        logs) cmd_compute_logs \"$@\" ;;\n",
        "        rightsize) cmd_compute_rightsize \"$@\" ;;\n",
    ] {
        assert!(cmd.contains(arm), "no arm {arm:?}");
    }
}

fn ok(words: &[&str]) -> Ask {
    match parse(words) {
        Some(Ok(a)) => a,
        other => panic!("{words:?}: {other:?}"),
    }
}

fn usage(words: &[&str]) -> String {
    match parse(words) {
        Some(Err(Stop::Usage(u))) => u,
        other => panic!("{words:?}: {other:?}"),
    }
}

#[test]
fn the_words() {
    assert_eq!(ok(&["ls", "x", "--json"]), Ask::Nodes { json: true });
    assert_eq!(usage(&["info"]), INFO_USAGE);
    assert_eq!(
        ok(&["info", "--json"]),
        Ask::Info {
            node: "--json".into(),
            json: true
        }
    );
    assert_eq!(usage(&["queue", "-x"]), QUEUE_USAGE);
    assert_eq!(
        ok(&["jobs", "--node", "a b", "--mine", "--tier", "standard", "--json", "--limit", "5"]),
        Ask::Jobs {
            query: "node=a%20b&mine=1&tier=normal&limit=5".into(),
            json: true
        }
    );
    assert_eq!(usage(&["jobs", "--tier", "x"]), TIER_USAGE);
    assert_eq!(usage(&["jobs", "--tier"]), TIER_USAGE);
    assert_eq!(usage(&["jobs", "x"]), JOBS_USAGE);
    assert_eq!(parse(&["jobs", "--node"]), Some(Err(Stop::Shift("--node".into()))));
    assert_eq!(
        ok(&["list", "--mine", "--state", "done", "--json"]),
        Ask::Jobs {
            query: "mine=1&state=done".into(),
            json: true
        }
    );
    assert_eq!(usage(&["list", "x"]), JOBS_USAGE);
    assert_eq!(ok(&["status"]), Ask::Status { json: false });
    assert_eq!(ok(&["status", "--json"]), Ask::Status { json: true });
    assert_eq!(usage(&["status", "--json", "x"]), STATUS_USAGE);
    assert_eq!(usage(&["status", ""]), STATUS_USAGE);
    assert_eq!(usage(&["get"]), GET_USAGE);
    assert_eq!(usage(&["get", ""]), GET_USAGE);
    assert_eq!(
        ok(&["status", "abcd", "--full"]),
        Ask::Get {
            job: "abcd".into(),
            full: true,
            json: false
        }
    );
    assert_eq!(
        ok(&["grants", "x", "--workspace", "/w", "--json"]),
        Ask::Grants {
            query: "workspace=%2Fw".into(),
            json: true
        }
    );
    assert_eq!(parse(&["grants", "--node"]), Some(Err(Stop::Shift("--node".into()))));
    assert_eq!(usage(&["receipt"]), RECEIPT_USAGE);
    assert_eq!(usage(&["receipt", "--out"]), PROVENANCE_OUT_USAGE);
    assert_eq!(
        usage(&["receipt", "--x"]),
        format!(
            "{0} compute receipt: unknown flag --x ({0} compute --help)",
            k2_client::brand::CLI_NAME
        )
    );
    assert_eq!(
        usage(&["receipt", "a", "b"]),
        format!(
            "{} compute receipt: one job at a time",
            k2_client::brand::CLI_NAME
        )
    );
    assert_eq!(
        ok(&["events", "--audit", "--limit", "5"]),
        Ask::EventsAudit {
            limit: "5".into(),
            json: false
        }
    );
    assert_eq!(
        ok(&["events", "--limit", "", "--audit"]),
        Ask::EventsAudit {
            limit: "50".into(),
            json: false
        }
    );
    assert_eq!(
        ok(&["events", "-f", "--max-wait", "1h30m", "--json"]),
        Ask::Events(events::Events {
            follow: true,
            max_wait: Some(5400),
            json: true,
            ..Default::default()
        })
    );
    assert_eq!(parse(&["events", "--max-wait", "1d"]), None);
    assert_eq!(
        parse(&["events", "--audit", "--limit"]),
        Some(Err(Stop::Shift("--limit".into())))
    );
    assert_eq!(usage(&["events", "x"]), EVENTS_USAGE);
    assert_eq!(usage(&["stats", "abc"]), STATS_USAGE);
    assert_eq!(usage(&["stats", "abcd", "efgh"]), STATS_USAGE);
    assert_eq!(usage(&["stats", "ab.cd"]), STATS_USAGE);
    assert_eq!(
        ok(&["stats", "--json", "abcd"]),
        Ask::Stats {
            job: "abcd".into(),
            json: true
        }
    );
    assert_eq!(
        ok(&["usage", "--days", ""]),
        Ask::Usage { days: "7".into() }
    );
    assert_eq!(
        ok(&["usage", "--days", "3"]),
        Ask::Usage { days: "3".into() }
    );
    for shell in [
        &["run"][..],
        &["wait", "x"],
        &[""],
        &["help"],
        &["jobs", "-h"],
        &["get", "x", "--help"],
        &["node", "servers"],
    ] {
        assert_eq!(parse(shell), None, "{shell:?}");
    }
}

#[test]
fn split_and_fail_as_the_shell() {
    assert_eq!(split(b"{}\n200"), ("200".into(), b"{}".to_vec()));
    assert_eq!(split(b"\n000\n000"), ("000".into(), b"\n000".to_vec()));
    assert_eq!(split(b"x"), ("x".into(), Vec::new()));
    assert_eq!(
        fail_line(
            "404",
            br#"{"error":{"code":"job_not_found","hint":"no job"}}"#
        ),
        r#"{"error": "job_not_found", "hint": "no job"}"#
    );
    assert_eq!(
        fail_line("403", br#"{"error":"forbidden","message":"no"}"#),
        r#"{"error": "forbidden", "hint": "no"}"#
    );
    assert_eq!(fail_line("500", b"oops\n"), "daemon_error: oops");
    assert_eq!(
        fail_line("000", b""),
        format!(
            "{{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the {PRODUCT_NAME} daemon (status 000)\"}}"
        )
    );
}

#[test]
fn renders_a_job_list() {
    let s = render::render("jobs", br#"{"jobs":[]}"#, false, 0);
    assert_eq!((s.stdout.as_str(), s.stderr.as_str()), ("(no jobs)\n", ""));
    let s = render::render(
        "grants",
        br#"{"grants":[{"node":"n1","workspace":"/w","maxJobSecs":60,"maxParallel":1,"maxQueued":2,"maxDiskGb":3,"grantedBy":"owner"}]}"#,
        false,
        0,
    );
    assert_eq!(
        s.stdout,
        "n1             /w                   max 60s/job  1 parallel  2 queued  3 GB disk  by owner\n"
    );
    let s = render::render("grants", br#"{"grants":[{}]}"#, false, 0);
    assert_eq!(s.exit, Exit::Error);
    assert!(
        s.stderr.ends_with("KeyError: 'maxJobSecs'\n"),
        "{}",
        s.stderr
    );
}
