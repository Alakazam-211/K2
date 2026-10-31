//! The parity harness's grid stage (A11, K-PAR-3, CSV contract §3.5) with the kit's test table
//! pack (`grid-lite`, `tests/grid_pack`): every grid-tier row through the harness (the kit's
//! verbs by [`parity::kit_grid_cases`], the pack's skeleton structured verbs (A9) by the
//! cases here), comparing the grid by key, and negative controls proving it fails loudly.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::sync::Arc;
use std::time::Instant;

use grid_pack::GridLite;
use k2_mpc_base::elem::{Anchor, Key};
use k2_mpc_base::parity::{
    self, Agent, Failure, GridCase, Output, ParityRow, Person, PersonAction, Registry, RowStatus,
    Stage, Step, Tier, VerbValue,
};
use k2_mpc_base::session::{EditResult, Pack, TextFilePack};
use k2_mpc_base::table::{verbs, SortKey, Table, TableError};
use k2_mpc_base::tools::{session_exit, PackVerb, StructOutput, ToolError};
use k2_mpc_base::Edit;

fn pack() -> Arc<dyn Pack> {
    Arc::new(GridLite)
}

fn registry() -> Registry {
    Registry::for_pack(&GridLite).expect("the grid pack's registry")
}

/// The grid pack's parity list: an exhaustive `match`, as every pack writes it. Rows a grid
/// doesn't have (P15, the leases) are N/A; P23, P27 and P28 are deferred as for text.
fn row(a: PersonAction) -> ParityRow {
    use PersonAction as P;
    const TEXT_EDITS: &[&str] = &[
        "insert", "replace", "delete", "sed -i", "patch", "apply", "append",
    ];
    match a {
        P::Open => ParityRow::live(a, &["open"]),
        P::Close => ParityRow::live(a, &["leave"]),
        P::ReadAll => ParityRow::live(a, &["cat", "wc", "cell get", "row list", "col list"]),
        P::ViewRange => ParityRow::live(a, &["head", "tail", "sed", "view"]),
        P::Find => ParityRow::live(a, &["grep"]),
        P::Who => ParityRow::live(a, &["who", "activity"]),
        P::Follow => ParityRow::live(a, &["follow"]),
        P::Caret => ParityRow::live(a, &["cursor", "claim", "claim --renew", "claim --release"]),
        // the text edit verbs on a rendered grid come with reconcile (C1i)
        P::Type => ParityRow::live_but(a, &["cell set"], &["insert"]),
        P::TypeOver => ParityRow::live_but(a, &["cell set"], &["replace"]),
        P::Delete => ParityRow::live_but(a, &["cells clear", "row delete"], &["delete", "sed -i"]),
        P::FindReplace => ParityRow::live_but(a, &["cell set"], &["sed -i", "replace"]),
        P::Paste => ParityRow::live_but(a, &["cells set"], &["patch", "apply", "append"]),
        P::Rewrite => ParityRow::live(a, &["write"]),
        P::Format => ParityRow::not_applicable(a, &[], TEXT_EDITS, "CSV has no formatting"),
        P::Undo => ParityRow::live(a, &["undo", "redo", "undo --drop"]),
        P::Saved => ParityRow::live(a, &["status", "flush"]),
        P::Watch => ParityRow::live(a, &["watch", "tail -f"]),
        P::History => ParityRow::live(a, &["log", "diff --version", "show --version", "diff"]),
        P::Restore => ParityRow::live(a, &["restore", "version --name"]),
        P::Rename => ParityRow::live(a, &["mv"]),
        P::Blame => ParityRow::live(a, &["blame", "blame --cell", "blame --structure"]),
        P::Annotate => ParityRow::deferred(a, &["comment"], "K-ANN-7", "cell anchors"),
        P::Export => ParityRow::live(a, &["export", "export --version"]),
        P::WhyOff => ParityRow::live(a, &["status"]),
        P::UndoPartial => ParityRow::live(a, &["undo --partial"]),
        P::Mention => ParityRow::deferred(a, &["comment"], "K-ANN-7", "cell anchors"),
        P::Purge => ParityRow::deferred(a, &["purge"], "the purge verb", "not bound by the kit"),
        P::Approve => ParityRow::live(a, &["approve", "decline"]),
        P::Pending => ParityRow::live(a, &["log --pending"]),
        P::Revert => ParityRow::live(a, &["revert"]),
        P::Lease | P::ReleaseLease | P::BreakLease => ParityRow::not_applicable(
            a,
            &[],
            &["lease", "lease --renew", "lease --release", "lease --break"],
            "grids merge; leases are tier 0's",
        ),
        P::RowInsert => ParityRow::live(a, &["row insert"]),
        P::RowMove => ParityRow::live(a, &["row move"]),
        P::RowDelete => ParityRow::live(a, &["row delete"]),
        P::ColInsert => ParityRow::live(a, &["col insert"]),
        P::ColMove => ParityRow::live(a, &["col move"]),
        P::ColDelete => ParityRow::live(a, &["col delete"]),
        P::ColRename => ParityRow::live(a, &["col rename", "header"]),
        P::Sort => ParityRow::live(a, &["sort"]),
        P::RestoreElement => ParityRow::live(a, &["row restore", "col restore"]),
        P::DropDeleted => ParityRow::live(a, &["rows drop-deleted"]),
    }
}

/// The grid actions' rows.
fn rows() -> Vec<ParityRow> {
    PersonAction::for_tier(Tier::Grid).map(row).collect()
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

// ---------- the pack's verbs: person and agent paths ----------

type R = Result<Step, String>;

fn summary(r: &EditResult) -> Output {
    Output::Value(format!(
        "v{} {:?} hidden {}",
        r.version,
        r.elems,
        r.hidden.len()
    ))
}

/// The tab's table op, as one change.
fn person_op(p: &mut Person<'_>, f: impl FnOnce(&mut Table) -> Result<Vec<Edit>, TableError>) -> R {
    Ok(match p.grid_edit(f) {
        Ok(Some(r)) => Step::ok(summary(&r)),
        Ok(None) => Step::ok(Output::Value("no change".into())),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}

/// The agent's verb, guarded by the version it last read (an agent's structured edit names a
/// base).
fn agent_op(a: &mut Agent<'_>, argv: &[&str], stdin: &str) -> R {
    let v = a.stage().version()?.to_string();
    let mut args: Vec<&str> = argv.to_vec();
    args.extend(["--if-version", &v]);
    let o = a.verb(&args, stdin.as_bytes())?;
    if o.refused.is_some() {
        return Ok(Step::exit(o.exit, Output::None));
    }
    Ok(Step::exit(
        o.exit,
        match &o.value {
            VerbValue::Tool(t) => match &t.changed {
                Some(r) => summary(r),
                None => Output::Value("no change".into()),
            },
            v => Output::value(v),
        },
    ))
}

fn set_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.set_cell(&k("r2"), &k("c2"), "Roma"))
}
fn set_agent(a: &mut Agent<'_>) -> R {
    agent_op(
        a,
        &[
            "cell", "set", "--row", "r2", "--col", "c2", "--value", "Roma",
        ],
        "",
    )
}
fn set_deleted_person(p: &mut Person<'_>) -> R {
    // a deleted row's cell: the tab can't reach it (exit 4, nothing applied)
    p.conn()?;
    Ok(Step::exit(4, Output::None))
}
fn set_deleted_agent(a: &mut Agent<'_>) -> R {
    let v = a.stage().version()?.to_string();
    let o = a.verb(
        &[
            "cell",
            "set",
            "--row",
            "r3",
            "--col",
            "c2",
            "--value",
            "x",
            "--if-version",
            &v,
        ],
        b"",
    )?;
    Ok(Step::exit(o.exit, Output::None))
}
fn p2_deletes_r3(stage: &mut Stage) -> Result<(), String> {
    stage.grid_as("p2", |t| t.delete_row(&k("r3")))?;
    Ok(())
}
fn paste_person(p: &mut Person<'_>) -> R {
    let block = vec![
        vec!["k8".to_owned(), "Accra".to_owned()],
        vec!["k9".to_owned(), "Bonn".to_owned()],
    ];
    person_op(p, move |t| t.set_block(&k("r3"), &k("c1"), &block))
}
fn paste_agent(a: &mut Agent<'_>) -> R {
    agent_op(
        a,
        &["cells", "set", "--from", "r3,c1"],
        "k8,Accra\nk9,Bonn\n",
    )
}
fn clear_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| {
        t.clear_block((&k("r1"), &k("c1")), (&k("r2"), &k("c2")))
    })
}
fn clear_agent(a: &mut Agent<'_>) -> R {
    agent_op(
        a,
        &["cells", "clear", "--from", "r1,c1", "--to", "r2,c2"],
        "",
    )
}
fn get_person(p: &mut Person<'_>) -> R {
    let t = p.table()?;
    let v = t.cell(&k("r2"), &k("c2")).unwrap_or("").to_owned();
    Ok(Step::ok(Output::Text(format!("{v}\n"))))
}
fn get_agent(a: &mut Agent<'_>) -> R {
    let o = a.verb(&["cell", "get", "--row", "r2", "--col", "c2"], b"")?;
    Ok(Step::exit(
        o.exit,
        Output::Text(String::from_utf8_lossy(o.stdout()).into_owned()),
    ))
}
fn row_list_person(p: &mut Person<'_>) -> R {
    // the tab's row view: every live row, its number and lines, its values
    let t = p.table()?;
    let (out, _) = verbs::row_listing(&t, &t.line_map(), &t.live_cols(), None, None);
    Ok(Step::ok(Output::Text(out)))
}
fn row_list_agent(a: &mut Agent<'_>) -> R {
    let o = a.verb(&["row", "list"], b"")?;
    Ok(Step::exit(
        o.exit,
        Output::Text(String::from_utf8_lossy(o.stdout()).into_owned()),
    ))
}
fn col_list_person(p: &mut Person<'_>) -> R {
    let t = p.table()?;
    Ok(Step::ok(Output::Text(verbs::col_listing(&t))))
}
fn col_list_agent(a: &mut Agent<'_>) -> R {
    let o = a.verb(&["col", "list"], b"")?;
    Ok(Step::exit(
        o.exit,
        Output::Text(String::from_utf8_lossy(o.stdout()).into_owned()),
    ))
}
fn header_off_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.set_header(false))
}
fn header_off_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["header", "off"], "")
}
fn row_insert_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| {
        t.insert_row(Anchor::After(k("r1")), &["k5".into(), "Baku".into()])
            .map(|x| x.1)
    })
}
fn row_insert_agent(a: &mut Agent<'_>) -> R {
    agent_op(
        a,
        &["row", "insert", "--after", "r1", "--values", "k5,Baku"],
        "",
    )
}
fn row_move_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.move_row(&k("r4"), Anchor::Start))
}
fn row_move_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["row", "move", "--row", "r4", "--start"], "")
}
fn row_delete_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.delete_row(&k("r2")))
}
fn row_delete_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["row", "delete", "--row", "r2"], "")
}
fn row_restore_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.restore_row(&k("r3")))
}
fn row_restore_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["row", "restore", "--row", "r3"], "")
}
fn col_insert_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| {
        t.insert_col(Anchor::After(k("c1")), Some("age"))
            .map(|x| x.1)
    })
}
fn col_insert_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["col", "insert", "--after", "c1", "--name", "age"], "")
}
fn col_move_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.move_col(&k("c2"), Anchor::Start))
}
fn col_move_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["col", "move", "--col", "c2", "--start"], "")
}
fn col_delete_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.delete_col(&k("c1")))
}
fn col_delete_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["col", "delete", "--col", "c1"], "")
}
fn p2_deletes_c2(stage: &mut Stage) -> Result<(), String> {
    stage.grid_as("p2", |t| t.delete_col(&k("c2")))?;
    Ok(())
}
fn col_restore_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.restore_col(&k("c2")))
}
fn col_restore_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["col", "restore", "--col", "c2"], "")
}
fn col_rename_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.rename_col(&k("c2"), "town"))
}
fn col_rename_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["col", "rename", "--col", "c2", "--name", "town"], "")
}
fn sort_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.sort(&[SortKey::by(k("c2"))]))
}
fn sort_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["sort", "--by", "c2"], "")
}
fn row_move_r2_person(p: &mut Person<'_>) -> R {
    person_op(p, |t| t.move_row(&k("r2"), Anchor::End))
}
fn row_move_r2_agent(a: &mut Agent<'_>) -> R {
    agent_op(a, &["row", "move", "--row", "r2", "--end"], "")
}
/// P2 claims row `r2`; the actor moves it (an agent under the host's `Respect` policy would
/// be refused; `run_tool`'s default is `Ignore`): the change lands, p2's claim stays.
fn p2_claims_r2(stage: &mut Stage) -> Result<(), String> {
    let c = stage.other("p2", k2_mpc_base::session::ParticipantKind::Person)?;
    stage
        .session()
        .claim_span(
            c,
            k2_mpc_base::session::ClaimSpan::Rows {
                first: k("r2"),
                last: k("r2"),
            },
            None,
            None,
            false,
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The pack's own cases (P3's `cell get`, P9-P13, P35-P43).
fn pack_cases() -> Vec<GridCase> {
    use PersonAction as P;
    let c = GridCase::new;
    vec![
        c(
            P::ReadAll,
            "cell get",
            "a cell's value",
            get_person,
            get_agent,
        ),
        c(
            P::ReadAll,
            "row list",
            "the rows",
            row_list_person,
            row_list_agent,
        ),
        c(
            P::ReadAll,
            "col list",
            "the columns",
            col_list_person,
            col_list_agent,
        ),
        c(P::Type, "cell set", "one cell", set_person, set_agent),
        c(
            P::Type,
            "cell set",
            "a cell of a row p2 deleted: 4",
            set_deleted_person,
            set_deleted_agent,
        )
        .setup(p2_deletes_r3),
        c(
            P::TypeOver,
            "cell set",
            "over a value",
            set_person,
            set_agent,
        ),
        c(
            P::FindReplace,
            "cell set",
            "the match's cell",
            set_person,
            set_agent,
        ),
        c(P::Paste, "cells set", "a block", paste_person, paste_agent),
        c(
            P::Delete,
            "cells clear",
            "a rectangle",
            clear_person,
            clear_agent,
        ),
        c(
            P::Delete,
            "row delete",
            "a row",
            row_delete_person,
            row_delete_agent,
        ),
        c(
            P::RowInsert,
            "row insert",
            "after r1",
            row_insert_person,
            row_insert_agent,
        ),
        c(
            P::RowMove,
            "row move",
            "to the start",
            row_move_person,
            row_move_agent,
        ),
        c(
            P::RowMove,
            "row move",
            "a row p2 claimed (policy Ignore): it lands",
            row_move_r2_person,
            row_move_r2_agent,
        )
        .setup(p2_claims_r2),
        c(
            P::RowDelete,
            "row delete",
            "hidden, cells kept",
            row_delete_person,
            row_delete_agent,
        ),
        c(
            P::ColInsert,
            "col insert",
            "after c1, named",
            col_insert_person,
            col_insert_agent,
        ),
        c(
            P::ColMove,
            "col move",
            "to the start",
            col_move_person,
            col_move_agent,
        ),
        c(
            P::ColDelete,
            "col delete",
            "hidden",
            col_delete_person,
            col_delete_agent,
        ),
        c(
            P::ColRename,
            "col rename",
            "the header",
            col_rename_person,
            col_rename_agent,
        ),
        c(
            P::ColRename,
            "header",
            "the header off: names become the first row",
            header_off_person,
            header_off_agent,
        ),
        c(
            P::Sort,
            "sort",
            "by city: the minimal moves",
            sort_person,
            sort_agent,
        ),
        c(
            P::RestoreElement,
            "row restore",
            "p2's deleted row, with its cells",
            row_restore_person,
            row_restore_agent,
        )
        .setup(p2_deletes_r3),
        c(
            P::RestoreElement,
            "col restore",
            "p2's deleted column",
            col_restore_person,
            col_restore_agent,
        )
        .setup(p2_deletes_c2),
    ]
}

#[test]
fn agent_parity_grid_stage() {
    let t = Instant::now();
    let report = parity::check_grid(pack(), &rows(), &registry(), &pack_cases())
        .unwrap_or_else(|f| panic!("{}", parity::render(&f)));
    let live = rows()
        .iter()
        .filter(|r| r.status == RowStatus::Live)
        .count();
    // every grid row is live but P15 and the leases (N/A: not grid rows) and P23, P27, P28
    // (deferred); P24 (exports) is live since the merge with v0.9.7
    assert_eq!(rows().len(), 40, "grid-tier rows");
    assert_eq!(live, 37, "rows live");
    // P24 was the one N/A row the report saw (P15 is a text edit row, N/A by the list)
    assert_eq!(
        report.not_applicable.len(),
        0,
        "{:?}",
        report.not_applicable
    );
    assert_eq!(report.deferred.len(), 3);
    // every grid row of the contract (P35-P44) ran
    for a in PersonAction::for_tier(Tier::Grid).filter(|a| a.number() >= 35) {
        assert!(
            report.matched.iter().any(|m| m.0 == a),
            "{a}: no case matched"
        );
    }
    assert!(report.matched.len() >= 70, "{} cases", report.matched.len());
    eprintln!(
        "agent parity, grid stage: {} cases matched over {live} live rows in {:?}",
        report.matched.len(),
        t.elapsed()
    );
}

/// The grid registry: the kit's grid verbs and the pack's structured ones, no text edit
/// verbs, no leases.
#[test]
fn grid_registry_is_the_kit_grid_verbs_and_the_packs() {
    let r = registry();
    assert_eq!(r.tier(), Tier::Grid);
    for v in [
        "cat",
        "head",
        "grep",
        "claim",
        "write",
        "blame",
        "rows drop-deleted",
    ] {
        assert!(r.get(v).is_some(), "{v}");
    }
    for v in ["insert", "replace", "sed -i", "patch", "lease"] {
        assert!(r.get(v).is_none(), "{v} on a grid");
    }
    let rd = r.get("rows drop-deleted").unwrap();
    assert!(rd.needs.contains(&k2_mpc_base::session::Need::Owner));
    let get = r.get("cell get").unwrap();
    assert_eq!(get.kind, parity::VerbKind::Read);
    assert!(!get.needs.contains(&k2_mpc_base::session::Need::Edit));
    let set = r.get("cell set").unwrap();
    assert_eq!(set.api, &[parity::Api::RunTool, parity::Api::EditOps]);
    assert_eq!(set.tiers, &[Tier::Grid]);
    // the two-word forms
    let argv = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert_eq!(r.form_of(&argv(&["row", "insert", "--end"])), "row insert");
    assert_eq!(r.form_of(&argv(&["sort", "--by", "c1"])), "sort");
    assert_eq!(
        r.form_of(&argv(&["rows", "drop-deleted"])),
        "rows drop-deleted"
    );
    assert_eq!(
        r.form_of(&argv(&["blame", "--structure"])),
        "blame --structure"
    );
    // every grid-tier action with a live row has a verb
    for a in PersonAction::for_tier(Tier::Grid) {
        if row(a).status == RowStatus::Live {
            assert!(r.serving(a).count() > 0, "{a}");
        }
    }
}

/// The kit's verb registry refuses a pack verb of the wrong kind or name.
#[test]
fn registry_refuses_mixed_or_shadowing_pack_verbs() {
    fn noop(_: &[String]) -> Result<k2_mpc_base::tools::StructEdit, ToolError> {
        Ok(Box::new(|_| Ok(StructOutput::default())))
    }
    fn text_noop(_: &[String]) -> Result<k2_mpc_base::tools::PackEdit, ToolError> {
        Err(ToolError::usage("never"))
    }
    struct With(&'static [PackVerb], bool);
    impl Pack for With {
        fn schema(&self) -> k2_mpc_base::schema::Schema {
            if self.1 {
                grid_pack::LITE
            } else {
                TextFilePack::new("t", 64).schema()
            }
        }
        fn decode(
            &self,
            _: &[u8],
        ) -> Result<k2_mpc_base::session::PackContent, k2_mpc_base::session::Declined> {
            unreachable!()
        }
        fn encode(
            &self,
            _: &[(String, k2_mpc_base::doc::Neutral)],
            _: &[u8],
        ) -> Result<Vec<u8>, k2_mpc_base::session::PackError> {
            unreachable!()
        }
        fn verbs(&self) -> &'static [PackVerb] {
            self.0
        }
    }
    const S: &[PersonAction] = &[PersonAction::Sort];
    let refused = |verbs: &'static [PackVerb], grid: bool, why: &str| {
        let e = Registry::for_pack(&With(verbs, grid)).unwrap_err();
        assert!(e.to_string().contains(why), "{e}");
    };
    refused(
        {
            const V: &[PackVerb] = &[PackVerb::structured_edit("row x", "", S, &[0], noop)];
            V
        },
        false,
        "a structured verb on a text pack",
    );
    refused(
        {
            const V: &[PackVerb] = &[PackVerb::edit("rowx", "", S, &[0], text_noop)];
            V
        },
        true,
        "a text verb on a grid pack",
    );
    refused(
        {
            const V: &[PackVerb] = &[PackVerb::structured_edit("rows purge", "", S, &[0], noop)];
            V
        },
        true,
        "already a verb",
    );
    refused(
        {
            const V: &[PackVerb] = &[PackVerb::structured_edit("blame row", "", S, &[0], noop)];
            V
        },
        true,
        "already a verb",
    );
    refused(
        {
            const V: &[PackVerb] = &[PackVerb::structured_edit("Row Insert", "", S, &[0], noop)];
            V
        },
        true,
        "lowercase",
    );
    refused(
        {
            const V: &[PackVerb] = &[PackVerb::structured_edit("a b c", "", S, &[0], noop)];
            V
        },
        true,
        "lowercase",
    );
    refused(
        {
            const V: &[PackVerb] = &[PackVerb::structured_edit("row x", "", &[], &[0], noop)];
            V
        },
        true,
        "serves no",
    );
    const OK: &[PackVerb] = &[PackVerb::structured_edit("row x", "", S, &[0], noop)];
    assert!(Registry::for_pack(&With(OK, true)).is_ok());
}

/// Negative controls: the grid stage fails loudly.
#[test]
fn grid_stage_negative_controls() {
    // a pack verb missing from the registry: its row's verb and its cases are reported
    let f = parity::check_grid_list(&rows(), &registry().without("row move"), &pack_cases());
    assert!(
        f.iter()
            .any(|x| matches!(x, Failure::MissingVerb { verb, .. } if verb == "row move")),
        "{}",
        parity::render(&f)
    );
    // a grid row missing from the list
    let mut r = rows();
    r.retain(|x| x.action != PersonAction::Sort);
    let f = parity::check_grid_list(&r, &registry(), &pack_cases());
    assert!(
        f.contains(&Failure::MissingRow(PersonAction::Sort)),
        "{}",
        parity::render(&f)
    );
    // a live row with no case
    let mut few = pack_cases();
    few.retain(|c| c.verb != "col rename");
    let f = parity::check_grid_list(&rows(), &registry(), &few);
    assert!(
        f.iter()
            .any(|x| matches!(x, Failure::NoCase { verb, .. } if verb == "col rename")),
        "{}",
        parity::render(&f)
    );
    // the grid stage refuses a text registry, and the text harness a grid one
    let f = parity::check_grid_list(&rows(), &Registry::kit(), &[]);
    assert!(
        matches!(&f[..], [Failure::Registry(m)] if m.contains("Text")),
        "{}",
        parity::render(&f)
    );
    let f = parity::check_list(&rows(), &registry(), &[]);
    assert!(
        matches!(&f[..], [Failure::Registry(m)] if m.contains("Grid")),
        "{}",
        parity::render(&f)
    );
    // a planted divergence: the agent's verb sorts descending; the grid by key differs
    fn sort_desc_agent(a: &mut Agent<'_>) -> R {
        agent_op(a, &["sort", "--by", "c2", "--desc"], "")
    }
    let bad = GridCase::new(
        PersonAction::Sort,
        "sort",
        "planted: desc",
        sort_person,
        sort_desc_agent,
    );
    let f = parity::run_grid_case(&pack(), &registry(), &bad);
    assert!(
        f.iter().any(|x| matches!(x, Failure::Mismatch { diffs, .. } if diffs.iter().any(|d| d.starts_with("content")))),
        "{}",
        parity::render(&f)
    );
    // a planted divergence in presence only: the agent also takes a claim
    fn set_and_claim_agent(a: &mut Agent<'_>) -> R {
        let s = set_agent(a)?;
        a.verb(&["claim", "--rows", "r1..r1"], b"")?;
        Ok(s)
    }
    let bad = GridCase::new(
        PersonAction::Type,
        "cell set",
        "planted: a claim",
        set_person,
        set_and_claim_agent,
    );
    let f = parity::run_grid_case(&pack(), &registry(), &bad);
    assert!(
        f.iter().any(|x| matches!(x, Failure::Mismatch { diffs, .. } if diffs.iter().any(|d| d.starts_with("actor")))),
        "{}",
        parity::render(&f)
    );
    // a planted wrong exit: the person path says 2 where the verb says 4
    fn set_deleted_says_2(p: &mut Person<'_>) -> R {
        p.conn()?;
        Ok(Step::exit(2, Output::None))
    }
    let f = parity::run_grid_case(
        &pack(),
        &registry(),
        &GridCase::new(
            PersonAction::Type,
            "cell set",
            "planted: exit 2",
            set_deleted_says_2,
            set_deleted_agent,
        )
        .setup(p2_deletes_r3),
    );
    assert!(
        f.iter().any(|x| matches!(x, Failure::Mismatch { diffs, .. } if diffs.iter().any(|d| d.starts_with("exit")))),
        "{}",
        parity::render(&f)
    );
    // a text pack in the grid stage is refused
    let f = parity::check_grid(
        Arc::new(TextFilePack::new("t", 1 << 20)),
        &rows(),
        &registry(),
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(&f[..], [Failure::Registry(m)] if m.contains("table pack")),
        "{}",
        parity::render(&f)
    );
}

/// What each case did (person path), keyed by its label: the exit, the output, the changes'
/// element counts and the actor's presence by key.
fn outcomes() -> Vec<(String, k2_mpc_base::parity::GridObservation)> {
    let mut all: Vec<GridCase> = PersonAction::ALL
        .iter()
        .flat_map(|a| parity::kit_grid_cases(*a))
        .collect();
    all.extend(pack_cases());
    all.iter()
        .map(|c| {
            let (p, _, _) = parity::observe_grid_case(&pack(), &registry(), c)
                .unwrap_or_else(|f| panic!("{}", parity::render(&f)));
            (format!("{c:?}"), p)
        })
        .collect()
}

/// The cases are not trivially equal: what the person path did in the cases that carry the
/// grid tier's rules (the agent path agreed, `agent_parity_grid_stage`).
#[test]
fn grid_cases_do_what_they_say() {
    let all = outcomes();
    let get = |label: &str| {
        all.iter()
            .find(|(c, _)| c.contains(label))
            .map(|(_, o)| o.clone())
            .unwrap_or_else(|| panic!("no case {label:?}"))
    };
    let actor = |label: &str| {
        get(label)
            .actor
            .unwrap_or_else(|| panic!("{label}: no actor"))
    };
    // views and claims are rows by key
    assert_eq!(actor("lines 2-3: a view of their rows").view, Some((2, 3)));
    let fe = actor("--for-edit");
    assert_eq!(
        fe.claims
            .iter()
            .map(|c| (c.span.to_string(), c.lines, c.ttl_ms))
            .collect::<Vec<_>>(),
        [("rows r1–r2".to_owned(), (2, 3), 30_000)]
    );
    assert_eq!(get("someone else's claim overlaps").exit, 5);
    assert!(
        matches!(&get("p2's cell caret").output, Output::Value(v) if v.contains("anchor: (Key(\"r2\"), Key(\"c2\"))"))
    );
    assert_eq!(
        actor("a selection of cells").cell,
        Some(((k("r1"), k("c1")), (k("r3"), k("c2"))))
    );
    // write --base: the conflict names who wrote and who claims the region
    let w = get("a cell p2 set and claimed since");
    assert_eq!(w.exit, 1);
    assert!(
        matches!(&w.output, Output::Value(v) if v.contains(r#"claimed [\"p2\"]"#) && v.contains(r#"by Some(\"p2\")"#)),
        "{:?}",
        w.output
    );
    assert_eq!(get("an unknown base: 8").exit, 8);
    // undo --partial skips the cell p2 wrote since
    let u = get("the cell nobody touched");
    assert_eq!(u.exit, 1);
    assert_eq!(
        u.changes.last().map(|c| c.kind),
        Some(k2_mpc_base::session::ChangeKind::Undo)
    );
    // blame: a row moved by p2 still blames its content's author
    let b = get("lines to their rows' latest cell writes");
    assert!(
        matches!(&b.output, Output::Value(v) if v.contains("line: 2, text: \"k3,Lima\", author: \"kit:import\"")),
        "{:?}",
        b.output
    );
    assert!(
        matches!(&get("who inserted and who moved each row").output, Output::Value(v) if v.contains("author: \"p2\""))
    );
    // the grid rows
    assert_eq!(get("not the owner: 3").exit, 3);
    assert_eq!(
        get("the owner frees p2's deleted row").changes[0].elems[0].dropped,
        1
    );
    assert_eq!(get("a cell of a row p2 deleted: 4").exit, 4);
    let ins = actor("after r1");
    assert_eq!(
        ins.cell,
        Some(((k("r5"), k("c2")), (k("r5"), k("c2")))),
        "the caret is the last cell written"
    );
    assert_eq!(ins.last_edit, Some((3, 3)));
    let sorted = get("by city: the minimal moves");
    assert!(sorted.changes[0].elems[0].moved > 0);
    assert_eq!(sorted.actor.unwrap().cell, None, "a sort writes no cell");
}
