//! The deck's verbs on the kit's tier-0 path, run by the kit (`BlobSession::run_tool`) on a
//! blob doc, against the file verbs ([`apply`]) on the same bytes: the legs `tests/blob_verbs.rs`
//! and the mutants share.

use std::io::Read;
use std::sync::Arc;

use k2_mpc_base::errors::Value;
use k2_mpc_base::session::{BlobSession, ConnId, Participant, ParticipantKind, RevertWhat, Stay};
use k2_mpc_base::store::{DocKey, VersionId};
use k2_mpc_base::tools::{ToolOptions, ToolOutput};
use k2_mpc_pptx::testgen::{self, LAYOUT_CONTENT};
use k2_mpc_pptx::verbs::{apply, Edit, LayoutSel, Place};
use k2_mpc_pptx::PptxPack;

use super::live::{fx, Fx};
use super::{deflated, limits, specs};

pub fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// A blob doc of `bytes` (its own path in `f`), indexed.
pub fn open(f: &Fx, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
    let s = f
        .mgr
        .open_blob(
            Arc::new(PptxPack::default()),
            &DocKey {
                space: "ws".into(),
                path: path.into(),
            },
            Some(&mut &bytes[..]),
        )
        .expect("the blob doc opens");
    if let Some(j) = s.index_job(None).expect("an index job") {
        j.run().expect("the index job runs");
    }
    s
}

pub fn join(s: &BlobSession, pid: &str, kind: ParticipantKind) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#123456", kind, "cli"),
        Stay::Held,
    )
    .expect("joins")
    .conn
}

/// A call with its result as data.
pub fn call_with(s: &BlobSession, c: ConnId, a: &[String], opts: ToolOptions) -> ToolOutput {
    s.run_tool(c, a, &opts.result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

pub fn res(o: &ToolOutput) -> &Value {
    o.result().expect("a result")
}

pub fn err_id(o: &ToolOutput) -> Option<String> {
    res(o)
        .get("error")
        .and_then(|e| e.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The doc's head, as bytes.
pub fn head_bytes(s: &BlobSession, c: ConnId) -> Vec<u8> {
    let v = s.status().expect("status").version;
    let mut r = s.read_version(c, v).expect("reads head");
    let mut out = Vec::new();
    r.reader.read_to_end(&mut out).expect("reads");
    out
}

/// The doc's head version.
pub fn head_version(s: &BlobSession) -> VersionId {
    s.status().expect("status").version
}

/// A call under the caller's own lease: taken, the call made, let go (the next participant's
/// verb, or a revert, then runs with no lease held).
pub fn leased(s: &BlobSession, c: ConnId, a: &[String]) -> ToolOutput {
    s.acquire_lease(c, None).expect("takes the lease");
    let o = call_with(s, c, a, ToolOptions::new());
    s.release_lease(c).expect("lets the lease go");
    o
}

/// A revert, undo or redo with its result as data.
pub fn revert(s: &BlobSession, c: ConnId, what: RevertWhat) -> ToolOutput {
    s.run_revert(c, what, &[], &ToolOptions::new().result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

/// Head's log entry: the verb that made it and the version it took away.
pub fn head_entry(s: &BlobSession, c: ConnId) -> (Option<String>, Option<VersionId>) {
    let h = head_version(s);
    s.log(c)
        .expect("the log")
        .into_iter()
        .find(|e| e.id == h)
        .map(|e| (e.verb, e.reverts))
        .unwrap_or((None, None))
}

/// One case: the verb's words and arguments, and the file verb's edit.
pub struct Case {
    pub args: Vec<String>,
    pub edit: Edit,
    pub word: &'static str,
}

fn case(args: &[&str], edit: Edit, word: &'static str) -> Case {
    Case {
        args: argv(args),
        edit,
        word,
    }
}

/// The cases on a deck whose first slide is `s` (with shape 2), with `slides` its slide ids
/// in order and `notes` whether it has a notes master; the sample deck adds its own.
pub fn cases(s: u32, slides: &[u32], notes: bool, sample: bool) -> Vec<Case> {
    let sld = format!("@sld:{s}");
    let sp2 = format!("@sld:{s}/sp:2");
    let layout = format!("@layout:{LAYOUT_CONTENT}");
    let mut v = vec![
        case(
            &["slide", "add", "--layout", &layout],
            Edit::AddSlide {
                layout: LayoutSel::Id(LAYOUT_CONTENT),
                place: None,
            },
            "added",
        ),
        case(
            &["slide", "add", "--layout", &layout, "--first"],
            Edit::AddSlide {
                layout: LayoutSel::Id(LAYOUT_CONTENT),
                place: Some(Place::First),
            },
            "added",
        ),
        case(
            &[
                "shape", "add", &sld, "--kind", "textbox", "--at", "1,2,3,4", "--text", "t",
            ],
            Edit::AddTextBox {
                slide: s,
                at: (1, 2, 3, 4),
                text: "t".into(),
            },
            "added",
        ),
        case(
            &["shape", "text", &sp2, "--text", "t"],
            Edit::SetText {
                slide: s,
                shape: 2,
                text: "t".into(),
            },
            "set",
        ),
        case(
            &[
                "shape", "move", &sp2, "--x", "10", "--y", "20", "--w", "30", "--h", "40",
            ],
            Edit::MoveShape {
                slide: s,
                shape: 2,
                x: Some(10),
                y: Some(20),
                w: Some(30),
                h: Some(40),
            },
            "moved",
        ),
    ];
    if notes {
        v.push(case(
            &["notes", "set", &sld, "--text", "n"],
            Edit::SetNotes {
                slide: s,
                text: "n".into(),
            },
            "set",
        ));
    }
    if let (Some(first), Some(last)) = (slides.first(), slides.last()) {
        if first != last {
            let (a, b) = (format!("@sld:{first}"), format!("@sld:{last}"));
            v.push(case(
                &["slide", "move", &a, "--after", &b],
                Edit::MoveSlide {
                    slide: *first,
                    place: Place::After(*last),
                },
                "moved",
            ));
            v.push(case(
                &["slide", "move", &b, "--first"],
                Edit::MoveSlide {
                    slide: *last,
                    place: Place::First,
                },
                "moved",
            ));
        }
    }
    if sample {
        let shape = |sp: u32| format!("@sld:258/sp:{sp}");
        let del = |sp: u32| {
            case(
                &["shape", "delete", &shape(sp)],
                Edit::DeleteShape {
                    slide: 258,
                    shape: sp,
                },
                "deleted",
            )
        };
        // a picture with shapes in front of it, a group with its shapes, a connected shape
        v.extend([del(7), del(9), del(5), del(4)]);
        v.push(case(
            &["shape", "text", &shape(2), "--text", "New title"],
            Edit::SetText {
                slide: 258,
                shape: 2,
                text: "New title".into(),
            },
            "set",
        ));
        v.push(case(
            &["shape", "move", &shape(4), "--x", "100"],
            Edit::MoveShape {
                slide: 258,
                shape: 4,
                x: Some(100),
                y: None,
                w: None,
                h: None,
            },
            "moved",
        ));
        v.push(case(
            &["notes", "set", "@sld:257", "--text", "x"],
            Edit::SetNotes {
                slide: 257,
                text: "x".into(),
            },
            "set",
        ));
        v.push(case(
            &[
                "slide",
                "add",
                "--layout",
                "Title and Content",
                "--after",
                "@sld:256",
            ],
            Edit::AddSlide {
                layout: LayoutSel::Name("Title and Content".into()),
                place: Some(Place::After(256)),
            },
            "added",
        ));
        v.push(case(
            &["shape", "text", "@sld:999/sp:2", "--text", "x"],
            Edit::SetText {
                slide: 999,
                shape: 2,
                text: "x".into(),
            },
            "set",
        ));
    }
    v
}

/// One case on a fresh doc of `bytes`, by an agent holding the lease: `Ok` when the kit's
/// verb gives what the file verb gives (the same bytes, or the same refusal), with how it
/// ended (`landed`, `unchanged`, `refused`).
pub fn one(f: &Fx, n: usize, name: &str, bytes: &[u8], c: &Case) -> Result<&'static str, String> {
    let s = open(f, &format!("d{n}.pptx"), bytes);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    s.acquire_lease(bot, None)
        .expect("the agent takes the lease");
    let before = s.status().expect("status").version;
    let o = call_with(&s, bot, &c.args, ToolOptions::new());
    let what = format!("{name} {:?}", c.args);
    match apply(bytes, &c.edit, &limits()) {
        Ok(a) => {
            if let Some(id) = err_id(&o) {
                return Err(format!(
                    "{what}: the kit refused {id}: {}",
                    res(&o).to_json()
                ));
            }
            let got = head_bytes(&s, bot);
            if got != a.bytes {
                return Err(format!(
                    "{what}: the kit's bytes ({}) aren't the file verb's ({})",
                    got.len(),
                    a.bytes.len()
                ));
            }
            if a.bytes == bytes {
                let after = s.status().expect("status").version;
                return match after == before {
                    true => Ok("unchanged"),
                    false => Err(format!("{what}: no change made a version")),
                };
            }
            let word = res(&o)
                .get("changed")
                .and_then(|x| x.get("word"))
                .and_then(Value::as_str);
            if word != Some(c.word) {
                return Err(format!("{what}: the word is {word:?}, not {:?}", c.word));
            }
            Ok("landed")
        }
        Err(e) => {
            let want = match e.exit() {
                4 => "PPTX-E004",
                _ => match &e {
                    k2_mpc_pptx::verbs::VerbError::Declined(_) => "PPTX-E001",
                    _ => "PPTX-E002",
                },
            };
            let got = err_id(&o);
            if got.as_deref() != Some(want) || o.exit != e.exit() {
                return Err(format!(
                    "{what}: the file verb refused ({e}, exit {}), the kit gave {got:?} exit {}",
                    e.exit(),
                    o.exit
                ));
            }
            if s.status().expect("status").version != before {
                return Err(format!("{what}: a refusal made a version"));
            }
            Ok("refused")
        }
    }
}

/// file_verb_equals_blob_verb: every case on every fixture deck (stored and deflated): the
/// kit's verb lands with the file verb's bytes, its word in the result, or is refused as the
/// file verb is. `Err` names the first case that differs; it fails unless cases landed.
pub fn file_verb_equals_blob_verb() -> Result<(), String> {
    let f = fx("blob-verbs", |_| {});
    let (mut landed, mut unchanged, mut refused, mut n) = (0, 0, 0, 0);
    for (name, spec) in specs() {
        let Some(first) = spec.slides.first() else {
            continue;
        };
        let ids: Vec<u32> = spec.slides.iter().map(|s| s.id).collect();
        let sample = name == "sample";
        let parts = testgen::parts(&spec);
        for bytes in [testgen::stored_zip(&parts), deflated(&parts)] {
            for c in cases(first.id, &ids, spec.notes_master, sample) {
                n += 1;
                match one(&f, n, name, &bytes, &c)? {
                    "landed" => landed += 1,
                    "unchanged" => unchanged += 1,
                    _ => refused += 1,
                }
            }
        }
    }
    if landed == 0 || refused == 0 {
        return Err(format!(
            "blob verbs: {n} cases, {landed} landed, {refused} refused: not exercised"
        ));
    }
    println!(
        "blob verbs: {n} cases: {landed} landed with the file verb's bytes, {unchanged} unchanged, {refused} refused as the file verb is"
    );
    Ok(())
}
