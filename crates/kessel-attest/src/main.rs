//! `kessel-attest`: check build statements offline. `kessel-attest --help`.

use std::path::PathBuf;
use std::process::ExitCode;

use kessel_attest::load;
use kessel_attest::roster::Signed;
use kessel_attest::statement::Isolation;
use kessel_attest::trust::TrustDir;
use kessel_attest::util::parse_rfc3339;
use kessel_attest::verify::{self, Input, Outcome, Policy, Report, Verdict};
use serde_json::json;

const USAGE: &str = "\
USAGE
  kessel-attest verify FILE... --provenance STATEMENT.intoto.jsonl
                       --trust TRUST_DIR --owner-key FP [options]
  kessel-attest verify [FILE...] --dir EVIDENCE_DIR --owner-key FP [options]
  kessel-attest --help | --version
";

const HELP_INTRO: &str = "\
kessel-attest: check a build statement offline, from the build machine
owner's key down to the files you hold. No server, no network.

";

const HELP_BODY: &str = "\
WHAT YOU NEED
  FILE            the file(s) you want to trust, e.g. a release binary.
  --provenance F  the build statement: one DSSE envelope per line
                  (<job label>.intoto.jsonl, e.g. L1.intoto.jsonl), signed by
                  the machine that built it.
  --trust DIR     the trust/ folder that came with it: rosters/, nodes/,
                  statements/, owners.json.
  --dir DIR       instead of --provenance and --trust: an evidence folder
                  (provenance/*.intoto.jsonl and trust/; files fetched into
                  DIR/<job label>/ are checked too).
  --owner-key FP  the owner's roster key fingerprint, 64 hex. Get it from the
                  owner (their website, release notes, a person you trust),
                  NOT from the trust folder: it is what everything else is
                  checked against.

OPTIONS
  --roster F            the owner's current roster ({body, sig} JSON), such as
                        trust/rosters/<FP>.json from a fresh download of the
                        evidence. The one in trust/ is as of the build: a key
                        revoked later still looks active in it. The newest
                        roster given (highest seq) is used.
  --existed-before T    a time by which you know the statement existed
                        (YYYY-MM-DDTHH:MM:SSZ), from a timestamp or log you
                        trust, e.g. the release's own signature date. The tool
                        doesn't check it: it takes your word. It only matters
                        when the signing key is no longer active.
  --as-of T             the same as --existed-before.
  --repo R              require this source repository (github.com/acme/app,
                        a URL, or the statement's exact uri).
  --commit SHA          require this source commit (40 hex).
  --isolation A,B       require one of these isolation levels: job_uid,
                        exclusive, tenant_uid, shared_uid (see ISOLATION).
  --twin F              a second statement for the same build from another
                        machine: it must name the same sha256 for each FILE.
  --json                print the result as one JSON object on stdout. Bad
                        usage or an unreadable input prints
                        {\"error\": \"...\", \"exitCode\": 2} there too.
  -h, --help            this text (also after verify).

WHAT IT CHECKS, in order (it stops at the first that fails)
  roster          the owner's roster verifies with --owner-key
  node key        the statement's signing key is in that roster
  signature       the DSSE signature verifies; the statement is well formed
  owner           the statement names that owner and that key
  key status      the key is not revoked and the roster vouches for it;
                  for a retired key, the job started on or before its retirement
  time            only for a key that is no longer active (see EXIT 3)
  self-statement  the machine's own signed statement: a job wall, no job
                  could read the key, never ran a job without the wall,
                  not a --dev node
  commit, repo, isolation   when asked for
  file            each FILE's sha256 is one the statement names
  twin            with --twin

ISOLATION (how the job was kept apart from other jobs on the machine)
  job_uid     a fresh OS user per job
  exclusive   nothing else ran on the machine
  tenant_uid  one OS user per tenant: that tenant's other jobs could touch it
  shared_uid  no separation between jobs (macOS sandbox, --dev nodes)

EXIT
  0  verified
  1  NOT verified (the failed check is named)
  2  bad usage, or an input can't be read
  3  verified, time not proven: everything holds, but the signing key is no
     longer active in the roster and the statement's times are the build
     machine's own clock, so it could have been signed after the key was
     retired. --existed-before settles it with a time you vouch for, from a
     timestamp or log you trust; the tool doesn't check that time.

SPEC
  The statement is an in-toto Statement v1 with a SLSA Provenance v1
  predicate in a DSSE envelope; a stock DSSE tool can check the signature
  alone with trust/nodes/<keyid>.pem. The owner and roster checks are this
  tool's. Formats:
    https://github.com/Alakazam-211/K2/blob/main/docs/provenance-v1.md
    https://github.com/Alakazam-211/K2/blob/main/docs/ci-formats-v1.md
";

fn print_help() -> ExitCode {
    print!("{HELP_INTRO}{USAGE}\n{HELP_BODY}");
    ExitCode::SUCCESS
}

/// Bad usage or an unreadable input: exit 2, as JSON on stdout with
/// `--json`, else as text on stderr.
fn usage(msg: &str, json: bool) -> ExitCode {
    if json {
        println!("{}", json!({"error": msg, "exitCode": 2}));
    } else {
        eprintln!("kessel-attest: {msg}\nRun kessel-attest --help for usage.");
    }
    ExitCode::from(2)
}

#[derive(Default)]
struct Args {
    files: Vec<PathBuf>,
    provenance: Option<PathBuf>,
    trust: Option<PathBuf>,
    dir: Option<PathBuf>,
    owner_key: Option<String>,
    rosters: Vec<PathBuf>,
    existed_before: Option<String>,
    repo: Option<String>,
    commit: Option<String>,
    isolation: Option<String>,
    twin: Option<PathBuf>,
    json: bool,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let mut val = || -> Result<String, String> {
            i += 1;
            argv.get(i).cloned().filter(|v| !v.is_empty()).ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg {
            "--provenance" => a.provenance = Some(val()?.into()),
            "--trust" => a.trust = Some(val()?.into()),
            "--dir" => a.dir = Some(val()?.into()),
            "--owner-key" => a.owner_key = Some(val()?),
            "--roster" => a.rosters.push(val()?.into()),
            "--existed-before" | "--as-of" => a.existed_before = Some(val()?),
            "--repo" => a.repo = Some(val()?),
            "--commit" => a.commit = Some(val()?),
            "--isolation" => a.isolation = Some(val()?),
            "--twin" => a.twin = Some(val()?.into()),
            "--json" => a.json = true,
            "--" => {
                a.files.extend(argv[i + 1..].iter().map(PathBuf::from));
                break;
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => a.files.push(s.into()),
        }
        i += 1;
    }
    Ok(a)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Options end at `--`; after it every word is a FILE.
    let opts = &argv[..argv.iter().position(|a| a == "--").unwrap_or(argv.len())];
    let json = opts.iter().any(|a| a == "--json");
    match argv.first().map(String::as_str) {
        None => {
            eprint!("kessel-attest: name a command (verify)\n\n{USAGE}\nRun kessel-attest --help for every option.\n");
            return ExitCode::from(2);
        }
        Some("-h" | "--help" | "help") => return print_help(),
        Some("-V" | "--version") => {
            println!("kessel-attest {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("verify") => {}
        Some(other) => return usage(&format!("unknown command {other:?}; the command is verify"), json),
    }
    if opts[1..].iter().any(|a| a == "-h" || a == "--help") {
        return print_help();
    }
    let args = match parse(&argv[1..]) {
        Ok(a) => a,
        Err(e) => return usage(&e, json),
    };
    match build_input(&args) {
        Ok(input) => {
            let report = verify::verify(&input);
            if args.json {
                println!("{}", to_json(&report));
            } else {
                print!("{}", to_text(&report, &input));
            }
            ExitCode::from(report.verdict.exit_code() as u8)
        }
        Err(e) => usage(&e, json),
    }
}

fn build_input(a: &Args) -> Result<Input, String> {
    let owner_fp = a.owner_key.clone().ok_or("--owner-key FP is required: the owner's roster key fingerprint, got from the owner (not from the trust folder)")?;
    let mut files = Vec::new();
    for f in &a.files {
        files.push(load::file_arg(f)?);
    }
    let (statements, trust) = match (&a.dir, &a.provenance) {
        (Some(_), Some(_)) => return Err("give --dir or --provenance, not both".into()),
        (Some(dir), None) => {
            if a.trust.is_some() {
                return Err("--dir reads DIR/trust; leave out --trust".into());
            }
            let ev = load::evidence_dir(dir)?;
            files.extend(ev.fetched);
            (ev.statements, Some(ev.trust))
        }
        (None, Some(p)) => {
            let t = a.trust.as_deref().ok_or("--trust DIR is required with --provenance: the trust/ folder that came with the statement")?;
            (load::envelopes(p)?, Some(TrustDir::open(t)?))
        }
        (None, None) => return Err("name the statement: --provenance FILE.intoto.jsonl (with --trust DIR), or --dir EVIDENCE_DIR".into()),
    };
    if files.is_empty() {
        return Err("name the file(s) to check: kessel-attest verify FILE... (or fetch the job files into the evidence folder)".into());
    }
    let twin = match &a.twin {
        Some(p) => {
            let mut t = load::envelopes(p)?;
            if t.len() != 1 {
                return Err(format!("{}: a twin is one statement", p.display()));
            }
            t.pop()
        }
        None => None,
    };
    let mut rosters = Vec::new();
    for r in &a.rosters {
        let text = std::fs::read_to_string(r).map_err(|e| format!("{}: {e}", r.display()))?;
        rosters.push((r.display().to_string(), Signed::from_json(&r.display().to_string(), &text)?));
    }
    let existed_before = match &a.existed_before {
        Some(t) => Some(parse_rfc3339(t).ok_or_else(|| format!("--existed-before {t:?}: give a time as YYYY-MM-DDTHH:MM:SSZ"))?),
        None => None,
    };
    let isolation = match &a.isolation {
        Some(list) => list
            .split(',')
            .map(|s| Isolation::parse(s.trim()).ok_or_else(|| format!("--isolation: {s:?} is not one of job_uid, exclusive, tenant_uid, shared_uid")))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).map_err(|e| format!("the clock: {e}"))?;
    Ok(Input {
        statements,
        twin,
        files,
        trust,
        rosters,
        policy: Policy { owner_fp, existed_before, repo: a.repo.clone(), commit: a.commit.clone(), isolation },
        now,
    })
}

fn to_text(r: &Report, input: &Input) -> String {
    let mut out = String::new();
    let what: Vec<&str> = input.files.iter().map(|f| f.path.as_str()).collect();
    match (r.verdict, r.failed()) {
        (Verdict::NotVerified, Some(c)) => out.push_str(&format!("NOT verified: {}: {}: {}\n", c.subject, c.rule, c.detail)),
        (v, _) => out.push_str(&format!("{}: {}\n", v.text(), what.join(", "))),
    }
    for c in &r.checks {
        out.push_str(&format!("  {:<9} {:<16} {:<15} {}\n", c.outcome.text(), c.subject, c.rule, c.detail));
    }
    if r.verdict != Verdict::NotVerified {
        for s in &r.statements {
            out.push_str(&format!("{}: built by node {} (host {}) of {}", s.label, s.node, s.host, s.owner));
            match &s.source {
                Some((uri, commit)) => out.push_str(&format!(" from {uri} at {commit}\n")),
                None => out.push_str(" (no source checkout)\n"),
            }
            out.push_str(&format!("  command: {}\n", s.argv.join(" ")));
            out.push_str(&format!(
                "  ran {} to {} (the node's clock), isolation {}, wall {}, network {}\n",
                s.started_on, s.finished_on, s.isolation, s.wall, s.network
            ));
        }
    }
    for w in &r.warnings {
        out.push_str(&format!("warning: {w}\n"));
    }
    out
}

fn to_json(r: &Report) -> String {
    let checks: Vec<_> = r
        .checks
        .iter()
        .map(|c| json!({"subject": c.subject, "rule": c.rule, "outcome": match c.outcome { Outcome::Pass => "pass", Outcome::Fail => "fail", Outcome::Unproven => "unproven" }, "detail": c.detail}))
        .collect();
    let statements: Vec<_> = r
        .statements
        .iter()
        .map(|s| {
            json!({
                "label": s.label, "owner": s.owner, "node": s.node, "host": s.host, "nodeKey": s.node_fp, "keyStatus": s.key_status,
                "source": s.source.as_ref().map(|(u, c)| json!({"uri": u, "commit": c})),
                "argv": s.argv, "isolation": s.isolation, "wall": s.wall, "network": s.network,
                "startedOn": s.started_on, "finishedOn": s.finished_on, "timeProven": s.time_proven,
                "subjects": s.subjects.iter().map(|(n, d)| json!({"name": n, "sha256": d})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "verdict": r.verdict.key(),
        "exitCode": r.verdict.exit_code(),
        "failed": r.failed().map(|c| json!({"subject": c.subject, "rule": c.rule, "detail": c.detail})),
        "roster": r.roster.as_ref().map(|(o, seq, at)| json!({"owner": o, "seq": seq, "issuedAt": kessel_attest::util::rfc3339(*at).ok()})),
        "checks": checks,
        "statements": statements,
        "warnings": r.warnings,
    })
    .to_string()
}
