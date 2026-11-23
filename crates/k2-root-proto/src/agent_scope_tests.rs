//! `agent_scope` (protocol 19): names, the classifier, caps, the
//! `StartTransientUnit` argv, the probe's verdict, the unit files, and the
//! verbs' argv.

use super::*;
use crate::verbs::{Class, Verb};

const UNIT: &str = "/k2cores.slice/k2-daemon.service";

fn ws(s: &str) -> AgentWs {
    AgentWs::parse(s).expect("ws")
}
fn id(s: &str) -> ScopeId {
    ScopeId::parse(s).expect("id")
}

#[test]
fn agent_scope_slice_names_have_no_dash_at_the_top() {
    // systemd reads a dash in a slice name as a level: the top has none and
    // each child names its parent.
    assert!(!TOP_SLICE.trim_end_matches(".slice").contains('-'));
    for k in Kind::ALL {
        assert!(k.slice().starts_with("k2agents-"), "{}", k.slice());
        assert!(k.anchor().starts_with(k.slice().trim_end_matches(".slice")), "{}", k.anchor());
    }
    assert_eq!(slice_cgroup(Kind::Open), "/k2agents.slice/k2agents-open.slice");
    assert_eq!(slice_cgroup(Kind::Fenced), "/k2agents.slice/k2agents-fenced.slice");
    assert_eq!(Level::Top.cgroup(), "/k2agents.slice");
}

#[test]
fn agent_scope_kinds_parse() {
    assert_eq!(Kind::parse("open"), Some(Kind::Open));
    assert_eq!(Kind::parse("fenced"), Some(Kind::Fenced));
    for bad in ["", "Open", "fence", "open ", "closed"] {
        assert_eq!(Kind::parse(bad), None, "{bad:?}");
    }
    for k in Kind::ALL {
        assert_eq!(Kind::parse(k.as_str()), Some(k));
    }
}

#[test]
fn agent_scope_ids_are_lowercase_hex_16_to_32() {
    assert!(ScopeId::parse("0123456789abcdef").is_some());
    assert!(ScopeId::parse(&"f".repeat(32)).is_some());
    for bad in ["", "0123456789abcde", &"a".repeat(33), "0123456789ABCDEF", "0123456789abcdeg", "0123456789abcde:", "0123456789abcd-f", "0123456789abcd f"] {
        assert!(ScopeId::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn agent_scope_names_round_trip_through_the_classifier() {
    let n = scope_name(&ws("abc"), &id("0123456789abcdef"));
    assert_eq!(n, "k2-agent-abc:0123456789abcdef.scope");
    assert_eq!(parse_scope_name(&n), Some((ws("abc"), id("0123456789abcdef"))));
    assert_eq!(
        scope_cgroup(Kind::Fenced, &ws("abc"), &id("0123456789abcdef")),
        "/k2agents.slice/k2agents-fenced.slice/k2-agent-abc:0123456789abcdef.scope"
    );
}

/// One workspace id that is a prefix of another: each name classifies as
/// its own workspace, never the other's.
#[test]
fn agent_scope_classifier_keeps_prefix_workspaces_apart() {
    let a = scope_name(&ws("abc"), &id("00000000000000aa"));
    let b = scope_name(&ws("abc-def"), &id("00000000000000aa"));
    assert_eq!(parse_scope_name(&a).map(|(w, _)| w), Some(ws("abc")));
    assert_eq!(parse_scope_name(&b).map(|(w, _)| w), Some(ws("abc-def")));
    assert_ne!(parse_scope_name(&a), parse_scope_name(&b));
    // A workspace's dash never reads as the separator.
    assert_eq!(parse_scope_name("k2-agent-abc-def-00000000000000aa.scope"), None);
}

#[test]
fn agent_scope_classifier_refuses_everything_else() {
    for bad in [
        "",
        "k2-agent-.scope",
        "k2-agent-abc.scope",
        "k2-agent-abc:.scope",
        "k2-agent-:0123456789abcdef.scope",
        "k2-agent-abc:0123456789abcdef",
        "k2-agent-abc:0123456789abcdef.service",
        "k2-agent-abc:0123456789abcdef.scope.d",
        "k2-agent-abc:0123456789abcdef:0123456789abcdef.scope",
        "k2-agent-a/b:0123456789abcdef.scope",
        "k2-agent-abc:0123456789ABCDEF.scope",
        "x-k2-agent-abc:0123456789abcdef.scope",
        "k2agents-open-anchor.service",
        "cgroup.procs",
        "k2-agent-abc:0123456789abcdef.scope ",
    ] {
        assert_eq!(parse_scope_name(bad), None, "{bad:?}");
    }
}

#[test]
fn agent_scope_probe_name_is_shaped_like_an_agents() {
    let n = probe_scope_name("00112233445566ff").expect("probe");
    assert_eq!(n, "k2-agent-probe:00112233445566ff.scope");
    assert!(parse_scope_name(&n).is_some(), "the probe asks for exactly what an agent would");
    assert_eq!(probe_scope_name("xyz"), None);
}

#[test]
fn agent_scope_processes_come_from_the_daemons_unit_only() {
    let w = ws("A");
    for ok in [
        UNIT.to_string(),
        format!("{UNIT}/daemon"),
        format!("{UNIT}/daemon/x"),
        format!("{UNIT}/agents"),
        format!("{UNIT}/agents/a-A"),
        format!("{UNIT}/agents/a-A/sub"),
        format!("{UNIT}/other-leaf"),
        format!("{UNIT}/"),
    ] {
        assert!(may_scope_from(UNIT, &w, &ok), "{ok}");
    }
    for bad in [
        format!("{UNIT}/agents/a-B"),
        format!("{UNIT}/agents/a-B/a-A"),
        format!("{UNIT}/agents/a-AB"),
        format!("{UNIT}x/daemon"),
        format!("{UNIT}/../x.service"),
        format!("{UNIT}//daemon"),
        "/k2agents.slice/k2agents-open.slice/k2-agent-A:0123456789abcdef.scope".to_string(),
        "/user.slice/user-1000.slice/session-2.scope".to_string(),
        "/".to_string(),
        String::new(),
    ] {
        assert!(!may_scope_from(UNIT, &w, &bad), "{bad}");
    }
}

#[test]
fn agent_scope_caps_take_bounded_numbers_only() {
    assert_eq!(Caps::parse(&[]), Some(Caps::default()));
    assert_eq!(Caps::parse(&["mem=512"]), Some(Caps { mem_mb: Some(512), cpu_pct: None }));
    assert_eq!(Caps::parse(&["cpu=150"]), Some(Caps { mem_mb: None, cpu_pct: Some(150) }));
    assert_eq!(Caps::parse(&["mem=16", "cpu=25600"]), Some(Caps { mem_mb: Some(16), cpu_pct: Some(25_600) }));
    assert_eq!(Caps::parse(&["mem=1048576", "cpu=1"]), Some(Caps { mem_mb: Some(1_048_576), cpu_pct: Some(1) }));
    for bad in [
        &["mem=15"][..],
        &["mem=1048577"],
        &["mem=0"],
        &["mem=0512"],
        &["mem=512M"],
        &["mem=512 "],
        &["mem=+512"],
        &["mem="],
        &["mem=-1"],
        &["cpu=0"],
        &["cpu=25601"],
        &["cpu=1.5"],
        &["cpu=50%"],
        &["mem=512", "mem=512"],
        &["cpu=50", "cpu=50"],
        &["cpu=50", "mem=512"],
        &["MemoryMax=512M"],
        &["mem=512", "MemoryMax=1"],
        &["CPUQuota=50%"],
        &["mem"],
        &[""],
    ] {
        assert_eq!(Caps::parse(bad), None, "{bad:?}");
    }
    let c = Caps { mem_mb: Some(512), cpu_pct: Some(150) };
    assert_eq!(c.memory_max(), Some(512 * 1024 * 1024));
    assert_eq!(c.cpu_quota_usec(), Some(1_500_000));
    assert_eq!(Caps { mem_mb: Some(1_048_576), cpu_pct: Some(25_600) }.memory_max(), Some(1_099_511_627_776));
    assert_eq!(c.tokens(), vec!["mem=512", "cpu=150"]);
}

#[test]
fn agent_scope_start_transient_argv_is_exact() {
    let n = "k2-agent-abc:0123456789abcdef.scope";
    let want: Vec<&str> = vec![
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
        n,
        "fail",
        "5",
        "Slice",
        "s",
        "k2agents-open.slice",
        "PIDs",
        "au",
        "1",
        "4242",
        "Delegate",
        "b",
        "false",
        "CollectMode",
        "s",
        "inactive-or-failed",
        "TimeoutStopUSec",
        "t",
        "10000000",
        "0",
    ];
    assert_eq!(start_transient_argv(n, Kind::Open, &[4242], &Caps::default()), want);
    let capped = start_transient_argv(n, Kind::Fenced, &[7, 8], &Caps { mem_mb: Some(512), cpu_pct: Some(150) });
    let want: Vec<&str> = vec![
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
        n,
        "fail",
        "7",
        "Slice",
        "s",
        "k2agents-fenced.slice",
        "PIDs",
        "au",
        "2",
        "7",
        "8",
        "Delegate",
        "b",
        "false",
        "CollectMode",
        "s",
        "inactive-or-failed",
        "TimeoutStopUSec",
        "t",
        "10000000",
        "MemoryMax",
        "t",
        "536870912",
        "CPUQuotaPerSecUSec",
        "t",
        "1500000",
        "0",
    ];
    assert_eq!(capped, want);
}

#[test]
fn agent_scope_probe_verdicts() {
    assert_eq!(classify_probe(0, ""), Probe::Allowed);
    assert_eq!(classify_probe(1, "Failed to start transient scope unit: Access denied"), Probe::Refused);
    assert_eq!(classify_probe(1, "Failed to start transient scope unit: Interactive authentication required."), Probe::Refused);
    assert_eq!(classify_probe(1, "Failed to connect to bus: No such file or directory"), Probe::Unknown);
    assert_eq!(classify_probe(1, "Unit k2-agent-probe:00112233445566ff.scope already exists."), Probe::Unknown);
    assert_eq!(Probe::Allowed.as_str(), "ALLOWED");
    assert_eq!(Probe::Refused.as_str(), "refused");
}

#[test]
fn agent_scope_unit_files_hold_the_exact_text() {
    assert_eq!(
        slice_unit(Level::Top),
        "# K2 agents: every agent session's scope sits in K2's agent slices, root's alone.\n\
         # Written by k2-root-helper `agents setup`.\n\
         [Unit]\n\
         Description=K2 agents\n\
         Before=slices.target\n"
    );
    assert_eq!(
        slice_unit(Level::Agents(Kind::Fenced)),
        "# K2 agents (fenced): every agent session's scope sits in K2's agent slices, root's alone.\n\
         # Written by k2-root-helper `agents setup`.\n\
         [Unit]\n\
         Description=K2 agents (fenced)\n\
         Before=slices.target\n"
    );
    assert_eq!(
        anchor_unit(Level::Agents(Kind::Open), "/usr/bin/sleep").expect("anchor"),
        "# Keeps k2agents-open.slice's cgroup alive while no agent runs.\n\
         # Written by k2-root-helper `agents setup`.\n\
         [Unit]\n\
         Description=K2 agents (open) anchor\n\
         \n\
         [Service]\n\
         Type=exec\n\
         Slice=k2agents-open.slice\n\
         User=nobody\n\
         ExecStart=/usr/bin/sleep infinity\n\
         Restart=always\n\
         RestartSec=5\n\
         NoNewPrivileges=yes\n\
         PrivateNetwork=yes\n\
         CapabilityBoundingSet=\n\
         TasksMax=4\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    );
    let top = anchor_unit(Level::Top, "/bin/sleep").expect("anchor");
    assert!(top.contains("\nSlice=k2agents.slice\n") && top.contains("\nExecStart=/bin/sleep infinity\n"), "{top}");
    assert_eq!(anchor_unit(Level::Top, "/tmp/sleep"), None, "sleep only from the closed list");
    let files = unit_files("/usr/bin/sleep").expect("files");
    let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [TOP_SLICE, OPEN_SLICE, FENCED_SLICE, TOP_ANCHOR, OPEN_ANCHOR, FENCED_ANCHOR],
        "three slices, then three anchors"
    );
    assert!(unit_files("sleep").is_none());
}

#[test]
fn agent_scope_verbs_round_trip_and_are_classed() {
    for v in ScopeVerb::catalog() {
        let mut argv = vec!["agents".to_string()];
        argv.extend(v.tail());
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert_eq!(Verb::parse(&refs), Ok(Verb::AgentScope(v.clone())), "{argv:?}");
        assert_eq!(Verb::AgentScope(v.clone()).argv(), argv);
        assert_eq!(Verb::AgentScope(v.clone()).namespace(), "agents");
    }
    let class = |a: &[&str]| Verb::parse(a).expect("parses").class();
    let id = "0123456789abcdef";
    for a in [
        &["agents", "scope", "ws-1", id, "4242", "1", "open"][..],
        &["agents", "scope", "ws-1", id, "4242", "1", "fenced", "mem=512", "cpu=150"],
        &["agents", "adopt", "ws-1", id, "open"],
        &["agents", "stop", "ws-1", id],
        &["agents", "stop-all"],
        &["agents", "layout"],
        &["agents", "setup"],
        &["agents", "teardown"],
    ] {
        assert_eq!(class(a), Class::Daemon, "{a:?}");
    }
    // M1b's verbs keep their own shapes.
    assert!(matches!(Verb::parse(&["agents", "check"]), Ok(Verb::AgentsCheck)));
    assert!(matches!(Verb::parse(&["agents", "place", "ws-1", "4242", "1"]), Ok(Verb::AgentsPlace { .. })));
    assert!(!Verb::AgentScope(ScopeVerb::Layout).takes_lock());
    assert!(Verb::AgentScope(ScopeVerb::StopAll).takes_lock());
    assert!(Verb::AgentScope(ScopeVerb::Setup).stdin_cap().is_none());
}

#[test]
fn agent_scope_verbs_refuse_other_argv() {
    let id = "0123456789abcdef";
    for a in [
        &["agents"][..],
        &["agents", "scope"],
        &["agents", "scope", "ws-1", id, "4242", "1"],
        &["agents", "scope", "ws-1", id, "4242", "1", "both"],
        &["agents", "scope", "ws-1", id, "1", "1", "open"],
        &["agents", "scope", "ws-1", id, "4242", "01", "open"],
        &["agents", "scope", "ws-1", "0123456789ABCDEF", "4242", "1", "open"],
        &["agents", "scope", "ws:1", id, "4242", "1", "open"],
        &["agents", "scope", "../x", id, "4242", "1", "open"],
        &["agents", "scope", "ws-1", id, "4242", "1", "open", "MemoryMax=1G"],
        &["agents", "scope", "ws-1", id, "4242", "1", "open", "mem=512", "cpu=150", "x"],
        &["agents", "scope", "ws-1", id, "4242", "1", "open", "cpu=150", "mem=512"],
        &["agents", "scope", "ws-1", id, "4242", "1", "open", "mem=512", "mem=512"],
        &["agents", "adopt", "ws-1", id],
        &["agents", "adopt", "ws-1", id, "open", "4242"],
        &["agents", "stop", "ws-1"],
        &["agents", "stop", "ws-1", id, "--now"],
        &["agents", "stop", "k2-agent-ws-1:0123456789abcdef.scope"],
        &["agents", "stop-all", "open"],
        &["agents", "layout", "--fix"],
        &["agents", "setup", "/etc/systemd/system"],
        &["agents", "teardown", "--keep"],
        &["agents", "unknown"],
    ] {
        assert!(Verb::parse(a).is_err(), "{a:?}");
    }
}
