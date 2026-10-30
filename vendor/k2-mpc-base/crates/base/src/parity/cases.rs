//! The kit's parity cases: for every person action, the person path and the agent path
//! through each kit verb. One exhaustive `match` ([`kit_cases`]): a new [`PersonAction`]
//! doesn't compile until it has cases here (or is deliberately left to the pack, deferred).

use super::action::PersonAction;
use super::harness::{
    Agent, Case, EventObs, Output, Person, Stage, Step, VerbOut, VerbValue, ACTOR, DOC,
};
use crate::session::{
    position, ApprovalState, At, ConflictRegion, EditResult, LineRange, LogItem, LogQuery,
    ParticipantKind, Rights, SessionError, UndoMode, UndoOutcome,
};
use crate::tools::{self, session_exit};

type R = Result<Step, String>;

// ---------- shared helpers ----------

/// An edit's observable summary: version, line counts, the set of lines touched. (Hunk
/// shapes can differ between a person's typed hunk and a verb's minimal diff; the result,
/// the counts and the lines touched may not.)
pub fn edit_summary(r: &EditResult) -> String {
    let mut lines: Vec<usize> = r.changed.iter().flat_map(|c| c.first..=c.last).collect();
    lines.sort_unstable();
    lines.dedup();
    format!(
        "v{} +{} -{} lines {lines:?}",
        r.version, r.lines_added, r.lines_removed
    )
}

fn regions(r: &[ConflictRegion]) -> Vec<String> {
    r.iter()
        .map(|c| format!("{}:{:?} {:?}", c.root, c.lines, c.kind))
        .collect()
}

/// An undo, redo or revert outcome's observable summary.
pub fn undo_summary(o: &UndoOutcome) -> String {
    match o {
        UndoOutcome::Applied {
            steps,
            result,
            skipped,
            blocked,
        } => format!(
            "applied {steps} [{}] skipped {:?} blocked {:?}",
            edit_summary(result),
            regions(skipped),
            blocked.as_deref().map(regions)
        ),
        UndoOutcome::Conflicts { step, conflicts } => {
            format!("conflicts at step {step}: {:?}", regions(conflicts))
        }
        UndoOutcome::Blocked { conflicts } => format!("blocked {:?}", regions(conflicts)),
        other => format!("{other:?}"),
    }
}

/// A person-path result: exit 0 and `f`'s output, or the refusal's exit code.
fn person_step<T>(r: Result<T, SessionError>, f: impl FnOnce(T) -> Output) -> R {
    Ok(match r {
        Ok(t) => Step::ok(f(t)),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}

/// An agent-path result: the verb's exit and `f`'s output (nothing when refused).
fn agent_step(out: VerbOut, f: impl FnOnce(&VerbOut) -> Output) -> R {
    Ok(if out.refused.is_some() {
        Step::exit(out.exit, Output::None)
    } else {
        Step::exit(out.exit, f(&out))
    })
}

fn tool_edit(out: &VerbOut) -> Output {
    match &out.value {
        VerbValue::Tool(t) => Output::Value(
            t.changed
                .as_ref()
                .map_or_else(|| "no change".to_owned(), edit_summary),
        ),
        v => Output::value(v),
    }
}

fn person_edit(r: Result<EditResult, SessionError>) -> R {
    person_step(r, |e| Output::Value(edit_summary(&e)))
}

fn stdout_text(out: &VerbOut) -> Output {
    Output::Text(String::from_utf8_lossy(out.stdout()).into_owned())
}

/// Lines `first..=last` of `text`, each with its `\n`.
fn lines(text: &str, first: usize, last: usize) -> String {
    text.split_inclusive('\n')
        .skip(first - 1)
        .take(last + 1 - first)
        .collect()
}

fn line_start(text: &str, line: usize) -> Result<usize, String> {
    position::line_start(text, line).ok_or_else(|| format!("no line {line}"))
}

/// A person looks at lines `first..=last` (scrolls the tab there) and sees them.
fn person_view(p: &mut Person<'_>, first: usize, last: usize) -> R {
    let c = p.conn()?;
    let root = p.root().to_owned();
    let text = p.text()?;
    p.session()
        .set_view(c, LineRange { root, first, last })
        .map_err(|e| e.to_string())?;
    Ok(Step::ok(Output::Text(lines(&text, first, last))))
}

fn agent_tool(a: &mut Agent<'_>, argv: &[&str], stdin: &str) -> Result<VerbOut, String> {
    a.verb(argv, stdin.as_bytes())
}

/// Bytes of `from` to `to` (`L:C`, half-open) in the person's live text.
fn span(
    p: &mut Person<'_>,
    from: (usize, usize),
    to: (usize, usize),
) -> Result<(usize, usize), String> {
    Ok((p.offset(from.0, from.1)?, p.offset(to.0, to.1)?))
}

fn sam(stage: &mut Stage) -> Result<(), String> {
    stage.other("sam", ParticipantKind::Person)?;
    Ok(())
}

/// Sam rewrites line 12 entirely (overlapping anything on it).
fn sam_rewrites_line_12(stage: &mut Stage) -> Result<(), String> {
    stage.edit_as("sam", |t| {
        let mut out = String::new();
        for (i, l) in t.split_inclusive('\n').enumerate() {
            out.push_str(if i == 11 { "Done: nothing left.\n" } else { l });
        }
        out
    })?;
    Ok(())
}

fn sam_edits_intro(stage: &mut Stage) -> Result<(), String> {
    stage.edit_as("sam", |t| t.replace("Intro line.", "Intro, revised."))?;
    Ok(())
}

// ---------- P1-P8: presence and reading ----------

fn p1_person(p: &mut Person<'_>) -> R {
    p.conn()?;
    Ok(Step::done())
}
fn p1_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["open"], b"")?, |_| Output::None)
}

fn p2_person(p: &mut Person<'_>) -> R {
    p.conn()?;
    p.close()?;
    Ok(Step::done())
}
fn p2_agent(a: &mut Agent<'_>) -> R {
    a.verb(&["open"], b"")?;
    agent_step(a.verb(&["leave"], b"")?, |_| Output::None)
}

fn p3_cat_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().read(c), |r| Output::Bytes(r.bytes))
}
fn p3_cat_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["cat"], b"")?, |o| {
        Output::Bytes(o.stdout().to_vec())
    })
}

fn counts(lines: usize, words: usize, bytes: usize) -> Output {
    Output::Value(format!("{lines} {words} {bytes}"))
}
fn p3_wc_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().read(c), |r| {
        let text = String::from_utf8_lossy(&r.bytes).into_owned();
        counts(
            r.bytes.iter().filter(|b| **b == b'\n').count(),
            text.split_whitespace().count(),
            r.bytes.len(),
        )
    })
}
fn p3_wc_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["wc"], b"")?, |o| {
        let s = String::from_utf8_lossy(o.stdout()).into_owned();
        let n: Vec<usize> = s
            .split_whitespace()
            .take(3)
            .filter_map(|x| x.parse().ok())
            .collect();
        match n[..] {
            [l, w, c] => counts(l, w, c),
            _ => Output::Text(s),
        }
    })
}

fn p4_view_person(p: &mut Person<'_>) -> R {
    person_view(p, 5, 8)
}
fn p4_view_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["view", "--lines", "5,8"], b"")?, stdout_text)
}
fn p4_head_person(p: &mut Person<'_>) -> R {
    person_view(p, 1, 4)
}
fn p4_head_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["head", "-n", "4"], b"")?, stdout_text)
}
fn p4_tail_person(p: &mut Person<'_>) -> R {
    person_view(p, 10, 13)
}
fn p4_tail_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["tail", "-n", "4"], b"")?, stdout_text)
}
fn p4_sed_person(p: &mut Person<'_>) -> R {
    person_view(p, 7, 8)
}
fn p4_sed_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["sed", "-n", "7,8p"], b"")?, stdout_text)
}

fn p5_person(p: &mut Person<'_>) -> R {
    let t = p.text()?;
    let hits: Vec<usize> = t
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("re"))
        .map(|(i, _)| i + 1)
        .collect();
    let exit = if hits.is_empty() { 1 } else { 0 };
    Ok(Step::exit(exit, Output::Lines(hits)))
}
fn p5_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["grep", "-n", "re"], b"")?, |o| {
        Output::Lines(
            String::from_utf8_lossy(o.stdout())
                .lines()
                .filter_map(|l| l.split_once(':').and_then(|(n, _)| n.parse().ok()))
                .collect(),
        )
    })
}

fn pids(r: &[crate::session::RosterEntry]) -> Output {
    Output::Pids(r.iter().map(|e| e.participant.pid.clone()).collect())
}
fn p6_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().roster(c), |r| pids(&r))
}
fn p6_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["who"], b"")?, |o| match &o.value {
        VerbValue::Roster(r) => pids(r),
        v => Output::value(v),
    })
}

fn p7_setup(stage: &mut Stage) -> Result<(), String> {
    let c = stage.other("sam", ParticipantKind::Person)?;
    let root = stage.root();
    stage
        .session()
        .set_cursor(
            c,
            root,
            At::Line(crate::session::LineCol { line: 7, col: 3 }),
            At::Line(crate::session::LineCol { line: 7, col: 3 }),
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn p7_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let r = p.session().roster(c).map_err(|e| e.to_string())?;
    let caret = r
        .into_iter()
        .find(|e| e.participant.pid == "sam")
        .and_then(|e| e.cursor)
        .map(|c| (c.anchor, c.head));
    Ok(Step::exit(u8::from(caret.is_none()), Output::Caret(caret)))
}
fn p7_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["follow", "sam"], b"")?, |o| match &o.value {
        VerbValue::Caret(c) => Output::Caret(c.as_ref().map(|c| (c.anchor, c.head))),
        v => Output::value(v),
    })
}

fn p8_person_caret(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let at = p.offset(7, 3)?;
    let root = p.root().to_owned();
    person_step(
        p.session().set_cursor(c, &root, At::Byte(at), At::Byte(at)),
        |_| Output::None,
    )
}
fn p8_agent_caret(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["cursor", "--at", "7:3"], b"")?, |_| Output::None)
}
fn p8_person_select(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let (s, e) = span(p, (7, 3), (8, 5))?;
    let root = p.root().to_owned();
    person_step(
        p.session().set_cursor(c, &root, At::Byte(s), At::Byte(e)),
        |_| Output::None,
    )
}
fn p8_agent_select(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["cursor", "--select", "7:3-8:5"], b"")?, |_| {
        Output::None
    })
}
fn p8_reader(stage: &mut Stage) -> Result<(), String> {
    stage.grant(ACTOR, Rights::read_only());
    Ok(())
}
fn p8_person_reader(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let root = p.root().to_owned();
    person_step(
        p.session().set_view(
            c,
            LineRange {
                root,
                first: 7,
                last: 7,
            },
        ),
        |_| Output::None,
    )
}

// ---------- amendment 13: agent presence (P4 --for-edit, P6 activity, P8 claims) ----------

/// Lines 5-8 for 30 s (the default `--for-edit` TTL), with a view: what a tab does when a
/// person says "I'm about to edit this".
fn p4_for_edit_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let root = p.root().to_owned();
    let text = p.text()?;
    let r = LineRange {
        root,
        first: 5,
        last: 8,
    };
    p.session()
        .set_view(c, r.clone())
        .map_err(|e| e.to_string())?;
    person_step(p.session().claim(c, r, Some(30_000), None, false), |_| {
        Output::Text(lines(&text, 5, 8))
    })
}
fn p4_for_edit_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["view", "--lines", "5,8", "--for-edit"], b"")?,
        stdout_text,
    )
}

/// Sam edits the intro, then the activity tail is read.
fn activity_rows(rows: &[crate::session::ActivityEntry]) -> Output {
    Output::Value(
        rows.iter()
            .map(|r| {
                format!(
                    "{} {} {} {:?}",
                    r.pid,
                    r.kind.as_str(),
                    r.verb,
                    r.lines.as_ref().map(|l| (l.first, l.last))
                )
            })
            .collect::<Vec<_>>()
            .join("; "),
    )
}
fn p6_activity_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().activity(c, 10), |r| activity_rows(&r))
}
fn p6_activity_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["activity", "-n", "10"], b"")?, |o| {
        match &o.value {
            VerbValue::Activity(r) => activity_rows(r),
            v => Output::value(v),
        }
    })
}

fn claim_lines(c: &crate::session::ClaimInfo) -> Output {
    Output::Value(format!(
        "lines {}-{} hold {} note {:?}",
        c.lines.first, c.lines.last, c.hold, c.note
    ))
}
fn claim_out(o: &VerbOut) -> Output {
    match &o.value {
        VerbValue::Claim(c) => claim_lines(c),
        VerbValue::ClaimsReleased(r) => Output::Value(format!("released {}", r.len())),
        v => Output::value(v),
    }
}
fn person_claim(
    p: &mut Person<'_>,
    first: usize,
    last: usize,
    ttl: Option<u64>,
    note: Option<&str>,
) -> Result<Result<crate::session::ClaimInfo, SessionError>, String> {
    let c = p.conn()?;
    let root = p.root().to_owned();
    Ok(p.session().claim(
        c,
        LineRange { root, first, last },
        ttl,
        note.map(str::to_owned),
        false,
    ))
}
fn p8_claim_person(p: &mut Person<'_>) -> R {
    person_step(
        person_claim(p, 5, 8, Some(60_000), Some("tidying goals"))?,
        |c| claim_lines(&c),
    )
}
fn p8_claim_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(
            &[
                "claim",
                "--lines",
                "5,8",
                "--ttl",
                "60000",
                "--note",
                "tidying goals",
            ],
            b"",
        )?,
        claim_out,
    )
}
fn p8_renew_person(p: &mut Person<'_>) -> R {
    person_claim(p, 5, 8, None, None)?.map_err(|e| e.to_string())?;
    let c = p.conn()?;
    person_step(
        p.session().renew_claim(c, None, Some(120_000), Some(true)),
        |c| claim_lines(&c),
    )
}
fn p8_renew_agent(a: &mut Agent<'_>) -> R {
    a.verb(&["claim", "--lines", "5,8"], b"")?;
    agent_step(
        a.verb(&["claim", "--renew", "--ttl", "120000", "--hold"], b"")?,
        claim_out,
    )
}
fn p8_release_person(p: &mut Person<'_>) -> R {
    person_claim(p, 5, 8, None, None)?.map_err(|e| e.to_string())?;
    let c = p.conn()?;
    person_step(p.session().release_claim(c, None), |r| {
        Output::Value(format!("released {}", r.len()))
    })
}
fn p8_release_agent(a: &mut Agent<'_>) -> R {
    a.verb(&["claim", "--lines", "5,8"], b"")?;
    agent_step(a.verb(&["claim", "--release"], b"")?, claim_out)
}
fn p8_release_none_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    Ok(match p.session().release_claim(c, None) {
        Ok(r) if r.is_empty() => Step::exit(1, Output::Value("released 0".into())),
        Ok(r) => Step::ok(Output::Value(format!("released {}", r.len()))),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}
fn p8_release_none_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["claim", "--release"], b"")?, claim_out)
}
/// Sam (an agent) has claimed lines 6-7.
fn sam_claims_6_7(stage: &mut Stage) -> Result<(), String> {
    let c = stage.other("sam", ParticipantKind::Agent)?;
    let root = stage.root().to_owned();
    stage
        .session()
        .claim(
            c,
            LineRange {
                root,
                first: 6,
                last: 7,
            },
            Some(60_000),
            Some("mine".into()),
            false,
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn p8_claimed_person(p: &mut Person<'_>) -> R {
    person_step(person_claim(p, 5, 8, None, None)?, |c| claim_lines(&c))
}
fn p8_claimed_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["claim", "--lines", "5,8"], b"")?, claim_out)
}

// ---------- P9-P15: editing ----------

fn p9_line_person(p: &mut Person<'_>) -> R {
    let at = line_start(&p.text()?, 7)?;
    person_edit(p.type_at(at, at, "- write tests\n"))
}
fn p9_line_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["insert", "--line", "7"], "- write tests\n")?,
        tool_edit,
    )
}
fn p9_at_person(p: &mut Person<'_>) -> R {
    let at = p.offset(7, 8)?;
    person_edit(p.type_at(at, at, "all of "))
}
fn p9_at_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["insert", "--at", "7:8"], "all of ")?,
        tool_edit,
    )
}
fn p9_after_person(p: &mut Person<'_>) -> R {
    let at = line_start(&p.text()?, 9)?;
    person_edit(p.type_at(at, at, "- add tests\n"))
}
fn p9_after_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["insert", "--after", "8"], "- add tests\n")?,
        tool_edit,
    )
}

fn p10_lines_person(p: &mut Person<'_>) -> R {
    // select from after "- " on line 7 to the end of line 8, and type
    let (s, e) = span(p, (7, 3), (8, 19))?;
    person_edit(p.type_at(s, e, "one\n- two"))
}
fn p10_lines_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["replace", "--lines", "7,8"], "- one\n- two\n")?,
        tool_edit,
    )
}
fn p10_range_person(p: &mut Person<'_>) -> R {
    let (s, e) = span(p, (7, 3), (7, 7))?;
    person_edit(p.type_at(s, e, "SHIP"))
}
fn p10_range_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["replace", "--range", "7:3-7:7"], "SHIP")?,
        tool_edit,
    )
}

fn p11_person(p: &mut Person<'_>) -> R {
    let t = p.text()?;
    let (s, e) = (line_start(&t, 12)?, line_start(&t, 13)?);
    person_edit(p.type_at(s, e, ""))
}
fn p11_delete_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["delete", "--lines", "12,12"], "")?,
        tool_edit,
    )
}
fn p11_sed_agent(a: &mut Agent<'_>) -> R {
    agent_step(agent_tool(a, &["sed", "-i", "12d"], "")?, tool_edit)
}

fn p12_person(p: &mut Person<'_>) -> R {
    person_step(p.edit_with(|t| t.replace("files", "docs")), |r| {
        Output::Value(r.as_ref().map_or_else(|| "no change".into(), edit_summary))
    })
}
fn p12_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["sed", "-i", "s/files/docs/g"], "")?,
        tool_edit,
    )
}

/// Find and replace with exact text: the dialog's "Replace" on its only match.
fn p12_exact_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["replace", "--old", "files", "--new", "docs"], "")?,
        tool_edit,
    )
}
/// "Replace all": both list items' "- " become "* ".
fn p12_all_person(p: &mut Person<'_>) -> R {
    person_step(p.edit_with(|t| t.replace("- ", "* ")), |r| {
        Output::Value(r.as_ref().map_or_else(|| "no change".into(), edit_summary))
    })
}
fn p12_all_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["replace", "--old", "- ", "--new", "* ", "--all"], "")?,
        tool_edit,
    )
}
/// A two-line find (lines 7-8) replaced by one line, the find text on stdin.
fn p12_lines_person(p: &mut Person<'_>) -> R {
    person_step(
        p.edit_with(|t| t.replace("- ship the core\n- keep", "- ship and keep")),
        |r| Output::Value(r.as_ref().map_or_else(|| "no change".into(), edit_summary)),
    )
}
fn p12_lines_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(
            a,
            &["replace", "--old", "-", "--new", "- ship and keep"],
            "- ship the core\n- keep",
        )?,
        tool_edit,
    )
}

/// Line 12's "more" becomes "less": the change P13's patches carry.
fn more_to_less(p: &mut Person<'_>) -> R {
    let (s, e) = span(p, (12, 13), (12, 17))?;
    person_edit(p.type_at(s, e, "less"))
}
fn patch_text() -> String {
    tools::unified(DOC, &DOC.replace("more", "less"), 3, DOC_LABEL, DOC_LABEL)
}
const DOC_LABEL: &str = super::harness::DOC_PATH;
fn p13_patch_agent(a: &mut Agent<'_>) -> R {
    agent_step(agent_tool(a, &["patch"], &patch_text())?, tool_edit)
}
fn p13_apply_agent(a: &mut Agent<'_>) -> R {
    agent_step(agent_tool(a, &["apply"], &patch_text())?, tool_edit)
}
fn p13_append_person(p: &mut Person<'_>) -> R {
    let end = p.text()?.len();
    person_edit(p.type_at(end, end, "Appended.\n"))
}
fn p13_append_agent(a: &mut Agent<'_>) -> R {
    agent_step(agent_tool(a, &["append"], "Appended.\n")?, tool_edit)
}

fn p14_person(p: &mut Person<'_>) -> R {
    p.text()?; // the tab shows the doc
    sam_edits_intro(p.stage())?;
    // typing lands on the live text: "more" -> "TBD" where it now is
    let (s, e) = span(p, (12, 13), (12, 17))?;
    person_edit(p.type_at(s, e, "TBD"))
}
fn p14_agent(a: &mut Agent<'_>) -> R {
    let v = a
        .verb(&["cat"], b"")?
        .version()
        .ok_or("cat gave no version")?;
    sam_edits_intro(a.stage())?;
    let base = v.to_string();
    let out = agent_tool(a, &["write", "--base", &base], &DOC.replace("more", "TBD"))?;
    agent_step(out, |o| match &o.value {
        VerbValue::Write(crate::session::WriteOutcome::Applied(r)) => {
            Output::Value(edit_summary(r))
        }
        v => Output::value(v),
    })
}

fn p15_heading_person(p: &mut Person<'_>) -> R {
    let at = line_start(&p.text()?, 3)?;
    person_edit(p.type_at(at, at, "### "))
}
fn p15_replace_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["replace", "--lines", "3,3"], "### Intro line.\n")?,
        tool_edit,
    )
}
fn p15_insert_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["insert", "--at", "3:1"], "### ")?,
        tool_edit,
    )
}
fn p15_bold_person(p: &mut Person<'_>) -> R {
    let (s, e) = span(p, (7, 12), (7, 16))?;
    person_edit(p.edit(vec![
        crate::text::diff::Hunk {
            start: s,
            end: s,
            insert: "**".into(),
        },
        crate::text::diff::Hunk {
            start: e,
            end: e,
            insert: "**".into(),
        },
    ]))
}
fn p15_bold_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        agent_tool(a, &["sed", "-i", "s/core/**core**/"], "")?,
        tool_edit,
    )
}

// ---------- P16, P26: undo ----------

fn undo_out(o: &VerbOut) -> Output {
    match &o.value {
        VerbValue::Undo(u) => Output::Value(undo_summary(u)),
        v => Output::value(v),
    }
}
fn person_undo(r: Result<UndoOutcome, SessionError>) -> R {
    Ok(match r {
        Ok(u) => Step::exit(u.exit(), Output::Value(undo_summary(&u))),
        Err(e) => Step::exit(session_exit(&e), Output::None),
    })
}

fn p16_person_edit(p: &mut Person<'_>) -> Result<(), String> {
    let at = line_start(&p.text()?, 7)?;
    p.type_at(at, at, "- write tests\n")
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn p16_agent_edit(a: &mut Agent<'_>) -> Result<(), String> {
    let o = a.verb(&["insert", "--line", "7"], b"- write tests\n")?;
    match o.refused {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
fn p16_undo_person(p: &mut Person<'_>) -> R {
    p16_person_edit(p)?;
    person_undo(p.undo(UndoMode::Partial)) // Ctrl+Z
}
fn p16_undo_agent(a: &mut Agent<'_>) -> R {
    p16_agent_edit(a)?;
    agent_step(a.verb(&["undo"], b"")?, undo_out)
}
fn p16_redo_person(p: &mut Person<'_>) -> R {
    p16_person_edit(p)?;
    p.undo(UndoMode::Partial).map_err(|e| e.to_string())?;
    person_undo(p.redo())
}
fn p16_redo_agent(a: &mut Agent<'_>) -> R {
    p16_agent_edit(a)?;
    a.verb(&["undo"], b"")?;
    agent_step(a.verb(&["redo"], b"")?, undo_out)
}
fn p16_drop_person(p: &mut Person<'_>) -> R {
    more_to_less(p)?;
    sam_rewrites_line_12(p.stage())?;
    let blocked = p.undo(UndoMode::Partial).map_err(|e| e.to_string())?;
    // the second Ctrl+Z on a blocked step offers "skip this change"
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
fn p16_drop_agent(a: &mut Agent<'_>) -> R {
    agent_tool(a, &["replace", "--range", "12:13-12:17"], "less")?;
    sam_rewrites_line_12(a.stage())?;
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

fn p26_person(p: &mut Person<'_>) -> R {
    // one change in two places: "core" -> "CORE" (line 7), "more" -> "MORE" (line 12)
    let (a0, a1) = span(p, (7, 12), (7, 16))?;
    let (b0, b1) = span(p, (12, 13), (12, 17))?;
    let hunk = |start, end, s: &str| crate::text::diff::Hunk {
        start,
        end,
        insert: s.into(),
    };
    p.edit(vec![hunk(a0, a1, "CORE"), hunk(b0, b1, "MORE")])
        .map_err(|e| e.to_string())?;
    sam_rewrites_line_12(p.stage())?;
    person_undo(p.undo(UndoMode::Partial))
}
fn p26_agent(a: &mut Agent<'_>) -> R {
    agent_tool(a, &["sed", "-i", "s/core/CORE/;s/more/MORE/"], "")?;
    sam_rewrites_line_12(a.stage())?;
    agent_step(a.verb(&["undo", "--partial"], b"")?, undo_out)
}

// ---------- P17, P18, P25: saving, watching, live editing off ----------

fn disk_out(disk: &Option<crate::session::DiskState>, declined: &Option<String>) -> Output {
    Output::Value(format!("disk {disk:?} declined {declined:?}"))
}
fn p17_status_person(p: &mut Person<'_>) -> R {
    more_to_less(p)?;
    person_step(p.session().status(), |s| disk_out(&s.disk, &s.declined))
}
fn p17_status_agent(a: &mut Agent<'_>) -> R {
    agent_tool(a, &["replace", "--range", "12:13-12:17"], "less")?;
    agent_step(a.verb(&["status"], b"")?, |o| match &o.value {
        VerbValue::Status(s) => disk_out(&s.disk, &s.declined),
        v => Output::value(v),
    })
}
fn p17_flush_person(p: &mut Person<'_>) -> R {
    more_to_less(p)?;
    // the tab saves by itself: write-back after a quiet second, on the host's tick
    p.stage().advance(1_001);
    p.stage().manager().tick().map_err(|e| e.to_string())?;
    person_step(p.session().status(), |s| Output::value(s.disk))
}
fn p17_flush_agent(a: &mut Agent<'_>) -> R {
    agent_tool(a, &["replace", "--range", "12:13-12:17"], "less")?;
    agent_step(a.verb(&["flush"], b"")?, |o| match &o.value {
        VerbValue::Flush(f) => Output::value(&f.disk),
        v => Output::value(v),
    })
}

fn p18_person(p: &mut Person<'_>) -> R {
    p.subscribe()?;
    sam_edits_intro(p.stage())?;
    Ok(Step::ok(Output::Events(p.events())))
}
fn watch_agent(a: &mut Agent<'_>, argv: &[&str]) -> R {
    a.verb(argv, b"")?;
    sam_edits_intro(a.stage())?;
    let ev: Vec<EventObs> = a.events();
    Ok(Step::ok(Output::Events(ev)))
}
fn p18_watch_agent(a: &mut Agent<'_>) -> R {
    watch_agent(a, &["watch"])
}
fn p18_tail_agent(a: &mut Agent<'_>) -> R {
    watch_agent(a, &["tail", "-f"])
}

fn p25_setup(stage: &mut Stage) -> Result<(), String> {
    // another program makes the file binary: live editing turns off
    stage.write_file(b"# Plan\n\0binary\n")?;
    Ok(())
}
fn p25_status_person(p: &mut Person<'_>) -> R {
    p.conn()?;
    person_step(p.session().status(), |s| disk_out(&s.disk, &s.declined))
}
fn p25_edit_person(p: &mut Person<'_>) -> R {
    p.conn()?;
    person_edit(p.type_at(0, 0, "x\n"))
}
fn p25_edit_agent(a: &mut Agent<'_>) -> R {
    agent_step(agent_tool(a, &["insert", "--line", "1"], "x\n")?, tool_edit)
}

// ---------- P19-P21: history, restore, rename ----------

fn history_setup(stage: &mut Stage) -> Result<(), String> {
    let first = stage
        .versions()?
        .first()
        .map(|v| v.id)
        .ok_or("no import version")?;
    stage.set_mark("v", first);
    sam_edits_intro(stage)
}
fn mark_arg(p: &Stage) -> Result<String, String> {
    Ok(p.mark("v")?.to_string())
}

fn p19_log_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().log(c, &LogQuery::default()), Output::value)
}
fn p19_log_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["log"], b"")?, |o| match &o.value {
        VerbValue::Log(l) => Output::value(l),
        v => Output::value(v),
    })
}
fn p19_show_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let v = p.stage().mark("v")?;
    person_step(p.session().show(c, v), Output::value)
}
fn p19_show_agent(a: &mut Agent<'_>) -> R {
    let v = mark_arg(a.stage())?;
    agent_step(a.verb(&["show", "--version", &v], b"")?, |o| {
        match &o.value {
            VerbValue::Show(s) => Output::value(s),
            v => Output::value(v),
        }
    })
}
fn p19_diffv_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let v = p.stage().mark("v")?;
    let root = p.root().to_owned();
    match p.session().diff_version(c, &root, v, None, 3) {
        Ok(d) => Ok(Step::exit(d.exit(), Output::Text(d.text))),
        Err(e) => Ok(Step::exit(session_exit(&e), Output::None)),
    }
}
fn p19_diffv_agent(a: &mut Agent<'_>) -> R {
    let v = mark_arg(a.stage())?;
    agent_step(a.verb(&["diff", "--version", &v], b"")?, |o| {
        match &o.value {
            VerbValue::Diff(d) => Output::Text(d.text.clone()),
            v => Output::value(v),
        }
    })
}
/// A unified diff's hunks, without its two label lines.
fn diff_body(s: &str) -> Output {
    Output::Text(s.lines().skip(2).map(|l| format!("{l}\n")).collect())
}
fn p19_diff_person(p: &mut Person<'_>) -> R {
    // "compare with my copy": the editor diffs the copy it holds against the live text
    let live = p.text()?;
    let d = tools::unified(DOC, &live, 3, "mine", "live");
    Ok(Step::exit(u8::from(!d.is_empty()), diff_body(&d)))
}
fn p19_diff_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["diff", "-u"], DOC.as_bytes())?, |o| {
        diff_body(&String::from_utf8_lossy(o.stdout()))
    })
}

fn p20_restore_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let v = p.stage().mark("v")?;
    person_edit(p.session().restore(c, v))
}
fn p20_restore_agent(a: &mut Agent<'_>) -> R {
    let v = mark_arg(a.stage())?;
    agent_step(a.verb(&["restore", "--version", &v], b"")?, |o| {
        match &o.value {
            VerbValue::Edit(r) => Output::Value(edit_summary(r)),
            v => Output::value(v),
        }
    })
}
fn p20_name_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    person_step(p.session().name_version(c, "draft", None), Output::value)
}
fn p20_name_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["version", "--name", "draft"], b"")?,
        |o| match &o.value {
            VerbValue::Version(v) => Output::value(v),
            v => Output::value(v),
        },
    )
}

fn p21_person(p: &mut Person<'_>) -> R {
    p.conn()?;
    // the tab's rename: the host renames the file and tells the kit
    person_step(p.stage().host_rename("renamed.md"), Output::value)
}
fn p21_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["mv", "renamed.md"], b"")?, |o| match &o.value {
        VerbValue::Moved(d) => Output::value(d),
        v => Output::value(v),
    })
}

fn p24_person(p: &mut Person<'_>) -> R {
    p.conn()?;
    // the export is the file itself
    let bytes = std::fs::read(p.stage().file()).map_err(|e| e.to_string())?;
    Ok(Step::ok(Output::Bytes(bytes)))
}

// ---------- P29-P31: on-behalf edits and revert ----------

/// An assistant ("aide") edits line 12 on the actor's behalf; mark "c" is its change.
fn aide_setup(stage: &mut Stage) -> Result<(), String> {
    stage.assistant("aide", ACTOR)?;
    let seq = stage.edit_as("aide", |t| t.replace("more", "MORE"))?;
    stage.set_mark("c", seq);
    Ok(())
}
fn approval_person(p: &mut Person<'_>, state: ApprovalState) -> R {
    let c = p.conn()?;
    let seq = p.stage().mark("c")?;
    person_step(p.session().set_approval(c, seq, state), Output::value)
}
fn approval_agent(a: &mut Agent<'_>, verb: &str) -> R {
    let seq = a.stage().mark("c")?.to_string();
    agent_step(a.verb(&[verb, &seq], b"")?, |o| match &o.value {
        VerbValue::Approval(x) => Output::value(x),
        v => Output::value(v),
    })
}
fn p29_approve_person(p: &mut Person<'_>) -> R {
    approval_person(p, ApprovalState::Approved)
}
fn p29_approve_agent(a: &mut Agent<'_>) -> R {
    approval_agent(a, "approve")
}
fn p29_decline_person(p: &mut Person<'_>) -> R {
    approval_person(p, ApprovalState::Declined)
}
fn p29_decline_agent(a: &mut Agent<'_>) -> R {
    approval_agent(a, "decline")
}

fn p30_setup(stage: &mut Stage) -> Result<(), String> {
    stage.assistant("aide", ACTOR)?;
    stage.edit_as("aide", |t| t.replace("more", "MORE"))?;
    stage.assistant("bot", "sam")?;
    stage.edit_as("bot", |t| t.replace("Intro", "INTRO"))?;
    stage.edit_as("aide", |t| t.replace("core", "CORE"))?;
    Ok(())
}
fn p30_person(p: &mut Person<'_>) -> R {
    // the tab's "waiting for you" list
    let c = p.conn()?;
    person_step(p.session().approvals(c), |all| {
        let seqs: Vec<u64> = all
            .into_iter()
            .filter(|(_, a)| a.state == ApprovalState::Pending && a.on_behalf_of == ACTOR)
            .map(|(s, _)| s)
            .collect();
        Output::value(seqs)
    })
}
fn p30_agent(a: &mut Agent<'_>) -> R {
    agent_step(
        a.verb(&["log", "--pending", "--on-behalf-of", "me"], b"")?,
        |o| match &o.value {
            VerbValue::Log(l) => {
                let mut seqs: Vec<u64> = l
                    .iter()
                    .filter_map(|i| match i {
                        LogItem::Change { info, .. } => Some(info.seq),
                        _ => None,
                    })
                    .collect();
                seqs.sort_unstable();
                Output::value(seqs)
            }
            v => Output::value(v),
        },
    )
}

fn p31_person(p: &mut Person<'_>) -> R {
    let c = p.conn()?;
    let seq = p.stage().mark("c")?;
    person_undo(p.session().revert(c, seq, false))
}
fn p31_agent(a: &mut Agent<'_>) -> R {
    let seq = a.stage().mark("c")?.to_string();
    agent_step(a.verb(&["revert", &seq], b"")?, undo_out)
}

// ---------- P22: blame ----------

/// A history with every kind of author: the import, a person, an agent, an assistant acting
/// for the actor (pending), an outside edit; a second apart each.
fn blame_setup(stage: &mut Stage) -> Result<(), String> {
    stage.advance(1_000);
    stage.edit_as("sam", |t| t.replace("Intro line.", "Intro, revised."))?;
    stage.advance(1_000);
    stage.other("bot", ParticipantKind::Agent)?;
    stage.edit_as("bot", |t| {
        t.replace(
            "- keep files plain
",
            "- keep files plain
- ship blame
",
        )
    })?;
    stage.advance(1_000);
    stage.assistant("aide", ACTOR)?;
    stage.edit_as("aide", |t| t.replace("write more", "write MORE"))?;
    stage.advance(1_000);
    // another program edits the file (after the kit wrote it back)
    let sam = stage.other("sam", ParticipantKind::Person)?;
    stage
        .session()
        .flush(sam)
        .map_err(|e| format!("flush: {e}"))?;
    let t = stage.text()?;
    stage.write_file(t.replace("End.", "The end.").as_bytes())?;
    stage.advance(1_000);
    Ok(())
}
fn blame_no_history_setup(stage: &mut Stage) -> Result<(), String> {
    blame_setup(stage)?;
    stage.grant(ACTOR, Rights::editor());
    Ok(())
}
fn blame_out(o: &VerbOut) -> Output {
    match &o.value {
        VerbValue::Blame(b) => Output::value(&b.lines),
        v => Output::value(v),
    }
}
fn p22_person(p: &mut Person<'_>) -> R {
    // the tab's blame gutter: the same session call, every line
    let c = p.conn()?;
    let root = p.root().to_owned();
    person_step(p.session().blame(c, &root, None), |b| {
        Output::value(b.lines)
    })
}
fn p22_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["blame"], b"")?, blame_out)
}
fn p22_range_person(p: &mut Person<'_>) -> R {
    // the gutter of the lines in view (3 to 9), as the verb prints them
    let c = p.conn()?;
    let root = p.root().to_owned();
    let r = LineRange {
        root: root.clone(),
        first: 3,
        last: 9,
    };
    person_step(p.session().blame(c, &root, Some(r)), |b| {
        Output::Text(b.render(|_| None))
    })
}
fn p22_range_agent(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["blame", "-L", "3,9"], b"")?, |o| match &o.value {
        VerbValue::Blame(b) => Output::Text(b.render(|_| None)),
        v => Output::value(v),
    })
}

/// The kit's cases for `action`: each kit verb serving it, the person's way and the
/// agent's way. Empty for the rows the kit doesn't bind yet (deferred: P23 and P27
/// annotations, P28 purge); a pack's list must then mark them deferred.
pub fn kit_cases(action: PersonAction) -> Vec<Case> {
    use PersonAction as P;
    let c = Case::new;
    match action {
        P::Open => vec![c(action, "open", "joins, present", p1_person, p1_agent)],
        P::Close => vec![c(
            action,
            "leave",
            "gone from presence",
            p2_person,
            p2_agent,
        )],
        P::ReadAll => vec![
            c(
                action,
                "cat",
                "the file's bytes",
                p3_cat_person,
                p3_cat_agent,
            ),
            c(
                action,
                "wc",
                "line, word, byte counts",
                p3_wc_person,
                p3_wc_agent,
            ),
        ],
        P::ViewRange => vec![
            c(
                action,
                "view",
                "lines 5-8, viewing range",
                p4_view_person,
                p4_view_agent,
            ),
            c(
                action,
                "head",
                "lines 1-4, viewing range",
                p4_head_person,
                p4_head_agent,
            ),
            c(
                action,
                "tail",
                "lines 10-13, viewing range",
                p4_tail_person,
                p4_tail_agent,
            ),
            c(
                action,
                "sed",
                "lines 7-8, viewing range",
                p4_sed_person,
                p4_sed_agent,
            ),
            c(
                action,
                "view",
                "--for-edit: a view and a 30 s claim",
                p4_for_edit_person,
                p4_for_edit_agent,
            ),
        ],
        P::Find => vec![c(action, "grep", "matching lines", p5_person, p5_agent)],
        P::Who => vec![
            c(action, "who", "the roster", p6_person, p6_agent).setup(sam),
            c(
                action,
                "activity",
                "sam's edit in the activity tail",
                p6_activity_person,
                p6_activity_agent,
            )
            .setup(sam_edits_intro),
        ],
        P::Follow => vec![c(action, "follow", "sam's caret", p7_person, p7_agent).setup(p7_setup)],
        P::Caret => vec![
            c(action, "cursor", "a caret", p8_person_caret, p8_agent_caret),
            c(
                action,
                "cursor",
                "a selection",
                p8_person_select,
                p8_agent_select,
            ),
            c(
                action,
                "cursor",
                "a reader gets a view",
                p8_person_reader,
                p8_agent_caret,
            )
            .setup(p8_reader),
            c(
                action,
                "claim",
                "lines 5-8, a TTL and a note",
                p8_claim_person,
                p8_claim_agent,
            ),
            c(
                action,
                "claim",
                "someone else's claim overlaps: 5",
                p8_claimed_person,
                p8_claimed_agent,
            )
            .setup(sam_claims_6_7),
            c(
                action,
                "claim --renew",
                "longer, held",
                p8_renew_person,
                p8_renew_agent,
            ),
            c(
                action,
                "claim --release",
                "mine, gone",
                p8_release_person,
                p8_release_agent,
            ),
            c(
                action,
                "claim --release",
                "none held: 1",
                p8_release_none_person,
                p8_release_none_agent,
            ),
        ],
        P::Type => vec![
            c(action, "insert", "--line", p9_line_person, p9_line_agent),
            c(action, "insert", "--at", p9_at_person, p9_at_agent),
            c(action, "insert", "--after", p9_after_person, p9_after_agent),
        ],
        P::TypeOver => vec![
            c(
                action,
                "replace",
                "--lines",
                p10_lines_person,
                p10_lines_agent,
            ),
            c(
                action,
                "replace",
                "--range",
                p10_range_person,
                p10_range_agent,
            ),
        ],
        P::Delete => vec![
            c(action, "delete", "a line", p11_person, p11_delete_agent),
            c(action, "sed -i", "a,bd", p11_person, p11_sed_agent),
        ],
        P::FindReplace => vec![
            c(action, "sed -i", "s///g", p12_person, p12_agent),
            c(
                action,
                "replace",
                "--old/--new (one match)",
                p12_person,
                p12_exact_agent,
            ),
            c(
                action,
                "replace",
                "--old/--new --all",
                p12_all_person,
                p12_all_agent,
            ),
            c(
                action,
                "replace",
                "--old from stdin, two lines",
                p12_lines_person,
                p12_lines_agent,
            ),
        ],
        P::Paste => vec![
            c(
                action,
                "patch",
                "a unified diff",
                more_to_less,
                p13_patch_agent,
            ),
            c(
                action,
                "apply",
                "a unified diff",
                more_to_less,
                p13_apply_agent,
            ),
            c(
                action,
                "append",
                "a block at the end",
                p13_append_person,
                p13_append_agent,
            ),
        ],
        P::Rewrite => vec![c(
            action,
            "write",
            "--base V merges with sam's edit",
            p14_person,
            p14_agent,
        )],
        P::Format => vec![
            c(
                action,
                "insert",
                "a heading",
                p15_heading_person,
                p15_insert_agent,
            ),
            c(
                action,
                "replace",
                "a heading",
                p15_heading_person,
                p15_replace_agent,
            ),
            c(action, "sed -i", "bold", p15_bold_person, p15_bold_agent),
        ],
        P::Undo => vec![
            c(action, "undo", "my edit", p16_undo_person, p16_undo_agent),
            c(action, "redo", "my undo", p16_redo_person, p16_redo_agent),
            c(
                action,
                "undo --drop",
                "skip a blocked step",
                p16_drop_person,
                p16_drop_agent,
            ),
        ],
        P::Saved => vec![
            c(
                action,
                "status",
                "dirty after an edit",
                p17_status_person,
                p17_status_agent,
            ),
            c(
                action,
                "flush",
                "clean, file written",
                p17_flush_person,
                p17_flush_agent,
            ),
        ],
        P::Watch => vec![
            c(
                action,
                "watch",
                "sam's change arrives",
                p18_person,
                p18_watch_agent,
            ),
            c(
                action,
                "tail -f",
                "sam's change arrives",
                p18_person,
                p18_tail_agent,
            ),
        ],
        P::History => vec![
            c(
                action,
                "log",
                "changes and versions",
                p19_log_person,
                p19_log_agent,
            )
            .setup(history_setup),
            c(
                action,
                "show --version",
                "the import",
                p19_show_person,
                p19_show_agent,
            )
            .setup(history_setup),
            c(
                action,
                "diff --version",
                "import to live",
                p19_diffv_person,
                p19_diffv_agent,
            )
            .setup(history_setup),
            c(
                action,
                "diff",
                "my copy to live",
                p19_diff_person,
                p19_diff_agent,
            )
            .setup(history_setup),
        ],
        P::Restore => vec![
            c(
                action,
                "restore",
                "the import",
                p20_restore_person,
                p20_restore_agent,
            )
            .setup(history_setup),
            c(
                action,
                "version --name",
                "a named version",
                p20_name_person,
                p20_name_agent,
            ),
        ],
        P::Rename => vec![c(action, "mv", "file and key move", p21_person, p21_agent)],
        P::Export => vec![c(
            action,
            "cat",
            "N/A: cat is the file's bytes",
            p24_person,
            p3_cat_agent,
        )],
        P::WhyOff => vec![
            c(
                action,
                "status",
                "declined, with the reason",
                p25_status_person,
                p17_status_like,
            )
            .setup(p25_setup),
            c(
                action,
                "insert",
                "edits exit 4",
                p25_edit_person,
                p25_edit_agent,
            )
            .setup(p25_setup),
        ],
        P::UndoPartial => vec![c(
            action,
            "undo --partial",
            "the untouched region",
            p26_person,
            p26_agent,
        )],
        P::Approve => vec![
            c(
                action,
                "approve",
                "an assistant's edit",
                p29_approve_person,
                p29_approve_agent,
            )
            .setup(aide_setup),
            c(
                action,
                "decline",
                "an assistant's edit",
                p29_decline_person,
                p29_decline_agent,
            )
            .setup(aide_setup),
        ],
        P::Pending => vec![c(
            action,
            "log --pending",
            "waiting for me",
            p30_person,
            p30_agent,
        )
        .setup(p30_setup)],
        P::Revert => vec![c(
            action,
            "revert",
            "an assistant's change",
            p31_person,
            p31_agent,
        )
        .setup(aide_setup)],
        P::Blame => vec![
            c(
                action,
                "blame",
                "import, person, agent, assistant, outside edit",
                p22_person,
                p22_agent,
            )
            .setup(blame_setup),
            c(
                action,
                "blame",
                "-L: the lines in view",
                p22_range_person,
                p22_range_agent,
            )
            .setup(blame_setup),
            c(
                action,
                "blame",
                "no history right: refused",
                p22_person,
                p22_agent,
            )
            .setup(blame_no_history_setup),
        ],
        // not bound by the kit yet: the pack's list marks these deferred
        P::Annotate | P::Mention | P::Purge => Vec::new(),
        // tier 0 only: `kit_blob_cases`
        P::Lease | P::ReleaseLease | P::BreakLease => Vec::new(),
    }
}

fn p17_status_like(a: &mut Agent<'_>) -> R {
    agent_step(a.verb(&["status"], b"")?, |o| match &o.value {
        VerbValue::Status(s) => disk_out(&s.disk, &s.declined),
        v => Output::value(v),
    })
}
