//! The kit's report for each refusal of the board crate: its row, the finished message, and every
//! key the row's message names in `details`.
//!
//! The message is the row's template filled from the same details (a string as it is, anything
//! else as JSON). A key the template names and the refusal lacks is a [`HostBug`], never a
//! message with a `{` left in it. Every board value in details goes through [`value_out`], so a
//! report's values match what `read` gives.

use crate::bug::HostBug;
use crate::rows::{conflict_row, impossible_pair, refusal_row, row, verb_code_row, Phase};
use crate::value::value_out;
use k2_mpc_base::errors::{ErrorEntry, ErrorReport, Value as KValue};
use k2_mpc_board::json::{Fields, Value as BValue};
use k2_mpc_board::verbs::{Facts, VerbCode, VerbError};
use k2_mpc_board::{
    ConflictKind, ExtraConflict, Held, Id, MergeConflicts, MergeError, ObjectConflict,
    PersonConflict, Refusal, SaveError, SettingsConflict, Target,
};

type Details = Vec<(&'static str, KValue)>;

/// The report: the row, its template filled from `details`, and `details`.
fn finish(entry: &'static ErrorEntry, details: Details) -> Result<ErrorReport, HostBug> {
    let message = fill(entry, &details)?;
    let mut r = ErrorReport::new(entry, message);
    for (k, v) in details {
        r = r.with(k, v);
    }
    Ok(r)
}

/// The row's message with each `{key}` replaced by its detail.
fn fill(entry: &'static ErrorEntry, details: &Details) -> Result<String, HostBug> {
    let mut out = String::new();
    let mut rest = entry.message;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or_else(|| HostBug::MissingFact {
            row: entry.id,
            key: after.to_string(),
        })?;
        let key = &after[..close];
        let value = details
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .ok_or_else(|| HostBug::MissingFact {
                row: entry.id,
                key: key.to_string(),
            })?;
        match value {
            KValue::Str(s) => out.push_str(s),
            other => out.push_str(&other.to_json()),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn need<T>(entry: &'static ErrorEntry, key: &str, v: Option<T>) -> Result<T, HostBug> {
    v.ok_or_else(|| HostBug::MissingFact {
        row: entry.id,
        key: key.to_string(),
    })
}

fn s(text: &str) -> KValue {
    KValue::Str(text.to_string())
}

/// A format refusal, by the call it came from. Every one carries its exact code in
/// `details.code`, and `details.at` where the board crate says where.
pub fn refusal_report(r: &Refusal, phase: Phase) -> Result<ErrorReport, HostBug> {
    let entry = refusal_row(r.code, phase)?;
    let mut d: Details = vec![("code", s(r.code.as_str()))];
    if let Some(at) = &r.at {
        d.push(("at", s(at)));
    }
    match entry.id {
        // a missing picture: the part's name is blobs/<hash>
        "MPC-E841" => {
            let hash = r.at.as_deref().and_then(|a| a.strip_prefix("blobs/"));
            d.push(("hash", s(need(entry, "hash", hash)?)));
        }
        // a locked object: at /objects/<id>
        "MPC-E835" => {
            let id = r.at.as_deref().and_then(|a| a.strip_prefix("/objects/"));
            d.push(("id", s(need(entry, "id", id)?)));
        }
        "MPC-E844" => {
            let needs = need(entry, "needs", r.needs_reader)?;
            d.push(("needs", KValue::from(u64::from(needs))));
            d.push(("have", KValue::from(u64::from(k2_mpc_board::MIN_READER))));
        }
        "MPC-E857" => d.push(("reason", s(&r.detail))),
        _ => {}
    }
    if !r.bad_ids.is_empty() {
        d.push((
            "bad_ids",
            KValue::list(r.bad_ids.iter().map(String::as_str)),
        ));
        d.push(("bad_ids_total", KValue::from(r.bad_ids_total)));
        d.push(("bad_ids_truncated", KValue::Bool(r.bad_ids_truncated)));
    }
    finish(entry, d)
}

/// What a target is, as three keys: `target` (what the message shows), `what`, and `id`.
fn target_details(t: &Target, d: &mut Details) {
    match t {
        Target::Object(id) => {
            d.push(("target", s(id.as_str())));
            d.push(("what", s("object")));
            d.push(("id", s(id.as_str())));
        }
        Target::Settings => {
            d.push(("target", s("the settings")));
            d.push(("what", s("settings")));
        }
        Target::Person(token) => {
            d.push(("target", s(token.as_str())));
            d.push(("what", s("person")));
            d.push(("id", s(token.as_str())));
        }
    }
}

/// A group that didn't apply. A refused one takes its refusal's row, by `phase`.
pub fn conflict_report(c: &k2_mpc_board::Conflict, phase: Phase) -> Result<ErrorReport, HostBug> {
    use k2_mpc_board::Conflict;
    let entry = conflict_row(c, phase)?;
    let mut d: Details = Vec::new();
    match c {
        Conflict::Changed { target, theirs } | Conflict::Exists { target, theirs } => {
            target_details(target, &mut d);
            d.push(("theirs", value_out(theirs)?));
        }
        Conflict::Deleted { id } => d.push(("id", s(id.as_str()))),
        Conflict::Refused(r) => return refusal_report(r, phase),
    }
    finish(entry, d)
}

/// A verb that couldn't be planned: `verb` is the verb run, `phase` the call a refusal came from.
pub fn verb_error_report(e: &VerbError, verb: &str, phase: Phase) -> Result<ErrorReport, HostBug> {
    match e {
        VerbError::Conflict(c) => conflict_report(c, phase),
        VerbError::Refused {
            code,
            detail,
            facts,
        } => verb_refusal(*code, detail, facts, verb),
        other => Err(HostBug::UnknownError {
            what: format!("{other:?}"),
        }),
    }
}

/// A verb's own refusal: the verb from the call, the rest from the facts the board crate attached.
fn verb_refusal(
    code: VerbCode,
    detail: &str,
    facts: &Facts,
    verb: &str,
) -> Result<ErrorReport, HostBug> {
    let entry = verb_code_row(code)?;
    let mut d: Details = Vec::new();
    let fact = |key: &str, v: &Option<String>| need(entry, key, v.as_deref()).map(s);
    match code {
        VerbCode::NeedsActor => d.push(("verb", s(verb))),
        VerbCode::VotingOff | VerbCode::NoVotesLeft | VerbCode::NoFreeId => {}
        VerbCode::WrongType => {
            d.push(("verb", s(verb)));
            let id = need(entry, "id", facts.id.as_ref())?;
            d.push(("id", s(id.as_str())));
            d.push(("type", fact("type", &facts.type_name)?));
        }
        VerbCode::TooFew => {
            d.push(("verb", s(verb)));
            d.push(("min", KValue::from(need(entry, "min", facts.min)?)));
        }
        VerbCode::NoTemplate => d.push(("name", fact("name", &facts.name)?)),
        VerbCode::UnknownField => {
            d.push(("type", fact("type", &facts.type_name)?));
            d.push(("field", fact("field", &facts.field)?));
        }
        VerbCode::UnknownType => d.push(("type", fact("type", &facts.type_name)?)),
        VerbCode::NoSuchItem => {
            let id = need(entry, "id", facts.id.as_ref())?;
            d.push(("id", s(id.as_str())));
            d.push(("item", fact("item", &facts.item)?));
        }
        VerbCode::BadValue => {
            d.push(("verb", s(verb)));
            d.push(("reason", s(detail)));
        }
    }
    finish(entry, d)
}

fn fields_value(f: &Fields) -> Result<KValue, HostBug> {
    value_out(&BValue::Obj(f.clone()))
}

/// One kept conflict: `what`, `id` where it has one, `kind`, and each side it has.
fn conflict_entry(
    what: &str,
    id: Option<&str>,
    kind: ConflictKind,
    sides: [Option<KValue>; 3],
) -> KValue {
    let mut o: Vec<(String, KValue)> = vec![("what".into(), s(what))];
    if let Some(id) = id {
        o.push(("id".into(), s(id)));
    }
    o.push(("kind".into(), s(kind.as_str())));
    for (name, side) in ["base", "ours", "theirs"].into_iter().zip(sides) {
        if let Some(v) = side {
            o.push((name.into(), v));
        }
    }
    KValue::Obj(o)
}

/// The kind a conflict with no kind of its own has, by the sides it has.
fn kind_by_sides(base: bool, ours: bool, theirs: bool) -> ConflictKind {
    match (base, ours, theirs) {
        (false, _, _) => ConflictKind::BothAdded,
        (true, false, _) => ConflictKind::OursDeleted,
        (true, _, false) => ConflictKind::TheirsDeleted,
        (true, true, true) => ConflictKind::BothChanged,
    }
}

fn settings_entry(c: &SettingsConflict) -> Result<KValue, HostBug> {
    Ok(conflict_entry(
        "settings",
        None,
        ConflictKind::BothChanged,
        [
            Some(fields_value(&c.base)?),
            Some(fields_value(&c.ours)?),
            Some(fields_value(&c.theirs)?),
        ],
    ))
}

fn extra_entry(c: &ExtraConflict) -> Result<KValue, HostBug> {
    let side = |v: &Option<BValue>| v.as_ref().map(value_out).transpose();
    Ok(conflict_entry(
        "extra",
        Some(&c.name),
        kind_by_sides(c.base.is_some(), c.ours.is_some(), c.theirs.is_some()),
        [side(&c.base)?, side(&c.ours)?, side(&c.theirs)?],
    ))
}

fn person_entry(c: &PersonConflict) -> Result<KValue, HostBug> {
    let base = c
        .base
        .as_ref()
        .map(|p| value_out(&p.to_value()))
        .transpose()?;
    Ok(conflict_entry(
        "person",
        Some(c.token.as_str()),
        kind_by_sides(c.base.is_some(), true, true),
        [
            base,
            Some(value_out(&c.ours.to_value())?),
            Some(value_out(&c.theirs.to_value())?),
        ],
    ))
}

fn object_entry(c: &ObjectConflict) -> Result<KValue, HostBug> {
    let side =
        |o: &Option<k2_mpc_board::Object>| o.as_ref().map(|o| value_out(&o.to_value())).transpose();
    Ok(conflict_entry(
        "object",
        Some(c.id.as_str()),
        c.kind,
        [side(&c.base)?, side(&c.ours)?, side(&c.theirs)?],
    ))
}

/// Every kept conflict, in the board crate's stated order: the settings, top-level fields by
/// name, people by token, objects by id.
fn merge_conflicts(m: &MergeConflicts) -> Result<Details, HostBug> {
    let mut list = Vec::with_capacity(m.kept());
    if let Some(c) = &m.settings {
        list.push(settings_entry(c)?);
    }
    for c in &m.extra {
        list.push(extra_entry(c)?);
    }
    for c in &m.people {
        list.push(person_entry(c)?);
    }
    for c in &m.objects {
        list.push(object_entry(c)?);
    }
    let t = &m.totals;
    Ok(vec![
        ("count", KValue::from(t.all())),
        ("conflicts", KValue::List(list)),
        (
            "totals",
            KValue::obj([
                ("settings", KValue::from(t.settings)),
                ("extra", KValue::from(t.extra)),
                ("people", KValue::from(t.people)),
                ("objects", KValue::from(t.objects)),
            ]),
        ),
        ("truncated", KValue::Bool(m.truncated)),
    ])
}

/// A merge that gave no board: the conflicts (MPC-E833, `count` every kind's total), or the
/// refusal of a result that wouldn't be valid.
pub fn merge_report(e: &MergeError) -> Result<ErrorReport, HostBug> {
    match e {
        MergeError::Conflicts(m) => finish(row("MPC-E833")?, merge_conflicts(m)?),
        MergeError::Refused(r) => refusal_report(r, Phase::Input),
    }
}

/// A save that wrote nothing usable. Its refusal is the write's: the guard is read first, apart,
/// so a refusal of the file on disk is the file's ([`Phase::GuardRead`]).
pub fn save_report(e: &SaveError) -> Result<ErrorReport, HostBug> {
    match e {
        SaveError::Stale { read, on_disk } => finish(
            row("MPC-E832")?,
            vec![
                ("read", s(&read.to_string())),
                ("on_disk", s(&on_disk.to_string())),
            ],
        ),
        SaveError::Refused(r) => refusal_report(r, Phase::Write),
    }
}

/// Why a group was held: `what` and `id` where it's about one thing, and `why`.
fn held_conflict(c: &k2_mpc_board::Conflict) -> Result<KValue, HostBug> {
    use k2_mpc_board::Conflict;
    impossible_pair(c)?;
    let mut d: Details = Vec::new();
    let why = match c {
        Conflict::Changed { target, .. } | Conflict::Exists { target, .. } => {
            target_details(target, &mut d);
            d.retain(|(k, _)| *k != "target");
            if matches!(c, Conflict::Changed { .. }) {
                "changed"
            } else {
                "exists"
            }
        }
        Conflict::Deleted { id } => {
            d.push(("what", s("object")));
            d.push(("id", s(id.as_str())));
            "deleted"
        }
        Conflict::Refused(r) => {
            if let Some(at) = &r.at {
                d.push(("at", s(at)));
            }
            r.code.as_str()
        }
    };
    d.push(("why", s(why)));
    Ok(KValue::obj(d))
}

/// The groups `apply_all` held, in the order they were given.
pub fn held_report(held: &[Held]) -> Result<ErrorReport, HostBug> {
    let mut list = Vec::with_capacity(held.len());
    for h in held {
        list.push(KValue::obj([
            ("group", s(&h.group.id)),
            ("conflict", held_conflict(&h.conflict)?),
        ]));
    }
    finish(
        row("MPC-E834")?,
        vec![
            ("count", KValue::from(held.len())),
            ("held", KValue::List(list)),
        ],
    )
}

/// No object has the id `read` was given.
pub fn no_object_report(id: &Id) -> Result<ErrorReport, HostBug> {
    finish(row("MPC-E838")?, vec![("id", s(id.as_str()))])
}

/// Reading or writing the file failed in the host.
pub fn io_report(reason: &str) -> Result<ErrorReport, HostBug> {
    finish(
        row("MPC-E857")?,
        vec![("code", s("io_error")), ("reason", s(reason))],
    )
}
