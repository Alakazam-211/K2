//! The compute readers against the shell's own Python, answer by answer.
//!
//! Each case is a daemon answer: a full one per verb (every branch of the
//! shell's printer), and the same with each of its fields swapped for a
//! value of another type, and some bodies that are not what the verb
//! expects at all. The shell's Python (`_compute_render` in
//! `cli/src/compute-lib.sh`, `cmd_compute_status`'s in
//! `cli/src/compute-jobs.sh`, `_compute_text`'s and `get --json`'s
//! one-liners) runs on each in one `python3` (a harness that runs the code
//! as `python3 -c` would, once per answer), and the binary's port on the
//! same answer must print the same stdout and, where Python raised, an
//! exception of the same class (the rig's rule for a traceback: its frames
//! and wording depend on the Python version).

#![cfg(unix)]

mod common;

use k5_cli::families::compute::{reads as compute, render, status};
use k5_cli::out::json::{self, Options, Text, Value};
use std::io::Write;
use std::process::{Command, Stdio};

/// The text of the first `python3 -c '…'` after `after` in `file`.
fn python_of(file: &str, after: &str) -> String {
    let text = std::fs::read_to_string(common::cli_src().join(file)).unwrap();
    let at = text
        .find(after)
        .unwrap_or_else(|| panic!("no {after:?} in {file}"));
    let open = "python3 -c '";
    let start = text[at..].find(open).expect("a python3 -c '") + at + open.len();
    let end = text[start..].find('\'').expect("its end") + start;
    text[start..end].to_string()
}

const HARNESS: &str = r#"
import contextlib, io, json, os, sys, traceback
code = compile(open(sys.argv[1]).read(), "<string>", "exec")
out = []
for line in open(sys.argv[2]):
    case = json.loads(line)
    os.environ.update(case["env"])
    sys.stdin = io.StringIO(case["body"])
    buf = io.StringIO()
    exc = ""
    try:
        with contextlib.redirect_stdout(buf):
            exec(code, {"__name__": "__main__"})
    except SystemExit:
        pass
    except BaseException as e:
        t = type(e)
        exc = t.__qualname__ if t.__module__ == "builtins" else t.__module__ + "." + t.__qualname__
    out.append({"stdout": buf.getvalue(), "exc": exc})
print(json.dumps(out))
"#;

struct Case {
    body: String,
    env: Vec<(&'static str, String)>,
}

/// What the shell's Python printed for each case: stdout, and the class
/// of the exception it raised ("" when none).
fn python(script: &str, cases: &[Case]) -> Vec<(String, String)> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("k5-compute-oracle-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let code = dir.join("code.py");
    std::fs::write(&code, script).unwrap();
    let input = dir.join("cases.jsonl");
    let mut f = std::fs::File::create(&input).unwrap();
    for c in cases {
        let env = Value::Object(
            c.env
                .iter()
                .map(|(k, v)| (Text::from(*k), Value::Str(Text::from(v.as_str()))))
                .collect(),
        );
        let row = Value::Object(vec![
            (Text::from("body"), Value::Str(Text::from(c.body.as_str()))),
            (Text::from("env"), env),
        ]);
        writeln!(f, "{}", json::to_string(&row, &Options::default()).unwrap()).unwrap();
    }
    drop(f);
    let harness = dir.join("harness.py");
    std::fs::write(&harness, HARNESS).unwrap();
    let o = Command::new("python3")
        .arg(&harness)
        .arg(&code)
        .arg(&input)
        .stdin(Stdio::null())
        .output()
        .expect("run python3");
    assert!(o.status.success(), "the harness: {o:?}");
    std::fs::remove_dir_all(&dir).unwrap();
    let v = json::parse(&String::from_utf8(o.stdout).unwrap()).unwrap();
    let Value::Array(rows) = v else {
        panic!("not a list")
    };
    rows.iter()
        .map(|r| {
            let Value::Object(p) = r else {
                panic!("not an object")
            };
            let get = |k: &str| match p.iter().find(|(key, _)| key.as_str() == Some(k)) {
                Some((_, Value::Str(t))) => t.as_str().unwrap().to_string(),
                other => panic!("{k}: {other:?}"),
            };
            (get("stdout"), get("exc"))
        })
        .collect()
}

/// A dict sliced (`{}[:8]`): Python 3.12 made slices hashable, so from
/// there it raises KeyError, which the binary follows (it has no Python
/// version to copy); before 3.12 it raises TypeError ("unhashable type:
/// 'slice'"). Only on a `python3` older than 3.12, read from the one the
/// harness runs, is that TypeError the binary's KeyError.
fn older_slice_error(exc: &str, rust_err: &str) -> bool {
    exc == "TypeError"
        && rust_err
            .lines()
            .last()
            .is_some_and(|l| l.starts_with("KeyError: slice("))
        && common::parity::python3_version() < (3, 12)
}

/// The class of the exception a traceback ends with ("" when none).
fn class_of(stderr: &str) -> String {
    if stderr.is_empty() {
        return String::new();
    }
    let last = stderr.lines().last().unwrap_or_default();
    last.split(':').next().unwrap_or_default().to_string()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// `text` with `{now±N}` filled in from `now`.
fn timed(text: &str, now: i64) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("{now") {
        out.push_str(&rest[..at]);
        let end = rest[at..].find('}').unwrap() + at;
        let off: i64 = rest[at + 4..end].parse().unwrap_or(0);
        out.push_str(&(now + off).to_string());
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The values a field is swapped for.
const OTHERS: &[&str] = &[
    "null",
    "0",
    "7",
    "-90061",
    "2.5",
    "true",
    "false",
    "\"\"",
    "\"x:y\"",
    "[]",
    "[\"a\", 3]",
    "{}",
    "{\"k\": \"v\"}",
];

/// `base` and every answer made by swapping one of its fields (every
/// object key and list item, at any depth) for each of [`OTHERS`].
fn variants(base: &str) -> Vec<String> {
    let v = json::parse(base).unwrap_or_else(|e| panic!("a base answer: {e}: {base}"));
    let mut out = vec![base.to_string()];
    let others: Vec<Value> = OTHERS.iter().map(|t| json::parse(t).unwrap()).collect();
    let mut paths = Vec::new();
    walk(&v, &mut Vec::new(), &mut paths);
    for path in paths {
        for o in &others {
            let mut w = v.clone();
            *at_path(&mut w, &path) = o.clone();
            out.push(json::to_string(&w, &Options::default()).unwrap());
        }
    }
    out
}

fn walk(v: &Value, here: &mut Vec<usize>, paths: &mut Vec<Vec<usize>>) {
    let kids: Vec<&Value> = match v {
        Value::Object(p) => p.iter().map(|(_, x)| x).collect(),
        Value::Array(items) => items.iter().collect(),
        _ => return,
    };
    for (i, k) in kids.into_iter().enumerate() {
        here.push(i);
        paths.push(here.clone());
        walk(k, here, paths);
        here.pop();
    }
}

fn at_path<'a>(v: &'a mut Value, path: &[usize]) -> &'a mut Value {
    let mut cur = v;
    for i in path {
        cur = match cur {
            Value::Object(p) => &mut p[*i].1,
            Value::Array(items) => &mut items[*i],
            _ => unreachable!("a path follows the value"),
        };
    }
    cur
}

const ODD_BODIES: &[&str] = &[
    "",
    "   ",
    "null",
    "5",
    "\"text\"",
    "[]",
    "[1, 2]",
    "{}",
    "not json",
    "{\"a\": 1} x",
];

fn check(
    name: &str,
    script: &str,
    bases: &[&str],
    env: &[(&'static str, &str)],
    rust: impl Fn(&[u8], i64) -> (String, String),
) {
    let now = now();
    let mut bodies: Vec<String> = ODD_BODIES.iter().map(|b| b.to_string()).collect();
    for b in bases {
        bodies.extend(variants(&timed(b, now)));
    }
    let cases: Vec<Case> = bodies
        .iter()
        .map(|b| Case {
            body: b.clone(),
            env: env.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        })
        .collect();
    let want = python(script, &cases);
    let mut bad = Vec::new();
    for (body, (stdout, exc)) in bodies.iter().zip(&want) {
        // `time.localtime(None)` and `time.time()` are when Python ran,
        // a few seconds after `now`: one of those seconds must match.
        let (out, err) = (0..6)
            .map(|d| rust(body.as_bytes(), now + d))
            .find(|(o, e)| o == stdout && (&class_of(e) == exc || older_slice_error(exc, e)))
            .unwrap_or_else(|| rust(body.as_bytes(), now));
        let class = class_of(&err);
        if &out != stdout || (&class != exc && !older_slice_error(exc, &err)) {
            bad.push(format!(
                "== {body}\n-- python ({exc}):\n{stdout}\n-- rust ({class}):\n{out}\n{err}"
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{name}: {} of {} answers differ:\n{}",
        bad.len(),
        bodies.len(),
        bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n")
    );
    println!("{name}: {} answers match", bodies.len());
}

fn render_check(verb: &'static str, bases: &[&str], full: bool) {
    let script = python_of("compute-lib.sh", "\n_compute_render() {");
    let env = [
        ("K2_CP_VERB", verb),
        ("K2_CP_FULL", if full { "1" } else { "0" }),
    ];
    check(verb, &script, bases, &env, |body, now| {
        let s = render::render(verb, body, full, now);
        (s.stdout, s.stderr)
    });
}

const NODE_FULL: &str = r#"{"name":"linux-1","platform":"ubuntu24.04-x86_64","state":"active","online":true,"liveRoute":"lan","offer":{"control":"active","caps":{"cpu_millis":8000,"mem_bytes":17179869184},"free":{"cpu_millis":6500,"mem_bytes":12884901888,"disk_free_bytes":107374182400},"parallelMax":4},"idleCpuMillis":7250,"room":{"busy":"this_server","running":3,"otherServers":1},"mine":{"running":1,"queued":0},"running":2,"queued":1,"queueTiers":{"ci":1,"priority":0,"normal":2,"batch":1},"grant":{"maxJobSecs":7200,"maxParallel":2,"maxQueued":5,"allowCi":true},"lastSeen":{now-30},"rebootPending":true,"reservedBy":[{"short":"aaaa1111","state":"running","startedAt":{now-630},"cpuMillis":2000,"memBytes":4294967296},{"short":"bbbb2222","state":"queued","createdAt":{now-7300}},{"state":"x"},{"short":"c"},{"short":"d"}],"leftovers":{"count":2,"procs":[{"pid":123,"age_secs":90000,"job_id":"abcdef123456","cmd":"cargo test"},{"pid":5,"age_secs":4000}]},"share":{"role":"tenant","ownerLabel":"Acme","servers":3,"expiresAt":{now+86400},"state":"paused","skewSecs":-3,"line":"run 1/2","priority":false},"update":{"state":"staged","drain":{"running":2,"jobsEndBy":{now+3600}},"version":"0.46.18"},"nodeVersion":"0.46.17","allowed":false}"#;

#[test]
fn nodes() {
    let others = r#"{"criticalPath":[{"submitter":"agent-a","jobs":2,"workspace":"/w/a"},{}],"nodes":[
        {"name":"mac-1","state":"pending","sas":"123456","os":"macos","arch":"arm64","rollout":{"code":"waiting_turn","node":"pending","state":"running","toVersion":"0.46.18"},"share":{"servers":2}},
        {"name":"n3","online":false,"controllerPause":"paused","pinnedOnly":true,"rollout":{"code":"node_needs_manual_install"}},
        {"name":"n4","online":true,"offer":{"control":"draining","controlBy":"k2-node node.update x"}},
        {"name":"n5","online":true,"offer":{"control":"paused","controlBy":"owner"},"rollout":{"node":"rolled_back","toVersion":"0.46.1"}},
        {"name":"n6","online":true,"offer":{"refusal":{"code":"disk_full"}},"rollout":{"code":"waiting_idle","node":"pending","state":"running"}},
        {"name":"n7","online":true,"offer":{"available":false,"unavailable":["gpu","disk"]},"update":{"state":"offered","version":"0.47"}}
    ],"waitingCodes":[{"name":"n9","expiresAt":{now+300}}],"allowNodes":["n1","n2"],"allowNodesWorkspace":"/w/a","allowNodesFixed":true}"#;
    let one = format!(r#"{{"nodes":[{NODE_FULL}],"allowNodes":[]}}"#);
    render_check(
        "nodes",
        &[
            &one,
            others,
            r#"{"nodes":[],"waitingCodes":null}"#,
            r#"{"nodes":[],"waitingCodes":[]}"#,
        ],
        false,
    );
}

#[test]
fn info() {
    let extra = r#"{"name":"linux-1","online":true,"shared":true,"offer":{"features":["tiers2"],"protocol":7,"tools":{"rust":"1.99","node":"22"},"caps":{}},"host":{"line":"Ubuntu 24.04"},"packages":{"manager":"apt","policy":"ask","openRequests":1},"routes":["lan","relay"],"grants":[{"workspace":"/w","maxJobSecs":3600,"maxParallel":2,"submitterMaxRunning":1,"maxQueued":4,"maxDiskGb":20,"allowCi":true},{"workspace":"/u","maxJobSecs":60,"maxParallel":1,"maxQueued":1,"maxDiskGb":1,"allowUserns":true,"allowNoAddrFilter":true}],"recentJobs":[{"short":"abcd1234","state":"done","exitCode":0,"argv":["cargo","test"]},{"short":"x","state":"failed"}]}"#;
    let tiers = r#"{"name":"n","online":true,"shared":true,"offer":{"features":["tiers"]},"packages":{"openRequests":2}}"#;
    let none = r#"{"name":"n","online":false,"shared":true,"nodeVersion":"0.46.1","offer":{}}"#;
    let solo = r#"{"name":"n","online":false,"nodeVersion":"0.46.1"}"#;
    render_check("info", &[NODE_FULL, extra, tiers, none, solo], false);
}

const JOB_ROWS: &str = r#"{"jobs":[{"short":"abcd1234","tier":"ci","node":"linux-1-with-a-long-name","state":"queued","pinned":true,"exitCode":null,"reason":"","createdAt":{now-150},"tag":"a-very-long-tag-name-here","submitter":"agent-a","via":"owner_token","argv":["cargo","nextest","run","--workspace"]},{"short":"efgh5678","tier":"priority","state":"done","exitCode":0,"startedAt":{now-4000},"createdAt":{now-90000},"submittedBy":"owner"},{"short":"ijkl","state":"queued","pinned":false,"tier":"batch"},{"short":"m","state":"failed","exitCode":2,"reason":"tenant_disk_full"},{"short":"held0001","tier":"standard","node":"linux-1","state":"queued","pinned":false,"reason":"grant_parallel_full","wait":{"reason":"held"},"createdAt":{now-30},"argv":["make"]}]}"#;

#[test]
fn jobs() {
    render_check("jobs", &[JOB_ROWS, r#"{"jobs":[]}"#], false);
}

const JOB_FULL: &str = r#"{"short":"abcd1234","node":"linux-1","state":"running","reason":"moved","argv":["bash","-c","echo hi"],"tier":"priority","tierAsked":"ci","ctl":{"why":"critical path","memDefaulted":true,"placement":{"mode":"auto","code":"moved_node_draining","node":"linux-2","policy":"v2","why":"free cpu","fits":"8 CPUs"}},"tag":"t1","submittedBy":"owner","submitter":"agent-a","via":"owner_token","supersedeKey":"k","secrets":["A","B"],"network":{"line":"none","dummies":true},"userns":true,"noAddrFilter":true,"avoid":["mac-1"],"move":{"note":"may move"},"createdAt":{now-7300},"startedAt":{now-630},"endedAt":{now-30},"srcCommit":"abc","srcTree":"def","exitCode":null,"signal":9,"attempt":2,"generation":3,"attempts":[{"attempt":1,"generation":1,"node":"mac-1","startedAt":{now-5000},"endedAt":{now-4000},"state":"failed","reason":"node_lost","why":"lost","next":"retried","log":{"available":true,"bytes":20480},"artifacts":{"status":"kept","files":["a","b"]}},{"attempt":1,"generation":2,"state":"cancelled","artifacts":{"status":"not_collected","note":"no room"}},{"attempt":2,"generation":3,"current":true,"startedAt":{now-630}}],"queue":{"line":"waits behind 2","aheadTiers":{"ci":1,"priority":2},"parallel":2,"ahead":[{"short":"zz","state":"running","workspace":"/w","elapsedSecs":125,"argv":"make","reason":"slot"},{"ageSecs":61},{}]},"wait":{"etaSecs":300},"stall":{"stallSecs":600,"warned":true},"reportedLate":{"secs":4000,"nodeEndedAt":{now-5000},"heardAt":{now-1110}},"hint":"try again","cancelledBy":{"by":"owner","at":{now-60}},"result":{"detail":"disk","notes":["evicted x","other","killed y","network: no-lan proxy: z"],"networkHosts":[{"host":"example.com","port":443,"verdict":"allowed","conns":3,"bytesIn":2048,"bytesOut":3000000},{"host":"::1","port":80,"verdict":"refused","addr":"::1","reason":"lan"},{"host":"h","port":1,"verdict":"failed","addr":"1.2.3.4","conns":2}],"tests":{"passed":10,"failed":1,"errors":0,"skipped":2},"artifacts":[{"name":"out.tar","bytes":100,"sha256":"0123456789abcdef0123"}],"receipt":{"id":"r1","verified":true},"tools":{"versions":{"rust":"1.99","cargo":"1.99"},"basis":"node-probe"},"resources":{"peakMemBytes":3221225472,"memLimitBytes":8589934592,"memSource":"cgroup","cpuSecs":12.5,"cpuSource":"cgroup","gpu":{"model":"RX","busyAvgPct":40,"busyMaxPct":90,"vramPeakBytes":1073741824,"gttPeakBytes":536870912,"source":"sysfs","memLimitBytes":2147483648,"jobMemPeakBytes":1073741824}},"host":{"start":{"load1":1.5,"cpus":8,"jobs":2,"steal":0.5,"gpuBusy":10,"quiet":false,"quietWhy":"busy"},"end":{"load1":0.1,"cpus":8,"jobs":0,"quiet":true}},"effective":{"mem":"8G","cpus":4},"shards":[{"index":1,"short":"s1","node":"n1","state":"done","exit":0,"tests":{"passed":5,"failed":1,"errors":1}},{"index":2,"short":"s2","state":"running"}]}}"#;

#[test]
fn get() {
    let on_node = r#"{"short":"a","node":"n","state":"queued","class":"big","queue":{"onNode":true,"message":"3rd in line","reason":"node_busy"},"wait":{"etaSecs":60},"result":{"tenant":1}}"#;
    let queued = r#"{"short":"a","state":"queued","queue":{"position":2,"of":5,"reason":"busy","message":"waiting","aheadTiers":{"ci":2}},"wait":{"etaSecs":3600},"stall":{"stallSecs":0}}"#;
    let words = r#"{"short":"a","state":"preparing","wait":{"words":"preparing the checkout","etaSecs":7260,"etaBasis":"history"},"argv":["x"],"reason":"tenant_disk_full","result":{"detail":"full"}}"#;
    let ended = r#"{"short":"a","state":"done","endedAt":{now-100},"exitCode":0,"reportedLate":{"secs":120}}"#;
    let long = format!(
        r#"{{"short":"a","state":"done","argv":["{}"]}}"#,
        "x".repeat(200)
    );
    let mut many = String::from(r#"{"short":"a","state":"done","result":{"networkHosts":["#);
    for i in 0..43 {
        if i > 0 {
            many.push(',');
        }
        many.push_str(&format!(
            r#"{{"host":"h{i}","port":{i},"verdict":"allowed","bytesIn":{}}}"#,
            i64::from(i) * 400_000_000
        ));
    }
    many.push_str("]}}");
    // 0.46.17: held on this server, and a pull back in flight.
    let held = r#"{"short":"a","node":"n","state":"queued","queue":{"where":"server","held":true,"fits":["node-a","node-b"],"line":"held: waiting for a node with a free slot (fits: node-a, node-b)","waitReason":"held","nodeWaitWords":"busy"},"wait":{"reason":"held"}}"#;
    let pulling = r#"{"short":"a","node":"n","state":"queued","queue":{"onNode":true,"message":"2nd in line","reason":"pull_requested"},"move":{"auto":true,"moving":true,"note":"pulling back"}}"#;
    render_check(
        "get",
        &[
            JOB_FULL, on_node, queued, words, ended, &long, &many, held, pulling,
        ],
        false,
    );
    render_check("get", &[&long], true);
}

#[test]
fn queue() {
    let q = r#"{"node":"linux-1","from":"node","entries":[{"pos":1,"mine":true,"short":"abcd","state":"queued","pinned":true,"argv":"cargo test","workspace":"/w","tier":"priority","tierWhy":"critical path","submitter":"agent-a","class":"big","etaSecs":61,"cpuMillis":2500,"why":"waits"},{"pos":2,"mine":true,"state":"running","etaSecs":0},{"pos":3,"server":"Globex","blockedBy":"slot","etaSecs":59},{"pos":4}],"tiers":{"ci":1},"totals":[{"label":"this","running":1,"queued":2,"cpuSecsToday":5400}]}"#;
    render_check("queue", &[q, r#"{"node":"n","entries":[]}"#], false);
}

#[test]
fn grants_receipt_usage_events() {
    render_check(
        "grants",
        &[
            r#"{"grants":[{"node":"n1","workspace":"/w","maxJobSecs":3600,"maxParallel":2,"maxQueued":3,"maxDiskGb":10,"allowCi":true,"grantedBy":"owner"},{"maxJobSecs":1,"maxParallel":1,"maxQueued":1,"maxDiskGb":1,"grantedBy":"x"},{"node":"n2","workspace":"/u","maxJobSecs":60,"maxParallel":1,"maxQueued":1,"maxDiskGb":1,"allowCi":true,"allowUserns":true,"allowNoAddrFilter":true,"grantedBy":"owner"}]}"#,
            r#"{"grants":[]}"#,
        ],
        false,
    );
    render_check(
        "receipt",
        &[
            r#"{"receipt":{"id":"r","sig":"ab","nested":{"a":[1,2.5,"é"]}},"verified":true,"proves":"the node ran it"}"#,
            r#"{"verified":false}"#,
        ],
        false,
    );
    render_check(
        "usage",
        &[
            r#"{"usage":[{"day":"2026-11-19","node":"n1","route":"relay","bytesIn":1234567,"bytesOut":5},{"day":"d","nodeId":"0123456789ab","route":"lan","bytesIn":0,"bytesOut":0}],"metered":"relay traffic is metered"}"#,
            r#"{"usage":[]}"#,
        ],
        false,
    );
    render_check(
        "events",
        &[
            r#"{"events":[{"at":{now-100},"kind":"node.pause","actor":"owner-with-a-long-name","nodeId":"0123456789","detail":{"why":"maintenance","n":1}},{"at":{now},"kind":"job.cancel","actor":"a","jobId":"abcdef1234","detail":null},{"at":1.5,"kind":"k","actor":"","detail":"é"}]}"#,
        ],
        false,
    );
}

#[test]
fn servers() {
    let owner = r#"{"servers":[{"slot":0,"label":"this server","state":"active","trust":"full","solo":true,"priority":false},{"slot":1,"label":"Acme server with a long label","state":"pending","sas":"123456","trust":"low","limits":{"max_running":2,"max_queued":4,"cpu_hours_day":10,"max_job_secs":7200,"max_cpus":4,"max_mem_gb":8,"network":"wan","weight":2},"usage":{"running":1,"queued":0,"cpu_secs_today":7200},"expiresAt":{now+86400},"priority":true},{"slot":2,"usage":{"refusal":"over quota"},"priority":false}],"requests":[{"id":"q1","state":"waiting","summary":"Globex asks"}],"note":"two servers"}"#;
    let tenant = r#"{"role":"tenant","node":"linux-1","owner":"Acme","line":"run 1/2","share":{"expires_in_secs":7300}}"#;
    render_check(
        "servers",
        &[owner, tenant, r#"{"role":"tenant","node":"n"}"#],
        false,
    );
}

#[test]
fn status_overview() {
    let script = python_of("compute-jobs.sh", "\ncmd_compute_status() {");
    let base = r#"{"nodes":[{"name":"linux-1","os":"linux","arch":"x86_64","status":{"state":"online","controllerPause":"paused","waiting":{"code":"busy","message":"full"},"unavailable":["gpu"],"parallelMax":4,"running":[{"short":"abcd1234","elapsedSecs":125,"workspace":"/w","argv":"cargo test","tier":"ci","cpuMillis":null,"tierWhy":"critical","submitter":"a"},{"elapsedSecs":4000}],"queued":[{"short":"q1","position":2,"ageSecs":30,"pinned":true,"workspace":"/w","argv":"x","reason":"slot","tier":"batch"},{"pinned":false}],"free":{"cpuMillis":6500,"memBytes":12884901888},"caps":{"cpuMillis":8000,"memBytes":17179869184},"idleCpuMillis":7250,"load":[1.25,0.5,0.0],"disk":{"freeBytes":107374182400,"floorBytes":10737418240,"floorFrom":"default","belowFloor":true}},"mine":{"running":1},"room":{"busy":"servers","running":3,"otherServers":2},"share":{"line":"run 1/2"}},{"name":"mac-1","status":{"disk":{"freeBytes":1}}}],"controllerMachine":{"name":"z","diskFreeBytes":53687091200,"diskTotalBytes":1099511627776,"tmLocalSnapshots":3,"store":{"line":"compute store: 2.0 GB (artifacts 1.5 GB in 3 folders, blobs 512 MB in 2 files) · keep 7 d after a job ends · cap 20.0 GB"}},"notYet":"GPU stats"}"#;
    let other =
        r#"{"nodes":[],"controllerMachine":{"name":"z","tmLocalSnapshotsError":"no tmutil"}}"#;
    // 0.46.17: the store line, empty or with no line.
    let store_empty = r#"{"nodes":[],"controllerMachine":{"name":"z","store":{"line":""}}}"#;
    let store_bare = r#"{"nodes":[],"controllerMachine":{"name":"z","store":{"bytes":1}}}"#;
    check(
        "status",
        &script,
        &[base, other, store_empty, store_bare],
        &[],
        |body, now| {
            let s = status::render(body, now);
            (s.stdout, s.stderr)
        },
    );
}

#[test]
fn text_and_result() {
    let text = "import json,sys; print(\"\\n\".join(json.load(sys.stdin).get(\"text\") or [\"(this server sent no text: k2 update)\"]))";
    let lib = std::fs::read_to_string(common::cli_src().join("compute-lib.sh")).unwrap();
    assert!(lib.contains(text), "_compute_text's Python moved");
    check(
        "text",
        text,
        &[r#"{"text":["a","b é"]}"#, r#"{"text":[]}"#],
        &[],
        |body, _| {
            let s = compute::text_lines(body);
            (s.stdout, s.stderr)
        },
    );
    let result =
        "import json,sys; print(json.dumps(json.load(sys.stdin).get(\"result\"), indent=2))";
    let cmd = std::fs::read_to_string(common::cli_src().join("compute-command.sh")).unwrap();
    assert!(cmd.contains(result), "get --json's Python moved");
    check(
        "result",
        result,
        &[r#"{"result":{"exit":0,"tests":{"passed":1},"list":[1,2.0,"x"]}}"#],
        &[],
        |body, _| {
            let s = compute::result_json(body);
            (s.stdout, s.stderr)
        },
    );
}
