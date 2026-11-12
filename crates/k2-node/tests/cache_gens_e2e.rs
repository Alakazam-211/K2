//! Cache generations end to end through a node (prd-compute-blacksmith-
//! class-v1 §4.1, §10, slice B1): a fake protocol-10 server sends plans
//! with `cache_commit`, the node syncs real git commits, forks, runs the
//! job and commits or discards its fork. The node is a debug test node
//! (no root; its wall exemption, `dev_test_exemption`, never applies to a
//! release binary or an installed node).
//!
//!   cargo test -p k2-node --test cache_gens_e2e
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only.
#![cfg(unix)]

mod common;

use common::*;
use k2_node_proto::crypto;
use k2_node_proto::frames::{Frame, JobState, LogStream, Src, Transfer, TransferKind};
use k2_node_proto::gens::{CacheCommit, CacheResult};

const PK: &str = "0123456789abcdef";

fn git_out(repo: &std::path::Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// A repo with two commits (`first` is the parent of `second`).
fn temp_repo() -> (std::path::PathBuf, String, String) {
    let d = k2_node::util::temp_dir("gens-repo").join("repo");
    std::fs::create_dir_all(&d).unwrap();
    git_out(&d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\n").unwrap();
    git_out(&d, &["add", "."]);
    git_out(&d, &["commit", "-q", "-m", "one"]);
    let first = git_out(&d, &["rev-parse", "HEAD"]);
    std::fs::write(d.join("a.txt"), "two\n").unwrap();
    git_out(&d, &["add", "."]);
    git_out(&d, &["commit", "-q", "-m", "two"]);
    let second = git_out(&d, &["rev-parse", "HEAD"]);
    (d, first, second)
}

/// The controller's side of a bundle: temp ref + `git bundle create`.
fn bundle_for(repo: &std::path::Path, job: &str, sha: &str, out: &std::path::Path) -> Vec<u8> {
    let r = format!("refs/k2-compute/{job}");
    git_out(repo, &["update-ref", &r, sha]);
    git_out(repo, &["bundle", "create", out.to_str().unwrap(), &r]);
    git_out(repo, &["update-ref", "-d", &r]);
    std::fs::read(out).unwrap()
}

fn send_blob(c: &Conn, job: &str, data: &[u8]) {
    let sha = crypto::sha256_hex(data);
    c.send(Frame::Transfer(Transfer {
        job_id: job.into(),
        generation: 1,
        kind: TransferKind::Bundle,
        seq: 0,
        data: crypto::b64(data),
        last: true,
        total_bytes: data.len() as u64,
        sha256: sha,
    }));
}

struct Ran {
    out: String,
    sys: String,
    state: JobState,
    warmth: Option<String>,
    cache: Option<CacheResult>,
}

/// Run one job at `commit` with the plan's cache fields; send the bundle
/// when the node asks for it.
async fn run(c: &mut Conn, n: &TestNode, repo: &std::path::Path, job: &str, commit: &str, cc: CacheCommit, cref: Option<&str>, script: &str) -> Ran {
    let mut p = plan(job, &["/bin/sh", "-c", script]);
    p.src = Some(Src { project_key: PK.into(), remote_url: None, commit: commit.into(), dirty: None, slots: 1 });
    p.cache_commit = Some(cc);
    p.cache_ref = cref.map(str::to_string);
    c.send(Frame::Assign(assign(p, 1)));
    let fr = c.until(|f| matches!(f, Frame::Need(x) if x.job_id == job) || matches!(f, Frame::Receipt(r) if r.job_id == job)).await;
    let fr = if fr.iter().any(|f| matches!(f, Frame::Need(_))) {
        let b = bundle_for(repo, job, commit, &n.dir.join(format!("{job}.bundle")));
        send_blob(c, job, &b);
        run_to_receipt(c, job).await
    } else {
        fr
    };
    let logs = logs_of(&fr, job);
    let states = states_of(&fr, job);
    let last = states.iter().rev().find(|s| s.state.is_terminal()).expect("a terminal state").clone();
    Ran {
        out: text(&logs, LogStream::Out),
        sys: text(&logs, LogStream::Sys),
        state: last.state,
        warmth: states.iter().rev().find_map(|s| s.src.clone()).and_then(|s| s.warmth),
        cache: last.outputs.and_then(|o| o.cache),
    }
}

fn head_seq(n: &TestNode, lineage: &str) -> u64 {
    let set = k2_node::gens::SetId::target(&n.layout.home, PK).unwrap();
    k2_node::gens::read_head(&set.lineage_dir(lineage).unwrap()).unwrap().map(|h| h.seq).unwrap_or(0)
}

const BUILD_GOOD: &str = r#"printf good > "$CARGO_TARGET_DIR/app"; mkdir -p "$CARGO_HOME/registry/src/x"; printf orig > "$CARGO_HOME/registry/src/x/lib.rs"; echo built"#;
const PLANT: &str = r#"cat "$CARGO_TARGET_DIR/app"; echo; printf planted > "$CARGO_TARGET_DIR/poison"; printf planted > "$CARGO_HOME/registry/src/x/lib.rs"; echo planted"#;
const LOOK: &str = r#"if [ -e "$CARGO_TARGET_DIR/poison" ]; then echo POISON; else echo clean; fi; cat "$CARGO_HOME/registry/src/x/lib.rs"; echo"#;

/// §10, through a node: a PR job's writes to `target/` and `CARGO_HOME`
/// never reach the next trusted check; a failed trusted check and an
/// older one commit nothing; a plain run at main saves to `scratch/main`.
#[tokio::test(flavor = "multi_thread")]
async fn trusted_checks_never_see_a_pr_jobs_writes() {
    let mut fake = Fake::start().await;
    fake.server_protocol.store(k2_node_proto::frames::PROTOCOL_GENS, std::sync::atomic::Ordering::SeqCst);
    let n = start_node(&fake, "gens-e2e", "", |o| o.cache_gens = Some(k2_node::gens::SwitchFile { on: true, lineage_max_gb: None })).await;
    let (repo, first, second) = temp_repo();
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let owner = n.node.owner();
    assert!(n.node.session_features(&owner).iter().any(|f| f == "cache-gens"), "a protocol-10 server is offered cache-gens");
    let g = k2_node::gens::gens_root(&n.layout.home);
    k2_node::gens::is_closed(&g).unwrap();

    let t1 = run(&mut c, &n, &repo, "t1", &second, CacheCommit::Trusted, None, BUILD_GOOD).await;
    assert_eq!(t1.state, JobState::Done, "{}", t1.sys);
    assert_eq!(t1.warmth.as_deref(), Some("cold"), "nothing to fork yet");
    let cr = t1.cache.expect("Outputs.cache");
    assert!(cr.committed && cr.commit_lineage.as_deref() == Some("trusted"), "{cr:?}");
    assert!(t1.sys.contains("target dir: gen cold"), "{}", t1.sys);

    let pr = run(&mut c, &n, &repo, "pr1", &second, CacheCommit::Scratch, Some("pr-1"), PLANT).await;
    assert!(pr.out.starts_with("good"), "the PR job starts from trusted #1: {}", pr.out);
    assert_eq!(pr.warmth.as_deref(), Some("gen"));
    assert_eq!(pr.cache.as_ref().and_then(|c| c.commit_lineage.as_deref()), Some("scratch/pr-1"), "{:?}", pr.cache);
    assert!(pr.sys.contains("gen trusted #1"), "{}", pr.sys);

    let t2 = run(&mut c, &n, &repo, "t2", &second, CacheCommit::Trusted, None, LOOK).await;
    assert_eq!(t2.out, "clean\norig\n", "a trusted check never sees the PR's target or CARGO_HOME writes");
    assert_eq!(t2.warmth.as_deref(), Some("gen"));
    assert_eq!(head_seq(&n, "trusted"), 2);

    let pr2 = run(&mut c, &n, &repo, "pr2", &second, CacheCommit::Scratch, Some("pr-1"), LOOK).await;
    assert_eq!(pr2.out, "POISON\nplanted\n", "only the same PR sees its own scratch copy");

    let bad = run(&mut c, &n, &repo, "bad", &second, CacheCommit::Trusted, None, "exit 101").await;
    assert_eq!(bad.state, JobState::Done, "a non-zero exit is a done job with its exit code");
    let cr = bad.cache.expect("Outputs.cache");
    assert!(!cr.committed && cr.why.contains("exit 101"), "{cr:?}");
    assert_eq!(head_seq(&n, "trusted"), 2, "a failed trusted check moves nothing");

    let old = run(&mut c, &n, &repo, "old", &first, CacheCommit::Trusted, None, "true").await;
    let cr = old.cache.expect("Outputs.cache");
    assert!(!cr.committed && cr.why.contains("older than the head"), "{cr:?}");
    assert_eq!(head_seq(&n, "trusted"), 2);

    let main = run(&mut c, &n, &repo, "plain", &second, CacheCommit::Scratch, Some("main"), PLANT).await;
    assert_eq!(main.cache.and_then(|c| c.commit_lineage), Some("scratch/main".to_string()), "a plain run at main never saves trusted");
    assert_eq!(head_seq(&n, "trusted"), 2);

    // Every fork is gone or became a generation; no record is left.
    let forks = k2_node::gens::forks_root(&n.layout.home);
    for _ in 0..100 {
        if std::fs::read_dir(&forks).map(|d| d.count()).unwrap_or(0) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read_dir(&forks).map(|d| d.count()).unwrap(), 0, "no fork left behind");
}

/// With the switch off (the default), a plan that carries `cache_commit`
/// runs on today's warm slot, and its result has no `cache`.
#[tokio::test(flavor = "multi_thread")]
async fn with_the_switch_off_jobs_run_as_today() {
    let mut fake = Fake::start().await;
    fake.server_protocol.store(k2_node_proto::frames::PROTOCOL_GENS, std::sync::atomic::Ordering::SeqCst);
    let n = start_node(&fake, "gens-e2e-off", "", |_| {}).await;
    let (repo, _first, second) = temp_repo();
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    assert!(!n.node.session_features(&n.node.owner()).iter().any(|f| f == "cache-gens"));
    let r = run(&mut c, &n, &repo, "o1", &second, CacheCommit::Trusted, None, BUILD_GOOD).await;
    assert_eq!(r.state, JobState::Done);
    assert_eq!(r.warmth.as_deref(), Some("cold"));
    assert!(r.sys.contains("target dir: slot 1"), "today's warm slot: {}", r.sys);
    assert!(r.cache.is_none());
    assert!(!k2_node::gens::gens_root(&n.layout.home).exists(), "no gens root is made");
}
