//! project-chat-thread-v1 bites (k2-core): a new line's Thread shape and
//! rollback copy, the move of old rows (ids, seqs, times, keys and unread
//! kept; nothing marked imported), the seq check's cursor remap, the
//! reconcile after a rollback, deletes in the views, the lifetime rules
//! (project delete, the sweep, a workspace delete never), and the walkers
//! that leave project chats out.

use super::*;
use crate::notify_prefs::People;
use crate::overlay::project as kp;
use crate::project_groups::{self, AUTHOR_KIND_AGENT, AUTHOR_KIND_GUEST, AUTHOR_KIND_PERSON};
use crate::read_state::{self, Person, KIND_PROJECT};

fn group(label: &str) -> String {
    project_groups::create_group(&format!("pct-{label}-{}", uuid::Uuid::new_v4())).expect("create group").id
}

fn owner() -> Person {
    Person::of_login(&People::default(), "owner")
}

fn alice() -> Person {
    Person::of_login(&People::default(), "alice")
}

fn guest() -> Person {
    Person::app_guest("guest:pid-dana", "dana")
}

fn catalog(conv: &str) -> Option<(String, Option<String>, String, i64)> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT kind, group_id, project_id, last_thread_seq FROM overlay_conversations WHERE conversation_id = ?1",
        params![conv],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
    .expect("catalog read")
}

fn copies(gid: &str) -> Vec<(String, i64, String)> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT id, seq, author_kind FROM project_group_messages WHERE group_id = ?1 ORDER BY seq")
        .expect("copies");
    let rows = stmt.query_map(params![gid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).expect("copies");
    rows.collect::<Result<_, _>>().expect("copy rows")
}

fn last_msg_seq(gid: &str) -> i64 {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT last_msg_seq FROM project_groups WHERE id = ?1", params![gid], |r| r.get(0)).expect("head")
}

/// Put `person`'s cursor on project `gid` at exactly `seq` (the person
/// seen first, so a first sight's seeding never moves it after).
fn set_cursor(person: &str, gid: &str, seq: i64) {
    let db = crate::db::shared();
    let conn = db.lock();
    read_state::ensure_person(&conn, person).expect("person");
    conn.execute(
        "INSERT INTO read_cursors (person, kind, conv_id, last_read_seq, updated_at) VALUES (?1, 'project', ?2, ?3, 0) \
         ON CONFLICT(person, kind, conv_id) DO UPDATE SET last_read_seq = excluded.last_read_seq",
        params![person, gid, seq],
    )
    .expect("cursor");
}

fn unread(p: &Person, gid: &str) -> i64 {
    let db = crate::db::shared();
    let conn = db.lock();
    read_state::item(&conn, p, KIND_PROJECT, gid, read_state::now_secs()).expect("read item").unread
}

/// A project as the release before left it: its lines in
/// `project_group_messages` (seqs as given), `messages_in_thread` 0.
/// `rows`: (author, author_kind, body, created_at, seq, client_msg_id).
fn legacy(label: &str, rows: &[(&str, &str, &str, i64, i64, Option<&str>)]) -> (String, Vec<String>) {
    let gid = group(label);
    let db = crate::db::shared();
    let conn = db.lock();
    let mut ids = Vec::new();
    let mut head = 0;
    for (author, kind, body, at, seq, client) in rows {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO project_group_messages (id, group_id, author, body, created_at, seq, author_kind, client_msg_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, gid, author, body, at, seq, kind, client],
        )
        .expect("old row");
        ids.push(id);
        head = head.max(*seq);
    }
    conn.execute("UPDATE project_groups SET messages_in_thread = 0, last_msg_seq = ?2 WHERE id = ?1", params![gid, head])
        .expect("legacy mark");
    (gid, ids)
}

/// Bite (new lines): a line is a Thread text doc of `project:<id>` (catalog
/// kind `project`, `group_id` and `project_id` = the group id), stamped
/// from its kind, redacted, and copied to the old table under the same id
/// and seq with `last_msg_seq` = seq. The views read it back in the old
/// shape.
#[test]
fn a_new_line_lives_in_the_project_thread_with_its_rollback_copy() {
    let gid = group("shape");
    let conv = kp::conversation_id(&gid);
    assert!(in_thread(&gid), "a new project starts on the Thread");
    for p in ["owner", "alice", "guest:pid-dana"] {
        set_cursor(p, &gid, 0);
    }
    let a = project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "kickoff").expect("owner line");
    let b = project_groups::post_message_as(&gid, "scout", AUTHOR_KIND_AGENT, "ack").expect("agent line");
    let c = project_groups::post_message_as(&gid, "alice", AUTHOR_KIND_PERSON, "hi").expect("login line");
    let d = project_groups::post_message_as(&gid, &project_groups::guest_author("dana"), AUTHOR_KIND_GUEST, "from the app").expect("guest");
    assert_eq!((a.seq, b.seq, c.seq, d.seq), (1, 2, 3, 4));
    assert_eq!((a.author_kind.as_str(), b.author_kind.as_str(), c.author_kind.as_str(), d.author_kind.as_str()), ("person", "agent", "person", "guest"));
    assert_eq!(d.author, "guest:dana");
    assert_eq!(catalog(&conv), Some(("project".into(), Some(gid.clone()), gid.clone(), 4)));

    let items = crate::overlay::read_thread(&conv, 0).expect("thread");
    assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![a.id.as_str(), b.id.as_str(), c.id.as_str(), d.id.as_str()]);
    assert!(items.iter().all(|i| i.doc.via.as_deref() == Some(kp::VIA_PROJECT) && i.doc.imported.is_none()));
    assert_eq!(items[0].doc.author, Some(crate::overlay::PostAuthor::owner()));
    assert_eq!(items[1].doc.author, None, "an agent's line carries no stamp");
    assert_eq!(items[2].doc.author, Some(crate::overlay::PostAuthor::user("alice")));
    assert_eq!(items[3].doc.author, Some(crate::overlay::PostAuthor::guest("dana")));

    let copy = copies(&gid);
    assert_eq!(
        copy,
        vec![(a.id.clone(), 1, "person".into()), (b.id.clone(), 2, "agent".into()), (c.id.clone(), 3, "person".into()), (d.id.clone(), 4, "guest".into())]
    );
    assert_eq!(last_msg_seq(&gid), 4, "the old head follows, so the older release's next post takes 5");

    let page = project_groups::list_messages(&gid, None, None).expect("page");
    assert_eq!(page.messages.iter().map(|m| (m.id.as_str(), m.seq, m.author.as_str(), m.body.as_str())).collect::<Vec<_>>(), vec![
        (a.id.as_str(), 1, "owner", "kickoff"),
        (b.id.as_str(), 2, "scout", "ack"),
        (c.id.as_str(), 3, "alice", "hi"),
        (d.id.as_str(), 4, "guest:dana", "from the app"),
    ]);
    assert!(serde_json::to_value(&page.messages[0]).expect("json").get("attachments").is_none(), "a text line's wire is unchanged");
    // Own lines are never unread (the stamp), everyone else's are.
    assert_eq!(unread(&owner(), &gid), 3);
    assert_eq!(unread(&alice(), &gid), 3);
    assert_eq!(unread(&guest(), &gid), 3);
}

/// Bite (the move): an old project's lines move with their ids, seqs,
/// times and send-once ids; nothing is marked imported; every person's
/// unread count is the same before and after; a re-run adds nothing; and
/// its next line takes the next seq.
#[test]
fn the_move_keeps_ids_seqs_and_unread() {
    let (gid, ids) = legacy(
        "move",
        &[
            ("owner", "person", "one", 100, 1, Some("cm-1")),
            ("scout", "agent", "two", 101, 2, None),
            ("alice", "person", "three", 102, 3, None),
            ("guest:dana", "guest", "four", 103, 4, None),
        ],
    );
    set_cursor("owner", &gid, 0);
    set_cursor("alice", &gid, 1);
    set_cursor("guest:pid-dana", &gid, 2);
    let before = (unread(&owner(), &gid), unread(&alice(), &gid), unread(&guest(), &gid));
    assert!(!in_thread(&gid));
    assert_eq!(move_group(&gid).expect("move"), 4);
    assert!(in_thread(&gid));
    assert_eq!((unread(&owner(), &gid), unread(&alice(), &gid), unread(&guest(), &gid)), before, "unread unchanged by the move");

    let conv = kp::conversation_id(&gid);
    let items = crate::overlay::read_thread(&conv, 0).expect("thread");
    assert_eq!(items.iter().map(|i| (i.seq, i.id.clone())).collect::<Vec<_>>(), ids.iter().enumerate().map(|(n, id)| (n as i64 + 1, id.clone())).collect::<Vec<_>>());
    assert!(items.iter().all(|i| i.doc.imported.is_none()), "moved lines are ordinary lines");
    assert_eq!(items[0].doc.created_at, 100);
    assert_eq!(items[0].doc.client_msg_id.as_deref(), Some("cm-1"));
    assert_eq!(items[3].doc.author, Some(crate::overlay::PostAuthor::guest("dana")));
    assert_eq!(catalog(&conv).map(|c| c.3), Some(4));

    assert_eq!(move_group(&gid).expect("again"), 0, "a moved project is left alone");
    assert_eq!(backfill_step("", 1000).map(|_| ()), Ok(()));
    assert_eq!(crate::overlay::read_thread(&conv, 0).expect("thread").len(), 4, "nothing twice");

    let next = project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "five").expect("post");
    assert_eq!(next.seq, 5);
    assert_eq!(last_msg_seq(&gid), 5);
}

/// Bite (a post moves first): a line to an unmoved project moves it, then
/// stores after its old lines.
#[test]
fn a_post_to_an_unmoved_project_moves_it_first() {
    let (gid, ids) = legacy("first-post", &[("scout", "agent", "old", 10, 1, None)]);
    let m = project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "new").expect("post");
    assert_eq!(m.seq, 2);
    let page = project_groups::list_messages(&gid, None, None).expect("page");
    assert_eq!(page.messages.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), vec![ids[0].clone(), m.id.clone()]);
}

/// Bite (the seq check): old seqs with a hole (a hand-trimmed database)
/// move to 1..n; each read cursor moves to the new seq of the highest old
/// seq at or below it, and the next line still takes a seq past every old
/// one.
#[test]
fn a_seq_hole_remaps_cursors_and_keeps_the_head() {
    let (gid, _) = legacy("hole", &[("scout", "agent", "a", 1, 1, None), ("scout", "agent", "b", 2, 2, None), ("scout", "agent", "c", 5, 5, None)]);
    set_cursor("owner", &gid, 4);
    assert_eq!(unread(&owner(), &gid), 1, "only c");
    move_group(&gid).expect("move");
    let cursor = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT last_read_seq FROM read_cursors WHERE person = 'owner' AND kind = 'project' AND conv_id = ?1",
            params![gid],
            |r| r.get::<_, i64>(0),
        )
        .expect("cursor")
    };
    assert_eq!(cursor, 2, "old 4 → the new seq of old 2");
    assert_eq!(unread(&owner(), &gid), 1, "still only c");
    let next = project_groups::post_message_as(&gid, "scout", AUTHOR_KIND_AGENT, "d").expect("post");
    assert_eq!(next.seq, 6, "past every old seq");
}

/// Bite (rollback and upgrade again): rows the older release wrote after
/// the move (continuing `last_msg_seq`) are imported by the reconcile, at
/// the same seqs.
#[test]
fn reconcile_imports_what_an_older_release_wrote() {
    let gid = group("reconcile");
    let m1 = project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "one").expect("post");
    let old_id = uuid::Uuid::new_v4().to_string();
    {
        // The older release's post: last_msg_seq + 1, its own row.
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE project_groups SET last_msg_seq = last_msg_seq + 1 WHERE id = ?1", params![gid]).expect("seq");
        conn.execute(
            "INSERT INTO project_group_messages (id, group_id, author, body, created_at, seq, author_kind) VALUES (?1, ?2, 'scout', 'from the old one', 50, 2, 'agent')",
            params![old_id, gid],
        )
        .expect("old post");
    }
    assert!(reconcile_all().expect("reconcile") >= 1);
    let page = project_groups::list_messages(&gid, None, None).expect("page");
    assert_eq!(page.messages.iter().map(|m| (m.id.clone(), m.seq)).collect::<Vec<_>>(), vec![(m1.id.clone(), 1), (old_id, 2)]);
    let m3 = project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "three").expect("post");
    assert_eq!(m3.seq, 3);
    let db = crate::db::shared();
    let conn = db.lock();
    assert_eq!(reconcile_group_with(&conn, &gid).expect("again"), 0, "nothing left");
}

/// Bite (delete): a deleted line leaves the views (`removed` lists it on a
/// delta), its seq stays taken, and its rollback copy and key go.
#[test]
fn a_deleted_line_leaves_the_views_and_the_copy() {
    let gid = group("delete");
    let conv = kp::conversation_id(&gid);
    let a = project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "oops a secret").expect("a");
    let b = project_groups::post_message_as(&gid, "scout", AUTHOR_KIND_AGENT, "ack").expect("b");
    crate::overlay::delete_thread_message(&conv, &crate::overlay::MessageRef::Id(a.id.clone()), crate::overlay::PostAuthor::owner(), |_| Ok(()))
        .expect("delete");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(note_deleted(&conn, &conv, &a.id).expect("note"), Some(gid.clone()));
        assert_eq!(note_deleted(&conn, "some-thread", &a.id).expect("not a project"), None);
    }
    let page = project_groups::list_messages(&gid, None, None).expect("page");
    assert_eq!(page.messages.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), vec![b.id.clone()]);
    let d = list_delta(&gid, 0, None).expect("delta");
    assert_eq!(d.messages.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), vec![b.id.clone()]);
    assert_eq!(d.removed, vec![(1, a.id.clone())]);
    let d = list_delta(&gid, 2, None).expect("delta past it");
    assert!(d.messages.is_empty());
    assert!(d.removed.contains(&(1, a.id.clone())), "a change below the cursor: {:?}", d.removed);
    assert_eq!(copies(&gid).iter().map(|c| c.0.clone()).collect::<Vec<_>>(), vec![b.id.clone()]);
    assert_eq!(project_groups::list_messages_before_seq(&gid, 3, None).expect("before").messages.len(), 1);
    let c = project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "again").expect("c");
    assert_eq!(c.seq, 3, "the deleted line's seq stays taken");
}

/// Bite (send-once): a key is answered as a duplicate in its own project
/// (`mismatch` when the hash differs), refused in another chat; a key the
/// older release kept (old table only) still answers its moved line.
#[test]
fn send_once_keys_across_the_move() {
    let gid = group("keys");
    let other = group("keys-other");
    let ns = format!("p:{}", uuid::Uuid::new_v4());
    let k = |h: &'static str| project_groups::ClientKey { ns: &ns, client_msg_id: "cm-9", at_ms: 1, body_hash: h };
    let first = match project_groups::post_message_keyed(&gid, "owner", AUTHOR_KIND_PERSON, "hi", Some(k("h1"))).expect("post") {
        project_groups::PostOutcome::Posted(m) => m,
        o => panic!("{o:?}"),
    };
    match project_groups::post_message_keyed(&gid, "owner", AUTHOR_KIND_PERSON, "hi", Some(k("h1"))).expect("repeat") {
        project_groups::PostOutcome::Duplicate { msg, mismatch } => assert_eq!((msg.id, mismatch), (first.id.clone(), false)),
        o => panic!("{o:?}"),
    }
    match project_groups::post_message_keyed(&gid, "owner", AUTHOR_KIND_PERSON, "other", Some(k("h2"))).expect("differs") {
        project_groups::PostOutcome::Duplicate { mismatch, .. } => assert!(mismatch),
        o => panic!("{o:?}"),
    }
    assert!(matches!(
        project_groups::post_message_keyed(&other, "owner", AUTHOR_KIND_PERSON, "hi", Some(k("h1"))).expect("elsewhere"),
        project_groups::PostOutcome::ReusedElsewhere
    ));

    // A key only the old table holds (the older release kept it before the move).
    let (old, ids) = legacy("old-key", &[("owner", "person", "kept", 10, 1, Some("cm-old"))]);
    let ns_old = format!("p:{}", uuid::Uuid::new_v4());
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO project_group_client_ids (ns, client_msg_id, group_id, message_id, created_ms) VALUES (?1, 'cm-old', ?2, ?3, 1)",
            params![ns_old, old, ids[0]],
        )
        .expect("old key");
    }
    let key_old = project_groups::ClientKey { ns: &ns_old, client_msg_id: "cm-old", at_ms: 2, body_hash: "x" };
    match project_groups::post_message_keyed(&old, "owner", AUTHOR_KIND_PERSON, "kept", Some(key_old)).expect("retry") {
        project_groups::PostOutcome::Duplicate { msg, mismatch } => assert_eq!((msg.id, mismatch), (ids[0].clone(), false)),
        o => panic!("{o:?}"),
    }
}

/// Bite (lifetime): deleting the project removes its chat (catalog row and
/// stored lines); deleting a member workspace never touches it; the sweep
/// removes a chat whose project went without its purge.
#[test]
fn the_chat_lives_as_long_as_its_project() {
    let gid = group("life");
    let conv = kp::conversation_id(&gid);
    let ws = uuid::Uuid::new_v4().to_string();
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            params![ws, format!("pct{}", &ws[..6]), format!("/tmp/pct-{ws}")],
        )
        .expect("workspace");
    }
    project_groups::add_member(&gid, &ws).expect("member");
    project_groups::post_message_as(&gid, "owner", AUTHOR_KIND_PERSON, "hello").expect("post");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![ws]).expect("workspace delete");
    }
    assert!(catalog(&conv).is_some(), "a workspace delete leaves the project's chat");
    project_groups::delete_group(&gid).expect("delete");
    assert!(catalog(&conv).is_none(), "the trigger took the catalog row");
    assert!(crate::overlay::read_thread(&conv, 0).expect("read").is_empty(), "the lines went with the project");

    // A project removed without its purge (a crash between the two).
    let lost = group("lost");
    let lost_conv = kp::conversation_id(&lost);
    project_groups::post_message_as(&lost, "owner", AUTHOR_KIND_PERSON, "stranded").expect("post");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM project_groups WHERE id = ?1", params![lost]).expect("raw delete");
    }
    assert_eq!(crate::overlay::read_thread(&lost_conv, 0).expect("read").len(), 1);
    let r = sweep_orphans().expect("sweep");
    assert!(r.conversations.contains(&lost_conv), "{r:?}");
    assert!(crate::overlay::read_thread(&lost_conv, 0).expect("read").is_empty());
}

/// Bite (walkers): a project line is counted once, from its project,
/// never as a Thread too (other tests share this database, so the check
/// is on this project's own rows).
#[test]
fn walkers_count_a_project_line_once() {
    let gid = group("walk");
    set_cursor("owner", &gid, 0);
    project_groups::post_message_as(&gid, "scout", AUTHOR_KIND_AGENT, "news").expect("post");
    let db = crate::db::shared();
    let conn = db.lock();
    let s = read_state::summary(&conn, &owner(), read_state::now_secs()).expect("summary");
    assert_eq!(s.items.iter().filter(|i| i.id == gid).count(), 1);
    assert!(s.items.iter().any(|i| i.kind == KIND_PROJECT && i.id == gid && i.unread == 1));
    let conv = kp::conversation_id(&gid);
    assert!(!s.items.iter().any(|i| i.id == conv), "never as a Thread: {:?}", s.items);
}

/// Drill (P9): a 10k-line chat moves in one batch with every seq kept, and
/// the views page it as before.
#[test]
fn a_ten_thousand_line_chat_moves_with_its_seqs() {
    let gid = group("big");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute_batch("SAVEPOINT big_rows").expect("savepoint");
        for i in 1..=10_000i64 {
            conn.execute(
                "INSERT INTO project_group_messages (id, group_id, author, body, created_at, seq, author_kind) \
                 VALUES (?1, ?2, 'scout', ?3, ?4, ?4, 'agent')",
                params![format!("big-{gid}-{i}"), gid, format!("line {i}"), i],
            )
            .expect("row");
        }
        conn.execute("UPDATE project_groups SET messages_in_thread = 0, last_msg_seq = 10000 WHERE id = ?1", params![gid]).expect("mark");
        conn.execute_batch("RELEASE big_rows").expect("release");
    }
    let started = std::time::Instant::now();
    assert_eq!(move_group(&gid).expect("move"), 10_000);
    eprintln!("10k-line move took {:?}", started.elapsed());
    let conv = kp::conversation_id(&gid);
    assert_eq!(crate::overlay::store::thread_seq_of(&conv, &format!("big-{gid}-1")).expect("seq"), Some(1));
    assert_eq!(crate::overlay::store::thread_seq_of(&conv, &format!("big-{gid}-10000")).expect("seq"), Some(10_000));
    let page = project_groups::list_messages(&gid, None, None).expect("page");
    assert_eq!(page.messages.first().map(|m| m.seq), Some(9_981));
    assert!(page.truncated);
    let page = project_groups::list_messages(&gid, Some(9_995), None).expect("after");
    assert_eq!(page.messages.iter().map(|m| m.seq).collect::<Vec<_>>(), (9_996..=10_000).collect::<Vec<_>>());
    let page = project_groups::list_messages_before_seq(&gid, 51, Some(50)).expect("before");
    assert_eq!((page.messages.len(), page.truncated), (50, false));
}
