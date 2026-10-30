//! Agent parity for tier 0 (any file, K-PAR-1…3 on blob docs): every tier-0 row through the
//! kit's blob harness, with negative controls proving it fails loudly. The leases (P32–P34)
//! are tier 0's own rows.

use std::sync::Arc;

use k2_mpc_base::parity::{
    self, BlobAgent, BlobCase, BlobPerson, Failure, ParityRow, PersonAction, Registry, RowStatus,
    Step, Tier,
};
use k2_mpc_base::session::{AnyFile, BlobPack};

fn pack() -> Arc<dyn BlobPack> {
    Arc::new(AnyFile)
}

fn blob() -> Registry {
    Registry::for_tier(Tier::Blob)
}

/// Tier 0's parity list for any file: an exhaustive `match`, as every pack writes it.
fn row(a: PersonAction) -> ParityRow {
    use PersonAction as P;
    match a {
        P::Open => ParityRow::live(a, &["open"]),
        P::Close => ParityRow::live(a, &["leave"]),
        P::ReadAll => ParityRow::live(a, &["cat"]),
        P::ViewRange => {
            ParityRow::deferred(a, &["view"], "later", "a view-region verb (`set_region`)")
        }
        P::Who => ParityRow::live(a, &["who"]),
        P::Rewrite => ParityRow::live(a, &["write"]),
        P::Saved => ParityRow::live(a, &["status", "flush"]),
        P::Watch => ParityRow::deferred(
            a,
            &["watch", "tail -f"],
            "later",
            "blob docs have no change feed yet",
        ),
        P::History => ParityRow::live(a, &["log", "show --version"]),
        P::Restore => ParityRow::live(a, &["restore", "version --name"]),
        P::Rename => ParityRow::live(a, &["mv"]),
        P::Blame => ParityRow::live(a, &["blame"]),
        P::Export => ParityRow::live(a, &["export", "export --version"]),
        P::WhyOff => ParityRow::live(a, &["status"]),
        P::Purge => ParityRow::deferred(a, &["purge"], "later", "no purge verb yet"),
        P::Approve => ParityRow::live(a, &["approve", "decline"]),
        P::Pending => ParityRow::live(a, &["log --pending", "approvals"]),
        P::Lease => ParityRow::live(a, &["lease", "lease --renew"]),
        P::ReleaseLease => ParityRow::live(a, &["lease --release"]),
        P::BreakLease => ParityRow::live(a, &["lease --break"]),
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
        | P::Revert => ParityRow::not_applicable(a, &[], &[], "text docs only"),
    }
}

fn rows() -> Vec<ParityRow> {
    PersonAction::for_tier(Tier::Blob).map(row).collect()
}

#[test]
fn agent_parity_any_file() {
    let report = parity::check_blob(pack(), &rows(), &blob(), &[])
        .unwrap_or_else(|f| panic!("{}", parity::render(&f)));
    let live = rows()
        .iter()
        .filter(|r| r.status == RowStatus::Live)
        .count();
    assert_eq!(live, 17, "rows live (P24 Export since exports P0)");
    assert_eq!(report.not_applicable.len(), 0);
    assert_eq!(report.deferred.len(), 3);
    for r in rows().iter().filter(|r| r.status == RowStatus::Live) {
        for v in r.verbs {
            assert!(
                report.matched.iter().any(|m| m.0 == r.action && m.1 == *v),
                "{} {v} not exercised",
                r.action
            );
        }
    }
    let leases = report
        .matched
        .iter()
        .filter(|m| m.0.tiers() == [Tier::Blob])
        .count();
    assert_eq!(leases, 8, "the lease rows' cases");
    let approvals = report
        .matched
        .iter()
        .filter(|m| matches!(m.0, PersonAction::Approve | PersonAction::Pending))
        .count();
    // P30 four times: `log --pending` and its alias `approvals`, each also with `--all`
    assert_eq!(approvals, 7, "the approval rows' cases (P29, P30)");
    assert_eq!(report.matched.len(), 35, "{:?}", report.matched);
}

/// A tier-0 list must have every blob action's row, not the text-only ones.
#[test]
fn the_tier0_list_is_checked_both_ways() {
    let f = parity::check_blob_list(&rows(), &blob(), &[]);
    assert!(f.is_empty(), "{}", parity::render(&f));
    assert!(!rows().iter().any(|r| r.action == PersonAction::Find));
    // a missing lease row
    let mut r = rows();
    r.retain(|x| x.action != PersonAction::BreakLease);
    let f = parity::check_blob_list(&r, &blob(), &[]);
    assert_eq!(
        f,
        vec![
            Failure::MissingRow(PersonAction::BreakLease),
            Failure::VerbNotListed {
                action: PersonAction::BreakLease,
                verb: "lease --break".into()
            }
        ],
        "{}",
        parity::render(&f)
    );
    // a lease verb taken out of the registry: missing from the list and the agent path
    let reg = blob().without("lease --renew");
    let f = parity::check_blob_list(&rows(), &reg, &[]);
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: PersonAction::Lease,
            verb: "lease --renew".into(),
            at: "parity list",
        }],
        "{}",
        parity::render(&f)
    );
    let case = parity::kit_blob_cases(PersonAction::Lease)
        .into_iter()
        .find(|c| c.verb == "lease --renew")
        .unwrap();
    let f = parity::run_blob_case(&pack(), &reg, &case);
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: PersonAction::Lease,
            verb: "lease --renew".into(),
            at: "agent path",
        }],
        "{}",
        parity::render(&f)
    );
    assert!(parity::run_blob_case(&pack(), &blob(), &case).is_empty());
    // the blob harness refuses a text registry
    let f = parity::check_blob_list(&rows(), &Registry::kit(), &[]);
    assert!(matches!(&f[..], [Failure::Registry(_)]), "{f:?}");
}

/// Negative control: a lease verb that does something else than the person did is a
/// mismatch (here: the agent renews for 1 s, the person takes the default 5 min).
#[test]
fn a_different_lease_is_a_mismatch() {
    fn person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
        let c = p.conn()?;
        p.session()
            .acquire_lease(c, None)
            .map_err(|e| e.to_string())?;
        Ok(Step::done())
    }
    fn agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
        a.verb(&["lease", "--ttl", "1000"], b"")?;
        Ok(Step::done())
    }
    let bad = BlobCase::new(PersonAction::Lease, "lease", "planted", person, agent);
    let f = parity::run_blob_case(&pack(), &blob(), &bad);
    assert!(
        matches!(&f[..], [Failure::Mismatch { diffs, .. }] if diffs.iter().any(|d| d.starts_with("lease:"))),
        "{}",
        parity::render(&f)
    );
}

/// Negative controls for the approval rows (P29, P30): a missing verb is reported in the
/// list and on the agent path, and an agent verb with another effect than the person's
/// (declining what the person approved) is a mismatch on the approvals and the output.
#[test]
fn an_approval_verb_missing_or_different_fails_loudly() {
    let reg = blob().without("decline");
    let f = parity::check_blob_list(&rows(), &reg, &[]);
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: PersonAction::Approve,
            verb: "decline".into(),
            at: "parity list",
        }],
        "{}",
        parity::render(&f)
    );
    let case = parity::kit_blob_cases(PersonAction::Approve)
        .into_iter()
        .find(|c| c.verb == "decline")
        .unwrap();
    let f = parity::run_blob_case(&pack(), &reg, &case);
    assert!(
        matches!(
            &f[..],
            [Failure::MissingVerb {
                at: "agent path",
                ..
            }]
        ),
        "{}",
        parity::render(&f)
    );
    assert!(parity::run_blob_case(&pack(), &blob(), &case).is_empty());

    // planted: the agent declines what the person approved
    let approve = parity::kit_blob_cases(PersonAction::Approve)
        .into_iter()
        .find(|c| c.verb == "approve")
        .unwrap();
    fn agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
        let v = a.stage().mark("v")?.to_string();
        a.verb(&["decline", &v], b"")?;
        Ok(Step::done())
    }
    fn person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
        let c = p.conn()?;
        let v = p.stage().mark("v")?;
        p.session()
            .set_approval(c, v, k2_mpc_base::session::ApprovalState::Approved)
            .map_err(|e| e.to_string())?;
        Ok(Step::done())
    }
    let bad = BlobCase {
        person,
        agent,
        ..approve
    };
    let f = parity::run_blob_case(&pack(), &blob(), &bad);
    assert!(
        matches!(&f[..], [Failure::Mismatch { diffs, .. }] if diffs.iter().any(|d| d.starts_with("approvals:"))),
        "{}",
        parity::render(&f)
    );
}
