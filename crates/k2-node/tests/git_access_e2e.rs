//! Compute git access on the node (protocol 8, prd-compute-git-access-v1,
//! GH#78) against a loopback fake controller: a `--dev` node (no wall; its
//! helper checks the peer's uid, `git_access.rs`), real git, a fake token.
//!
//! - The job's own `git credential fill` reaches the node's helper over
//!   git's `credential-cache` protocol; the node asks the fake server
//!   (`git_cred_need`), opens the sealed answer and hands it to git.
//! - A repo the plan doesn't name gets nothing, and no ask goes out.
//! - The job keeps its `--cache` (a `--secret` job may not have one).
//! - The token is redacted from the job's log and is in no file the node
//!   keeps (journal, ledger, caches, the job's tree).
//! - A refusal from the server reaches the job's log with its code.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::path::PathBuf;

use common::*;
use k2_node_proto::frames::{self, Frame, GitCred, GitCredRefused, JobState, LogStream};
use k2_node_proto::gitcred::{self, Cred, FEATURE_GIT_ACCESS};

const TOKEN: &str = "ghs_k2testE2EGITACCESS0123456789abcdef";

/// A short home under /tmp: the helper's socket path must stay under 100
/// bytes (macOS's temp dir alone is about 50).
fn short_dir(tag: &str) -> PathBuf {
    let id = k2_node_proto::crypto::random_id().unwrap();
    let d = PathBuf::from("/tmp").join(format!("k2g-{tag}-{}", &id[..8]));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

/// A job that asks git for two repos' credentials and reports its cache.
fn asker(job: &str) -> frames::Plan {
    let script = "printf 'protocol=https\\nhost=github.com\\npath=acme/app.git\\n\\n' | git credential fill; \
                  printf 'protocol=https\\nhost=github.com\\npath=acme/other.git\\n\\n' | git credential fill 2>/dev/null || echo other-refused; \
                  echo cache=${K2_JOB_CACHE:-none}";
    let mut p = plan(job, &["/bin/sh", "-c", script]);
    p.git_access = vec!["github.com/acme/app".into()];
    p.cache = Some("build".into());
    p.env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    p
}

fn files_with(dir: &std::path::Path, needle: &[u8], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => files_with(&p, needle, out),
            Ok(t) if t.is_file() => {
                if std::fs::read(&p).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle)) {
                    out.push(p);
                }
            }
            _ => {}
        }
    }
}

/// Run `p` to its receipt, answering each `git_cred_need` with `answer`.
async fn run_answering(c: &mut Conn, p: frames::Plan, answer: impl Fn(&frames::GitCredNeed) -> Frame) -> (Vec<Frame>, Vec<frames::GitCredNeed>) {
    let job = p.job_id.clone();
    c.send(Frame::Assign(assign(p, 1)));
    let mut seen = Vec::new();
    let mut needs = Vec::new();
    loop {
        let f = c.next().await;
        if let Frame::GitCredNeed(n) = &f {
            needs.push(n.clone());
            c.send(answer(n));
        }
        let done = matches!(&f, Frame::Receipt(r) if r.job_id == job);
        seen.push(f);
        if done {
            return (seen, needs);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_gets_a_granted_repos_token_through_gits_helper_and_keeps_its_cache() {
    let mut fake = Fake::start().await;
    let n = start_node_in(&fake, short_dir("ok"), "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let offer = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = offer.last() else { unreachable!() };
    assert!(o.features.iter().any(|f| f == FEATURE_GIT_ACCESS), "a protocol-8 server is offered git access: {:?}", o.features);

    let (cfp, nfp) = (fake.key.fingerprint(), n.node.key.fingerprint());
    let (fr, needs) = run_answering(&mut c, asker("g1"), |need| {
        let b = gitcred::Binding {
            controller_fp: cfp.clone(),
            node_fp: nfp.clone(),
            job_id: need.job_id.clone(),
            generation: need.generation,
            attempt: need.attempt,
            plan_digest: need.plan_digest.clone(),
            req: need.req,
            repo: need.repo.clone(),
            purpose: need.purpose,
        };
        let cred = Cred { username: gitcred::USERNAME.into(), password: TOKEN.into(), expires_at: 4_000_000_000 };
        let (epk, ct) = gitcred::seal(&cred, &need.epk, &b).expect("seal");
        Frame::GitCred(GitCred { job_id: need.job_id.clone(), generation: need.generation, req: need.req, epk, ct })
    })
    .await;
    let logs = logs_of(&fr, "g1");
    let (out, sys) = (text(&logs, LogStream::Out), text(&logs, LogStream::Sys));
    assert_eq!(states_of(&fr, "g1").last().unwrap().state, JobState::Done, "out: {out}\nsys: {sys}");
    // One ask, for the granted repo, from inside the job.
    assert_eq!(needs.len(), 1, "{needs:?}");
    assert_eq!((needs[0].repo.as_str(), needs[0].purpose), ("github.com/acme/app", gitcred::Purpose::Job));
    // git got it (as x-access-token) and printed it: the log shows the
    // redaction, never the token.
    assert!(out.contains("username=x-access-token"), "{out}");
    assert!(out.contains("password=[k2v:github.com/acme/app]"), "{out}");
    assert!(!out.contains(TOKEN) && !sys.contains(TOKEN), "the token reached the log");
    // The ungranted repo: nothing for git, a line naming the command.
    assert!(out.contains("other-refused"), "{out}");
    assert!(sys.contains("github.com/acme/other isn't granted") && sys.contains("k2 compute git-access grant --github acme/other"), "{sys}");
    assert!(sys.contains("git access: acme/app"), "{sys}");
    // The job kept its cache (a --secret job is refused one).
    assert!(out.contains("cache=/") && !out.contains("cache=none"), "{out}");
    assert!(!sys.contains("keeps nothing between runs"), "{sys}");
    // Nothing the node keeps holds the token: journal, ledger, caches,
    // the job's tree, its spool files.
    let mut hits = Vec::new();
    files_with(&n.dir, TOKEN.as_bytes(), &mut hits);
    assert!(hits.is_empty(), "the token was written to {hits:?}");
    // The helper's socket went with the job.
    let mut socks = Vec::new();
    fn find(d: &std::path::Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == ".k2-git.sock") {
                out.push(p.clone());
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                find(&p, out);
            }
        }
    }
    find(&n.dir, &mut socks);
    assert!(socks.is_empty(), "the helper's socket outlived the job: {socks:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_reaches_the_jobs_log_and_git_gets_nothing() {
    let mut fake = Fake::start().await;
    let _n = start_node_in(&fake, short_dir("no"), "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let (fr, needs) = run_answering(&mut c, asker("g2"), |need| {
        Frame::GitCredRefused(GitCredRefused {
            job_id: need.job_id.clone(),
            generation: need.generation,
            req: need.req,
            code: gitcred::codes::NOT_GRANTED.into(),
            message: "acme/app was revoked for this workspace".into(),
        })
    })
    .await;
    let logs = logs_of(&fr, "g2");
    let (out, sys) = (text(&logs, LogStream::Out), text(&logs, LogStream::Sys));
    assert_eq!(needs.len(), 1, "{needs:?}");
    assert!(!out.contains("password="), "git got a password: {out}");
    assert!(sys.contains("git_access_not_granted") && sys.contains("revoked"), "{sys}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_cant_make_up_a_refusal_code_or_open_with_another_binding() {
    let mut fake = Fake::start().await;
    let n = start_node_in(&fake, short_dir("bad"), "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let (cfp, nfp) = (fake.key.fingerprint(), n.node.key.fingerprint());
    // Sealed for another repo: the node must not open it as acme/app.
    let (fr, needs) = run_answering(&mut c, asker("g3"), |need| {
        let b = gitcred::Binding {
            controller_fp: cfp.clone(),
            node_fp: nfp.clone(),
            job_id: need.job_id.clone(),
            generation: need.generation,
            attempt: need.attempt,
            plan_digest: need.plan_digest.clone(),
            req: need.req,
            repo: "github.com/acme/other".into(),
            purpose: need.purpose,
        };
        let cred = Cred { username: gitcred::USERNAME.into(), password: TOKEN.into(), expires_at: 4_000_000_000 };
        let (epk, ct) = gitcred::seal(&cred, &need.epk, &b).expect("seal");
        Frame::GitCred(GitCred { job_id: need.job_id.clone(), generation: need.generation, req: need.req, epk, ct })
    })
    .await;
    let logs = logs_of(&fr, "g3");
    let (out, sys) = (text(&logs, LogStream::Out), text(&logs, LogStream::Sys));
    assert_eq!(needs.len(), 1);
    assert!(!out.contains("password="), "git got a password from a mis-bound seal: {out}");
    assert!(sys.contains("git_access_stale") && sys.contains("doesn't open"), "{sys}");
}
