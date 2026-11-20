//! The rule set: from a pinned owner key to a verdict on the files you hold.
//!
//! In order, stopping at the first rule that fails:
//!
//! 1. **roster**: the owner's roster verifies with the pinned roster key
//!    (the newest one given, by `seq`; two different rosters with one `seq`
//!    are refused).
//! 2. For each statement:
//!    - **node key**: the envelope's key is in that roster; a node PEM in
//!      `trust/` must be the same key;
//!    - **signature**: the DSSE signature verifies with the roster's key,
//!      and the payload is a well-formed statement in its canonical form;
//!    - **owner**: `builder.id` names the pinned owner (`…/owner/<fp>`), and
//!      the statement names the key that signed it, at the roster's epoch;
//!    - **key status**: not revoked; started on or after `valid_from`; a
//!      retired key's statement started on or before `valid_to`; the roster
//!      vouches for the key (`taint_basis`);
//!    - **time** (only for a key not active in the roster): every time in a
//!      statement is the node's own clock, so a retired key could sign a
//!      back-dated one. Settled only by [`Policy::existed_before`] on or
//!      before `valid_to`, a time the caller vouches for (not checked here);
//!      otherwise the verdict is [`Verdict::TimeNotProven`];
//!    - **self-statement**: the node's own statement (from `trust/`)
//!      verifies with its key: no `--dev` node, no job could read the key,
//!      a wall that matches the build's, not tainted;
//!    - **policy**: the repository, commit and isolation asked for.
//! 3. **files**: each file's sha256 is a subject of a verified statement.
//! 4. **twin**: a second statement from another machine (another roster
//!    `host`) for the same commit names the same digest for every file.
//!
//! Warnings (they don't change the verdict): a roster issued before the
//! build (a key revoked after it would still look active: fetch the owner's
//! current roster), a roster older than [`ROSTER_OLD_DAYS`].

use crate::dsse::{Envelope, NodeKey};
use crate::node_statement::{NodeStatement, Taint};
use crate::roster::{KeyStatus, NodeRoster, RosterEntry, Signed, TaintBasis};
use crate::statement::{names, Isolation, Owner, Statement, Wall};
use crate::trust::TrustDir;
use crate::util::{is_fp, is_git_hex, rfc3339};

/// A roster this many days old gets a warning.
pub const ROSTER_OLD_DAYS: i64 = 30;
/// A roster issued this far in the future (the owner's clock ahead of
/// this machine's) gets a warning.
pub const FUTURE_SLACK_SECS: i64 = 3600;

/// The outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every rule passed.
    Verified,
    /// Every rule passed, but a statement was signed by a key that is not
    /// active in the roster, and nothing proves when it was signed.
    TimeNotProven,
    /// A rule failed.
    NotVerified,
}

impl Verdict {
    /// The process exit code: 0, 3 (time not proven) or 1.
    pub fn exit_code(self) -> i32 {
        match self {
            Verdict::Verified => 0,
            Verdict::TimeNotProven => 3,
            Verdict::NotVerified => 1,
        }
    }

    /// `verified`, `verified, time not proven`, `NOT verified`.
    pub fn text(self) -> &'static str {
        match self {
            Verdict::Verified => "verified",
            Verdict::TimeNotProven => "verified, time not proven",
            Verdict::NotVerified => "NOT verified",
        }
    }

    /// For `--json`: `verified`, `time_not_proven`, `not_verified`.
    pub fn key(self) -> &'static str {
        match self {
            Verdict::Verified => "verified",
            Verdict::TimeNotProven => "time_not_proven",
            Verdict::NotVerified => "not_verified",
        }
    }
}

/// What the caller requires beyond the chain.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// The owner's roster key fingerprint, got out of band (64 hex).
    pub owner_fp: String,
    /// A time (unix seconds) by which the caller knows the statement
    /// existed, from a timestamp or log the caller trusts, such as a
    /// release's own signature date. Not checked: the verifier takes the
    /// caller's word. Settles the time of a statement from a key that is no
    /// longer active.
    pub existed_before: Option<i64>,
    /// The source repository (`github.com/acme/app`, a URL, or the exact uri).
    pub repo: Option<String>,
    /// The source commit (40 hex).
    pub commit: Option<String>,
    /// Isolation levels accepted; empty = any.
    pub isolation: Vec<Isolation>,
}

/// A file to check against the statements.
#[derive(Debug, Clone)]
pub struct FileArg {
    /// How to name it in the report.
    pub path: String,
    /// Its SHA-256, 64 hex.
    pub sha256: String,
    /// Set for a file fetched beside an evidence bundle: the statement
    /// label and the subject name it must be.
    pub expect: Option<(String, String)>,
}

/// Everything [`verify`] reads.
#[derive(Debug, Clone)]
pub struct Input {
    /// `(label, envelope)`: the statements to check.
    pub statements: Vec<(String, Envelope)>,
    /// A second statement for the same build from another machine.
    pub twin: Option<(String, Envelope)>,
    /// The files to check.
    pub files: Vec<FileArg>,
    /// The `trust/` folder, if any.
    pub trust: Option<TrustDir>,
    /// More rosters (`--roster`), `(where from, document)`.
    pub rosters: Vec<(String, Signed)>,
    /// The requirements.
    pub policy: Policy,
    /// This machine's clock, unix seconds.
    pub now: i64,
}

/// One rule's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What it was about: `roster`, a statement label, a file.
    pub subject: String,
    /// The rule.
    pub rule: &'static str,
    /// How it came out.
    pub outcome: Outcome,
    /// Plain words.
    pub detail: String,
}

/// How one rule came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It holds.
    Pass,
    /// It doesn't: the verdict is [`Verdict::NotVerified`].
    Fail,
    /// The time rule couldn't be proven: the verdict is at best
    /// [`Verdict::TimeNotProven`].
    Unproven,
}

impl Outcome {
    /// `ok`, `FAILED`, `unproven`.
    pub fn text(self) -> &'static str {
        match self {
            Outcome::Pass => "ok",
            Outcome::Fail => "FAILED",
            Outcome::Unproven => "unproven",
        }
    }
}

/// What a verified statement says, for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The statement's label (its file name).
    pub label: String,
    /// The owner (roster `owner_id`).
    pub owner: String,
    /// The node's roster name.
    pub node: String,
    /// The node's roster `host` label.
    pub host: String,
    /// The node key's fingerprint.
    pub node_fp: String,
    /// `active`, `retired` (from the roster).
    pub key_status: &'static str,
    /// Source uri and commit, if the job had source.
    pub source: Option<(String, String)>,
    /// The command.
    pub argv: Vec<String>,
    /// The isolation, wall and network.
    pub isolation: &'static str,
    /// Wall.
    pub wall: &'static str,
    /// Network.
    pub network: &'static str,
    /// Node clock.
    pub started_on: String,
    /// Node clock.
    pub finished_on: String,
    /// The time rule held (active key, or proven).
    pub time_proven: bool,
    /// `(name, sha256)` of every subject.
    pub subjects: Vec<(String, String)>,
}

/// The whole result.
#[derive(Debug, Clone)]
pub struct Report {
    /// The verdict.
    pub verdict: Verdict,
    /// Every rule run, in order; the last one failed when the verdict is
    /// [`Verdict::NotVerified`].
    pub checks: Vec<Check>,
    /// Things to know that don't change the verdict.
    pub warnings: Vec<String>,
    /// The roster used: owner, seq, issued at.
    pub roster: Option<(String, u64, i64)>,
    /// One per verified statement (and the twin).
    pub statements: Vec<Summary>,
}

impl Report {
    /// The rule that failed, if any.
    pub fn failed(&self) -> Option<&Check> {
        self.checks.iter().find(|c| c.outcome == Outcome::Fail)
    }
}

struct Run {
    checks: Vec<Check>,
    warnings: Vec<String>,
}

struct Stop;

impl Run {
    fn pass(&mut self, subject: &str, rule: &'static str, detail: impl Into<String>) {
        self.checks.push(Check { subject: subject.into(), rule, outcome: Outcome::Pass, detail: detail.into() });
    }
    fn unproven(&mut self, subject: &str, rule: &'static str, detail: impl Into<String>) {
        self.checks.push(Check { subject: subject.into(), rule, outcome: Outcome::Unproven, detail: detail.into() });
    }
    fn fail(&mut self, subject: &str, rule: &'static str, detail: impl Into<String>) -> Stop {
        self.checks.push(Check { subject: subject.into(), rule, outcome: Outcome::Fail, detail: detail.into() });
        Stop
    }
}

fn short(fp: &str) -> &str {
    fp.get(..16).unwrap_or(fp)
}

fn when(t: i64) -> String {
    rfc3339(t).unwrap_or_else(|_| t.to_string())
}

/// Run every rule. Never panics on bad input: every problem is a failed
/// check.
pub fn verify(input: &Input) -> Report {
    let mut run = Run { checks: Vec::new(), warnings: Vec::new() };
    let mut out = Report { verdict: Verdict::NotVerified, checks: Vec::new(), warnings: Vec::new(), roster: None, statements: Vec::new() };
    let result = verify_inner(input, &mut run, &mut out);
    out.checks = run.checks;
    out.warnings = run.warnings;
    out.verdict = match result {
        Err(Stop) => Verdict::NotVerified,
        Ok(()) if out.statements.iter().all(|s| s.time_proven) => Verdict::Verified,
        Ok(()) => Verdict::TimeNotProven,
    };
    out
}

fn verify_inner(input: &Input, run: &mut Run, out: &mut Report) -> Result<(), Stop> {
    let p = &input.policy;
    if !is_fp(&p.owner_fp) {
        return Err(run.fail("owner key", "pin", format!("{:?} is not an owner key fingerprint (64 lowercase hex)", p.owner_fp)));
    }
    if p.commit.as_deref().is_some_and(|c| !is_git_hex(c)) {
        return Err(run.fail("policy", "commit", "a commit is 40 lowercase hex"));
    }
    if input.statements.is_empty() {
        return Err(run.fail("statements", "input", "no build statement to check"));
    }

    let roster = pick_roster(input, run)?;
    out.roster = Some((roster.owner_id.clone(), roster.seq, roster.issued_at));

    let mut verified: Vec<(String, Statement)> = Vec::new();
    for (label, env) in &input.statements {
        let (st, summary) = check_statement(label, env, &roster, input, run)?;
        out.statements.push(summary);
        verified.push((label.clone(), st));
    }

    // Warnings about the roster's age, once every statement is known.
    let newest_build = verified.iter().map(|(_, s)| s.finished()).max().unwrap_or(i64::MIN);
    if roster.issued_at < newest_build {
        run.warnings.push(format!(
            "the roster (seq {}, issued {}) is older than the build ({}): a key retired or revoked after {} would still look active here. Get the owner's current roster and pass it with --roster",
            roster.seq,
            when(roster.issued_at),
            when(newest_build),
            when(roster.issued_at)
        ));
    }
    if input.now - roster.issued_at > ROSTER_OLD_DAYS * 86_400 {
        run.warnings.push(format!(
            "the roster (seq {}) was issued {}, over {ROSTER_OLD_DAYS} days ago: get the owner's current roster and pass it with --roster",
            roster.seq,
            when(roster.issued_at)
        ));
    }
    if roster.issued_at > input.now + FUTURE_SLACK_SECS {
        run.warnings.push(format!("the roster says it was issued {}, later than this machine's clock", when(roster.issued_at)));
    }

    // Files.
    let mut matched: Vec<(String, String)> = Vec::new(); // (subject name, sha256) per file, for the twin
    for f in &input.files {
        let hits: Vec<(&str, &str)> = verified
            .iter()
            .flat_map(|(label, st)| st.subject.iter().map(move |s| (label.as_str(), s)))
            .filter(|(_, s)| s.sha256() == Some(f.sha256.as_str()))
            .map(|(label, s)| (label, s.name.as_deref().unwrap_or("")))
            .collect();
        match &f.expect {
            Some((label, name)) => {
                if !hits.iter().any(|(l, n)| l == label && n == name) {
                    return Err(run.fail(&f.path, "file", format!("sha256 {} is not what statement {label} says {name} is", f.sha256)));
                }
                run.pass(&f.path, "file", format!("sha256 {}… is {label}'s subject {name}", short(&f.sha256)));
                matched.push((name.clone(), f.sha256.clone()));
            }
            None => {
                let Some((label, name)) = hits.first() else {
                    let names: Vec<String> = verified.iter().flat_map(|(_, st)| st.subject.iter().filter_map(|s| s.name.clone())).collect();
                    return Err(run.fail(
                        &f.path,
                        "file",
                        format!("its sha256 {} is not a subject of the statement (it names: {})", f.sha256, names.join(", ")),
                    ));
                };
                let lbl = if verified.len() > 1 { format!("{label}'s ") } else { String::new() };
                run.pass(&f.path, "file", format!("sha256 {}… is {lbl}subject {name}", short(&f.sha256)));
                matched.push((name.to_string(), f.sha256.clone()));
            }
        }
    }

    // Twin.
    if let Some((tlabel, tenv)) = &input.twin {
        let [(plabel, primary)] = verified.as_slice() else {
            return Err(run.fail(tlabel, "twin", "a twin pairs with exactly one statement"));
        };
        let (twin, summary) = check_statement(tlabel, tenv, &roster, input, run)?;
        let pkey = primary.node_key_fp().unwrap_or_default();
        let tkey = twin.node_key_fp().unwrap_or_default();
        let (Some(pe), Some(te)) = (roster.entry(pkey), roster.entry(tkey)) else {
            return Err(run.fail(tlabel, "twin", "a node key left the roster between checks"));
        };
        if pe.name == te.name {
            return Err(run.fail(tlabel, "twin", format!("both statements come from node {}: a twin is a build on another machine", pe.name)));
        }
        if pe.host == te.host {
            return Err(run.fail(
                tlabel,
                "twin",
                format!("nodes {} and {} are on one machine (host {} in the roster): a twin is a build on another machine", pe.name, te.name, pe.host),
            ));
        }
        if primary.commit() != twin.commit() {
            return Err(run.fail(tlabel, "twin", format!("{plabel} built commit {:?}, {tlabel} built {:?}", primary.commit(), twin.commit())));
        }
        if matched.is_empty() {
            return Err(run.fail(tlabel, "twin", "name the files the two builds must agree on"));
        }
        for (name, sha) in &matched {
            let same = twin.subject.iter().any(|s| s.name.as_deref() == Some(name.as_str()) && s.sha256() == Some(sha.as_str()));
            if !same {
                let theirs = twin.subject.iter().find(|s| s.name.as_deref() == Some(name.as_str())).and_then(|s| s.sha256()).unwrap_or("nothing");
                return Err(run.fail(tlabel, "twin", format!("{name}: {plabel} has sha256 {sha}, {tlabel} has {theirs}")));
            }
        }
        run.pass(
            tlabel,
            "twin",
            format!("node {} on host {} built the same bytes as node {} on host {} for {} file(s)", te.name, te.host, pe.name, pe.host, matched.len()),
        );
        out.statements.push(summary);
    }
    Ok(())
}

/// The newest roster for the pinned owner among `trust/` and `--roster`.
fn pick_roster(input: &Input, run: &mut Run) -> Result<NodeRoster, Stop> {
    let fp = &input.policy.owner_fp;
    let mut given: Vec<(String, Signed)> = Vec::new();
    if let Some(t) = &input.trust {
        match t.roster(fp) {
            Ok(Some(s)) => given.push((format!("{}/rosters/{fp}.json", t.root().display()), s)),
            Ok(None) => {}
            Err(e) => return Err(run.fail("roster", "roster", e)),
        }
    }
    given.extend(input.rosters.iter().cloned());
    if given.is_empty() {
        let listed = input.trust.as_ref().and_then(|t| t.owners().ok()).unwrap_or_default();
        let hint = if listed.is_empty() {
            String::new()
        } else {
            format!(" (the trust folder lists: {})", listed.iter().map(|o| format!("{} {}", o.id, o.roster_key_fp)).collect::<Vec<_>>().join(", "))
        };
        return Err(run.fail("roster", "roster", format!("no roster for owner key {fp}: pass the trust folder that came with the statement, or --roster FILE{hint}")));
    }
    let mut opened: Vec<(String, NodeRoster, String)> = Vec::new();
    for (from, s) in &given {
        match NodeRoster::open(s, fp) {
            Ok(r) => opened.push((from.clone(), r, s.digest())),
            Err(e) => return Err(run.fail("roster", "roster", format!("{from}: {e}"))),
        }
    }
    opened.sort_by_key(|(_, r, _)| r.seq);
    for w in opened.windows(2) {
        let ((af, a, ad), (bf, b, bd)) = (&w[0], &w[1]);
        if a.seq == b.seq && ad != bd {
            return Err(run.fail("roster", "roster", format!("{af} and {bf} are two different rosters with seq {}: the owner key signed both", a.seq)));
        }
        if b.seq == a.seq + 1 && b.prev_digest.as_deref() != Some(ad.as_str()) {
            return Err(run.fail("roster", "roster", format!("{bf} (seq {}) does not follow {af} (seq {}): its prev_digest names another roster", b.seq, a.seq)));
        }
    }
    let Some((from, newest, _)) = opened.pop() else {
        return Err(run.fail("roster", "roster", "no roster"));
    };
    if let Some(older) = opened.iter().find(|(f, _, _)| input.rosters.iter().any(|(rf, _)| rf == f)) {
        run.warnings.push(format!("{} is roster seq {}, older than seq {} in {from}: using seq {}", older.0, older.1.seq, newest.seq, newest.seq));
    }
    run.pass(
        "roster",
        "roster",
        format!("{}'s roster seq {} (issued {}, {} {}) verifies with the pinned owner key {}…", newest.owner_id, newest.seq, when(newest.issued_at), newest.entries.len(), if newest.entries.len() == 1 { "key" } else { "keys" }, short(fp)),
    );
    Ok(newest)
}

fn check_statement(label: &str, env: &Envelope, roster: &NodeRoster, input: &Input, run: &mut Run) -> Result<(Statement, Summary), Stop> {
    let p = &input.policy;
    let sig = env.check_shape().map_err(|e| run.fail(label, "envelope", e))?;
    let keyid = sig.keyid.clone();

    // Node key: from the roster, the trust root.
    let Some(entry) = roster.entry(&keyid) else {
        return Err(run.fail(label, "node key", format!("the statement is signed by node key {keyid}, which is not in {}'s roster (seq {})", roster.owner_id, roster.seq)));
    };
    let key = entry.node_key().map_err(|e| run.fail(label, "node key", e))?;
    if let Some(t) = &input.trust {
        match t.node_key(&keyid) {
            Ok(Some(pem)) if pem.spki_der() != key.spki_der() => {
                return Err(run.fail(label, "node key", format!("trust/nodes/{keyid}.pem is not the roster's key for node {}", entry.name)));
            }
            Ok(_) => {}
            Err(e) => return Err(run.fail(label, "node key", e)),
        }
    }
    run.pass(label, "node key", format!("node {} (host {}), key {}…, epoch {}, {} in the roster", entry.name, entry.host, short(&keyid), entry.key_epoch, status_str(entry.status)));

    // Signature and format.
    let st = env.open(&key).map_err(|e| run.fail(label, "signature", e))?;
    run.pass(label, "signature", "the DSSE signature verifies with the node key; the statement is well formed");

    // Owner.
    match st.builder_owner() {
        Some(Owner::RosterKey(fp)) if fp == p.owner_fp => {}
        Some(Owner::RosterKey(fp)) => return Err(run.fail(label, "owner", format!("builder.id names owner key {fp}, not the pinned {}", p.owner_fp))),
        Some(Owner::Server(fp)) => {
            return Err(run.fail(
                label,
                "owner",
                format!("builder.id is the owner-server form ({fp}): the node held no roster when it signed, so nothing ties it to the pinned owner key"),
            ))
        }
        None => return Err(run.fail(label, "owner", "builder.id is not an owner's")),
    }
    if st.node_key_fp() != Some(keyid.as_str()) {
        return Err(run.fail(label, "owner", "the statement names another node key than the one that signed it"));
    }
    if st.key_epoch() != Some(u64::from(entry.key_epoch)) {
        return Err(run.fail(label, "owner", format!("the statement says key epoch {:?}, the roster {}", st.key_epoch(), entry.key_epoch)));
    }
    run.pass(label, "owner", format!("builder.id names the pinned owner {}", roster.owner_id));

    // Key status and time.
    let time_proven = check_status(label, &st, entry, p, run)?;

    // The node's self-statement.
    check_self_statement(label, &st, entry, &key, input, run)?;

    // Policy.
    let ip = &st.predicate.build_definition.internal_parameters;
    if let Some(c) = &p.commit {
        if st.commit() != Some(c.as_str()) {
            return Err(run.fail(label, "commit", format!("built from commit {}, not {c}", st.commit().unwrap_or("(no source)"))));
        }
        run.pass(label, "commit", format!("built from commit {c}"));
    }
    if let Some(r) = &p.repo {
        let have = st.source_uri().unwrap_or("");
        if !same_repo(have, r) {
            return Err(run.fail(label, "repo", format!("built from {}, not {r}", if have.is_empty() { "(no source)" } else { have })));
        }
        run.pass(label, "repo", format!("built from {have}"));
    }
    if !p.isolation.is_empty() {
        if !p.isolation.contains(&ip.isolation) {
            let want: Vec<&str> = p.isolation.iter().map(|i| i.as_str()).collect();
            return Err(run.fail(label, "isolation", format!("ran with isolation {}, not one of {}", ip.isolation.as_str(), want.join(", "))));
        }
        run.pass(label, "isolation", format!("ran with isolation {}", ip.isolation.as_str()));
    }

    let ep = &st.predicate.build_definition.external_parameters;
    let summary = Summary {
        label: label.to_string(),
        owner: roster.owner_id.clone(),
        node: entry.name.clone(),
        host: entry.host.clone(),
        node_fp: keyid,
        key_status: status_str(entry.status),
        source: st.source_uri().zip(st.commit()).map(|(u, c)| (u.to_string(), c.to_string())),
        argv: ep.argv.clone(),
        isolation: ip.isolation.as_str(),
        wall: wall_str(ip.wall),
        network: match ip.network {
            crate::statement::Network::None => "none",
            crate::statement::Network::NoLan => "no-lan",
            crate::statement::Network::On => "on",
        },
        started_on: st.predicate.run_details.metadata.started_on.clone(),
        finished_on: st.predicate.run_details.metadata.finished_on.clone(),
        time_proven,
        subjects: st.subject.iter().map(|s| (s.name.clone().unwrap_or_default(), s.sha256().unwrap_or_default().to_string())).collect(),
    };
    Ok((st, summary))
}

fn revoke_str(r: Option<crate::roster::RevokeReason>) -> &'static str {
    use crate::roster::RevokeReason as R;
    match r {
        Some(R::Compromised) => "compromised",
        Some(R::KeyLost) => "key lost",
        Some(R::Other) | None => "other",
    }
}

fn status_str(s: KeyStatus) -> &'static str {
    match s {
        KeyStatus::Active => "active",
        KeyStatus::Retired => "retired",
        KeyStatus::Revoked => "revoked",
    }
}

fn wall_str(w: Wall) -> &'static str {
    match w {
        Wall::JobUser => "job_user",
        Wall::Seatbelt => "seatbelt",
        Wall::None => "none",
    }
}

/// Key status and the time rule. `Ok(true)`: the time holds (an active key,
/// or a retired key with a time the caller vouches for); `Ok(false)`:
/// verified, time not proven.
fn check_status(label: &str, st: &Statement, e: &RosterEntry, p: &Policy, run: &mut Run) -> Result<bool, Stop> {
    let started = st.started();
    if e.taint_basis.is_none() {
        return Err(run.fail(label, "key status", format!("the roster vouches for nothing about node {}'s key (taint_basis null): it may have run a job without the wall", e.name)));
    }
    if started < e.valid_from {
        return Err(run.fail(label, "key status", format!("the job started {}, before node {}'s key was valid ({})", when(started), e.name, when(e.valid_from))));
    }
    match e.status {
        KeyStatus::Revoked => Err(run.fail(
            label,
            "key status",
            format!("node {}'s key is revoked ({}) in the roster: every statement from it is void", e.name, revoke_str(e.revoked_reason)),
        )),
        KeyStatus::Active => {
            run.pass(label, "key status", format!("node {}'s key is active; the job started {} (node clock)", e.name, when(started)));
            Ok(true)
        }
        KeyStatus::Retired => {
            let to = e.valid_to.unwrap_or(i64::MIN);
            if started > to {
                return Err(run.fail(label, "key status", format!("node {}'s key was retired {}, before the job started ({})", e.name, when(to), when(started))));
            }
            run.pass(label, "key status", format!("node {}'s key is retired since {}; the statement says the job started {}", e.name, when(to), when(started)));
            match p.existed_before {
                Some(t) if t <= to => {
                    run.pass(label, "time", format!("you vouch the statement existed by {} (--existed-before), while the key was still valid (until {})", when(t), when(to)));
                    Ok(true)
                }
                Some(t) => {
                    run.unproven(
                        label,
                        "time",
                        format!(
                            "you vouch the statement existed by {} (--existed-before), after the key was retired ({}): it could have been signed, back-dated, after the retirement",
                            when(t),
                            when(to)
                        ),
                    );
                    Ok(false)
                }
                None => {
                    run.unproven(
                        label,
                        "time",
                        format!(
                            "the job's times are the node's own clock, and the key was retired {}. Whoever holds a retired key can back-date a statement. Pass --existed-before TIME: a time by which you know the statement existed, from a timestamp or log you trust (for example the release's signature date); the tool takes your word for it",
                            when(to)
                        ),
                    );
                    Ok(false)
                }
            }
        }
    }
}

fn check_self_statement(label: &str, st: &Statement, e: &RosterEntry, key: &NodeKey, input: &Input, run: &mut Run) -> Result<(), Stop> {
    let named = st.builder_dep(names::NODE_STATEMENT).and_then(|d| d.sha256()).map(|h| format!("sha256:{h}"));
    let digest = named.unwrap_or_else(|| e.statement_digest.clone());
    let Some(trust) = &input.trust else {
        return Err(run.fail(label, "self-statement", format!("node {}'s self-statement {digest} is needed: pass the trust folder (--trust)", e.name)));
    };
    let signed = match trust.self_statement(&digest) {
        Ok(Some(s)) => s,
        Ok(None) => return Err(run.fail(label, "self-statement", format!("the trust folder lacks node {}'s self-statement {digest}", e.name))),
        Err(err) => return Err(run.fail(label, "self-statement", err)),
    };
    let ns = NodeStatement::open(&signed, key).map_err(|err| run.fail(label, "self-statement", err))?;
    let ip = &st.predicate.build_definition.internal_parameters;
    let bad = |why: String| format!("node {}: {why}", e.name);
    if ns.key_epoch != e.key_epoch {
        return Err(run.fail(label, "self-statement", bad(format!("its self-statement says key epoch {}, the roster {}", ns.key_epoch, e.key_epoch))));
    }
    if ns.dev {
        return Err(run.fail(label, "self-statement", bad("a --dev node (no job wall)".into())));
    }
    if ns.key_exposure.job_readable {
        return Err(run.fail(label, "self-statement", bad("its jobs can read its key".into())));
    }
    if ns.wall == Wall::None || ip.wall == Wall::None {
        return Err(run.fail(label, "self-statement", bad("the job ran with no wall".into())));
    }
    if ns.wall != ip.wall {
        return Err(run.fail(label, "self-statement", bad(format!("its self-statement says wall {}, the statement {}", wall_str(ns.wall), wall_str(ip.wall)))));
    }
    match (ns.taint, e.taint_basis) {
        (Taint::Tainted, _) => return Err(run.fail(label, "self-statement", bad("its key is tainted (a job ran without the wall)".into()))),
        (Taint::Unknown, Some(TaintBasis::OwnerAttested)) | (Taint::Clean, _) => {}
        (Taint::Unknown, _) => return Err(run.fail(label, "self-statement", bad("its key predates the taint flag and the owner did not vouch for it".into()))),
    }
    if ns.taint == Taint::Clean && e.taint_basis == Some(TaintBasis::Rekeyed) && ns.key_epoch == 0 {
        return Err(run.fail(label, "self-statement", bad("the roster says re-keyed, but this is the enrolment key (epoch 0)".into())));
    }
    if ns.issued_at > st.started() {
        return Err(run.fail(label, "self-statement", bad("its self-statement was issued after the job started".into())));
    }
    let taint = if ns.taint == Taint::Clean { "clean" } else { "unknown, vouched for by the owner" };
    run.pass(label, "self-statement", format!("node {}: wall {}, key not readable by jobs, taint {taint}", e.name, wall_str(ns.wall)));
    Ok(())
}

/// One repository under two names, as [`norm_repo`] forms: GitHub moved
/// it (a transfer and rename, which GitHub redirects). A statement made
/// before the move names the old name and one made after names the new, so
/// a policy that names either accepts both. Exactly these pairs, never a
/// pattern: K2's repo, Alakazam-211/K2, became AlakazamLabs/K5.
const RENAMED_REPOS: &[[&str; 2]] = &[["github.com/Alakazam-211/K2", "github.com/AlakazamLabs/K5"]];

/// `have` (a statement's source) is the repository `want` names: equal as
/// [`norm_repo`] forms, or the two names of one repository in
/// [`RENAMED_REPOS`].
pub fn same_repo(have: &str, want: &str) -> bool {
    let (h, w) = (norm_repo(have), norm_repo(want));
    h == w || RENAMED_REPOS.iter().any(|pair| pair.contains(&h.as_str()) && pair.contains(&w.as_str()))
}

/// A repository reference reduced to `host/path` for comparison: no
/// `git+`, scheme, user, trailing `.git` or `/`; the host lowercased.
pub fn norm_repo(s: &str) -> String {
    let s = s.trim();
    if s.starts_with("k2-project:") {
        return s.to_string();
    }
    let s = s.strip_prefix("git+").unwrap_or(s);
    let s = match s.find("://") {
        Some(i) => &s[i + 3..],
        None => s,
    };
    let (host, path) = match s.find('/') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let host = host.rsplit('@').next().unwrap_or(host);
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!("{}{}", host.to_ascii_lowercase(), path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_forms_compare_equal() {
        let want = "git.example.test/acme/app";
        for r in ["git+https://git.example.test/acme/app.git", "https://Git.Example.Test/acme/app/", "git.example.test/acme/app", "git+ssh://git@git.example.test/acme/app"] {
            assert_eq!(norm_repo(r), want, "{r}");
        }
        assert_ne!(norm_repo("git.example.test/acme/app2"), want);
        assert_ne!(norm_repo("git.example.test/acme"), want);
        assert_eq!(norm_repo("k2-project:abc"), "k2-project:abc");
    }

    /// K2's repo under its old name and its new one is one repository,
    /// either way round and in any form; a third name is not.
    #[test]
    fn a_renamed_repo_matches_under_both_names_and_no_other() {
        let old = ["git+https://github.com/Alakazam-211/K2.git", "https://github.com/Alakazam-211/K2", "github.com/Alakazam-211/K2"];
        let new = ["git+https://github.com/AlakazamLabs/K5.git", "https://GitHub.com/AlakazamLabs/K5/", "github.com/AlakazamLabs/K5"];
        for a in old.iter().chain(new.iter()) {
            for b in old.iter().chain(new.iter()) {
                assert!(same_repo(a, b), "{a} vs {b}");
            }
        }
        for third in [
            "github.com/AlakazamLabs/K2",
            "github.com/Alakazam-211/K5",
            "github.com/Alakazam-211/K2SO",
            "github.com/someone/K5",
            "git.example.test/Alakazam-211/K2",
            "github.com/alakazamlabs/k5",
        ] {
            for a in old.iter().chain(new.iter()) {
                assert!(!same_repo(third, a), "{third} built, {a} wanted");
                assert!(!same_repo(a, third), "{a} built, {third} wanted");
            }
        }
        // Other repositories still compare by their one name.
        assert!(same_repo("git+https://git.example.test/acme/app.git", "git.example.test/acme/app"));
        assert!(!same_repo("git.example.test/acme/app", "git.example.test/acme/app2"));
    }

    #[test]
    fn verdicts_have_distinct_exit_codes() {
        let codes = [Verdict::Verified.exit_code(), Verdict::TimeNotProven.exit_code(), Verdict::NotVerified.exit_code()];
        assert_eq!(codes, [0, 3, 1]);
    }
}
