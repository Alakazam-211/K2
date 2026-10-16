//! Slice B tests: export / import / settle of a clone's Threads. Shared
//! in-memory test DB + the per-process temp redb (never `~/.k2`). Every
//! test uses fresh ids, so they run in parallel with the rest of the crate.

use super::*;
use crate::overlay::{self, doc::OverlayDoc, PostAuthor};
use rusqlite::params;

fn dbh() -> std::sync::Arc<parking_lot::ReentrantMutex<Connection>> {
    crate::db::init_for_tests()
}

fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A project with a unique handle (`<prefix>-<8 hex>`). Returns (id, handle).
fn seed_project(conn: &Connection, prefix: &str) -> (String, String) {
    let id = uid();
    let handle = format!("{prefix}-{}", &id[..8]);
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, format!("/tmp/ct-{handle}")],
    )
    .expect("seed project");
    (id, handle)
}

fn pin(conn: &Connection, project_id: &str, session_id: &str) {
    crate::db::schema::WorkspaceSession::upsert(conn, &uid(), project_id, None, Some(session_id), "claude", "system", "stopped")
        .expect("pin");
}

fn opts(clone_id: &str) -> ExportOptions {
    ExportOptions::new(clone_id, "src.k2.dev", ThreadsChoice::All)
}

fn thread_docs(conv: &str) -> Vec<OverlayDoc> {
    overlay::read_thread(conv, 0).expect("read thread").into_iter().map(|i| i.doc).collect()
}

/// A source workspace with a pinned Chat holding: an agent line, an owner
/// post, a Connect user's post, a guest post, an author-less compose post
/// (pre-author era), a pending choice card, an answered choice card, a
/// pending secret card and a set secret card; plus Chatter. Returns
/// (project id, handle, pin).
fn seed_source(conn: &Connection) -> (String, String, String) {
    let (pid, handle) = seed_project(conn, "ct-src");
    let p = uid();
    pin(conn, &pid, &p);
    let to = handle.as_str();
    overlay::post_thread(conn, &p, &pid, &handle, "owner", "agent says hi", "thread").unwrap();
    overlay::post_thread_with_origin(conn, &p, &pid, "Rosson", to, "owner post", "compose", None, Some(PostAuthor::owner()))
        .unwrap();
    overlay::post_thread_with_origin(conn, &p, &pid, "alice", to, "alice post", "compose", None, Some(PostAuthor::user("alice")))
        .unwrap();
    overlay::post_thread_with_origin(conn, &p, &pid, "bob", to, "guest post", "thread", None, Some(PostAuthor::guest("bob")))
        .unwrap();
    overlay::post_thread(conn, &p, &pid, "Rosson", to, "old compose post", "compose").unwrap();
    overlay::post_choice(conn, &p, &pid, &handle, to, "Ship it?", vec!["Go".into(), "Wait".into()], false, Some(now_secs() + 3600))
        .unwrap();
    let (answered, _) =
        overlay::post_choice(conn, &p, &pid, &handle, to, "Lunch?", vec!["Yes".into()], false, None).unwrap();
    overlay::answer_card(&p, &pid, &answered.id, Some("Yes"), None, Some(PostAuthor::owner_named("Rosson"))).unwrap();
    overlay::post_secret(conn, &p, &pid, &handle, to, "STRIPE_KEY", Some("need it"), Default::default(), Some(now_secs() + 3600))
        .unwrap();
    let (set, _) =
        overlay::post_secret(conn, &p, &pid, &handle, to, "DB_URL", None, Default::default(), None).unwrap();
    overlay::mark_secret(&p, &set.id, "set", Some(format!("k2v://ws/{handle}/DB_URL")), "ok".into(), None).unwrap();
    // Chatter on the same conversation: never exported.
    overlay::record_chatter(conn, &p, &pid, None, "peer", &handle, "chatter PLANTED-CHATTER-VALUE", "msg", "accepted")
        .unwrap();
    (pid, handle, p)
}

#[test]
fn threads_choice_wire_forms() {
    let p = |s: &str| serde_json::from_str::<ThreadsChoice>(s);
    assert_eq!(p(r#""all""#).unwrap(), ThreadsChoice::All);
    assert_eq!(p(r#""none""#).unwrap(), ThreadsChoice::None);
    assert_eq!(p("true").unwrap(), ThreadsChoice::All);
    assert_eq!(p("false").unwrap(), ThreadsChoice::None);
    assert_eq!(p(r#"{"sinceDays":30}"#).unwrap(), ThreadsChoice::SinceDays(30));
    assert!(p(r#"{"sinceDays":0}"#).is_err());
    assert!(p(r#""some""#).is_err());
    assert!(p(r#"{"sinceDays":3,"x":1}"#).is_err());
    assert_eq!(serde_json::to_string(&ThreadsChoice::SinceDays(9)).unwrap(), r#"{"sinceDays":9}"#);
    assert_eq!(ThreadsChoice::default(), ThreadsChoice::All, "Bring the Thread is on by default (D3)");
}

#[test]
fn export_is_thread_only_and_never_carries_chatter_inject_or_secret_values() {
    let db = dbh();
    let conn = db.lock();
    let (pid, handle, p) = seed_source(&conn);
    // A Thread line with an inject payload (dropped on export).
    let mut d = OverlayDoc::text(uid(), handle.clone(), handle.clone(), "with inject".into(), "thread");
    d.inject = Some("PLANTED-INJECT-VALUE".into());
    let seq = catalog::next_thread_seq(&conn, &p, &pid).unwrap();
    store::commit_write(&d, Some((&p, seq)), &[], None).unwrap();
    // A collected Grok Bot line in the same catalog project never travels.
    let gb = format!("gb:ct-{}", &uid()[..8]);
    let seq = catalog::next_thread_seq(&conn, &gb, &pid).unwrap();
    let gbd = OverlayDoc::text(uid(), "bot".into(), "x".into(), "PLANTED-GB-LINE".into(), "thread");
    store::commit_write(&gbd, Some((&gb, seq)), &[], None).unwrap();

    let ex = export_project_threads(&conn, &pid, &opts("clone-x1")).unwrap();
    let json = String::from_utf8(ex.to_json().unwrap()).unwrap();
    assert_eq!(ex.format, EXPORT_FORMAT);
    assert_eq!(ex.source_handle, handle);
    assert_eq!(ex.conversations.len(), 1, "the gb: conversation is not exported: {:?}", ex.conversations);
    let c = &ex.conversations[0];
    assert_eq!((c.role.as_str(), c.source_conversation_id.as_str()), (ROLE_CANONICAL, p.as_str()));
    assert_eq!(c.harness.as_deref(), Some("claude"));
    assert_eq!(c.docs.len(), 10, "every Thread line, no Chatter");
    for planted in ["PLANTED-CHATTER-VALUE", "PLANTED-INJECT-VALUE", "PLANTED-GB-LINE", "\"inject\""] {
        assert!(!json.contains(planted), "export carries {planted}: {json}");
    }
    assert!(c.docs.iter().all(|d| d.doc.kind != "chatter" && d.doc.inject.is_none()));
    // Secret cards carry a name and a reference, never a value field.
    let secrets: Vec<_> = c.docs.iter().filter_map(|d| d.doc.secret.as_ref()).collect();
    assert_eq!(secrets.len(), 2);
    for s in &secrets {
        let v = serde_json::to_value(s).unwrap();
        assert!(v.get("value").is_none() && v.get("secret").is_none(), "{v}");
    }
    // Pending cards stay pending in the file (closed at import).
    assert!(c.docs.iter().any(|d| d.doc.is_pending_choice()));
    assert_eq!(ex.participants(), vec!["owner".to_string(), "alice".into(), "guest bob".into(), "Rosson".into()]);
    let sum = ex.summary(json.len());
    assert_eq!((sum.conversations, sum.docs, sum.dropped), (1, 10, 0));
    // The source is untouched by an export.
    assert_eq!(thread_docs(&p).len(), 10);
    assert_eq!(overlay::read_chatter(&p, 0).unwrap().len(), 1);
}

#[test]
fn export_caps_drop_oldest_first_and_since_days_cuts() {
    let db = dbh();
    let conn = db.lock();
    let (pid, handle) = seed_project(&conn, "ct-cap");
    let p = uid();
    pin(&conn, &pid, &p);
    for (i, age_days) in [40i64, 20, 10, 1].iter().enumerate() {
        let mut d = OverlayDoc::text(uid(), handle.clone(), handle.clone(), format!("line {i}"), "thread");
        d.created_at = now_secs() - age_days * 86_400;
        let seq = catalog::next_thread_seq(&conn, &p, &pid).unwrap();
        store::commit_write(&d, Some((&p, seq)), &[], None).unwrap();
    }
    let mut o = opts("clone-cap");
    o.max_docs = 2;
    let ex = export_project_threads(&conn, &pid, &o).unwrap();
    let bodies: Vec<_> = ex.conversations[0].docs.iter().map(|d| d.doc.body.clone().unwrap()).collect();
    assert_eq!(bodies, vec!["line 2", "line 3"], "the two newest stay");
    assert_eq!(ex.limits.dropped, 2);
    let since = ExportOptions::new("clone-since", "src.k2.dev", ThreadsChoice::SinceDays(15));
    let ex = export_project_threads(&conn, &pid, &since).unwrap();
    assert_eq!(ex.conversations[0].docs.len(), 2, "only the last 15 days");
    assert_eq!(ex.limits.since_days, Some(15));
}

#[test]
fn import_rewrites_authors_closes_cards_adds_divider_and_continues_live() {
    let db = dbh();
    let conn = db.lock();
    let (src_pid, src_handle, _p) = seed_source(&conn);
    let ex = export_project_threads(&conn, &src_pid, &opts("clone-r1")).unwrap();
    let ex = ThreadsExport::parse(&ex.to_json().unwrap()).unwrap();

    let (dst_pid, dst_handle) = seed_project(&conn, "ct-dst");
    let dst_pin = uid();
    pin(&conn, &dst_pid, &dst_pin);
    let ctx = ImportContext {
        project_id: dst_pid.clone(),
        copied_secret_names: ["DB_URL".to_string()].into_iter().collect(),
        ..Default::default()
    };
    let rep = import_threads(&conn, &ex, &ctx).unwrap();
    assert_eq!((rep.docs_imported, rep.docs_skipped, rep.pending, rep.detached), (9, 0, false, 0));
    assert_eq!(rep.conversations[0].local_conversation_id, dst_pin);
    assert_eq!(rep.conversations[0].role, ROLE_CANONICAL);

    let docs = thread_docs(&dst_pin);
    assert_eq!(docs.len(), 10, "divider + 9 lines");
    // Divider first: a plain text line from K2 (CL8).
    let div = &docs[0];
    assert_eq!((div.kind.as_str(), div.from.as_str(), div.via.as_deref()), ("text", "K2", Some("import")));
    assert!(div.author.is_none());
    assert!(div.imported.as_ref().unwrap().divider);
    let body = div.body.as_deref().unwrap();
    assert!(body.starts_with(&format!("Imported from `{src_handle}` on src.k2.dev, ")), "{body}");
    assert!(body.ends_with("(9 messages)"), "{body}");
    assert!(docs[1..].iter().all(|d| d.imported.as_ref().is_some_and(|m| !m.divider && m.clone_id == "clone-r1")));
    assert!(docs.iter().all(|d| d.created_at >= div.created_at), "divider sorts first");

    let by_body = |b: &str| docs.iter().find(|d| d.body.as_deref() == Some(b)).unwrap_or_else(|| panic!("{b}"));
    // Agent line: its address follows the clone's handle.
    let a = by_body("agent says hi");
    assert_eq!(a.from, dst_handle);
    assert_eq!(a.imported.as_ref().unwrap().from.as_deref(), Some(src_handle.as_str()));
    // People: historical names, never the target owner's "You" (CL7).
    let owner = by_body("owner post");
    let oa = owner.author.as_ref().unwrap();
    assert_eq!((oa.kind.as_str(), oa.login.as_str(), oa.name.as_deref()), (AUTHOR_IMPORTED, "owner", Some("Rosson")));
    assert_eq!(owner.from, "Rosson");
    assert_eq!(owner.via.as_deref(), Some("import"), "out of the via=compose rule");
    assert_eq!(owner.imported.as_ref().unwrap().author, Some(PostAuthor::owner()));
    assert_eq!(owner.imported.as_ref().unwrap().via.as_deref(), Some("compose"));
    let alice = by_body("alice post");
    assert_eq!(alice.from, "alice (Connect, src.k2.dev)");
    assert_eq!(alice.author.as_ref().unwrap().kind, AUTHOR_IMPORTED);
    let guest = by_body("guest post");
    assert_eq!(guest.from, "guest bob (App)");
    assert_eq!(guest.via.as_deref(), Some("thread"), "only compose posts change via");
    let old = by_body("old compose post");
    let oa = old.author.as_ref().expect("author-less compose post is stamped");
    assert_eq!((oa.kind.as_str(), oa.login.as_str()), (AUTHOR_IMPORTED, "Rosson"));
    for d in &docs {
        assert!(
            d.author.as_ref().is_none_or(|a| a.kind == AUTHOR_IMPORTED),
            "no live author kind survives: {d:?}"
        );
        assert!(d.via.as_deref() != Some("compose"), "no via=compose survives: {d:?}");
        assert!(d.inject.is_none());
    }
    // Cards: open ones closed with a note, answered ones keep their answer.
    let ship = docs.iter().find(|d| d.choice.as_ref().is_some_and(|c| c.prompt == "Ship it?")).unwrap();
    let c = ship.choice.as_ref().unwrap();
    assert_eq!((c.status.as_str(), c.expires_at), ("voided", None));
    assert_eq!(ship.imported.as_ref().unwrap().note.as_deref(), Some(CARRIED_OVER_NOTE));
    let lunch = docs.iter().find(|d| d.choice.as_ref().is_some_and(|c| c.prompt == "Lunch?")).unwrap();
    let c = lunch.choice.as_ref().unwrap();
    assert_eq!((c.status.as_str(), c.answer.as_deref()), ("answered", Some("Yes")));
    assert_eq!(c.answered_by.as_ref().unwrap().kind, AUTHOR_IMPORTED);
    let stripe = docs.iter().find(|d| d.secret.as_ref().is_some_and(|s| s.name == "STRIPE_KEY")).unwrap();
    assert_eq!(stripe.secret.as_ref().unwrap().status, "voided");
    let dburl = docs.iter().find(|d| d.secret.as_ref().is_some_and(|s| s.name == "DB_URL")).unwrap();
    assert_eq!(
        dburl.secret.as_ref().unwrap().reference.as_deref(),
        Some(format!("k2v://ws/{dst_handle}/DB_URL").as_str()),
        "a copied secret's ref points at the clone"
    );
    // No expiry rows for imported cards: a sweep far in the future closes nothing here.
    // (Read-only look at the index: a real sweep would touch other tests.)
    let due = store::due_card_expiries(now_secs() + 400 * 86_400).unwrap();
    assert!(due.iter().all(|(_, _, conv)| conv != &dst_pin), "{due:?}");
    // Imported lines are read-only (CL31); the Thread continues live below.
    assert_eq!(
        overlay::answer_card(&dst_pin, &dst_pid, &ship.id, Some("Go"), None, None).unwrap_err(),
        IMPORTED_READ_ONLY
    );
    assert_eq!(overlay::void_card(&dst_pin, &dst_pid, &ship.id, None).unwrap_err(), IMPORTED_READ_ONLY);
    assert!(is_read_only(&dst_pin, Some(&ship.id)).unwrap());
    assert!(!is_read_only(&dst_pin, None).unwrap(), "the pinned Thread itself takes new posts");
    let (live, _) = overlay::post_thread(&conn, &dst_pin, &dst_pid, &dst_handle, "x", "live after import", "thread").unwrap();
    assert_eq!(live.seq, 11, "live posts continue after the imported history");
    let (q, _) = overlay::post_choice(&conn, &dst_pin, &dst_pid, &dst_handle, "x", "New?", vec!["A".into()], false, None).unwrap();
    overlay::answer_card(&dst_pin, &dst_pid, &q.id, Some("A"), None, None).expect("new cards still work");
    // The source keeps everything as it was.
    assert_eq!(thread_docs(&_p).len(), 9);
}

#[test]
fn import_is_idempotent() {
    let db = dbh();
    let conn = db.lock();
    let (src_pid, _, _) = seed_source(&conn);
    let ex = export_project_threads(&conn, &src_pid, &opts("clone-idem")).unwrap();
    let (dst_pid, _) = seed_project(&conn, "ct-idem");
    let dst_pin = uid();
    pin(&conn, &dst_pid, &dst_pin);
    let ctx = ImportContext { project_id: dst_pid.clone(), ..Default::default() };
    let first = import_threads(&conn, &ex, &ctx).unwrap();
    let n = thread_docs(&dst_pin).len();
    let again = import_threads(&conn, &ex, &ctx).unwrap();
    assert_eq!((first.docs_imported, again.docs_imported, again.docs_skipped), (9, 0, 9));
    assert_eq!(thread_docs(&dst_pin).len(), n, "a re-run adds nothing (no second divider)");
    let rows = list_imports(&conn, &dst_pid).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].doc_count, rows[0].skipped_count), (9, 9));
}

#[test]
fn pending_import_waits_for_the_pin_then_settles_and_reruns_land_on_the_pin() {
    let db = dbh();
    let conn = db.lock();
    let (src_pid, _, _) = seed_source(&conn);
    let ex = export_project_threads(&conn, &src_pid, &opts("clone-pend")).unwrap();
    // Non-Claude clone: no pin after unpack.
    let (dst_pid, dst_handle) = seed_project(&conn, "ct-pend");
    let ctx = ImportContext { project_id: dst_pid.clone(), ..Default::default() };
    let rep = import_threads(&conn, &ex, &ctx).unwrap();
    assert!(rep.pending);
    let key = rep.conversations[0].local_conversation_id.clone();
    assert!(is_import_key(&key), "{key}");
    assert_eq!(pending_canonical_for(&conn, &dst_pid).unwrap().as_deref(), Some(key.as_str()));
    assert!(is_read_only(&key, None).unwrap(), "pending history is read-only");
    assert_eq!(
        crate::workspace_session_handles::project_id_for_session_id(&conn, &key).unwrap().as_deref(),
        Some(dst_pid.as_str()),
        "an import key resolves to its workspace (CL30)"
    );
    // No pin yet: settling is a no-op.
    assert!(settle_pending(&conn, &dst_pid).unwrap().is_empty());
    // The clone's first chat gets a pin, and a live line raced in under it.
    let new_pin = uid();
    pin(&conn, &dst_pid, &new_pin);
    overlay::post_thread(&conn, &new_pin, &dst_pid, &dst_handle, "x", "first live line", "thread").unwrap();
    let settled = settle_all_pending(&conn).unwrap();
    let mine: Vec<_> = settled.iter().filter(|s| s.project_id == dst_pid).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!((mine[0].from.as_str(), mine[0].pin.as_str()), (key.as_str(), new_pin.as_str()));
    let docs = thread_docs(&new_pin);
    assert_eq!(docs.len(), 11, "divider + 9 imported + the live line");
    assert!(docs[0].imported.as_ref().unwrap().divider, "divider first after the merge");
    assert_eq!(docs[10].body.as_deref(), Some("first live line"), "live line after the history");
    assert!(thread_docs(&key).is_empty(), "the pending key is empty after the move");
    assert_eq!(pending_canonical_for(&conn, &dst_pid).unwrap(), None);
    let rows = list_imports(&conn, &dst_pid).unwrap();
    assert_eq!((rows[0].role.as_str(), rows[0].local_conversation_id.as_str()), (ROLE_CANONICAL, new_pin.as_str()));
    assert!(rows[0].settled_at.is_some());
    // A re-run of the same export lands on the pin and adds nothing.
    let again = import_threads(&conn, &ex, &ctx).unwrap();
    assert_eq!((again.docs_imported, again.pending), (0, false));
    assert_eq!(thread_docs(&new_pin).len(), 11);
}

#[test]
fn same_box_clone_never_attaches_to_the_source_conversation() {
    let db = dbh();
    let conn = db.lock();
    let (src_pid, _, src_pin) = seed_source(&conn);
    let ex = export_project_threads(&conn, &src_pid, &opts("clone-same")).unwrap();
    // Same daemon: the clone re-pins the SAME session id (CL11).
    let (dst_pid, _) = seed_project(&conn, "ct-same");
    pin(&conn, &dst_pid, &src_pin);
    let rep = import_threads(&conn, &ex, &ImportContext { project_id: dst_pid.clone(), ..Default::default() }).unwrap();
    assert!(rep.pending, "the shared pin belongs to the source: import waits");
    assert_ne!(rep.conversations[0].local_conversation_id, src_pin);
    assert!(settle_pending(&conn, &dst_pid).unwrap().is_empty(), "never settles onto another workspace's Thread");
    assert_eq!(thread_docs(&src_pin).len(), 9, "source Thread untouched");
    assert_eq!(catalog::get(&conn, &src_pin).unwrap().unwrap().0, src_pid, "source catalog row untouched");
}

#[test]
fn sidecar_keeps_its_address_only_when_its_transcript_travelled() {
    let db = dbh();
    let conn = db.lock();
    let (src_pid, handle) = seed_project(&conn, "ct-side");
    pin(&conn, &src_pid, &uid());
    let (with_file, without_file) = (uid(), uid());
    for key in [&with_file, &without_file] {
        crate::workspace_session_handles::allocate_ordinal(&conn, &src_pid, key).unwrap();
        overlay::post_thread(&conn, key, &src_pid, &format!("{handle}/x"), &handle, "sidecar line", "thread").unwrap();
    }
    crate::workspace_session_handles::retire_slug(&conn, &src_pid, "old-reviewer", &with_file).unwrap();
    let ex = export_project_threads(&conn, &src_pid, &opts("clone-side")).unwrap();
    let side: Vec<_> = ex.conversations.iter().filter(|c| c.role == ROLE_SIDECAR).collect();
    assert_eq!(side.len(), 2);
    let src_ord = side.iter().find(|c| c.source_conversation_id == with_file).unwrap().ordinal;
    assert!(src_ord.is_some());
    // Another box: the destination has never seen these keys. (One test DB
    // here, so move the source's copies out of the way first.)
    for key in [&with_file, &without_file] {
        overlay::move_conversation(&conn, key, &format!("ct-gone-{key}")).unwrap().expect("moved");
    }

    let (dst_pid, _) = seed_project(&conn, "ct-side-dst");
    let ctx = ImportContext {
        project_id: dst_pid.clone(),
        travelled_sessions: [with_file.clone()].into_iter().collect(),
        ..Default::default()
    };
    let rep = import_threads(&conn, &ex, &ctx).unwrap();
    assert_eq!(rep.detached, 1);
    let kept = rep.conversations.iter().find(|c| c.source_conversation_id == with_file).unwrap();
    assert_eq!((kept.role.as_str(), kept.local_conversation_id.as_str()), (ROLE_SIDECAR, with_file.as_str()));
    let row = crate::workspace_session_handles::get(&conn, &dst_pid, &with_file).unwrap().expect("handle row");
    assert_eq!(Some(row.ordinal), src_ord, "the source ordinal (free here) is kept");
    assert_eq!(
        crate::workspace_session_handles::alias_owner(&conn, &dst_pid, "old-reviewer").unwrap().as_deref(),
        Some(with_file.as_str()),
        "older names come along"
    );
    assert_eq!(thread_docs(&with_file).len(), 2, "divider + the sidecar line, under its own key");
    let det = rep.conversations.iter().find(|c| c.source_conversation_id == without_file).unwrap();
    assert_eq!(det.role, ROLE_DETACHED);
    assert!(is_import_key(&det.local_conversation_id));
    assert!(is_read_only(&det.local_conversation_id, None).unwrap(), "detached history is read-only");
    assert_eq!(thread_docs(&det.local_conversation_id).len(), 2);
    assert!(crate::workspace_session_handles::get(&conn, &dst_pid, &without_file).unwrap().is_none());
}
