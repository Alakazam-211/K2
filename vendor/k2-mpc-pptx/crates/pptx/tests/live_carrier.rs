//! The carrier through the kit (K1): the bytes a live deck is spliced into are the ones the kit
//! last read from the file or wrote to it, kept in the kit's store, so they survive a restart.
//!
//! - F5 through the kit: an untouched live deck writes back byte for byte, by the kit's
//!   write-back, before and after a restart;
//! - after a restart, a verb writes back by splicing into the kit's stored carrier, with no
//!   read of the file (the doc opened from the store alone renders it);
//! - `file_at` of an earlier version is that version's deck, byte for byte, after later edits,
//!   an app's save that changed opaque parts, and a restart;
//! - an app's save, then a restart and a verb: the verb is spliced into the saved bytes;
//! - the pack contract's byte-exact row, the kit's judge (`test_support::carrier_judge`) over
//!   the whole corpus: the hook, the file opened live, and the same after a restart.
//!
//! A restart is a new manager on the same stores **and a new pack**: nothing the old process
//! held in memory survives. Each row returns `Err("<row>: ...")`.

mod common;

use common::live::{app_save, args, fx, Fx};
use common::{deflated, entries, entry, limits};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::{ConnId, DiskState, Session};
use k2_mpc_pptx::live::model;
use k2_mpc_pptx::live::splice::encode_from;
use k2_mpc_pptx::live::SHAPES;
use k2_mpc_pptx::testgen::{self, Shape};

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

/// The pack's own splice of `content` into `carrier`: what the kit's write-back must write
/// when `carrier` is the carrier it hands the pack.
fn spliced_into(carrier: &[u8], content: &[(String, Neutral)]) -> Result<Vec<u8>, String> {
    encode_from(carrier, content, &[], &limits()).map_err(|e| e.0)
}

/// An agent's verb with the version of the read it just made as its base; exit 0 required.
fn verb(row: &str, s: &Session, c: ConnId, a: &[&str]) -> Result<(), String> {
    let v = s
        .run_tool(c, SHAPES, &args(&["slide", "list", "--limit", "1"]), b"")
        .map_err(|e| format!("{row}: the read before {a:?}: {e}"))?
        .version;
    let mut all = args(a);
    all.push("--base".into());
    all.push(v.to_string());
    let o = s
        .run_tool(c, SHAPES, &all, b"")
        .map_err(|e| format!("{row}: {a:?}: {e}"))?;
    need(o.exit == 0, row, || {
        format!(
            "{a:?} exited {}: {}",
            o.exit,
            String::from_utf8_lossy(&o.stdout)
        )
    })
}

/// The live content as `c` reads it.
fn content(row: &str, s: &Session, c: ConnId) -> Result<Vec<(String, Neutral)>, String> {
    Ok(s.read(c).map_err(|e| format!("{row}: read: {e}"))?.content)
}

/// `bytes` as an app saves it after changing only an opaque part: `docProps/app.xml` names
/// another application. Re-zipped (stored), as an app writes its own package.
fn opaque_resave(bytes: &[u8]) -> Vec<u8> {
    let parts: Vec<(String, Vec<u8>)> = entries(bytes)
        .into_iter()
        .map(|(n, c)| {
            if n == "docProps/app.xml" {
                let t = String::from_utf8(c).expect("utf-8");
                assert!(
                    t.contains("k2-mpc-pptx test generator"),
                    "the fixture's app.xml"
                );
                let t = t.replace("k2-mpc-pptx test generator", "Another app");
                (n, t.into_bytes())
            } else {
                (n, c)
            }
        })
        .collect();
    testgen::stored_zip(&parts)
}

/// Open the file live (after a restart, the reopen is named in the error).
fn reopen(row: &str, f: &Fx, when: &str) -> Result<std::sync::Arc<Session>, String> {
    f.try_open()
        .map_err(|e| format!("{row}: {when}, the deck didn't open live again: {e}"))
}

// ---------- F5 through the kit ----------

/// F5 through the kit: every fixture (stored and deflated) opened live renders as its file
/// byte for byte and the write-back leaves the file; on the sample deck, a change put back is
/// written back as the file byte for byte, before and after a restart.
fn row_f5_through_the_kits_write_back() -> Result<(), String> {
    const ROW: &str = "row_f5_through_the_kits_write_back";
    for (name, spec) in common::specs() {
        let parts = testgen::parts(&spec);
        for (layout, bytes) in [
            ("stored", testgen::stored_zip(&parts)),
            ("deflated", deflated(&parts)),
        ] {
            let f = fx("k1-f5", |_| {});
            f.write(&bytes);
            let s = f
                .try_open()
                .map_err(|e| format!("{ROW}: {name}/{layout} didn't open live: {e}"))?;
            let p = f.person(&s);
            let now = s
                .file_now(p)
                .map_err(|e| format!("{ROW}: {name}/{layout}: file_now: {e}"))?;
            need(now.bytes == bytes, ROW, || {
                format!("{name}/{layout}: the rendered file isn't the file")
            })?;
            s.flush(p)
                .map_err(|e| format!("{ROW}: {name}/{layout}: flush: {e}"))?;
            need(f.bytes() == bytes, ROW, || {
                format!("{name}/{layout}: the write-back changed the file")
            })?;
        }
    }
    let bytes = testgen::deck(&testgen::sample());
    let mut f = fx("k1-f5-back", |_| {});
    f.write(&bytes);
    let put_back = |f: &Fx, s: &Session, when: &str| -> Result<(), String> {
        let a = f.agent(s);
        verb(
            ROW,
            s,
            a,
            &["shape", "move", "@sld:258/sp:4", "--x", "900000"],
        )?;
        verb(
            ROW,
            s,
            a,
            &["shape", "move", "@sld:258/sp:4", "--x", "838200"],
        )?;
        let out = s
            .flush(a)
            .map_err(|e| format!("{ROW}: {when}, a change put back: the write-back: {e}"))?;
        need(matches!(out.disk, Some(DiskState::Clean)), ROW, || {
            format!(
                "{when}, a change put back: the file isn't clean: {:?}",
                out.disk
            )
        })?;
        need(f.bytes() == bytes, ROW, || {
            format!("{when}, a change put back: the file isn't its bytes")
        })?;
        let now = s
            .file_now(a)
            .map_err(|e| format!("{ROW}: {when}, file_now: {e}"))?;
        need(now.bytes == bytes, ROW, || {
            format!("{when}, the rendered file isn't the file")
        })
    };
    let s = f.open();
    put_back(&f, &s, "before a restart")?;
    drop(s);
    f.restart();
    let s = reopen(ROW, &f, "after a restart")?;
    put_back(&f, &s, "after a restart")
}

#[test]
fn f5_through_the_kits_write_back() {
    row_f5_through_the_kits_write_back().unwrap();
}

// ---------- a restart, then a verb ----------

/// After a restart a verb is written back by splicing into the kit's stored carrier (its last
/// write), with no read of the file: opened from the store alone the doc renders as that
/// splice, and opened with its file the write-back writes it.
fn row_restart_then_a_verb_writes_back() -> Result<(), String> {
    const ROW: &str = "row_restart_then_a_verb_writes_back";
    let bytes = testgen::deck(&testgen::sample());
    let mut f = fx("k1-restart", |_| {});
    f.write(&bytes);
    let s = f.open();
    let a = f.agent(&s);
    verb(
        ROW,
        &s,
        a,
        &["shape", "move", "@sld:258/sp:4", "--x", "1000000"],
    )?;
    let out = s
        .flush(a)
        .map_err(|e| format!("{ROW}: control: the first write-back: {e}"))?;
    need(out.written, ROW, || {
        "control: the kit wrote the file".into()
    })?;
    let w1 = f.bytes();
    need(w1 != bytes, ROW, || {
        "control: the write changed the file".into()
    })?;
    drop(s);
    f.restart();
    // from the store alone: the file isn't read
    let s = f
        .open_without_file()
        .map_err(|e| format!("{ROW}: after a restart, opened without its file: {e}"))?;
    let a = f.agent(&s);
    verb(
        ROW,
        &s,
        a,
        &["shape", "move", "@sld:258/sp:4", "--y", "2000000"],
    )?;
    let live = content(ROW, &s, a)?;
    let want = spliced_into(&w1, &live).map_err(|e| format!("{ROW}: the oracle: {e}"))?;
    let got = s
        .file_now(a)
        .map_err(|e| format!("{ROW}: after a restart, without the file, file_now: {e}"))?;
    need(got.bytes == want, ROW, || {
        "after a restart, without the file: the rendering isn't the verb spliced into the kit's \
         last write"
            .into()
    })?;
    drop(s);
    f.restart();
    // with its file: the write-back
    let s = reopen(ROW, &f, "after a second restart")?;
    let a = f.agent(&s);
    verb(
        ROW,
        &s,
        a,
        &["shape", "move", "@sld:258/sp:4", "--w", "2500000"],
    )?;
    let live = content(ROW, &s, a)?;
    let want = spliced_into(&w1, &live).map_err(|e| format!("{ROW}: the oracle: {e}"))?;
    let out = s
        .flush(a)
        .map_err(|e| format!("{ROW}: after a restart, the write-back: {e}"))?;
    need(out.written, ROW, || {
        "after a restart, the write-back didn't write".into()
    })?;
    need(f.bytes() == want, ROW, || {
        "after a restart, the file written isn't the verbs spliced into the kit's last write".into()
    })?;
    let back = model::read(&f.bytes(), &limits())
        .map_err(|d| format!("{ROW}: the file written doesn't read: {d}"))?;
    let g = back.slides[2]
        .shapes
        .iter()
        .find(|x| x.fid == 4)
        .and_then(|x| x.geom)
        .ok_or_else(|| format!("{ROW}: @sld:258/sp:4 has no box"))?;
    need(
        (g.x, g.y, g.w) == (1_000_000, 2_000_000, 2_500_000),
        ROW,
        || format!("@sld:258/sp:4's box {g:?}"),
    )
}

#[test]
fn restart_then_a_verb_writes_back() {
    row_restart_then_a_verb_writes_back().unwrap();
}

// ---------- file_at ----------

/// `file_at` of an earlier version is that version's deck byte for byte, after later edits,
/// an app's save that changed an opaque part, and a restart: the import's version is the
/// file opened, a version named after the kit's first write is that write.
fn row_file_at_an_earlier_version_byte_for_byte() -> Result<(), String> {
    const ROW: &str = "row_file_at_an_earlier_version_byte_for_byte";
    let bytes = testgen::deck(&testgen::sample());
    let mut f = fx("k1-file-at", |_| {});
    f.write(&bytes);
    let s = f.open();
    let a = f.agent(&s);
    let first = s
        .versions(a)
        .map_err(|e| format!("{ROW}: versions: {e}"))?
        .first()
        .map(|v| v.id)
        .ok_or_else(|| format!("{ROW}: control: the import cut no version"))?;
    verb(
        ROW,
        &s,
        a,
        &["shape", "move", "@sld:258/sp:4", "--x", "1000000"],
    )?;
    let out = s
        .flush(a)
        .map_err(|e| format!("{ROW}: control: the first write-back: {e}"))?;
    need(out.written, ROW, || {
        "control: the kit wrote the file".into()
    })?;
    let w1 = f.bytes();
    let v = s
        .name_version(a, "after the first move", None)
        .map_err(|e| format!("{ROW}: naming the version: {e}"))?;
    verb(
        ROW,
        &s,
        a,
        &["shape", "move", "@sld:258/sp:4", "--y", "2000000"],
    )?;
    verb(
        ROW,
        &s,
        a,
        &["shape", "text", "@sld:256/sp:2", "--text", "Later"],
    )?;
    let out = s
        .flush(a)
        .map_err(|e| format!("{ROW}: control: the second write-back: {e}"))?;
    need(out.written, ROW, || {
        "control: the kit's second write".into()
    })?;
    // an app's save that changes an opaque part
    f.advance(1_000);
    app_save(&f.path(), &opaque_resave(&f.bytes()));
    s.file_changed()
        .map_err(|e| format!("{ROW}: control: the app's save: {e}"))?;
    drop(s);
    f.restart();
    let s = reopen(ROW, &f, "after a restart")?;
    let a = f.agent(&s);
    let got = s
        .file_at(a, v)
        .map_err(|e| format!("{ROW}: file_at of the named version: {e}"))?;
    need(got.bytes == w1, ROW, || {
        "file_at of the named version isn't the kit's write then".into()
    })?;
    let got = s
        .file_at(a, first)
        .map_err(|e| format!("{ROW}: file_at of the import's version: {e}"))?;
    need(got.bytes == bytes, ROW, || {
        "file_at of the import's version isn't the file opened".into()
    })
}

#[test]
fn file_at_an_earlier_version_byte_for_byte() {
    row_file_at_an_earlier_version_byte_for_byte().unwrap();
}

// ---------- an outside save, then a verb ----------

/// An app's save (a new text box, and an opaque part changed) taken in; after a restart a verb
/// is spliced into the saved bytes: the file written is the save with the verb spliced in, and
/// the app's opaque part is kept.
fn row_outside_save_then_a_verb_splices_into_the_save() -> Result<(), String> {
    const ROW: &str = "row_outside_save_then_a_verb_splices_into_the_save";
    let spec = testgen::sample();
    let mut f = fx("k1-save", |_| {});
    f.write(&testgen::deck(&spec));
    let s = f.open();
    let mut saved = spec.clone();
    saved.slides[0]
        .shapes
        .push(Shape::text_box(5, (100, 100, 1000, 1000), "From the app"));
    let save = opaque_resave(&testgen::deck(&saved));
    f.advance(1_000);
    app_save(&f.path(), &save);
    s.file_changed()
        .map_err(|e| format!("{ROW}: control: the app's save: {e}"))?;
    drop(s);
    f.restart();
    let s = reopen(ROW, &f, "after a restart")?;
    let a = f.agent(&s);
    verb(
        ROW,
        &s,
        a,
        &["shape", "move", "@sld:256/sp:5", "--x", "5000"],
    )?;
    let live = content(ROW, &s, a)?;
    let want = spliced_into(&save, &live).map_err(|e| format!("{ROW}: the oracle: {e}"))?;
    let out = s
        .flush(a)
        .map_err(|e| format!("{ROW}: after the save and a restart, the write-back: {e}"))?;
    need(out.written, ROW, || "the write-back didn't write".into())?;
    let file = f.bytes();
    need(file == want, ROW, || {
        "the file written isn't the verb spliced into the app's save".into()
    })?;
    let app = entry(&file, "docProps/app.xml").unwrap_or_default();
    need(
        String::from_utf8_lossy(&app).contains("Another app"),
        ROW,
        || "the app's opaque part wasn't kept".into(),
    )
}

#[test]
fn outside_save_then_a_verb_splices_into_the_save() {
    row_outside_save_then_a_verb_splices_into_the_save().unwrap();
}

// ---------- the pack contract's byte-exact row, by the kit's judge ----------

#[test]
fn carrier_judge_byte_exact_over_the_corpus() {
    common::splice_rows::row_carrier_judge().unwrap();
}
