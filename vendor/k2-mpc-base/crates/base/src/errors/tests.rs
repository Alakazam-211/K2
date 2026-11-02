//! The registry's own checks, and the mapping: every error variant maps to a live id with
//! the exit code it always had, and every live id is some variant's (or outcome's) id.

use std::collections::{BTreeSet, HashSet};

use super::*;
use crate::blob::BlobError;
use crate::doc::DocError;
use crate::export::ExportError;
use crate::parity::{Registry, Tier};
use crate::schema::Kind;
use crate::session::{
    ClaimId, ConflictKind, ConflictRegion, ConnId, Declined, EditResult, Need, PackError,
    SessionError, UndoOutcome, WriteOutcome,
};
use crate::store::{DocKey, StoreError};
use crate::text::diff::HunkError;
use crate::text::merge::Conflict;
use crate::tools::{session_exit, ToolError, ToolRunError};

fn key() -> DocKey {
    DocKey {
        space: "s".into(),
        path: "p.md".into(),
    }
}

/// One of every store error.
fn store_errors_with(x: &str) -> Vec<StoreError> {
    use StoreError as S;
    vec![
        S::Db(x.into()),
        S::Newer { found: 99 },
        S::RewriteNewer { found: 7 },
        S::ForeignEngine {
            stored: "mpce-0.1".into(),
            stored_format: 1,
        },
        S::NewerData {
            table: "later_kit".into(),
        },
        S::Corrupt("log entry"),
        S::ClockOverflow(1 << 32),
        S::NoDoc(1),
        S::KeyTaken(key()),
        S::NoVersion(1, 2),
        S::Locked,
        S::NamedVersion(1, 2),
        S::KeyChange,
        S::NoApproval(1, 2),
        S::LogMoved(1, 2, 3),
        S::NoBlobSpace,
        S::NoBlob("ab".into()),
        S::CurrentVersion(1, 2),
        S::NoRandom(x.into()),
        S::PurgedVersion(1, 2),
    ]
}

fn store_errors() -> Vec<StoreError> {
    store_errors_with("x")
}

fn doc_errors_with(x: &str) -> Vec<DocError> {
    use DocError as D;
    vec![
        D::Decode(x.into()),
        D::ForeignClient {
            allowed: 5,
            found: 6,
        },
        D::MissingDeps {
            client: 5,
            from: 3,
            have: 1,
        },
        D::Pending,
        D::Apply(x.into()),
        D::Panic,
        D::Load(x.into()),
        D::Unaccepted,
        D::Quarantined,
        D::BadClient(0),
        D::NoRoot {
            schema: "markdown",
            root: "body".into(),
        },
        D::WrongKind {
            root: "source".into(),
            wanted: Kind::Text,
            actual: Kind::Map,
        },
        D::Unsupported(Kind::Tree),
        D::Hunk(HunkError::Order { index: 1 }),
        D::Hunk(HunkError::Range { index: 1 }),
        D::Hunk(HunkError::Empty { index: 1 }),
        D::RootName("a/b".into()),
        D::Op {
            root: "rows".into(),
            index: 0,
            refusal: crate::elem::OpRefusal::UnknownKey("r9".into()),
        },
        D::Malformed(x.into()),
    ]
}

/// One of every session error (and every inner error it carries).
fn session_errors_with(x: &str) -> Vec<SessionError> {
    use SessionError as S;
    let mut v = vec![
        S::Forbidden { need: Need::Read },
        S::Forbidden { need: Need::Edit },
        S::Forbidden {
            need: Need::History,
        },
        S::Forbidden {
            need: Need::Approver,
        },
        S::Forbidden { need: Need::Owner },
        S::Forbidden {
            need: Need::AuthorOrOwner,
        },
        S::NotJoined {
            conn: ConnId::fresh(),
        },
        S::Watching { max: 32 },
        S::Stale {
            expected: 12,
            actual: 14,
        },
        S::BaseExpired { base: 12 },
        S::RateLimited { retry_after_ms: 90 },
        S::NotFound(key()),
        S::PackMismatch {
            stored: "markdown".into(),
            pack: "text".into(),
        },
        S::EngineMismatch {
            stored: "loro-1.0".into(),
            stored_format: 1,
            engine: "yrs-0.28".into(),
            engine_format: 2,
        },
        S::BadParticipant(x.into()),
        S::BadPosition(x.into()),
        S::BadEdit(x.into()),
        S::NoApproval { seq: 3 },
        S::BadApproval(x.into()),
        S::BadPath(x.into()),
        S::Held {
            reason: "approvals_backlog",
        },
        S::RevertTooOld { seq: 3 },
        S::NoSuchChange(3),
        S::Disk(x.into()),
        S::Leased {
            holder: "h".into(),
            name: "H".into(),
            expires_ms: 5,
        },
        S::NoLease,
        S::Claimed {
            id: ClaimId::new(1),
            by: "b".into(),
            name: "B".into(),
            lines: (3, 5),
            remaining_ms: 900,
            note: Some("tables".into()),
        },
        S::NoClaim(None),
        S::NoClaim(Some(ClaimId::new(2))),
        S::ClaimContended {
            id: ClaimId::new(1),
            waiters: 2,
            remaining_ms: 5,
        },
        S::BadClaim("a claim needs a TTL above 0 ms".into()),
        // the caps, worded exactly as claims.rs words them
        S::BadClaim("this doc has 256 live claims (the most)".into()),
        S::BadClaim("you hold 16 claims here (the most); release one".into()),
        S::BadClaim("64 already waiting for claim-3".into()),
        S::Blob(BlobError::TooLarge { limit: 10 }),
        S::Blob(BlobError::NotFound("ab".into())),
        S::Blob(BlobError::Damaged("ab".into())),
        S::Blob(BlobError::Io(x.into())),
        S::Blob(BlobError::NoRandom(x.into())),
        S::NoBlobStore,
        S::Pack(PackError(x.into())),
        S::Unloaded,
        S::Released,
        S::Deleted,
        S::Closed { reason: "core_off" },
        S::Broken(x.into()),
        S::NeedsBase,
        S::UnknownElem {
            root: "rows",
            key: "r2".into(),
            dropped: Some(crate::session::DroppedBy {
                change: 4,
                by: "p1".into(),
                at_ms: 9,
            }),
        },
        S::UnknownElem {
            root: "cols",
            key: "c99".into(),
            dropped: None,
        },
        S::AtLimit {
            what: "cells",
            limit: 25_000,
            would_be: 25_001,
        },
        S::AtLimit {
            what: "value bytes",
            limit: 65_536,
            would_be: 70_000,
        },
        S::FacetConflict(vec![crate::session::FacetConflict {
            root: "rows".into(),
            key: "r3".into(),
            facet: crate::elem::Facet::Value,
            by: Some("p1".into()),
            change: 7,
            now: "deleted".into(),
            rewritten_row: false,
        }]),
    ];
    for code in [
        "too_large",
        "not_utf8",
        "binary",
        "mixed_line_endings",
        "declined",
        "unreadable",
        "ragged_quotes",
        "hard_linked",
    ] {
        v.push(S::Declined(Declined {
            code,
            message: x.into(),
        }));
    }
    v.extend(store_errors_with(x).into_iter().map(S::Store));
    v.extend(
        store_errors_with(x)
            .into_iter()
            .map(|e| S::Blob(BlobError::Store(e))),
    );
    v.extend(doc_errors_with(x).into_iter().map(S::Doc));
    v
}

fn session_errors() -> Vec<SessionError> {
    session_errors_with("x")
}

fn tool_errors_with(x: &str) -> Vec<ToolError> {
    vec![
        ToolError::unsupported("-z", "use -n"),
        ToolError::refused("sed w", "writes another file"),
        ToolError::usage(x),
        ToolError::Limit(x.into()),
        ToolError::address(crate::addr::ambiguous(
            x,
            "column",
            x,
            &[crate::addr::Candidate::new(x)],
        )),
        ToolError::address(
            ErrorReport::new(&super::ids::POSITIONAL_NEEDS_VERSION, x).with("address", x),
        ),
    ]
}

fn tool_errors() -> Vec<ToolError> {
    tool_errors_with("x")
}

fn region(kind: ConflictKind) -> ConflictRegion {
    ConflictRegion {
        root: "source".into(),
        range: 3..9,
        lines: (2, 2),
        kind,
        base: "b".into(),
        ours: "o".into(),
        theirs: "t".into(),
        key: None,
        why: None,
    }
}

fn outcomes() -> Vec<ErrorReport> {
    let mut v: Vec<ErrorReport> = [
        UndoOutcome::Conflicts {
            step: 1,
            conflicts: vec![region(ConflictKind::Overlap), region(ConflictKind::Taint)],
        },
        UndoOutcome::Blocked {
            conflicts: vec![region(ConflictKind::Erased)],
        },
        UndoOutcome::Applied {
            steps: 1,
            result: EditResult::default(),
            skipped: vec![region(ConflictKind::Overlap)],
            blocked: None,
        },
        UndoOutcome::HistoryLost,
        UndoOutcome::ClearedByPurge,
    ]
    .iter()
    .map(|o| o.refusal().expect("a refusal"))
    .collect();
    let c = Conflict {
        base: 0..3,
        lines: (1, 1),
        base_text: "a\n".into(),
        ours: "b\n".into(),
        theirs: "c\n".into(),
        ours_lines: (1, 1),
        theirs_lines: (2, 2),
    };
    v.push(WriteOutcome::Conflicts(vec![c.clone()]).refusal(7).unwrap());
    v.push(WriteOutcome::Conflicts(vec![c]).refusal_at(7, 9).unwrap());
    v.push(WriteOutcome::BaseExpired.refusal(7).unwrap());
    v.push(crate::tools::udiff_refusal_for_tests(
        "patching file p\n1 out of 2 hunks FAILED -- nothing applied\n",
        false,
    ));
    v.push(crate::tools::udiff_refusal_for_tests(
        "patching file p\n1 out of 2 hunks FAILED -- rejects follow\n",
        true,
    ));
    v
}

#[test]
fn ids_names_and_areas_are_well_formed() {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for e in entries() {
        let (prefix, digits) = if e.is_engine() {
            (ENGINE_PREFIX, &e.id[ENGINE_PREFIX.len()..])
        } else {
            assert!(e.id.starts_with(KIT_PREFIX), "{}", e.id);
            (KIT_PREFIX, &e.id[KIT_PREFIX.len()..])
        };
        assert!(
            digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit()),
            "{}: {prefix} and three digits",
            e.id
        );
        assert_eq!(
            digits.as_bytes()[0] - b'0',
            e.area.digit(),
            "{}: its hundreds digit is its area's",
            e.id
        );
        assert!(ids.insert(e.id), "{} twice", e.id);
        assert!(names.insert(e.name), "name {} twice", e.name);
        assert!(
            e.name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "{}: snake_case",
            e.name
        );
        assert!(e.exit <= 8, "{}: exit {} outside the table", e.id, e.exit);
        assert!(
            !e.message.is_empty() && !e.message.contains('\n'),
            "{}",
            e.id
        );
        assert!(e.next.len() > 20, "{}: say what to do next", e.id);
        assert!(!e.rule.is_empty(), "{}: name the rule", e.id);
        assert!(!e.verbs.is_empty(), "{}: name the verbs", e.id);
        assert_eq!(lookup(e.id), Some(e));
    }
    // the kit's are in id order (append only means: a new id goes at the end of its area)
    let kit: Vec<&str> = KIT.iter().map(|e| e.id).collect();
    let mut sorted = kit.clone();
    sorted.sort();
    assert_eq!(kit, sorted, "KIT is in id order");
    assert_eq!(lookup("e110").map(|e| e.id), Some("MPC-E110"));
    assert_eq!(lookup("110").map(|e| e.id), Some("MPC-E110"));
    assert_eq!(lookup("mpc-e110").map(|e| e.id), Some("MPC-E110"));
    assert_eq!(lookup("MPC-E999"), None);
}

/// Every verb an entry names is a verb (either tier), a meta verb, or a group.
#[test]
fn entries_name_real_verbs() {
    let kit = [
        Registry::for_tier(Tier::Text),
        Registry::for_tier(Tier::Blob),
    ];
    // host verbs the kit serves through its API but doesn't register (purge: deferred, P28),
    // and the CSV verbs the reserved ids name
    let other = ["purge", "cell set"];
    for e in entries() {
        for v in e.verbs {
            let ok = matches!(
                *v,
                "*" | "@change" | "@history" | "@run_tool" | "@pack" | "(sync)" | "(host)"
            ) || kit.iter().any(|r| r.get(v).is_some())
                || META_VERBS.iter().any(|(m, _)| m == v)
                || other.contains(v);
            assert!(ok, "{}: {v:?} is not a verb", e.id);
        }
    }
}

/// K-EXIT-1 and condition 2: an id never changes an exit code. Every variant maps to a live
/// id whose exit is the one `session_exit` / `ToolError::exit` gives.
#[test]
fn every_variant_maps_to_a_live_id_with_its_exit() {
    for e in session_errors() {
        let r = e.report();
        assert_eq!(r.exit(), session_exit(&e), "{e:?} → {}", r.id());
        assert_eq!(r.entry().status, Status::Live, "{e:?} → {}", r.id());
        assert_eq!(r.message(), e.to_string(), "the message is today's wording");
        assert_eq!(r.plain(), format!("{e} ({})", r.id()));
        let run = ToolRunError::Session(e.clone()).report();
        assert_eq!(run, r);
    }
    for e in export_errors() {
        let r = e.report();
        assert_eq!(r.exit(), e.exit(), "{e:?} → {}", r.id());
        assert_eq!(r.entry().status, Status::Live, "{e:?} → {}", r.id());
        assert_eq!(r.message(), e.to_string(), "the message is today's wording");
    }
    for e in tool_errors() {
        let r = e.report();
        assert_eq!(r.exit(), e.exit(), "{e:?}");
        assert_eq!(ToolRunError::Tool(e.clone()).report(), r);
    }
    for e in store_errors() {
        assert_eq!(
            e.report().exit(),
            session_exit(&SessionError::Store(e.clone())),
            "{e:?}"
        );
    }
    for o in outcomes() {
        assert_eq!(o.entry().status, Status::Live, "{}", o.id());
    }
    // the outcomes' exits are their own exit()s
    assert_eq!(
        WriteOutcome::BaseExpired.refusal(1).unwrap().exit(),
        WriteOutcome::BaseExpired.exit()
    );
    assert_eq!(
        UndoOutcome::HistoryLost.refusal().unwrap().exit(),
        UndoOutcome::HistoryLost.exit()
    );
}

/// "Every id is reachable or retired", at the mapping: every live kit id is some error's or
/// outcome's (the integration test `error_ids` reaches the `Reach::Verb` ones through the
/// API). Reserved ids map from nothing yet.
#[test]
fn every_live_id_has_a_path() {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    seen.extend(session_errors().iter().map(|e| e.report().id()));
    seen.extend(tool_errors().iter().map(|e| e.report().id()));
    seen.extend(export_errors().iter().map(|e| e.report().id()));
    seen.extend(outcomes().iter().map(ErrorReport::id));
    for code in ["too_large", "hard_linked", "x"] {
        seen.insert(
            Declined {
                code,
                message: "m".into(),
            }
            .report()
            .id(),
        );
    }
    let meta = run_meta(
        &["explain".to_owned(), "MPC-E999".to_owned()],
        false,
        &[],
        &crate::session::Config::default(),
    )
    .unwrap();
    seen.insert(meta.refusal.unwrap().id());
    // a change whose reply was lost: the JS client's own (client/src/wire.ts), no kit path
    seen.insert("MPC-E000");
    // exact replace and pack verbs: produced by run_tool (the integration test reaches them)
    seen.extend(["MPC-E125", "MPC-E126", "MPC-E127"]);
    // a grid verb's exit 4 (an unknown or deleted element): produced by run_tool on a table
    // (tests/grid_verbs.rs)
    seen.insert("MPC-E805");
    // a refused header switch (names that would render): the header verb on a table
    seen.insert("MPC-E806");
    // a refused header switch (the only row blank): the header verb on a table
    seen.insert("MPC-E807");
    for e in KIT.iter() {
        match e.status {
            Status::Live => assert!(seen.contains(e.id), "{} {} has no path", e.id, e.name),
            Status::Reserved(_) | Status::Retired(_) => {
                assert!(
                    !seen.contains(e.id),
                    "{} is {} but returned",
                    e.id,
                    e.status.word()
                )
            }
        }
    }
}

#[test]
fn json_shape_is_stable() {
    let r = SessionError::Stale {
        expected: 12,
        actual: 14,
    }
    .report();
    let j = r.to_json();
    assert!(j.starts_with(
        r#"{"id":"MPC-E110","exit":1,"message":"doc changed since 12 (now 14)","next":""#
    ));
    assert!(j.ends_with(
        r#","rule":"A14 (--if-version), K-T0-5","details":{"expected":12,"actual":14}}"#
    ));
    assert_eq!(r.plain(), "doc changed since 12 (now 14) (MPC-E110)");
    // details are never prose: no detail value is the message
    for e in session_errors() {
        let r = e.report();
        for (k, v) in r.details() {
            assert_ne!(v.as_str(), Some(r.message()), "{}: details.{k}", r.id());
        }
    }
}

#[test]
fn engine_codes_cross_as_strings() {
    let r = ErrorReport::engine("MPCE-E123", "the engine said no");
    assert_eq!(r.id(), "MPC-E161");
    assert_eq!(r.detail("code").and_then(Value::as_str), Some("MPCE-E123"));
    // a code the catalog lists is reported as itself
    let r = ErrorReport::engine("MPCE-E199", "the engine said no");
    assert_eq!(r.id(), "MPCE-E199");
    assert!(
        ENGINE.iter().all(|e| e.is_engine()),
        "the engine's table holds only MPCE ids (the engine allocates its own ranges)"
    );
}

#[test]
fn explain_and_capabilities_run() {
    let cfg = crate::session::Config::default();
    let argv = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let out = run_meta(&argv(&["explain", "E301"]), false, &[], &cfg).unwrap();
    assert_eq!(out.exit, 0);
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .starts_with("MPC-E301 claimed (presence and claims)\n"));
    let out = run_meta(&argv(&["explain", "nope"]), true, &[], &cfg).unwrap();
    assert_eq!((out.exit, out.refusal.unwrap().id()), (4, "MPC-E160"));
    let out = run_meta(&argv(&["capabilities", "--json"]), false, &[], &cfg).unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("\"edit_rate_per_s\": 10"));
    assert!(run_meta(&argv(&["cat"]), false, &[], &cfg).is_none());
    let out = run_meta(&argv(&["capabilities", "-x"]), false, &[], &cfg).unwrap();
    assert_eq!(out.refusal.unwrap().id(), "MPC-E120");
}

/// K7 (v0.9.8): every refusal, its message and every detail, with a known workspace root
/// fed through every string a constructor takes (a path inside the root, one outside it,
/// as OS errors print them), then [`ErrorReport::redacted`]: no absolute path survives; the
/// one inside is shown relative to the root.
#[test]
fn no_absolute_path_survives_redaction() {
    let root = std::path::Path::new("/srv/k2-redact-root");
    let x = "/srv/k2-redact-root/notes/a.md: Permission denied (os error 13) via /opt/secret/b.md";
    let mut reports: Vec<ErrorReport> = Vec::new();
    reports.extend(session_errors_with(x).iter().map(|e| e.report()));
    reports.extend(tool_errors_with(x).iter().map(|e| e.report()));
    reports.extend(store_errors_with(x).iter().map(|e| e.report()));
    reports.extend(doc_errors_with(x).iter().map(|e| e.report()));
    let mut carried = 0;
    for r in reports {
        let before = format!("{}\n{}", r.plain(), r.to_json());
        let red = r.redacted(root);
        let after = format!(
            "{}\n{}\n{}",
            red.plain(),
            red.to_json(),
            red.to_json_for("app")
        );
        assert!(
            !after.contains("/srv/") && !after.contains("/opt/"),
            "{after}"
        );
        if before.contains("/srv/") {
            carried += 1;
            assert!(after.contains("notes/a.md"), "{after}");
            assert!(after.contains("<outside>"), "{after}");
        }
    }
    assert!(
        carried > 10,
        "only {carried} reports carried the path: the test is vacuous"
    );
}

/// K6 (v0.9.8): a host's `next` for one surface changes that surface's `next` only: never
/// the id, exit code, message, rule or details, nor the registry's `next` elsewhere.
#[test]
fn next_override_never_changes_the_rest() {
    let mut reports: Vec<ErrorReport> = session_errors().iter().map(|e| e.report()).collect();
    reports.extend(outcomes());
    for r in reports {
        let o = r
            .clone()
            .with_next("app", "Ask the doc's owner.")
            .with_next("tab", "first")
            .with_next("tab", "Reload the tab.");
        assert_eq!(
            (o.id(), o.exit(), o.message(), o.rule()),
            (r.id(), r.exit(), r.message(), r.rule())
        );
        assert_eq!(o.details(), r.details());
        assert_eq!(o.next(), r.next());
        assert_eq!(o.to_json(), r.to_json());
        assert_eq!(o.next_for("app"), "Ask the doc's owner.");
        assert_eq!(o.next_for("tab"), "Reload the tab.");
        assert_eq!(o.next_for("cli"), r.next());
        // the surface's object differs in `next` only
        let a = o.to_json_for("app");
        assert_eq!(
            a.replace(&json_str("Ask the doc's owner."), &json_str(r.next())),
            r.to_json()
        );
    }
}

fn json_str(s: &str) -> String {
    Value::from(s).to_json()
}

/// One of each export error (crate::export).
fn export_errors() -> Vec<ExportError> {
    vec![
        ExportError::Unsupported {
            kind: "a markdown doc".into(),
            target: "png".into(),
        },
        ExportError::Declined {
            code: "bad_option",
            message: "m".into(),
        },
        ExportError::Limit("output bytes"),
        ExportError::Cancelled,
        ExportError::Decode("bad header".into()),
        ExportError::Io(std::io::Error::other("disk full")),
        ExportError::Internal {
            exporter: "x@1".into(),
            message: "boom".into(),
        },
    ]
}

/// K-PAR-10: the names a step's verb may take: the kit's verbs, the doc-less verbs and the
/// grid verbs, by their registry names (never a hand list).
fn kit_verb_names() -> BTreeSet<&'static str> {
    crate::parity::KIT_VERBS
        .iter()
        .map(|v| v.name)
        .chain(META_VERBS.iter().map(|(n, _)| *n))
        .chain(crate::table::verbs::VERBS.iter().map(|v| v.name()))
        .collect()
}

/// Host command text: a word `k2` (any case), `k2doc`, `k2 doc`, a shell prompt.
fn host_text(s: &str) -> Option<&'static str> {
    let low = s.to_ascii_lowercase();
    if low.contains("k2doc") || low.contains("k2 doc") {
        return Some("k2 doc");
    }
    if low
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .any(|w| w == "k2")
    {
        return Some("the word k2");
    }
    if s.starts_with("$ ") || s.contains("\n$ ") {
        return Some("a shell prompt");
    }
    None
}

/// K-PAR-10, over every entry: its step is absent (nothing to run), or valid: the verb is a
/// kit verb by its registry name (flags in `args`), with a reason.
#[test]
fn every_next_step_names_a_kit_verb_and_a_reason() {
    let verbs = kit_verb_names();
    assert!(verbs.contains("undo --partial") && verbs.contains("row list"));
    assert!(KIT.len() >= 80, "only {} entries", KIT.len());
    let (mut with, mut without) = (0, 0);
    for e in entries() {
        let Some(st) = e.step else {
            without += 1;
            continue;
        };
        with += 1;
        assert!(
            verbs.contains(st.verb),
            "{}: {:?} is not a kit verb",
            e.id,
            st.verb
        );
        assert!(!st.why.trim().is_empty(), "{} {}: no why", e.id, e.name);
        assert_eq!(st.why.trim(), st.why, "{}: why has outer space", e.id);
    }
    assert!(with > 40, "only {with} entries have a step");
    assert!(
        without > 0,
        "every entry has a step: the absent case is untested"
    );
}

/// K-PAR-10: no host command text in any entry's message or `next`, nor in a step's reason
/// or arguments; arguments are single argv tokens with no host prefix; a reason names no
/// command line.
#[test]
fn no_host_command_text_in_the_registry() {
    for e in entries() {
        for (what, s) in [("message", e.message), ("next", e.next)] {
            assert_eq!(host_text(s), None, "{} {what}: {s:?}", e.id);
        }
        let Some(st) = e.step else { continue };
        assert_eq!(host_text(st.why), None, "{} why: {:?}", e.id, st.why);
        assert_eq!(host_text(st.verb), None, "{} verb: {:?}", e.id, st.verb);
        for a in st.args {
            assert_eq!(host_text(a), None, "{} arg {a:?}", e.id);
            assert!(
                !a.is_empty() && !a.contains(char::is_whitespace),
                "{} arg {a:?}",
                e.id
            );
            let low = a.to_ascii_lowercase();
            assert!(
                low != "doc" && !low.starts_with("k2") && !a.contains('/'),
                "{} arg {a:?}: a host prefix or a path",
                e.id
            );
        }
        // the reason is prose: no flags, no backticks, no shell prompt
        assert!(
            !st.why.contains("--") && !st.why.contains('`') && !st.why.contains("$ "),
            "{}: why names a command line: {:?}",
            e.id,
            st.why
        );
        assert!(st.why.ends_with('.'), "{}: why is a sentence", e.id);
    }
    // the check itself catches what it is for
    assert!(host_text("run k2 doc cat").is_some());
    assert!(host_text("K2 undo").is_some());
    assert!(host_text("k2doc").is_some());
    assert!(host_text("$ cat").is_some());
    assert_eq!(host_text("the K2-agnostic kit's mpc-k2x"), None);
}

/// K-PAR-10: an argument `{key}` is a plain identifier, and names a detail the refusal
/// carries (its message template names it); any other argument has no braces.
#[test]
fn step_placeholders_are_plain_detail_keys() {
    let mut seen = 0;
    for e in entries() {
        let Some(st) = e.step else { continue };
        for a in st.args {
            match a.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
                Some(k) => {
                    seen += 1;
                    let mut cs = k.chars();
                    assert!(
                        cs.next()
                            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
                            && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                        "{}: placeholder {a:?} is not a plain identifier",
                        e.id
                    );
                    assert!(
                        e.message.contains(&format!("{{{k}}}")),
                        "{}: {a:?} is no detail of its message",
                        e.id
                    );
                }
                None => assert!(
                    !a.contains('{') && !a.contains('}'),
                    "{}: arg {a:?} has a stray brace",
                    e.id
                ),
            }
        }
    }
    assert!(seen >= 1, "no placeholder: the check is vacuous");
}

/// K-PAR-10 (additive this release): the report's JSON carries `next_step` right after
/// `next` when the entry has one, and leaves it out when not; the same on every surface;
/// `next` keeps its prose.
#[test]
fn next_step_is_in_the_json_after_next() {
    let r = SessionError::Stale {
        expected: 12,
        actual: 14,
    }
    .report();
    assert_eq!(r.next(), super::ids::STALE.next);
    let j = r.to_json();
    let next = format!("\"next\":{},", json_str(r.next()));
    let at = j.find(&next).expect("next") + next.len();
    assert!(
        j[at..].starts_with(r#""next_step":{"verb":"cat","args":[],"why":""#),
        "{j}"
    );
    let st = r.next_step().expect("stale has a step");
    assert_eq!(Some(st), super::ids::STALE.step);
    let o = r.clone().with_next("app", "Reload.");
    assert_eq!(o.next_step(), r.next_step());
    assert!(o.to_json_for("app").contains(&st.to_value().to_json()));
    let c = super::ids::CLAIM_CONTENDED
        .step
        .unwrap()
        .to_value()
        .to_json();
    assert!(c.contains(r#""args":["--id","{claim}"]"#), "{c}");
    // nothing to run: no `next_step` at all, `next` then `rule`
    let w = ErrorReport::new(&super::ids::FORBIDDEN_READ, "refused: needs Read rights");
    assert_eq!(w.next_step(), None);
    let wj = w.to_json();
    assert!(!wj.contains("next_step"), "{wj}");
    assert!(
        wj.contains(&format!(
            "\"next\":{},\"rule\":",
            json_str(super::ids::FORBIDDEN_READ.next)
        )),
        "{wj}"
    );
    assert!(!explain(&super::ids::FORBIDDEN_READ).contains("step:"));
    assert!(!explain_json(&super::ids::FORBIDDEN_READ).contains("next_step"));
    assert!(explain(&super::ids::STALE).contains("\nstep:     cat; why: "));
}
