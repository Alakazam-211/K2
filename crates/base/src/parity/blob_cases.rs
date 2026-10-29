//! The kit's tier-0 parity cases: for every person action a blob doc has, the person path
//! (a tab, the `BlobSession` API) and the agent path through each verb of the tier-0
//! registry. One exhaustive `match` ([`kit_blob_cases`]): a new [`PersonAction`] doesn't
//! compile until it is placed here too.

use super::action::PersonAction;
use super::blob::{BlobAgent, BlobCase, BlobPerson, BlobStage, BLOB_MAX_BYTES};
use super::harness::{Output, Step, VerbOut, VerbValue};
use crate::session::{
    ApprovalState, BlobBlame, BlobLogEntry, BlobStatus, BlobWriteResult, FlushOutcome, LeaseInfo,
    Rights, RosterEntry, SessionError,
};
use crate::tools::session_exit;

type R = Result<Step, String>;

const NEW: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Lang (en) >> endobj\n%%EOF\n";

// ---------- shared helpers ----------

fn person_step<T>(r: Result<T, SessionError>, f: impl FnOnce(T) -> Output) -> R {
    Ok(match r {
        Ok(t) => Step::ok(f(t)),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}

fn agent_step(out: VerbOut, f: impl FnOnce(&VerbValue) -> Output) -> R {
    Ok(if out.refused.is_some() {
        Step::exit(out.exit, Output::None)
    } else {
        Step::exit(out.exit, f(&out.value))
    })
}

/// A write's or restore's observable result (the blob id is keyed per store: left out).
fn write_out(w: &BlobWriteResult) -> Output {
    Output::Value(format!(
        "v{} {} bytes changed={} written={}",
        w.version, w.size, w.changed, w.written
    ))
}

fn lease_out(l: Option<&LeaseInfo>) -> Output {
    Output::value(l.map(|l| (&l.pid, &l.name, l.acquired_ms, l.expires_ms, l.auto)))
}

fn status_out(s: &BlobStatus) -> Output {
    Output::value((
        s.version,
        &s.disk,
        s.lease.as_ref().map(|l| &l.pid),
        s.participants,
        s.conflict_copies.len(),
    ))
}

fn flush_out(f: &FlushOutcome) -> Output {
    Output::value((f.written, &f.disk))
}

fn log_out(log: &[BlobLogEntry]) -> Output {
    Output::value(
        log.iter()
            .map(|e| (e.id, e.kind, &e.author, &e.name, e.of, e.redacted, e.size))
            .collect::<Vec<_>>(),
    )
}

fn blame_out(b: &BlobBlame) -> Output {
    Output::value((
        b.version,
        &b.current.author,
        b.versions,
        b.authors
            .iter()
            .map(|a| (&a.pid, a.versions))
            .collect::<Vec<_>>(),
    ))
}

fn pids(r: &[RosterEntry]) -> Output {
    Output::Pids(r.iter().map(|e| e.participant.pid.clone()).collect())
}

fn other(v: &VerbValue) -> Output {
    Output::value(v)
}

// ---------- setups ----------

fn sam_joins(st: &mut BlobStage) -> Result<(), String> {
    st.other("sam").map(|_| ())
}

fn sam_writes(st: &mut BlobStage) -> Result<(), String> {
    st.advance(1_000);
    st.write_as("sam", b"%PDF-1.7 sam's version\n%%EOF\n")?;
    st.advance(1_000);
    let v = st.version()?;
    st.set_mark("v", v);
    Ok(())
}

fn mark_v(st: &mut BlobStage) -> Result<(), String> {
    let v = st.version()?;
    st.set_mark("v", v);
    Ok(())
}

fn sam_leases(st: &mut BlobStage) -> Result<(), String> {
    st.lease_as("sam")?;
    mark_v(st)
}

fn sam_leases_actor_owns(st: &mut BlobStage) -> Result<(), String> {
    st.grant(super::harness::ACTOR, Rights::full().with_owner(true));
    sam_leases(st)
}

fn no_history(st: &mut BlobStage) -> Result<(), String> {
    sam_writes(st)?;
    st.grant(super::harness::ACTOR, Rights::editor());
    Ok(())
}

fn too_large(st: &mut BlobStage) -> Result<(), String> {
    mark_v(st)?;
    let big = vec![b'x'; BLOB_MAX_BYTES as usize + 1];
    st.write_file(&big)?;
    Ok(())
}

// ---------- P1, P2: open, leave ----------

fn p1_person(p: &mut BlobPerson<'_>) -> R {
    p.conn()?;
    Ok(Step::done())
}
fn p1_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["open"], b"")?, |_| Output::None)
}
fn p2_person(p: &mut BlobPerson<'_>) -> R {
    p.conn()?;
    p.close()?;
    Ok(Step::done())
}
fn p2_agent(a: &mut BlobAgent<'_>) -> R {
    a.verb(&["open"], b"")?;
    agent_step(a.verb(&["leave"], b"")?, |_| Output::None)
}

// ---------- P3, P24: the bytes ----------

fn p3_person(p: &mut BlobPerson<'_>) -> R {
    person_step(p.bytes(), Output::Bytes)
}
fn p3_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["cat"], b"")?, |v| match v {
        VerbValue::Bytes(b) => Output::Bytes(b.clone()),
        v => other(v),
    })
}

// ---------- P6: who ----------

fn p6_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().roster(c), |r| pids(&r))
}
fn p6_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["who"], b"")?, |v| match v {
        VerbValue::Roster(r) => pids(r),
        v => other(v),
    })
}

// ---------- P14: write --base V ----------

fn p14_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    let v = p.stage().mark("v")?;
    person_step(p.session().write_bytes(c, NEW, v), |w| write_out(&w))
}
fn p14_agent(a: &mut BlobAgent<'_>) -> R {
    let v = a.stage().mark("v")?.to_string();
    agent_step(a.verb(&["write", "--base", &v], NEW)?, |v| match v {
        VerbValue::BlobWrite(w) => write_out(w),
        v => other(v),
    })
}
fn p14_stale_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().write_bytes(c, NEW, 1), |w| write_out(&w))
}
fn p14_stale_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["write", "--base", "1"], NEW)?, other)
}

// ---------- P17, P25: status, flush ----------

fn p17_status_person(p: &mut BlobPerson<'_>) -> R {
    p.conn()?;
    person_step(p.session().status(), |s| status_out(&s))
}
fn p17_status_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["status"], b"")?, |v| match v {
        VerbValue::BlobStatus(s) => status_out(s),
        v => other(v),
    })
}
fn p17_flush_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().flush(c), |f| flush_out(&f))
}
fn p17_flush_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["flush"], b"")?, |v| match v {
        VerbValue::Flush(f) => flush_out(f),
        v => other(v),
    })
}

// ---------- P19: log, show --version ----------

fn p19_log_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().log(c), |l| log_out(&l))
}
fn p19_log_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["log"], b"")?, |v| match v {
        VerbValue::BlobLog(l) => log_out(l),
        v => other(v),
    })
}
fn p19_show_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    let r = p.session().read_version(c, 1).and_then(|r| {
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut { r.reader }, &mut b)
            .map_err(|e| SessionError::Disk(e.to_string()))?;
        Ok(b)
    });
    person_step(r, Output::Bytes)
}
fn p19_show_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["show", "--version", "1"], b"")?, |v| match v {
        VerbValue::Bytes(b) => Output::Bytes(b.clone()),
        v => other(v),
    })
}

// ---------- P20: restore, version --name ----------

fn p20_restore_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().restore(c, 1), |w| write_out(&w))
}
fn p20_restore_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["restore", "--version", "1"], b"")?, |v| match v {
        VerbValue::BlobWrite(w) => write_out(w),
        v => other(v),
    })
}
fn p20_name_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    let s = p.session();
    let v = s.status().map_err(|e| e.to_string())?.version;
    person_step(s.name_version(c, v, "final"), |()| Output::value(v))
}
fn p20_name_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["version", "--name", "final"], b"")?, |v| match v {
        VerbValue::Version(v) => Output::value(*v),
        v => other(v),
    })
}

// ---------- P21: mv ----------

fn p21_person(p: &mut BlobPerson<'_>) -> R {
    p.conn()?;
    person_step(p.stage().host_rename("renamed.pdf"), Output::value)
}
fn p21_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["mv", "renamed.pdf"], b"")?, |v| match v {
        VerbValue::Moved(d) => Output::value(d),
        v => other(v),
    })
}

// ---------- P22: blame ----------

fn p22_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().blame(c), |b| blame_out(&b))
}
fn p22_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["blame"], b"")?, |v| match v {
        VerbValue::BlobBlame(b) => blame_out(b),
        v => other(v),
    })
}

// ---------- P32-P34: the lease ----------

fn lease_value(v: &VerbValue) -> Output {
    match v {
        VerbValue::Lease(l) => lease_out(l.as_ref()),
        v => other(v),
    }
}

fn p32_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().acquire_lease(c, None), |l| lease_out(Some(&l)))
}
fn p32_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["lease"], b"")?, lease_value)
}
fn p32_renew_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    let s = p.session();
    s.acquire_lease(c, Some(60_000))
        .map_err(|e| e.to_string())?;
    p.stage().advance(30_000);
    person_step(s.renew_lease(c, None), |l| lease_out(Some(&l)))
}
fn p32_renew_agent(a: &mut BlobAgent<'_>) -> R {
    let out = a.verb(&["lease", "--ttl", "60000"], b"")?;
    if out.exit != 0 {
        return Err(format!("lease --ttl: exit {}", out.exit));
    }
    a.stage().advance(30_000);
    agent_step(a.verb(&["lease", "--renew"], b"")?, lease_value)
}
fn p32_renew_none_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().renew_lease(c, None), |l| lease_out(Some(&l)))
}
fn p32_renew_none_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["lease", "--renew"], b"")?, lease_value)
}

fn released(held: bool) -> Step {
    Step::exit(u8::from(!held), Output::value(held))
}

fn p33_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    let s = p.session();
    s.acquire_lease(c, None).map_err(|e| e.to_string())?;
    Ok(match s.release_lease(c) {
        Ok(h) => released(h),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}
fn p33_agent(a: &mut BlobAgent<'_>) -> R {
    a.verb(&["lease"], b"")?;
    release_agent(a)
}
fn p33_not_mine_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    Ok(match p.session().release_lease(c) {
        Ok(h) => released(h),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}
fn release_agent(a: &mut BlobAgent<'_>) -> R {
    let out = a.verb(&["lease", "--release"], b"")?;
    Ok(match (&out.refused, &out.value) {
        (None, VerbValue::Released(h)) => Step::exit(out.exit, Output::value(*h)),
        (None, v) => Step::exit(out.exit, other(v)),
        (Some(_), _) => Step::exit(out.exit, Output::None),
    })
}

fn p34_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    Ok(match p.session().break_lease(c) {
        Ok(l) => Step::exit(u8::from(l.is_none()), lease_out(l.as_ref())),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}
fn p34_agent(a: &mut BlobAgent<'_>) -> R {
    agent_step(a.verb(&["lease", "--break"], b"")?, lease_value)
}

// ---------- P29, P30: on-behalf writes (K-ID-10, K-ID-11, K-T0-9) ----------

/// `pid`, an assistant acting for `person`, writes `bytes` from the current version.
fn assistant_writes(
    st: &mut BlobStage,
    pid: &str,
    person: &str,
    bytes: &[u8],
) -> Result<u64, String> {
    let c = st.assistant(pid, person)?;
    let base = st.version()?;
    st.advance(1_000);
    let w = st
        .session()
        .write_bytes(c, bytes, base)
        .map_err(|e| format!("{pid} write: {e}"))?;
    Ok(w.version)
}

/// "aide", an assistant acting for the actor, writes; mark "v" is its version.
fn aide_writes(st: &mut BlobStage) -> Result<(), String> {
    let v = assistant_writes(st, "aide", super::harness::ACTOR, NEW)?;
    st.set_mark("v", v);
    st.advance(1_000);
    Ok(())
}

/// As [`aide_writes`], then sam writes over it (a decline names that later version).
fn aide_then_sam_writes(st: &mut BlobStage) -> Result<(), String> {
    aide_writes(st)?;
    st.write_as("sam", b"%PDF-1.7 sam's version\n%%EOF\n")?;
    st.advance(1_000);
    Ok(())
}

/// "bot", an assistant acting for sam, writes; mark "v" is its version (not the actor's).
fn bot_writes_for_sam(st: &mut BlobStage) -> Result<(), String> {
    let v = assistant_writes(st, "bot", "sam", NEW)?;
    st.set_mark("v", v);
    Ok(())
}

/// Two writes for the actor and one for sam between them.
fn p30_setup(st: &mut BlobStage) -> Result<(), String> {
    assistant_writes(st, "aide", super::harness::ACTOR, NEW)?;
    assistant_writes(st, "bot", "sam", b"%PDF-1.7 bot for sam\n%%EOF\n")?;
    assistant_writes(
        st,
        "aide",
        super::harness::ACTOR,
        b"%PDF-1.7 aide again\n%%EOF\n",
    )?;
    Ok(())
}

fn approval_person(p: &mut BlobPerson<'_>, state: ApprovalState) -> R {
    let c = p.conn()?;
    let v = p.stage().mark("v")?;
    person_step(p.session().set_approval(c, v, state), Output::value)
}
fn approval_agent(a: &mut BlobAgent<'_>, verb: &str) -> R {
    let v = a.stage().mark("v")?.to_string();
    agent_step(a.verb(&[verb, &v], b"")?, |o| match o {
        VerbValue::BlobApproval(x) => Output::value(x),
        v => other(v),
    })
}
fn p29_approve_person(p: &mut BlobPerson<'_>) -> R {
    approval_person(p, ApprovalState::Approved)
}
fn p29_approve_agent(a: &mut BlobAgent<'_>) -> R {
    approval_agent(a, "approve")
}
fn p29_decline_person(p: &mut BlobPerson<'_>) -> R {
    approval_person(p, ApprovalState::Declined)
}
fn p29_decline_agent(a: &mut BlobAgent<'_>) -> R {
    approval_agent(a, "decline")
}

fn p30_person(p: &mut BlobPerson<'_>) -> R {
    // the tab's "waiting for you" list
    let c = p.conn()?;
    person_step(p.session().approvals(c), |all| {
        let vs: Vec<u64> = all
            .into_iter()
            .filter(|(_, a)| {
                a.state == ApprovalState::Pending && a.on_behalf_of == super::harness::ACTOR
            })
            .map(|(v, _)| v)
            .collect();
        Output::value(vs)
    })
}
fn p30_agent(a: &mut BlobAgent<'_>) -> R {
    p30_agent_as(a, &["log", "--pending", "--on-behalf-of", "me"])
}
/// As [`p30_setup`], then sam settles the bot's two writes made for sam: one approved, one
/// declined (`approvals --all` lists settled records too).
fn p30_all_setup(st: &mut BlobStage) -> Result<(), String> {
    p30_setup(st)?;
    let first = assistant_writes(st, "bot", "sam", b"%PDF-1.7 bot again\n%%EOF\n")?;
    let second = assistant_writes(st, "bot", "sam", b"%PDF-1.7 bot thrice\n%%EOF\n")?;
    let sam = st.other("sam")?;
    for (v, state) in [
        (first, ApprovalState::Approved),
        (second, ApprovalState::Declined),
    ] {
        st.session()
            .set_approval(sam, v, state)
            .map_err(|e| format!("sam settles {v}: {e}"))?;
    }
    Ok(())
}
/// `approvals --all`: every approval record, settled ones too, newest first, with its state.
fn p30_all_person(p: &mut BlobPerson<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().approvals(c), |mut all| {
        all.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
        let rows: Vec<(u64, ApprovalState)> = all.into_iter().map(|(v, a)| (v, a.state)).collect();
        Output::value(rows)
    })
}
fn p30_all_agent(a: &mut BlobAgent<'_>) -> R {
    p30_all_agent_as(a, &["approvals", "--all"])
}
fn p30_all_log_agent(a: &mut BlobAgent<'_>) -> R {
    p30_all_agent_as(a, &["log", "--pending", "--all"])
}
fn p30_all_agent_as(a: &mut BlobAgent<'_>, argv: &[&str]) -> R {
    agent_step(a.verb(argv, b"")?, |o| match o {
        VerbValue::BlobLog(l) => Output::value(
            l.iter()
                .filter_map(|e| e.approval.map(|s| (e.id, s)))
                .collect::<Vec<_>>(),
        ),
        v => other(v),
    })
}
/// P30 through the alias (`approvals`, the same as `log --pending`).
fn p30_alias_agent(a: &mut BlobAgent<'_>) -> R {
    p30_agent_as(a, &["approvals", "--on-behalf-of", "me"])
}
fn p30_agent_as(a: &mut BlobAgent<'_>, argv: &[&str]) -> R {
    agent_step(a.verb(argv, b"")?, |o| match o {
        VerbValue::BlobLog(l) => Output::value(l.iter().map(|e| e.id).collect::<Vec<_>>()),
        v => other(v),
    })
}

/// The kit's tier-0 cases for `action` (empty for text-only actions, and for blob actions
/// the kit doesn't bind yet: a tier-0 list marks those deferred).
pub fn kit_blob_cases(action: PersonAction) -> Vec<BlobCase> {
    use PersonAction as P;
    let c = BlobCase::new;
    match action {
        P::Open => vec![c(
            action,
            "open",
            "a tab opens the file",
            p1_person,
            p1_agent,
        )],
        P::Close => vec![c(action, "leave", "the tab closes", p2_person, p2_agent)],
        P::ReadAll => vec![c(action, "cat", "the file's bytes", p3_person, p3_agent)],
        P::Export => vec![c(
            action,
            "cat",
            "N/A: cat is the file's bytes",
            p3_person,
            p3_agent,
        )],
        P::Who => vec![c(action, "who", "the roster", p6_person, p6_agent).setup(sam_joins)],
        P::Rewrite => vec![
            c(action, "write", "--base V", p14_person, p14_agent).setup(mark_v),
            c(
                action,
                "write",
                "a stale base: exit 1",
                p14_stale_person,
                p14_stale_agent,
            )
            .setup(sam_writes),
            c(
                action,
                "write",
                "sam holds the lease: exit 5",
                p14_person,
                p14_agent,
            )
            .setup(sam_leases),
        ],
        P::Saved => vec![
            c(
                action,
                "status",
                "clean after sam's write",
                p17_status_person,
                p17_status_agent,
            )
            .setup(sam_writes),
            c(
                action,
                "flush",
                "nothing to write",
                p17_flush_person,
                p17_flush_agent,
            ),
        ],
        P::WhyOff => vec![
            c(
                action,
                "status",
                "too large: declined",
                p17_status_person,
                p17_status_agent,
            )
            .setup(too_large),
            c(
                action,
                "write",
                "too large: writes exit 4",
                p14_person,
                p14_agent,
            )
            .setup(too_large),
        ],
        P::History => vec![
            c(
                action,
                "log",
                "every version",
                p19_log_person,
                p19_log_agent,
            )
            .setup(sam_writes),
            c(
                action,
                "show --version",
                "the import",
                p19_show_person,
                p19_show_agent,
            )
            .setup(sam_writes),
        ],
        P::Restore => vec![
            c(
                action,
                "restore",
                "the import",
                p20_restore_person,
                p20_restore_agent,
            )
            .setup(sam_writes),
            c(
                action,
                "version --name",
                "the current version",
                p20_name_person,
                p20_name_agent,
            ),
        ],
        P::Rename => vec![c(action, "mv", "file and key move", p21_person, p21_agent)],
        P::Blame => vec![
            c(action, "blame", "who set it", p22_person, p22_agent).setup(sam_writes),
            c(
                action,
                "blame",
                "no history right: refused",
                p22_person,
                p22_agent,
            )
            .setup(no_history),
        ],
        P::Lease => vec![
            c(action, "lease", "take it", p32_person, p32_agent),
            c(
                action,
                "lease",
                "sam holds it: exit 5",
                p32_person,
                p32_agent,
            )
            .setup(sam_leases),
            c(
                action,
                "lease --renew",
                "extend mine",
                p32_renew_person,
                p32_renew_agent,
            ),
            c(
                action,
                "lease --renew",
                "nobody holds it: exit 2",
                p32_renew_none_person,
                p32_renew_none_agent,
            ),
        ],
        P::ReleaseLease => vec![
            c(action, "lease --release", "mine", p33_person, p33_agent),
            c(
                action,
                "lease --release",
                "sam's: exit 1, still sam's",
                p33_not_mine_person,
                release_agent,
            )
            .setup(sam_leases),
        ],
        P::BreakLease => vec![
            c(
                action,
                "lease --break",
                "sam's, as owner",
                p34_person,
                p34_agent,
            )
            .setup(sam_leases_actor_owns),
            c(
                action,
                "lease --break",
                "not owner: exit 3",
                p34_person,
                p34_agent,
            )
            .setup(sam_leases),
        ],
        P::Approve => vec![
            c(
                action,
                "approve",
                "an assistant's write for me",
                p29_approve_person,
                p29_approve_agent,
            )
            .setup(aide_writes),
            c(
                action,
                "decline",
                "kept; the way back names a later write",
                p29_decline_person,
                p29_decline_agent,
            )
            .setup(aide_then_sam_writes),
            c(
                action,
                "approve",
                "made for sam: exit 3",
                p29_approve_person,
                p29_approve_agent,
            )
            .setup(bot_writes_for_sam),
        ],
        P::Pending => vec![
            c(
                action,
                "log --pending",
                "waiting for me",
                p30_person,
                p30_agent,
            )
            .setup(p30_setup),
            c(
                action,
                "approvals",
                "waiting for me",
                p30_person,
                p30_alias_agent,
            )
            .setup(p30_setup),
            c(
                action,
                "approvals",
                "--all: settled ones too, newest first",
                p30_all_person,
                p30_all_agent,
            )
            .setup(p30_all_setup),
            c(
                action,
                "log --pending",
                "--all: settled ones too, newest first",
                p30_all_person,
                p30_all_log_agent,
            )
            .setup(p30_all_setup),
        ],
        // a blob doc has these, but no verb yet: a tier-0 list marks them deferred
        P::ViewRange | P::Watch | P::Purge => Vec::new(),
        // text docs only
        P::Find
        | P::Follow
        | P::Caret
        | P::Type
        | P::TypeOver
        | P::Delete
        | P::FindReplace
        | P::Paste
        | P::Format
        | P::Undo
        | P::Annotate
        | P::UndoPartial
        | P::Mention
        | P::Revert => Vec::new(),
    }
}
