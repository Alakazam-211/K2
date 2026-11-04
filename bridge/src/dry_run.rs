//! A call run on a file, and the dry run.
//!
//! The run, under the host's writer lock: open the file; for a change, read the guard of the file
//! on disk first, then plan, apply, and save to `out`. Reading the guard apart tells a file the
//! host can't read (exit 4) from a refusal on writing (exit 2), and a file that moved on since it
//! was read is MPC-E832 (`busy`), after which the host starts again from the open. The host asks
//! [`k2_mpc_board::BoardFile::still_current`] once more just before it replaces the file.
//!
//! The dry run is the same run with the save into nowhere ([`std::io::sink`]): every refusal a
//! real run gives shows, with the same exit, and the file is never written. Its echo's version is
//! the guard read, since the file didn't change; what the save would have written is in its value
//! as `would_write`.
//!
//! A refusal is boxed, as the board crate's file calls box theirs: it carries the echo and a
//! report.
//!
//! A change's value (every board value through [`crate::value_out`]):
//!
//! ```text
//! {"verb", "ids", "labels", "removed", "removed_locked", "orphans", "skipped",
//!  "report": {"added", "changed", "deleted", "already_gone", "moved", "settings_changed",
//!             "people_added", "blobs_cleared"},
//!  "saved" | "would_write": {"guard", "revs"} or null when nothing changed}
//! ```
//!
//! A read's value is the parser's answer, as given.

use crate::answer::{Answer, Refused, Why};
use crate::parse::{Call, CallKind};
use crate::report::{conflict_report, io_report, refusal_report, save_report};
use crate::rows::Phase;
use crate::value::value_out;
use crate::HostBug;
use k2_mpc_base::errors::Value as KValue;
use k2_mpc_board::verbs::{reference_value, Ctx, Planned, Removed};
use k2_mpc_board::{
    package_guard_of, BoardFile, FileTarget, Limits, On, OrderList, Report, SaveError, Saved,
};
use std::io::{self, Read, Seek, SeekFrom, Write};

/// Runs `call` on the file: `file` the bytes as read, `on_disk` the file on disk now (for the
/// guard), `out` where the new file goes. A read never touches `on_disk` or `out`.
pub fn run<F: Read + Seek, D: Read + Seek, W: Write>(
    call: &Call,
    target: FileTarget,
    file: F,
    on_disk: D,
    out: &mut W,
    cx: &mut Ctx<'_>,
    limits: &Limits,
) -> Result<Answer, Box<Refused>> {
    go(call, target, file, on_disk, Some(out), cx, limits)
}

/// [`run`] with the save into nowhere: the file is never written, and the answer says so.
pub fn dry_run<F: Read + Seek, D: Read + Seek>(
    call: &Call,
    target: FileTarget,
    file: F,
    on_disk: D,
    cx: &mut Ctx<'_>,
    limits: &Limits,
) -> Result<Answer, Box<Refused>> {
    go::<F, D, io::Sink>(call, target, file, on_disk, None, cx, limits)
}

/// The run; with no `out`, the dry run, saving into nowhere.
fn go<F: Read + Seek, D: Read + Seek, W: Write>(
    call: &Call,
    target: FileTarget,
    file: F,
    mut on_disk: D,
    out: Option<&mut W>,
    cx: &mut Ctx<'_>,
    limits: &Limits,
) -> Result<Answer, Box<Refused>> {
    let dry = out.is_none();
    let mut sink = io::sink();
    let mut out: &mut dyn Write = match out {
        Some(w) => w,
        None => &mut sink,
    };
    let mut file = match BoardFile::open(target, file, limits) {
        Ok(file) => file,
        Err(e) => return Err(refused(e.on, refusal_report(&e.value, Phase::Open))),
    };
    let read_on = file.on();
    if call.kind() == CallKind::Read {
        return match call.read(file.board()) {
            Ok(value) => Ok(Answer {
                on: read_on,
                value,
                dry_run: dry,
            }),
            Err(why) => Err(Box::new(Refused { on: read_on, why })),
        };
    }
    // the guard, apart, before anything is planned
    let read = file.guard();
    let now = rewind(&mut on_disk)
        .map_err(|e| refused(read_on.clone(), io_report(&e.to_string())))
        .and_then(|()| {
            package_guard_of(&mut on_disk, limits)
                .map_err(|r| refused(read_on.clone(), refusal_report(&r, Phase::GuardRead)))
        })?;
    if now != read {
        let stale = SaveError::Stale { read, on_disk: now };
        return Err(refused(read_on, save_report(&stale)));
    }
    let planned = call.plan(file.board(), cx).map_err(|why| {
        Box::new(Refused {
            on: read_on.clone(),
            why,
        })
    })?;
    // a raw op's refusal is the ops' own; the rows by phase: tests/rows.rs,
    // each_code_a_group_can_give_has_its_ruled_rows (GROUP_ROWS) and
    // a_number_past_two_to_the_53_takes_the_row_of_the_call
    let phase = if call.name() == "apply" {
        Phase::Ops
    } else {
        Phase::Input
    };
    let report = match file.apply(&planned.group) {
        Ok(r) => r.value,
        Err(e) => return Err(refused(e.on, conflict_report(&e.value, phase))),
    };
    rewind(&mut on_disk).map_err(|e| refused(read_on.clone(), io_report(&e.to_string())))?;
    let saved = match file.save(&mut on_disk, &mut out) {
        Ok(s) => s,
        Err(e) => return Err(refused(e.on, save_report(&e.value))),
    };
    // A host bug here can't be reached from an opened file: `removed` and `orphans` are the opened
    // board's own objects and references, whose numbers passed the file's JSON reader (finite).
    let value = change_value(call.name(), &planned, &report, &saved.value, dry)
        .map_err(|b| refused(read_on.clone(), Err(b)))?;
    Ok(Answer {
        on: if dry { read_on } else { saved.on },
        value,
        dry_run: dry,
    })
}

fn rewind<D: Seek>(d: &mut D) -> io::Result<()> {
    d.seek(SeekFrom::Start(0)).map(|_| ())
}

fn refused(on: On, r: Result<k2_mpc_base::errors::ErrorReport, HostBug>) -> Box<Refused> {
    Box::new(Refused {
        on,
        why: Why::from(r),
    })
}

fn strs<T: ToString>(items: impl IntoIterator<Item = T>) -> KValue {
    KValue::List(
        items
            .into_iter()
            .map(|i| KValue::Str(i.to_string()))
            .collect(),
    )
}

fn removed_value(r: &Removed) -> Result<KValue, HostBug> {
    Ok(KValue::obj([
        ("object", value_out(&r.object.to_value())?),
        ("after", strs(r.after.iter().map(|i| i.as_str()))),
        (
            "frame_after",
            strs(r.frame_after.iter().map(|i| i.as_str())),
        ),
    ]))
}

fn list_word(l: OrderList) -> &'static str {
    match l {
        OrderList::Objects => "objects",
        OrderList::Frames => "frames",
    }
}

fn report_value(r: &Report) -> KValue {
    KValue::obj([
        ("added", strs(r.added.iter().map(|i| i.as_str()))),
        ("changed", strs(r.changed.iter().map(|i| i.as_str()))),
        ("deleted", strs(r.deleted.iter().map(|i| i.as_str()))),
        (
            "already_gone",
            strs(r.already_gone.iter().map(|i| i.as_str())),
        ),
        (
            "moved",
            KValue::List(
                r.moved
                    .iter()
                    .map(|m| {
                        KValue::obj([
                            ("id", KValue::from(m.id.as_str())),
                            ("list", KValue::from(list_word(m.list))),
                            ("after", KValue::from(m.after.as_ref().map(|a| a.as_str()))),
                            ("exact", KValue::Bool(m.exact)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("settings_changed", KValue::Bool(r.settings_changed)),
        (
            "people_added",
            strs(r.people_added.iter().map(|i| i.as_str())),
        ),
        ("blobs_cleared", strs(&r.blobs_cleared)),
    ])
}

fn saved_value(s: &Saved) -> KValue {
    match s {
        Saved::Nothing => KValue::Null,
        Saved::Wrote { written, revs } => KValue::obj([
            ("guard", KValue::from(written.package_guard.to_string())),
            (
                "revs",
                KValue::Obj(
                    revs.iter()
                        .map(|(id, rev)| (id.as_str().to_string(), KValue::from(rev.to_string())))
                        .collect(),
                ),
            ),
        ]),
    }
}

fn change_value(
    verb: &str,
    p: &Planned,
    report: &Report,
    saved: &Saved,
    dry: bool,
) -> Result<KValue, HostBug> {
    let mut removed = Vec::with_capacity(p.removed.len());
    for r in &p.removed {
        removed.push(removed_value(r)?);
    }
    let mut orphans = Vec::with_capacity(p.orphans.len());
    for r in &p.orphans {
        orphans.push(value_out(&reference_value(r))?);
    }
    Ok(KValue::obj([
        ("verb", KValue::from(verb)),
        ("ids", strs(p.ids.iter().map(|i| i.as_str()))),
        (
            "labels",
            KValue::Obj(
                p.labels
                    .iter()
                    .map(|(k, v)| (k.clone(), KValue::from(v.as_str())))
                    .collect(),
            ),
        ),
        ("removed", KValue::List(removed)),
        (
            "removed_locked",
            strs(p.removed_locked.iter().map(|i| i.as_str())),
        ),
        ("orphans", KValue::List(orphans)),
        ("skipped", strs(p.skipped.iter().map(|i| i.as_str()))),
        ("report", report_value(report)),
        (
            if dry { "would_write" } else { "saved" },
            saved_value(saved),
        ),
    ]))
}
