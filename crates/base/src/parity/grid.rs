//! The parity harness's grid stage (A11, K-PAR-3, CSV contract §3.5): the same check as
//! [`super::check`], on a grid doc (a [`crate::table`] pack's session). Each case runs the
//! **person path** (a tab: the session API directly) and the **agent path** (registered verbs
//! only: the kit's of [`Tier::Grid`] and the pack's structured verbs) on two identical grid
//! docs, then compares **the grid by key** (rows and columns in order, cells, names, quoting,
//! meta), the rendered bytes and the file, changes (with their element counts) and versions,
//! approvals, the actor's roster entry (its cell caret, view, last edit and claims by key) and
//! the exit and output.
//!
//! The kit's grid cases are one exhaustive `match` ([`super::kit_grid_cases`]); the grid
//! actions' own verbs (P35-P43: `row insert`...) are a pack's, so a pack (or the kit's test
//! table pack) adds their cases.

use std::fmt;
use std::sync::Arc;

use super::action::PersonAction;
use super::harness::{
    list_check, mark, wrong_tier, Agent, AgentFn, Failure, Mark, Output, ParityRow, Person,
    PersonFn, Report, RowStatus, SetupFn, Side, Stage, Step, VersionObs, ACTOR, AGENT_VIA,
    PERSON_VIA,
};
use super::registry::{Registry, Tier};
use crate::doc::Neutral;
use crate::elem::Key;
use crate::session::{
    ApprovalState, ChangeKind, ClaimSpan, DiskState, LogItem, LogQuery, Pack, Role,
};
use crate::store::{DocKey, ElemCounts, Seq, VersionReason};

/// The grid stage's doc file.
pub const GRID_PATH: &str = "table.csv";

/// The default grid doc: a header and four records. Its rows are `r1`… and its columns `c1`…
/// whatever the pack's header setting (the import mints keys in file order; with the header
/// on, `r1` is the first record after it), so the kit's cases name `r1`-`r3` and `c1`, `c2`.
pub const GRID_DOC: &str = "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\nk4,Kyiv\n";

/// One grid parity check: an action, the verb under test, the doc, others' setup, and both
/// paths (the same paths as text cases: [`Person`] and [`Agent`] work on any doc).
#[derive(Clone, Copy)]
pub struct GridCase {
    pub action: PersonAction,
    /// The registry form under test (the agent path must run it).
    pub verb: &'static str,
    /// What this case shows, a few words.
    pub name: &'static str,
    pub initial: &'static str,
    pub setup: SetupFn,
    pub person: PersonFn,
    pub agent: AgentFn,
}

fn no_setup(_: &mut Stage) -> Result<(), String> {
    Ok(())
}

impl GridCase {
    /// A case on [`GRID_DOC`] with no setup.
    pub const fn new(
        action: PersonAction,
        verb: &'static str,
        name: &'static str,
        person: PersonFn,
        agent: AgentFn,
    ) -> Self {
        GridCase {
            action,
            verb,
            name,
            initial: GRID_DOC,
            setup: no_setup,
            person,
            agent,
        }
    }
    pub const fn initial(self, initial: &'static str) -> Self {
        GridCase { initial, ..self }
    }
    pub const fn setup(self, setup: SetupFn) -> Self {
        GridCase { setup, ..self }
    }
}

impl fmt::Debug for GridCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?} (grid: {})", self.action, self.verb, self.name)
    }
}

/// A change on a grid, as the log shows it (without its time and via): its kind, author and
/// element counts (contract §5.1: `+2 rows −1 row ~5 cells`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridChangeObs {
    pub kind: ChangeKind,
    pub of: Option<u64>,
    pub author: String,
    pub on_behalf_of: Option<String>,
    pub elems: Vec<ElemCounts>,
    pub approval: Option<ApprovalState>,
}

/// A claim on a grid as others see it: its span by key, its rendered lines now, hold, note
/// and TTL as taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridClaimObs {
    pub span: ClaimSpan,
    pub lines: (usize, usize),
    pub hold: bool,
    pub note: Option<String>,
    pub ttl_ms: u64,
}

/// The actor's presence on a grid, as others see it in the roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridPresenceObs {
    pub name: String,
    pub colour: String,
    pub role: Role,
    pub away: bool,
    /// The cell caret or selection: anchor and head `(row, col)`.
    pub cell: Option<((Key, Key), (Key, Key))>,
    /// The rendered lines of the rows in view.
    pub view: Option<(usize, usize)>,
    /// The rendered lines of the rows its last change touched.
    pub last_edit: Option<(usize, usize)>,
    pub connections: usize,
    pub claims: Vec<GridClaimObs>,
    pub selection: Option<crate::session::ElemSelection>,
    pub pointer: Option<crate::session::Point>,
    pub viewport: Option<crate::session::Rect>,
}

/// Everything compared after a grid path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridObservation {
    /// The grid by key: every root's content (rows and columns in order, cells, names,
    /// quoting, meta).
    pub content: Vec<(String, Neutral)>,
    /// The rendered bytes (a read).
    pub bytes: Vec<u8>,
    pub key: DocKey,
    pub file: Option<Vec<u8>>,
    pub disk: Option<DiskState>,
    pub declined: Option<String>,
    pub changes: Vec<GridChangeObs>,
    /// Versions cut during the path, without the agent-only safety cut (`BeforeAgent`).
    pub versions: Vec<VersionObs>,
    pub approvals: Vec<(Seq, ApprovalState, Option<String>)>,
    pub actor: Option<GridPresenceObs>,
    pub exit: u8,
    pub output: Output,
}

fn observe(stage: &Stage, m: &Mark, step: Step, via: &str) -> Result<GridObservation, String> {
    let s = stage.session();
    let o = stage.observer();
    let status = s.status().map_err(|e| e.to_string())?;
    let read = s.read(o).map_err(|e| format!("observer read: {e}"))?;
    let log = s
        .log(o, &LogQuery::default())
        .map_err(|e| format!("observer log: {e}"))?;
    let mut changes = Vec::new();
    for item in log.iter().rev() {
        if let LogItem::Change { info, approval } = item {
            if info.seq <= m.seq {
                continue;
            }
            if info.author == ACTOR && info.via != via {
                return Err(format!(
                    "change {} by the actor came via {:?}, not this path's {via:?}: the path \
                     acted outside its handle",
                    info.seq, info.via
                ));
            }
            changes.push(GridChangeObs {
                kind: info.kind,
                of: info.of,
                author: info.author.clone(),
                on_behalf_of: info.on_behalf_of.clone(),
                elems: info.elems.clone(),
                approval: *approval,
            });
        }
    }
    let versions = stage
        .versions()?
        .into_iter()
        .filter(|v| v.id > m.version_id && v.reason != VersionReason::BeforeAgent)
        .map(|v| VersionObs {
            reason: v.reason,
            name: v.name,
            authors: v.authors,
        })
        .collect();
    let approvals = s
        .approvals(o)
        .map_err(|e| format!("observer approvals: {e}"))?
        .into_iter()
        .map(|(seq, a)| (seq, a.state, a.by))
        .collect();
    let actor = s
        .roster(o)
        .map_err(|e| format!("observer roster: {e}"))?
        .into_iter()
        .find(|e| e.participant.pid == ACTOR)
        .map(|e| GridPresenceObs {
            name: e.participant.name,
            colour: e.participant.colour,
            role: e.role,
            away: e.away,
            cell: e.cell.map(|c| (c.anchor, c.head)),
            view: e.view.map(|v| (v.first, v.last)),
            last_edit: e.last_edit.map(|v| (v.first, v.last)),
            connections: e.connections,
            selection: e.selection,
            pointer: e.pointer,
            viewport: e.viewport,
            claims: e
                .claims
                .iter()
                .map(|c| GridClaimObs {
                    span: c.span.clone(),
                    lines: (c.lines.first, c.lines.last),
                    hold: c.hold,
                    note: c.note.clone(),
                    ttl_ms: c.expires_ms - c.taken_ms,
                })
                .collect(),
        });
    let file = s
        .file_path()
        .map_err(|e| e.to_string())?
        .and_then(|p| std::fs::read(p).ok());
    Ok(GridObservation {
        content: read.content,
        bytes: read.bytes,
        key: status.key,
        file,
        disk: status.disk,
        declined: status.declined,
        changes,
        versions,
        approvals,
        actor,
        exit: step.exit,
        output: step.output,
    })
}

fn differences(p: &GridObservation, a: &GridObservation) -> Vec<String> {
    let mut d = Vec::new();
    macro_rules! cmp {
        ($($f:ident),*) => {$(
            if p.$f != a.$f {
                d.push(format!(
                    "{}:\n      person: {:?}\n      agent:  {:?}",
                    stringify!($f), p.$f, a.$f
                ));
            }
        )*};
    }
    cmp!(
        content, bytes, key, file, disk, declined, changes, versions, approvals, actor, exit,
        output
    );
    d
}

/// Run one grid case: the person path on one fresh world, the agent path on another, then
/// compare. Returns the failures (empty when the paths agree).
pub fn run_grid_case(pack: &Arc<dyn Pack>, registry: &Registry, case: &GridCase) -> Vec<Failure> {
    match observe_grid_case(pack, registry, case) {
        Err(f) => f,
        Ok((person, agent, mut problems)) => {
            let diffs = differences(&person, &agent);
            if !diffs.is_empty() {
                problems.push(Failure::Mismatch {
                    case: format!("{case:?}"),
                    diffs,
                });
            }
            problems
        }
    }
}

/// Run one grid case's two paths and return what each observed (person, agent), with the
/// agent path's registry problems (a missing verb, an undeclared call or exit): for a test's
/// own assertions on **what** a case did, beyond the paths agreeing. A path that couldn't
/// run is the error.
pub fn observe_grid_case(
    pack: &Arc<dyn Pack>,
    registry: &Registry,
    case: &GridCase,
) -> Result<(GridObservation, GridObservation, Vec<Failure>), Vec<Failure>> {
    let label = format!("{case:?}");
    let fail = |side, error: String| {
        vec![Failure::Path {
            case: label.clone(),
            side,
            error,
        }]
    };
    let world = || -> Result<(Stage, Mark), String> {
        let mut st = Stage::new_at(pack.clone(), case.initial, GRID_PATH)?;
        (case.setup)(&mut st)?;
        let m = mark(&st)?;
        Ok((st, m))
    };
    let person = match world() {
        Err(e) => return Err(fail(Side::Setup, e)),
        Ok((mut st, m)) => {
            let mut p = Person::on(&mut st);
            let step = (case.person)(&mut p);
            drop(p);
            match step.and_then(|s| observe(&st, &m, s, PERSON_VIA)) {
                Ok(o) => o,
                Err(e) => return Err(fail(Side::Person, e)),
            }
        }
    };
    match world() {
        Err(e) => Err(fail(Side::Setup, e)),
        Ok((mut st, m)) => {
            let mut a = Agent::on(&mut st, registry, case.action);
            let step = (case.agent)(&mut a);
            let problems = a.take_problems();
            drop(a);
            match step.and_then(|s| observe(&st, &m, s, AGENT_VIA)) {
                Ok(o) => Ok((person, o, problems)),
                Err(e) => {
                    let mut f = problems;
                    if !f.iter().any(|x| matches!(x, Failure::MissingVerb { .. })) {
                        f.extend(fail(Side::Agent, e));
                    }
                    Err(f)
                }
            }
        }
    }
}

fn all_grid_cases(extra: &[GridCase]) -> Vec<GridCase> {
    let mut cases: Vec<GridCase> = PersonAction::ALL
        .iter()
        .flat_map(|a| super::kit_grid_cases(*a))
        .collect();
    cases.extend_from_slice(extra);
    cases
}

fn covered(cases: &[GridCase]) -> Vec<(PersonAction, &'static str)> {
    cases.iter().map(|c| (c.action, c.verb)).collect()
}

/// The parity check for a grid pack (A11), as [`super::check`] is for a text pack: one row
/// per action of [`Tier::Grid`]; the list and the grid registry ([`Registry::for_pack`] of a
/// [`crate::table`] pack: the kit's grid verbs and the pack's structured verbs) agree both
/// ways; every listed verb has a case ([`super::kit_grid_cases`], or the pack's `extra`), and
/// in every case the agent's verb has the same effect as the person path: the grid by key,
/// the rendered bytes and the file, attribution (with element counts), versions, approvals,
/// the actor's presence by key, exit and output. Returns every failure.
pub fn check_grid(
    pack: Arc<dyn Pack>,
    rows: &[ParityRow],
    registry: &Registry,
    extra: &[GridCase],
) -> Result<Report, Vec<Failure>> {
    if let Some(f) = wrong_tier(registry, Tier::Grid) {
        return Err(vec![f]);
    }
    if !crate::table::is_table(&pack.schema()) {
        return Err(vec![Failure::Registry(format!(
            "the grid stage runs a table pack; {:?} isn't one",
            pack.schema().name
        ))]);
    }
    let all = all_grid_cases(extra);
    let (mut report, mut failures) = list_check(rows, registry, &covered(&all), |a| {
        !super::kit_grid_cases(a).is_empty()
    });
    let cases: Vec<GridCase> = all
        .into_iter()
        .filter(|c| {
            !rows
                .iter()
                .any(|r| r.action == c.action && matches!(r.status, RowStatus::Deferred { .. }))
        })
        .collect();
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 4);
    let chunk = cases.len().div_ceil(threads).max(1);
    let results: Vec<Vec<Failure>> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                let pack = pack.clone();
                scope.spawn(move || {
                    part.iter()
                        .map(|c| run_grid_case(&pack, registry, c))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| match h.join() {
                Ok(r) => r,
                Err(p) => std::panic::resume_unwind(p),
            })
            .collect()
    });
    for (c, f) in cases.iter().zip(results) {
        if f.is_empty() {
            report.matched.push((c.action, c.verb, c.name));
        }
        failures.extend(f);
    }
    if failures.is_empty() {
        Ok(report)
    } else {
        Err(failures)
    }
}

/// Steps 1 and 2 of [`check_grid`] and case coverage, without opening docs.
pub fn check_grid_list(
    rows: &[ParityRow],
    registry: &Registry,
    extra: &[GridCase],
) -> Vec<Failure> {
    if let Some(f) = wrong_tier(registry, Tier::Grid) {
        return vec![f];
    }
    list_check(rows, registry, &covered(&all_grid_cases(extra)), |a| {
        !super::kit_grid_cases(a).is_empty()
    })
    .1
}
