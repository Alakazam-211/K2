//! The kit's parity cases on a grid doc (A11): for every grid-tier action the kit binds, the
//! person path and the agent path through each kit verb, on [`super::GRID_DOC`]. One
//! exhaustive `match` ([`kit_grid_cases`]); the grid actions' own verbs (P9-P13's `cell set`,
//! `cells set`, `cells clear`; P35-P43) are a pack's, so their cases are the pack's.
//!
//! The agent edits only through registered verbs: the kit's grid edit verbs are `write --base
//! V` (merged by key) and `rows drop-deleted`, so a case that needs the actor's own edit
//! makes it with `write --base V` on the agent path and with the table op the tab sends
//! ([`Person::grid_edit`]) on the person path. Others act through [`Stage::grid_as`].

use super::action::PersonAction;
use super::cases::*;
use super::grid::GridCase;
use super::harness::{Agent, Output, Person, Stage, Step, VerbOut, VerbValue, ACTOR};
use crate::elem::{Anchor, Key};
use crate::session::{
    ClaimInfo, ClaimSpan, GridWrite, GridWriteOptions, GridWriteReport, LineRange, ParticipantKind,
    Rights, UndoMode,
};
use crate::session::{ElemSelection, Point, Rect};
use crate::table::Table;
use crate::tools::{self, session_exit};

fn k(s: &str) -> Key {
    Key::new(s).expect("a key")
}

/// The rendered file, as the tab (or observer) reads it.
fn rendered_of(p: &mut Person<'_>) -> Result<String, String> {
    let c = p.conn()?;
    let r = p.session().read(c).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&r.bytes).into_owned())
}

fn stage_rendered(s: &Stage) -> Result<String, String> {
    let r = s
        .session()
        .read(s.observer())
        .map_err(|e| format!("observer read: {e}"))?;
    Ok(String::from_utf8_lossy(&r.bytes).into_owned())
}

/// Lines `first..=last` of `text`, each with its `\n`.
fn lines_of(text: &str, first: usize, last: usize) -> String {
    text.split_inclusive('\n')
        .skip(first - 1)
        .take(last + 1 - first)
        .collect()
}

/// The body a `write --base` sends: the rendered file with `from` replaced by `to` once.
fn rewritten(text: &str, from: &str, to: &str) -> Result<String, String> {
    if !text.contains(from) {
        return Err(format!("{from:?} is not in the file"));
    }
    Ok(text.replacen(from, to, 1))
}

// ---------- shared setups ----------

/// Another person, `p2`, joins.
fn p2(stage: &mut Stage) -> Result<(), String> {
    stage.other("p2", ParticipantKind::Person)?;
    Ok(())
}

/// P2 (a person) sets `r2/c2` (`k2`'s city) to `Paris`.
fn p2_sets(stage: &mut Stage) -> Result<(), String> {
    stage.grid_as("p2", |t| t.set_cell(&k("r2"), &k("c2"), "Paris"))?;
    Ok(())
}

/// The import version (mark `v`), then p2's edit.
fn grid_history_setup(stage: &mut Stage) -> Result<(), String> {
    let first = stage
        .versions()?
        .first()
        .map(|v| v.id)
        .ok_or("no import version")?;
    stage.set_mark("v", first);
    p2_sets(stage)
}

/// An assistant ("aide") sets `r1/c2` on the actor's behalf; mark `c` is its change.
fn aide_grid_setup(stage: &mut Stage) -> Result<(), String> {
    stage.assistant("aide", ACTOR)?;
    let seq = stage.grid_as("aide", |t| t.set_cell(&k("r1"), &k("c2"), "Bergen"))?;
    stage.set_mark("c", seq);
    Ok(())
}

fn p30_grid_setup(stage: &mut Stage) -> Result<(), String> {
    stage.assistant("aide", ACTOR)?;
    stage.grid_as("aide", |t| t.set_cell(&k("r1"), &k("c2"), "Bergen"))?;
    stage.assistant("bot", "p2")?;
    stage.grid_as("bot", |t| t.set_cell(&k("r2"), &k("c2"), "Milan"))?;
    stage.grid_as("aide", |t| t.set_cell(&k("r3"), &k("c2"), "Cusco"))?;
    Ok(())
}

fn p30_all_grid_setup(stage: &mut Stage) -> Result<(), String> {
    p30_grid_setup(stage)?;
    let first = stage.grid_as("bot", |t| t.set_cell(&k("r2"), &k("c1"), "K2x"))?;
    let second = stage.grid_as("bot", |t| t.set_cell(&k("r2"), &k("c2"), "Turin"))?;
    let p2 = stage.other("p2", ParticipantKind::Person)?;
    for (seq, state) in [
        (first, crate::session::ApprovalState::Approved),
        (second, crate::session::ApprovalState::Declined),
    ] {
        stage
            .session()
            .set_approval(p2, seq, state)
            .map_err(|e| format!("p2 settles {seq}: {e}"))?;
    }
    Ok(())
}

// ---------- P4, P5: views and find on the rendered file ----------

fn grid_view_person(p: &mut Person<'_>, first: usize, last: usize) -> R {
    let c = p.conn()?;
    let root = p.root().to_owned();
    let text = rendered_of(p)?;
    p.session()
        .set_view(c, LineRange { root, first, last })
        .map_err(|e| e.to_string())?;
    Ok(Step::ok(Output::Text(lines_of(&text, first, last))))
}
fn g4_view_person(p: &mut Person<'_>) -> R {
    grid_view_person(p, 2, 3)
}
fn g4_view_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["view", "--lines", "2,3"], b"")?, stdout_text)
}
fn g4_head_person(p: &mut Person<'_>) -> R {
    grid_view_person(p, 1, 2)
}
fn g4_head_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["head", "-n", "2"], b"")?, stdout_text)
}
fn g4_tail_person(p: &mut Person<'_>) -> R {
    grid_view_person(p, 4, 5)
}
fn g4_tail_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["tail", "-n", "2"], b"")?, stdout_text)
}
fn g4_sed_person(p: &mut Person<'_>) -> R {
    grid_view_person(p, 3, 4)
}
fn g4_sed_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["sed", "-n", "3,4p"], b"")?, stdout_text)
}
fn g4_for_edit_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let root = p.root().to_owned();
    let text = rendered_of(p)?;
    let r = LineRange {
        root,
        first: 2,
        last: 3,
    };
    p.session()
        .set_view(c, r.clone())
        .map_err(|e| e.to_string())?;
    let span = ClaimSpan::Lines(r);
    person_step(
        p.session().claim_span(c, span, Some(30_000), None, false),
        |_| Output::Text(lines_of(&text, 2, 3)),
    )
}
fn g4_for_edit_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["view", "--lines", "2,3", "--for-edit"], b"")?,
        stdout_text,
    )
}

fn g5_person(p: &mut Person<'_>) -> R {
    let t = rendered_of(p)?;
    let hits: Vec<usize> = t
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("Rome"))
        .map(|(i, _)| i + 1)
        .collect();
    let exit = if hits.is_empty() { 1 } else { 0 };
    Ok(Step::exit(exit, Output::Lines(hits)))
}
fn g5_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["grep", "-n", "Rome"], b"")?, |o| {
        Output::Lines(
            String::from_utf8_lossy(o.stdout())
                .lines()
                .filter_map(|l| l.split_once(':').and_then(|(n, _)| n.parse().ok()))
                .collect(),
        )
    })
}

// ---------- P7, P8: carets and claims by key ----------

fn p2_caret(stage: &mut Stage) -> Result<(), String> {
    let c = stage.other("p2", ParticipantKind::Person)?;
    stage
        .session()
        .set_cell_cursor(c, (&k("r2"), &k("c2")), (&k("r2"), &k("c2")))
        .map_err(|e| format!("p2's caret: {e}"))?;
    Ok(())
}
fn g7_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().roster(c), |r| {
        let cell = r
            .into_iter()
            .find(|e| e.participant.pid == "p2")
            .and_then(|e| e.cell);
        Output::value(cell)
    })
    .map(|s| match &s.output {
        Output::Value(v) if v == "None" => Step::exit(1, s.output),
        _ => s,
    })
}
fn g7_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["follow", "p2"], b"")?, |o| match &o.value {
        VerbValue::Cell(c) => Output::value(c),
        v => Output::value(v),
    })
}

fn g8_caret_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(
        p.session()
            .set_cell_cursor(c, (&k("r2"), &k("c2")), (&k("r2"), &k("c2"))),
        |_| Output::None,
    )
}
fn g8_caret_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["cursor", "--cell", "r2,c2"], b"")?, |_| {
        Output::None
    })
}
fn g8_select_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(
        p.session()
            .set_cell_cursor(c, (&k("r1"), &k("c1")), (&k("r3"), &k("c2"))),
        |_| Output::None,
    )
}
fn g8_select_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["cursor", "--cell", "r1,c1", "--to", "r3,c2"], b"")?,
        |_| Output::None,
    )
}

fn g8_elems_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let s = ElemSelection::new(crate::table::ROWS, vec![k("r1"), k("r3")]);
    person_step(p.session().set_selection(c, Some(s)), |_| Output::None)
}
fn g8_elems_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["select", "--root", "rows", "--keys", "r1,r3"], b"")?,
        |_| Output::None,
    )
}
fn g8_pointer_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let at = Point::new(12.5, -3.0).map_err(|e| e.to_string())?;
    person_step(p.session().set_pointer(c, Some(at)), |_| Output::None)
}
fn g8_pointer_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["pointer", "--at", "12.5,-3"], b"")?, |_| {
        Output::None
    })
}
fn g4_viewport_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let r = Rect::new(0.0, 10.0, 640.0, 480.5).map_err(|e| e.to_string())?;
    person_step(p.session().set_viewport(c, Some(r)), |_| Output::None)
}
fn g4_viewport_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["viewport", "--rect", "0,10,640,480.5"], b"")?,
        |_| Output::None,
    )
}

fn grid_claim_desc(c: &ClaimInfo) -> Output {
    Output::Value(format!(
        "{} lines {}-{} hold {} note {:?}",
        c.span, c.lines.first, c.lines.last, c.hold, c.note
    ))
}
fn grid_claim_out(o: &VerbOut) -> Output {
    match &o.value {
        VerbValue::Claim(c) => grid_claim_desc(c),
        VerbValue::ClaimsReleased(r) => Output::Value(format!("released {}", r.len())),
        v => Output::value(v),
    }
}
fn person_span(
    p: &mut Person<'_>,
    span: ClaimSpan,
    ttl: Option<u64>,
    note: Option<&str>,
    hold: bool,
) -> R {
    let c = p.conn()?;
    person_step(
        p.session()
            .claim_span(c, span, ttl, note.map(str::to_owned), hold),
        |i| grid_claim_desc(&i),
    )
}
fn rows_span(a: &str, b: &str) -> ClaimSpan {
    ClaimSpan::Rows {
        first: k(a),
        last: k(b),
    }
}
fn g8_rows_person(p: &mut Person<'_>) -> R {
    person_span(
        p,
        rows_span("r1", "r2"),
        Some(60_000),
        Some("dedupe"),
        false,
    )
}
fn g8_rows_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(
            &[
                "claim", "--rows", "r1..r2", "--ttl", "60000", "--note", "dedupe",
            ],
            b"",
        )?,
        grid_claim_out,
    )
}
fn g8_cols_person(p: &mut Person<'_>) -> R {
    let span = ClaimSpan::Cols {
        first: k("c2"),
        last: k("c2"),
    };
    person_span(p, span, None, None, false)
}
fn g8_cols_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["claim", "--cols", "c2..c2"], b"")?, grid_claim_out)
}
fn g8_cells_person(p: &mut Person<'_>) -> R {
    let span = ClaimSpan::Cells {
        rows: (k("r1"), k("r2")),
        cols: (k("c1"), k("c2")),
    };
    person_span(p, span, None, None, true)
}
fn g8_cells_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["claim", "--cells", "r1,c1..r2,c2", "--hold"], b"")?,
        grid_claim_out,
    )
}
fn g8_lines_person(p: &mut Person<'_>) -> R {
    let root = p.root().to_owned();
    let span = ClaimSpan::Lines(LineRange {
        root,
        first: 2,
        last: 3,
    });
    person_span(p, span, None, None, false)
}
fn g8_lines_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["claim", "--lines", "2,3"], b"")?, grid_claim_out)
}
fn p2_claims_r3(stage: &mut Stage) -> Result<(), String> {
    let c = stage.other("p2", ParticipantKind::Agent)?;
    stage
        .session()
        .claim_span(c, rows_span("r3", "r3"), None, Some("mine".into()), false)
        .map_err(|e| format!("p2's claim: {e}"))?;
    Ok(())
}
fn g8_claimed_person(p: &mut Person<'_>) -> R {
    person_span(p, rows_span("r2", "r3"), None, None, false)
}
fn g8_claimed_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["claim", "--rows", "r2..r3"], b"")?, grid_claim_out)
}
fn g8_renew_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    p.session()
        .claim_span(c, rows_span("r1", "r2"), None, None, false)
        .map_err(|e| e.to_string())?;
    person_step(
        p.session().renew_claim(c, None, Some(120_000), Some(true)),
        |i| grid_claim_desc(&i),
    )
}
fn g8_renew_agent(a: &mut Agent<'_>) -> R {
    a.verb(&["claim", "--rows", "r1..r2"], b"")?;
    agent_step(
        a.verb(&["claim", "--renew", "--ttl", "120000", "--hold"], b"")?,
        grid_claim_out,
    )
}
fn g8_release_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    p.session()
        .claim_span(c, rows_span("r1", "r2"), None, None, false)
        .map_err(|e| e.to_string())?;
    person_step(p.session().release_claim(c, None), |r| {
        Output::Value(format!("released {}", r.len()))
    })
}
fn g8_release_agent(a: &mut Agent<'_>) -> R {
    a.verb(&["claim", "--rows", "r1..r2"], b"")?;
    agent_step(a.verb(&["claim", "--release"], b"")?, grid_claim_out)
}

// ---------- P14: write --base, merged by key ----------

/// A grid write's observable summary: exit, what applied (element counts), each conflict's
/// rule, region, who wrote it and who claims it.
fn grid_write_summary(r: &GridWriteReport) -> String {
    let conflicts = |c: &[crate::session::GridConflict]| -> Vec<String> {
        c.iter()
            .map(|c| {
                format!(
                    "{} at {} by {:?} claimed {:?} lines {:?}",
                    c.conflict.rule, c.conflict.at, c.written_by, c.claimed_by, c.lines
                )
            })
            .collect()
    };
    match &r.outcome {
        GridWrite::Applied(e) => format!(
            "exit {} applied v{} {:?} skipped {:?}",
            r.exit(),
            e.version,
            e.elems,
            conflicts(&r.skipped)
        ),
        GridWrite::Conflicts(c) => format!("exit {} conflicts {:?}", r.exit(), conflicts(c)),
        GridWrite::BaseExpired => format!("exit {} base expired", r.exit()),
    }
}
fn person_grid_write(p: &mut Person<'_>, base: u64, body: &str) -> R {
    let c = p.conn()?;
    match p
        .session()
        .write_base_grid(c, base, body.as_bytes(), &GridWriteOptions::default())
    {
        Ok(r) => Ok(Step::exit(r.exit(), Output::Value(grid_write_summary(&r)))),
        Err(e) => Ok(Step::exit(session_exit(&e), Output::None)),
    }
}
fn agent_grid_write(a: &mut Agent<'_>, base: u64, body: &str) -> R {
    let b = base.to_string();
    agent_step(
        a.verb(&["write", "--base", &b], body.as_bytes())?,
        |o| match &o.value {
            VerbValue::GridWrite(r) => Output::Value(grid_write_summary(r)),
            v => Output::value(v),
        },
    )
}
fn g14_clean_person(p: &mut Person<'_>) -> R {
    let v = p.stage().version()?;
    let body = rewritten(&rendered_of(p)?, "Rome", "Roma")?;
    person_grid_write(p, v, &body)
}
fn g14_clean_agent(a: &mut Agent<'_>) -> R {
    let v = a.stage().version()?;
    let body = rewritten(&stage_rendered(a.stage())?, "Rome", "Roma")?;
    agent_grid_write(a, v, &body)
}
/// The base (mark `b`), then p2 sets the cell holding `Rome` and claims its row.
fn g14_conflict_setup(stage: &mut Stage) -> Result<(), String> {
    let v = stage.version()?;
    stage.set_mark("b", v);
    let (r, col) = find_cell(&stage.table()?, "Rome")?;
    stage.grid_as("p2", |t| t.set_cell(&r, &col, "Paris"))?;
    let c = stage.conn_of("p2")?;
    let span = ClaimSpan::Rows {
        first: r.clone(),
        last: r,
    };
    stage
        .session()
        .claim_span(c, span, None, None, false)
        .map_err(|e| format!("p2's claim: {e}"))?;
    Ok(())
}
fn g14_conflict_person(p: &mut Person<'_>) -> R {
    let b = p.stage().mark("b")?;
    person_grid_write(p, b, GRID_BODY_ROMA)
}
fn g14_conflict_agent(a: &mut Agent<'_>) -> R {
    let b = a.stage().mark("b")?;
    agent_grid_write(a, b, GRID_BODY_ROMA)
}
/// [`super::GRID_DOC`] as the actor last saw it, with `k2`'s city rewritten.
const GRID_BODY_ROMA: &str = "id,city\nk1,Oslo\nk2,Roma\nk3,Lima\nk4,Kyiv\n";
fn g14_expired_person(p: &mut Person<'_>) -> R {
    person_grid_write(p, 99_999, GRID_BODY_ROMA)
}
fn g14_expired_agent(a: &mut Agent<'_>) -> R {
    agent_grid_write(a, 99_999, GRID_BODY_ROMA)
}

// ---------- P16, P26: undo by element ----------

/// The live cell holding `value` (the kit's cases find cells by their plain, unique values,
/// so they hold whatever the pack's header setting makes of [`super::GRID_DOC`]).
fn find_cell(t: &Table, value: &str) -> Result<(Key, Key), String> {
    t.cells
        .iter()
        .find(|((r, c), v)| v.as_str() == value && t.rows.is_live(r) && t.cols.is_live(c))
        .map(|(rc, _)| rc.clone())
        .ok_or_else(|| format!("no live cell holds {value:?}"))
}

/// The actor's own cell edits (`from` value to `to`, one change): the tab's table op.
fn person_cells(p: &mut Person<'_>, cells: &[(&str, &str)]) -> Result<(), String> {
    let t = p.table()?;
    let mut sets = Vec::new();
    for (from, to) in cells {
        let (r, c) = find_cell(&t, from)?;
        sets.push((r, c, (*to).to_owned()));
    }
    p.grid_edit(move |t: &mut Table| {
        let mut out = Vec::new();
        for (r, c, v) in &sets {
            out.extend(t.set_cell(r, c, v)?);
        }
        Ok(out)
    })
    .map_err(|e| e.to_string())?
    .ok_or("the edit changed nothing")?;
    Ok(())
}

/// The same edits by the agent: `write --base` of the file with those values rewritten.
fn agent_cells(a: &mut Agent<'_>, cells: &[(&str, &str)]) -> Result<(), String> {
    let v = a.stage().version()?;
    let mut body = stage_rendered(a.stage())?;
    for (from, to) in cells {
        body = rewritten(&body, from, to)?;
    }
    let o = a.verb(&["write", "--base", &v.to_string()], body.as_bytes())?;
    match (o.refused, o.exit) {
        (Some(e), _) => Err(e),
        (None, 0) => Ok(()),
        (None, x) => Err(format!("write --base exited {x}")),
    }
}
/// P2 sets the cell now holding `X` (the actor's) to `Nice`.
fn p2_overwrites_x(stage: &mut Stage) -> Result<(), String> {
    let (r, c) = find_cell(&stage.table()?, "X")?;
    stage.grid_as("p2", |t| t.set_cell(&r, &c, "Nice"))?;
    Ok(())
}
fn g16_undo_person(p: &mut Person<'_>) -> R {
    person_cells(p, &[("Oslo", "X")])?;
    person_undo(p.undo(UndoMode::Partial)) // Ctrl+Z
}
fn g16_undo_agent(a: &mut Agent<'_>) -> R {
    agent_cells(a, &[("Oslo", "X")])?;
    agent_step(a.verb(&["undo"], b"")?, undo_out)
}
fn g16_redo_person(p: &mut Person<'_>) -> R {
    person_cells(p, &[("Oslo", "X")])?;
    p.undo(UndoMode::Partial).map_err(|e| e.to_string())?;
    person_undo(p.redo())
}
fn g16_redo_agent(a: &mut Agent<'_>) -> R {
    agent_cells(a, &[("Oslo", "X")])?;
    a.verb(&["undo"], b"")?;
    agent_step(a.verb(&["redo"], b"")?, undo_out)
}
fn g16_drop_person(p: &mut Person<'_>) -> R {
    person_cells(p, &[("Oslo", "X")])?;
    p2_overwrites_x(p.stage())?;
    let blocked = p.undo(UndoMode::Partial).map_err(|e| e.to_string())?;
    let dropped = p.undo(UndoMode::Drop).map_err(|e| e.to_string())?;
    Ok(Step::exit(
        dropped.exit(),
        Output::Value(format!(
            "{} | {}",
            undo_summary(&blocked),
            undo_summary(&dropped)
        )),
    ))
}
fn g16_drop_agent(a: &mut Agent<'_>) -> R {
    agent_cells(a, &[("Oslo", "X")])?;
    p2_overwrites_x(a.stage())?;
    let blocked = a.verb(&["undo", "--partial"], b"")?;
    let dropped = a.verb(&["undo", "--drop"], b"")?;
    let (Output::Value(b), Output::Value(d)) = (undo_out(&blocked), undo_out(&dropped)) else {
        return Err("undo gave no outcome".into());
    };
    Ok(Step::exit(
        dropped.exit,
        Output::Value(format!("{b} | {d}")),
    ))
}
fn g26_person(p: &mut Person<'_>) -> R {
    person_cells(p, &[("Oslo", "X"), ("Rome", "Y")])?;
    p2_overwrites_x(p.stage())?;
    person_undo(p.undo(UndoMode::Partial))
}
fn g26_agent(a: &mut Agent<'_>) -> R {
    agent_cells(a, &[("Oslo", "X"), ("Rome", "Y")])?;
    p2_overwrites_x(a.stage())?;
    agent_step(a.verb(&["undo", "--partial"], b"")?, undo_out)
}

// ---------- P17, P18, P25: saving, watching, live editing off ----------

fn g17_status_person(p: &mut Person<'_>) -> R {
    person_cells(p, &[("Oslo", "X")])?;
    person_step(p.session().status(), |s| disk_out(&s.disk, &s.declined))
}
fn g17_status_agent(a: &mut Agent<'_>) -> R {
    agent_cells(a, &[("Oslo", "X")])?;
    agent_step(a.verb(&["status"], b"")?, |o| match &o.value {
        VerbValue::Status(s) => disk_out(&s.disk, &s.declined),
        v => Output::value(v),
    })
}
fn g17_flush_person(p: &mut Person<'_>) -> R {
    person_cells(p, &[("Oslo", "X")])?;
    p.stage().advance(1_001);
    p.stage().manager().tick().map_err(|e| e.to_string())?;
    person_step(p.session().status(), |s| Output::value(s.disk))
}
fn g17_flush_agent(a: &mut Agent<'_>) -> R {
    agent_cells(a, &[("Oslo", "X")])?;
    agent_step(a.verb(&["flush"], b"")?, |o| match &o.value {
        VerbValue::Flush(f) => Output::value(&f.disk),
        v => Output::value(v),
    })
}
fn g18_person(p: &mut Person<'_>) -> R {
    p.subscribe()?;
    p2_sets(p.stage())?;
    Ok(Step::ok(Output::Events(p.events())))
}
fn g18_agent_as(a: &mut Agent<'_>, argv: &[&str]) -> R {
    a.verb(argv, b"")?;
    p2_sets(a.stage())?;
    Ok(Step::ok(Output::Events(a.events())))
}
fn g18_watch_agent(a: &mut Agent<'_>) -> R {
    g18_agent_as(a, &["watch"])
}
fn g18_tail_agent(a: &mut Agent<'_>) -> R {
    g18_agent_as(a, &["tail", "-f"])
}
fn g25_setup(stage: &mut Stage) -> Result<(), String> {
    // another program writes bytes that aren't text: live editing turns off
    stage.write_file(b"id,city\n\xff\xfe,\n")?;
    Ok(())
}

// ---------- P19: history ----------

fn g19_show_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let v = p.stage().mark("v")?;
    person_step(p.session().show_content(c, v), Output::value)
}
fn g19_show_agent(a: &mut Agent<'_>) -> R {
    let v = a.stage().mark("v")?.to_string();
    agent_step(a.verb(&["show", "--version", &v], b"")?, |o| {
        match &o.value {
            VerbValue::Content(c) => Output::value(c),
            v => Output::value(v),
        }
    })
}
fn g19_diff_person(p: &mut Person<'_>) -> R {
    let live = rendered_of(p)?;
    let d = tools::unified(super::GRID_DOC, &live, 3, "mine", "live");
    Ok(Step::exit(
        u8::from(!d.is_empty()),
        Output::Text(d.lines().skip(2).map(|l| format!("{l}\n")).collect()),
    ))
}
fn g19_diff_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["diff", "-u"], super::GRID_DOC.as_bytes())?, |o| {
        Output::Text(
            String::from_utf8_lossy(o.stdout())
                .lines()
                .skip(2)
                .map(|l| format!("{l}\n"))
                .collect(),
        )
    })
}

// ---------- P22: blame by element ----------

/// Every kind of author on the grid: p2 sets a cell, a bot inserts a row, an assistant
/// sets a cell for the actor (pending), p2 moves a row (moves author no content), another
/// program edits a cell in the file; a second apart each.
fn grid_blame_setup(stage: &mut Stage) -> Result<(), String> {
    stage.advance(1_000);
    p2_sets(stage)?;
    stage.advance(1_000);
    stage.other("bot", ParticipantKind::Agent)?;
    stage.grid_as("bot", |t| {
        t.insert_row(Anchor::End, &["k5".into(), "Baku".into()])
            .map(|x| x.1)
    })?;
    stage.advance(1_000);
    stage.assistant("aide", ACTOR)?;
    stage.grid_as("aide", |t| t.set_cell(&k("r1"), &k("c2"), "Bergen"))?;
    stage.advance(1_000);
    stage.grid_as("p2", |t| t.move_row(&k("r3"), Anchor::Start))?;
    stage.advance(1_000);
    let p2 = stage.conn_of("p2")?;
    stage
        .session()
        .flush(p2)
        .map_err(|e| format!("flush: {e}"))?;
    let t = stage_rendered(stage)?;
    stage.write_file(t.replace("Kyiv", "Lviv").as_bytes())?;
    stage.advance(1_000);
    Ok(())
}
fn grid_blame_no_history_setup(stage: &mut Stage) -> Result<(), String> {
    grid_blame_setup(stage)?;
    stage.grant(ACTOR, Rights::editor());
    Ok(())
}
fn g22_cell_person(p: &mut Person<'_>) -> R {
    g22_cell_person_at(p, "r1", "c2")
}
fn g22_cell_person_at(p: &mut Person<'_>, r: &str, col: &str) -> R {
    let c = p.conn()?;
    match p.session().blame_cell(c, &k(r), &k(col)) {
        Ok(Some(b)) => Ok(Step::ok(Output::value(Some(Box::new(b))))),
        // the gutter shows nothing; the verb refuses it as bad_position (exit 2)
        Ok(None) => Ok(Step::exit(2, Output::None)),
        Err(e) => Ok(Step::exit(session_exit(&e), Output::None)),
    }
}
fn g22_unknown_person(p: &mut Person<'_>) -> R {
    g22_cell_person_at(p, "r99", "c1")
}
fn g22_unknown_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["blame", "--cell", "r99,c1"], b"")?, |_| {
        Output::None
    })
}
fn g22_absent_person(p: &mut Person<'_>) -> R {
    g22_cell_person_at(p, "r1", "c3")
}
fn g22_absent_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["blame", "--cell", "r1,c3"], b"")?, |_| {
        Output::None
    })
}
/// Column `c3` exists, but row `r1` never had a value there.
fn absent_cell_setup(stage: &mut Stage) -> Result<(), String> {
    stage.grid_as("p2", |t| t.insert_col(Anchor::End, None).map(|x| x.1))?;
    Ok(())
}
fn g22_cell_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["blame", "--cell", "r1,c2"], b"")?, |o| {
        match &o.value {
            VerbValue::CellBlame(b) => Output::value(b),
            v => Output::value(v),
        }
    })
}
fn g22_structure_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().blame_structure(c), Output::value)
}
fn g22_structure_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["blame", "--structure"], b"")?, |o| {
        match &o.value {
            VerbValue::Structure(s) => Output::value(s),
            v => Output::value(v),
        }
    })
}

// ---------- P44: rows drop-deleted ----------

/// P2 deletes row `r3`; the actor owns the doc.
fn drop_setup(stage: &mut Stage) -> Result<(), String> {
    stage.grant(ACTOR, Rights::full().with_owner(true));
    stage.grid_as("p2", |t| t.delete_row(&k("r3")))?;
    Ok(())
}
fn drop_not_owner_setup(stage: &mut Stage) -> Result<(), String> {
    drop_setup(stage)?;
    stage.grant(ACTOR, Rights::full());
    Ok(())
}
fn g44_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().drop_deleted(c), |r| {
        Output::Value(format!("v{} {:?}", r.version, r.elems))
    })
}
fn g44_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["rows", "drop-deleted"], b"")?, |o| {
        match &o.value {
            VerbValue::Edit(r) => Output::Value(format!("v{} {:?}", r.version, r.elems)),
            v => Output::value(v),
        }
    })
}

/// The kit's grid cases for `action` (A11): each kit verb serving it on a grid doc, the
/// person's way and the agent's way. Empty for the actions whose verbs are a pack's (P9-P13,
/// P35-P43), the ones a grid doesn't have (P15 formatting; the leases), and the ones the kit
/// doesn't bind yet (P23, P27 annotations, P28 purge): a grid pack's list maps those.
pub fn kit_grid_cases(action: PersonAction) -> Vec<GridCase> {
    use PersonAction as P;
    let c = GridCase::new;
    match action {
        P::Open => vec![c(action, "open", "joins, present", p1_person, p1_agent)],
        P::Close => vec![c(action, "leave", "gone", p2_person, p2_agent)],
        P::ReadAll => vec![
            c(
                action,
                "cat",
                "the rendered file",
                p3_cat_person,
                p3_cat_agent,
            ),
            c(action, "wc", "its counts", p3_wc_person, p3_wc_agent),
        ],
        P::ViewRange => vec![
            c(
                action,
                "view",
                "lines 2-3: a view of their rows",
                g4_view_person,
                g4_view_agent,
            ),
            c(
                action,
                "head",
                "the header and row 1",
                g4_head_person,
                g4_head_agent,
            ),
            c(
                action,
                "tail",
                "the last rows",
                g4_tail_person,
                g4_tail_agent,
            ),
            c(action, "sed", "lines 3-4", g4_sed_person, g4_sed_agent),
            c(
                action,
                "viewport",
                "an area in the doc's coordinates",
                g4_viewport_person,
                g4_viewport_agent,
            ),
            c(
                action,
                "view",
                "--for-edit: a view and a 30 s claim on the rows",
                g4_for_edit_person,
                g4_for_edit_agent,
            ),
        ],
        P::Find => vec![c(action, "grep", "matching lines", g5_person, g5_agent)],
        P::Who => vec![
            c(action, "who", "the roster", p6_person, p6_agent).setup(p2),
            c(
                action,
                "activity",
                "p2's cell edit in the activity tail",
                p6_activity_person,
                p6_activity_agent,
            )
            .setup(p2_sets),
        ],
        P::Follow => {
            vec![c(action, "follow", "p2's cell caret", g7_person, g7_agent).setup(p2_caret)]
        }
        P::Caret => vec![
            c(
                action,
                "cursor",
                "a cell caret",
                g8_caret_person,
                g8_caret_agent,
            ),
            c(
                action,
                "cursor",
                "a selection of cells",
                g8_select_person,
                g8_select_agent,
            ),
            c(
                action,
                "select",
                "two rows by key",
                g8_elems_person,
                g8_elems_agent,
            ),
            c(
                action,
                "pointer",
                "a place in the doc's coordinates",
                g8_pointer_person,
                g8_pointer_agent,
            ),
            c(
                action,
                "claim",
                "rows, a TTL and a note",
                g8_rows_person,
                g8_rows_agent,
            ),
            c(action, "claim", "a column", g8_cols_person, g8_cols_agent),
            c(
                action,
                "claim",
                "a rectangle of cells, held",
                g8_cells_person,
                g8_cells_agent,
            ),
            c(
                action,
                "claim",
                "--lines mapped to rows",
                g8_lines_person,
                g8_lines_agent,
            ),
            c(
                action,
                "claim",
                "someone else's claim overlaps: 5",
                g8_claimed_person,
                g8_claimed_agent,
            )
            .setup(p2_claims_r3),
            c(
                action,
                "claim --renew",
                "longer, held",
                g8_renew_person,
                g8_renew_agent,
            ),
            c(
                action,
                "claim --release",
                "mine, gone",
                g8_release_person,
                g8_release_agent,
            ),
        ],
        P::Rewrite => vec![
            c(
                action,
                "write",
                "--base V merged by key",
                g14_clean_person,
                g14_clean_agent,
            ),
            c(
                action,
                "write",
                "a cell p2 set and claimed since: 1, who wrote and claims it",
                g14_conflict_person,
                g14_conflict_agent,
            )
            .setup(g14_conflict_setup),
            c(
                action,
                "write",
                "an unknown base: 8",
                g14_expired_person,
                g14_expired_agent,
            ),
        ],
        P::Undo => vec![
            c(action, "undo", "my cell", g16_undo_person, g16_undo_agent),
            c(action, "redo", "my undo", g16_redo_person, g16_redo_agent),
            c(
                action,
                "undo --drop",
                "skip a blocked step",
                g16_drop_person,
                g16_drop_agent,
            ),
        ],
        P::UndoPartial => vec![c(
            action,
            "undo --partial",
            "the cell nobody touched",
            g26_person,
            g26_agent,
        )],
        P::Saved => vec![
            c(
                action,
                "status",
                "dirty after an edit",
                g17_status_person,
                g17_status_agent,
            ),
            c(
                action,
                "flush",
                "clean, file written",
                g17_flush_person,
                g17_flush_agent,
            ),
        ],
        P::Watch => vec![
            c(
                action,
                "watch",
                "p2's change arrives by key",
                g18_person,
                g18_watch_agent,
            ),
            c(
                action,
                "tail -f",
                "p2's change arrives by key",
                g18_person,
                g18_tail_agent,
            ),
        ],
        P::History => vec![
            c(
                action,
                "log",
                "changes with element counts",
                p19_log_person,
                p19_log_agent,
            )
            .setup(grid_history_setup),
            c(
                action,
                "show --version",
                "the import, by key",
                g19_show_person,
                g19_show_agent,
            )
            .setup(grid_history_setup),
            c(
                action,
                "diff --version",
                "the rendered file, import to live",
                p19_diffv_person,
                p19_diffv_agent,
            )
            .setup(grid_history_setup),
            c(
                action,
                "diff",
                "my copy to live",
                g19_diff_person,
                g19_diff_agent,
            )
            .setup(p2_sets),
        ],
        P::Restore => vec![
            c(
                action,
                "restore",
                "the import, by identity",
                p20_restore_person,
                p20_restore_agent,
            )
            .setup(grid_history_setup),
            c(
                action,
                "version --name",
                "a named version",
                p20_name_person,
                p20_name_agent,
            ),
        ],
        P::Rename => vec![c(action, "mv", "file and key move", p21_person, p21_agent)],
        P::Blame => vec![
            c(
                action,
                "blame",
                "lines to their rows' latest cell writes",
                p22_person,
                p22_agent,
            )
            .setup(grid_blame_setup),
            c(
                action,
                "blame",
                "-L 3,9, rendered",
                p22_range_person,
                p22_range_agent,
            )
            .setup(grid_blame_setup),
            c(
                action,
                "blame",
                "no history right: refused",
                p22_person,
                p22_agent,
            )
            .setup(grid_blame_no_history_setup),
            c(
                action,
                "blame --cell",
                "the assistant's pending write",
                g22_cell_person,
                g22_cell_agent,
            )
            .setup(grid_blame_setup),
            c(
                action,
                "blame --cell",
                "an absent cell: 2, bad_position",
                g22_absent_person,
                g22_absent_agent,
            )
            .setup(absent_cell_setup),
            c(
                action,
                "blame --cell",
                "a row that doesn't exist: 2, bad_position",
                g22_unknown_person,
                g22_unknown_agent,
            ),
            c(
                action,
                "blame --structure",
                "who inserted and who moved each row",
                g22_structure_person,
                g22_structure_agent,
            )
            .setup(grid_blame_setup),
        ],
        // P24 on a grid: the rendered file through the export registry, now and at a version
        // (a grid's version renders with the codec it stored)
        P::Export => vec![
            c(
                action,
                "export",
                "the file now, as-is",
                p24_now_person,
                p24_now_agent,
            )
            .setup(grid_history_setup),
            c(
                action,
                "export --version",
                "the import, as-is",
                p24_at_person,
                p24_at_agent,
            )
            .setup(grid_history_setup),
        ],
        P::WhyOff => vec![c(
            action,
            "status",
            "declined, with the reason",
            p25_status_person,
            p17_status_like,
        )
        .setup(g25_setup)],
        P::Approve => vec![
            c(
                action,
                "approve",
                "an assistant's cell",
                p29_approve_person,
                p29_approve_agent,
            )
            .setup(aide_grid_setup),
            c(
                action,
                "decline",
                "an assistant's cell",
                p29_decline_person,
                p29_decline_agent,
            )
            .setup(aide_grid_setup),
        ],
        P::Pending => vec![
            c(
                action,
                "log --pending",
                "waiting for me",
                p30_person,
                p30_agent,
            )
            .setup(p30_grid_setup),
            c(
                action,
                "approvals",
                "waiting for me",
                p30_person,
                p30_alias_agent,
            )
            .setup(p30_grid_setup),
            c(
                action,
                "approvals",
                "--all: settled ones too, newest first",
                p30_all_person,
                p30_all_agent,
            )
            .setup(p30_all_grid_setup),
            c(
                action,
                "log --pending",
                "--all: settled ones too, newest first",
                p30_all_person,
                p30_all_log_agent,
            )
            .setup(p30_all_grid_setup),
        ],
        P::Revert => vec![c(
            action,
            "revert",
            "an assistant's cell",
            p31_person,
            p31_agent,
        )
        .setup(aide_grid_setup)],
        P::DropDeleted => vec![
            c(
                action,
                "rows drop-deleted",
                "the owner frees p2's deleted row",
                g44_person,
                g44_agent,
            )
            .setup(drop_setup),
            c(
                action,
                "rows drop-deleted",
                "not the owner: 3",
                g44_person,
                g44_agent,
            )
            .setup(drop_not_owner_setup),
        ],
        // the pack's verbs (`cell set`, `cells set`, `cells clear`, `row insert`...)
        P::Type
        | P::TypeOver
        | P::Delete
        | P::FindReplace
        | P::Paste
        | P::RowInsert
        | P::RowMove
        | P::RowDelete
        | P::ColInsert
        | P::ColMove
        | P::ColDelete
        | P::ColRename
        | P::Sort
        | P::RestoreElement => Vec::new(),
        // not bound by the kit yet: the pack's list marks these deferred
        P::Annotate | P::Mention | P::Purge => Vec::new(),
        // no such action on a grid: formatting (CSV has none), leases (tier 0's)
        P::Format | P::Lease | P::ReleaseLease | P::BreakLease => Vec::new(),
    }
}
